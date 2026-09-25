use crate::app::{
    off_the_workers, record_activity, AppState, DeliveryRunner, LifecycleDiagnostic, PumpWake,
    SelfReport, TabKey, TabPumps, TabRole, AGENT_DELIVERY_METHOD, NO_ANSWER_SESSION_ENDED,
};
use crate::harness::{AgentSession, AgentStatus};
use crate::screen::{ScreenHandle, TERM_FLUSH_MS};
use crate::thread::SessionInstance;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;

/// Start whichever pump this tab's session needs: the byte pump for a terminal,
/// the activity pump for a session that reports its own work.
///
/// One or the other and never both, because the two capabilities are
/// alternatives — and never neither, because the death rites hang off a stream
/// closing ([`open_session`] refuses a session with no stream at all).
pub(in crate::app) fn spawn_tab_pumps(state: &Arc<Mutex<AppState>>, key: TabKey, pumps: TabPumps) {
    let TabPumps {
        session,
        session_instance,
        screen,
        output,
        push_runtime,
    } = pumps;
    super::delivery::receipts::spawn_receipt_pump(state, &session, session_instance.clone());
    spawn_tab_pump(
        state,
        key.clone(),
        Arc::clone(&session),
        session_instance.clone(),
        BytePump {
            screen,
            rx: output.bytes,
            push_runtime,
        },
    );
    let status_changed = session.status_changed();
    spawn_activity_pump(
        state,
        key.clone(),
        Arc::clone(&session),
        session_instance.clone(),
        output.activity,
        output.surfaces,
    );
    spawn_status_pump(state, key, session, session_instance, status_changed);
}

pub(in crate::app) fn spawn_status_pump(
    state: &Arc<Mutex<AppState>>,
    key: TabKey,
    session: Arc<dyn AgentSession>,
    instance: Option<SessionInstance>,
    mut changed: Option<tokio::sync::watch::Receiver<crate::harness::SessionStatusSnapshot>>,
) {
    let (Some(mut changed), Some(instance)) = (changed.take(), instance) else {
        return;
    };
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    let mut pump = StatusPump {
        state: Arc::downgrade(state),
        key,
        session: Arc::downgrade(&session),
        instance,
        recorded_context: None,
        recorded_limit: super::usage_limits::UsageObservation::default(),
    };
    tokio::spawn(async move {
        loop {
            let snapshot = changed.borrow_and_update().clone();
            let still_watching;
            (pump, still_watching) = off_the_workers(move || {
                let still_watching = pump.observe(&snapshot);
                (pump, still_watching)
            })
            .await;
            if !still_watching || changed.changed().await.is_err() {
                return;
            }
        }
    });
}

/// What the status pump carries from one snapshot to the next. It travels to
/// the blocking pool and back with each snapshot, because writing a snapshot
/// down takes the app mutex.
struct StatusPump {
    state: std::sync::Weak<Mutex<AppState>>,
    key: TabKey,
    session: std::sync::Weak<dyn AgentSession>,
    instance: SessionInstance,
    recorded_context: Option<crate::harness::TurnContext>,
    recorded_limit: super::usage_limits::UsageObservation,
}

impl StatusPump {
    /// Write one snapshot down, and drain if it freed a turn that was waiting.
    /// Whether to go on watching: not once the session has ended, or the tab
    /// has passed to another session.
    fn observe(&mut self, snapshot: &crate::harness::SessionStatusSnapshot) -> bool {
        let Some(state) = self.state.upgrade() else {
            return false;
        };
        let (ended, retry_deferred, clock) = {
            let mut app = state.lock().unwrap();
            let Some(session) = self.session.upgrade() else {
                return false;
            };
            let instance = &self.instance;
            if !still_pumping_instance(&app, &self.key, &session, instance) {
                return false;
            }
            let owner = &instance.entity_id;
            app.record_agent_status_snapshot(owner, &instance.agent_id, snapshot);
            record_new_turn_context(&mut app, instance, snapshot, &mut self.recorded_context);
            // A limit recorded wants its reset waited for, and one cleared
            // may have left agents to start again: either is a drain.
            let usage_limit_moved = app.record_usage_limit(
                owner,
                &instance.agent_id,
                snapshot,
                &mut self.recorded_limit,
            );
            let ended = matches!(snapshot.status, AgentStatus::Ended { .. });
            if let Some(reason) = ended.then(|| session.start_refused()).flatten() {
                app.record_agent_start_refused(owner, &instance.agent_id, &reason);
            }
            let retry_deferred = usage_limit_moved
                || !matches!(snapshot.status, AgentStatus::Working)
                    && something_waits_for_the_turn(&mut app, owner, &instance.agent_id);
            (ended, retry_deferred, Arc::clone(&app.frame_clock))
        };
        if retry_deferred {
            let timer = clock.frame(AGENT_DELIVERY_METHOD);
            DeliveryRunner::drain(&state, &timer);
        }
        !ended
    }
}

