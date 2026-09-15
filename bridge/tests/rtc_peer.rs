//! The peer carrier end to end: a second webrtc peer in this process plays the
//! browser, upgrades a session it minted over the relay, and works the device
//! over both wires.
//!
//! Nothing leaves the machine. The browser gathers on loopback and the device
//! is offered no ICE server, so only host candidates pair and no TURN and no
//! STUN server is ever reached.
//!
//! One part of rule 8 is not provable here: the bridge resolves a browser's
//! mDNS (`<uuid>.local`) host candidates, and the in-process browser offers
//! IP candidates like every non-browser peer does, so nothing in this file
//! ever hands it a name to resolve. A real Chrome or Safari is what verifies
//! that (stage 06's browser pass).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use build_bridge::app::AppState;
use build_bridge::carrier::testing::{client_request, reporting, within_patience};
use build_bridge::carrier::FrameIntake;
use build_bridge::rtc::testing::{
    browser_peer, browser_peer_with, orphan_part, past_one_message, BrowserIce, BrowserPeer,
    RelaySignaling,
};
use build_bridge::rtc::{IceMode, IcePolicy, WebrtcPeerFactory};
use build_bridge::transport::{self, Envelope};
use build_bridge::transport_ledger::RecordingLedger;
use common::{connected_device, device_identity, recv, request_message, session_init_message};
use serde_json::{json, Value};
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;

mod common;

/// The browser's end of one E2EE session over the relay: what it sends, and
/// the two things that come back — replies to its own requests, and the
/// device's pushes, which is where the bridge's trickled candidates arrive.
///
/// Every read is behind its own lock because the upgrade waits on a reply and
/// on a push at the same time, as a browser signaling over one socket does.
struct RelaySession {
    session_id: String,
    session_key: String,
    to_device: mpsc::Sender<Value>,
    replies: Mutex<mpsc::UnboundedReceiver<Value>>,
    pushes: Mutex<mpsc::UnboundedReceiver<Value>>,
    next_id: AtomicU64,
}

impl RelaySession {
    /// Send one request and wait for its own reply, skipping any reply that
    /// belongs to a request trickling alongside it.
    async fn call(&self, method: &str, params: Value) -> Value {
        let id = self.ask(method, params).await;
        let mut replies = self.replies.lock().await;
        loop {
            let reply = within_patience(replies.recv()).await;
            if reply["id"] == json!(id) {
                return reply;
            }
        }
    }

    /// Send one app verb over the relay and read the refusal rule 1 owes it:
    /// the rendezvous carries the negotiation and nothing else.
    async fn refused_app_call(&self, method: &str) -> Value {
        let refused = self.call(method, json!({})).await;
        assert_eq!(refused["ok"], false, "{refused}");
        assert_eq!(refused["error_code"], "unavailable", "{refused}");
        assert_eq!(refused["retryable"], false, "{refused}");
        assert_eq!(
            refused["details"]["reason"], "relay_is_not_a_data_plane",
            "{refused}"
        );
        refused
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

/// Signaling is pinned to the relay carrier (spec §Signaling): the browser's
/// offer and both sides' candidates ride this session, never the channels they
/// negotiate.
#[async_trait::async_trait]
impl RelaySignaling for RelaySession {
    async fn offer(&self, sdp: String, ice_servers: &[Value]) -> String {
        let answered = self
            .call(
                "rtc.offer",
                json!({ "sdp": sdp, "ice_servers": ice_servers }),
            )
            .await;
        assert_eq!(answered["ok"], true, "{answered}");
        answered["result"]["sdp"]
            .as_str()
            .expect("the device answers with an SDP")
            .to_string()
    }

    async fn trickle(&self, candidate: Value) {
        self.ask("rtc.ice", json!({ "candidate": candidate })).await;
    }

    async fn device_candidate(&self) -> Value {
        let pushed = within_patience(self.pushes.lock().await.recv()).await;
        assert_eq!(pushed["type"], "rtc.ice", "{pushed}");
        pushed["candidate"].clone()
    }
}

/// Mint a session over the relay and split what comes back: replies from
/// pushes, decrypted, as a browser's session module does.
async fn browser_session(
    session_id: &str,
    intake: Arc<FrameIntake>,
) -> (RelaySession, JoinHandle<()>) {
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
            replies: Mutex::new(replies),
            pushes: Mutex::new(pushes),
            next_id: AtomicU64::new(1),
        },
        demux,
    )
}

