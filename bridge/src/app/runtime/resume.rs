//! Putting the agents back after the binary was rolled under them.
//!
//! Two halves, at the two ends of a restart. At shutdown
//! [`AppState::resume_roster`] reads who was live or working off the agent
//! records and hands it to [`crate::resume::ResumeRoster`]. At boot
//! [`AppState::resume_recorded_agents`] reads that roster back, puts a notice
//! from Build on each agent's conversation, and queues the same turn the
//! `agent.start` button queues — so the relaunch is the spawn path that already
//! works, resume session id and all, rather than a second one to keep in step.

use crate::app::{
    AppState, DeliveryRunner, PendingAgentTurn, TurnText, NEW_THREAD_MESSAGES_PROMPT,
};
use crate::resume::{
    clear_opt_out_file, restart_notice, resume_is_wanted, ResumeRoster, ResumingAgent,
};
use crate::store::now_rfc3339;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// What a resume is timed under. It is not a relay method and not a tool call,
/// so it gets a name of its own rather than borrowing one.
const RESUME_METHOD: &str = "bridge.resume";

impl AppState {
    /// Write down who would have to come back, at the moment the daemon is
    /// going down. The whole of the shutdown half.
    ///
    /// Silent about its own failure beyond a line on stderr: a roster that
    /// could not be written costs the next boot its resume, and must not cost
    /// this shutdown its exit — the harnesses are already dying and the store
    /// is already durable.
    pub fn record_resume_roster(state: &Arc<Mutex<AppState>>, dir: &Path, version: &str) {
        if !resume_is_wanted(dir, |key| std::env::var(key).ok()) {
            eprintln!("resume: opted out of this roll; recording nobody");
            ResumeRoster::forget(dir);
            return;
        }
        let roster = state.lock().unwrap().resume_roster(version);
        let count = roster.agents.len();
        match roster.save(dir) {
            Ok(()) if count == 0 => eprintln!("resume: no agent was working; recorded nobody"),
            Ok(()) => eprintln!("resume: recorded {count} agent(s) to bring back"),
            Err(error) => eprintln!("resume: could not record the roster: {error}"),
        }
    }

    /// Read that roster back and bring its agents up. The whole of the boot
    /// half, and the only entry point the daemon calls.
    ///
    /// The roster is consumed whether or not it is acted on, and the per-roll
    /// marker with it: opting out of one roll must not opt out of the next, and
    /// a roster left on disk would resurrect the same agents at every boot
    /// from here on.
    pub fn resume_after_restart(
        state: &Arc<Mutex<AppState>>,
        dir: &Path,
        version: &str,
    ) -> Vec<String> {
        let wanted = resume_is_wanted(dir, |key| std::env::var(key).ok());
        clear_opt_out_file(dir);
        let Some(roster) = ResumeRoster::take(dir) else {
            return Vec::new();
        };
        if !wanted {
            eprintln!(
                "resume: opted out of this roll; {} agent(s) left as they are",
                roster.agents.len()
            );
            return Vec::new();
        }
        let clock = Arc::clone(&state.lock().unwrap().frame_clock);
        let timer = clock.frame(RESUME_METHOD);
        let resumed = timer.lock(state).resume_recorded_agents(&roster, version);
        DeliveryRunner::drain(state, &timer);
        if !resumed.is_empty() {
            eprintln!(
                "resume: brought back {} agent(s) that were working at {}",
                resumed.len(),
                roster.recorded_at
            );
        }
        resumed
    }

