//! The run model and its lifecycle state machine.
//!
//! A run is *worktree-scoped*: one implementation attempt — a worktree, a
//! branch, a sequence of build/fix/validation sessions, and a state. A run
//! usually implements a plan (`plan_id`); an *adopted* run — one minted around
//! a worktree that already existed — is the only kind with `plan_id = None`.
//! This module is the pure domain core — no IO, no git, no
//! PTY — so the lifecycle rules are testable in isolation.
//!
//! The run lifecycle (spec: Plan/Run Split):
//!
//! ```text
//! created → building → review → merged
//!              │  ▲        │
//!              │  └ changes┘
//!              ├──⇄ stage_gate (multi-stage: between validated stages)
//!              └── blocked / failed / idle_unreported / interrupted
//!              └── abandoned / archived (terminal)
//! ```
//!
//! `StageGate` replaces the fused task machine's reuse of `PlanReview` as the
//! between-stages board: the run parks there after a stage validates (or its
//! validation fails) until the human dispatches the next stage or a fix
//! session. Four interruptions can occur while a build agent works: `blocked`
//! and `failed` (the agent calls `done` with that status), `idle_unreported`
//! (the PTY went quiet without any `done`), and `interrupted` (the daemon died
//! mid-session and recovered the run from the durable store on boot). There is
//! exactly one working phase, so no interruption needs to remember which phase
//! it interrupted.

use serde::{Deserialize, Serialize};

use crate::plan::PlanId;

/// Opaque run identifier (`run-<uuid>`). The caller supplies it (the bridge
/// mints the UUID).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RunId(pub String);

impl RunId {
    pub fn new(id: impl Into<String>) -> Self {
        RunId(id.into())
    }
}

/// Every state a run can occupy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunState {
    /// Dispatched, worktree being prepared (plan docs materialized and
    /// committed for a planned run). No session yet.
    Created,
    /// A build agent is running in a PTY, executing the plan (or, for an
    /// adopted run, its derived goal).
    Building,
    /// Multi-stage only: between stages. The previous stage's validation
    /// verdict is in; the human dispatches the next stage or a fix session.
    StageGate,
    /// The diff is ready; the human is reviewing it (a gate).
    Review,
    /// The agent called `done(blocked)`: it needs something from the human.
    Blocked,
    /// The agent called `done(failed)`: the approach didn't work.
    Failed,
    /// The PTY went quiet without reporting `done`. An anomaly, explicitly
    /// *not* treated as completion.
    IdleUnreported,
    /// The daemon died (or was restarted) while an agent was building. The
    /// worktree survived; the session did not. The user decides: re-dispatch,
    /// send changes, or abandon.
    Interrupted,
    /// Approved and merged. Terminal.
    Merged,
    /// Abandoned; worktree removed, branch kept. Terminal.
    Abandoned,
    /// The worktree disappeared out from under the run (the user deleted it
    /// themselves). The run is kept as quiet read-only history — its plan was
    /// never in the worktree to lose. Terminal.
    Archived,
}

impl RunState {
    /// Terminal states accept no further events.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RunState::Merged | RunState::Abandoned | RunState::Archived
        )
    }

    /// States that belong in the board's "Needs you" bucket (UI brief §4.1):
    /// a human decision is the only thing that moves the run forward.
    pub fn needs_attention(&self) -> bool {
        matches!(
            self,
            RunState::Review
                | RunState::StageGate
                | RunState::Blocked
                | RunState::Failed
                | RunState::IdleUnreported
                | RunState::Interrupted
        )
    }

    /// States where an agent is actively working (the quiet "Working" bucket).
    pub fn is_working(&self) -> bool {
        matches!(self, RunState::Building)
    }
}

/// Everything that can drive a run lifecycle transition.
///
/// Agent-originated events (`BuildReady`, `Blocked`, `Failed`, the validation
/// verdicts) arrive via the `done` MCP tool; `WentIdle` is the quiescence
/// timer; `Archive` is the worktree sweep; the rest are human actions from the
/// review surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunEvent {
    /// Begin work: the first build session (Created → Building), or the next
    /// stage / fix session from the stage gate (StageGate → Building).
    Dispatch,
    /// `done(phase=build, completed)`. Building → Review.
    BuildReady,
    /// The user submits a batch of diff comments. Legal from Review (revise)
    /// *and* from Building — it redirects the running agent.
    RequestChanges,
    /// The user approves the diff. Review → Merged (git ops run elsewhere).
    ApproveMerge,
    /// `done(status=blocked)` while building.
    Blocked,
    /// `done(status=failed)` while building.
    Failed,
    /// Quiescence: the PTY went silent without a `done`.
    WentIdle,
    /// The daemon restarted while the agent was building: the session is gone.
    /// Raised during boot recovery, never by a live agent.
    Interrupt,
    /// The user replies to a blocked/failed/idle card; resume building.
    Reply,
    /// Abandon the run from any non-terminal state.
    Abandon,
    /// The run's worktree disappeared from disk, either outside Build or when
    /// the human used Done to finish and archive its checkout.
    Archive,
    /// The validation agent reported done(phase=validate, completed,
    /// passed=true). `last_stage` = the validated stage is the manifest's
    /// final stage.
    ValidationPassed { last_stage: bool },
    /// done(phase=validate, completed, passed=false).
    ValidationFailed,
}

