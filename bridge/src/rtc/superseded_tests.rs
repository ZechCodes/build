//! A reloaded client's earlier session, whose ICE has failed, is torn down as
//! soon as a newer session presents the same hint (#373). The hint is a bearer
//! value, not an identity, so it never closes a peer that still works.

use super::recording::{RecordingPeer, RecordingPeerFactory};
use super::*;
use crate::carrier::SessionSender;

fn peers() -> (Arc<SessionPeers>, Arc<RecordingPeerFactory>) {
    let factory = RecordingPeerFactory::new();
    (SessionPeers::with_factory(factory.clone()), factory)
}

fn session(id: &str) -> SessionSender {
    SessionSender::detached(id)
}

/// Let queued closes run. Tests that need it run on one thread, where every
/// task spawned before this yield finishes first, so a close that should not
/// happen has had its chance.
async fn settled() {
    tokio::task::yield_now().await;
}

/// Offer on `sender` from a blocking thread, the way a frame handler does.
async fn offered(
    peers: &Arc<SessionPeers>,
    factory: &RecordingPeerFactory,
    sender: &SessionSender,
    hint: Option<uuid::Uuid>,
) -> Arc<RecordingPeer> {
    let (peers, sender) = (Arc::clone(peers), sender.clone());
    let session_id = sender.session_id().to_string();
    tokio::task::spawn_blocking(move || peers.offer_with_client("v=0", &[], sender, hint))
        .await
        .unwrap()
        .unwrap();
    factory.peer_of(&session_id).unwrap()
}

#[tokio::test]
async fn a_newer_session_of_the_same_client_closes_its_failed_predecessor() {
    let (peers, factory) = peers();
    let client = uuid::Uuid::new_v4();
    let old = offered(&peers, &factory, &session("old"), Some(client)).await;
    old.fail_ice();
    let new = offered(&peers, &factory, &session("new"), Some(client)).await;
    settled().await;
    assert!(old.is_closed());
    assert_eq!(peers.count(), 1, "only the newer session keeps a peer");
    assert!(!new.is_closed());
}

#[tokio::test]
async fn a_predecessor_whose_ice_fails_after_the_newer_session_opened_is_closed() {
    let (peers, factory) = peers();
    let client = uuid::Uuid::new_v4();
    let old = offered(&peers, &factory, &session("old"), Some(client)).await;
    let new = offered(&peers, &factory, &session("new"), Some(client)).await;
    assert!(!old.is_closed(), "a predecessor that still carries is kept");
    old.fail_ice();
    settled().await;
    assert!(old.is_closed());
    assert_eq!(peers.count(), 1);
    assert!(!new.is_closed());
}

#[tokio::test]
async fn the_hint_alone_never_closes_a_session_whose_ice_has_not_failed() {
    let (peers, factory) = peers();
    let client = uuid::Uuid::new_v4();
    let old = offered(&peers, &factory, &session("old"), Some(client)).await;
    old.fail_ice();
    old.recover_ice();
    let new = offered(&peers, &factory, &session("new"), Some(client)).await;
    new.fail_ice();
    new.recover_ice();
    settled().await;
    assert!(
        !old.is_closed(),
        "an ICE restart that recovered is connected"
    );
    assert!(!new.is_closed());
    assert_eq!(peers.count(), 2);
}

#[tokio::test]
async fn a_failed_session_is_never_closed_for_an_older_one() {
    let (peers, factory) = peers();
    let client = uuid::Uuid::new_v4();
    let old = offered(&peers, &factory, &session("old"), Some(client)).await;
    let new = offered(&peers, &factory, &session("new"), Some(client)).await;
    new.fail_ice();
    let newest = offered(&peers, &factory, &session("newest"), None).await;
    settled().await;
    assert!(
        !new.is_closed(),
        "the newest failed session may still restart"
    );
    assert!(!old.is_closed());
    assert!(!newest.is_closed());
    assert_eq!(peers.count(), 3);
}

#[tokio::test]
async fn sessions_without_a_shared_hint_keep_todays_behaviour() {
    let (peers, factory) = peers();
    let unhinted = offered(&peers, &factory, &session("unhinted"), None).await;
    let other = offered(
        &peers,
        &factory,
        &session("other"),
        Some(uuid::Uuid::new_v4()),
    )
    .await;
    unhinted.fail_ice();
    other.fail_ice();
    for (id, hint) in [("next", None), ("third", Some(uuid::Uuid::new_v4()))] {
        offered(&peers, &factory, &session(id), hint).await;
    }
    settled().await;
    assert!(!unhinted.is_closed(), "no hint, nothing to correlate");
    assert!(
        !other.is_closed(),
        "another client's hint is not this one's"
    );
    assert_eq!(peers.count(), 4);
}

