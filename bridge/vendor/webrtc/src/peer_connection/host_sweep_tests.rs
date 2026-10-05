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

#[test]
fn neighbor_priority_changes_order_without_authorizing_any_address() {
    let local = address("192.168.2.1:45000");
    let iface = interface("192.168.2.1:0", "255.255.255.0:0", "wifi");
    let mut subnet = SweepSubnet::from_interface(local, &iface, 2, &[]).unwrap();
    let original = subnet.addresses.to_vec();
    subnet.prioritize(&[
        Ipv4Addr::new(8, 8, 8, 8),
        Ipv4Addr::new(172, 17, 0, 2),
        Ipv4Addr::new(192, 168, 2, 0),
        Ipv4Addr::new(192, 168, 2, 1),
        Ipv4Addr::new(192, 168, 2, 255),
        Ipv4Addr::new(192, 168, 2, 254),
        Ipv4Addr::new(192, 168, 2, 254),
        Ipv4Addr::new(192, 168, 3, 2),
    ]);
    assert_eq!(subnet.addresses[0], Ipv4Addr::new(192, 168, 2, 254));
    let mut reordered = subnet.addresses.to_vec();
    reordered.sort_unstable();
    assert_eq!(
        reordered, original,
        "neighbor hints cannot widen or duplicate the allowed set"
    );
}

#[test]
fn socket_pressure_keeps_the_same_destination_until_it_is_sent() {
    let now = Instant::now();
    let mut sweep = HostSweep::default();
    sweep.start(7, "ufrag".into(), 40000, now);
    sweep.prepare(40000, vec![subnet()]);
    let first = sweep.next_probe(now + GRACE, false).unwrap();
    sweep.record_result(first.port, false);
    let retry = sweep.next_probe(now + GRACE + PACE, false).unwrap();
    assert_eq!(
        retry.destination, first.destination,
        "pressure cannot omit an address"
    );
    sweep.record_result(retry.port, true);
    let second = sweep.next_probe(now + GRACE + 2 * PACE, false).unwrap();
    assert_ne!(second.destination, first.destination);
    sweep.record_result(second.port, false);
    assert!(
        !sweep.plans[&40000].repeat,
        "pressure cannot consume the pass budget"
    );
    let again = sweep.next_probe(now + GRACE + REPEAT_DELAY, true).unwrap();
    assert_eq!(again.destination, second.destination);
    sweep.record_result(again.port, true);
    assert!(sweep.plans[&40000].repeat);
    assert_eq!(sweep.packets, 4);
    assert_eq!(sweep.sent, 2);
    let repeat = now + GRACE + 2 * REPEAT_DELAY;
    let first_repeat = sweep.next_probe(repeat, true).unwrap();
    sweep.record_result(first_repeat.port, true);
    let last_repeat = sweep.next_probe(repeat + PACE, true).unwrap();
    sweep.record_result(last_repeat.port, false);
    assert!(
        !sweep.plans[&40000].stopped,
        "pressure cannot prematurely finish the repeat"
    );
    let retry_repeat = sweep.next_probe(repeat + 2 * PACE, true).unwrap();
    assert_eq!(retry_repeat.destination, last_repeat.destination);
    sweep.record_result(retry_repeat.port, true);
    assert!(sweep.plans[&40000].stopped);
    assert_eq!(sweep.sent, 4);
}

