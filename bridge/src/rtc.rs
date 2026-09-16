//! The peer connection, as everything above it sees one: one answerer per E2EE
//! session, handed the browser's offer and its candidates, trickling its own
//! back.
//!
//! Signaling rides the rendezvous carrier, and is the only thing that may
//! (strict P2P transport spec, rule 1): `rtc.offer`, `rtc.ice` and `rtc.close`
//! ride the carrier the client sent them on and the bridge's candidates go back
//! over that same carrier — never over the channels they negotiate, which do
//! not exist yet when they are needed. Which wire that carrier is, is not this
//! module's business: today it is the relay socket, and a future direct-network
//! rendezvous is the same path (`carrier.rs`, rule 7).
//!
//! Everything else the client asks for rides the channels this module
//! negotiates. A client whose channels never open has no data path at all —
//! there is no relay to stay on — so it fails closed and blocks that device.
//!
//! How a peer built here reaches a browser is one value, [`IcePolicy`]
//! (`policy.rs`, spec rule 8): mDNS resolution, the interfaces it gathers on,
//! how long a TURN pair waits before it may be accepted, and whether TURN is
//! allowed at all. It is resolved once at startup and read here; nothing in
//! this module decides any of it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use webrtc::data_channel::{DataChannel, DataChannelEvent, RTCDataChannelInit};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfiguration,
    RTCConfigurationBuilder, RTCIceCandidateInit, RTCIceConnectionState, RTCIceServer,
    RTCPeerConnectionIceEvent, RTCPeerConnectionState, RTCSessionDescription, RTCStatsReport,
    RTCStatsReportEntry, StatsSelector,
};

use rtc::ice::mdns::MulticastDnsMode;

use crate::carrier::{self, CarrierHandle, FrameIntake, OutboundEnvelope, SessionSender};
use crate::transport_ledger::{TransportEvent, TransportLedger, TransportPath};

/// Content-free lifecycle events share a wall clock with browser diagnostics.
/// Debug formatting escapes session ids so a wire value cannot forge log lines.
pub(crate) fn diagnostic(session_id: &str, event: &str) {
    let timestamp = time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000;
    eprintln!("rtc: timestamp_ms={timestamp} session={session_id:?} {event}");
}

pub(crate) mod chunk;
mod policy;

pub use policy::{IceMode, IcePolicy, ICE_INTERFACES_ENV, ICE_POLICY_ENV, ICE_RELAY_MIN_WAIT_ENV};

#[cfg(any(test, feature = "testing"))]
pub mod testing;

/// What a peer connection could not do. Fatal to the client's data path, not
/// to the session: a refused offer leaves the session alive on its rendezvous
/// with nothing carrying for it, which is the case rule 3 has the browser
/// block that device on.
#[derive(Debug, thiserror::Error)]
pub enum RtcError {
    #[error("no peer connection for session {0}")]
    NoPeer(String),
    #[error("this bridge has no peer transport")]
    Unavailable,
    #[error("the peer connection refused the offer: {0}")]
    Refused(String),
}

/// One live peer connection — one per E2EE session, always the answerer.
#[async_trait]
pub trait SessionPeer: Send + Sync {
    /// Answer the browser's offer.
    ///
    /// The ICE servers ride with every offer and nothing else carries them: the
    /// first call configures the peer, a later one reconfigures it and restarts
    /// ICE, which is how fresh TURN credentials arrive.
    ///
    /// `signaling` is the caller's own sender, passed with every offer and never
    /// owned from construction — it is the carrier the offer arrived on, so a
    /// candidate trickled back goes to the relay socket generation that is live
    /// now rather than the one that was live when the peer was built.
    async fn answer(
        &self,
        offer_sdp: &str,
        ice_servers: &[Value],
        signaling: SessionSender,
    ) -> Result<String, RtcError>;

    async fn add_remote_candidate(&self, candidate: Value) -> Result<(), RtcError>;

    /// Tear this peer connection down. Nothing to report: the session keeps
    /// working over the relay either way.
    async fn close(&self);
}

/// The only place a peer implementation is chosen and built.
pub trait SessionPeerFactory: Send + Sync {
    fn open(&self, session_id: &str) -> Result<Arc<dyn SessionPeer>, RtcError>;
}

/// Which peer connection each E2EE session has, and when it stops being its.
///
/// **Boundary** the whole lifecycle behind three verbs the signaling arms call
/// and one a session end calls: a session's first offer opens its peer, every
/// later offer reconfigures that same one, and a peer stops being the
/// session's exactly once. **Hides** the map and its lock, the factory, the
/// open-once race, and that a peer's work is async at all — a handler runs on
/// a blocking thread and gets its answer back before it replies.
pub struct SessionPeers {
    factory: Arc<dyn SessionPeerFactory>,
    peers: Mutex<HashMap<String, Arc<dyn SessionPeer>>>,
}

impl SessionPeers {
    pub fn with_factory(factory: Arc<dyn SessionPeerFactory>) -> Arc<Self> {
        Arc::new(SessionPeers {
            factory,
            peers: Mutex::new(HashMap::new()),
        })
    }

    /// Answer this session's offer, opening its peer if this is the first one.
    ///
    /// A first offer the peer cannot answer leaves the session with no peer, so
    /// the browser's retry builds a fresh one rather than reaching the
    /// half-open peer that just failed; a failed ICE restart keeps the peer
    /// that is already carrying.
    pub fn offer(
        &self,
        session_id: &str,
        offer_sdp: &str,
        ice_servers: &[Value],
        signaling: SessionSender,
    ) -> Result<String, RtcError> {
        let (peer, opened_by_this_offer) = self.riding_or_opened(session_id)?;
        match awaited(peer.answer(offer_sdp, ice_servers, signaling)) {
            Ok(answer) => Ok(answer),
            Err(refused) => {
                if opened_by_this_offer {
                    if let Some(unusable) = self.take(session_id) {
                        awaited(unusable.close());
                    }
                }
                Err(refused)
            }
        }
    }

    /// Trickle one of the browser's candidates to the peer this session is
    /// negotiating over. A candidate for a session that never offered is
    /// refused, not answered by opening a peer nobody negotiated.
    pub fn candidate(&self, session_id: &str, candidate: Value) -> Result<(), RtcError> {
        let peer = self
            .peers
            .lock()
            .unwrap()
            .get(session_id)
            .cloned()
            .ok_or_else(|| RtcError::NoPeer(session_id.to_string()))?;
        awaited(peer.add_remote_candidate(candidate))
    }

