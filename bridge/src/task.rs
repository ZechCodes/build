//! The task model and its lifecycle state machine.
//!
//! A task is a goal, a worktree + branch, a sequence of phase sessions, and a
//! state. This module is the pure domain core — no IO, no git, no PTY — so the
//! lifecycle rules are testable in isolation.
//!
//! The canonical lifecycle (scope §4):
//!
//! ```text
//! created → planning → plan_review → building → review → merged
//!               │  ▲                    │  ▲          │
//!               │  └── notes (batch) ───┘  └─ changes ┘
//!               └────────────── abandoned (from any state) ─┘
//! ```
//!
//! A *quick task* skips planning (goal → building → review → merged). Four
//! interruptions can occur during a working phase: `blocked` and `failed` (the
//! agent calls `done` with that status), `idle_unreported` (the PTY went quiet
//! without any `done`), and `interrupted` (the daemon itself died mid-phase and
//! recovered the task from the durable store on boot). Each remembers the phase
//! it interrupted so the user's reply returns the task to the right working
//! state.

use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

/// Opaque task identifier. The caller supplies it (the bridge mints a UUID).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TaskId(pub String);

impl TaskId {
    pub fn new(id: impl Into<String>) -> Self {
        TaskId(id.into())
    }
}

/// Whether a task carries the full plan gate or skips straight to building.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskKind {
    /// goal → plan → approve → build → review → merge
    Standard,
    /// goal → build → review → merge (no plan phase; for small, unambiguous work)
    Quick,
}

/// The working phase an agent runs in. Used to route interruption recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Plan,
    Build,
}

/// Every state a task can occupy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    /// Dispatched, worktree being prepared. No session yet.
    Created,
    /// A plan agent is running in a PTY, writing `.build/plan.md`.
    Planning,
    /// The plan is ready; the human is reviewing it (a gate).
    PlanReview,
    /// A build agent is running in a PTY, executing the plan.
    Building,
    /// The diff is ready; the human is reviewing it (a gate).
    Review,
    /// The agent called `done(blocked)` during `Phase`: it needs something.
    Blocked(Phase),
    /// The agent called `done(failed)` during `Phase`: the approach didn't work.
    Failed(Phase),
    /// The PTY went quiet during `Phase` without reporting `done`. An anomaly,
    /// explicitly *not* treated as completion.
    IdleUnreported(Phase),
    /// The daemon died (or was restarted) while an agent was working in `Phase`.
    /// The worktree survived; the session did not. The user decides: re-dispatch
    /// the phase, send notes/changes, or abandon.
    Interrupted(Phase),
    /// Approved and merged. Terminal.
    Merged,
    /// Abandoned; worktree removed, branch kept. Terminal.
    Abandoned,
    /// The worktree disappeared out from under the task (the user deleted it
    /// themselves). The task is kept as quiet read-only history — its plan
    /// docs survive in the store's snapshot. Terminal.
    Archived,
}

impl TaskState {
    /// Terminal states accept no further events.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskState::Merged | TaskState::Abandoned | TaskState::Archived
        )
    }

    /// States that belong in the board's "Needs you" bucket (UI brief §4.1):
    /// a human decision is the only thing that moves the task forward.
    pub fn needs_attention(&self) -> bool {
        matches!(
            self,
            TaskState::PlanReview
                | TaskState::Review
                | TaskState::Blocked(_)
                | TaskState::Failed(_)
                | TaskState::IdleUnreported(_)
                | TaskState::Interrupted(_)
        )
    }

    /// States where an agent is actively working (the quiet "Working" bucket).
    pub fn is_working(&self) -> bool {
        matches!(self, TaskState::Planning | TaskState::Building)
    }
}

/// Everything that can drive a lifecycle transition.
///
/// Agent-originated events (`PlanReady`, `BuildReady`, `Blocked`, `Failed`) arrive
/// via the `done` MCP tool; `WentIdle` is the quiescence timer; the rest are
/// human actions from the review surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskEvent {
    /// Begin work. Created → Planning (or → Building for a quick task).
    Dispatch,
    /// `done(phase=plan, completed)`. Planning → PlanReview.
    PlanReady,
    /// The user submits a batch of plan notes. PlanReview → Planning (revision).
    SendNotes,
    /// The user approves the plan. PlanReview → Building (fresh session).
    ApprovePlan,
    /// `done(phase=build, completed)`. Building → Review.
    BuildReady,
    /// The user submits a batch of diff comments. Review → Building (revise).
    RequestChanges,
    /// The user approves the diff. Review → Merged (git ops run elsewhere).
    ApproveMerge,
    /// `done(status=blocked)` during a working phase.
    Blocked,
    /// `done(status=failed)` during a working phase.
    Failed,
    /// Quiescence: the PTY went silent without a `done`.
    WentIdle,
    /// The daemon restarted while the agent was working: the session is gone.
    /// Raised during boot recovery, never by a live agent.
    Interrupt,
    /// The user replies to a blocked/failed/idle card; resume the working phase.
    Reply,
    /// Abandon the task from any non-terminal state.
    Abandon,
    /// The task's worktree disappeared from disk (deleted by the user outside
    /// Build). Raised by the archive sweep, never by a human action.
    Archive,
    /// The validation agent reported done(phase=validate, completed, passed=true).
    /// `last_stage` = the validated stage is the manifest's final stage.
    ValidationPassed { last_stage: bool },
    /// done(phase=validate, completed, passed=false).
    ValidationFailed,
}