/// Whether an agent whose turn just ended has something waiting for it: a
/// queued turn, or a compaction it asked for mid-turn, which is ready now.
fn something_waits_for_the_turn(app: &mut AppState, owner: &str, agent_id: &str) -> bool {
    let compaction_ready = app.compactions.turn_ended(owner, agent_id);
    compaction_ready
        || app
            .delivery_queue
            .queued()
            .any(|turn| turn.owner == owner && turn.agent_id == agent_id)
}

/// Record the turn context `snapshot` carries unless this pump already has.
///
/// Keyed on what the pump recorded rather than on the record, because the
/// record is cleared when a compaction is asked for (see
/// [`compact_before_turn`](super::delivery::compaction::compact_before_turn)):
/// the snapshots that follow still carry the reading from before it until the
/// harness reports a new one, and writing that back would ask again.
fn record_new_turn_context(
    app: &mut AppState,
    instance: &SessionInstance,
    snapshot: &crate::harness::SessionStatusSnapshot,
    recorded: &mut Option<crate::harness::TurnContext>,
) {
    let Some(context) = snapshot
        .context
        .filter(|context| *recorded != Some(*context))
    else {
        return;
    };
    app.record_agent_turn_context(&instance.entity_id, &instance.agent_id, context);
    *recorded = Some(context);
}

/// What a byte pump paints and where: the tab's screen, the stream it reads,
/// and the runtime it runs on.
pub(in crate::app) struct BytePump {
    pub(in crate::app) screen: Option<ScreenHandle>,
    pub(in crate::app) rx: Option<broadcast::Receiver<Vec<u8>>>,
    pub(in crate::app) push_runtime: Option<tokio::runtime::Handle>,
}

/// Pump one tab's PTY into its screen model, coalescing at `TERM_FLUSH_MS` and
/// flushing one keyed frame to every attached client.
///
/// **It never takes the app mutex to paint.** The screen is its own lock, held
/// by this task for the microseconds a chunk takes to parse, so three agents
/// flooding at once contend with each other's readers and with nothing else in
/// the daemon. The app mutex is taken exactly twice, at EOF, to write down that
/// the session ended.
///
/// Start of session: the parser is reset to a blank screen of the current grid
/// and `term.reset` is pushed (clients wipe; a replacement process starts
/// clean) — the cursor is NEVER reset, because client dedupe rides it. On EOF a
/// Shell tab is removed, reaped, and pushed `term.closed{exited}`; an Agent tab
/// is RETAINED with `live = false` and pushed `term.closed{agent_session_ended}`,
/// because the tab must still show the last screen.
///
/// One pump per tab for the tab's whole life, and it pumps the session it was
/// started for: a kill is asynchronous now, so a replaced session's EOF can
/// arrive after its replacement is already in the registry. The tab is only
/// ended by the pump that holds that tab's own session.
///
/// A closed screen ends the pump without the registry: a retired tab's child
/// may keep producing until its kill lands, and nobody is watching.
pub(in crate::app) fn spawn_tab_pump(
    state: &Arc<Mutex<AppState>>,
    key: TabKey,
    session: Arc<dyn AgentSession>,
    instance: Option<SessionInstance>,
    pump: BytePump,
) {
    // No terminal, no bytes: the pump exists to paint a stream into a grid, and
    // a session that offers none has nothing for it to do.
    let BytePump {
        screen: Some(screen),
        rx: Some(mut rx),
        push_runtime,
    } = pump
    else {
        return;
    };
    // The daemon's push runtime, where the paint competes with nothing that
    // answers a request; with none set, the runtime starting the pump.
    let Some(runtime) = push_runtime.or_else(|| tokio::runtime::Handle::try_current().ok()) else {
        // Sync unit tests drive the registry without a runtime; there is
        // nothing to spawn the pump onto and nothing attached to feed.
        return;
    };
    let state = Arc::clone(state);
    runtime.spawn(async move {
        screen.restart();
        let mut flush = tokio::time::interval(Duration::from_millis(TERM_FLUSH_MS));
        flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let still_open = tokio::select! {
                recv = rx.recv() => match recv {
                    Ok(chunk) => screen.feed(&chunk),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        off_the_workers(move || {
                            end_of_session(&state, &key, &session, instance.as_ref(), &screen)
                        })
                        .await;
                        return;
                    }
                },
                _ = flush.tick() => screen.flush(),
            };
            if !still_open {
                return;
            }
        }
    });
}