/// A bridge with the real peer transport: one intake, the app behind it, and
/// the factory that builds a peer connection per session. Every frame the app
/// is given is reported as `<frame_type>:<session_id>`, the synthetic `close`
/// of a session that ended among them.
fn peer_bridge(state_dir: &std::path::Path) -> (Arc<FrameIntake>, mpsc::UnboundedReceiver<String>) {
    let (intake, reports, _ledger) = ledgered_peer_bridge(state_dir);
    (intake, reports)
}

/// [`peer_bridge`], with the transport ledger the bridge writes handed back,
/// so a test can read the session's trail as the admin page would.
fn ledgered_peer_bridge(
    state_dir: &std::path::Path,
) -> (
    Arc<FrameIntake>,
    mpsc::UnboundedReceiver<String>,
    Arc<RecordingLedger>,
) {
    policy_peer_bridge(state_dir, IcePolicy::default())
}

/// [`ledgered_peer_bridge`], with the ICE policy every peer it builds gathers
/// under — what an operator sets `BRIDGE_ICE_*` to (spec rule 8).
fn policy_peer_bridge(
    state_dir: &std::path::Path,
    policy: IcePolicy,
) -> (
    Arc<FrameIntake>,
    mpsc::UnboundedReceiver<String>,
    Arc<RecordingLedger>,
) {
    let app = AppState::new_unrooted(
        state_dir.join("worktrees"),
        "main",
        false,
        state_dir.join("mcp.sock").to_string_lossy().into_owned(),
    )
    .shared();
    let (handler, reports) = reporting(AppState::handler(app.clone()));
    let ledger = RecordingLedger::new();
    let intake = FrameIntake::with_ledger(
        handler,
        transport::generate_transport_keypair(),
        ledger.clone(),
    );
    app.lock()
        .unwrap()
        .set_peer_factory(WebrtcPeerFactory::new(intake.clone(), policy));
    (intake, reports, ledger)
}

/// The upgrade the spec's policy performs, over the session that just went
/// live: the browser offers over the relay, both sides trickle over it too, and
/// the two negotiated channels open.
async fn upgraded(session: &RelaySession) -> BrowserPeer {
    browser_peer(&session.session_id, &session.session_key, session).await
}

/// One session's signaling with a copy kept of every candidate the device
/// trickled. The browser still gets each one — this only reads what went past,
/// which is the one place the wire shape of an `rtc.ice` push is visible.
struct Recorded<'a> {
    session: &'a RelaySession,
    candidates: Mutex<Vec<Value>>,
}

impl<'a> Recorded<'a> {
    fn over(session: &'a RelaySession) -> Self {
        Recorded {
            session,
            candidates: Mutex::new(Vec::new()),
        }
    }

    async fn trickled_by_the_device(&self) -> Vec<Value> {
        self.candidates.lock().await.clone()
    }
}

#[async_trait::async_trait]
impl RelaySignaling for Recorded<'_> {
    async fn offer(&self, sdp: String, ice_servers: &[Value]) -> String {
        self.session.offer(sdp, ice_servers).await
    }

    async fn trickle(&self, candidate: Value) {
        self.session.trickle(candidate).await
    }

    async fn device_candidate(&self) -> Value {
        let candidate = self.session.device_candidate().await;
        self.candidates.lock().await.push(candidate.clone());
        candidate
    }
}

/// The address family of one trickled candidate, read off the attribute the
/// way a peer does.
fn candidate_address(candidate: &Value) -> std::net::IpAddr {
    candidate["candidate"]
        .as_str()
        .expect("a candidate attribute")
        .split_whitespace()
        .nth(4)
        .expect("`candidate:<foundation> <component> <transport> <priority> <address>`")
        .parse()
        .expect("a candidate address")
}

/// Whether this machine has an IPv6 address a peer could send to — the
/// premise of the test below, and the same question `every_interface` asks
/// before it hands the crate the IPv6 wildcard.
fn machine_has_ipv6() -> bool {
    rtc::shared::ifaces::ifaces()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|interface| interface.addr)
        .any(|addr| match addr.ip() {
            std::net::IpAddr::V6(v6) => {
                !v6.is_loopback() && !v6.is_unspecified() && v6.segments()[0] & 0xffc0 != 0xfe80
            }
            std::net::IpAddr::V4(_) => false,
        })
}