/// The SPA binds its hint once the greeting advertises the capability, which
/// may be on the newer session's second offer.
#[tokio::test]
async fn a_hint_bound_on_a_later_offer_still_supersedes() {
    let (peers, factory) = peers();
    let client = uuid::Uuid::new_v4();
    let old = offered(&peers, &factory, &session("old"), Some(client)).await;
    old.fail_ice();
    let new_sender = session("new");
    offered(&peers, &factory, &new_sender, None).await;
    assert!(!old.is_closed());
    offered(&peers, &factory, &new_sender, Some(client)).await;
    settled().await;
    assert!(old.is_closed());
    assert_eq!(peers.count(), 1);
}

/// A closed predecessor leaves its session open on the bridge, so a later
/// ICE restart from it gets a fresh peer like any first offer would.
#[tokio::test]
async fn a_superseded_session_that_offers_again_negotiates_a_fresh_peer() {
    let (peers, factory) = peers();
    let client = uuid::Uuid::new_v4();
    let old_sender = session("old");
    let old = offered(&peers, &factory, &old_sender, Some(client)).await;
    old.fail_ice();
    offered(&peers, &factory, &session("new"), Some(client)).await;
    settled().await;
    assert!(old.is_closed());
    assert_eq!(peers.count(), 1);
    let again = offered(&peers, &factory, &old_sender, Some(client)).await;
    assert!(!Arc::ptr_eq(&old, &again));
    assert!(!again.is_closed());
    assert_eq!(peers.count(), 2);
}

/// The real peer learns failure from its agent's ICE state events, and a
/// restart's checking state is no longer failed.
#[tokio::test]
async fn a_real_peer_reports_ice_failure_from_its_state_events() {
    let fired = Arc::new(AtomicU64::new(0));
    let ice = Arc::new(IceHealth::default());
    let counted = fired.clone();
    let _ = ice.on_failed.set(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
    }));
    let (connected, _) = mpsc::unbounded_channel();
    let (sweeps, _) = mpsc::channel(1);
    let events = PeerEvents {
        session_id: "ice-health".into(),
        signaling: Arc::new(Trickling::default()),
        connected,
        gathered: Mutex::new(GatheredTypes::default()),
        sweeps,
        ice: ice.clone(),
    };
    for (state, failed, hooks) in [
        (RTCIceConnectionState::Connected, false, 0),
        (RTCIceConnectionState::Disconnected, false, 0),
        (RTCIceConnectionState::Failed, true, 1),
        (RTCIceConnectionState::Checking, false, 1),
        (RTCIceConnectionState::Failed, true, 2),
    ] {
        events.on_ice_connection_state_change(state).await;
        assert_eq!(ice.reported_failed(), failed, "{state}");
        assert_eq!(fired.load(Ordering::SeqCst), hooks, "{state}");
    }
}

/// Review #455: a close decided while ICE had failed must not run once ICE
/// has recovered. On one thread the queued close cannot run before recovery.
#[tokio::test]
async fn a_peer_that_recovers_before_its_queued_close_runs_is_kept() {
    let (peers, factory) = peers();
    let client = uuid::Uuid::new_v4();
    let old = offered(&peers, &factory, &session("old"), Some(client)).await;
    offered(&peers, &factory, &session("new"), Some(client)).await;
    old.fail_ice();
    old.recover_ice();
    settled().await;
    assert_eq!(old.close_attempts(), 1, "the queued close ran");
    assert!(
        !old.is_closed(),
        "the queued close tore down a recovered peer"
    );
    assert_eq!(peers.count(), 2);
}

/// Review #455: the older session's own restart supplies the hint first. Its
/// offer must not retire the peer it is answering through.
#[tokio::test]
async fn an_older_sessions_first_hinted_offer_answers_through_a_registered_peer() {
    let (peers, factory) = peers();
    let client = uuid::Uuid::new_v4();
    let old_sender = session("old");
    let old = offered(&peers, &factory, &old_sender, None).await;
    old.fail_ice();
    offered(&peers, &factory, &session("new"), Some(client)).await;
    assert!(!old.is_closed());
    offered(&peers, &factory, &old_sender, Some(client)).await;
    settled().await;
    let candidate = tokio::task::spawn_blocking(move || {
        peers.candidate(
            &old_sender,
            serde_json::json!({ "candidate": "after-answer" }),
        )
    })
    .await
    .unwrap();
    assert!(candidate.is_ok(), "{candidate:?}");
    assert_eq!(old.remote_candidates().len(), 1);
}

