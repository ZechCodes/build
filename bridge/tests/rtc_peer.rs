//! The peer carrier end to end: a second webrtc peer in this process plays the
//! browser, upgrades a session it minted over the relay, and works the device
//! over both wires.
//!
//! Nothing leaves the machine. The browser gathers on loopback and the device
//! is offered no ICE server, so only host candidates pair and no TURN and no
//! STUN server is ever reached.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use build_bridge::app::AppState;
use build_bridge::carrier::testing::{client_request, next_report};
use build_bridge::carrier::FrameIntake;
use build_bridge::carrier::{FrameHandler, SessionSender};
use build_bridge::rtc::{chunk, WebrtcPeerFactory, NEGOTIATED_CHANNELS};
use build_bridge::transport::{self, Envelope, Frame, DATA_FRAME_TYPE};
use common::{connected_device, device_identity, recv, request_message, session_init_message};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use webrtc::data_channel::{DataChannel, DataChannelEvent, RTCDataChannelInit};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceCandidateInit,
    RTCPeerConnectionIceEvent, RTCSessionDescription,
};

mod common;

/// Nothing in the negotiation may take this long on one machine; past it the
/// test has hung rather than failed.
const PATIENCE: Duration = Duration::from_secs(20);

/// The browser's end of one E2EE session over the relay: what it sends, and
/// the two things that come back — replies to its own requests, and the
/// device's pushes, which is where the bridge's trickled candidates arrive.
struct RelaySession {
    session_id: String,
    session_key: String,
    to_device: mpsc::Sender<Value>,
    replies: mpsc::UnboundedReceiver<Value>,
    next_id: AtomicU64,
}

impl RelaySession {
    /// Send one request and wait for its own reply, skipping any reply that
    /// belongs to a request trickling alongside it.
    async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.ask(method, params).await;
        loop {
            let reply = tokio::time::timeout(PATIENCE, self.replies.recv())
                .await
                .expect("the device answered in time")
                .expect("the relay session is open");
            if reply["id"] == json!(id) {
                return reply;
            }
        }
    }

    /// Send one request and leave its reply to whoever reads next — what a
    /// trickled candidate is: it has no answer worth waiting for.
    async fn ask(&self, method: &str, params: Value) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.to_device
            .send(request_message(
                &self.session_key,
                &self.session_id,
                json!({ "id": id, "method": method, "params": params }),
            ))
            .await
            .expect("the relay carries the request");
        id
    }
}

/// Mint a session over the relay and split what comes back: replies from
/// pushes, decrypted, as a browser's session module does.
async fn browser_session(
    session_id: &str,
    intake: Arc<FrameIntake>,
) -> (RelaySession, mpsc::UnboundedReceiver<Value>, JoinHandle<()>) {
    let identity = device_identity();
    let device = connected_device(intake, &identity).await;
    let mut from_device = device.from_device;
    let session_key = transport::generate_session_key();
    device
        .to_device
        .send(session_init_message(
            session_id,
            &device.transport_public_key,
            &session_key,
        ))
        .await
        .expect("the relay carries the session_init");
    let accept = recv(&mut from_device).await;
    assert_eq!(accept["type"], "session_accept");

    let (replied, replies) = mpsc::unbounded_channel();
    let (pushed, pushes) = mpsc::unbounded_channel();
    let key = session_key.clone();
    let demux = tokio::spawn(async move {
        while let Some(message) = from_device.recv().await {
            let Ok(envelope) = serde_json::from_value::<Envelope>(message["envelope"].clone())
            else {
                continue;
            };
            let frame = transport::decrypt_envelope(&key, &envelope).expect("the browser's key");
            let bound = match frame.payload.get("id") {
                Some(_) => &replied,
                None => &pushed,
            };
            if bound.send(frame.payload).is_err() {
                break;
            }
        }
    });

    (
        RelaySession {
            session_id: session_id.to_string(),
            session_key,
            to_device: device.to_device,
            replies,
            next_id: AtomicU64::new(1),
        },
        pushes,
        demux,
    )
}

/// What one of the browser's channels reports. The device's channels are the
/// same two, so what a test reads here is what the SPA's carrier will.
enum ChannelEvent {
    Opened,
    Envelope(String),
}

