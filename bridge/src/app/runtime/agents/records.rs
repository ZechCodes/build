use crate::app::{
    activity_event_kind, open_session_id, open_session_lineage, record_session_death_in_thread,
    AppState, PendingAgentTurn, TabKey, AGENT_START_DECLINED_SESSION_OVER,
};
use crate::delivery::SessionProbes;
use crate::harness::{AgentSession, AgentStatus};
use crate::models::{AgentProvider, ModelChoice};
use crate::store::now_rfc3339;
use crate::thread::SessionInstance;
use std::sync::Arc;
use tokio::sync::broadcast;

pub(in crate::app) enum PumpWake {
    Reported(Result<crate::harness::ActivityReport, broadcast::error::RecvError>),
    SurfacesMoved,
    SurfacesUnwatchable,
}

/// What a session says about itself: the conversation it is having, and the
/// model it is running.
///
/// Read from the session, never through the registry. For a terminal the name
/// comes from a locator listing the harness's transcript tree — a filesystem
/// walk that grows with every conversation the human has ever had — so the
/// caller holds an `Arc` and asks with the app mutex released.
pub(in crate::app) struct SelfReport {
    pub(in crate::app) named: Option<String>,
    pub(in crate::app) model: Option<String>,
    pub(in crate::app) effort: Option<String>,
}

impl SelfReport {
    pub(in crate::app) fn read(session: &Arc<dyn AgentSession>) -> SelfReport {
        let (model, effort) = session.active_choice();
        SelfReport {
            named: session.session_id(),
            model,
            effort,
        }
    }
}

/// What the conversation reads when a harness died mid-turn.
pub(in crate::app) const SESSION_DIED_SUMMARY: &str =
    "The agent's session ended without reporting back";

/// What a call's row says when its answer never came, named by the boundary
/// that closed it. Pending is a claim too — "this is still running" — so a call
/// nothing will ever answer says which thing ended instead.
pub(in crate::app) const NO_ANSWER_TURN_ENDED: &str = "no answer — turn ended";

pub(in crate::app) const NO_ANSWER_SESSION_ENDED: &str = "no answer — session ended";

pub(in crate::app) fn record_activity(
    state: &mut AppState,
    key: &TabKey,
    owner: &str,
    agent_id: &str,
    report: &crate::harness::ActivityReport,
) {
    use crate::harness::AgentActivity;
    let parent_sequence = parent_row_sequence(state, key, report.parent_call_id.as_deref());
    match &report.activity {
        AgentActivity::Compaction { completed: true } => {
            let landed = state
                .edit_agent_conversation(owner, agent_id, |thread, _artifact| {
                    let session_id = open_session_id(thread, agent_id);
                    Ok(thread.resolve_compaction(session_id.as_deref()))
                })
                .unwrap_or(false);
            if !landed {
                state.record_agent_activity(owner, agent_id, &report.activity, parent_sequence);
            }
        }
        AgentActivity::ToolUse { call_id, .. } => {
            let minted =
                state.record_agent_activity(owner, agent_id, &report.activity, parent_sequence);
            if let Some(sequence) = minted {
                state
                    .session_registry
                    .insert_call_sequence(key, call_id.clone(), sequence);
            }
        }
        AgentActivity::ToolResult {
            call_id,
            outcome,
            summary,
        } => {
            let answer = match outcome {
                crate::harness::ToolOutcome::Unanswered => NO_ANSWER_TURN_ENDED,
                _ => summary.as_str(),
            };
            let landed = mark_call_answered(state, key, call_id).is_some_and(|sequence| {
                state.resolve_agent_tool_call(
                    owner,
                    agent_id,
                    sequence,
                    tool_call_outcome(*outcome),
                    answer,
                )
            });
            // An empty answer mints nothing, exactly as it never did: a row
            // saying only that some tool answered says nothing at all.
            if !landed && !answer.is_empty() {
                state.record_activity_row(
                    owner,
                    agent_id,
                    crate::thread::ThreadEventKind::ToolResult,
                    answer.to_string(),
                    parent_sequence,
                );
            }
        }
        _ => {
            state.record_agent_activity(owner, agent_id, &report.activity, parent_sequence);
        }
    }
}

pub(in crate::app) fn parent_row_sequence(
    state: &AppState,
    key: &TabKey,
    parent_call_id: Option<&str>,
) -> Option<u64> {
    let call_id = parent_call_id?;
    state.session_registry.parent_call_sequence(key, call_id)
}

pub(in crate::app) fn mark_call_answered(
    state: &mut AppState,
    key: &TabKey,
    call_id: &str,
) -> Option<u64> {
    state.session_registry.mark_call_answered(key, call_id)
}

