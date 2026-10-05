//! Content-free, fixed-code observations for host probes and isolated scouts.

use serde::Serialize;
use std::sync::Arc;
use tokio::sync::mpsc;
use webrtc::peer_connection::HostCandidateSweepEvent;

pub(super) async fn report(
    remote: Arc<super::remote::RemoteCandidates>,
    mut events: mpsc::Receiver<HostCandidateSweepEvent>,
) {
    while let Some(event) = events.recv().await {
        remote.observe_sweep(event).await;
    }
}

#[derive(Clone, Serialize)]
pub(super) struct Snapshot {
    pub generation: u64,
    pub status: &'static str,
    /// Successful sends from the actual advertised ICE host socket.
    pub addresses_sent: u32,
    /// Real host-socket probe attempts, excluding scout traffic.
    pub addresses_attempted: u32,
    /// Successful UDP discard enqueues from isolated scout sockets this generation.
    pub scout_datagrams_sent: u32,
    /// Paced scout syscall and pressure attempts this generation.
    pub scout_attempted: u32,
    /// Unique successful source/interface/destination scout admissions,
    /// excluding destinations with already usable neighbors.
    pub destinations_scouted: u32,
    /// Latest conservative namespace-wide INCOMPLETE count plus this process's
    /// unobserved successful scout reservations.
    pub neighbors_pending: u32,
    /// Generation maximum of the conservative pending-neighbor count.
    pub neighbors_pending_peak: u32,
    pub reason: Option<&'static str>,
    pub eligible: bool,
    pub eligible_unresolved: u32,
    pub prflx_followed: bool,
}

impl From<HostCandidateSweepEvent> for Snapshot {
    fn from(event: HostCandidateSweepEvent) -> Self {
        Self {
            generation: event.generation,
            status: match event.status {
                "started" | "progress" | "stopped" | "skipped" => event.status,
                _ => "skipped",
            },
            addresses_sent: event.addresses_sent,
            addresses_attempted: event.addresses_attempted,
            scout_datagrams_sent: event.scout_datagrams_sent,
            scout_attempted: event.scout_attempted,
            destinations_scouted: event.destinations_scouted,
            neighbors_pending: event.neighbors_pending,
            neighbors_pending_peak: event.neighbors_pending_peak,
            reason: event.reason.filter(|reason| known_reason(reason)),
            eligible: event.eligible,
            eligible_unresolved: event.eligible_unresolved,
            prflx_followed: event.prflx_followed,
        }
    }
}

