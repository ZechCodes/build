//! Handing an agent's answer back to the agent that asked for it.
//!
//! A message one agent sends another creates an operation, and that operation
//! remembers the requester: which agent asked, and the conversation an answer
//! is owed to. When the turn ends, the terminal message goes back there as an
//! ordinary post — the agent that wrote it needs no tool for this and never
//! learns it was summoned by a machine rather than by the user.
//!
//! Two rules keep the hand-off from becoming a conversation between machines.
//! Only a TERMINAL message travels, so a progress note or a question does not
//! interrupt the reader mid-turn. And a forwarded answer records no requester
//! of its own, so it is never itself forwarded and the turn it starts owes
//! nobody a reply.

use crate::app::AppState;
use crate::mcp::{DoneReport, DoneStatus};
use crate::operation::OperationRequester;
use serde_json::{json, Value};

/// How a turn ended, as the agent that was waiting for it reads it. `None` for
/// a report that is not the end of a turn.
fn ending(status: DoneStatus) -> Option<&'static str> {
    match status {
        DoneStatus::Completed => Some("Complete"),
        DoneStatus::Blocked => Some("Blocked"),
        // A failed phase is a lifecycle outcome the daemon records on the
        // conversation; the agent's own words for it arrive as one of the two
        // above. Nothing to hand over.
        DoneStatus::Failed => None,
    }
}

impl AppState {
    /// One agent finished a turn another agent started: hand its terminal
    /// message back. The status leads, because what the reader does next
    /// depends on it, and the agent's own words follow.
    ///
    /// Quiet about everything it cannot do: a report nobody is waiting for, an
    /// agent that has since been removed, a conversation that has closed. None
    /// of them is the reporting agent's problem, and none of them may cost it
    /// its own report.
    pub(in crate::app) fn forward_terminal_reply(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        report: &DoneReport,
    ) {
        let Some(ending) = ending(report.status) else {
            return;
        };
        let Ok(conversation_id) = self
            .entity_agents(entity_id)
            .and_then(|agents| agents.resolve(Some(agent_id)))
            .map(|agent| agent.conversation_id().to_string())
        else {
            return;
        };
        let owed = self.take_answers_owed(&conversation_id);
        if owed.is_empty() {
            return;
        }
        let body = format!("{ending}.\n\n{}", report.summary);
        let sender = crate::thread::AgentIdentity {
            id: agent_id.to_string(),
        };
        // One answer per conversation waiting for one: several messages handed
        // over before the turn ended are all answered by the message that ends
        // it, and saying so twice would only repeat the same words.
        let mut answered: Vec<String> = Vec::new();
        for requester in owed {
            if answered.contains(&requester.conversation_id) {
                continue;
            }
            answered.push(requester.conversation_id.clone());
            if let Err(error) = self.forward_reply_to(&requester, &sender, &body) {
                eprintln!("forward reply to {}: {error}", requester.agent_id);
            }
        }
    }

    /// The post itself, addressed to the agent that asked. It carries an
    /// operation id so the answer is DELIVERED — with an operation the waiting
    /// agent is handed the words themselves, and without one it would be told
    /// to go and fetch them through a tool a project agent does not have.
    fn forward_reply_to(
        &mut self,
        requester: &OperationRequester,
        sender: &crate::thread::AgentIdentity,
        body: &str,
    ) -> Result<Value, String> {
        let params = json!({
            "entity_id": requester.entity_id,
            "agent_id": requester.agent_id,
            "body": body,
            "operation_id": format!("op-{}", uuid::Uuid::new_v4()),
        });
        self.thread_post_forwarded(&params, sender.clone())
    }

    /// Every answer this conversation owes an agent, settled as it is read.
    ///
    /// The durable record is the operation receipt's requester, so a restart
    /// between the question and the answer still knows where the answer goes.
    /// Without a store the process-local mirror holds the same thing.
    pub(in crate::app) fn take_answers_owed(
        &mut self,
        conversation_id: &str,
    ) -> Vec<OperationRequester> {
        match self.store.as_ref() {
            Some(store) => store
                .take_operation_requesters(conversation_id)
                .unwrap_or_else(|error| {
                    eprintln!("operation store: {error}");
                    Vec::new()
                }),
            None => self.operation_ledger.take_requesters(conversation_id),
        }
    }
}
