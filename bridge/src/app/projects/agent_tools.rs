//! What an agent may read of its project.
//!
//! Two reads, both of the project the agent belongs to, both through the code
//! path the client verb uses. Which project that is comes from the agent's
//! owner binding and never from an argument, so no agent can be talked into
//! reading a project it is not bound to.

use crate::app::AppState;
use serde_json::{json, Value};

impl AppState {
    /// `list_workspaces` — the workspaces of this agent's project, answered by
    /// `workspace.list` itself so the agent and the client see one list.
    pub(in crate::app) fn project_agent_workspaces(
        &mut self,
        owner_id: &str,
    ) -> Result<Value, String> {
        let project_id = self.project_agent_project(owner_id)?;
        self.workspace_list(&json!({ "project_id": project_id }))
    }

    /// `list_workspace_agents` — the agents on one of this project's
    /// workspaces, answered by `agent.list` on that workspace's conversation
    /// owner. A workspace of another project is refused; a workspace nobody has
    /// talked to yet has no owner and so no agents.
    pub(in crate::app) fn project_agent_workspace_agents(
        &mut self,
        owner_id: &str,
        workspace_id: &str,
    ) -> Result<Value, String> {
        let workspace = self.project_agent_workspace(owner_id, workspace_id)?;
        let Some(run_id) = self.workspace_conversation_owner(&workspace) else {
            return Ok(json!({
                "workspace_id": workspace_id,
                "entity_id": Value::Null,
                "agents": [],
            }));
        };
        let mut listed = self.agent_list(&json!({ "entity_id": run_id }))?;
        listed["workspace_id"] = json!(workspace_id);
        Ok(listed)
    }

    /// What a project agent is told it is, on a cold start: the project it is
    /// the agent of, the scratch directory it stands in, and the reads it has.
    /// Never the coding protocol — that one is about phases, a plan and a diff,
    /// and a project agent has none of them.
    pub(in crate::app) fn project_agent_prompt(&self, owner_id: &str) -> String {
        let name = self
            .projects
            .project_id_of(owner_id)
            .and_then(|project_id| self.projects.get(project_id))
            .map(|project| project.name.clone())
            .unwrap_or_default();
        crate::templates::render(
            &crate::templates::Templates::default().project_agent,
            &crate::templates::Vars {
                project_name: &name,
                ..crate::templates::Vars::default()
            },
        )
    }

    /// The project an agent's owner is bound to: the binding
    /// `project.ensure_conversation` wrote for a project agent, and the one
    /// `workspace.ensure_conversation` (or a run's dispatch) wrote for an agent
    /// working in a checkout. Either way the scope of every project-scoped
    /// tool, read or write, is fixed when the agent is created and no argument
    /// can widen it.
    pub(super) fn project_agent_project(&self, owner_id: &str) -> Result<String, String> {
        self.projects
            .project_id_of(owner_id)
            .map(str::to_string)
            .ok_or_else(|| format!("{owner_id} belongs to no project"))
    }
}
