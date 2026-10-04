//! Exercise the public setting and RTC stats with real ICE packets between two cores.
use rtc::peer_connection::configuration::setting_engine::SettingEngine;
use rtc::peer_connection::transport::RTCIceCandidateInit;
use rtc::peer_connection::{RTCPeerConnection, RTCPeerConnectionBuilder};
use rtc::sansio::Protocol;
use rtc::shared::TaggedBytesMut;
use rtc::statistics::StatsSelector;
use rtc::statistics::report::{RTCStatsReport, RTCStatsReportEntry};
use rtc::statistics::stats::ice_candidate_pair::{
    RTCIceCandidatePairStats, RTCStatsIceCandidatePairState,
};
use std::time::{Duration, Instant};

const BRIDGE: &str = "candidate:1 1 udp 2130706431 192.0.2.1 5000 typ host";
// TURN's underlying socket is distinct from the host candidate socket.
const RELAY: &str =
    "candidate:2 1 udp 16777215 203.0.113.1 6000 typ relay raddr 192.0.2.2 rport 5002";
const HOST: &str = "candidate:3 1 udp 2130706431 192.0.2.2 5001 typ host";

fn candidate(line: &str) -> RTCIceCandidateInit {
    RTCIceCandidateInit {
        candidate: line.into(),
        ..Default::default()
    }
}

fn core(enabled: bool) -> RTCPeerConnection {
    let mut settings = SettingEngine::default();
    settings.set_relay_acceptance_min_wait(Some(Duration::ZERO));
    settings.set_check_pending_direct_pairs(enabled);
    RTCPeerConnectionBuilder::new()
        .with_setting_engine(settings)
        .build()
        .unwrap()
}

fn written(peer: &mut RTCPeerConnection) -> Vec<TaggedBytesMut> {
    while peer.poll_event().is_some() {}
    std::iter::from_fn(|| peer.poll_write())
        .filter(|packet| packet.message.first().is_some_and(|byte| byte & 0xc0 == 0))
        .collect()
}

fn deliver(peer: &mut RTCPeerConnection, packets: Vec<TaggedBytesMut>) {
    for mut packet in packets {
        std::mem::swap(
            &mut packet.transport.local_addr,
            &mut packet.transport.peer_addr,
        );
        peer.handle_read(packet).unwrap();
    }
}

fn selected_turn(enabled: bool) -> (RTCPeerConnection, RTCPeerConnection, Instant) {
    let mut bridge = core(enabled);
    let mut browser = core(false);
    browser.create_data_channel("app", None).unwrap();
    browser.add_local_candidate(candidate(RELAY)).unwrap();
    let offer = browser.create_offer(None).unwrap();
    browser.set_local_description(offer.clone()).unwrap();
    bridge.set_remote_description(offer).unwrap();
    bridge.add_local_candidate(candidate(BRIDGE)).unwrap();
    let answer = bridge.create_answer(None).unwrap();
    bridge.set_local_description(answer.clone()).unwrap();
    browser.set_remote_description(answer).unwrap();
    browser.add_remote_candidate(candidate(BRIDGE)).unwrap();
    bridge.add_remote_candidate(candidate(RELAY)).unwrap();
    let mut now = Instant::now();
    for _ in 0..10 {
        bridge.handle_timeout(now).unwrap();
        browser.handle_timeout(now).unwrap();
        deliver(&mut browser, written(&mut bridge));
        deliver(&mut bridge, written(&mut browser));
        now += Duration::from_millis(250);
    }
    let report = bridge.get_stats(now, StatsSelector::None);
    let selected = report
        .transport()
        .unwrap()
        .selected_candidate_pair_id
        .as_str();
    assert!(
        report
            .candidate_pairs()
            .any(|pair| pair.stats.id == selected && pair.nominated),
        "fixture must actually select TURN before adding the host candidate"
    );
    browser.add_local_candidate(candidate(HOST)).unwrap();
    bridge.add_remote_candidate(candidate(HOST)).unwrap();
    (bridge, browser, now)
}

fn host_pair(report: &RTCStatsReport) -> &RTCIceCandidatePairStats {
    report.candidate_pairs().find(|pair| {
        let remote_id = format!("RTCRemoteIceCandidate_{}", pair.remote_candidate_id);
        matches!(report.get(&remote_id), Some(RTCStatsReportEntry::RemoteCandidate(remote)) if remote.address.as_deref() == Some("192.0.2.2"))
    }).expect("the unselected host pair must be present in RTC getStats")
}

