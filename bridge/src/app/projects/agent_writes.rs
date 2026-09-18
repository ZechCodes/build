//! What a project's agent may change.
//!
//! Each tool is a thin wrapper over the verb the client calls: the same code
//! path, the same refusals, the same record afterwards. What the wrapper adds
//! is the scope. The project comes from the agent's owner binding and the
//! workspace is checked against it before anything runs, so a tool call cannot
//! reach a project this agent is not the agent of however it is spelled.

use crate::app::AppState;
use serde_json::{json, Value};

/// Which agent on which workspace a message is for. Naming no agent is the
/// workspace's primary one, the way every other addressed verb reads it.
pub(in crate::app) struct WorkspaceAgentAddress<'a> {
    pub(in crate::app) workspace_id: &'a str,
    pub(in crate::app) agent_id: Option<&'a str>,
}

/// What a project agent asked its new workspace agent to run on.
///
/// Every field is optional and an absent one is left OUT of the params rather
/// than written as null: `agent.add` reads the PRESENCE of a choice key to tell
/// "run it on this" from "run it on whatever the workspace runs on", and a null
/// would read as the former.
///
/// `harness` is the tool's word for what the wire calls `provider`. The model
/// is told what it is choosing between; the daemon is told which field it is.
pub(in crate::app) struct AgentChoiceArgs<'a> {
    pub(in crate::app) harness: Option<&'a str>,
    pub(in crate::app) model: Option<&'a str>,
    pub(in crate::app) effort: Option<&'a str>,
}

impl AgentChoiceArgs<'_> {
    fn params(&self) -> Value {
        let mut params = json!({});
        for (key, value) in [
            ("provider", self.harness),
            ("model", self.model),
            ("effort", self.effort),
        ] {
            if let Some(value) = value {
                params[key] = json!(value);
            }
        }
        params
    }
}

/// What a project agent asked one added folder to be made from. Every field is
/// optional and an absent one is left OUT of the params: the verb reads which
/// keys are present to tell a path from a remote, and a null would read as a
/// blank one of either.
pub(in crate::app) struct ProjectSourceArgs<'a> {
    pub(in crate::app) path: Option<&'a str>,
    pub(in crate::app) remote: Option<&'a str>,
    pub(in crate::app) name: Option<&'a str>,
    pub(in crate::app) base_branch: Option<&'a str>,
}

impl ProjectSourceArgs<'_> {
    fn params(&self) -> Value {
        named_params(&[
            ("path", self.path),
            ("remote", self.remote),
            ("name", self.name),
            ("base_branch", self.base_branch),
        ])
    }
}

/// The same, for one directory added to a workspace: a project source it was
/// not cut with, a path, or a remote.
pub(in crate::app) struct WorkspaceDirectoryArgs<'a> {
    pub(in crate::app) source_id: Option<&'a str>,
    pub(in crate::app) path: Option<&'a str>,
    pub(in crate::app) remote: Option<&'a str>,
    pub(in crate::app) name: Option<&'a str>,
}

impl WorkspaceDirectoryArgs<'_> {
    fn params(&self) -> Value {
        named_params(&[
            ("source_id", self.source_id),
            ("path", self.path),
            ("remote", self.remote),
            ("name", self.name),
        ])
    }
}

/// The params an optional-argument tool sends: the keys it was given, and no
/// key at all for the ones it was not.
fn named_params(fields: &[(&str, Option<&str>)]) -> Value {
    let mut params = json!({});
    for (key, value) in fields {
        if let Some(value) = value {
            params[*key] = json!(value);
        }
    }
    params
}

impl AppState {
    /// `create_workspace` — cut a workspace in this agent's project, through
    /// `workspace.create` itself. The project id is the binding's; the call
    /// carries none, so there is none to disagree with.
    pub(in crate::app) fn project_agent_create_workspace(
        &mut self,
        owner_id: &str,
        name: &str,
        isolation: Option<&str>,
    ) -> Result<Value, String> {
        let project_id = self.project_agent_project(owner_id)?;
        let mut params = json!({ "project_id": project_id, "name": name });
        if let Some(isolation) = isolation {
            params["isolation"] = json!(isolation);
        }
        self.workspace_create(&params)
    }

    /// `delete_workspace` — take one of this project's workspaces away,
    /// through `workspace.delete` itself, refusals and all.
    pub(in crate::app) fn project_agent_delete_workspace(
        &mut self,
        owner_id: &str,
        workspace_id: &str,
    ) -> Result<Value, String> {
        self.project_agent_workspace(owner_id, workspace_id)?;
        self.workspace_delete(&json!({ "workspace_id": workspace_id }))
    }

    /// `add_project_source` — one more folder on this agent's project, through
    /// `project.add_source`. The project id is the binding's; the call carries
    /// none, so there is none to disagree with.
    pub(in crate::app) fn project_agent_add_project_source(
        &mut self,
        owner_id: &str,
        source: ProjectSourceArgs<'_>,
    ) -> Result<Value, String> {
        let project_id = self.project_agent_project(owner_id)?;
        let mut params = source.params();
        params["project_id"] = json!(project_id);
        self.project_add_source(&params)
    }

