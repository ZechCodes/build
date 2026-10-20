//! Stage canonical conversation histories with owner projections.
use super::*;

impl AppState {
    pub(super) fn prepare_snapshot_histories(
        &self,
        mut delta: SnapshotDelta,
        owner: Option<&str>,
    ) -> SnapshotDelta {
        let previous = self.read_view.load();
        let owners = owner
            .map(|owner| vec![owner.to_owned()])
            .unwrap_or_else(|| self.snapshot_owner_ids());
        let histories = owners
            .into_iter()
            .flat_map(|owner| self.owner_snapshot_histories(&previous, &owner))
            .collect::<BTreeMap<_, _>>();
        let retained = histories.keys().cloned().collect::<BTreeSet<_>>();
        for (id, history) in histories {
            match history {
                Ok(history) => delta = delta.put_shared_history(id, history),
                Err(error) => eprintln!("snapshot history warm {id}: {error}"),
            }
        }
        self.remove_snapshot_histories(delta, &previous, owner, &retained)
    }
    fn owner_snapshot_histories(
        &self,
        previous: &ReadView,
        owner: &str,
    ) -> Vec<(
        String,
        Result<Arc<crate::app::read_view::history::SharedHistory>, String>,
    )> {
        let Ok(agents) = self.entity_agents(owner) else {
            return Vec::new();
        };
        agents
            .iter()
            .map(|agent| {
                let thread = self
                    .agent_conversation(owner, Some(&agent.id))
                    .unwrap_or(&agent.thread);
                let id = thread.agent.id.clone();
                let history = match previous.history(&id) {
                    Some(history) => Ok(self.updated_snapshot_history(&id, history, thread)),
                    None => crate::app::read_view::history::SharedHistory::warm(
                        thread,
                        (!self.snapshot_observations.ready)
                            .then_some(self.store.as_ref())
                            .flatten(),
                    )
                    .map(Arc::new),
                };
                (id, history)
            })
            .collect()
    }
    fn remove_snapshot_histories(
        &self,
        mut delta: SnapshotDelta,
        previous: &ReadView,
        owner: Option<&str>,
        retained: &BTreeSet<String>,
    ) -> SnapshotDelta {
        let removed = match owner {
            None => previous
                .history_keys()
                .filter(|id| !retained.contains(*id))
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            Some(owner) => previous
                .get("agent.list", owner)
                .and_then(|row| row["agents"].as_array())
                .into_iter()
                .flatten()
                .filter_map(|agent| agent["conversation_id"].as_str())
                .filter(|id| !retained.contains(*id) && !self.snapshot_has_conversation(id))
                .map(str::to_owned)
                .collect(),
        };
        for id in removed {
            delta = delta.remove_history(id);
        }
        delta
    }
    fn snapshot_has_conversation(&self, conversation: &str) -> bool {
        self.plans
            .values()
            .map(|plan| &plan.agents)
            .chain(self.runs.values().map(|run| &run.agents))
            .any(|roster| {
                roster
                    .iter()
                    .any(|agent| agent.conversation_id() == conversation)
            })
    }
}
