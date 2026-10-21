use super::board_index::EntityClock;
use crate::app::{locate_conversations, plan_state_str, run_state_str, AppState, DigestScope};
use crate::orchestrator::{ActivePlan, ActiveRun};
use crate::store::{now_rfc3339, PersistedPlan, PersistedRun, Store};
use crate::thread::ThreadDetail;

mod committed;
mod notices;
mod staging;
pub(in crate::app) use committed::CommittedEntities;
use serde_json::Value;

/// Adoption installs its compatibility workspace in the same publication as
/// the durable owner. Ordinary mutations leave registry maintenance alone.
enum RunMutationKind {
    Existing,
    Adoption,
}

impl AppState {
    /// Write a plan's durable core to the store (atomic replace). A no-op
    /// without a configured store (unit tests); an error surfaces to the caller
    /// — a plan the store cannot hold would silently vanish on the next
    /// restart. Docs are NOT snapshotted here: the store copy is canonical and
    /// written transactionally at each plan/revise `done`.
    pub(in crate::app) fn persist_plan_record(
        &mut self,
        plan_id: &str,
        active: &ActivePlan,
    ) -> Result<(), String> {
        let clock = self.board.attention().clock(plan_id);
        self.persist_plan_record_at(plan_id, active, clock)
    }

    fn persist_plan_record_at(
        &mut self,
        plan_id: &str,
        active: &ActivePlan,
        clock: EntityClock,
    ) -> Result<(), String> {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("finisher.save");
        let Some(store) = self.store.as_ref() else {
            self.operation_ledger.remember_in_memory_acceptance(plan_id);
            return Ok(());
        };
        let record = self.plan_record(plan_id, active, clock);
        let acceptance = self.operation_ledger.consume_acceptance_for(plan_id);
        match acceptance.as_ref() {
            Some(acceptance) => store
                .save_task_plan_accepting_operation(&record, &acceptance.receipt)
                .map(|_| ()),
            None => store.save_task_plan(&record),
        }
        .map_err(|e| format!("task store: {e}"))?;
        self.board
            .attention_mut()
            .record_created_if_absent(plan_id, record.created_at.clone());
        self.committed_entities.remember_plan(record);
        Ok(())
    }

    fn plan_record(&self, plan_id: &str, active: &ActivePlan, clock: EntityClock) -> PersistedPlan {
        let now = now_rfc3339();
        let created_at = clock.created_at.unwrap_or_else(|| now.clone());
        let updated_at = clock.updated_at.unwrap_or(now);
        let project_path = self.project_path_for(plan_id);
        PersistedPlan {
            id: plan_id.to_string(),
            goal: active.plan.goal.clone(),
            project_path,
            base_branch: active.base_branch.clone(),
            state: active.plan.state,
            archived_at: active.plan.archived_at.clone(),
            implementation_intent: active.plan.implementation_intent.clone(),
            implementation_activity: active.plan.implementation_activity.clone(),
            plan_path: active.plan_path.clone(),
            stages: active.stages.clone(),
            provider: active.model_choice.provider,
            model: active.model_choice.model.clone(),
            effort: active.model_choice.effort.clone(),
            agents: active.agents.agents().to_vec(),
            legacy_thread: crate::thread::Thread::default(),
            last_summary: active.last_summary.clone(),
            last_error: active.last_error.clone(),
            created_at,
            updated_at,
            state_changed_at: clock.state_changed_at,
        }
    }

    /// Write a run's durable core to the store (atomic replace). Same discipline
    /// as [`persist_plan_record`](Self::persist_plan_record); a run stores its
    /// `plan_id`, never plan docs.
    pub(in crate::app) fn persist_run_record(
        &mut self,
        run_id: &str,
        active: &ActiveRun,
    ) -> Result<(), String> {
        let clock = self.board.attention().clock(run_id);
        self.persist_run_record_at(run_id, active, clock)
    }

    fn persist_run_record_at(
        &mut self,
        run_id: &str,
        active: &ActiveRun,
        clock: EntityClock,
    ) -> Result<(), String> {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("finisher.save");
        let Some(store) = self.store.as_ref() else {
            self.operation_ledger.remember_in_memory_acceptance(run_id);
            self.note_run_messages(run_id, active);
            return Ok(());
        };
        let record = self.run_record(run_id, active, clock);
        let acceptance = self.operation_ledger.consume_acceptance_for(run_id);
        match acceptance.as_ref() {
            Some(acceptance) => store
                .save_run_accepting_operation(&record, &acceptance.receipt)
                .map(|_| ()),
            None => store.save_run(&record),
        }
        .map_err(|e| format!("run store: {e}"))?;
        self.board
            .attention_mut()
            .record_created_if_absent(run_id, record.created_at.clone());
        self.committed_entities.remember_run(record);
        self.note_run_messages(run_id, active);
        Ok(())
    }

