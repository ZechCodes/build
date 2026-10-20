use super::notices::MutationNotice;
use crate::agent::Agent;
use crate::store::{PersistedPlan, PersistedRun, RESIDENT_CONVERSATION_TAIL};
use std::collections::{HashMap, HashSet};

/// Durable owner fields are independent of process lifetime. These records
/// contain no session/PTY handles. Reuse the owned SQL payload after commit and
/// keep only the same bounded conversation tail that boot recovery loads.
#[derive(Default)]
pub(in crate::app) struct CommittedEntities {
    plans: HashMap<String, PersistedPlan>,
    runs: HashMap<String, PersistedRun>,
    notices: Vec<MutationNotice>,
}

impl CommittedEntities {
    pub(super) fn queue_notice(&mut self, notice: MutationNotice) {
        self.notices.push(notice);
    }

    pub(super) fn has_notices(&self) -> bool {
        !self.notices.is_empty()
    }

    pub(super) fn take_notices(&mut self) -> Vec<MutationNotice> {
        std::mem::take(&mut self.notices)
    }

    pub(in crate::app) fn remember_plan(&mut self, mut record: PersistedPlan) {
        bound_histories(&mut record.agents);
        self.plans.insert(record.id.clone(), record);
    }

    pub(in crate::app) fn remember_run(&mut self, mut record: PersistedRun) {
        bound_histories(&mut record.agents);
        self.runs.insert(record.id.clone(), record);
    }

    pub(super) fn plan(&self, id: &str) -> Option<&PersistedPlan> {
        self.plans.get(id)
    }

    pub(super) fn run(&self, id: &str) -> Option<&PersistedRun> {
        self.runs.get(id)
    }

    /// A separate Store message transaction already committed these rows.
    /// Rollback of a later roster save must preserve them and their allocator.
    pub(in crate::app) fn apply_thread_items(
        &mut self,
        delta: &crate::store::CommittedThreadItems,
    ) {
        for agents in self
            .plans
            .values_mut()
            .map(|record| &mut record.agents)
            .chain(self.runs.values_mut().map(|record| &mut record.agents))
        {
            for agent in agents {
                delta.apply_to(&mut agent.thread);
            }
        }
    }

    /// Reset commits independently of an owner save. Patch only its exact
    /// durable payload so a later failed save cannot restore the erased
    /// generation, including aliases owned by other plans or runs.
    pub(in crate::app) fn apply_conversation_reset(
        &mut self,
        agents: &[Agent],
        summary_owners: &HashSet<String>,
    ) {
        for agent in agents {
            if let Some(record) = self.plans.get_mut(&agent.owner_id) {
                replace_reset_agent(&mut record.agents, agent);
            }
            if let Some(record) = self.runs.get_mut(&agent.owner_id) {
                replace_reset_agent(&mut record.agents, agent);
            }
        }
        for owner in summary_owners {
            if let Some(record) = self.plans.get_mut(owner) {
                record.last_summary = None;
                record.last_error = None;
            }
            if let Some(record) = self.runs.get_mut(owner) {
                record.last_summary = None;
                record.last_error = None;
            }
        }
    }

    pub(in crate::app) fn forget(&mut self, id: &str) {
        self.plans.remove(id);
        self.runs.remove(id);
    }

    /// A legacy writer temporarily removes owners from the live maps. Whole
    /// board refreshes must retain their previous publication until it settles.
    pub(in crate::app) fn has_borrowed(&self, app: &crate::app::AppState) -> bool {
        self.plans.keys().any(|id| !app.plans.contains_key(id))
            || self.runs.keys().any(|id| !app.runs.contains_key(id))
    }
}

fn replace_reset_agent(agents: &mut [Agent], replacement: &Agent) {
    if let Some(agent) = agents.iter_mut().find(|agent| agent.id == replacement.id) {
        *agent = replacement.clone();
    }
}

fn bound_histories(agents: &mut [crate::agent::Agent]) {
    for agent in agents {
        let thread = &mut agent.thread;
        if thread.items.len() <= RESIDENT_CONVERSATION_TAIL {
            continue;
        }
        let count = thread.total_item_count();
        let sequence = thread.last_sequence();
        let tail = thread
            .items
            .split_off(thread.items.len() - RESIDENT_CONVERSATION_TAIL);
        thread.adopt_stored_tail(tail, count - RESIDENT_CONVERSATION_TAIL as u64, sequence);
    }
}
