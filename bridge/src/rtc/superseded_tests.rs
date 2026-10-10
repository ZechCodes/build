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
        offers: Arc::default(),
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
        offers: Arc::default(),
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
/// test hands it, and its events are drained and delivered separately, as the
/// driver does.
struct NativeCore {
    core: rtc::peer_connection::RTCPeerConnection,
    offer: String,
    clock: Instant,
    /// Have-remote-offer events delivered, as the connection's events count them.
    offers: Arc<AtomicU64>,
}

/// One drain's events, held until they are delivered.
#[derive(Default)]
struct Drained {
    states: Vec<RTCIceConnectionState>,
    offers: u64,
}

impl Drained {
    /// Hand the events to the connection's own handler, the offers first.
    async fn deliver(self, events: &PeerEvents) -> Vec<RTCIceConnectionState> {
        self.deliver_offers(events).await;
        self.deliver_states(events).await
    }

    async fn deliver_offers(&self, events: &PeerEvents) {
        for _ in 0..self.offers {
            events
                .on_signaling_state_change(RTCSignalingState::HaveRemoteOffer)
                .await;
        }
    }

    async fn deliver_states(&self, events: &PeerEvents) -> Vec<RTCIceConnectionState> {
        for state in &self.states {
            events.on_ice_connection_state_change(*state).await;
        }
        self.states.clone()
    }
}

/// The handler a connection's driver delivers to.
fn peer_events(ice: Arc<IceHealth>, offers: Arc<AtomicU64>) -> PeerEvents {
    let (connected, _) = mpsc::unbounded_channel();
    let (sweeps, _) = mpsc::channel(1);
    PeerEvents {
        session_id: "native".into(),
        signaling: Arc::new(Trickling::default()),
        connected,
        gathered: Mutex::new(GatheredTypes::default()),
        sweeps,
        ice,
        offers,
    }
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
            offers: Arc::default(),
        };
        let offer = RTCSessionDescription::offer(offer).unwrap();
        native.core.set_remote_description(offer).unwrap();
        let answer = native.core.create_answer(None).unwrap();
        native.core.set_local_description(answer).unwrap();
        native
    }

    fn restart_offer(&self, ufrag: &str) -> String {
        use super::ice_diagnostic_tests::with_ice_credentials;
        let password = format!("{ufrag}-password-0123456789");
        with_ice_credentials(&self.offer, ufrag, &password)
    }

    /// The events the core has queued, as the driver drains them.
    fn drain(&mut self) -> Drained {
        use rtc::peer_connection::event::RTCPeerConnectionEvent;
        use rtc::sansio::Protocol;

        let mut drained = Drained::default();
        while let Some(event) = self.core.poll_event() {
            match event {
                RTCPeerConnectionEvent::OnIceConnectionStateChangeEvent(state) => {
                    drained.states.push(state)
                }
                RTCPeerConnectionEvent::OnSignalingStateChangeEvent(
                    RTCSignalingState::HaveRemoteOffer,
                ) => drained.offers += 1,
                _ => {}
            }
        }
        drained
    }

    async fn deliver(&mut self, ice: Arc<IceHealth>) -> Vec<RTCIceConnectionState> {
        let events = peer_events(ice, self.offers.clone());
        self.drain().deliver(&events).await
    }

    /// Run the agent's clock past its failure timeout with no peer answering.
    fn time_out(&mut self) {
        for _ in 0..2 {
            self.tick(Duration::from_secs(61));
        }
    }

    fn tick(&mut self, by: Duration) {
        use rtc::sansio::Protocol;

        self.clock += by;
        self.core.handle_timeout(self.clock).unwrap();
    }

    fn state(&mut self) -> Option<RTCIceTransportState> {
        native_ice_state(&self.core.get_stats(self.clock, StatsSelector::None))
    }
}