    /// The browser gave up on the peer carrier: tear this session's peer down
    /// and leave the session working over the relay.
    pub fn close(&self, session_id: &str) -> Result<(), RtcError> {
        diagnostic(session_id, "close_requested");
        let peer = self
            .take(session_id)
            .ok_or_else(|| RtcError::NoPeer(session_id.to_string()))?;
        awaited(peer.close());
        Ok(())
    }

    /// The session ended, so its peer does: an ICE negotiation belongs to the
    /// session that offered it. The teardown is spawned, because this runs
    /// where the app releases everything else the session held and nothing
    /// there waits on a socket.
    pub fn end_session(&self, session_id: &str) {
        if let Some(peer) = self.take(session_id) {
            diagnostic(session_id, "session_ended_closing_peer");
            tokio::spawn(async move { peer.close().await });
        }
    }

    /// This session's peer and whether this call is the one that opened it.
    ///
    /// Building a peer is foreign work — a real one allocates an ICE agent and
    /// a DTLS transport — so it runs with no lock held, and two offers racing
    /// on one session still leave one peer: the one that reached the map first,
    /// the loser closed rather than left negotiating.
    fn riding_or_opened(&self, session_id: &str) -> Result<(Arc<dyn SessionPeer>, bool), RtcError> {
        if let Some(peer) = self.peers.lock().unwrap().get(session_id) {
            return Ok((peer.clone(), false));
        }
        let opened = self.factory.open(session_id)?;
        let won_the_race = {
            let mut peers = self.peers.lock().unwrap();
            match peers.get(session_id) {
                Some(peer) => Some(peer.clone()),
                None => {
                    peers.insert(session_id.to_string(), opened.clone());
                    None
                }
            }
        };
        match won_the_race {
            Some(peer) => {
                awaited(opened.close());
                Ok((peer, false))
            }
            None => Ok((opened, true)),
        }
    }

    /// Take this session's peer out — the one place a peer stops being the
    /// session's, so an answer still in flight cannot put a closed one back.
    fn take(&self, session_id: &str) -> Option<Arc<dyn SessionPeer>> {
        self.peers.lock().unwrap().remove(session_id)
    }

    /// Test-only: how many sessions hold a peer.
    #[cfg(test)]
    pub fn count(&self) -> usize {
        self.peers.lock().unwrap().len()
    }
}

/// Finish one peer-connection call where a frame handler can wait for it.
/// Handlers run on the runtime's blocking pool (`carrier::dispatch`), so the
/// peer's async work is finished here rather than outliving the reply the
/// client is waiting for — the one place that happens.
fn awaited<T>(work: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Handle::current().block_on(work)
}

/// A bridge with no peer transport built in. Every offer is refused, and a
/// refused offer is the end of the road: the client has no data path to this
/// device (rule 3) and blocks it, rather than falling back to a relay that
/// carries no application traffic.
pub struct NoPeerFactory;

impl SessionPeerFactory for NoPeerFactory {
    fn open(&self, _session_id: &str) -> Result<Arc<dyn SessionPeer>, RtcError> {
        Err(RtcError::Unavailable)
    }
}

/// Send one of the bridge's own candidates to the browser — the one place the
/// shape of that push is written. False once the carrier it was given is gone.
pub fn trickle_candidate(signaling: &SessionSender, candidate: Value) -> bool {
    signaling.push(json!({ "type": "rtc.ice", "candidate": candidate }))
}

/// How many bytes a channel may hold undelivered before it stops taking more
/// (spec §Backpressure). The peer connection owns the waiting: a send past this
/// blocks until the peer acknowledges enough of what it already holds, and
/// fails the moment the channel is closing. The envelope queue behind it is
/// unbounded, so parking a writer trades the channel's send buffer for device
/// heap while a client that will not drain is attached.
const DC_BUFFERED_HIGH: usize = 1024 * 1024;

/// The two channels every peer carries, created identically on both sides with
/// explicit ids so no in-band open handshake is needed (spec §DataChannels).
/// They mirror today's two relay sockets: SCTP streams are independent, so a
/// terminal flood does not head-of-line block an RPC reply.
pub(crate) const NEGOTIATED_CHANNELS: [(&str, u16); 2] = [("app", 0), ("term", 1)];

/// How one of [`NEGOTIATED_CHANNELS`] is created, on whichever side is
/// creating it: ordered, and negotiated on the id both sides already agreed.
/// Both ends of a channel must ask for the same thing, so both ends ask here.
pub(crate) fn negotiated_channel(id: u16) -> RTCDataChannelInit {
    RTCDataChannelInit {
        ordered: true,
        negotiated: Some(id),
        ..Default::default()
    }
}

impl From<webrtc::error::Error> for RtcError {
    fn from(err: webrtc::error::Error) -> Self {
        RtcError::Refused(err.to_string())
    }
}

/// The peer transport itself: one `RTCPeerConnection` per E2EE session, built
/// on the offer that needs it.
///
/// **Hides** webrtc-rs, DTLS, ICE, the negotiated channels, chunking and
/// backpressure. Everything above it holds three async methods and an SDP
/// string.
pub struct WebrtcPeerFactory {
    intake: Arc<FrameIntake>,
    policy: Arc<IcePolicy>,
}

impl WebrtcPeerFactory {
    /// The one construction point of a real peer, holding the intake every
    /// channel it opens delivers through and the [`IcePolicy`] every one of
    /// them gathers under — resolved once at startup, so every session of a
    /// run reaches its browser the same way.
    pub fn new(intake: Arc<FrameIntake>, policy: IcePolicy) -> Arc<Self> {
        Arc::new(WebrtcPeerFactory {
            intake,
            policy: Arc::new(policy),
        })
    }
}

impl SessionPeerFactory for WebrtcPeerFactory {
    fn open(&self, session_id: &str) -> Result<Arc<dyn SessionPeer>, RtcError> {
        Ok(Arc::new(WebrtcPeer {
            session_id: session_id.to_string(),
            intake: self.intake.clone(),
            policy: self.policy.clone(),
            signaling: Arc::new(Trickling::default()),
            negotiation: tokio::sync::Mutex::new(None),
        }))
    }
}

