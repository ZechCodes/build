//! The transport ledger: what happened to each client session's wire, as the
//! bridge saw it (`planning/v2/Transport Telemetry Spec.md`).
//!
//! Four events, one line each, content-free — a session id, a word, and for a
//! `carrying` the candidate types the pair won on. The registry records the
//! session's life (`minted`, `channels_lost`, `ended`); the peer transport records
//! every path it carries on (`carrying`), the first time and after every ICE
//! restart. Where the events go is a sink behind [`TransportLedger`]: stderr
//! on every bridge, and whatever else is installed beside it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

    /// The session sent a `ping`, which the intake answered the moment it
    /// arrived. Not an event: nothing reports it, and most sinks keep no count.
    fn pinged(&self, _session_id: &str) {}
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

    fn pinged(&self, session_id: &str) {
        for sink in &self.0 {
            sink.pinged(session_id);
        }
    }
}

/// How long after the one before a ping may arrive before it counts as a gap:
/// the client's own deadline for an answer, so a gap is a ping the client had
/// already given up on by the time it could have been answered.
pub const PING_GAP: Duration = Duration::from_secs(3);

/// How many sessions a summary tallies at once. Every session ends, and its
/// tally with it; the bound is for a ledger that is somehow never told.
const MAX_TALLIES: usize = 4096;

/// One line per session when it ends, saying what its life came to (#131):
/// how long it lived, how long it carried and over which path, how many times
/// ICE restarted and its channels were lost, and how its pings arrived — how
/// many, and how many came more than [`PING_GAP`] after the one before. The
/// bridge answers a ping the moment it arrives, so a late ping was late on the
/// way in: the path stalled.
pub struct SummaryLedger {
    gap: Duration,
    say: Arc<dyn Fn(&str) + Send + Sync>,
    tallies: Mutex<HashMap<String, Tally>>,
}

struct Tally {
    minted_at: Instant,
    carrying_since: Option<Instant>,
    carried: Duration,
    path: Option<TransportPath>,
    carryings: u32,
    channel_losses: u32,
    pings: u64,
    last_ping: Option<Instant>,
    gaps: u64,
}

impl Tally {
    fn new(now: Instant) -> Tally {
        Tally {
            minted_at: now,
            carrying_since: None,
            carried: Duration::ZERO,
            path: None,
            carryings: 0,
            channel_losses: 0,
            pings: 0,
            last_ping: None,
            gaps: 0,
        }
    }

    fn stop_carrying(&mut self, now: Instant) {
        if let Some(since) = self.carrying_since.take() {
            self.carried += now.duration_since(since);
        }
    }

    fn line(mut self, session_id: &str, gap: Duration, now: Instant) -> String {
        self.stop_carrying(now);
        let carried = match self.path {
            Some(path) => format!("carried {}s over {}", self.carried.as_secs(), path.as_str()),
            None => "carried nothing".to_string(),
        };
        format!(
            "transport: session {session_id} summary: lived {}s, {carried}, {}, {}, {}, {} over {}s",
            now.duration_since(self.minted_at).as_secs(),
            counted(self.carryings.saturating_sub(1).into(), "ICE restart", "ICE restarts"),
            counted(self.channel_losses.into(), "channel loss", "channel losses"),
            counted(self.pings, "ping", "pings"),
            counted(self.gaps, "gap", "gaps"),
            gap.as_secs(),
        )
    }
}

fn counted(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

impl SummaryLedger {
    /// The daemon's: gaps over [`PING_GAP`], lines on stderr.
    pub fn new() -> Arc<SummaryLedger> {
        SummaryLedger::saying_to(PING_GAP, Arc::new(|line: &str| crate::logline::say(line)))
    }

    /// Gaps over `gap`, lines to `say`.
    pub fn saying_to(gap: Duration, say: Arc<dyn Fn(&str) + Send + Sync>) -> Arc<SummaryLedger> {
        Arc::new(SummaryLedger {
            gap,
            say,
            tallies: Mutex::new(HashMap::new()),
        })
    }

    /// The tally of `session_id`, begun now if the summary never saw it minted.
    fn tally<'a>(
        tallies: &'a mut HashMap<String, Tally>,
        session_id: &str,
        now: Instant,
    ) -> Option<&'a mut Tally> {
        if !tallies.contains_key(session_id) && tallies.len() >= MAX_TALLIES {
            return None;
        }
        Some(
            tallies
                .entry(session_id.to_string())
                .or_insert_with(|| Tally::new(now)),
        )
    }
}

