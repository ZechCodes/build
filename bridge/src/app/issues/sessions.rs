use crate::app::{
    append_plan_stage_announcements, attach_plan_operation_turn, err, model_choice_from,
    record_report_in_thread, require_str, thread_detail, with_post_receipt, AppState,
    PendingAgentTurn, PlanSessionOpening,
};
use crate::lifecycle::{LifecycleEpilogue, OpenPlanWorkspace, PendingRow, WorktreeLifecycleJob};
use crate::mcp::{DonePhase, DoneReport, DoneStatus};
use crate::models::ModelChoice;
use crate::operation::OperationReceipt;
use crate::orchestrator::{ActivePlan, AgentTurn};
use crate::plan::{PlanId, StageDoc};
use crate::store::now_rfc3339;
use crate::thread::ThreadDetail;
use serde_json::Value;

/// `plan.create` asked: the Issue's record, its first turn, and the view the
/// caller wanted.
pub(in crate::app) struct IssueOpened {
    pub(in crate::app) project_id: String,
    pub(in crate::app) plan_id: String,
    pub(in crate::app) goal: String,
    pub(in crate::app) base_branch: String,
    pub(in crate::app) model_choice: ModelChoice,
    pub(in crate::app) detail: ThreadDetail,
}

impl PlanSessionOpening for IssueOpened {
    fn open(
        self: Box<Self>,
        state: &mut AppState,
        workspace: crate::orchestrator::PlanWorkspace,
    ) -> Result<Value, String> {
        state.open_planned_issue(*self, workspace)
    }
}

/// The first message to an inert Issue asked: the session it never had, and the
/// Issue's own view — with the sequence the message landed at, which is what
/// the composer is waiting for.
pub(in crate::app) struct PlanDraftingStarted {
    pub(in crate::app) issue_id: String,
    pub(in crate::app) detail: ThreadDetail,
    pub(in crate::app) posted_sequence: Option<u64>,
    pub(in crate::app) receipt: Option<OperationReceipt>,
}

impl PlanSessionOpening for PlanDraftingStarted {
    fn open(
        self: Box<Self>,
        state: &mut AppState,
        workspace: crate::orchestrator::PlanWorkspace,
    ) -> Result<Value, String> {
        match state.open_inert_plan_drafting(&self.issue_id, workspace, self.detail) {
            Ok(view) => {
                if let Some(receipt) = self.receipt.as_ref() {
                    if let Err(error) = attach_plan_operation_turn(state, receipt) {
                        return Ok(state.settle_accepted_operation_error(receipt, error));
                    }
                }
                Ok(with_post_receipt(
                    view,
                    self.posted_sequence,
                    self.receipt.as_ref(),
                ))
            }
            Err(error) => match self.receipt.as_ref() {
                Some(receipt) => Ok(state.settle_accepted_operation_error(receipt, error)),
                None => Err(error),
            },
        }
    }

    fn refused(self: Box<Self>, state: &mut AppState, error: String) -> Result<Value, String> {
        match self.receipt.as_ref() {
            Some(receipt) => Ok(state.settle_accepted_operation_error(receipt, error)),
            None => Err(error),
        }
    }
}

/// The planning workspace is written; what is left is the door that asked for
/// it.
pub struct PlanWorkspaceOpened {
    pub workspace: crate::orchestrator::PlanWorkspace,
    pub opening: Box<dyn PlanSessionOpening>,
}

impl LifecycleEpilogue for PlanWorkspaceOpened {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        self.opening.open(state, self.workspace)
    }
}

/// The planning workspace could not be written. What that leaves behind is the
/// door's own business, so it comes back as an epilogue rather than an error.
pub struct PlanWorkspaceRefused {
    pub error: String,
    pub opening: Box<dyn PlanSessionOpening>,
}

impl LifecycleEpilogue for PlanWorkspaceRefused {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        self.opening.refused(state, self.error)
    }
}

impl AppState {
    // ---- Plan surface ---------------------------------------------------------

