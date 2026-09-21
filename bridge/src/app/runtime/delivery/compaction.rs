//! Compacting a session before the turn that would grow it past its limit.
//!
//! A turn is where a context grows, so the turn is where it is measured: the
//! status pump writes down what each turn cost ([`AppState::record_agent_turn_context`]),
//! and the next warm delivery to an agent whose last turn reached its
//! threshold asks the session to `/compact` first. The turn itself waits in
//! the queue and travels on the drain the session's return to waiting sets
//! off, so it lands on the compacted context.

use super::preflight::record_command_activity;
use crate::app::{AppState, PendingAgentTurn, Spawned};
use crate::harness::{harness_for, AgentSession, Turn};
use crate::timing::FrameTimer;
use std::sync::{Arc, Mutex};

/// What a compaction is asked for with. Every harness that compacts on a
/// command spells it this way, and says so through `starts_compaction`.
pub(in crate::app) const COMPACT_COMMAND: &str = "/compact";

impl AppState {
    /// Whether `turn`, about to be said as `prompt` to a session that was
    /// `spawned` for it, should wait for a compaction first.
    ///
    /// Only a warm session is compacted: a cold spawn opens a context this
    /// process has not measured, so there is nothing yet to act on — its first
    /// turn reports one. Never mid-turn: an agent still working defers the
    /// turn before it gets here, and an interrupting turn goes as it is. Never
    /// for a harness that cannot compact, never ahead of a turn that is itself
    /// a compaction, and only once the last reported context has reached the
    /// agent's threshold (its own, else the device's; 0 is never).
    pub(in crate::app) fn compaction_due_before(
        &self,
        turn: &PendingAgentTurn,
        spawned: Spawned,
        prompt: &str,
    ) -> bool {
        let harness = harness_for(turn.model_choice.provider);
        spawned == Spawned::Warm
            && !turn.interrupt
            && harness.starts_compaction(COMPACT_COMMAND)
            && !harness.starts_compaction(prompt)
            && self
                .entity_agents(&turn.owner)
                .ok()
                .and_then(|agents| agents.by_id(&turn.agent_id))
                .is_some_and(|agent| {
                    agent.working_since.is_none() && agent.compaction_due(self.compact_above_tokens)
                })
    }

    /// The live session of exactly the agent and conversation `turn` names.
    fn session_for_turn(&self, turn: &PendingAgentTurn) -> Option<Arc<dyn AgentSession>> {
        let tab = self.session_registry.agent_snapshot(&turn.tab_key())?;
        tab.instance
            .as_ref()
            .is_some_and(|instance| {
                instance.entity_id == turn.owner
                    && instance.agent_id == turn.agent_id
                    && instance.conversation_id == turn.conversation_id
            })
            .then(|| Arc::clone(&tab.session))
    }
}

/// Ask the warm session behind `turn` to compact when
/// [`AppState::compaction_due_before`] says it is due, and say whether it was
/// asked. When it was, the caller defers `turn` rather than sending it.
///
/// Safe for the turn it goes ahead of: the compaction carries no operation
/// id, so no receipt it earns can mark that turn delivered, and the deferred
/// turn is sent only by a later drain — the status pump's, once the session
/// is no longer working. The agent's recorded context is cleared as the
/// compaction is asked for, and the pump records only a context it has not
/// recorded before, so the stale reading later snapshots still carry cannot
/// ask for a second one. A session that refuses the command is logged and the
/// turn goes on as it would have; its reading is kept for the next turn.
pub(in crate::app) fn compact_before_turn(
    state: &Arc<Mutex<AppState>>,
    turn: &PendingAgentTurn,
    spawned: Spawned,
    prompt: &str,
    timer: &FrameTimer,
) -> bool {
    let session = {
        let app = timer.lock(state);
        app.compaction_due_before(turn, spawned, prompt)
            .then(|| app.session_for_turn(turn))
            .flatten()
    };
    let Some(session) = session else {
        return false;
    };
    let compaction = Turn::with_choice(
        COMPACT_COMMAND,
        turn.model_choice.clone(),
        turn.choice_revision,
    );
    if let Err(refused) = session.send_turn(&compaction) {
        eprintln!("compact {}/{}: {refused}", turn.owner, turn.agent_id);
        return false;
    }
    record_command_activity(state, timer, turn, session.as_ref(), COMPACT_COMMAND);
    timer
        .lock(state)
        .forget_agent_context(&turn.owner, &turn.agent_id);
    true
}