/// The conversation's word for what the harness saw.
pub(in crate::app) fn tool_call_outcome(
    outcome: crate::harness::ToolOutcome,
) -> crate::thread::ToolCallOutcome {
    use crate::harness::ToolOutcome;
    use crate::thread::ToolCallOutcome;
    match outcome {
        ToolOutcome::Ok => ToolCallOutcome::Ok,
        ToolOutcome::Error => ToolCallOutcome::Error,
        ToolOutcome::Unanswered => ToolCallOutcome::Unanswered,
    }
}

impl AppState {
    /// A cold delivery started a new harness for `turn.owner`: open the
    /// conversation's session lineage for it, exactly as the phase spawner used
    /// to. A warm delivery never calls this — the session it continues is
    /// already open, and a second `start_session` would read back as an agent
    /// restart that never happened.
    ///
    /// `cwd` is where the process stands, which a provider files its
    /// conversation by: the checkout, for every agent but a project agent.
    pub(in crate::app) fn record_agent_session_start_in(
        &mut self,
        owner: &str,
        agent_id: &str,
        checkout: &std::path::Path,
        cwd: &std::path::Path,
        model_choice: &ModelChoice,
        phase: &str,
    ) -> Option<SessionInstance> {
        // A router owns no conversation — it decides which one the capture
        // becomes. What its session start records is that there is now a
        // process to have lost, which is what makes a dead one detectable.
        if let Some(session) = self.router_sessions.get_mut(owner) {
            session.mark_started();
            self.note_board_changed();
            return None;
        }
        let mut opened = None;
        let checkout = checkout.display().to_string();
        let cwd = cwd.display().to_string();
        if let Err(error) = self.edit_agent_conversation(owner, agent_id, |thread, _artifact| {
            opened = Some(open_session_lineage(
                thread,
                owner,
                agent_id,
                &checkout,
                &cwd,
                model_choice,
                phase,
            ));
            Ok(())
        }) {
            eprintln!("record_agent_session_start {owner}: {error}");
        }
        self.note_board_changed();
        opened
    }

    /// A session started in its own checkout.
    #[cfg(test)]
    pub(in crate::app) fn record_agent_session_start(
        &mut self,
        owner: &str,
        agent_id: &str,
        checkout: &std::path::Path,
        model_choice: &ModelChoice,
        phase: &str,
    ) -> Option<SessionInstance> {
        self.record_agent_session_start_in(owner, agent_id, checkout, checkout, model_choice, phase)
    }

    /// The agent process an id owns has ended: close the conversation's session
    /// lineage for it, and close the turn it died holding. The mirror of
    /// [`record_agent_session_start_in`](Self::record_agent_session_start_in), and it
    /// is the PUMP that calls it — the only place that learns a harness died on
    /// its own. An owner that no longer exists (its record was deleted with the
    /// tab) has no lineage left to close, which is why this is quiet.
    pub(in crate::app) fn record_agent_session_end(
        &mut self,
        owner: &str,
        agent_id: &str,
        instance: &SessionInstance,
    ) {
        let now = now_rfc3339();
        let mut ended_current = false;
        if let Err(error) = self.edit_agent_conversation(owner, agent_id, |thread, _artifact| {
            let was_current = thread.open_session_instance(agent_id).as_ref() == Some(instance);
            ended_current = thread.finish_session_instance(instance, &now) && was_current;
            Ok(())
        }) {
            eprintln!("record_agent_session_end {owner}: {error}");
        }
        if !ended_current {
            return;
        }
        self.close_turn_of_dead_agent(owner, agent_id);
        self.record_agent_working_since(owner, agent_id, None);
        if !self.entity_agents_working(owner)
            && self
                .board
                .attention_mut()
                .observe_working(owner, false, &now)
        {
            self.persist_attention();
        }
        // Agent liveness is feed state, and it is the one kind that moves with
        // no verb behind it: the pump learned a harness died. Noted here rather
        // than left to the mutation tails above, which do nothing at all for an
        // owner whose record is already gone.
        self.note_board_changed();
    }

    /// `agent_id`'s process is gone. If it died mid-turn, say so on the
    /// conversation that agent speaks in — which closes the turn, and with it
    /// the row's and the bubble's claim that work is in flight.
    ///
    /// The conversation is chosen exactly the way
    /// [`agent_conversation`](Self::agent_conversation) reads it: through the
    /// addressed agent's stable binding. A turn that is already closed —
    /// the agent replied, reported `done`, or was told the branch was abandoned
    /// — is left alone: this marker exists only for a turn nobody else will
    /// ever close.
    pub(in crate::app) fn close_turn_of_dead_agent(&mut self, owner: &str, agent_id: &str) {
        let died_mid_turn = self
            .entity_agents(owner)
            .ok()
            .and_then(|agents| agents.by_id(agent_id))
            .is_some_and(|agent| agent.working_since.is_some());
        if !died_mid_turn {
            return;
        }
        let now = now_rfc3339();
        if let Err(error) = self.edit_agent_conversation(owner, agent_id, |thread, _artifact| {
            record_session_death_in_thread(thread, &now);
            Ok(())
        }) {
            eprintln!("close_turn_of_dead_agent {owner}: {error}");
        }
    }

