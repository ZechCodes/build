//! A reloaded client's earlier session, whose ICE has failed, is torn down as
//! soon as the same paired client has a newer session (#373). The hint is a
//! bearer value, not an identity, so it never closes a peer that still works.

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_newer_session_of_the_same_client_closes_its_failed_predecessor() {
    let (peers, factory) = peers();
    let client = uuid::Uuid::new_v4();
    let old = offered(&peers, &factory, &session("old"), Some(client)).await;
    old.fail_ice();
    let new = offered(&peers, &factory, &session("new"), Some(client)).await;
    assert_eq!(peers.count(), 1, "only the newer session keeps a peer");
    old.closed().await;
    assert!(!new.is_closed());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_predecessor_whose_ice_fails_after_the_newer_session_opened_is_closed() {
    let (peers, factory) = peers();
    let client = uuid::Uuid::new_v4();
    let old = offered(&peers, &factory, &session("old"), Some(client)).await;
    let new = offered(&peers, &factory, &session("new"), Some(client)).await;
    assert!(!old.is_closed(), "a predecessor that still carries is kept");
    old.fail_ice();
    assert_eq!(peers.count(), 1);
    old.closed().await;
    assert!(!new.is_closed());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_hint_alone_never_closes_a_session_whose_ice_has_not_failed() {
    let (peers, factory) = peers();
    let client = uuid::Uuid::new_v4();
    let old = offered(&peers, &factory, &session("old"), Some(client)).await;
    old.fail_ice();
    old.recover_ice();
    let new = offered(&peers, &factory, &session("new"), Some(client)).await;
    new.fail_ice();
    new.recover_ice();
    assert!(
        !old.is_closed(),
        "an ICE restart that recovered is connected"
    );
    assert!(!new.is_closed());
    assert_eq!(peers.count(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_session_is_never_closed_for_an_older_one() {
    let (peers, factory) = peers();
    let client = uuid::Uuid::new_v4();
    let old = offered(&peers, &factory, &session("old"), Some(client)).await;
    let new = offered(&peers, &factory, &session("new"), Some(client)).await;
    new.fail_ice();
    let newest = offered(&peers, &factory, &session("newest"), None).await;
    assert!(
        !new.is_closed(),
        "the newest failed session may still restart"
    );
    assert!(!old.is_closed());
    assert!(!newest.is_closed());
    assert_eq!(peers.count(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
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
    assert!(!unhinted.is_closed(), "no hint, nothing to correlate");
    assert!(
        !other.is_closed(),
        "another client's hint is not this one's"
    );
    assert_eq!(peers.count(), 4);
}

/// The SPA binds its hint once the greeting advertises the capability, which
/// may be on the newer session's second offer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hint_bound_on_a_later_offer_still_supersedes() {
    let (peers, factory) = peers();
    let client = uuid::Uuid::new_v4();
    let old = offered(&peers, &factory, &session("old"), Some(client)).await;
    old.fail_ice();
    let new_sender = session("new");
    offered(&peers, &factory, &new_sender, None).await;
    assert!(!old.is_closed());
    offered(&peers, &factory, &new_sender, Some(client)).await;
    assert_eq!(peers.count(), 1);
    old.closed().await;
}

/// A closed predecessor leaves its session open on the bridge, so a later
/// ICE restart from it gets a fresh peer like any first offer would.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_superseded_session_that_offers_again_negotiates_a_fresh_peer() {
    let (peers, factory) = peers();
    let client = uuid::Uuid::new_v4();
    let old_sender = session("old");
    let old = offered(&peers, &factory, &old_sender, Some(client)).await;
    old.fail_ice();
    offered(&peers, &factory, &session("new"), Some(client)).await;
    assert_eq!(peers.count(), 1);
    old.closed().await;
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