/// Every candidate this device trickles names the section it belongs to, so a
/// browser will take it.
///
/// The crate stamps its own candidates `sdpMid: ""`, and `addIceCandidate`
/// rejects a non-null mid that matches no m-section outright — it does not fall
/// back to the `sdpMLineIndex` beside it. A browser therefore dropped every one
/// of this bridge's candidates and paired only on the peer-reflexive candidate
/// this agent's checks created at its end; on any path where those checks do
/// not arrive first (asymmetric NAT, TURN-only) there was no pair at all and
/// the device went blocked. The in-process peer here takes an empty mid
/// happily, which is why this is asserted on the wire shape rather than on
/// whether the session came up.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_trickled_candidate_names_the_section_it_belongs_to() {
    let state_dir = tempfile::tempdir().expect("a state dir");
    let (intake, _reports) = peer_bridge(state_dir.path());
    let (session, _demux) = browser_session("sess-mid", intake).await;

    let recorded = Recorded::over(&session);
    let mut peer = browser_peer(&session.session_id, &session.session_key, &recorded).await;
    assert_eq!(peer.app.call("session.hello", json!({})).await["ok"], true);

    let trickled = recorded.trickled_by_the_device().await;
    assert!(!trickled.is_empty(), "the device gathered something");
    for candidate in &trickled {
        assert_eq!(
            candidate["sdpMid"], "0",
            "the mid of the one BUNDLE section the answer gave: {candidate}"
        );
        assert_eq!(candidate["sdpMLineIndex"], 0, "{candidate}");
    }
}

/// Rule 8 says the bridge gathers over UDP4 **and UDP6**, and the crate honours
/// that only for a wildcard of each family: it expands one into the interface
/// addresses of its own family and skips every other. A bridge handed the IPv4
/// wildcard alone therefore has no IPv6 host candidate at all, and a browser
/// and a device whose only shared path is IPv6 — an IPv6-only LAN, a Tailnet —
/// never pair; under `direct-only` there is then no path at all.
///
/// Nothing to prove on a machine with no IPv6 address: the policy does not ask
/// for the wildcard there, because the crate would bind `[::]` verbatim and
/// name `::` in a candidate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_device_on_an_ipv6_machine_trickles_an_ipv6_host_candidate() {
    if !machine_has_ipv6() {
        return;
    }
    let state_dir = tempfile::tempdir().expect("a state dir");
    let (intake, _reports) = peer_bridge(state_dir.path());
    let (session, _demux) = browser_session("sess-ipv6", intake).await;

    let recorded = Recorded::over(&session);
    let mut peer = browser_peer(&session.session_id, &session.session_key, &recorded).await;
    assert_eq!(peer.app.call("session.hello", json!({})).await["ok"], true);

    let trickled = recorded.trickled_by_the_device().await;
    assert!(
        trickled
            .iter()
            .map(candidate_address)
            .any(|ip| ip.is_ipv6()),
        "the device gathered on both families: {trickled:?}"
    );
}

/// The spec's whole claim about the carriers: one session, two channels
/// answering alike, a rendezvous that refuses to be one of them (rule 1), and
/// a session that outlives the peer it negotiated.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_session_answers_over_both_channels_and_the_relay_refuses_app_traffic() {
    let state_dir = tempfile::tempdir().expect("a state dir");
    let (intake, _reports) = peer_bridge(state_dir.path());
    let (session, _demux) = browser_session("sess-peer", intake).await;

    let mut peer = upgraded(&session).await;

    // Migration, as the SPA performs it: the app session greets over its new
    // wire, which is what moves this session's pushes onto the channel.
    let greeted = peer.app.call("session.hello", json!({})).await;
    assert_eq!(greeted["ok"], true, "{greeted}");
    assert_eq!(greeted["result"]["push_events"], true);

    let over_the_peer = peer.app.call("project.list", json!({})).await;
    assert_eq!(over_the_peer["ok"], true, "{over_the_peer}");
    let over_the_terminal_channel = peer.term.call("project.list", json!({})).await;
    assert_eq!(
        over_the_terminal_channel["result"], over_the_peer["result"],
        "both channels carry the one session"
    );

    // The same verb over the rendezvous is refused, by the same live session:
    // the refusal comes back encrypted under its key, so this is the session
    // saying no, not a wire that lost it.
    session.refused_app_call("project.list").await;

    // The browser gives up on the peer: the relay carrier is still riding the
    // session, so the session did not end with it — and it still carries the
    // one thing it is for.
    let closed = session.call("rtc.close", json!({})).await;
    assert_eq!(closed["ok"], true, "{closed}");
    let signaling_again = session.call("rtc.close", json!({})).await;
    assert_eq!(
        signaling_again["error"],
        json!(format!(
            "no peer connection for session {}",
            session.session_id
        )),
        "the rendezvous still carries signaling to the app once the peer is gone: \
         only the app has nothing left to close"
    );
    session.refused_app_call("project.list").await;
}

