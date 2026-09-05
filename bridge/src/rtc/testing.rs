//! The browser side of the peer connection, as a test builds it.
//!
//! One home for the offerer the device answers: the same two negotiated
//! channels, the same chunking, the same reassembly, built from the crate's own
//! constants rather than from a second copy of them. What a test reads off a
//! [`BrowserChannel`] is what the SPA's carrier reads off a `RTCDataChannel`.
//! Compiled for tests only, never into the daemon.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use webrtc::data_channel::{DataChannel, DataChannelEvent};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
    RTCIceCandidateInit, RTCIceTransportPolicy, RTCPeerConnectionIceEvent, RTCSessionDescription,
    StatsSelector,
};

use super::{
    chunk, negotiated_channel, negotiated_path, offered_server, NegotiatedPath, NEGOTIATED_CHANNELS,
};
use crate::carrier::testing::{client_request, closed_within_patience, within_patience};
use crate::transport::{self, Envelope, DATA_FRAME_TYPE};

/// How the browser reaches the device before a peer path exists: the relay
/// session it already holds. The upgrade needs these three things off it and
/// nothing else about the relay.
#[async_trait]
pub trait RelaySignaling: Send + Sync {
    /// `rtc.offer` — the browser's SDP out, with the ICE servers the api
    /// minted for it, and the device's answer back.
    async fn offer(&self, sdp: String, ice_servers: &[Value]) -> String;

    /// `rtc.ice` — one of the browser's own gathered candidates.
    async fn trickle(&self, candidate: Value);

    /// The next candidate the device trickled back, off its push stream.
    async fn device_candidate(&self) -> Value;
}

/// Where the browser's candidates may come from.
///
/// The default is the loopback-only, no-server shape every ordinary test
/// wants: host candidates pair and nothing leaves the machine. A list of
/// servers gathers from every interface instead, as a browser does, and
/// `relay_only` is the browser's `iceTransportPolicy: "relay"` — the one way
/// to make a test PROVE it crossed a TURN server rather than merely offered
/// one.
#[derive(Default)]
pub struct BrowserIce {
    /// ICE servers as the api mints them, the array `rtc.offer` carries.
    pub servers: Vec<Value>,
    /// Gather and pair on relay candidates alone.
    pub relay_only: bool,
}

/// The browser's end of the upgrade, whole job in one call: build the peer
/// connection, create the two negotiated channels, offer over `signaling`,
/// trickle both ways, and settle once both channels carry.
///
/// It gathers on loopback and offers the device no ICE server, so only host
/// candidates pair and nothing leaves the machine.
pub async fn browser_peer(
    session_id: &str,
    session_key: &str,
    signaling: &dyn RelaySignaling,
) -> BrowserPeer {
    browser_peer_with(session_id, session_key, signaling, BrowserIce::default()).await
}

/// [`browser_peer`], gathering as `ice` says.
pub async fn browser_peer_with(
    session_id: &str,
    session_key: &str,
    signaling: &dyn RelaySignaling,
    ice: BrowserIce,
) -> BrowserPeer {
    let (gathered, mut candidates) = mpsc::unbounded_channel();
    let gather_from = if ice.servers.is_empty() {
        LOOPBACK_ONLY
    } else {
        EVERY_INTERFACE
    };
    let configuration = RTCConfigurationBuilder::new()
        .with_ice_servers(ice.servers.iter().map(offered_server).collect())
        .with_ice_transport_policy(if ice.relay_only {
            RTCIceTransportPolicy::Relay
        } else {
            RTCIceTransportPolicy::All
        })
        .build();
    let connection: Arc<dyn PeerConnection> = Arc::new(
        PeerConnectionBuilder::new()
            .with_configuration(configuration)
            .with_handler(Arc::new(BrowserEvents { gathered }))
            .with_udp_addrs(vec![gather_from.to_string()])
            .build()
            .await
            .expect("the browser opens a peer connection"),
    );
    let mut opened = Vec::new();
    for (label, id) in NEGOTIATED_CHANNELS {
        let channel = connection
            .create_data_channel(label, Some(negotiated_channel(id)))
            .await
            .expect("a negotiated channel needs no handshake");
        opened.push(BrowserChannel::watching(channel, session_id, session_key));
    }
    let mut opened = opened.into_iter();
    let mut peer = BrowserPeer {
        connection,
        app: opened.next().expect("the app channel is the first"),
        term: opened.next().expect("the terminal channel is the second"),
    };

    let offer = peer
        .connection
        .create_offer(None)
        .await
        .expect("the browser offers");
    peer.connection
        .set_local_description(offer.clone())
        .await
        .expect("the browser's own offer");
    let answer = signaling.offer(offer.sdp, &ice.servers).await;
    peer.connection
        .set_remote_description(
            RTCSessionDescription::answer(answer).expect("the device's answer parses"),
        )
        .await
        .expect("the device's answer");

    let mut open_channels = 0;
    while open_channels < 2 {
        tokio::select! {
            mine = candidates.recv() => signaling.trickle(mine.expect("the browser gathers")).await,
            theirs = signaling.device_candidate() => {
                let candidate: RTCIceCandidateInit = serde_json::from_value(theirs)
                    .expect("the device trickles candidates");
                peer.connection
                    .add_ice_candidate(candidate)
                    .await
                    .expect("the browser takes the device's candidate");
            }
            app = peer.app.events.recv() => open_channels += opened_one(app),
            term = peer.term.events.recv() => open_channels += opened_one(term),
        }
    }
    peer
}