struct WebrtcPeer {
    session_id: String,
    intake: Arc<FrameIntake>,
    policy: Arc<IcePolicy>,
    signaling: Arc<Trickling>,
    negotiation: tokio::sync::Mutex<Option<Negotiation>>,
}

/// One live peer connection, the channels carrying for it, and the one line it
/// logs about the path that won. Dropping it drops the carriers, which is each
/// channel released.
struct Negotiation {
    connection: Arc<dyn PeerConnection>,
    carriers: Vec<DataChannelCarrier>,
    path_report: tokio::task::JoinHandle<()>,
}

impl Drop for Negotiation {
    fn drop(&mut self) {
        self.carriers.clear();
        self.path_report.abort();
    }
}

#[async_trait]
impl SessionPeer for WebrtcPeer {
    async fn answer(
        &self,
        offer_sdp: &str,
        ice_servers: &[Value],
        signaling: SessionSender,
    ) -> Result<String, RtcError> {
        self.signaling.hold(signaling);
        let allowed = self.policy.allowed_ice_servers(ice_servers);
        let configuration = RTCConfigurationBuilder::new()
            .with_ice_servers(allowed.iter().map(offered_server).collect())
            .build();
        let mut negotiation = self.negotiation.lock().await;
        let connection = match negotiation.as_ref() {
            Some(open) => {
                open.connection.set_configuration(configuration).await?;
                open.connection.clone()
            }
            None => {
                let opened = self.connect(configuration).await?;
                let connection = opened.connection.clone();
                *negotiation = Some(opened);
                connection
            }
        };
        let offer =
            RTCSessionDescription::offer(self.policy.allowed_offer(offer_sdp).into_owned())?;
        connection.set_remote_description(offer).await?;
        let answer = connection.create_answer(None).await?;
        let sdp = answer.sdp.clone();
        connection.set_local_description(answer).await?;
        // Before the answer goes back, because the browser may trickle — and
        // this peer may gather — the moment it lands.
        self.signaling.answered(&sdp);
        Ok(sdp)
    }

    async fn add_remote_candidate(&self, candidate: Value) -> Result<(), RtcError> {
        if !self.policy.allows_remote_candidate(&candidate) {
            eprintln!(
                "rtc: session {} dropped a relay candidate ({ICE_POLICY_ENV}=direct-only)",
                self.session_id
            );
            return Ok(());
        }
        let trickled: RTCIceCandidateInit = serde_json::from_value(candidate)
            .map_err(|e| RtcError::Refused(format!("not an ICE candidate: {e}")))?;
        let negotiation = self.negotiation.lock().await;
        let open = negotiation
            .as_ref()
            .ok_or_else(|| RtcError::NoPeer(self.session_id.clone()))?;
        open.connection.add_ice_candidate(trickled).await?;
        Ok(())
    }

    async fn close(&self) {
        let Some(open) = self.negotiation.lock().await.take() else {
            return;
        };
        if let Err(e) = open.connection.close().await {
            eprintln!("rtc: session {} peer close: {e}", self.session_id);
        }
    }
}

impl WebrtcPeer {
    /// Build this session's peer connection: the two negotiated channels, the
    /// candidates it trickles back, and the one line it logs about the path
    /// that won.
    async fn connect(&self, configuration: RTCConfiguration) -> Result<Negotiation, RtcError> {
        let (connected, first_connect) = mpsc::unbounded_channel();
        let events: Arc<dyn PeerConnectionEventHandler> = Arc::new(PeerEvents {
            session_id: self.session_id.clone(),
            signaling: self.signaling.clone(),
            connected,
        });
        let udp_addrs = self.policy.gather_from()?;
        let connection: Arc<dyn PeerConnection> = Arc::new(
            built_or_without_mdns(&self.session_id, |multicast_dns| {
                let attempt = PeerConnectionBuilder::new()
                    .with_configuration(configuration.clone())
                    .with_setting_engine(self.policy.setting_engine(multicast_dns))
                    .with_data_channel_send_buffer_limit(DC_BUFFERED_HIGH)
                    .with_handler(events.clone())
                    .with_udp_addrs(udp_addrs.clone());
                async move { attempt.build().await }
            })
            .await?,
        );
        let path_report = tokio::spawn(report_negotiated_path(
            self.session_id.clone(),
            connection.clone(),
            first_connect,
            self.intake.ledger(),
        ));
        let mut carriers = Vec::new();
        for (label, id) in NEGOTIATED_CHANNELS {
            let channel = connection
                .create_data_channel(label, Some(negotiated_channel(id)))
                .await?;
            carriers.push(DataChannelCarrier::ride(
                channel,
                self.intake.clone(),
                self.session_id.clone(),
                label,
            ));
        }
        Ok(Negotiation {
            connection,
            carriers,
            path_report,
        })
    }
}

/// Build one peer connection under rule 8's ICE agent, and — if that fails —
/// once more without mDNS.
///
/// mDNS is not a knob the crate applies lazily: `MulticastDnsMode::QueryOnly`
/// makes it bind 224.0.0.251:5353 and join the group on every interface inside
/// `bind_transports`, and that error is returned from `build()`. On a host or
/// container where the join is refused — no multicast route, a locked-down
/// network namespace — every `rtc.offer` would be refused and every device
/// would go blocked (rule 3), where the same bridge connected over STUN/TURN
/// before rule 8 landed. Resolving a browser's `<uuid>.local` host candidates
/// is worth a great deal on a LAN and nothing at all on a host that cannot ask,
/// so this trades it away rather than the connection. Said once per run: on
/// such a host every session would say the same thing.
async fn built_or_without_mdns<T, E, Attempt>(
    session_id: &str,
    mut build: impl FnMut(MulticastDnsMode) -> Attempt,
) -> Result<T, E>
where
    E: std::fmt::Display,
    Attempt: std::future::Future<Output = Result<T, E>>,
{
    match build(MulticastDnsMode::QueryOnly).await {
        Ok(built) => Ok(built),
        Err(refused) => {
            static SAID: std::sync::Once = std::sync::Once::new();
            SAID.call_once(|| {
                eprintln!(
                    "rtc: session {session_id} could not build a peer connection with mDNS \
                     ({refused}); building without it, so a browser that offers only \
                     `<uuid>.local` candidates cannot be reached on a LAN"
                );
            });
            build(MulticastDnsMode::Disabled).await
        }
    }
}

