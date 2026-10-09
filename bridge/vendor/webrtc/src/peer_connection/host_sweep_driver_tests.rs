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

#[cfg(all(target_os = "linux", feature = "runtime-tokio"))]
#[test]
fn a_sweep_pass_reads_interfaces_while_the_core_lock_is_free() {
    use super::driver::SweepInterfaces;
    use rtc::shared::ifaces::Interface;
    use std::cell::RefCell;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    // The pass's interface read parks here, as a slow netlink dump would, until released.
    thread_local! {
        static READ_GATE: RefCell<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>> =
            const { RefCell::new(None) };
    }
    fn parked_interfaces() -> std::io::Result<Vec<Interface>> {
        READ_GATE.with(|gate| {
            if let Some((entered, release)) = gate.borrow().as_ref() {
                entered.send(()).unwrap();
                release.recv_timeout(Duration::from_secs(10)).unwrap();
            }
        });
        Ok(Vec::new())
    }

    let runtime = default_runtime().unwrap();
    let mut connection = None;
    runtime.block_on(Box::pin(async {
        connection = Some(new_test_peer_connection().await);
    }));
    let (mut inner, _events) = connection.unwrap();
    Arc::get_mut(&mut inner).unwrap().host_candidate_sweep = true;
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let driver_inner = Arc::clone(&inner);
    let pass = std::thread::spawn(move || {
        READ_GATE.with(|gate| *gate.borrow_mut() = Some((entered_tx, release_rx)));
        let mut driver = PeerConnectionDriver::new(
            driver_inner,
            Vec::<SocketAddr>::new(),
            Vec::<SocketAddr>::new(),
            rtc::ice::mdns::MulticastDnsMode::Disabled,
            Vec::new(),
            RTCIceTransportPolicy::All,
            false,
        );
        driver.sweep_interfaces = SweepInterfaces::reading(parked_interfaces);
        driver
            .host_sweep
            .start(1, "ufrag".into(), 40000, Instant::now());
        default_runtime()
            .unwrap()
            .block_on(Box::pin(driver.poll_host_sweep(Instant::now())));
    });
    entered_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("an active sweep pass must read the interface list");
    let core_free = inner.core.try_lock().is_some();
    release_tx.send(()).unwrap();
    pass.join().unwrap();
    assert!(
        core_free,
        "the core lock must stay free while the sweep reads system state"
    );
}

#[test]
fn sweep_passes_share_one_interface_read_until_it_is_stale() {
    use super::driver::SweepInterfaces;
    use rtc::shared::ifaces::Interface;
    use std::cell::Cell;
    use std::time::{Duration, Instant};

    thread_local! {
        static READS: Cell<usize> = const { Cell::new(0) };
    }
    fn counted_interfaces() -> std::io::Result<Vec<Interface>> {
        READS.with(|reads| reads.set(reads.get() + 1));
        Ok(Vec::new())
    }

    let mut interfaces = SweepInterfaces::reading(counted_interfaces);
    let start = Instant::now();
    interfaces.refresh(start);
    interfaces.refresh(start + Duration::from_millis(99));
    assert_eq!(
        READS.with(Cell::get),
        1,
        "passes within 100 ms reuse the list"
    );
    interfaces.refresh(start + Duration::from_millis(100));
    assert_eq!(READS.with(Cell::get), 2, "a 100 ms old list is read again");
    let read_at = start + Duration::from_millis(100);
    assert!(interfaces.current(read_at).is_some());
    assert!(
        interfaces
            .current(read_at + Duration::from_millis(100))
            .is_none(),
        "a list is not used once it is 100 ms old"
    );
}

#[cfg(all(target_os = "linux", feature = "runtime-tokio"))]
#[test]
fn a_pass_that_waits_past_the_interface_bound_for_the_core_lock_does_not_run() {
    use super::driver::SweepInterfaces;
    use rtc::shared::ifaces::Interface;
    use std::cell::RefCell;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    thread_local! {
        static READ: RefCell<Option<mpsc::Sender<()>>> = const { RefCell::new(None) };
    }
    fn signalled_interfaces() -> std::io::Result<Vec<Interface>> {
        READ.with(|read| {
            if let Some(read) = read.borrow().as_ref() {
                let _ = read.send(());
            }
        });
        Ok(Vec::new())
    }

    let runtime = default_runtime().unwrap();
    let mut connection = None;
    runtime.block_on(Box::pin(async {
        connection = Some(new_test_peer_connection().await);
    }));
    let (mut inner, _events) = connection.unwrap();
    Arc::get_mut(&mut inner).unwrap().host_candidate_sweep = true;
    let mut core = None;
    runtime.block_on(Box::pin(async {
        core = Some(inner.core.lock().await);
    }));
    let (read_tx, read_rx) = mpsc::channel();
    let driver_inner = Arc::clone(&inner);
    let pass = std::thread::spawn(move || {
        READ.with(|read| *read.borrow_mut() = Some(read_tx));
        let mut driver = PeerConnectionDriver::new(
            driver_inner,
            Vec::<SocketAddr>::new(),
            Vec::<SocketAddr>::new(),
            rtc::ice::mdns::MulticastDnsMode::Disabled,
            Vec::new(),
            RTCIceTransportPolicy::All,
            false,
        );
        driver.sweep_interfaces = SweepInterfaces::reading(signalled_interfaces);
        let started = Instant::now();
        driver.host_sweep.start(1, "ufrag".into(), 40000, started);
        let runtime = default_runtime().unwrap();
        runtime.block_on(Box::pin(driver.poll_host_sweep(Instant::now())));
        let stale = driver.host_sweep.deadline();
        runtime.block_on(Box::pin(driver.poll_host_sweep(Instant::now())));
        (started, stale, driver.host_sweep.deadline())
    });
    read_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the pass must read the interface list before the lock");
    std::thread::sleep(Duration::from_millis(150));
    drop(core);
    let (started, stale, fresh) = pass.join().unwrap();
    // A pass that runs cancels this plan, whose credentials the core never had; an abandoned
    // pass leaves it due at its start.
    assert_eq!(
        stale,
        Some(started),
        "a list older than 100 ms by the time the lock is held must not be used"
    );
    assert_ne!(
        fresh,
        Some(started),
        "the next pass reads the list again and runs"
    );
}

