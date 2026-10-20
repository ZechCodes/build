//! Reuse pure rendering work only while one candidate owns its projection guard.
use super::AppState;
use crate::branch::WorkItemCandidate;
use crate::run::RunState;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub(in crate::app) type AgentDigestKey = (String, String, Option<PathBuf>, bool);

#[derive(Default)]
pub(in crate::app) struct ProjectionRendering {
    agent_owners: BTreeMap<String, String>,
    run_states: BTreeMap<String, RunState>,
    root_states: BTreeMap<(String, PathBuf), RunState>,
    branches: Mutex<BTreeMap<String, (Value, WorkItemCandidate)>>,
    agents: Mutex<BTreeMap<AgentDigestKey, Value>>,
    external_rows: Mutex<Option<BTreeMap<String, Value>>>,
}

impl ProjectionRendering {
    pub(super) fn capture(app: &AppState) -> Self {
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
                captured
                    .agent_owners
                    .entry(agent.id.clone())
                    .or_insert_with(|| owner.clone());
            }
        }
        for (owner, active) in &app.runs {
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

    pub(in crate::app) fn branch(&self, owner: &str, stat: &Value) -> Option<WorkItemCandidate> {
        self.branches
            .lock()
            .unwrap()
            .get(owner)
            .filter(|(captured_stat, _)| captured_stat == stat)
            .map(|(_, row)| row.clone())
    }

    pub(in crate::app) fn remember_branch(
        &self,
        owner: &str,
        stat: &Value,
        row: &WorkItemCandidate,
    ) {
        self.branches
            .lock()
            .unwrap()
            .insert(owner.to_owned(), (stat.clone(), row.clone()));
    }

    pub(in crate::app) fn agent_digest(&self, key: &AgentDigestKey) -> Option<Value> {
        self.agents.lock().unwrap().get(key).cloned()
    }

    pub(in crate::app) fn remember_agent_digest(&self, key: AgentDigestKey, row: &Value) {
        self.agents.lock().unwrap().insert(key, row.clone());
    }

    /// Distinguish an uncaptured scan from a captured missing id.
    pub(in crate::app) fn external_row(&self, id: &str) -> Option<Option<Value>> {
        self.external_rows
            .lock()
            .unwrap()
            .as_ref()
            .map(|rows| rows.get(id).cloned())
    }

    pub(in crate::app) fn remember_external_rows(&self, rows: BTreeMap<String, Value>) {
        *self.external_rows.lock().unwrap() = Some(rows);
    }
}
