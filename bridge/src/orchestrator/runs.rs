use portable_pty::PtySize;

use crate::agent::AgentRoster;
use crate::diff::{diff_against_base, diff_against_merge_base, WorktreeDiff};
use crate::git_process::run_git;
use crate::mcp::DoneReport;
use crate::models::ModelChoice;
use crate::plan::{
    stage_doc_transition, PlanId, PlanState, StageDoc, StageDocEvent, StageDocState,
};
use crate::run::{run_transition, Run, RunEvent, RunId, StageProgress, StageProgressState};
use crate::store::{PersistedRun, Store};
use crate::templates::{self, Templates, Vars};
use crate::worktree::{configured_remote_for_branch, slugify, Worktree, WorktreeManager};
use std::path::PathBuf;

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
    /// "Run all": auto-dispatch the next approved stage when one completes.
    pub auto_advance: bool,
    /// True for a run minted around a pre-existing (user-created) worktree.
    ///
    /// Read by the prune rules, boot-recovery parking, the release verb and the
    /// SPA's option sets — and by nothing about conversations: an adopted
    /// branch's first agent starts a conversation of its own like any other,
    /// because the one the human was already having is one Build never heard
    /// and cannot show.
    pub adopted: bool,
    /// Push/merge write-ahead intent, retained across a crash until repository
    /// refs independently prove or disprove publication.
    pub publication_attempt: Option<crate::run::PublicationAttempt>,
    /// Which model/effort this run's agents run on (None = harness default).
    pub model_choice: ModelChoice,
    /// This entity's agents — see [`ActivePlan::agents`]. A branch carries as
    /// many as the human adds; they share the one worktree.
    pub agents: AgentRoster,
    /// The most recent `done` summary, surfaced on cards.
    pub last_summary: Option<String>,
    /// The most recent failure surfaced to the reviewer (merge failure,
    /// harness crash). Cleared whenever the run advances again.
    pub last_error: Option<String>,
}

impl ActiveRun {
    /// A conversation owner for a durable workspace container. This is shaped
    /// like an adopted run so the existing conversation and agent surfaces can
    /// address it, but it performs no Git adoption: the agent works from the
    /// workspace root where every materialized source is reachable.
    pub fn workspace_conversation(
        id: RunId,
        workspace_name: String,
        workspace_root: PathBuf,
        model_choice: ModelChoice,
    ) -> Self {
        let mut run = Run::new(id, None, workspace_name.clone());
        run.apply(RunEvent::Dispatch)
            .expect("a new workspace conversation can enter building");
        run.apply(RunEvent::BuildReady)
            .expect("a workspace conversation can wait in review");
        ActiveRun {
            run,
            worktree: Worktree {
                name: workspace_name.clone(),
                path: workspace_root,
                recorded_branch: workspace_name,
                base_branch: String::new(),
            },
            base_sha: None,
            plan_path: crate::templates::DEFAULT_PLAN_PATH.to_string(),
            stages: Vec::new(),
            current_stage_id: None,
            revising_stage_id: None,
            auto_advance: false,
            adopted: true,
            publication_attempt: None,
            model_choice,
            agents: AgentRoster::empty(),
            last_summary: None,
            last_error: None,
        }
    }