/// What one of this peer's own candidates needs to reach the browser: the
/// carrier the latest offer arrived on, and the mid of the section it belongs
/// to. A peer that captured a carrier at construction would trickle into a
/// relay socket generation that has since been replaced, so only the newest of
/// each is kept.
#[derive(Default)]
struct Trickling {
    signaling: Mutex<Option<SessionSender>>,
    bundle_mid: Mutex<Option<String>>,
}

impl Trickling {
    fn hold(&self, signaling: SessionSender) {
        *self.signaling.lock().unwrap() = Some(signaling);
    }

    /// The answer this peer just sent, which is the document its candidates
    /// belong to and the only statement of what that section is called.
    fn answered(&self, answer_sdp: &str) {
        *self.bundle_mid.lock().unwrap() = bundle_mid_of(answer_sdp);
    }

    fn trickle(&self, candidate: Value) {
        let signaling = self.signaling.lock().unwrap().clone();
        let mid = self.bundle_mid.lock().unwrap().clone();
        if let Some(signaling) = signaling {
            trickle_candidate(&signaling, placed_in_bundle(candidate, mid.as_deref()));
        }
    }
}

/// One of this peer's own candidates, named so a browser will take it.
///
/// The crate stamps every candidate it gathers `sdpMid: ""` (`rtc`'s
/// `RTCIceCandidate::to_json`, a hard-coded default). An empty mid is a mid no
/// m-section has, and `addIceCandidate` rejects a non-null `sdpMid` that
/// matches no section rather than falling back to the `sdpMLineIndex` beside
/// it — so a spec-conformant browser drops every candidate this bridge
/// gathers, and pairs, if at all, only on the peer-reflexive candidate this
/// agent's own connectivity checks create at the far end. Where those checks do
/// not arrive first (asymmetric NAT, TURN-only) there is no pair at all.
///
/// A data-only session has one m-section and BUNDLE puts every candidate on it,
/// so naming it is the whole fix. A mid this peer does not know yet is written
/// `null`, never `""`: a null mid is what tells the browser to place the
/// candidate by the index instead.
fn placed_in_bundle(candidate: Value, bundle_mid: Option<&str>) -> Value {
    let Value::Object(mut candidate) = candidate else {
        return candidate;
    };
    candidate.insert(
        "sdpMid".to_string(),
        bundle_mid.map_or(Value::Null, Value::from),
    );
    candidate.insert("sdpMLineIndex".to_string(), Value::from(0));
    Value::Object(candidate)
}

/// The mid of the one BUNDLE m-section an SDP describes, as `a=mid:` states it.
fn bundle_mid_of(sdp: &str) -> Option<String> {
    sdp.lines()
        .filter_map(|line| line.trim().strip_prefix("a=mid:"))
        .map(str::to_string)
        .next()
}

/// What the peer connection tells this session about itself: its own gathered
/// candidates, which go back over the signaling carrier, and the moment it is
/// carrying.
struct PeerEvents {
    session_id: String,
    signaling: Arc<Trickling>,
    connected: mpsc::UnboundedSender<()>,
}

#[async_trait]
impl PeerConnectionEventHandler for PeerEvents {
    async fn on_ice_candidate(&self, event: RTCPeerConnectionIceEvent) {
        match event.candidate.to_json() {
            Ok(candidate) => self.signaling.trickle(json!(candidate)),
            Err(e) => eprintln!(
                "rtc: session {} dropped its own candidate: {e}",
                self.session_id
            ),
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        diagnostic(&self.session_id, &format!("connection_state={state}"));
        if state == RTCPeerConnectionState::Connected {
            let _ = self.connected.send(());
        }
    }

    async fn on_ice_connection_state_change(&self, state: RTCIceConnectionState) {
        diagnostic(&self.session_id, &format!("ice_state={state}"));
    }
}

/// Say, each time a session's peer starts carrying, which kind of path won at
/// each end: `host` and `srflx` are direct and free, `relay` at either end is
/// TURN egress somebody pays for, and the line says so. Each time, because an
/// ICE restart — a phone leaving Wi-Fi — re-negotiates the pair under the same
/// session, and the path it lands on is as billable as the first. That is
/// what makes "how often is TURN actually used" answerable (spec §Open
/// questions, closed; telemetry spec §Events).
///
/// The event goes to the transport ledger, whose stderr sink is the daemon's
/// one log stream, so a measurement is not split off from the errors around it.
async fn report_negotiated_path(
    session_id: String,
    connection: Arc<dyn PeerConnection>,
    mut connected: mpsc::UnboundedReceiver<()>,
    ledger: Arc<dyn TransportLedger>,
) {
    while connected.recv().await.is_some() {
        let report = connection
            .get_stats(std::time::Instant::now(), StatsSelector::None)
            .await;
        let path = negotiated_path(&report);
        ledger.record(
            &session_id,
            TransportEvent::Carrying {
                path: if path.billed() {
                    TransportPath::Turn
                } else {
                    TransportPath::Direct
                },
                detail: path.to_string(),
            },
        );
    }
}

/// The path a nominated pair won on, both ends named: the device's own
/// candidate type and the browser's.
///
/// Both, because TURN bills whichever end allocated the relay and the common
/// billed shape is the browser's — a device on a home box pairs its host
/// candidate with a browser's relay one. So the line carries the billing fact
/// itself (`billed`), and nothing above it has to know which side to look at.
pub(crate) struct NegotiatedPath {
    local: String,
    remote: String,
}

/// The candidate type ICE reports for a relayed (TURN) candidate.
const RELAY_CANDIDATE: &str = "relay";

/// What a report with no nominated pair reads as: not a type, and never billed.
const UNKNOWN_CANDIDATE: &str = "unknown";

/// The type of a candidate the ICE agent made for itself off a connectivity
/// check — the one kind of candidate that exists without ever being added, so
/// the one kind the stats report has no entry for.
const PEER_REFLEXIVE_CANDIDATE: &str = "prflx";

impl NegotiatedPath {
    pub(crate) fn new(local: &str, remote: &str) -> Self {
        NegotiatedPath {
            local: local.to_string(),
            remote: remote.to_string(),
        }
    }