/// One negotiated channel as the browser holds it: messages reassembled the
/// way `spa/src/core/chunk.js` will, over the same shape the device wrote.
struct BrowserChannel {
    channel: Arc<dyn DataChannel>,
    events: mpsc::UnboundedReceiver<ChannelEvent>,
}

impl BrowserChannel {
    fn watching(channel: Arc<dyn DataChannel>) -> Self {
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
        BrowserChannel { channel, events }
    }

    /// One request over this channel, answered over this channel.
    async fn call(&mut self, session: &RelaySession, method: &str, params: Value) -> Value {
        let envelope = client_request(
            &session.session_key,
            &session.session_id,
            DATA_FRAME_TYPE,
            json!({ "id": 1, "method": method, "params": params }),
        );
        let json = serde_json::to_string(&envelope).expect("an envelope serializes");
        for message in chunk::split(&json) {
            self.channel
                .send_text(&message)
                .await
                .expect("the channel takes the request");
        }
        loop {
            match tokio::time::timeout(PATIENCE, self.events.recv())
                .await
                .expect("the device answered over the channel")
                .expect("the channel is open")
            {
                ChannelEvent::Envelope(json) => {
                    let envelope: Envelope =
                        serde_json::from_str(&json).expect("the device sends envelopes");
                    return transport::decrypt_envelope(&session.session_key, &envelope)
                        .expect("the session key opens what the channel carried")
                        .payload;
                }
                ChannelEvent::Opened => continue,
            }
        }
    }
}

/// The browser's peer connection: the offerer, with the same two negotiated
/// channels the device creates.
struct BrowserPeer {
    connection: Arc<dyn PeerConnection>,
    app: BrowserChannel,
    term: BrowserChannel,
    candidates: mpsc::UnboundedReceiver<Value>,
}

struct BrowserEvents {
    gathered: mpsc::UnboundedSender<Value>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for BrowserEvents {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        let candidate = event.candidate.to_json().expect("a gathered candidate");
        let _ = self.gathered.send(json!(candidate));
    }
}

impl BrowserPeer {
    async fn offering() -> Self {
        let (gathered, candidates) = mpsc::unbounded_channel();
        let connection: Arc<dyn PeerConnection> = Arc::new(
            PeerConnectionBuilder::new()
                .with_handler(Arc::new(BrowserEvents { gathered }))
                .with_udp_addrs(vec!["127.0.0.1:0".to_string()])
                .build()
                .await
                .expect("the browser opens a peer connection"),
        );
        let mut opened = Vec::new();
        for (label, id) in NEGOTIATED_CHANNELS {
            opened.push(BrowserChannel::watching(
                negotiated(&connection, label, id).await,
            ));
        }
        let mut opened = opened.into_iter();
        BrowserPeer {
            connection,
            app: opened.next().expect("the app channel is the first"),
            term: opened.next().expect("the terminal channel is the second"),
            candidates,
        }
    }
}

async fn negotiated(
    connection: &Arc<dyn PeerConnection>,
    label: &str,
    id: u16,
) -> Arc<dyn DataChannel> {
    connection
        .create_data_channel(
            label,
            Some(RTCDataChannelInit {
                ordered: true,
                negotiated: Some(id),
                ..Default::default()
            }),
        )
        .await
        .expect("a negotiated channel needs no handshake")
}

/// A bridge with the real peer transport: one intake, the app behind it, and
/// the factory that builds a peer connection per session. Every frame the app
/// is given is reported as `<frame_type>:<session_id>`, the synthetic `close`
/// of a session that ended among them.
fn peer_bridge(state_dir: &std::path::Path) -> (Arc<FrameIntake>, mpsc::UnboundedReceiver<String>) {
    let peer_factory = WebrtcPeerFactory::new();
    let app = AppState::new_unrooted(
        state_dir.join("worktrees"),
        "main",
        false,
        state_dir.join("mcp.sock").to_string_lossy().into_owned(),
    )
    .with_peer_factory(peer_factory.clone())
    .shared();
    let (handler, reports) = reporting(AppState::handler(app));
    let intake = FrameIntake::new(handler, transport::generate_transport_keypair());
    peer_factory.carries(intake.clone());
    (intake, reports)
}