    /// Apply `edit` to `agent_id`'s own RECORD — what the agent IS, not what it
    /// said — and persist the entity that owns it.
    ///
    /// Separate from [`edit_agent_conversation`](Self::edit_agent_conversation), which edits
    /// the conversation an agent SPEAKS in: for a planned implementation's
    /// first agent that is the Task's thread, which is not the agent.
    ///
    /// Quiet about an owner or agent it cannot find: a record deleted with its
    /// tab has no roster left to write onto.
    pub(in crate::app) fn edit_agent_record(
        &mut self,
        context: &str,
        owner: &str,
        agent_id: &str,
        edit: impl FnOnce(&mut crate::agent::Agent),
    ) {
        if self.plans.contains_key(owner) {
            let Ok(mut active) = self.take_plan(owner) else {
                return;
            };
            if let Some(agent) = active.agents.by_id_mut(agent_id) {
                edit(agent);
            }
            if let Err(error) = self.finish_plan_mutation(owner.to_string(), active) {
                eprintln!("{context} {owner}: {error}");
            }
            return;
        }
        let Ok(mut active) = self.take_run(owner) else {
            return;
        };
        if let Some(agent) = active.agents.by_id_mut(agent_id) {
            edit(agent);
        }
        if let Err(error) = self.finish_run_mutation(owner.to_string(), active) {
            eprintln!("{context} {owner}: {error}");
        }
    }

    /// The transcript-tree reads a spawn makes, taken together so the
    /// disk work can be handed over in one piece.
    pub(in crate::app) fn session_probes(&self) -> SessionProbes {
        SessionProbes {
            resume_id: Arc::clone(&self.resume_id_probe),
            locator: Arc::clone(&self.session_locator_factory),
        }
    }

    /// The name the agent's record says its conversation has — `None` for one
    /// no session of its has ever announced.
    pub(in crate::app) fn recorded_resume_id(&self, owner: &str, agent_id: &str) -> Option<String> {
        self.entity_agents(owner)
            .ok()?
            .by_id(agent_id)?
            .resume_session_id
            .clone()
    }

    /// An exact provider id is resumable only when persisted lineage binds it
    /// to this agent, provider, and checkout, and to the directory the process
    /// about to start will stand in: a provider files a conversation by where
    /// it was had, so one had elsewhere — a project agent's, from before it
    /// moved into its project's base — is not picked up from here, however
    /// the provider would choose between two directories' copies of it.
    /// Legacy/partial records start fresh and catch up from canonical history
    /// instead of guessing.
    pub(in crate::app) fn resumable_session_id(
        &self,
        owner: &str,
        agent_id: &str,
        root: &std::path::Path,
        provider: AgentProvider,
    ) -> Option<String> {
        let named = self.recorded_resume_id(owner, agent_id)?;
        let checkout = root.display().to_string();
        let cwd = self.agent_process_cwd(owner, root).display().to_string();
        self.agent_conversation(owner, Some(agent_id))
            .ok()?
            .sessions
            .iter()
            .rev()
            .any(|session| {
                session.agent_id == agent_id
                    && session.checkout.as_deref() == Some(checkout.as_str())
                    && session.stood_in() == Some(cwd.as_str())
                    && session.provider == provider.label()
                    && session.resume_session_id.as_deref() == Some(named.as_str())
            })
            .then_some(named)
    }

    /// Whether a queued turn still names the same executable agent and
    /// canonical conversation it named when accepted.
    ///
    /// This is deliberately stricter than owner liveness. A drained batch can
    /// outlive `agent.remove`, and a Task implementation can share history
    /// with its Task while retaining a distinct process identity. Neither may
    /// be reconstructed from roster position after the turn left the queue.
    pub(in crate::app) fn queued_agent_target_exists(&self, turn: &PendingAgentTurn) -> bool {
        self.agent_target_exists(
            &turn.owner,
            &turn.agent_id,
            &turn.conversation_id,
            &turn.root,
        )
    }

