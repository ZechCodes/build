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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    /// `minted` | `carrying` | `channels_lost` | `ended`. The api also takes
    /// this event's retired word — the one it had while the relay was still a
    /// data plane — for one release, so a fleet mid-upgrade reports one number
    /// (`buildapp/transport_report.py`).
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

/// How long one report may take, and how many may wait behind it.
///
/// Reports go one at a time, in order, so a request the api never answers
/// held every report after it, and an unbounded queue grew behind it for as
/// long as the api was gone (#131). A report that takes longer than the
/// timeout is dropped like any refused one, and past the queue's bound new
/// events are dropped and counted: the stderr ledger beside this sink is the
/// record of truth either way.
#[derive(Debug, Clone, Copy)]
pub struct ReportBounds {
    pub timeout: Duration,
    pub queue: usize,
}

impl ReportBounds {
    /// Ten seconds a report — the api answers in milliseconds when it
    /// answers — and a few hundred events of a burst of sessions behind it.
    pub const DAEMON: ReportBounds = ReportBounds {
        timeout: Duration::from_secs(10),
        queue: 256,
    };
}

/// The sink that reports to the api. Cheap to hand around: recording an event
/// is one channel send; the sending runs on its own task.
pub struct TransportReporter {
    queue: mpsc::Sender<(String, TransportEvent)>,
    dropped: AtomicU64,
}

impl TransportReporter {
    /// Start reporting to `api_url` as `device_id`. The sender task lives as
    /// long as the reporter does.
    pub fn start(api_url: &str, device_id: &str, identity_private_key_b64: &str) -> Arc<Self> {
        TransportReporter::start_bounded(
            api_url,
            device_id,
            identity_private_key_b64,
            ReportBounds::DAEMON,
        )
    }

    /// The same, within `bounds`.
    pub fn start_bounded(
        api_url: &str,
        device_id: &str,
        identity_private_key_b64: &str,
        bounds: ReportBounds,
    ) -> Arc<Self> {
        let (queue, events) = mpsc::channel(bounds.queue);
        let sender = Sender {
            api_url: api_url.trim_end_matches('/').to_string(),
            device_id: device_id.to_string(),
            identity_private_key_b64: identity_private_key_b64.to_string(),
            client: reqwest::Client::builder()
                .timeout(bounds.timeout)
                .build()
                .unwrap_or_default(),
        };
        tokio::spawn(sender.drain(events));
        Arc::new(TransportReporter {
            queue,
            dropped: AtomicU64::new(0),
        })
    }

    /// How many events a full queue has dropped since the reporter started.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

impl TransportLedger for TransportReporter {
    fn record(&self, session_id: &str, event: TransportEvent) {
        // Credential transitions are bridge diagnostics. The control API's
        // transport contract remains its four existing lifecycle/path events.
        if matches!(event, TransportEvent::IceRestart) {
            return;
        }
        // A reporter whose task is gone drops the event: the stderr ledger
        // beside it still has it, and nothing above waits on this. A full
        // queue drops it too, and says so at every doubling of the count.
        if let Err(mpsc::error::TrySendError::Full(_)) =
            self.queue.try_send((session_id.to_string(), event))
        {
            let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
            if dropped.is_power_of_two() {
                crate::logline::say(format!(
                    "transport: report queue full; {dropped} reports dropped so far"
                ));
            }
        }
    }
}

struct Sender {
    api_url: String,
    device_id: String,
    identity_private_key_b64: String,
    client: reqwest::Client,
}

impl Sender {
    async fn drain(self, mut events: mpsc::Receiver<(String, TransportEvent)>) {
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

    #[test]
    fn restart_events_never_enter_the_control_report_queue() {
        let (queue, mut reports) = mpsc::channel(4);
        let reporter = TransportReporter {
            queue,
            dropped: AtomicU64::new(0),
        };
        reporter.record("sess-1", TransportEvent::IceRestart);
        reporter.record("sess-1", TransportEvent::Minted);
        assert_eq!(reports.try_recv().unwrap().1, TransportEvent::Minted);
        assert!(reports.try_recv().is_err());
        assert_eq!(reporter.dropped(), 0);
    }

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

    /// An api that takes a report and never answers: without a timeout that
    /// one request held the queue forever, and every event after it waited
    /// behind it (#131). A report gives up after its timeout and the next one
    /// goes.
    #[tokio::test]
    async fn a_report_the_api_never_answers_gives_way_to_the_next() {
        let (api_url, requests) = an_api_that_never_answers().await;
        let identity = transport::generate_identity_keypair();
        let reporter = TransportReporter::start_bounded(
            &api_url,
            "dev-1",
            &identity.private_key_b64,
            ReportBounds {
                timeout: Duration::from_millis(200),
                queue: 16,
            },
        );

        reporter.record("sess-1", TransportEvent::Minted);
        reporter.record("sess-2", TransportEvent::Minted);

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while requests.load(Ordering::SeqCst) < 2 && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            requests.load(Ordering::SeqCst),
            2,
            "the second report went once the first gave up"
        );
    }

    /// A queue in front of an api that is not answering fills, and past its
    /// bound the reporter drops and counts rather than growing for as long
    /// as the api is gone.
    #[tokio::test]
    async fn a_full_queue_drops_reports_rather_than_growing() {
        let (api_url, _) = an_api_that_never_answers().await;
        let identity = transport::generate_identity_keypair();
        let reporter = TransportReporter::start_bounded(
            &api_url,
            "dev-1",
            &identity.private_key_b64,
            ReportBounds {
                timeout: Duration::from_secs(60),
                queue: 4,
            },
        );

        for n in 0..50 {
            reporter.record(&format!("sess-{n}"), TransportEvent::Minted);
        }

        let dropped = reporter.dropped();
        assert!(
            (45..=46).contains(&dropped),
            "{dropped} of 50 dropped behind a queue of 4 and one report in flight"
        );
    }

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// An api that reads each request and never writes a byte back. Counts
    /// the reports it was sent.
    async fn an_api_that_never_answers() -> (String, Arc<AtomicUsize>) {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(AtomicUsize::new(0));
        let counting = Arc::clone(&requests);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let counting = Arc::clone(&counting);
                tokio::spawn(async move {
                    let mut buffer = vec![0u8; 16 * 1024];
                    let mut seen = String::new();
                    while let Ok(read) = socket.read(&mut buffer).await {
                        if read == 0 {
                            break;
                        }
                        seen.push_str(&String::from_utf8_lossy(&buffer[..read]));
                        let posts = seen.matches("POST /api/transport/report").count();
                        if posts > 0 {
                            counting.fetch_add(posts, Ordering::SeqCst);
                            seen.clear();
                        }
                    }
                });
            }
        });
        (url, requests)
    }
}