    /// Who a restart would have to bring back: every agent that was live, and
    /// every agent that was mid-turn.
    ///
    /// Both, because they are two different losses. A working agent loses a
    /// turn nobody will finish. A live one loses a session the human was in the
    /// middle of using, and finds an empty rail when they come back to it. An
    /// agent that was idle is left alone — it was waiting for somebody to speak
    /// before the roll and it can go on waiting after one.
    pub(crate) fn resume_roster(&self, version: &str) -> ResumeRoster {
        let mut agents = Vec::new();
        for entity_id in self.resumable_entity_ids() {
            let Ok(roster) = self.entity_agents(&entity_id) else {
                continue;
            };
            for agent in roster.iter() {
                let working = agent.working_since.is_some();
                if !working && agent.state != crate::agent::AgentLifecycle::Live {
                    continue;
                }
                agents.push(ResumingAgent {
                    entity_id: entity_id.clone(),
                    agent_id: agent.id.clone(),
                    conversation_id: agent
                        .conversation_id
                        .clone()
                        .unwrap_or_else(|| agent.id.clone()),
                    resume_session_id: agent.resume_session_id.clone(),
                    was_working: working,
                });
            }
        }
        ResumeRoster {
            recorded_at: now_rfc3339(),
            version: version.to_string(),
            agents,
        }
    }

    /// Every entity whose agents are worth recording: the runs and the plans,
    /// which between them own every conversation an agent can be on — a
    /// workspace's, a project's, and an issue's alike.
    fn resumable_entity_ids(&self) -> Vec<String> {
        self.runs.keys().chain(self.plans.keys()).cloned().collect()
    }

    /// Bring one roster's agents back, and tell each of them why it is awake.
    ///
    /// Returns the agents it actually queued, which is what the caller logs and
    /// what the tests read. An entry whose entity or agent is gone — deleted
    /// while the daemon was down, or on a workspace that no longer stands — is
    /// skipped rather than refused: a stale line in the roster must not cost
    /// the boot, or the agents beside it.
    ///
    /// The notice goes on the conversation BEFORE the turn is queued, so the
    /// catch-up packet the cold session is handed already contains it. An agent
    /// coming back cold reads it as the newest thing said to it, which is what
    /// it is.
    pub(crate) fn resume_recorded_agents(
        &mut self,
        roster: &ResumeRoster,
        version: &str,
    ) -> Vec<String> {
        let mut resumed = Vec::new();
        for entry in &roster.agents {
            match self.resume_one_agent(entry, &roster.recorded_at, version) {
                Ok(()) => resumed.push(entry.agent_id.clone()),
                Err(why) => eprintln!(
                    "resume: {} on {} was not brought back: {why}",
                    entry.agent_id, entry.entity_id
                ),
            }
        }
        resumed
    }

    fn resume_one_agent(
        &mut self,
        entry: &ResumingAgent,
        went_down_at: &str,
        version: &str,
    ) -> Result<(), String> {
        let addressed = self.addressed_agent(&serde_json::json!({
            "id": entry.entity_id,
            "agent_id": entry.agent_id,
        }))?;
        let notice = restart_notice(went_down_at, version, entry.was_working);
        let now = now_rfc3339();
        self.edit_agent_conversation(&entry.entity_id, &entry.agent_id, |thread, _| {
            thread.post_user_from_build(notice, &now);
            Ok(serde_json::Value::Null)
        })?;
        self.delivery_queue.enqueue(PendingAgentTurn {
            operation_id: None,
            root: addressed.root.clone(),
            owner: addressed.entity_id.clone(),
            agent_id: addressed.agent_id.clone(),
            conversation_id: addressed.conversation_id.clone(),
            model_choice: addressed.model_choice.clone(),
            choice_revision: addressed.choice_revision,
            interrupt: false,
            // The notice is already on the thread, so what the turn says is the
            // same nudge every other unread message gets: go and read it. A
            // resumed session is a new process with no memory of the turn that
            // died, so it gets the cold form — the protocol and the catch-up
            // packet — exactly as a hand-started agent does.
            say: Some(TurnText {
                cold: crate::orchestrator::conversation_prompt(NEW_THREAD_MESSAGES_PROMPT),
                warm: NEW_THREAD_MESSAGES_PROMPT.to_string(),
            }),
            phase: "resume",
            wants_catch_up: true,
            survives_refusal: false,
        });
        self.touch_attention(&addressed.entity_id);
        Ok(())
    }
}