/// Whether the tab at `key` is still the one `session` was pumped for.
///
/// A retirement kills and reaps on a thread of its own, so a replaced session's
/// stream can end after its replacement is already in the registry — and a
/// pump's death rites take the app mutex more than once, with a filesystem read
/// between, so the tab can turn over mid-rite. Every acquisition asks, not just
/// the first: writing a dead session's findings onto a live one closes the
/// replacement's turn and records the wrong conversation against it.
pub(in crate::app) fn still_pumping(
    s: &AppState,
    key: &TabKey,
    session: &Arc<dyn AgentSession>,
) -> bool {
    s.session_registry.shell_pump_matches(key, session)
}

/// Stronger guard for an agent callback: the tab still holds both the process
/// and the exact lineage instance captured when its pumps were spawned.
pub(in crate::app) fn still_pumping_instance(
    s: &AppState,
    key: &TabKey,
    session: &Arc<dyn AgentSession>,
    instance: &SessionInstance,
) -> bool {
    s.session_registry
        .agent_pump_matches(key, session, instance)
}

/// The death rites of the session a byte pump was watching.
///
/// The app mutex is taken twice, with the reading a dying session owes between
/// them: what the tab becomes and what its clients are told are one bounded
/// step and are taken together, while the reading is a filesystem walk that
/// belongs under no lock at all. Both acquisitions are guarded by
/// [`still_pumping`], because that walk is the gap between them.
pub(in crate::app) fn end_of_session(
    state: &Arc<Mutex<AppState>>,
    key: &TabKey,
    session: &Arc<dyn AgentSession>,
    instance: Option<&SessionInstance>,
    screen: &ScreenHandle,
) {
    let ended_agent = {
        let mut s = state.lock().unwrap();
        if !still_pumping(&s, key, session) {
            return;
        }
        let role = s
            .session_registry
            .agent_snapshot(key)
            .map(|tab| tab.role)
            .unwrap_or(TabRole::Shell);
        let provider_thread_id = role
            .agent()
            .and_then(|(owner_id, agent_id)| s.recorded_resume_id(owner_id, agent_id));
        match role.agent() {
            Some(_) => {
                let Some(instance) = instance else {
                    return;
                };
                if !still_pumping_instance(&s, key, session, instance) {
                    return;
                }
                let ended = instance.clone();
                if !s.session_registry.mark_agent_ended_if_current(
                    key,
                    session,
                    instance,
                    LifecycleDiagnostic {
                        event: "session_ended_observed",
                        origin: "session_output_closed",
                        reason: Some("agent_session_ended"),
                        operation_id: None,
                        provider_thread_id: provider_thread_id.as_deref(),
                        caller: None,
                    },
                ) {
                    return;
                }
                // Told in the same acquisition that marks the tab, because a
                // marked tab is a REPLACEABLE one: the next spawn takes this
                // screen, clients and all, onto its own session without a
                // word. A close pushed after the release would land on
                // browsers already watching the replacement, in among its
                // opening reset. The screen's lock and one send per client is
                // bounded work, which is what makes it allowed here.
                screen.flush();
                // The clients hear the session ended and STAY: the grid they
                // are watching is the last thing this agent painted, and the
                // session that replaces it paints onto the same screen, with
                // the same clients still on it.
                screen.session_ended("agent_session_ended");
                Some(ended)
            }
            None => {
                s.retire_tab(key, "exited");
                None
            }
        }
    };
    let Some(instance) = ended_agent else {
        return;
    };
    // One final reading, so a session shorter than a sweep tick is still named
    // — and the respawn that needs the name is the very next thing after a
    // close. It RECORDS; it never clears: a terminal resumed in place writes no
    // new transcript, so a locator finding nothing is its normal answer here,
    // and clearing on that would throw a good name away at every restart. A
    // name that no longer resolves is caught at the reservation instead. The
    // reading itself is a filesystem walk, so it happens here, between the two
    // acquisitions, and not inside either.
    let report = SelfReport::read(session);
    let mut s = state.lock().unwrap();
    // A replacement can have taken the tab over while that walk ran. Its turn
    // is in flight and its conversation is its own; this session's findings
    // would close the one and overwrite the other.
    if !still_pumping_instance(&s, key, session, &instance) {
        return;
    }
    s.note_self_report(&instance.entity_id, &instance.agent_id, &instance, report);
    // The process is what a session IS, so this is where the conversation's
    // lineage closes — and where a turn the dead process was holding is closed,
    // so the row stops reading as working.
    s.record_agent_session_end(&instance.entity_id, &instance.agent_id, &instance);
}