/// The async wrapper's scheduling replaced by hand around a real sans-I/O core,
/// so the production `answer()` and close run against genuine native SDP,
/// timeouts, drains and statistics (after review #455 round 4's adapter).
struct CorePeer {
    native: Mutex<NativeCore>,
    events: PeerEvents,
    closed: AtomicBool,
    /// Fail the agent's next tick as soon as the restart is applied, the way
    /// the driver can win the core lock before `answer()`'s next call.
    fail_after_restart: AtomicBool,
    failure_pending: AtomicBool,
    deferred: Mutex<Drained>,
}

impl CorePeer {
    fn new(native: NativeCore, ice: Arc<IceHealth>) -> Arc<Self> {
        Arc::new(Self {
            events: peer_events(ice, native.offers.clone()),
            native: Mutex::new(native),
            closed: AtomicBool::new(false),
            fail_after_restart: AtomicBool::new(false),
            failure_pending: AtomicBool::new(false),
            deferred: Mutex::default(),
        })
    }

    async fn deliver(&self) -> Vec<RTCIceConnectionState> {
        let drained = self.native.lock().unwrap().drain();
        drained.deliver(&self.events).await
    }

    fn take_deferred(&self) -> Drained {
        std::mem::take(&mut *self.deferred.lock().unwrap())
    }

    fn with_native<T>(&self, f: impl FnOnce(&mut NativeCore) -> T) -> T {
        f(&mut self.native.lock().unwrap())
    }
}

