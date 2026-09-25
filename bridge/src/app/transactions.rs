use crate::app::{locate_conversations, plan_state_str, run_state_str, AppState, DigestScope};
use crate::orchestrator::{ActivePlan, ActiveRun};
use crate::store::{now_rfc3339, PersistedPlan, PersistedRun, Store};
use crate::thread::ThreadDetail;
use serde_json::Value;

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
        if self.store.is_none() {
            self.operation_ledger.remember_in_memory_acceptance(plan_id);
            return Ok(());
        }
        let now = now_rfc3339();
        self.board
            .attention_mut()
            .record_created_if_absent(plan_id, now.clone());
        let clock = self.board.attention().clock(plan_id);
        let created_at = clock.created_at.expect("created clock was seeded");
        let updated_at = clock.updated_at.unwrap_or(now);
        let project_path = self.project_path_for(plan_id);
        let record = PersistedPlan {
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
        };
        let acceptance = self.operation_ledger.consume_acceptance_for(plan_id);
        match acceptance.as_ref() {
            Some(acceptance) => self
                .store
                .as_ref()
                .expect("checked above")
                .save_issue_plan_accepting_operation(&record, &acceptance.receipt)
                .map(Some),
            None => self
                .store
                .as_ref()
                .expect("checked above")
                .save_issue_plan(&record)
                .map(|()| None),
        }
        .map_err(|e| format!("issue store: {e}"))?;
        Ok(())
    }

    /// Write a run's durable core to the store (atomic replace). Same discipline
    /// as [`persist_plan_record`](Self::persist_plan_record); a run stores its
    /// `plan_id`, never plan docs.
    pub(in crate::app) fn persist_run_record(
        &mut self,
        run_id: &str,
        active: &ActiveRun,
    ) -> Result<(), String> {
        if self.store.is_none() {
            self.operation_ledger.remember_in_memory_acceptance(run_id);
            self.note_run_messages(run_id, active);
            return Ok(());
        }
        let now = now_rfc3339();
        self.board
            .attention_mut()
            .record_created_if_absent(run_id, now.clone());
        let clock = self.board.attention().clock(run_id);
        let created_at = clock.created_at.expect("created clock was seeded");
        let updated_at = clock.updated_at.unwrap_or(now);
        let project_path = self.project_path_for(run_id);
        let record = PersistedRun {
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
            last_summary: active.last_summary.clone(),
            last_error: active.last_error.clone(),
            created_at,
            updated_at,
            state_changed_at: clock.state_changed_at,
        };
        // One write path, whether or not the run belongs to an Issue: the
        // `issue_id` column is `record.plan_id`, so asking the store whether
        // the Issue exists first only bought a lock acquisition per save.
        let acceptance = self.operation_ledger.consume_acceptance_for(run_id);
        match acceptance.as_ref() {
            Some(acceptance) => self
                .store
                .as_ref()
                .expect("checked above")
                .save_run_accepting_operation(&record, &acceptance.receipt)
                .map(Some),
            None => self
                .store
                .as_ref()
                .expect("checked above")
                .save_run(&record)
                .map(|()| None),
        }
        .map_err(|e| format!("run store: {e}"))?;
        self.note_run_messages(run_id, active);
        Ok(())
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
        self.board
            .attention_mut()
            .record_created_if_absent(&plan_id, now.clone());
        self.board
            .attention_mut()
            .record_updated(&plan_id, now.clone());
        self.stamp_state_change(&plan_id, plan_state_str(&active.plan.state), now);
        let persisted = self.persist_plan_record(&plan_id, &active);
        let news = self.conversation_news(active.agents.sole_thread());
        let state_kind = crate::notify::kind_for_plan_state(&active.plan.state);
        self.push_attention_notify(&plan_id, news, state_kind);
        self.plans.insert(plan_id.clone(), active);
        // After the insert: the attention file is pruned to what exists when it
        // is written, and an anchor stamped while the record was checked out
        // would be dropped on the way to disk.
        self.seed_anchor(&plan_id);
        self.reap_orphaned_terminals();
        // Every plan mutation ends here — an RPC's, an agent's `done`, a
        // delivery failure — so this is the one place that has to tell the
        // browsers, whatever started it. And the live roster, for the same
        // reason: a turn starting or stopping is a mutation like any other.
        self.note_entity_changed(&plan_id);
        self.publish_live_roster_for(&plan_id);
        persisted
    }

    pub(in crate::app) fn answer_plan_mutation(
        &mut self,
        plan_id: String,
        active: ActivePlan,
        thread_detail: ThreadDetail,
    ) -> (Value, Result<(), String>) {
        let settled = self.finish_plan_mutation(plan_id.clone(), active);
        let active = self.plans.get(&plan_id).expect("the finish put it back");
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
        let now = now_rfc3339();
        self.board
            .attention_mut()
            .record_created_if_absent(&run_id, now.clone());
        self.board
            .attention_mut()
            .record_updated(&run_id, now.clone());
        self.stamp_state_change(&run_id, run_state_str(&active.run.state), now);
        // The mutation likely changed the tree; drop the cached diffstat.
        self.invalidate_run_stat(&run_id);
        let persisted = self.persist_run_record(&run_id, &active);
        let news = self
            .conversation_thread_for_run(&active)
            .map(|thread| self.conversation_news(thread));
        let state_kind = crate::notify::kind_for_run_state(&active.run.state);
        if let Some(news) = news {
            self.push_attention_notify(&run_id, news, state_kind);
        }
        self.runs.insert(run_id.clone(), active);
        // See `finish_plan_mutation`: seeded once the record is back in its map.
        self.seed_anchor(&run_id);
        self.reap_orphaned_terminals();
        self.note_entity_changed(&run_id);
        self.publish_live_roster_for(&run_id);
        persisted
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
        let active = self.runs.get(&run_id).expect("the finish put it back");
        (
            self.run_view(&run_id, active, thread_detail, DigestScope::Detail),
            settled,
        )
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
    /// checkout they are about: an Issue's is the checkout of the
    /// implementation working it right now, or the primary checkout its own
    /// agent runs in when nothing is implementing it yet.
    pub(in crate::app) fn take_plan(&mut self, plan_id: &str) -> Result<ActivePlan, String> {
        let implementation_checkout = self
            .current_issue_implementation(plan_id)
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