/// A rejected run transition: `event` is not valid from `from`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("illegal run transition: {event:?} is not valid from {from:?}")]
pub struct IllegalRunTransition {
    pub from: RunState,
    pub event: RunEvent,
}

/// The pure transition function: given the current state and an event, produce
/// the next state — or reject the transition. No side effects.
pub fn run_transition(state: &RunState, event: RunEvent) -> Result<RunState, IllegalRunTransition> {
    use RunEvent as E;
    use RunState::*;

    let illegal = || {
        Err(IllegalRunTransition {
            from: *state,
            event,
        })
    };

    match (state, event) {
        // Dispatch begins work: the first build session from Created, or the
        // next stage / fix session from the between-stages gate.
        (Created | StageGate, E::Dispatch) => Ok(Building),

        // Building: the agent reports, blocks, fails, or goes quiet. Change
        // requests are also accepted *while* the agent works — they redirect
        // the running build.
        (Building, E::BuildReady) => Ok(Review),
        (Building, E::RequestChanges) => Ok(Building),
        (Building, E::Blocked) => Ok(Blocked),
        (Building, E::Failed) => Ok(Failed),
        (Building, E::WentIdle) => Ok(IdleUnreported),
        (Building, E::Interrupt) => Ok(Interrupted),

        // Diff review gate: request changes (revise) or approve (merge).
        (Review, E::RequestChanges) => Ok(Building),
        (Review, E::ApproveMerge) => Ok(Merged),

        // Blocked / failed: the user's reply resumes building.
        (Blocked | Failed, E::Reply) => Ok(Building),

        // Blocked asked for help; it never closed the session. The reviewer
        // answers on the thread (or straight in the terminal) and the same
        // warm agent may finish, fail, or find itself still stuck — all are
        // honored, so a blocked run can never veto the agent's own progress.
        (Blocked, E::BuildReady) => Ok(Review),
        (Blocked, E::Failed) => Ok(Failed),
        (Blocked, E::Blocked) => Ok(Blocked),

        // Idle-unreported: the agent was merely quiet. A reply resumes it, but
        // a later `done`/block/fail is still honored — quiescence never
        // decided anything.
        (IdleUnreported, E::Reply) => Ok(Building),
        (IdleUnreported, E::BuildReady) => Ok(Review),
        (IdleUnreported, E::Blocked) => Ok(Blocked),
        (IdleUnreported, E::Failed) => Ok(Failed),

        // Interrupted: the daemon restarted mid-session, killing it. A reply
        // re-dispatches building; change requests route into the revision loop
        // so the user can steer instead of merely restarting.
        (Interrupted, E::Reply) => Ok(Building),
        (Interrupted, E::RequestChanges) => Ok(Building),

        // Multi-stage validation verdicts. The run stays `Building` while a
        // stage's validation agent runs; only the verdict moves the coarse
        // state. The final stage's pass opens merge review; otherwise the run
        // parks at the between-stages gate. The IdleUnreported and Blocked
        // arms preserve the standing rule: neither quiescence nor a plea for
        // help decided anything, so a late validation `done` is still honored.
        (Building | IdleUnreported | Blocked, E::ValidationPassed { last_stage: true }) => {
            Ok(Review)
        }
        (Building | IdleUnreported | Blocked, E::ValidationPassed { last_stage: false }) => {
            Ok(StageGate)
        }
        (Building | IdleUnreported | Blocked, E::ValidationFailed) => Ok(StageGate),

        // Abandon is legal from any non-terminal state.
        (s, E::Abandon) if !s.is_terminal() => Ok(Abandoned),

        // Archive when the worktree vanishes, including Done after a merge
        // whose cleanup policy kept the checkout around.
        (Merged, E::Archive) => Ok(Archived),
        (s, E::Archive) if !s.is_terminal() => Ok(Archived),

        // Terminal states and every other pairing are rejected.
        _ => illegal(),
    }
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

/// How much review one hunk of a diff needs.
///
/// Presentational, and only that: triage orders and collapses the review
/// surface. No level gates a lifecycle transition, and no state waits on a
/// triage pass — the diff underneath it is always the whole diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriageLevel {
    Critical,
    Normal,
    Low,
}

