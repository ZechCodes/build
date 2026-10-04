//! Measure connectivity checks without exposing candidate addresses or credentials.

use std::net::{IpAddr, SocketAddr};

use serde::Serialize;
use webrtc::peer_connection::{RTCIceCandidateType, RTCStatsReport, RTCStatsReportEntry};

use rtc::statistics::stats::ice_candidate::RTCIceCandidateStats;

const MAX_HOSTS: usize = 64;

#[derive(Default)]
pub(super) struct DirectChecks {
    hosts: Vec<TrackedHost>,
    measured: bool,
}

struct TrackedHost {
    address: SocketAddr,
    checks: HostChecks,
}

#[derive(Default, Serialize)]
pub(super) struct HostChecks {
    ordinal: usize,
    requests_sent: u64,
    responses_received: u64,
    succeeded: bool,
}

struct PairChecks {
    host: SocketAddr,
    requests_sent: u64,
    responses_received: u64,
}

impl DirectChecks {
    pub(super) fn track_candidate(&mut self, line: &str) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.get(6) != Some(&"typ") || fields.get(7) != Some(&"host") {
            return;
        }
        if let (Ok(address), Ok(port)) = (fields[4].parse::<IpAddr>(), fields[5].parse::<u16>()) {
            self.track(SocketAddr::new(address, port));
        }
    }

    fn track(&mut self, address: SocketAddr) {
        if self.hosts.len() == MAX_HOSTS || self.hosts.iter().any(|host| host.address == address) {
            return;
        }
        self.hosts.push(TrackedHost {
            address,
            checks: HostChecks {
                ordinal: self.hosts.len() + 1,
                ..Default::default()
            },
        });
    }

    pub(super) fn observe(&mut self, report: &RTCStatsReport) {
        let pairs = report.candidate_pairs().filter_map(|pair| {
            let local = candidate(report, &pair.local_candidate_id, "RTCLocalIceCandidate_")?;
            let remote = candidate(report, &pair.remote_candidate_id, "RTCRemoteIceCandidate_")?;
            if local.candidate_type == RTCIceCandidateType::Relay
                || remote.candidate_type != RTCIceCandidateType::Host
            {
                return None;
            }
            Some(PairChecks {
                host: SocketAddr::new(remote.address.as_deref()?.parse().ok()?, remote.port),
                requests_sent: pair.requests_sent,
                responses_received: pair.responses_received,
            })
        });
        self.observe_pairs(pairs);
    }

    fn observe_pairs(&mut self, pairs: impl Iterator<Item = PairChecks>) {
        let mut totals: [HostChecks; MAX_HOSTS] = std::array::from_fn(|_| HostChecks::default());
        for pair in pairs {
            self.track(pair.host);
            let Some(index) = self.hosts.iter().position(|host| host.address == pair.host) else {
                continue;
            };
            totals[index].requests_sent = totals[index]
                .requests_sent
                .saturating_add(pair.requests_sent);
            totals[index].responses_received = totals[index]
                .responses_received
                .saturating_add(pair.responses_received);
            totals[index].succeeded |= pair.responses_received > 0;
        }
        for (host, total) in self.hosts.iter_mut().zip(totals) {
            host.checks.requests_sent = host.checks.requests_sent.max(total.requests_sent);
            host.checks.responses_received =
                host.checks.responses_received.max(total.responses_received);
            host.checks.succeeded |= total.succeeded;
        }
        self.measured = true;
    }

    pub(super) fn reason(&self) -> Option<&'static str> {
        if self.hosts.is_empty() {
            return None;
        }
        if !self.measured {
            return Some("direct-checks-pending");
        }
        if self.hosts.iter().any(|host| host.checks.succeeded) {
            return Some("direct-checks-succeeded");
        }
        if self.hosts.iter().all(|host| host.checks.requests_sent == 0) {
            return Some("direct-checks-not-sent");
        }
        Some("direct-checks-no-reply")
    }

    pub(super) fn hosts(&self) -> Vec<&HostChecks> {
        self.hosts.iter().map(|host| &host.checks).collect()
    }
}

#[cfg(test)]
pub(super) fn unanswered_report() -> RTCStatsReport {
    use rtc::peer_connection::transport::RTCIceCandidateInit;
    use rtc::peer_connection::RTCPeerConnectionBuilder;
    use rtc::sansio::Protocol;
    use rtc::statistics::StatsSelector;
    use std::time::Instant;

    let mut browser = RTCPeerConnectionBuilder::new().build().unwrap();
    browser.create_data_channel("app", None).unwrap();
    browser
        .add_local_candidate(RTCIceCandidateInit {
            candidate: "candidate:1 1 udp 2130706431 192.0.2.2 48861 typ host".into(),
            ..Default::default()
        })
        .unwrap();
    let offer = browser.create_offer(None).unwrap();
    browser.set_local_description(offer.clone()).unwrap();
    let mut bridge = RTCPeerConnectionBuilder::new().build().unwrap();
    bridge.set_remote_description(offer).unwrap();
    for line in [
        "candidate:2 1 udp 2130706431 192.0.2.1 5000 typ host",
        "candidate:3 1 udp 2130706431 192.0.2.1 5001 typ host",
        "candidate:4 1 udp 16777215 203.0.113.1 6000 typ relay raddr 192.0.2.1 rport 5002",
    ] {
        bridge
            .add_local_candidate(RTCIceCandidateInit {
                candidate: line.into(),
                ..Default::default()
            })
            .unwrap();
    }
    let answer = bridge.create_answer(None).unwrap();
    bridge.set_local_description(answer).unwrap();
    let now = Instant::now();
    bridge.handle_timeout(now).unwrap();
    bridge.get_stats(now, StatsSelector::None)
}

