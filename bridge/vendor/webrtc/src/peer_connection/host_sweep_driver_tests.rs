use super::driver::{PeerConnectionDriver, PeerConnectionDriverEvent};
use super::tests::new_test_peer_connection;
use super::{PeerConnection, PeerConnectionImpl};
use crate::runtime::default_runtime;
use rtc::peer_connection::RTCPeerConnectionBuilder;
use rtc::peer_connection::configuration::RTCIceTransportPolicy;
use rtc::peer_connection::configuration::setting_engine::SettingEngine;
use rtc::peer_connection::sdp::RTCSessionDescription;
use std::net::SocketAddr;
use std::sync::Arc;

fn sweep_candidate(value: &str) -> rtc::peer_connection::transport::RTCIceCandidateInit {
    rtc::peer_connection::transport::RTCIceCandidateInit {
        candidate: value.into(),
        ..Default::default()
    }
}

fn exchange_sweep_packets(
    source: &mut rtc::peer_connection::RTCPeerConnection,
    destination: &mut rtc::peer_connection::RTCPeerConnection,
) {
    use rtc::sansio::Protocol;
    while source.poll_event().is_some() {}
    while let Some(mut packet) = source.poll_write() {
        std::mem::swap(
            &mut packet.transport.local_addr,
            &mut packet.transport.peer_addr,
        );
        destination.handle_read(packet).unwrap();
    }
}

#[test]
fn native_recovery_from_direct_to_relay_rearms_the_sweep_for_new_credentials() {
    use super::host_sweep::{SweepCredentials, SweepSubnet};
    use rtc::ice::candidate::CandidateType;
    use rtc::peer_connection::configuration::RTCOfferOptions;
    use rtc::sansio::Protocol;
    use rtc::shared::ifaces::{Interface, Kind};
    use std::time::{Duration, Instant};

    let runtime = default_runtime().unwrap();
    let test = async {
        let (mut inner, _) = new_test_peer_connection().await;
        Arc::get_mut(&mut inner).unwrap().host_candidate_sweep = true;
        let mut settings = SettingEngine::default();
        settings.set_discard_local_candidates_during_ice_restart(true);
        settings.set_relay_acceptance_min_wait(Some(Duration::ZERO));
        let mut browser = RTCPeerConnectionBuilder::new()
            .with_setting_engine(settings)
            .build()
            .unwrap();
        browser.create_data_channel("sweep", None).unwrap();
        let mut driver = PeerConnectionDriver::new(
            Arc::clone(&inner),
            Vec::<SocketAddr>::new(),
            Vec::<SocketAddr>::new(),
            rtc::ice::mdns::MulticastDnsMode::Disabled,
            Vec::new(),
            RTCIceTransportPolicy::All,
            false,
        );
        let mut now = Instant::now();
        let local: SocketAddr = "192.168.2.1:45000".parse().unwrap();
        let iface = Interface {
            name: "fixture".into(),
            kind: Kind::Ipv4,
            addr: Some("192.168.2.1:0".parse().unwrap()),
            mask: Some("255.255.255.252:0".parse().unwrap()),
            hop: None,
        };
        let mut previous_credentials: Option<SweepCredentials> = None;
        for (generation, candidate, port, remote_type) in [
            (
                1,
                "candidate:1 1 udp 2130706431 192.168.2.2 40000 typ host",
                40000,
                CandidateType::PeerReflexive,
            ),
            (
                2,
                "candidate:2 1 udp 16777215 203.0.113.1 6000 typ relay raddr 192.168.2.2 rport 5002",
                40001,
                CandidateType::Relay,
            ),
        ] {
            if generation == 2 {
                inner.host_sweep_control.clear();
            }
            let mut core = inner.core.lock().await;
            let offer = browser
                .create_offer(Some(RTCOfferOptions {
                    ice_restart: generation == 2,
                }))
                .unwrap();
            browser
                .add_local_candidate(sweep_candidate(candidate))
                .unwrap();
            browser.set_local_description(offer.clone()).unwrap();
            core.set_remote_description(offer).unwrap();
            if generation == 1 {
                core.add_local_candidate(sweep_candidate(
                    "candidate:bridge 1 udp 2130706431 192.168.2.1 45000 typ host",
                ))
                .unwrap();
            } else {
                core.add_remote_candidate(sweep_candidate(candidate))
                    .unwrap();
            }
            let answer = core.create_answer(None).unwrap();
            core.set_local_description(answer.clone()).unwrap();
            browser.set_remote_description(answer).unwrap();
            browser
                .add_remote_candidate(sweep_candidate(
                    "candidate:bridge 1 udp 2130706431 192.168.2.1 45000 typ host",
                ))
                .unwrap();
            let (ufrag, password) = core.remote_ice_credentials();
            if let Some(previous) = &previous_credentials {
                assert!(!previous.matches((ufrag, password)));
            }
            let credentials = SweepCredentials::new(ufrag, password);
            previous_credentials = Some(credentials.clone());
            let ufrag = ufrag.to_owned();
            for _ in 0..16 {
                core.handle_timeout(now).unwrap();
                browser.handle_timeout(now).unwrap();
                exchange_sweep_packets(&mut browser, &mut core);
                exchange_sweep_packets(&mut core, &mut browser);
                now += Duration::from_millis(250);
            }
            let selected = core
                .selected_ice_candidates()
                .unwrap_or_else(|| panic!("native fixture never selected generation {generation}"));
            assert_eq!(selected.1.candidate_type(), remote_type);
            drop(core);

            assert!(inner.host_sweep_control.start(generation, &ufrag, port));
            driver
                .handle_driver_event(PeerConnectionDriverEvent::StartHostSweep {
                    generation,
                    credentials,
                    port,
                })
                .await;
            driver.host_sweep.prepare(
                port,
                vec![SweepSubnet::from_interface(local, &iface, 1, &[]).unwrap()],
            );
            let fresh = std::iter::from_fn(|| driver.host_sweep.pop_event())
                .last()
                .unwrap();
            assert_eq!(fresh.addresses_sent, 0);
            assert_eq!(fresh.addresses_attempted, 0);
            assert!(!fresh.prflx_followed);
            let probe = driver
                .host_sweep
                .next_probe(now + Duration::from_millis(250), true)
                .unwrap();
            driver.host_sweep.record_result(probe.port, true);
            driver
                .host_sweep
                .observe_prflx(local, format!("192.168.2.2:{port}").parse().unwrap());
            let followed = std::iter::from_fn(|| driver.host_sweep.pop_event())
                .last()
                .unwrap();
            assert!(followed.prflx_followed);
            assert_eq!(followed.addresses_sent, 1);
            if generation == 1 {
                driver.host_sweep.stop_all("direct-selected");
            }
        }
    };
    runtime.block_on(Box::pin(test));
}

