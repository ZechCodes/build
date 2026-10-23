//! Bounded ICE pair snapshots without candidate addresses or credentials.

use rtc::statistics::stats::ice_candidate_pair::{
    RTCIceCandidatePairStats, RTCStatsIceCandidatePairState,
};
use sha2::{Digest, Sha256};
use webrtc::peer_connection::{RTCStatsReport, RTCStatsReportEntry};

const MAX_PAIRS: usize = 64;

pub(super) fn lines(report: &RTCStatsReport) -> Vec<String> {
    let mut pairs: Vec<_> = report.candidate_pairs().collect();
    // Keep nominated pairs in detail even when the report exceeds the limit.
    pairs.sort_unstable_by(|left, right| {
        right
            .nominated
            .cmp(&left.nominated)
            .then_with(|| left.stats.id.cmp(&right.stats.id))
    });
    let mut output: Vec<_> = pairs
        .iter()
        .take(MAX_PAIRS)
        .map(|pair| pair_line(report, pair))
        .collect();
    if pairs.len() > MAX_PAIRS {
        output.push(omitted_line(&pairs[MAX_PAIRS..]));
    }
    output
}

fn pair_line(report: &RTCStatsReport, pair: &RTCIceCandidatePairStats) -> String {
    // Candidate IDs are random internal IDs, separate from SDP foundations.
    // Hash them to keep the correlation token compact and independent of text
    // fields in the report. Unlike ordinals it survives newly discovered pairs.
    let digest = format!("{:x}", Sha256::digest(pair.stats.id.as_bytes()));
    format!(
        "pair={} local={} remote={} state={} nominated={} requests_sent={} requests_received={} responses_sent={} responses_received={}",
        &digest[..16],
        candidate_type(report, &pair.local_candidate_id, "RTCLocalIceCandidate_"),
        candidate_type(report, &pair.remote_candidate_id, "RTCRemoteIceCandidate_"),
        pair_state(pair.state),
        pair.nominated,
        pair.requests_sent,
        pair.requests_received,
        pair.responses_sent,
        pair.responses_received,
    )
}

fn candidate_type(report: &RTCStatsReport, id: &str, prefix: &str) -> String {
    match report
        .get(id)
        .or_else(|| report.get(&format!("{prefix}{id}")))
    {
        Some(RTCStatsReportEntry::LocalCandidate(candidate))
        | Some(RTCStatsReportEntry::RemoteCandidate(candidate)) => {
            candidate.candidate_type.to_string()
        }
        _ => "unknown".to_owned(),
    }
}

fn pair_state(state: RTCStatsIceCandidatePairState) -> &'static str {
    match state {
        RTCStatsIceCandidatePairState::Unspecified => "unspecified",
        RTCStatsIceCandidatePairState::Frozen => "frozen",
        RTCStatsIceCandidatePairState::Waiting => "waiting",
        RTCStatsIceCandidatePairState::InProgress => "in-progress",
        RTCStatsIceCandidatePairState::Failed => "failed",
        RTCStatsIceCandidatePairState::Succeeded => "succeeded",
    }
}

