//! Render committed task inputs without reopening SQL or resolving disk paths.

use super::identities::mentioned;
use super::notices::ActorWords;
use super::read_identities::unknown_identity;
use crate::app::AppState;
use crate::tracker::{Actor, Task, TaskAgentIdentity, TimelineEntry};
use crate::workspace::{Workspace, WorkspaceStatus};

impl AppState {
    pub(super) fn task_with_observed_identities(
        &self,
        mut task: Task,
        timeline: &[TimelineEntry],
    ) -> Task {
        for id in mentioned(&task, timeline) {
            task.identities
                .entry(id.clone())
                .or_insert_with(|| unknown_identity(&id));
        }
        for identity in task.identities.values_mut() {
            self.overlay_observed_task_identity(identity);
        }
        task
    }

    fn overlay_observed_task_identity(&self, identity: &mut TaskAgentIdentity) {
        if self
            .session_registry
            .agent_binding_retired(&identity.agent_id)
        {
            identity.available = false;
            return;
        }
        let Some(owner) = self.entity_of_agent(&identity.agent_id) else {
            identity.available = false;
            return;
        };
        if crate::agent::is_project_agent(&identity.agent_id) {
            identity.workspace_id = None;
            identity.workspace_name = None;
            identity.available = true;
            return;
        }
        let workspace = self
            .observed_actor_workspace(&owner, Some(identity))
            .filter(|workspace| !self.observed_workspace_is_project_root(workspace));
        identity.available =
            workspace.is_some_and(|workspace| workspace.status == WorkspaceStatus::Ready);
        if let Some(workspace) = workspace {
            identity.workspace_id = Some(workspace.id.clone());
            identity.workspace_name = Some(workspace.name.clone());
        }
        if identity.project_role.is_some() {
            identity.workspace_id = None;
            identity.workspace_name = None;
        }
    }

    fn observed_workspace_is_project_root(&self, workspace: &Workspace) -> bool {
        self.projects
            .get(&workspace.project_id)
            .is_some_and(|project| {
                if self.snapshot_projection_initialized() {
                    self.observed_same_path(&workspace.root, &project.repo_path)
                } else {
                    workspace.root == project.repo_path
                }
            })
    }

    /// The immutable path observation is authoritative once admitted. Before
    /// startup warming, a write carries the association captured before SQL.
    fn observed_actor_workspace(
        &self,
        owner: &str,
        identity: Option<&TaskAgentIdentity>,
    ) -> Option<&Workspace> {
        if self.snapshot_projection_initialized() {
            self.workspace_snapshot_for_conversation(owner, None)
        } else {
            identity
                .and_then(|identity| identity.workspace_id.as_deref())
                .and_then(|id| self.workspaces.get(id))
        }
    }

    pub(super) fn observed_task_actor_words(&self, task: &Task, actor: &Actor) -> ActorWords {
        let label = match actor {
            Actor::User => {
                return ActorWords {
                    to_agent: "the user".into(),
                    to_reader: "you".into(),
                }
            }
            Actor::Build => "Build".into(),
            Actor::Agent { agent_id } => self.observed_task_actor_label(task, agent_id),
        };
        ActorWords {
            to_agent: label.clone(),
            to_reader: label,
        }
    }

    fn observed_task_actor_label(&self, task: &Task, agent: &str) -> String {
        let identity = task.identities.get(agent);
        if let Some(owner) = self.entity_of_agent(agent) {
            return self
                .live_observed_actor_label(&owner, agent, identity)
                .unwrap_or_else(|| format!("agent {agent}"));
        }
        identity
            .and_then(|identity| identity.name.clone())
            .or_else(|| {
                identity
                    .and_then(|identity| identity.workspace_name.as_ref())
                    .map(|name| format!("the {name} agent"))
            })
            .unwrap_or_else(|| format!("agent {agent}"))
    }

    fn live_observed_actor_label(
        &self,
        owner: &str,
        agent: &str,
        identity: Option<&TaskAgentIdentity>,
    ) -> Option<String> {
        if let Some(name) = self.agent_name(owner, agent) {
            return Some(name);
        }
        let project_owner = crate::agent::is_project_agent(agent)
            || identity.is_some_and(|identity| identity.project_role.is_some())
            || (self.snapshot_projection_initialized()
                && self.is_project_snapshot_conversation_owner(owner));
        if project_owner {
            return self
                .projects
                .project_id_of(owner)
                .and_then(|id| self.projects.get(id))
                .map(|project| format!("the {} project's agent", project.name));
        }
        self.observed_actor_workspace(owner, identity)
            .map(|workspace| format!("the {} agent", workspace.name))
    }
}