    /// The two ends as the stats report resolved them for the nominated pair.
    ///
    /// An end the report has no entry for is a peer-reflexive candidate: the
    /// agent registers every candidate it is given, so the only candidate it
    /// can pair on without an entry is one it discovered itself from a
    /// connectivity check — which, with a browser, is the usual order of
    /// events, its check landing before its trickled candidate does. Both
    /// ends missing is no resolvable pair, and reads `unknown`.
    pub(crate) fn from_report_ends(local: Option<&str>, remote: Option<&str>) -> Self {
        match (local, remote) {
            (None, None) => NegotiatedPath::new(UNKNOWN_CANDIDATE, UNKNOWN_CANDIDATE),
            (local, remote) => NegotiatedPath::new(
                local.unwrap_or(PEER_REFLEXIVE_CANDIDATE),
                remote.unwrap_or(PEER_REFLEXIVE_CANDIDATE),
            ),
        }
    }

    /// The device's own candidate type: `host`, `srflx`, `prflx`, `relay`, or
    /// `unknown`.
    pub(crate) fn local(&self) -> &str {
        &self.local
    }

    /// Whether this pair moves bytes through a TURN server somebody pays for.
    pub(crate) fn billed(&self) -> bool {
        self.local == RELAY_CANDIDATE || self.remote == RELAY_CANDIDATE
    }
}

impl std::fmt::Display for NegotiatedPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{} candidates", self.local, self.remote)?;
        if self.billed() {
            write!(f, " (TURN, billed)")?;
        }
        Ok(())
    }
}

/// Which kinds of candidate the nominated pair won on, at both ends. The pair
/// names each candidate by the id ICE gave it and the report keys the
/// candidate under a prefixed form of that same id, so the entry is found by
/// the id it ends with rather than by a key built from a convention this
/// module does not own.
pub(crate) fn negotiated_path(report: &RTCStatsReport) -> NegotiatedPath {
    let Some(pair) = report.candidate_pairs().find(|pair| pair.nominated) else {
        return NegotiatedPath::new(UNKNOWN_CANDIDATE, UNKNOWN_CANDIDATE);
    };
    let (mut local, mut remote) = (None, None);
    for entry in report.iter() {
        match entry {
            RTCStatsReportEntry::LocalCandidate(candidate)
                if candidate.stats.id.ends_with(&pair.local_candidate_id) =>
            {
                local = Some(candidate.candidate_type.to_string());
            }
            RTCStatsReportEntry::RemoteCandidate(candidate)
                if candidate.stats.id.ends_with(&pair.remote_candidate_id) =>
            {
                remote = Some(candidate.candidate_type.to_string());
            }
            _ => {}
        }
    }
    NegotiatedPath::from_report_ends(local.as_deref(), remote.as_deref())
}

/// One ICE server as the browser fetched it from the api, as this crate takes
/// one. Tolerant on purpose: Cloudflare answers a credentialed entry and a
/// device with no TURN key configured answers a bare STUN url, and both are
/// the same array to everything above the peer.
pub(crate) fn offered_server(offered: &Value) -> RTCIceServer {
    RTCIceServer {
        urls: policy::offered_urls(offered),
        username: field_or_empty(offered, "username"),
        credential: field_or_empty(offered, "credential"),
    }
}

