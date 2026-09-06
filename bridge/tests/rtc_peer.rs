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
use build_bridge::carrier::testing::{client_request, reporting, within_patience};
use build_bridge::carrier::FrameIntake;
use build_bridge::rtc::testing::{
    browser_peer, browser_peer_with, orphan_part, past_one_message, BrowserIce, BrowserPeer,
    RelaySignaling,
};
use build_bridge::rtc::WebrtcPeerFactory;
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
        .set_peer_factory(WebrtcPeerFactory::new(intake.clone()));
    (intake, reports, ledger)
}

/// The upgrade the spec's policy performs, over the session that just went
/// live: the browser offers over the relay, both sides trickle over it too, and
/// the two negotiated channels open.
async fn upgraded(session: &RelaySession) -> BrowserPeer {
    browser_peer(&session.session_id, &session.session_key, session).await
}

/// The spec's whole claim about the second carrier: one session, two wires,
/// the same answers over either, and the relay still carrying when the peer is
/// gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_session_answers_the_same_over_the_relay_and_over_the_peer() {
    let state_dir = tempfile::tempdir().expect("a state dir");
    let (intake, _reports) = peer_bridge(state_dir.path());
    let (session, _demux) = browser_session("sess-peer", intake).await;

    let mut peer = upgraded(&session).await;

    // Migration, as the SPA performs it: the app session greets over its new
    // wire, which is what moves this session's pushes onto the channel.
    let greeted = peer.app.call("session.hello", json!({})).await;
    assert_eq!(greeted["ok"], true, "{greeted}");
    assert_eq!(greeted["result"]["push_events"], true);

    let over_the_relay = session.call("project.list", json!({})).await;
    let over_the_peer = peer.app.call("project.list", json!({})).await;
    assert_eq!(over_the_relay["ok"], true, "{over_the_relay}");
    assert_eq!(
        over_the_peer["result"], over_the_relay["result"],
        "the same session answers the same over either carrier"
    );

    let over_the_terminal_channel = peer.term.call("project.list", json!({})).await;
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
/// the relay, carrying direct once the peer connects, back on the relay when
/// its channels close under a live relay carrier, ended when the client says
/// so. Read off the same ledger the daemon writes to stderr and reports.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_s_transport_trail_reads_minted_carrying_fell_back_ended() {
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
        vec!["minted", "carrying:direct", "fell_back"],
        "the last channel closing under a live relay carrier is one fallback"
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
        vec!["minted", "carrying:direct", "fell_back", "ended"]
    );
}
