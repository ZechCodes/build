//! The task-spine: where lifecycle, worktrees, PTY sessions, `done` reports, and
//! the diff come together — now split across the two entities of the plan/run
//! model.
//!
//! The orchestrator owns project-level configuration (the repo, where worktrees
//! go, the harness adapter, the prompt templates) and drives the split's two
//! entities through their lifecycles:
//!
//! - An [`ActivePlan`] (project-scoped) is authored in a disposable `plan/<slug>`
//!   worktree; its canonical docs live in the store. Its seams: `dispatch_plan`,
//!   `on_plan_done`, the plan-review gates (`approve_plan`, `send_plan_notes`,
//!   the per-stage `approve_plan_stage` / `send_plan_stage_notes`), and the
//!   interaction verbs (`message_plan` / `resume_plan` / `abandon_plan`).
//! - An [`ActiveRun`] (worktree-scoped) is one implementation attempt on a
//!   `build/<slug>` branch. Its seams: `dispatch_run`, `on_run_done` (build +
//!   validation, plus the sequential stage gate `dispatch_run_stage` /
//!   `fix_run_stage`), `run_diff`, the interaction verbs (`message_run` /
//!   `resume_run` / `run_request_changes`), the git finishers
//!   (`run_approve_merge` / `run_commit` / `run_push` / `run_merge_and_push`),
//!   `abandon_run`, and `adopt_run` (a run minted around a pre-existing
//!   worktree, `plan_id` `None` — the only plan-less runs left).
//!
//! The caller owns each active entity and hands it back by `&mut` for each
//! transition, so the orchestrator never hides state. The cross-entity seams —
//! the sequential stage gate (`dispatch_run_stage` consults the plan's
//! stage-doc states) and the mid-run stage-doc write-back
//! (`send_run_stage_notes` / `consume_run_stage_revision`) — take the other
//! entity's read-only view or `&mut` handle as a parameter rather than reaching
//! into any app-level map.
//!
//! The two pipes from the scope are both here: Build → agent is `write_prompt`
//! into the warm PTY; agent → Build is [`on_plan_done`](Orchestrator::on_plan_done)
//! / [`on_run_done`](Orchestrator::on_run_done), the typed events the MCP server
//! forwards (the caller routes each report by owner lookup).

use std::path::{Path, PathBuf};
use std::process::Command;

use portable_pty::PtySize;

use crate::diff::{diff_against_base, diff_against_merge_base, DiffError, WorktreeDiff};
use crate::mcp::{DonePhase, DoneReport, DoneStatus};
use crate::models::{AgentProvider, ModelChoice};
use crate::plan::{
    plan_transition, stage_doc_transition, CommentState as PlanCommentState, IllegalPlanTransition,
    IllegalStageDocTransition, Plan, PlanEvent, PlanId, PlanState,
    StageComment as PlanStageComment, StageDoc, StageDocEvent, StageDocState, StageManifestEntry,
};
use crate::pty::{HarnessSpec, PtyError, PtySession};
use crate::run::{
    run_transition, IllegalRunTransition, IllegalStageProgressTransition, Run, RunEvent, RunId,
    RunState, StageProgress, StageProgressEvent, StageProgressState,
};
use crate::store::{PersistedPlan, PersistedRun, Store, StoreError};
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

