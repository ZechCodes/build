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
//! A *quick task* skips planning (goal → building → review → merged). Three
//! agent-reported interruptions can occur during a working phase: `blocked` and
//! `failed` (the agent calls `done` with that status) and `idle_unreported` (the
//! PTY went quiet without any `done`). Each remembers the phase it interrupted so
//! the user's reply returns the task to the right working state.

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
    /// Approved and merged. Terminal.
    Merged,
    /// Abandoned; worktree removed, branch kept. Terminal.
    Abandoned,
}

impl TaskState {
    /// Terminal states accept no further events.
    pub fn is_terminal(&self) -> bool {
        matches!(self, TaskState::Merged | TaskState::Abandoned)
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
    /// The user replies to a blocked/failed/idle card; resume the working phase.
    Reply,
    /// Abandon the task from any non-terminal state.
    Abandon,
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

        // Plan review gate: revise (notes) or approve (start build).
        (PlanReview, E::SendNotes) => Ok(Planning),
        (PlanReview, E::ApprovePlan) => Ok(Building),

        // Building: same interruption shapes as planning.
        (Building, E::BuildReady) => Ok(Review),
        (Building, E::Blocked) => Ok(Blocked(Build)),
        (Building, E::Failed) => Ok(Failed(Build)),
        (Building, E::WentIdle) => Ok(IdleUnreported(Build)),

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

        // Abandon is legal from any non-terminal state.
        (s, E::Abandon) if !s.is_terminal() => Ok(Abandoned),

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