    /// `remove_project_source` — take a folder off this agent's project,
    /// through `project.remove_source`.
    pub(in crate::app) fn project_agent_remove_project_source(
        &mut self,
        owner_id: &str,
        source_id: &str,
    ) -> Result<Value, String> {
        let project_id = self.project_agent_project(owner_id)?;
        self.project_remove_source(&json!({
            "project_id": project_id,
            "source_id": source_id,
        }))
    }

    /// `add_workspace_directory` — one more directory in one of this project's
    /// workspaces, through `workspace.add_directory`.
    pub(in crate::app) fn project_agent_add_workspace_directory(
        &mut self,
        owner_id: &str,
        workspace_id: &str,
        source: WorkspaceDirectoryArgs<'_>,
    ) -> Result<Value, String> {
        self.project_agent_workspace(owner_id, workspace_id)?;
        let mut params = source.params();
        params["workspace_id"] = json!(workspace_id);
        self.workspace_add_directory(&params)
    }

    /// `remove_workspace_directory` — one directory leaves one of this
    /// project's workspaces, through `workspace.remove_directory`.
    pub(in crate::app) fn project_agent_remove_workspace_directory(
        &mut self,
        owner_id: &str,
        workspace_id: &str,
        directory_id: &str,
    ) -> Result<Value, String> {
        self.project_agent_workspace(owner_id, workspace_id)?;
        self.workspace_remove_directory(&json!({
            "workspace_id": workspace_id,
            "directory_id": directory_id,
        }))
    }

    /// `add_workspace_agent` — put an agent on one of this project's
    /// workspaces, through `agent.add` on that workspace's conversation owner.
    ///
    /// The owner is minted on the way in, because a workspace nobody has talked
    /// to yet has none and there would otherwise be nowhere for the agent to
    /// live. That is what `workspace.ensure_conversation` is for, and a
    /// workspace that already owns one keeps it.
    pub(in crate::app) fn project_agent_add_workspace_agent(
        &mut self,
        owner_id: &str,
        workspace_id: &str,
        choice: AgentChoiceArgs<'_>,
    ) -> Result<Value, String> {
        self.project_agent_workspace(owner_id, workspace_id)?;
        let mut params = choice.params();
        params["workspace_id"] = json!(workspace_id);
        let conversation = self.workspace_ensure_conversation(&params)?;
        let entity_id = conversation["run_id"]
            .as_str()
            .ok_or("the workspace conversation has no owner")?
            .to_string();
        params["entity_id"] = json!(entity_id);
        let mut added = self.agent_add(&params)?;
        added["workspace_id"] = json!(workspace_id);
        Ok(added)
    }

    /// `remove_workspace_agent` — take an agent off one of this project's
    /// workspaces, through `agent.remove`. A workspace nobody has talked to has
    /// no agents, so there is nothing there to name.
    pub(in crate::app) fn project_agent_remove_workspace_agent(
        &mut self,
        owner_id: &str,
        workspace_id: &str,
        agent_id: &str,
    ) -> Result<Value, String> {
        let workspace = self.project_agent_workspace(owner_id, workspace_id)?;
        let entity_id = self
            .workspace_conversation_owner(&workspace)
            .ok_or_else(|| format!("workspace {workspace_id} has no agents"))?;
        let mut removed =
            self.agent_remove(&json!({ "entity_id": entity_id, "agent_id": agent_id }))?;
        removed["workspace_id"] = json!(workspace_id);
        Ok(removed)
    }

    /// `message_workspace_agent` — say something to an agent on one of this
    /// project's workspaces, addressed by the workspace rather than by the
    /// agent's id.
    ///
    /// A thin alias for `message_agent`: it resolves the workspace to a
    /// conversation and an agent on it, then goes through the one send path
    /// every agent-originated message goes through — the same role, the same
    /// sender stamp, the same requester on the operation, the same refusals.
    /// Naming no agent is the workspace's primary one, and a workspace nobody
    /// has staffed gets one, the way every addressed verb reads it.
    pub(in crate::app) fn project_agent_message_workspace_agent(
        &mut self,
        owner_id: &str,
        sender_id: &str,
        target: WorkspaceAgentAddress<'_>,
        body: &str,
    ) -> Result<Value, String> {
        let workspace = self.project_agent_workspace(owner_id, target.workspace_id)?;
        let entity_id = self
            .workspace_conversation_owner(&workspace)
            .ok_or_else(|| format!("workspace {} has no agents", target.workspace_id))?;
        let target_agent_id = match target.agent_id {
            Some(agent_id) => self
                .entity_agents(&entity_id)?
                .resolve(Some(agent_id))?
                .id
                .clone(),
            None => self.ensure_primary_agent(&entity_id)?,
        };
        let sender = crate::app::AgentSender {
            entity_id: owner_id,
            agent_id: sender_id,
        };
        let mut sent = self.post_from_agent_to_agent(sender, &entity_id, &target_agent_id, body)?;
        sent["workspace_id"] = json!(target.workspace_id);
        Ok(sent)
    }

    /// One workspace of this agent's project, or why it is none of its
    /// business. The one gate every workspace tool passes through: the id in
    /// the call is checked against the binding before anything runs, so a
    /// workspace in another project is refused rather than acted on.
    pub(super) fn project_agent_workspace(
        &self,
        owner_id: &str,
        workspace_id: &str,
    ) -> Result<crate::workspace::Workspace, String> {
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
        Ok(workspace)
    }
}
