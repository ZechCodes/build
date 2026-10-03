//! `compact_self` and `compact_agent`: a compaction an agent asks for.
//!
//! Neither sends anything under the app lock. Each checks the session can be
//! compacted at all and writes the request down; what sends it is a drain —
//! the one the MCP socket runs after every tool call, for an agent between
//! turns, and the status pump's once a working agent's turn ends. A request
//! goes ahead of any turn queued for the agent, through the same
//! [`compact_before_turn`](super::compact_before_turn) the automatic one
//! takes; with nothing queued the drain sends it itself.

use super::CompactionSend;
use crate::app::{AgentSender, AppState};
use crate::harness::{harness_for, AgentSession};
use crate::mcp::BridgeAction;
use crate::tracker::Actor;
use serde_json::{json, Value};
use std::sync::Arc;

/// What `compact_self` calls the session it compacts: the caller's own.
const OWN_SESSION: &str = "your session";

impl AppState {
    /// The compaction tools, answered for the agent that called them. `None`
    /// is "not one of mine".
    pub(in crate::app) fn compaction_action(
        &mut self,
        sender: AgentSender<'_>,
        action: &BridgeAction,
    ) -> Option<Result<Value, String>> {
        Some(match action {
            BridgeAction::CompactSelf { instructions } => self.compact_self(sender, instructions),
            BridgeAction::CompactAgent {
                agent_id,
                instructions,
            } => self.compact_agent(sender, agent_id, instructions),
            _ => return None,
        })
    }

    /// `compact_self`. Its caller is inside its own turn, so it always waits
    /// for that turn to end.
    fn compact_self(
        &mut self,
        sender: AgentSender<'_>,
        instructions: &str,
    ) -> Result<Value, String> {
        self.refuse_what_cannot_compact(sender.entity_id, sender.agent_id, OWN_SESSION)?;
        self.compactions
            .request(sender.entity_id, sender.agent_id, instructions, true);
        Ok(json!({ "message": "Build will compact your session when this turn ends." }))
    }

    /// `compact_agent`: another agent of the caller's project, at once between
    /// its turns, else when its current one ends.
    fn compact_agent(
        &mut self,
        sender: AgentSender<'_>,
        agent_id: &str,
        instructions: &str,
    ) -> Result<Value, String> {
        let owner = self.reachable_agent("compact_agent", sender, agent_id)?;
        let name = self
            .actor_words(&Actor::Agent {
                agent_id: agent_id.to_string(),
            })
            .to_agent;
        self.refuse_what_cannot_compact(&owner, agent_id, &name)?;
        let working = self
            .entity_agents(&owner)
            .ok()
            .and_then(|agents| agents.by_id(agent_id))
            .is_some_and(|agent| agent.working_since.is_some());
        self.compactions
            .request(&owner, agent_id, instructions, working);
        let message = match working {
            true => format!("Build will compact {name} when its current turn ends."),
            false => format!("Build is compacting {name} now."),
        };
        Ok(json!({ "message": message }))
    }

    /// Refuse, in a sentence naming it `called`, an agent whose harness cannot
    /// compact or that has no session running to compact.
    fn refuse_what_cannot_compact(
        &self,
        owner: &str,
        agent_id: &str,
        called: &str,
    ) -> Result<(), String> {
        let agent = self
            .entity_agents(owner)
            .ok()
            .and_then(|agents| agents.by_id(agent_id))
            .ok_or_else(|| format!("unknown agent_id: {agent_id}"))?;
        if !harness_for(agent.choice.provider).compacts_on_command() {
            return Err(format!(
                "Build cannot compact {called}: its harness cannot compact."
            ));
        }
        if self.live_agent_session(owner, agent_id).is_none() {
            return Err(format!(
                "Build cannot compact {called}: it has no running session."
            ));
        }
        Ok(())
    }

    /// The running session of one agent, wherever its tab is keyed.
    pub(super) fn live_agent_session(
        &self,
        owner: &str,
        agent_id: &str,
    ) -> Option<Arc<dyn AgentSession>> {
        self.session_registry
            .live_agent_snapshots()
            .into_iter()
            .find(|tab| {
                tab.instance.as_ref().is_some_and(|instance| {
                    instance.entity_id == owner && instance.agent_id == agent_id
                })
            })
            .map(|tab| tab.session)
    }

    /// Every requested compaction a drain can send itself, taken out of the
    /// ledger: ready, for an agent between turns, with nothing queued for it.
    /// A queued turn sends the request ahead of itself instead.
    pub(in crate::app) fn take_ready_compactions(&mut self) -> Vec<CompactionSend> {
        self.compactions
            .ready_agents()
            .into_iter()
            .filter_map(|(owner, agent_id)| self.take_ready_compaction(&owner, &agent_id))
            .collect()
    }

    /// One agent's ready request, when a drain can send it now. A request
    /// whose agent or session has since gone is dropped with it.
    fn take_ready_compaction(&mut self, owner: &str, agent_id: &str) -> Option<CompactionSend> {
        let queued = self
            .delivery_queue
            .queued()
            .any(|turn| turn.owner == owner && turn.agent_id == agent_id);
        let Some((working, model_choice, choice_revision)) = self
            .entity_agents(owner)
            .ok()
            .and_then(|agents| agents.by_id(agent_id))
            .map(|agent| {
                let working = agent.working_since.is_some();
                (working, agent.choice.clone(), agent.choice_revision)
            })
        else {
            self.compactions.withdraw(owner, agent_id);
            return None;
        };
        if queued || working {
            return None;
        }
        let instructions = self.compactions.withdraw(owner, agent_id)?;
        let session = self.live_agent_session(owner, agent_id)?;
        Some(CompactionSend {
            thread_id: self
                .agent_conversation(owner, Some(agent_id))
                .ok()?
                .id
                .clone(),
            owner: owner.to_string(),
            agent_id: agent_id.to_string(),
            model_choice,
            choice_revision,
            session,
            instructions: Some(instructions),
        })
    }
}