    /// Reattach a run recovered from the durable store after a daemon restart:
    /// the worktree survived on disk, the PTY session did not. `plan_path` is
    /// re-derived by the caller from the owning plan's record (adopted runs pass
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
                recorded_branch: record.branch.clone(),
                base_branch: record.base_branch.clone(),
            },
            base_sha: record.base_sha.clone(),
            plan_path,
            stages: record.stages.clone(),
            current_stage_id: record.current_stage_id.clone(),
            revising_stage_id: record.revising_stage_id.clone(),
            auto_advance: record.auto_advance,
            adopted: record.adopted,
            publication_attempt: record.publication_attempt.clone(),
            model_choice: record.model_choice(),
            agents: record.roster(),
            last_summary: record.last_summary.clone(),
            last_error: record.last_error.clone(),
        }
    }

    /// This run's progress record for one stage, if the stage was dispatched.
    pub fn stage_progress(&self, stage_id: &str) -> Option<&StageProgress> {
        self.stages.iter().find(|p| p.stage_id == stage_id)
    }

    pub(super) fn stage_progress_index(&self, stage_id: &str) -> Option<usize> {
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

/// The checkout an implementation opens in, as the git left it: on its own
/// branch, scaffolded, with the Task's canonical docs committed. `base_sha` is
/// that commit — the baseline the review diff is read against.
pub struct PreparedImplementation {
    pub worktree: Worktree,
    pub base_sha: String,
}

/// A Task cleared to have an implementation opened for it, and everything
/// opening one needs before any git runs: the words its checkout is named
/// after, and the plan whose canonical docs are committed into it as the
/// review baseline.
///
/// Construction IS the gate — a plan that is not ready, one somebody else is
/// already writing for, or one whose first stage the human has not approved
/// never becomes one — so nothing downstream can cut a checkout for work that
/// was refused.
pub struct ImplementableTask {
    pub(super) plan_id: String,
    pub(super) goal: String,
    slug: String,
}

impl ImplementableTask {
    /// What every implementation of a Task must be true of before any
    /// checkout is touched, whichever worktree it is going to run in: the plan
    /// is ready, nobody else is writing for it, and the stage the first session
    /// would build is one the human approved.
    pub fn judge(source: RunSource<'_>) -> Result<ImplementableTask, OrchestratorError> {
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
        Ok(ImplementableTask {
            plan_id: plan_link.plan.id.0.clone(),
            goal: plan_link.plan.goal.clone(),
            slug: slugify(&plan_link.plan.goal),
        })
    }

    /// What the checkout this implementation cuts is named after — the board's
    /// placeholder id is hashed from the path it makes, so the decide phase and
    /// the git that follows it must read the slug from one place.
    pub fn slug(&self) -> &str {
        &self.slug
    }
}

use super::reporting::as_merge_failure;
use super::workspace::append_stage_catalog;
use super::{ActivePlan, Agent, AgentLaunch, AgentTurn, Orchestrator, OrchestratorError};

impl Orchestrator {
    pub fn new(
        repo_path: impl Into<PathBuf>,
        worktrees_root: impl Into<PathBuf>,
        agent: Agent,
        templates: Templates,
        bridge_exe: PathBuf,
    ) -> Self {
        let repo_path = repo_path.into();
        let worktrees_root = worktrees_root.into();
        let worktrees = WorktreeManager::new(repo_path.clone(), worktrees_root.clone());
        // Named before #190, and kept: a plan drafting across the upgrade
        // resumes in the scratch docs it already has.
        let plan_docs_root = worktrees_root.join(".issue-docs");
        Orchestrator {
            repo_path: repo_path.clone(),
            worktrees,
            plan_docs_root,
            launch: AgentLaunch {
                repo_path,
                bridge_exe,
                agent,
                pty_size: PtySize {
                    rows: 40,
                    cols: 120,
                    pixel_width: 0,
                    pixel_height: 0,
                },
            },
            templates,
        }
    }
    pub(crate) fn agent_launch(&self) -> AgentLaunch {
        self.launch.clone()
    }
    /// Open the run that stands for a prepared checkout: the record, the agent
    /// that will do the work, and the turn that starts it.
    ///
    /// Pure bookkeeping — the git ran in [`prepare_run_checkout`], and the
    /// refusals were made when the [`ImplementableTask`] was judged.
    ///
    /// A multi-stage plan's first session is its first stage's build — the
    /// plan-level `Approved` gate covers starting stage one; later stages
    /// dispatch from the stage gate.
    ///
    /// [`prepare_run_checkout`]: Self::prepare_run_checkout
    pub fn open_prepared_run(
        &self,
        id: RunId,
        plan_link: &ActivePlan,
        prepared: PreparedImplementation,
        model_choice: ModelChoice,
    ) -> Result<(ActiveRun, AgentTurn), OrchestratorError> {
        let mut run = Run::new(
            id,
            Some(plan_link.plan.id.clone()),
            plan_link.plan.goal.clone(),
        );
        run.apply(RunEvent::Dispatch)?;

        let agents = AgentRoster::with_first(
            &run.id.0,
            model_choice.clone(),
            &crate::store::now_rfc3339(),
        );
        let mut active = ActiveRun {
            run,
            worktree: prepared.worktree,
            base_sha: Some(prepared.base_sha),
            plan_path: plan_link.plan_path.clone(),
            stages: Vec::new(),
            current_stage_id: None,
            revising_stage_id: None,
            auto_advance: false,
            adopted: false,
            publication_attempt: None,
            model_choice,
            agents,
            last_summary: None,
            last_error: None,
        };

        let turn = self.open_implementation(&mut active, plan_link);
        Ok((active, turn))
    }
    /// The first turn of an implementation, on a run whose worktree is already
    /// prepared and whose baseline is already pinned.
    ///
    /// Multi-stage plan → the first stage's build session (progress record
    /// created, stage diff pinned to the materialization commit); a single-doc
    /// plan → the whole-plan build prompt.
    pub(super) fn open_implementation(
        &self,
        active: &mut ActiveRun,
        plan_link: &ActivePlan,
    ) -> AgentTurn {
        let prompt = if plan_link.is_multi_stage() {
            let first_stage = &plan_link.stages[0];
            active.current_stage_id = Some(first_stage.id.clone());
            let mut progress = StageProgress::dispatched(&first_stage.id);
            progress.start_sha = active.base_sha.clone();
            active.stages.push(progress);
            self.render_run_stage(
                &self.templates.build_stage,
                active,
                &plan_link.stages,
                0,
                "",
            )
        } else {
            self.render_run(&self.templates.build, active, "", &plan_link.stages)
        };
        AgentTurn::dispatched(prompt, "build")
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
    /// stage completed ON THIS RUN (its `StageProgress` is `Completed`). Only
    /// then does it capture `start_sha` and spawn.
    pub fn dispatch_run_stage(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
        stage_id: &str,
        model_override: Option<ModelChoice>,
    ) -> Result<AgentTurn, OrchestratorError> {
        let doc_index = plan_stage_docs
            .iter()
            .position(|doc| doc.id == stage_id)
            .ok_or_else(|| {
                OrchestratorError::Gate(format!("stage {stage_id} is not in the plan's stage docs"))
            })?;
        // Run coarse-state legality first (Dispatch is legal from StageGate; the
        // very first stage comes through `open_prepared_run` instead) — nothing is
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
        // Sequential gate: every earlier stage must have completed on this
        // run (the run consults its own progress, keyed by the plan's ids).
        if let Some(incomplete) = plan_stage_docs[..doc_index].iter().find(|doc| {
            active
                .stage_progress(&doc.id)
                .map(|progress| progress.state)
                != Some(StageProgressState::Completed)
        }) {
            return Err(OrchestratorError::Gate(format!(
                "stage {} has not completed yet",
                incomplete.id
            )));
        }

        // Probe the candidate boundary before mutating the run machine. A
        // vanished/corrupt checkout must leave StageGate intact for recovery.
        let start_sha = run_git(&active.worktree.path, &["rev-parse", "HEAD"])?
            .trim()
            .to_string();
        active.run.apply(RunEvent::Dispatch)?;
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
        let prompt = self.render_run_stage(
            &self.templates.build_stage,
            active,
            plan_stage_docs,
            doc_index,
            "",
        );
        Ok(AgentTurn::dispatched(prompt, "build"))
    }
    /// Submit a batch of diff comments (the run-side `request_changes`): put the
    /// run back to work and hand the caller the turn to deliver. Valid both from
    /// `Review` (agent parked) and `Building` (agent still working).
    ///
    /// The worktree's agent is never ended and never replaced: the reviewer is
    /// mid conversation with a process, and killing it to say something to it
    /// throws away the context that made the review worth having.
    /// `conversation_agent` names whose conversation the comments were posted
    /// to, for the catch-up packet a cold spawn opens on. `None` is the roster
    /// standing on the run — the first agent's, which the caller may have
    /// swapped for the Task's.
    pub fn run_request_changes(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
        comments: &str,
        conversation_agent: Option<&str>,
    ) -> Result<AgentTurn, OrchestratorError> {
        active.run.apply(RunEvent::RequestChanges)?;
        active.last_error = None;
        let rendered = self.render_run(
            &self.templates.review_changes,
            active,
            comments,
            plan_stage_docs,
        );
        // The agent whose conversation the reviewer was reading has to be on
        // this run's roster: the turn is addressed to it, and a name that is
        // not here is a caller error rather than a turn for somebody else.
        active
            .agents
            .resolve(conversation_agent)
            .map_err(OrchestratorError::Gate)?;
        Ok(AgentTurn::posted(rendered, comments, "revise"))
    }
    /// A freeform human message to the run's agent (the run-side `message`). A
    /// working run keeps working; parked states (blocked / failed / idle /
    /// interrupted) resume building.
    ///
    /// Review gates (`Review`, `StageGate`) are still refused, but the reason
    /// narrowed when the agent became persistent. It is no longer "there is no
    /// session to talk to" — there is, and `thread.post` reaches it at a gate
    /// without moving anything. It is that `run.message` MOVES the run back to
    /// `Building`, and what reopens a gate is the gate's own structured verb
    /// (request changes, dispatch a stage). A freeform side channel that
    /// restarts the build would bypass the batched-review contract.
    pub fn message_run(
        &self,
        active: &mut ActiveRun,
        plan_stage_docs: &[StageDoc],
        message: &str,
    ) -> Result<AgentTurn, OrchestratorError> {
        if message.trim().is_empty() {
            return Err(OrchestratorError::Gate("message must not be empty".into()));
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
                    "the run's conversation is closed — there is nothing to message".into(),
                ))
            }
        };
        if let Some(event) = event {
            // Pure legality first — the caller persists the run even on Err.
            run_transition(&active.run.state, event)?;
        }
        let prompt = self.render_run(&self.templates.message, active, message, plan_stage_docs);
        if let Some(event) = event {
            active.run.apply(event)?;
        }
        active.last_error = None;
        Ok(AgentTurn::posted(prompt, message, "message"))
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
    ) -> Result<AgentTurn, OrchestratorError> {
        run_transition(&active.run.state, RunEvent::Reply)?;
        let prompt = if plan_stage_docs.is_empty() {
            self.render_run(&self.templates.build, active, "", plan_stage_docs)
        } else {
            self.resume_run_stage_prompt(active, plan_stage_docs)?
        };
        active.run.apply(RunEvent::Reply)?;
        active.last_error = None;
        Ok(AgentTurn::dispatched(prompt, "resume"))
    }
    /// An interrupted multi-stage build phase, routed by the current stage's
    /// persisted progress: `Building` respawns the build session. A `Completed`
    /// current stage means the interrupted session was a post-review change
    /// request, whose comments were not persisted — it cannot be resumed
    /// blindly.
    pub(super) fn resume_run_stage_prompt(
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
            StageProgressState::Building => Ok(self.render_run_stage(
                &self.templates.build_stage,
                active,
                plan_stage_docs,
                doc_index,
                "",
            )),
            StageProgressState::Completed => Err(OrchestratorError::Gate(format!(
                "stage {stage_id} is already complete — the interrupted session was a \
                 post-review change request; re-send the diff comments with Request Changes, \
                 or approve the merge"
            ))),
        }
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
        self.worktrees
            .merge_into_base(
                &active.worktree.path,
                &active.worktree.branch(),
                &active.worktree.base_branch,
            )
            .map_err(|error| as_merge_failure(error.into()))?;
        active.run.apply(RunEvent::ApproveMerge)?;
        active.last_error = None;
        Ok(())
    }
    /// Commit any outstanding work on the run branch (the implicit commit step
    /// every finish action shares). Keeps the worktree; no lifecycle change.
    pub fn run_commit(&self, active: &ActiveRun) -> Result<(), OrchestratorError> {
        self.commit_all(&active.worktree.path, &active.run.goal)
    }
    /// Commit, then push the run branch to its configured remote. Keeps the
    /// worktree, so the agent can keep working / the user can open a PR.
    pub fn run_push(&self, active: &ActiveRun) -> Result<(), OrchestratorError> {
        self.commit_all(&active.worktree.path, &active.run.goal)?;
        let repo = git2::Repository::discover(&active.worktree.path)
            .map_err(|error| OrchestratorError::Git(error.to_string()))?;
        let remote = configured_remote_for_branch(&repo, &active.worktree.branch())
            .ok_or_else(|| OrchestratorError::Git("no configured push remote".to_string()))?;
        run_git(
            &active.worktree.path,
            // `--` stops option parsing so option-shaped names remain opaque.
            &["push", "-u", &remote, "--", &active.worktree.branch()],
        )?;
        Ok(())
    }
    /// Approve & merge (as [`run_approve_merge`](Self::run_approve_merge)) and
    /// then push the updated base branch to `origin`.
    pub fn run_merge_and_push(&self, active: &mut ActiveRun) -> Result<(), OrchestratorError> {
        let base = active.worktree.base_branch.clone();
        self.run_approve_merge(active)?;
        let repo = git2::Repository::open(&self.repo_path)
            .map_err(|error| OrchestratorError::Git(error.to_string()))?;
        let remote = configured_remote_for_branch(&repo, &base)
            .ok_or_else(|| OrchestratorError::Git("no configured push remote".to_string()))?;
        run_git(&self.repo_path, &["push", &remote, &base])?;
        Ok(())
    }
    /// Consume a mid-run stage-doc revision's `done(revise)`: ingest the revised
    /// docs from the run's worktree back into the canonical store (fail-fast —
    /// the revision is never accepted with unpersisted docs), reset the plan's
    /// stage-doc state (a revised doc's approval is stale). Cross-entity by
    /// design: the caller hands both the run (whose worktree holds the docs)
    /// and the owning plan (whose store id and doc state are updated). The
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
        active.revising_stage_id = None;
        active.last_summary = Some(report.summary.clone());
        active.last_error = None;
        Ok(())
    }
    pub(super) fn render_run(
        &self,
        template: &str,
        active: &ActiveRun,
        comments: &str,
        plan_stage_docs: &[StageDoc],
    ) -> String {
        let rendered = templates::render(
            template,
            &Vars {
                goal: &active.run.goal,
                plan_path: &active.plan_path,
                comments,
                base_branch: &active.worktree.base_branch,
                ..Vars::default()
            },
        );
        append_stage_catalog(rendered, plan_stage_docs, |stage_id| {
            active
                .stage_progress(stage_id)
                .map(|progress| format!("{:?}", progress.state))
                .unwrap_or_else(|| "not started".to_string())
        })
    }
    /// Render a stage-scoped template for a run with the full stage variable
    /// set: the stage doc's own fields (from the plan's manifest), the next
    /// stage's doc path (empty on the final stage), and where this stage
    /// started — the split twin of
    /// [`render_stage`](Self::render_stage), joining plan docs to run progress
    /// by stage id.
    pub(super) fn render_run_stage(
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
        let stage_start_sha = active
            .stage_progress(&doc.id)
            .and_then(|p| p.start_sha.as_deref())
            .unwrap_or("");
        let rendered = templates::render(
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
                ..Vars::default()
            },
        );
        append_stage_catalog(rendered, plan_stage_docs, |stage_id| {
            active
                .stage_progress(stage_id)
                .map(|progress| match progress.state {
                    StageProgressState::Building => "building",
                    StageProgressState::Completed => "complete",
                })
                .unwrap_or("not started")
                .to_string()
        })
    }
}
