//! The plan model and its lifecycle state machine.
//!
//! A plan is *project-scoped*: a goal, canonical stage docs living in the
//! bridge store, per-stage doc states, and persisted review comments. Its
//! agent works in the project's primary checkout and owns no worktree —
//! worktrees belong to runs (`crate::run`). This module is the pure domain core — no IO,
//! no git, no PTY — so the lifecycle rules are testable in isolation.
//!
//! The plan lifecycle (spec: Plan/Run Split):
//!
//! ```text
//! created → drafting → plan_review → approved
//!               │  ▲                     │
//!               │  └── notes (batch) ────┘   (revision loop)
//!               └── blocked / failed / idle_unreported / interrupted
//!               └── abandoned (terminal, from any non-terminal state)
//! ```
//!
//! Four interruptions can occur while the plan agent works: `blocked` and
//! `failed` (the agent calls `done` with that status), `idle_unreported` (the
//! PTY went quiet without any `done`), and `interrupted` (the daemon died
//! mid-session and recovered the plan from the durable store on boot — or the
//! scratch docs dir vanished from disk; losing it never archives a plan,
//! because the canonical docs live in the store). There is exactly one
//! working phase, so — unlike the fused task machine — no interruption needs
//! to remember which phase it interrupted.
//!
//! "Implemented" is deliberately *not* a plan state: it is derived from the
//! plan's runs (any run merged). Once approved, a plan's coarse state stays
//! `Approved`; mid-run doc churn is carried by the per-stage doc states only.

use serde::{Deserialize, Serialize};

pub use crate::fs_scope::is_worktree_contained_path;

/// Opaque plan identifier (`plan-<uuid>`). The caller supplies it (the bridge
/// mints the UUID).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlanId(pub String);

impl PlanId {
    pub fn new(id: impl Into<String>) -> Self {
        PlanId(id.into())
    }
}

/// Every state a plan can occupy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanState {
    /// Filed, with nothing started. No workspace, no session yet.
    Created,
    /// A plan agent is running in a PTY, writing `.build/plan/` docs.
    Drafting,
    /// The docs are ingested; the human is reviewing the plan (a gate).
    PlanReview,
    /// The human approved the plan. It rests here — implementable by any
    /// number of runs over time — until abandoned.
    Approved,
    /// The agent called `done(blocked)`: it needs something from the human.
    Blocked,
    /// The agent called `done(failed)`: the approach didn't work.
    Failed,
    /// The PTY went quiet without reporting `done`. An anomaly, explicitly
    /// *not* treated as completion.
    IdleUnreported,
    /// The daemon died (or the scratch docs dir vanished) while the agent was
    /// drafting. The docs survive in the store; the session did not. The user
    /// decides: re-dispatch, send notes, or abandon.
    Interrupted,
    /// Abandoned. Terminal.
    Abandoned,
}

impl PlanState {
    /// Terminal states accept no further events.
    pub fn is_terminal(&self) -> bool {
        matches!(self, PlanState::Abandoned)
    }

    /// States that belong in the board's "Needs you" bucket (UI brief §4.1):
    /// a human decision is the only thing that moves the plan forward.
    pub fn needs_attention(&self) -> bool {
        matches!(
            self,
            PlanState::PlanReview
                | PlanState::Blocked
                | PlanState::Failed
                | PlanState::IdleUnreported
                | PlanState::Interrupted
        )
    }

    /// States where an agent is actively working (the quiet "Working" bucket).
    pub fn is_working(&self) -> bool {
        matches!(self, PlanState::Drafting)
    }
}

/// Everything that can drive a plan lifecycle transition.
///
/// Agent-originated events (`PlanReady`, `Blocked`, `Failed`) arrive via the
/// `done` MCP tool; `WentIdle` is the quiescence timer; the rest are human
/// actions from the review surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanEvent {
    /// Begin work. Created → Drafting.
    Dispatch,
    /// The plan agent reported Complete; docs ingested. Drafting → PlanReview.
    PlanReady,
    /// The user submits a batch of plan notes. PlanReview → Drafting (a
    /// revision turn, with the scratch docs refilled from the store first).
    SendNotes,
    /// The user approves the plan. PlanReview → Approved (the scratch docs are
    /// dropped elsewhere; the store copy is canonical).
    Approve,
    /// `done(status=blocked)` while drafting.
    Blocked,
    /// `done(status=failed)` while drafting.
    Failed,
    /// Quiescence: the PTY went silent without a `done`.
    WentIdle,
    /// The daemon restarted (or the scratch docs dir vanished) while the
    /// agent was drafting. Raised during boot recovery, never by a live agent.
    Interrupt,
    /// The user replies to a blocked/failed/idle card; resume drafting.
    Reply,
    /// Abandon the plan from any non-terminal state.
    Abandon,
}