fn field_or_empty(offered: &Value, field: &str) -> String {
    offered
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One negotiated channel doing a carrier's whole job: a writer draining the
/// session envelopes bound for it, chunked and paced against the channel's
/// buffer, and a reader reassembling what arrives into the intake.
///
/// **Hides** chunking and backpressure. It registers no senders itself — a
/// browser that migrates re-sends `session.hello` and re-attaches its
/// terminals over the channel, and the frames it does that with are what bind
/// the session to this carrier.
struct DataChannelCarrier {
    writer: tokio::task::JoinHandle<()>,
    reader: tokio::task::JoinHandle<()>,
}

impl DataChannelCarrier {
    fn ride(
        channel: Arc<dyn DataChannel>,
        intake: Arc<FrameIntake>,
        session_id: String,
        label: &'static str,
    ) -> Self {
        let (carrier, envelopes) = CarrierHandle::open_channel();
        DataChannelCarrier {
            writer: tokio::spawn(write_envelopes(
                channel.clone(),
                envelopes,
                session_id.clone(),
                label,
            )),
            reader: tokio::spawn(pump_channel_events(
                channel, intake, carrier, session_id, label,
            )),
        }
    }
}

/// Dropping the carrier is the channel no longer carrying: both its tasks stop,
/// and the reader stopping is what releases the wire the intake's sessions ride.
impl Drop for DataChannelCarrier {
    fn drop(&mut self) {
        self.writer.abort();
        self.reader.abort();
    }
}

/// The channel's outbound half: every envelope for every session riding this
/// carrier, split into messages the channel can carry and handed over no
/// faster than it drains — the channel itself holds the writer at
/// [`DC_BUFFERED_HIGH`] and fails the send once it is closing.
async fn write_envelopes(
    channel: Arc<dyn DataChannel>,
    mut envelopes: mpsc::UnboundedReceiver<OutboundEnvelope>,
    session_id: String,
    label: &'static str,
) {
    while let Some(outbound) = envelopes.recv().await {
        let Some(json) = as_channel_text(&outbound) else {
            continue;
        };
        for message in chunk::split(&json) {
            if channel.send_text(&message).await.is_err() {
                diagnostic(&session_id, &format!("channel={label} write_failed"));
                return;
            }
        }
    }
}

/// What one outbound goes onto a channel as: a frame is the envelope itself —
/// the channel adds no wrapper — and an accept is the same
/// `{"type":"session_accept",…}` object the relay sends, so a session minted
/// over a channel (a future direct-network rendezvous) reads the one shape.
fn as_channel_text(outbound: &OutboundEnvelope) -> Option<String> {
    match outbound {
        OutboundEnvelope::Frame(envelope) => serde_json::to_string(envelope).ok(),
        OutboundEnvelope::SessionAccept {
            session_id,
            envelope,
        } => Some(carrier::session_accept_message(session_id, envelope).to_string()),
    }
}

/// The channel's inbound half: what the wire says happened, in the wire's own
/// vocabulary. A message goes to the carrier that rides this channel; a
/// reassembly it cannot finish closes the channel, because the parts carry no
/// way to ask for the missing one again.
async fn pump_channel_events(
    channel: Arc<dyn DataChannel>,
    intake: Arc<FrameIntake>,
    carrier: CarrierHandle,
    session_id: String,
    label: &'static str,
) {
    let riding = RidingChannel { intake, carrier };
    let mut reassembler = chunk::Reassembler::default();
    while let Some(event) = channel.poll().await {
        match event {
            DataChannelEvent::OnOpen => diagnostic(&session_id, &format!("channel={label} opened")),
            DataChannelEvent::OnMessage(message) => {
                if riding
                    .accept(&mut reassembler, &message.data)
                    .await
                    .is_err()
                {
                    diagnostic(&session_id, &format!("channel={label} reassembly_failed"));
                    let _ = channel.close().await;
                    break;
                }
            }
            DataChannelEvent::OnClose => {
                diagnostic(&session_id, &format!("channel={label} closed"));
                break;
            }
            _ => {}
        }
    }
    diagnostic(&session_id, &format!("channel={label} reader_ended"));
}

/// This channel as one of the wires the intake's sessions ride. Dropping it —
/// the channel closing, the peer being torn down, the task being aborted — is
/// one carrier released, and the teardown rule decides from there whether a
/// session ended with it.
struct RidingChannel {
    intake: Arc<FrameIntake>,
    carrier: CarrierHandle,
}

impl RidingChannel {
    /// One message off this channel, as far as it goes: the text of a part or
    /// of a whole envelope, then the reassembly it completes, then the intake,
    /// which owns what a frame means. A message that is not an envelope this
    /// carrier can honour is dropped by the frame module's rule; only a
    /// reassembly that cannot be finished comes back as an error, and that one
    /// is fatal to the channel.
    async fn accept(
        &self,
        reassembler: &mut chunk::Reassembler,
        message: &[u8],
    ) -> Result<(), chunk::ChunkError> {
        let Ok(text) = std::str::from_utf8(message) else {
            return Ok(());
        };
        let Some(envelope_json) = reassembler.accept(text)? else {
            return Ok(());
        };
        let Ok(envelope) = serde_json::from_str(&envelope_json) else {
            return Ok(());
        };
        if let Err(refused) = self.intake.accept(envelope, &self.carrier).await {
            carrier::drop_frame_error(&refused);
        }
        Ok(())
    }
}

impl Drop for RidingChannel {
    fn drop(&mut self) {
        self.intake.close_carrier(&self.carrier);
    }
}

#[cfg(test)]
pub mod recording {
    //! The peer the signaling stage ships against: it negotiates nothing and
    //! records everything it was given, so a test can say what reached the peer
    //! and what the peer was allowed to push back.

    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tokio::sync::{Notify, Semaphore};

    /// Everything one recording peer was handed.
    #[derive(Default)]
    struct PeerRecord {
        offers: Vec<(String, Vec<Value>)>,
        remote_candidates: Vec<Value>,
        closed: bool,
        /// The sender of the latest offer — the peer keeps only that one.
        signaling: Option<SessionSender>,
    }

    /// Holds every answer until a test lets it go, so "a close arrived while the
    /// offer was still being answered" is an order of events rather than a bet
    /// on wall-clock time.
    pub struct AnswerGate {
        /// One permit per answer that has reached the gate, so a test that asks
        /// after the fact still hears about it.
        answering: Semaphore,
        release: Notify,
    }

    impl AnswerGate {
        fn new() -> Arc<Self> {
            Arc::new(AnswerGate {
                answering: Semaphore::new(0),
                release: Notify::new(),
            })
        }

        pub async fn wait_until_answering(&self) {
            self.answering
                .acquire()
                .await
                .expect("a peer is answering")
                .forget();
        }

        pub fn release(&self) {
            self.release.notify_waiters();
        }

        async fn hold(&self) {
            let waiting = self.release.notified();
            self.answering.add_permits(1);
            waiting.await;
        }
    }

    pub struct RecordingPeer {
        record: Mutex<PeerRecord>,
        gate: Option<Arc<AnswerGate>>,
        refuses_offers: bool,
        closing: Notify,
    }

    impl RecordingPeer {
        pub fn offers(&self) -> Vec<(String, Vec<Value>)> {
            self.record.lock().unwrap().offers.clone()
        }

        pub fn remote_candidates(&self) -> Vec<Value> {
            self.record.lock().unwrap().remote_candidates.clone()
        }

        pub fn is_closed(&self) -> bool {
            self.record.lock().unwrap().closed
        }

        /// Wait for this peer to be torn down, however far away the teardown
        /// was spawned — an order of events rather than a wall-clock bet.
        pub async fn closed(&self) {
            while !self.is_closed() {
                self.closing.notified().await;
            }
        }

        /// Trickle one of the bridge's candidates back over the carrier the
        /// latest offer arrived on.
        pub fn trickle(&self, candidate: Value) -> bool {
            let signaling = self.record.lock().unwrap().signaling.clone();
            match signaling {
                Some(signaling) => trickle_candidate(&signaling, candidate),
                None => false,
            }
        }
    }

    #[async_trait]
    impl SessionPeer for RecordingPeer {
        async fn answer(
            &self,
            offer_sdp: &str,
            ice_servers: &[Value],
            signaling: SessionSender,
        ) -> Result<String, RtcError> {
            {
                let mut record = self.record.lock().unwrap();
                record
                    .offers
                    .push((offer_sdp.to_string(), ice_servers.to_vec()));
                record.signaling = Some(signaling);
            }
            if let Some(gate) = &self.gate {
                gate.hold().await;
            }
            if self.refuses_offers {
                return Err(RtcError::Refused(offer_sdp.to_string()));
            }
            Ok(format!("answer-to:{offer_sdp}"))
        }

        async fn add_remote_candidate(&self, candidate: Value) -> Result<(), RtcError> {
            self.record
                .lock()
                .unwrap()
                .remote_candidates
                .push(candidate);
            Ok(())
        }

        async fn close(&self) {
            self.record.lock().unwrap().closed = true;
            self.closing.notify_one();
        }
    }

    #[derive(Default)]
    pub struct RecordingPeerFactory {
        opened: Mutex<HashMap<String, Arc<RecordingPeer>>>,
        opens: AtomicUsize,
        gate: Mutex<Option<Arc<AnswerGate>>>,
        refuse_offers: AtomicBool,
    }

    impl RecordingPeerFactory {
        pub fn new() -> Arc<Self> {
            Arc::new(RecordingPeerFactory::default())
        }

        /// Make every answer from here on wait for the returned gate.
        pub fn hold_answers(&self) -> Arc<AnswerGate> {
            let gate = AnswerGate::new();
            *self.gate.lock().unwrap() = Some(gate.clone());
            gate
        }

        /// Make every peer opened from here on refuse the offers it is given —
        /// the malformed or unusable SDP a real peer connection reports.
        pub fn fail_answers(&self) {
            self.refuse_offers.store(true, Ordering::SeqCst);
        }

        pub fn peer_of(&self, session_id: &str) -> Option<Arc<RecordingPeer>> {
            self.opened.lock().unwrap().get(session_id).cloned()
        }

        /// How many peers were built, not how many sessions hold one: a second
        /// peer for one session is the regression these tests watch for.
        pub fn opened_count(&self) -> usize {
            self.opens.load(Ordering::SeqCst)
        }
    }

    impl SessionPeerFactory for RecordingPeerFactory {
        fn open(&self, session_id: &str) -> Result<Arc<dyn SessionPeer>, RtcError> {
            let peer = Arc::new(RecordingPeer {
                record: Mutex::new(PeerRecord::default()),
                gate: self.gate.lock().unwrap().clone(),
                refuses_offers: self.refuse_offers.load(Ordering::SeqCst),
                closing: Notify::new(),
            });
            self.opens.fetch_add(1, Ordering::SeqCst);
            self.opened
                .lock()
                .unwrap()
                .insert(session_id.to_string(), peer.clone());
            Ok(peer)
        }
    }
}

#[cfg(test)]
mod channel_writer_tests {
    use super::*;
    use crate::transport::Envelope;

    fn envelope(session_id: &str) -> Envelope {
        Envelope {
            version: 1,
            session_id: session_id.into(),
            route_to: format!("session:{session_id}"),
            nonce: "bm9uY2U=".into(),
            ciphertext: "Y2lwaGVy".into(),
        }
    }

    /// A channel is the wire that adds nothing: the envelope is the message.
    #[test]
    fn a_frame_goes_onto_the_channel_as_the_envelope_itself() {
        let outbound = OutboundEnvelope::new(envelope("s-1"));

        let text = as_channel_text(&outbound).expect("the channel carries it");

        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            serde_json::to_value(envelope("s-1")).unwrap()
        );
    }

    /// Rule 7's other half: an accept is representable on a channel, and it is
    /// the same object the relay sends — a direct-mode rendezvous reading this
    /// wire needs no second shape.
    #[test]
    fn an_accept_goes_onto_the_channel_as_the_same_session_accept_the_relay_sends() {
        let outbound = OutboundEnvelope::SessionAccept {
            session_id: "s-1".into(),
            envelope: envelope("s-1"),
        };

        let text = as_channel_text(&outbound).expect("the channel carries it");

        let wire: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(wire["type"], "session_accept");
        assert_eq!(wire["session_id"], "s-1");
        assert_eq!(
            wire["envelope"],
            serde_json::to_value(envelope("s-1")).unwrap()
        );
    }
}