fn known_reason(reason: &str) -> bool {
    matches!(
        reason,
        "subnet-too-large"
            | "no-host-socket"
            | "no-on-link-interface"
            | "non-private-subnet"
            | "invalid-netmask"
            | "point-to-point"
            | "unsupported-platform"
            | "port-limit"
            | "packet-limit"
            | "window-expired"
            | "resolved"
            | "direct-selected"
            | "generation-changed"
            | "closed"
            | "send-error"
            | "no-usable-addresses"
            | "completed"
            | "ambiguous-interface"
            | "neighbor-pressure"
            | "neighbor-snapshot-unavailable"
            | "scout-socket-limit"
            | "nat-evidence-missing"
            | "nat-address-mismatch"
            | "interface-scout-cooldown"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(status: &'static str, reason: Option<&'static str>) -> HostCandidateSweepEvent {
        HostCandidateSweepEvent {
            generation: 7,
            status,
            addresses_sent: 253,
            addresses_attempted: 254,
            scout_datagrams_sent: 1004,
            scout_attempted: 1005,
            destinations_scouted: 600,
            neighbors_pending: 25,
            neighbors_pending_peak: 600,
            reason,
            eligible: true,
            eligible_unresolved: 2,
            prflx_followed: true,
        }
    }

    #[test]
    fn sweep_snapshots_preserve_honest_counts_and_fixed_codes() {
        assert_eq!(
            serde_json::to_value(Snapshot::from(event("progress", Some("completed")))).unwrap(),
            json!({"generation": 7, "status": "progress", "addresses_sent": 253,
                "addresses_attempted": 254, "reason": "completed", "eligible": true,
                "eligible_unresolved": 2,
                "scout_datagrams_sent": 1004, "scout_attempted": 1005,
                "destinations_scouted": 600, "neighbors_pending": 25,
                "neighbors_pending_peak": 600,
                "prflx_followed": true})
        );
    }

    #[test]
    fn successful_scout_datagrams_never_inflate_real_host_probe_counts() {
        let mut scout_only = event("progress", None);
        scout_only.addresses_sent = 0;
        scout_only.addresses_attempted = 0;
        let snapshot = serde_json::to_value(Snapshot::from(scout_only)).unwrap();
        assert_eq!(snapshot["addresses_sent"], 0);
        assert_eq!(snapshot["addresses_attempted"], 0);
        assert_eq!(snapshot["scout_datagrams_sent"], 1004);
        assert_eq!(snapshot["scout_attempted"], 1005);
        assert_eq!(snapshot["destinations_scouted"], 600);
        assert_eq!(snapshot["neighbors_pending"], 25);
        assert_eq!(snapshot["neighbors_pending_peak"], 600);
    }

    #[test]
    fn scout_failures_keep_the_existing_fixed_reason_codes() {
        for reason in ["window-expired", "unsupported-platform", "send-error"] {
            let snapshot =
                serde_json::to_value(Snapshot::from(event("stopped", Some(reason)))).unwrap();
            assert_eq!(snapshot["reason"], reason);
        }
    }

    #[test]
    fn coalesced_scout_pauses_keep_their_fixed_reason_codes() {
        for reason in [
            "neighbor-pressure",
            "neighbor-snapshot-unavailable",
            "scout-socket-limit",
        ] {
            let snapshot =
                serde_json::to_value(Snapshot::from(event("progress", Some(reason)))).unwrap();
            assert_eq!(snapshot["reason"], reason);
        }
    }

    #[test]
    fn serialization_preserves_supplied_nat_wait_observation_fields() {
        for (status, reason) in [
            ("progress", "nat-evidence-missing"),
            ("skipped", "nat-evidence-missing"),
            ("progress", "nat-address-mismatch"),
            ("skipped", "nat-address-mismatch"),
        ] {
            let mut blocked = event(status, Some(reason));
            blocked.eligible = false;
            blocked.eligible_unresolved = 0;
            let snapshot = serde_json::to_value(Snapshot::from(blocked)).unwrap();
            assert_eq!(snapshot["reason"], reason);
            assert_eq!(snapshot["status"], status);
            assert_eq!(snapshot["eligible"], false);
            assert_eq!(snapshot["eligible_unresolved"], 0);
            assert_eq!(
                snapshot["scout_datagrams_sent"], 1004,
                "serialization preserves the supplied historical traffic count"
            );
        }
    }

    #[test]
    fn interface_scout_cooldown_preserves_authoritative_eligibility() {
        let snapshot = serde_json::to_value(Snapshot::from(event(
            "progress",
            Some("interface-scout-cooldown"),
        )))
        .unwrap();
        assert_eq!(snapshot["reason"], "interface-scout-cooldown");
        assert_eq!(snapshot["eligible"], true);
        assert_eq!(snapshot["eligible_unresolved"], 2);
    }

    #[test]
    fn unknown_event_codes_never_enter_logs_or_pushes() {
        let snapshot = serde_json::to_value(Snapshot::from(event(
            "ufrag 192.168.68.30:48861",
            Some("73af967b-f3ee-4e8d-b0bd-213da4ec5901.local"),
        )))
        .unwrap();
        assert_eq!(snapshot["status"], "skipped");
        assert_eq!(snapshot["reason"], serde_json::Value::Null);
        assert_eq!(snapshot["generation"], 7);
        assert!(!snapshot.to_string().contains("48861"));
        assert!(!snapshot.to_string().contains("73af967b"));
        let mut keys = snapshot
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "addresses_attempted",
                "addresses_sent",
                "destinations_scouted",
                "eligible",
                "eligible_unresolved",
                "generation",
                "neighbors_pending",
                "neighbors_pending_peak",
                "prflx_followed",
                "reason",
                "scout_attempted",
                "scout_datagrams_sent",
                "status"
            ]
        );
    }
}
