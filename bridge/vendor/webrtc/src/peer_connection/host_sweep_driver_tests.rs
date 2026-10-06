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