/// A request and its reply, both past what one DataChannel message carries.
/// The reply names the method it refused, so one oversized method exercises
/// the chunker in both directions over a real channel.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_request_too_large_for_one_message_crosses_in_parts() {
    let state_dir = tempfile::tempdir().expect("a state dir");
    let (intake, _reports) = peer_bridge(state_dir.path());
    let (session, _demux) = browser_session("sess-chunked", intake).await;
    let mut peer = upgraded(&session).await;

    let oversized = past_one_message('m');
    let refused = peer.app.call(&oversized, json!({})).await;

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
    let (session, _demux) = browser_session("sess-gap", intake).await;
    let mut peer = upgraded(&session).await;

    peer.app.send_text(&orphan_part()).await;

    assert!(
        peer.app.closed().await,
        "the channel a reassembly was lost on is closed"
    );
    let after = session.call("rtc.close", json!({})).await;
    assert_eq!(
        after["ok"], true,
        "the session did not end with the channel it lost: {after}"
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
    let (session, _demux) = browser_session("sess-last", intake).await;
    let mut peer = upgraded(&session).await;

    let greeted = peer.app.call("session.hello", json!({})).await;
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

    peer.app.close().await;

    assert_eq!(
        within_patience(reports.recv()).await,
        format!("close:{}", session.session_id),
        "the last carrier takes the session with it"
    );
}

/// The ICE servers the api would mint, handed in by the environment as the
/// JSON array `POST /api/rtc/ice-servers` answers with. Absent means no TURN
/// key is at hand and the test has nothing to reach.
const ICE_SERVERS_ENV: &str = "BUILD_ICE_SERVERS_JSON";

/// The paid path, exercised for real: a browser that may only use relay
/// candidates reaches the device through Cloudflare TURN with credentials the
/// api minted, and the session answers over that channel as over any other.
///
/// This is the one claim a loopback test cannot make. It runs only when the
/// environment carries a minted list — CI has no TURN key and skips; an
/// operator with the key runs it before a rollout (`deploy/OPS.md`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_browser_that_can_only_relay_rides_cloudflare_turn() {
    let Ok(minted) = std::env::var(ICE_SERVERS_ENV) else {
        eprintln!("skipped: {ICE_SERVERS_ENV} is not set, so there is no TURN server to reach");
        return;
    };
    let servers: Vec<Value> = serde_json::from_str(&minted).expect("a minted ICE server array");
    assert!(
        servers.iter().any(|server| {
            server["urls"]
                .as_array()
                .map(|urls| {
                    urls.iter()
                        .any(|u| u.as_str().unwrap_or("").starts_with("turn"))
                })
                .unwrap_or(false)
                && server.get("credential").is_some()
        }),
        "the minted list carries no credentialed TURN server: {minted}"
    );

    let state_dir = tempfile::tempdir().expect("a state dir");
    let (intake, _reports) = peer_bridge(state_dir.path());
    let (session, _demux) = browser_session("sess-turn", intake).await;

    let mut peer = browser_peer_with(
        &session.session_id,
        &session.session_key,
        &session,
        BrowserIce {
            servers,
            relay_only: true,
        },
    )
    .await;

    assert_eq!(
        peer.negotiated_local_candidate_type().await,
        "relay",
        "a relay-only browser must have paired on a TURN allocation"
    );
    assert!(
        peer.negotiated_path_is_billed().await,
        "and the same pair, read the device's way, is the billed shape"
    );
    let over_turn = peer.app.call("project.list", json!({})).await;
    assert_eq!(over_turn["ok"], true, "{over_turn}");
}