/// A rejected transition: `event` is not valid from `from`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("illegal transition: {event:?} is not valid from {from:?}")]
pub struct IllegalTransition {
    pub from: TaskState,
    pub event: TaskEvent,
}

/// The pure transition function: given the current state, the task kind, and an
/// event, produce the next state — or reject the transition. No side effects.
pub fn transition(
    state: &TaskState,
    kind: TaskKind,
    event: TaskEvent,
) -> Result<TaskState, IllegalTransition> {
    use Phase::{Build, Plan};
    use TaskEvent as E;
    use TaskState::*;

    let illegal = || {
        Err(IllegalTransition {
            from: state.clone(),
            event,
        })
    };

    match (state, event) {
        // Dispatch begins work; a quick task skips the plan phase entirely.
        (Created, E::Dispatch) => Ok(match kind {
            TaskKind::Standard => Planning,
            TaskKind::Quick => Building,
        }),

        // Planning: the agent reports, blocks, fails, or goes quiet.
        (Planning, E::PlanReady) => Ok(PlanReview),
        (Planning, E::Blocked) => Ok(Blocked(Plan)),
        (Planning, E::Failed) => Ok(Failed(Plan)),
        (Planning, E::WentIdle) => Ok(IdleUnreported(Plan)),
        (Planning, E::Interrupt) => Ok(Interrupted(Plan)),

        // Plan review gate: revise (notes) or approve (start build).
        (PlanReview, E::SendNotes) => Ok(Planning),
        (PlanReview, E::ApprovePlan) => Ok(Building),

        // Building: same interruption shapes as planning. Change requests are also
        // accepted *while* the agent works — it redirects the running build.
        (Building, E::BuildReady) => Ok(Review),
        (Building, E::RequestChanges) => Ok(Building),
        (Building, E::Blocked) => Ok(Blocked(Build)),
        (Building, E::Failed) => Ok(Failed(Build)),
        (Building, E::WentIdle) => Ok(IdleUnreported(Build)),
        (Building, E::Interrupt) => Ok(Interrupted(Build)),

        // Diff review gate: request changes (revise) or approve (merge).
        (Review, E::RequestChanges) => Ok(Building),
        (Review, E::ApproveMerge) => Ok(Merged),

        // Blocked / failed: the user's reply resumes the interrupted phase.
        (Blocked(phase) | Failed(phase), E::Reply) => Ok(working_state(*phase)),

        // Idle-unreported: the agent was merely quiet. A reply resumes it, but a
        // later `done`/block/fail is still honored — quiescence never decided
        // anything.
        (IdleUnreported(phase), E::Reply) => Ok(working_state(*phase)),
        (IdleUnreported(Plan), E::PlanReady) => Ok(PlanReview),
        (IdleUnreported(Build), E::BuildReady) => Ok(Review),
        (IdleUnreported(phase), E::Blocked) => Ok(Blocked(*phase)),
        (IdleUnreported(phase), E::Failed) => Ok(Failed(*phase)),

        // Interrupted: the daemon restarted mid-phase, killing the session. A
        // reply re-dispatches the phase; notes/changes route to their revision
        // loops so the user can steer instead of merely restarting.
        (Interrupted(phase), E::Reply) => Ok(working_state(*phase)),
        (Interrupted(Plan), E::SendNotes) => Ok(Planning),
        (Interrupted(Build), E::RequestChanges) => Ok(Building),

        // Multi-stage validation outcomes. The task stays `Building` while a
        // stage's validation agent runs; only the verdict moves the coarse state.
        // The final stage's pass opens merge review; otherwise the task returns
        // to the between-stages gate (PlanReview — the stage board). The
        // IdleUnreported(Build) arms preserve the existing rule: quiescence never
        // decided anything, so a late validation `done` is still honored.
        (Building | IdleUnreported(Build), E::ValidationPassed { last_stage: true }) => Ok(Review),
        (Building | IdleUnreported(Build), E::ValidationPassed { last_stage: false }) => {
            Ok(PlanReview)
        }
        (Building | IdleUnreported(Build), E::ValidationFailed) => Ok(PlanReview),

        // Abandon is legal from any non-terminal state.
        (s, E::Abandon) if !s.is_terminal() => Ok(Abandoned),

        // Archive (the worktree vanished from disk) likewise — the user deleted
        // the files themselves, so the task retires to quiet history.
        (s, E::Archive) if !s.is_terminal() => Ok(Archived),

        // Terminal states and every other pairing are rejected.
        _ => illegal(),
    }
}