/// A rejected plan transition: `event` is not valid from `from`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("illegal plan transition: {event:?} is not valid from {from:?}")]
pub struct IllegalPlanTransition {
    pub from: PlanState,
    pub event: PlanEvent,
}

/// The pure transition function: given the current state and an event, produce
/// the next state — or reject the transition. No side effects.
pub fn plan_transition(
    state: &PlanState,
    event: PlanEvent,
) -> Result<PlanState, IllegalPlanTransition> {
    use PlanEvent as E;
    use PlanState::*;

    let illegal = || {
        Err(IllegalPlanTransition {
            from: *state,
            event,
        })
    };

    match (state, event) {
        // Dispatch begins work: a planning workspace and a plan agent session
        // in the primary checkout.
        (Created, E::Dispatch) => Ok(Drafting),

        // Drafting: the agent reports, blocks, fails, or goes quiet.
        (Drafting, E::PlanReady) => Ok(PlanReview),
        (Drafting, E::Blocked) => Ok(Blocked),
        (Drafting, E::Failed) => Ok(Failed),
        (Drafting, E::WentIdle) => Ok(IdleUnreported),
        (Drafting, E::Interrupt) => Ok(Interrupted),

        // Plan review gate: revise (notes) or approve. Approval is the last
        // human gate — the plan rests at Approved until a run implements it.
        (PlanReview, E::SendNotes) => Ok(Drafting),
        (PlanReview, E::Approve) => Ok(Approved),

        // Blocked / failed: the user's reply resumes drafting.
        (Blocked | Failed, E::Reply) => Ok(Drafting),

        // Blocked asked for help; it never closed the session. The reviewer
        // answers on the thread (or straight in the terminal) and the same
        // warm agent may finish, fail, or find itself still stuck — all are
        // honored, so a blocked plan can never veto the agent's own progress.
        (Blocked, E::PlanReady) => Ok(PlanReview),
        (Blocked, E::Failed) => Ok(Failed),
        (Blocked, E::Blocked) => Ok(Blocked),

        // Idle-unreported: the agent was merely quiet. A reply resumes it, but
        // a later `done`/block/fail is still honored — quiescence never
        // decided anything.
        (IdleUnreported, E::Reply) => Ok(Drafting),
        (IdleUnreported, E::PlanReady) => Ok(PlanReview),
        (IdleUnreported, E::Blocked) => Ok(Blocked),
        (IdleUnreported, E::Failed) => Ok(Failed),

        // Interrupted: the session (and possibly the scratch docs dir) is
        // gone, but the docs survive in the store. A reply re-dispatches
        // drafting; notes route into the revision loop so the user can steer
        // instead of merely restarting.
        (Interrupted, E::Reply) => Ok(Drafting),
        (Interrupted, E::SendNotes) => Ok(Drafting),

        // Abandon is legal from any non-terminal state.
        (s, E::Abandon) if !s.is_terminal() => Ok(Abandoned),

        // Terminal states and every other pairing are rejected.
        _ => illegal(),
    }
}

/// Position of one stage *doc* in its plan-side review lifecycle. The plan's
/// coarse state stays `Approved` once approved; this is the sub-state that
/// carries per-stage doc review, including mid-run revisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageDocState {
    /// The stage doc exists in the manifest; not yet approved by the human.
    Planned,
    /// The human approved this stage's doc.
    Approved,
}

/// Everything that can drive a stage-doc transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StageDocEvent {
    /// The human approves the stage doc. Planned → Approved.
    Approve,
    /// A revision session rewrote this stage's doc, so any approval is stale.
    /// Planned → Planned; Approved → Planned.
    Revised,
}

