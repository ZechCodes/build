//! Captures: what the user said, kept before anything decided where it goes.
//!
//! Capture first, route after (spec: UX Redesign Decisions, "Capture and
//! router"). The text the user typed is durable BEFORE the router is asked
//! anything, so a router that never answers, a daemon that dies mid-route, or a
//! reload in the middle of it all costs a routing decision and never the thing
//! the user said.
//!
//! A capture is its own presence on the feed only while it is still unfinished
//! business — unrouted, routing, failed, or holding a question nobody has
//! answered. Once it is routed and quiet, the issue or branch it became is the
//! presence, and the capture leaves the feed rather than doubling it.

use serde::{Deserialize, Serialize};

/// What every capture id starts with, so an id read off the wire says what kind
/// of thing it names without a lookup.
pub const CAPTURE_ID_PREFIX: &str = "capture-";

/// How much of the text a row's title shows before it is cut short.
const TITLE_WIDTH: usize = 80;

/// Where a capture is in its routing life. `Unrouted` is the state it is born
/// in, and the state a restart puts an interrupted route back into.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureState {
    #[default]
    Unrouted,
    Routing,
    Routed,
    Failed,
}

impl CaptureState {
    pub fn as_str(self) -> &'static str {
        match self {
            CaptureState::Unrouted => "unrouted",
            CaptureState::Routing => "routing",
            CaptureState::Routed => "routed",
            CaptureState::Failed => "failed",
        }
    }
}

/// The two things a capture can become. Branch and issue are the only work
/// items, so they are the only destinations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureTarget {
    Issue,
    Branch,
}

impl CaptureTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            CaptureTarget::Issue => "issue",
            CaptureTarget::Branch => "branch",
        }
    }
}

/// Where a capture went, and why. Durable so a misroute stays legible — and
/// reversible — long after the router process is gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRouting {
    pub project_id: String,
    pub kind: CaptureTarget,
    /// The issue id or branch name the capture became.
    pub target_id: String,
    pub routed_at: String,
    /// The router's one line about why it chose this destination. What makes a
    /// route reviewable rather than merely reversible.
    #[serde(default)]
    pub rationale: Option<String>,
}

/// The router's clarifying question, asked only when even the project is
/// ambiguous, and the answer when the user gives one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureQuestion {
    pub text: String,
    pub asked_at: String,
    #[serde(default)]
    pub answer: Option<String>,
}

/// One capture: the durable record of something the user said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capture {
    pub id: String,
    pub text: String,
    pub created_at: String,
    #[serde(default)]
    pub state: CaptureState,
    #[serde(default)]
    pub routing: Option<CaptureRouting>,
    #[serde(default)]
    pub question: Option<CaptureQuestion>,
}

impl Capture {
    /// A brand new capture: the text, and nothing decided about it.
    pub fn new(
        id: impl Into<String>,
        text: impl Into<String>,
        created_at: impl Into<String>,
    ) -> Self {
        Capture {
            id: id.into(),
            text: text.into(),
            created_at: created_at.into(),
            state: CaptureState::Unrouted,
            routing: None,
            question: None,
        }
    }

    /// Whether the router asked something the user has not answered. It is the
    /// one thing a capture can need a human for, so it is the one thing that
    /// keeps a routed capture on the feed.
    pub fn awaiting_answer(&self) -> bool {
        self.question
            .as_ref()
            .is_some_and(|question| question.answer.is_none())
    }

    /// Whether this capture is its own row on the feed.
    ///
    /// Unfinished business is: not yet routed, being routed, routing failed, or
    /// a question waiting on the user. A routed, quiet capture is spoken for by
    /// the issue or branch it became.
    pub fn is_on_the_feed(&self) -> bool {
        match self.state {
            CaptureState::Unrouted | CaptureState::Routing | CaptureState::Failed => true,
            CaptureState::Routed => self.awaiting_answer(),
        }
    }

    /// Why this capture needs the user, as the feed's unread-reason token —
    /// `None` when it needs nobody. A capture has no conversation and no
    /// agents, so what it wants is read off the record rather than off events:
    /// an unanswered question first (the router is asking), then a route that
    /// gave up (nobody is going to try again on its own).
    pub fn unread_reason(&self) -> Option<&'static str> {
        if self.awaiting_answer() {
            return Some("router_question");
        }
        (self.state == CaptureState::Failed).then_some("routing_failed")
    }

    /// The record as boot recovers it: a route that was in flight when the
    /// daemon died goes back to unrouted, because the router process died with
    /// it and the re-fire is what finishes the job. Everything else survives
    /// untouched — a routed capture is a decision, not a session.
    pub fn recovered_at_boot(&self) -> Capture {
        let mut recovered = self.clone();
        if recovered.state == CaptureState::Routing {
            recovered.state = CaptureState::Unrouted;
        }
        recovered
    }

    /// What a row calls this capture: its first non-empty line, cut to a width
    /// a row can show. The full text ships beside it, so the cut is display
    /// only.
    pub fn title(&self) -> String {
        let first_line = self
            .text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("");
        if first_line.chars().count() <= TITLE_WIDTH {
            return first_line.to_string();
        }
        let kept: String = first_line.chars().take(TITLE_WIDTH).collect();
        format!("{}…", kept.trim_end())
    }
}