    pub(in crate::app) fn agent_target_exists(
        &self,
        owner: &str,
        agent_id: &str,
        conversation_id: &str,
        root: &std::path::Path,
    ) -> bool {
        if crate::router::is_router_agent(agent_id) {
            return self.router_sessions.get(owner).is_some_and(|session| {
                session.agent_id() == agent_id
                    && conversation_id == agent_id
                    && Self::canonical_root(session.scratch_dir()) == root
            });
        }
        let Ok(address) = self.resolve_conversation_address(owner, Some(agent_id)) else {
            return false;
        };
        address.conversation_id == conversation_id
            && self
                .entity_agent_root(owner)
                .is_ok_and(|actual| Self::canonical_root(&actual) == root)
    }

    /// Whether the exact process behind `instance` was launched with `choice`.
    pub(in crate::app) fn session_instance_uses_choice(
        &self,
        instance: &SessionInstance,
        choice: &ModelChoice,
    ) -> bool {
        self.agent_conversation(&instance.entity_id, Some(&instance.agent_id))
            .ok()
            .and_then(|thread| {
                thread
                    .sessions
                    .iter()
                    .find(|session| session.id == instance.id)
            })
            .is_some_and(|session| {
                session.conversation_id == instance.conversation_id
                    && session.checkout.as_deref().unwrap_or_default() == instance.checkout
                    && session.provider == choice.provider.label()
                    && session.model == choice.model
                    && session.effort == choice.effort
            })
    }

    /// Write down the name the agent's live session gave its conversation, so
    /// the next spawn resumes it BY NAME instead of guessing the newest
    /// transcript in the checkout.
    ///
    /// `None` clears it, which is what a session that ended having never
    /// announced one asks for: that is the shape of a spawn whose `--resume`
    /// id no longer resolved, and clearing costs one restart where keeping it
    /// would cost every restart.
    pub(in crate::app) fn record_agent_resume_id(
        &mut self,
        owner: &str,
        agent_id: &str,
        named: Option<String>,
    ) {
        self.edit_agent_record("record_agent_resume_id", owner, agent_id, |agent| {
            agent.resume_session_id = named;
        });
    }

    pub(in crate::app) fn recorded_active_model(
        &self,
        owner: &str,
        agent_id: &str,
    ) -> Option<String> {
        self.entity_agents(owner)
            .ok()?
            .by_id(agent_id)?
            .active_model
            .clone()
    }

    pub(in crate::app) fn recorded_active_effort(
        &self,
        owner: &str,
        agent_id: &str,
    ) -> Option<String> {
        self.entity_agents(owner)
            .ok()?
            .by_id(agent_id)?
            .active_effort
            .clone()
    }

    /// A turn is on its way to this agent, so why the LAST one never arrived is
    /// history: the row the client is about to wear a "starting" state on must
    /// not be answered by the failure before it.
    ///
    /// Written only when there is one to forget, so an ordinary delivery costs
    /// no store write.
    pub(in crate::app) fn forget_agent_start_error(&mut self, owner: &str, agent_id: &str) {
        let recorded = self
            .entity_agents(owner)
            .ok()
            .and_then(|roster| roster.by_id(agent_id))
            .and_then(|agent| agent.start_error.as_ref());
        if recorded.is_none() {
            return;
        }
        self.edit_agent_record("forget_agent_start_error", owner, agent_id, |agent| {
            agent.start_error = None;
        });
    }

    /// Build ended the agent's session before it ran a turn (task #72: a child
    /// that opened another model than the agent asks for). The reason is the
    /// agent's `start_error`, so the row says why it never started instead of
    /// sitting idle with nothing on it.
    pub(in crate::app) fn record_agent_start_refused(
        &mut self,
        owner: &str,
        agent_id: &str,
        reason: &str,
    ) {
        eprintln!("agent start_refused: agent={agent_id} owner={owner} {reason}");
        self.edit_agent_record("record_agent_start_refused", owner, agent_id, |agent| {
            agent.start_error = Some(reason.to_string());
        });
    }

    /// What the agent says its conversation is about, written onto its record
    /// and answered back so the tool result confirms what the header now wears.
    pub(in crate::app) fn set_agent_topic(
        &mut self,
        owner: &str,
        agent_id: &str,
        topic: &str,
    ) -> Result<serde_json::Value, String> {
        let mut found = false;
        self.edit_agent_record("set_topic", owner, agent_id, |agent| {
            found = true;
            agent.topic = Some(topic.to_string());
        });
        if !found {
            return Err(format!("agent {agent_id} is not on {owner}"));
        }
        Ok(serde_json::json!({ "topic": topic }))
    }