/// The trail the telemetry spec promises for one ordinary session: minted over
/// the rendezvous, carrying direct once the peer connects, carrying nothing
/// when its channels close under a live relay carrier, ended when the client
/// says so. Read off the same ledger the daemon writes to stderr and reports.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_s_transport_trail_reads_minted_carrying_channels_lost_ended() {
    let state_dir = tempfile::tempdir().expect("a state dir");
    let (intake, _reports, ledger) = ledgered_peer_bridge(state_dir.path());
    let (session, _demux) = browser_session("sess-trail", intake).await;
    assert_eq!(ledger.trail_of("sess-trail"), vec!["minted"]);

    let mut peer = upgraded(&session).await;
    let greeted = peer.app.call("session.hello", json!({})).await;
    assert_eq!(greeted["ok"], true, "{greeted}");
    assert_eq!(
        ledger.trail_of("sess-trail"),
        vec!["minted", "carrying:direct"],
        "a loopback pair is direct, and it is written once the channels carry"
    );

    // Both channels go while the relay socket still carries the session.
    peer.app.close().await;
    peer.term.close().await;
    within_patience(async {
        loop {
            if ledger.trail_of("sess-trail").len() >= 3 {
                return Some(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert_eq!(
        ledger.trail_of("sess-trail"),
        vec!["minted", "carrying:direct", "channels_lost"],
        "the last channel closing under a live relay carrier is one channels_lost"
    );

    // The client closes the session outright over the relay.
    let closing = client_request(
        &session.session_key,
        &session.session_id,
        transport::CLOSE_FRAME_TYPE,
        json!({}),
    );
    session
        .to_device
        .send(json!({ "type": "e2ee_envelope", "session_id": session.session_id, "envelope": closing }))
        .await
        .expect("the relay carries the close");
    within_patience(async {
        loop {
            if ledger.trail_of("sess-trail").last().map(String::as_str) == Some("ended") {
                return Some(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert_eq!(
        ledger.trail_of("sess-trail"),
        vec!["minted", "carrying:direct", "channels_lost", "ended"]
    );
}

/// Rule 8's ordinary case, end to end: a browser and a bridge that can see
/// each other pair host candidate to host candidate, and the session's one
/// `carrying` says so at both ends — `Direct`, and neither end a relay.
///
/// The device's end is the claim that needs a peer connection to make: it
/// gathers on every non-loopback interface this machine has, and the pair the
/// agent nominated is one of those addresses rather than a server-reflexive
/// or relayed stand-in for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_browser_and_a_bridge_that_can_see_each_other_pair_host_to_host() {
    let state_dir = tempfile::tempdir().expect("a state dir");
    let (intake, _reports, ledger) = ledgered_peer_bridge(state_dir.path());
    let (session, _demux) = browser_session("sess-host", intake).await;

    let mut peer = upgraded(&session).await;
    let greeted = peer.app.call("session.hello", json!({})).await;
    assert_eq!(greeted["ok"], true, "{greeted}");

    assert_eq!(
        ledger.trail_of("sess-host"),
        vec!["minted", "carrying:direct"]
    );
    let detail = ledger
        .carrying_details_of("sess-host")
        .pop()
        .expect("the pair the peer carried on");
    assert!(
        detail.starts_with("host/"),
        "the device paired on its own host candidate: {detail}"
    );
    assert!(!detail.contains("relay"), "and on nobody's TURN: {detail}");
    assert_eq!(
        peer.negotiated_local_candidate_type().await,
        "host",
        "and the browser, reading the same pair from its end, on its own"
    );
}

/// `direct-only` never lets a TURN server the browser offered reach the peer
/// connection, and never pairs with a relay candidate the browser trickles.
///
/// The list is asserted on the policy itself — that is the one place the
/// decision is made, and a list is easier to read than an SDP — and then the
/// same policy carries a real session, so the filtered (here: emptied) list is
/// one a peer connection can still be built from.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn direct_only_offers_the_peer_no_turn_server_and_still_carries() {
    // A TURN url on an address that routes nowhere (TEST-NET-2): the browser
    // offers it and never reaches it, so what this test proves is the
    // bridge's filter rather than somebody's TURN server being up.
    let minted = vec![json!({
        "urls": ["turn:198.51.100.7:3478?transport=udp"],
        "username": "user-1",
        "credential": "minted-for-this-test",
    })];
    let direct_only = IcePolicy {
        mode: IceMode::DirectOnly,
        ..IcePolicy::default()
    };

    assert!(
        direct_only.allowed_ice_servers(&minted).is_empty(),
        "a list that was nothing but TURN is nothing under direct-only"
    );
    assert_eq!(
        IcePolicy::default().allowed_ice_servers(&minted),
        minted.clone(),
        "and the hosted default is unchanged by any of this"
    );
    assert!(!direct_only.allows_remote_candidate(&json!({
        "candidate": "candidate:1 1 udp 41885439 198.51.100.7 51234 typ relay raddr 0.0.0.0 rport 0",
        "sdpMid": "0",
    })));

    let state_dir = tempfile::tempdir().expect("a state dir");
    let (intake, _reports, ledger) = policy_peer_bridge(state_dir.path(), direct_only);
    let (session, _demux) = browser_session("sess-direct-only", intake).await;

    // The browser offers the TURN server it was minted, as it always does.
    let mut peer = browser_peer_with(
        &session.session_id,
        &session.session_key,
        &session,
        BrowserIce {
            servers: minted,
            relay_only: false,
        },
    )
    .await;
    let greeted = peer.app.call("session.hello", json!({})).await;
    assert_eq!(greeted["ok"], true, "{greeted}");
    assert_eq!(
        ledger.trail_of("sess-direct-only"),
        vec!["minted", "carrying:direct"],
        "a bridge with no server to gather from still has its own interfaces"
    );
}

/// One session's signaling with a relay candidate carried **inside** the offer,
/// the way a browser that had already gathered one sends it.
struct RelayInTheOffer<'a>(&'a RelaySession);

/// A TURN candidate on an address that routes nowhere (TEST-NET-3), so what is
/// under test is the bridge's filter rather than somebody's TURN server.
const RELAY_CANDIDATE_LINE: &str =
    "a=candidate:9 1 udp 41885439 203.0.113.7 51234 typ relay raddr 0.0.0.0 rport 0\r\n";

#[async_trait::async_trait]
impl RelaySignaling for RelayInTheOffer<'_> {
    async fn offer(&self, sdp: String, ice_servers: &[Value]) -> String {
        self.0
            .offer(format!("{sdp}{RELAY_CANDIDATE_LINE}"), ice_servers)
            .await
    }

    async fn trickle(&self, candidate: Value) {
        self.0.trickle(candidate).await
    }

    async fn device_candidate(&self) -> Value {
        self.0.device_candidate().await
    }
}

/// Trickling is not the only way a relay candidate arrives: an `a=candidate`
/// line inside the offer is extracted by `set_remote_description` itself and
/// added without the policy being asked, so `direct-only` takes those lines out
/// before the peer sees the offer.
///
/// That the line is gone is asserted on the policy, where the decision is made
/// and where it can be read. What needs a real peer connection is the other
/// half: an SDP this bridge rewrote is still an SDP, and the session it
/// negotiates still carries — a filter that broke the offer would refuse every
/// `direct-only` connection instead of every relay candidate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relay_candidate_inside_the_offer_does_not_stop_the_session() {
    let direct_only = IcePolicy {
        mode: IceMode::DirectOnly,
        ..IcePolicy::default()
    };
    assert!(
        !direct_only
            .allowed_offer(&format!("v=0\r\n{RELAY_CANDIDATE_LINE}"))
            .contains("typ relay"),
        "the line the offer carried is not one the peer is given"
    );

    let state_dir = tempfile::tempdir().expect("a state dir");
    let (intake, _reports, ledger) = policy_peer_bridge(state_dir.path(), direct_only);
    let (session, _demux) = browser_session("sess-inline-relay", intake).await;

    let spliced = RelayInTheOffer(&session);
    let mut peer = browser_peer(&session.session_id, &session.session_key, &spliced).await;
    assert_eq!(peer.app.call("session.hello", json!({})).await["ok"], true);

    assert_eq!(
        ledger.trail_of("sess-inline-relay"),
        vec!["minted", "carrying:direct"],
        "the rewritten offer parsed, and the pair is the direct one"
    );
}

/// An interface allow-list is applied, and a bridge it leaves with nothing to
/// bind fails closed: the offer is refused, naming the list, and the browser
/// blocks that device (rule 3) instead of waiting out a deadline on a peer
/// that could never have gathered a candidate.
///
/// The offer carries a placeholder SDP on purpose — the refusal happens where
/// the sockets are bound, before an offer is parsed at all, so what comes back
/// says the allow-list rather than "not an SDP".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_interface_allow_list_that_names_nothing_refuses_the_offer() {
    let state_dir = tempfile::tempdir().expect("a state dir");
    let (intake, _reports, _ledger) = policy_peer_bridge(
        state_dir.path(),
        IcePolicy {
            interfaces: Some(vec!["bridge-nope0".to_string()]),
            ..IcePolicy::default()
        },
    );
    let (session, _demux) = browser_session("sess-no-interface", intake).await;

    let refused = session
        .call("rtc.offer", json!({ "sdp": "v=0", "ice_servers": [] }))
        .await;

    assert_eq!(refused["ok"], false, "{refused}");
    let error = refused["error"].as_str().expect("a refusal says why");
    assert!(error.contains("bridge-nope0"), "{error}");
    assert!(error.contains("BRIDGE_ICE_INTERFACES"), "{error}");
}
