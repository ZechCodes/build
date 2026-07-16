//! The task-spine: where lifecycle, worktrees, PTY sessions, `done` reports, and
//! the diff come together.
//!
//! The orchestrator owns project-level configuration (the repo, where worktrees
//! go, the harness adapter, the prompt templates) and drives the split's two
//! entities through their lifecycles: an [`ActivePlan`] (project-scoped, authored
//! in a disposable `plan/<slug>` worktree, canonical docs in the store) and an
//! [`ActiveRun`] (worktree-scoped, one implementation attempt on a `build/<slug>`
//! branch). The caller owns each active entity and hands it back by `&mut` for
//! each transition, so the orchestrator never hides state.
//!
//! The two pipes from the scope are both here: Build → agent is `write_prompt`
//! into the warm PTY; agent → Build is [`on_plan_done`](Orchestrator::on_plan_done)
//! / [`on_run_done`](Orchestrator::on_run_done), the typed events the MCP server
//! forwards (the caller routes each report by owner lookup).
//!
//! PERIPHERY: the fused [`ActiveTask`] spine (`dispatch`/`on_done`/
//! `approve_task_plan`, stage flows, message/resume, adopt, merge ops) is still
//! what `app.rs` speaks; the periphery stage moves those callers onto the split
//! seams and retires the fused path together with `task.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

use portable_pty::PtySize;

use crate::diff::{diff_against_base, diff_against_merge_base, DiffError, WorktreeDiff};
use crate::mcp::{DonePhase, DoneReport, DoneStatus};
use crate::models::ModelChoice;
use crate::plan::{
    plan_transition, stage_doc_transition, CommentState as PlanCommentState, IllegalPlanTransition,
    IllegalStageDocTransition, Plan, PlanEvent, PlanId, PlanState,
    StageComment as PlanStageComment, StageDoc, StageDocEvent, StageDocState,
};
use crate::pty::{HarnessSpec, PtyError, PtySession};
use crate::run::{
    run_transition, IllegalRunTransition, IllegalStageProgressTransition, Run, RunEvent, RunId,
    StageProgress, StageProgressEvent, StageProgressState, ValidationReport as RunValidationReport,
};
use crate::store::{PersistedPlan, PersistedRun, Store, StoreError};
use crate::task::{
    stage_transition, CommentState, IllegalStageTransition, IllegalTransition, Phase, Stage,
    StageComment, StageEvent, StageManifestEntry, StageState, Task, TaskEvent, TaskId, TaskKind,
    TaskState,
};
use crate::templates::{self, Templates, Vars, DEFAULT_PLAN_PATH};
use crate::worktree::{
    derive_adoption_goal, slugify, ExternalWorktree, Worktree, WorktreeError, WorktreeManager,
    PLAN_BRANCH_PREFIX,
};