#[test]
fn queued_start_is_rejected_after_native_password_only_restart() {
    let runtime = default_runtime().unwrap();
    runtime.block_on(Box::pin(async {
        let (mut inner, _) = new_test_peer_connection().await;
        Arc::get_mut(&mut inner).unwrap().host_candidate_sweep = true;
        let mut settings = SettingEngine::default();
        settings.set_ice_credentials("same-ufrag".into(), "original-password-for-the-test".into());
        let mut browser = RTCPeerConnectionBuilder::new()
            .with_setting_engine(settings)
            .build()
            .unwrap();
        browser.create_data_channel("sweep", None).unwrap();
        let old_offer = browser.create_offer(None).unwrap();
        {
            let mut core = inner.core.lock().await;
            core.set_remote_description(old_offer.clone()).unwrap();
            let answer = core.create_answer(None).unwrap();
            core.set_local_description(answer).unwrap();
        }
        inner.host_sweep_control.start(7, "same-ufrag", 40000);
        let queued = PeerConnectionDriverEvent::StartHostSweep {
            generation: 7,
            credentials: super::host_sweep::SweepCredentials::new(
                "same-ufrag",
                "original-password-for-the-test",
            ),
            port: 40000,
        };
        let changed = RTCSessionDescription::offer(old_offer.sdp.replace(
            "original-password-for-the-test",
            "replacement-password-for-test",
        ))
        .unwrap();
        inner
            .core
            .lock()
            .await
            .set_remote_description(changed)
            .unwrap();
        let mut driver = PeerConnectionDriver::new(
            inner,
            Vec::<SocketAddr>::new(),
            Vec::<SocketAddr>::new(),
            rtc::ice::mdns::MulticastDnsMode::Disabled,
            Vec::new(),
            RTCIceTransportPolicy::All,
            false,
        );
        driver.handle_driver_event(queued).await;
        assert!(
            driver.host_sweep.is_empty(),
            "a queued command must retain its accepted credential epoch"
        );
    }));
}