#[cfg(test)]
mod peer_build_tests {
    use super::*;

    /// mDNS is rule 8's, and the crate makes it a hard dependency of the whole
    /// connection: the multicast join happens inside `bind_transports` and its
    /// error comes back out of `build()`. On a host that cannot join the group
    /// — no multicast route, a locked-down namespace — every offer would be
    /// refused and every device blocked, where the same bridge used to connect
    /// over STUN/TURN. Resolving a browser's `<uuid>.local` candidates is worth
    /// a great deal on a LAN and nothing at all on a host that cannot ask, so
    /// the build is tried once more without it.
    #[tokio::test]
    async fn a_peer_that_cannot_join_the_multicast_group_is_built_without_mdns() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let attempts = asked.clone();

        let built = built_or_without_mdns("s-mdns", |mode| {
            attempts.lock().unwrap().push(mode);
            async move {
                match mode {
                    MulticastDnsMode::QueryOnly => Err("the multicast join was refused"),
                    _ => Ok("a peer connection"),
                }
            }
        })
        .await;

        assert_eq!(built, Ok("a peer connection"));
        assert_eq!(
            *asked.lock().unwrap(),
            vec![MulticastDnsMode::QueryOnly, MulticastDnsMode::Disabled],
            "rule 8's agent first, and only then the one without it"
        );
    }

    /// The fallback is a fallback: a host that can join the group never gives
    /// up mDNS, and a build that fails for some other reason still fails.
    #[tokio::test]
    async fn an_ordinary_host_keeps_mdns_and_a_peer_that_cannot_build_still_refuses() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let attempts = asked.clone();
        let built = built_or_without_mdns("s-ok", |mode| {
            attempts.lock().unwrap().push(mode);
            async move { Ok::<&str, &str>("a peer connection") }
        })
        .await;
        assert_eq!(built, Ok("a peer connection"));
        assert_eq!(*asked.lock().unwrap(), vec![MulticastDnsMode::QueryOnly]);

        let refused = built_or_without_mdns("s-no", |_| async {
            Err::<&str, &str>("no udp_sockets or tcp_listeners available")
        })
        .await;
        assert_eq!(refused, Err("no udp_sockets or tcp_listeners available"));
    }
}

#[cfg(test)]
mod trickle_tests {
    use super::*;