impl TriageLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            TriageLevel::Critical => "critical",
            TriageLevel::Normal => "normal",
            TriageLevel::Low => "low",
        }
    }
}

/// One hunk's classification, keyed by the id `crate::diff::patch_hunks`
/// assigns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriageHunk {
    pub hunk_id: String,
    pub level: TriageLevel,
    /// The one line the reviewer reads instead of a collapsed hunk, or the one
    /// that says what to look at in a critical one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    /// The named group `low` hunks collapse into, shared by several hunks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

/// A triage pass over one revision of a run's diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriageReport {
    /// The diff revision this pass read — the content hash of the patch it was
    /// given. A triage whose `based_on` is not the current revision is stale:
    /// it still ships, labelled, because a stale ordering beats none.
    pub based_on: String,
    pub hunks: Vec<TriageHunk>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryState {
    Started,
    Succeeded,
    Failed,
}

/// Durable nonce and outcome for one attempt to recover an Issue's original
/// implementation branch. The agent's MCP report is a claim; `Succeeded` is
/// written only after the daemon independently verifies the restored checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryAttempt {
    pub id: String,
    pub requested_stage_id: String,
    pub branch: String,
    pub state: RecoveryState,
    #[serde(default)]
    pub report: Option<crate::mcp::RecoveryReport>,
    pub started_at: String,
    #[serde(default)]
    pub completed_at: Option<String>,
}

/// Position of one stage in its run-side execution lifecycle. Plan-side doc
/// review (`Planned/Approved`) lives on the plan (`crate::plan::StageDoc`);
/// a progress record exists only once the stage has been dispatched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageProgressState {
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

/// Everything that can drive a stage-progress transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StageProgressEvent {
    /// A fix session is spawned after a failed validation.
    /// Validated{passed:false} → Building. (The *first* build session creates
    /// the progress record instead — see `StageProgress::dispatched`.)
    Dispatch,
    /// The stage's build/fix session reported done(completed). Building → Built.
    BuildDone,
    /// The validation session is spawned. Built → Validating.
    StartValidation,
    /// The validation session reported done(completed). Validating → Validated.
    ValidationDone { passed: bool },
}

/// A rejected stage-progress transition.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("illegal stage progress transition: {event:?} is not valid from {from:?}")]
pub struct IllegalStageProgressTransition {
    pub from: StageProgressState,
    pub event: StageProgressEvent,
}

/// The pure stage-progress transition function — same discipline as
/// `run_transition`.
pub fn stage_progress_transition(
    state: &StageProgressState,
    event: StageProgressEvent,
) -> Result<StageProgressState, IllegalStageProgressTransition> {
    use StageProgressEvent as E;
    use StageProgressState::*;

    match (state, event) {
        // The fix path: a new build session after a failed validation.
        // `Validated{passed:true}` is stage-terminal.
        (Validated { passed: false }, E::Dispatch) => Ok(Building),

        // Build → validation pipeline.
        (Building, E::BuildDone) => Ok(Built),
        (Built, E::StartValidation) => Ok(Validating),
        (Validating, E::ValidationDone { passed }) => Ok(Validated { passed }),

        _ => Err(IllegalStageProgressTransition {
            from: *state,
            event,
        }),
    }
}

/// Publication evidence for a completed stage commit. `LegacyUnknown` is the
/// fail-closed default for records written before Build pinned completion SHAs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StagePublication {
    Local,
    Pushed,
    Merged,
    #[default]
    LegacyUnknown,
}

/// Git publication intent persisted before push/merge side effects. The
/// candidate commit makes restart recovery an observation of refs rather than
/// a guess based on whether the previous process returned success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicationAttempt {
    pub action: String,
    pub candidate_sha: String,
    pub started_at: String,
}

