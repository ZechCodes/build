//! Pure roster-owner and preferred-run indexes; no rendered payloads.
use crate::app::AppState;
use crate::run::RunState;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(in crate::app::workspaces) struct Indexes {
    agent_owners: BTreeMap<String, String>,
    run_states: BTreeMap<String, RunState>,
    root_states: BTreeMap<(String, PathBuf), RunState>,
}
impl Indexes {
    pub(in crate::app::workspaces) fn capture(app: &AppState) -> Self {
        #[cfg(test)]
        crate::app::workspaces::ownership::work_counts::render_index_build();
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.ownership_render_indexes");
        let mut captured = Self::default();
        // Match entity_of_agent's plans-before-runs traversal, including an
        // old malformed roster containing an id already owned elsewhere.
        for (owner, roster) in app
            .plans
            .iter()
            .map(|(owner, plan)| (owner, &plan.agents))
            .chain(app.runs.iter().map(|(owner, run)| (owner, &run.agents)))
        {
            for agent in roster.iter() {
                #[cfg(test)]
                crate::app::workspaces::ownership::work_counts::source_row();
                captured
                    .agent_owners
                    .entry(agent.id.clone())
                    .or_insert_with(|| owner.clone());
            }
        }
        for (owner, active) in &app.runs {
            #[cfg(test)]
            crate::app::workspaces::ownership::work_counts::source_row();
            let state = active.run.state;
            captured.run_states.insert(owner.clone(), state);
            if let Some(project) = app.projects.project_id_of(owner) {
                let key = (project.to_owned(), app.snapshot_root(&active.worktree.path));
                captured
                    .root_states
                    .entry(key)
                    .and_modify(|held| {
                        *held = crate::branch::preferred_run_state([(false, *held), (false, state)])
                            .unwrap()
                    })
                    .or_insert(state);
            }
        }
        captured
    }

    pub(in crate::app) fn agent_owner(&self, agent: &str) -> Option<&String> {
        self.agent_owners.get(agent)
    }

    pub(in crate::app) fn preferred_run_state(
        &self,
        project: &str,
        root: &Path,
        workspace: Option<&str>,
    ) -> Option<RunState> {
        // A direct id wins even when its retained project/root differs.
        workspace
            .and_then(|owner| self.run_states.get(owner))
            .copied()
            .or_else(|| {
                self.root_states
                    .get(&(project.to_owned(), root.to_owned()))
                    .copied()
            })
    }
}
