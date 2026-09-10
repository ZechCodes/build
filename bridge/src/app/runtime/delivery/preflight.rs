use crate::app::{
    ensure_agent_tab, still_pumping_instance, AgentSpawnRequest, AppState, DeliveryOutcome,
    DeliveryPreflight, LifecycleDiagnostic, PendingAgentTurn, Spawned, TabKey,
    TAB_CLOSED_UNDER_A_TURN,
};
use crate::harness::{Turn, TurnChoiceSupport};
use crate::store::now_rfc3339;
use crate::timing::FrameTimer;
use serde_json::Value;
use std::sync::{Arc, Mutex};

pub(in crate::app) const NEW_THREAD_MESSAGES_PROMPT: &str =
    "New reviewer messages are available. Call `read_unread_messages` now, then act on every unread message. Reply with `post_thread_message` only when the conversation policy requires a written response.";

/// What a read has to tell the agent about the indicator it just started.
/// Reading stamps `seen_at`, which is what the reviewer sees as "Working" with
/// a running timer; posting a reply (`still_working` omitted or false) hands the
/// turn back and stops it.
pub(in crate::app) const WORKING_INDICATOR_NOTICE: &str = "Reading these marked them seen, which started the reviewer's \"Working\" indicator and its timer on the newest message. It runs until you post a reply with post_thread_message — an ordinary reply (still_working omitted or false) hands the turn back and stops it; a progress note (still_working: true) keeps it running. The `done` tool also stops it. Do not leave it running after you have finished.";

/// Decide how one frozen turn reaches its exact captured destination before
/// any provider-facing operation occurs.
pub(in crate::app) fn preflight_delivery(
    state: &Arc<Mutex<AppState>>,
    turn: &PendingAgentTurn,
    timer: &FrameTimer,
) -> DeliveryPreflight {
    let key = turn.tab_key();
    let (plan, interrupt_session) = {
        let s = timer.lock(state);
        if !s.queued_agent_target_exists(turn) {
            return DeliveryPreflight::Declined;
        }
        let Some(tab) = s.tabs.get(&key) else {
            let force_fresh = s
                .resumable_session_id(
                    &turn.owner,
                    &turn.agent_id,
                    &turn.root,
                    turn.model_choice.provider,
                )
                .and_then(|named| {
                    s.agent_conversation(&turn.owner, Some(&turn.agent_id))
                        .ok()?
                        .sessions
                        .iter()
                        .rev()
                        .find(|session| {
                            session.agent_id == turn.agent_id
                                && session.resume_session_id.as_deref() == Some(named.as_str())
                        })
                        .map(|session| {
                            session.model != turn.model_choice.model
                                || session.effort != turn.model_choice.effort
                        })
                })
                .unwrap_or(false);
            return DeliveryPreflight::Proceed { force_fresh };
        };
        let exact_tab = tab
            .role
            .agent()
            .is_some_and(|(owner, agent_id)| owner == turn.owner && agent_id == turn.agent_id)
            && tab.session_instance.as_ref().is_some_and(|instance| {
                instance.conversation_id == turn.conversation_id
                    && instance.checkout == turn.root.display().to_string()
            });
        if !exact_tab {
            return DeliveryPreflight::Proceed { force_fresh: false };
        }
        let instance = tab
            .session_instance
            .as_ref()
            .expect("an exact agent tab has its session instance");
        if !tab.session_is_live() {
            return DeliveryPreflight::Proceed {
                force_fresh: !s.session_instance_uses_choice(instance, &turn.model_choice),
            };
        }
        if s.session_instance_uses_choice(instance, &turn.model_choice)
            || tab.session.turn_choice_support(&turn.model_choice) == TurnChoiceSupport::Native
        {
            return DeliveryPreflight::Proceed { force_fresh: false };
        }
        let working = s
            .entity_agents(&turn.owner)
            .ok()
            .and_then(|agents| agents.by_id(&turn.agent_id))
            .is_some_and(|agent| agent.working_since.is_some());
        if working && !turn.interrupt {
            return DeliveryPreflight::Deferred;
        }
        if turn.interrupt {
            let provider_thread_id = s.recorded_resume_id(&turn.owner, &turn.agent_id);
            tab.log_lifecycle(LifecycleDiagnostic {
                event: "interrupt_requested",
                origin: "delivery_preflight_model_change",
                reason: Some("replace_session_for_turn_choice"),
                operation_id: turn.operation_id.as_deref(),
                provider_thread_id: provider_thread_id.as_deref(),
                caller: None,
            });
        }
        (
            DeliveryPreflight::Proceed { force_fresh: true },
            turn.interrupt.then(|| Arc::clone(&tab.session)),
        )
    };
    if let Some(session) = interrupt_session {
        if let Err(refused) = session.interrupt() {
            eprintln!("thread.post {}: interrupt refused: {refused}", turn.owner);
        }
    }
    plan
}

