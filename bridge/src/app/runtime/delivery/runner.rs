use super::compaction::{send_compaction, CompactionSend};
use crate::app::{
    deliver, AppState, DeliveryOutcome, PendingTurns, AGENT_START_DECLINED_SESSION_OVER,
};
use crate::operation::OperationStatus;
use crate::timing::FrameTimer;
use crate::update::AdmissionLease;
use std::sync::{Arc, Mutex};

/// Sending the queued turns, off the frame that queued them.
///
/// DELIBERATE, and the whole of spec step 2: a cold delivery spawns a harness
/// and waits on its readiness for up to [`HARNESS_READY_GRACE`], and the
/// browser gives up at twelve seconds. The verb's own state change is durable
/// before the queue is even taken, so nothing about the reply depends on the
/// agent being up. What the delivery does reaches the browser the way every
/// other background outcome does: the entity's own record
/// ([`AppState::record_agent_session_start`],
/// [`AppState::record_agent_delivery_failure`]) and a push invalidation.
///
/// [`HARNESS_READY_GRACE`]: crate::orchestrator::HARNESS_READY_GRACE
pub(in crate::app) struct DeliveryRunner;

impl DeliveryRunner {
    /// Take whatever the verbs that just ran queued, under one acquisition
    /// charged to `timer`, and deliver it off this thread. The one call every
    /// path that queues a turn makes once its own state change is durable.
    ///
    /// A turn left settling (an issue notice waiting out its window) has
    /// nothing else to drain it when the window ends, so the same acquisition
    /// asks for a wake then.
    pub(in crate::app) fn drain(state: &Arc<Mutex<AppState>>, timer: &FrameTimer) {
        let admission = state.lock().unwrap().update_admission();
        let lease = match admission {
            Some(gate) => match gate.try_enter() {
                Some(lease) => Some(lease),
                None => return, // The turns remain in the queue until admission reopens.
            },
            None => None,
        };
        let (compactions, turns, settle_wake, usage_limit_wake) = {
            let mut app = timer.lock(state);
            // Before the turns are taken: an agent with a turn queued has
            // its compaction sent ahead of that turn, by the turn's delivery.
            let compactions = app.take_ready_compactions();
            (
                compactions,
                app.take_pending_turns(),
                app.delivery_queue.settle_wake_due(),
                app.usage_limit_wake_due(),
            )
        };
        if let Some(at) = settle_wake {
            DeliveryRunner::wake_at(state, timer, at, SETTLE_WAKE_METHOD, |app, at| {
                app.delivery_queue.settle_wake_fired(at)
            });
        }
        // Wake once at a reported reset to retry agents stopped by that limit.
        if let Some(at) = usage_limit_wake {
            let wait = at - time::OffsetDateTime::now_utc();
            let wait = std::time::Duration::try_from(wait).unwrap_or_default();
            DeliveryRunner::wake_at(
                state,
                timer,
                std::time::Instant::now() + wait,
                USAGE_LIMIT_WAKE_METHOD,
                |_, _| {},
            );
        }
        DeliveryRunner::spawn_compactions(state, timer, compactions, lease.clone());
        DeliveryRunner::spawn(state, turns, lease);
    }

    /// Send the compactions agents asked for that a drain took, off this
    /// thread the way a turn is sent, and return at once. With no runtime
    /// under it they are sent here.
    fn spawn_compactions(
        state: &Arc<Mutex<AppState>>,
        timer: &FrameTimer,
        compactions: Vec<CompactionSend>,
        lease: Option<AdmissionLease>,
    ) {
        if compactions.is_empty() {
            return;
        }
        let send_all = {
            let state = Arc::clone(state);
            let clock = Arc::clone(timer.clock());
            move || {
                let _lease = lease;
                let timer = clock.frame(AGENT_DELIVERY_METHOD);
                for compaction in &compactions {
                    send_compaction(&state, &timer, compaction);
                }
            }
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            send_all();
            return;
        };
        let sending = runtime.spawn_blocking(send_all);
        runtime.spawn(async move {
            if let Err(joined) = sending.await {
                eprintln!("requested compaction failed: {joined}");
            }
        });
    }