/// An offer already riding a peer when that peer is closed as superseded
/// answers through a fresh registered peer instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_offer_whose_peer_is_closed_mid_answer_answers_through_a_fresh_peer() {
    let (peers, factory) = peers();
    let client = uuid::Uuid::new_v4();
    let gate = factory.hold_answers();
    let old_sender = session("old");
    let first = {
        let (peers, old_sender) = (peers.clone(), old_sender.clone());
        tokio::task::spawn_blocking(move || {
            peers.offer_with_client("v=0", &[], old_sender, Some(client))
        })
    };
    gate.wait_until_answering().await;
    gate.release();
    first.await.unwrap().unwrap();
    let old = factory.peer_of("old").unwrap();
    old.fail_ice();

    let restart = {
        let (peers, old_sender) = (peers.clone(), old_sender.clone());
        tokio::task::spawn_blocking(move || {
            peers.offer_with_client("v=1", &[], old_sender, Some(client))
        })
    };
    gate.wait_until_answering().await;
    let newer = {
        let peers = peers.clone();
        tokio::task::spawn_blocking(move || {
            peers.offer_with_client("v=0", &[], session("new"), Some(client))
        })
    };
    old.closed().await;
    gate.wait_until_answering().await;
    gate.open();
    restart.await.unwrap().unwrap();
    newer.await.unwrap().unwrap();

    let fresh = factory.peer_of("old").unwrap();
    assert!(!Arc::ptr_eq(&old, &fresh), "the restart got a fresh peer");
    assert!(!fresh.is_closed());
    assert_eq!(fresh.offers().len(), 1);
    let candidate = tokio::task::spawn_blocking(move || {
        peers.candidate(
            &old_sender,
            serde_json::json!({ "candidate": "after-answer" }),
        )
    })
    .await
    .unwrap();
    assert!(candidate.is_ok(), "{candidate:?}");
    assert_eq!(fresh.remote_candidates().len(), 1);
}

fn webrtc_peer(session_id: &str) -> WebrtcPeer {
    let (handler, _) = crate::carrier::testing::reporting_handler();
    WebrtcPeer {
        session_id: session_id.into(),
        intake: FrameIntake::new(handler, crate::transport::generate_transport_keypair()),
        policy: Arc::new(IcePolicy::default()),
        signaling: Arc::default(),
        remote: remote::RemoteCandidates::new(
            session_id.into(),
            None,
            Arc::new(|_| {}),
            Arc::new(mdns::LanAddressCache::new()),
            None,
        ),
        check_pending_direct_pairs: true,
        host_candidate_sweep: true,
        negotiation: tokio::sync::Mutex::new(None),
        ice: Arc::default(),
    }
}

/// The driver drains its agent's events under the core lock and delivers them
/// later, so a failure drained before a restart can reach the peer after the
/// restart was accepted. It belongs to the old ICE generation (review #455).
#[tokio::test]
async fn a_delayed_failure_callback_must_not_retire_an_accepted_restart() {
    use super::ice_diagnostic_tests::with_ice_credentials;
    use rtc::peer_connection::RTCPeerConnectionBuilder;

    let mut peer = webrtc_peer("delayed-failure");
    let signaling = session("delayed-failure");
    let mut browser = RTCPeerConnectionBuilder::new().build().unwrap();
    browser.create_data_channel("app", None).unwrap();
    let initial = browser.create_offer(None).unwrap().sdp;
    peer.answer(&initial, &[], signaling.clone()).await.unwrap();

    // Deliver the agent's events by hand, as the driver would.
    peer.ice = Arc::default();
    let (connected, _) = mpsc::unbounded_channel();
    let (sweeps, _) = mpsc::channel(1);
    let events = PeerEvents {
        session_id: "delayed-failure".into(),
        signaling: peer.signaling.clone(),
        connected,
        gathered: Mutex::new(GatheredTypes::default()),
        sweeps,
        ice: peer.ice.clone(),
    };
    let restart = with_ice_credentials(&initial, "restart", "restart-password-0123456789");
    peer.answer(&restart, &[], signaling).await.unwrap();
    assert!(!peer.ice_failed(), "the accepted restart cleared failure");

    events
        .on_ice_connection_state_change(RTCIceConnectionState::Failed)
        .await;
    let removed = Arc::new(AtomicBool::new(false));
    let flag = removed.clone();
    let closed = peer
        .close_if_ice_failed(Box::new(move || flag.store(true, Ordering::SeqCst)))
        .await;
    assert!(
        !closed && !removed.load(Ordering::SeqCst),
        "an old-generation failure callback retired a peer after its restart was accepted"
    );
}

