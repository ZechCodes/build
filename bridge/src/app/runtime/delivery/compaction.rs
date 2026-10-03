//! Compacting a session before the turn that would grow it past its limit.
//!
//! A turn is where a context grows, so the turn is where it is measured: the
//! status pump writes down what each turn cost ([`AppState::record_agent_turn_context`]),
//! and the next warm delivery to an agent whose last turn reached its
//! threshold asks the session to `/compact` first. The turn itself waits in
//! the queue and travels on the drain the session's return to waiting sets
//! off, so it lands on the compacted context.

use crate::app::{AppState, PendingAgentTurn, Spawned};
use crate::harness::{harness_for, AgentSession, Turn};
use crate::models::ModelChoice;
use crate::thread::CompactionDetail;
use crate::timing::FrameTimer;
use std::sync::{Arc, Mutex};

mod ledger;
mod tools;

pub(in crate::app) use crate::harness::COMPACT_COMMAND;
pub(in crate::app) use ledger::CompactionLedger;
use ledger::SentCompaction;

/// Whether `prompt` is itself a compaction, focus instructions and all.
fn is_compaction(prompt: &str) -> bool {
    prompt.split_whitespace().next() == Some(COMPACT_COMMAND)
}

/// One compaction about to be sent: who to, on what settings, through which
/// session, and the focus it was asked for — `None` for an automatic one.
pub(in crate::app) struct CompactionSend {
    owner: String,
    agent_id: String,
    thread_id: String,
    model_choice: ModelChoice,
    choice_revision: u64,
    session: Arc<dyn AgentSession>,
    instructions: Option<String>,
}

impl AppState {
    fn compaction_generation_is_current(&self, send: &CompactionSend) -> bool {
        self.agent_conversation(&send.owner, Some(&send.agent_id))
            .is_ok_and(|thread| thread.id == send.thread_id)
            && self
                .live_agent_session(&send.owner, &send.agent_id)
                .is_some_and(|session| Arc::ptr_eq(&session, &send.session))
    }
    /// Whether `turn`, about to be said as `prompt` to a session that was
    /// `spawned` for it, should wait for a compaction first. See
    /// [`compaction_before`](Self::compaction_before).
    #[cfg(test)]
    pub(in crate::app) fn compaction_due_before(
        &self,
        turn: &PendingAgentTurn,
        spawned: Spawned,
        prompt: &str,
    ) -> bool {
        self.compaction_before(turn, spawned, prompt).is_some()
    }

