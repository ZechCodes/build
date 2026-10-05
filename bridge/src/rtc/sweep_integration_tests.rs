use super::*;

#[tokio::test]
async fn production_opts_in_and_namespace_controls_remain_independent() {
    let (handler, _) = crate::carrier::testing::reporting_handler();
    let intake = FrameIntake::new(handler, crate::transport::generate_transport_keypair());
    let production = WebrtcPeerFactory::new(intake.clone(), IcePolicy::default());
    assert!(production.check_pending_direct_pairs);
    assert!(production.host_candidate_sweep);
    let baseline = WebrtcPeerFactory::with_connection_checks(
        intake.clone(),
        IcePolicy::default(),
        true,
        false,
    );
    assert!(baseline.check_pending_direct_pairs);
    assert!(!baseline.host_candidate_sweep);
    let sweep =
        WebrtcPeerFactory::with_connection_checks(intake, IcePolicy::default(), false, true);
    assert!(!sweep.check_pending_direct_pairs);
    assert!(sweep.host_candidate_sweep);
}

#[tokio::test]
async fn sweep_callbacks_queue_in_order_without_waiting_for_remote_discovery() {
    let (connected, _) = mpsc::unbounded_channel();
    let (sweeps, mut observed) = mpsc::channel(256);
    let handler = PeerEvents {
        session_id: "sweep-queue".into(),
        signaling: Arc::new(Trickling::default()),
        connected,
        gathered: Mutex::new(GatheredTypes::default()),
        sweeps,
    };
    for status in ["started", "progress", "stopped"] {
        handler
            .on_host_candidate_sweep(HostCandidateSweepEvent {
                generation: 1,
                status,
                addresses_sent: 0,
                addresses_attempted: 0,
                reason: None,
                eligible: true,
                eligible_unresolved: 1,
                prflx_followed: false,
            })
            .await;
    }
    for status in ["started", "progress", "stopped"] {
        assert_eq!(observed.try_recv().unwrap().status, status);
    }
}
