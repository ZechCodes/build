use crate::agent::AgentRoster;
use crate::git_process::run_git;
use crate::mcp::{DonePhase, DoneReport, DoneStatus};
use crate::models::ModelChoice;
use crate::plan::{
    plan_transition, stage_doc_transition, Plan, PlanEvent, PlanId, StageDoc, StageDocEvent,
};
use crate::run::RunState;
use crate::store::{PersistedPlan, Store};
use crate::templates::{self, Vars, DEFAULT_PLAN_PATH};
use crate::thread::DocComment;
use std::path::{Path, PathBuf};

/// Where an issue's planning agent works.
///
/// Planning never gets a worktree: the agent runs in the project's PRIMARY
/// checkout (it reads the code as it stands on the base branch and writes no
/// code at all), and the plan documents it produces go to a scratch docs
/// directory outside the repo, whose path the prompt hands it. `done(plan)`
/// ingests that directory into the store, which is where the canonical docs
/// live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanWorkspace {
    /// The project's primary checkout — the agent's working directory.
    pub checkout: PathBuf,
    /// The scratch directory the plan docs are written into, laid out exactly
    /// as the store holds them (`.build/plan/…`), outside the repo so planning
    /// never dirties the primary checkout.
    pub docs_dir: PathBuf,
}

/// One plan in flight: its lifecycle state, its planning workspace (while one
/// is alive), and its warm session. The canonical docs live in the store — the
/// docs dir is throwaway scratch space for the plan agent.
pub struct ActivePlan {
    pub plan: Plan,
    /// Where the planning agent works: the primary checkout plus its scratch
    /// docs dir. `None` before the session starts (an inert issue) and once
    /// the workspace is dropped (approve / abandon) — the store docs are
    /// canonical either way.
    pub workspace: Option<PlanWorkspace>,
    /// The branch the plan is written against — what the primary checkout is
    /// expected to be on, and what a run cuts its worktree from.
    pub base_branch: String,
    /// Where the plan doc lives, docs-dir-relative — convention by default,
    /// updated from `done` outputs (and fenced by the ingest).
    pub plan_path: String,
    /// Stage docs: manifest metadata + plan-side review sub-state. Empty for
    /// single-doc plans.
    pub stages: Vec<StageDoc>,
    /// The stage a plan-revision session is (or was last) running for. Not
    /// persisted on the plan record — a restart falls back to a full re-plan.
    pub revising_stage_id: Option<String>,
    /// Which model/effort this plan's agents run on (None = harness default).
    pub model_choice: ModelChoice,
    /// This entity's agents. Each owns its own durable conversation; the
    /// roster reads as the first agent's, which is what entity-level events
    /// speak to. Harness processes are recorded per agent in `sessions`.
    pub agents: AgentRoster,
    /// The most recent `done` summary, surfaced on cards.
    pub last_summary: Option<String>,
    /// The most recent failure surfaced to the reviewer (unpersisted docs,
    /// harness crash). Cleared whenever the plan advances again.
    pub last_error: Option<String>,
}