    /// The compaction `turn` should wait for, with the focus it carries:
    /// `Some(Some(..))` for one an agent asked for, `Some(None)` for an
    /// automatic one, `None` for none.
    ///
    /// Only a warm session is compacted: a cold spawn opens a context this
    /// process has not measured, so there is nothing yet to act on — its first
    /// turn reports one. Never mid-turn: an agent still working defers the
    /// turn before it gets here, and an interrupting turn goes as it is. Never
    /// for a harness that cannot compact, and never ahead of a turn that is
    /// itself a compaction. A compaction an agent asked for goes first once it
    /// is ready; otherwise one is due once the last reported context has
    /// reached the agent's threshold (its own, else the device's; 0 is never).
    fn compaction_before(
        &self,
        turn: &PendingAgentTurn,
        spawned: Spawned,
        prompt: &str,
    ) -> Option<Option<String>> {
        let eligible = spawned == Spawned::Warm
            && !turn.interrupt
            && harness_for(turn.model_choice.provider).compacts_on_command()
            && !is_compaction(prompt);
        let agent = self
            .entity_agents(&turn.owner)
            .ok()?
            .by_id(&turn.agent_id)
            .filter(|agent| eligible && agent.working_since.is_none())?;
        if let Some(instructions) = self.compactions.ready(&turn.owner, &turn.agent_id) {
            return Some(Some(instructions.to_string()));
        }
        agent
            .compaction_due(self.compact_above_tokens)
            .then_some(None)
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

    /// The compaction `turn` waits for, taken out of the ledger to be sent.
    fn take_compaction_before(
        &mut self,
        turn: &PendingAgentTurn,
        spawned: Spawned,
        prompt: &str,
    ) -> Option<CompactionSend> {
        let instructions = self.compaction_before(turn, spawned, prompt)?;
        let session = self.session_for_turn(turn)?;
        if instructions.is_some() {
            self.compactions.withdraw(&turn.owner, &turn.agent_id);
        }
        Some(CompactionSend {
            thread_id: self
                .agent_conversation(&turn.owner, Some(&turn.agent_id))
                .ok()?
                .id
                .clone(),
            owner: turn.owner.clone(),
            agent_id: turn.agent_id.clone(),
            model_choice: turn.model_choice.clone(),
            choice_revision: turn.choice_revision,
            session,
            instructions,
        })
    }

    /// What Build knows as `command` goes to the session: the sequence its
    /// row will follow, the focus the harness will keep, and the context the
    /// agent's last turn left.
    fn compaction_about_to_send(&mut self, send: &CompactionSend, command: &str) -> SentCompaction {
        let sent_after = self
            .edit_agent_conversation(&send.owner, &send.agent_id, |thread, _| {
                Ok(thread.last_sequence())
            })
            .unwrap_or_default();
        let context_before = self
            .entity_agents(&send.owner)
            .ok()
            .and_then(|agents| agents.by_id(&send.agent_id))
            .and_then(|agent| agent.last_context_tokens);
        SentCompaction {
            sent_after,
            detail: CompactionDetail {
                instructions: send
                    .instructions
                    .clone()
                    .filter(|_| command != COMPACT_COMMAND),
                context_before,
                context_after: None,
            },
        }
    }

    /// Stamp the compaction last sent to this agent, if it has not been
    /// measured yet, with `context_after` — the first context reported since
    /// — onto the row it produced.
    pub(in crate::app) fn stamp_sent_compaction(
        &mut self,
        owner: &str,
        agent_id: &str,
        context_after: u64,
    ) {
        let Some(sent) = self.compactions.take_sent(owner, agent_id) else {
            return;
        };
        let detail = CompactionDetail {
            context_after: Some(context_after),
            ..sent.detail
        };
        let stamped = self.edit_agent_conversation(owner, agent_id, |thread, _| {
            Ok(thread.stamp_compaction(sent.sent_after, &detail))
        });
        if !matches!(stamped, Ok(true)) {
            eprintln!(
                "stamp compaction {owner}/{agent_id}: no row since {}",
                sent.sent_after
            );
        }
    }
}

/// Ask the warm session behind `turn` to compact when
/// [`AppState::compaction_due_before`] says it is due, and say whether it was
/// asked. When it was, the caller defers `turn` rather than sending it.
///
/// Safe for the turn it goes ahead of: the compaction carries no operation
/// id, so no receipt it earns can mark that turn delivered, and the deferred
/// turn is sent only by a later drain — the status pump's, once the session
/// is no longer working. See [`send_compaction`] for what sending does.
pub(in crate::app) fn compact_before_turn(
    state: &Arc<Mutex<AppState>>,
    turn: &PendingAgentTurn,
    spawned: Spawned,
    prompt: &str,
    timer: &FrameTimer,
) -> bool {
    let send = timer
        .lock(state)
        .take_compaction_before(turn, spawned, prompt);
    send.is_some_and(|send| send_compaction(state, timer, &send))
}

/// Say one compaction to its session, with the app lock released, and say
/// whether the session took it.
///
/// The agent's recorded context is cleared as the compaction is asked for,
/// and the pump records only a context it has not recorded before, so the
/// stale reading later snapshots still carry cannot ask for a second one.
/// What Build knew as it sent it waits in the ledger for the next context,
/// which [`AppState::stamp_sent_compaction`] writes onto its row. A session
/// that refuses the command is logged and nothing is recorded; its reading is
/// kept for the next turn.
pub(in crate::app) fn send_compaction(
    state: &Arc<Mutex<AppState>>,
    timer: &FrameTimer,
    send: &CompactionSend,
) -> bool {
    let command =
        harness_for(send.model_choice.provider).compaction_command(send.instructions.as_deref());
    let sent = {
        let mut app = timer.lock(state);
        if !app.compaction_generation_is_current(send) {
            return false;
        }
        app.compaction_about_to_send(send, &command)
    };
    let compaction = Turn::with_choice(
        command.as_str(),
        send.model_choice.clone(),
        send.choice_revision,
    );
    if let Err(refused) = send.session.send_turn(&compaction) {
        eprintln!("compact {}/{}: {refused}", send.owner, send.agent_id);
        return false;
    }
    let mut app = timer.lock(state);
    if !app.compaction_generation_is_current(send) {
        return false;
    }
    if send.session.terminal().is_some() {
        app.record_agent_activity(
            &send.owner,
            &send.agent_id,
            &crate::harness::AgentActivity::Compaction { completed: false },
            None,
        );
    }
    app.forget_agent_context(&send.owner, &send.agent_id);
    app.compactions.sent(&send.owner, &send.agent_id, sent);
    true
}