#[async_trait]
impl PeerConnection for CorePeer {
    async fn close(&self) -> webrtc::error::Result<()> {
        use rtc::sansio::Protocol;

        self.native.lock().unwrap().core.close()?;
        self.closed.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn get_stats(&self, now: Instant, selector: StatsSelector) -> RTCStatsReport {
        self.native.lock().unwrap().core.get_stats(now, selector)
    }

    async fn create_offer(
        &self,
        _options: Option<rtc::peer_connection::configuration::RTCOfferOptions>,
    ) -> webrtc::error::Result<RTCSessionDescription> {
        unimplemented!("the bridge answers")
    }

    async fn create_answer(
        &self,
        options: Option<rtc::peer_connection::configuration::RTCAnswerOptions>,
    ) -> webrtc::error::Result<RTCSessionDescription> {
        Ok(self.native.lock().unwrap().core.create_answer(options)?)
    }

    async fn set_local_description(
        &self,
        description: RTCSessionDescription,
    ) -> webrtc::error::Result<()> {
        Ok(self
            .native
            .lock()
            .unwrap()
            .core
            .set_local_description(description)?)
    }

    async fn local_description(&self) -> Option<RTCSessionDescription> {
        unimplemented!("not read by the bridge")
    }

    async fn current_local_description(&self) -> Option<RTCSessionDescription> {
        unimplemented!("not read by the bridge")
    }

    async fn pending_local_description(&self) -> Option<RTCSessionDescription> {
        unimplemented!("not read by the bridge")
    }

    async fn can_trickle_ice_candidates(&self) -> Option<bool> {
        unimplemented!("not read by the bridge")
    }

    async fn set_remote_description(
        &self,
        description: RTCSessionDescription,
    ) -> webrtc::error::Result<()> {
        let applied = self
            .native
            .lock()
            .unwrap()
            .core
            .set_remote_description(description);
        if self.fail_after_restart.swap(false, Ordering::SeqCst) {
            self.failure_pending.store(true, Ordering::SeqCst);
        }
        Ok(applied?)
    }

    async fn remote_description(&self) -> Option<RTCSessionDescription> {
        unimplemented!("not read by the bridge")
    }

    async fn current_remote_description(&self) -> Option<RTCSessionDescription> {
        unimplemented!("not read by the bridge")
    }

    async fn pending_remote_description(&self) -> Option<RTCSessionDescription> {
        let mut native = self.native.lock().unwrap();
        if self.failure_pending.swap(false, Ordering::SeqCst) {
            // The driver wins the core lock first: a restart while checking
            // keeps the agent's old timer, so its very next tick fails.
            native.tick(Duration::from_millis(201));
            let drained = native.drain();
            assert_eq!(drained.states, [RTCIceConnectionState::Failed]);
            assert_eq!(drained.offers, 1, "the restart's offer drains with it");
            *self.deferred.lock().unwrap() = drained;
        }
        native.core.pending_remote_description().cloned()
    }

    async fn add_ice_candidate(
        &self,
        _candidate: RTCIceCandidateInit,
    ) -> webrtc::error::Result<()> {
        unimplemented!("no candidates are trickled")
    }

    async fn restart_ice(&self) -> webrtc::error::Result<()> {
        unimplemented!("the bridge never restarts ICE itself")
    }

    async fn get_configuration(&self) -> RTCConfiguration {
        unimplemented!("not read by the bridge")
    }

    async fn set_configuration(
        &self,
        configuration: RTCConfiguration,
    ) -> webrtc::error::Result<()> {
        Ok(self
            .native
            .lock()
            .unwrap()
            .core
            .set_configuration(configuration)?)
    }

    async fn create_data_channel(
        &self,
        _label: &str,
        _options: Option<RTCDataChannelInit>,
    ) -> webrtc::error::Result<Arc<dyn DataChannel>> {
        unimplemented!("no carriers")
    }

    async fn get_senders(&self) -> Vec<Arc<dyn webrtc::rtp_transceiver::RtpSender>> {
        unimplemented!("no media")
    }

    async fn get_receivers(&self) -> Vec<Arc<dyn webrtc::rtp_transceiver::RtpReceiver>> {
        unimplemented!("no media")
    }

    async fn get_transceivers(&self) -> Vec<Arc<dyn webrtc::rtp_transceiver::RtpTransceiver>> {
        unimplemented!("no media")
    }

    async fn add_track(
        &self,
        _track: Arc<dyn webrtc::media_stream::track_local::TrackLocal>,
    ) -> webrtc::error::Result<Arc<dyn webrtc::rtp_transceiver::RtpSender>> {
        unimplemented!("no media")
    }

    async fn remove_track(
        &self,
        _sender: &Arc<dyn webrtc::rtp_transceiver::RtpSender>,
    ) -> webrtc::error::Result<()> {
        unimplemented!("no media")
    }

    async fn add_transceiver_from_track(
        &self,
        _track: Arc<dyn webrtc::media_stream::track_local::TrackLocal>,
        _init: Option<rtc::rtp_transceiver::RTCRtpTransceiverInit>,
    ) -> webrtc::error::Result<Arc<dyn webrtc::rtp_transceiver::RtpTransceiver>> {
        unimplemented!("no media")
    }

    async fn add_transceiver_from_kind(
        &self,
        _kind: rtc::rtp_transceiver::rtp_sender::RtpCodecKind,
        _init: Option<rtc::rtp_transceiver::RTCRtpTransceiverInit>,
    ) -> webrtc::error::Result<Arc<dyn webrtc::rtp_transceiver::RtpTransceiver>> {
        unimplemented!("no media")
    }
}

/// A real peer whose connection is `native`, negotiated on its first offer as
/// `answer()` would have: one have-remote-offer queued, its delivery up to
/// the native core's event stream.
async fn native_peer(session_id: &str, native: NativeCore) -> (WebrtcPeer, Arc<CorePeer>) {
    let peer = webrtc_peer(session_id);
    peer.remote.begin(&native.offer).await;
    let offers_delivered = native.offers.clone();
    let connection = CorePeer::new(native, peer.ice.clone());
    *peer.negotiation.lock().await = Some(Negotiation {
        connection: connection.clone(),
        carriers: Vec::new(),
        path_report: tokio::spawn(std::future::pending::<()>()),
        sweep_report: tokio::spawn(std::future::pending::<()>()),
        offers_queued: 1,
        offers_delivered,
    });
    (peer, connection)
}

async fn failed_native() -> NativeCore {
    let mut native = NativeCore::answering();
    let opened = native.deliver(Arc::default()).await;
    assert!(opened.contains(&RTCIceConnectionState::Checking));
    native.time_out();
    let failed = native.deliver(Arc::default()).await;
    assert!(failed.contains(&RTCIceConnectionState::Failed));
    assert_eq!(native.state(), Some(RTCIceTransportState::Failed));
    native
}

/// Attempt the close a newer session's cleanup makes, and say what it did.
async fn try_close(peer: &WebrtcPeer, connection: &CorePeer) -> (bool, bool, bool, bool) {
    let removed = Arc::new(AtomicBool::new(false));
    let flag = removed.clone();
    let closed = peer
        .close_if_ice_failed(Box::new(move || flag.store(true, Ordering::SeqCst)))
        .await;
    (
        closed,
        removed.load(Ordering::SeqCst),
        peer.ice.retired.load(Ordering::SeqCst),
        connection.closed.load(Ordering::SeqCst),
    )
}

const KEPT: (bool, bool, bool, bool) = (false, false, false, false);
const RETIRED: (bool, bool, bool, bool) = (true, true, true, true);

/// Review #455 round 4, P1: a cleanup that runs after a restart was accepted,
/// while the core still records the old failure and the restart's checking is
/// queued, keeps the peer. It looks again once the driver has drained past
/// the restart, and only a failure of the new generation retires it.
#[tokio::test]
async fn r4_actual_close_must_not_combine_old_native_failure_with_new_checking() {
    let native = failed_native().await;
    let restart = native.restart_offer("restarted");
    let (peer, connection) = native_peer("r4-snapshot", native).await;
    peer.answer(&restart, &[], session("r4-snapshot"))
        .await
        .unwrap();
    assert_eq!(
        connection.with_native(NativeCore::state),
        Some(RTCIceTransportState::Failed),
        "the restart's checking is still queued"
    );
    assert_eq!(try_close(&peer, &connection).await, KEPT);

    assert_eq!(
        connection.deliver().await,
        [RTCIceConnectionState::Checking]
    );
    assert_eq!(try_close(&peer, &connection).await, KEPT);

    connection.with_native(NativeCore::time_out);
    assert_eq!(connection.deliver().await, [RTCIceConnectionState::Failed]);
    assert_eq!(try_close(&peer, &connection).await, RETIRED);
    assert!(peer.negotiation.lock().await.is_none());
}

/// The production close unregisters, retires, tears down its negotiation and
/// closes a native core that genuinely failed.
#[tokio::test]
async fn r4_actual_close_retires_and_closes_a_genuine_native_failure() {
    let native = failed_native().await;
    let (peer, connection) = native_peer("r4-failed", native).await;
    assert_eq!(try_close(&peer, &connection).await, RETIRED);
    assert!(peer.negotiation.lock().await.is_none());
    let after = peer
        .answer(
            &connection.with_native(|n| n.offer.clone()),
            &[],
            session("r4-failed"),
        )
        .await;
    assert!(matches!(after, Err(RtcError::Retired(_))), "{after:?}");
}

/// Review #455 round 4, P2: a restart accepted while the agent is checking
/// keeps its old timer, so the next tick can genuinely fail before `answer()`
/// returns. That failure is current and the idle peer is closed.
#[tokio::test]
async fn r4_answer_must_not_mask_a_current_failure_forever() {
    let mut native = NativeCore::answering();
    native.deliver(Arc::default()).await;
    native.tick(Duration::from_secs(1));
    native.deliver(Arc::default()).await;
    native.tick(Duration::from_secs(30));
    native.deliver(Arc::default()).await;
    assert_eq!(native.state(), Some(RTCIceTransportState::Checking));
    let restart = native.restart_offer("current");
    let (peer, connection) = native_peer("r4-current", native).await;
    let looked = Arc::new(AtomicU64::new(0));
    let counted = looked.clone();
    let _ = peer.ice.on_failed.set(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
    }));
    connection.fail_after_restart.store(true, Ordering::SeqCst);
    peer.answer(&restart, &[], session("r4-current"))
        .await
        .unwrap();
    let drained = connection.take_deferred().deliver(&connection.events).await;
    assert_eq!(drained, [RTCIceConnectionState::Failed]);
    assert!(
        looked.load(Ordering::SeqCst) >= 1,
        "no cleanup was prompted"
    );

    // Nothing further comes: no checking, and an idle channel never stalls.
    connection.with_native(|native| native.tick(Duration::from_secs(600)));
    assert!(connection.deliver().await.is_empty());
    let mut stall = StallWatch::default();
    let idle = SendReading {
        handed: 0,
        outstanding: 0,
    };
    assert_eq!(stall.sample(idle, Duration::from_secs(600)), None);
    assert_eq!(try_close(&peer, &connection).await, RETIRED);
}

