//! One agent writing to another.
//!
//! Every agent-originated send lands here, whichever tool spelled it: the id a
//! coding agent answers a message on, and the workspace a project agent
//! addresses. One path so there is one behaviour — the same scope, the same
//! refusals, the same record on both threads — rather than one per surface.
//!
//! The scope is the project. An agent reaches the agents of conversation owners
//! bound to the same project its own owner is bound to, and nothing else. An
//! agent in another project, an id that names nobody, and the sender itself are
//! each refused by name before anything is written.

use crate::app::AppState;
use crate::operation::OperationRequester;
use serde_json::{json, Value};

/// Who is speaking: the conversation owner the MCP session authenticated
/// against, and the agent on it.
#[derive(Debug, Clone, Copy)]
pub(in crate::app) struct AgentSender<'a> {
    pub(in crate::app) entity_id: &'a str,
    pub(in crate::app) agent_id: &'a str,
}

impl AppState {
    /// `message_agent` — say something to another agent of this project.
    ///
    /// It goes in with the user's role, because that is the side of the
    /// conversation an instruction arrives on whoever wrote it, and wearing the
    /// sender, so the agent reading it knows a machine sent it. The operation
    /// records the sender as the requester, so the target's terminal report
    /// comes back the same way a project agent's does — one behaviour for every
    /// agent-originated send.
    pub(in crate::app) fn agent_message_agent(
        &mut self,
        sender: AgentSender<'_>,
        target_agent_id: &str,
        body: &str,
    ) -> Result<Value, String> {
        let target_entity_id = self.reachable_agent(sender, target_agent_id)?;
        self.post_from_agent_to_agent(sender, &target_entity_id, target_agent_id, body)
    }

    /// The same send, for a caller that has already resolved the target's
    /// conversation owner — `message_workspace_agent`, which addresses an agent
    /// by the workspace it is on. The scope still holds: a workspace of this
    /// project has an owner bound to this project.
    pub(in crate::app) fn post_from_agent_to_agent(
        &mut self,
        sender: AgentSender<'_>,
        target_entity_id: &str,
        target_agent_id: &str,
        body: &str,
    ) -> Result<Value, String> {
        let requester = self.agent_requester(sender)?;
        self.refuse_self_send(&requester, target_entity_id, target_agent_id)?;
        let operation_id = format!("op-{}", uuid::Uuid::new_v4());
        let posted = self.thread_post_from_agent(
            &json!({
                "entity_id": target_entity_id,
                "agent_id": target_agent_id,
                "body": body,
                "operation_id": operation_id,
            }),
            requester,
        )?;
        Ok(json!({
            "entity_id": posted["entity_id"].as_str().unwrap_or(target_entity_id),
            "agent_id": posted["agent_id"],
            "operation_id": operation_id,
            "posted_sequence": posted["posted_sequence"],
        }))
    }

    /// The conversation owner of the agent this send names, or why it is not
    /// this sender's to write to.
    ///
    /// Three refusals, each naming what it refused: an id nobody answers to, an
    /// agent of another project, and a sender with no project of its own — which
    /// is an entity Build cannot scope a send from at all.
    fn reachable_agent(
        &self,
        sender: AgentSender<'_>,
        target_agent_id: &str,
    ) -> Result<String, String> {
        let target_entity_id = self
            .entity_of_agent(target_agent_id)
            .ok_or_else(|| format!("unknown agent_id: {target_agent_id}"))?;
        let project_id = self
            .projects
            .project_id_of(sender.entity_id)
            .ok_or_else(|| format!("message_agent: {} belongs to no project", sender.entity_id))?;
        if self.projects.project_id_of(&target_entity_id) != Some(project_id) {
            return Err(format!(
                "message_agent: agent {target_agent_id} is not in project {project_id}"
            ));
        }
        Ok(target_entity_id)
    }

    /// Refuse a send that would land in the sender's own conversation.
    ///
    /// Compared by CONVERSATION and not by id: the sender is asking for words
    /// it would then read as somebody else's, and a conversation is the thing
    /// it would read them in. It observed itself in `list_workspaces`, messaged
    /// what it found, and answered its own instruction.
    fn refuse_self_send(
        &self,
        requester: &OperationRequester,
        target_entity_id: &str,
        target_agent_id: &str,
    ) -> Result<(), String> {
        let target_conversation = self
            .entity_agents(target_entity_id)
            .and_then(|agents| agents.resolve(Some(target_agent_id)))
            .map(|agent| agent.conversation_id().to_string())
            .unwrap_or_default();
        if target_conversation == requester.conversation_id {
            return Err(format!(
                "message_agent: {target_agent_id} is your own conversation — an agent cannot message itself"
            ));
        }
        Ok(())
    }

    /// The sending agent as the thing an operation is owed an answer by: which
    /// agent it is, the owner it belongs to, and its own conversation.
    fn agent_requester(&self, sender: AgentSender<'_>) -> Result<OperationRequester, String> {
        let conversation_id = self
            .entity_agents(sender.entity_id)?
            .resolve(Some(sender.agent_id))?
            .conversation_id()
            .to_string();
        Ok(OperationRequester {
            agent_id: sender.agent_id.to_string(),
            entity_id: sender.entity_id.to_string(),
            conversation_id,
        })
    }
}
