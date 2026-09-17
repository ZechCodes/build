//! What a project's agent may change.
//!
//! Each tool is a thin wrapper over the verb the client calls: the same code
//! path, the same refusals, the same record afterwards. What the wrapper adds
//! is the scope. The project comes from the agent's owner binding and the
//! workspace is checked against it before anything runs, so a tool call cannot
//! reach a project this agent is not the agent of however it is spelled.

use crate::app::AppState;
use serde_json::{json, Value};

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