    /// What the crate hands this module is `sdpMid: ""` — its own hard-coded
    /// default (`rtc`'s `RTCIceCandidate::to_json`) — and an empty mid is a mid
    /// no m-section has. `addIceCandidate` rejects a non-null `sdpMid` that
    /// matches no section, so a spec-conformant browser drops the candidate
    /// whole rather than falling back to the index beside it. The one BUNDLE
    /// section this session negotiated is what places it, read off the answer
    /// this peer sent.
    #[test]
    fn a_trickled_candidate_names_the_bundle_section_the_answer_gave_it() {
        let gathered = json!({
            "candidate": "candidate:1 1 udp 2130706431 192.168.1.9 48861 typ host",
            "sdpMid": "",
            "sdpMLineIndex": 0,
            "usernameFragment": Value::Null,
        });

        let placed = placed_in_bundle(gathered.clone(), Some("0"));

        assert_eq!(placed["sdpMid"], "0");
        assert_eq!(placed["sdpMLineIndex"], 0);
        assert_eq!(placed["candidate"], gathered["candidate"]);
        assert_eq!(
            placed_in_bundle(gathered.clone(), Some("data"))["sdpMid"],
            "data",
            "whatever the browser's offer named the section"
        );
        assert_eq!(
            placed_in_bundle(gathered, None)["sdpMid"],
            Value::Null,
            "a mid this peer does not know yet is null, never the empty string: \
             a null mid is what says `place it by the index`"
        );
    }

    /// The mid comes off the answer this peer sent, because that is the
    /// document the candidate belongs to. One m-section is all a data-only
    /// session has, and BUNDLE puts every candidate on it.
    #[test]
    fn the_bundle_mid_is_the_one_the_local_description_states() {
        let answer = "v=0\r\n\
                      o=- 1 1 IN IP4 0.0.0.0\r\n\
                      s=-\r\n\
                      t=0 0\r\n\
                      a=group:BUNDLE 0\r\n\
                      m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n\
                      a=mid:0\r\n\
                      a=sctp-port:5000\r\n";

        assert_eq!(bundle_mid_of(answer).as_deref(), Some("0"));
        assert_eq!(
            bundle_mid_of(&answer.replace("a=mid:0", "a=mid:data")).as_deref(),
            Some("data")
        );
        assert_eq!(
            bundle_mid_of("v=0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n"),
            None,
            "an SDP with no mid at all places nothing"
        );
    }

    #[test]
    fn a_bridge_candidate_reaches_the_client_as_an_rtc_ice_push() {
        let (signaling, mut pushes, key) = SessionSender::observable("s-trickle");

        assert!(trickle_candidate(
            &signaling,
            json!({ "candidate": "candidate:1 1 udp", "sdpMid": "0" })
        ));

        let pushed = SessionSender::decrypt_push(
            &key,
            &pushes.try_recv().expect("the candidate was pushed"),
        );
        assert_eq!(pushed["type"], "rtc.ice");
        assert_eq!(pushed["candidate"]["candidate"], "candidate:1 1 udp");
    }

    #[test]
    fn a_bridge_with_no_peer_transport_opens_nothing() {
        assert!(matches!(
            NoPeerFactory.open("s-1"),
            Err(RtcError::Unavailable)
        ));
    }
}

#[cfg(test)]
mod peer_transport_tests {
    use super::*;

    /// The api answers Cloudflare's array verbatim when a TURN key is
    /// configured and a bare STUN entry when none is; both reach the peer as
    /// the same list, so neither shape may be the one that parses.
    #[test]
    fn a_credentialed_server_and_a_bare_stun_one_are_both_taken() {
        let credentialed = offered_server(&json!({
            "urls": ["turn:turn.cloudflare.com:3478?transport=udp"],
            "username": "user-1",
            "credential": "secret-1",
        }));
        assert_eq!(
            credentialed.urls,
            vec!["turn:turn.cloudflare.com:3478?transport=udp".to_string()]
        );
        assert_eq!(credentialed.username, "user-1");

        let stun_only = offered_server(&json!({ "urls": ["stun:stun.cloudflare.com:3478"] }));
        assert_eq!(
            stun_only.urls,
            vec!["stun:stun.cloudflare.com:3478".to_string()]
        );
        assert!(stun_only.username.is_empty());
    }

    #[test]
    fn a_single_url_is_the_same_server_as_a_list_of_one() {
        let one = offered_server(&json!({ "urls": "stun:stun.cloudflare.com:3478" }));

        assert_eq!(one.urls, vec!["stun:stun.cloudflare.com:3478".to_string()]);
    }
}

#[cfg(test)]
mod negotiated_path_tests {
    use super::NegotiatedPath;

    /// TURN egress bills whichever end allocated the relay, and the common
    /// shape is the browser's: a device on a home box pairs its own host
    /// candidate with the browser's relay one. A log that named only the
    /// device's end would count exactly that session as free.
    #[test]
    fn a_pair_with_a_relay_on_either_end_is_billed() {
        assert!(NegotiatedPath::new("host", "relay").billed());
        assert!(NegotiatedPath::new("relay", "host").billed());
        assert!(NegotiatedPath::new("relay", "relay").billed());
        assert!(!NegotiatedPath::new("host", "srflx").billed());
        assert!(!NegotiatedPath::new("unknown", "unknown").billed());
    }

    /// A remote the report has no entry for is one the ICE agent made for
    /// itself off a connectivity check that arrived before the browser's
    /// trickled candidate did — a peer-reflexive candidate, the only kind an
    /// agent creates on its own. The line says so instead of `unknown`, which
    /// is reserved for a report with no nominated pair at all.
    #[test]
    fn a_remote_the_report_never_registered_is_peer_reflexive() {
        let path = NegotiatedPath::from_report_ends(Some("host"), None);
        assert_eq!(path.to_string(), "host/prflx candidates");
        assert!(!path.billed());
        assert_eq!(
            NegotiatedPath::from_report_ends(None, None).to_string(),
            "unknown/unknown candidates",
            "no nominated pair is still unknown at both ends"
        );
    }

    /// The line states the billing fact itself, so the ops count greps for a
    /// phrase the bridge asserts rather than a rule the reader applies to one
    /// side of the pair.
    #[test]
    fn the_log_line_names_both_ends_and_states_the_bill() {
        assert_eq!(
            NegotiatedPath::new("host", "relay").to_string(),
            "host/relay candidates (TURN, billed)"
        );
        assert_eq!(
            NegotiatedPath::new("host", "srflx").to_string(),
            "host/srflx candidates"
        );
        assert_eq!(
            NegotiatedPath::new("unknown", "unknown").to_string(),
            "unknown/unknown candidates"
        );
    }
}