/// Review #455 rounds 3 and 4: a refused second restart neither ends the
/// first restart's protection nor blocks cleanup once the new generation
/// fails.
#[tokio::test]
async fn r4_refused_second_restart_preserves_actual_stale_failure_hold() {
    let native = failed_native().await;
    let first = native.restart_offer("first");
    let second = native.restart_offer("second");
    let (peer, connection) = native_peer("r4-refused", native).await;
    peer.answer(&first, &[], session("r4-refused"))
        .await
        .unwrap();
    let refused = &second[..second.find("m=").unwrap()];
    let answered = peer.answer(refused, &[], session("r4-refused")).await;
    assert!(
        matches!(answered, Err(RtcError::Refused(_))),
        "{answered:?}"
    );
    assert_eq!(try_close(&peer, &connection).await, KEPT);

    assert_eq!(
        connection.deliver().await,
        [RTCIceConnectionState::Checking]
    );
    connection.with_native(NativeCore::time_out);
    assert_eq!(connection.deliver().await, [RTCIceConnectionState::Failed]);
    assert_eq!(try_close(&peer, &connection).await, RETIRED);
}

/// A restart while checking queues no checking event; the failure after it
/// still closes the idle peer.
#[tokio::test]
async fn r4_checking_restart_with_stats_before_failure_still_closes_idle_peer() {
    let mut native = NativeCore::answering();
    native.deliver(Arc::default()).await;
    let restart = native.restart_offer("checking");
    let (peer, connection) = native_peer("r4-checking", native).await;
    peer.answer(&restart, &[], session("r4-checking"))
        .await
        .unwrap();
    assert!(connection.deliver().await.is_empty(), "no checking event");
    connection.with_native(NativeCore::time_out);
    assert_eq!(connection.deliver().await, [RTCIceConnectionState::Failed]);
    assert_eq!(try_close(&peer, &connection).await, RETIRED);
}

