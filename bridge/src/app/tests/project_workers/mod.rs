//! Project workers at work (#533): the coding tools that read and add the
//! project's agents, assignment to a worker, and what removing, resetting,
//! reclaiming or deleting around them may and may not touch.

use super::project_agent::{added_project, workspace};
use super::project_coordinator::{a_repo, a_state_root, add_worker, booted, ensure};
use super::*;

mod assignment;
mod tools;

/// A booted bridge over a temporary store with one project, its owner and
/// the pinned coordinator the owner is born with.
pub(super) struct WorkerProject {
    pub(super) _home: tempfile::TempDir,
    pub(super) _tmp: tempfile::TempDir,
    pub(super) repo: PathBuf,
    pub(super) state: AppState,
    pub(super) project_id: String,
    pub(super) owner: String,
    pub(super) pin: String,
}

impl WorkerProject {
    pub(super) fn new() -> Self {
        let (home, repo) = a_repo();
        let (tmp, state_root) = a_state_root();
        let mut state = booted(&state_root);
        let project_id = added_project(&mut state, &repo);
        let owner = ensure(&mut state, &project_id);
        let pin = state.project_pin(&owner).unwrap().to_string();
        Self {
            _home: home,
            _tmp: tmp,
            repo,
            state,
            project_id,
            owner,
            pin,
        }
    }

    /// One more worker on the project's owner, named `name`.
    pub(super) fn worker(&mut self, name: &str) -> String {
        add_worker(&mut self.state, &self.owner, json!({ "name": name }))
    }

    /// A named workspace of this project with one agent on its conversation:
    /// `(workspace_id, owner, agent_id)`.
    pub(super) fn workspace_agent(&mut self, name: &str) -> (String, String, String) {
        let workspace_id = workspace(&mut self.state, &self.project_id, name);
        let owner = self.state.handle(req(
            "workspace.ensure_conversation",
            json!({ "workspace_id": workspace_id }),
        ))["result"]["run_id"]
            .as_str()
            .unwrap()
            .to_string();
        let added = self
            .state
            .handle(req("agent.add", json!({ "entity_id": owner })));
        assert_eq!(added["ok"], true, "{added:?}");
        let agent_id = added["result"]["agent"]["id"].as_str().unwrap().to_string();
        (workspace_id, owner, agent_id)
    }

    /// The ids on the project owner's whole roster, in rail order.
    pub(super) fn roster(&self) -> Vec<String> {
        self.state
            .entity_agents(&self.owner)
            .unwrap()
            .iter()
            .map(|agent| agent.id.clone())
            .collect()
    }
}

/// The ids an agent list answered, in order.
pub(super) fn listed_ids(listed: &Value) -> Vec<String> {
    listed["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["id"].as_str().unwrap().to_string())
        .collect()
}