    /// Drain again at `at`, on a timer of the runtime's. Holds the state
    /// weakly: a daemon shutting down is not kept alive for a notice.
    ///
    /// With no runtime under it there is no timer to set, and the synchronous
    /// tests end the window by hand instead.
    fn wake_at(
        state: &Arc<Mutex<AppState>>,
        timer: &FrameTimer,
        at: std::time::Instant,
        method: &'static str,
        fired: fn(&mut AppState, std::time::Instant),
    ) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let state = Arc::downgrade(state);
        let clock = Arc::clone(timer.clock());
        runtime.spawn(async move {
            tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await;
            // The app mutex is a blocking lock, and a runtime worker must not
            // wait on it behind a slow frame.
            let drained = tokio::task::spawn_blocking(move || {
                let Some(state) = state.upgrade() else {
                    return;
                };
                let timer = clock.frame(method);
                fired(&mut timer.lock(&state), at);
                DeliveryRunner::drain(&state, &timer);
            });
            if let Err(joined) = drained.await {
                eprintln!("{method} failed: {joined}");
            }
        });
    }

    /// Deliver `turns` on a thread of the runtime's, and return at once.
    ///
    /// With no runtime under it — the synchronous unit tests — there is no
    /// thread to hand the work to and it runs here, which is the same
    /// delivery, made on the caller's time.
    ///
    /// The delivery is JOINED by a task of its own rather than detached, so a
    /// delivery that panicked, or one a shutting-down runtime never ran, says
    /// so on the log instead of vanishing. Either way the batch's in-flight
    /// marks come back with it: [`PendingTurns`] settles what it owes on drop.
    ///
    /// The blocking half is submitted BEFORE the joiner, and not from inside
    /// it: a single-threaded runtime runs a spawned task only when something
    /// awaits, and a delivery that waited for its caller to await would be a
    /// delivery that never left the frame.
    pub(in crate::app) fn spawn(
        state: &Arc<Mutex<AppState>>,
        turns: PendingTurns,
        lease: Option<AdmissionLease>,
    ) {
        if turns.is_empty() {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            let _lease = lease;
            DeliveryRunner::run(state, turns);
            return;
        };
        let state = Arc::clone(state);
        let delivering = runtime.spawn_blocking(move || {
            let _lease = lease;
            DeliveryRunner::run(&state, turns)
        });
        runtime.spawn(async move {
            if let Err(joined) = delivering.await {
                eprintln!("agent delivery failed: {joined}");
            }
        });
    }

    /// Send every turn, one at a time, and write down what each one did.
    ///
    /// A cold delivery starts a new harness process, so it opens the
    /// conversation's session lineage — the record the thread reads back as
    /// "the revise agent started here". A warm delivery continues the session
    /// already open.
    pub(in crate::app) fn run(state: &Arc<Mutex<AppState>>, mut turns: PendingTurns) {
        let timer = turns.clock.frame(AGENT_DELIVERY_METHOD);
        let mut completed_delivery_freed_capacity = false;
        while let Some((turn, mark)) = turns.next_turn() {
            if let Some(operation_id) = turn.operation_id.as_deref() {
                let claimed = timer.lock(state).transition_delivery_operation(
                    operation_id,
                    OperationStatus::Queued,
                    OperationStatus::Claimed,
                    None,
                );
                match claimed {
                    Ok(true) => {}
                    Ok(false) => {
                        mark.settle(&mut timer.lock(state));
                        completed_delivery_freed_capacity = true;
                        continue;
                    }
                    Err(error) => {
                        eprintln!("claim delivery {operation_id}: {error}");
                        let mut app = timer.lock(state);
                        app.delivery_queue.requeue(turn);
                        mark.settle(&mut app);
                        continue;
                    }
                }
            }
            let delivered = deliver(state, &turn, &timer);
            let completed = !matches!(&delivered, Ok(DeliveryOutcome::Deferred));
            let mut s = timer.lock(state);
            if let Some(operation_id) = turn.operation_id.as_deref() {
                let next = match &delivered {
                    Ok(DeliveryOutcome::Deferred) => OperationStatus::Queued,
                    Ok(DeliveryOutcome::Delivered(_)) => OperationStatus::Delivered,
                    Err(_) => OperationStatus::Uncertain,
                };
                let execution_error = match &delivered {
                    Ok(DeliveryOutcome::Delivered(None)) => Some(AGENT_START_DECLINED_SESSION_OVER),
                    Err(error) => Some(error.as_str()),
                    _ => None,
                };
                if let Err(error) = s.transition_delivery_operation(
                    operation_id,
                    OperationStatus::Claimed,
                    next,
                    execution_error,
                ) {
                    eprintln!("settle delivery {operation_id}: {error}");
                }
            }
            match delivered {
                // An issue whose session is over (approved, abandoned) holds no
                // workspace — and its checkout is the project's primary one,
                // which is emphatically not a place to spawn a replacement for
                // work nobody is doing. The turn stays on its thread; the
                // agent says why nothing opened.
                Ok(DeliveryOutcome::Delivered(None)) => s.record_agent_start_declined(&turn),
                Ok(DeliveryOutcome::Delivered(Some(_))) => {}
                Ok(DeliveryOutcome::Deferred) => {
                    s.delivery_queue.requeue(turn);
                }
                // The message stays durable with its delivery failure, so the
                // entity carries the reason without silently replaying an
                // input whose handoff may have succeeded. The idle sweep finishes the
                // job: an entity left working with no agent tab is demoted on
                // the next pass.
                Err(error) => {
                    eprintln!("deliver to {}: {error}", turn.owner);
                    s.record_agent_delivery_failure(&turn, &error);
                }
            }
            // Off the queue and out of flight: from here the entity's agent tab
            // is the whole truth about whether an agent is there.
            mark.settle(&mut s);
            completed_delivery_freed_capacity |= completed;
        }
        if completed_delivery_freed_capacity {
            DeliveryRunner::drain(state, &timer);
        }
    }
}

/// Where a delivery's own time is charged. A spawn is not the frame that asked
/// for it, and counting it there would make every verb that speaks to an agent
/// look like the daemon's slowest.
pub(in crate::app) const AGENT_DELIVERY_METHOD: &str = "agent.deliver";

/// Where a settle window's wake is charged: a timer, not any client's frame.
pub(in crate::app) const SETTLE_WAKE_METHOD: &str = "agent.settle_wake";
/// The frame a usage limit's reset wakes the delivery queue under (issue #58).
pub(in crate::app) const USAGE_LIMIT_WAKE_METHOD: &str = "agent.usage_limit_wake";
