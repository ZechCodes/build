//! The transport report — bridge → api, one signed request per ledger event
//! (`planning/v2/Transport Telemetry Spec.md` §Wire).
//!
//! A [`TransportReporter`] is a [`TransportLedger`] sink: the registry and the
//! peer transport hand it events under their locks, it queues them, and one
//! task POSTs them to `/api/transport/report` in order. Content-free by
//! construction — a device id, a session id, an event word, a path word, a
//! timestamp — and authenticated the way a push notify is: an Ed25519
//! signature by the device identity key over a challenge that binds every
//! field, which `buildapp/transport_report.py` verifies byte for byte.
//!
//! Best effort. A report the api refuses or cannot be reached for is logged
//! and dropped; the stderr ledger beside this sink is the record of truth on
//! the device, and a session is never held up by the api.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::transport;
use crate::transport_ledger::{TransportEvent, TransportLedger};

/// The path field of an event that carries none.
pub const NO_PATH: &str = "-";

/// The payload POSTed to `/api/transport/report`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportReport {
    pub device_id: String,
    pub session_id: String,
    /// `minted` | `carrying` | `fell_back` | `ended`.
    pub event: String,
    /// `direct` | `turn` for a `carrying`, else [`NO_PATH`].
    pub path: String,
    /// Unix seconds; the api rejects requests outside its freshness window.
    pub timestamp: i64,
    /// Ed25519 signature (padded b64) over [`report_challenge`].
    pub signature_b64: String,
}

/// The canonical message signed for a report. Binds the device, the session,
/// the event, the path and the timestamp, so a captured signature cannot be
/// replayed as a different event. Mirrors `buildapp.transport_report.report_challenge`.
pub fn report_challenge(
    device_id: &str,
    session_id: &str,
    event: &str,
    path: &str,
    timestamp: i64,
) -> String {
    format!("transport.{device_id}.{session_id}.{event}.{path}.{timestamp}")
}

/// Build a fully signed [`TransportReport`] at `timestamp` (unix seconds).
pub fn build_transport_report(
    device_id: &str,
    identity_private_key_b64: &str,
    session_id: &str,
    event: &TransportEvent,
    timestamp: i64,
) -> Result<TransportReport, String> {
    let path = event.path().map(|p| p.as_str()).unwrap_or(NO_PATH);
    let challenge = report_challenge(device_id, session_id, event.name(), path, timestamp);
    let signature_b64 = transport::sign_message_b64(identity_private_key_b64, challenge.as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(TransportReport {
        device_id: device_id.to_string(),
        session_id: session_id.to_string(),
        event: event.name().to_string(),
        path: path.to_string(),
        timestamp,
        signature_b64,
    })
}

/// The sink that reports to the api. Cheap to hand around: recording an event
/// is one channel send; the sending runs on its own task.
pub struct TransportReporter {
    queue: mpsc::UnboundedSender<(String, TransportEvent)>,
}

impl TransportReporter {
    /// Start reporting to `api_url` as `device_id`. The sender task lives as
    /// long as the reporter does.
    pub fn start(api_url: &str, device_id: &str, identity_private_key_b64: &str) -> Arc<Self> {
        let (queue, events) = mpsc::unbounded_channel();
        let sender = Sender {
            api_url: api_url.trim_end_matches('/').to_string(),
            device_id: device_id.to_string(),
            identity_private_key_b64: identity_private_key_b64.to_string(),
            client: reqwest::Client::new(),
        };
        tokio::spawn(sender.drain(events));
        Arc::new(TransportReporter { queue })
    }
}

impl TransportLedger for TransportReporter {
    fn record(&self, session_id: &str, event: TransportEvent) {
        // A reporter whose task is gone drops the event: the stderr ledger
        // beside it still has it, and nothing above waits on this.
        let _ = self.queue.send((session_id.to_string(), event));
    }
}

struct Sender {
    api_url: String,
    device_id: String,
    identity_private_key_b64: String,
    client: reqwest::Client,
}

impl Sender {
    async fn drain(self, mut events: mpsc::UnboundedReceiver<(String, TransportEvent)>) {
        while let Some((session_id, event)) = events.recv().await {
            if let Err(error) = self.send(&session_id, &event).await {
                eprintln!(
                    "transport: report of {} for session {session_id} dropped: {error}",
                    event.name()
                );
            }
        }
    }

    async fn send(&self, session_id: &str, event: &TransportEvent) -> Result<(), String> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs() as i64;
        let report = build_transport_report(
            &self.device_id,
            &self.identity_private_key_b64,
            session_id,
            event,
            timestamp,
        )?;
        let url = format!("{}/api/transport/report", self.api_url);
        let response = self
            .client
            .post(&url)
            .json(&report)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("api rejected the report: {}", response.status()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport_ledger::TransportPath;

    /// The exact string `buildapp.transport_report.report_challenge` produces,
    /// and every field bound into it.
    #[test]
    fn the_challenge_matches_the_api_contract_and_binds_every_field() {
        let base = report_challenge("dev-1", "sess-1", "carrying", "turn", 1_750_000_000);
        assert_eq!(base, "transport.dev-1.sess-1.carrying.turn.1750000000");
        for other in [
            report_challenge("dev-2", "sess-1", "carrying", "turn", 1_750_000_000),
            report_challenge("dev-1", "sess-2", "carrying", "turn", 1_750_000_000),
            report_challenge("dev-1", "sess-1", "ended", "turn", 1_750_000_000),
            report_challenge("dev-1", "sess-1", "carrying", "direct", 1_750_000_000),
            report_challenge("dev-1", "sess-1", "carrying", "turn", 1_750_000_001),
        ] {
            assert_ne!(base, other);
        }
    }

    /// An event with no path signs and sends `-`, so the api's challenge is
    /// one shape for all four events.
    #[test]
    fn a_report_carries_the_event_word_and_a_dash_for_no_path() {
        let identity = transport::generate_identity_keypair();
        let minted = build_transport_report(
            "dev-1",
            &identity.private_key_b64,
            "sess-1",
            &TransportEvent::Minted,
            1_750_000_000,
        )
        .expect("signs");
        assert_eq!(
            (minted.event.as_str(), minted.path.as_str()),
            ("minted", "-")
        );
        let carrying = build_transport_report(
            "dev-1",
            &identity.private_key_b64,
            "sess-1",
            &TransportEvent::Carrying {
                path: TransportPath::Turn,
                detail: "host/relay candidates (TURN, billed)".to_string(),
            },
            1_750_000_000,
        )
        .expect("signs");
        assert_eq!(
            (carrying.event.as_str(), carrying.path.as_str()),
            ("carrying", "turn")
        );
        assert!(transport::verify_message_b64(
            &identity.public_key_b64,
            report_challenge("dev-1", "sess-1", "carrying", "turn", 1_750_000_000).as_bytes(),
            &carrying.signature_b64,
        )
        .is_ok());
    }
}
