//! What Build remembers about the compactions it has been asked for and sent,
//! per agent, between the moment it learns of one and the moment it is done.
//!
//! Two short-lived facts. A REQUEST is a compaction an agent asked for that
//! has not gone yet — one per agent, the newest instructions winning — and
//! whether it is still waiting for the agent's current turn to end. A SENT
//! compaction is one Build has asked a session for and not yet measured: the
//! conversation sequence it was sent after, which finds the row it produced,
//! and what Build knew when it sent it.
//!
//! In memory: a restart ends every session, and with them every compaction
//! either fact could be about.

use crate::thread::CompactionDetail;
use std::collections::HashMap;

/// The agent a fact is about: its conversation owner and its id.
type AgentKey = (String, String);

fn key(owner: &str, agent_id: &str) -> AgentKey {
    (owner.to_string(), agent_id.to_string())
}

/// A compaction an agent asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CompactionRequest {
    instructions: String,
    /// Whether it waits for the agent's current turn to end. A request made
    /// between turns does not; one made mid-turn does until the status pump
    /// sees the turn end.
    after_current_turn: bool,
}

/// A compaction Build sent, until the context after it is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) struct SentCompaction {
    /// The conversation's last sequence when it was sent: its row is the
    /// newest `Compaction` minted after this.
    pub(in crate::app) sent_after: u64,
    pub(in crate::app) detail: CompactionDetail,
}

#[derive(Debug, Default)]
pub(in crate::app) struct CompactionLedger {
    requested: HashMap<AgentKey, CompactionRequest>,
    sent: HashMap<AgentKey, SentCompaction>,
}

impl CompactionLedger {
    /// Ask for one agent's compaction, replacing whatever it had asked for.
    pub(in crate::app) fn request(
        &mut self,
        owner: &str,
        agent_id: &str,
        instructions: &str,
        after_current_turn: bool,
    ) {
        self.requested.insert(
            key(owner, agent_id),
            CompactionRequest {
                instructions: instructions.to_string(),
                after_current_turn,
            },
        );
    }

    /// The agent's turn ended: a request that was waiting for it no longer
    /// waits. Says whether the agent has one ready to go.
    pub(in crate::app) fn turn_ended(&mut self, owner: &str, agent_id: &str) -> bool {
        let Some(request) = self.requested.get_mut(&key(owner, agent_id)) else {
            return false;
        };
        request.after_current_turn = false;
        true
    }

    /// The instructions of the agent's request, when it is ready to go.
    pub(in crate::app) fn ready(&self, owner: &str, agent_id: &str) -> Option<&str> {
        self.requested
            .get(&key(owner, agent_id))
            .filter(|request| !request.after_current_turn)
            .map(|request| request.instructions.as_str())
    }

    /// Every agent with a request ready to go.
    pub(in crate::app) fn ready_agents(&self) -> Vec<(String, String)> {
        self.requested
            .iter()
            .filter(|(_, request)| !request.after_current_turn)
            .map(|(agent, _)| agent.clone())
            .collect()
    }

    /// Take the agent's request, ready or not, once it has been sent or can
    /// no longer be.
    pub(in crate::app) fn withdraw(&mut self, owner: &str, agent_id: &str) -> Option<String> {
        self.requested
            .remove(&key(owner, agent_id))
            .map(|request| request.instructions)
    }

    /// Remember a compaction sent to the agent until it can be measured.
    pub(in crate::app) fn sent(&mut self, owner: &str, agent_id: &str, sent: SentCompaction) {
        self.sent.insert(key(owner, agent_id), sent);
    }

    /// The agent's unmeasured compaction, taken to be stamped.
    pub(in crate::app) fn take_sent(
        &mut self,
        owner: &str,
        agent_id: &str,
    ) -> Option<SentCompaction> {
        self.sent.remove(&key(owner, agent_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_made_mid_turn_is_ready_once_the_turn_ends() {
        let mut ledger = CompactionLedger::default();
        ledger.request("run", "agent", "keep the notes", true);
        assert_eq!(ledger.ready("run", "agent"), None);
        assert!(ledger.ready_agents().is_empty());

        assert!(ledger.turn_ended("run", "agent"));

        assert_eq!(ledger.ready("run", "agent"), Some("keep the notes"));
        assert_eq!(
            ledger.ready_agents(),
            vec![("run".to_string(), "agent".to_string())]
        );
        assert!(!ledger.turn_ended("run", "other"), "nobody else asked");
    }

    #[test]
    fn the_newest_request_wins_and_is_taken_once() {
        let mut ledger = CompactionLedger::default();
        ledger.request("run", "agent", "old", false);
        ledger.request("run", "agent", "new", false);

        assert_eq!(ledger.withdraw("run", "agent").as_deref(), Some("new"));
        assert_eq!(ledger.withdraw("run", "agent"), None);
    }

    #[test]
    fn a_sent_compaction_is_taken_once() {
        let mut ledger = CompactionLedger::default();
        let sent = SentCompaction {
            sent_after: 4,
            detail: CompactionDetail::default(),
        };
        ledger.sent("run", "agent", sent.clone());

        assert_eq!(ledger.take_sent("run", "agent"), Some(sent));
        assert_eq!(ledger.take_sent("run", "agent"), None);
    }
}
