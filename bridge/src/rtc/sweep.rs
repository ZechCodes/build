//! Content-free, fixed-code observations from the bounded host-socket sweep.

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
    pub addresses_sent: u32,
    pub addresses_attempted: u32,
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
                "prflx_followed": true})
        );
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
    }
}
