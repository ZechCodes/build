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

/// How many options a router may offer beside its question. Three concrete
/// choices is an offer; a fourth is a menu, and a menu is the friction capture
/// exists to remove.
pub const MAX_CAPTURE_OPTIONS: usize = 3;

/// One concrete choice offered beside the router's question, as Build stores it.
///
/// The label is what the user taps. The route hint is the destination that
/// label stood for, in the same terms a reroute names one — enough that the
/// answer says where to go and not only that the user chose something.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureOption {
    /// Stable within this question, and assigned here rather than by the
    /// router: an id the user taps is worth something only if it names one
    /// option and always the same one.
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub kind: Option<CaptureTarget>,
    /// The branch a branch option continues, when the router named one.
    #[serde(default)]
    pub branch: Option<String>,
}

impl CaptureOption {
    /// What the router reads when the user taps this option: the label they
    /// saw, and the destination it stood for spelled out in the terms the
    /// router routes in. An option with no route hint is only what it says.
    pub fn as_answer(&self) -> String {
        match self.route_phrase() {
            Some(phrase) => format!("{} — {phrase}", self.label),
            None => self.label.clone(),
        }
    }

    /// The destination clause of the answer, or `None` when this option is a
    /// label and nothing more.
    fn route_phrase(&self) -> Option<String> {
        let mut phrase = match self.project_id.as_deref() {
            Some(project_id) => format!("route this to project {project_id}"),
            None if self.kind.is_none() => return None,
            None => "route this".to_string(),
        };
        match self.kind {
            Some(CaptureTarget::Issue) => phrase.push_str(" as an issue"),
            Some(CaptureTarget::Branch) => phrase.push_str(" as a branch"),
            None => {}
        }
        if let Some(branch) = &self.branch {
            phrase.push_str(&format!(", on the branch {branch}"));
        }
        Some(phrase)
    }
}

/// An option as the router offers it: everything but the id, which is Build's
/// to give.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureOptionDraft {
    pub label: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub kind: Option<CaptureTarget>,
    #[serde(default)]
    pub branch: Option<String>,
}

impl CaptureOptionDraft {
    /// This draft as the `position`th option offered (1-based), with whitespace
    /// off its text.
    pub fn numbered(&self, position: usize) -> CaptureOption {
        let trimmed = |value: &Option<String>| {
            value
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let branch = trimmed(&self.branch);
        CaptureOption {
            id: format!("option-{position}"),
            label: self.label.trim().to_string(),
            project_id: trimmed(&self.project_id),
            // A branch name says the option is a branch, so the router never
            // has to say it twice.
            kind: self.kind.or(branch.as_ref().map(|_| CaptureTarget::Branch)),
            branch,
        }
    }
}

/// The router's offer, numbered and checked. Refused when there are too many,
/// when one carries no label, or when one names a branch it could not be on.
pub fn numbered_options(drafts: &[CaptureOptionDraft]) -> Result<Vec<CaptureOption>, String> {
    if drafts.len() > MAX_CAPTURE_OPTIONS {
        return Err(format!(
            "at most {MAX_CAPTURE_OPTIONS} options can be offered beside a question; {} were",
            drafts.len()
        ));
    }
    drafts
        .iter()
        .enumerate()
        .map(|(index, draft)| {
            let option = draft.numbered(index + 1);
            if option.label.is_empty() {
                return Err("an option with no label is nothing the user can choose".to_string());
            }
            if option.kind == Some(CaptureTarget::Issue) && option.branch.is_some() {
                return Err(format!(
                    "option {:?} names a branch and an issue; an issue has no branch to be on",
                    option.label
                ));
            }
            Ok(option)
        })
        .collect()
}

/// The router's clarifying question, asked only when even the project is
/// ambiguous, and the answer when the user gives one.
///
/// The question may carry up to three concrete choices. They are an offer and
/// never a requirement: typing an answer, and abandoning the capture, are
/// available whatever the router thought of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureQuestion {
    pub text: String,
    pub asked_at: String,
    #[serde(default)]
    pub answer: Option<String>,
    #[serde(default)]
    pub options: Vec<CaptureOption>,
    /// Which option the user tapped, when they tapped one rather than typed.
    #[serde(default)]
    pub chosen_option_id: Option<String>,
}

impl CaptureQuestion {
    /// A question with nothing answered and nothing offered.
    pub fn new(text: impl Into<String>, asked_at: impl Into<String>) -> CaptureQuestion {
        CaptureQuestion {
            text: text.into(),
            asked_at: asked_at.into(),
            answer: None,
            options: Vec::new(),
            chosen_option_id: None,
        }
    }