    /// What to call this agent, written onto its record once it is nobody
    /// else's name.
    ///
    /// Unique among the agents of this conversation, case- and
    /// spacing-insensitively: the name exists to tell two agents apart in a
    /// rail, and two agents wearing one name would be worse than the ordinals
    /// it replaces. An agent renaming itself to what it is already called is
    /// not a clash — it is a no-op somebody asked for twice.
    ///
    /// The refusal is a sentence, because the agent reads it and repeats it to
    /// the user.
    pub(in crate::app) fn set_agent_name(
        &mut self,
        owner: &str,
        agent_id: &str,
        name: &str,
    ) -> Result<serde_json::Value, String> {
        let taken = self.entity_agents(owner)?.iter().any(|agent| {
            agent.id != agent_id
                && agent
                    .name
                    .as_deref()
                    .is_some_and(|theirs| crate::agent::same_agent_name(theirs, name))
        });
        if taken {
            return Err(format!(
                "Another agent on this conversation is already called \"{name}\". Pick a different name."
            ));
        }
        let mut found = false;
        self.edit_agent_record("set_name", owner, agent_id, |agent| {
            found = true;
            agent.name = Some(name.to_string());
            // Named is named: the ask has nothing left to ask for, and a
            // record that still wanted it would ask an agent to name what it
            // just called itself.
            agent.name_asked = true;
        });
        if !found {
            return Err(format!("agent {agent_id} is not on {owner}"));
        }
        Ok(serde_json::json!({ "name": name }))
    }

    /// Reserve the one request to name an unnamed coding agent as its turn is
    /// being sent. The project's agent has no set_name tool and is exempt.
    pub(in crate::app) fn claim_agent_name_request(&mut self, owner: &str, agent_id: &str) -> bool {
        if crate::agent::is_project_agent(agent_id) {
            return false;
        }
        let needs_name = self
            .entity_agents(owner)
            .ok()
            .and_then(|agents| agents.by_id(agent_id))
            .is_some_and(|agent| agent.name.is_none() && !agent.name_asked);
        if needs_name {
            self.edit_agent_record("ask_agent_to_name_itself", owner, agent_id, |agent| {
                agent.name_asked = true;
            });
        }
        needs_name
    }

    /// A rejected provider write was never an ask. Keep the next turn able to
    /// carry it, unless the agent named itself while that write was in flight.
    pub(in crate::app) fn release_agent_name_request(&mut self, owner: &str, agent_id: &str) {
        self.edit_agent_record("retry_agent_name_request", owner, agent_id, |agent| {
            if agent.name.is_none() {
                agent.name_asked = false;
            }
        });
    }

    /// `conversation.watch` / `conversation.unwatch` — whether this
    /// conversation is in the user's inbox.
    ///
    /// Unwatching is what Mute means on its row: the row goes and nothing else
    /// changes, so the agent keeps working and the user keeps not hearing
    /// about it.
    pub(crate) fn set_conversation_watched(
        &mut self,
        owner: &str,
        agent_id: &str,
        watching: bool,
    ) -> Result<serde_json::Value, String> {
        let mut found = false;
        self.edit_agent_record("set_conversation_watched", owner, agent_id, |agent| {
            found = true;
            agent.watched = watching;
        });
        if !found {
            return Err(format!("agent {agent_id} is not on {owner}"));
        }
        Ok(serde_json::json!({ "agent_id": agent_id, "watched": watching }))
    }

    /// `conversation.settings` — the context size this conversation compacts
    /// at: `None` follows the device's `compact_above_tokens`, `Some(0)` never
    /// compacts. Answers the limit as stored and the threshold in effect.
    pub(crate) fn set_conversation_context_limit(
        &mut self,
        owner: &str,
        agent_id: &str,
        max_context_tokens: Option<u64>,
    ) -> Result<serde_json::Value, String> {
        let device_threshold = self.compact_above_tokens;
        let mut compact_at_tokens = None;
        self.edit_agent_record("set_conversation_context_limit", owner, agent_id, |agent| {
            agent.max_context_tokens = max_context_tokens;
            compact_at_tokens = Some(agent.compact_at_tokens(device_threshold));
        });
        let Some(compact_at_tokens) = compact_at_tokens else {
            return Err(format!("agent {agent_id} is not on {owner}"));
        };
        Ok(serde_json::json!({
            "agent_id": agent_id,
            "max_context_tokens": max_context_tokens,
            "compact_at_tokens": compact_at_tokens,
        }))
    }

    #[cfg(test)]
    pub(in crate::app) fn record_agent_active_model(
        &mut self,
        owner: &str,
        agent_id: &str,
        running: Option<String>,
    ) {
        if self.recorded_active_model(owner, agent_id) == running {
            return;
        }
        self.edit_agent_record("record_agent_active_model", owner, agent_id, |agent| {
            agent.active_model = running;
        });
    }