/// The one pipe from Build to a worktree's agent.
///
/// Ensures the tab exists, then hands the agent the turn it was queued with —
/// a value the carrier decides how to say, which for a PTY is the harness's own
/// submit key and bracketed paste framing and never a raw write with a
/// hardcoded `\r`. Which half of the text travels is decided by whether the tab
/// had to be spawned; a turn that says nothing at all is a start, and the tab
/// existing is the whole of it. Returns the tab's wire id and which half
/// travelled — a `Fresh` delivery is a new agent process, which the
/// conversation records as the start of a session.
pub(in crate::app) fn deliver(
    state: &Arc<Mutex<AppState>>,
    turn: &PendingAgentTurn,
    timer: &FrameTimer,
) -> Result<DeliveryOutcome, String> {
    let PendingAgentTurn {
        root,
        owner,
        agent_id,
        model_choice,
        choice_revision,
        interrupt,
        phase,
        say,
        ..
    } = turn;
    let force_fresh = match preflight_delivery(state, turn, timer) {
        DeliveryPreflight::Proceed { force_fresh } => force_fresh,
        DeliveryPreflight::Deferred => return Ok(DeliveryOutcome::Deferred),
        DeliveryPreflight::Declined => return Ok(DeliveryOutcome::Delivered(None)),
    };
    let Some((wire_id, spawned)) = ensure_agent_tab(
        state,
        root,
        AgentSpawnRequest {
            owner,
            agent_id,
            conversation_id: &turn.conversation_id,
            model_choice,
            force_fresh,
            phase,
        },
        timer,
    )?
    else {
        return Ok(DeliveryOutcome::Delivered(None));
    };
    let Some(say) = say else {
        return Ok(DeliveryOutcome::Delivered(Some((wire_id, spawned))));
    };
    let prompt = match spawned {
        Spawned::Fresh => &say.cold,
        Spawned::Warm => &say.warm,
    };
    let key = TabKey::agent(&AppState::canonical_root(root), agent_id);
    // The handle comes out of the registry so the turn travels with the
    // app-wide state lock RELEASED: every RPC, every terminal pump and the idle
    // sweep wait on that lock, and how long a session takes to accept a turn is
    // its own business — a protocol write to a full pipe, an ack a harness
    // answers late, the exit-race wait below.
    let (session, instance) = {
        let s = timer.lock(state);
        if !s.queued_agent_target_exists(turn) {
            return Ok(DeliveryOutcome::Delivered(None));
        }
        let tab = s.tabs.get(&key).ok_or(TAB_CLOSED_UNDER_A_TURN)?;
        let exact_instance = tab.session_instance.as_ref().is_some_and(|instance| {
            instance.entity_id == turn.owner
                && instance.agent_id == turn.agent_id
                && instance.conversation_id == turn.conversation_id
        });
        if !exact_instance {
            return Ok(DeliveryOutcome::Delivered(None));
        }
        if *interrupt && spawned == Spawned::Warm {
            let provider_thread_id = s.recorded_resume_id(owner, agent_id);
            tab.log_lifecycle(LifecycleDiagnostic {
                event: "interrupt_requested",
                origin: "deliver_warm_turn",
                reason: Some("thread_post_interrupt"),
                operation_id: turn.operation_id.as_deref(),
                provider_thread_id: provider_thread_id.as_deref(),
                caller: None,
            });
        }
        (
            Arc::clone(&tab.session),
            tab.session_instance
                .clone()
                .expect("an exact delivery tab has its session instance"),
        )
    };
    if *interrupt && spawned == Spawned::Warm {
        if let Err(refused) = session.interrupt() {
            eprintln!("thread.post {owner}: interrupt refused: {refused}");
        }
    }
    let reports_turn_boundaries = session.status_changed().is_some();
    if let Err(error) = session.send_turn(&Turn::with_choice(
        prompt,
        model_choice.clone(),
        *choice_revision,
    )) {
        // A harness that exits immediately still owns its tab: PTYs return EIO
        // once the child's side is closed, and the child closes it BEFORE the
        // OS makes its exit status reapable, so a single poll here races the
        // kernel. The bounded wait covers that lag; a genuinely wedged session
        // (live but unwritable) still surfaces its error.
        if !session.exited_within(crate::orchestrator::PROMPT_WRITE_EXIT_GRACE) {
            return Err(error.to_string());
        }
    }
    // The quiescence clock restarts here: whatever the agent was silent about
    // before, it now has something to answer for. A tab that closed while the
    // turn was in flight has no clock left to restart — and the turn still
    // travelled, so that is not a delivery failure to report.
    let now = now_rfc3339();
    let mut app = timer.lock(state);
    if still_pumping_instance(&app, &key, &session, &instance) {
        let was_working = app
            .entity_agents(owner)
            .ok()
            .and_then(|agents| agents.by_id(agent_id))
            .is_some_and(|agent| agent.working_since.is_some());
        let tab = app
            .tabs
            .get_mut(&key)
            .expect("the exact delivered session is still registered");
        tab.last_delivered_at = Some(std::time::Instant::now());
        if !reports_turn_boundaries && !was_working {
            app.record_agent_working_since(owner, &turn.agent_id, Some(now.clone()));
            app.observe_working_state(owner, true, &now);
        }
    }
    Ok(DeliveryOutcome::Delivered(Some((wire_id, spawned))))
}