/// A string past what one DataChannel message carries, so what crosses is
/// parts.
pub fn past_one_message(fill: char) -> String {
    fill.to_string().repeat(chunk::CHUNK_BYTES * 3)
}

/// The last part of a message whose earlier parts never arrived — a reassembly
/// the receiver cannot finish and cannot ask again for.
pub fn orphan_part() -> String {
    chunk::split(&past_one_message('o'))
        .pop()
        .expect("a message that large is parts")
}

/// Where the browser gathers from: loopback only, so a test pairs on host
/// candidates and reaches no STUN or TURN server.
const LOOPBACK_ONLY: &str = "127.0.0.1:0";

/// Where a browser given ICE servers gathers from: every interface, the way
/// the device does (`GATHER_FROM`), so a STUN or TURN server can be reached.
const EVERY_INTERFACE: &str = "0.0.0.0:0";

/// The browser's peer connection, with the same two negotiated channels the
/// device creates. Holding it is what keeps the connection open.
pub struct BrowserPeer {
    connection: Arc<dyn PeerConnection>,
    pub app: BrowserChannel,
    pub term: BrowserChannel,
}

impl BrowserPeer {
    /// Which kind of local candidate this browser's nominated pair won on —
    /// `host`, `srflx`, `prflx` or `relay` — read the same way the device reads
    /// its own, so a test asserts the path with the code that reports it.
    pub async fn negotiated_local_candidate_type(&self) -> String {
        self.negotiated_path().await.local().to_string()
    }

    /// Whether this browser's nominated pair moves bytes through a TURN
    /// server — what the device's log line says of the same pair from its end.
    pub async fn negotiated_path_is_billed(&self) -> bool {
        self.negotiated_path().await.billed()
    }

    async fn negotiated_path(&self) -> NegotiatedPath {
        let report = self
            .connection
            .get_stats(std::time::Instant::now(), StatsSelector::None)
            .await;
        negotiated_path(&report)
    }
}

/// One negotiated channel as the browser holds it: one session's frames,
/// chunked on the way out and reassembled on the way in, the way
/// `spa/src/core/carrier.js` does it.
pub struct BrowserChannel {
    channel: Arc<dyn DataChannel>,
    session_id: String,
    session_key: String,
    next_id: AtomicU64,
    events: mpsc::UnboundedReceiver<ChannelEvent>,
}

impl BrowserChannel {
    /// One request over this channel, answered over this channel.
    pub async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let envelope = client_request(
            &self.session_key,
            &self.session_id,
            DATA_FRAME_TYPE,
            json!({ "id": id, "method": method, "params": params }),
        );
        let json = serde_json::to_string(&envelope).expect("an envelope serializes");
        for message in chunk::split(&json) {
            self.send_text(&message).await;
        }
        loop {
            if let ChannelEvent::Envelope(json) = within_patience(self.events.recv()).await {
                let envelope: Envelope =
                    serde_json::from_str(&json).expect("the device sends envelopes");
                return transport::decrypt_envelope(&self.session_key, &envelope)
                    .expect("the session key opens what the channel carried")
                    .payload;
            }
        }
    }

    /// Put one message on the wire as it stands — what a test that is about the
    /// chunking itself sends.
    pub async fn send_text(&self, message: &str) {
        self.channel
            .send_text(message)
            .await
            .expect("the channel takes the message");
    }

    /// The browser lets this channel go — one carrier released, whatever else
    /// is still carrying the session.
    pub async fn close(&self) {
        self.channel
            .close()
            .await
            .expect("the browser closes the channel it was riding");
    }

    /// Whether the device took this channel down.
    pub async fn closed(&mut self) -> bool {
        closed_within_patience(self.events.recv()).await
    }

    fn watching(channel: Arc<dyn DataChannel>, session_id: &str, session_key: &str) -> Self {
        let (reported, events) = mpsc::unbounded_channel();
        let polled = channel.clone();
        tokio::spawn(async move {
            let mut reassembler = chunk::Reassembler::default();
            while let Some(event) = polled.poll().await {
                let reported_event = match event {
                    DataChannelEvent::OnOpen => ChannelEvent::Opened,
                    DataChannelEvent::OnMessage(message) => {
                        let text = std::str::from_utf8(&message.data).expect("text messages");
                        match reassembler
                            .accept(text)
                            .expect("the device chunks correctly")
                        {
                            Some(envelope) => ChannelEvent::Envelope(envelope),
                            None => continue,
                        }
                    }
                    DataChannelEvent::OnClose => break,
                    _ => continue,
                };
                if reported.send(reported_event).is_err() {
                    break;
                }
            }
        });
        BrowserChannel {
            channel,
            session_id: session_id.to_string(),
            session_key: session_key.to_string(),
            next_id: AtomicU64::new(1),
            events,
        }
    }
}

/// What one of the browser's channels reports.
enum ChannelEvent {
    Opened,
    Envelope(String),
}

fn opened_one(event: Option<ChannelEvent>) -> usize {
    match event.expect("a channel of a live peer connection reports") {
        ChannelEvent::Opened => 1,
        ChannelEvent::Envelope(_) => 0,
    }
}

struct BrowserEvents {
    gathered: mpsc::UnboundedSender<Value>,
}

#[async_trait]
impl PeerConnectionEventHandler for BrowserEvents {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        let candidate = event.candidate.to_json().expect("a gathered candidate");
        let _ = self.gathered.send(json!(candidate));
    }
}