    pub(in crate::app) fn record_agent_runtime_choice(
        &mut self,
        owner: &str,
        agent_id: &str,
        model: Option<String>,
        effort: Option<String>,
    ) {
        let unchanged = self
            .entity_agents(owner)
            .ok()
            .and_then(|agents| agents.by_id(agent_id))
            .is_some_and(|agent| agent.active_model == model && agent.active_effort == effort);
        if unchanged {
            return;
        }
        self.edit_agent_record("record_agent_runtime_choice", owner, agent_id, |agent| {
            agent.active_model = model;
            agent.active_effort = effort;
        });
    }

    /// Set the current execution clock on the addressed agent record, never on
    /// the canonical conversation it may share with another agent.
    pub(in crate::app) fn record_agent_working_since(
        &mut self,
        owner: &str,
        agent_id: &str,
        working_since: Option<String>,
    ) {
        let unchanged = self
            .entity_agents(owner)
            .ok()
            .and_then(|agents| agents.by_id(agent_id))
            .is_some_and(|agent| agent.working_since == working_since);
        if unchanged {
            return;
        }
        self.edit_agent_record("record_agent_working_since", owner, agent_id, |agent| {
            agent.working_since = working_since;
        });
    }

    /// Write down what the agent's session said its last turn cost in
    /// context, and when it said so. Compared before it is written, so a snapshot that repeats the
    /// reading costs nothing. The first reading after a compaction Build sent
    /// is also that compaction's size afterwards.
    pub(in crate::app) fn record_agent_turn_context(
        &mut self,
        owner: &str,
        agent_id: &str,
        context: crate::harness::TurnContext,
    ) {
        self.stamp_sent_compaction(owner, agent_id, context.context_tokens);
        let unchanged = self
            .entity_agents(owner)
            .ok()
            .and_then(|agents| agents.by_id(agent_id))
            .is_some_and(|agent| {
                agent.last_context_tokens == Some(context.context_tokens)
                    && agent.session_cache_read_tokens == Some(context.cache_read_tokens)
            });
        if unchanged {
            return;
        }
        let now = crate::store::now_rfc3339();
        self.edit_agent_record("record_agent_turn_context", owner, agent_id, |agent| {
            agent.last_context_tokens = Some(context.context_tokens);
            agent.last_context_at = Some(now);
            agent.session_cache_read_tokens = Some(context.cache_read_tokens);
        });
    }

    /// Forget the context an agent's last turn left, once a compaction has
    /// been asked for: the reading describes a context that is going away, and
    /// one left standing would ask for the compaction again.
    pub(in crate::app) fn forget_agent_context(&mut self, owner: &str, agent_id: &str) {
        self.edit_agent_record("forget_agent_context", owner, agent_id, |agent| {
            agent.last_context_tokens = None;
            agent.last_context_at = None;
        });
    }

    /// Start one agent's execution interval without moving an interval already
    /// in flight. A second read or a turn queued onto a native session is more
    /// work for the same execution, not a new start time.
    pub(in crate::app) fn start_agent_working(&mut self, owner: &str, agent_id: &str, now: &str) {
        let already_working = self
            .entity_agents(owner)
            .ok()
            .and_then(|agents| agents.by_id(agent_id))
            .is_some_and(|agent| agent.working_since.is_some());
        if !already_working {
            self.record_agent_working_since(owner, agent_id, Some(now.to_string()));
        }
    }

    /// Apply one provider-reported boundary to the exact agent whose session
    /// emitted it. The entity attention clock remains the aggregate of all
    /// agent intervals, while the execution clock itself never moves onto the
    /// canonical conversation another agent may share.
    pub(in crate::app) fn record_agent_status_snapshot(
        &mut self,
        owner: &str,
        agent_id: &str,
        snapshot: &crate::harness::SessionStatusSnapshot,
    ) {
        let working_since =
            matches!(snapshot.status, AgentStatus::Working).then(|| snapshot.changed_at.clone());
        self.record_agent_working_since(owner, agent_id, working_since);
        let working = self.entity_agents_working(owner);
        if self.board.attention_mut().observe_status(
            owner,
            working,
            &snapshot.changed_at,
            snapshot.last_worked_at.as_deref(),
        ) {
            self.persist_attention();
            self.note_entity_changed(owner);
        }
    }

    /// Post one thing the agent reported doing into the conversation it speaks
    /// in.
    ///
    /// Activity is conversation: an event-stream harness has no second tab and
    /// no second scrollback, so its reasoning, tool calls and narration ride the
    /// timeline the human already reads, told apart from what the agent SAID by
    /// class rather than by living somewhere else.
    ///
    /// Quiet about an owner it cannot find, for the reason
    /// [`edit_owner_thread`](Self::edit_owner_thread) is: a router owns no
    /// conversation, and an entity whose record was deleted with its tab has
    /// none left to speak in. Neither is worth a line per tool call.
    pub(in crate::app) fn record_agent_activity(
        &mut self,
        owner: &str,
        agent_id: &str,
        activity: &crate::harness::AgentActivity,
        parent_sequence: Option<u64>,
    ) -> Option<u64> {
        self.record_activity_row(
            owner,
            agent_id,
            activity_event_kind(activity),
            activity.summary().to_string(),
            parent_sequence,
        )
    }