/// A stand-in kernel for the interface checks at send time: the address table a full read
/// returns, the live index and mask lookup, and the notification socket the kernel writes to
/// whenever an IPv4 address is added or removed. `the_address_watch_hears_an_address_added_in_a_private_namespace`
/// pins that the real kernel writes one.
#[cfg(target_os = "linux")]
mod fake_kernel {
    use rtc::shared::ifaces::{Interface, Kind};
    use std::cell::RefCell;
    use std::net::Ipv4Addr;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixDatagram;

    thread_local! {
        static TABLE: RefCell<Vec<Interface>> = const { RefCell::new(Vec::new()) };
        static NOTIFY: RefCell<Option<UnixDatagram>> = const { RefCell::new(None) };
    }

    pub(super) fn interface(ip: &str, mask: &str, name: &str) -> Interface {
        Interface {
            name: name.into(),
            kind: Kind::Ipv4,
            addr: Some(format!("{ip}:0").parse().unwrap()),
            mask: Some(format!("{mask}:0").parse().unwrap()),
            hop: None,
        }
    }

    pub(super) fn table() -> std::io::Result<Vec<Interface>> {
        Ok(TABLE.with(|table| table.borrow().clone()))
    }

    pub(super) fn watch() -> std::io::Result<OwnedFd> {
        let (ours, kernel) = UnixDatagram::pair()?;
        NOTIFY.with(|notify| *notify.borrow_mut() = Some(kernel));
        Ok(ours.into())
    }

    /// `wifi`, index 2, still holds every address under 255.255.255.0.
    pub(super) fn live(_: &str, _: Ipv4Addr) -> (u32, Option<Ipv4Addr>) {
        (2, Some(Ipv4Addr::new(255, 255, 255, 0)))
    }

    pub(super) fn add_address(interface: Interface) {
        TABLE.with(|table| table.borrow_mut().push(interface));
        NOTIFY.with(|notify| {
            if let Some(notify) = notify.borrow().as_ref() {
                notify.send(b"RTM_NEWADDR").unwrap();
            }
        });
    }

    pub(super) fn reset(interfaces: Vec<Interface>) {
        TABLE.with(|table| *table.borrow_mut() = interfaces);
        NOTIFY.with(|notify| *notify.borrow_mut() = None);
    }
}

#[cfg(target_os = "linux")]
fn watched_wifi() -> (
    super::driver::SweepInterfaces,
    super::host_sweep::SweepSubnet,
    std::time::Instant,
) {
    use super::driver::SweepInterfaces;
    use super::host_sweep::SweepSubnet;

    let wifi = fake_kernel::interface("192.168.2.1", "255.255.255.0", "wifi");
    fake_kernel::reset(vec![wifi.clone()]);
    let subnet =
        SweepSubnet::for_socket("192.168.2.1:45000".parse().unwrap(), &[wifi], |_| 2).unwrap();
    let mut interfaces = SweepInterfaces::reading(fake_kernel::table);
    interfaces.live = fake_kernel::live;
    interfaces.watch = fake_kernel::watch;
    let now = std::time::Instant::now();
    interfaces.refresh(now);
    (interfaces, subnet, now)
}

#[cfg(target_os = "linux")]
#[test]
fn a_source_address_another_interface_takes_after_the_read_stops_the_send() {
    let (interfaces, subnet, now) = watched_wifi();
    let remote = std::net::Ipv4Addr::new(192, 168, 2, 2);
    assert!(interfaces.owns(now, &subnet, remote));
    // The selected interface's index, address and mask are unchanged; only the list is not.
    fake_kernel::add_address(fake_kernel::interface(
        "192.168.2.1",
        "255.255.255.0",
        "bridge",
    ));
    assert!(
        !interfaces.owns(now, &subnet, remote),
        "a source address two interfaces now hold must not be sent from"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_destination_that_became_a_local_address_after_the_read_stops_the_send() {
    let (interfaces, subnet, now) = watched_wifi();
    let remote = std::net::Ipv4Addr::new(192, 168, 2, 2);
    assert!(interfaces.owns(now, &subnet, remote));
    fake_kernel::add_address(fake_kernel::interface(
        "192.168.2.2",
        "255.255.255.255",
        "docker0",
    ));
    assert!(
        !interfaces.owns(now, &subnet, remote),
        "a destination this host now holds must not be probed"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn the_next_read_after_an_address_change_is_trusted_again() {
    use std::time::Duration;

    let (mut interfaces, subnet, now) = watched_wifi();
    let remote = std::net::Ipv4Addr::new(192, 168, 2, 2);
    fake_kernel::add_address(fake_kernel::interface(
        "192.168.2.9",
        "255.255.255.0",
        "wifi",
    ));
    assert!(!interfaces.owns(now, &subnet, remote));
    let later = now + Duration::from_millis(100);
    interfaces.refresh(later);
    assert!(
        interfaces.owns(later, &subnet, remote),
        "a list read after the change, with no change since, stands again"
    );
}