#[derive(Debug, thiserror::Error)]
pub enum OrchestratorError {
    #[error(transparent)]
    Worktree(#[from] WorktreeError),
    #[error(transparent)]
    Pty(#[from] PtyError),
    #[error(transparent)]
    Diff(#[from] DiffError),
    #[error(transparent)]
    Transition(#[from] IllegalTransition),
    #[error(transparent)]
    Stage(#[from] IllegalStageTransition),
    #[error(transparent)]
    PlanTransition(#[from] IllegalPlanTransition),
    #[error(transparent)]
    RunTransition(#[from] IllegalRunTransition),
    #[error(transparent)]
    StageDoc(#[from] IllegalStageDocTransition),
    #[error(transparent)]
    StageProgress(#[from] IllegalStageProgressTransition),
    /// A store operation hit during a lifecycle move (the transactional
    /// plan-doc ingest, run-dispatch materialization) failed; the move never
    /// happened.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A rejected stage-gate precondition; the message is surfaced verbatim over RPC.
    #[error("{0}")]
    Gate(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("git command failed: {0}")]
    Git(String),
    /// A merge approval whose git work failed (conflict, wrong base checkout,
    /// nothing to commit). The `merge_failed:` prefix is the cross-stream contract
    /// the web client keys on to show the reason in the task banner; the state
    /// stays in review because the merge never happened.
    #[error("merge_failed: {0}")]
    MergeFailed(String),
}

/// Convert any git failure hit during a merge approval into [`OrchestratorError::MergeFailed`]
/// so the RPC message carries the contract's `merge_failed:` prefix.
fn as_merge_failure(error: OrchestratorError) -> OrchestratorError {
    match error {
        OrchestratorError::Git(reason) => OrchestratorError::MergeFailed(reason),
        already @ OrchestratorError::MergeFailed(_) => already,
        other => OrchestratorError::MergeFailed(other.to_string()),
    }
}

/// Advance one stage's sub-state; the stage owns the mutation, `stage_transition`
/// stays pure (same discipline as `Task::apply`).
fn apply_stage_event(stage: &mut Stage, event: StageEvent) -> Result<(), IllegalStageTransition> {
    stage.state = stage_transition(&stage.state, event)?;
    Ok(())
}

/// Merge a fresh manifest echo into the existing stage records by id: an id
/// that already exists keeps its lifecycle (`state`/`start_sha`/`validation`)
/// and takes the new `title`/`path`/`summary`; new ids append as `Planned`; ids
/// missing from the echo are dropped while still un-built (Planned/Approved)
/// but kept with a warning once building started — a plan revision must not
/// vaporize built work. On the first plan the merge is trivially "all new".
fn merge_stage_manifest(stages: &mut Vec<Stage>, entries: Vec<StageManifestEntry>) {
    let previous = std::mem::take(stages);
    let mut leftover: Vec<(usize, Stage)> = previous.into_iter().enumerate().collect();
    let mut merged: Vec<Stage> = Vec::with_capacity(entries.len());
    for entry in entries {
        match leftover.iter().position(|(_, stage)| stage.id == entry.id) {
            Some(position) => {
                let (_, mut existing) = leftover.remove(position);
                existing.title = entry.title;
                existing.path = entry.path;
                existing.summary = entry.summary;
                merged.push(existing);
            }
            None => merged.push(Stage::from_manifest(entry)),
        }
    }
    for (original_index, stage) in leftover {
        if matches!(stage.state, StageState::Planned | StageState::Approved) {
            continue; // never built; the revision dropped it.
        }
        eprintln!(
            "plan revision dropped stage {:?} which already has built work; keeping it",
            stage.id
        );
        merged.insert(original_index.min(merged.len()), stage);
    }
    *stages = merged;
}

/// Merge a fresh manifest echo into the plan's stage docs by id — the
/// plan-side successor of [`merge_stage_manifest`]: an id that already exists
/// keeps its review sub-state (`Planned`/`Approved`) and takes the new
/// `title`/`path`/`summary`; new ids append as `Planned`; ids missing from the
/// echo are dropped. The fused merge kept built stages here, but run-side
/// execution progress now lives on the run and is never deleted by a re-plan,
/// so the plan side only carries doc review and can drop freely.
fn merge_stage_docs(stages: &mut Vec<StageDoc>, entries: &[StageManifestEntry]) {
    let mut leftover = std::mem::take(stages);
    let mut merged: Vec<StageDoc> = Vec::with_capacity(entries.len());
    for entry in entries {
        match leftover.iter().position(|doc| doc.id == entry.id) {
            Some(position) => {
                let mut existing = leftover.remove(position);
                existing.title = entry.title.clone();
                existing.path = entry.path.clone();
                existing.summary = entry.summary.clone();
                merged.push(existing);
            }
            None => merged.push(StageDoc {
                id: entry.id.clone(),
                title: entry.title.clone(),
                path: entry.path.clone(),
                summary: entry.summary.clone(),
                state: StageDocState::Planned,
            }),
        }
    }
    *stages = merged;
}

/// The `done` wire still speaks the legacy validation shape (it dies with
/// `task.rs` in the final cutover); the run stores its own field-identical type.
fn run_validation_from_report(report: &crate::task::ValidationReport) -> RunValidationReport {
    RunValidationReport {
        passed: report.passed,
        findings: report.findings.clone(),
        notes_for_next_stage: report.notes_for_next_stage.clone(),
    }
}

/// Warm-session bookkeeping shared by [`ActivePlan`] and [`ActiveRun`]: the
/// live PTY session for the current phase plus the spawn generation the
/// agent-screen pump keys on. (The fused [`ActiveTask`] keeps its own copy of
/// these methods until the periphery retires it.)
#[derive(Default)]
pub struct SessionSlot {
    /// Counts session spawns (1-based; 0 = never spawned). The agent-screen
    /// pump keys off it so a viewer attached across a phase boundary gets
    /// exactly one pump per session, never a duplicate for the same one.
    generation: u64,
    /// The warm PTY session (None before dispatch / after end / after reattach).
    session: Option<PtySession>,
}

impl SessionSlot {
    /// The current spawn generation (0 = never spawned).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Subscribe to the live terminal stream, if a session is warm.
    pub fn subscribe(&self) -> Option<tokio::sync::broadcast::Receiver<Vec<u8>>> {
        self.session.as_ref().map(|s| s.subscribe())
    }

    /// The live session's generation + a fresh subscription, if one is warm —
    /// the agent-screen pump records the generation so the same session is
    /// never pumped twice.
    pub fn subscribe_with_generation(
        &self,
    ) -> Option<(u64, tokio::sync::broadcast::Receiver<Vec<u8>>)> {
        self.session
            .as_ref()
            .map(|s| (self.generation, s.subscribe()))
    }

    /// Write raw bytes (attached-terminal keystrokes) to the warm session; a
    /// dead session swallows them silently (the idle monitor's contract).
    pub fn write_input(&self, bytes: &[u8]) -> Result<(), OrchestratorError> {
        if let Some(session) = &self.session {
            session.write_input(bytes)?;
        }
        Ok(())
    }

    /// Like [`write_input`](Self::write_input) but a dead session is an error —
    /// the agent-tab contract surfaces "no active agent session" to the typer.
    pub fn write_input_strict(&self, bytes: &[u8]) -> Result<(), String> {
        match &self.session {
            Some(session) => session.write_input(bytes).map_err(|e| e.to_string()),
            None => Err("no active agent session".to_string()),
        }
    }

    /// Resize the live session's PTY, returning whether one was live. A dead
    /// session is a no-op `false` — the retained last agent screen must never
    /// be garbled by a dead resize.
    pub fn resize(&self, size: PtySize) -> Result<bool, OrchestratorError> {
        match &self.session {
            Some(session) => {
                session.resize(size)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Whether the phase's harness process has exited (crashed or finished
    /// without a `done`). `false` when no session is live (nothing to watch).
    pub fn harness_exited(&self) -> bool {
        self.session.as_ref().is_some_and(PtySession::has_exited)
    }

    /// The exit code of the phase's harness once it has exited. `None` while
    /// it is still running or no session is live.
    pub fn harness_exit_code(&self) -> Option<i32> {
        self.session.as_ref().and_then(PtySession::exit_code)
    }

    /// How long the phase's PTY has been silent, if a session is live — the
    /// quiescence signal that demotes to `idle_unreported` when no `done` arrives.
    pub fn harness_idle_for(&self) -> Option<std::time::Duration> {
        self.session.as_ref().map(PtySession::idle_for)
    }

    /// The harness's OS process id, if a session is live and running.
    pub fn harness_pid(&self) -> Option<u32> {
        self.session.as_ref().and_then(PtySession::pid)
    }

    /// Kill AND reap the phase's harness, dropping the session. Kill alone
    /// leaves a zombie per phase transition, which over a long-lived daemon
    /// exhausts the process table.
    pub fn end(&mut self) {
        if let Some(session) = self.session.take() {
            session.kill_and_reap();
        }
    }

    /// Take ownership of a freshly spawned session, bumping the generation.
    fn install(&mut self, session: PtySession) {
        self.generation += 1;
        self.session = Some(session);
    }
}

/// One plan in flight: its lifecycle state, its disposable planning worktree
/// (while one is alive), and its warm session. The canonical docs live in the
/// store — the worktree is throwaway scratch space for the plan agent.
pub struct ActivePlan {
    pub plan: Plan,
    /// The disposable planning worktree (branch `plan/<slug>`), kept warm
    /// through the notes/revision loop. `None` once torn down (approve /
    /// abandon) or before a revision re-dispatch re-creates one — the store
    /// docs are canonical either way.
    pub worktree: Option<Worktree>,
    /// The branch planning worktrees are cut from. Kept on the plan (not just
    /// the worktree) so a revision re-dispatch can re-create a worktree after
    /// teardown.
    pub base_branch: String,
    /// Where the plan doc lives, worktree-relative — convention by default,
    /// updated from `done` outputs (and fenced by the ingest).
    pub plan_path: String,
    /// Stage docs: manifest metadata + plan-side review sub-state. Empty for
    /// single-doc plans.
    pub stages: Vec<StageDoc>,
    /// Persisted per-stage plan comments (flat; each carries its stage_id).
    pub comments: Vec<PlanStageComment>,
    /// The stage a plan-revision session is (or was last) running for. Not
    /// persisted on the plan record — a restart falls back to a full re-plan.
    pub revising_stage_id: Option<String>,
    /// Which model/effort this plan's agents run on (None = harness default).
    pub model_choice: ModelChoice,
    /// The most recent `done` summary, surfaced on cards.
    pub last_summary: Option<String>,
    /// The most recent failure surfaced to the reviewer (unpersisted docs,
    /// harness crash). Cleared whenever the plan advances again.
    pub last_error: Option<String>,
    /// The warm PTY session for the current planning phase.
    pub session: SessionSlot,
}

impl ActivePlan {
    /// Reattach a plan recovered from the durable store after a daemon
    /// restart: the store docs are canonical, the PTY session is gone, and the
    /// disposable worktree may or may not have survived on disk. The caller
    /// (boot recovery) moves a working state to `Interrupted` itself.
    pub fn reattach(record: &PersistedPlan) -> Self {
        // A live planning worktree persists all three coordinates; a torn-down
        // one persists none. Anything partial is treated as torn down — the
        // store docs are canonical, so nothing is lost.
        let worktree = match (&record.worktree_name, &record.worktree_path, &record.branch) {
            (Some(name), Some(path), Some(branch)) => Some(Worktree {
                name: name.clone(),
                path: PathBuf::from(path),
                branch: branch.clone(),
                base_branch: record.base_branch.clone(),
            }),
            _ => None,
        };
        ActivePlan {
            plan: Plan {
                id: PlanId::new(record.id.clone()),
                goal: record.goal.clone(),
                state: record.state,
            },
            worktree,
            base_branch: record.base_branch.clone(),
            plan_path: record.plan_path.clone(),
            stages: record.stages.clone(),
            comments: record.comments.clone(),
            revising_stage_id: None,
            model_choice: ModelChoice {
                model: record.model.clone(),
                effort: record.effort.clone(),
            },
            last_summary: record.last_summary.clone(),
            last_error: record.last_error.clone(),
            session: SessionSlot::default(),
        }
    }

    /// A plan is multi-stage iff its stage-doc manifest is non-empty.
    pub fn is_multi_stage(&self) -> bool {
        !self.stages.is_empty()
    }

    /// Index of a stage doc in manifest (= execution) order.
    pub fn stage_doc_index(&self, stage_id: &str) -> Result<usize, String> {
        self.stages
            .iter()
            .position(|doc| doc.id == stage_id)
            .ok_or_else(|| format!("unknown stage_id: {stage_id}"))
    }

    /// Open comments on one stage, insertion order.
    pub fn open_comments_for(&self, stage_id: &str) -> Vec<&PlanStageComment> {
        self.comments
            .iter()
            .filter(|c| c.stage_id == stage_id && c.state == PlanCommentState::Open)
            .collect()
    }

    /// Mint the next comment id: "c-<n>", n = 1 + max numeric suffix among the
    /// plan's existing comment ids — so ids never collide after deletes.
    pub fn mint_comment_id(&self) -> String {
        let max_suffix = self
            .comments
            .iter()
            .filter_map(|c| c.id.strip_prefix("c-"))
            .filter_map(|n| n.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        format!("c-{}", max_suffix + 1)
    }
}

/// One run in flight: one implementation attempt — a worktree on a
/// `build/<slug>` branch, its lifecycle state, per-stage execution progress,
/// and the warm session. A quick task is a run whose `run.plan_id` is `None`.
pub struct ActiveRun {
    pub run: Run,
    pub worktree: Worktree,
    /// The "plan: <goal>" materialization commit recorded at dispatch — the
    /// baseline of the run's review diff, keeping the materialized docs out of
    /// review noise. `None` for quick/adopted/migrated runs (the diff falls
    /// back to the merge-base).
    pub base_sha: Option<String>,
    /// Worktree-relative path the build prompts point at: the owning plan's
    /// `plan_path`, or the convention default for quick runs. In-memory only —
    /// the caller re-derives it from the plan record on reattach.
    pub plan_path: String,
    /// Run-side per-stage execution progress, keyed by the plan's stage ids.
    /// A progress record exists only once its stage has been dispatched.
    pub stages: Vec<StageProgress>,
    /// The stage whose build/fix/validate session is (or was last) in flight.
    pub current_stage_id: Option<String>,
    /// The stage a mid-run revision session is running for (disambiguates
    /// store write-back from post-review changes on `done(revise)`).
    /// PERIPHERY: the mid-run revision flow itself lands with the stage flows.
    pub revising_stage_id: Option<String>,
    /// "Run all": auto-dispatch the next approved stage when validation passes.
    pub auto_advance: bool,
    /// True for a run minted around a pre-existing (user-created) worktree.
    pub adopted: bool,
    /// One-shot continuation flag: set at adoption, consumed by the first
    /// session spawn afterwards.
    pub pending_continuation: bool,
    /// Which model/effort this run's agents run on (None = harness default).
    pub model_choice: ModelChoice,
    /// The most recent `done` summary, surfaced on cards.
    pub last_summary: Option<String>,
    /// The most recent failure surfaced to the reviewer (merge failure,
    /// harness crash). Cleared whenever the run advances again.
    pub last_error: Option<String>,
    /// The warm PTY session for the current build phase.
    pub session: SessionSlot,
}

impl ActiveRun {
    /// Reattach a run recovered from the durable store after a daemon restart:
    /// the worktree survived on disk, the PTY session did not. `plan_path` is
    /// re-derived by the caller from the owning plan's record (quick runs pass
    /// the convention default). The caller (boot recovery) moves a working
    /// state to `Interrupted` itself.
    pub fn reattach(record: &PersistedRun, plan_path: String) -> Self {
        ActiveRun {
            run: Run {
                id: RunId::new(record.id.clone()),
                plan_id: record.plan_id.clone().map(PlanId::new),
                goal: record.goal.clone(),
                state: record.state,
            },
            worktree: Worktree {
                name: record.worktree_name.clone(),
                path: PathBuf::from(&record.worktree_path),
                branch: record.branch.clone(),
                base_branch: record.base_branch.clone(),
            },
            base_sha: record.base_sha.clone(),
            plan_path,
            stages: record.stages.clone(),
            current_stage_id: record.current_stage_id.clone(),
            revising_stage_id: record.revising_stage_id.clone(),
            auto_advance: record.auto_advance,
            adopted: record.adopted,
            pending_continuation: record.pending_continuation,
            model_choice: ModelChoice {
                model: record.model.clone(),
                effort: record.effort.clone(),
            },
            last_summary: record.last_summary.clone(),
            last_error: record.last_error.clone(),
            session: SessionSlot::default(),
        }
    }

    /// This run's progress record for one stage, if the stage was dispatched.
    pub fn stage_progress(&self, stage_id: &str) -> Option<&StageProgress> {
        self.stages.iter().find(|p| p.stage_id == stage_id)
    }

    fn stage_progress_index(&self, stage_id: &str) -> Option<usize> {
        self.stages.iter().position(|p| p.stage_id == stage_id)
    }
}

/// What a run implements: an approved plan (docs materialized from the store)
/// or a quick goal that goes straight to a build agent.
pub enum RunSource<'a> {
    /// A quick task: no plan gate, no materialization; the review diff falls
    /// back to merge-base semantics.
    Quick { goal: &'a str },
    /// Implement an approved plan. `has_active_run` is the caller's
    /// active-runs view (the orchestrator holds no app-level maps): `true`
    /// when the plan already has a non-terminal run, which rejects the
    /// dispatch — the single-active-writer rule.
    Plan {
        plan: &'a ActivePlan,
        has_active_run: bool,
    },
}

/// PERIPHERY: one FUSED task in flight — the pre-split spine `app.rs` still
/// speaks. Dies with `task.rs` once the periphery stage moves the handlers
/// onto [`ActivePlan`]/[`ActiveRun`].
///
/// One task in flight: its lifecycle state, its worktree, and its warm session.
pub struct ActiveTask {
    pub task: Task,
    pub worktree: Worktree,
    /// Where the plan lives — convention by default, updated from `done` outputs.
    pub plan_path: String,
    /// The most recent `done` summary, surfaced on cards.
    pub last_summary: Option<String>,
    /// Which model/effort this task's agents run on (None = harness default).
    pub model_choice: ModelChoice,
    /// The most recent failure surfaced to the reviewer (merge failure, harness
    /// crash). Set by the app layer; cleared here whenever the task advances again.
    pub last_error: Option<String>,
    /// Multi-stage plan: manifest + per-stage sub-state. Empty = legacy path.
    pub stages: Vec<Stage>,
    /// The stage whose build/fix/validate session is (or was last) in flight.
    pub current_stage_id: Option<String>,
    /// The stage a plan-revision session is running for (routes Interrupted(Plan)).
    pub revising_stage_id: Option<String>,
    /// "Run all": auto-dispatch the next approved stage when validation passes.
    pub auto_advance: bool,
    /// Persisted per-stage plan comments (flat; each carries its stage_id).
    pub comments: Vec<StageComment>,
    /// True for a task minted around a pre-existing (user-created) worktree.
    /// Gates pruning (spec §0.5), release, and boot-recovery parking.
    pub adopted: bool,
    /// One-shot continuation flag (spec §0.7): set at adoption, consumed by the
    /// first session spawn afterwards.
    pub pending_continuation: bool,
    /// Counts session spawns for this task (1-based; 0 = never spawned). The
    /// agent-screen pump keys off it so a viewer attached across a phase boundary
    /// gets exactly one pump per session, never a duplicate for the same one.
    pub session_generation: u64,
    /// The warm PTY session for the current phase (None before dispatch/after end).
    session: Option<PtySession>,
}

impl ActiveTask {
    /// Reattach a task recovered from the durable store after a daemon restart:
    /// the worktree survived on disk, the PTY session did not. The caller (boot
    /// recovery) has already moved a working state to `Interrupted`.
    #[allow(clippy::too_many_arguments)] // mirrors the persisted record field-for-field
    pub fn reattach(
        task: Task,
        worktree: Worktree,
        plan_path: String,
        last_summary: Option<String>,
        model_choice: ModelChoice,
        last_error: Option<String>,
        stages: Vec<Stage>,
        current_stage_id: Option<String>,
        revising_stage_id: Option<String>,
        auto_advance: bool,
        comments: Vec<StageComment>,
        adopted: bool,
        pending_continuation: bool,
    ) -> Self {
        ActiveTask {
            task,
            worktree,
            plan_path,
            last_summary,
            model_choice,
            last_error,
            stages,
            current_stage_id,
            revising_stage_id,
            auto_advance,
            comments,
            adopted,
            pending_continuation,
            session_generation: 0,
            session: None,
        }
    }

    /// A task is multi-stage iff its manifest is non-empty (spec §0.4).
    pub fn is_multi_stage(&self) -> bool {
        !self.stages.is_empty()
    }

    /// The stage with `id`, in manifest order.
    pub fn stage(&self, stage_id: &str) -> Result<&Stage, String> {
        self.stages
            .iter()
            .find(|s| s.id == stage_id)
            .ok_or_else(|| format!("unknown stage_id: {stage_id}"))
    }

    pub fn stage_mut(&mut self, stage_id: &str) -> Result<&mut Stage, String> {
        self.stages
            .iter_mut()
            .find(|s| s.id == stage_id)
            .ok_or_else(|| format!("unknown stage_id: {stage_id}"))
    }

    /// Index of a stage in manifest (= execution) order.
    pub fn stage_index(&self, stage_id: &str) -> Result<usize, String> {
        self.stages
            .iter()
            .position(|s| s.id == stage_id)
            .ok_or_else(|| format!("unknown stage_id: {stage_id}"))
    }

    /// Open comments on one stage, insertion order.
    pub fn open_comments_for(&self, stage_id: &str) -> Vec<&StageComment> {
        self.comments
            .iter()
            .filter(|c| c.stage_id == stage_id && c.state == CommentState::Open)
            .collect()
    }

    /// Mint the next comment id: "c-<n>", n = 1 + max numeric suffix among the
    /// task's existing comment ids — so ids never collide after deletes.
    pub fn mint_comment_id(&self) -> String {
        let max_suffix = self
            .comments
            .iter()
            .filter_map(|c| c.id.strip_prefix("c-"))
            .filter_map(|n| n.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        format!("c-{}", max_suffix + 1)
    }

    /// Subscribe to the live terminal stream, if a session is warm.
    pub fn subscribe(&self) -> Option<tokio::sync::broadcast::Receiver<Vec<u8>>> {
        self.session.as_ref().map(|s| s.subscribe())
    }

    /// The live session's generation + a fresh subscription, if one is warm.
    /// The agent-screen pump records the generation so the same session is
    /// never pumped twice (the existing [`subscribe`](Self::subscribe) stays
    /// for the idle monitor).
    pub fn subscribe_with_generation(
        &self,
    ) -> Option<(u64, tokio::sync::broadcast::Receiver<Vec<u8>>)> {
        self.session
            .as_ref()
            .map(|s| (self.session_generation, s.subscribe()))
    }

    /// Write raw bytes (attached-terminal keystrokes) to the warm session.
    pub fn write_input(&self, bytes: &[u8]) -> Result<(), OrchestratorError> {
        if let Some(session) = &self.session {
            session.write_input(bytes)?;
        }
        Ok(())
    }

    /// Like [`write_input`](Self::write_input) but a dead session is an error —
    /// the agent-tab contract surfaces "no active agent session" to the typer
    /// instead of silently swallowing keystrokes.
    pub fn write_input_strict(&self, bytes: &[u8]) -> Result<(), String> {
        match &self.session {
            Some(session) => session.write_input(bytes).map_err(|e| e.to_string()),
            None => Err("no active agent session".to_string()),
        }
    }

    /// Resize the live session's PTY, returning whether one was live. A dead
    /// session is a no-op `false` — the retained last agent screen must never
    /// be garbled by a dead resize.
    pub fn resize_session(&self, size: PtySize) -> Result<bool, OrchestratorError> {
        match &self.session {
            Some(session) => {
                session.resize(size)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Whether the phase's harness process has exited (crashed or finished without
    /// a `done`). `false` when no session is live (nothing to watch).
    pub fn harness_exited(&self) -> bool {
        self.session.as_ref().is_some_and(PtySession::has_exited)
    }

    /// The exit code of the phase's harness once it has exited — the `N` in the
    /// "agent exited unexpectedly (exit code N)" attention message. `None` while it
    /// is still running or no session is live.
    pub fn harness_exit_code(&self) -> Option<i32> {
        self.session.as_ref().and_then(PtySession::exit_code)
    }

    /// How long the phase's PTY has been silent, if a session is live — the
    /// quiescence signal that demotes to `idle_unreported` when no `done` arrives.
    pub fn harness_idle_for(&self) -> Option<std::time::Duration> {
        self.session.as_ref().map(PtySession::idle_for)
    }

    /// The harness's OS process id, if a session is live and running.
    pub fn harness_pid(&self) -> Option<u32> {
        self.session.as_ref().and_then(PtySession::pid)
    }

    /// Kill and reap the phase's harness, dropping the session. Used when a task is
    /// torn down (deleted) while it may still hold a live PTY — a Failed task keeps
    /// its session alive for replies, so deleting one must not leak the process.
    pub fn end_session(&mut self) {
        if let Some(session) = self.session.take() {
            session.kill_and_reap();
        }
    }
}

/// Per-spawn context a one-shot harness builder may honor.
#[derive(Debug, Clone, Copy, Default)]
pub struct SpawnOptions {
    /// Resume the harness's own most-recent conversation for this cwd
    /// (claude: `--continue`). Set only for the first session after adoption.
    pub continue_session: bool,
}

/// Builds the one-shot harness command for a rendered prompt + model + context.
pub type OneShotBuilder =
    std::sync::Arc<dyn Fn(&str, &ModelChoice, &SpawnOptions) -> HarnessSpec + Send + Sync>;

/// Whether the harness has an existing conversation transcript for a worktree
/// cwd. Injectable so tests never touch the real home directory.
pub type TranscriptProbe = std::sync::Arc<dyn Fn(&Path) -> bool + Send + Sync>;

/// How the orchestrator launches an agent for a phase.
#[derive(Clone)]
pub enum Agent {
    /// A warm interactive session: spawn the binary, then write the prompt to its
    /// PTY. Supports in-session revision rounds (the QA harness uses this).
    Warm(HarnessSpec),
    /// One-shot: build the full spawn command from the rendered prompt (e.g.
    /// `claude -p "<prompt>"`). The agent runs, does the work, reports `done`, and
    /// exits — no warm session.
    OneShot(OneShotBuilder),
}

/// Owns project configuration and drives plans and runs through their
/// lifecycles.
pub struct Orchestrator {
    repo_path: PathBuf,
    /// Run (and legacy task) worktrees: `build/<slug>` branches.
    worktrees: WorktreeManager,
    /// Disposable planning worktrees: `plan/<slug>` branches, same root.
    plan_worktrees: WorktreeManager,
    agent: Agent,
    templates: Templates,
    pty_size: PtySize,
    /// Decides whether an adopted task's first session may continue the
    /// harness's prior conversation. Defaults to "never" — the app layer opts in.
    transcript_probe: TranscriptProbe,
}

impl Orchestrator {
    pub fn new(
        repo_path: impl Into<PathBuf>,
        worktrees_root: impl Into<PathBuf>,
        agent: Agent,
        templates: Templates,
    ) -> Self {
        let repo_path = repo_path.into();
        let worktrees_root = worktrees_root.into();
        let worktrees = WorktreeManager::new(repo_path.clone(), worktrees_root.clone());
        let plan_worktrees = WorktreeManager::new(repo_path.clone(), worktrees_root)
            .with_branch_prefix(PLAN_BRANCH_PREFIX);
        Orchestrator {
            repo_path,
            worktrees,
            plan_worktrees,
            agent,
            templates,
            pty_size: PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            },
            transcript_probe: std::sync::Arc::new(|_| false),
        }
    }

    /// Opt in to harness-conversation continuation for adopted tasks: `probe`
    /// answers whether a transcript exists for a worktree cwd.
    pub fn with_transcript_probe(mut self, probe: TranscriptProbe) -> Self {
        self.transcript_probe = probe;
        self
    }

    // ---- Plan seams (Plan/Run split) --------------------------------------

    /// Dispatch a plan: create the disposable planning worktree (branch
    /// `plan/<slug>`), scaffold `.build/` (the MCP config carries the plan id
    /// so `done` reports route back to this plan), transition out of
    /// `Created`, and spawn the plan session. The worktree is throwaway — the
    /// canonical docs land in the store at each plan/revise `done` — but it
    /// stays warm through the notes/revision loop (the scope doc's
    /// warm-session property).
    pub fn dispatch_plan(
        &self,
        id: PlanId,
        goal: impl Into<String>,
        base_branch: &str,
        model_choice: ModelChoice,
    ) -> Result<ActivePlan, OrchestratorError> {
        let goal = goal.into();
        let slug = slugify(&goal);
        let worktree = self.plan_worktrees.create(&slug, base_branch)?;
        self.scaffold_build_dir(&worktree, &id.0)?;

        let mut plan = Plan::new(id, goal);
        plan.apply(PlanEvent::Dispatch)?;

        let mut active = ActivePlan {
            plan,
            worktree: Some(worktree),
            base_branch: base_branch.to_string(),
            plan_path: DEFAULT_PLAN_PATH.to_string(),
            stages: Vec::new(),
            comments: Vec::new(),
            revising_stage_id: None,
            model_choice,
            last_summary: None,
            last_error: None,
            session: SessionSlot::default(),
        };
        let prompt = self.render_plan(&self.templates.plan, &active, "");
        self.spawn_plan_session(&mut active, &prompt)?;
        Ok(active)
    }

    /// Consume a plan agent's `done` report. Doc persistence is TRANSACTIONAL:
    /// the worktree docs are ingested into the store BEFORE the lifecycle
    /// transition fires, so an ingest failure errors the `done`, leaves the
    /// plan in its working state, and surfaces on the card via `last_error` —
    /// a plan never advances with unpersisted docs.
    pub fn on_plan_done(
        &self,
        active: &mut ActivePlan,
        store: &Store,
        report: DoneReport,
    ) -> Result<(), OrchestratorError> {
        match (report.phase, report.status) {
            // A blocked/failed report from any plan-side session parks the plan.
            (_, DoneStatus::Blocked) => {
                active.plan.apply(PlanEvent::Blocked)?;
            }
            (_, DoneStatus::Failed) => {
                active.plan.apply(PlanEvent::Failed)?;
            }
            (DonePhase::Plan, DoneStatus::Completed) => {
                // Legality FIRST: a stray plan report must be rejected with
                // zero mutation — the caller persists the plan even on Err, so
                // a manifest merged (or docs ingested) before the check would
                // smuggle agent output past a closed gate.
                plan_transition(&active.plan.state, PlanEvent::PlanReady)?;
                let plan_path = report
                    .outputs
                    .plan_path
                    .clone()
                    .unwrap_or_else(|| active.plan_path.clone());
                self.ingest_plan_docs_transactionally(active, store, &plan_path)?;
                if let Some(entries) = &report.outputs.stages {
                    if !entries.is_empty() {
                        merge_stage_docs(&mut active.stages, entries);
                    }
                }
                active.plan.apply(PlanEvent::PlanReady)?;
                // The reported path was fenced and ingested; adopt it.
                active.plan_path = plan_path;
            }
            // A per-stage plan-revision session completed: the doc changed
            // (any prior approval is stale) and the agent's per-comment
            // resolutions land on the stored comments.
            (DonePhase::Revise, DoneStatus::Completed) => {
                self.consume_plan_stage_revision(active, store, &report)?;
            }
            (DonePhase::Build | DonePhase::Validate, DoneStatus::Completed) => {
                return Err(OrchestratorError::Gate(format!(
                    "a planning session reported phase={:?}; plans only accept plan/revise \
                     reports",
                    report.phase
                )));
            }
        }
        // Only a consumed report leaves a trace: the surfaced summary and the
        // clearing of any stale error land strictly after the arms above
        // succeeded.
        active.last_summary = Some(report.summary.clone());
        active.last_error = None;
        Ok(())
    }

    /// The transactional half of every plan/revise `done`: copy the worktree
    /// docs into the store, or fail the report with the reason surfaced on the
    /// card. Setting `last_error` here is the one deliberate mutation on the
    /// error path — the plan stays in its working state, but the reviewer must
    /// see why the gate never opened.
    fn ingest_plan_docs_transactionally(
        &self,
        active: &mut ActivePlan,
        store: &Store,
        plan_path: &str,
    ) -> Result<(), OrchestratorError> {
        let Some(worktree) = &active.worktree else {
            let reason = "plan docs were not persisted: the planning worktree is gone".to_string();
            active.last_error = Some(reason.clone());
            return Err(OrchestratorError::Gate(reason));
        };
        if let Err(ingest_error) =
            store.ingest_plan_docs(&active.plan.id.0, &worktree.path, plan_path)
        {
            active.last_error = Some(format!("plan docs were not persisted: {ingest_error}"));
            return Err(OrchestratorError::Store(ingest_error));
        }
        Ok(())
    }

    /// A per-stage plan-revision session completed (plan-side `done(revise)`).
    /// Probes every transition before committing any, then ingests the revised
    /// docs transactionally, then lets the state moves land.
    fn consume_plan_stage_revision(
        &self,
        active: &mut ActivePlan,
        store: &Store,
        report: &DoneReport,
    ) -> Result<(), OrchestratorError> {
        let stage_id = active.revising_stage_id.clone().ok_or_else(|| {
            OrchestratorError::Gate(
                "revise report for a plan with no stage revision in flight".to_string(),
            )
        })?;
        let index = active
            .stage_doc_index(&stage_id)
            .map_err(OrchestratorError::Gate)?;
        stage_doc_transition(&active.stages[index].state, StageDocEvent::Revised)?;
        plan_transition(&active.plan.state, PlanEvent::PlanReady)?;
        let plan_path = active.plan_path.clone();
        self.ingest_plan_docs_transactionally(active, store, &plan_path)?;

        active.stages[index].state =
            stage_doc_transition(&active.stages[index].state, StageDocEvent::Revised)?;
        active.plan.apply(PlanEvent::PlanReady)?;
        if let Some(resolutions) = &report.outputs.comment_resolutions {
            for resolution in resolutions {
                let matching = active.comments.iter_mut().find(|c| {
                    c.id == resolution.comment_id
                        && c.stage_id == stage_id
                        && c.state == PlanCommentState::Open
                });
                match matching {
                    Some(comment) => {
                        comment.state = PlanCommentState::Addressed;
                        comment.agent_reply = Some(resolution.response.clone());
                    }
                    None => eprintln!(
                        "stage revision for {stage_id}: unknown or non-open comment {:?}; skipping",
                        resolution.comment_id
                    ),
                }
            }
        }
        active.revising_stage_id = None;
        Ok(())
    }

    /// The quiescence timer fired without a `done`: demote to `idle_unreported`.
    pub fn on_plan_idle(&self, active: &mut ActivePlan) -> Result<(), OrchestratorError> {
        active.plan.apply(PlanEvent::WentIdle)?;
        Ok(())
    }

    /// Approve the plan: the last human gate. The plan rests at `Approved`
    /// (the store docs are canonical) and the disposable planning worktree is
    /// torn down — worktree AND branch. Teardown failure is an error, not a
    /// shrug: the plan stays at `PlanReview` so a re-approve retries the
    /// teardown, because a leaked planning worktree would linger as a stray.
    pub fn approve_plan(&self, active: &mut ActivePlan) -> Result<(), OrchestratorError> {
        // Pure legality first — nothing is torn down for an illegal approve.
        plan_transition(&active.plan.state, PlanEvent::Approve)?;
        active.session.end();
        if let Some(worktree) = &active.worktree {
            self.worktrees.remove(worktree, /* keep_branch */ false)?;
        }
        active.worktree = None;
        active.plan.apply(PlanEvent::Approve)?;
        active.last_error = None;
        Ok(())
    }

    /// Submit a batch of plan notes: re-plan against them in a **fresh**
    /// session (cold-agent discipline, as everywhere). The planning worktree
    /// is kept warm through the notes loop; when it was torn down or vanished
    /// (interrupted plans), a fresh disposable worktree is created and the
    /// canonical docs are re-materialized from the store first.
    pub fn send_plan_notes(
        &self,
        active: &mut ActivePlan,
        store: &Store,
        notes: &str,
    ) -> Result<(), OrchestratorError> {
        plan_transition(&active.plan.state, PlanEvent::SendNotes)?;
        self.ensure_planning_worktree(active, store)?;
        active.plan.apply(PlanEvent::SendNotes)?;
        active.last_error = None;
        active.session.end();
        let prompt = self.render_plan(&self.templates.revise, active, notes);
        self.spawn_plan_session(active, &prompt)?;
        Ok(())
    }

    /// Make sure the plan has a live planning worktree, re-creating one (with
    /// the canonical docs materialized) when it was torn down or vanished from
    /// disk. A vanished worktree's stale git bookkeeping is pruned best-effort
    /// so the fresh worktree's name/branch never collide with the carcass.
    fn ensure_planning_worktree(
        &self,
        active: &mut ActivePlan,
        store: &Store,
    ) -> Result<(), OrchestratorError> {
        if let Some(worktree) = &active.worktree {
            if worktree.path.exists() {
                return Ok(());
            }
            if let Err(cleanup) = self.worktrees.remove(worktree, /* keep_branch */ false) {
                eprintln!(
                    "plan {}: pruning the vanished planning worktree {} failed: {cleanup}",
                    active.plan.id.0, worktree.name
                );
            }
            active.worktree = None;
        }
        let slug = slugify(&active.plan.goal);
        let worktree = self.plan_worktrees.create(&slug, &active.base_branch)?;
        let prepared = self
            .scaffold_build_dir(&worktree, &active.plan.id.0)
            .and_then(|()| {
                store
                    .materialize_plan_docs(&active.plan.id.0, &worktree.path)
                    .map_err(OrchestratorError::from)
            });
        if let Err(error) = prepared {
            // Nothing references the half-prepared worktree yet; don't leak it.
            self.discard_worktree(&worktree);
            return Err(error);
        }
        active.worktree = Some(worktree);
        Ok(())
    }

    // ---- Run seams (Plan/Run split) ----------------------------------------

    /// Dispatch a run: create the `build/<slug>` worktree, scaffold `.build/`
    /// (the MCP config carries the run id), and spawn the first build session.
    ///
    /// A planned run additionally materializes the plan's canonical docs from
    /// the store into the fresh worktree and commits them ("plan: <goal>" —
    /// the intent record the scope doc keeps through merge); that commit is
    /// recorded as the run's `base_sha`, the baseline of the review diff, so
    /// the materialized docs never show up as review noise. A quick run skips
    /// materialization and its diff falls back to merge-base semantics.
    ///
    /// Single-active-writer: at most one active run per plan. The caller owns
    /// the runs map, so it passes its view via
    /// [`RunSource::Plan::has_active_run`]; `true` rejects the dispatch before
    /// anything is created.
    ///
    /// A multi-stage plan's first session is its first stage's build — the
    /// plan-level `Approved` gate covers starting stage one; later stages
    /// dispatch from the stage gate.
    /// PERIPHERY: the stage-gate dispatch (StageGate → next stage / fix
    /// session) lands with the stage flows.
    pub fn dispatch_run(
        &self,
        id: RunId,
        source: RunSource<'_>,
        base_branch: &str,
        model_choice: ModelChoice,
        store: &Store,
    ) -> Result<ActiveRun, OrchestratorError> {
        let plan_link = match &source {
            RunSource::Quick { .. } => None,
            RunSource::Plan {
                plan,
                has_active_run,
            } => {
                if plan.plan.state != PlanState::Approved {
                    return Err(OrchestratorError::Gate(format!(
                        "only an approved plan can be implemented (plan {} is {:?})",
                        plan.plan.id.0, plan.plan.state
                    )));
                }
                if *has_active_run {
                    return Err(OrchestratorError::Gate(format!(
                        "plan {} already has an active run — a second concurrent run is \
                         rejected (single-active-writer)",
                        plan.plan.id.0
                    )));
                }
                Some(*plan)
            }
        };
        let goal = match (&source, plan_link) {
            (RunSource::Quick { goal }, _) => goal.to_string(),
            (_, Some(plan)) => plan.plan.goal.clone(),
            (RunSource::Plan { .. }, None) => unreachable!("a planned source always links"),
        };

        let slug = slugify(&goal);
        let worktree = self.worktrees.create(&slug, base_branch)?;
        self.scaffold_build_dir(&worktree, &id.0)?;
        let base_sha = match plan_link {
            Some(plan) => {
                match self.materialize_and_commit_plan_docs(plan, &worktree, &goal, store) {
                    Ok(sha) => Some(sha),
                    Err(error) => {
                        // Nothing has been handed to the caller; don't leak
                        // the half-prepared worktree.
                        self.discard_worktree(&worktree);
                        return Err(error);
                    }
                }
            }
            None => None,
        };

        let mut run = Run::new(id, plan_link.map(|plan| plan.plan.id.clone()), goal);
        run.apply(RunEvent::Dispatch)?;

        let mut active = ActiveRun {
            run,
            worktree,
            base_sha,
            plan_path: plan_link
                .map(|plan| plan.plan_path.clone())
                .unwrap_or_else(|| DEFAULT_PLAN_PATH.to_string()),
            stages: Vec::new(),
            current_stage_id: None,
            revising_stage_id: None,
            auto_advance: false,
            adopted: false,
            pending_continuation: false,
            model_choice,
            last_summary: None,
            last_error: None,
            session: SessionSlot::default(),
        };

        // Multi-stage plan → the first stage's build session (progress record
        // created, stage diff pinned to the materialization commit); anything
        // else → the whole-plan/quick build prompt.
        let prompt = match plan_link.filter(|plan| plan.is_multi_stage()) {
            Some(plan) => {
                let first_stage = &plan.stages[0];
                active.current_stage_id = Some(first_stage.id.clone());
                let mut progress = StageProgress::dispatched(&first_stage.id);
                progress.start_sha = active.base_sha.clone();
                active.stages.push(progress);
                self.render_run_stage(&self.templates.build_stage, &active, &plan.stages, 0, "")
            }
            None => self.render_run(&self.templates.build, &active, ""),
        };
        self.spawn_run_session(&mut active, &prompt)?;
        Ok(active)
    }

    /// Materialize a plan's canonical docs into a fresh run worktree and
    /// commit them, returning the commit sha that baselines the run's review
    /// diff.
    fn materialize_and_commit_plan_docs(
        &self,
        plan: &ActivePlan,
        worktree: &Worktree,
        goal: &str,
        store: &Store,
    ) -> Result<String, OrchestratorError> {
        store.materialize_plan_docs(&plan.plan.id.0, &worktree.path)?;
        self.commit_all_with_message(&worktree.path, &format!("plan: {goal}"))?;
        Ok(self
            .git(&worktree.path, &["rev-parse", "HEAD"])?
            .trim()
            .to_string())
    }

    /// Consume a build-side agent's `done` report for a run. `plan_stage_docs`
    /// is the owning plan's stage-doc manifest (the caller joins by `plan_id`;
    /// empty for quick runs and single-doc plans), which routes multi-stage
    /// reports through the stage pipeline and supplies the validation prompt's
    /// stage metadata.
    ///
    /// PERIPHERY: a mid-run stage-revision `done` (`revising_stage_id` set)
    /// must ingest the revised docs back into the store and reset the
    /// plan-side doc state — that write-back lands with the stage flows.
    pub fn on_run_done(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
        report: DoneReport,
    ) -> Result<(), OrchestratorError> {
        match (report.phase, report.status) {
            // A blocked/failed report from any session — stage build, fix,
            // validation, or single-plan — parks the run and disarms run-all.
            (_, DoneStatus::Blocked) => {
                active.run.apply(RunEvent::Blocked)?;
                active.auto_advance = false;
            }
            (_, DoneStatus::Failed) => {
                active.run.apply(RunEvent::Failed)?;
                active.auto_advance = false;
            }
            (DonePhase::Plan, DoneStatus::Completed) => {
                // Plan reports belong to plans; consuming one here would let
                // any in-flight build session smuggle manifest/doc edits.
                return Err(OrchestratorError::Gate(
                    "a run session reported phase=plan; plan reports belong to plans".to_string(),
                ));
            }
            (DonePhase::Build | DonePhase::Revise, DoneStatus::Completed)
                if !plan_stage_docs.is_empty() =>
            {
                self.on_run_stage_session_done(active, plan_stage_docs)?;
            }
            (DonePhase::Validate, DoneStatus::Completed) => {
                self.on_run_validation_done(active, plan_stage_docs, &report)?;
            }
            // Single-plan / quick path: a completed build opens review.
            (DonePhase::Build | DonePhase::Revise, DoneStatus::Completed) => {
                active.run.apply(RunEvent::BuildReady)?;
            }
        }
        // Only a consumed report leaves a trace (same discipline as plans).
        active.last_summary = Some(report.summary.clone());
        active.last_error = None;
        Ok(())
    }

    /// A stage build/fix session reported done(completed): commit the stage's
    /// work and hand it to a fresh validation session. No run-level event —
    /// the run stays `Building` until validation's verdict moves it.
    fn on_run_stage_session_done(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
    ) -> Result<(), OrchestratorError> {
        let Some(stage_id) = active.current_stage_id.clone() else {
            eprintln!(
                "on_run_done {}: build report for a multi-stage run with no current stage; \
                 ignoring",
                active.run.id.0
            );
            return Ok(());
        };
        // Resolve both sides of the stage join up front, so a mismatch between
        // the plan's manifest and the run's progress rejects before mutation.
        let doc_index = plan_stage_docs
            .iter()
            .position(|doc| doc.id == stage_id)
            .ok_or_else(|| {
                OrchestratorError::Gate(format!("stage {stage_id} is not in the plan's stage docs"))
            })?;
        let Some(progress_index) = active.stage_progress_index(&stage_id) else {
            eprintln!(
                "on_run_done {}: no progress record for stage {stage_id}; ignoring",
                active.run.id.0
            );
            return Ok(());
        };
        match active.stages[progress_index].state {
            StageProgressState::Building => {
                // Coarse-state legality before ANY mutation: a report landing
                // while the run is Blocked/Failed must be rejected atomically —
                // the stage advance, commit, and session swap below would
                // otherwise leave the run and stage machines incoherent (the
                // caller persists the run even on Err).
                run_transition(&active.run.state, RunEvent::BuildReady)?;
            }
            // A build report while the validation agent runs would skip the
            // gate; only a `validate` report may move a Validating stage.
            StageProgressState::Validating | StageProgressState::Built => {
                eprintln!(
                    "on_run_done {}: stage {stage_id} is awaiting validation; ignoring a \
                     non-validate report",
                    active.run.id.0
                );
                return Ok(());
            }
            // Post-review change requests run while the current stage is
            // already validated; their `done` closes the loop exactly as on
            // the single-plan path.
            StageProgressState::Validated { .. } => {
                active.run.apply(RunEvent::BuildReady)?;
                return Ok(());
            }
        }
        active.stages[progress_index].apply(StageProgressEvent::BuildDone)?;
        // The agent authors the stage's atomic commits; this is only a safety
        // net (a no-op on a clean tree) — but it stays load-bearing: it
        // GUARANTEES a committed boundary before the validation gate's
        // `git diff {stage_start_sha}` and before the next stage captures HEAD.
        self.commit_all_with_message(
            &active.worktree.path,
            &format!("Build: stage {stage_id} — checkpoint (swept by Build)"),
        )?;
        active.stages[progress_index].apply(StageProgressEvent::StartValidation)?;
        active.session.end();
        let prompt = self.render_run_stage(
            &self.templates.validate,
            active,
            plan_stage_docs,
            doc_index,
            "",
        );
        self.spawn_run_session(active, &prompt)?;
        Ok(())
    }

    /// The validation agent's verdict. Pass: the final stage opens merge
    /// review, an inner stage parks the run at the stage gate. Fail: the stage
    /// gate with run-all disarmed; the stored report drives the fix session.
    ///
    /// PERIPHERY: with run-all armed and a mid-plan pass, the fused path
    /// auto-dispatched the next approved stage here; on the split that
    /// dispatch (StageGate → Building) lands with the stage flows.
    fn on_run_validation_done(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
        report: &DoneReport,
    ) -> Result<(), OrchestratorError> {
        if plan_stage_docs.is_empty() {
            eprintln!(
                "on_run_done {}: validate report for a run without stages; ignoring",
                active.run.id.0
            );
            return Ok(());
        }
        let Some(stage_id) = active.current_stage_id.clone() else {
            eprintln!(
                "on_run_done {}: validate report with no current stage; ignoring",
                active.run.id.0
            );
            return Ok(());
        };
        let doc_index = plan_stage_docs
            .iter()
            .position(|doc| doc.id == stage_id)
            .ok_or_else(|| {
                OrchestratorError::Gate(format!("stage {stage_id} is not in the plan's stage docs"))
            })?;
        let Some(progress_index) = active.stage_progress_index(&stage_id) else {
            eprintln!(
                "on_run_done {}: no progress record for stage {stage_id}; ignoring",
                active.run.id.0
            );
            return Ok(());
        };
        if active.stages[progress_index].state != StageProgressState::Validating {
            eprintln!(
                "on_run_done {}: stage {stage_id} is not validating; ignoring a validate report",
                active.run.id.0
            );
            return Ok(());
        }
        let validation = report
            .outputs
            .validation
            .clone()
            .expect("mcp validated: outputs.validation present on phase=validate/completed");
        let passed = validation.passed;
        let last_stage = doc_index + 1 == plan_stage_docs.len();
        let verdict = if passed {
            RunEvent::ValidationPassed { last_stage }
        } else {
            RunEvent::ValidationFailed
        };
        // Coarse-state legality before ANY mutation: a verdict landing while
        // the run is Blocked/Failed must be rejected atomically — advancing
        // the stage to its terminal Validated and killing the session here
        // would strand the run (the caller persists it even on Err).
        run_transition(&active.run.state, verdict)?;
        active.stages[progress_index].apply(StageProgressEvent::ValidationDone { passed })?;
        active.stages[progress_index].validation = Some(run_validation_from_report(&validation));
        active.session.end();
        active.run.apply(verdict)?;
        if !passed {
            active.auto_advance = false;
        }
        Ok(())
    }

    /// The quiescence timer fired without a `done`: demote to `idle_unreported`.
    pub fn on_run_idle(&self, active: &mut ActiveRun) -> Result<(), OrchestratorError> {
        active.run.apply(RunEvent::WentIdle)?;
        Ok(())
    }

    /// The run's diff for review: baselined on the materialization commit
    /// (`base_sha`) when one was recorded — keeping the committed plan docs
    /// out of review noise while still surfacing any build-agent edits to
    /// them — falling back to the merge-base with the base branch for
    /// quick/adopted/migrated runs.
    pub fn run_diff(&self, active: &ActiveRun) -> Result<WorktreeDiff, OrchestratorError> {
        match &active.base_sha {
            Some(sha) => Ok(diff_against_base(&active.worktree.path, sha)?),
            None => Ok(diff_against_merge_base(
                &active.worktree.path,
                &active.worktree.base_branch,
            )?),
        }
    }

    // ---- PERIPHERY: fused-task spine (dies with task.rs) -------------------

    /// Dispatch a goal: create the worktree, scaffold `.build/`, transition out of
    /// `Created`, and spawn the first phase session with its prompt. A standard
    /// task starts planning; a quick task goes straight to building.
    pub fn dispatch(
        &self,
        id: TaskId,
        goal: impl Into<String>,
        kind: TaskKind,
        base_branch: &str,
        model_choice: ModelChoice,
    ) -> Result<ActiveTask, OrchestratorError> {
        let goal = goal.into();
        let slug = slugify(&goal);
        let worktree = self.worktrees.create(&slug, base_branch)?;
        self.scaffold_build_dir(&worktree, &id.0)?;

        let mut task = Task::new(id, goal.clone(), kind);
        task.apply(TaskEvent::Dispatch)?;

        let mut active = ActiveTask {
            task,
            worktree,
            plan_path: DEFAULT_PLAN_PATH.to_string(),
            last_summary: None,
            model_choice,
            last_error: None,
            stages: Vec::new(),
            current_stage_id: None,
            revising_stage_id: None,
            auto_advance: false,
            comments: Vec::new(),
            adopted: false,
            pending_continuation: false,
            session_generation: 0,
            session: None,
        };

        // Standard → Planning (plan prompt); Quick → Building (build prompt).
        let prompt = match active.task.state {
            TaskState::Planning => self.render(&self.templates.plan, &active, ""),
            TaskState::Building => self.render(&self.templates.build, &active, ""),
            ref other => unreachable!("dispatch left task in {other:?}"),
        };
        self.spawn_session(&mut active, &prompt)?;
        Ok(active)
    }

    /// Mint a Quick-kind task around an existing external worktree. No agent
    /// session is spawned — the task lands in `Review` (there is work to
    /// review). Order matters: checkpoint FIRST (pre-Build work stays its own
    /// legible commit), then scaffold `.build/mcp.json` (left uncommitted, as
    /// on the native path). Any error aborts with nothing persisted — the
    /// caller only persists on `Ok`.
    pub fn adopt(
        &self,
        id: TaskId,
        external: &ExternalWorktree,
        base_branch: &str,
        model_choice: ModelChoice,
    ) -> Result<ActiveTask, OrchestratorError> {
        let Some(branch) = external.branch.clone() else {
            return Err(OrchestratorError::Gate(
                "cannot adopt a detached-HEAD worktree — check out a branch first".to_string(),
            ));
        };
        if branch == base_branch {
            // Merging a branch into itself is meaningless, and the primary
            // checkout could never merge while its base is checked out elsewhere.
            return Err(OrchestratorError::Gate(format!(
                "cannot adopt a worktree with the base branch {base_branch:?} checked out"
            )));
        }
        // The branch name is an EXTERNAL, untrusted string (parsed verbatim from
        // `git worktree list --porcelain`), and it is later handed to `git merge`
        // and `git push` as a bare argv element. A name beginning with `-` would
        // be read by git as an option (`--exec=…` → arbitrary code execution), so
        // refuse to adopt it. Native branches are always `build/<slug>` and can
        // never trip this.
        if branch.starts_with('-') {
            return Err(OrchestratorError::Gate(format!(
                "cannot adopt a worktree whose branch name {branch:?} looks like a command-line \
                 option — rename the branch first"
            )));
        }

        self.commit_all_with_message(&external.path, "Checkpoint: adopted by Build")?;

        let worktree = Worktree {
            name: external.name.clone(),
            path: external.path.clone(),
            branch: branch.clone(),
            base_branch: base_branch.to_string(),
        };
        self.scaffold_build_dir(&worktree, &id.0)?;

        let goal = derive_adoption_goal(&branch, &external.head_subject);
        let mut task = Task::new(id, goal, TaskKind::Quick);
        task.apply(TaskEvent::Dispatch)?;
        task.apply(TaskEvent::BuildReady)?;

        Ok(ActiveTask {
            task,
            worktree,
            plan_path: DEFAULT_PLAN_PATH.to_string(),
            last_summary: None,
            model_choice,
            last_error: None,
            stages: Vec::new(),
            current_stage_id: None,
            revising_stage_id: None,
            auto_advance: false,
            comments: Vec::new(),
            adopted: true,
            pending_continuation: true,
            session_generation: 0,
            session: None,
        })
    }

    /// Consume an agent's `done` report (the MCP server forwards these), mapping
    /// it to the matching lifecycle event — and, on the multi-stage path, to the
    /// matching stage sub-state transition (spec §6.2).
    pub fn on_done(
        &self,
        active: &mut ActiveTask,
        report: DoneReport,
    ) -> Result<(), OrchestratorError> {
        match (report.phase, report.status) {
            // A blocked/failed report from any session — stage build, fix,
            // validation, or legacy — parks the task and disarms run-all (§0.6).
            (_, DoneStatus::Blocked) => {
                active.task.apply(TaskEvent::Blocked)?;
                active.auto_advance = false;
            }
            (_, DoneStatus::Failed) => {
                active.task.apply(TaskEvent::Failed)?;
                active.auto_advance = false;
            }
            (DonePhase::Plan, DoneStatus::Completed) => {
                // Legality FIRST: a stray plan report (any in-flight session may
                // misuse phase=plan) must be rejected with zero mutation — the
                // caller persists the task even when this returns Err, so a
                // manifest merged before the check would smuggle agent-chosen
                // paths/titles into already-reviewed stages.
                crate::task::transition(
                    &active.task.state,
                    active.task.kind,
                    TaskEvent::PlanReady,
                )?;
                if let Some(entries) = &report.outputs.stages {
                    if !entries.is_empty() {
                        merge_stage_manifest(&mut active.stages, entries.clone());
                    }
                }
                active.task.apply(TaskEvent::PlanReady)?;
            }
            // A per-stage plan-revision session (the task is Planning — or was
            // demoted to IdleUnreported(Plan) by quiescence, which never decides
            // anything, so its late report is still honored).
            (DonePhase::Revise, DoneStatus::Completed)
                if active.is_multi_stage()
                    && matches!(
                        active.task.state,
                        TaskState::Planning | TaskState::IdleUnreported(Phase::Plan)
                    ) =>
            {
                self.consume_stage_revision(active, &report)?;
            }
            (DonePhase::Build | DonePhase::Revise, DoneStatus::Completed)
                if active.is_multi_stage() =>
            {
                self.on_stage_session_done(active)?;
            }
            (DonePhase::Validate, DoneStatus::Completed) => {
                self.on_validation_done(active, &report)?;
            }
            // Legacy single-plan / Quick path: a completed build opens review.
            (DonePhase::Build | DonePhase::Revise, DoneStatus::Completed) => {
                active.task.apply(TaskEvent::BuildReady)?;
            }
        }
        // Only a consumed report leaves a trace: the surfaced summary, the
        // reported plan path, and the clearing of any stale crash/merge error
        // all land strictly after the transition above succeeded.
        if let Some(path) = &report.outputs.plan_path {
            active.plan_path = path.clone();
        }
        active.last_summary = Some(report.summary.clone());
        active.last_error = None;
        Ok(())
    }

    /// A stage build/fix session reported done(completed): commit the stage's
    /// work and hand it to a fresh validation session. No task-level event — the
    /// task stays `Building` until validation's verdict moves it.
    fn on_stage_session_done(&self, active: &mut ActiveTask) -> Result<(), OrchestratorError> {
        let Some(stage_id) = active.current_stage_id.clone() else {
            eprintln!(
                "on_done {}: build report for a multi-stage task with no current stage; ignoring",
                active.task.id.0
            );
            return Ok(());
        };
        let index = active
            .stage_index(&stage_id)
            .map_err(OrchestratorError::Gate)?;
        match active.stages[index].state {
            StageState::Building => {
                // Coarse-state legality before ANY mutation: accepting a stage
                // build completion has the same legality as legacy BuildReady
                // (Building / IdleUnreported(Build)). A report landing while the
                // task is Blocked/Failed must be rejected atomically — the stage
                // advance, commit, and session swap below would otherwise leave
                // the task and stage machines incoherent (the caller persists
                // the task even on Err).
                crate::task::transition(
                    &active.task.state,
                    active.task.kind,
                    TaskEvent::BuildReady,
                )?;
            }
            // A build report while the validation agent runs would skip the gate;
            // only a `validate` report may move a Validating stage.
            StageState::Validating | StageState::Built => {
                eprintln!(
                    "on_done {}: stage {stage_id} is awaiting validation; ignoring a non-validate report",
                    active.task.id.0
                );
                return Ok(());
            }
            // Post-review change requests (`request_changes`) run while the
            // current stage is already validated; their `done` closes the loop
            // exactly as on the legacy path.
            _ => {
                active.task.apply(TaskEvent::BuildReady)?;
                return Ok(());
            }
        }
        apply_stage_event(&mut active.stages[index], StageEvent::BuildDone)?;
        // The agent authors the stage's atomic, self-messaged commits (see the
        // build/fix_stage templates). This is only a safety net: a no-op on a
        // clean tree, so a fully-committing agent produces ZERO Build commits;
        // otherwise it sweeps whatever the agent left (and the first stage's
        // plan docs) with an honest message. It stays load-bearing regardless —
        // it GUARANTEES a committed boundary before the validation gate's
        // `git diff {stage_start_sha}` and before the next stage captures HEAD.
        self.commit_all_with_message(
            &active.worktree.path,
            &format!("Build: stage {stage_id} — checkpoint (swept by Build)"),
        )?;
        apply_stage_event(&mut active.stages[index], StageEvent::StartValidation)?;
        self.end_session(active);
        let prompt = self.render_stage(&self.templates.validate, active, index, "");
        self.spawn_session(active, &prompt)?;
        Ok(())
    }

    /// The validation agent's verdict. Pass: the stage is done — the final stage
    /// opens merge review, an inner stage returns to the stage board (and, with
    /// run-all armed, auto-dispatches the next approved stage). Fail: back to the
    /// stage board with run-all disarmed; the stored report drives `fix_stage`.
    fn on_validation_done(
        &self,
        active: &mut ActiveTask,
        report: &DoneReport,
    ) -> Result<(), OrchestratorError> {
        if !active.is_multi_stage() {
            eprintln!(
                "on_done {}: validate report for a task without stages; ignoring",
                active.task.id.0
            );
            return Ok(());
        }
        let Some(stage_id) = active.current_stage_id.clone() else {
            eprintln!(
                "on_done {}: validate report with no current stage; ignoring",
                active.task.id.0
            );
            return Ok(());
        };
        let index = active
            .stage_index(&stage_id)
            .map_err(OrchestratorError::Gate)?;
        if active.stages[index].state != StageState::Validating {
            eprintln!(
                "on_done {}: stage {stage_id} is not validating; ignoring a validate report",
                active.task.id.0
            );
            return Ok(());
        }
        let validation = report
            .outputs
            .validation
            .clone()
            .expect("mcp validated: outputs.validation present on phase=validate/completed");
        let passed = validation.passed;
        let last_stage = index + 1 == active.stages.len();
        let verdict = if passed {
            TaskEvent::ValidationPassed { last_stage }
        } else {
            TaskEvent::ValidationFailed
        };
        // Coarse-state legality before ANY mutation: a verdict landing while the
        // task is Blocked/Failed (e.g. the validation agent blocked, then was
        // nudged over the raw PTY) must be rejected atomically — advancing the
        // stage to its terminal Validated and killing the session here would
        // strand the task (the caller persists it even on Err).
        crate::task::transition(&active.task.state, active.task.kind, verdict)?;
        apply_stage_event(
            &mut active.stages[index],
            StageEvent::ValidationDone { passed },
        )?;
        active.stages[index].validation = Some(validation);
        self.end_session(active);
        active.task.apply(verdict)?;

        if passed {
            if !last_stage && active.auto_advance {
                let next = &active.stages[index + 1];
                if next.state == StageState::Approved {
                    let next_id = next.id.clone();
                    self.dispatch_stage(active, &next_id, None)?;
                }
            }
        } else {
            active.auto_advance = false;
        }
        Ok(())
    }

    /// A per-stage plan-revision session completed: the doc changed (any prior
    /// approval is stale), and the agent's per-comment resolutions land on the
    /// stored comments.
    fn consume_stage_revision(
        &self,
        active: &mut ActiveTask,
        report: &DoneReport,
    ) -> Result<(), OrchestratorError> {
        // Resolve the stage and probe both transitions before committing either,
        // so a rejected report leaves zero mutation behind.
        let stage_id = active.revising_stage_id.clone().ok_or_else(|| {
            OrchestratorError::Gate(
                "revise report for a multi-stage task with no stage revision in flight".to_string(),
            )
        })?;
        let index = active
            .stage_index(&stage_id)
            .map_err(OrchestratorError::Gate)?;
        stage_transition(&active.stages[index].state, StageEvent::Revised)?;
        active.task.apply(TaskEvent::PlanReady)?;
        apply_stage_event(&mut active.stages[index], StageEvent::Revised)?;
        if let Some(resolutions) = &report.outputs.comment_resolutions {
            for resolution in resolutions {
                let matching = active.comments.iter_mut().find(|c| {
                    c.id == resolution.comment_id
                        && c.stage_id == stage_id
                        && c.state == CommentState::Open
                });
                match matching {
                    Some(comment) => {
                        comment.state = CommentState::Addressed;
                        comment.agent_reply = Some(resolution.response.clone());
                    }
                    None => eprintln!(
                        "stage revision for {stage_id}: unknown or non-open comment {:?}; skipping",
                        resolution.comment_id
                    ),
                }
            }
        }
        active.revising_stage_id = None;
        Ok(())
    }

    /// The quiescence timer fired without a `done`: demote to `idle_unreported`.
    pub fn on_idle(&self, active: &mut ActiveTask) -> Result<(), OrchestratorError> {
        active.task.apply(TaskEvent::WentIdle)?;
        Ok(())
    }

    /// PERIPHERY: the fused approve-and-build (renamed from `approve_plan`,
    /// which now names the split's plan gate). On the split, approval rests
    /// the plan and the build starts at `dispatch_run`.
    ///
    /// Approve the plan and start the build in a **fresh** session — if a cold
    /// agent can't execute the plan, the plan wasn't done.
    pub fn approve_task_plan(
        &self,
        active: &mut ActiveTask,
        model_override: Option<ModelChoice>,
    ) -> Result<(), OrchestratorError> {
        active.task.apply(TaskEvent::ApprovePlan)?;
        active.last_error = None;
        if let Some(choice) = model_override {
            active.model_choice = choice;
        }
        self.end_session(active);
        let prompt = self.render(&self.templates.build, active, "");
        self.spawn_session(active, &prompt)?;
        Ok(())
    }

    /// Approve one stage's doc: Planned → Approved. Pure bookkeeping, no session.
    /// Legal from any non-terminal task state — approving future stages while an
    /// earlier one builds is how "run all" gets armed.
    pub fn approve_stage(
        &self,
        active: &mut ActiveTask,
        stage_id: &str,
    ) -> Result<(), OrchestratorError> {
        if active.task.state.is_terminal() {
            return Err(OrchestratorError::Gate(format!(
                "cannot approve a stage on a terminal task (state {:?})",
                active.task.state
            )));
        }
        let stage = active
            .stage_mut(stage_id)
            .map_err(OrchestratorError::Gate)?;
        apply_stage_event(stage, StageEvent::Approve)?;
        Ok(())
    }

    /// Dispatch one stage's build in a **fresh** cold session — the multi-stage
    /// analogue of `approve_plan`. The sequential gate lives here: a stage runs
    /// only from the stage board (`PlanReview`), only once approved, and only
    /// after every earlier stage passed validation.
    pub fn dispatch_stage(
        &self,
        active: &mut ActiveTask,
        stage_id: &str,
        model_override: Option<ModelChoice>,
    ) -> Result<(), OrchestratorError> {
        let index = active
            .stage_index(stage_id)
            .map_err(OrchestratorError::Gate)?;
        crate::task::transition(&active.task.state, active.task.kind, TaskEvent::ApprovePlan)
            .map_err(|e| OrchestratorError::Gate(format!("cannot dispatch a stage: {e}")))?;
        if active.stages[index].state != StageState::Approved {
            return Err(OrchestratorError::Gate(format!(
                "stage {stage_id} is not approved (state {:?})",
                active.stages[index].state
            )));
        }
        if let Some(unvalidated) = active.stages[..index]
            .iter()
            .find(|s| s.state != (StageState::Validated { passed: true }))
        {
            return Err(OrchestratorError::Gate(format!(
                "stage {} has not passed validation yet",
                unvalidated.id
            )));
        }

        active.task.apply(TaskEvent::ApprovePlan)?;
        apply_stage_event(&mut active.stages[index], StageEvent::Dispatch)?;
        active.current_stage_id = Some(stage_id.to_string());
        if active.stages[index].start_sha.is_none() {
            let sha = self
                .git(&active.worktree.path, &["rev-parse", "HEAD"])?
                .trim()
                .to_string();
            active.stages[index].start_sha = Some(sha);
        }
        if let Some(choice) = model_override {
            active.model_choice = choice;
        }
        active.last_error = None;
        self.end_session(active);
        let prompt = self.render_stage(&self.templates.build_stage, active, index, "");
        self.spawn_session(active, &prompt)?;
        Ok(())
    }

    /// Submit a batch of plan notes: re-plan against them in a **fresh** session.
    /// The existing plan file is the agent's starting point (the revise template
    /// points at it), so a cold agent can revise it — same discipline as
    /// `approve_plan` starting the build cold.
    pub fn send_notes(
        &self,
        active: &mut ActiveTask,
        notes: &str,
    ) -> Result<(), OrchestratorError> {
        active.task.apply(TaskEvent::SendNotes)?;
        active.last_error = None;
        self.end_session(active);
        let prompt = self.render(&self.templates.revise, active, notes);
        self.spawn_session(active, &prompt)?;
        Ok(())
    }

    /// Send a stage's open comments to a fresh plan-revision session (the
    /// multi-stage analogue of `send_notes`): the persisted open comments ARE
    /// the payload, rendered server-side per the §4.4 format.
    pub fn send_stage_notes(
        &self,
        active: &mut ActiveTask,
        stage_id: &str,
    ) -> Result<(), OrchestratorError> {
        let index = active
            .stage_index(stage_id)
            .map_err(OrchestratorError::Gate)?;
        crate::task::transition(&active.task.state, active.task.kind, TaskEvent::SendNotes)
            .map_err(|e| OrchestratorError::Gate(format!("cannot send stage notes: {e}")))?;
        if !matches!(
            active.stages[index].state,
            StageState::Planned | StageState::Approved
        ) {
            return Err(OrchestratorError::Gate(format!(
                "stage {stage_id} is not in plan review (state {:?})",
                active.stages[index].state
            )));
        }
        let open: Vec<StageComment> = active
            .open_comments_for(stage_id)
            .into_iter()
            .cloned()
            .collect();
        if open.is_empty() {
            return Err(OrchestratorError::Gate(format!(
                "no open comments on stage {stage_id}"
            )));
        }

        active.task.apply(TaskEvent::SendNotes)?;
        active.revising_stage_id = Some(stage_id.to_string());
        active.last_error = None;
        self.end_session(active);
        let comments = templates::assemble_stage_comments(&open);
        let prompt = self.render_stage(&self.templates.revise_stage, active, index, &comments);
        self.spawn_session(active, &prompt)?;
        Ok(())
    }

    /// Send a validation-failed stage back to a fresh fix session. The stored
    /// validation findings drive the prompt; `note` is the reviewer's optional
    /// steer. The start sha is kept so the stage diff covers all of its work.
    pub fn fix_stage(
        &self,
        active: &mut ActiveTask,
        stage_id: &str,
        note: &str,
    ) -> Result<(), OrchestratorError> {
        let index = active
            .stage_index(stage_id)
            .map_err(OrchestratorError::Gate)?;
        crate::task::transition(&active.task.state, active.task.kind, TaskEvent::ApprovePlan)
            .map_err(|e| OrchestratorError::Gate(format!("cannot fix a stage: {e}")))?;
        if active.stages[index].state != (StageState::Validated { passed: false }) {
            return Err(OrchestratorError::Gate(format!(
                "stage {stage_id} has no failed validation to fix (state {:?})",
                active.stages[index].state
            )));
        }

        active.task.apply(TaskEvent::ApprovePlan)?;
        apply_stage_event(&mut active.stages[index], StageEvent::Dispatch)?;
        active.current_stage_id = Some(stage_id.to_string());
        active.last_error = None;
        self.end_session(active);
        let prompt = self.render_stage(&self.templates.fix_stage, active, index, note);
        self.spawn_session(active, &prompt)?;
        Ok(())
    }

    /// Submit a batch of diff comments: address them in a **fresh** build session.
    /// Valid both from `review` (agent done) and `building` (agent still running) —
    /// any running session is ended first, so a change request redirects the build
    /// at any time. The agent's in-progress work stays in the worktree as the
    /// starting point.
    pub fn request_changes(
        &self,
        active: &mut ActiveTask,
        comments: &str,
    ) -> Result<(), OrchestratorError> {
        // On a multi-stage task the "building" window includes the stage's
        // validation pass. Ending THAT session here would replace it with a
        // review_changes session whose completed report the stage machine
        // ignores (only a `validate` report may move a Validating stage) —
        // hanging the task and discarding the user's comments. Hold the
        // change request until the verdict lands.
        if active.is_multi_stage() {
            if let Some(stage_id) = active.current_stage_id.clone() {
                let stage = active.stage(&stage_id).map_err(OrchestratorError::Gate)?;
                if matches!(stage.state, StageState::Built | StageState::Validating) {
                    return Err(OrchestratorError::Gate(format!(
                        "stage {stage_id} is awaiting validation; wait for the verdict \
                         before requesting changes"
                    )));
                }
            }
        }
        active.task.apply(TaskEvent::RequestChanges)?;
        active.last_error = None;
        self.end_session(active);
        let prompt = self.render(&self.templates.review_changes, active, comments);
        self.spawn_session(active, &prompt)?;
        Ok(())
    }

    /// A freeform human message to the task's agent. Live sessions (planning /
    /// building) are redirected: the session ends and a fresh one continues the
    /// harness's own conversation (`--continue`) with the message as its next
    /// turn — or, with no transcript to continue, a message-template session
    /// with full task context. Parked states (blocked / failed / idle /
    /// interrupted) resume their phase the same way. Review gates are refused:
    /// they have structured verbs (send notes / request changes), and a side
    /// channel there would bypass the batched-review contract.
    pub fn message(&self, active: &mut ActiveTask, message: &str) -> Result<(), OrchestratorError> {
        if message.trim().is_empty() {
            return Err(OrchestratorError::Gate("message must not be empty".into()));
        }
        // A stage awaiting validation cannot be redirected (the same invariant
        // as request_changes): only a validate report may move it.
        if active.is_multi_stage() {
            if let Some(stage_id) = active.current_stage_id.clone() {
                let stage = active.stage(&stage_id).map_err(OrchestratorError::Gate)?;
                if matches!(stage.state, StageState::Built | StageState::Validating) {
                    return Err(OrchestratorError::Gate(format!(
                        "stage {stage_id} is awaiting validation; wait for the \
                         verdict before messaging the agent"
                    )));
                }
            }
        }
        use crate::task::TaskState as S;
        let event = match active.task.state {
            S::Planning | S::Building => None,
            S::Blocked(_) | S::Failed(_) | S::IdleUnreported(_) | S::Interrupted(_) => {
                Some(TaskEvent::Reply)
            }
            S::PlanReview | S::Review => {
                return Err(OrchestratorError::Gate(
                    "the task is at a review gate — use send notes / request changes there".into(),
                ))
            }
            S::Created | S::Merged | S::Abandoned | S::Archived => {
                return Err(OrchestratorError::Gate(
                    "no agent session to message".into(),
                ))
            }
        };
        if let Some(event) = event {
            // Pure legality first — the caller persists the task even on Err.
            crate::task::transition(&active.task.state, active.task.kind, event)?;
        }
        // With a transcript the message IS the next conversation turn; without
        // one, a fresh session gets it wrapped in full task context.
        let prompt = if (self.transcript_probe)(&active.worktree.path) {
            message.to_string()
        } else {
            self.render(&self.templates.message, active, message)
        };
        if let Some(event) = event {
            active.task.apply(event)?;
        }
        active.last_error = None;
        self.end_session(active);
        active.pending_continuation = true;
        self.spawn_session(active, &prompt)?;
        Ok(())
    }

    /// Re-dispatch an interrupted phase in a **fresh** session. The daemon that
    /// spawned the original session died; the worktree (the agent's real state)
    /// is the starting point, exactly like `approve_plan` starting a cold build.
    /// On the multi-stage path the persisted stage sub-state routes which
    /// session to respawn (spec §1.3).
    pub fn resume(&self, active: &mut ActiveTask) -> Result<(), OrchestratorError> {
        // Route the prompt BEFORE committing the Reply transition: the caller
        // persists the task even when this returns Err, so a routing failure
        // (e.g. a stage sub-state with no resume path) after a committed Reply
        // would strand the task in Building with no session — un-resumable,
        // un-demotable, and outside every attention bucket.
        let resumed_state =
            crate::task::transition(&active.task.state, active.task.kind, TaskEvent::Reply)?;
        let prompt = match resumed_state {
            TaskState::Planning if active.is_multi_stage() => {
                self.resume_multi_stage_plan_prompt(active)?
            }
            TaskState::Planning => self.render(&self.templates.plan, active, ""),
            TaskState::Building if active.is_multi_stage() => {
                self.resume_multi_stage_build_prompt(active)?
            }
            TaskState::Building => self.render(&self.templates.build, active, ""),
            ref other => unreachable!("reply would leave task in {other:?}"),
        };
        active.task.apply(TaskEvent::Reply)?;
        active.last_error = None;
        self.end_session(active);
        self.spawn_session(active, &prompt)?;
        Ok(())
    }

    /// An interrupted multi-stage plan phase: a per-stage revision (there is a
    /// revising stage) respawns `revise_stage` with the stage's open comments;
    /// a full (re-)plan respawns the initial plan template.
    fn resume_multi_stage_plan_prompt(
        &self,
        active: &ActiveTask,
    ) -> Result<String, OrchestratorError> {
        let Some(stage_id) = active.revising_stage_id.clone() else {
            return Ok(self.render(&self.templates.plan, active, ""));
        };
        let index = active
            .stage_index(&stage_id)
            .map_err(OrchestratorError::Gate)?;
        let open: Vec<StageComment> = active
            .open_comments_for(&stage_id)
            .into_iter()
            .cloned()
            .collect();
        let comments = templates::assemble_stage_comments(&open);
        Ok(self.render_stage(&self.templates.revise_stage, active, index, &comments))
    }

    /// An interrupted multi-stage build phase, routed by the current stage's
    /// persisted sub-state: `Building` respawns the build session (or the fix
    /// session, when a failed validation report shows that is what died);
    /// `Built`/`Validating` respawn the validation pass (a `Built` stage is
    /// forced to `Validating` first).
    fn resume_multi_stage_build_prompt(
        &self,
        active: &mut ActiveTask,
    ) -> Result<String, OrchestratorError> {
        let stage_id = active.current_stage_id.clone().ok_or_else(|| {
            OrchestratorError::Gate(
                "multi-stage task is building but has no current stage".to_string(),
            )
        })?;
        let index = active
            .stage_index(&stage_id)
            .map_err(OrchestratorError::Gate)?;
        match active.stages[index].state {
            StageState::Building => {
                let died_in_fix_session = active.stages[index]
                    .validation
                    .as_ref()
                    .is_some_and(|report| !report.passed);
                let template = if died_in_fix_session {
                    &self.templates.fix_stage
                } else {
                    &self.templates.build_stage
                };
                Ok(self.render_stage(template, active, index, ""))
            }
            StageState::Built => {
                apply_stage_event(&mut active.stages[index], StageEvent::StartValidation)?;
                Ok(self.render_stage(&self.templates.validate, active, index, ""))
            }
            StageState::Validating => {
                Ok(self.render_stage(&self.templates.validate, active, index, ""))
            }
            // The stage machinery is done; the interrupted session was a
            // post-review change request, whose comments were not persisted.
            StageState::Validated { passed: true } => Err(OrchestratorError::Gate(format!(
                "stage {stage_id} already passed validation — the interrupted session was a \
                 post-review change request; re-send the diff comments with Request Changes, \
                 or approve the merge"
            ))),
            ref other => Err(OrchestratorError::Gate(format!(
                "stage {stage_id} cannot resume from state {other:?}"
            ))),
        }
    }

    /// The worktree's current diff against base — progress fact while building, the
    /// full review surface at the gate.
    pub fn diff(&self, active: &ActiveTask) -> Result<WorktreeDiff, OrchestratorError> {
        Ok(diff_against_base(
            &active.worktree.path,
            &active.worktree.base_branch,
        )?)
    }

    /// Approve the diff and merge. Merge honesty (contract): the git work runs
    /// **first** — only if commit + merge succeed does the task become `Merged`. A
    /// git failure (conflict, wrong base checkout) leaves the task in `review` and
    /// returns a `merge_failed:` error, so the board never shows a merged task whose
    /// branch was in fact never merged.
    ///
    /// Worktree/branch cleanup is deliberately **not** done here: the caller
    /// persists the `Merged` verdict first and only then prunes (via
    /// [`discard_worktree`](Self::discard_worktree)). Ordering matters (contract #3)
    /// — if cleanup ran before the persist and the daemon died in between, the
    /// stored record would still say `review` with its worktree/branch gone, and
    /// boot recovery would mislabel genuinely-merged work as `Abandoned`. Deferring
    /// cleanup to after the persist collapses that window to a self-healing one (a
    /// surviving worktree just re-merges as a no-op on re-approve).
    pub fn approve_merge(&self, active: &mut ActiveTask) -> Result<(), OrchestratorError> {
        // Reject the approval up front if the task isn't at a review gate, without
        // touching git or the lifecycle (a pure legality check).
        crate::task::transition(
            &active.task.state,
            active.task.kind,
            TaskEvent::ApproveMerge,
        )?;

        // Run (and only then commit to) the merge. Any git failure keeps the task in
        // its review state with a contract-shaped `merge_failed:` reason.
        self.commit_all(&active.worktree.path, &active.task.goal)
            .map_err(as_merge_failure)?;
        self.merge_into_base(&active.worktree.branch, &active.worktree.base_branch)
            .map_err(as_merge_failure)?;

        // The merge landed: advance the lifecycle and end the session. The worktree
        // stays on disk until the caller has persisted this verdict.
        active.task.apply(TaskEvent::ApproveMerge)?;
        active.last_error = None;
        self.end_session(active);
        Ok(())
    }

    /// Commit any outstanding work on the task branch (the implicit commit step
    /// every finish action shares). Keeps the worktree; no lifecycle change.
    pub fn commit(&self, active: &ActiveTask) -> Result<(), OrchestratorError> {
        self.commit_all(&active.worktree.path, &active.task.goal)
    }

    /// Commit, then push the task branch to its `origin`. Keeps the worktree, so
    /// the agent can keep working / the user can open a PR. Errors if there is no
    /// push destination configured.
    pub fn push(&self, active: &ActiveTask) -> Result<(), OrchestratorError> {
        self.commit_all(&active.worktree.path, &active.task.goal)?;
        self.git(
            &active.worktree.path,
            // `--` stops option parsing so an option-shaped branch name can never
            // be read by git as a flag (defense in depth alongside the adopt-time
            // guard in `adopt`).
            &["push", "-u", "origin", "--", &active.worktree.branch],
        )?;
        Ok(())
    }

    /// Approve & merge (as [`approve_merge`](Self::approve_merge)) and then push
    /// the updated base branch to `origin`.
    pub fn merge_and_push(&self, active: &mut ActiveTask) -> Result<(), OrchestratorError> {
        let base = active.worktree.base_branch.clone();
        self.approve_merge(active)?;
        self.git(&self.repo_path, &["push", "origin", &base])?;
        Ok(())
    }

    /// Abandon from any non-terminal state: kill the harness, mark the task
    /// `Abandoned`, and best-effort prune its worktree + branch. Cleanup is
    /// best-effort by contract — a leftover worktree/branch is logged, never a
    /// reason to fail the abandon (the lifecycle verdict is what matters and must
    /// persist).
    pub fn abandon(&self, active: &mut ActiveTask) -> Result<(), OrchestratorError> {
        active.task.apply(TaskEvent::Abandon)?;
        self.end_session(active);
        if let Err(cleanup) = self
            .worktrees
            .remove(&active.worktree, /* keep_branch */ false)
        {
            eprintln!(
                "abandon {}: task abandoned but worktree/branch cleanup failed: {cleanup}",
                active.worktree.name
            );
        }
        Ok(())
    }

    /// Best-effort teardown of a leftover worktree + branch for a task being
    /// deleted from the board. A failed cleanup is logged, never fatal — deleting
    /// the task record is what removes it, and a stray worktree is only clutter.
    pub fn discard_worktree(&self, worktree: &Worktree) {
        if let Err(e) = self.worktrees.remove(worktree, /* keep_branch */ false) {
            eprintln!("discard_worktree {}: {e}", worktree.name);
        }
    }

    // --- internals -------------------------------------------------------------

    fn render_plan(&self, template: &str, active: &ActivePlan, comments: &str) -> String {
        templates::render(
            template,
            &Vars {
                goal: &active.plan.goal,
                plan_path: &active.plan_path,
                comments,
                base_branch: &active.base_branch,
                ..Vars::default()
            },
        )
    }

    fn render_run(&self, template: &str, active: &ActiveRun, comments: &str) -> String {
        templates::render(
            template,
            &Vars {
                goal: &active.run.goal,
                plan_path: &active.plan_path,
                comments,
                base_branch: &active.worktree.base_branch,
                ..Vars::default()
            },
        )
    }

    /// Render a stage-scoped template for a run with the full stage variable
    /// set: the stage doc's own fields (from the plan's manifest), the next
    /// stage's doc path (empty on the final stage), the previous stage's
    /// validation notes, and this stage's own findings — the split twin of
    /// [`render_stage`](Self::render_stage), joining plan docs to run progress
    /// by stage id.
    fn render_run_stage(
        &self,
        template: &str,
        active: &ActiveRun,
        plan_stage_docs: &[StageDoc],
        doc_index: usize,
        comments: &str,
    ) -> String {
        let doc = &plan_stage_docs[doc_index];
        let next_stage_path = plan_stage_docs
            .get(doc_index + 1)
            .map(|next| next.path.as_str())
            .unwrap_or("");
        let prior_notes = doc_index
            .checked_sub(1)
            .and_then(|previous| active.stage_progress(&plan_stage_docs[previous].id))
            .and_then(|progress| progress.validation.as_ref())
            .map(|v| v.notes_for_next_stage.as_str())
            .unwrap_or("");
        let progress = active.stage_progress(&doc.id);
        let stage_start_sha = progress.and_then(|p| p.start_sha.as_deref()).unwrap_or("");
        let findings = progress
            .and_then(|p| p.validation.as_ref())
            .map(|v| v.findings.as_str())
            .unwrap_or("");
        templates::render(
            template,
            &Vars {
                goal: &active.run.goal,
                plan_path: &active.plan_path,
                comments,
                base_branch: &active.worktree.base_branch,
                stage_id: &doc.id,
                stage_title: &doc.title,
                stage_path: &doc.path,
                stage_summary: &doc.summary,
                next_stage_path,
                stage_start_sha,
                findings,
                prior_notes,
            },
        )
    }

    /// Spawn the plan's next session in its disposable worktree. Plans never
    /// carry a continuation flag — every planning session starts cold.
    fn spawn_plan_session(
        &self,
        active: &mut ActivePlan,
        prompt: &str,
    ) -> Result<(), OrchestratorError> {
        let cwd = active
            .worktree
            .as_ref()
            .ok_or_else(|| {
                OrchestratorError::Gate(
                    "the plan has no planning worktree to run a session in".to_string(),
                )
            })?
            .path
            .clone();
        let model_choice = active.model_choice.clone();
        self.spawn_into_slot(&mut active.session, &cwd, &model_choice, false, prompt)
    }

    /// Spawn the run's next session, consuming the one-shot continuation flag:
    /// the first spawn after adoption probes for an existing harness
    /// transcript in the worktree and asks the one-shot builder to continue it.
    fn spawn_run_session(
        &self,
        active: &mut ActiveRun,
        prompt: &str,
    ) -> Result<(), OrchestratorError> {
        let continue_session =
            active.pending_continuation && (self.transcript_probe)(&active.worktree.path);
        active.pending_continuation = false;
        let cwd = active.worktree.path.clone();
        let model_choice = active.model_choice.clone();
        self.spawn_into_slot(
            &mut active.session,
            &cwd,
            &model_choice,
            continue_session,
            prompt,
        )
    }

    /// The shared spawn tail for both split entities: build the harness
    /// command, spawn it in `cwd`, and install it in the slot (bumping the
    /// generation). Mirrors [`spawn_session`](Self::spawn_session), which the
    /// periphery retires with the fused path.
    fn spawn_into_slot(
        &self,
        slot: &mut SessionSlot,
        cwd: &Path,
        model_choice: &ModelChoice,
        continue_session: bool,
        prompt: &str,
    ) -> Result<(), OrchestratorError> {
        let options = SpawnOptions { continue_session };
        let session = match &self.agent {
            Agent::Warm(spec) => {
                // Warm harnesses take the prompt over the PTY and never see
                // SpawnOptions: continuation is one-shot-specific.
                let s = PtySession::spawn(spec, Some(cwd.to_path_buf()), self.pty_size)?;
                s.write_prompt(prompt)?;
                s
            }
            Agent::OneShot(build) => {
                // The prompt is baked into the command (e.g. `claude -p`);
                // nothing is written to stdin.
                let spec = build(prompt, model_choice, &options);
                PtySession::spawn(&spec, Some(cwd.to_path_buf()), self.pty_size)?
            }
        };
        slot.install(session);
        Ok(())
    }

    fn render(&self, template: &str, active: &ActiveTask, comments: &str) -> String {
        templates::render(
            template,
            &Vars {
                goal: &active.task.goal,
                plan_path: &active.plan_path,
                comments,
                base_branch: &active.worktree.base_branch,
                ..Vars::default()
            },
        )
    }

    /// Render a stage-scoped template with the full stage variable set: the
    /// stage's own fields, the next stage's doc path (empty on the final stage),
    /// the previous stage's validation notes, and this stage's own findings
    /// (which the `fix_stage` template consumes).
    fn render_stage(
        &self,
        template: &str,
        active: &ActiveTask,
        index: usize,
        comments: &str,
    ) -> String {
        let stage = &active.stages[index];
        let next_stage_path = active
            .stages
            .get(index + 1)
            .map(|s| s.path.as_str())
            .unwrap_or("");
        let prior_notes = index
            .checked_sub(1)
            .and_then(|previous| active.stages[previous].validation.as_ref())
            .map(|v| v.notes_for_next_stage.as_str())
            .unwrap_or("");
        let findings = stage
            .validation
            .as_ref()
            .map(|v| v.findings.as_str())
            .unwrap_or("");
        templates::render(
            template,
            &Vars {
                goal: &active.task.goal,
                plan_path: &active.plan_path,
                comments,
                base_branch: &active.worktree.base_branch,
                stage_id: &stage.id,
                stage_title: &stage.title,
                stage_path: &stage.path,
                stage_summary: &stage.summary,
                next_stage_path,
                stage_start_sha: stage.start_sha.as_deref().unwrap_or(""),
                findings,
                prior_notes,
            },
        )
    }

    /// Spawn the task's next session, consuming the one-shot continuation flag:
    /// the first spawn after adoption probes for an existing harness transcript
    /// in the worktree and asks the one-shot builder to continue it. Native
    /// tasks (flag false) are byte-identical to the pre-adoption behavior.
    fn spawn_session(
        &self,
        active: &mut ActiveTask,
        prompt: &str,
    ) -> Result<(), OrchestratorError> {
        let continue_session =
            active.pending_continuation && (self.transcript_probe)(&active.worktree.path);
        active.pending_continuation = false;
        let options = SpawnOptions { continue_session };
        let session = match &self.agent {
            Agent::Warm(spec) => {
                // Warm harnesses (the QA agent) take the prompt over the PTY and
                // never see SpawnOptions: continuation is one-shot-specific.
                let s = PtySession::spawn(spec, Some(active.worktree.path.clone()), self.pty_size)?;
                s.write_prompt(prompt)?;
                s
            }
            Agent::OneShot(build) => {
                // The prompt is baked into the command (e.g. `claude -p`); nothing
                // is written to stdin. The model choice becomes harness argv.
                let spec = build(prompt, &active.model_choice, &options);
                PtySession::spawn(&spec, Some(active.worktree.path.clone()), self.pty_size)?
            }
        };
        active.session_generation += 1;
        active.session = Some(session);
        Ok(())
    }

    fn end_session(&self, active: &mut ActiveTask) {
        // Kill AND reap: kill alone leaves a zombie per phase transition, which over
        // a long-lived daemon exhausts the process table.
        active.end_session();
    }

    /// Write the per-entity MCP config under `.build/` so it never trips
    /// plan-scope enforcement, pointing the harness at the owning entity's
    /// `done` server. `owner_id` is a plan, run, or (legacy) task id — the MCP
    /// CLI stays `mcp --task <id>` (opaque); the daemon routes each report by
    /// owner lookup.
    fn scaffold_build_dir(
        &self,
        worktree: &Worktree,
        owner_id: &str,
    ) -> Result<(), OrchestratorError> {
        let build_dir = worktree.path.join(".build");
        std::fs::create_dir_all(&build_dir)?;
        // A hard guard so the AGENT's own commits can never capture mcp.json: the
        // build templates now instruct the agent to commit its work, and a routine
        // `git add -A` would otherwise stage this machine-local config (absolute
        // exe path, per-task identity) into the branch — leaking the local path
        // into base history and add/add-conflicting against every other task's
        // copy. Build's own sweep already excludes it via pathspec; this ignore
        // closes the agent path too. `.build/plan/*` stays committable (the
        // gitignore itself rides Build's sweep, so the rule persists on-branch).
        std::fs::write(build_dir.join(".gitignore"), "mcp.json\n")?;
        // Absolute path to this binary so the harness can spawn it regardless of PATH.
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.to_str().map(String::from))
            .unwrap_or_else(|| "build-bridge".to_string());
        let mcp = serde_json::json!({
            "mcpServers": {
                "build": {
                    "command": exe,
                    "args": ["mcp", "--task", owner_id]
                }
            }
        });
        std::fs::write(
            build_dir.join("mcp.json"),
            serde_json::to_string_pretty(&mcp)?,
        )?;
        Ok(())
    }

    fn commit_all(&self, worktree_path: &Path, goal: &str) -> Result<(), OrchestratorError> {
        self.commit_all_with_message(worktree_path, &format!("Build: {goal}"))
    }

    /// Stage everything and commit with `message` verbatim; a clean tree is a
    /// no-op (nothing staged, nothing committed).
    fn commit_all_with_message(
        &self,
        worktree_path: &Path,
        message: &str,
    ) -> Result<(), OrchestratorError> {
        // The scaffolded MCP config is machine-local plumbing (absolute binary
        // path, per-task identity): committing it would merge it into the base
        // branch and add/add-conflict against every other branch's copy. It is
        // kept out of every commit by `.build/.gitignore` (written at scaffold
        // time), which `git add -A` honors silently — and which also guards the
        // agent's own commits. (A `:(exclude)` pathspec here would instead ERROR,
        // since it names an ignored path explicitly.)
        self.git(worktree_path, &["add", "-A", "--", "."])?;
        // Only commit if something is staged (the MCP config alone must not
        // produce a commit).
        let staged = self.git(worktree_path, &["diff", "--cached", "--name-only"])?;
        if !staged.trim().is_empty() {
            self.git(worktree_path, &["commit", "-m", message])?;
        }
        Ok(())
    }

    /// Merge the task branch into `base_branch` via the primary checkout. The
    /// primary repo is the user's live checkout, so first verify it actually has
    /// the base branch checked out — merging into whatever happens to be at HEAD
    /// would land the task on the wrong branch (and a later push of the base
    /// branch would silently publish nothing).
    fn merge_into_base(&self, branch: &str, base_branch: &str) -> Result<(), OrchestratorError> {
        let head = self
            .git(&self.repo_path, &["symbolic-ref", "--short", "HEAD"])?
            .trim()
            .to_string();
        if head != base_branch {
            return Err(OrchestratorError::Git(format!(
                "primary checkout is on {head:?}, not the base branch {base_branch:?} — \
                 check out {base_branch:?} (or commit/stash your work) and approve again"
            )));
        }
        // `--` stops option parsing so an option-shaped branch name can never be
        // read by git as a flag (defense in depth alongside the adopt-time guard).
        if let Err(merge_error) = self.git(&self.repo_path, &["merge", "--no-edit", "--", branch]) {
            // A conflict leaves the primary checkout wedged mid-merge; abort it so
            // the checkout returns to a clean base and later merges aren't poisoned.
            // Best-effort — the merge failure is the error we surface either way.
            if let Err(abort_error) = self.git(&self.repo_path, &["merge", "--abort"]) {
                eprintln!(
                    "merge_into_base {branch}: merge failed and abort also failed: {abort_error}"
                );
            }
            return Err(merge_error);
        }
        Ok(())
    }

    fn git(&self, dir: &Path, args: &[&str]) -> Result<String, OrchestratorError> {
        let out = Command::new("git").args(args).current_dir(dir).output()?;
        if !out.status.success() {
            // git splits its story across streams (a conflicting merge reports
            // "CONFLICT …" on stdout); surface both so the user sees why.
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stdout = String::from_utf8_lossy(&out.stdout);
            let detail: Vec<&str> = [stderr.trim(), stdout.trim()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect();
            return Err(OrchestratorError::Git(format!(
                "git {args:?}: {}",
                detail.join("\n")
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::DoneOutputs;

    fn init_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success());
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "t@build.ing"]);
        git(&["config", "user.name", "T"]);
        std::fs::write(repo.join("README.md"), "# project\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "initial"]);
        (dir, repo)
    }

    /// A warm "harness" that stays alive and drains stdin (it discards the
    /// prompt), like a real interactive CLI. Draining matters: a child that never
    /// reads lets the PTY's canonical-mode input queue fill, so writing a
    /// full-size rendered prompt would block and then fail with EIO. The test
    /// plays the agent: it writes files and forwards `done` reports.
    fn warm_harness() -> HarnessSpec {
        HarnessSpec::new("sh").arg("-c").arg("cat >/dev/null")
    }

    fn orchestrator(dir: &tempfile::TempDir, repo: &Path) -> Orchestrator {
        Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            Agent::Warm(warm_harness()),
            Templates::default(),
        )
    }

    /// A one-shot agent that records every (prompt, model choice) it is asked
    /// to spawn, standing in for the real `claude` command builder.
    fn recording_agent(log: std::sync::Arc<std::sync::Mutex<Vec<ModelChoice>>>) -> Agent {
        Agent::OneShot(std::sync::Arc::new(
            move |_prompt: &str, choice: &ModelChoice, _options: &SpawnOptions| {
                log.lock().unwrap().push(choice.clone());
                HarnessSpec::new("sh").arg("-c").arg("exit 0")
            },
        ))
    }

    #[tokio::test]
    async fn model_choice_reaches_every_spawn_and_approve_can_override_it() {
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            recording_agent(log.clone()),
            Templates::default(),
        );

        let dispatch_choice = ModelChoice {
            model: Some("claude-opus-4-8".into()),
            effort: Some("xhigh".into()),
        };
        let mut t = orch
            .dispatch(
                TaskId::new("m1"),
                "Add a greeting",
                TaskKind::Standard,
                "main",
                dispatch_choice.clone(),
            )
            .unwrap();
        assert_eq!(
            log.lock().unwrap().as_slice(),
            std::slice::from_ref(&dispatch_choice)
        );

        // Plan done → approve with a DIFFERENT model for the coding agent.
        t.task.apply(TaskEvent::PlanReady).unwrap();
        let build_choice = ModelChoice {
            model: Some("claude-sonnet-5".into()),
            effort: Some("high".into()),
        };
        orch.approve_task_plan(&mut t, Some(build_choice.clone()))
            .unwrap();
        assert_eq!(t.model_choice, build_choice);
        assert_eq!(
            log.lock().unwrap().as_slice(),
            &[dispatch_choice, build_choice.clone()]
        );

        // A change request re-spawns with the task's CURRENT choice, no new input.
        t.task.apply(TaskEvent::BuildReady).unwrap();
        orch.request_changes(&mut t, "tweak it").unwrap();
        assert_eq!(log.lock().unwrap().last().unwrap(), &build_choice);
    }

    /// A one-shot agent that records the [`SpawnOptions`] of every spawn.
    fn options_recording_agent(log: std::sync::Arc<std::sync::Mutex<Vec<SpawnOptions>>>) -> Agent {
        Agent::OneShot(std::sync::Arc::new(
            move |_prompt: &str, _choice: &ModelChoice, options: &SpawnOptions| {
                log.lock().unwrap().push(*options);
                HarnessSpec::new("sh").arg("-c").arg("exit 0")
            },
        ))
    }

    /// The native path is byte-identical to today: no spawn ever asks the
    /// harness to continue a prior conversation, even with a transcript present.
    #[tokio::test]
    async fn native_dispatch_never_continues() {
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            options_recording_agent(log.clone()),
            Templates::default(),
        )
        .with_transcript_probe(std::sync::Arc::new(|_| true));

        let mut t = orch
            .dispatch(
                TaskId::new("n1"),
                "do work",
                TaskKind::Quick,
                "main",
                Default::default(),
            )
            .unwrap();
        orch.request_changes(&mut t, "tweak it").unwrap();
        let recorded = log.lock().unwrap();
        assert_eq!(recorded.len(), 2);
        assert!(
            recorded.iter().all(|options| !options.continue_session),
            "native spawns never continue: {recorded:?}"
        );
    }

    fn done(phase: DonePhase, status: DoneStatus, plan_path: Option<&str>) -> DoneReport {
        DoneReport {
            phase,
            status,
            summary: "summary".into(),
            outputs: DoneOutputs {
                plan_path: plan_path.map(String::from),
                ..DoneOutputs::default()
            },
        }
    }

    #[tokio::test]
    async fn full_spine_plan_build_merge() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);

        // Dispatch → Planning, worktree + MCP config scaffolded.
        let mut t = orch
            .dispatch(
                TaskId::new("t1"),
                "Add a greeting",
                TaskKind::Standard,
                "main",
                Default::default(),
            )
            .unwrap();
        assert_eq!(t.task.state, TaskState::Planning);
        assert!(t.worktree.path.join(".build/mcp.json").exists());
        assert!(t.subscribe().is_some(), "a warm session streams output");

        // The agent writes the plan, then reports done → PlanReview.
        std::fs::write(
            t.worktree.path.join(".build/plan.md"),
            "# Plan\n1. add greeting.txt\n",
        )
        .unwrap();
        orch.on_done(
            &mut t,
            done(
                DonePhase::Plan,
                DoneStatus::Completed,
                Some(".build/plan.md"),
            ),
        )
        .unwrap();
        assert_eq!(t.task.state, TaskState::PlanReview);
        assert_eq!(t.plan_path, ".build/plan.md");

        // Plan-phase enforcement: only `.build/` touched so far.
        assert!(!orch.diff(&t).unwrap().touched_outside_plan_scope());

        // Approve → fresh build session.
        orch.approve_task_plan(&mut t, None).unwrap();
        assert_eq!(t.task.state, TaskState::Building);

        // The agent writes code, then reports done → Review.
        std::fs::write(t.worktree.path.join("greeting.txt"), "hello\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.task.state, TaskState::Review);

        // The full diff at the gate shows the code.
        let review_diff = orch.diff(&t).unwrap();
        assert!(review_diff.files().iter().any(|f| f.path == "greeting.txt"));

        // Approve & merge → Merged, base branch has the file. The merge itself does
        // NOT prune the worktree — that is the caller's step, after persisting the
        // verdict (contract #3), so the worktree survives the merge.
        orch.approve_merge(&mut t).unwrap();
        assert_eq!(t.task.state, TaskState::Merged);
        assert!(
            t.worktree.path.exists(),
            "the merge leaves cleanup to the caller"
        );
        assert!(
            repo.join("greeting.txt").exists(),
            "merged into base working tree"
        );
        // The caller prunes once the Merged verdict is durable.
        orch.discard_worktree(&t.worktree);
        assert!(!t.worktree.path.exists(), "worktree pruned after persist");
        // The plan is kept through merge (intent as infrastructure).
        assert!(
            repo.join(".build/plan.md").exists(),
            "plan kept through merge"
        );
    }

    /// Merge honesty (contract #2): a conflicting merge must fail cleanly. It
    /// reports `merge_failed:`, keeps the task in review — and, critically, must
    /// NOT leave the primary checkout wedged mid-merge, or every later merge is
    /// poisoned. Two quick tasks branch from the same base and touch the same
    /// file; the first lands, the second conflicts, and a third unrelated task
    /// must still merge afterwards.
    #[tokio::test]
    async fn conflicting_merge_aborts_and_leaves_the_checkout_mergeable() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);

        let to_review = |id: &str, file: &str, contents: &str| {
            let mut t = orch
                .dispatch(
                    TaskId::new(id),
                    "same file",
                    TaskKind::Quick,
                    "main",
                    Default::default(),
                )
                .unwrap();
            std::fs::write(t.worktree.path.join(file), contents).unwrap();
            orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
                .unwrap();
            assert_eq!(t.task.state, TaskState::Review);
            t
        };

        // Both cut from the same base tip and write the SAME file.
        let mut first = to_review("c1", "result.txt", "first\n");
        let mut second = to_review("c2", "result.txt", "second\n");

        orch.approve_merge(&mut first).unwrap();
        assert_eq!(first.task.state, TaskState::Merged);

        let conflict = orch
            .approve_merge(&mut second)
            .expect_err("the second write must conflict");
        assert!(
            conflict.to_string().starts_with("merge_failed:"),
            "{conflict:?}"
        );
        assert_eq!(second.task.state, TaskState::Review, "task stays in review");

        // The primary checkout must be clean, not mid-merge.
        assert!(
            !repo.join(".git/MERGE_HEAD").exists(),
            "a failed merge must be aborted, not left staged with conflicts"
        );

        // Proof the checkout recovered: an unrelated task still merges.
        let mut third = to_review("c3", "other.txt", "third\n");
        orch.approve_merge(&mut third)
            .expect("checkout still mergeable");
        assert_eq!(third.task.state, TaskState::Merged);
        assert!(repo.join("other.txt").exists());
    }

    #[tokio::test]
    async fn quick_task_skips_planning() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);

        let mut t = orch
            .dispatch(
                TaskId::new("q1"),
                "fix typo",
                TaskKind::Quick,
                "main",
                Default::default(),
            )
            .unwrap();
        assert_eq!(t.task.state, TaskState::Building, "no plan phase");

        std::fs::write(t.worktree.path.join("fix.txt"), "fixed\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.task.state, TaskState::Review);

        orch.approve_merge(&mut t).unwrap();
        assert_eq!(t.task.state, TaskState::Merged);
        assert!(repo.join("fix.txt").exists());
    }

    #[tokio::test]
    async fn blocked_then_reply_resumes_building() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = orch
            .dispatch(
                TaskId::new("b1"),
                "do work",
                TaskKind::Quick,
                "main",
                Default::default(),
            )
            .unwrap();

        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Blocked, None))
            .unwrap();
        assert!(matches!(t.task.state, TaskState::Blocked(_)));
        assert_eq!(t.last_summary.as_deref(), Some("summary"));

        orch.message(&mut t, "use the staging credentials").unwrap();
        assert_eq!(t.task.state, TaskState::Building);
        assert!(t.subscribe().is_some(), "a fresh session carries the reply");
    }

    /// A one-shot agent that records every spawn's prompt and continue flag.
    fn prompt_spy_agent(log: std::sync::Arc<std::sync::Mutex<Vec<(String, bool)>>>) -> Agent {
        Agent::OneShot(std::sync::Arc::new(
            move |prompt: &str, _choice: &ModelChoice, options: &SpawnOptions| {
                log.lock()
                    .unwrap()
                    .push((prompt.to_string(), options.continue_session));
                HarnessSpec::new("sh").arg("-c").arg("exit 0")
            },
        ))
    }

    #[tokio::test]
    async fn message_continues_the_agents_conversation_when_a_transcript_exists() {
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            prompt_spy_agent(log.clone()),
            Templates::default(),
        )
        .with_transcript_probe(std::sync::Arc::new(|_| true));

        let mut t = orch
            .dispatch(
                TaskId::new("m1"),
                "do work",
                TaskKind::Quick,
                "main",
                Default::default(),
            )
            .unwrap();
        orch.message(&mut t, "also bump the version").unwrap();

        assert_eq!(
            t.task.state,
            TaskState::Building,
            "redirect keeps the phase"
        );
        let recorded = log.lock().unwrap();
        let (prompt, continued) = recorded.last().unwrap();
        assert!(
            *continued,
            "the message rides the harness's own conversation"
        );
        assert_eq!(
            prompt, "also bump the version",
            "with --continue the message IS the next turn, unwrapped"
        );
    }

    #[tokio::test]
    async fn message_wraps_in_task_context_without_a_transcript() {
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            prompt_spy_agent(log.clone()),
            Templates::default(),
        ); // default probe: never a transcript

        let mut t = orch
            .dispatch(
                TaskId::new("m2"),
                "polish the readme",
                TaskKind::Quick,
                "main",
                Default::default(),
            )
            .unwrap();
        orch.message(&mut t, "keep the badge table").unwrap();

        let recorded = log.lock().unwrap();
        let (prompt, continued) = recorded.last().unwrap();
        assert!(!continued);
        assert!(prompt.contains("keep the badge table"), "{prompt}");
        assert!(
            prompt.contains("polish the readme"),
            "a cold session needs the goal for context: {prompt}"
        );
    }

    #[tokio::test]
    async fn message_is_refused_at_gates_terminals_and_when_empty() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = orch
            .dispatch(
                TaskId::new("m3"),
                "gate test",
                TaskKind::Quick,
                "main",
                Default::default(),
            )
            .unwrap();

        let err = orch.message(&mut t, "   ").unwrap_err().to_string();
        assert!(err.contains("empty"), "{err}");

        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.task.state, TaskState::Review);
        let err = orch.message(&mut t, "hello").unwrap_err().to_string();
        assert!(err.contains("review gate"), "{err}");

        orch.approve_merge(&mut t).unwrap();
        let err = orch.message(&mut t, "hello").unwrap_err().to_string();
        assert!(err.contains("no agent"), "{err}");
    }

    #[tokio::test]
    async fn send_notes_revises_then_returns_to_plan_review() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = orch
            .dispatch(
                TaskId::new("p1"),
                "add greeting",
                TaskKind::Standard,
                "main",
                Default::default(),
            )
            .unwrap();

        // First plan → PlanReview.
        std::fs::write(t.worktree.path.join(".build/plan.md"), "# Plan v1\n").unwrap();
        orch.on_done(
            &mut t,
            done(
                DonePhase::Plan,
                DoneStatus::Completed,
                Some(".build/plan.md"),
            ),
        )
        .unwrap();
        assert_eq!(t.task.state, TaskState::PlanReview);

        // Request updates → back to Planning in a fresh revise session.
        orch.send_notes(&mut t, "tighten step 2 and add error handling")
            .unwrap();
        assert_eq!(t.task.state, TaskState::Planning);
        assert!(t.subscribe().is_some(), "a fresh revise session is warm");

        // The agent revises the plan and reports done again → PlanReview.
        std::fs::write(t.worktree.path.join(".build/plan.md"), "# Plan v2\n").unwrap();
        orch.on_done(
            &mut t,
            done(
                DonePhase::Plan,
                DoneStatus::Completed,
                Some(".build/plan.md"),
            ),
        )
        .unwrap();
        assert_eq!(t.task.state, TaskState::PlanReview);
    }

    #[tokio::test]
    async fn request_changes_respawns_build_from_review_and_while_building() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = orch
            .dispatch(
                TaskId::new("c1"),
                "do work",
                TaskKind::Quick,
                "main",
                Default::default(),
            )
            .unwrap();
        assert_eq!(t.task.state, TaskState::Building);

        // While the agent is still building, a change request redirects it: the task
        // stays Building with a fresh session.
        orch.request_changes(&mut t, "use the repository pattern")
            .unwrap();
        assert_eq!(t.task.state, TaskState::Building);
        assert!(t.subscribe().is_some(), "a fresh session is warm");

        // The agent finishes → Review; a change request from Review re-spawns build.
        std::fs::write(t.worktree.path.join("out.txt"), "v1\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.task.state, TaskState::Review);
        orch.request_changes(&mut t, "rename out.txt to result.txt")
            .unwrap();
        assert_eq!(t.task.state, TaskState::Building);
        // The in-progress work is still in the worktree for the new session.
        assert!(t.worktree.path.join("out.txt").exists());
    }

    #[tokio::test]
    async fn commit_keeps_worktree_push_and_merge_push_reach_origin() {
        let (dir, repo) = init_repo();
        // A bare origin, wired as the repo's remote (worktrees share it).
        let origin = dir.path().join("origin.git");
        assert!(Command::new("git")
            .args(["init", "--bare", "-b", "main", origin.to_str().unwrap()])
            .status()
            .unwrap()
            .success());
        assert!(Command::new("git")
            .args([
                "-C",
                repo.to_str().unwrap(),
                "remote",
                "add",
                "origin",
                origin.to_str().unwrap(),
            ])
            .status()
            .unwrap()
            .success());
        let git_origin = |args: &[&str]| {
            let out = Command::new("git")
                .arg("--git-dir")
                .arg(&origin)
                .args(args)
                .output()
                .unwrap();
            (
                out.status.success(),
                String::from_utf8_lossy(&out.stdout).into_owned(),
            )
        };

        let orch = orchestrator(&dir, &repo);
        let mut t = orch
            .dispatch(
                TaskId::new("g1"),
                "do work",
                TaskKind::Quick,
                "main",
                Default::default(),
            )
            .unwrap();
        std::fs::write(t.worktree.path.join("f.txt"), "hi\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.task.state, TaskState::Review);

        // Commit: keeps the worktree and the Review state.
        orch.commit(&t).unwrap();
        assert!(t.worktree.path.exists());
        assert_eq!(t.task.state, TaskState::Review);

        // Push: the feature branch lands in origin; worktree still there.
        orch.push(&t).unwrap();
        assert!(
            git_origin(&["rev-parse", &t.worktree.branch]).0,
            "feature branch pushed"
        );
        assert!(t.worktree.path.exists());

        // Merge + push: base updated in origin, task merged. Cleanup is the caller's
        // step (contract #3), so the worktree survives the merge itself.
        orch.merge_and_push(&mut t).unwrap();
        assert_eq!(t.task.state, TaskState::Merged);
        assert!(
            t.worktree.path.exists(),
            "merge leaves cleanup to the caller"
        );
        orch.discard_worktree(&t.worktree);
        assert!(!t.worktree.path.exists());
        let (ok, tree) = git_origin(&["ls-tree", "--name-only", "main"]);
        assert!(
            ok && tree.contains("f.txt"),
            "base pushed with the work: {tree:?}"
        );
    }

    #[tokio::test]
    async fn resume_redispatches_an_interrupted_build_in_a_fresh_session() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let dispatched = orch
            .dispatch(
                TaskId::new("r1"),
                "do work",
                TaskKind::Quick,
                "main",
                Default::default(),
            )
            .unwrap();

        // Simulate a daemon restart: only the durable core survives, and boot
        // recovery marked the working phase interrupted.
        let mut task = dispatched.task.clone();
        task.apply(TaskEvent::Interrupt).unwrap();
        let mut revived = ActiveTask::reattach(
            task,
            dispatched.worktree.clone(),
            dispatched.plan_path.clone(),
            None,
            Default::default(),
            None,
            Vec::new(),
            None,
            None,
            false,
            Vec::new(),
            false,
            false,
        );
        assert_eq!(
            revived.task.state,
            TaskState::Interrupted(crate::task::Phase::Build)
        );
        assert!(
            revived.subscribe().is_none(),
            "no live session after reattach"
        );

        orch.resume(&mut revived).unwrap();
        assert_eq!(revived.task.state, TaskState::Building);
        assert!(
            revived.subscribe().is_some(),
            "a fresh build session is warm"
        );
    }

    #[tokio::test]
    async fn resume_redispatches_an_interrupted_plan() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let dispatched = orch
            .dispatch(
                TaskId::new("r2"),
                "plan work",
                TaskKind::Standard,
                "main",
                Default::default(),
            )
            .unwrap();

        let mut task = dispatched.task.clone();
        task.apply(TaskEvent::Interrupt).unwrap();
        let mut revived = ActiveTask::reattach(
            task,
            dispatched.worktree.clone(),
            dispatched.plan_path.clone(),
            Some("earlier summary".into()),
            Default::default(),
            None,
            Vec::new(),
            None,
            None,
            false,
            Vec::new(),
            false,
            false,
        );
        orch.resume(&mut revived).unwrap();
        assert_eq!(revived.task.state, TaskState::Planning);
        assert!(
            revived.subscribe().is_some(),
            "a fresh plan session is warm"
        );
        assert_eq!(revived.last_summary.as_deref(), Some("earlier summary"));
    }

    #[tokio::test]
    async fn approve_merge_refuses_when_primary_checkout_is_not_on_base() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = orch
            .dispatch(
                TaskId::new("m1"),
                "do work",
                TaskKind::Quick,
                "main",
                Default::default(),
            )
            .unwrap();
        std::fs::write(t.worktree.path.join("f.txt"), "hi\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();

        // The user wandered off base in their live checkout.
        assert!(Command::new("git")
            .args(["checkout", "-b", "user-feature"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());

        let error = orch.approve_merge(&mut t).expect_err("must refuse");
        let message = error.to_string();
        assert!(
            message.starts_with("merge_failed:"),
            "merge failure carries the contract prefix: {message}"
        );
        assert!(
            message.contains("user-feature") && message.contains("main"),
            "error names both branches: {message}"
        );
        assert!(
            !repo.join("f.txt").exists(),
            "nothing was merged into the wrong branch"
        );
        // Merge honesty: a failed merge leaves the task at its review gate, never
        // `Merged` — the state must match what git actually did.
        assert_eq!(t.task.state, TaskState::Review);

        // Back on base, the same approval now succeeds end to end.
        assert!(Command::new("git")
            .args(["checkout", "main"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());
        orch.approve_merge(&mut t).expect("merge succeeds on base");
        assert_eq!(t.task.state, TaskState::Merged);
        assert!(repo.join("f.txt").exists());
    }

    #[tokio::test]
    async fn ending_a_session_reaps_the_harness_instead_of_leaving_a_zombie() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = orch
            .dispatch(
                TaskId::new("z1"),
                "plan work",
                TaskKind::Standard,
                "main",
                Default::default(),
            )
            .unwrap();
        let pid = t.harness_pid().expect("warm harness is running");
        orch.on_done(
            &mut t,
            done(
                DonePhase::Plan,
                DoneStatus::Completed,
                Some(".build/plan.md"),
            ),
        )
        .unwrap();

        // approve_plan ends the plan session; the old harness must be fully reaped
        // (no zombie), not just killed.
        orch.approve_task_plan(&mut t, None).unwrap();
        let stat = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let stat = String::from_utf8_lossy(&stat.stdout).trim().to_string();
        assert!(
            !stat.starts_with('Z'),
            "old harness pid {pid} is a zombie (stat {stat:?})"
        );
    }

    #[tokio::test]
    async fn harness_exit_and_idle_are_observable_for_the_quiescence_monitor() {
        let (dir, repo) = init_repo();
        // A harness that exits immediately — the crash/exit-without-done case.
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            Agent::Warm(HarnessSpec::new("sh").arg("-c").arg("exit 0")),
            Templates::default(),
        );
        let mut t = orch
            .dispatch(
                TaskId::new("i1"),
                "do work",
                TaskKind::Quick,
                "main",
                Default::default(),
            )
            .unwrap();
        assert_eq!(t.task.state, TaskState::Building);

        // The child exits promptly; poll until has_exited observes it.
        let mut exited = false;
        for _ in 0..50 {
            if t.harness_exited() {
                exited = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(exited, "a dead harness must be observable");
        assert!(t.harness_idle_for().is_some());

        // The monitor's transition: WentIdle demotes to idle_unreported.
        orch.on_idle(&mut t).unwrap();
        assert_eq!(
            t.task.state,
            TaskState::IdleUnreported(crate::task::Phase::Build)
        );
    }

    #[tokio::test]
    async fn abandon_removes_worktree_and_prunes_branch() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = orch
            .dispatch(
                TaskId::new("a1"),
                "scrap this",
                TaskKind::Standard,
                "main",
                Default::default(),
            )
            .unwrap();
        let branch = t.worktree.branch.clone();

        orch.abandon(&mut t).unwrap();
        assert_eq!(t.task.state, TaskState::Abandoned);
        assert!(!t.worktree.path.exists());

        // Contract: abandon best-effort prunes the worktree *and* the branch.
        let r = git2::Repository::open(&repo).unwrap();
        assert!(
            r.find_branch(&branch, git2::BranchType::Local).is_err(),
            "branch pruned after abandon"
        );
    }

    // ---- Worktree adoption ----

    use crate::worktree::{discover_external_worktrees, ExternalWorktree};

    /// Create a user worktree at `dir/<name>` on a new `branch` (cut from the
    /// primary HEAD) and return its discovered summary — the same shape the
    /// app layer resolves a `worktree_id` to.
    fn user_worktree(
        dir: &tempfile::TempDir,
        repo: &Path,
        name: &str,
        branch: &str,
    ) -> ExternalWorktree {
        let path = dir.path().join(name);
        assert!(Command::new("git")
            .args(["worktree", "add", "-b", branch, path.to_str().unwrap()])
            .current_dir(repo)
            .status()
            .unwrap()
            .success());
        discover_external_worktrees(repo, "main", &std::collections::HashSet::new())
            .unwrap()
            .into_iter()
            .find(|w| w.branch.as_deref() == Some(branch))
            .expect("the new worktree is discoverable")
    }

    fn worktree_head(worktree: &Path) -> String {
        let out = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(worktree)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[tokio::test]
    async fn adopt_lands_in_review_with_a_checkpoint_commit() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let external = user_worktree(&dir, &repo, "wt-user", "user/thing");
        std::fs::write(external.path.join("notes.txt"), "pre-Build work\n").unwrap();

        let t = orch
            .adopt(TaskId::new("ad1"), &external, "main", Default::default())
            .unwrap();
        assert_eq!(t.task.state, TaskState::Review);
        assert_eq!(t.task.kind, TaskKind::Quick);
        assert_eq!(t.task.goal, "user/thing", "goal derived from the branch");
        assert!(t.adopted);
        assert!(t.pending_continuation);
        assert!(t.subscribe().is_none(), "adoption spawns no session");
        assert!(external.path.join(".build/mcp.json").exists());

        // The dirty state became its own legible commit, with the exact message.
        assert_eq!(
            last_commit_subject(&external.path),
            "Checkpoint: adopted by Build"
        );
        // Checkpoint-before-scaffold: the checkpoint must not smuggle in mcp.json.
        let shown = Command::new("git")
            .args(["show", "--name-only", "--format=", "HEAD"])
            .current_dir(&external.path)
            .output()
            .unwrap();
        let files = String::from_utf8_lossy(&shown.stdout).into_owned();
        assert!(files.contains("notes.txt"), "{files}");
        assert!(!files.contains(".build/mcp.json"), "{files}");
    }

    #[tokio::test]
    async fn adopt_on_a_clean_worktree_makes_no_checkpoint() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let external = user_worktree(&dir, &repo, "wt-clean", "user/clean");
        let head_before = worktree_head(&external.path);

        let t = orch
            .adopt(TaskId::new("ad2"), &external, "main", Default::default())
            .unwrap();
        assert_eq!(t.task.state, TaskState::Review);
        assert_eq!(
            worktree_head(&external.path),
            head_before,
            "no commit on a clean tree"
        );
    }

    #[tokio::test]
    async fn adopt_refuses_detached_head_and_base_branch() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut external = user_worktree(&dir, &repo, "wt-refuse", "user/refuse");

        let refusal = |external: &ExternalWorktree, id: &str| match orch.adopt(
            TaskId::new(id),
            external,
            "main",
            Default::default(),
        ) {
            Ok(_) => panic!("adoption must be refused"),
            Err(e) => e.to_string(),
        };

        external.branch = None;
        assert_eq!(
            refusal(&external, "rf1"),
            "cannot adopt a detached-HEAD worktree — check out a branch first"
        );

        external.branch = Some("main".into());
        assert_eq!(
            refusal(&external, "rf2"),
            "cannot adopt a worktree with the base branch \"main\" checked out"
        );

        // An option-shaped branch name (untrusted, straight out of `git worktree
        // list`) would be read by `git merge`/`git push` as a flag — refuse it.
        external.branch = Some("--exec=/tmp/pwn.sh".into());
        assert_eq!(
            refusal(&external, "rf3"),
            "cannot adopt a worktree whose branch name \"--exec=/tmp/pwn.sh\" looks like a \
             command-line option — rename the branch first"
        );
    }

    #[tokio::test]
    async fn adopt_generic_branch_takes_the_commit_subject() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let external = user_worktree(&dir, &repo, "wt-wip", "wip");
        assert_eq!(external.head_subject, "initial");

        let t = orch
            .adopt(TaskId::new("gg1"), &external, "main", Default::default())
            .unwrap();
        assert_eq!(t.task.goal, "initial", "generic branch → HEAD subject");
    }

    #[tokio::test]
    async fn adopted_first_session_continues_when_a_transcript_exists() {
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            options_recording_agent(log.clone()),
            Templates::default(),
        )
        .with_transcript_probe(std::sync::Arc::new(|_| true));
        let external = user_worktree(&dir, &repo, "wt-cont", "user/continue-me");

        let mut t = orch
            .adopt(TaskId::new("ct1"), &external, "main", Default::default())
            .unwrap();
        assert!(log.lock().unwrap().is_empty(), "adoption spawns nothing");

        // The FIRST session after adoption continues the user's conversation.
        orch.request_changes(&mut t, "polish it").unwrap();
        assert!(log.lock().unwrap()[0].continue_session);
        assert!(!t.pending_continuation, "one-shot flag consumed");

        // Back to Review, then a second session is fresh (cold-agent discipline).
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.task.state, TaskState::Review);
        orch.request_changes(&mut t, "one more pass").unwrap();
        let recorded = log.lock().unwrap();
        assert_eq!(recorded.len(), 2);
        assert!(!recorded[1].continue_session);
    }

