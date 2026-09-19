use crate::app::{
    deliver, AppState, DeliveryOutcome, PendingTurns, AGENT_START_DECLINED_SESSION_OVER,
};
use crate::operation::OperationStatus;
use crate::timing::FrameTimer;
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
    pub(in crate::app) fn drain(state: &Arc<Mutex<AppState>>, timer: &FrameTimer) {
        let turns = timer.lock(state).take_pending_turns();
        DeliveryRunner::spawn(state, turns);
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
    pub(in crate::app) fn spawn(state: &Arc<Mutex<AppState>>, turns: PendingTurns) {
        if turns.is_empty() {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            DeliveryRunner::run(state, turns);
            return;
        };
        let state = Arc::clone(state);
        let delivering = runtime.spawn_blocking(move || DeliveryRunner::run(&state, turns));
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
