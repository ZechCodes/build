use crate::app::read_view::SnapshotDelta;
use crate::app::AppState;
use crate::orchestrator::{ActivePlan, ActiveRun};
use crate::session_summary::SessionSummary;

struct RunSummaries {
    summaries: Vec<(String, Option<SessionSummary>)>,
    seen: Vec<((String, String), Option<u64>)>,
}

impl RunSummaries {
    fn capture(app: &AppState, owner: &str, active: &ActiveRun) -> Self {
        let owners = std::iter::once(owner).chain(app.projects.project_id_of(owner));
        Self {
            summaries: owners
                .map(|id| (id.to_string(), app.session_summaries.get(id).copied()))
                .collect(),
            seen: active
                .agents
                .iter()
                .map(|agent| {
                    let key = (owner.to_string(), agent.id.clone());
                    let seen = app.session_seen.get(&key).copied();
                    (key, seen)
                })
                .collect(),
        }
    }

    fn restore(self, app: &mut AppState) {
        for (id, summary) in self.summaries {
            restore_entry(&mut app.session_summaries, id, summary);
        }
        for (id, seen) in self.seen {
            restore_entry(&mut app.session_seen, id, seen);
        }
    }
}

fn restore_entry<K: std::hash::Hash + Eq, T>(
    entries: &mut std::collections::HashMap<K, T>,
    id: K,
    previous: Option<T>,
) {
    match previous {
        Some(value) => {
            entries.insert(id, value);
        }
        None => {
            entries.remove(&id);
        }
    }
}

impl AppState {
    pub(super) fn prepare_plan_mutation_snapshot(
        &mut self,
        owner: &str,
        active: ActivePlan,
        state: &str,
        now: &str,
    ) -> (ActivePlan, Option<SnapshotDelta>) {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("finisher.snapshot");
        if !self.snapshot_projection_initialized() {
            return (active, None);
        }
        let attention = self.board.attention_mut().stage_mutation(owner, state, now);
        self.plans.insert(owner.to_string(), active);
        self.refresh_owner_snapshot_observations(owner);
        let projection = self.prepare_owner_snapshot_projection(owner);
        let active = self
            .plans
            .remove(owner)
            .expect("the staged plan is still checked out");
        self.board
            .attention_mut()
            .restore_mutation(owner, attention);
        (active, projection)
    }

    pub(super) fn prepare_run_mutation_snapshot(
        &mut self,
        owner: &str,
        active: ActiveRun,
        state: &str,
        now: &str,
    ) -> (ActiveRun, Option<SnapshotDelta>) {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("finisher.snapshot");
        if !self.snapshot_projection_initialized() {
            return (active, None);
        }
        let attention = self.board.attention_mut().stage_mutation(owner, state, now);
        let summaries = RunSummaries::capture(self, owner, &active);
        self.note_run_messages(owner, &active);
        self.runs.insert(owner.to_string(), active);
        self.refresh_owner_snapshot_observations(owner);
        let projection = self.prepare_owner_snapshot_projection(owner);
        let active = self
            .runs
            .remove(owner)
            .expect("the staged run is still checked out");
        self.board
            .attention_mut()
            .restore_mutation(owner, attention);
        summaries.restore(self);
        (active, projection)
    }
}