/// The driver may deliver a drained failure before the restart offer drained
/// with it. The failure prompts a look the close must refuse; the offer's
/// delivery prompts the look that closes the peer.
#[tokio::test]
async fn a_restart_offer_delivered_after_its_failure_prompts_the_close() {
    let mut native = NativeCore::answering();
    native.deliver(Arc::default()).await;
    native.tick(Duration::from_secs(1));
    native.deliver(Arc::default()).await;
    native.tick(Duration::from_secs(30));
    native.deliver(Arc::default()).await;
    let restart = native.restart_offer("late-offer");
    let (peer, connection) = native_peer("late-offer", native).await;
    let looks = Arc::new(Mutex::new(Vec::new()));
    let (seen, weak) = (looks.clone(), Arc::downgrade(&connection));
    let peer = Arc::new(peer);
    let looking = Arc::downgrade(&peer);
    let _ = peer.ice.on_failed.set(Arc::new(move || {
        let (Some(peer), Some(connection)) = (looking.upgrade(), weak.upgrade()) else {
            return;
        };
        let seen = seen.clone();
        tokio::spawn(async move {
            let outcome = try_close(&peer, &connection).await;
            seen.lock().unwrap().push(outcome);
        });
    }));
    connection.fail_after_restart.store(true, Ordering::SeqCst);
    peer.answer(&restart, &[], session("late-offer"))
        .await
        .unwrap();

    let drained = connection.take_deferred();
    drained.deliver_states(&connection.events).await;
    settled().await;
    assert_eq!(*looks.lock().unwrap(), [KEPT]);
    drained.deliver_offers(&connection.events).await;
    settled().await;
    assert_eq!(*looks.lock().unwrap(), [KEPT, RETIRED]);
}