/// `handler`, with every frame it is given named the way
/// `carrier::testing::reporting_handler` names one — over a handler that still
/// answers, which the signaling this test upgrades over needs.
fn reporting(handler: FrameHandler) -> (FrameHandler, mpsc::UnboundedReceiver<String>) {
    let (reported, reports) = mpsc::unbounded_channel();
    let watched: FrameHandler = Arc::new(move |sender: SessionSender, frame: Frame| {
        let _ = reported.send(format!("{}:{}", frame.frame_type, frame.session_id));
        handler(sender, frame)
    });
    (watched, reports)
}

/// The upgrade the spec's policy performs: the browser offers over the relay,
/// both sides trickle over it too, and the two negotiated channels open.
async fn upgraded(
    session: &mut RelaySession,
    pushes: &mut mpsc::UnboundedReceiver<Value>,
) -> BrowserPeer {
    let mut peer = BrowserPeer::offering().await;
    let offer = peer
        .connection
        .create_offer(None)
        .await
        .expect("the browser offers");
    peer.connection
        .set_local_description(offer.clone())
        .await
        .expect("the browser's own offer");

    let answered = session
        .call(
            "rtc.offer",
            json!({ "sdp": offer.sdp, "ice_servers": no_reachable_ice_servers() }),
        )
        .await;
    assert_eq!(answered["ok"], true, "{answered}");
    let answer = answered["result"]["sdp"]
        .as_str()
        .expect("the device answers with an SDP")
        .to_string();
    peer.connection
        .set_remote_description(
            RTCSessionDescription::answer(answer).expect("the device's answer parses"),
        )
        .await
        .expect("the device's answer");

    let mut open_channels = 0;
    while open_channels < 2 {
        tokio::select! {
            mine = peer.candidates.recv() => {
                session
                    .ask("rtc.ice", json!({ "candidate": mine.expect("the browser gathers") }))
                    .await;
            }
            pushed = pushes.recv() => {
                let pushed = pushed.expect("the relay carries the device's pushes");
                assert_eq!(pushed["type"], "rtc.ice", "{pushed}");
                let candidate: RTCIceCandidateInit =
                    serde_json::from_value(pushed["candidate"].clone())
                        .expect("the device trickles candidates");
                peer.connection
                    .add_ice_candidate(candidate)
                    .await
                    .expect("the browser takes the device's candidate");
            }
            app = peer.app.events.recv() => {
                open_channels += opened(app);
            }
            term = peer.term.events.recv() => {
                open_channels += opened(term);
            }
        }
    }
    peer
}

fn opened(event: Option<ChannelEvent>) -> usize {
    match event.expect("a channel of a live peer connection reports") {
        ChannelEvent::Opened => 1,
        ChannelEvent::Envelope(_) => 0,
    }
}

/// The ICE servers a browser forwards when the api has no TURN key and no
/// reachable STUN server: an entry with nowhere to go. The device gathers its
/// host candidates and nothing in this test leaves the machine.
fn no_reachable_ice_servers() -> Value {
    json!([{ "urls": [] }])
}

/// The spec's whole claim about the second carrier: one session, two wires,
/// the same answers over either, and the relay still carrying when the peer is
/// gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_session_answers_the_same_over_the_relay_and_over_the_peer() {
    let state_dir = tempfile::tempdir().expect("a state dir");
    let (intake, _reports) = peer_bridge(state_dir.path());
    let (mut session, mut pushes, _demux) = browser_session("sess-peer", intake).await;

    let mut peer = upgraded(&mut session, &mut pushes).await;

    // Migration, as the SPA performs it: the app session greets over its new
    // wire, which is what moves this session's pushes onto the channel.
    let greeted = peer.app.call(&session, "session.hello", json!({})).await;
    assert_eq!(greeted["ok"], true, "{greeted}");
    assert_eq!(greeted["result"]["push_events"], true);

    let over_the_relay = session.call("project.list", json!({})).await;
    let over_the_peer = peer.app.call(&session, "project.list", json!({})).await;
    assert_eq!(over_the_relay["ok"], true, "{over_the_relay}");
    assert_eq!(
        over_the_peer["result"], over_the_relay["result"],
        "the same session answers the same over either carrier"
    );

    let over_the_terminal_channel = peer.term.call(&session, "project.list", json!({})).await;
    assert_eq!(
        over_the_terminal_channel["result"], over_the_relay["result"],
        "both channels carry the one session"
    );

    // The browser gives up on the peer: the relay carrier is still riding the
    // session, so the session did not end with it.
    let closed = session.call("rtc.close", json!({})).await;
    assert_eq!(closed["ok"], true, "{closed}");
    let after_the_peer = session.call("project.list", json!({})).await;
    assert_eq!(
        after_the_peer["result"], over_the_relay["result"],
        "the relay path still works once the peer is gone"
    );
}