#[cfg(all(target_os = "linux", feature = "runtime-tokio"))]
#[test]
fn remote_credentials_capture_baseline_before_answer_and_failed_offer_preserves_it() {
    let runtime = default_runtime().unwrap();
    runtime.block_on(Box::pin(async {
        let (mut inner, _events) = new_test_peer_connection().await;
        Arc::get_mut(&mut inner).unwrap().host_candidate_sweep = true;
        inner.host_sweep_control.set_owners(vec![123456]);
        let peer = PeerConnectionImpl {
            inner: Arc::clone(&inner),
            driver_handle: super::Mutex::new(None),
            dedicated_reactor: false,
        };
        let mut browser = RTCPeerConnectionBuilder::new().build().unwrap();
        browser.create_data_channel("sweep", None).unwrap();
        let offer = browser.create_offer(None).unwrap();
        let credentials = super::ice_credentials(&offer).unwrap();
        peer.set_remote_description(offer.clone()).await.unwrap();
        assert!(
            inner
                .host_sweep_control
                .baseline_captured((&credentials.0, &credentials.1), 123456),
            "baseline must exist before answer creation or any unresolved port event"
        );
        let changed = RTCSessionDescription::offer(
            offer
                .sdp
                .replace(&credentials.1, "different-password-for-failed-offer"),
        )
        .unwrap();
        assert!(peer.set_remote_description(changed).await.is_err());
        assert!(
            inner
                .host_sweep_control
                .baseline_captured((&credentials.0, &credentials.1), 123456)
        );
        let answer = inner.core.lock().await.create_answer(None);
        assert!(answer.is_ok());
    }));
}

#[cfg(all(target_os = "linux", feature = "runtime-tokio"))]
#[test]
fn explicit_close_erases_baseline_even_when_no_unresolved_plan_arrived() {
    let runtime = default_runtime().unwrap();
    runtime.block_on(Box::pin(async {
        let (mut inner, mut events) = new_test_peer_connection().await;
        Arc::get_mut(&mut inner).unwrap().host_candidate_sweep = true;
        inner.host_sweep_control.set_owners(vec![123456]);
        let peer = PeerConnectionImpl {
            inner: Arc::clone(&inner),
            driver_handle: super::Mutex::new(None),
            dedicated_reactor: false,
        };
        let mut browser = RTCPeerConnectionBuilder::new().build().unwrap();
        browser.create_data_channel("sweep", None).unwrap();
        let offer = browser.create_offer(None).unwrap();
        let credentials = super::ice_credentials(&offer).unwrap();
        peer.set_remote_description(offer).await.unwrap();
        assert!(
            inner
                .host_sweep_control
                .baseline_captured((&credentials.0, &credentials.1), 123456)
        );
        let _ = events.recv().await;
        peer.close().await.unwrap();
        assert!(
            !inner
                .host_sweep_control
                .baseline_captured((&credentials.0, &credentials.1), 123456)
        );
    }));
}

#[test]
fn delayed_old_clear_does_not_retire_the_new_generation() {
    let runtime = default_runtime().unwrap();
    runtime.block_on(Box::pin(async {
        let (inner, _) = new_test_peer_connection().await;
        let mut driver = PeerConnectionDriver::new(
            inner,
            Vec::<SocketAddr>::new(),
            Vec::<SocketAddr>::new(),
            rtc::ice::mdns::MulticastDnsMode::Disabled,
            Vec::new(),
            RTCIceTransportPolicy::All,
            false,
        );
        driver
            .host_sweep
            .start(8, "new".into(), 40000, std::time::Instant::now());
        driver
            .handle_driver_event(PeerConnectionDriverEvent::WriteNotify)
            .await;
        assert!(
            !driver.host_sweep.is_empty(),
            "a stale clear notification must not destroy newer plans"
        );
    }));
}