#[test]
fn rtc_stats_expose_waiting_and_in_progress_unselected_pair_counters() {
    let (mut bridge, _browser, now) = selected_turn(true);
    let before = bridge.get_stats(now, StatsSelector::None);
    let selected = before
        .transport()
        .unwrap()
        .selected_candidate_pair_id
        .clone();
    let host = host_pair(&before);
    assert_eq!(host.state, RTCStatsIceCandidatePairState::Waiting);
    assert_eq!(host.requests_sent, 0);
    assert!(!host.nominated);

    bridge.handle_timeout(now).unwrap();
    let packets = written(&mut bridge);
    assert_eq!(
        packets
            .iter()
            .filter(|packet| packet.transport.peer_addr
                == "192.0.2.2:5001".parse::<std::net::SocketAddr>().unwrap())
            .count(),
        1
    );
    let after = bridge.get_stats(now, StatsSelector::None);
    let host = host_pair(&after);
    assert_eq!(host.state, RTCStatsIceCandidatePairState::InProgress);
    assert_eq!(host.requests_sent, 1);
    assert_eq!(host.responses_received, 0);
    assert!(!host.nominated);
    assert_eq!(
        after.transport().unwrap().selected_candidate_pair_id,
        selected
    );
    assert!(after.candidate_pairs().any(|pair| pair.stats.id == selected
        && pair.nominated
        && pair.state == RTCStatsIceCandidatePairState::Succeeded));
}

#[test]
fn rtc_stats_expose_successful_alternate_response_without_renomination() {
    let (mut bridge, mut browser, mut now) = selected_turn(true);
    let selected = bridge
        .get_stats(now, StatsSelector::None)
        .transport()
        .unwrap()
        .selected_candidate_pair_id
        .clone();
    bridge.handle_timeout(now).unwrap();
    deliver(&mut browser, written(&mut bridge));
    deliver(&mut bridge, written(&mut browser));
    let report = bridge.get_stats(now, StatsSelector::None);
    let host = host_pair(&report);
    assert_eq!(host.requests_sent, 1);
    assert_eq!(host.responses_received, 1);
    assert_eq!(host.state, RTCStatsIceCandidatePairState::Succeeded);
    assert!(!host.nominated);
    for _ in 0..12 {
        now += Duration::from_millis(250);
        bridge.handle_timeout(now).unwrap();
        assert!(
            written(&mut bridge)
                .iter()
                .all(|packet| packet.transport.peer_addr
                    != "192.0.2.2:5001".parse::<std::net::SocketAddr>().unwrap())
        );
    }
    assert_eq!(
        bridge
            .get_stats(now, StatsSelector::None)
            .transport()
            .unwrap()
            .selected_candidate_pair_id,
        selected
    );
}

#[test]
fn rtc_stats_expose_failed_alternate_pair_after_the_normal_check_budget() {
    let (mut bridge, _browser, mut now) = selected_turn(true);
    let selected = bridge
        .get_stats(now, StatsSelector::None)
        .transport()
        .unwrap()
        .selected_candidate_pair_id
        .clone();
    for _ in 0..12 {
        bridge.handle_timeout(now).unwrap();
        written(&mut bridge);
        now += Duration::from_millis(250);
    }
    let report = bridge.get_stats(now, StatsSelector::None);
    let host = host_pair(&report);
    assert_eq!(host.requests_sent, 8);
    assert_eq!(host.responses_received, 0);
    assert_eq!(host.state, RTCStatsIceCandidatePairState::Failed);
    assert!(!host.nominated);
    assert_eq!(
        report.transport().unwrap().selected_candidate_pair_id,
        selected
    );
}

#[test]
fn rtc_setting_defaults_preserve_waiting_pairs_without_probing_them() {
    let (mut bridge, _browser, now) = selected_turn(false);
    bridge.handle_timeout(now).unwrap();
    assert!(
        written(&mut bridge)
            .iter()
            .all(|packet| packet.transport.peer_addr
                != "192.0.2.2:5001".parse::<std::net::SocketAddr>().unwrap())
    );
    let report = bridge.get_stats(now, StatsSelector::None);
    let host = host_pair(&report);
    assert_eq!(host.requests_sent, 0);
    assert_eq!(host.state, RTCStatsIceCandidatePairState::Waiting);
}

#[test]
fn rtc_stats_remove_previous_generation_pairs_on_restart() {
    let (mut bridge, _browser, now) = selected_turn(true);
    let old = bridge.get_stats(now, StatsSelector::None);
    assert_eq!(old.candidate_pairs().count(), 2);
    bridge
        .create_offer(Some(rtc::peer_connection::configuration::RTCOfferOptions {
            ice_restart: true,
        }))
        .unwrap();
    assert_eq!(
        bridge
            .get_stats(now, StatsSelector::None)
            .candidate_pairs()
            .count(),
        0,
        "an earlier generation's success must not survive as current evidence"
    );
}