/// A request and its reply, both past what one DataChannel message carries.
/// The reply names the method it refused, so one oversized method exercises
/// the chunker in both directions over a real channel.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_too_large_for_one_message_crosses_in_parts() {
    let state_dir = tempfile::tempdir().expect("a state dir");
    let (intake, _reports) = peer_bridge(state_dir.path());
    let (mut session, mut pushes, _demux) = browser_session("sess-chunked", intake).await;
    let mut peer = upgraded(&mut session, &mut pushes).await;

    let oversized = "m".repeat(chunk::CHUNK_BYTES * 3);
    let refused = peer.app.call(&session, &oversized, json!({})).await;

    assert_eq!(refused["ok"], false, "an unknown method is refused");
    assert_eq!(
        refused["error"],
        json!(format!("unknown method: {oversized}")),
        "the reply came back whole"
    );
}

/// A part that cannot be reassembled is fatal to the channel and to nothing
/// else: the parts carry no way to ask for the missing one again, so the
/// device closes the wire, and the session keeps working over the relay.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_part_of_a_message_that_never_started_closes_the_channel() {
    let state_dir = tempfile::tempdir().expect("a state dir");
    let (intake, _reports) = peer_bridge(state_dir.path());
    let (mut session, mut pushes, _demux) = browser_session("sess-gap", intake).await;
    let mut peer = upgraded(&mut session, &mut pushes).await;

    let orphan = chunk::split(&"o".repeat(chunk::CHUNK_BYTES * 2))
        .pop()
        .expect("a message that large is parts");
    peer.app
        .channel
        .send_text(&orphan)
        .await
        .expect("the channel takes it");

    assert!(
        tokio::time::timeout(PATIENCE, peer.app.events.recv())
            .await
            .expect("the device closed the channel in time")
            .is_none(),
        "the channel a reassembly was lost on is closed"
    );
    let after = session.call("project.list", json!({})).await;
    assert_eq!(
        after["ok"], true,
        "the session did not end with the channel it lost"
    );
}

/// Every frame reported up to now, forgotten: what this test asserts about is
/// what happens from here.
fn drain_reports(reports: &mut mpsc::UnboundedReceiver<String>) {
    while reports.try_recv().is_ok() {}
}

/// The other half of the teardown rule, the half the relay cannot show: a
/// session whose last carrier is a channel ends when that channel does. Nothing
/// above the wire is told which carrier went — the app hears the same synthetic
/// `close` it hears when a relay socket is lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_channel_that_carried_last_ends_the_session_with_it() {
    let state_dir = tempfile::tempdir().expect("a state dir");
    let (intake, mut reports) = peer_bridge(state_dir.path());
    let (mut session, mut pushes, _demux) = browser_session("sess-last", intake).await;
    let mut peer = upgraded(&mut session, &mut pushes).await;

    let greeted = peer.app.call(&session, "session.hello", json!({})).await;
    assert_eq!(greeted["ok"], true, "{greeted}");
    drain_reports(&mut reports);

    // The relay lets this session go while the channel is carrying it: one
    // carrier released, and the client is still working.
    session
        .to_device
        .send(json!({ "type": "session_closed", "session_id": session.session_id }))
        .await
        .expect("the relay carries the session_closed");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        reports.try_recv().is_err(),
        "the session did not end with the relay carrier the channel outlived"
    );

    peer.app
        .channel
        .close()
        .await
        .expect("the browser closes the channel it was riding");

    assert_eq!(
        next_report(&mut reports).await,
        format!("close:{}", session.session_id),
        "the last carrier takes the session with it"
    );
}