/// Which of the router's options the user tapped, named either by id or by
/// position — `None` when they typed an answer instead.
///
/// An id or an index that names nothing is refused rather than falling through
/// to the typed answer: a tap that misses is a tap the user thinks landed.
pub(in crate::app) fn chosen_option_id(
    capture: &crate::capture::Capture,
    params: &Value,
) -> Result<Option<String>, String> {
    if let Some(option_id) = params
        .get("option_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|option_id| !option_id.is_empty())
    {
        return Ok(Some(option_id.to_string()));
    }
    let Some(index) = params.get("option_index").and_then(Value::as_u64) else {
        return Ok(None);
    };
    capture
        .question
        .as_ref()
        .and_then(|question| question.option_at(index as usize))
        .map(|option| Some(option.id.clone()))
        .ok_or_else(|| format!("capture.answer: no option was offered at position {index}"))
}

impl AppState {
    // ---- Captures -------------------------------------------------------------

    // ---- Routing --------------------------------------------------------------

    /// The catch-up packet for one conversation: the last `limit` messages to
    /// and from the agent, read out of the store when the tail this process
    /// booted onto does not hold them.
    ///
    /// The gate is answered off two integers, so a conversation held whole —
    /// the common case, and every storeless test daemon — is byte-identical to
    /// what the tail alone said and touches no SQL. A store that cannot answer
    /// falls back to the tail-built packet: a starved packet is today's
    /// behaviour, and it is a far smaller loss than dropping the turn.
    pub(in crate::app) fn catch_up_packet(
        &self,
        thread: &crate::thread::Thread,
        limit: usize,
    ) -> String {
        if !thread.catch_up_reaches_stored_history(limit) {
            return thread.catch_up_markdown(limit);
        }
        let Some(store) = self.store.as_ref() else {
            return thread.catch_up_markdown(limit);
        };
        match store.thread_message_page(&thread.agent.id, limit) {
            Ok(history) => thread.catch_up_markdown_including_history(&history, limit),
            Err(error) => {
                eprintln!(
                    "catch-up packet for {}: {error}; the resident tail is what it carries",
                    thread.agent.id
                );
                thread.catch_up_markdown(limit)
            }
        }
    }

    /// The cold prompt a turn is actually handed over with: the prompt and its
    /// protocol block, closed with the durable conversation.
    ///
    /// Composed here, at the drain, rather than where the turn was built:
    /// transitions hold a conversation and no store, and a packet baked when
    /// the turn was queued would miss whatever was said while it waited for
    /// the lock. An owner whose conversation has gone (an entity closed under
    /// a queued turn) is handed the prompt as it stands — the delivery gate
    /// above has the last word on whether it travels at all.
    pub(in crate::app) fn cold_prompt_with_catch_up(
        &self,
        owner: &str,
        agent_id: &str,
        cold: &str,
    ) -> String {
        let Ok(thread) = self.agent_conversation(owner, Some(agent_id)) else {
            return cold.to_string();
        };
        crate::orchestrator::append_durable_conversation(
            cold.to_string(),
            &self.catch_up_packet(thread, crate::orchestrator::CATCH_UP_MESSAGES),
            thread,
        )
    }
}
