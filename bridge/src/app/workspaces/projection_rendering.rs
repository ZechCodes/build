//! Reuse pure rendering work only while one candidate owns its projection guard.
use crate::branch::WorkItemCandidate;
use crate::run::RunState;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

mod history;
mod indexes;
pub(super) use indexes::Indexes;

pub(in crate::app) type AgentDigestKey = (String, String, Option<PathBuf>, bool);

#[derive(Default)]
pub(in crate::app) struct ProjectionRendering {
    histories: history::Histories,
    indexes: Arc<Indexes>,
    branches: Mutex<BTreeMap<String, (Value, WorkItemCandidate)>>,
    agents: Mutex<BTreeMap<AgentDigestKey, Value>>,
    external_rows: Mutex<Option<BTreeMap<String, Value>>>,
}

impl ProjectionRendering {
    pub(super) fn new(indexes: Arc<Indexes>) -> Self {
        Self {
            indexes,
            ..Self::default()
        }
    }

    pub(in crate::app) fn agent_owner(&self, agent: &str) -> Option<&String> {
        self.indexes.agent_owner(agent)
    }

    pub(in crate::app) fn preferred_run_state(
        &self,
        project: &str,
        root: &Path,
        workspace: Option<&str>,
    ) -> Option<RunState> {
        self.indexes.preferred_run_state(project, root, workspace)
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
