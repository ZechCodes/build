//! The task-spine: where lifecycle, worktrees, PTY sessions, `done` reports, and
//! the diff come together.
//!
//! The orchestrator owns project-level configuration (the repo, where worktrees
//! go, the harness adapter, the prompt templates) and drives one [`ActiveTask`]
//! at a time through the lifecycle. The caller owns each `ActiveTask` and hands it
//! back by `&mut` for each transition, so the orchestrator never hides state.
//!
//! The two pipes from the scope are both here: Build → agent is `write_prompt`
//! into the warm PTY; agent → Build is [`on_done`](Orchestrator::on_done), the
//! typed event the MCP server forwards.

use std::path::{Path, PathBuf};
use std::process::Command;

use portable_pty::PtySize;

use crate::diff::{diff_against_base, DiffError, WorktreeDiff};
use crate::mcp::{DonePhase, DoneReport, DoneStatus};
use crate::models::ModelChoice;
use crate::pty::{HarnessSpec, PtyError, PtySession};
use crate::task::{
    stage_transition, CommentState, IllegalStageTransition, IllegalTransition, Phase, Stage,
    StageComment, StageEvent, StageManifestEntry, StageState, Task, TaskEvent, TaskId, TaskKind,
    TaskState,
};
use crate::templates::{self, Templates, Vars, DEFAULT_PLAN_PATH};
use crate::worktree::{
    derive_adoption_goal, slugify, ExternalWorktree, Worktree, WorktreeError, WorktreeManager,
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

    /// Write raw bytes (attached-terminal keystrokes) to the warm session.
    pub fn write_input(&self, bytes: &[u8]) -> Result<(), OrchestratorError> {
        if let Some(session) = &self.session {
            session.write_input(bytes)?;
        }
        Ok(())
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

/// Owns project configuration and drives tasks through the lifecycle.
pub struct Orchestrator {
    repo_path: PathBuf,
    worktrees: WorktreeManager,
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
        let worktrees = WorktreeManager::new(repo_path.clone(), worktrees_root);
        Orchestrator {
            repo_path,
            worktrees,
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
        self.scaffold_build_dir(&worktree, &id)?;

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

        self.commit_all_with_message(&external.path, "Checkpoint: adopted by Build")?;

        let worktree = Worktree {
            name: external.name.clone(),
            path: external.path.clone(),
            branch: branch.clone(),
            base_branch: base_branch.to_string(),
        };
        self.scaffold_build_dir(&worktree, &id)?;

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
        let commit_goal = format!("{} — stage {stage_id}", active.task.goal);
        self.commit_all(&active.worktree.path, &commit_goal)?;
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

    /// Approve the plan and start the build in a **fresh** session — if a cold
    /// agent can't execute the plan, the plan wasn't done.
    pub fn approve_plan(
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

    /// Reply to a blocked/failed/idle card with a human follow-up; resume the phase.
    pub fn reply(&self, active: &mut ActiveTask, message: &str) -> Result<(), OrchestratorError> {
        active.task.apply(TaskEvent::Reply)?;
        active.last_error = None;
        self.prompt_warm_session(active, message)?;
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
            &["push", "-u", "origin", &active.worktree.branch],
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
        active.session = Some(session);
        Ok(())
    }

    fn prompt_warm_session(
        &self,
        active: &ActiveTask,
        prompt: &str,
    ) -> Result<(), OrchestratorError> {
        if let Some(session) = &active.session {
            session.write_prompt(prompt)?;
        }
        Ok(())
    }

    fn end_session(&self, active: &mut ActiveTask) {
        // Kill AND reap: kill alone leaves a zombie per phase transition, which over
        // a long-lived daemon exhausts the process table.
        active.end_session();
    }

    /// Write the per-task MCP config under `.build/` so it never trips plan-scope
    /// enforcement, pointing the harness at this task's `done` server.
    fn scaffold_build_dir(
        &self,
        worktree: &Worktree,
        id: &TaskId,
    ) -> Result<(), OrchestratorError> {
        let build_dir = worktree.path.join(".build");
        std::fs::create_dir_all(&build_dir)?;
        // Absolute path to this binary so the harness can spawn it regardless of PATH.
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.to_str().map(String::from))
            .unwrap_or_else(|| "build-bridge".to_string());
        let mcp = serde_json::json!({
            "mcpServers": {
                "build": {
                    "command": exe,
                    "args": ["mcp", "--task", id.0]
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
        self.git(worktree_path, &["add", "-A"])?;
        // Only commit if something is staged.
        let status = self.git(worktree_path, &["status", "--porcelain"])?;
        if !status.trim().is_empty() {
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
        if let Err(merge_error) = self.git(&self.repo_path, &["merge", "--no-edit", branch]) {
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
            return Err(OrchestratorError::Git(format!(
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
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
        orch.approve_plan(&mut t, Some(build_choice.clone()))
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
        orch.approve_plan(&mut t, None).unwrap();
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

        orch.reply(&mut t, "use the staging credentials").unwrap();
        assert_eq!(t.task.state, TaskState::Building);
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
        orch.approve_plan(&mut t, None).unwrap();
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
}