    fn run_record(&self, run_id: &str, active: &ActiveRun, clock: EntityClock) -> PersistedRun {
        let now = now_rfc3339();
        let created_at = clock.created_at.unwrap_or_else(|| now.clone());
        let updated_at = clock.updated_at.unwrap_or(now);
        let project_path = self.project_path_for(run_id);
        PersistedRun {
            id: run_id.to_string(),
            plan_id: active.run.plan_id.as_ref().map(|p| p.0.clone()),
            goal: active.run.goal.clone(),
            project_path,
            base_branch: active.worktree.base_branch.clone(),
            state: active.run.state,
            branch: active.worktree.branch(),
            worktree_name: active.worktree.name.clone(),
            worktree_path: active.worktree.path.display().to_string(),
            base_sha: active.base_sha.clone(),
            stages: active.stages.clone(),
            current_stage_id: active.current_stage_id.clone(),
            revising_stage_id: active.revising_stage_id.clone(),
            auto_advance: active.auto_advance,
            adopted: active.adopted,
            publication_attempt: active.publication_attempt.clone(),
            provider: active.model_choice.provider,
            model: active.model_choice.model.clone(),
            effort: active.model_choice.effort.clone(),
            agents: active.agents.agents().to_vec(),
            legacy_thread: crate::thread::Thread::default(),
            pinned_agent_id: active.pinned_agent_id.clone(),
            last_summary: active.last_summary.clone(),
            last_error: active.last_error.clone(),
            created_at,
            updated_at,
            state_changed_at: clock.state_changed_at,
        }
    }

    /// The shared tail of every plan mutation: stamp times, persist the durable
    /// core, throttle a notify, put the plan back in the map, and prompt
    /// terminal closure + pump start.
    pub(in crate::app) fn finish_plan_mutation(
        &mut self,
        plan_id: String,
        active: ActivePlan,
    ) -> Result<(), String> {
        let now = now_rfc3339();
        let state = plan_state_str(&active.plan.state);
        let clock = self
            .board
            .attention()
            .mutation_clock(&plan_id, &state, &now);
        #[cfg(test)]
        let _publication_stage = self.begin_publication_stage(&plan_id);
        let (active, projection) =
            self.prepare_plan_mutation_snapshot(&plan_id, active, &state, &now);
        if let Err(error) = self.persist_plan_record_at(&plan_id, &active, clock) {
            self.restore_plan_after_failed_save(&plan_id, active);
            self.settle_failed_mutation(&plan_id);
            return Err(error);
        }
        let attention_dirty = self.commit_mutation_clock(&plan_id, state, now);
        let news = self.roster_news(&active.agents);
        self.plans.insert(plan_id.clone(), active);
        self.settle_committed_mutation(plan_id, news, attention_dirty, projection);
        Ok(())
    }

    pub(in crate::app) fn answer_plan_mutation(
        &mut self,
        plan_id: String,
        active: ActivePlan,
        thread_detail: ThreadDetail,
    ) -> (Value, Result<(), String>) {
        let settled = self.finish_plan_mutation(plan_id.clone(), active);
        let Some(active) = self.plans.get(&plan_id) else {
            return (Value::Null, settled);
        };
        (
            self.plan_view(&plan_id, active, thread_detail, DigestScope::Detail),
            settled,
        )
    }

    /// The run-half twin of
    /// [`finish_plan_mutation`](Self::finish_plan_mutation).
    pub(in crate::app) fn finish_run_mutation(
        &mut self,
        run_id: String,
        active: ActiveRun,
    ) -> Result<(), String> {
        self.finish_run_mutation_kind(run_id, active, RunMutationKind::Existing)
    }

    pub(in crate::app) fn finish_adopted_run_mutation(
        &mut self,
        run_id: String,
        active: ActiveRun,
    ) -> Result<(), String> {
        self.finish_run_mutation_kind(run_id, active, RunMutationKind::Adoption)
    }

    fn finish_run_mutation_kind(
        &mut self,
        run_id: String,
        active: ActiveRun,
        kind: RunMutationKind,
    ) -> Result<(), String> {
        let staged_adoption = match kind {
            RunMutationKind::Existing => None,
            RunMutationKind::Adoption => self.stage_adoption_workspace(&run_id, &active),
        };
        let now = now_rfc3339();
        let state = run_state_str(&active.run.state);
        let clock = self.board.attention().mutation_clock(&run_id, &state, &now);
        #[cfg(test)]
        let _publication_stage = self.begin_publication_stage(&run_id);
        let (active, projection) =
            self.prepare_run_mutation_snapshot(&run_id, active, &state, &now);
        if let Err(error) = self.persist_run_record_at(&run_id, &active, clock) {
            // Restore the alias before this failure can publish any deferred
            // notices belonging to successful nested owner mutations.
            self.rollback_adoption_workspace(staged_adoption);
            self.restore_run_after_failed_save(&run_id, active);
            self.settle_failed_mutation(&run_id);
            return Err(error);
        }
        let attention_dirty = self.commit_mutation_clock(&run_id, state, now);
        self.invalidate_run_stat(&run_id);
        let news = self.roster_news(&active.agents);
        self.runs.insert(run_id.clone(), active);
        self.settle_committed_mutation(run_id, news, attention_dirty, projection);
        Ok(())
    }

