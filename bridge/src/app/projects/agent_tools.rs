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
    /// workspaces, answered by the roster of that workspace's conversation
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
    /// the agent of, the base it stands in and may only read, and the tools it
    /// has. Never the coding protocol — that one is about phases, a plan and a
    /// diff, and a project agent has none of them.
    ///
    /// A resumed session is a cold start too, so an agent that was talking
    /// before its project gained a base hears this on its first turn after.
    pub(in crate::app) fn project_agent_prompt(&self, owner_id: &str) -> String {
        let project = self
            .projects
            .project_id_of(owner_id)
            .and_then(|project_id| self.projects.get(project_id));
        let name = project
            .map(|project| project.name.clone())
            .unwrap_or_default();
        let base = project
            .map(|project| project.repo_path.display().to_string())
            .unwrap_or_default();
        let others = project
            .map(|project| {
                project
                    .sources
                    .iter()
                    .filter(|source| source.path != project.repo_path)
                    .map(|source| format!("`{}` at {}", source.name, source.path.display()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let sources = if others.is_empty() {
            String::new()
        } else {
            format!(
                " Its other sources are part of the base too: {}.",
                others.join(", ")
            )
        };
        crate::templates::render(
            &crate::templates::Templates::default().project_agent,
            &crate::templates::Vars {
                project_name: &name,
                project_base: &base,
                project_sources: &sources,
                ..crate::templates::Vars::default()
            },
        )
    }

    /// What a project WORKER is told it is, on a cold start: the project, the
    /// folder it starts in, every source by label and path, and that those
    /// checkouts are shared rather than its own.
    pub(in crate::app) fn project_worker_prompt(&self, owner_id: &str) -> String {
        let project = self
            .projects
            .project_id_of(owner_id)
            .and_then(|project_id| self.projects.get(project_id));
        let name = project
            .map(|project| project.name.clone())
            .unwrap_or_default();
        let base = project
            .map(|project| project.repo_path.display().to_string())
            .unwrap_or_default();
        let sources = project
            .map(|project| {
                project
                    .sources
                    .iter()
                    .map(|source| format!("`{}` at {}", source.name, source.path.display()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let sources = match sources.as_slice() {
            [] => String::new(),
            listed => format!(" The project's sources are {}.", listed.join(", ")),
        };
        crate::templates::render(
            &crate::templates::Templates::default().project_worker,
            &crate::templates::Vars {
                project_name: &name,
                project_base: &base,
                project_sources: &sources,
                ..crate::templates::Vars::default()
            },
        )
    }

    /// The cold prompt an agent of a project's conversation owner is given in
    /// place of the coding protocol, chosen by its persisted purpose: the
    /// coordinator's, or a worker's. `None` for every other agent.
    pub(in crate::app) fn project_surface_prompt(
        &self,
        owner_id: &str,
        agent_id: &str,
    ) -> Option<String> {
        if self.is_project_coordinator(owner_id, agent_id) {
            return Some(self.project_agent_prompt(owner_id));
        }
        let on_roster = self
            .entity_agents(owner_id)
            .is_ok_and(|roster| roster.by_id(agent_id).is_some());
        (on_roster && self.is_project_conversation_owner(owner_id))
            .then(|| self.project_worker_prompt(owner_id))
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