/// The working state an agent runs in for a given phase.
fn working_state(phase: Phase) -> TaskState {
    match phase {
        Phase::Plan => TaskState::Planning,
        Phase::Build => TaskState::Building,
    }
}

/// Position of one stage in its per-stage lifecycle. The task-level state stays
/// coarse; this is the sub-state the stage carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageState {
    /// The stage doc exists in the manifest; not yet approved by the human.
    Planned,
    /// The human approved this stage's doc.
    Approved,
    /// A build (or fix) session is running for this stage.
    Building,
    /// The build session reported done; validation has not started yet.
    Built,
    /// A validation agent session is running for this stage.
    Validating,
    /// Validation reported. `passed: true` is terminal for the stage;
    /// `passed: false` awaits `Dispatch` (a fix session) or a plan change.
    Validated { passed: bool },
}

/// Everything that can drive a stage transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StageEvent {
    /// The human approves the stage doc. Planned → Approved.
    Approve,
    /// A build session is spawned for this stage. Approved → Building;
    /// Validated{passed:false} → Building (the fix path).
    Dispatch,
    /// The stage's build/fix session reported done(completed). Building → Built.
    BuildDone,
    /// The validation session is spawned. Built → Validating.
    StartValidation,
    /// The validation session reported done(completed). Validating → Validated.
    ValidationDone { passed: bool },
    /// A plan-revision session completed for this stage; the doc changed, so any
    /// approval is stale. Planned → Planned; Approved → Planned.
    Revised,
}

/// A rejected stage transition.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("illegal stage transition: {event:?} is not valid from {from:?}")]
pub struct IllegalStageTransition {
    pub from: StageState,
    pub event: StageEvent,
}

/// The pure stage transition function — same discipline as `transition`.
pub fn stage_transition(
    state: &StageState,
    event: StageEvent,
) -> Result<StageState, IllegalStageTransition> {
    use StageEvent as E;
    use StageState::*;

    match (state, event) {
        // Plan review of the stage doc: approve it, or a revision session
        // rewrote it (any prior approval is stale).
        (Planned, E::Approve) => Ok(Approved),
        (Planned | Approved, E::Revised) => Ok(Planned),

        // Dispatch spawns a build session (first build, or a fix session after a
        // failed validation). `Validated{passed:true}` is stage-terminal.
        (Approved | Validated { passed: false }, E::Dispatch) => Ok(Building),

        // Build → validation pipeline.
        (Building, E::BuildDone) => Ok(Built),
        (Built, E::StartValidation) => Ok(Validating),
        (Validating, E::ValidationDone { passed }) => Ok(Validated { passed }),

        _ => Err(IllegalStageTransition {
            from: *state,
            event,
        }),
    }
}

/// True iff `path` stays inside whatever directory it is joined under: it is
/// non-empty, relative, and made only of normal components — no `..`, no `.`
/// segments, no root. This is the fence that keeps agent-supplied paths (the
/// manifest echo, `plan_path`) from escaping the worktree; a naive prefix check
/// alone would accept `.build/plan/../../../etc/passwd`.
pub fn is_worktree_contained_path(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// One entry of the plan manifest as the agent reports it (`.build/plan/stages.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageManifestEntry {
    pub id: String,
    pub title: String,
    pub path: String,
    #[serde(default)]
    pub summary: String,
}

/// The validation agent's verdict for one stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationReport {
    pub passed: bool,
    /// Markdown findings — what matched/diverged from the stage doc.
    pub findings: String,
    /// Markdown notes handed to the next stage's build prompt (and surfaced on
    /// the next stage in the UI). Empty string when there is nothing to say.
    pub notes_for_next_stage: String,
}

/// One stage: manifest data + lifecycle sub-state + validation outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stage {
    pub id: String,
    pub title: String,
    pub path: String,
    #[serde(default)]
    pub summary: String,
    pub state: StageState,
    /// `git rev-parse HEAD` of the worktree at the moment this stage was first
    /// dispatched — the base of "the diff this stage produced". Kept across fix
    /// re-dispatches so the stage diff always covers all of the stage's work.
    #[serde(default)]
    pub start_sha: Option<String>,
    #[serde(default)]
    pub validation: Option<ValidationReport>,
}

impl Stage {
    /// A freshly planned stage from a manifest entry.
    pub fn from_manifest(entry: StageManifestEntry) -> Stage {
        Stage {
            id: entry.id,
            title: entry.title,
            path: entry.path,
            summary: entry.summary,
            state: StageState::Planned,
            start_sha: None,
            validation: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommentState {
    Open,
    Addressed,
}

/// Where a plan comment anchors inside a stage doc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentAnchor {
    /// The chain of enclosing heading *texts* (raw markdown text, outermost
    /// first), e.g. ["Database schema", "Tables"]. Empty for a top-of-doc anchor.
    pub heading_path: Vec<String>,
    /// The selected passage, trimmed, capped at 400 chars by the producer.
    pub snippet: String,
}

/// One persisted, structured plan-review comment on a stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageComment {
    /// Bridge-minted: "c-<n>", n = 1 + max numeric suffix among the task's
    /// existing comment ids (so ids never collide after deletes).
    pub id: String,
    pub stage_id: String,
    /// None = a general comment on the stage (no text anchor).
    #[serde(default)]
    pub anchor: Option<CommentAnchor>,
    pub body: String,
    pub state: CommentState,
    #[serde(default)]
    pub agent_reply: Option<String>,
}

/// A task: identity, goal, kind, and current lifecycle state. Worktree/branch
/// and session bookkeeping attach in later slices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub goal: String,
    pub kind: TaskKind,
    pub state: TaskState,
}