/// What a real peer refuses whatever its callbacks said: the native core's
/// own ICE state decides, and this one never failed.
async fn refuses_to_close(peer: &WebrtcPeer) {
    let removed = Arc::new(AtomicBool::new(false));
    let flag = removed.clone();
    let closed = peer
        .close_if_ice_failed(Box::new(move || flag.store(true, Ordering::SeqCst)))
        .await;
    assert!(
        !closed && !removed.load(Ordering::SeqCst),
        "closed a live peer"
    );
}

/// Review #455 round 3: what the callbacks delivered, in whatever order and
/// however torn, never closes a peer whose native core has not failed.
#[tokio::test]
async fn a_real_peer_is_never_closed_on_failure_its_native_core_does_not_show() {
    use rtc::peer_connection::RTCPeerConnectionBuilder;

    let mut peer = webrtc_peer("delivered-only");
    let mut browser = RTCPeerConnectionBuilder::new().build().unwrap();
    browser.create_data_channel("app", None).unwrap();
    let initial = browser.create_offer(None).unwrap().sdp;
    peer.answer(&initial, &[], session("delivered-only"))
        .await
        .unwrap();
    peer.ice = Arc::default();
    peer.ice.observe(RTCIceConnectionState::Failed);
    assert!(
        peer.ice_failed(),
        "the delivered failure still prompts a look"
    );
    refuses_to_close(&peer).await;
}

/// Review #455 round 3: refusing a second restart leaves the first one's
/// generation alone.
#[tokio::test]
async fn a_refused_second_restart_keeps_the_accepted_restart() {
    use super::ice_diagnostic_tests::with_ice_credentials;
    use rtc::peer_connection::RTCPeerConnectionBuilder;

    let mut peer = webrtc_peer("second-restart");
    let signaling = session("second-restart");
    let mut browser = RTCPeerConnectionBuilder::new().build().unwrap();
    browser.create_data_channel("app", None).unwrap();
    let initial = browser.create_offer(None).unwrap().sdp;
    peer.answer(&initial, &[], signaling.clone()).await.unwrap();
    peer.ice = Arc::default();
    peer.ice.observe(RTCIceConnectionState::Failed);
    let first = with_ice_credentials(&initial, "restart", "restart-password-0123456789");
    peer.answer(&first, &[], signaling.clone()).await.unwrap();

    // New credentials but no media section to carry them: refused natively.
    let second = with_ice_credentials(&initial, "second", "second-password-0123456789");
    let refused = second[..second.find("m=").unwrap()].to_string();
    let answered = peer.answer(&refused, &[], signaling).await;
    assert!(
        matches!(answered, Err(RtcError::Refused(_))),
        "{answered:?}"
    );
    refuses_to_close(&peer).await;
}

/// A peer closed as superseded answers no later offer.
#[tokio::test]
async fn a_retired_real_peer_refuses_to_answer() {
    use rtc::peer_connection::RTCPeerConnectionBuilder;

    let peer = webrtc_peer("retired");
    let mut browser = RTCPeerConnectionBuilder::new().build().unwrap();
    browser.create_data_channel("app", None).unwrap();
    let offer = browser.create_offer(None).unwrap().sdp;
    peer.ice.retired.store(true, Ordering::SeqCst);
    let answered = peer.answer(&offer, &[], session("retired")).await;
    assert!(
        matches!(answered, Err(RtcError::Retired(_))),
        "{answered:?}"
    );
}

/// A native answering core driven by hand: its clock is whatever instant the
/// test hands it, and its ICE state events are drained and delivered
/// separately, as the driver does.
struct NativeCore {
    core: rtc::peer_connection::RTCPeerConnection,
    offer: String,
    clock: Instant,
}