fn omitted_line(pairs: &[&RTCIceCandidatePairStats]) -> String {
    let mut totals = [0u64; 4];
    for pair in pairs {
        let counts = [
            pair.requests_sent,
            pair.requests_received,
            pair.responses_sent,
            pair.responses_received,
        ];
        for (total, count) in totals.iter_mut().zip(counts) {
            *total = total.saturating_add(count);
        }
    }
    let failed = pairs
        .iter()
        .filter(|pair| pair.state == RTCStatsIceCandidatePairState::Failed)
        .count();
    let succeeded = pairs
        .iter()
        .filter(|pair| pair.state == RTCStatsIceCandidatePairState::Succeeded)
        .count();
    let nominated = pairs.iter().filter(|pair| pair.nominated).count();
    format!(
        "pairs_omitted={} failed={failed} succeeded={succeeded} nominated={nominated} requests_sent={} requests_received={} responses_sent={} responses_received={}",
        pairs.len(), totals[0], totals[1], totals[2], totals[3],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rtc::peer_connection::configuration::setting_engine::SettingEngine;
    use rtc::peer_connection::transport::{RTCIceCandidateInit, RTCIceTransportState};
    use rtc::peer_connection::{RTCPeerConnection, RTCPeerConnectionBuilder};
    use rtc::sansio::Protocol;
    use rtc::statistics::StatsSelector;
    use std::time::{Duration, Instant};

    fn core(local_candidates: usize) -> (RTCPeerConnection, RTCPeerConnection) {
        let mut settings = SettingEngine::default();
        settings.set_host_acceptance_min_wait(Some(Duration::ZERO));
        let mut browser = RTCPeerConnectionBuilder::new()
            .with_setting_engine(settings.clone())
            .build()
            .unwrap();
        browser.create_data_channel("app", None).unwrap();
        browser
            .add_local_candidate(candidate("192.0.2.2", 48861))
            .unwrap();
        let offer = browser.create_offer(None).unwrap();
        browser.set_local_description(offer.clone()).unwrap();
        let mut bridge = RTCPeerConnectionBuilder::new()
            .with_setting_engine(settings)
            .build()
            .unwrap();
        bridge.set_remote_description(offer).unwrap();
        for port in 5000..5000 + local_candidates as u16 {
            bridge
                .add_local_candidate(candidate("192.0.2.1", port))
                .unwrap();
        }
        let answer = bridge.create_answer(None).unwrap();
        bridge.set_local_description(answer.clone()).unwrap();
        browser.set_remote_description(answer).unwrap();
        (bridge, browser)
    }

    fn candidate(address: &str, port: u16) -> RTCIceCandidateInit {
        RTCIceCandidateInit {
            candidate: format!("candidate:{port} 1 udp 2130706431 {address} {port} typ host"),
            ..Default::default()
        }
    }

    fn transfer_stun(from: &mut RTCPeerConnection, to: &mut RTCPeerConnection) {
        while from.poll_event().is_some() {}
        let packets: Vec<_> = std::iter::from_fn(|| from.poll_write()).collect();
        for mut packet in packets {
            if packet.message.get(4..8) != Some(&[0x21, 0x12, 0xa4, 0x42]) {
                continue;
            }
            std::mem::swap(
                &mut packet.transport.local_addr,
                &mut packet.transport.peer_addr,
            );
            to.handle_read(packet).unwrap();
        }
    }

    fn assert_private(lines: &[String], report: &RTCStatsReport) {
        let text = lines.join("\n");
        assert!(!text.contains("192.0.2."));
        for pair in report.candidate_pairs() {
            assert!(!text.contains(&pair.local_candidate_id));
            assert!(!text.contains(&pair.remote_candidate_id));
        }
        if let Some(transport) = report
            .transport()
            .filter(|transport| !transport.ice_local_username_fragment.is_empty())
        {
            assert!(!text.contains(&transport.ice_local_username_fragment));
        }
    }

    #[test]
    fn selected_pair_snapshot_keeps_real_request_and_response_counters() {
        let (mut bridge, mut browser) = core(1);
        let mut now = Instant::now();
        for _ in 0..10 {
            bridge.handle_timeout(now).unwrap();
            browser.handle_timeout(now).unwrap();
            transfer_stun(&mut bridge, &mut browser);
            transfer_stun(&mut browser, &mut bridge);
            now += Duration::from_millis(250);
        }
        while bridge.poll_event().is_some() {}
        let report = bridge.get_stats(now, StatsSelector::None);
        assert!(matches!(
            report.transport().unwrap().ice_state,
            RTCIceTransportState::Connected | RTCIceTransportState::Completed
        ));
        let pair = report
            .candidate_pairs()
            .find(|pair| pair.nominated)
            .unwrap();
        assert!(pair.requests_sent > 0);
        assert!(pair.requests_received > 0);
        assert!(pair.responses_sent > 0);
        assert!(pair.responses_received > 0);
        let output = lines(&report);
        assert_eq!(output.len(), 1);
        let pair_id = output[0].split_whitespace().next().unwrap();
        let opaque_id = pair_id.strip_prefix("pair=").unwrap();
        assert_eq!(opaque_id.len(), 16);
        assert!(opaque_id.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(output[0], format!(
            "{} local=host remote=host state=succeeded nominated=true requests_sent={} requests_received={} responses_sent={} responses_received={}",
            pair_id, pair.requests_sent, pair.requests_received, pair.responses_sent, pair.responses_received,
        ));
        assert_eq!(lines(&report), output, "pair ids must be deterministic");
        assert_private(&output, &report);
    }

    #[test]
    fn relay_local_pairs_keep_their_type_and_check_outcomes() {
        let report = super::super::checks::unanswered_report();
        let output = lines(&report);
        assert_eq!(output.len(), 3);
        assert_eq!(
            output
                .iter()
                .filter(|line| line.contains("local=relay remote=host"))
                .count(),
            1
        );
        assert!(output.iter().all(|line| line.contains(
            "requests_sent=1 requests_received=0 responses_sent=0 responses_received=0"
        )));
        assert_private(&output, &report);
    }

    #[test]
    fn failed_generation_snapshot_preserves_first_sample_check_counters() {
        let (mut bridge, _) = core(1);
        let now = Instant::now();
        bridge.handle_timeout(now).unwrap();
        bridge
            .handle_timeout(now + Duration::from_secs(61))
            .unwrap();
        while bridge.poll_event().is_some() {}
        let report = bridge.get_stats(now + Duration::from_secs(61), StatsSelector::None);
        let pair = report.candidate_pairs().next().unwrap();
        assert_eq!(pair.requests_sent, 1);
        assert_eq!(pair.responses_received, 0);
        let output = lines(&report);
        assert_eq!(output.len(), 1);
        assert!(output[0].contains("local=host remote=host"));
        assert!(output[0].contains("nominated=false requests_sent=1 requests_received=0 responses_sent=0 responses_received=0"));
        assert_private(&output, &report);
    }

    #[test]
    fn native_generation_snapshot_ignores_stale_failure_trigger_during_checking() {
        use super::super::{pair_snapshot_lines, Trickling};
        use std::sync::atomic::Ordering;

        let signaling = Trickling::default();
        signaling.generation.store(2, Ordering::SeqCst);
        let (mut bridge, _) = core(1);
        let now = Instant::now();
        bridge.handle_timeout(now).unwrap();
        while bridge.poll_event().is_some() {}
        let checking = bridge.get_stats(now, StatsSelector::None);
        assert_eq!(
            checking.transport().unwrap().ice_state,
            RTCIceTransportState::Checking
        );
        assert!(pair_snapshot_lines(&signaling, &checking).is_empty());
        bridge
            .handle_timeout(now + Duration::from_secs(61))
            .unwrap();
        while bridge.poll_event().is_some() {}
        let failed = bridge.get_stats(now + Duration::from_secs(61), StatsSelector::None);
        assert_eq!(
            failed.transport().unwrap().ice_state,
            RTCIceTransportState::Failed
        );
        let output = pair_snapshot_lines(&signaling, &failed);
        assert_eq!(output.len(), 1);
        assert!(output[0].contains("generation=2 event=failed pair="));
    }

    #[test]
    fn pair_identifier_survives_new_pairs_and_later_terminal_snapshot() {
        let (mut bridge, _) = core(1);
        let now = Instant::now();
        bridge.handle_timeout(now).unwrap();
        let first = lines(&bridge.get_stats(now, StatsSelector::None));
        assert_eq!(first.len(), 1);
        let first_id = first[0].split_whitespace().next().unwrap();
        bridge
            .add_local_candidate(candidate("192.0.2.1", 5001))
            .unwrap();
        bridge
            .handle_timeout(now + Duration::from_secs(61))
            .unwrap();
        while bridge.poll_event().is_some() {}
        let terminal = lines(&bridge.get_stats(now + Duration::from_secs(61), StatsSelector::None));
        assert_eq!(terminal.len(), 2);
        assert!(terminal
            .iter()
            .any(|line| line.split_whitespace().next() == Some(first_id)));
    }

    #[test]
    fn large_snapshots_include_bounded_detail_and_omitted_pair_counters() {
        let (mut bridge, _) = core(70);
        let now = Instant::now();
        bridge.handle_timeout(now).unwrap();
        let report = bridge.get_stats(now, StatsSelector::None);
        assert_eq!(report.candidate_pairs().count(), 70);
        let output = lines(&report);
        assert_eq!(output.len(), 65);
        assert!(output[64].starts_with("pairs_omitted=6 "));
        assert!(output[64]
            .contains("requests_sent=6 requests_received=0 responses_sent=0 responses_received=0"));
        assert_private(&output, &report);
    }
}