fn candidate<'a>(
    report: &'a RTCStatsReport,
    id: &str,
    prefix: &str,
) -> Option<&'a RTCIceCandidateStats> {
    let entry = report
        .get(id)
        .or_else(|| report.get(&format!("{prefix}{id}")))?;
    match entry {
        RTCStatsReportEntry::LocalCandidate(candidate)
        | RTCStatsReportEntry::RemoteCandidate(candidate) => Some(candidate),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pair(host: &str, requests: u64, responses: u64) -> PairChecks {
        PairChecks {
            host: host.parse().unwrap(),
            requests_sent: requests,
            responses_received: responses,
        }
    }

    #[test]
    fn multiple_local_pairs_count_once_per_host_and_keep_individual_host_outcomes() {
        let mut checks = DirectChecks::default();
        checks.track_candidate("candidate:1 1 udp 123 192.0.2.2 5000 typ host");
        checks.track_candidate("candidate:2 1 udp 123 192.0.2.3 5001 typ host");
        assert_eq!(checks.reason(), Some("direct-checks-pending"));
        checks.observe_pairs(
            [
                pair("192.0.2.2:5000", 3, 1),
                pair("192.0.2.2:5000", 8, 0),
                pair("192.0.2.3:5001", 8, 0),
            ]
            .into_iter(),
        );
        let expected = json!([
            {"ordinal": 1, "requests_sent": 11, "responses_received": 1, "succeeded": true},
            {"ordinal": 2, "requests_sent": 8, "responses_received": 0, "succeeded": false},
        ]);
        assert_eq!(serde_json::to_value(checks.hosts()).unwrap(), expected);
        assert_eq!(checks.reason(), Some("direct-checks-succeeded"));
        checks.observe_pairs(
            [
                pair("192.0.2.2:5000", 3, 1),
                pair("192.0.2.2:5000", 8, 0),
                pair("192.0.2.3:5001", 8, 0),
            ]
            .into_iter(),
        );
        assert_eq!(
            serde_json::to_value(checks.hosts()).unwrap(),
            expected,
            "sampling the same counters twice must not double them"
        );
    }

    #[test]
    fn observed_zero_checks_and_unanswered_checks_have_distinct_reasons() {
        let mut checks = DirectChecks::default();
        checks.track_candidate("candidate:1 1 udp 123 192.0.2.2 5000 typ host");
        checks.observe_pairs(std::iter::empty());
        assert_eq!(checks.reason(), Some("direct-checks-not-sent"));
        checks.observe_pairs([pair("192.0.2.2:5000", 2, 0)].into_iter());
        assert_eq!(checks.reason(), Some("direct-checks-no-reply"));
    }

    #[test]
    fn hosts_are_deduplicated_bounded_and_never_serialized_as_addresses() {
        let mut checks = DirectChecks::default();
        for port in 1..=100 {
            checks.track_candidate(&format!("candidate:1 1 udp 123 192.0.2.2 {port} typ host"));
        }
        checks.track_candidate("candidate:1 1 udp 123 192.0.2.2 1 typ host");
        checks.track_candidate("candidate:1 1 udp 123 example.local 5 typ host");
        checks.track_candidate("candidate:1 1 udp 123 192.0.2.4 5 typ relay");
        assert_eq!(checks.hosts().len(), MAX_HOSTS);
        let value = serde_json::to_value(checks.hosts()).unwrap().to_string();
        assert!(!value.contains("192.0.2."));
        assert!(!value.contains("example.local"));
    }

    #[test]
    fn real_rtc_counters_join_candidate_ids_and_exclude_local_turn_pairs() {
        let report = unanswered_report();
        assert_eq!(
            report.candidate_pairs().count(),
            3,
            "the fixture has two direct local pairs and one TURN local pair"
        );
        assert!(report.candidate_pairs().all(|pair| pair.requests_sent == 1));
        let mut checks = DirectChecks::default();
        checks.observe(&report);
        assert_eq!(
            serde_json::to_value(checks.hosts()).unwrap(),
            json!([
                {"ordinal": 1, "requests_sent": 2, "responses_received": 0, "succeeded": false}
            ])
        );
        assert_eq!(checks.reason(), Some("direct-checks-no-reply"));
    }
}
