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
    /// Destinations this capture was sent to and then moved off. A misroute
    /// whose artifact could not be taken back — a branch an agent already
    /// worked — is kept, and this is what keeps it reachable from the capture
    /// instead of orphaned beside it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rerouted_from: Vec<CaptureRouting>,
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
            rerouted_from: Vec::new(),
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

    /// The record as the router picks it up: in the router's hands. Re-firing a
    /// capture that already failed, or one holding a question that has now been
    /// answered, is the same transition — the router is looking at it again.
    pub fn routing_started(&self) -> Capture {
        Capture {
            state: CaptureState::Routing,
            ..self.clone()
        }
    }

    /// The record once the router reached a destination. A route is a decision,
    /// so it is terminal for the capture: what it became is the presence now.
    ///
    /// Routing something that was already routed is a reroute, and the
    /// destination it is moving off is remembered rather than overwritten — the
    /// artifact there may be work nobody can take back.
    pub fn routed_to(&self, routing: CaptureRouting) -> Capture {
        let mut rerouted_from = self.rerouted_from.clone();
        if let Some(previous) = &self.routing {
            rerouted_from.push(previous.clone());
        }
        Capture {
            state: CaptureState::Routed,
            routing: Some(routing),
            rerouted_from,
            ..self.clone()
        }
    }

    /// The record once the router asked the user something.
    ///
    /// The state goes BACK to unrouted, because a question is not a route:
    /// nothing has been decided, and the answer is what lets the router decide.
    /// The unanswered question is what keeps the row on the feed and what it
    /// says it needs.
    pub fn asked(&self, question: CaptureQuestion) -> Capture {
        Capture {
            state: CaptureState::Unrouted,
            question: Some(question),
            ..self.clone()
        }
    }

    /// The record once the user answered. Refused when nothing asked: an answer
    /// to no question is a message with nowhere to go.
    pub fn answered(&self, text: impl Into<String>) -> Result<Capture, String> {
        let question = self
            .question
            .as_ref()
            .ok_or("the router has not asked anything about this capture")?;
        if question.answer.is_some() {
            return Err("that question has already been answered".to_string());
        }
        Ok(Capture {
            question: Some(CaptureQuestion {
                answer: Some(text.into()),
                ..question.clone()
            }),
            ..self.clone()
        })
    }

    /// The record once the router gave up — or stopped without deciding, which
    /// is the same thing from the user's side. A capture that already reached a
    /// destination is left alone: a router exiting after a route is a router
    /// that finished.
    pub fn routing_failed(&self) -> Capture {
        if self.state == CaptureState::Routed {
            return self.clone();
        }
        Capture {
            state: CaptureState::Failed,
            ..self.clone()
        }
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

    /// Every transition answers with a new record and leaves the one it read
    /// alone: the store owns when a change is written, not these.
    #[test]
    fn the_routing_transitions_are_readings_of_the_record() {
        let unrouted = capture(CaptureState::Unrouted);

        let routing = unrouted.routing_started();
        assert_eq!(routing.state, CaptureState::Routing);
        assert_eq!(unrouted.state, CaptureState::Unrouted);

        let routed = routing.routed_to(CaptureRouting {
            project_id: "proj-1".to_string(),
            kind: CaptureTarget::Issue,
            target_id: "plan-7".to_string(),
            routed_at: "2026-08-13T10:00:05Z".to_string(),
            rationale: Some("no branch names this work".to_string()),
        });
        assert_eq!(routed.state, CaptureState::Routed);
        assert_eq!(routed.routing.as_ref().unwrap().target_id, "plan-7");
        assert!(routed.rerouted_from.is_empty());
        assert_eq!(routing.state, CaptureState::Routing);
    }

    /// A reroute never overwrites where the capture has already been: the
    /// artifact there may be work, and work nobody can point at is work lost.
    #[test]
    fn rerouting_remembers_the_destination_it_moved_off() {
        let routing = |target: &str| CaptureRouting {
            project_id: "proj-1".to_string(),
            kind: CaptureTarget::Issue,
            target_id: target.to_string(),
            routed_at: "2026-08-13T10:00:05Z".to_string(),
            rationale: None,
        };
        let first = capture(CaptureState::Unrouted).routed_to(routing("plan-7"));
        let second = first.routed_to(routing("plan-8"));
        let third = second.routed_to(routing("plan-9"));

        assert_eq!(third.routing.as_ref().unwrap().target_id, "plan-9");
        let left_behind: Vec<&str> = third
            .rerouted_from
            .iter()
            .map(|routing| routing.target_id.as_str())
            .collect();
        assert_eq!(left_behind, vec!["plan-7", "plan-8"]);
    }

    /// A question is not a route. The record goes back to where the router
    /// picks work up, because the answer is what lets it decide at all.
    #[test]
    fn asking_returns_the_capture_to_unrouted_and_keeps_it_on_the_feed() {
        let asking = capture(CaptureState::Routing).asked(question(None));
        assert_eq!(asking.state, CaptureState::Unrouted);
        assert!(asking.awaiting_answer());
        assert!(asking.is_on_the_feed());
        assert_eq!(asking.unread_reason(), Some("router_question"));

        let answered = asking.answered("the bridge").unwrap();
        assert!(!answered.awaiting_answer());
        assert_eq!(
            answered.question.as_ref().unwrap().answer.as_deref(),
            Some("the bridge")
        );
        assert_eq!(
            answered.question.as_ref().unwrap().text,
            asking.question.as_ref().unwrap().text,
            "answering never rewrites the question"
        );
    }

    #[test]
    fn an_answer_needs_an_unanswered_question_to_answer() {
        assert!(capture(CaptureState::Unrouted).answered("x").is_err());
        let answered = capture(CaptureState::Unrouted).asked(question(Some("the bridge")));
        assert!(answered.answered("again").is_err());
    }

    /// A router that stopped without deciding leaves the capture needing the
    /// user; one that stopped after deciding leaves a decision alone.
    #[test]
    fn failing_marks_only_a_capture_that_never_reached_a_destination() {
        for undecided in [
            CaptureState::Unrouted,
            CaptureState::Routing,
            CaptureState::Failed,
        ] {
            assert_eq!(
                capture(undecided).routing_failed().state,
                CaptureState::Failed
            );
        }
        assert_eq!(
            capture(CaptureState::Routed).routing_failed().state,
            CaptureState::Routed,
            "a router exiting after a route is a router that finished"
        );
    }

    #[test]
    fn a_capture_id_says_what_it_names() {
        let id = new_capture_id();
        assert!(id.starts_with(CAPTURE_ID_PREFIX), "{id}");
        assert_ne!(id, new_capture_id());
    }
}