    /// Author a new plan: open a planning workspace (the primary checkout plus
    /// a scratch docs dir) and a plan agent session (the docs land canonically
    /// in the store on `done`).
    ///
    /// `dispatch: false` files the record and starts nothing — an inert issue,
    /// which is what the router and the toolbar's New issue create. The first
    /// `thread.post` to it starts the planning session.
    pub(in crate::app) fn plan_create(&mut self, params: &Value) -> Result<Value, String> {
        let goal = require_str(params, "goal")?;
        let project_id = match params.get("project_id").and_then(Value::as_str) {
            Some(p) => p.to_string(),
            None => self.default_project()?,
        };
        let base = self.base_for(&project_id)?;
        let model_choice = model_choice_from(params, self.default_harness)?;
        self.require_store()?;
        let plan_id = format!("plan-{}", uuid::Uuid::new_v4());
        if !params
            .get("dispatch")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            let active = self.orch_for(&project_id)?.create_plan(
                PlanId::new(&plan_id),
                goal,
                &base,
                model_choice,
            );
            self.projects
                .bind_entity(plan_id.clone(), project_id.clone());
            let (view, persisted) =
                self.answer_plan_mutation(plan_id, active, thread_detail(params));
            persisted?;
            return Ok(view);
        }
        let job = self.reserve_plan_workspace(
            &plan_id,
            project_id.clone(),
            goal.clone(),
            Box::new(IssueOpened {
                project_id,
                plan_id: plan_id.clone(),
                goal,
                base_branch: base,
                model_choice,
                detail: thread_detail(params),
            }),
        )?;
        Ok(self.defer_job(job))
    }

    /// `plan.create`'s apply half: the Issue's record, its first turn, and the
    /// view the caller asked for. The workspace its agent works in is on disk
    /// by now, which is why nothing here can fail on a directory.
    ///
    /// What a failure here leaves is the workspace: a scratch docs dir for an
    /// Issue that never opened, and the `.build/` config the next plan this
    /// project drafts overwrites. Removing either is filesystem work, which an
    /// epilogue may not do; neither is a checkout or a branch, so no board is
    /// missing anything.
    pub(in crate::app) fn open_planned_issue(
        &mut self,
        opened: IssueOpened,
        workspace: crate::orchestrator::PlanWorkspace,
    ) -> Result<Value, String> {
        let IssueOpened {
            project_id,
            plan_id,
            goal,
            base_branch,
            model_choice,
            detail,
        } = opened;
        let project = self.orch_for(&project_id)?.clone();
        let mut active =
            project.create_plan(PlanId::new(&plan_id), goal, &base_branch, model_choice);
        let turn = project
            .open_plan_drafting(&mut active, workspace)
            .map_err(err)?;
        self.projects
            .bind_entity(plan_id.clone(), project_id.clone());
        self.queue_plan_turn(&plan_id, &active, turn);
        if self.qa_agent {
            self.qa_simulate_plan(&project_id, &mut active)?;
        }
        let (view, persisted) = self.answer_plan_mutation(plan_id, active, detail);
        persisted?;
        Ok(view)
    }

    /// Reserve the workspace an inert Issue's first planning session needs.
    ///
    /// `Ok(None)` is "there is nothing to start": the session is already
    /// running, or another caller's spawn is already on its way to the same
    /// checkout. `Err` is "no session can start here at all" — the caller
    /// decides whether that is fatal, because a routed capture keeps its
    /// destination either way.
    ///
    /// The Issue must be in its map: this reads the record it is about to
    /// reserve a row for.
    pub(in crate::app) fn reserve_plan_drafting(
        &mut self,
        issue_id: &str,
        opening: Box<dyn PlanSessionOpening>,
    ) -> Result<Option<WorktreeLifecycleJob>, String> {
        let active = self
            .plans
            .get(issue_id)
            .ok_or_else(|| format!("unknown issue_id: {issue_id}"))?;
        // Already has a session: this is a first turn, not a nudge, and a
        // second harness in the same checkout would report `done` twice.
        if active.workspace.is_some() {
            return Ok(None);
        }
        crate::plan::plan_transition(&active.plan.state, crate::plan::PlanEvent::Dispatch)
            .map_err(|error| error.to_string())?;
        let title = active.plan.goal.clone();
        let agent_id = active.agents.sole().id.clone();
        let checkout = self.primary_checkout_of(issue_id)?;
        if self.agent_is_on_its_way(&checkout, &agent_id) {
            return Ok(None);
        }
        let project_id = self.project_of(issue_id)?;
        self.reserve_plan_workspace(issue_id, project_id, title, opening)
            .map(Some)
    }

    /// Reserve the workspace one door to an Issue's planning agent needs, and
    /// build the job that writes it. The row stands on the Issue itself: what
    /// it holds is the one workspace every door writes into, so a second door
    /// waits rather than racing this one's `.build/` config.
    ///
    /// The project comes from the caller: an Issue being created is not in
    /// `entity_project` until its epilogue runs, and `plan.create` reserves
    /// through here like every other door.
    pub(in crate::app) fn reserve_plan_workspace(
        &mut self,
        issue_id: &str,
        project_id: String,
        title: String,
        opening: Box<dyn PlanSessionOpening>,
    ) -> Result<WorktreeLifecycleJob, String> {
        let project = self.orch_for(&project_id)?.clone();
        let store = self.require_store()?.clone();
        // A plan cuts no branch and claims no checkout: it is written against
        // the primary one, so nothing else can collide with it.
        let row = PendingRow::creating(issue_id.to_string(), Some(project_id), title);
        self.reserve_lifecycle(
            row,
            Box::new(OpenPlanWorkspace {
                project,
                plan_id: issue_id.to_string(),
                store,
                opening,
            }),
        )
    }

    /// Start the planning session an inert Issue has never had, now that its
    /// workspace is on disk: the dispatch reads everything said to it so far,
    /// and the turn that spawns the session is queued.
    pub(in crate::app) fn open_inert_plan_drafting(
        &mut self,
        issue_id: &str,
        workspace: crate::orchestrator::PlanWorkspace,
        detail: ThreadDetail,
    ) -> Result<Value, String> {
        let project_id = self.project_of(issue_id)?;
        self.settle_plan_session(issue_id, detail, |state, active| {
            let turn = state
                .orch_for(&project_id)?
                .open_plan_drafting(active, workspace)
                .map_err(err)?;
            state.queue_plan_turn(issue_id, active, turn);
            if state.qa_agent {
                state.qa_simulate_plan(&project_id, active)?;
            }
            Ok(())
        })
    }

    /// The tail every door to a planning agent shares: take the Issue's record
    /// out, open the session its workspace was written for, put the record back
    /// and answer with it. What differs is the middle, which is the door's own.
    ///
    /// The record is persisted either way — a session that could not open
    /// leaves the Issue as it was, with what was said still on its thread, and
    /// the error is what the caller hears.
    pub(in crate::app) fn settle_plan_session(
        &mut self,
        plan_id: &str,
        detail: ThreadDetail,
        open: impl FnOnce(&mut AppState, &mut ActivePlan) -> Result<(), String>,
    ) -> Result<Value, String> {
        let mut active = self.take_plan(plan_id)?;
        let opened = open(self, &mut active);
        let (view, persisted) = self.answer_plan_mutation(plan_id.to_string(), active, detail);
        opened?;
        persisted?;
        Ok(view)
    }

    /// A plan agent reported `done`: ingest + advance on the plan's orchestrator.
    pub(in crate::app) fn on_plan_agent_done(&mut self, plan_id: &str, report: DoneReport) {
        let Some(mut active) = self.plans.remove(plan_id) else {
            return;
        };
        let previous_stage_ids: Vec<String> =
            active.stages.iter().map(|stage| stage.id.clone()).collect();
        let report_for_thread = report.clone();
        let outcome = (|| -> Result<(), String> {
            let project_id = self.project_of(plan_id)?;
            let store = self.require_store()?;
            self.orch_for(&project_id)?
                .on_plan_done(&mut active, store, report)
                .map_err(err)
        })();
        if let Err(e) = &outcome {
            eprintln!("on_agent_done {plan_id}: {e}");
        }
        if outcome.is_ok()
            && report_for_thread.phase == DonePhase::Plan
            && report_for_thread.status == DoneStatus::Completed
        {
            let new_stages: Vec<(usize, StageDoc)> = active
                .stages
                .iter()
                .enumerate()
                .filter(|(_, stage)| !previous_stage_ids.contains(&stage.id))
                .map(|(index, stage)| (index, stage.clone()))
                .collect();
            append_plan_stage_announcements(active.agents.sole_thread_mut(), plan_id, &new_stages);
        }
        record_report_in_thread(
            active.agents.sole_thread_mut(),
            &report_for_thread,
            outcome.as_ref().err().map(String::as_str),
        );
        if outcome.is_ok() && report_for_thread.status == DoneStatus::Completed {
            if let Some(contents) = self.plan_revision_contents(plan_id, &active) {
                active.agents.sole_thread_mut().add_revision(
                    crate::thread::ArtifactKind::Plan,
                    &contents,
                    &now_rfc3339(),
                );
            }
        }
        let persisted = self.finish_plan_mutation(plan_id.to_string(), active);
        if let Err(e) = persisted {
            eprintln!("on_agent_done {plan_id}: {e}");
        }
    }

    /// Queue a plan's turn for its agent in the primary checkout. A plan whose
    /// workspace is gone has no agent to hear it; the turn is dropped rather
    /// than delivered somewhere it does not belong.
    pub(in crate::app) fn queue_plan_turn(
        &mut self,
        plan_id: &str,
        active: &ActivePlan,
        turn: AgentTurn,
    ) {
        if let Some(pending) = PendingAgentTurn::for_plan(plan_id, active, turn) {
            self.delivery_queue.enqueue(pending);
        }
    }
}
