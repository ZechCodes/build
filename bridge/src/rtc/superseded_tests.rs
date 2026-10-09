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
        assert_eq!(ice.failed.load(Ordering::SeqCst), failed, "{state}");
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

/// The real peer decides under its negotiation lock, the one an offer holds,
/// and an accepted ICE restart is no longer failed.
#[tokio::test]
async fn a_real_peer_closes_only_while_failed_and_a_restart_clears_failure() {
    use super::ice_diagnostic_tests::with_ice_credentials;
    use rtc::peer_connection::RTCPeerConnectionBuilder;

    let mut peer = webrtc_peer("restarting");
    let signaling = session("restarting");
    let mut browser = RTCPeerConnectionBuilder::new().build().unwrap();
    browser.create_data_channel("app", None).unwrap();
    let initial = browser.create_offer(None).unwrap().sdp;
    peer.answer(&initial, &[], signaling.clone()).await.unwrap();
    // The agent's own state events now land elsewhere: the restart's checking
    // event may arrive after a queued close has run, so the answer alone must
    // clear the failure.
    peer.ice = Arc::default();
    peer.ice.observe(RTCIceConnectionState::Failed);
    let restart = with_ice_credentials(&initial, "restart", "restart-password-0123456789");
    peer.answer(&restart, &[], signaling.clone()).await.unwrap();
    assert!(
        !peer.ice_failed(),
        "an accepted ICE restart is checking again"
    );

    let unregistered = Arc::new(AtomicBool::new(false));
    let flag = unregistered.clone();
    let unregister = move || flag.store(true, Ordering::SeqCst);
    assert!(!peer.close_if_ice_failed(Box::new(unregister.clone())).await);
    assert!(!unregistered.load(Ordering::SeqCst));

    peer.ice.observe(RTCIceConnectionState::Failed);
    assert!(peer.close_if_ice_failed(Box::new(unregister)).await);
    assert!(unregistered.load(Ordering::SeqCst));
    let after = peer.answer(&restart, &[], signaling).await;
    assert!(matches!(after, Err(RtcError::Retired(_))), "{after:?}");
}
