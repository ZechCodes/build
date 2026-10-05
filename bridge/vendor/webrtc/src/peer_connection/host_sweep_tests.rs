use super::*;
use rtc::shared::ifaces::Kind;

fn address(value: &str) -> SocketAddr {
    value.parse().unwrap()
}
fn interface(ip: &str, mask: &str, name: &str) -> Interface {
    Interface {
        name: name.into(),
        kind: Kind::Ipv4,
        addr: Some(address(ip)),
        mask: Some(address(mask)),
        hop: None,
    }
}
fn subnet() -> SweepSubnet {
    SweepSubnet {
        local: address("192.168.2.1:45000"),
        interface_name: "wifi".into(),
        interface_index: 2,
        addresses: vec![Ipv4Addr::new(192, 168, 2, 2), Ipv4Addr::new(192, 168, 2, 3)].into(),
        mask: u32::from(Ipv4Addr::new(255, 255, 255, 0)),
    }
}

#[test]
fn generation_prflx_observation_survives_sibling_port_stop_and_close() {
    let now = Instant::now();
    let mut sweep = HostSweep::default();
    for port in [40000, 40001] {
        sweep.start(7, "same".into(), port, now);
        sweep.prepare(port, vec![subnet()]);
    }
    let probe = sweep.next_probe(now + GRACE, false).unwrap();
    sweep.record_result(probe.port, true);
    sweep.observe_prflx(address("192.168.2.1:45000"), address("192.168.2.2:40000"));
    sweep.stop_all("direct-selected");
    let mut last = None;
    while let Some(event) = sweep.pop_event() {
        last = Some(event);
    }
    assert!(last.unwrap().prflx_followed);
    sweep.clear("closed");
    while let Some(event) = sweep.pop_event() {
        assert!(event.prflx_followed);
    }
    sweep.start(8, "new".into(), 40001, now);
    sweep.prepare(40001, vec![subnet()]);
    assert!(!sweep.pop_event().unwrap().prflx_followed);
}

#[test]
fn prflx_attribution_excludes_an_address_that_has_not_been_sent_a_probe() {
    let now = Instant::now();
    let mut sweep = HostSweep::default();
    sweep.start(7, "same".into(), 40000, now);
    sweep.prepare(40000, vec![subnet()]);
    while sweep.pop_event().is_some() {}
    let probe = sweep.next_probe(now + GRACE, false).unwrap();
    assert_eq!(probe.destination, Ipv4Addr::new(192, 168, 2, 2));
    sweep.record_result(40000, true);
    sweep.observe_prflx(address("192.168.2.1:45000"), address("192.168.2.3:40000"));
    assert!(sweep.pop_event().is_none());
    sweep.observe_prflx(address("192.168.2.1:45000"), address("192.168.2.2:40000"));
    assert!(sweep.pop_event().unwrap().prflx_followed);
}

#[test]
fn generation_totals_never_decrease_across_ports_skips_cancel_and_close() {
    let now = Instant::now();
    let mut sweep = HostSweep::default();
    for port in [40000, 40001] {
        let mut route = subnet();
        route.addresses = vec![Ipv4Addr::new(192, 168, 2, 2)].into();
        sweep.start(7, "same".into(), port, now);
        sweep.prepare(port, vec![route]);
    }
    while sweep.pop_event().is_some() {}
    let first = sweep.next_probe(now + GRACE, false).unwrap();
    sweep.record_result(first.port, true);
    assert_eq!(sweep.pop_event().unwrap().addresses_sent, 1);
    let second = sweep.next_probe(now + GRACE + PACE, false).unwrap();
    sweep.record_result(second.port, true);
    assert_eq!(sweep.pop_event().unwrap().addresses_sent, 2);
    sweep.start(7, "same".into(), 40002, now);
    sweep.skip(40002, "subnet-too-large");
    assert_eq!(sweep.pop_event().unwrap().addresses_sent, 2);
    sweep.cancel(7, "same", 40000);
    assert_eq!(sweep.pop_event().unwrap().addresses_sent, 2);
    sweep.clear("closed");
    while let Some(event) = sweep.pop_event() {
        assert_eq!((event.addresses_sent, event.addresses_attempted), (2, 2));
    }
    sweep.start(8, "new".into(), 40000, now);
    sweep.prepare(40000, vec![subnet()]);
    assert_eq!(sweep.pop_event().unwrap().addresses_sent, 0);
}