impl Task {
    /// A freshly created task starts in `Created`.
    pub fn new(id: TaskId, goal: impl Into<String>, kind: TaskKind) -> Self {
        Task {
            id,
            goal: goal.into(),
            kind,
            state: TaskState::Created,
        }
    }

    /// Apply an event, advancing the task's own state. The task owns the
    /// mutation; `transition` stays pure.
    pub fn apply(&mut self, event: TaskEvent) -> Result<&TaskState, IllegalTransition> {
        self.state = transition(&self.state, self.kind, event)?;
        Ok(&self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(kind: TaskKind) -> Task {
        Task::new(TaskId::new("t1"), "do the thing", kind)
    }

    /// Drive a task through a sequence of events, asserting each resulting state.
    fn drive(kind: TaskKind, steps: &[(TaskEvent, TaskState)]) {
        let mut t = task(kind);
        for (event, expected) in steps {
            let got = t.apply(*event).expect("transition should be legal");
            assert_eq!(got, expected, "after {event:?}");
        }
    }

    #[test]
    fn new_task_starts_created() {
        assert_eq!(task(TaskKind::Standard).state, TaskState::Created);
    }

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

    #[test]
    fn standard_happy_path() {
        drive(
            TaskKind::Standard,
            &[
                (TaskEvent::Dispatch, TaskState::Planning),
                (TaskEvent::PlanReady, TaskState::PlanReview),
                (TaskEvent::ApprovePlan, TaskState::Building),
                (TaskEvent::BuildReady, TaskState::Review),
                (TaskEvent::ApproveMerge, TaskState::Merged),
            ],
        );
    }

    #[test]
    fn quick_task_skips_planning() {
        drive(
            TaskKind::Quick,
            &[
                (TaskEvent::Dispatch, TaskState::Building),
                (TaskEvent::BuildReady, TaskState::Review),
                (TaskEvent::ApproveMerge, TaskState::Merged),
            ],
        );
    }

    #[test]
    fn plan_notes_loop() {
        drive(
            TaskKind::Standard,
            &[
                (TaskEvent::Dispatch, TaskState::Planning),
                (TaskEvent::PlanReady, TaskState::PlanReview),
                (TaskEvent::SendNotes, TaskState::Planning),
                (TaskEvent::PlanReady, TaskState::PlanReview),
                (TaskEvent::ApprovePlan, TaskState::Building),
            ],
        );
    }

    #[test]
    fn diff_changes_loop() {
        drive(
            TaskKind::Standard,
            &[
                (TaskEvent::Dispatch, TaskState::Planning),
                (TaskEvent::PlanReady, TaskState::PlanReview),
                (TaskEvent::ApprovePlan, TaskState::Building),
                (TaskEvent::BuildReady, TaskState::Review),
                (TaskEvent::RequestChanges, TaskState::Building),
                (TaskEvent::BuildReady, TaskState::Review),
                (TaskEvent::ApproveMerge, TaskState::Merged),
            ],
        );
    }

    #[test]
    fn request_changes_while_building_redirects_the_agent() {
        // A change request can land while the build agent is still running.
        drive(
            TaskKind::Quick,
            &[
                (TaskEvent::Dispatch, TaskState::Building),
                (TaskEvent::RequestChanges, TaskState::Building),
                (TaskEvent::BuildReady, TaskState::Review),
                (TaskEvent::RequestChanges, TaskState::Building),
            ],
        );
    }

    #[test]
    fn blocked_during_planning_then_reply_resumes_planning() {
        drive(
            TaskKind::Standard,
            &[
                (TaskEvent::Dispatch, TaskState::Planning),
                (TaskEvent::Blocked, TaskState::Blocked(Phase::Plan)),
                (TaskEvent::Reply, TaskState::Planning),
            ],
        );
    }

    #[test]
    fn blocked_during_building_then_reply_resumes_building() {
        drive(
            TaskKind::Quick,
            &[
                (TaskEvent::Dispatch, TaskState::Building),
                (TaskEvent::Blocked, TaskState::Blocked(Phase::Build)),
                (TaskEvent::Reply, TaskState::Building),
            ],
        );
    }

    #[test]
    fn failed_during_planning_then_reply_resumes_planning() {
        drive(
            TaskKind::Standard,
            &[
                (TaskEvent::Dispatch, TaskState::Planning),
                (TaskEvent::Failed, TaskState::Failed(Phase::Plan)),
                (TaskEvent::Reply, TaskState::Planning),
            ],
        );
    }

    #[test]
    fn failed_during_building() {
        drive(
            TaskKind::Quick,
            &[
                (TaskEvent::Dispatch, TaskState::Building),
                (TaskEvent::Failed, TaskState::Failed(Phase::Build)),
                (TaskEvent::Reply, TaskState::Building),
            ],
        );
    }

    #[test]
    fn idle_unreported_during_building_then_agent_reports_done() {
        drive(
            TaskKind::Quick,
            &[
                (TaskEvent::Dispatch, TaskState::Building),
                (TaskEvent::WentIdle, TaskState::IdleUnreported(Phase::Build)),
                // The agent was merely quiet; a later `done` still advances it.
                (TaskEvent::BuildReady, TaskState::Review),
            ],
        );
    }

    #[test]
    fn idle_unreported_during_planning_then_reply() {
        drive(
            TaskKind::Standard,
            &[
                (TaskEvent::Dispatch, TaskState::Planning),
                (TaskEvent::WentIdle, TaskState::IdleUnreported(Phase::Plan)),
                (TaskEvent::Reply, TaskState::Planning),
            ],
        );
    }

    #[test]
    fn interrupt_during_planning_surfaces_interrupted_plan() {
        drive(
            TaskKind::Standard,
            &[
                (TaskEvent::Dispatch, TaskState::Planning),
                (TaskEvent::Interrupt, TaskState::Interrupted(Phase::Plan)),
            ],
        );
    }

    #[test]
    fn interrupt_during_building_surfaces_interrupted_build() {
        drive(
            TaskKind::Quick,
            &[
                (TaskEvent::Dispatch, TaskState::Building),
                (TaskEvent::Interrupt, TaskState::Interrupted(Phase::Build)),
            ],
        );
    }

    #[test]
    fn interrupted_plan_reply_redispatches_planning() {
        drive(
            TaskKind::Standard,
            &[
                (TaskEvent::Dispatch, TaskState::Planning),
                (TaskEvent::Interrupt, TaskState::Interrupted(Phase::Plan)),
                (TaskEvent::Reply, TaskState::Planning),
            ],
        );
    }

    #[test]
    fn interrupted_plan_accepts_notes_back_to_planning() {
        drive(
            TaskKind::Standard,
            &[
                (TaskEvent::Dispatch, TaskState::Planning),
                (TaskEvent::Interrupt, TaskState::Interrupted(Phase::Plan)),
                (TaskEvent::SendNotes, TaskState::Planning),
            ],
        );
    }

    #[test]
    fn interrupted_build_accepts_change_requests_back_to_building() {
        drive(
            TaskKind::Quick,
            &[
                (TaskEvent::Dispatch, TaskState::Building),
                (TaskEvent::Interrupt, TaskState::Interrupted(Phase::Build)),
                (TaskEvent::RequestChanges, TaskState::Building),
            ],
        );
    }

    #[test]
    fn interrupted_can_be_abandoned() {
        drive(
            TaskKind::Quick,
            &[
                (TaskEvent::Dispatch, TaskState::Building),
                (TaskEvent::Interrupt, TaskState::Interrupted(Phase::Build)),
                (TaskEvent::Abandon, TaskState::Abandoned),
            ],
        );
    }

    #[test]
    fn interrupt_is_rejected_outside_working_states() {
        for state in [
            TaskState::Created,
            TaskState::PlanReview,
            TaskState::Review,
            TaskState::Blocked(Phase::Build),
            TaskState::Failed(Phase::Plan),
            TaskState::IdleUnreported(Phase::Build),
            TaskState::Interrupted(Phase::Build),
            TaskState::Merged,
            TaskState::Abandoned,
        ] {
            assert!(
                transition(&state, TaskKind::Standard, TaskEvent::Interrupt).is_err(),
                "Interrupt should be rejected from {state:?}"
            );
        }
    }

    #[test]
    fn interrupted_needs_attention() {
        assert!(TaskState::Interrupted(Phase::Plan).needs_attention());
        assert!(TaskState::Interrupted(Phase::Build).needs_attention());
        assert!(!TaskState::Interrupted(Phase::Build).is_working());
        assert!(!TaskState::Interrupted(Phase::Build).is_terminal());
    }

    #[test]
    fn archive_from_any_nonterminal_state() {
        for setup in [
            vec![],
            vec![TaskEvent::Dispatch],
            vec![TaskEvent::Dispatch, TaskEvent::PlanReady],
            vec![
                TaskEvent::Dispatch,
                TaskEvent::PlanReady,
                TaskEvent::ApprovePlan,
            ],
            vec![
                TaskEvent::Dispatch,
                TaskEvent::PlanReady,
                TaskEvent::ApprovePlan,
                TaskEvent::BuildReady,
            ],
            vec![TaskEvent::Dispatch, TaskEvent::Blocked],
        ] {
            let mut t = task(TaskKind::Standard);
            for e in setup {
                t.apply(e).expect("setup transition legal");
            }
            assert!(!t.state.is_terminal());
            t.apply(TaskEvent::Archive)
                .expect("archive should be legal");
            assert_eq!(t.state, TaskState::Archived);
        }
    }

    #[test]
    fn archived_is_terminal_quiet_history() {
        assert!(TaskState::Archived.is_terminal());
        assert!(!TaskState::Archived.needs_attention());
        assert!(!TaskState::Archived.is_working());
        let mut t = task(TaskKind::Standard);
        t.apply(TaskEvent::Archive).expect("archive legal");
        assert!(t.apply(TaskEvent::Reply).is_err());
        assert!(t.apply(TaskEvent::Abandon).is_err());
        assert!(t.apply(TaskEvent::Archive).is_err());
    }

    #[test]
    fn abandon_from_any_nonterminal_state() {
        for setup in [
            vec![],
            vec![TaskEvent::Dispatch],
            vec![TaskEvent::Dispatch, TaskEvent::PlanReady],
            vec![
                TaskEvent::Dispatch,
                TaskEvent::PlanReady,
                TaskEvent::ApprovePlan,
            ],
            vec![
                TaskEvent::Dispatch,
                TaskEvent::PlanReady,
                TaskEvent::ApprovePlan,
                TaskEvent::BuildReady,
            ],
            vec![TaskEvent::Dispatch, TaskEvent::Blocked],
        ] {
            let mut t = task(TaskKind::Standard);
            for e in setup {
                t.apply(e).expect("setup transition legal");
            }
            assert!(!t.state.is_terminal());
            t.apply(TaskEvent::Abandon)
                .expect("abandon should be legal");
            assert_eq!(t.state, TaskState::Abandoned);
        }
    }

    #[test]
    fn terminal_states_reject_all_events() {
        for terminal in [
            vec![
                TaskEvent::Dispatch,
                TaskEvent::PlanReady,
                TaskEvent::ApprovePlan,
                TaskEvent::BuildReady,
                TaskEvent::ApproveMerge,
            ], // → Merged
            vec![TaskEvent::Abandon], // → Abandoned
        ] {
            let mut t = task(TaskKind::Standard);
            for e in terminal {
                t.apply(e).expect("setup legal");
            }
            assert!(t.state.is_terminal());
            for e in [
                TaskEvent::Dispatch,
                TaskEvent::PlanReady,
                TaskEvent::ApprovePlan,
                TaskEvent::BuildReady,
                TaskEvent::ApproveMerge,
                TaskEvent::Abandon,
                TaskEvent::Reply,
            ] {
                assert!(
                    transition(&t.state, TaskKind::Standard, e).is_err(),
                    "{e:?} should be rejected from {:?}",
                    t.state
                );
            }
        }
    }

    #[test]
    fn illegal_transitions_are_rejected() {
        // Can't approve a merge straight from Planning.
        let mut t = task(TaskKind::Standard);
        t.apply(TaskEvent::Dispatch).unwrap();
        let err = transition(&t.state, TaskKind::Standard, TaskEvent::ApproveMerge)
            .expect_err("merge from Planning is illegal");
        assert_eq!(err.from, TaskState::Planning);
        assert_eq!(err.event, TaskEvent::ApproveMerge);

        // Can't dispatch twice.
        assert!(transition(
            &TaskState::Building,
            TaskKind::Standard,
            TaskEvent::Dispatch
        )
        .is_err());
        // Can't send notes from Review (that's RequestChanges).
        assert!(transition(&TaskState::Review, TaskKind::Standard, TaskEvent::SendNotes).is_err());
    }

    // ---- Stage sub-state machine (multi-stage planning) ----

    #[test]
    fn stage_transition_full_table() {
        use StageEvent as E;
        use StageState::*;
        let table: &[(StageState, StageEvent, StageState)] = &[
            (Planned, E::Approve, Approved),
            (Planned, E::Revised, Planned),
            (Approved, E::Revised, Planned),
            (Approved, E::Dispatch, Building),
            (Validated { passed: false }, E::Dispatch, Building),
            (Building, E::BuildDone, Built),
            (Built, E::StartValidation, Validating),
            (
                Validating,
                E::ValidationDone { passed: true },
                Validated { passed: true },
            ),
            (
                Validating,
                E::ValidationDone { passed: false },
                Validated { passed: false },
            ),
        ];
        for (from, event, to) in table {
            assert_eq!(
                stage_transition(from, *event).expect("legal stage transition"),
                *to,
                "{event:?} from {from:?}"
            );
        }
    }

    #[test]
    fn stage_transition_rejects_everything_not_in_the_table() {
        use StageEvent as E;
        use StageState::*;
        let all_events = [
            E::Approve,
            E::Dispatch,
            E::BuildDone,
            E::StartValidation,
            E::ValidationDone { passed: true },
            E::ValidationDone { passed: false },
            E::Revised,
        ];
        let legal: &[(StageState, StageEvent)] = &[
            (Planned, E::Approve),
            (Planned, E::Revised),
            (Approved, E::Revised),
            (Approved, E::Dispatch),
            (Validated { passed: false }, E::Dispatch),
            (Building, E::BuildDone),
            (Built, E::StartValidation),
            (Validating, E::ValidationDone { passed: true }),
            (Validating, E::ValidationDone { passed: false }),
        ];
        for from in [
            Planned,
            Approved,
            Building,
            Built,
            Validating,
            Validated { passed: true },
            Validated { passed: false },
        ] {
            for event in all_events {
                if legal.contains(&(from, event)) {
                    continue;
                }
                let err = stage_transition(&from, event)
                    .expect_err(&format!("{event:?} should be rejected from {from:?}"));
                assert_eq!(err.from, from);
                assert_eq!(err.event, event);
            }
        }
    }

    #[test]
    fn validated_passed_is_stage_terminal() {
        use StageEvent as E;
        for event in [
            E::Approve,
            E::Dispatch,
            E::BuildDone,
            E::StartValidation,
            E::ValidationDone { passed: true },
            E::Revised,
        ] {
            assert!(
                stage_transition(&StageState::Validated { passed: true }, event).is_err(),
                "{event:?} should be rejected from Validated{{passed:true}}"
            );
        }
    }

    #[test]
    fn building_stage_rejects_approve_and_dispatch() {
        assert!(stage_transition(&StageState::Building, StageEvent::Approve).is_err());
        assert!(stage_transition(&StageState::Building, StageEvent::Dispatch).is_err());
    }

    #[test]
    fn revised_resets_approval() {
        assert_eq!(
            stage_transition(&StageState::Approved, StageEvent::Revised).unwrap(),
            StageState::Planned
        );
        assert_eq!(
            stage_transition(&StageState::Planned, StageEvent::Revised).unwrap(),
            StageState::Planned
        );
    }

    // ---- Task-level validation events ----

    #[test]
    fn validation_passed_on_last_stage_moves_building_to_review() {
        drive(
            TaskKind::Standard,
            &[
                (TaskEvent::Dispatch, TaskState::Planning),
                (TaskEvent::PlanReady, TaskState::PlanReview),
                (TaskEvent::ApprovePlan, TaskState::Building),
                (
                    TaskEvent::ValidationPassed { last_stage: true },
                    TaskState::Review,
                ),
            ],
        );
    }

    #[test]
    fn validation_passed_mid_plan_returns_to_plan_review() {
        drive(
            TaskKind::Standard,
            &[
                (TaskEvent::Dispatch, TaskState::Planning),
                (TaskEvent::PlanReady, TaskState::PlanReview),
                (TaskEvent::ApprovePlan, TaskState::Building),
                (
                    TaskEvent::ValidationPassed { last_stage: false },
                    TaskState::PlanReview,
                ),
            ],
        );
    }

    #[test]
    fn validation_failed_returns_to_plan_review() {
        drive(
            TaskKind::Standard,
            &[
                (TaskEvent::Dispatch, TaskState::Planning),
                (TaskEvent::PlanReady, TaskState::PlanReview),
                (TaskEvent::ApprovePlan, TaskState::Building),
                (TaskEvent::ValidationFailed, TaskState::PlanReview),
            ],
        );
    }

    #[test]
    fn late_validation_outcomes_are_honored_from_idle_unreported_build() {
        // Quiescence never decided anything: a late validation `done` still moves
        // the task, exactly like a late BuildReady.
        for (event, expected) in [
            (
                TaskEvent::ValidationPassed { last_stage: true },
                TaskState::Review,
            ),
            (
                TaskEvent::ValidationPassed { last_stage: false },
                TaskState::PlanReview,
            ),
            (TaskEvent::ValidationFailed, TaskState::PlanReview),
        ] {
            let got = transition(
                &TaskState::IdleUnreported(Phase::Build),
                TaskKind::Standard,
                event,
            )
            .expect("legal from IdleUnreported(Build)");
            assert_eq!(got, expected, "after {event:?}");
        }
    }

    #[test]
    fn validation_events_are_rejected_outside_build_working_states() {
        for state in [
            TaskState::Created,
            TaskState::Planning,
            TaskState::PlanReview,
            TaskState::Review,
            TaskState::IdleUnreported(Phase::Plan),
            TaskState::Blocked(Phase::Build),
            TaskState::Failed(Phase::Build),
            TaskState::Interrupted(Phase::Build),
            TaskState::Merged,
            TaskState::Abandoned,
        ] {
            for event in [
                TaskEvent::ValidationPassed { last_stage: true },
                TaskEvent::ValidationPassed { last_stage: false },
                TaskEvent::ValidationFailed,
            ] {
                assert!(
                    transition(&state, TaskKind::Standard, event).is_err(),
                    "{event:?} should be rejected from {state:?}"
                );
            }
        }
    }

    // ---- Serde shapes ----

    #[test]
    fn stage_state_serde_round_trips() {
        for (state, json) in [
            (StageState::Planned, "\"planned\""),
            (StageState::Approved, "\"approved\""),
            (StageState::Building, "\"building\""),
            (StageState::Built, "\"built\""),
            (StageState::Validating, "\"validating\""),
            (
                StageState::Validated { passed: true },
                "{\"validated\":{\"passed\":true}}",
            ),
            (
                StageState::Validated { passed: false },
                "{\"validated\":{\"passed\":false}}",
            ),
        ] {
            assert_eq!(serde_json::to_string(&state).unwrap(), json);
            assert_eq!(
                serde_json::from_str::<StageState>(json).unwrap(),
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
    fn stage_from_manifest_starts_planned_with_empty_extras() {
        let stage = Stage::from_manifest(StageManifestEntry {
            id: "api-endpoints".into(),
            title: "API endpoints".into(),
            path: ".build/plan/02-api-endpoints.md".into(),
            summary: "CRUD routes.".into(),
        });
        assert_eq!(stage.id, "api-endpoints");
        assert_eq!(stage.title, "API endpoints");
        assert_eq!(stage.path, ".build/plan/02-api-endpoints.md");
        assert_eq!(stage.summary, "CRUD routes.");
        assert_eq!(stage.state, StageState::Planned);
        assert_eq!(stage.start_sha, None);
        assert_eq!(stage.validation, None);
    }

    #[test]
    fn stage_serde_round_trips_including_validation_report() {
        let stage = Stage {
            id: "database-schema".into(),
            title: "Database schema".into(),
            path: ".build/plan/01-database-schema.md".into(),
            summary: "Tables and migration.".into(),
            state: StageState::Validated { passed: false },
            start_sha: Some("abc123".into()),
            validation: Some(ValidationReport {
                passed: false,
                findings: "- migration missing".into(),
                notes_for_next_stage: "".into(),
            }),
        };
        let json = serde_json::to_string(&stage).unwrap();
        assert_eq!(serde_json::from_str::<Stage>(&json).unwrap(), stage);

        // start_sha/validation are #[serde(default)]: a bare stage still loads.
        let bare: Stage = serde_json::from_str(
            r#"{"id":"s","title":"S","path":".build/plan/01-s.md","state":"planned"}"#,
        )
        .unwrap();
        assert_eq!(bare.summary, "");
        assert_eq!(bare.start_sha, None);
        assert_eq!(bare.validation, None);
    }

    #[test]
    fn validation_report_serde_round_trips() {
        let report = ValidationReport {
            passed: true,
            findings: "- all good".into(),
            notes_for_next_stage: "watch the renamed symbol".into(),
        };
        let json = serde_json::to_string(&report).unwrap();
        assert_eq!(
            serde_json::from_str::<ValidationReport>(&json).unwrap(),
            report
        );
    }

    #[test]
    fn stage_comment_serde_with_and_without_anchor() {
        let anchored = StageComment {
            id: "c-3".into(),
            stage_id: "database-schema".into(),
            anchor: Some(CommentAnchor {
                heading_path: vec!["Database schema".into(), "Tables".into()],
                snippet: "users table gets a soft-delete column".into(),
            }),
            body: "use a deleted_at timestamp".into(),
            state: CommentState::Open,
            agent_reply: None,
        };
        let json = serde_json::to_string(&anchored).unwrap();
        assert!(
            json.contains("\"open\""),
            "CommentState is lowercase: {json}"
        );
        assert_eq!(
            serde_json::from_str::<StageComment>(&json).unwrap(),
            anchored
        );

        let general = StageComment {
            id: "c-4".into(),
            stage_id: "database-schema".into(),
            anchor: None,
            body: "split this stage".into(),
            state: CommentState::Addressed,
            agent_reply: Some("done".into()),
        };
        let json = serde_json::to_string(&general).unwrap();
        assert!(json.contains("\"addressed\""));
        assert_eq!(
            serde_json::from_str::<StageComment>(&json).unwrap(),
            general
        );

        // anchor/agent_reply are #[serde(default)]: a minimal comment loads.
        let minimal: StageComment =
            serde_json::from_str(r#"{"id":"c-1","stage_id":"s","body":"b","state":"open"}"#)
                .unwrap();
        assert_eq!(minimal.anchor, None);
        assert_eq!(minimal.agent_reply, None);
    }

    #[test]
    fn attention_and_working_buckets() {
        assert!(TaskState::PlanReview.needs_attention());
        assert!(TaskState::Review.needs_attention());
        assert!(TaskState::Blocked(Phase::Plan).needs_attention());
        assert!(TaskState::Failed(Phase::Build).needs_attention());
        assert!(TaskState::IdleUnreported(Phase::Build).needs_attention());

        assert!(TaskState::Planning.is_working());
        assert!(TaskState::Building.is_working());
        assert!(!TaskState::Planning.needs_attention());

        assert!(TaskState::Merged.is_terminal());
        assert!(TaskState::Abandoned.is_terminal());
    }
}