    #[tokio::test]
    async fn adopted_first_session_is_fresh_without_a_transcript() {
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            options_recording_agent(log.clone()),
            Templates::default(),
        )
        .with_transcript_probe(std::sync::Arc::new(|_| false));
        let external = user_worktree(&dir, &repo, "wt-fresh", "user/fresh");

        let mut t = orch
            .adopt(TaskId::new("fr1"), &external, "main", Default::default())
            .unwrap();
        orch.request_changes(&mut t, "polish it").unwrap();
        assert!(!log.lock().unwrap()[0].continue_session);
        assert!(
            !t.pending_continuation,
            "flag consumed even without a transcript"
        );
    }

    #[tokio::test]
    async fn adopted_merge_then_abandon_flow_still_works() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);

        // Merge: the adopted work lands on base like any reviewed task.
        let external = user_worktree(&dir, &repo, "wt-merge", "user/merge-me");
        std::fs::write(external.path.join("feature.txt"), "work\n").unwrap();
        let mut t = orch
            .adopt(TaskId::new("mg1"), &external, "main", Default::default())
            .unwrap();
        orch.approve_merge(&mut t).unwrap();
        assert_eq!(t.task.state, TaskState::Merged);
        assert!(repo.join("feature.txt").exists(), "merged into base");

        // Abandon: user-triggered, so pruning the adopted worktree is allowed.
        let external = user_worktree(&dir, &repo, "wt-drop", "user/abandon-me");
        let mut t = orch
            .adopt(TaskId::new("ab1"), &external, "main", Default::default())
            .unwrap();
        orch.abandon(&mut t).unwrap();
        assert_eq!(t.task.state, TaskState::Abandoned);
        assert!(!external.path.exists(), "worktree pruned");
        let r = git2::Repository::open(&repo).unwrap();
        assert!(
            r.find_branch("user/abandon-me", git2::BranchType::Local)
                .is_err(),
            "branch pruned"
        );
    }

    /// Run git in `dir`, asserting success.
    fn run_git(dir: &Path, args: &[&str]) {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(dir)
                .status()
                .unwrap()
                .success(),
            "git {args:?} failed in {dir:?}"
        );
    }

    #[tokio::test]
    async fn commits_exclude_the_scaffolded_mcp_config() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let external = user_worktree(&dir, &repo, "wt-mcp", "user/mcp-exclude");
        std::fs::write(external.path.join("work.txt"), "user work\n").unwrap();

        let mut t = orch
            .adopt(TaskId::new("adm1"), &external, "main", Default::default())
            .unwrap();
        // approve_merge's implicit commit_all sweeps the worktree; the scaffolded
        // machine-local MCP config must never ride along into base.
        orch.approve_merge(&mut t).unwrap();

        let tracked = Command::new("git")
            .args(["ls-tree", "-r", "--name-only", "main"])
            .current_dir(&repo)
            .output()
            .unwrap();
        let tracked = String::from_utf8_lossy(&tracked.stdout).into_owned();
        assert!(tracked.contains("work.txt"), "{tracked}");
        assert!(
            !tracked.contains(".build/mcp.json"),
            "machine-local MCP config merged into base: {tracked}"
        );
    }

    #[tokio::test]
    async fn agent_git_add_all_cannot_commit_the_scaffolded_mcp_config() {
        // The build templates now tell the agent to commit its own work. A stage
        // agent that runs `git add -A && git commit` must NOT capture the
        // machine-local .build/mcp.json — the scaffolded .build/.gitignore is the
        // hard guard (Build's sweep pathspec only protects Build's own commits).
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = orch
            .dispatch(
                TaskId::new("gi1"),
                "Add a greeting",
                TaskKind::Standard,
                "main",
                Default::default(),
            )
            .unwrap();
        assert!(t.worktree.path.join(".build/mcp.json").exists());
        assert!(t.worktree.path.join(".build/.gitignore").exists());

        // The agent implements work and commits everything the way a harness does.
        std::fs::write(t.worktree.path.join("greeting.txt"), "hi\n").unwrap();
        run_git(&t.worktree.path, &["add", "-A"]);
        run_git(&t.worktree.path, &["commit", "-m", "feat: add greeting"]);

        let tracked = Command::new("git")
            .args(["ls-files"])
            .current_dir(&t.worktree.path)
            .output()
            .unwrap();
        let tracked = String::from_utf8_lossy(&tracked.stdout).into_owned();
        assert!(tracked.contains("greeting.txt"), "{tracked}");
        assert!(
            !tracked.contains(".build/mcp.json"),
            "agent git add -A committed the machine-local MCP config: {tracked}"
        );

        t.end_session();
    }

    #[tokio::test]
    async fn adopted_merge_succeeds_when_base_already_tracks_a_task_mcp_config() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        // The external branch forks first; base then gains a committed
        // .build/mcp.json (what earlier native-task merges used to leave behind).
        let external = user_worktree(&dir, &repo, "wt-old-fork", "user/older-fork");
        std::fs::create_dir_all(repo.join(".build")).unwrap();
        std::fs::write(repo.join(".build/mcp.json"), "{\"mcpServers\":{}}\n").unwrap();
        run_git(&repo, &["add", "-A"]);
        run_git(&repo, &["commit", "-m", "polluted base"]);
        std::fs::write(external.path.join("feature.txt"), "work\n").unwrap();

        let mut t = orch
            .adopt(TaskId::new("adm2"), &external, "main", Default::default())
            .unwrap();
        orch.approve_merge(&mut t)
            .expect("adopted merge must not add/add-conflict on the MCP config");
        assert_eq!(t.task.state, TaskState::Merged);
    }

    #[tokio::test]
    async fn merge_conflict_reports_the_conflict_and_leaves_base_clean() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let external = user_worktree(&dir, &repo, "wt-conflict", "user/conflicting");
        // Same path, different content on both sides of the fork.
        std::fs::write(repo.join("shared.txt"), "base version\n").unwrap();
        run_git(&repo, &["add", "-A"]);
        run_git(&repo, &["commit", "-m", "base edit"]);
        std::fs::write(external.path.join("shared.txt"), "external version\n").unwrap();

        let mut t = orch
            .adopt(TaskId::new("adc1"), &external, "main", Default::default())
            .unwrap();
        let err = orch
            .approve_merge(&mut t)
            .expect_err("conflicting merge must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("CONFLICT") || msg.contains("shared.txt"),
            "the conflict detail must reach the user, got: {msg}"
        );
        // The failed merge was aborted: the primary checkout is clean again.
        let status = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&status.stdout).trim(),
            "",
            "primary checkout left dirty after failed merge"
        );
    }

    // ---- Multi-stage: lifecycle ----

    use crate::task::{StageManifestEntry, StageState, ValidationReport};

    fn manifest_entry(id: &str, title: &str, position: usize) -> StageManifestEntry {
        StageManifestEntry {
            id: id.into(),
            title: title.into(),
            path: format!(".build/plan/{position:02}-{id}.md"),
            summary: format!("{title}."),
        }
    }

    fn done_plan_stages(entries: Vec<StageManifestEntry>) -> DoneReport {
        DoneReport {
            phase: DonePhase::Plan,
            status: DoneStatus::Completed,
            summary: "planned".into(),
            outputs: DoneOutputs {
                plan_path: Some(templates::STAGES_MANIFEST_PATH.to_string()),
                stages: Some(entries),
                ..DoneOutputs::default()
            },
        }
    }

    fn done_validate(passed: bool, findings: &str, notes: &str) -> DoneReport {
        DoneReport {
            phase: DonePhase::Validate,
            status: DoneStatus::Completed,
            summary: "validated".into(),
            outputs: DoneOutputs {
                validation: Some(ValidationReport {
                    passed,
                    findings: findings.into(),
                    notes_for_next_stage: notes.into(),
                }),
                ..DoneOutputs::default()
            },
        }
    }

    /// Dispatch a standard task and land it at PlanReview with a two-stage
    /// manifest (the test plays the plan agent: write the docs, echo via done).
    fn two_stage_task(orch: &Orchestrator, id: &str) -> ActiveTask {
        let mut t = orch
            .dispatch(
                TaskId::new(id),
                "Add greetings",
                TaskKind::Standard,
                "main",
                Default::default(),
            )
            .unwrap();
        let plan_dir = t.worktree.path.join(".build/plan");
        std::fs::create_dir_all(&plan_dir).unwrap();
        std::fs::write(plan_dir.join("01-first.md"), "# Stage: First\n").unwrap();
        std::fs::write(plan_dir.join("02-second.md"), "# Stage: Second\n").unwrap();
        orch.on_done(
            &mut t,
            done_plan_stages(vec![
                manifest_entry("first", "First", 1),
                manifest_entry("second", "Second", 2),
            ]),
        )
        .unwrap();
        assert_eq!(t.task.state, TaskState::PlanReview);
        t
    }

    fn last_commit_subject(worktree: &Path) -> String {
        let out = Command::new("git")
            .args(["log", "-1", "--format=%s"])
            .current_dir(worktree)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A one-shot agent that records every rendered prompt it is asked to spawn.
    fn prompt_recording_agent(log: std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> Agent {
        Agent::OneShot(std::sync::Arc::new(
            move |prompt: &str, _choice: &ModelChoice, _options: &SpawnOptions| {
                log.lock().unwrap().push(prompt.to_string());
                HarnessSpec::new("sh").arg("-c").arg("exit 0")
            },
        ))
    }

    #[tokio::test]
    async fn multi_stage_spine_stages_build_validate_then_review() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "ms1");
        assert!(t.is_multi_stage());
        assert_eq!(t.plan_path, templates::STAGES_MANIFEST_PATH);
        assert_eq!(t.stage("first").unwrap().state, StageState::Planned);
        assert_eq!(t.stage("second").unwrap().state, StageState::Planned);

        orch.approve_stage(&mut t, "first").unwrap();
        orch.approve_stage(&mut t, "second").unwrap();
        assert_eq!(t.stage("first").unwrap().state, StageState::Approved);
        assert_eq!(
            t.task.state,
            TaskState::PlanReview,
            "approval is bookkeeping"
        );

        // Dispatch stage 1: fresh cold session, start sha pinned.
        orch.dispatch_stage(&mut t, "first", None).unwrap();
        assert_eq!(t.task.state, TaskState::Building);
        assert_eq!(t.stage("first").unwrap().state, StageState::Building);
        assert_eq!(t.current_stage_id.as_deref(), Some("first"));
        let start_sha = t.stage("first").unwrap().start_sha.clone().expect("sha");
        assert!(t.subscribe().is_some(), "stage build session is warm");

        // The build agent works, then reports done → committed + validating; the
        // task-level state does NOT move (only validation's verdict moves it).
        std::fs::write(t.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.task.state, TaskState::Building, "validation is running");
        assert_eq!(t.stage("first").unwrap().state, StageState::Validating);
        let subject = last_commit_subject(&t.worktree.path);
        assert!(
            subject.contains("stage first"),
            "stage work committed before validation: {subject:?}"
        );

        // Validation passes → back to the stage board (PlanReview), report kept.
        orch.on_done(&mut t, done_validate(true, "- ok", "note for second"))
            .unwrap();
        assert_eq!(t.task.state, TaskState::PlanReview);
        assert_eq!(
            t.stage("first").unwrap().state,
            StageState::Validated { passed: true }
        );
        assert_eq!(
            t.stage("first")
                .unwrap()
                .validation
                .as_ref()
                .map(|v| v.notes_for_next_stage.as_str()),
            Some("note for second")
        );
        assert_eq!(
            t.stage("first").unwrap().start_sha.as_deref(),
            Some(start_sha.as_str()),
            "start sha survives validation"
        );

        // Stage 2 builds on stage 1's commits; final validation opens merge review.
        orch.dispatch_stage(&mut t, "second", None).unwrap();
        assert_eq!(t.task.state, TaskState::Building);
        std::fs::write(t.worktree.path.join("second.txt"), "two\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        orch.on_done(&mut t, done_validate(true, "- ok", ""))
            .unwrap();
        assert_eq!(t.task.state, TaskState::Review, "last stage → merge review");

        orch.approve_merge(&mut t).unwrap();
        assert_eq!(t.task.state, TaskState::Merged);
        assert!(repo.join("first.txt").exists());
        assert!(repo.join("second.txt").exists());
    }

    /// Working-tree entries other than the machine-local `.build/mcp.json`,
    /// which is always untracked (Build excludes it from every sweep).
    fn dirty_paths_excluding_mcp(worktree: &Path) -> Vec<String> {
        let out = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(worktree)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|line| !line.contains(".build/mcp.json"))
            .map(str::to_string)
            .collect()
    }

    #[tokio::test]
    async fn stage_agent_commits_are_kept_without_a_build_sweep_commit() {
        // When the agent authors its own atomic commits and leaves a clean
        // tree, Build's safety-net sweep is a no-op: HEAD stays the agent's
        // commit, so the history is entirely agent-authored.
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "ms-commit");
        orch.approve_stage(&mut t, "first").unwrap();
        orch.approve_stage(&mut t, "second").unwrap();
        orch.dispatch_stage(&mut t, "first", None).unwrap();

        // The agent commits its own work the way a real harness does — a plain
        // `git add -A`; the scaffolded `.build/.gitignore` keeps the machine-local
        // mcp config out without the agent needing to know about it.
        std::fs::write(t.worktree.path.join("first.txt"), "one\n").unwrap();
        run_git(&t.worktree.path, &["add", "-A"]);
        run_git(
            &t.worktree.path,
            &["commit", "-m", "Add the first greeting module"],
        );
        let agent_head = worktree_head(&t.worktree.path);

        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();

        assert_eq!(t.stage("first").unwrap().state, StageState::Validating);
        assert_eq!(
            worktree_head(&t.worktree.path),
            agent_head,
            "Build must append no sweep commit over a clean tree"
        );
        assert_eq!(
            last_commit_subject(&t.worktree.path),
            "Add the first greeting module",
            "HEAD is the agent's own message"
        );
        assert!(
            dirty_paths_excluding_mcp(&t.worktree.path).is_empty(),
            "tree clean after done"
        );
    }

    #[tokio::test]
    async fn build_sweeps_uncommitted_stage_work_with_an_honest_checkpoint_message() {
        // When the agent leaves work uncommitted, Build's fallback sweep still
        // guarantees a committed boundary before validation — with an honest
        // "swept by Build" checkpoint message that still names the stage.
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "ms-sweep");
        orch.approve_stage(&mut t, "first").unwrap();
        orch.approve_stage(&mut t, "second").unwrap();
        orch.dispatch_stage(&mut t, "first", None).unwrap();

        std::fs::write(t.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();

        assert_eq!(t.stage("first").unwrap().state, StageState::Validating);
        let subject = last_commit_subject(&t.worktree.path);
        assert!(
            subject.contains("stage first"),
            "checkpoint subject names the stage: {subject:?}"
        );
        assert!(
            subject.contains("checkpoint") && subject.contains("swept by Build"),
            "honest safety-net subject: {subject:?}"
        );
        assert!(
            dirty_paths_excluding_mcp(&t.worktree.path).is_empty(),
            "tree clean after the fallback sweep"
        );
        assert!(t.worktree.path.join("first.txt").exists());
    }

    #[tokio::test]
    async fn dispatch_stage_gates_reject_out_of_order_dispatches() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "g1");

        // Unknown stage id.
        let err = orch.dispatch_stage(&mut t, "nope", None).unwrap_err();
        assert_eq!(err.to_string(), "unknown stage_id: nope");

        // Not approved yet.
        let err = orch.dispatch_stage(&mut t, "first", None).unwrap_err();
        assert!(err.to_string().contains("not approved"), "{err}");
        assert_eq!(t.task.state, TaskState::PlanReview, "gate rejects cleanly");

        orch.approve_stage(&mut t, "first").unwrap();
        orch.approve_stage(&mut t, "second").unwrap();

        // Stage 2 while stage 1 has not passed validation.
        let err = orch.dispatch_stage(&mut t, "second", None).unwrap_err();
        assert!(
            err.to_string().contains("has not passed validation"),
            "{err}"
        );

        // From a non-PlanReview state (stage 1 building).
        orch.dispatch_stage(&mut t, "first", None).unwrap();
        let err = orch.dispatch_stage(&mut t, "second", None).unwrap_err();
        assert!(err.to_string().contains("cannot dispatch a stage"), "{err}");
        assert_eq!(t.stage("second").unwrap().state, StageState::Approved);
    }

    #[tokio::test]
    async fn failed_validation_parks_at_plan_review_and_disarms_run_all() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "vf1");
        orch.approve_stage(&mut t, "first").unwrap();
        orch.approve_stage(&mut t, "second").unwrap();
        t.auto_advance = true;
        orch.dispatch_stage(&mut t, "first", None).unwrap();
        std::fs::write(t.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();

        orch.on_done(&mut t, done_validate(false, "- migration missing", ""))
            .unwrap();
        assert_eq!(t.task.state, TaskState::PlanReview);
        assert_eq!(
            t.stage("first").unwrap().state,
            StageState::Validated { passed: false }
        );
        assert_eq!(
            t.stage("first")
                .unwrap()
                .validation
                .as_ref()
                .map(|v| v.findings.as_str()),
            Some("- migration missing")
        );
        assert!(!t.auto_advance, "a failed validation disarms run-all");
        assert_eq!(
            t.stage("second").unwrap().state,
            StageState::Approved,
            "the next stage was never dispatched"
        );
    }

    #[tokio::test]
    async fn auto_advance_dispatches_the_next_approved_stage() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "aa1");
        orch.approve_stage(&mut t, "first").unwrap();
        orch.approve_stage(&mut t, "second").unwrap();
        t.auto_advance = true;
        orch.dispatch_stage(&mut t, "first", None).unwrap();
        std::fs::write(t.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();

        // Validation pass → the next approved stage dispatches with no human call.
        orch.on_done(&mut t, done_validate(true, "- ok", ""))
            .unwrap();
        assert_eq!(t.task.state, TaskState::Building);
        assert_eq!(t.current_stage_id.as_deref(), Some("second"));
        assert_eq!(t.stage("second").unwrap().state, StageState::Building);
        assert!(t.auto_advance, "run-all stays armed after a pass");
        assert!(t.subscribe().is_some(), "stage 2 session is warm");
    }

    #[tokio::test]
    async fn auto_advance_waits_at_plan_review_when_next_stage_is_unapproved() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "aa2");
        orch.approve_stage(&mut t, "first").unwrap();
        t.auto_advance = true;
        orch.dispatch_stage(&mut t, "first", None).unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        orch.on_done(&mut t, done_validate(true, "- ok", ""))
            .unwrap();

        assert_eq!(t.task.state, TaskState::PlanReview, "waits for approval");
        assert_eq!(t.stage("second").unwrap().state, StageState::Planned);
        assert!(t.auto_advance, "run-all stays armed while waiting");
    }

    #[tokio::test]
    async fn message_waits_for_a_stage_validation_verdict() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "mv1");
        orch.approve_stage(&mut t, "first").unwrap();
        orch.dispatch_stage(&mut t, "first", None).unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.stage("first").unwrap().state, StageState::Validating);

        // Only a validate report may move a Validating stage — a redirect here
        // would orphan the verdict, exactly like request_changes.
        let err = orch.message(&mut t, "hurry up").unwrap_err().to_string();
        assert!(err.contains("awaiting validation"), "{err}");
    }

    #[tokio::test]
    async fn blocked_during_validation_keeps_the_stage_validating() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "bv1");
        orch.approve_stage(&mut t, "first").unwrap();
        t.auto_advance = true;
        orch.dispatch_stage(&mut t, "first", None).unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.stage("first").unwrap().state, StageState::Validating);

        orch.on_done(&mut t, done(DonePhase::Validate, DoneStatus::Blocked, None))
            .unwrap();
        assert_eq!(t.task.state, TaskState::Blocked(crate::task::Phase::Build));
        assert_eq!(
            t.stage("first").unwrap().state,
            StageState::Validating,
            "the stage sub-state routes recovery back to a validate session"
        );
        assert!(!t.auto_advance, "blocked disarms run-all");
    }

    #[tokio::test]
    async fn stray_validate_and_build_reports_are_ignored_not_promoted() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);

        // A legacy quick task must ignore a validate report outright.
        let mut quick = orch
            .dispatch(
                TaskId::new("sv1"),
                "quick work",
                TaskKind::Quick,
                "main",
                Default::default(),
            )
            .unwrap();
        orch.on_done(&mut quick, done_validate(true, "- ok", ""))
            .unwrap();
        assert_eq!(quick.task.state, TaskState::Building, "ignored");

        // A multi-stage task mid-validation must ignore a stray build report —
        // otherwise a rogue `done(build)` would skip the validation gate.
        let mut t = two_stage_task(&orch, "sv2");
        orch.approve_stage(&mut t, "first").unwrap();
        orch.dispatch_stage(&mut t, "first", None).unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.stage("first").unwrap().state, StageState::Validating);
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.task.state, TaskState::Building);
        assert_eq!(t.stage("first").unwrap().state, StageState::Validating);
    }

    #[tokio::test]
    async fn stray_plan_report_in_an_illegal_state_never_touches_the_manifest() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "sp1");
        orch.approve_stage(&mut t, "first").unwrap();
        orch.dispatch_stage(&mut t, "first", None).unwrap();
        assert_eq!(t.task.state, TaskState::Building);

        // A live build/fix/validate session misusing done(phase=plan) must be
        // rejected with ZERO mutation: on_agent_done persists the task even on
        // Err, so a manifest merged before the legality check would let any
        // in-flight session rewrite an already-reviewed stage's path/title.
        let mut poisoned = manifest_entry("first", "First (poisoned)", 1);
        poisoned.path = ".build/plan/99-other.md".into();
        let mut stray = done_plan_stages(vec![poisoned]);
        stray.summary = "stray".into();
        stray.outputs.plan_path = Some(".build/evil.md".into());
        let err = orch.on_done(&mut t, stray).unwrap_err();
        assert!(matches!(err, OrchestratorError::Transition(_)), "{err}");
        assert_eq!(t.task.state, TaskState::Building);
        assert_eq!(t.stage("first").unwrap().title, "First");
        assert_eq!(t.stage("first").unwrap().path, ".build/plan/01-first.md");
        assert_eq!(t.plan_path, templates::STAGES_MANIFEST_PATH);
        assert_ne!(t.last_summary.as_deref(), Some("stray"));
    }

    #[tokio::test]
    async fn late_build_report_while_blocked_is_rejected_without_mutation() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "lb1");
        orch.approve_stage(&mut t, "first").unwrap();
        orch.dispatch_stage(&mut t, "first", None).unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Blocked, None))
            .unwrap();
        assert_eq!(t.task.state, TaskState::Blocked(crate::task::Phase::Build));
        assert_eq!(t.stage("first").unwrap().state, StageState::Building);

        // The user types into the PTY, the agent finishes and reports completed
        // while the task is still Blocked — same as legacy, the report must be
        // rejected atomically (no stage advance, no commit, no session swap).
        let before = last_commit_subject(&t.worktree.path);
        std::fs::write(t.worktree.path.join("first.txt"), "one\n").unwrap();
        let err = orch
            .on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap_err();
        assert!(matches!(err, OrchestratorError::Transition(_)), "{err}");
        assert_eq!(t.task.state, TaskState::Blocked(crate::task::Phase::Build));
        assert_eq!(
            t.stage("first").unwrap().state,
            StageState::Building,
            "no half-applied stage advance"
        );
        assert_eq!(
            last_commit_subject(&t.worktree.path),
            before,
            "no commit while blocked"
        );
        assert!(t.subscribe().is_some(), "session kept for the reply");
    }

    #[tokio::test]
    async fn late_validation_verdict_while_blocked_is_rejected_without_mutation() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "lv1");
        orch.approve_stage(&mut t, "first").unwrap();
        orch.dispatch_stage(&mut t, "first", None).unwrap();
        std::fs::write(t.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        orch.on_done(&mut t, done(DonePhase::Validate, DoneStatus::Blocked, None))
            .unwrap();
        assert_eq!(t.task.state, TaskState::Blocked(crate::task::Phase::Build));
        assert_eq!(t.stage("first").unwrap().state, StageState::Validating);

        // A completed verdict while the task is Blocked must not advance the
        // stage to a terminal Validated, store the report, or kill the session
        // — that pairing (Blocked + stage-terminal, no session) is unrecoverable.
        let err = orch
            .on_done(&mut t, done_validate(true, "- ok", ""))
            .unwrap_err();
        assert!(matches!(err, OrchestratorError::Transition(_)), "{err}");
        assert_eq!(t.task.state, TaskState::Blocked(crate::task::Phase::Build));
        assert_eq!(t.stage("first").unwrap().state, StageState::Validating);
        assert!(
            t.stage("first").unwrap().validation.is_none(),
            "verdict not stored"
        );
        assert!(t.subscribe().is_some(), "session kept for the reply");
    }

    #[tokio::test]
    async fn multi_stage_review_change_requests_round_trip_like_legacy() {
        // After the final stage validates, the diff-review loop (request_changes →
        // done) must still work even though the task is multi-stage: the current
        // stage is already validated, so the revise `done` closes as BuildReady.
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "rc1");
        for stage_id in ["first", "second"] {
            orch.approve_stage(&mut t, stage_id).unwrap();
            orch.dispatch_stage(&mut t, stage_id, None).unwrap();
            std::fs::write(t.worktree.path.join(format!("{stage_id}.txt")), "x\n").unwrap();
            orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
                .unwrap();
            orch.on_done(&mut t, done_validate(true, "- ok", ""))
                .unwrap();
        }
        assert_eq!(t.task.state, TaskState::Review);

        orch.request_changes(&mut t, "rename the file").unwrap();
        assert_eq!(t.task.state, TaskState::Building);
        orch.on_done(&mut t, done(DonePhase::Revise, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.task.state, TaskState::Review, "change loop closes");
    }

    #[tokio::test]
    async fn stage_prompts_carry_stage_doc_prior_notes_and_start_sha() {
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            prompt_recording_agent(log.clone()),
            Templates::default(),
        );
        let mut t = two_stage_task(&orch, "pp1");
        orch.approve_stage(&mut t, "first").unwrap();
        orch.approve_stage(&mut t, "second").unwrap();

        orch.dispatch_stage(&mut t, "first", None).unwrap();
        let build_prompt = log.lock().unwrap().last().unwrap().clone();
        assert!(build_prompt.contains("\"First\""), "{build_prompt}");
        assert!(
            build_prompt.contains(".build/plan/01-first.md"),
            "{build_prompt}"
        );

        std::fs::write(t.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        let validate_prompt = log.lock().unwrap().last().unwrap().clone();
        let start_sha = t.stage("first").unwrap().start_sha.clone().unwrap();
        assert!(validate_prompt.contains("VALIDATION"), "{validate_prompt}");
        assert!(validate_prompt.contains(&start_sha), "{validate_prompt}");
        assert!(
            validate_prompt.contains(".build/plan/02-second.md"),
            "next stage doc is in the validation prompt: {validate_prompt}"
        );

        orch.on_done(&mut t, done_validate(true, "- ok", "watch the rename"))
            .unwrap();
        orch.dispatch_stage(&mut t, "second", None).unwrap();
        let second_prompt = log.lock().unwrap().last().unwrap().clone();
        assert!(
            second_prompt.contains("watch the rename"),
            "prior validation notes reach the next stage build: {second_prompt}"
        );

        // Final-stage validation renders an empty next_stage_path.
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        let final_validate = log.lock().unwrap().last().unwrap().clone();
        assert!(
            !final_validate.contains("{next_stage_path}"),
            "{final_validate}"
        );
    }

    #[tokio::test]
    async fn replan_merges_the_manifest_by_id_and_keeps_built_stages() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "rm1");

        // Drive "first" all the way to validated so it has built work.
        orch.approve_stage(&mut t, "first").unwrap();
        orch.dispatch_stage(&mut t, "first", None).unwrap();
        std::fs::write(t.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        orch.on_done(&mut t, done_validate(true, "- ok", ""))
            .unwrap();
        assert_eq!(t.task.state, TaskState::PlanReview);

        // A re-plan drops "first" (built → kept, with its record), retitles
        // "second" (state kept, metadata refreshed), and appends "third".
        orch.send_notes(&mut t, "restructure").unwrap();
        orch.on_done(
            &mut t,
            done_plan_stages(vec![
                manifest_entry("second", "Second v2", 2),
                manifest_entry("third", "Third", 3),
            ]),
        )
        .unwrap();
        let ids: Vec<&str> = t.stages.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["first", "second", "third"]);
        assert_eq!(
            t.stage("first").unwrap().state,
            StageState::Validated { passed: true },
            "built work is never vaporized by a revision"
        );
        assert!(t.stage("first").unwrap().validation.is_some());
        assert_eq!(t.stage("second").unwrap().title, "Second v2");
        assert_eq!(t.stage("second").unwrap().state, StageState::Planned);
        assert_eq!(t.stage("third").unwrap().state, StageState::Planned);

        // A dropped stage that never built simply disappears.
        orch.send_notes(&mut t, "drop third").unwrap();
        orch.on_done(
            &mut t,
            done_plan_stages(vec![manifest_entry("second", "Second v2", 2)]),
        )
        .unwrap();
        let ids: Vec<&str> = t.stages.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["first", "second"]);
    }

    #[tokio::test]
    async fn stage_revision_done_resolves_comments_and_resets_approval() {
        use crate::task::CommentState;
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "cr1");
        orch.approve_stage(&mut t, "first").unwrap();
        t.comments = vec![
            comment_on("c-1", "first", CommentState::Open),
            comment_on("c-2", "first", CommentState::Open),
        ];

        // A revise session is in flight for "first" (send_stage_notes sets this
        // up; here the state is arranged directly to isolate the done handling).
        t.task.apply(TaskEvent::SendNotes).unwrap();
        t.revising_stage_id = Some("first".into());

        orch.on_done(
            &mut t,
            DoneReport {
                phase: DonePhase::Revise,
                status: DoneStatus::Completed,
                summary: "revised".into(),
                outputs: DoneOutputs {
                    comment_resolutions: Some(vec![
                        crate::mcp::CommentResolution {
                            comment_id: "c-1".into(),
                            response: "switched to a timestamp".into(),
                        },
                        crate::mcp::CommentResolution {
                            comment_id: "c-999".into(),
                            response: "unknown id is skipped".into(),
                        },
                    ]),
                    ..DoneOutputs::default()
                },
            },
        )
        .unwrap();

        assert_eq!(t.task.state, TaskState::PlanReview);
        assert_eq!(
            t.stage("first").unwrap().state,
            StageState::Planned,
            "a revised doc resets the stale approval"
        );
        assert_eq!(t.revising_stage_id, None);
        let c1 = t.comments.iter().find(|c| c.id == "c-1").unwrap();
        assert_eq!(c1.state, CommentState::Addressed);
        assert_eq!(c1.agent_reply.as_deref(), Some("switched to a timestamp"));
        let c2 = t.comments.iter().find(|c| c.id == "c-2").unwrap();
        assert_eq!(
            c2.state,
            CommentState::Open,
            "unresolved comments stay open"
        );
    }

    #[tokio::test]
    async fn late_stage_revision_report_after_idle_demotion_is_still_consumed() {
        use crate::task::CommentState;
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "ir1");
        orch.approve_stage(&mut t, "first").unwrap();
        t.comments = vec![comment_on("c-1", "first", CommentState::Open)];
        orch.send_stage_notes(&mut t, "first").unwrap();
        assert_eq!(t.task.state, TaskState::Planning);

        // The revise agent went quiet past the idle threshold, then finished
        // anyway. Quiescence never decided anything, so the late report must be
        // consumed exactly like the on-time one — not misrouted and dropped.
        orch.on_idle(&mut t).unwrap();
        assert_eq!(
            t.task.state,
            TaskState::IdleUnreported(crate::task::Phase::Plan)
        );
        orch.on_done(
            &mut t,
            DoneReport {
                phase: DonePhase::Revise,
                status: DoneStatus::Completed,
                summary: "revised".into(),
                outputs: DoneOutputs {
                    comment_resolutions: Some(vec![crate::mcp::CommentResolution {
                        comment_id: "c-1".into(),
                        response: "reworded the schema section".into(),
                    }]),
                    ..DoneOutputs::default()
                },
            },
        )
        .unwrap();
        assert_eq!(t.task.state, TaskState::PlanReview);
        assert_eq!(t.revising_stage_id, None);
        assert_eq!(
            t.stage("first").unwrap().state,
            StageState::Planned,
            "the revised doc resets the stale approval"
        );
        let c1 = t.comments.iter().find(|c| c.id == "c-1").unwrap();
        assert_eq!(c1.state, CommentState::Addressed);
        assert_eq!(
            c1.agent_reply.as_deref(),
            Some("reworded the schema section")
        );
    }

    // ---- Multi-stage: per-stage revision, fix sessions, and resume routing ----

    fn anchored_comment(id: &str, stage_id: &str) -> crate::task::StageComment {
        crate::task::StageComment {
            id: id.to_string(),
            stage_id: stage_id.to_string(),
            anchor: Some(crate::task::CommentAnchor {
                heading_path: vec!["Database schema".into(), "Tables".into()],
                snippet: "users table gets a soft-delete column".into(),
            }),
            body: "use a deleted_at timestamp".into(),
            state: crate::task::CommentState::Open,
            agent_reply: None,
        }
    }

    /// Simulate a daemon restart mid-flight: only the durable core survives and
    /// boot recovery has marked the working phase interrupted.
    fn interrupted_copy(t: &ActiveTask) -> ActiveTask {
        let mut task = t.task.clone();
        task.apply(TaskEvent::Interrupt).unwrap();
        ActiveTask::reattach(
            task,
            t.worktree.clone(),
            t.plan_path.clone(),
            t.last_summary.clone(),
            Default::default(),
            None,
            t.stages.clone(),
            t.current_stage_id.clone(),
            t.revising_stage_id.clone(),
            t.auto_advance,
            t.comments.clone(),
            false,
            false,
        )
    }

    #[tokio::test]
    async fn send_stage_notes_spawns_a_revise_session_from_the_stored_comments() {
        use crate::task::CommentState;
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            prompt_recording_agent(log.clone()),
            Templates::default(),
        );
        let mut t = two_stage_task(&orch, "sn1");
        orch.approve_stage(&mut t, "first").unwrap();

        // No open comments → gate.
        let err = orch.send_stage_notes(&mut t, "first").unwrap_err();
        assert_eq!(err.to_string(), "no open comments on stage first");

        t.comments = vec![
            anchored_comment("c-1", "first"),
            comment_on("c-2", "first", CommentState::Open),
            comment_on("c-3", "first", CommentState::Addressed),
            comment_on("c-4", "second", CommentState::Open),
        ];
        orch.send_stage_notes(&mut t, "first").unwrap();
        assert_eq!(t.task.state, TaskState::Planning);
        assert_eq!(t.revising_stage_id.as_deref(), Some("first"));
        let prompt = log.lock().unwrap().last().unwrap().clone();
        assert!(
            prompt.contains(
                "1. [c-1] Under \"Database schema > Tables\", on the passage: \"users table gets a soft-delete column\""
            ),
            "{prompt}"
        );
        assert!(prompt.contains("2. [c-2] (general)"), "{prompt}");
        assert!(
            !prompt.contains("c-3") && !prompt.contains("c-4"),
            "addressed and other-stage comments stay out: {prompt}"
        );
        assert!(prompt.contains("\"First\""), "{prompt}");
    }

    #[tokio::test]
    async fn send_stage_notes_gates_on_task_state_and_stage_state() {
        use crate::task::CommentState;
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "sn2");
        orch.approve_stage(&mut t, "first").unwrap();
        t.comments = vec![
            comment_on("c-1", "first", CommentState::Open),
            comment_on("c-2", "second", CommentState::Open),
        ];

        // Task not at a plan gate (stage 1 building) → rejected.
        orch.dispatch_stage(&mut t, "first", None).unwrap();
        let err = orch.send_stage_notes(&mut t, "second").unwrap_err();
        assert!(err.to_string().contains("cannot send stage notes"), "{err}");

        // Stage past plan review (validated_failed awaits fix, not notes).
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        orch.on_done(&mut t, done_validate(false, "- broken", ""))
            .unwrap();
        assert_eq!(t.task.state, TaskState::PlanReview);
        let err = orch.send_stage_notes(&mut t, "first").unwrap_err();
        assert!(err.to_string().contains("not in plan review"), "{err}");
    }

    #[tokio::test]
    async fn fix_stage_respawns_with_findings_and_keeps_the_start_sha() {
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            prompt_recording_agent(log.clone()),
            Templates::default(),
        );
        let mut t = two_stage_task(&orch, "fx1");
        orch.approve_stage(&mut t, "first").unwrap();
        orch.dispatch_stage(&mut t, "first", None).unwrap();
        let start_sha = t.stage("first").unwrap().start_sha.clone().unwrap();
        std::fs::write(t.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        orch.on_done(&mut t, done_validate(false, "- missing index", ""))
            .unwrap();

        // Only a validation-failed stage can be sent to a fix session.
        let err = orch.fix_stage(&mut t, "second", "").unwrap_err();
        assert!(err.to_string().contains("no failed validation"), "{err}");

        orch.fix_stage(&mut t, "first", "also add the covering index")
            .unwrap();
        assert_eq!(t.task.state, TaskState::Building);
        assert_eq!(t.stage("first").unwrap().state, StageState::Building);
        assert_eq!(
            t.stage("first").unwrap().start_sha.as_deref(),
            Some(start_sha.as_str()),
            "the stage diff keeps covering all of the stage's work"
        );
        let prompt = log.lock().unwrap().last().unwrap().clone();
        assert!(prompt.contains("- missing index"), "{prompt}");
        assert!(prompt.contains("also add the covering index"), "{prompt}");
        assert!(prompt.contains(&start_sha), "{prompt}");

        // The fix round closes exactly like a first build: validate → stage board.
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.stage("first").unwrap().state, StageState::Validating);
        orch.on_done(&mut t, done_validate(true, "- fixed", ""))
            .unwrap();
        assert_eq!(t.task.state, TaskState::PlanReview);
        assert_eq!(
            t.stage("first").unwrap().state,
            StageState::Validated { passed: true }
        );
    }

    #[tokio::test]
    async fn resume_routes_an_interrupted_stage_build_to_a_fresh_stage_session() {
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            prompt_recording_agent(log.clone()),
            Templates::default(),
        );
        let mut t = two_stage_task(&orch, "rs1");
        orch.approve_stage(&mut t, "first").unwrap();
        orch.dispatch_stage(&mut t, "first", None).unwrap();

        // Died during the stage build session → build_stage template again.
        let mut revived = interrupted_copy(&t);
        orch.resume(&mut revived).unwrap();
        assert_eq!(revived.task.state, TaskState::Building);
        assert_eq!(revived.stage("first").unwrap().state, StageState::Building);
        let prompt = log.lock().unwrap().last().unwrap().clone();
        assert!(prompt.contains("Execute ONE stage"), "{prompt}");
        assert!(prompt.contains(".build/plan/01-first.md"), "{prompt}");
    }

    #[tokio::test]
    async fn resume_routes_an_interrupted_fix_session_back_to_fix_stage() {
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            prompt_recording_agent(log.clone()),
            Templates::default(),
        );
        let mut t = two_stage_task(&orch, "rs2");
        orch.approve_stage(&mut t, "first").unwrap();
        orch.dispatch_stage(&mut t, "first", None).unwrap();
        std::fs::write(t.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        orch.on_done(&mut t, done_validate(false, "- missing index", ""))
            .unwrap();
        orch.fix_stage(&mut t, "first", "").unwrap();

        // Died during the FIX session: the stage is Building with a failed
        // report stored — recovery must respawn a fix session, not a build one.
        let mut revived = interrupted_copy(&t);
        orch.resume(&mut revived).unwrap();
        assert_eq!(revived.task.state, TaskState::Building);
        let prompt = log.lock().unwrap().last().unwrap().clone();
        assert!(
            prompt.contains("did not pass. Findings:"),
            "fix template respawned: {prompt}"
        );
        assert!(prompt.contains("- missing index"), "{prompt}");
    }

    #[tokio::test]
    async fn resume_routes_an_interrupted_validation_back_to_a_validate_session() {
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            prompt_recording_agent(log.clone()),
            Templates::default(),
        );
        let mut t = two_stage_task(&orch, "rs3");
        orch.approve_stage(&mut t, "first").unwrap();
        orch.dispatch_stage(&mut t, "first", None).unwrap();
        std::fs::write(t.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.stage("first").unwrap().state, StageState::Validating);

        // Died while the validation agent ran → a fresh validate session.
        let mut revived = interrupted_copy(&t);
        orch.resume(&mut revived).unwrap();
        assert_eq!(revived.task.state, TaskState::Building);
        assert_eq!(
            revived.stage("first").unwrap().state,
            StageState::Validating
        );
        let prompt = log.lock().unwrap().last().unwrap().clone();
        assert!(prompt.contains("VALIDATION agent"), "{prompt}");

        // Died between BuildDone and StartValidation (stage persisted as Built):
        // recovery forces it to Validating and validates.
        let mut built = interrupted_copy(&revived);
        built.stage_mut("first").unwrap().state = StageState::Built;
        orch.resume(&mut built).unwrap();
        assert_eq!(built.stage("first").unwrap().state, StageState::Validating);
        let prompt = log.lock().unwrap().last().unwrap().clone();
        assert!(prompt.contains("VALIDATION agent"), "{prompt}");
    }

    #[tokio::test]
    async fn resume_routes_an_interrupted_stage_revision_back_to_revise_stage() {
        use crate::task::CommentState;
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            prompt_recording_agent(log.clone()),
            Templates::default(),
        );
        let mut t = two_stage_task(&orch, "rs4");
        t.comments = vec![comment_on("c-1", "first", CommentState::Open)];
        orch.send_stage_notes(&mut t, "first").unwrap();
        assert_eq!(t.task.state, TaskState::Planning);

        let mut revived = interrupted_copy(&t);
        orch.resume(&mut revived).unwrap();
        assert_eq!(revived.task.state, TaskState::Planning);
        assert_eq!(revived.revising_stage_id.as_deref(), Some("first"));
        let prompt = log.lock().unwrap().last().unwrap().clone();
        assert!(
            prompt.contains("reviewer left comments") && prompt.contains("[c-1]"),
            "revise_stage respawned with the open comments: {prompt}"
        );

        // A multi-stage task interrupted during a full re-plan (no revising
        // stage) resumes the initial plan template instead.
        let mut replanning = interrupted_copy(&revived);
        replanning.revising_stage_id = None;
        orch.resume(&mut replanning).unwrap();
        let prompt = log.lock().unwrap().last().unwrap().clone();
        assert!(prompt.contains("PLAN mode"), "{prompt}");
    }

    #[tokio::test]
    async fn resume_failure_never_strands_the_task_out_of_its_interrupted_state() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "rs5");
        for stage_id in ["first", "second"] {
            orch.approve_stage(&mut t, stage_id).unwrap();
            orch.dispatch_stage(&mut t, stage_id, None).unwrap();
            std::fs::write(t.worktree.path.join(format!("{stage_id}.txt")), "x\n").unwrap();
            orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
                .unwrap();
            orch.on_done(&mut t, done_validate(true, "- ok", ""))
                .unwrap();
        }
        assert_eq!(t.task.state, TaskState::Review);
        // A post-review change request is running when the daemon dies: the
        // current stage is stage-terminal (Validated{passed:true}).
        orch.request_changes(&mut t, "rename the file").unwrap();
        let mut revived = interrupted_copy(&t);
        assert_eq!(
            revived.task.state,
            TaskState::Interrupted(crate::task::Phase::Build)
        );

        // There is no resume route for a validated stage — but the failure must
        // leave the task in its resumable, needs-attention Interrupted state,
        // never half-applied to Building with no session (which nothing could
        // ever demote or resume again).
        let err = orch.resume(&mut revived).unwrap_err();
        assert!(err.to_string().contains("Request Changes"), "{err}");
        assert_eq!(
            revived.task.state,
            TaskState::Interrupted(crate::task::Phase::Build)
        );
        assert!(revived.task.state.needs_attention());

        // The documented escape hatch: re-send the change request, which closes
        // exactly like the legacy loop.
        orch.request_changes(&mut revived, "rename the file")
            .unwrap();
        assert_eq!(revived.task.state, TaskState::Building);
        orch.on_done(
            &mut revived,
            done(DonePhase::Revise, DoneStatus::Completed, None),
        )
        .unwrap();
        assert_eq!(revived.task.state, TaskState::Review);
    }

    #[tokio::test]
    async fn request_changes_is_gated_while_a_stage_awaits_validation() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "rq1");
        orch.approve_stage(&mut t, "first").unwrap();
        orch.dispatch_stage(&mut t, "first", None).unwrap();
        std::fs::write(t.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Build, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(t.stage("first").unwrap().state, StageState::Validating);

        // The task reads "building" so the diff tab accepts comments — but
        // firing them now would kill the validation agent and the replacement
        // session's report would be ignored. Reject without touching anything.
        let err = orch.request_changes(&mut t, "use tabs").unwrap_err();
        assert!(err.to_string().contains("awaiting validation"), "{err}");
        assert_eq!(t.task.state, TaskState::Building);
        assert_eq!(t.stage("first").unwrap().state, StageState::Validating);
        assert!(t.subscribe().is_some(), "validation session not killed");

        // The verdict still lands normally afterwards.
        orch.on_done(&mut t, done_validate(true, "- ok", ""))
            .unwrap();
        assert_eq!(t.task.state, TaskState::PlanReview);
    }

    #[tokio::test]
    async fn request_changes_redirects_a_running_stage_build_through_the_stage_pipeline() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = two_stage_task(&orch, "rq2");
        orch.approve_stage(&mut t, "first").unwrap();
        orch.dispatch_stage(&mut t, "first", None).unwrap();
        assert_eq!(t.stage("first").unwrap().state, StageState::Building);

        // Mid-build redirects stay legal on multi-stage tasks: the replacement
        // session's done still flows through the stage pipeline (commit →
        // validation), never around it.
        orch.request_changes(&mut t, "use tabs").unwrap();
        assert_eq!(t.task.state, TaskState::Building);
        std::fs::write(t.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_done(&mut t, done(DonePhase::Revise, DoneStatus::Completed, None))
            .unwrap();
        assert_eq!(
            t.stage("first").unwrap().state,
            StageState::Validating,
            "redirected stage work still passes the validation gate"
        );
        assert_eq!(t.task.state, TaskState::Building);
    }

    // ---- Multi-stage: ActiveTask bookkeeping ----

    fn stage_named(id: &str, state: crate::task::StageState) -> crate::task::Stage {
        crate::task::Stage {
            id: id.to_string(),
            title: format!("Stage {id}"),
            path: format!(".build/plan/01-{id}.md"),
            summary: String::new(),
            state,
            start_sha: None,
            validation: None,
        }
    }

    fn comment_on(
        id: &str,
        stage_id: &str,
        state: crate::task::CommentState,
    ) -> crate::task::StageComment {
        crate::task::StageComment {
            id: id.to_string(),
            stage_id: stage_id.to_string(),
            anchor: None,
            body: format!("comment {id}"),
            state,
            agent_reply: None,
        }
    }

    /// A detached ActiveTask for pure bookkeeping tests (no worktree on disk).
    fn bare_active(
        stages: Vec<crate::task::Stage>,
        comments: Vec<crate::task::StageComment>,
    ) -> ActiveTask {
        ActiveTask::reattach(
            Task::new(TaskId::new("t"), "goal", TaskKind::Standard),
            Worktree {
                name: "wt".into(),
                path: PathBuf::from("/nonexistent"),
                branch: "build/wt".into(),
                base_branch: "main".into(),
            },
            DEFAULT_PLAN_PATH.into(),
            None,
            Default::default(),
            None,
            stages,
            None,
            None,
            false,
            comments,
            false,
            false,
        )
    }

    #[test]
    fn multi_stage_iff_stages_nonempty() {
        use crate::task::StageState;
        assert!(!bare_active(vec![], vec![]).is_multi_stage());
        assert!(bare_active(vec![stage_named("a", StageState::Planned)], vec![]).is_multi_stage());
    }

    #[test]
    fn stage_lookups_find_by_id_and_name_unknown_ids() {
        use crate::task::StageState;
        let mut active = bare_active(
            vec![
                stage_named("first", StageState::Planned),
                stage_named("second", StageState::Approved),
            ],
            vec![],
        );
        assert_eq!(active.stage("second").unwrap().state, StageState::Approved);
        assert_eq!(active.stage_index("second").unwrap(), 1);
        assert_eq!(active.stage_mut("first").unwrap().id, "first");
        for outcome in [
            active.stage("nope").err(),
            active.stage_index("nope").err(),
            active.stage_mut("nope").err(),
        ] {
            assert_eq!(outcome, Some("unknown stage_id: nope".to_string()));
        }
    }

    #[test]
    fn open_comments_for_filters_by_stage_and_state_in_insertion_order() {
        use crate::task::{CommentState, StageState};
        let active = bare_active(
            vec![stage_named("first", StageState::Planned)],
            vec![
                comment_on("c-1", "first", CommentState::Open),
                comment_on("c-2", "other", CommentState::Open),
                comment_on("c-3", "first", CommentState::Addressed),
                comment_on("c-4", "first", CommentState::Open),
            ],
        );
        let open: Vec<&str> = active
            .open_comments_for("first")
            .iter()
            .map(|c| c.id.as_str())
            .collect();
        assert_eq!(open, vec!["c-1", "c-4"]);
    }

    #[test]
    fn mint_comment_id_never_reuses_a_numeric_suffix() {
        use crate::task::CommentState;
        assert_eq!(bare_active(vec![], vec![]).mint_comment_id(), "c-1");
        let active = bare_active(
            vec![],
            vec![
                comment_on("c-3", "first", CommentState::Open),
                comment_on("c-7", "first", CommentState::Addressed),
                comment_on("garbled", "first", CommentState::Open),
            ],
        );
        assert_eq!(active.mint_comment_id(), "c-8");
    }

    #[test]
    fn reattach_carries_the_stage_bookkeeping() {
        use crate::task::{CommentState, StageState};
        let active = ActiveTask::reattach(
            Task::new(TaskId::new("t"), "goal", TaskKind::Standard),
            Worktree {
                name: "wt".into(),
                path: PathBuf::from("/nonexistent"),
                branch: "build/wt".into(),
                base_branch: "main".into(),
            },
            DEFAULT_PLAN_PATH.into(),
            None,
            Default::default(),
            None,
            vec![stage_named("first", StageState::Building)],
            Some("first".into()),
            Some("first".into()),
            true,
            vec![comment_on("c-1", "first", CommentState::Open)],
            false,
            false,
        );
        assert_eq!(active.stages.len(), 1);
        assert_eq!(active.current_stage_id.as_deref(), Some("first"));
        assert_eq!(active.revising_stage_id.as_deref(), Some("first"));
        assert!(active.auto_advance);
        assert_eq!(active.comments.len(), 1);
    }

    #[tokio::test]
    async fn session_generation_counts_spawns_and_strict_io_needs_a_live_session() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut active = orch
            .dispatch(
                TaskId::new("g1"),
                "track generations",
                TaskKind::Standard,
                "main",
                Default::default(),
            )
            .unwrap();

        // The first spawn is generation 1, and the warm session is subscribable
        // together with its generation.
        assert_eq!(active.session_generation, 1);
        let (generation, _rx) = active
            .subscribe_with_generation()
            .expect("a warm session after dispatch");
        assert_eq!(generation, 1);

        // Strict input reaches the live PTY; resize reports a live session.
        active.write_input_strict(b"hello\r").unwrap();
        let size = PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        };
        assert!(active.resize_session(size).unwrap(), "live session resizes");

        // With the session ended, strict input errors and resize is a not-live
        // no-op — the generation is left where it was.
        active.end_session();
        assert!(active.subscribe_with_generation().is_none());
        assert_eq!(
            active.write_input_strict(b"hello\r").unwrap_err(),
            "no active agent session"
        );
        assert!(!active.resize_session(size).unwrap());
        assert_eq!(active.session_generation, 1);

        // The next spawn (plan approval) is generation 2.
        active.task.apply(TaskEvent::PlanReady).unwrap();
        orch.approve_task_plan(&mut active, None).unwrap();
        assert_eq!(active.session_generation, 2);
        let (generation, _rx) = active
            .subscribe_with_generation()
            .expect("a warm session after approval");
        assert_eq!(generation, 2);
    }

    #[test]
    fn reattach_starts_at_generation_zero() {
        let active = ActiveTask::reattach(
            Task::new(TaskId::new("t"), "goal", TaskKind::Standard),
            Worktree {
                name: "wt".into(),
                path: PathBuf::from("/nonexistent"),
                branch: "build/wt".into(),
                base_branch: "main".into(),
            },
            DEFAULT_PLAN_PATH.into(),
            None,
            Default::default(),
            None,
            vec![],
            None,
            None,
            false,
            vec![],
            false,
            false,
        );
        assert_eq!(active.session_generation, 0);
        assert!(active.subscribe_with_generation().is_none());
    }

    #[tokio::test]
    async fn abandon_succeeds_even_when_worktree_cleanup_cannot_run() {
        // Cleanup is best-effort: a worktree already deleted out from under the
        // bridge must not fail the abandon — the lifecycle verdict still lands.
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let mut t = orch
            .dispatch(
                TaskId::new("a2"),
                "scrap this too",
                TaskKind::Quick,
                "main",
                Default::default(),
            )
            .unwrap();
        // Remove the git worktree registration so prune will error inside abandon.
        orch.worktrees.remove(&t.worktree, true).unwrap();

        orch.abandon(&mut t)
            .expect("abandon never fails on cleanup");
        assert_eq!(t.task.state, TaskState::Abandoned);
    }

    // ---- Plan/Run split seams ----

    use crate::plan::{CommentState as PlanCommentState, PlanState, StageDocState};
    use crate::run::{RunId, RunState, StageProgressState};
    use crate::store::{PersistedPlan, PersistedRun, Store};

    fn split_store(dir: &tempfile::TempDir) -> Store {
        Store::new(dir.path().join("store"))
    }

    fn plan_comment(id: &str, stage_id: &str, state: PlanCommentState) -> PlanStageComment {
        PlanStageComment {
            id: id.to_string(),
            stage_id: stage_id.to_string(),
            anchor: None,
            body: format!("comment {id}"),
            state,
            agent_reply: None,
        }
    }

    /// The path of the plan's live disposable worktree (panics when torn down).
    fn plan_worktree_path(plan: &ActivePlan) -> PathBuf {
        plan.worktree
            .as_ref()
            .expect("plan has a live planning worktree")
            .path
            .clone()
    }

    fn drafting_plan(orch: &Orchestrator, id: &str, goal: &str) -> ActivePlan {
        orch.dispatch_plan(PlanId::new(id), goal, "main", Default::default())
            .unwrap()
    }

    /// Play the plan agent: write a single plan doc and report done, landing
    /// the plan at PlanReview with its docs ingested into the store.
    fn plan_in_review(orch: &Orchestrator, store: &Store, id: &str) -> ActivePlan {
        let mut plan = drafting_plan(orch, id, "Add a greeting");
        let worktree_path = plan_worktree_path(&plan);
        std::fs::write(worktree_path.join(".build/plan.md"), "# Plan v1\n").unwrap();
        orch.on_plan_done(
            &mut plan,
            store,
            done(
                DonePhase::Plan,
                DoneStatus::Completed,
                Some(".build/plan.md"),
            ),
        )
        .unwrap();
        assert_eq!(plan.plan.state, PlanState::PlanReview);
        plan
    }

    fn approved_plan(orch: &Orchestrator, store: &Store, id: &str) -> ActivePlan {
        let mut plan = plan_in_review(orch, store, id);
        orch.approve_plan(&mut plan).unwrap();
        plan
    }

    /// A plan whose agent produced `stage_count` stage docs plus the manifest,
    /// driven to PlanReview (the docs are ingested into the store).
    fn multi_stage_plan_in_review(
        orch: &Orchestrator,
        store: &Store,
        id: &str,
        stage_count: usize,
    ) -> ActivePlan {
        let titles = ["First", "Second", "Third"];
        let mut plan = drafting_plan(orch, id, "Add greetings");
        let plan_dir = plan_worktree_path(&plan).join(".build/plan");
        std::fs::create_dir_all(&plan_dir).unwrap();
        let mut entries = Vec::new();
        for (position, title) in titles.iter().take(stage_count).enumerate() {
            let stage_id = title.to_lowercase();
            std::fs::write(
                plan_dir.join(format!("{:02}-{stage_id}.md", position + 1)),
                format!("# Stage: {title}\n"),
            )
            .unwrap();
            entries.push(manifest_entry(&stage_id, title, position + 1));
        }
        std::fs::write(plan_dir.join("stages.json"), "[]").unwrap();
        orch.on_plan_done(&mut plan, store, done_plan_stages(entries))
            .unwrap();
        assert_eq!(plan.plan.state, PlanState::PlanReview);
        plan
    }

    fn approved_multi_stage_plan(
        orch: &Orchestrator,
        store: &Store,
        id: &str,
        stage_count: usize,
    ) -> ActivePlan {
        let mut plan = multi_stage_plan_in_review(orch, store, id, stage_count);
        orch.approve_plan(&mut plan).unwrap();
        plan
    }

    fn dispatch_planned_run(
        orch: &Orchestrator,
        store: &Store,
        plan: &ActivePlan,
        id: &str,
    ) -> ActiveRun {
        orch.dispatch_run(
            RunId::new(id),
            RunSource::Plan {
                plan,
                has_active_run: false,
            },
            "main",
            Default::default(),
            store,
        )
        .unwrap()
    }

    fn dispatch_quick_run(orch: &Orchestrator, store: &Store, id: &str, goal: &str) -> ActiveRun {
        orch.dispatch_run(
            RunId::new(id),
            RunSource::Quick { goal },
            "main",
            Default::default(),
            store,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn dispatch_plan_creates_a_disposable_worktree_on_the_plan_branch() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);

        let plan = drafting_plan(&orch, "plan-1", "Add a greeting");
        assert_eq!(plan.plan.state, PlanState::Drafting);
        let worktree = plan.worktree.as_ref().expect("disposable worktree");
        assert!(
            worktree.branch.starts_with("plan/"),
            "planning branches live in the plan/ namespace: {}",
            worktree.branch
        );
        assert!(worktree.path.join("README.md").exists());
        assert!(plan.session.subscribe().is_some(), "plan session is warm");

        // The scaffolded MCP config routes `done` reports back to THIS plan.
        let mcp = std::fs::read_to_string(worktree.path.join(".build/mcp.json")).unwrap();
        assert!(mcp.contains("plan-1"), "{mcp}");
    }

    #[tokio::test]
    async fn plan_done_ingests_docs_into_the_store_before_plan_review() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        let plan = plan_in_review(&orch, &store, "plan-1");
        assert_eq!(plan.plan_path, ".build/plan.md");
        assert_eq!(plan.last_summary.as_deref(), Some("summary"));
        assert_eq!(
            store.read_plan_doc("plan-1", ".build/plan.md").as_deref(),
            Some("# Plan v1\n"),
            "the store copy is canonical the moment the gate opens"
        );
    }

    #[tokio::test]
    async fn plan_done_ingest_failure_keeps_the_plan_drafting_with_the_error_surfaced() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = drafting_plan(&orch, "plan-1", "Add a greeting");

        // The agent reports done but wrote NO docs: the ingest is transactional,
        // so the done errors and the plan never advances with unpersisted docs.
        let err = orch
            .on_plan_done(
                &mut plan,
                &store,
                done(
                    DonePhase::Plan,
                    DoneStatus::Completed,
                    Some(".build/plan.md"),
                ),
            )
            .expect_err("ingest failure fails the done");
        assert!(matches!(err, OrchestratorError::Store(_)), "{err}");
        assert_eq!(plan.plan.state, PlanState::Drafting, "no state advance");
        assert!(
            plan.last_error
                .as_deref()
                .is_some_and(|e| e.contains("not persisted")),
            "{:?}",
            plan.last_error
        );
        assert_eq!(store.read_plan_doc("plan-1", ".build/plan.md"), None);
    }

    #[tokio::test]
    async fn plan_blocked_and_failed_reports_park_the_plan() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        let mut blocked = drafting_plan(&orch, "plan-b", "goal b");
        orch.on_plan_done(
            &mut blocked,
            &store,
            done(DonePhase::Plan, DoneStatus::Blocked, None),
        )
        .unwrap();
        assert_eq!(blocked.plan.state, PlanState::Blocked);
        assert_eq!(blocked.last_summary.as_deref(), Some("summary"));

        let mut failed = drafting_plan(&orch, "plan-f", "goal f");
        orch.on_plan_done(
            &mut failed,
            &store,
            done(DonePhase::Plan, DoneStatus::Failed, None),
        )
        .unwrap();
        assert_eq!(failed.plan.state, PlanState::Failed);
    }

    #[tokio::test]
    async fn plan_idle_then_late_done_is_still_honored() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = drafting_plan(&orch, "plan-1", "Add a greeting");

        orch.on_plan_idle(&mut plan).unwrap();
        assert_eq!(plan.plan.state, PlanState::IdleUnreported);

        // Quiescence never decided anything: the late report still lands.
        std::fs::write(
            plan_worktree_path(&plan).join(".build/plan.md"),
            "# Late plan\n",
        )
        .unwrap();
        orch.on_plan_done(
            &mut plan,
            &store,
            done(
                DonePhase::Plan,
                DoneStatus::Completed,
                Some(".build/plan.md"),
            ),
        )
        .unwrap();
        assert_eq!(plan.plan.state, PlanState::PlanReview);
        assert_eq!(
            store.read_plan_doc("plan-1", ".build/plan.md").as_deref(),
            Some("# Late plan\n")
        );
    }

    #[tokio::test]
    async fn build_or_validate_reports_on_a_plan_are_rejected_without_mutation() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = drafting_plan(&orch, "plan-1", "Add a greeting");

        for phase in [DonePhase::Build, DonePhase::Validate] {
            let err = orch
                .on_plan_done(&mut plan, &store, done(phase, DoneStatus::Completed, None))
                .expect_err("plans only accept plan/revise reports");
            assert!(matches!(err, OrchestratorError::Gate(_)), "{err}");
        }
        assert_eq!(plan.plan.state, PlanState::Drafting);
        assert_eq!(plan.last_summary, None, "a rejected report leaves no trace");
    }

    #[tokio::test]
    async fn plan_done_merges_the_manifest_into_stage_docs() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        let mut plan = multi_stage_plan_in_review(&orch, &store, "plan-1", 2);
        assert_eq!(plan.plan_path, templates::STAGES_MANIFEST_PATH);
        let ids: Vec<&str> = plan.stages.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["first", "second"]);
        assert!(plan
            .stages
            .iter()
            .all(|s| s.state == StageDocState::Planned));
        assert_eq!(
            store
                .read_plan_doc("plan-1", ".build/plan/01-first.md")
                .as_deref(),
            Some("# Stage: First\n")
        );

        // A re-plan retitles "second" (doc state kept), drops "first", and
        // appends "third" — the plan side only carries doc review, so a
        // dropped id simply disappears (run progress is never deleted).
        plan.stages[1].state = StageDocState::Approved;
        orch.send_plan_notes(&mut plan, &store, "restructure")
            .unwrap();
        std::fs::write(
            plan_worktree_path(&plan).join(".build/plan/03-third.md"),
            "# Stage: Third\n",
        )
        .unwrap();
        orch.on_plan_done(
            &mut plan,
            &store,
            done_plan_stages(vec![
                manifest_entry("second", "Second v2", 2),
                manifest_entry("third", "Third", 3),
            ]),
        )
        .unwrap();
        let ids: Vec<&str> = plan.stages.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["second", "third"]);
        assert_eq!(plan.stages[0].title, "Second v2");
        assert_eq!(
            plan.stages[0].state,
            StageDocState::Approved,
            "an existing id keeps its review sub-state across a re-plan"
        );
        assert_eq!(plan.stages[1].state, StageDocState::Planned);
    }

    #[tokio::test]
    async fn send_plan_notes_revises_in_the_warm_worktree() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = plan_in_review(&orch, &store, "plan-1");
        let worktree_path = plan_worktree_path(&plan);

        orch.send_plan_notes(&mut plan, &store, "tighten step 2")
            .unwrap();
        assert_eq!(plan.plan.state, PlanState::Drafting);
        assert_eq!(
            plan_worktree_path(&plan),
            worktree_path,
            "the worktree stays warm through the notes loop"
        );
        assert!(plan.session.subscribe().is_some(), "fresh revise session");

        // The revised doc lands in the store on the next done.
        std::fs::write(worktree_path.join(".build/plan.md"), "# Plan v2\n").unwrap();
        orch.on_plan_done(
            &mut plan,
            &store,
            done(
                DonePhase::Plan,
                DoneStatus::Completed,
                Some(".build/plan.md"),
            ),
        )
        .unwrap();
        assert_eq!(plan.plan.state, PlanState::PlanReview);
        assert_eq!(
            store.read_plan_doc("plan-1", ".build/plan.md").as_deref(),
            Some("# Plan v2\n")
        );
    }

    #[tokio::test]
    async fn send_plan_notes_recreates_a_vanished_worktree_from_the_store() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = plan_in_review(&orch, &store, "plan-1");

        // The user deleted the planning worktree out from under the plan. The
        // docs are canonical in the store, so a revision just re-creates one.
        let old_path = plan_worktree_path(&plan);
        std::fs::remove_dir_all(&old_path).unwrap();
        orch.send_plan_notes(&mut plan, &store, "tighten step 2")
            .unwrap();
        assert_eq!(plan.plan.state, PlanState::Drafting);
        let new_path = plan_worktree_path(&plan);
        assert!(new_path.exists());
        assert_eq!(
            std::fs::read_to_string(new_path.join(".build/plan.md")).unwrap(),
            "# Plan v1\n",
            "docs re-materialized from the store before the session"
        );
        assert!(plan.session.subscribe().is_some());
    }

    #[tokio::test]
    async fn send_plan_notes_recreates_a_torn_down_worktree_from_the_store() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = plan_in_review(&orch, &store, "plan-1");

        // Simulate the interrupted arm where the worktree record is gone
        // entirely (e.g. reattached after teardown).
        plan.worktree = None;
        orch.send_plan_notes(&mut plan, &store, "tighten step 2")
            .unwrap();
        assert_eq!(plan.plan.state, PlanState::Drafting);
        let new_path = plan_worktree_path(&plan);
        assert_eq!(
            std::fs::read_to_string(new_path.join(".build/plan.md")).unwrap(),
            "# Plan v1\n"
        );
        // The fresh worktree is fully scaffolded (done reports must route).
        assert!(new_path.join(".build/mcp.json").exists());
    }

    #[tokio::test]
    async fn approve_plan_tears_down_the_worktree_and_branch() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = plan_in_review(&orch, &store, "plan-1");
        let worktree = plan.worktree.clone().unwrap();

        orch.approve_plan(&mut plan).unwrap();
        assert_eq!(plan.plan.state, PlanState::Approved);
        assert_eq!(plan.worktree, None, "the disposable worktree is gone");
        assert!(!worktree.path.exists());
        let r = git2::Repository::open(&repo).unwrap();
        assert!(
            r.find_branch(&worktree.branch, git2::BranchType::Local)
                .is_err(),
            "the plan/ branch is deleted with the worktree"
        );
        assert!(plan.session.subscribe().is_none(), "session ended");
        // The canonical docs survive the teardown.
        assert_eq!(
            store.read_plan_doc("plan-1", ".build/plan.md").as_deref(),
            Some("# Plan v1\n")
        );
    }

    #[tokio::test]
    async fn approve_plan_teardown_failure_is_an_error_not_a_shrug() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = plan_in_review(&orch, &store, "plan-1");

        // Break the teardown: unregister the worktree behind the plan's back.
        let worktree = plan.worktree.clone().unwrap();
        orch.worktrees.remove(&worktree, true).unwrap();

        orch.approve_plan(&mut plan)
            .expect_err("teardown failure must surface");
        assert_eq!(
            plan.plan.state,
            PlanState::PlanReview,
            "the plan stays at its gate so a re-approve retries the teardown"
        );
        assert!(plan.worktree.is_some(), "the worktree record is kept");
    }

    #[tokio::test]
    async fn plan_stage_revision_done_ingests_resolves_comments_and_resets_approval() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = multi_stage_plan_in_review(&orch, &store, "plan-1", 2);
        plan.stages[0].state = StageDocState::Approved;
        plan.comments = vec![
            plan_comment("c-1", "first", PlanCommentState::Open),
            plan_comment("c-2", "first", PlanCommentState::Open),
        ];

        // A stage-revision session is in flight for "first" (the dispatching
        // verb lands with the periphery; the state is arranged directly to
        // isolate the done handling).
        plan.plan.apply(crate::plan::PlanEvent::SendNotes).unwrap();
        plan.revising_stage_id = Some("first".into());
        std::fs::write(
            plan_worktree_path(&plan).join(".build/plan/01-first.md"),
            "# Stage: First (revised)\n",
        )
        .unwrap();

        orch.on_plan_done(
            &mut plan,
            &store,
            DoneReport {
                phase: DonePhase::Revise,
                status: DoneStatus::Completed,
                summary: "revised".into(),
                outputs: DoneOutputs {
                    comment_resolutions: Some(vec![
                        crate::mcp::CommentResolution {
                            comment_id: "c-1".into(),
                            response: "switched to a timestamp".into(),
                        },
                        crate::mcp::CommentResolution {
                            comment_id: "c-999".into(),
                            response: "unknown id is skipped".into(),
                        },
                    ]),
                    ..DoneOutputs::default()
                },
            },
        )
        .unwrap();

        assert_eq!(plan.plan.state, PlanState::PlanReview);
        assert_eq!(
            plan.stages[0].state,
            StageDocState::Planned,
            "a revised doc resets the stale approval"
        );
        assert_eq!(plan.revising_stage_id, None);
        let c1 = plan.comments.iter().find(|c| c.id == "c-1").unwrap();
        assert_eq!(c1.state, PlanCommentState::Addressed);
        assert_eq!(c1.agent_reply.as_deref(), Some("switched to a timestamp"));
        let c2 = plan.comments.iter().find(|c| c.id == "c-2").unwrap();
        assert_eq!(c2.state, PlanCommentState::Open);
        assert_eq!(
            store
                .read_plan_doc("plan-1", ".build/plan/01-first.md")
                .as_deref(),
            Some("# Stage: First (revised)\n"),
            "the revision is ingested into the canonical store copy"
        );
    }

    #[tokio::test]
    async fn stray_revise_report_with_no_revision_in_flight_is_rejected() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = multi_stage_plan_in_review(&orch, &store, "plan-1", 1);

        let err = orch
            .on_plan_done(
                &mut plan,
                &store,
                done(DonePhase::Revise, DoneStatus::Completed, None),
            )
            .expect_err("no stage revision is in flight");
        assert!(matches!(err, OrchestratorError::Gate(_)), "{err}");
        assert_eq!(plan.plan.state, PlanState::PlanReview);
    }

    #[test]
    fn reattach_plan_mirrors_the_store_record() {
        let record = PersistedPlan {
            id: "plan-1".into(),
            goal: "add a greeting".into(),
            project_path: "/home/u/code/proj".into(),
            base_branch: "main".into(),
            state: crate::plan::PlanState::Interrupted,
            worktree_name: Some("add-a-greeting".into()),
            worktree_path: Some("/tmp/wt/add-a-greeting".into()),
            branch: Some("plan/add-a-greeting".into()),
            plan_path: ".build/plan.md".into(),
            stages: vec![],
            comments: vec![plan_comment("c-1", "first", PlanCommentState::Open)],
            model: Some("claude-opus-4-8".into()),
            effort: Some("xhigh".into()),
            last_summary: Some("planned it".into()),
            last_error: Some("boom".into()),
            created_at: "2026-07-01T10:00:00Z".into(),
            updated_at: "2026-07-01T10:05:00Z".into(),
        };
        let active = ActivePlan::reattach(&record);
        assert_eq!(active.plan.id.0, "plan-1");
        assert_eq!(active.plan.state, PlanState::Interrupted);
        let worktree = active.worktree.as_ref().unwrap();
        assert_eq!(worktree.branch, "plan/add-a-greeting");
        assert_eq!(worktree.base_branch, "main");
        assert_eq!(active.base_branch, "main");
        assert_eq!(
            active.model_choice.model.as_deref(),
            Some("claude-opus-4-8")
        );
        assert_eq!(active.comments.len(), 1);
        assert_eq!(active.last_error.as_deref(), Some("boom"));
        assert_eq!(active.session.generation(), 0);
        assert!(active.session.subscribe().is_none());

        // A record whose worktree was torn down reattaches without one.
        let torn_down = PersistedPlan {
            worktree_name: None,
            worktree_path: None,
            branch: None,
            ..record
        };
        assert_eq!(ActivePlan::reattach(&torn_down).worktree, None);
    }

    #[test]
    fn reattach_run_mirrors_the_store_record() {
        let record = PersistedRun {
            id: "run-1".into(),
            plan_id: Some("plan-1".into()),
            goal: "add a greeting".into(),
            project_path: "/home/u/code/proj".into(),
            base_branch: "main".into(),
            state: crate::run::RunState::Interrupted,
            branch: "build/add-a-greeting".into(),
            worktree_name: "add-a-greeting".into(),
            worktree_path: "/tmp/wt/add-a-greeting".into(),
            base_sha: Some("deadbeef".into()),
            stages: vec![crate::run::StageProgress::dispatched("first")],
            current_stage_id: Some("first".into()),
            revising_stage_id: None,
            auto_advance: true,
            adopted: true,
            pending_continuation: true,
            model: None,
            effort: Some("high".into()),
            last_summary: Some("built it".into()),
            last_error: None,
            created_at: "2026-07-01T10:00:00Z".into(),
            updated_at: "2026-07-01T10:05:00Z".into(),
        };
        let active = ActiveRun::reattach(&record, ".build/plan.md".into());
        assert_eq!(active.run.id.0, "run-1");
        assert_eq!(
            active.run.plan_id.as_ref().map(|p| p.0.as_str()),
            Some("plan-1")
        );
        assert_eq!(active.run.state, RunState::Interrupted);
        assert_eq!(active.worktree.branch, "build/add-a-greeting");
        assert_eq!(active.base_sha.as_deref(), Some("deadbeef"));
        assert_eq!(active.plan_path, ".build/plan.md");
        assert_eq!(active.stages.len(), 1);
        assert_eq!(active.current_stage_id.as_deref(), Some("first"));
        assert!(active.auto_advance);
        assert!(active.adopted);
        assert!(active.pending_continuation);
        assert_eq!(active.model_choice.effort.as_deref(), Some("high"));
        assert_eq!(active.session.generation(), 0);
        assert!(active.session.subscribe().is_none());
    }

    #[tokio::test]
    async fn dispatch_quick_run_goes_straight_to_building() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        let mut run = dispatch_quick_run(&orch, &store, "run-1", "fix typo");
        assert_eq!(run.run.state, RunState::Building);
        assert_eq!(run.run.plan_id, None);
        assert_eq!(run.base_sha, None, "quick runs diff from the merge-base");
        assert!(run.worktree.branch.starts_with("build/"));
        assert!(run.worktree.path.join(".build/mcp.json").exists());
        assert!(run.session.subscribe().is_some(), "build session is warm");

        std::fs::write(run.worktree.path.join("fix.txt"), "fixed\n").unwrap();
        orch.on_run_done(
            &mut run,
            &[],
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();
        assert_eq!(run.run.state, RunState::Review);
        let diff = orch.run_diff(&run).unwrap();
        assert!(diff.files().iter().any(|f| f.path == "fix.txt"));
    }

    #[tokio::test]
    async fn dispatch_planned_run_materializes_commits_and_baselines_the_diff() {
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            prompt_recording_agent(log.clone()),
            Templates::default(),
        );
        let store = split_store(&dir);
        let plan = approved_plan(&orch, &store, "plan-1");

        let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
        assert_eq!(run.run.state, RunState::Building);
        assert_eq!(
            run.run.plan_id.as_ref().map(|p| p.0.as_str()),
            Some("plan-1")
        );
        assert_eq!(
            run.run.goal, "Add a greeting",
            "the run inherits the plan's goal"
        );

        // Dispatch order: materialize → commit ("plan: <goal>") → base_sha.
        assert_eq!(
            std::fs::read_to_string(run.worktree.path.join(".build/plan.md")).unwrap(),
            "# Plan v1\n"
        );
        assert_eq!(
            last_commit_subject(&run.worktree.path),
            "plan: Add a greeting"
        );
        let head = worktree_head(&run.worktree.path);
        assert_eq!(run.base_sha.as_deref(), Some(head.as_str()));

        // The materialized docs are the diff baseline — zero review noise…
        assert!(orch.run_diff(&run).unwrap().files().is_empty());
        // …while build-agent work (and any doc edits) still surface.
        std::fs::write(run.worktree.path.join("greeting.txt"), "hello\n").unwrap();
        let diff = orch.run_diff(&run).unwrap();
        let paths: Vec<&str> = diff.files().iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["greeting.txt"]);

        // The build prompt points at the plan's doc.
        let prompt = log.lock().unwrap().last().unwrap().clone();
        assert!(prompt.contains(".build/plan.md"), "{prompt}");
        assert!(prompt.contains("Add a greeting"), "{prompt}");

        orch.on_run_done(
            &mut run,
            &[],
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();
        assert_eq!(run.run.state, RunState::Review);
    }

    #[tokio::test]
    async fn dispatch_run_enforces_the_single_active_writer_rule() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let plan = approved_plan(&orch, &store, "plan-1");

        let err = match orch.dispatch_run(
            RunId::new("run-2"),
            RunSource::Plan {
                plan: &plan,
                has_active_run: true,
            },
            "main",
            Default::default(),
            &store,
        ) {
            Ok(_) => panic!("a second concurrent run of the same plan must be rejected"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("active run"), "{err}");
    }

    #[tokio::test]
    async fn dispatch_run_requires_an_approved_plan() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let plan = plan_in_review(&orch, &store, "plan-1");

        let err = match orch.dispatch_run(
            RunId::new("run-1"),
            RunSource::Plan {
                plan: &plan,
                has_active_run: false,
            },
            "main",
            Default::default(),
            &store,
        ) {
            Ok(_) => panic!("only an approved plan can be implemented"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("approved"), "{err}");
    }

    #[tokio::test]
    async fn dispatch_multi_stage_run_starts_the_first_stage() {
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            prompt_recording_agent(log.clone()),
            Templates::default(),
        );
        let store = split_store(&dir);
        let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);

        let run = dispatch_planned_run(&orch, &store, &plan, "run-1");
        assert_eq!(run.run.state, RunState::Building);
        assert_eq!(run.current_stage_id.as_deref(), Some("first"));
        assert_eq!(run.stages.len(), 1);
        assert_eq!(run.stages[0].state, StageProgressState::Building);
        assert_eq!(
            run.stages[0].start_sha, run.base_sha,
            "the first stage's diff starts at the materialization commit"
        );
        assert!(run.worktree.path.join(".build/plan/01-first.md").exists());
        let prompt = log.lock().unwrap().last().unwrap().clone();
        assert!(prompt.contains("Execute ONE stage"), "{prompt}");
        assert!(prompt.contains(".build/plan/01-first.md"), "{prompt}");
    }

    #[tokio::test]
    async fn run_stage_build_done_commits_and_hands_off_to_validation() {
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            prompt_recording_agent(log.clone()),
            Templates::default(),
        );
        let store = split_store(&dir);
        let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
        let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");

        std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();
        assert_eq!(run.run.state, RunState::Building, "validation is running");
        assert_eq!(run.stages[0].state, StageProgressState::Validating);
        let subject = last_commit_subject(&run.worktree.path);
        assert!(
            subject.contains("stage first"),
            "stage work committed before validation: {subject:?}"
        );
        let prompt = log.lock().unwrap().last().unwrap().clone();
        let start_sha = run.stages[0].start_sha.clone().unwrap();
        assert!(prompt.contains("VALIDATION"), "{prompt}");
        assert!(prompt.contains(&start_sha), "{prompt}");
        assert!(
            prompt.contains(".build/plan/02-second.md"),
            "next stage doc is in the validation prompt: {prompt}"
        );
    }

    #[tokio::test]
    async fn run_validation_pass_mid_plan_parks_at_the_stage_gate() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
        let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
        run.auto_advance = true;
        std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();

        orch.on_run_done(
            &mut run,
            &plan.stages,
            done_validate(true, "- ok", "note for second"),
        )
        .unwrap();
        assert_eq!(run.run.state, RunState::StageGate);
        assert_eq!(
            run.stages[0].state,
            StageProgressState::Validated { passed: true }
        );
        assert_eq!(
            run.stages[0]
                .validation
                .as_ref()
                .map(|v| v.notes_for_next_stage.as_str()),
            Some("note for second")
        );
        assert!(run.auto_advance, "run-all stays armed after a pass");
    }

    #[tokio::test]
    async fn run_validation_pass_on_the_last_stage_opens_review() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 1);
        let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
        std::fs::write(run.worktree.path.join("only.txt"), "one\n").unwrap();
        orch.on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();

        orch.on_run_done(&mut run, &plan.stages, done_validate(true, "- ok", ""))
            .unwrap();
        assert_eq!(run.run.state, RunState::Review, "last stage → merge review");
    }

    #[tokio::test]
    async fn run_validation_failure_parks_at_the_stage_gate_and_disarms_run_all() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
        let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
        run.auto_advance = true;
        std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();

        orch.on_run_done(
            &mut run,
            &plan.stages,
            done_validate(false, "- migration missing", ""),
        )
        .unwrap();
        assert_eq!(run.run.state, RunState::StageGate);
        assert_eq!(
            run.stages[0].state,
            StageProgressState::Validated { passed: false }
        );
        assert_eq!(
            run.stages[0]
                .validation
                .as_ref()
                .map(|v| v.findings.as_str()),
            Some("- migration missing")
        );
        assert!(!run.auto_advance, "a failed validation disarms run-all");
    }

    #[tokio::test]
    async fn stray_run_reports_are_ignored_or_rejected_not_promoted() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        // A quick run must ignore a validate report outright.
        let mut quick = dispatch_quick_run(&orch, &store, "run-q", "quick work");
        orch.on_run_done(&mut quick, &[], done_validate(true, "- ok", ""))
            .unwrap();
        assert_eq!(quick.run.state, RunState::Building, "ignored");

        // A run session misusing phase=plan is rejected: plan reports belong
        // to plans, and consuming one here would smuggle manifest edits.
        let err = orch
            .on_run_done(
                &mut quick,
                &[],
                done(DonePhase::Plan, DoneStatus::Completed, None),
            )
            .expect_err("plan reports belong to plans");
        assert!(matches!(err, OrchestratorError::Gate(_)), "{err}");
        assert_eq!(quick.run.state, RunState::Building);

        // A multi-stage run mid-validation must ignore a stray build report —
        // otherwise a rogue done(build) would skip the validation gate.
        let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
        let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
        std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();
        assert_eq!(run.stages[0].state, StageProgressState::Validating);
        orch.on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();
        assert_eq!(run.run.state, RunState::Building);
        assert_eq!(run.stages[0].state, StageProgressState::Validating);
    }

    #[tokio::test]
    async fn run_blocked_then_late_reports_are_rejected_without_mutation() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
        let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
        run.auto_advance = true;

        orch.on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Blocked, None),
        )
        .unwrap();
        assert_eq!(run.run.state, RunState::Blocked);
        assert_eq!(run.stages[0].state, StageProgressState::Building);
        assert!(!run.auto_advance, "blocked disarms run-all");

        // A completed build report while Blocked must be rejected atomically:
        // no stage advance, no commit, no session swap.
        let before = last_commit_subject(&run.worktree.path);
        std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
        let err = orch
            .on_run_done(
                &mut run,
                &plan.stages,
                done(DonePhase::Build, DoneStatus::Completed, None),
            )
            .expect_err("late completion while blocked is rejected");
        assert!(matches!(err, OrchestratorError::RunTransition(_)), "{err}");
        assert_eq!(run.run.state, RunState::Blocked);
        assert_eq!(run.stages[0].state, StageProgressState::Building);
        assert_eq!(last_commit_subject(&run.worktree.path), before);
        assert!(
            run.session.subscribe().is_some(),
            "session kept for the reply"
        );
    }

    #[tokio::test]
    async fn run_idle_then_late_done_is_still_honored() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut run = dispatch_quick_run(&orch, &store, "run-1", "quick work");

        orch.on_run_idle(&mut run).unwrap();
        assert_eq!(run.run.state, RunState::IdleUnreported);
        orch.on_run_done(
            &mut run,
            &[],
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();
        assert_eq!(run.run.state, RunState::Review);
    }
}