#[test]
fn ambiguous_source_address_ownership_and_interface_replacement_are_skipped() {
    let local = address("192.168.2.1:45000");
    let wifi = interface("192.168.2.1:0", "255.255.255.0:0", "wifi");
    let duplicate = interface("192.168.2.1:0", "255.255.255.0:0", "bridge");
    assert_eq!(
        SweepSubnet::for_socket(local, &[wifi.clone(), duplicate], |_| 2).unwrap_err(),
        "ambiguous-interface"
    );
    let subnet = SweepSubnet::for_socket(local, &[wifi.clone()], |_| 2).unwrap();
    let remote = Ipv4Addr::new(192, 168, 2, 2);
    assert!(subnet.still_owned(&[wifi.clone()], 2, remote));
    assert!(!subnet.still_owned(&[wifi.clone()], 3, remote));
    let own_new = interface("192.168.2.2:0", "255.255.255.0:0", "wifi");
    assert!(!subnet.still_owned(&[wifi.clone(), own_new], 2, remote));
    let changed_mask = interface("192.168.2.1:0", "255.255.255.128:0", "wifi");
    assert!(!subnet.still_owned(&[changed_mask], 2, remote));
    assert!(!subnet.still_owned(&[], 2, remote));
}

#[test]
fn last_successful_send_is_counted_and_prflx_attribution_requires_the_swept_socket_port_and_ip() {
    let now = Instant::now();
    let mut sweep = HostSweep::default();
    sweep.start(7, "same".into(), 40000, now);
    sweep.prepare(40000, vec![subnet()]);
    while sweep.pop_event().is_some() {}
    assert!(sweep.next_probe(now + GRACE, false).is_some());
    sweep.record_result(40000, true);
    assert!(sweep.next_probe(now + GRACE + PACE, false).is_some());
    sweep.record_result(40000, true);
    assert_eq!(sweep.pop_event().unwrap().addresses_sent, 2);
    sweep.observe_prflx(address("192.168.2.1:45001"), address("192.168.2.2:40000"));
    sweep.observe_prflx(address("192.168.2.1:45000"), address("192.168.3.2:40000"));
    sweep.observe_prflx(address("192.168.2.1:45000"), address("192.168.2.2:40001"));
    assert!(sweep.pop_event().is_none());
    sweep.observe_prflx(address("192.168.2.1:45000"), address("192.168.2.2:40000"));
    assert!(sweep.pop_event().unwrap().prflx_followed);
    sweep.start(8, "new".into(), 40000, now);
    sweep.prepare(40000, vec![subnet()]);
    while sweep.pop_event().is_some() {}
    sweep.observe_prflx(address("192.168.2.1:45000"), address("192.168.2.2:40000"));
    assert!(sweep.pop_event().is_none());
}

#[test]
fn missing_host_gathering_can_be_deferred_without_extending_the_window() {
    let now = Instant::now();
    let mut sweep = HostSweep::default();
    sweep.start(7, "same".into(), 40000, now);
    assert_eq!(sweep.unprepared_ports(now + GRACE), vec![40000]);
    sweep.defer_preparation(40000, now + GRACE);
    assert!(sweep.unprepared_ports(now + GRACE).is_empty());
    sweep.prepare(40000, vec![subnet()]);
    assert!(
        sweep
            .next_probe(now + GRACE + Duration::from_millis(50), false)
            .is_some()
    );
    assert!(sweep.next_probe(now + WINDOW, false).is_none());
}

#[test]
fn numeric_generation_restarts_budget_even_when_ufrag_is_reused() {
    let now = Instant::now();
    let mut sweep = HostSweep::default();
    sweep.start(7, "same".into(), 40000, now);
    sweep.prepare(40000, vec![subnet()]);
    sweep.cancel(7, "same", 40000);
    sweep.start(8, "same".into(), 40000, now);
    sweep.prepare(40000, vec![subnet()]);
    assert!(sweep.next_probe(now + GRACE, false).is_some());
    sweep.start(7, "same".into(), 40001, now);
    assert!(!sweep.plans.contains_key(&40001));
}

#[test]
fn password_only_restart_retires_all_old_port_plans() {
    let now = Instant::now();
    let mut sweep = HostSweep::default();
    sweep.sync_credentials("same", "old-password");
    sweep.start(7, "same".into(), 40000, now);
    sweep.prepare(40000, vec![subnet()]);
    sweep.sync_credentials("same", "new-password");
    assert!(sweep.next_probe(now + GRACE, false).is_none());
}

#[test]
fn aggregate_eligibility_survives_unrelated_skips_and_expiry_but_resolution_clears_one_port() {
    let now = Instant::now();
    let mut sweep = HostSweep::default();
    sweep.start(7, "same".into(), 40000, now);
    sweep.prepare(40000, vec![subnet()]);
    sweep.start(7, "same".into(), 40001, now);
    sweep.prepare(40001, vec![subnet()]);
    sweep.start(7, "same".into(), 40002, now);
    sweep.skip(40002, "subnet-too-large");
    while let Some(event) = sweep.pop_event() {
        assert_eq!(event.eligible_unresolved, 2);
    }
    sweep.next_probe(now + WINDOW, true);
    assert_eq!(sweep.pop_event().unwrap().eligible_unresolved, 2);
    sweep.cancel(7, "same", 40000);
    let event = sweep.pop_event().unwrap();
    assert_eq!(event.eligible_unresolved, 1);
    sweep.stop_all("direct-selected");
    while let Some(event) = sweep.pop_event() {
        assert_eq!(event.eligible_unresolved, 0);
    }
}