/// A fresh capture id.
pub fn new_capture_id() -> String {
    format!("{CAPTURE_ID_PREFIX}{}", uuid::Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(state: CaptureState) -> Capture {
        Capture {
            state,
            ..Capture::new("capture-1", "ship the thing", "2026-08-13T10:00:00Z")
        }
    }

    fn question(answer: Option<&str>) -> CaptureQuestion {
        CaptureQuestion {
            text: "which project?".to_string(),
            asked_at: "2026-08-13T10:00:01Z".to_string(),
            answer: answer.map(str::to_string),
        }
    }

    /// A capture is unfinished business until it is routed AND quiet.
    #[test]
    fn unfinished_business_stays_on_the_feed() {
        assert!(capture(CaptureState::Unrouted).is_on_the_feed());
        assert!(capture(CaptureState::Routing).is_on_the_feed());
        assert!(capture(CaptureState::Failed).is_on_the_feed());
        assert!(
            !capture(CaptureState::Routed).is_on_the_feed(),
            "what it became is the presence; two rows for one thing is a lie"
        );
    }

    /// A question nobody has answered keeps the capture on the feed whatever
    /// the router managed to do with it.
    #[test]
    fn an_unanswered_question_keeps_a_routed_capture_on_the_feed() {
        let mut asked = capture(CaptureState::Routed);
        asked.question = Some(question(None));
        assert!(asked.awaiting_answer());
        assert!(asked.is_on_the_feed());

        let mut answered = capture(CaptureState::Routed);
        answered.question = Some(question(Some("the bridge")));
        assert!(!answered.awaiting_answer());
        assert!(!answered.is_on_the_feed());
    }

    /// What a capture wants, and in which order it wants it.
    #[test]
    fn a_capture_asks_for_the_user_only_when_it_needs_one() {
        assert_eq!(capture(CaptureState::Unrouted).unread_reason(), None);
        assert_eq!(capture(CaptureState::Routing).unread_reason(), None);
        assert_eq!(
            capture(CaptureState::Failed).unread_reason(),
            Some("routing_failed")
        );
        assert_eq!(capture(CaptureState::Routed).unread_reason(), None);

        let mut asking = capture(CaptureState::Failed);
        asking.question = Some(question(None));
        assert_eq!(
            asking.unread_reason(),
            Some("router_question"),
            "a question outranks a failure: it is the thing the user can act on"
        );

        let mut answered = capture(CaptureState::Failed);
        answered.question = Some(question(Some("the bridge")));
        assert_eq!(answered.unread_reason(), Some("routing_failed"));
    }

    /// A route that was in flight when the daemon died is re-fired from
    /// scratch, so the record goes back to the state the router picks work up
    /// in. A decision already made is never undone.
    #[test]
    fn boot_puts_an_interrupted_route_back_to_unrouted() {
        assert_eq!(
            capture(CaptureState::Routing).recovered_at_boot().state,
            CaptureState::Unrouted
        );
        for untouched in [
            CaptureState::Unrouted,
            CaptureState::Routed,
            CaptureState::Failed,
        ] {
            let before = capture(untouched);
            assert_eq!(before.recovered_at_boot(), before);
        }
    }

    /// Recovery is a pure reading of the record: it answers with the recovered
    /// copy and leaves the one it was given alone.
    #[test]
    fn boot_recovery_leaves_the_record_it_read_alone() {
        let interrupted = capture(CaptureState::Routing);
        let recovered = interrupted.recovered_at_boot();
        assert_eq!(interrupted.state, CaptureState::Routing);
        assert_eq!(recovered.state, CaptureState::Unrouted);
    }

    #[test]
    fn a_title_is_the_first_line_a_row_can_show() {
        let mut multiline = capture(CaptureState::Unrouted);
        multiline.text = "\n  fix the login redirect  \nand also the toast\n".to_string();
        assert_eq!(multiline.title(), "fix the login redirect");

        let mut long = capture(CaptureState::Unrouted);
        long.text = "x".repeat(TITLE_WIDTH + 10);
        assert_eq!(long.title().chars().count(), TITLE_WIDTH + 1);
        assert!(long.title().ends_with('…'));

        let mut blank = capture(CaptureState::Unrouted);
        blank.text = "   \n\n".to_string();
        assert_eq!(blank.title(), "");
    }

    /// The wire tokens are the record's own spelling: one source of truth for
    /// the state a row shows and the state the store holds.
    #[test]
    fn states_and_targets_serialize_as_their_wire_tokens() {
        for state in [
            CaptureState::Unrouted,
            CaptureState::Routing,
            CaptureState::Routed,
            CaptureState::Failed,
        ] {
            assert_eq!(
                serde_json::to_value(state).unwrap(),
                serde_json::Value::String(state.as_str().to_string())
            );
        }
        for target in [CaptureTarget::Issue, CaptureTarget::Branch] {
            assert_eq!(
                serde_json::to_value(target).unwrap(),
                serde_json::Value::String(target.as_str().to_string())
            );
        }
    }

    /// A record written by an older bridge (text and id only) still loads: the
    /// capture is the text, and everything decided about it is optional.
    #[test]
    fn a_record_loads_with_only_what_a_capture_always_has() {
        let loaded: Capture = serde_json::from_str(
            r#"{"id":"capture-1","text":"ship it","created_at":"2026-08-13T10:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(loaded.state, CaptureState::Unrouted);
        assert!(loaded.routing.is_none());
        assert!(loaded.question.is_none());
    }

    #[test]
    fn a_capture_id_says_what_it_names() {
        let id = new_capture_id();
        assert!(id.starts_with(CAPTURE_ID_PREFIX), "{id}");
        assert_ne!(id, new_capture_id());
    }
}