    /// The run-half twin of
    /// [`answer_plan_mutation`](Self::answer_plan_mutation).
    pub(in crate::app) fn answer_run_mutation(
        &mut self,
        run_id: String,
        active: ActiveRun,
        thread_detail: ThreadDetail,
    ) -> (Value, Result<(), String>) {
        let settled = self.finish_run_mutation(run_id.clone(), active);
        self.run_mutation_answer(&run_id, thread_detail, settled)
    }

    /// Adoption's settlement boundary also owns its compatibility workspace.
    pub(in crate::app) fn answer_adopted_run_mutation(
        &mut self,
        run_id: String,
        active: ActiveRun,
        thread_detail: ThreadDetail,
    ) -> (Value, Result<(), String>) {
        let settled = self.finish_adopted_run_mutation(run_id.clone(), active);
        self.run_mutation_answer(&run_id, thread_detail, settled)
    }

    fn run_mutation_answer(
        &self,
        run_id: &str,
        thread_detail: ThreadDetail,
        settled: Result<(), String>,
    ) -> (Value, Result<(), String>) {
        let Some(active) = self.runs.get(run_id) else {
            return (Value::Null, settled);
        };
        (
            self.run_view(run_id, active, thread_detail, DigestScope::Detail),
            settled,
        )
    }

    fn commit_mutation_clock(&mut self, owner: &str, state: String, now: String) -> bool {
        let created = self
            .board
            .attention()
            .clock(owner)
            .created_at
            .unwrap_or_else(|| now.clone());
        self.board
            .attention_mut()
            .record_created_if_absent(owner, created.clone());
        self.board
            .attention_mut()
            .record_updated(owner, now.clone());
        self.stamp_state_change(owner, state, now);
        self.board.attention_mut().seed_anchor(owner, &created)
    }

    fn restore_plan_after_failed_save(&mut self, plan_id: &str, candidate: ActivePlan) {
        if let Some(record) = self.committed_entities.plan(plan_id) {
            let mut restored = ActivePlan::reattach(record);
            // Physical workspace cleanup may already have happened. Process
            // handles live in session_registry and are never rolled back.
            restored.workspace = candidate.workspace;
            // This routes the actual in-flight revision session, independently
            // of the canonical plan fields restored above.
            restored.revising_stage_id = candidate.revising_stage_id;
            self.plans.insert(plan_id.to_string(), restored);
        }
    }

    pub(in crate::app) fn restore_run_after_failed_save(
        &mut self,
        run_id: &str,
        candidate: ActiveRun,
    ) {
        if let Some(record) = self.committed_entities.run(run_id) {
            let restored = ActiveRun::reattach(record, candidate.plan_path);
            self.runs.insert(run_id.to_string(), restored);
        }
    }

    // ---- Store accessor + take/finish plumbing --------------------------------

    /// The durable store, or a clean error. Plans and runs both require one:
    /// plan docs are canonical in the store, and `prepare_run_checkout`
    /// writes/reads through it. Only unit tests that never create a plan/run skip it.
    pub(in crate::app) fn require_store(&self) -> Result<&Store, String> {
        self.store
            .as_ref()
            .ok_or_else(|| "no task store configured".to_string())
    }

    /// Take a plan out for mutation, having told its conversations which
    /// checkout they are about: a Task's is the checkout of the
    /// implementation working it right now, or the primary checkout its own
    /// agent runs in when nothing is implementing it yet.
    pub(in crate::app) fn take_plan(&mut self, plan_id: &str) -> Result<ActivePlan, String> {
        let implementation_checkout = self
            .current_task_implementation(plan_id)
            .map(|run| run.worktree.path.clone());
        let mut active = self
            .plans
            .remove(plan_id)
            .ok_or_else(|| "unknown plan_id".to_string())?;
        let checkout = implementation_checkout
            .or_else(|| active.workspace.as_ref().map(|w| w.checkout.clone()));
        if let Some(checkout) = checkout {
            locate_conversations(&mut active.agents, &checkout);
        }
        Ok(active)
    }

    pub(in crate::app) fn take_run(&mut self, run_id: &str) -> Result<ActiveRun, String> {
        let mut active = self
            .runs
            .remove(run_id)
            .ok_or_else(|| "unknown run_id".to_string())?;
        let checkout = active.worktree.path.clone();
        locate_conversations(&mut active.agents, &checkout);
        Ok(active)
    }
}