#[test]
fn synchronous_cancel_stops_next_send_and_stale_cancel_cannot_touch_new_generation() {
    let control = HostSweepControl::default();
    assert!(control.start(7, "old", 40000));
    assert!(control.allowed(7, "old", 40000));
    control.cancel(7, "old", 40000);
    assert!(!control.allowed(7, "old", 40000));
    assert!(control.start(8, "new", 40000));
    control.cancel(7, "old", 40000);
    assert!(control.allowed(8, "new", 40000));
    assert!(!control.allowed(7, "old", 40000));
    control.clear();
    assert!(!control.allowed(8, "new", 40000));
    assert!(!control.start(8, "new", 40001));
    assert!(!control.start(7, "old", 40001));
    assert!(control.start(9, "new", 40000));
    control.cancel(8, "new", 40000);
    assert!(control.allowed(9, "new", 40000));
}

#[test]
fn payload_contains_only_fingerprint_and_fresh_nonidentifying_transaction_id() {
    use rtc::stun::attributes::ATTR_FINGERPRINT;
    use rtc::stun::fingerprint::FINGERPRINT;
    use rtc::stun::message::{CLASS_INDICATION, METHOD_BINDING, Message};
    let first = binding_indication().unwrap();
    let second = binding_indication().unwrap();
    assert_eq!(first.len(), 28);
    assert_ne!(&first[8..20], &second[8..20]);
    let mut message = Message::new();
    message.raw = first;
    message.decode().unwrap();
    assert_eq!(message.typ.method, METHOD_BINDING);
    assert_eq!(message.typ.class, CLASS_INDICATION);
    assert_eq!(message.attributes.0.len(), 1);
    assert_eq!(message.attributes.0[0].typ, ATTR_FINGERPRINT);
    FINGERPRINT.check(&message).unwrap();
}

#[test]
fn link_local_is_allowed_but_single_host_and_point_to_point_masks_have_no_targets() {
    let local = address("169.254.7.1:45000");
    let iface = interface("169.254.7.1:0", "255.255.255.0:0", "wifi");
    assert_eq!(
        SweepSubnet::from_interface(local, &iface, 2, &[])
            .unwrap()
            .addresses
            .len(),
        253
    );
    for mask in ["255.255.255.254:0", "255.255.255.255:0"] {
        let iface = interface("169.254.7.1:0", mask, "wifi");
        assert_eq!(
            SweepSubnet::from_interface(local, &iface, 2, &[]).unwrap_err(),
            "no-usable-addresses"
        );
    }
}

#[test]
fn the_global_packet_cap_stops_even_with_multiple_subnets() {
    let now = Instant::now();
    let mut sweep = HostSweep::default();
    sweep.start(7, "ufrag".into(), 5353, now);
    sweep.prepare(5353, vec![subnet()]);
    sweep.packets = MAX_PACKETS - 1;
    assert_eq!(sweep.next_probe(now + GRACE, false).unwrap().port, 5353);
    assert!(sweep.next_probe(now + GRACE + PACE, false).is_none());
    assert!(
        sweep
            .events
            .iter()
            .any(|event| event.reason == Some("packet-limit"))
    );
}

#[test]
fn only_the_host_socket_owning_interface_supplies_destinations() {
    let local = address("192.168.2.1:45000");
    let wifi = interface("192.168.2.1:0", "255.255.255.0:0", "wifi");
    let docker = interface("172.17.0.1:0", "255.255.255.0:0", "docker0");
    let own = SweepSubnet::from_interface(local, &wifi, 2, &[local.ip()]).unwrap();
    assert_eq!(own.addresses.len(), 253);
    assert!(
        own.addresses
            .iter()
            .all(|ip| ip.octets()[..3] == [192, 168, 2])
    );
    assert_eq!(own.local, local);
    assert!(SweepSubnet::from_interface(local, &docker, 3, &[]).is_err());
}