/// Pump one session's reported activity and surface revisions.
///
/// The mirror of [`spawn_tab_pump`] for a session protocol that has no bytes. Where the
/// byte pump paints a stream into a grid, this one posts what the agent
/// reported doing as the activity kinds — reasoning, tool calls, narration and
/// background work — which are conversation, classed `Status`: they move no
/// unread count, reach no Issue conversation and pull nobody in.
/// Surface revisions independently invalidate the owning entity, including an
/// initial invalidation after subscription, so a carrier does not need an
/// activity stream merely to publish its cached surface snapshot.
///
/// It owes the same death rites, minus the screen's half: on close the tab goes
/// not live and the conversation's session lineage ends. There is no
/// `term.closed` to push because there is no screen — the step-3 refusals
/// already keep every client off one — and the tab is RETAINED for the same
/// reason the byte pump retains an agent's, so the rail still shows the agent
/// that was here.
///
/// It carries the session it pumps for the same reason the byte pump does, and
/// asks [`still_pumping`] at every acquisition: what it writes belongs to that
/// session, and a tab holding a different one is somebody else's. A surface
/// watch closing only retires that watch; only activity EOF performs the death
/// rites because the revision channel is not a session-lifetime signal.
pub(in crate::app) fn spawn_activity_pump(
    state: &Arc<Mutex<AppState>>,
    key: TabKey,
    session: Arc<dyn AgentSession>,
    instance: Option<SessionInstance>,
    mut rx: Option<broadcast::Receiver<crate::harness::ActivityReport>>,
    mut surfaces_changed: Option<tokio::sync::watch::Receiver<u64>>,
) {
    if rx.is_none() && surfaces_changed.is_none() {
        return;
    }
    // Everything the pump writes belongs to the instance it was started for,
    // so without one there is nothing it could write.
    let Some(instance) = instance else {
        return;
    };
    if tokio::runtime::Handle::try_current().is_err() {
        // Sync unit tests drive the registry without a runtime; there is
        // nothing to spawn the pump onto.
        return;
    }
    let pump = Arc::new(ActivityPump {
        state: Arc::downgrade(state),
        key,
        session: Arc::downgrade(&session),
        instance,
    });
    tokio::spawn(async move {
        if surfaces_changed.is_some() && !pump.step(ActivityPump::surfaces_moved).await {
            return;
        }
        loop {
            let Some(woke) = next_pump_wake(&mut rx, &mut surfaces_changed).await else {
                return;
            };
            let still_pumping = match woke {
                PumpWake::SurfacesMoved => pump.step(ActivityPump::surfaces_moved).await,
                PumpWake::SurfacesUnwatchable => {
                    surfaces_changed = None;
                    true
                }
                PumpWake::Reported(Ok(report)) => {
                    pump.step(move |pump| pump.reported(&report)).await
                }
                // A turn that called forty tools while the lock was busy is a
                // reader problem, not a reason to stop reading: what is lost is
                // lost, and the events after it still belong in the timeline.
                PumpWake::Reported(Err(broadcast::error::RecvError::Lagged(_))) => true,
                PumpWake::Reported(Err(broadcast::error::RecvError::Closed)) => {
                    pump.step(ActivityPump::closed).await;
                    false
                }
            };
            if !still_pumping {
                return;
            }
        }
    });
}