impl TransportLedger for SummaryLedger {
    fn record(&self, session_id: &str, event: TransportEvent) {
        let now = Instant::now();
        let mut tallies = self.tallies.lock().unwrap();
        if let TransportEvent::Ended = event {
            let tally = tallies
                .remove(session_id)
                .unwrap_or_else(|| Tally::new(now));
            drop(tallies);
            (self.say)(&tally.line(session_id, self.gap, now));
            return;
        }
        let Some(tally) = SummaryLedger::tally(&mut tallies, session_id, now) else {
            return;
        };
        match event {
            TransportEvent::Carrying { path, .. } => {
                tally.stop_carrying(now);
                tally.carrying_since = Some(now);
                tally.path = Some(path);
                tally.carryings += 1;
            }
            TransportEvent::ChannelsLost => {
                tally.stop_carrying(now);
                tally.channel_losses += 1;
            }
            TransportEvent::Minted | TransportEvent::Ended => {}
        }
    }

    fn pinged(&self, session_id: &str) {
        let now = Instant::now();
        let mut tallies = self.tallies.lock().unwrap();
        let Some(tally) = SummaryLedger::tally(&mut tallies, session_id, now) else {
            return;
        };
        tally.pings += 1;
        if tally
            .last_ping
            .is_some_and(|last| now.duration_since(last) > self.gap)
        {
            tally.gaps += 1;
        }
        tally.last_ping = Some(now);
    }
}

/// A sink that remembers, for tests: every event as it was recorded, read
/// back as the trail `session:event[:path]` or as what a `carrying` said the
/// pair was.
#[cfg(any(test, feature = "testing"))]
#[derive(Default)]
pub struct RecordingLedger {
    events: Mutex<Vec<(String, TransportEvent)>>,
    pings: Mutex<HashMap<String, u64>>,
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

    /// How many pings one session sent.
    pub fn pings_of(&self, session_id: &str) -> u64 {
        self.pings
            .lock()
            .unwrap()
            .get(session_id)
            .copied()
            .unwrap_or(0)
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

    fn pinged(&self, session_id: &str) {
        *self
            .pings
            .lock()
            .unwrap()
            .entry(session_id.to_string())
            .or_default() += 1;
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

    #[test]
    fn a_fan_out_feeds_every_sink_each_ping() {
        let first = RecordingLedger::new();
        let second = RecordingLedger::new();
        let ledger = FanOutLedger::new(vec![first.clone(), second.clone()]);
        ledger.pinged("sess-1");
        ledger.pinged("sess-1");
        assert_eq!(first.pings_of("sess-1"), 2);
        assert_eq!(second.pings_of("sess-1"), 2);
    }

    /// When a session ends, one line says what its life came to: how long it
    /// lived and carried and over which path, how often ICE restarted and the
    /// channels were lost, and how its pings arrived — how many, and how many
    /// came longer than the client's deadline after the one before (#131).
    #[test]
    fn an_ended_session_says_what_its_life_came_to() {
        let said: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = Arc::clone(&said);
        let ledger = SummaryLedger::saying_to(
            Duration::from_millis(50),
            Arc::new(move |line: &str| sink.lock().unwrap().push(line.to_string())),
        );
        let turn = || TransportEvent::Carrying {
            path: TransportPath::Turn,
            detail: "host/relay candidates (TURN, billed)".to_string(),
        };

        ledger.record("sess-1", TransportEvent::Minted);
        ledger.record("sess-1", turn());
        ledger.pinged("sess-1");
        ledger.pinged("sess-1");
        std::thread::sleep(Duration::from_millis(80));
        ledger.pinged("sess-1");
        ledger.record("sess-1", TransportEvent::ChannelsLost);
        ledger.record("sess-1", turn());
        ledger.pinged("sess-1");
        ledger.record("sess-2", TransportEvent::Minted);
        assert!(said.lock().unwrap().is_empty(), "nothing until an end");
        ledger.record("sess-1", TransportEvent::Ended);

        let said = said.lock().unwrap().clone();
        assert_eq!(said.len(), 1, "{said:?}");
        let line = &said[0];
        assert!(
            line.starts_with("transport: session sess-1 summary: lived 0s, carried 0s over turn, ")
                && line.ends_with(", 1 ICE restart, 1 channel loss, 4 pings, 1 gap over 0s"),
            "{line}"
        );
    }

    /// A session that never carried says so, and one the summary never saw
    /// minted (a bridge started mid-session) still gets its line.
    #[test]
    fn a_session_that_never_carried_says_so() {
        let said: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = Arc::clone(&said);
        let ledger = SummaryLedger::saying_to(
            Duration::from_secs(3),
            Arc::new(move |line: &str| sink.lock().unwrap().push(line.to_string())),
        );
        ledger.record("sess-1", TransportEvent::Minted);
        ledger.record("sess-1", TransportEvent::Ended);
        ledger.record("sess-9", TransportEvent::Ended);
        let said = said.lock().unwrap().clone();
        assert_eq!(
            said,
            vec![
                "transport: session sess-1 summary: lived 0s, carried nothing, 0 ICE restarts, 0 channel losses, 0 pings, 0 gaps over 3s",
                "transport: session sess-9 summary: lived 0s, carried nothing, 0 ICE restarts, 0 channel losses, 0 pings, 0 gaps over 3s",
            ]
        );
    }
}