#[test]
fn subnet_cap_is_total_addresses_and_all_our_addresses_are_skipped() {
    let local = address("10.3.0.1:45000");
    let too_large = interface("10.3.0.1:0", "255.255.248.0:0", "wifi");
    assert_eq!(
        SweepSubnet::from_interface(local, &too_large, 2, &[]).unwrap_err(),
        "subnet-too-large"
    );
    let fits = interface("10.3.0.1:0", "255.255.252.0:0", "wifi");
    let result =
        SweepSubnet::from_interface(local, &fits, 2, &[local.ip(), address("10.3.0.2:0").ip()])
            .unwrap();
    assert_eq!(result.addresses.len(), 1020);
    assert!(!result.addresses.contains(&Ipv4Addr::new(10, 3, 0, 0)));
    assert!(!result.addresses.contains(&Ipv4Addr::new(10, 3, 3, 255)));
    assert!(!result.addresses.contains(&Ipv4Addr::new(10, 3, 0, 2)));
}

#[test]
fn public_cgnat_bad_mask_and_point_to_point_are_skipped() {
    for ip in ["8.8.8.1:45000", "100.64.0.1:45000", "127.0.0.1:45000"] {
        let iface = interface(ip, "255.255.255.0:0", "wifi");
        assert_eq!(
            SweepSubnet::from_interface(address(ip), &iface, 2, &[]).unwrap_err(),
            "non-private-subnet"
        );
    }
    let local = address("192.168.2.1:45000");
    let mut iface = interface("192.168.2.1:0", "255.0.255.0:0", "wifi");
    assert_eq!(
        SweepSubnet::from_interface(local, &iface, 2, &[]).unwrap_err(),
        "invalid-netmask"
    );
    iface.mask = None;
    assert_eq!(
        SweepSubnet::from_interface(local, &iface, 2, &[]).unwrap_err(),
        "invalid-netmask"
    );
    iface.mask = Some(address("255.255.255.0:0"));
    iface.hop = Some(NextHop::Destination(address("192.168.2.2:0")));
    assert_eq!(
        SweepSubnet::from_interface(local, &iface, 2, &[]).unwrap_err(),
        "point-to-point"
    );
}

#[test]
fn initial_grace_and_global_pace_apply_across_ports() {
    let now = Instant::now();
    let mut sweep = HostSweep::default();
    for port in [40000, 40001] {
        sweep.start(7, "ufrag".into(), port, now);
        sweep.prepare(port, vec![subnet()]);
    }
    assert!(sweep.next_probe(now, false).is_none());
    let first = sweep.next_probe(now + GRACE, false).expect("first probe");
    assert_eq!(first.subnet.local.port(), 45000);
    assert!(sweep.next_probe(now + GRACE, false).is_none());
    assert!(sweep.next_probe(now + GRACE + PACE, false).is_some());
}

#[test]
fn one_repeat_only_on_relay_and_inside_absolute_window() {
    let now = Instant::now();
    let mut sweep = HostSweep::default();
    sweep.start(7, "ufrag".into(), 40000, now);
    sweep.prepare(40000, vec![subnet()]);
    assert!(sweep.next_probe(now + GRACE, false).is_some());
    assert!(sweep.next_probe(now + GRACE + PACE, false).is_some());
    assert!(
        sweep
            .next_probe(now + GRACE + PACE + REPEAT_DELAY, false)
            .is_none()
    );
    assert!(
        sweep
            .next_probe(now + GRACE + PACE + REPEAT_DELAY, true)
            .is_some()
    );
    assert!(
        sweep
            .next_probe(now + GRACE + 2 * PACE + REPEAT_DELAY, true)
            .is_some()
    );
    assert!(
        sweep
            .next_probe(now + Duration::from_secs(20), true)
            .is_none()
    );
    sweep.start(8, "new".into(), 40001, now);
    sweep.prepare(40001, vec![subnet()]);
    assert!(sweep.next_probe(now + WINDOW, true).is_none());
}

#[test]
fn resolve_and_generation_changes_stop_and_stale_cancels_do_not() {
    let now = Instant::now();
    let mut sweep = HostSweep::default();
    sweep.start(7, "ufrag".into(), 40000, now);
    sweep.prepare(40000, vec![subnet()]);
    sweep.cancel(7, "ufrag", 40000);
    assert!(sweep.next_probe(now + GRACE, true).is_none());
    sweep.start(8, "new".into(), 40000, now);
    sweep.prepare(40000, vec![subnet()]);
    sweep.cancel(7, "ufrag", 40000);
    assert!(sweep.next_probe(now + GRACE, true).is_some());
    sweep.sync_generation("newer");
    assert!(sweep.next_probe(now + GRACE + PACE, true).is_none());
}

#[test]
fn duplicate_ports_and_plan_cap_are_bounded() {
    let now = Instant::now();
    let mut sweep = HostSweep::default();
    for port in 1..=(MAX_PORTS as u16 + 1) {
        sweep.start(7, "ufrag".into(), port, now);
    }
    sweep.start(7, "ufrag".into(), 1, now);
    assert_eq!(sweep.plans.len(), MAX_PORTS);
}