impl ActivePlan {
    /// Reattach a plan recovered from the durable store after a daemon
    /// restart: the store docs are canonical and the PTY session is gone. The
    /// workspace is not restored — the next dispatch re-derives it (and
    /// re-materializes the docs) from the store. The caller (boot recovery)
    /// moves a working state to `Interrupted` itself.
    pub fn reattach(record: &PersistedPlan) -> Self {
        ActivePlan {
            plan: Plan {
                id: PlanId::new(record.id.clone()),
                goal: record.goal.clone(),
                state: record.state,
                archived_at: record.archived_at.clone(),
                implementation_intent: record.implementation_intent.clone(),
                implementation_activity: record.implementation_activity.clone(),
            },
            workspace: None,
            base_branch: record.base_branch.clone(),
            plan_path: record.plan_path.clone(),
            stages: record.stages.clone(),
            revising_stage_id: None,
            model_choice: record.model_choice(),
            agents: record.roster(),
            last_summary: record.last_summary.clone(),
            last_error: record.last_error.clone(),
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

    /// Open comments on one stage, insertion order. Read off the Issue
    /// conversation, where the comments live as posts.
    pub fn open_comments_for(&self, stage_id: &str) -> Vec<DocComment> {
        self.agents.sole_thread().open_doc_comments_for(stage_id)
    }
}

use super::reporting::merge_stage_docs;
use super::workspace::{
    append_stage_catalog, dir_holds_a_file, gate_plan_message, gate_plan_stage_notes,
    plan_docs_dir_display, NEW_THREAD_MESSAGES_PROMPT,
};
use super::{ActiveRun, AgentTurn, Orchestrator, OrchestratorError};

impl Orchestrator {
    /// File a plan and start nothing: a record in `Created` with its goal as the
    /// conversation's first message, no worktree and no session.
    ///
    /// This is what an issue is before anyone has said anything to it — the
    /// router files them this way, and so does the toolbar's New issue. The goal
    /// is posted UNREAD: no agent exists to have read it, and
    /// [`start_plan_drafting`](Self::start_plan_drafting) is what marks it seen,
    /// because dispatching the session is the agent acting on it.
    pub fn create_plan(
        &self,
        id: PlanId,
        goal: impl Into<String>,
        base_branch: &str,
        model_choice: ModelChoice,
    ) -> ActivePlan {
        let plan = Plan::new(id, goal);
        let now = crate::store::now_rfc3339();
        let mut agents = AgentRoster::with_first(&plan.id.0, model_choice.clone(), &now);
        agents
            .sole_thread_mut()
            .post_user(plan.goal.clone(), None, &now);
        ActivePlan {
            plan,
            workspace: None,
            base_branch: base_branch.to_string(),
            plan_path: DEFAULT_PLAN_PATH.to_string(),
            stages: Vec::new(),
            revising_stage_id: None,
            model_choice,
            agents,
            last_summary: None,
            last_error: None,
        }
    }
    /// Start the planning session for a plan that has none, once its workspace
    /// is real: the plan leaves
    /// `Created`, reads everything the human has said to it so far, and the
    /// turn that spawns its session is rendered.
    ///
    /// The docs dir is throwaway — the canonical docs land in the store at each
    /// plan/revise `done` — but the session stays warm through the
    /// notes/revision loop (the scope doc's warm-session property).
    ///
    /// Pure bookkeeping: the disk work was
    /// [`prepare_plan_workspace`](Self::prepare_plan_workspace), and it ran
    /// with the app mutex released. Workspace first, state second — a failed
    /// prepare leaves the plan inert and re-startable rather than `Drafting`
    /// with nothing drafting.
    pub fn open_plan_drafting(
        &self,
        active: &mut ActivePlan,
        workspace: PlanWorkspace,
    ) -> Result<AgentTurn, OrchestratorError> {
        active.plan.apply(PlanEvent::Dispatch)?;
        active.workspace = Some(workspace);
        // Everything the user said before this moment is what the session is
        // being started to answer, so the dispatch reads all of it.
        let _ = active
            .agents
            .sole_thread_mut()
            .read_unread(&crate::store::now_rfc3339());
        let prompt = self.render_plan(&self.templates.plan, active, "");
        Ok(AgentTurn::dispatched(prompt, "plan"))
    }
    /// Where this issue's planning agent works — the primary checkout, always,
    /// plus the scratch docs dir that belongs to this issue alone.
    pub(super) fn plan_workspace(&self, plan_id: &str) -> PlanWorkspace {
        PlanWorkspace {
            checkout: self.repo_path.clone(),
            docs_dir: self.plan_docs_root.join(plan_id),
        }
    }
    /// Make that workspace real, and hold every disk touch a planning workspace
    /// needs: the scratch docs dir exists, it holds the docs as they stand, and
    /// the primary checkout carries this issue's MCP config so its `done`
    /// reports route back here.
    ///
    /// An empty docs dir — a restart, a workspace dropped at approve, a plan
    /// being drafted for the first time — is filled from the canonical store;
    /// a plan the store holds no docs for simply starts from the empty one. A
    /// dir the agent is already working in is left exactly as it is:
    /// re-materializing would overwrite the revision in flight.
    ///
    /// This is the only way a planning workspace is written, and it is a
    /// [`WorktreeMutation`](crate::lifecycle::WorktreeMutation)'s work — every
    /// door to an Issue's planning agent reaches it with the app mutex
    /// released.
    pub fn prepare_plan_workspace(
        &self,
        plan_id: &str,
        store: &Store,
    ) -> Result<PlanWorkspace, OrchestratorError> {
        let workspace = self.plan_workspace(plan_id);
        // The stage-doc directory is made up front so the agent only ever has
        // to write files into a directory that is already there.
        std::fs::create_dir_all(workspace.docs_dir.join(templates::STAGES_DIR))?;
        self.launch
            .scaffold_agent_worktree(&workspace.checkout, plan_id)?;
        if !dir_holds_a_file(&workspace.docs_dir) {
            match store.materialize_plan_docs(plan_id, &workspace.docs_dir) {
                Ok(()) | Err(crate::store::StoreError::NoStoredDocs { .. }) => {}
                Err(error) => return Err(OrchestratorError::Store(error)),
            }
        }
        Ok(workspace)
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
            (
                DonePhase::Build
                | DonePhase::Validate
                | DonePhase::Triage
                | DonePhase::Recover
                | DonePhase::Route,
                DoneStatus::Completed,
            ) => {
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
    /// The transactional half of every plan/revise `done`: copy the scratch
    /// docs into the store, or fail the report with the reason surfaced on the
    /// card. Setting `last_error` here is the one deliberate mutation on the
    /// error path — the plan stays in its working state, but the reviewer must
    /// see why the gate never opened.
    pub(super) fn ingest_plan_docs_transactionally(
        &self,
        active: &mut ActivePlan,
        store: &Store,
        plan_path: &str,
    ) -> Result<(), OrchestratorError> {
        let Some(workspace) = &active.workspace else {
            let reason = "plan docs were not persisted: the planning workspace is gone".to_string();
            active.last_error = Some(reason.clone());
            return Err(OrchestratorError::Gate(reason));
        };
        if let Err(ingest_error) =
            store.ingest_plan_docs(&active.plan.id.0, &workspace.docs_dir, plan_path)
        {
            active.last_error = Some(format!("plan docs were not persisted: {ingest_error}"));
            return Err(OrchestratorError::Store(ingest_error));
        }
        Ok(())
    }
    /// A per-stage plan-revision session completed (plan-side `done(revise)`).
    /// Probes every transition before committing any, then ingests the revised
    /// docs transactionally, then lets the state moves land.
    pub(super) fn consume_plan_stage_revision(
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
                let answers_this_stage = active
                    .open_comments_for(&stage_id)
                    .iter()
                    .any(|comment| comment.id == resolution.comment_id);
                if !answers_this_stage
                    || !active
                        .agents
                        .sole_thread_mut()
                        .resolve_doc_comment(&resolution.comment_id, &resolution.response)
                {
                    eprintln!(
                        "stage revision for {stage_id}: unknown or non-open comment {:?}; skipping",
                        resolution.comment_id
                    );
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
    /// (the store docs are canonical) and the scratch docs dir is dropped.
    /// Dropping it is best-effort: it holds a copy of what the store already
    /// has, so a leftover directory is clutter, never a reason to refuse the
    /// approval the reviewer just gave.
    pub fn approve_plan(&self, active: &mut ActivePlan) -> Result<(), OrchestratorError> {
        // Pure legality first — nothing is dropped for an illegal approve.
        // Stage docs deliberately do NOT gate the coarse approve: per-stage
        // review is progressive (later docs keep getting approved/revised
        // while an earlier stage builds); the dispatch seams re-gate each doc
        // at the moment its build session would spawn.
        plan_transition(&active.plan.state, PlanEvent::Approve)?;
        self.discard_plan_docs_dir(active);
        active.plan.apply(PlanEvent::Approve)?;
        active.last_error = None;
        Ok(())
    }
    /// Drop a plan's scratch docs dir and forget the workspace. Best-effort by
    /// design — the canonical docs are in the store.
    pub(super) fn discard_plan_docs_dir(&self, active: &mut ActivePlan) {
        if let Some(workspace) = active.workspace.take() {
            if workspace.docs_dir.exists() {
                if let Err(cleanup) = std::fs::remove_dir_all(&workspace.docs_dir) {
                    eprintln!(
                        "plan {}: removing the scratch docs dir {} failed: {cleanup}",
                        active.plan.id.0,
                        workspace.docs_dir.display()
                    );
                }
            }
        }
    }
    /// Submit a batch of plan notes: re-plan against them and hand the caller
    /// the turn to deliver. An issue hosts exactly one agent, so the notes
    /// reach the process the reviewer has been reading, never a replacement.
    /// The workspace is kept through the notes loop — this is handed the one
    /// [`prepare_plan_workspace`](Self::prepare_plan_workspace) just made.
    pub fn open_plan_notes(
        &self,
        active: &mut ActivePlan,
        workspace: PlanWorkspace,
        notes: &str,
    ) -> Result<AgentTurn, OrchestratorError> {
        active.plan.apply(PlanEvent::SendNotes)?;
        active.workspace = Some(workspace);
        active.last_error = None;
        let prompt = self.render_plan(&self.templates.revise, active, notes);
        Ok(AgentTurn::posted(prompt, notes, "revise"))
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
    pub fn open_plan_stage_notes(
        &self,
        active: &mut ActivePlan,
        workspace: PlanWorkspace,
        stage_id: &str,
    ) -> Result<AgentTurn, OrchestratorError> {
        let index = gate_plan_stage_notes(active, stage_id)?;
        active.plan.apply(PlanEvent::SendNotes)?;
        active.workspace = Some(workspace);
        active.revising_stage_id = Some(stage_id.to_string());
        active.last_error = None;
        let prompt = self.render_plan_stage(
            &self.templates.revise_stage,
            active,
            index,
            NEW_THREAD_MESSAGES_PROMPT,
        );
        Ok(AgentTurn::posted(
            prompt,
            NEW_THREAD_MESSAGES_PROMPT,
            "revise",
        ))
    }
    /// A freeform human message to the plan's agent (the plan-side `message`).
    /// A live `Drafting` session is redirected; parked states (blocked / failed
    /// / idle / interrupted) resume drafting with the message as the steer. The
    /// review gate is refused — `PlanReview` has the structured send-notes
    /// verb, and a freeform channel that moved the plan back to drafting would
    /// bypass the batched-review contract (`thread.post` is how you reach a
    /// plan agent at its gate without moving anything).
    pub fn open_plan_message(
        &self,
        active: &mut ActivePlan,
        workspace: PlanWorkspace,
        message: &str,
    ) -> Result<AgentTurn, OrchestratorError> {
        let event = gate_plan_message(active, message)?;
        // An interrupted plan lost its workspace; this is the one that was
        // re-made for it, docs and all.
        active.workspace = Some(workspace);
        let prompt = self.render_plan(&self.templates.message, active, message);
        if let Some(event) = event {
            active.plan.apply(event)?;
        }
        active.last_error = None;
        Ok(AgentTurn::posted(prompt, message, "message"))
    }
    /// Re-dispatch an interrupted plan phase in a fresh session (the plan-side
    /// `resume`). The plan machine has a single working phase, so the only
    /// routing is whether a per-stage revision was in flight
    /// (`revising_stage_id` → `revise_stage` with the stage's open comments) or
    /// a full (re-)plan. The prompt is routed BEFORE the `Reply` transition
    /// commits, so a routing failure never strands the plan out of its
    /// interrupted state (the caller persists it even on Err).
    pub fn open_plan_resume(
        &self,
        active: &mut ActivePlan,
        workspace: PlanWorkspace,
    ) -> Result<AgentTurn, OrchestratorError> {
        plan_transition(&active.plan.state, PlanEvent::Reply)?;
        active.workspace = Some(workspace);
        let prompt = match active.revising_stage_id.clone() {
            Some(stage_id) => {
                let index = active
                    .stage_doc_index(&stage_id)
                    .map_err(OrchestratorError::Gate)?;
                self.render_plan_stage(
                    &self.templates.revise_stage,
                    active,
                    index,
                    NEW_THREAD_MESSAGES_PROMPT,
                )
            }
            None => self.render_plan(&self.templates.plan, active, ""),
        };
        active.plan.apply(PlanEvent::Reply)?;
        active.last_error = None;
        Ok(AgentTurn::dispatched(prompt, "revise"))
    }
    /// Abandon a plan from any non-terminal state: kill the plan agent, mark
    /// the plan `Abandoned`, and drop its scratch docs dir. The cleanup is
    /// best-effort — a leftover directory is logged, never a reason to fail the
    /// abandon; the store docs are canonical and survive either way.
    pub fn abandon_plan(&self, active: &mut ActivePlan) -> Result<(), OrchestratorError> {
        active.plan.apply(PlanEvent::Abandon)?;
        self.discard_plan_docs_dir(active);
        Ok(())
    }
    /// Render a stage-scoped template for a plan (the plan-side twin of
    /// [`render_run_stage`](Self::render_run_stage)): the stage doc's own fields
    /// plus the next stage's doc path. The run-side variables (start sha,
    /// validation findings, prior-stage notes) are all empty — they belong to a
    /// run's execution progress, not a plan's doc review.
    pub(super) fn render_plan_stage(
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
        let rendered = templates::render(
            template,
            &Vars {
                goal: &active.plan.goal,
                plan_path: &active.plan_path,
                docs_dir: &plan_docs_dir_display(active),
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
                ..Vars::default()
            },
        );
        append_stage_catalog(rendered, &active.stages, |_| "not started".to_string())
    }
    /// Where the checkout for `slug` will go if nothing is in its way. The
    /// decide phase of a create has no directory to hash an id out of yet, and
    /// this is the path it expects one at.
    pub fn planned_checkout_path(&self, slug: &str) -> PathBuf {
        self.worktrees.path_for(slug)
    }
    /// Materialize a plan's canonical docs into a fresh run worktree and
    /// commit them, returning the commit sha that baselines the run's review
    /// diff.
    pub(super) fn materialize_and_commit_plan_docs(
        &self,
        plan_id: &str,
        checkout: &Path,
        goal: &str,
        store: &Store,
    ) -> Result<String, OrchestratorError> {
        store.materialize_plan_docs(plan_id, checkout)?;
        self.commit_all_with_message(checkout, &format!("plan: {goal}"))?;
        Ok(run_git(checkout, &["rev-parse", "HEAD"])?
            .trim()
            .to_string())
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
    ) -> Result<AgentTurn, OrchestratorError> {
        if active.run.state != RunState::StageGate {
            return Err(OrchestratorError::Gate(format!(
                "stage-doc revisions run from the stage gate (run is {:?})",
                active.run.state
            )));
        }
        let doc_index = plan
            .stage_doc_index(stage_id)
            .map_err(OrchestratorError::Gate)?;
        let open = plan.open_comments_for(stage_id);
        if open.is_empty() {
            return Err(OrchestratorError::Gate(format!(
                "no open comments on stage {stage_id}"
            )));
        }
        active.revising_stage_id = Some(stage_id.to_string());
        active.last_error = None;
        let prompt = self.render_run_stage(
            &self.templates.revise_stage,
            active,
            &plan.stages,
            doc_index,
            NEW_THREAD_MESSAGES_PROMPT,
        );
        Ok(AgentTurn::posted(
            prompt,
            NEW_THREAD_MESSAGES_PROMPT,
            "revise",
        ))
    }
    pub(super) fn render_plan(
        &self,
        template: &str,
        active: &ActivePlan,
        comments: &str,
    ) -> String {
        let rendered = templates::render(
            template,
            &Vars {
                goal: &active.plan.goal,
                plan_path: &active.plan_path,
                docs_dir: &plan_docs_dir_display(active),
                comments,
                base_branch: &active.base_branch,
                ..Vars::default()
            },
        );
        append_stage_catalog(rendered, &active.stages, |_| "not started".to_string())
    }
}
