//! What a project's agent may read.
//!
//! Two reads, both of the project the agent belongs to, both through the code
//! path the client verb uses. Which project that is comes from the agent's
//! owner binding and never from an argument, so a project agent cannot be
//! talked into reading a project it is not the agent for.

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
        let project_id = self.project_agent_project(owner_id)?;
        let workspace = self
            .workspaces
            .get(workspace_id)
            .cloned()
            .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))?;
        if workspace.project_id != project_id {
            return Err(format!(
                "workspace {workspace_id} is not in project {project_id}"
            ));
        }
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

    /// The project a project agent reads. It is the one its owner is bound to —
    /// the binding `project.ensure_conversation` wrote — so the scope of every
    /// project tool is fixed when the agent is created.
    fn project_agent_project(&self, owner_id: &str) -> Result<String, String> {
        self.projects
            .project_id_of(owner_id)
            .map(str::to_string)
            .ok_or_else(|| format!("{owner_id} belongs to no project"))
    }
}