/// What the activity pump writes on behalf of: one session, as one instance,
/// in one tab. Shared with the blocking pool for each step, because every step
/// takes the app mutex — and most read the session's self-report, a
/// filesystem walk, first.
struct ActivityPump {
    state: std::sync::Weak<Mutex<AppState>>,
    key: TabKey,
    session: std::sync::Weak<dyn AgentSession>,
    instance: SessionInstance,
}

/// The state and the session an [`ActivityPump`] writes for, while both last.
type LivePump = (Arc<Mutex<AppState>>, Arc<dyn AgentSession>);

impl ActivityPump {
    /// Run one step on the blocking pool.
    async fn step<T: Send + 'static>(
        self: &Arc<Self>,
        step: impl FnOnce(&ActivityPump) -> T + Send + 'static,
    ) -> T {
        let pump = Arc::clone(self);
        off_the_workers(move || step(&pump)).await
    }

    fn live(&self) -> Option<LivePump> {
        Some((self.state.upgrade()?, self.session.upgrade()?))
    }

    /// Tell the owning entity's watchers its surfaces moved. Whether the tab is
    /// still this session's.
    fn surfaces_moved(&self) -> bool {
        let Some((state, session)) = self.live() else {
            return false;
        };
        let state = state.lock().unwrap();
        if !still_pumping_instance(&state, &self.key, &session, &self.instance) {
            return false;
        }
        state.note_entity_changed(&self.instance.entity_id);
        true
    }

    /// Post one reported activity. Whether the tab is still this session's.
    fn reported(&self, report: &crate::harness::ActivityReport) -> bool {
        let instance = &self.instance;
        let Some((state, session)) = self.live() else {
            return false;
        };
        let said = SelfReport::read(&session);
        let mut s = state.lock().unwrap();
        if !still_pumping_instance(&s, &self.key, &session, instance) {
            return false;
        }
        s.note_self_report(&instance.entity_id, &instance.agent_id, instance, said);
        record_activity(
            &mut s,
            &self.key,
            &instance.entity_id,
            &instance.agent_id,
            report,
        );
        true
    }

    /// The death rites, when the activity stream closes.
    fn closed(&self) {
        let instance = &self.instance;
        let Some((state, session)) = self.live() else {
            return;
        };
        let said = SelfReport::read(&session);
        // Protocol sessions publish their final status before closing the
        // activity stream. Consume that exact boundary here as well as in the
        // watch pump: the two tasks race, and EOF must not call a completed
        // turn an interruption merely because it acquired the app lock first.
        let final_status = session
            .status_changed()
            .map(|status| status.borrow().clone());
        let mut s = state.lock().unwrap();
        // The reading is a filesystem walk, and a replacement can have taken
        // the tab over while it ran: what follows ends a session, and ending the
        // live one would mark it dead, harvest its open tool calls and close its
        // turn.
        if !still_pumping_instance(&s, &self.key, &session, instance) {
            return;
        }
        let Some(unanswered_call_sequences) = s
            .session_registry
            .end_agent_stream_if_current(&self.key, &session, instance)
        else {
            return;
        };
        match said.named {
            Some(_) => s.note_self_report(&instance.entity_id, &instance.agent_id, instance, said),
            // A session that ended having never announced a conversation of its
            // own is the shape of one spawned with an id that no longer
            // resolves: the child exits without an init line. Clearing sends
            // the next spawn back to the transcript probe, so one dead id costs
            // one restart rather than every restart — and where the child died
            // at startup for an unrelated reason, the probe is what would have
            // answered anyway.
            None => s.record_agent_resume_id(&instance.entity_id, &instance.agent_id, None),
        }
        // A call still open when the child's stream ended never got an answer
        // and never will: it is closed here, saying so, BEFORE the session ends
        // — so the timeline reads calls-closed-then-session-ended rather than a
        // session ending over work that still claims to run.
        for sequence in unanswered_call_sequences {
            s.resolve_agent_tool_call(
                &instance.entity_id,
                &instance.agent_id,
                sequence,
                crate::thread::ToolCallOutcome::Unanswered,
                NO_ANSWER_SESSION_ENDED,
            );
        }
        if let Some(snapshot) = final_status.as_ref() {
            s.record_agent_status_snapshot(&instance.entity_id, &instance.agent_id, snapshot);
        }
        // The process is what a session IS, so this is where the conversation's
        // lineage closes — and where a turn the dead process was holding is
        // closed, so the row stops reading as working.
        s.record_agent_session_end(&instance.entity_id, &instance.agent_id, instance);
    }
}

