//! The transport ledger: what happened to each client session's wire, as the
//! bridge saw it (`planning/v2/Transport Telemetry Spec.md`).
//!
//! Four events, one line each, content-free — a session id, a word, and for a
//! `carrying` the candidate types the pair won on. The registry records the
//! session's life (`minted`, `fell_back`, `ended`); the peer transport records
//! every path it carries on (`carrying`), the first time and after every ICE
//! restart. Where the events go is a sink behind [`TransportLedger`]: stderr
//! on every bridge, and whatever else is installed beside it.

use std::sync::Arc;
// `Mutex` is the recording sink's alone, and that sink is test-only.
#[cfg(any(test, feature = "testing"))]
use std::sync::Mutex;

/// Which kind of path a peer connection carries on. `Turn` when either end of
/// the nominated pair is a relay candidate — the egress somebody pays for —
/// else `Direct`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportPath {
    Direct,
    Turn,
}

impl TransportPath {
    pub fn as_str(self) -> &'static str {
        match self {
            TransportPath::Direct => "direct",
            TransportPath::Turn => "turn",
        }
    }
}

/// One thing that happened to one session's transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportEvent {
    /// The registry admitted a new session (not a carrier re-attach). It is on
    /// the relay from here.
    Minted,
    /// The session's peer connection reached `Connected`: the first time, and
    /// again after every ICE restart. `detail` is the pair as the peer names it
    /// (`host/relay candidates (TURN, billed)`), for the log line.
    Carrying { path: TransportPath, detail: String },
    /// The session's last DataChannel carrier closed while the session lives:
    /// it is on the relay again.
    FellBack,
    /// The registry ended the session — its client said so, or its last
    /// carrier is gone.
    Ended,
}

impl TransportEvent {
    /// The wire word for this event.
    pub fn name(&self) -> &'static str {
        match self {
            TransportEvent::Minted => "minted",
            TransportEvent::Carrying { .. } => "carrying",
            TransportEvent::FellBack => "fell_back",
            TransportEvent::Ended => "ended",
        }
    }

    pub fn path(&self) -> Option<TransportPath> {
        match self {
            TransportEvent::Carrying { path, .. } => Some(*path),
            _ => None,
        }
    }
}

/// Where transport events go. Called under the registry lock for the session
/// events, so a sink must not block: write, or hand off to a task.
pub trait TransportLedger: Send + Sync {
    fn record(&self, session_id: &str, event: TransportEvent);
}

/// The line a sink that writes text writes: the `rtc:` line the ops checklist
/// greps stays exactly as it was, and the other three say what they are.
pub fn render(session_id: &str, event: &TransportEvent) -> String {
    match event {
        TransportEvent::Minted => format!("transport: session {session_id} minted over the relay"),
        TransportEvent::Carrying { detail, .. } => {
            format!("rtc: session {session_id} carrying over {detail}")
        }
        TransportEvent::FellBack => {
            format!("transport: session {session_id} fell back to the relay")
        }
        TransportEvent::Ended => format!("transport: session {session_id} ended"),
    }
}

/// The ledger every bridge has: the daemon's stderr, one line per event.
pub struct StderrLedger;

impl TransportLedger for StderrLedger {
    fn record(&self, session_id: &str, event: TransportEvent) {
        eprintln!("{}", render(session_id, &event));
    }
}

/// Several sinks fed the same events, in order.
pub struct FanOutLedger(Vec<Arc<dyn TransportLedger>>);

impl FanOutLedger {
    pub fn new(sinks: Vec<Arc<dyn TransportLedger>>) -> Arc<Self> {
        Arc::new(FanOutLedger(sinks))
    }
}

impl TransportLedger for FanOutLedger {
    fn record(&self, session_id: &str, event: TransportEvent) {
        for sink in &self.0 {
            sink.record(session_id, event.clone());
        }
    }
}

/// A sink that remembers, for tests: the trail as `session:event[:path]`.
#[cfg(any(test, feature = "testing"))]
#[derive(Default)]
pub struct RecordingLedger {
    trail: Mutex<Vec<String>>,
}

#[cfg(any(test, feature = "testing"))]
impl RecordingLedger {
    pub fn new() -> Arc<Self> {
        Arc::new(RecordingLedger::default())
    }

    /// Every event so far, oldest first, as `session:name` or
    /// `session:carrying:path`.
    pub fn trail(&self) -> Vec<String> {
        self.trail.lock().unwrap().clone()
    }

    /// The trail of one session, `name` or `carrying:path`.
    pub fn trail_of(&self, session_id: &str) -> Vec<String> {
        let prefix = format!("{session_id}:");
        self.trail()
            .into_iter()
            .filter_map(|line| line.strip_prefix(&prefix).map(str::to_string))
            .collect()
    }
}

#[cfg(any(test, feature = "testing"))]
impl TransportLedger for RecordingLedger {
    fn record(&self, session_id: &str, event: TransportEvent) {
        let line = match event.path() {
            Some(path) => format!("{session_id}:{}:{}", event.name(), path.as_str()),
            None => format!("{session_id}:{}", event.name()),
        };
        self.trail.lock().unwrap().push(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `rtc:` line is the one the ops checklist greps (`TURN, billed`), so
    /// it does not move; the three session events read as what they are.
    #[test]
    fn the_lines_say_what_happened_and_keep_the_rtc_line_the_ops_check_greps() {
        assert_eq!(
            render("sess-1", &TransportEvent::Minted),
            "transport: session sess-1 minted over the relay"
        );
        assert_eq!(
            render(
                "sess-1",
                &TransportEvent::Carrying {
                    path: TransportPath::Turn,
                    detail: "host/relay candidates (TURN, billed)".to_string(),
                }
            ),
            "rtc: session sess-1 carrying over host/relay candidates (TURN, billed)"
        );
        assert_eq!(
            render("sess-1", &TransportEvent::FellBack),
            "transport: session sess-1 fell back to the relay"
        );
        assert_eq!(
            render("sess-1", &TransportEvent::Ended),
            "transport: session sess-1 ended"
        );
    }

    #[test]
    fn the_wire_words_are_the_four_the_api_accepts() {
        assert_eq!(TransportEvent::Minted.name(), "minted");
        assert_eq!(TransportEvent::FellBack.name(), "fell_back");
        assert_eq!(TransportEvent::Ended.name(), "ended");
        let carrying = TransportEvent::Carrying {
            path: TransportPath::Direct,
            detail: String::new(),
        };
        assert_eq!(carrying.name(), "carrying");
        assert_eq!(carrying.path(), Some(TransportPath::Direct));
        assert_eq!(TransportPath::Turn.as_str(), "turn");
    }

    #[test]
    fn a_fan_out_feeds_every_sink_in_order() {
        let first = RecordingLedger::new();
        let second = RecordingLedger::new();
        let ledger = FanOutLedger::new(vec![first.clone(), second.clone()]);
        ledger.record("sess-1", TransportEvent::Minted);
        ledger.record("sess-1", TransportEvent::Ended);
        assert_eq!(first.trail(), vec!["sess-1:minted", "sess-1:ended"]);
        assert_eq!(second.trail(), first.trail());
    }
}