/// One stage's execution progress on a run, keyed by the plan's stage id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageProgress {
    pub stage_id: String,
    pub state: StageProgressState,
    /// `git rev-parse HEAD` of the worktree at the moment this stage was first
    /// dispatched — the base of "the diff this stage produced". Kept across fix
    /// re-dispatches so the stage diff always covers all of the stage's work.
    #[serde(default)]
    pub start_sha: Option<String>,
    /// Candidate commit observed immediately after the build safety-net commit.
    #[serde(default)]
    pub built_sha: Option<String>,
    /// Immutable successful boundary. Stable stage diffs are
    /// `start_sha..completion_sha`, never a diff against the moving worktree.
    #[serde(default)]
    pub completion_sha: Option<String>,
    #[serde(default)]
    pub publication: StagePublication,
    /// Evidence retained when worktree loss or recovery invalidates execution.
    #[serde(default)]
    pub invalidation_reason: Option<String>,
    #[serde(default)]
    pub validation: Option<ValidationReport>,
}

impl StageProgress {
    /// A stage's progress record is created at its first dispatch: the build
    /// session is already running, so it starts `Building`.
    pub fn dispatched(stage_id: impl Into<String>) -> StageProgress {
        StageProgress {
            stage_id: stage_id.into(),
            state: StageProgressState::Building,
            start_sha: None,
            built_sha: None,
            completion_sha: None,
            publication: StagePublication::Local,
            invalidation_reason: None,
            validation: None,
        }
    }

    /// Apply an event, advancing this stage's own state. The record owns the
    /// mutation; `stage_progress_transition` stays pure.
    pub fn apply(
        &mut self,
        event: StageProgressEvent,
    ) -> Result<&StageProgressState, IllegalStageProgressTransition> {
        self.state = stage_progress_transition(&self.state, event)?;
        Ok(&self.state)
    }
}

/// A run: identity, optional plan link, goal, and current lifecycle state.
/// Worktree/branch and session bookkeeping attach in later slices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    /// The plan this run implements. `None` = an adopted run: its goal is
    /// derived from the worktree Build adopted, and there is no plan gate.
    #[serde(default)]
    pub plan_id: Option<PlanId>,
    pub goal: String,
    pub state: RunState,
}

impl Run {
    /// A freshly created run starts in `Created`.
    pub fn new(id: RunId, plan_id: Option<PlanId>, goal: impl Into<String>) -> Self {
        Run {
            id,
            plan_id,
            goal: goal.into(),
            state: RunState::Created,
        }
    }

    /// Apply an event, advancing the run's own state. The run owns the
    /// mutation; `run_transition` stays pure.
    pub fn apply(&mut self, event: RunEvent) -> Result<&RunState, IllegalRunTransition> {
        self.state = run_transition(&self.state, event)?;
        Ok(&self.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_less_run() -> Run {
        Run::new(RunId::new("run-1"), None, "do the thing")
    }

    fn planned_run() -> Run {
        Run::new(
            RunId::new("run-2"),
            Some(PlanId::new("plan-1")),
            "implement the plan",
        )
    }

    /// Drive a run through a sequence of events, asserting each resulting state.
    fn drive(mut run: Run, steps: &[(RunEvent, RunState)]) -> Run {
        for (event, expected) in steps {
            let got = run.apply(*event).expect("transition should be legal");
            assert_eq!(got, expected, "after {event:?}");
        }
        run
    }

    #[test]
    fn new_run_starts_created() {
        assert_eq!(plan_less_run().state, RunState::Created);
        assert_eq!(planned_run().state, RunState::Created);
    }

    #[test]
    fn happy_path_to_merged() {
        // The machine is identical for plan-less and planned runs: the plan gate
        // lives on the plan now, so every run dispatches straight to building.
        for run in [plan_less_run(), planned_run()] {
            drive(
                run,
                &[
                    (RunEvent::Dispatch, RunState::Building),
                    (RunEvent::BuildReady, RunState::Review),
                    (RunEvent::ApproveMerge, RunState::Merged),
                ],
            );
        }
    }

    #[test]
    fn diff_changes_loop() {
        drive(
            plan_less_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (RunEvent::BuildReady, RunState::Review),
                (RunEvent::RequestChanges, RunState::Building),
                (RunEvent::BuildReady, RunState::Review),
                (RunEvent::ApproveMerge, RunState::Merged),
            ],
        );
    }

    #[test]
    fn request_changes_while_building_redirects_the_agent() {
        // A change request can land while the build agent is still running.
        drive(
            plan_less_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (RunEvent::RequestChanges, RunState::Building),
                (RunEvent::BuildReady, RunState::Review),
                (RunEvent::RequestChanges, RunState::Building),
            ],
        );
    }