async fn next_pump_wake(
    activity: &mut Option<broadcast::Receiver<crate::harness::ActivityReport>>,
    surfaces: &mut Option<tokio::sync::watch::Receiver<u64>>,
) -> Option<PumpWake> {
    match (activity.as_mut(), surfaces.as_mut()) {
        (Some(activity), Some(surfaces)) => Some(tokio::select! {
            reported = activity.recv() => PumpWake::Reported(reported),
            noticed = surfaces.changed() => match noticed {
                Ok(()) => PumpWake::SurfacesMoved,
                Err(_) => PumpWake::SurfacesUnwatchable,
            },
        }),
        (Some(activity), None) => Some(PumpWake::Reported(activity.recv().await)),
        (None, Some(surfaces)) => Some(match surfaces.changed().await {
            Ok(()) => PumpWake::SurfacesMoved,
            Err(_) => PumpWake::SurfacesUnwatchable,
        }),
        (None, None) => None,
    }
}

/// The terminal's capture point: ask every live agent session for the
/// name its conversation has, and write down each answer that moved.
///
/// A terminal announces nothing, so no task wakes on its behalf the way the
/// activity pump wakes on a session protocol's events — which is why the sweep
/// is daemon-owned and fixed-cadence rather than hung off the status poll. The
/// poll is client-driven: with no browser open nothing would ever be captured,
/// and every attached client would multiply this filesystem read by its own
/// poll rate, on the RPC path that answers from under the state lock.
///
/// The lock is HELD only to collect the live agents and to write the answers.
/// The one call that may touch the filesystem — a locator listing the harness's
/// transcript tree — is made between the two, with the lock released, the way a
/// turn is handed over.
pub(in crate::app) fn capture_conversation_names(state: &Arc<Mutex<AppState>>) {
    /// One live agent, taken out of the registry so the name can be asked for
    /// with the lock released, and put back by `key` once it is known.
    struct LiveAgent {
        key: TabKey,
        instance: SessionInstance,
        session: Arc<dyn AgentSession>,
        recorded: Option<String>,
        recorded_model: Option<String>,
        recorded_effort: Option<String>,
    }

    let live: Vec<LiveAgent> = {
        let s = state.lock().unwrap();
        s.session_registry
            .live_agent_snapshots()
            .into_iter()
            .filter_map(|tab| {
                let instance = tab.instance?;
                Some(LiveAgent {
                    key: tab.key,
                    instance: instance.clone(),
                    session: tab.session,
                    recorded: s.recorded_resume_id(&instance.entity_id, &instance.agent_id),
                    recorded_model: s
                        .recorded_active_model(&instance.entity_id, &instance.agent_id),
                    recorded_effort: s
                        .recorded_active_effort(&instance.entity_id, &instance.agent_id),
                })
            })
            .collect()
    };
    let moved: Vec<(TabKey, SessionInstance, Arc<dyn AgentSession>, SelfReport)> = live
        .into_iter()
        .filter_map(|agent| {
            let said = SelfReport::read(&agent.session);
            let name_moved = said.named.is_some() && agent.recorded != said.named;
            let model_moved = said.model.is_some() && agent.recorded_model != said.model;
            let effort_moved = said.model.is_some() && agent.recorded_effort != said.effort;
            (name_moved || model_moved || effort_moved).then_some((
                agent.key,
                agent.instance,
                agent.session,
                said,
            ))
        })
        .collect();
    if moved.is_empty() {
        return;
    }
    let mut s = state.lock().unwrap();
    for (key, instance, session, said) in moved {
        if still_pumping_instance(&s, &key, &session, &instance) {
            s.note_self_report(&instance.entity_id, &instance.agent_id, &instance, said);
        }
    }
}