    /// Mint one activity row, and hand back the counter value it was minted at
    /// — the handle a tool call's answer comes back on. `None` for an owner
    /// with no conversation to speak in, for the reason above.
    pub(in crate::app) fn record_activity_row(
        &mut self,
        owner: &str,
        agent_id: &str,
        event: crate::thread::ThreadEventKind,
        summary: String,
        parent_sequence: Option<u64>,
    ) -> Option<u64> {
        let now = now_rfc3339();
        self.edit_agent_conversation(owner, agent_id, |thread, _artifact| {
            let session_id = open_session_id(thread, agent_id);
            Ok(thread.push_drafted_event(
                crate::thread::ThreadEventDraft {
                    event,
                    summary: Some(summary),
                    session_id,
                    revision_id: None,
                    links: Vec::new(),
                    parent_sequence,
                },
                now,
            ))
        })
        .ok()
    }

    /// Land a tool call's answer on the row the call minted, closing it.
    ///
    /// `false` when that row is not there to be closed — the conversation is
    /// gone, or a reload left the call under the resident tail — which is what
    /// sends the pump back to minting the answer as a row of its own rather
    /// than losing it.
    pub(in crate::app) fn resolve_agent_tool_call(
        &mut self,
        owner: &str,
        agent_id: &str,
        sequence: u64,
        outcome: crate::thread::ToolCallOutcome,
        answer: &str,
    ) -> bool {
        self.edit_agent_conversation(owner, agent_id, |thread, _artifact| {
            Ok(thread.resolve_tool_call(sequence, outcome, answer))
        })
        .unwrap_or(false)
    }

    /// Edit the conversation `agent_id` speaks in, and persist the entity that
    /// owns it.
    ///
    /// Which conversation that is has one rule and this is where it lives: the
    /// addressed agent's persisted binding names the storage owner. The artifact
    /// that conversation is about travels with the addressed entity, since a
    /// plan's links resolve against a document and a run's against a diff.
    ///
    /// The record is persisted whether the edit succeeded or not, and the
    /// edit's own error is what the caller hears: a rejected action must not
    /// take the writes made before it down with it.
    pub(in crate::app) fn edit_agent_conversation<T>(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        edit: impl FnOnce(&mut crate::thread::Thread, crate::thread::ArtifactKind) -> Result<T, String>,
    ) -> Result<T, String> {
        let address = self.resolve_conversation_address(entity_id, Some(agent_id))?;
        if self.plans.contains_key(&address.conversation_entity_id) {
            let mut active = self.take_plan(&address.conversation_entity_id)?;
            let result = active
                .agents
                .resolve_mut(Some(&address.conversation_id))
                .and_then(|agent| edit(&mut agent.thread, address.artifact));
            let persisted =
                self.finish_plan_mutation(address.conversation_entity_id.clone(), active);
            let value = result?;
            persisted?;
            return Ok(value);
        }
        if self.runs.contains_key(&address.conversation_entity_id) {
            let mut active = self.take_run(&address.conversation_entity_id)?;
            let result = active
                .agents
                .resolve_mut(Some(&address.conversation_id))
                .and_then(|agent| edit(&mut agent.thread, address.artifact));
            let persisted =
                self.finish_run_mutation(address.conversation_entity_id.clone(), active);
            let value = result?;
            persisted?;
            return Ok(value);
        }
        Err(format!("unknown conversation owner: {entity_id}"))
    }

    /// A turn never reached an agent: record why on the entity and on the agent
    /// itself, and persist it, so the surface says what happened instead of
    /// showing a working task with nobody working and a row still starting.
    ///
    /// The state is deliberately left alone. The transition that queued this
    /// turn is already durable, and demotion belongs to one place — the idle
    /// sweep, which now reads a working entity with no agent tab as the anomaly
    /// it is. This method's whole job is the reason.
    pub(in crate::app) fn record_agent_delivery_failure(
        &mut self,
        turn: &PendingAgentTurn,
        error: &str,
    ) {
        // A router that never reached a harness is a route that failed, and the
        // capture is where that has to show — there is no entity behind it to
        // carry the reason.
        if self.router_sessions.contains_key(&turn.owner) {
            eprintln!("router {}: {error}", turn.owner);
            self.settle_router_session(&turn.owner);
            return;
        }
        self.record_agent_start_error(turn, format!("could not reach the agent: {error}"));
    }