/// A rejected stage-doc transition.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("illegal stage doc transition: {event:?} is not valid from {from:?}")]
pub struct IllegalStageDocTransition {
    pub from: StageDocState,
    pub event: StageDocEvent,
}

/// The pure stage-doc transition function — same discipline as
/// `plan_transition`.
pub fn stage_doc_transition(
    state: &StageDocState,
    event: StageDocEvent,
) -> Result<StageDocState, IllegalStageDocTransition> {
    use StageDocEvent as E;
    use StageDocState::*;

    match (state, event) {
        // Approve the doc, or a revision session rewrote it (any prior
        // approval is stale — the staleness rule).
        (Planned, E::Approve) => Ok(Approved),
        (Planned | Approved, E::Revised) => Ok(Planned),

        _ => Err(IllegalStageDocTransition {
            from: *state,
            event,
        }),
    }
}

/// One entry of the plan manifest as the agent reports it
/// (`.build/plan/stages.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageManifestEntry {
    pub id: String,
    pub title: String,
    pub path: String,
    #[serde(default)]
    pub summary: String,
}

/// A stable kebab-case slug: lowercase alphanumerics in hyphen-separated runs,
/// no leading/trailing/doubled hyphens.
fn is_kebab_slug(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// Read a plan agent's `.build/plan/stages.json`: the ordered stage entries,
/// or the first rule the manifest breaks, named so the reviewer can act on it.
///
/// Build reads the manifest from disk when the plan agent reports Complete —
/// the file is the plan, so nothing the agent says about it has to agree.
pub fn parse_stage_manifest(contents: &str) -> Result<Vec<StageManifestEntry>, String> {
    let entries: Vec<StageManifestEntry> = serde_json::from_str(contents)
        .map_err(|error| format!("stages.json is not a list of stages: {error}"))?;
    if entries.is_empty() {
        return Err("stages.json lists no stages".to_string());
    }
    let mut seen_ids = std::collections::HashSet::new();
    for entry in &entries {
        if !is_kebab_slug(&entry.id) {
            return Err(format!(
                "stage id \"{}\" is not a kebab-case slug",
                entry.id
            ));
        }
        if !seen_ids.insert(entry.id.as_str()) {
            return Err(format!("stage id \"{}\" appears twice", entry.id));
        }
        // The prefix check alone accepts `.build/plan/../../..` — the path
        // must also be traversal-free so it can never leave the plan dir.
        if !entry.path.starts_with(".build/plan/") || !is_worktree_contained_path(&entry.path) {
            return Err(format!(
                "stage path \"{}\" must be a plain path under .build/plan/",
                entry.path
            ));
        }
        if entry.title.trim().is_empty() {
            return Err(format!("stage \"{}\" has an empty title", entry.id));
        }
    }
    Ok(entries)
}

/// One stage doc: manifest metadata + plan-side review sub-state. Run-side
/// execution progress for the same stage id lives on the run
/// (`crate::run::StageProgress`), joined by stage id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageDoc {
    pub id: String,
    pub title: String,
    pub path: String,
    #[serde(default)]
    pub summary: String,
    pub state: StageDocState,
}