#[test]
fn retired_sweep_address_state_is_erased_but_numeric_eligibility_survives_expiry() {
    let now = Instant::now();
    for reason in [
        "resolved",
        "direct-selected",
        "generation-changed",
        "closed",
    ] {
        let mut sweep = HostSweep::default();
        sweep.start(7, "ufrag".into(), 40000, now);
        sweep.prepare(40000, vec![subnet()]);
        let probe = sweep.next_probe(now + GRACE, false).unwrap();
        sweep.record_result(probe.port, true);
        if reason == "resolved" {
            sweep.cancel(7, "ufrag", 40000);
        } else {
            sweep.stop_all(reason);
        }
        let plan = &sweep.plans[&40000];
        assert!(
            plan.subnets.is_none(),
            "{reason} must retire subnet and neighbor observations"
        );
        assert!(plan.successful_destinations.is_empty());
        assert!(plan.attempted_destination.is_none());
    }
    let mut sweep = HostSweep::default();
    sweep.start(7, "ufrag".into(), 40000, now);
    sweep.prepare(40000, vec![subnet()]);
    for tick in [now + GRACE, now + GRACE + PACE] {
        let probe = sweep.next_probe(tick, false).unwrap();
        sweep.record_result(probe.port, true);
    }
    let repeat = now + GRACE + PACE + REPEAT_DELAY;
    for tick in [repeat, repeat + PACE] {
        let probe = sweep.next_probe(tick, true).unwrap();
        sweep.record_result(probe.port, true);
    }
    assert!(sweep.plans[&40000].stopped);
    assert_eq!(sweep.deadline(), Some(now + WINDOW));
    sweep.next_probe(now + WINDOW, true);
    let plan = &sweep.plans[&40000];
    assert!(
        plan.subnets.is_none(),
        "completed plans still expire address observations"
    );
    assert!(plan.successful_destinations.is_empty());
    assert!(
        plan.eligible && plan.unresolved,
        "only numeric fresh-restart evidence is retained"
    );
}
fn subnet() -> SweepSubnet {
    SweepSubnet {
        local: address("192.168.2.1:45000"),
        interface_name: "wifi".into(),
        interface_index: 2,
        addresses: vec![Ipv4Addr::new(192, 168, 2, 2), Ipv4Addr::new(192, 168, 2, 3)].into(),
        authorized: vec![Ipv4Addr::new(192, 168, 2, 2), Ipv4Addr::new(192, 168, 2, 3)].into(),
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
    sweep.record_result(40000, true);
    assert!(sweep.next_probe(now + GRACE + PACE, false).is_some());
    sweep.record_result(40000, true);
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
    sweep.record_result(40000, true);
    assert!(
        sweep
            .next_probe(now + GRACE + 2 * PACE + REPEAT_DELAY, true)
            .is_some()
    );
    sweep.record_result(40000, true);
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

#[test]
fn scouts_cluster_once_around_only_authorized_anchors_with_deterministic_ties() {
    let local = address("10.72.0.1:45000");
    let iface = interface("10.72.0.1:0", "255.255.252.0:0", "wifi");
    let mut subnet = SweepSubnet::from_interface(local, &iface, 2, &[]).unwrap();
    let original = subnet.addresses.to_vec();
    subnet.cluster_order(&[
        "10.72.0.254".parse().unwrap(),
        "10.72.0.254".parse().unwrap(),
        "10.72.8.1".parse().unwrap(),
        "8.8.8.8".parse().unwrap(),
        "10.72.0.0".parse().unwrap(),
    ]);
    assert_eq!(
        subnet.addresses[0],
        "10.72.0.254".parse::<Ipv4Addr>().unwrap()
    );
    let cluster = subnet
        .addresses
        .iter()
        .position(|ip| *ip == "10.72.1.2".parse::<Ipv4Addr>().unwrap())
        .unwrap();
    assert!(cluster < 16, "DHCP-near phone should be discovered early");
    assert_eq!(
        subnet.addresses.last().unwrap(),
        &"10.72.3.254".parse::<Ipv4Addr>().unwrap()
    );
    let mut ordered = subnet.addresses.to_vec();
    ordered.sort_unstable();
    assert_eq!(ordered, original);
    let tie = subnet
        .addresses
        .iter()
        .position(|ip| *ip == "10.72.0.253".parse::<Ipv4Addr>().unwrap())
        .unwrap();
    assert_eq!(
        subnet.addresses[tie + 1],
        "10.72.0.255".parse::<Ipv4Addr>().unwrap()
    );
}

#[test]
fn real_ice_probe_never_targets_unknown_neighbors_but_can_reach_newly_resolved_tail() {
    let now = Instant::now();
    let mut sweep = HostSweep::default();
    sweep.start(7, "ufrag".into(), 40000, now);
    let local = address("10.72.0.1:45000");
    let iface = interface("10.72.0.1:0", "255.255.252.0:0", "wifi");
    sweep.prepare(
        40000,
        vec![SweepSubnet::from_interface(local, &iface, 2, &[]).unwrap()],
    );
    assert!(
        sweep
            .next_usable_probe(now + GRACE, false, |_, _| false)
            .is_none()
    );
    let phone: Ipv4Addr = "10.72.3.254".parse().unwrap();
    let probe = sweep
        .next_usable_probe(now + GRACE + PACE, false, |_, ip| ip == phone)
        .unwrap();
    assert_eq!(probe.destination, phone);
    sweep.record_result(probe.port, true);
    assert!(
        sweep
            .next_usable_probe(now + GRACE + 2 * PACE, false, |_, ip| ip == phone)
            .is_none(),
        "at most once per pass"
    );
    assert!(
        !sweep.plans[&40000].repeat,
        "unanswered tail cannot count as a full pass"
    );
}

#[test]
fn unrelated_early_wakes_do_not_slide_the_sweep_deadline() {
    let now = Instant::now();
    let mut sweep = HostSweep::default();
    sweep.start(7, "ufrag".into(), 40000, now);
    sweep.prepare(40000, vec![subnet()]);
    let mut sent = 0;
    for ms in 250..300 {
        let tick = now + Duration::from_millis(ms);
        if let Some(probe) = sweep.next_probe(tick, false) {
            sweep.record_result(probe.port, true);
            sent += 1;
        }
        sweep.defer_tick(tick);
    }
    assert_eq!(
        sent, 2,
        "both initial destinations still progress amid1ms wakes"
    );
    assert!(sweep.deadline().unwrap() <= now + WINDOW);
}

#[test]
fn sparse_discovery_waits_for_settlement_then_completes_using_only_live_neighbors() {
    let now = Instant::now();
    let mut sweep = HostSweep::default();
    sweep.start(7, "ufrag".into(), 40000, now);
    let local = address("10.72.0.1:45000");
    let iface = interface("10.72.0.1:0", "255.255.252.0:0", "wifi");
    sweep.prepare(
        40000,
        vec![SweepSubnet::from_interface(local, &iface, 2, &[]).unwrap()],
    );
    let gateway = "10.72.0.254".parse::<Ipv4Addr>().unwrap();
    let phone = "10.72.3.254".parse::<Ipv4Addr>().unwrap();
    let initial = now + GRACE;
    let probe = sweep
        .next_usable_probe(initial, true, |_, ip| ip == gateway)
        .unwrap();
    sweep.record_result(probe.port, true);
    sweep.settle_discovery(initial + PACE, true, |_, ip| ip == gateway, |_| false);
    assert!(
        !sweep.plans[&40000].repeat,
        "gateway exhaustion cannot finish while scouting"
    );
    let late = now + Duration::from_secs(10);
    let probe = sweep
        .next_usable_probe(late, true, |_, ip| ip == gateway || ip == phone)
        .unwrap();
    assert_eq!(probe.destination, phone);
    sweep.record_result(probe.port, true);
    sweep.settle_discovery(
        late + PACE,
        true,
        |_, ip| ip == gateway || ip == phone,
        |_| true,
    );
    assert!(
        sweep.plans[&40000].repeat,
        "settled dead neighbors need no ICE sends"
    );
    let repeat = late + PACE + REPEAT_DELAY;
    for offset in 0..2 {
        let probe = sweep
            .next_usable_probe(repeat + offset * PACE, true, |_, ip| {
                ip == gateway || ip == phone
            })
            .unwrap();
        sweep.record_result(probe.port, true);
    }
    sweep.settle_discovery(
        repeat + 2 * PACE,
        true,
        |_, ip| ip == gateway || ip == phone,
        |_| true,
    );
    assert!(sweep.plans[&40000].stopped);
    assert_eq!(sweep.sent, 4);
}
