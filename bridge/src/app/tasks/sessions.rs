use crate::app::WorktreeLifecycleJob;
use crate::app::{
    append_plan_stage_announcements, attach_plan_operation_turn, err, record_report_in_thread,
    with_post_receipt, AppState, PendingAgentTurn, PlanSessionOpening,
};
use crate::lifecycle::{OpenPlanWorkspace, PendingRow};
use crate::mcp::{DoneReport, DoneStatus};
use crate::operation::OperationReceipt;
use crate::orchestrator::{ActivePlan, AgentTurn};
use crate::plan::StageDoc;
use crate::store::now_rfc3339;
use crate::thread::ThreadDetail;
use serde_json::Value;

/// The first message to an inert Task asked: the session it never had, and the
/// Task's own view — with the sequence the message landed at, which is what
/// the composer is waiting for.
pub(in crate::app) struct PlanDraftingStarted {
    pub(in crate::app) task_id: String,
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
        match state.open_inert_plan_drafting(&self.task_id, workspace, self.detail) {
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

impl AppState {
    // ---- Plan surface ---------------------------------------------------------

    /// Reserve the workspace an inert Task's first planning session needs.
    ///
    /// `Ok(None)` is "there is nothing to start": the session is already
    /// running, or another caller's spawn is already on its way to the same
    /// checkout. `Err` is "no session can start here at all" — the caller
    /// decides whether that is fatal, because a routed capture keeps its
    /// destination either way.
    ///
    /// The Task must be in its map: this reads the record it is about to
    /// reserve a row for.
    pub(in crate::app) fn reserve_plan_drafting(
        &mut self,
        task_id: &str,
        opening: Box<dyn PlanSessionOpening>,
    ) -> Result<Option<WorktreeLifecycleJob>, String> {
        let active = self
            .plans
            .get(task_id)
            .ok_or_else(|| format!("unknown task_id: {task_id}"))?;
        // Already has a session: this is a first turn, not a nudge, and a
        // second harness in the same checkout would report `done` twice.
        if active.workspace.is_some() {
            return Ok(None);
        }
        crate::plan::plan_transition(&active.plan.state, crate::plan::PlanEvent::Dispatch)
            .map_err(|error| error.to_string())?;
        let title = active.plan.goal.clone();
        let agent_id = active.agents.sole().id.clone();
        let checkout = self.project_repository_of(task_id)?;
        if self.agent_is_on_its_way(&checkout, &agent_id) {
            return Ok(None);
        }
        let project_id = self.project_of(task_id)?;
        self.reserve_plan_workspace(task_id, project_id, title, opening)
            .map(Some)
    }

    /// Reserve the workspace one door to a Task's planning agent needs, and
    /// build the job that writes it. The row stands on the Task itself: what
    /// it holds is the one workspace every door writes into, so a second door
    /// waits rather than racing this one's `.build/` config.
    ///
    /// The project comes from the caller: a Task being created is not in
    /// `entity_project` until its epilogue runs, and `plan.create` reserves
    /// through here like every other door.
    pub(in crate::app) fn reserve_plan_workspace(
        &mut self,
        task_id: &str,
        project_id: String,
        title: String,
        opening: Box<dyn PlanSessionOpening>,
    ) -> Result<WorktreeLifecycleJob, String> {
        let project = self.orch_for(&project_id)?.clone();
        let store = self.require_store()?.clone();
        // A plan cuts no branch and claims no checkout: it is written against
        // the primary one, so nothing else can collide with it.
        let row = PendingRow::creating(task_id.to_string(), Some(project_id), title);
        self.reserve_lifecycle(
            row,
            OpenPlanWorkspace {
                project,
                plan_id: task_id.to_string(),
                store,
            },
            crate::app::runtime::lifecycle::PlanWorkspaceSettlement { opening },
        )
    }

    /// Start the planning session an inert Task has never had, now that its
    /// workspace is on disk: the dispatch reads everything said to it so far,
    /// and the turn that spawns the session is queued.
    pub(in crate::app) fn open_inert_plan_drafting(
        &mut self,
        task_id: &str,
        workspace: crate::orchestrator::PlanWorkspace,
        detail: ThreadDetail,
    ) -> Result<Value, String> {
        let project_id = self.project_of(task_id)?;
        self.settle_plan_session(task_id, detail, |state, active| {
            let turn = state
                .orch_for(&project_id)?
                .open_plan_drafting(active, workspace)
                .map_err(err)?;
            state.queue_plan_turn(task_id, active, turn);
            if state.qa_agent {
                state.qa_simulate_plan(&project_id, active)?;
            }
            Ok(())
        })
    }

    /// The tail every door to a planning agent shares: take the Task's record
    /// out, open the session its workspace was written for, put the record back
    /// and answer with it. What differs is the middle, which is the door's own.
    ///
    /// The record is persisted either way — a session that could not open
    /// leaves the Task as it was, with what was said still on its thread, and
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
    pub(in crate::app) fn on_plan_agent_done(
        &mut self,
        plan_id: &str,
        reporting_agent_id: Option<&str>,
        report: DoneReport,
    ) {
        let Some(mut active) = self.plans.remove(plan_id) else {
            return;
        };
        let previous_stage_ids: Vec<String> =
            active.stages.iter().map(|stage| stage.id.clone()).collect();
        let was_revising_stage = active.revising_stage_id.is_some();
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
            && !was_revising_stage
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
        let conversation = match reporting_agent_id {
            None => active.agents.sole_thread_mut(),
            Some(reporting_agent_id) => {
                let conversation_id = active
                    .agents
                    .by_id(reporting_agent_id)
                    .expect("authenticated reporting agent belongs to the Task")
                    .conversation_id()
                    .to_string();
                &mut active
                    .agents
                    .by_id_mut(&conversation_id)
                    .expect("Task reporting agent's canonical conversation exists")
                    .thread
            }
        };
        record_report_in_thread(
            conversation,
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