    /// The option this question offered under `option_id`.
    pub fn option(&self, option_id: &str) -> Option<&CaptureOption> {
        self.options.iter().find(|option| option.id == option_id)
    }

    /// The option at `index`, counting from the first one offered.
    pub fn option_at(&self, index: usize) -> Option<&CaptureOption> {
        self.options.get(index)
    }
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
    /// The latest non-blocking router progress line. Unlike `question`, this
    /// never asks for an answer or changes the routing state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<String>,
    /// Where this capture sits in the inbox: the moment it was said. Every
    /// entity carries one, and this is the oldest of them all — the issue or
    /// branch the capture becomes inherits it, so what the user said and the
    /// work it turned into hold ONE place in the list rather than two.
    ///
    /// It never moves. A capture is unfinished business for as long as it takes
    /// the router to decide, which is seconds; anything that lives long enough
    /// to be picked back up is the work it became, and that moves its own.
    /// `None` on a record written before anchors existed — it reads as
    /// `created_at`, which is what it would have been seeded to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_at: Option<String>,
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
        let created_at = created_at.into();
        Capture {
            id: id.into(),
            text: text.into(),
            anchor_at: Some(created_at.clone()),
            created_at,
            state: CaptureState::Unrouted,
            routing: None,
            question: None,
            progress: None,
            rerouted_from: Vec::new(),
        }
    }

    /// This capture's place in the inbox, for a record from before anchors as
    /// much as for one seeded at creation.
    pub fn anchor(&self) -> &str {
        self.anchor_at.as_deref().unwrap_or(&self.created_at)
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
            recovered.progress = None;
        }
        // A capture taken before anchors existed is anchored where it was said,
        // durably, so the work it becomes can inherit it.
        if recovered.anchor_at.is_none() {
            recovered.anchor_at = Some(recovered.created_at.clone());
        }
        recovered
    }

    /// The record as the router picks it up: in the router's hands. Re-firing a
    /// capture that already failed, or one holding a question that has now been
    /// answered, is the same transition — the router is looking at it again.
    pub fn routing_started(&self) -> Capture {
        Capture {
            state: CaptureState::Routing,
            progress: None,
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
            progress: None,
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
            progress: None,
            ..self.clone()
        }
    }

    pub fn working(&self, message: impl Into<String>) -> Capture {
        Capture {
            progress: Some(message.into()),
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

    /// The record once the user tapped one of the options the router offered.
    ///
    /// The choice reaches the router as words — the label, and the destination
    /// it stood for — because the router decides in words and a tap it cannot
    /// read is a tap that decides nothing. Which option was tapped is kept
    /// beside the answer, so the record says what the user was shown and what
    /// of it they picked.
    pub fn answered_with_option(&self, option_id: &str) -> Result<Capture, String> {
        let option = self
            .question
            .as_ref()
            .and_then(|question| question.option(option_id))
            .ok_or_else(|| format!("no option {option_id:?} was offered with that question"))?;
        let chosen_option_id = option.id.clone();
        let mut answered = self.answered(option.as_answer())?;
        if let Some(question) = answered.question.as_mut() {
            question.chosen_option_id = Some(chosen_option_id);
        }
        Ok(answered)
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
            progress: None,
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

/// Whether `id` names a capture — the owner a router's turn is queued under.
pub fn is_capture_id(id: &str) -> bool {
    id.starts_with(CAPTURE_ID_PREFIX)
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
            answer: answer.map(str::to_string),
            ..CaptureQuestion::new("which project?", "2026-08-13T10:00:01Z")
        }
    }

    fn draft(label: &str) -> CaptureOptionDraft {
        CaptureOptionDraft {
            label: label.to_string(),
            ..CaptureOptionDraft::default()
        }
    }

    /// A question with the two concrete choices the router thought of, and the
    /// free-form answer still available beside them.
    fn asking_with_options() -> Capture {
        let options = numbered_options(&[
            CaptureOptionDraft {
                label: "File as an issue on Build".to_string(),
                project_id: Some("proj-build".to_string()),
                kind: Some(CaptureTarget::Issue),
                branch: None,
            },
            CaptureOptionDraft {
                label: "New branch on Do".to_string(),
                project_id: Some("proj-do".to_string()),
                kind: Some(CaptureTarget::Branch),
                branch: None,
            },
        ])
        .unwrap();
        capture(CaptureState::Routing).asked(CaptureQuestion {
            options,
            ..CaptureQuestion::new("which project?", "2026-08-13T10:00:01Z")
        })
    }

    /// The capture's place in the inbox is the moment it was said, and nothing
    /// the router does to it moves that.
    #[test]
    fn a_capture_is_anchored_where_it_was_said() {
        let said = capture(CaptureState::Unrouted);
        assert_eq!(said.anchor(), "2026-08-13T10:00:00Z");

        let asked = said.asked(question(None));
        let answered = asked.answered("the Build one").unwrap();
        let routed = answered.routed_to(CaptureRouting {
            project_id: "proj-1".to_string(),
            kind: CaptureTarget::Issue,
            target_id: "plan-1".to_string(),
            routed_at: "2026-08-14T09:00:00Z".to_string(),
            rationale: None,
        });
        assert_eq!(routed.anchor(), "2026-08-13T10:00:00Z");
    }

    /// Anchors are new; every capture already on disk predates them and reads
    /// as anchored where it was said.
    #[test]
    fn a_capture_written_before_anchors_reads_as_anchored_at_creation() {
        let stored = serde_json::json!({
            "id": "capture-old",
            "text": "ship the thing",
            "created_at": "2026-08-01T08:00:00Z",
        });
        let capture: Capture = serde_json::from_value(stored).expect("an old capture loads");
        assert!(capture.anchor_at.is_none());
        assert_eq!(capture.anchor(), "2026-08-01T08:00:00Z");
    }

    /// A question with no options is the question this surface started with:
    /// options are an offer, never a requirement.
    #[test]
    fn a_question_may_offer_no_options_at_all() {
        assert_eq!(numbered_options(&[]).unwrap(), Vec::new());
        let asked = capture(CaptureState::Routing).asked(question(None));
        assert!(asked.question.as_ref().unwrap().options.is_empty());
        assert!(asked.awaiting_answer());
    }

    /// Options are numbered by Build, not by the router: an id the user taps is
    /// only worth anything if it names one option and always the same one.
    #[test]
    fn options_are_numbered_in_the_order_the_router_offered_them() {
        let options = numbered_options(&[draft("first"), draft("second"), draft("third")]).unwrap();
        let ids: Vec<&str> = options.iter().map(|option| option.id.as_str()).collect();
        assert_eq!(ids, vec!["option-1", "option-2", "option-3"]);
        let labels: Vec<&str> = options.iter().map(|option| option.label.as_str()).collect();
        assert_eq!(labels, vec!["first", "second", "third"]);
    }

    /// Three is the whole offer. A fourth choice is a menu, and a menu is the
    /// friction capture exists to remove.
    #[test]
    fn a_fourth_option_is_refused() {
        let four = [draft("a"), draft("b"), draft("c"), draft("d")];
        assert_eq!(MAX_CAPTURE_OPTIONS, 3);
        assert!(numbered_options(&four[..MAX_CAPTURE_OPTIONS]).is_ok());
        assert!(numbered_options(&four).is_err());
    }

    #[test]
    fn an_option_with_nothing_written_on_it_is_refused() {
        assert!(numbered_options(&[draft("   ")]).is_err());
        let trimmed = numbered_options(&[draft("  File it  ")]).unwrap();
        assert_eq!(trimmed[0].label, "File it");
    }

    /// A branch name says the option is a branch. Naming one on an issue is two
    /// destinations in one choice, and the user would be tapping a guess.
    #[test]
    fn a_branch_name_makes_an_option_a_branch_and_never_an_issue() {
        let inferred = numbered_options(&[CaptureOptionDraft {
            label: "Continue the login work".to_string(),
            project_id: Some("proj-build".to_string()),
            kind: None,
            branch: Some("fix-login".to_string()),
        }])
        .unwrap();
        assert_eq!(inferred[0].kind, Some(CaptureTarget::Branch));
        assert_eq!(inferred[0].branch.as_deref(), Some("fix-login"));

        assert!(
            numbered_options(&[CaptureOptionDraft {
                label: "File it".to_string(),
                project_id: None,
                kind: Some(CaptureTarget::Issue),
                branch: Some("fix-login".to_string()),
            }])
            .is_err(),
            "an issue has no branch to be on"
        );
    }

    /// What the router reads when the user taps a choice: the label they saw,
    /// and the destination it stood for spelled out in the terms the router
    /// routes in.
    #[test]
    fn a_chosen_option_reads_as_an_answer_naming_the_destination() {
        let options = numbered_options(&[
            CaptureOptionDraft {
                label: "File as an issue on Build".to_string(),
                project_id: Some("proj-build".to_string()),
                kind: Some(CaptureTarget::Issue),
                branch: None,
            },
            CaptureOptionDraft {
                label: "Continue the login work".to_string(),
                project_id: Some("proj-build".to_string()),
                kind: Some(CaptureTarget::Branch),
                branch: Some("fix-login".to_string()),
            },
            CaptureOptionDraft {
                label: "It is about Do".to_string(),
                project_id: Some("proj-do".to_string()),
                kind: None,
                branch: None,
            },
        ])
        .unwrap();
        assert_eq!(
            options[0].as_answer(),
            "File as an issue on Build — route this to project proj-build as an issue"
        );
        assert_eq!(
            options[1].as_answer(),
            "Continue the login work — route this to project proj-build as a branch, on the branch fix-login"
        );
        assert_eq!(
            options[2].as_answer(),
            "It is about Do — route this to project proj-do"
        );
        assert_eq!(
            draft("Just this").numbered(1).as_answer(),
            "Just this",
            "an option with no route hint is only what it says"
        );
    }

    /// Tapping an option answers the question: the router hears the choice as
    /// words, and the record remembers which one was tapped.
    #[test]
    fn choosing_an_option_answers_the_question_in_words() {
        let asking = asking_with_options();
        let chosen = asking.answered_with_option("option-2").unwrap();
        assert!(!chosen.awaiting_answer());
        let question = chosen.question.as_ref().unwrap();
        assert_eq!(
            question.answer.as_deref(),
            Some("New branch on Do — route this to project proj-do as a branch")
        );
        assert_eq!(question.chosen_option_id.as_deref(), Some("option-2"));
        assert_eq!(
            question.options.len(),
            2,
            "the offer stays on the record it was made from"
        );
        assert!(asking.awaiting_answer(), "the record it read is untouched");
    }

    /// An id or a position picks the same choice, so a client that tracked
    /// either one is answering the same question.
    #[test]
    fn an_option_is_reachable_by_id_and_by_position() {
        let asking = asking_with_options();
        let question = asking.question.as_ref().unwrap();
        assert_eq!(
            question.option("option-1").unwrap().label,
            "File as an issue on Build"
        );
        assert_eq!(
            question.option_at(0).map(|option| option.id.as_str()),
            Some("option-1"),
            "position 0 is the first option offered"
        );
        assert_eq!(
            question.option_at(1).map(|option| option.id.as_str()),
            Some("option-2")
        );
        assert!(question.option("option-9").is_none());
        assert!(question.option_at(2).is_none());
    }

    /// A choice nobody offered is not a choice. Answering with one would put
    /// words in the user's mouth that the router never proposed.
    #[test]
    fn an_option_that_was_never_offered_is_refused() {
        assert!(asking_with_options()
            .answered_with_option("option-9")
            .is_err());
        assert!(
            capture(CaptureState::Unrouted)
                .answered_with_option("option-1")
                .is_err(),
            "nothing was asked, so nothing was offered"
        );
        let already = asking_with_options()
            .answered_with_option("option-1")
            .unwrap();
        assert!(already.answered_with_option("option-2").is_err());
    }

    /// A free-form answer to a question that offered options is still an
    /// answer: the choices never take the keyboard away.
    #[test]
    fn free_form_still_answers_a_question_that_offered_options() {
        let typed = asking_with_options()
            .answered("neither, it is the relay")
            .unwrap();
        assert!(!typed.awaiting_answer());
        let question = typed.question.as_ref().unwrap();
        assert_eq!(question.answer.as_deref(), Some("neither, it is the relay"));
        assert_eq!(
            question.chosen_option_id, None,
            "nothing was tapped, so nothing is recorded as tapped"
        );
    }

    /// A question written by an older bridge has no options and no choice, and
    /// still loads: the offer is the new part, not the question.
    #[test]
    fn a_question_loads_without_the_options_it_never_had() {
        let loaded: Capture = serde_json::from_str(
            r#"{"id":"capture-1","text":"ship it","created_at":"2026-08-13T10:00:00Z",
                "question":{"text":"which project?","asked_at":"2026-08-13T10:00:01Z"}}"#,
        )
        .unwrap();
        let question = loaded.question.as_ref().unwrap();
        assert!(question.options.is_empty());
        assert_eq!(question.chosen_option_id, None);
        assert!(loaded.awaiting_answer());
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
        assert!(is_capture_id(&id), "{id}");
        assert!(!is_capture_id(&crate::router::new_router_agent_id()));
        assert_ne!(id, new_capture_id());
    }
}