    #[test]
    fn blocked_then_reply_resumes_building() {
        drive(
            plan_less_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (RunEvent::Blocked, RunState::Blocked),
                (RunEvent::Reply, RunState::Building),
            ],
        );
    }

    #[test]
    fn failed_then_reply_resumes_building() {
        drive(
            plan_less_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (RunEvent::Failed, RunState::Failed),
                (RunEvent::Reply, RunState::Building),
            ],
        );
    }

    #[test]
    fn idle_unreported_then_late_done_is_honored() {
        // Quiescence never decided anything: the agent was merely quiet, so a
        // later `done` still advances the run.
        drive(
            plan_less_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (RunEvent::WentIdle, RunState::IdleUnreported),
                (RunEvent::BuildReady, RunState::Review),
            ],
        );
    }

    #[test]
    fn idle_unreported_then_reply_resumes_building() {
        drive(
            plan_less_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (RunEvent::WentIdle, RunState::IdleUnreported),
                (RunEvent::Reply, RunState::Building),
            ],
        );
    }

    #[test]
    fn idle_unreported_then_late_blocked_or_failed_is_honored() {
        drive(
            plan_less_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (RunEvent::WentIdle, RunState::IdleUnreported),
                (RunEvent::Blocked, RunState::Blocked),
            ],
        );
        drive(
            plan_less_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (RunEvent::WentIdle, RunState::IdleUnreported),
                (RunEvent::Failed, RunState::Failed),
            ],
        );
    }

    #[test]
    fn blocked_then_late_done_is_honored() {
        // Blocking asked for help; it never closed the session. The reviewer
        // can answer on the thread or straight in the terminal, and the same
        // warm agent finishes — that completion opens merge review.
        drive(
            plan_less_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (RunEvent::Blocked, RunState::Blocked),
                (RunEvent::BuildReady, RunState::Review),
            ],
        );
    }

    #[test]
    fn blocked_then_late_failed_or_reblock_is_honored() {
        drive(
            plan_less_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (RunEvent::Blocked, RunState::Blocked),
                (RunEvent::Failed, RunState::Failed),
            ],
        );
        drive(
            plan_less_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (RunEvent::Blocked, RunState::Blocked),
                (RunEvent::Blocked, RunState::Blocked),
            ],
        );
    }

    #[test]
    fn interrupt_during_building_surfaces_interrupted() {
        drive(
            plan_less_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (RunEvent::Interrupt, RunState::Interrupted),
            ],
        );
    }

    #[test]
    fn interrupted_reply_redispatches_building() {
        drive(
            plan_less_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (RunEvent::Interrupt, RunState::Interrupted),
                (RunEvent::Reply, RunState::Building),
            ],
        );
    }

    #[test]
    fn interrupted_accepts_change_requests_back_to_building() {
        // The user steers instead of merely restarting.
        drive(
            plan_less_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (RunEvent::Interrupt, RunState::Interrupted),
                (RunEvent::RequestChanges, RunState::Building),
            ],
        );
    }

    #[test]
    fn interrupted_can_be_abandoned() {
        drive(
            plan_less_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (RunEvent::Interrupt, RunState::Interrupted),
                (RunEvent::Abandon, RunState::Abandoned),
            ],
        );
    }

    #[test]
    fn interrupt_is_rejected_outside_building() {
        for state in [
            RunState::Created,
            RunState::StageGate,
            RunState::Review,
            RunState::Blocked,
            RunState::Failed,
            RunState::IdleUnreported,
            RunState::Interrupted,
            RunState::Merged,
            RunState::Abandoned,
            RunState::Archived,
        ] {
            assert!(
                run_transition(&state, RunEvent::Interrupt).is_err(),
                "Interrupt should be rejected from {state:?}"
            );
        }
    }

    // ---- Multi-stage: validation verdicts and the stage gate ----

    #[test]
    fn validation_passed_on_last_stage_moves_building_to_review() {
        drive(
            planned_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (
                    RunEvent::ValidationPassed { last_stage: true },
                    RunState::Review,
                ),
            ],
        );
    }

    #[test]
    fn validation_passed_mid_plan_parks_at_the_stage_gate() {
        drive(
            planned_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (
                    RunEvent::ValidationPassed { last_stage: false },
                    RunState::StageGate,
                ),
            ],
        );
    }

    #[test]
    fn validation_failed_parks_at_the_stage_gate() {
        drive(
            planned_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (RunEvent::ValidationFailed, RunState::StageGate),
            ],
        );
    }

    #[test]
    fn blocked_then_late_validation_verdict_is_honored() {
        // A validation agent can block (it needs something) and then, once
        // answered, still deliver its verdict from the same warm session.
        drive(
            planned_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (RunEvent::Blocked, RunState::Blocked),
                (
                    RunEvent::ValidationPassed { last_stage: false },
                    RunState::StageGate,
                ),
            ],
        );
        drive(
            planned_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (RunEvent::Blocked, RunState::Blocked),
                (RunEvent::ValidationFailed, RunState::StageGate),
            ],
        );
    }

    #[test]
    fn stage_gate_dispatch_starts_the_next_stage() {
        drive(
            planned_run(),
            &[
                (RunEvent::Dispatch, RunState::Building),
                (
                    RunEvent::ValidationPassed { last_stage: false },
                    RunState::StageGate,
                ),
                (RunEvent::Dispatch, RunState::Building),
                (
                    RunEvent::ValidationPassed { last_stage: true },
                    RunState::Review,
                ),
                (RunEvent::ApproveMerge, RunState::Merged),
            ],
        );
    }

    #[test]
    fn stage_gate_rejects_everything_but_dispatch_abandon_archive() {
        for event in [
            RunEvent::BuildReady,
            RunEvent::RequestChanges,
            RunEvent::ApproveMerge,
            RunEvent::Blocked,
            RunEvent::Failed,
            RunEvent::WentIdle,
            RunEvent::Interrupt,
            RunEvent::Reply,
            RunEvent::ValidationPassed { last_stage: true },
            RunEvent::ValidationPassed { last_stage: false },
            RunEvent::ValidationFailed,
        ] {
            assert!(
                run_transition(&RunState::StageGate, event).is_err(),
                "{event:?} should be rejected from StageGate"
            );
        }
        assert_eq!(
            run_transition(&RunState::StageGate, RunEvent::Dispatch).unwrap(),
            RunState::Building
        );
        assert_eq!(
            run_transition(&RunState::StageGate, RunEvent::Abandon).unwrap(),
            RunState::Abandoned
        );
        assert_eq!(
            run_transition(&RunState::StageGate, RunEvent::Archive).unwrap(),
            RunState::Archived
        );
    }

    #[test]
    fn late_validation_outcomes_are_honored_from_idle_unreported() {
        // Quiescence never decided anything: a late validation `done` still
        // moves the run, exactly like a late BuildReady.
        for (event, expected) in [
            (
                RunEvent::ValidationPassed { last_stage: true },
                RunState::Review,
            ),
            (
                RunEvent::ValidationPassed { last_stage: false },
                RunState::StageGate,
            ),
            (RunEvent::ValidationFailed, RunState::StageGate),
        ] {
            let got = run_transition(&RunState::IdleUnreported, event)
                .expect("legal from IdleUnreported");
            assert_eq!(got, expected, "after {event:?}");
        }
    }

    #[test]
    fn validation_events_are_rejected_outside_working_states() {
        // Blocked is deliberately absent: a plea for help decided nothing, so
        // a late verdict from a nudged validation agent is still honored.
        for state in [
            RunState::Created,
            RunState::StageGate,
            RunState::Review,
            RunState::Failed,
            RunState::Interrupted,
            RunState::Merged,
            RunState::Abandoned,
            RunState::Archived,
        ] {
            for event in [
                RunEvent::ValidationPassed { last_stage: true },
                RunEvent::ValidationPassed { last_stage: false },
                RunEvent::ValidationFailed,
            ] {
                assert!(
                    run_transition(&state, event).is_err(),
                    "{event:?} should be rejected from {state:?}"
                );
            }
        }
    }

    // ---- Terminal arms ----

    #[test]
    fn abandon_from_any_nonterminal_state() {
        for setup in [
            vec![],
            vec![RunEvent::Dispatch],
            vec![RunEvent::Dispatch, RunEvent::BuildReady],
            vec![RunEvent::Dispatch, RunEvent::Blocked],
            vec![RunEvent::Dispatch, RunEvent::WentIdle],
            vec![RunEvent::Dispatch, RunEvent::ValidationFailed],
        ] {
            let mut r = planned_run();
            for e in setup {
                r.apply(e).expect("setup transition legal");
            }
            assert!(!r.state.is_terminal());
            r.apply(RunEvent::Abandon).expect("abandon should be legal");
            assert_eq!(r.state, RunState::Abandoned);
        }
    }

    #[test]
    fn archive_from_any_nonterminal_state() {
        for setup in [
            vec![],
            vec![RunEvent::Dispatch],
            vec![RunEvent::Dispatch, RunEvent::BuildReady],
            vec![RunEvent::Dispatch, RunEvent::Blocked],
            vec![RunEvent::Dispatch, RunEvent::ValidationFailed],
        ] {
            let mut r = planned_run();
            for e in setup {
                r.apply(e).expect("setup transition legal");
            }
            assert!(!r.state.is_terminal());
            r.apply(RunEvent::Archive).expect("archive should be legal");
            assert_eq!(r.state, RunState::Archived);
        }
    }

    #[test]
    fn merged_run_can_be_archived_when_done_cleans_up_its_kept_worktree() {
        let mut run = plan_less_run();
        run.apply(RunEvent::Dispatch).unwrap();
        run.apply(RunEvent::BuildReady).unwrap();
        run.apply(RunEvent::ApproveMerge).unwrap();

        run.apply(RunEvent::Archive)
            .expect("Done retires merged lineage after worktree cleanup");
        assert_eq!(run.state, RunState::Archived);
    }

    #[test]
    fn terminal_states_reject_all_other_events() {
        let terminal_setups: &[&[RunEvent]] = &[
            &[
                RunEvent::Dispatch,
                RunEvent::BuildReady,
                RunEvent::ApproveMerge,
            ], // → Merged
            &[RunEvent::Abandon], // → Abandoned
            &[RunEvent::Archive], // → Archived
        ];
        for setup in terminal_setups {
            let mut r = plan_less_run();
            for e in *setup {
                r.apply(*e).expect("setup legal");
            }
            assert!(r.state.is_terminal());
            for e in [
                RunEvent::Dispatch,
                RunEvent::BuildReady,
                RunEvent::RequestChanges,
                RunEvent::ApproveMerge,
                RunEvent::Blocked,
                RunEvent::Failed,
                RunEvent::WentIdle,
                RunEvent::Interrupt,
                RunEvent::Reply,
                RunEvent::Abandon,
                RunEvent::ValidationPassed { last_stage: true },
                RunEvent::ValidationFailed,
            ] {
                assert!(
                    run_transition(&r.state, e).is_err(),
                    "{e:?} should be rejected from {:?}",
                    r.state
                );
            }
            if r.state != RunState::Merged {
                assert!(run_transition(&r.state, RunEvent::Archive).is_err());
            }
        }
    }

    #[test]
    fn illegal_transitions_are_rejected_with_context() {
        // Can't approve a merge straight from Building.
        let err = run_transition(&RunState::Building, RunEvent::ApproveMerge)
            .expect_err("merge from Building is illegal");
        assert_eq!(err.from, RunState::Building);
        assert_eq!(err.event, RunEvent::ApproveMerge);

        // Can't dispatch twice.
        assert!(run_transition(&RunState::Building, RunEvent::Dispatch).is_err());
        // BuildReady means nothing before dispatch.
        assert!(run_transition(&RunState::Created, RunEvent::BuildReady).is_err());
        // Reply is for interruption cards, not the review gate.
        assert!(run_transition(&RunState::Review, RunEvent::Reply).is_err());
    }

    #[test]
    fn attention_and_working_buckets() {
        assert!(RunState::Review.needs_attention());
        assert!(RunState::StageGate.needs_attention());
        assert!(RunState::Blocked.needs_attention());
        assert!(RunState::Failed.needs_attention());
        assert!(RunState::IdleUnreported.needs_attention());
        assert!(RunState::Interrupted.needs_attention());

        assert!(RunState::Building.is_working());
        assert!(!RunState::Building.needs_attention());
        assert!(!RunState::StageGate.is_working());
        assert!(!RunState::Created.is_working());

        assert!(RunState::Merged.is_terminal());
        assert!(RunState::Abandoned.is_terminal());
        assert!(RunState::Archived.is_terminal());
        assert!(!RunState::StageGate.is_terminal());
    }

    // ---- Stage-progress sub-state machine ----

    #[test]
    fn stage_progress_transition_full_table() {
        use StageProgressEvent as E;
        use StageProgressState::*;
        let table: &[(StageProgressState, StageProgressEvent, StageProgressState)] = &[
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
                stage_progress_transition(from, *event).expect("legal stage progress transition"),
                *to,
                "{event:?} from {from:?}"
            );
        }
    }

    #[test]
    fn stage_progress_rejects_everything_not_in_the_table() {
        use StageProgressEvent as E;
        use StageProgressState::*;
        let all_events = [
            E::Dispatch,
            E::BuildDone,
            E::StartValidation,
            E::ValidationDone { passed: true },
            E::ValidationDone { passed: false },
        ];
        let legal: &[(StageProgressState, StageProgressEvent)] = &[
            (Validated { passed: false }, E::Dispatch),
            (Building, E::BuildDone),
            (Built, E::StartValidation),
            (Validating, E::ValidationDone { passed: true }),
            (Validating, E::ValidationDone { passed: false }),
        ];
        for from in [
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
                let err = stage_progress_transition(&from, event)
                    .expect_err(&format!("{event:?} should be rejected from {from:?}"));
                assert_eq!(err.from, from);
                assert_eq!(err.event, event);
            }
        }
    }

    #[test]
    fn validated_passed_is_stage_terminal() {
        use StageProgressEvent as E;
        for event in [
            E::Dispatch,
            E::BuildDone,
            E::StartValidation,
            E::ValidationDone { passed: true },
        ] {
            assert!(
                stage_progress_transition(&StageProgressState::Validated { passed: true }, event)
                    .is_err(),
                "{event:?} should be rejected from Validated{{passed:true}}"
            );
        }
    }

    #[test]
    fn dispatched_stage_progress_starts_building() {
        let progress = StageProgress::dispatched("database-schema");
        assert_eq!(progress.stage_id, "database-schema");
        assert_eq!(progress.state, StageProgressState::Building);
        assert_eq!(progress.start_sha, None);
        assert_eq!(progress.validation, None);
    }

    #[test]
    fn stage_progress_apply_walks_the_pipeline() {
        let mut progress = StageProgress::dispatched("api-endpoints");
        progress
            .apply(StageProgressEvent::BuildDone)
            .expect("build done legal");
        progress
            .apply(StageProgressEvent::StartValidation)
            .expect("start validation legal");
        progress
            .apply(StageProgressEvent::ValidationDone { passed: false })
            .expect("validation done legal");
        assert_eq!(
            progress.state,
            StageProgressState::Validated { passed: false }
        );
        // The fix path: dispatch again after a failed validation.
        progress
            .apply(StageProgressEvent::Dispatch)
            .expect("fix dispatch legal");
        assert_eq!(progress.state, StageProgressState::Building);
    }

    // ---- Serde shapes ----

    #[test]
    fn stage_progress_state_serde_round_trips() {
        for (state, json) in [
            (StageProgressState::Building, "\"building\""),
            (StageProgressState::Built, "\"built\""),
            (StageProgressState::Validating, "\"validating\""),
            (
                StageProgressState::Validated { passed: true },
                "{\"validated\":{\"passed\":true}}",
            ),
            (
                StageProgressState::Validated { passed: false },
                "{\"validated\":{\"passed\":false}}",
            ),
        ] {
            assert_eq!(serde_json::to_string(&state).unwrap(), json);
            assert_eq!(
                serde_json::from_str::<StageProgressState>(json).unwrap(),
                state,
                "round-trip of {json}"
            );
        }
    }

    #[test]
    fn stage_progress_serde_round_trips_including_validation_report() {
        let progress = StageProgress {
            stage_id: "database-schema".into(),
            state: StageProgressState::Validated { passed: false },
            start_sha: Some("abc123".into()),
            built_sha: Some("def456".into()),
            completion_sha: None,
            publication: StagePublication::Local,
            invalidation_reason: None,
            validation: Some(ValidationReport {
                passed: false,
                findings: "- migration missing".into(),
                notes_for_next_stage: "".into(),
            }),
        };
        let json = serde_json::to_string(&progress).unwrap();
        assert_eq!(
            serde_json::from_str::<StageProgress>(&json).unwrap(),
            progress
        );

        // New boundary fields default safely for records written before they existed.
        let bare: StageProgress =
            serde_json::from_str(r#"{"stage_id":"s","state":"building"}"#).unwrap();
        assert_eq!(bare.start_sha, None);
        assert_eq!(bare.built_sha, None);
        assert_eq!(bare.completion_sha, None);
        assert_eq!(bare.publication, StagePublication::LegacyUnknown);
        assert_eq!(bare.invalidation_reason, None);
        assert_eq!(bare.validation, None);
    }

    #[test]
    fn dispatched_stage_starts_local_with_no_commit_boundaries() {
        let progress = StageProgress::dispatched("stage-1");
        assert_eq!(progress.publication, StagePublication::Local);
        assert_eq!(progress.built_sha, None);
        assert_eq!(progress.completion_sha, None);
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
    fn run_serde_keeps_optional_plan_link() {
        let plan_less = plan_less_run();
        let json = serde_json::to_string(&plan_less).unwrap();
        assert_eq!(serde_json::from_str::<Run>(&json).unwrap(), plan_less);

        let planned = planned_run();
        let json = serde_json::to_string(&planned).unwrap();
        let loaded: Run = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.plan_id, Some(PlanId::new("plan-1")));

        // plan_id is #[serde(default)]: a record without one is an adopted run.
        let bare: Run =
            serde_json::from_str(r#"{"id":"run-3","goal":"g","state":"Created"}"#).unwrap();
        assert_eq!(bare.plan_id, None);
    }
}