    /// A turn's agent was not started because the installed CLI cannot run
    /// its model: `why` is already the sentence the person who chose that
    /// model reads, and says what to do.
    pub(in crate::app) fn record_agent_start_unrunnable(
        &mut self,
        turn: &PendingAgentTurn,
        why: &str,
    ) {
        if self.router_sessions.contains_key(&turn.owner) {
            eprintln!("router {}: {why}", turn.owner);
            self.settle_router_session(&turn.owner);
            return;
        }
        self.record_agent_start_error(turn, why.to_string());
    }

    fn record_agent_start_error(&mut self, turn: &PendingAgentTurn, reason: String) {
        // Both facts land in the one mutation. The entity's `last_error` is
        // the surface's line about the work; the agent's `start_error` is its
        // own word about the session it was asked to open, which is what the
        // client laid a "starting" state over the row waiting for.
        // Plan and run ids are disjoint, so the owner lookup is the router.
        if self.plans.contains_key(&turn.owner) {
            let Ok(mut active) = self.take_plan(&turn.owner) else {
                return;
            };
            if let Some(agent) = active.agents.by_id_mut(&turn.agent_id) {
                agent.start_error = Some(reason.clone());
            }
            active.last_error = Some(reason);
            let persisted = self.finish_plan_mutation(turn.owner.clone(), active);
            if let Err(error) = persisted {
                eprintln!("record_agent_delivery_failure {}: {error}", turn.owner);
            }
            return;
        }
        let Ok(mut active) = self.take_run(&turn.owner) else {
            return;
        };
        if let Some(agent) = active.agents.by_id_mut(&turn.agent_id) {
            agent.start_error = Some(reason.clone());
        }
        active.last_error = Some(reason);
        let persisted = self.finish_run_mutation(turn.owner.clone(), active);
        if let Err(error) = persisted {
            eprintln!("record_agent_delivery_failure {}: {error}", turn.owner);
        }
    }

    /// A turn found no session to open: the entity's session is over, so no
    /// agent is spawned for it. Not a failure of the work — the entity's
    /// `last_error` is left alone — but the client laid a "starting" state
    /// over the agent it addressed, and only the agent's own word takes it off.
    pub(in crate::app) fn record_agent_start_declined(&mut self, turn: &PendingAgentTurn) {
        self.edit_agent_record(
            "record_agent_start_declined",
            &turn.owner,
            &turn.agent_id,
            |agent| agent.start_error = Some(AGENT_START_DECLINED_SESSION_OVER.to_string()),
        );
    }

    /// Is this entity's agent merely on its way — a turn still queued, or one
    /// off the queue and mid-delivery? Between the verb that transitions an
    /// entity (under the state lock) and the tab its turn spawns (lock free,
    /// seconds for a cold harness), a working entity has no agent tab and is
    /// perfectly healthy. Everywhere else, a working entity without one is an
    /// anomaly.
    pub(in crate::app) fn agent_turn_is_undelivered(&self, owner: &str) -> bool {
        self.delivery_queue.holds_owner(owner)
    }

    /// Is a turn that will TELL this agent to read its thread already coming?
    ///
    /// Asked by every verb that would otherwise queue a SECOND turn for it: two
    /// harnesses in one checkout both report `done` for the same owner, and
    /// even where the spawn claim prevents that, the second turn survives as a
    /// duplicate `read_unread_messages` nudge. Keyed per (root, agent), so a
    /// branch's second agent is never suppressed by its first agent's turn.
    ///
    /// Only a turn with words counts. The verb that asks is about to leave a
    /// message durable on the thread, and a harness that opens on a cold
    /// prompt is told to call `read_unread_messages` — so that turn reads the
    /// message, and a second turn is a duplicate. A turn that says nothing
    /// (`agent.start` with nothing unread) opens a harness and sends it
    /// nothing: it promises the agent nothing to read, so the message has to
    /// queue its own turn, which lands Warm on the tab the start opened. The
    /// spawn claim is not consulted: a textful delivery gives its mark back
    /// only after settling the claim it became, so the mark already covers the
    /// claim's whole lifetime, and a claim with no textful mark behind it is a
    /// textless spawn.
    ///
    /// Two states, then, and a turn with words is never in neither: queued,
    /// and taken off the queue and mid-delivery.
    pub(in crate::app) fn agent_is_on_its_way(
        &self,
        root: &std::path::Path,
        agent_id: &str,
    ) -> bool {
        let key = TabKey::agent(&Self::canonical_root(root), agent_id);
        self.delivery_queue.holds_agent(&key)
    }
}