/// Merge a fresh manifest echo into the plan's stage docs by id: an id that
/// already exists keeps its review sub-state (`Planned`/`Approved`) and takes
/// the new `title`/`path`/`summary`; new ids append as `Planned`; ids missing
/// from the echo are dropped. Run-side execution progress lives on the run and
/// is never deleted by a re-plan, so the plan side only carries doc review and
/// can drop freely. On the first plan the merge is trivially "all new".
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

    /// Submit a prompt turn to the warm session, honoring the harness's own
    /// submit key and paste framing; a dead session swallows it (the message
    /// stays durable and the next session's catch-up carries it).
    ///
    /// Distinct from [`write_input`](Self::write_input) on purpose: raw input is
    /// the user's keystrokes, where Build must not decide what a turn is. Build
    /// speaking to a live agent is a turn, and it must travel the same way a
    /// dispatched prompt does — otherwise the two paths drift and one of them
    /// submits a multi-line body as N fragmented turns.
    pub fn write_prompt(&self, prompt: &str) -> Result<(), OrchestratorError> {
        if let Some(session) = &self.session {
            session.write_prompt(prompt)?;
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
    /// The durable review conversation. Build itself is the stable logical
    /// owner; individual harness processes are recorded in `sessions`.
    pub thread: crate::thread::Thread,
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
        let mut thread = record.thread.clone();
        thread.normalize(&record.id);
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
                provider: record.provider,
                model: record.model.clone(),
                effort: record.effort.clone(),
            },
            thread,
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
/// and the warm session. An adopted run is one whose `run.plan_id` is `None`.
pub struct ActiveRun {
    pub run: Run,
    pub worktree: Worktree,
    /// The "plan: <goal>" materialization commit recorded at dispatch — the
    /// baseline of the run's review diff, keeping the materialized docs out of
    /// review noise. `None` for adopted/migrated runs (the diff falls
    /// back to the merge-base).
    pub base_sha: Option<String>,
    /// Worktree-relative path the build prompts point at: the owning plan's
    /// `plan_path`, or the convention default for adopted runs. In-memory only —
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
    /// Warm-session continuation flag: set at adoption, consumed by the first
    /// session spawn afterwards.
    pub pending_continuation: bool,
    /// Which model/effort this run's agents run on (None = harness default).
    pub model_choice: ModelChoice,
    /// The durable conversation paired with the evolving review diff.
    pub thread: crate::thread::Thread,
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
    /// re-derived by the caller from the owning plan's record (adopted runs pass
    /// the convention default). The caller (boot recovery) moves a working
    /// state to `Interrupted` itself.
    pub fn reattach(record: &PersistedRun, plan_path: String) -> Self {
        let mut thread = record.thread.clone();
        thread.normalize(&record.id);
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
                provider: record.provider,
                model: record.model.clone(),
                effort: record.effort.clone(),
            },
            thread,
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

/// What a run implements: an approved plan, whose docs are materialized from the
/// store into the run's worktree. Every dispatched run has one — an unplanned
/// coding session is an agent terminal the human drives, not a run.
pub struct RunSource<'a> {
    pub plan: &'a ActivePlan,
    /// The caller's active-runs view (the orchestrator holds no app-level maps):
    /// `true` when the plan already has a non-terminal run, which rejects the
    /// dispatch — the single-active-writer rule.
    pub has_active_run: bool,
}

/// Per-spawn context an interactive harness builder may honor.
#[derive(Debug, Clone, Default)]
pub struct SpawnOptions {
    /// Resume the harness's own most-recent conversation for this cwd
    /// (claude: `--continue`). Set only for the first session after adoption.
    pub continue_session: bool,
    /// Entity whose per-session MCP server receives the terminal `done` report.
    pub owner_id: String,
    /// The worktree the harness will run in. Providers gate an interactive
    /// session behind a workspace-trust dialog for a directory they have not
    /// seen before, and Build mints a fresh worktree per run — so the adapter
    /// needs the path to pre-trust it, or the dialog eats the injected prompt.
    pub cwd: PathBuf,
}

/// Builds an interactive harness command for a rendered prompt + model + context.
///
/// The prompt is supplied so test and custom adapters can inspect the turn being
/// dispatched, but it is always submitted through the spawned PTY by
/// [`spawn_into_slot`](Orchestrator::spawn_into_slot), never baked into argv.
pub type WarmBuilder =
    std::sync::Arc<dyn Fn(&str, &ModelChoice, &SpawnOptions) -> HarnessSpec + Send + Sync>;

/// Whether the harness has an existing conversation transcript for a worktree
/// cwd. Injectable so tests never touch the real home directory.
pub type TranscriptProbe = std::sync::Arc<dyn Fn(&Path, AgentProvider) -> bool + Send + Sync>;

const THREAD_NOTIFICATION: &str = "New reviewer messages are available. Call `read_unread_messages` now and act on every unread message.";

/// How long a failed prompt write waits for the harness's exit status to
/// become reapable before the failure is treated as fatal. Long enough to
/// cover the kernel's close-fds-then-reap lag for a harness that exited
/// under the write; short enough that a genuinely wedged PTY still surfaces
/// its write error promptly.
const PROMPT_WRITE_EXIT_GRACE: std::time::Duration = std::time::Duration::from_millis(250);

/// How long a fresh spawn waits for the harness's first output before writing
/// the prompt into its PTY. Real harnesses are interactive TUIs: injecting the
/// prompt before the TUI has started servicing the PTY risks it landing on a
/// startup screen. First output is the readiness signal; when the grace
/// expires the prompt is written anyway — a spawn that silently never delivers
/// its prompt is worse than one that races the startup screen.
/// Upper bound on waiting for a harness to become ready. Must comfortably
/// exceed a real TUI's full startup — claude 2.1.219 settles at ~1.8s, plus its
/// declared settle window — or the wait expires and the prompt is written into
/// a still-painting screen, which is the failure it exists to prevent.
pub(crate) const HARNESS_READY_GRACE: std::time::Duration = std::time::Duration::from_millis(6000);

fn conversation_prompt(prompt: &str, thread: &crate::thread::Thread) -> String {
    let mut out = String::with_capacity(prompt.len() + 2048);
    out.push_str(prompt);
    // This block is the canonical reply policy. The `post_thread_message` tool
    // description in mcp.rs and NEW_THREAD_MESSAGES_PROMPT in app.rs defer to
    // it by reference — never restate these bullets elsewhere, restated copies
    // drift. The ambiguity rule stays above the silent-directive allowance so
    // an in-order reader hits the carve-out before committing to silence.
    out.push_str(
        "\n\nBuild conversation protocol:\n\
         - When Build says new reviewer messages are available, call `read_unread_messages`.\n\
         - If a reviewer message reads as either a question or a directive, post a one-line clarifying reply via `post_thread_message` instead of silently changing code.\n\
         - You may implement an unambiguous directive without replying; the next revision is its acknowledgment.\n\
         - Call `post_thread_message` only for a question, necessary pushback or clarification, or an explicit request for a response.\n\
         - Do not post acknowledgments or diff recaps.\n",
    );
    let catch_up = thread.catch_up_markdown(40);
    if !catch_up.is_empty() {
        out.push_str("\nCatch-up packet from the durable conversation (oldest to newest):\n");
        if catch_up.len() <= 12_000 {
            out.push_str(&catch_up);
        } else {
            let mut boundary = catch_up.len() - 12_000;
            while !catch_up.is_char_boundary(boundary) {
                boundary += 1;
            }
            out.push_str(&catch_up[boundary..]);
        }
        out.push('\n');
    }
    if let Some(report) = &thread.last_completion {
        out.push_str("\nPrevious structured completion report:\n");
        out.push_str(&serde_json::to_string(report).unwrap_or_default());
        out.push('\n');
    }
    out
}

/// How the orchestrator launches an agent for a phase.
#[derive(Clone)]
pub enum Agent {
    /// A fixed warm interactive session: spawn the binary, then write the prompt
    /// to its PTY.
    Warm(HarnessSpec),
    /// A provider/model-aware warm interactive session. The builder supplies
    /// argv and environment; Build still injects the prompt through the PTY.
    WarmBuilder(WarmBuilder),
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
            transcript_probe: std::sync::Arc::new(|_, _| false),
        }
    }

    /// Opt in to harness-conversation continuation for adopted tasks: `probe`
    /// answers whether a transcript exists for a worktree cwd.
    pub fn with_transcript_probe(mut self, probe: TranscriptProbe) -> Self {
        self.transcript_probe = probe;
        self
    }

    /// The grid an agent PTY is spawned at (40 × 120). Attaching clients resize
    /// it to their own viewport; this is what it paints into until one does.
    pub fn pty_size(&self) -> PtySize {
        self.pty_size
    }

    /// The harness command for an agent tab rooted at `cwd` and owned by
    /// `owner_id`. The MCP socket lives inside the [`Agent::WarmBuilder`]
    /// closure, so this is the only way the app layer can build a spec that
    /// reaches Build's `done` / `read_unread_messages` server.
    ///
    /// The builder is handed an empty prompt on purpose: the prompt is never
    /// baked into argv — every turn travels through the PTY.
    pub fn agent_harness_spec(
        &self,
        owner_id: &str,
        cwd: &Path,
        model_choice: &ModelChoice,
        continue_session: bool,
    ) -> HarnessSpec {
        let options = SpawnOptions {
            continue_session,
            owner_id: owner_id.to_string(),
            cwd: cwd.to_path_buf(),
        };
        match &self.agent {
            // A fixed warm harness (the QA agent) is provider-unaware: it takes
            // its prompt over the PTY and needs no SpawnOptions.
            Agent::Warm(spec) => spec.clone(),
            Agent::WarmBuilder(build) => build("", model_choice, &options),
        }
    }

    /// Write `.build/mcp.json` + `.build/.gitignore` into a worktree that is
    /// about to host an agent. Idempotent, and required before every spawn:
    /// under `--strict-mcp-config` claude exits before reading a byte of the
    /// prompt when the config is missing, so a worktree that never hosted a run
    /// (or whose `.build/` was deleted) would open a tab that paints nothing.
    pub fn scaffold_agent_worktree(
        &self,
        worktree_path: &Path,
        owner_id: &str,
    ) -> Result<(), OrchestratorError> {
        self.write_build_dir(worktree_path, owner_id)
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

        let mut thread = crate::thread::Thread::new(&plan.id.0);
        // The goal is the first turn in the durable conversation. Mark it seen:
        // dispatching the planning session is the agent acting on that prompt.
        let now = crate::store::now_rfc3339();
        thread.post_user(plan.goal.clone(), None, &now);
        let _ = thread.read_unread(&now);
        let mut active = ActivePlan {
            plan,
            worktree: Some(worktree),
            base_branch: base_branch.to_string(),
            plan_path: DEFAULT_PLAN_PATH.to_string(),
            stages: Vec::new(),
            comments: Vec::new(),
            revising_stage_id: None,
            model_choice,
            thread,
            last_summary: None,
            last_error: None,
            session: SessionSlot::default(),
        };
        let prompt = self.render_plan(&self.templates.plan, &active, "");
        self.spawn_plan_session(&mut active, &prompt, "plan")?;
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
        // Stage docs deliberately do NOT gate the coarse approve: per-stage
        // review is progressive (later docs keep getting approved/revised
        // while an earlier stage builds); the dispatch seams re-gate each doc
        // at the moment its build session would spawn.
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
        self.spawn_plan_session(active, &prompt, "revise")?;
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

    /// Approve one stage's doc: `Planned` → `Approved`. Pure bookkeeping, no
    /// session — the plan-side successor of the fused `approve_stage`. Legal on
    /// any non-terminal plan (an `Approved` plan keeps taking per-stage
    /// approvals: that is how a run's later stages get their gate opened while
    /// an earlier one is already building).
    pub fn approve_plan_stage(
        &self,
        active: &mut ActivePlan,
        stage_id: &str,
    ) -> Result<(), OrchestratorError> {
        if active.plan.state.is_terminal() {
            return Err(OrchestratorError::Gate(format!(
                "cannot approve a stage on a terminal plan (state {:?})",
                active.plan.state
            )));
        }
        let index = active
            .stage_doc_index(stage_id)
            .map_err(OrchestratorError::Gate)?;
        active.stages[index].state =
            stage_doc_transition(&active.stages[index].state, StageDocEvent::Approve)?;
        Ok(())
    }

    /// Send a stage's open comments to a fresh plan-revision session (the
    /// per-stage successor of `send_plan_notes`): the persisted open comments
    /// ARE the payload, rendered server-side. The plan re-plans against them in
    /// its disposable worktree — kept warm through the loop, or re-created with
    /// the canonical docs materialized when it was torn down/vanished — and
    /// `revising_stage_id` routes the resulting `done(revise)` through
    /// [`consume_plan_stage_revision`](Self::consume_plan_stage_revision).
    pub fn send_plan_stage_notes(
        &self,
        active: &mut ActivePlan,
        store: &Store,
        stage_id: &str,
    ) -> Result<(), OrchestratorError> {
        let index = active
            .stage_doc_index(stage_id)
            .map_err(OrchestratorError::Gate)?;
        // Pure legality first — nothing is spawned for an illegal revise.
        plan_transition(&active.plan.state, PlanEvent::SendNotes)
            .map_err(|e| OrchestratorError::Gate(format!("cannot send stage notes: {e}")))?;
        let open: Vec<PlanStageComment> = active
            .open_comments_for(stage_id)
            .into_iter()
            .cloned()
            .collect();
        if open.is_empty() {
            return Err(OrchestratorError::Gate(format!(
                "no open comments on stage {stage_id}"
            )));
        }
        self.ensure_planning_worktree(active, store)?;
        active.plan.apply(PlanEvent::SendNotes)?;
        active.revising_stage_id = Some(stage_id.to_string());
        active.last_error = None;
        active.session.end();
        let prompt = self.render_plan_stage(
            &self.templates.revise_stage,
            active,
            index,
            THREAD_NOTIFICATION,
        );
        self.spawn_plan_session(active, &prompt, "revise")?;
        Ok(())
    }

    /// A freeform human message to the plan's agent (the plan-side `message`).
    /// A live `Drafting` session is redirected; parked states (blocked / failed
    /// / idle / interrupted) resume drafting with the message as the steer. The
    /// review gate is refused — `PlanReview` has the structured send-notes verb,
    /// and a side channel there would bypass the batched-review contract. Plans
    /// always start cold (no harness continuation), so the message is always
    /// wrapped in the message template with full plan context.
    pub fn message_plan(
        &self,
        active: &mut ActivePlan,
        store: &Store,
        message: &str,
    ) -> Result<(), OrchestratorError> {
        if message.trim().is_empty() {
            return Err(OrchestratorError::Gate("message must not be empty".into()));
        }
        use crate::plan::PlanState as S;
        let event = match active.plan.state {
            S::Drafting => None,
            S::Blocked | S::Failed | S::IdleUnreported | S::Interrupted => Some(PlanEvent::Reply),
            S::PlanReview => {
                return Err(OrchestratorError::Gate(
                    "the plan is at the review gate — use send notes there".into(),
                ))
            }
            S::Created | S::Approved | S::Abandoned => {
                return Err(OrchestratorError::Gate(
                    "no plan agent session to message".into(),
                ))
            }
        };
        // Pure legality first — the caller persists the plan even on Err.
        if let Some(event) = event {
            plan_transition(&active.plan.state, event)?;
        }
        // An interrupted plan lost its worktree; re-create it (docs
        // materialized) before the session can run.
        self.ensure_planning_worktree(active, store)?;
        let prompt = self.render_plan(&self.templates.message, active, message);
        if let Some(event) = event {
            active.plan.apply(event)?;
        }
        active.last_error = None;
        active.session.end();
        self.spawn_plan_session(active, &prompt, "message")?;
        Ok(())
    }

    /// Re-dispatch an interrupted plan phase in a fresh session (the plan-side
    /// `resume`). The plan machine has a single working phase, so the only
    /// routing is whether a per-stage revision was in flight
    /// (`revising_stage_id` → `revise_stage` with the stage's open comments) or
    /// a full (re-)plan. The prompt is routed BEFORE the `Reply` transition
    /// commits, so a routing failure never strands the plan out of its
    /// interrupted state (the caller persists it even on Err).
    pub fn resume_plan(
        &self,
        active: &mut ActivePlan,
        store: &Store,
    ) -> Result<(), OrchestratorError> {
        plan_transition(&active.plan.state, PlanEvent::Reply)?;
        self.ensure_planning_worktree(active, store)?;
        let prompt = match active.revising_stage_id.clone() {
            Some(stage_id) => {
                let index = active
                    .stage_doc_index(&stage_id)
                    .map_err(OrchestratorError::Gate)?;
                self.render_plan_stage(
                    &self.templates.revise_stage,
                    active,
                    index,
                    THREAD_NOTIFICATION,
                )
            }
            None => self.render_plan(&self.templates.plan, active, ""),
        };
        active.plan.apply(PlanEvent::Reply)?;
        active.last_error = None;
        active.session.end();
        self.spawn_plan_session(active, &prompt, "revise")?;
        Ok(())
    }

    /// Abandon a plan from any non-terminal state: kill the plan agent, mark the
    /// plan `Abandoned`, and tear down its disposable planning worktree (branch
    /// included). Teardown is best-effort — a leftover worktree is logged, never
    /// a reason to fail the abandon; the store docs are canonical and survive
    /// either way.
    pub fn abandon_plan(&self, active: &mut ActivePlan) -> Result<(), OrchestratorError> {
        active.plan.apply(PlanEvent::Abandon)?;
        active.session.end();
        if let Some(worktree) = &active.worktree {
            if let Err(cleanup) = self.worktrees.remove(worktree, /* keep_branch */ false) {
                eprintln!(
                    "abandon plan {}: worktree/branch cleanup failed: {cleanup}",
                    active.plan.id.0
                );
            }
        }
        active.worktree = None;
        Ok(())
    }

    /// Render a stage-scoped template for a plan (the plan-side twin of
    /// [`render_run_stage`](Self::render_run_stage)): the stage doc's own fields
    /// plus the next stage's doc path. The run-side variables (start sha,
    /// validation findings, prior-stage notes) are all empty — they belong to a
    /// run's execution progress, not a plan's doc review.
    fn render_plan_stage(
        &self,
        template: &str,
        active: &ActivePlan,
        index: usize,
        comments: &str,
    ) -> String {
        let doc = &active.stages[index];
        let next_stage_path = active
            .stages
            .get(index + 1)
            .map(|next| next.path.as_str())
            .unwrap_or("");
        templates::render(
            template,
            &Vars {
                goal: &active.plan.goal,
                plan_path: &active.plan_path,
                comments,
                base_branch: &active.base_branch,
                stage_id: &doc.id,
                stage_title: &doc.title,
                stage_path: &doc.path,
                stage_summary: &doc.summary,
                next_stage_path,
                stage_start_sha: "",
                findings: "",
                prior_notes: "",
            },
        )
    }

    // ---- Run seams (Plan/Run split) ----------------------------------------

    /// Create a worktree with nothing attached to it — no `.build/` scaffold, no
    /// run record, no session. The human works in it by hand (a terminal or an
    /// agent tab); Build only owns the directory and the branch it cut. It sits
    /// on the same `build/<slug>` naming as run worktrees so teardown, adoption
    /// and the scan all treat it identically.
    pub fn create_bare_worktree(
        &self,
        slug: &str,
        base_branch: &str,
    ) -> Result<Worktree, OrchestratorError> {
        Ok(self.worktrees.create(slug, base_branch)?)
    }

    /// Dispatch a run: create the `build/<slug>` worktree, scaffold `.build/`
    /// (the MCP config carries the run id), and spawn the first build session.
    ///
    /// The run materializes the plan's canonical docs from the store into the
    /// fresh worktree and commits them ("plan: <goal>" — the intent record the
    /// scope doc keeps through merge); that commit is recorded as the run's
    /// `base_sha`, the baseline of the review diff, so the materialized docs
    /// never show up as review noise.
    ///
    /// Single-active-writer: at most one active run per plan. The caller owns
    /// the runs map, so it passes its view via [`RunSource::has_active_run`];
    /// `true` rejects the dispatch before anything is created.
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
        let RunSource {
            plan: plan_link,
            has_active_run,
        } = source;
        if plan_link.plan.state != PlanState::Approved {
            return Err(OrchestratorError::Gate(format!(
                "only an approved plan can be implemented (plan {} is {:?})",
                plan_link.plan.id.0, plan_link.plan.state
            )));
        }
        if has_active_run {
            return Err(OrchestratorError::Gate(format!(
                "plan {} already has an active run — a second concurrent run is \
                 rejected (single-active-writer)",
                plan_link.plan.id.0
            )));
        }
        // Dispatch spawns the first stage's build session immediately, so its doc
        // must carry a live approval. `approve_plan` already guarantees this for
        // natively approved plans; migrated plans (and revision-staled docs on a
        // re-run) are re-gated here.
        if let Some(first_stage) = plan_link.stages.first() {
            if first_stage.state != StageDocState::Approved {
                return Err(OrchestratorError::Gate(format!(
                    "cannot implement plan {}: stage {:?} is not approved",
                    plan_link.plan.id.0, first_stage.id
                )));
            }
        }
        let goal = plan_link.plan.goal.clone();

        let slug = slugify(&goal);
        let worktree = self.worktrees.create(&slug, base_branch)?;
        self.scaffold_build_dir(&worktree, &id.0)?;
        let base_sha =
            match self.materialize_and_commit_plan_docs(plan_link, &worktree, &goal, store) {
                Ok(sha) => Some(sha),
                Err(error) => {
                    // Nothing has been handed to the caller; don't leak the
                    // half-prepared worktree.
                    self.discard_worktree(&worktree);
                    return Err(error);
                }
            };

        let mut run = Run::new(id, Some(plan_link.plan.id.clone()), goal);
        run.apply(RunEvent::Dispatch)?;

        let thread = crate::thread::Thread::new(&run.id.0);
        let mut active = ActiveRun {
            run,
            worktree,
            base_sha,
            plan_path: plan_link.plan_path.clone(),
            stages: Vec::new(),
            current_stage_id: None,
            revising_stage_id: None,
            auto_advance: false,
            adopted: false,
            pending_continuation: false,
            model_choice,
            thread,
            last_summary: None,
            last_error: None,
            session: SessionSlot::default(),
        };

        // Multi-stage plan → the first stage's build session (progress record
        // created, stage diff pinned to the materialization commit); a
        // single-doc plan → the whole-plan build prompt.
        let prompt = if plan_link.is_multi_stage() {
            let first_stage = &plan_link.stages[0];
            active.current_stage_id = Some(first_stage.id.clone());
            let mut progress = StageProgress::dispatched(&first_stage.id);
            progress.start_sha = active.base_sha.clone();
            active.stages.push(progress);
            self.render_run_stage(
                &self.templates.build_stage,
                &active,
                &plan_link.stages,
                0,
                "",
            )
        } else {
            self.render_run(&self.templates.build, &active, "")
        };
        self.spawn_run_session(&mut active, &prompt, "build")?;
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
    /// empty for adopted runs and single-doc plans), which routes multi-stage
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
            // A mid-run stage-doc revision does not advance the build: its
            // `done` is a store write-back to the plan, not a build report.
            // Routing it here would let `on_run_stage_session_done` commit and
            // validate as if the stage were built — reject and point the caller
            // at the cross-entity consumer.
            (DonePhase::Revise, DoneStatus::Completed) if active.revising_stage_id.is_some() => {
                return Err(OrchestratorError::Gate(
                    "this run has a stage-doc revision in flight; route the report to \
                     consume_run_stage_revision (it writes the revision back to the plan store)"
                        .to_string(),
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
            // Single-doc plan / adopted path: a completed build opens review.
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
        self.spawn_run_session(active, &prompt, "validate")?;
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
        // The mcp layer guarantees outputs.validation on validate/completed,
        // but reports also arrive over the daemon socket as raw JSON (a
        // version-skewed mcp binary, any local writer) — a missing report is
        // a rejected report, never a panic inside the app mutex.
        let Some(validation) = report.outputs.validation.clone() else {
            return Err(OrchestratorError::Gate(
                "validate/completed report carried no outputs.validation; rejected".to_string(),
            ));
        };
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
        active.stages[progress_index].validation = Some(validation);
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
    /// adopted/migrated runs.
    pub fn run_diff(&self, active: &ActiveRun) -> Result<WorktreeDiff, OrchestratorError> {
        match &active.base_sha {
            Some(sha) => Ok(diff_against_base(&active.worktree.path, sha)?),
            None => Ok(diff_against_merge_base(
                &active.worktree.path,
                &active.worktree.base_branch,
            )?),
        }
    }

    /// Dispatch one stage's build in a fresh cold session from the between-
    /// stages gate — the run-side successor of the fused `dispatch_stage`. The
    /// sequential gate lives here and is deliberately cross-entity without any
    /// map lookup: the caller passes the owning plan's stage docs, so the run
    /// checks (1) the plan marks THIS stage `Approved`, and (2) every earlier
    /// stage passed validation ON THIS RUN (its `StageProgress` is
    /// `Validated{passed:true}`). Only then does it capture `start_sha` and
    /// spawn.
    pub fn dispatch_run_stage(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
        stage_id: &str,
        model_override: Option<ModelChoice>,
    ) -> Result<(), OrchestratorError> {
        let doc_index = plan_stage_docs
            .iter()
            .position(|doc| doc.id == stage_id)
            .ok_or_else(|| {
                OrchestratorError::Gate(format!("stage {stage_id} is not in the plan's stage docs"))
            })?;
        // Run coarse-state legality first (Dispatch is legal from StageGate; the
        // very first stage comes through `dispatch_run` instead) — nothing is
        // spawned for an illegal dispatch.
        run_transition(&active.run.state, RunEvent::Dispatch)
            .map_err(|e| OrchestratorError::Gate(format!("cannot dispatch a stage: {e}")))?;
        // Plan-side gate: the human approved this stage's doc.
        if plan_stage_docs[doc_index].state != StageDocState::Approved {
            return Err(OrchestratorError::Gate(format!(
                "stage {stage_id} is not approved (plan doc state {:?})",
                plan_stage_docs[doc_index].state
            )));
        }
        // Sequential gate: every earlier stage must have passed validation on
        // this run (the run consults its own progress, keyed by the plan's ids).
        if let Some(unvalidated) = plan_stage_docs[..doc_index].iter().find(|doc| {
            active
                .stage_progress(&doc.id)
                .map(|progress| progress.state)
                != Some(StageProgressState::Validated { passed: true })
        }) {
            return Err(OrchestratorError::Gate(format!(
                "stage {} has not passed validation yet",
                unvalidated.id
            )));
        }

        active.run.apply(RunEvent::Dispatch)?;
        let start_sha = self
            .git(&active.worktree.path, &["rev-parse", "HEAD"])?
            .trim()
            .to_string();
        // A fresh next stage has no progress record yet; create one (`Building`,
        // pinned to the current HEAD). A record already present keeps its
        // `start_sha` so the stage diff always covers all of its work.
        match active.stage_progress_index(stage_id) {
            Some(existing) => {
                if active.stages[existing].start_sha.is_none() {
                    active.stages[existing].start_sha = Some(start_sha);
                }
            }
            None => {
                let mut progress = StageProgress::dispatched(stage_id);
                progress.start_sha = Some(start_sha);
                active.stages.push(progress);
            }
        }
        active.current_stage_id = Some(stage_id.to_string());
        if let Some(choice) = model_override {
            active.model_choice = choice;
        }
        active.last_error = None;
        active.session.end();
        let prompt = self.render_run_stage(
            &self.templates.build_stage,
            active,
            plan_stage_docs,
            doc_index,
            "",
        );
        self.spawn_run_session(active, &prompt, "build")?;
        Ok(())
    }

    /// Send a validation-failed stage back to a fresh fix session (the run-side
    /// `fix_stage`). The stored validation findings drive the prompt; `note` is
    /// the reviewer's optional steer. The stage's `start_sha` is kept across the
    /// fix re-dispatch so its diff still covers all of the stage's work.
    pub fn fix_run_stage(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
        stage_id: &str,
        note: &str,
    ) -> Result<(), OrchestratorError> {
        let doc_index = plan_stage_docs
            .iter()
            .position(|doc| doc.id == stage_id)
            .ok_or_else(|| {
                OrchestratorError::Gate(format!("stage {stage_id} is not in the plan's stage docs"))
            })?;
        run_transition(&active.run.state, RunEvent::Dispatch)
            .map_err(|e| OrchestratorError::Gate(format!("cannot fix a stage: {e}")))?;
        let progress_index = active.stage_progress_index(stage_id).ok_or_else(|| {
            OrchestratorError::Gate(format!("stage {stage_id} has no progress to fix"))
        })?;
        if active.stages[progress_index].state != (StageProgressState::Validated { passed: false })
        {
            return Err(OrchestratorError::Gate(format!(
                "stage {stage_id} has no failed validation to fix (state {:?})",
                active.stages[progress_index].state
            )));
        }

        active.run.apply(RunEvent::Dispatch)?;
        // Validated{passed:false} → Building, keeping start_sha and the stored
        // findings the fix prompt consumes.
        active.stages[progress_index].apply(StageProgressEvent::Dispatch)?;
        active.current_stage_id = Some(stage_id.to_string());
        active.last_error = None;
        active.session.end();
        let prompt = self.render_run_stage(
            &self.templates.fix_stage,
            active,
            plan_stage_docs,
            doc_index,
            note,
        );
        self.spawn_run_session(active, &prompt, "build")?;
        Ok(())
    }

    /// Submit a batch of diff comments: address them in a fresh build session
    /// (the run-side `request_changes`). Valid both from `Review` (agent done)
    /// and `Building` (agent still running). A stage awaiting its validation
    /// verdict is refused — only a `validate` report may move it, so redirecting
    /// it here would hang the run.
    pub fn run_request_changes(
        &self,
        active: &mut ActiveRun,
        comments: &str,
    ) -> Result<(), OrchestratorError> {
        if let Some(stage_id) = active.current_stage_id.clone() {
            if let Some(progress) = active.stage_progress(&stage_id) {
                if matches!(
                    progress.state,
                    StageProgressState::Built | StageProgressState::Validating
                ) {
                    return Err(OrchestratorError::Gate(format!(
                        "stage {stage_id} is awaiting validation; wait for the verdict \
                         before requesting changes"
                    )));
                }
            }
        }
        active.run.apply(RunEvent::RequestChanges)?;
        active.last_error = None;
        active.session.end();
        let prompt = self.render_run(&self.templates.review_changes, active, comments);
        self.spawn_run_session(active, &prompt, "revise")?;
        Ok(())
    }

    /// A freeform human message to the run's agent (the run-side `message`). A
    /// live `Building` session is redirected; parked states (blocked / failed /
    /// idle / interrupted) resume building. Review gates (`Review`, `StageGate`)
    /// are refused — they have structured verbs. Like the fused path, a live
    /// session with an existing harness transcript continues that conversation
    /// (`--continue`) with the message as its next turn; otherwise a fresh
    /// session gets it wrapped in full run context. A stage awaiting its
    /// validation verdict is refused (the same invariant as `run_request_changes`).
    pub fn message_run(
        &self,
        active: &mut ActiveRun,
        message: &str,
    ) -> Result<(), OrchestratorError> {
        if message.trim().is_empty() {
            return Err(OrchestratorError::Gate("message must not be empty".into()));
        }
        if let Some(stage_id) = active.current_stage_id.clone() {
            if let Some(progress) = active.stage_progress(&stage_id) {
                if matches!(
                    progress.state,
                    StageProgressState::Built | StageProgressState::Validating
                ) {
                    return Err(OrchestratorError::Gate(format!(
                        "stage {stage_id} is awaiting validation; wait for the \
                         verdict before messaging the agent"
                    )));
                }
            }
        }
        use crate::run::RunState as S;
        let event = match active.run.state {
            S::Building => None,
            S::Blocked | S::Failed | S::IdleUnreported | S::Interrupted => Some(RunEvent::Reply),
            S::Review | S::StageGate => {
                return Err(OrchestratorError::Gate(
                    "the run is at a review gate — use request changes / dispatch a stage there"
                        .into(),
                ))
            }
            S::Created | S::Merged | S::Abandoned | S::Archived => {
                return Err(OrchestratorError::Gate(
                    "no agent session to message".into(),
                ))
            }
        };
        if let Some(event) = event {
            // Pure legality first — the caller persists the run even on Err.
            run_transition(&active.run.state, event)?;
        }
        let prompt = if (self.transcript_probe)(&active.worktree.path, active.model_choice.provider)
        {
            message.to_string()
        } else {
            self.render_run(&self.templates.message, active, message)
        };
        if let Some(event) = event {
            active.run.apply(event)?;
        }
        active.last_error = None;
        active.session.end();
        active.pending_continuation = true;
        self.spawn_run_session(active, &prompt, "message")?;
        Ok(())
    }

    /// Re-dispatch an interrupted build phase in a fresh session (the run-side
    /// `resume`). `plan_stage_docs` (the caller's join by `plan_id`; empty for a
    /// single-doc/adopted run) routes a multi-stage run by its current stage's
    /// persisted progress. The prompt is routed BEFORE the `Reply` transition
    /// commits, so a routing failure never strands the run out of its
    /// interrupted state.
    pub fn resume_run(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
    ) -> Result<(), OrchestratorError> {
        run_transition(&active.run.state, RunEvent::Reply)?;
        let prompt = if plan_stage_docs.is_empty() {
            self.render_run(&self.templates.build, active, "")
        } else {
            self.resume_run_stage_prompt(active, plan_stage_docs)?
        };
        active.run.apply(RunEvent::Reply)?;
        active.last_error = None;
        active.session.end();
        self.spawn_run_session(active, &prompt, "resume")?;
        Ok(())
    }

    /// An interrupted multi-stage build phase, routed by the current stage's
    /// persisted progress: `Building` respawns the build session (or `fix_stage`
    /// when a failed validation report shows that is what died); `Built` /
    /// `Validating` respawn the validation pass (a `Built` stage is forced to
    /// `Validating` first). A `Validated` current stage means the interrupted
    /// session was a post-review change request, whose comments were not
    /// persisted — it cannot be resumed blindly.
    fn resume_run_stage_prompt(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
    ) -> Result<String, OrchestratorError> {
        let stage_id = active.current_stage_id.clone().ok_or_else(|| {
            OrchestratorError::Gate(
                "multi-stage run is building but has no current stage".to_string(),
            )
        })?;
        let doc_index = plan_stage_docs
            .iter()
            .position(|doc| doc.id == stage_id)
            .ok_or_else(|| {
                OrchestratorError::Gate(format!("stage {stage_id} is not in the plan's stage docs"))
            })?;
        let progress_index = active.stage_progress_index(&stage_id).ok_or_else(|| {
            OrchestratorError::Gate(format!("no progress record for stage {stage_id}"))
        })?;
        match active.stages[progress_index].state {
            StageProgressState::Building => {
                let died_in_fix_session = active.stages[progress_index]
                    .validation
                    .as_ref()
                    .is_some_and(|report| !report.passed);
                let template = if died_in_fix_session {
                    &self.templates.fix_stage
                } else {
                    &self.templates.build_stage
                };
                Ok(self.render_run_stage(template, active, plan_stage_docs, doc_index, ""))
            }
            StageProgressState::Built => {
                active.stages[progress_index].apply(StageProgressEvent::StartValidation)?;
                Ok(self.render_run_stage(&self.templates.validate, active, plan_stage_docs, doc_index, ""))
            }
            StageProgressState::Validating => Ok(self.render_run_stage(
                &self.templates.validate,
                active,
                plan_stage_docs,
                doc_index,
                "",
            )),
            StageProgressState::Validated { passed: true } => {
                Err(OrchestratorError::Gate(format!(
                    "stage {stage_id} already passed validation — the interrupted session was a \
                     post-review change request; re-send the diff comments with Request Changes, \
                     or approve the merge"
                )))
            }
            StageProgressState::Validated { passed: false } => {
                Err(OrchestratorError::Gate(format!(
                    "stage {stage_id} failed validation — dispatch a fix session instead of resuming"
                )))
            }
        }
    }

    /// Mint a plan-less run around an existing external worktree (the run-side
    /// `adopt`; `plan_id` is `None`). No agent session is spawned — the run
    /// lands in `Review` (there is work to review). Order matches the fused
    /// path: checkpoint FIRST (pre-Build work stays its own legible commit),
    /// then scaffold `.build/mcp.json` (left uncommitted). Any error aborts with
    /// nothing persisted — the caller only persists on `Ok`.
    pub fn adopt_run(
        &self,
        id: RunId,
        external: &ExternalWorktree,
        base_branch: &str,
        model_choice: ModelChoice,
    ) -> Result<ActiveRun, OrchestratorError> {
        let Some(branch) = external.branch.clone() else {
            return Err(OrchestratorError::Gate(
                "cannot adopt a detached-HEAD worktree — check out a branch first".to_string(),
            ));
        };
        if branch == base_branch {
            return Err(OrchestratorError::Gate(format!(
                "cannot adopt a worktree with the base branch {base_branch:?} checked out"
            )));
        }
        // The branch name is an EXTERNAL, untrusted string handed to `git merge`
        // / `git push` as a bare argv element later; a leading `-` would be read
        // as an option (arbitrary code execution). Native branches are always
        // `build/<slug>` and can never trip this.
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
        let mut run = Run::new(id, None, goal);
        run.apply(RunEvent::Dispatch)?;
        run.apply(RunEvent::BuildReady)?;

        let thread = crate::thread::Thread::new(&run.id.0);
        Ok(ActiveRun {
            run,
            worktree,
            // Adopted runs baseline their review diff on the merge-base — there
            // is no materialization commit to pin.
            base_sha: None,
            plan_path: DEFAULT_PLAN_PATH.to_string(),
            stages: Vec::new(),
            current_stage_id: None,
            revising_stage_id: None,
            auto_advance: false,
            adopted: true,
            pending_continuation: true,
            model_choice,
            thread,
            last_summary: None,
            last_error: None,
            session: SessionSlot::default(),
        })
    }

    /// Approve the run's diff and merge (the run-side `approve_merge`). Merge
    /// honesty (contract): the git work runs FIRST — only if commit + merge
    /// succeed does the run become `Merged`; a git failure keeps it in `Review`
    /// with a `merge_failed:` reason. Worktree cleanup is deliberately left to
    /// the caller (after it persists the `Merged` verdict), collapsing the
    /// crash window to a self-healing one.
    pub fn run_approve_merge(&self, active: &mut ActiveRun) -> Result<(), OrchestratorError> {
        // Reject up front if the run isn't at a review gate (pure legality).
        run_transition(&active.run.state, RunEvent::ApproveMerge)?;
        self.commit_all(&active.worktree.path, &active.run.goal)
            .map_err(as_merge_failure)?;
        self.merge_into_base(&active.worktree.branch, &active.worktree.base_branch)
            .map_err(as_merge_failure)?;
        active.run.apply(RunEvent::ApproveMerge)?;
        active.last_error = None;
        active.session.end();
        Ok(())
    }

    /// Commit any outstanding work on the run branch (the implicit commit step
    /// every finish action shares). Keeps the worktree; no lifecycle change.
    pub fn run_commit(&self, active: &ActiveRun) -> Result<(), OrchestratorError> {
        self.commit_all(&active.worktree.path, &active.run.goal)
    }

    /// Commit, then push the run branch to its `origin`. Keeps the worktree, so
    /// the agent can keep working / the user can open a PR.
    pub fn run_push(&self, active: &ActiveRun) -> Result<(), OrchestratorError> {
        self.commit_all(&active.worktree.path, &active.run.goal)?;
        self.git(
            &active.worktree.path,
            // `--` stops option parsing so an option-shaped branch name can
            // never be read as a flag (defense in depth alongside `adopt_run`).
            &["push", "-u", "origin", "--", &active.worktree.branch],
        )?;
        Ok(())
    }

    /// Approve & merge (as [`run_approve_merge`](Self::run_approve_merge)) and
    /// then push the updated base branch to `origin`.
    pub fn run_merge_and_push(&self, active: &mut ActiveRun) -> Result<(), OrchestratorError> {
        let base = active.worktree.base_branch.clone();
        self.run_approve_merge(active)?;
        self.git(&self.repo_path, &["push", "origin", &base])?;
        Ok(())
    }

    /// Abandon a run from any non-terminal state: kill the harness, mark the run
    /// `Abandoned`, and remove its worktree. Per the run entity's contract the
    /// BRANCH is kept — a run's work survives an abandon so it can be
    /// re-attempted — unlike the fused path, which pruned both. Cleanup is
    /// best-effort: a leftover worktree is logged, never a reason to fail the
    /// abandon (the lifecycle verdict is what must persist).
    pub fn abandon_run(&self, active: &mut ActiveRun) -> Result<(), OrchestratorError> {
        active.run.apply(RunEvent::Abandon)?;
        active.session.end();
        if let Err(cleanup) = self
            .worktrees
            .remove(&active.worktree, /* keep_branch */ true)
        {
            eprintln!(
                "abandon run {}: run abandoned but worktree cleanup failed: {cleanup}",
                active.worktree.name
            );
        }
        Ok(())
    }

    /// Mid-run stage-doc revision (spec seam #3): re-plan one stage's doc from
    /// the plan's open comments, but run the revision session in the RUN's
    /// worktree (that is where the docs are materialized and where the diff /
    /// PTY live). Only legal at the between-stages gate, where the upcoming
    /// stage's doc is under review. The run's coarse state is untouched — the
    /// revision is a plan-doc operation that merely borrows the run's worktree;
    /// `revising_stage_id` marks it so the resulting `done(revise)` is routed to
    /// [`consume_run_stage_revision`](Self::consume_run_stage_revision) (a store
    /// write-back) rather than through [`on_run_done`](Self::on_run_done).
    pub fn send_run_stage_notes(
        &self,
        active: &mut ActiveRun,
        plan: &ActivePlan,
        stage_id: &str,
    ) -> Result<(), OrchestratorError> {
        if active.run.state != RunState::StageGate {
            return Err(OrchestratorError::Gate(format!(
                "stage-doc revisions run from the stage gate (run is {:?})",
                active.run.state
            )));
        }
        let doc_index = plan
            .stage_doc_index(stage_id)
            .map_err(OrchestratorError::Gate)?;
        let open: Vec<PlanStageComment> = plan
            .open_comments_for(stage_id)
            .into_iter()
            .cloned()
            .collect();
        if open.is_empty() {
            return Err(OrchestratorError::Gate(format!(
                "no open comments on stage {stage_id}"
            )));
        }
        active.revising_stage_id = Some(stage_id.to_string());
        active.last_error = None;
        active.session.end();
        let prompt = self.render_run_stage(
            &self.templates.revise_stage,
            active,
            &plan.stages,
            doc_index,
            THREAD_NOTIFICATION,
        );
        self.spawn_run_session(active, &prompt, "revise")?;
        Ok(())
    }

    /// Consume a mid-run stage-doc revision's `done(revise)`: ingest the revised
    /// docs from the run's worktree back into the canonical store (fail-fast —
    /// the revision is never accepted with unpersisted docs), reset the plan's
    /// stage-doc state (a revised doc's approval is stale), and land the agent's
    /// per-comment resolutions on the plan's comments. Cross-entity by design:
    /// the caller hands both the run (whose worktree holds the docs) and the
    /// owning plan (whose store id, doc state, and comments are updated). The
    /// run's coarse state is untouched.
    pub fn consume_run_stage_revision(
        &self,
        active: &mut ActiveRun,
        plan: &mut ActivePlan,
        store: &Store,
        report: &DoneReport,
    ) -> Result<(), OrchestratorError> {
        let stage_id = active.revising_stage_id.clone().ok_or_else(|| {
            OrchestratorError::Gate(
                "revise report for a run with no stage revision in flight".to_string(),
            )
        })?;
        let index = plan
            .stage_doc_index(&stage_id)
            .map_err(OrchestratorError::Gate)?;
        // Probe the doc transition before any mutation.
        stage_doc_transition(&plan.stages[index].state, StageDocEvent::Revised)?;
        if let Err(ingest_error) =
            store.ingest_plan_docs(&plan.plan.id.0, &active.worktree.path, &plan.plan_path)
        {
            active.last_error = Some(format!("stage revision not persisted: {ingest_error}"));
            return Err(OrchestratorError::Store(ingest_error));
        }
        plan.stages[index].state =
            stage_doc_transition(&plan.stages[index].state, StageDocEvent::Revised)?;
        if let Some(resolutions) = &report.outputs.comment_resolutions {
            for resolution in resolutions {
                let matching = plan.comments.iter_mut().find(|c| {
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
                        "run stage revision for {stage_id}: unknown or non-open comment {:?}; \
                         skipping",
                        resolution.comment_id
                    ),
                }
            }
        }
        active.revising_stage_id = None;
        active.last_summary = Some(report.summary.clone());
        active.last_error = None;
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
        phase: &str,
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
        let prompt = conversation_prompt(prompt, &active.thread);
        self.spawn_into_slot(
            &mut active.session,
            &cwd,
            &active.plan.id.0,
            &model_choice,
            false,
            &prompt,
        )?;
        let session_id = active.thread.start_session(
            active.model_choice.provider.label(),
            active.model_choice.model.as_deref(),
            active.model_choice.effort.as_deref(),
            phase,
            &crate::store::now_rfc3339(),
        );
        active.thread.push_event(
            crate::thread::ThreadEventKind::RunStarted,
            Some(format!("{phase} run started")),
            Some(session_id),
            None,
            crate::store::now_rfc3339(),
        );
        Ok(())
    }

    /// Spawn the run's next session, consuming the warm-session continuation flag:
    /// the first spawn after adoption probes for an existing harness
    /// transcript in the worktree and asks the TUI builder to continue it.
    fn spawn_run_session(
        &self,
        active: &mut ActiveRun,
        prompt: &str,
        phase: &str,
    ) -> Result<(), OrchestratorError> {
        let continue_session = active.pending_continuation
            && (self.transcript_probe)(&active.worktree.path, active.model_choice.provider);
        active.pending_continuation = false;
        let cwd = active.worktree.path.clone();
        let model_choice = active.model_choice.clone();
        let prompt = conversation_prompt(prompt, &active.thread);
        self.spawn_into_slot(
            &mut active.session,
            &cwd,
            &active.run.id.0,
            &model_choice,
            continue_session,
            &prompt,
        )?;
        let session_id = active.thread.start_session(
            active.model_choice.provider.label(),
            active.model_choice.model.as_deref(),
            active.model_choice.effort.as_deref(),
            phase,
            &crate::store::now_rfc3339(),
        );
        active.thread.push_event(
            crate::thread::ThreadEventKind::RunStarted,
            Some(format!("{phase} run started")),
            Some(session_id),
            None,
            crate::store::now_rfc3339(),
        );
        Ok(())
    }

    /// The shared spawn tail for both split entities: build the harness
    /// command, spawn it in `cwd`, and install it in the slot (bumping the
    /// generation). Mirrors [`spawn_session`](Self::spawn_session), which the
    /// periphery retires with the fused path.
    fn spawn_into_slot(
        &self,
        slot: &mut SessionSlot,
        cwd: &Path,
        owner_id: &str,
        model_choice: &ModelChoice,
        continue_session: bool,
        prompt: &str,
    ) -> Result<(), OrchestratorError> {
        let options = SpawnOptions {
            continue_session,
            owner_id: owner_id.to_string(),
            cwd: cwd.to_path_buf(),
        };
        let spec = match &self.agent {
            // Fixed warm harnesses take the prompt over the PTY and never
            // need provider/model-specific SpawnOptions.
            Agent::Warm(spec) => spec.clone(),
            Agent::WarmBuilder(build) => build(prompt, model_choice, &options),
        };
        let session = PtySession::spawn(&spec, Some(cwd.to_path_buf()), self.pty_size)?;
        // Wait (bounded) for the TUI's first output before injecting the
        // prompt; on expiry or an early exit, write anyway — the exit-race
        // guard below decides whether a failed write is benign.
        session.ready_within(HARNESS_READY_GRACE);
        if let Err(error) = session.write_prompt(prompt) {
            Self::absorb_prompt_write_failure_of_exiting_harness(&session, error)?;
        }
        slot.install(session);
        Ok(())
    }

    /// Decide whether a failed prompt write into a fresh session is benign. A
    /// harness that exits immediately is still a session the idle/crash
    /// observer must retain and report — PTYs return EIO once the child's side
    /// is closed — so an exiting harness keeps its session and the write error
    /// is swallowed. The child closes the PTY *before* the OS makes its exit
    /// status reapable, so a single `has_exited` poll here races the kernel
    /// and can fail the spawn for a harness that is already gone; the bounded
    /// wait covers that reap lag while still surfacing the write error for a
    /// genuinely wedged (live but unwritable) PTY.
    fn absorb_prompt_write_failure_of_exiting_harness(
        session: &PtySession,
        error: PtyError,
    ) -> Result<(), OrchestratorError> {
        if session.exited_within(PROMPT_WRITE_EXIT_GRACE) {
            Ok(())
        } else {
            Err(error.into())
        }
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
        self.write_build_dir(&worktree.path, owner_id)
    }

    /// The scaffold itself, over a bare path — an agent tab may be opened in a
    /// worktree Build has no [`Worktree`] record for yet.
    fn write_build_dir(
        &self,
        worktree_path: &Path,
        owner_id: &str,
    ) -> Result<(), OrchestratorError> {
        let build_dir = worktree_path.join(".build");
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
    /// full-size rendered prompt would block and then fail with EIO. The startup
    /// byte matters too: like a real TUI painting its screen, it satisfies the
    /// spawn's readiness wait so dispatches don't idle out the grace. The test
    /// plays the agent: it writes files and forwards `done` reports.
    fn warm_harness() -> HarnessSpec {
        HarnessSpec::new("sh")
            .arg("-c")
            .arg("printf '\\033[?2004h'; cat >/dev/null")
    }

    fn orchestrator(dir: &tempfile::TempDir, repo: &Path) -> Orchestrator {
        Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            Agent::Warm(warm_harness()),
            Templates::default(),
        )
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

    /// A warm agent builder that records every spawn's prompt and continue flag.
    fn prompt_spy_agent(log: std::sync::Arc<std::sync::Mutex<Vec<(String, bool)>>>) -> Agent {
        Agent::WarmBuilder(std::sync::Arc::new(
            move |prompt: &str, _choice: &ModelChoice, options: &SpawnOptions| {
                log.lock()
                    .unwrap()
                    .push((prompt.to_string(), options.continue_session));
                HarnessSpec::new("sh").arg("-c").arg("exit 0")
            },
        ))
    }

    // ---- The prompt-write race ----

    #[tokio::test]
    async fn prompt_write_failure_is_absorbed_while_the_exit_is_not_yet_reapable() {
        // The race, deterministically: a dying harness closes its PTY (the
        // write fails with EIO) *before* the OS makes its exit reapable, so a
        // single `has_exited` poll says "running" and the old guard failed
        // the spawn. A child that is un-reapable now but exits shortly after
        // is that kernel window with the lag under test control.
        let spec = HarnessSpec::new("sh").arg("-c").arg("sleep 0.15");
        let session = PtySession::spawn(
            &spec,
            None,
            PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            },
        )
        .unwrap();
        assert!(
            !session.has_exited(),
            "precondition: the exit status must not be reapable yet"
        );
        let write_error = PtyError::Io(std::io::Error::from_raw_os_error(libc_eio()));

        let verdict =
            Orchestrator::absorb_prompt_write_failure_of_exiting_harness(&session, write_error);

        assert!(
            verdict.is_ok(),
            "a write failure against an exiting harness is benign: {verdict:?}"
        );
    }

    /// EIO — the errno a PTY write returns once the child's side is closed.
    fn libc_eio() -> i32 {
        5
    }

    #[tokio::test]
    async fn dispatch_survives_a_harness_that_exits_before_the_prompt_write() {
        // End-to-end: an instantly exiting harness must still yield a spawned,
        // installed session (the idle/crash observer retains and reports it),
        // never a failed dispatch.
        let (dir, repo) = init_repo();
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            Agent::WarmBuilder(std::sync::Arc::new(
                |_prompt: &str, _choice: &ModelChoice, _options: &SpawnOptions| {
                    HarnessSpec::new("sh").arg("-c").arg("exit 0")
                },
            )),
            Templates::default(),
        );

        let plan = orch
            .dispatch_plan(
                PlanId::new("plan-1"),
                "Add a greeting",
                "main",
                Default::default(),
            )
            .expect("an instantly exiting harness must not fail the dispatch");

        assert_eq!(plan.session.generation(), 1, "the session was installed");
        assert!(plan.session.subscribe().is_some());
    }

    // ---- Worktree adoption ----

    use crate::worktree::discover_external_worktrees;

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

    // ---- Multi-stage: lifecycle ----

    use crate::plan::StageManifestEntry;
    use crate::run::ValidationReport;

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

    fn last_commit_subject(worktree: &Path) -> String {
        let out = Command::new("git")
            .args(["log", "-1", "--format=%s"])
            .current_dir(worktree)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A warm agent builder that records every rendered prompt it is asked to spawn.
    fn prompt_recording_agent(log: std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> Agent {
        Agent::WarmBuilder(std::sync::Arc::new(
            move |prompt: &str, _choice: &ModelChoice, _options: &SpawnOptions| {
                log.lock().unwrap().push(prompt.to_string());
                HarnessSpec::new("sh").arg("-c").arg("exit 0")
            },
        ))
    }

    // ---- Multi-stage: per-stage revision, fix sessions, and resume routing ----

    // ---- Multi-stage: ActiveTask bookkeeping ----

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
        plan_in_review_with_goal(orch, store, id, "Add a greeting")
    }

    fn plan_in_review_with_goal(
        orch: &Orchestrator,
        store: &Store,
        id: &str,
        goal: &str,
    ) -> ActivePlan {
        let mut plan = drafting_plan(orch, id, goal);
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
        approved_plan_with_goal(orch, store, id, "Add a greeting")
    }

    fn approved_plan_with_goal(
        orch: &Orchestrator,
        store: &Store,
        id: &str,
        goal: &str,
    ) -> ActivePlan {
        let mut plan = plan_in_review_with_goal(orch, store, id, goal);
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
        let stage_ids: Vec<String> = plan.stages.iter().map(|s| s.id.clone()).collect();
        for stage_id in stage_ids {
            orch.approve_plan_stage(&mut plan, &stage_id).unwrap();
        }
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
            RunSource {
                plan,
                has_active_run: false,
            },
            "main",
            Default::default(),
            store,
        )
        .unwrap()
    }

    /// A run implementing a single-doc plan: one build session, no stage
    /// pipeline. This is what the retired goal-only dispatch used to stand in
    /// for, so it is the fixture for every run-side test that only needs "a run
    /// the agent is building in". The plan id derives from the run id, so
    /// repeated calls inside one test never collide.
    fn dispatch_single_stage_run(
        orch: &Orchestrator,
        store: &Store,
        id: &str,
        goal: &str,
    ) -> ActiveRun {
        let plan = approved_plan_with_goal(orch, store, &format!("plan-of-{id}"), goal);
        dispatch_planned_run(orch, store, &plan, id)
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
    async fn warm_builder_receives_the_rendered_prompt_through_its_pty() {
        let (dir, repo) = init_repo();
        let capture = dir.path().join("warm-prompt.txt");
        let capture_for_builder = capture.clone();
        let agent = Agent::WarmBuilder(std::sync::Arc::new(
            move |_prompt: &str, _choice: &ModelChoice, _options: &SpawnOptions| {
                // The startup byte signals readiness like a real TUI's first paint.
                HarnessSpec::new("sh")
                    .arg("-c")
                    .arg("printf ready; cat > \"$1\"")
                    .arg("build-warm-capture")
                    .arg(capture_for_builder.to_string_lossy())
            },
        ));
        let orch = Orchestrator::new(
            repo,
            dir.path().join("worktrees"),
            agent,
            Templates::default(),
        );

        let plan = drafting_plan(&orch, "plan-warm", "Warm prompt marker");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if std::fs::read_to_string(&capture)
                    .is_ok_and(|contents| contents.contains("Warm prompt marker"))
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("rendered prompt should be written into the warm PTY");

        assert!(plan.session.subscribe().is_some());
    }

    #[tokio::test]
    async fn dispatch_paste_frames_the_prompt_and_writes_even_without_readiness() {
        // The rendered dispatch prompt is always multi-line (conversation_prompt
        // appends the protocol block), so through a real TUI it must arrive as
        // ONE bracketed paste. The capture harness never produces output, so
        // this also proves the readiness grace expires into a write rather
        // than a silently lost prompt.
        let (dir, repo) = init_repo();
        let capture = dir.path().join("warm-stdin.txt");
        let capture_for_builder = capture.clone();
        let agent = Agent::WarmBuilder(std::sync::Arc::new(
            move |_prompt: &str, _choice: &ModelChoice, _options: &SpawnOptions| {
                HarnessSpec::new("sh")
                    .arg("-c")
                    .arg("cat > \"$1\"")
                    .arg("build-warm-capture")
                    .arg(capture_for_builder.to_string_lossy())
            },
        ));
        let orch = Orchestrator::new(
            repo,
            dir.path().join("worktrees"),
            agent,
            Templates::default(),
        );

        drafting_plan(&orch, "plan-paste", "Paste framing marker");
        let captured = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if let Ok(contents) = std::fs::read_to_string(&capture) {
                    if contents.contains("\u{1b}[201~") {
                        return contents;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the framed prompt should reach the harness despite its silence");

        assert!(
            captured.starts_with("\u{1b}[200~"),
            "the prompt opens as a bracketed paste: {captured:?}"
        );
        assert!(
            captured.contains("Paste framing marker"),
            "the rendered prompt rides inside the frame: {captured:?}"
        );
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
    async fn dispatch_run_rejects_a_plan_whose_first_stage_doc_is_unapproved() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        // A migrated plan can rest at Approved while a stage doc is Planned
        // (legacy records never re-gate); dispatch must still hold the line.
        let mut plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
        plan.stages[0].state = StageDocState::Planned;

        let Err(error) = orch.dispatch_run(
            RunId::new("run-1"),
            RunSource {
                plan: &plan,
                has_active_run: false,
            },
            "main",
            Default::default(),
            &store,
        ) else {
            panic!("stage 0 must be approved before its build session spawns");
        };
        assert!(
            error.to_string().contains("first"),
            "the gate names the unapproved stage: {error}"
        );
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
            provider: crate::models::AgentProvider::Claude,
            model: Some("claude-opus-4-8".into()),
            effort: Some("xhigh".into()),
            thread: crate::thread::Thread::new("plan-1"),
            last_summary: Some("planned it".into()),
            last_error: Some("boom".into()),
            created_at: "2026-07-01T10:00:00Z".into(),
            updated_at: "2026-07-01T10:05:00Z".into(),
            state_changed_at: None,
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
            provider: crate::models::AgentProvider::Claude,
            model: None,
            effort: Some("high".into()),
            thread: crate::thread::Thread::new("run-1"),
            last_summary: Some("built it".into()),
            last_error: None,
            created_at: "2026-07-01T10:00:00Z".into(),
            updated_at: "2026-07-01T10:05:00Z".into(),
            state_changed_at: None,
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
    async fn dispatch_single_stage_run_goes_straight_to_building() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        let mut run = dispatch_single_stage_run(&orch, &store, "run-1", "fix typo");
        assert_eq!(run.run.state, RunState::Building);
        assert_eq!(
            run.run.plan_id.as_ref().map(|id| id.0.as_str()),
            Some("plan-of-run-1"),
            "every run implements a plan"
        );
        assert!(
            run.base_sha.is_some(),
            "the materialized plan doc baselines the review diff"
        );
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
            RunSource {
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
            RunSource {
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

        // A single-stage run must ignore a validate report outright.
        let mut single = dispatch_single_stage_run(&orch, &store, "run-q", "single stage work");
        orch.on_run_done(&mut single, &[], done_validate(true, "- ok", ""))
            .unwrap();
        assert_eq!(single.run.state, RunState::Building, "ignored");

        // A run session misusing phase=plan is rejected: plan reports belong
        // to plans, and consuming one here would smuggle manifest edits.
        let err = orch
            .on_run_done(
                &mut single,
                &[],
                done(DonePhase::Plan, DoneStatus::Completed, None),
            )
            .expect_err("plan reports belong to plans");
        assert!(matches!(err, OrchestratorError::Gate(_)), "{err}");
        assert_eq!(single.run.state, RunState::Building);

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
        let mut run = dispatch_single_stage_run(&orch, &store, "run-1", "single stage work");

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

    // ---- Plan/Run split: stage flows, interaction verbs, git finishers ----

    /// Drive a two-stage planned run through its first stage (build + a passing
    /// validation), leaving it parked at the between-stages gate with stage one
    /// `Validated{passed:true}`.
    fn run_past_first_stage(
        orch: &Orchestrator,
        store: &Store,
        plan: &ActivePlan,
        id: &str,
    ) -> ActiveRun {
        let mut run = dispatch_planned_run(orch, store, plan, id);
        std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();
        orch.on_run_done(&mut run, &plan.stages, done_validate(true, "- ok", "notes"))
            .unwrap();
        assert_eq!(run.run.state, RunState::StageGate);
        run
    }

    #[tokio::test]
    async fn approve_plan_stage_approves_a_doc_and_rejects_on_terminal() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = multi_stage_plan_in_review(&orch, &store, "plan-1", 2);

        // An approved plan keeps taking per-stage approvals (that is how a run's
        // later stages get gated while an earlier one builds).
        orch.approve_plan(&mut plan).unwrap();
        orch.approve_plan_stage(&mut plan, "second").unwrap();
        assert_eq!(plan.stages[1].state, StageDocState::Approved);
        assert_eq!(plan.stages[0].state, StageDocState::Planned);

        // Double-approve is rejected by the pure doc-state machine.
        let err = orch
            .approve_plan_stage(&mut plan, "second")
            .expect_err("double approve is illegal");
        assert!(matches!(err, OrchestratorError::StageDoc(_)), "{err}");

        // Unknown stage id → a gate error, not a panic.
        assert!(matches!(
            orch.approve_plan_stage(&mut plan, "ghost"),
            Err(OrchestratorError::Gate(_))
        ));

        // A terminal plan takes no approvals.
        orch.abandon_plan(&mut plan).unwrap();
        let err = orch
            .approve_plan_stage(&mut plan, "first")
            .expect_err("no approvals on a terminal plan");
        assert!(err.to_string().contains("terminal"), "{err}");
    }

    #[tokio::test]
    async fn send_plan_stage_notes_revises_a_stage_and_round_trips_through_done() {
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            prompt_recording_agent(log.clone()),
            Templates::default(),
        );
        let store = split_store(&dir);
        let mut plan = multi_stage_plan_in_review(&orch, &store, "plan-1", 2);
        plan.stages[0].state = StageDocState::Approved;
        plan.comments = vec![
            plan_comment("c-1", "first", PlanCommentState::Open),
            plan_comment("c-2", "second", PlanCommentState::Open),
        ];

        orch.send_plan_stage_notes(&mut plan, &store, "first")
            .unwrap();
        assert_eq!(plan.plan.state, PlanState::Drafting);
        assert_eq!(plan.revising_stage_id.as_deref(), Some("first"));
        assert!(plan.session.subscribe().is_some(), "revise session is warm");
        let prompt = log.lock().unwrap().last().unwrap().clone();
        assert!(prompt.contains("read_unread_messages"), "{prompt}");
        assert!(
            !prompt.contains("[c-1]"),
            "comments travel through MCP: {prompt}"
        );
        assert!(prompt.contains(".build/plan/01-first.md"), "{prompt}");

        // The agent revises the doc and reports done → back to PlanReview, the
        // stage approval reset, the comment resolved, the store copy updated.
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
                    comment_resolutions: Some(vec![crate::mcp::CommentResolution {
                        comment_id: "c-1".into(),
                        response: "done".into(),
                    }]),
                    ..DoneOutputs::default()
                },
            },
        )
        .unwrap();
        assert_eq!(plan.plan.state, PlanState::PlanReview);
        assert_eq!(plan.stages[0].state, StageDocState::Planned);
        assert_eq!(plan.revising_stage_id, None);
        assert_eq!(
            plan.comments.iter().find(|c| c.id == "c-1").unwrap().state,
            PlanCommentState::Addressed
        );
        assert_eq!(
            store
                .read_plan_doc("plan-1", ".build/plan/01-first.md")
                .as_deref(),
            Some("# Stage: First (revised)\n")
        );
    }

    #[tokio::test]
    async fn send_plan_stage_notes_gates_on_plan_state_and_open_comments() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = multi_stage_plan_in_review(&orch, &store, "plan-1", 2);

        // No open comments on the stage.
        let err = orch
            .send_plan_stage_notes(&mut plan, &store, "first")
            .expect_err("no open comments");
        assert!(err.to_string().contains("no open comments"), "{err}");
        assert_eq!(plan.plan.state, PlanState::PlanReview);

        // Not at the review gate (approved) → the transition is rejected.
        orch.approve_plan(&mut plan).unwrap();
        plan.comments = vec![plan_comment("c-1", "first", PlanCommentState::Open)];
        let err = orch
            .send_plan_stage_notes(&mut plan, &store, "first")
            .expect_err("an approved plan is past the review gate");
        assert!(err.to_string().contains("cannot send stage notes"), "{err}");
    }

    #[tokio::test]
    async fn message_plan_redirects_drafting_resumes_parked_and_refuses_gates() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        // Empty message is refused before any state is touched.
        let mut plan = drafting_plan(&orch, "plan-1", "Add a greeting");
        assert!(orch
            .message_plan(&mut plan, &store, "   ")
            .unwrap_err()
            .to_string()
            .contains("empty"));

        // Drafting → a live redirect (no state change), fresh session.
        orch.message_plan(&mut plan, &store, "focus on error paths")
            .unwrap();
        assert_eq!(plan.plan.state, PlanState::Drafting);
        assert!(plan.session.subscribe().is_some());

        // A blocked plan resumes drafting on reply.
        orch.on_plan_done(
            &mut plan,
            &store,
            done(DonePhase::Plan, DoneStatus::Blocked, None),
        )
        .unwrap();
        assert_eq!(plan.plan.state, PlanState::Blocked);
        orch.message_plan(&mut plan, &store, "here is the missing detail")
            .unwrap();
        assert_eq!(plan.plan.state, PlanState::Drafting);

        // The review gate refuses a side-channel message.
        let mut in_review = plan_in_review(&orch, &store, "plan-2");
        let err = orch
            .message_plan(&mut in_review, &store, "sneak past the gate")
            .expect_err("review gate has send-notes");
        assert!(err.to_string().contains("review gate"), "{err}");
    }

    #[tokio::test]
    async fn resume_plan_redispatches_an_interrupted_plan_recreating_its_worktree() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        // A plan interrupted mid-draft: its store docs survive, its worktree is
        // gone (simulate the loss by tearing it down after reattach).
        let mut plan = plan_in_review(&orch, &store, "plan-1");
        // Move it back to a working phase then interrupt it.
        orch.send_plan_notes(&mut plan, &store, "revise").unwrap();
        plan.plan.apply(crate::plan::PlanEvent::Interrupt).unwrap();
        // Drop the worktree from disk to prove resume re-creates one.
        let stale = plan.worktree.take().unwrap();
        orch.discard_worktree(&stale);
        assert!(!stale.path.exists());

        orch.resume_plan(&mut plan, &store).unwrap();
        assert_eq!(plan.plan.state, PlanState::Drafting);
        let worktree = plan
            .worktree
            .as_ref()
            .expect("resume re-created a worktree");
        assert!(
            worktree.path.join(".build/plan.md").exists(),
            "docs materialized"
        );
        assert!(
            plan.session.subscribe().is_some(),
            "a fresh plan session is warm"
        );
    }

    #[tokio::test]
    async fn abandon_plan_tears_down_the_worktree_and_keeps_the_store_docs() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = plan_in_review(&orch, &store, "plan-1");
        let worktree = plan.worktree.clone().unwrap();

        orch.abandon_plan(&mut plan).unwrap();
        assert_eq!(plan.plan.state, PlanState::Abandoned);
        assert_eq!(plan.worktree, None);
        assert!(!worktree.path.exists(), "the disposable worktree is gone");
        assert_eq!(
            store.read_plan_doc("plan-1", ".build/plan.md").as_deref(),
            Some("# Plan v1\n"),
            "canonical docs survive an abandon"
        );
    }

    #[tokio::test]
    async fn dispatch_run_stage_enforces_the_sequential_gate_and_pins_start_sha() {
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            prompt_recording_agent(log.clone()),
            Templates::default(),
        );
        let store = split_store(&dir);
        let mut plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
        let mut run = run_past_first_stage(&orch, &store, &plan, "run-1");
        // Un-approve the second doc (a mid-run revision resets approval the
        // same way) so the gate has something to refuse.
        plan.stages[1].state = StageDocState::Planned;

        // The next stage's doc is not approved yet → refused.
        let err = orch
            .dispatch_run_stage(&mut run, &plan.stages, "second", None)
            .expect_err("an unapproved stage cannot dispatch");
        assert!(err.to_string().contains("not approved"), "{err}");
        assert_eq!(
            run.run.state,
            RunState::StageGate,
            "no state change on refusal"
        );

        // Approve it → the sequential gate opens (stage one validated).
        orch.approve_plan_stage(&mut plan, "second").unwrap();
        orch.dispatch_run_stage(&mut run, &plan.stages, "second", None)
            .unwrap();
        assert_eq!(run.run.state, RunState::Building);
        assert_eq!(run.current_stage_id.as_deref(), Some("second"));
        let second = run.stage_progress("second").unwrap();
        assert_eq!(second.state, StageProgressState::Building);
        assert_eq!(
            second.start_sha.as_deref(),
            Some(worktree_head(&run.worktree.path).as_str()),
            "the stage diff pins to HEAD at dispatch"
        );
        let prompt = log.lock().unwrap().last().unwrap().clone();
        assert!(prompt.contains(".build/plan/02-second.md"), "{prompt}");
    }

    #[tokio::test]
    async fn validate_done_without_a_validation_report_is_rejected_not_a_panic() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
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
        assert_eq!(
            run.stage_progress("first").unwrap().state,
            StageProgressState::Validating
        );

        // The daemon socket deserializes reports as raw JSON — a
        // validate/completed with no outputs.validation must be rejected
        // with zero mutation, never unwrapped.
        let err = orch
            .on_run_done(
                &mut run,
                &plan.stages,
                done(DonePhase::Validate, DoneStatus::Completed, None),
            )
            .expect_err("a report without outputs.validation is rejected");
        assert!(err.to_string().contains("no outputs.validation"), "{err}");
        assert_eq!(
            run.stage_progress("first").unwrap().state,
            StageProgressState::Validating,
            "the stage still awaits a real verdict"
        );
        assert_eq!(run.run.state, RunState::Building);
    }

    #[tokio::test]
    async fn dispatch_run_stage_rejects_a_stage_whose_predecessor_has_not_validated() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
        // Stage one fails validation → the run parks at the gate, stage one
        // `Validated{passed:false}`.
        let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
        std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();
        orch.on_run_done(&mut run, &plan.stages, done_validate(false, "- nope", ""))
            .unwrap();
        assert_eq!(run.run.state, RunState::StageGate);

        let err = orch
            .dispatch_run_stage(&mut run, &plan.stages, "second", None)
            .expect_err("stage one has not passed validation");
        assert!(
            err.to_string().contains("has not passed validation"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn fix_run_stage_respawns_with_findings_and_keeps_the_start_sha() {
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
        orch.on_run_done(
            &mut run,
            &plan.stages,
            done_validate(false, "- migration missing", ""),
        )
        .unwrap();
        let start_before = run.stage_progress("first").unwrap().start_sha.clone();

        orch.fix_run_stage(&mut run, &plan.stages, "first", "add the migration")
            .unwrap();
        assert_eq!(run.run.state, RunState::Building);
        let first = run.stage_progress("first").unwrap();
        assert_eq!(first.state, StageProgressState::Building);
        assert_eq!(
            first.start_sha, start_before,
            "the fix keeps the stage's start sha"
        );
        let prompt = log.lock().unwrap().last().unwrap().clone();
        assert!(
            prompt.contains("- migration missing"),
            "findings drive the fix: {prompt}"
        );
        assert!(
            prompt.contains("add the migration"),
            "the note is the steer: {prompt}"
        );

        // Nothing to fix on a stage without a failed validation.
        let err = orch
            .fix_run_stage(&mut run, &plan.stages, "second", "")
            .expect_err("second has no progress to fix");
        assert!(matches!(err, OrchestratorError::Gate(_)), "{err}");
    }

    #[tokio::test]
    async fn run_request_changes_respawns_from_review_and_is_gated_while_validating() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        // A single-stage run at review → a change request respawns building.
        let mut single = dispatch_single_stage_run(&orch, &store, "run-q", "single stage work");
        orch.on_run_done(
            &mut single,
            &[],
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();
        assert_eq!(single.run.state, RunState::Review);
        orch.run_request_changes(&mut single, "tweak it").unwrap();
        assert_eq!(single.run.state, RunState::Building);

        // A stage awaiting its validation verdict must not be redirected.
        let plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
        let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
        std::fs::write(run.worktree.path.join("first.txt"), "one\n").unwrap();
        orch.on_run_done(
            &mut run,
            &plan.stages,
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();
        assert_eq!(
            run.stage_progress("first").unwrap().state,
            StageProgressState::Validating
        );
        let err = orch
            .run_request_changes(&mut run, "no")
            .expect_err("cannot redirect a validating stage");
        assert!(err.to_string().contains("awaiting validation"), "{err}");
    }

    #[tokio::test]
    async fn message_run_redirects_building_continues_and_refuses_gates() {
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, bool)>::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            prompt_spy_agent(log.clone()),
            Templates::default(),
        )
        .with_transcript_probe(std::sync::Arc::new(|_, _| true));
        let store = split_store(&dir);

        let mut run = dispatch_single_stage_run(&orch, &store, "run-1", "single stage work");
        assert!(orch
            .message_run(&mut run, "  ")
            .unwrap_err()
            .to_string()
            .contains("empty"));

        // Building → live redirect that rides the harness's own conversation.
        orch.message_run(&mut run, "also handle the empty case")
            .unwrap();
        assert_eq!(run.run.state, RunState::Building);
        let (prompt, continued) = log.lock().unwrap().last().unwrap().clone();
        assert!(continued, "the message continues the conversation");
        assert!(prompt.starts_with("also handle the empty case"));
        assert!(prompt.contains("Build conversation protocol"));

        // The review gate refuses a message (request-changes is the verb there).
        orch.on_run_done(
            &mut run,
            &[],
            done(DonePhase::Build, DoneStatus::Completed, None),
        )
        .unwrap();
        let err = orch
            .message_run(&mut run, "sneak past")
            .expect_err("review gate refuses messages");
        assert!(err.to_string().contains("review gate"), "{err}");
    }

    #[tokio::test]
    async fn spawned_plan_and_run_prompts_put_the_ambiguity_rule_before_silent_directives() {
        let (dir, repo) = init_repo();
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(String, bool)>::new()));
        let orch = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees"),
            prompt_spy_agent(log.clone()),
            Templates::default(),
        );
        let store = split_store(&dir);

        drafting_plan(&orch, "plan-1", "Add a greeting");
        let (plan_prompt, _) = log.lock().unwrap().last().unwrap().clone();
        dispatch_single_stage_run(&orch, &store, "run-1", "single stage work");
        let (run_prompt, _) = log.lock().unwrap().last().unwrap().clone();

        for (path, prompt) in [("plan", plan_prompt), ("run", run_prompt)] {
            let ambiguity_rule = prompt
                .find("either a question or a directive")
                .unwrap_or_else(|| {
                    panic!("{path} spawn prompt lacks the ambiguity rule: {prompt}")
                });
            let silent_directive_allowance = prompt
                .find("directive without replying")
                .unwrap_or_else(|| {
                    panic!("{path} spawn prompt lacks the silent-directive allowance: {prompt}")
                });
            assert!(
                ambiguity_rule < silent_directive_allowance,
                "{path}: the ambiguity rule must precede the silent-directive allowance so an \
                 in-order reader hits the carve-out before committing to silence: {prompt}"
            );
        }
    }

    #[test]
    fn conversation_prompt_instructs_clarifying_reply_for_ambiguous_comments() {
        let thread = crate::thread::Thread::new("run-1");
        let prompt = conversation_prompt("do the work", &thread);
        assert!(prompt.contains("Build conversation protocol"), "{prompt}");
        assert!(
            prompt.contains("either a question or a directive"),
            "ambiguous reviewer messages must trigger a clarifying reply: {prompt}"
        );
        assert!(
            prompt.contains("one-line clarifying reply"),
            "the reply must be a one-liner, not a silent code change: {prompt}"
        );
    }

    #[tokio::test]
    async fn resume_run_redispatches_single_stage_and_multi_stage_builds() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        // A single-stage run interrupted mid-build → resumes the whole-run build.
        let mut single = dispatch_single_stage_run(&orch, &store, "run-q", "single stage work");
        single.run.apply(crate::run::RunEvent::Interrupt).unwrap();
        orch.resume_run(&mut single, &[]).unwrap();
        assert_eq!(single.run.state, RunState::Building);
        assert!(single.session.subscribe().is_some());

        // Multi-stage run interrupted mid stage-build → resumes THAT stage.
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let orch2 = Orchestrator::new(
            repo.to_path_buf(),
            dir.path().join("worktrees2"),
            prompt_recording_agent(log.clone()),
            Templates::default(),
        );
        let plan = approved_multi_stage_plan(&orch2, &store, "plan-1", 2);
        let mut run = dispatch_planned_run(&orch2, &store, &plan, "run-1");
        run.run.apply(crate::run::RunEvent::Interrupt).unwrap();
        orch2.resume_run(&mut run, &plan.stages).unwrap();
        assert_eq!(run.run.state, RunState::Building);
        let prompt = log.lock().unwrap().last().unwrap().clone();
        assert!(
            prompt.contains(".build/plan/01-first.md"),
            "resumes stage one: {prompt}"
        );
    }

    #[tokio::test]
    async fn adopt_run_lands_in_review_as_a_plan_less_run() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let external = user_worktree(&dir, &repo, "wt-user", "user/thing");
        std::fs::write(external.path.join("notes.txt"), "pre-Build work\n").unwrap();

        let run = orch
            .adopt_run(RunId::new("run-ad"), &external, "main", Default::default())
            .unwrap();
        assert_eq!(run.run.state, RunState::Review);
        assert_eq!(run.run.plan_id, None, "an adopted run has no plan");
        assert_eq!(run.run.goal, "user/thing");
        assert_eq!(
            run.base_sha, None,
            "adopted runs baseline on the merge-base"
        );
        assert!(run.adopted);
        assert!(run.pending_continuation);
        assert!(
            run.session.subscribe().is_none(),
            "adoption spawns no session"
        );
        assert_eq!(
            last_commit_subject(&external.path),
            "Checkpoint: adopted by Build"
        );
    }

    #[tokio::test]
    async fn run_finishers_commit_merge_and_report_conflicts() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);

        let to_review = |id: &str, file: &str, contents: &str| {
            let mut run = dispatch_single_stage_run(&orch, &store, id, "same file");
            std::fs::write(run.worktree.path.join(file), contents).unwrap();
            orch.on_run_done(
                &mut run,
                &[],
                done(DonePhase::Build, DoneStatus::Completed, None),
            )
            .unwrap();
            assert_eq!(run.run.state, RunState::Review);
            run
        };

        // Both runs branch from the same base tip and touch the same file.
        let mut first = to_review("run-1", "result.txt", "first\n");
        let mut second = to_review("run-2", "result.txt", "second\n");

        // Commit keeps the worktree and makes an honest commit.
        orch.run_commit(&first).unwrap();
        assert_eq!(
            last_commit_subject(&first.worktree.path),
            "Build: same file"
        );

        // Approve & merge → Merged, base branch tracks the file, worktree kept
        // until the caller prunes.
        orch.run_approve_merge(&mut first).unwrap();
        assert_eq!(first.run.state, RunState::Merged);
        assert!(repo.join("result.txt").exists());
        assert!(
            first.worktree.path.exists(),
            "merge leaves cleanup to the caller"
        );

        // The second run now conflicts: it reports merge_failed and stays in review.
        let err = orch
            .run_approve_merge(&mut second)
            .expect_err("the second write conflicts");
        assert!(err.to_string().starts_with("merge_failed:"), "{err}");
        assert_eq!(second.run.state, RunState::Review);
        assert!(
            !repo.join(".git/MERGE_HEAD").exists(),
            "a failed merge is aborted"
        );
    }

    #[tokio::test]
    async fn abandon_run_removes_the_worktree_but_keeps_the_branch() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut run = dispatch_single_stage_run(&orch, &store, "run-1", "single stage work");
        let branch = run.worktree.branch.clone();
        let path = run.worktree.path.clone();

        orch.abandon_run(&mut run).unwrap();
        assert_eq!(run.run.state, RunState::Abandoned);
        assert!(!path.exists(), "the worktree is removed");
        // The branch survives — a run's work outlives an abandon so it can be
        // re-attempted (the run entity's documented contract).
        let branches = Command::new("git")
            .args(["branch", "--list", &branch])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&branches.stdout).contains(&branch),
            "the branch is kept on abandon"
        );
    }

    #[tokio::test]
    async fn mid_run_stage_revision_writes_back_to_the_plan_store() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
        // The upcoming stage's doc was approved; a reviewer left a comment.
        plan.stages[1].state = StageDocState::Approved;
        plan.comments = vec![plan_comment("c-1", "second", PlanCommentState::Open)];
        let mut run = run_past_first_stage(&orch, &store, &plan, "run-1");

        // The revision runs in the RUN's worktree; the run's coarse state is
        // untouched (it merely lends its worktree).
        orch.send_run_stage_notes(&mut run, &plan, "second")
            .unwrap();
        assert_eq!(run.run.state, RunState::StageGate);
        assert_eq!(run.revising_stage_id.as_deref(), Some("second"));
        assert!(run.session.subscribe().is_some());

        // While a revision is in flight, a revise report must NOT go through
        // on_run_done — it is a store write-back, not a build report.
        let revise = DoneReport {
            phase: DonePhase::Revise,
            status: DoneStatus::Completed,
            summary: "revised".into(),
            outputs: DoneOutputs {
                comment_resolutions: Some(vec![crate::mcp::CommentResolution {
                    comment_id: "c-1".into(),
                    response: "reworked the section".into(),
                }]),
                ..DoneOutputs::default()
            },
        };
        let guard = orch
            .on_run_done(&mut run, &plan.stages, revise.clone())
            .expect_err("on_run_done rejects a revision in flight");
        assert!(
            guard.to_string().contains("consume_run_stage_revision"),
            "{guard}"
        );

        // The agent revised the doc in the run's worktree; consuming ingests it
        // back to the plan store, resets the stale approval, resolves the comment.
        std::fs::write(
            run.worktree.path.join(".build/plan/02-second.md"),
            "# Stage: Second (reworked)\n",
        )
        .unwrap();
        orch.consume_run_stage_revision(&mut run, &mut plan, &store, &revise)
            .unwrap();
        assert_eq!(run.revising_stage_id, None);
        assert_eq!(
            run.run.state,
            RunState::StageGate,
            "the build did not advance"
        );
        assert_eq!(
            plan.stages[1].state,
            StageDocState::Planned,
            "a revised doc resets its stale approval"
        );
        assert_eq!(plan.comments[0].state, PlanCommentState::Addressed);
        assert_eq!(
            store
                .read_plan_doc("plan-1", ".build/plan/02-second.md")
                .as_deref(),
            Some("# Stage: Second (reworked)\n"),
            "the revision reached the canonical store copy"
        );
    }

    #[tokio::test]
    async fn send_run_stage_notes_is_only_legal_at_the_stage_gate() {
        let (dir, repo) = init_repo();
        let orch = orchestrator(&dir, &repo);
        let store = split_store(&dir);
        let mut plan = approved_multi_stage_plan(&orch, &store, "plan-1", 2);
        plan.comments = vec![plan_comment("c-1", "first", PlanCommentState::Open)];
        // A run still building its first stage is not at the gate.
        let mut run = dispatch_planned_run(&orch, &store, &plan, "run-1");
        let err = orch
            .send_run_stage_notes(&mut run, &plan, "first")
            .expect_err("not at the stage gate");
        assert!(err.to_string().contains("stage gate"), "{err}");
    }
}