impl NativeCore {
    fn answering() -> Self {
        use rtc::peer_connection::RTCPeerConnectionBuilder;

        let mut browser = RTCPeerConnectionBuilder::new().build().unwrap();
        browser.create_data_channel("app", None).unwrap();
        let offer = browser.create_offer(None).unwrap().sdp;
        let mut native = Self {
            core: RTCPeerConnectionBuilder::new().build().unwrap(),
            offer: offer.clone(),
            clock: Instant::now(),
        };
        native.accept(offer);
        native
    }

    fn accept(&mut self, offer: String) {
        let offer = RTCSessionDescription::offer(offer).unwrap();
        self.core.set_remote_description(offer).unwrap();
        let answer = self.core.create_answer(None).unwrap();
        self.core.set_local_description(answer).unwrap();
    }

    fn restart(&mut self, ufrag: &str) {
        use super::ice_diagnostic_tests::with_ice_credentials;
        let password = format!("{ufrag}-password-0123456789");
        self.accept(with_ice_credentials(&self.offer, ufrag, &password));
    }

    /// The ICE state events the core has queued, as the driver drains them.
    fn drain(&mut self) -> Vec<RTCIceConnectionState> {
        use rtc::peer_connection::event::RTCPeerConnectionEvent;
        use rtc::sansio::Protocol;

        let mut states = Vec::new();
        while let Some(event) = self.core.poll_event() {
            if let RTCPeerConnectionEvent::OnIceConnectionStateChangeEvent(state) = event {
                states.push(state);
            }
        }
        states
    }

    fn deliver(&mut self, ice: &IceHealth) -> Vec<RTCIceConnectionState> {
        let states = self.drain();
        states.iter().for_each(|state| ice.observe(*state));
        states
    }

    /// Run the agent's clock past its failure timeout with no peer answering.
    fn time_out(&mut self) {
        use rtc::sansio::Protocol;

        for _ in 0..2 {
            self.clock += Duration::from_secs(61);
            self.core.handle_timeout(self.clock).unwrap();
        }
    }

    fn state(&mut self) -> Option<rtc::peer_connection::transport::RTCIceTransportState> {
        native_ice_state(&self.core.get_stats(self.clock, StatsSelector::None))
    }
}

/// Review #455 round 2 on the native core: a failure the driver drained
/// before a restart and delivers after it belongs to the old generation, and
/// the restart's own checking event opens the new one.
#[test]
fn a_failure_drained_before_a_restart_is_the_old_generations() {
    use rtc::peer_connection::transport::RTCIceTransportState;

    let mut native = NativeCore::answering();
    let ice = IceHealth::default();
    native.deliver(&ice);
    native.time_out();
    let held = native.drain();
    assert!(held.contains(&RTCIceConnectionState::Failed), "{held:?}");

    let delivered = ice.checkings();
    native.restart("restart");
    let after_restart = native.state();
    assert_eq!(after_restart, Some(RTCIceTransportState::Failed));
    ice.accept_restart(delivered, after_restart);
    held.iter().for_each(|state| ice.observe(*state));
    assert!(
        !ice.failed_now(native.state()),
        "a failure from before the restart closed the restarted peer"
    );

    let opened = native.deliver(&ice);
    assert_eq!(opened, [RTCIceConnectionState::Checking]);
    assert!(!ice.failed_now(native.state()));
    native.time_out();
    native.deliver(&ice);
    assert!(ice.failed_now(native.state()), "the new generation failed");
}

/// Review #455 round 3: a restart accepted while the agent was still checking
/// queues no checking event. The failure that follows is current, and the
/// idle peer is still cleaned up.
#[test]
fn a_failure_after_a_restart_that_queued_no_checking_is_current() {
    let mut native = NativeCore::answering();
    let ice = IceHealth::default();
    let looked = Arc::new(AtomicU64::new(0));
    let counted = looked.clone();
    let _ = ice.on_failed.set(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
    }));
    let opened = native.deliver(&ice);
    assert!(
        opened.contains(&RTCIceConnectionState::Checking),
        "{opened:?}"
    );

    let delivered = ice.checkings();
    native.restart("restart");
    ice.accept_restart(delivered, native.state());
    let restarted = native.deliver(&ice);
    assert!(
        !restarted.contains(&RTCIceConnectionState::Checking),
        "{restarted:?}"
    );

    native.time_out();
    let failed = native.deliver(&ice);
    assert!(
        failed.contains(&RTCIceConnectionState::Failed),
        "{failed:?}"
    );
    assert_eq!(looked.load(Ordering::SeqCst), 1, "no cleanup was prompted");
    assert!(
        ice.failed_now(native.state()),
        "the native failure was masked"
    );
}
