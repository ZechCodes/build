//! The transport ledger: what happened to each client session's wire, as the
//! bridge saw it (`planning/v2/Transport Telemetry Spec.md`).
//!
//! Four events, one line each, content-free — a session id, a word, and for a
//! `carrying` the candidate types the pair won on. The registry records the
//! session's life (`minted`, `channels_lost`, `ended`); the peer transport records
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
    /// an ICE restart is under way, or the session is about to end. It is not a
    /// fallback — the relay carries no application traffic to fall back to
    /// (strict P2P transport spec, rule 1).
    ChannelsLost,
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
            TransportEvent::ChannelsLost => "channels_lost",
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
        TransportEvent::ChannelsLost => {
            format!("transport: session {session_id} lost its last channel")
        }
        TransportEvent::Ended => format!("transport: session {session_id} ended"),
    }
}

/// The ledger every bridge has: the daemon's stderr, one line per event.
pub struct StderrLedger;

impl TransportLedger for StderrLedger {
    fn record(&self, session_id: &str, event: TransportEvent) {
        crate::logline::say(render(session_id, &event));
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

/// A sink that remembers, for tests: every event as it was recorded, read
/// back as the trail `session:event[:path]` or as what a `carrying` said the
/// pair was.
#[cfg(any(test, feature = "testing"))]
#[derive(Default)]
pub struct RecordingLedger {
    events: Mutex<Vec<(String, TransportEvent)>>,
}

#[cfg(any(test, feature = "testing"))]
impl RecordingLedger {
    pub fn new() -> Arc<Self> {
        Arc::new(RecordingLedger::default())
    }

    /// Every event so far, oldest first, as `session:name` or
    /// `session:carrying:path`.
    pub fn trail(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|(session_id, event)| match event.path() {
                Some(path) => format!("{session_id}:{}:{}", event.name(), path.as_str()),
                None => format!("{session_id}:{}", event.name()),
            })
            .collect()
    }

    /// What each `carrying` of one session said the pair was — the candidate
    /// types the trail's one word leaves out, and the only place a test can
    /// read which kind of path actually won.
    pub fn carrying_details_of(&self, session_id: &str) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|(recorded, _)| recorded == session_id)
            .filter_map(|(_, event)| match event {
                TransportEvent::Carrying { detail, .. } => Some(detail.clone()),
                _ => None,
            })
            .collect()
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
        self.events
            .lock()
            .unwrap()
            .push((session_id.to_string(), event));
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
            render("sess-1", &TransportEvent::ChannelsLost),
            "transport: session sess-1 lost its last channel"
        );
        assert_eq!(
            render("sess-1", &TransportEvent::Ended),
            "transport: session sess-1 ended"
        );
    }

    #[test]
    fn the_wire_words_are_the_four_the_api_accepts() {
        assert_eq!(TransportEvent::Minted.name(), "minted");
        assert_eq!(TransportEvent::ChannelsLost.name(), "channels_lost");
        assert_eq!(TransportEvent::Ended.name(), "ended");
        let carrying = TransportEvent::Carrying {
            path: TransportPath::Direct,
            detail: String::new(),
        };
        assert_eq!(carrying.name(), "carrying");
        assert_eq!(carrying.path(), Some(TransportPath::Direct));
        assert_eq!(TransportPath::Turn.as_str(), "turn");
    }

    /// A `carrying` is two facts — which kind of path, and which candidate
    /// types it won on — and a test that is about rule 8 needs the second.
    #[test]
    fn the_recording_sink_keeps_the_pair_a_carrying_named() {
        let ledger = RecordingLedger::new();
        ledger.record("sess-1", TransportEvent::Minted);
        ledger.record(
            "sess-1",
            TransportEvent::Carrying {
                path: TransportPath::Direct,
                detail: "host/host candidates".to_string(),
            },
        );
        ledger.record(
            "sess-2",
            TransportEvent::Carrying {
                path: TransportPath::Turn,
                detail: "host/relay candidates (TURN, billed)".to_string(),
            },
        );

        assert_eq!(ledger.trail_of("sess-1"), vec!["minted", "carrying:direct"]);
        assert_eq!(
            ledger.carrying_details_of("sess-1"),
            vec!["host/host candidates"],
            "the detail the trail's one word leaves out"
        );
        assert_eq!(
            ledger.carrying_details_of("sess-2"),
            vec!["host/relay candidates (TURN, billed)"]
        );
        assert!(ledger.carrying_details_of("sess-3").is_empty());
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