impl StageDoc {
    /// A freshly planned stage doc from a manifest entry.
    pub fn from_manifest(entry: StageManifestEntry) -> StageDoc {
        StageDoc {
            id: entry.id,
            title: entry.title,
            path: entry.path,
            summary: entry.summary,
            state: StageDocState::Planned,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImplementationIntent {
    #[default]
    None,
    Stage(String),
    All,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImplementationActivity {
    #[default]
    Idle,
    Preparing,
    Running(String),
    WaitingApproval(String),
    Blocked {
        stage_id: String,
        reason: String,
    },
}

/// A plan: identity, goal, and current lifecycle state. Stage docs, comments,
/// and store bookkeeping attach in the persistence slice. On the canonical
/// wire this is the Task aggregate; the type name remains for disk/API
/// compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub id: PlanId,
    pub goal: String,
    pub state: PlanState,
    /// Durable archival metadata, orthogonal to the lifecycle state. Archiving
    /// never discards canonical docs or linked run history.
    #[serde(default)]
    pub archived_at: Option<String>,
    #[serde(default)]
    pub implementation_intent: ImplementationIntent,
    #[serde(default)]
    pub implementation_activity: ImplementationActivity,
}

impl Plan {
    /// A freshly created plan starts in `Created`.
    pub fn new(id: PlanId, goal: impl Into<String>) -> Self {
        Plan {
            id,
            goal: goal.into(),
            state: PlanState::Created,
            archived_at: None,
            implementation_intent: ImplementationIntent::None,
            implementation_activity: ImplementationActivity::Idle,
        }
    }

    /// Apply an event, advancing the plan's own state. The plan owns the
    /// mutation; `plan_transition` stays pure.
    pub fn apply(&mut self, event: PlanEvent) -> Result<&PlanState, IllegalPlanTransition> {
        self.state = plan_transition(&self.state, event)?;
        Ok(&self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_manifest_parses_in_order() {
        let entries = parse_stage_manifest(
            r#"[{"id":"db-schema","title":"Schema","path":".build/plan/01-db-schema.md","summary":"Tables."},
                {"id":"api","title":"API","path":".build/plan/02-api.md"}]"#,
        )
        .expect("a well-formed manifest parses");
        let ids: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
        assert_eq!(ids, ["db-schema", "api"]);
        assert_eq!(entries[1].summary, "");
    }

    #[test]
    fn stage_manifest_names_the_rule_it_breaks() {
        let broken = [
            ("[]", "lists no stages"),
            ("{}", "not a list of stages"),
            (
                r#"[{"id":"Not_Kebab","title":"A","path":".build/plan/01-a.md"}]"#,
                "kebab-case",
            ),
            (
                r#"[{"id":"a","title":"A","path":".build/plan/01-a.md"},{"id":"a","title":"B","path":".build/plan/02-b.md"}]"#,
                "appears twice",
            ),
            (
                r#"[{"id":"a","title":"A","path":"docs/01-a.md"}]"#,
                "under .build/plan/",
            ),
            (
                r#"[{"id":"a","title":"A","path":".build/plan/../../../etc/passwd"}]"#,
                "under .build/plan/",
            ),
            (
                r#"[{"id":"a","title":" ","path":".build/plan/01-a.md"}]"#,
                "empty title",
            ),
        ];
        for (manifest, expected) in broken {
            let error = parse_stage_manifest(manifest).expect_err(manifest);
            assert!(error.contains(expected), "{manifest}: {error}");
        }
    }

    fn plan() -> Plan {
        Plan::new(PlanId::new("plan-1"), "plan the thing")
    }

    /// Drive a plan through a sequence of events, asserting each resulting state.
    fn drive(steps: &[(PlanEvent, PlanState)]) {
        let mut p = plan();
        for (event, expected) in steps {
            let got = p.apply(*event).expect("transition should be legal");
            assert_eq!(got, expected, "after {event:?}");
        }
    }

    #[test]
    fn new_plan_starts_created() {
        assert_eq!(plan().state, PlanState::Created);
    }

    #[test]
    fn happy_path_to_approved() {
        drive(&[
            (PlanEvent::Dispatch, PlanState::Drafting),
            (PlanEvent::PlanReady, PlanState::PlanReview),
            (PlanEvent::Approve, PlanState::Approved),
        ]);
    }

    #[test]
    fn notes_loop_revises_the_draft() {
        drive(&[
            (PlanEvent::Dispatch, PlanState::Drafting),
            (PlanEvent::PlanReady, PlanState::PlanReview),
            (PlanEvent::SendNotes, PlanState::Drafting),
            (PlanEvent::PlanReady, PlanState::PlanReview),
            (PlanEvent::Approve, PlanState::Approved),
        ]);
    }

    #[test]
    fn blocked_then_reply_resumes_drafting() {
        drive(&[
            (PlanEvent::Dispatch, PlanState::Drafting),
            (PlanEvent::Blocked, PlanState::Blocked),
            (PlanEvent::Reply, PlanState::Drafting),
        ]);
    }

    #[test]
    fn failed_then_reply_resumes_drafting() {
        drive(&[
            (PlanEvent::Dispatch, PlanState::Drafting),
            (PlanEvent::Failed, PlanState::Failed),
            (PlanEvent::Reply, PlanState::Drafting),
        ]);
    }

    #[test]
    fn idle_unreported_then_late_done_is_honored() {
        // Quiescence never decided anything: the agent was merely quiet, so a
        // later `done` still advances the plan.
        drive(&[
            (PlanEvent::Dispatch, PlanState::Drafting),
            (PlanEvent::WentIdle, PlanState::IdleUnreported),
            (PlanEvent::PlanReady, PlanState::PlanReview),
        ]);
    }

    #[test]
    fn idle_unreported_then_reply_resumes_drafting() {
        drive(&[
            (PlanEvent::Dispatch, PlanState::Drafting),
            (PlanEvent::WentIdle, PlanState::IdleUnreported),
            (PlanEvent::Reply, PlanState::Drafting),
        ]);
    }

    #[test]
    fn idle_unreported_then_late_blocked_or_failed_is_honored() {
        drive(&[
            (PlanEvent::Dispatch, PlanState::Drafting),
            (PlanEvent::WentIdle, PlanState::IdleUnreported),
            (PlanEvent::Blocked, PlanState::Blocked),
        ]);
        drive(&[
            (PlanEvent::Dispatch, PlanState::Drafting),
            (PlanEvent::WentIdle, PlanState::IdleUnreported),
            (PlanEvent::Failed, PlanState::Failed),
        ]);
    }

    #[test]
    fn blocked_then_late_done_is_honored() {
        // Blocking asked for help; it never closed the session. The reviewer
        // can answer on the thread or straight in the terminal, and the same
        // warm agent finishes — that completion opens the review gate.
        drive(&[
            (PlanEvent::Dispatch, PlanState::Drafting),
            (PlanEvent::Blocked, PlanState::Blocked),
            (PlanEvent::PlanReady, PlanState::PlanReview),
        ]);
    }

    #[test]
    fn blocked_then_late_failed_or_reblock_is_honored() {
        drive(&[
            (PlanEvent::Dispatch, PlanState::Drafting),
            (PlanEvent::Blocked, PlanState::Blocked),
            (PlanEvent::Failed, PlanState::Failed),
        ]);
        drive(&[
            (PlanEvent::Dispatch, PlanState::Drafting),
            (PlanEvent::Blocked, PlanState::Blocked),
            (PlanEvent::Blocked, PlanState::Blocked),
        ]);
    }

    #[test]
    fn interrupt_during_drafting_surfaces_interrupted() {
        drive(&[
            (PlanEvent::Dispatch, PlanState::Drafting),
            (PlanEvent::Interrupt, PlanState::Interrupted),
        ]);
    }

    #[test]
    fn interrupted_reply_redispatches_drafting() {
        drive(&[
            (PlanEvent::Dispatch, PlanState::Drafting),
            (PlanEvent::Interrupt, PlanState::Interrupted),
            (PlanEvent::Reply, PlanState::Drafting),
        ]);
    }

    #[test]
    fn interrupted_accepts_notes_back_to_drafting() {
        // The user steers instead of merely restarting: notes route into the
        // revision loop (a fresh worktree, docs re-materialized from the store).
        drive(&[
            (PlanEvent::Dispatch, PlanState::Drafting),
            (PlanEvent::Interrupt, PlanState::Interrupted),
            (PlanEvent::SendNotes, PlanState::Drafting),
        ]);
    }

    #[test]
    fn interrupted_can_be_abandoned() {
        drive(&[
            (PlanEvent::Dispatch, PlanState::Drafting),
            (PlanEvent::Interrupt, PlanState::Interrupted),
            (PlanEvent::Abandon, PlanState::Abandoned),
        ]);
    }

    #[test]
    fn interrupt_is_rejected_outside_drafting() {
        for state in [
            PlanState::Created,
            PlanState::PlanReview,
            PlanState::Approved,
            PlanState::Blocked,
            PlanState::Failed,
            PlanState::IdleUnreported,
            PlanState::Interrupted,
            PlanState::Abandoned,
        ] {
            assert!(
                plan_transition(&state, PlanEvent::Interrupt).is_err(),
                "Interrupt should be rejected from {state:?}"
            );
        }
    }

    #[test]
    fn abandon_from_any_nonterminal_state() {
        for setup in [
            vec![],
            vec![PlanEvent::Dispatch],
            vec![PlanEvent::Dispatch, PlanEvent::PlanReady],
            vec![
                PlanEvent::Dispatch,
                PlanEvent::PlanReady,
                PlanEvent::Approve,
            ],
            vec![PlanEvent::Dispatch, PlanEvent::Blocked],
            vec![PlanEvent::Dispatch, PlanEvent::WentIdle],
        ] {
            let mut p = plan();
            for e in setup {
                p.apply(e).expect("setup transition legal");
            }
            assert!(!p.state.is_terminal());
            p.apply(PlanEvent::Abandon)
                .expect("abandon should be legal");
            assert_eq!(p.state, PlanState::Abandoned);
        }
    }

    #[test]
    fn approved_rests_accepting_only_abandon() {
        // Once approved, the plan's coarse state stays put: implementation
        // belongs to runs, and mid-run doc churn is carried by per-stage doc
        // states. Only abandon moves it.
        let approved = PlanState::Approved;
        for event in [
            PlanEvent::Dispatch,
            PlanEvent::PlanReady,
            PlanEvent::SendNotes,
            PlanEvent::Approve,
            PlanEvent::Blocked,
            PlanEvent::Failed,
            PlanEvent::WentIdle,
            PlanEvent::Interrupt,
            PlanEvent::Reply,
        ] {
            assert!(
                plan_transition(&approved, event).is_err(),
                "{event:?} should be rejected from Approved"
            );
        }
        assert_eq!(
            plan_transition(&approved, PlanEvent::Abandon).unwrap(),
            PlanState::Abandoned
        );
        assert!(!approved.is_terminal());
        assert!(!approved.needs_attention());
        assert!(!approved.is_working());
    }

    #[test]
    fn abandoned_is_terminal_and_rejects_all_events() {
        let mut p = plan();
        p.apply(PlanEvent::Abandon).expect("abandon legal");
        assert!(p.state.is_terminal());
        for event in [
            PlanEvent::Dispatch,
            PlanEvent::PlanReady,
            PlanEvent::SendNotes,
            PlanEvent::Approve,
            PlanEvent::Blocked,
            PlanEvent::Failed,
            PlanEvent::WentIdle,
            PlanEvent::Interrupt,
            PlanEvent::Reply,
            PlanEvent::Abandon,
        ] {
            assert!(
                plan_transition(&p.state, event).is_err(),
                "{event:?} should be rejected from Abandoned"
            );
        }
    }

    #[test]
    fn illegal_transitions_are_rejected_with_context() {
        // Can't approve straight from Drafting — the gate needs a PlanReady.
        let err = plan_transition(&PlanState::Drafting, PlanEvent::Approve)
            .expect_err("approve from Drafting is illegal");
        assert_eq!(err.from, PlanState::Drafting);
        assert_eq!(err.event, PlanEvent::Approve);

        // Can't dispatch twice.
        assert!(plan_transition(&PlanState::Drafting, PlanEvent::Dispatch).is_err());
        // Reply is for interruption cards, not the review gate.
        assert!(plan_transition(&PlanState::PlanReview, PlanEvent::Reply).is_err());
        // PlanReady means nothing before dispatch.
        assert!(plan_transition(&PlanState::Created, PlanEvent::PlanReady).is_err());
    }

    #[test]
    fn attention_and_working_buckets() {
        assert!(PlanState::PlanReview.needs_attention());
        assert!(PlanState::Blocked.needs_attention());
        assert!(PlanState::Failed.needs_attention());
        assert!(PlanState::IdleUnreported.needs_attention());
        assert!(PlanState::Interrupted.needs_attention());

        assert!(PlanState::Drafting.is_working());
        assert!(!PlanState::Drafting.needs_attention());
        assert!(!PlanState::Created.is_working());

        assert!(PlanState::Abandoned.is_terminal());
        assert!(!PlanState::Approved.is_terminal());
        assert!(!PlanState::Interrupted.is_terminal());
    }

    // ---- Stage-doc sub-state machine ----

    #[test]
    fn stage_doc_transition_full_table() {
        use StageDocEvent as E;
        use StageDocState::*;
        let table: &[(StageDocState, StageDocEvent, StageDocState)] = &[
            (Planned, E::Approve, Approved),
            (Planned, E::Revised, Planned),
            (Approved, E::Revised, Planned),
        ];
        for (from, event, to) in table {
            assert_eq!(
                stage_doc_transition(from, *event).expect("legal stage doc transition"),
                *to,
                "{event:?} from {from:?}"
            );
        }
    }

    #[test]
    fn stage_doc_approve_is_rejected_when_already_approved() {
        let err = stage_doc_transition(&StageDocState::Approved, StageDocEvent::Approve)
            .expect_err("double approve is illegal");
        assert_eq!(err.from, StageDocState::Approved);
        assert_eq!(err.event, StageDocEvent::Approve);
    }

    #[test]
    fn revised_resets_approval() {
        // The staleness rule: a revision session rewrote the doc, so approval
        // of the previous text no longer stands.
        assert_eq!(
            stage_doc_transition(&StageDocState::Approved, StageDocEvent::Revised).unwrap(),
            StageDocState::Planned
        );
        assert_eq!(
            stage_doc_transition(&StageDocState::Planned, StageDocEvent::Revised).unwrap(),
            StageDocState::Planned
        );
    }

    // ---- Path fence ----

    #[test]
    fn worktree_contained_path_accepts_only_plain_relative_paths() {
        assert!(is_worktree_contained_path(".build/plan/01-a.md"));
        assert!(is_worktree_contained_path(".build/plan.md"));
        assert!(!is_worktree_contained_path(""));
        assert!(!is_worktree_contained_path("/etc/passwd"));
        assert!(!is_worktree_contained_path(
            ".build/plan/../../../../etc/passwd"
        ));
        assert!(!is_worktree_contained_path("../sibling.md"));
        assert!(!is_worktree_contained_path("./.build/plan/01-a.md"));
    }

    // ---- Serde shapes ----

    #[test]
    fn stage_doc_state_serde_round_trips() {
        for (state, json) in [
            (StageDocState::Planned, "\"planned\""),
            (StageDocState::Approved, "\"approved\""),
        ] {
            assert_eq!(serde_json::to_string(&state).unwrap(), json);
            assert_eq!(
                serde_json::from_str::<StageDocState>(json).unwrap(),
                state,
                "round-trip of {json}"
            );
        }
    }

    #[test]
    fn stage_manifest_entry_summary_defaults_empty() {
        let entry: StageManifestEntry = serde_json::from_str(
            r#"{"id":"database-schema","title":"Database schema","path":".build/plan/01-database-schema.md"}"#,
        )
        .unwrap();
        assert_eq!(entry.summary, "");
        assert_eq!(entry.id, "database-schema");
    }

    #[test]
    fn stage_doc_from_manifest_starts_planned() {
        let doc = StageDoc::from_manifest(StageManifestEntry {
            id: "api-endpoints".into(),
            title: "API endpoints".into(),
            path: ".build/plan/02-api-endpoints.md".into(),
            summary: "CRUD routes.".into(),
        });
        assert_eq!(doc.id, "api-endpoints");
        assert_eq!(doc.title, "API endpoints");
        assert_eq!(doc.path, ".build/plan/02-api-endpoints.md");
        assert_eq!(doc.summary, "CRUD routes.");
        assert_eq!(doc.state, StageDocState::Planned);
    }

    #[test]
    fn stage_doc_serde_round_trips() {
        let doc = StageDoc {
            id: "database-schema".into(),
            title: "Database schema".into(),
            path: ".build/plan/01-database-schema.md".into(),
            summary: "Tables and migration.".into(),
            state: StageDocState::Approved,
        };
        let json = serde_json::to_string(&doc).unwrap();
        assert_eq!(serde_json::from_str::<StageDoc>(&json).unwrap(), doc);

        // summary is #[serde(default)]: a bare doc still loads.
        let bare: StageDoc = serde_json::from_str(
            r#"{"id":"s","title":"S","path":".build/plan/01-s.md","state":"planned"}"#,
        )
        .unwrap();
        assert_eq!(bare.summary, "");
        assert_eq!(bare.state, StageDocState::Planned);
    }
}
