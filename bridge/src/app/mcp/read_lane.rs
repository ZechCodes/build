//! Exact MCP reads bypass mutation turns while retaining session admission.
mod admission;
mod guarded;
mod history;
mod job;
mod mixed;
pub(super) mod service;
mod snapshots;
mod sql;

use crate::app::off_the_workers;
#[cfg(test)]
use crate::app::AppState;
use crate::mcp::{BridgeAction, DoneReport};
use crate::timing::{FrameTimer, WaitTimer};
use admission::ReadSession;
use job::{PreparedRead, ReadWork};
use serde_json::Value;
use service::{Inputs, McpReadService};
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

pub(super) fn action(frame: &Value) -> Option<BridgeAction> {
    // Avoid cloning/parsing potentially large writer payloads for scheduling.
    let name = frame.get("request")?.get("action")?.as_str()?;
    if !matches!(
        name,
        "tracker_get_task"
            | "tracker_list_tasks"
            | "tracker_task_dependents"
            | "list_harnesses"
            | "list_workspace_agents"
            | "list_project_agents"
            | "list_projects"
            | "list_work"
            | "list_workspaces"
            | "search_conversation"
            | "read_conversation"
            | "tracker_read_comment"
            | "tracker_get_review"
            | "tracker_read_review"
    ) {
        return None;
    }
    // A valid report is a notification even when a request is also present.
    if serde_json::from_value::<DoneReport>(frame.get("report").cloned().unwrap_or(Value::Null))
        .is_ok()
    {
        return None;
    }
    let action = super::parse_action_request(frame).ok()?;
    (sql::supports(&action)
        || guarded::supports(&action)
        || snapshots::supports(&action)
        || history::supports(&action)
        || mixed::supports(&action))
    .then_some(action)
}

fn prepare(
    service: &McpReadService,
    inputs: &Arc<Inputs>,
    session: &ReadSession,
    action: &BridgeAction,
) -> Result<PreparedRead, String> {
    sql::prepare(service, inputs, session, action)
        .or_else(|| guarded::prepare(service, inputs, session, action))
        .or_else(|| snapshots::prepare(service, inputs, session, action))
        .or_else(|| history::prepare(service, inputs, session, action))
        .or_else(|| mixed::prepare(service, inputs, session, action))
        .unwrap_or_else(|| Err("invalid MCP action request".into()))
}

struct Admitted {
    session: ReadSession,
    read: PreparedRead,
    lease: Option<crate::update::AdmissionLease>,
}

fn admit(
    service: &McpReadService,
    frame: &Value,
    action: &BridgeAction,
) -> Result<Admitted, String> {
    let lease = service
        .admission
        .as_ref()
        .map(|gate| {
            gate.try_enter()
                .ok_or_else(|| "bridge update handoff in progress".to_string())
        })
        .transpose()?;
    let inputs = Arc::new(service.inputs());
    let session = ReadSession::authenticate(&inputs, frame, action)?;
    let read = prepare(service, &inputs, &session, action)?;
    Ok(Admitted {
        session,
        read,
        lease,
    })
}

pub(super) async fn answer(
    service: Arc<McpReadService>,
    frame: Value,
    action: BridgeAction,
    timer: FrameTimer,
) -> Value {
    #[cfg(test)]
    timer.sample_tool(action.tool_name());
    let timer = Arc::new(timer);
    let admission = {
        let service = Arc::clone(&service);
        let frame = frame.clone();
        let action = action.clone();
        blocking(Arc::clone(&timer), move || admit(&service, &frame, &action)).await
    };
    let Admitted {
        session,
        read,
        lease,
    } = match admission {
        Ok(admitted) => admitted,
        Err(error) => return super::mcp_action_response(Err(error)),
    };
    let PreparedRead { pool, query, scope } = read;
    let mut connection = match pool {
        Some(pool) => Some(pool.acquire().await),
        None => None,
    };
    let (outcome, lease) = blocking(Arc::clone(&timer), move || {
        let prepared = query(connection.as_mut());
        // Mixed/external reads must never retain SQL capacity during I/O.
        drop(connection);
        let outcome = match prepared {
            Ok(work) => work.run(),
            Err(error) => job::Outcome::error(error),
        };
        // A cancelled caller cannot release admission while this blocking
        // closure still runs. Returning the lease covers the next boundary.
        (outcome, lease)
    })
    .await;
    blocking(Arc::clone(&timer), move || {
        let _lease = lease;
        finish(&service, &frame, &action, session, scope, outcome)
    })
    .await
}

fn finish(
    service: &McpReadService,
    frame: &Value,
    action: &BridgeAction,
    session: ReadSession,
    scope: Option<service::ProjectScope>,
    outcome: job::Outcome,
) -> Value {
    let mut outcome = Some(outcome);
    let inputs = service.inputs();
    let result = session
        .validate(&inputs, frame, action)
        .and_then(|()| {
            scope
                .as_ref()
                .map_or(Ok(()), |scope| scope.validate(&inputs))
        })
        .and_then(|()| outcome.take().unwrap().finish(&inputs));
    // Rejected payloads and serialization never run under an application lock,
    // including when current capability or target fences reject the read.
    drop(outcome);
    super::mcp_action_response(result)
}

/// Drive the actual no-SQL adapter boundaries on one measured thread. The
/// optional producer between them is deliberately outside the read's counter.
#[cfg(test)]
pub(super) fn probe_guarded_read(
    state: &Arc<Mutex<AppState>>,
    frame: &Value,
    before_final: impl FnOnce(),
) -> (Value, usize) {
    probe_guarded_stages(state, frame, before_final, ProbeKind::CanonicalPaths)
}

#[cfg(test)]
pub(super) fn probe_guarded_executables(
    state: &Arc<Mutex<AppState>>,
    frame: &Value,
) -> (Value, usize) {
    probe_guarded_stages(state, frame, || {}, ProbeKind::GuardedExecutables)
}

#[cfg(test)]
enum ProbeKind {
    CanonicalPaths,
    GuardedExecutables,
}

#[cfg(test)]
impl ProbeKind {
    fn measure<T>(&self, guarded: bool, work: impl FnOnce() -> T) -> (T, usize) {
        match self {
            Self::CanonicalPaths => crate::app::workspaces::count_path_canonicalizations(work),
            Self::GuardedExecutables if guarded => {
                let (value, probes) =
                    crate::harness::installed::executable::probe_counts::measure(work);
                (value, probes.total())
            }
            Self::GuardedExecutables => (work(), 0),
        }
    }
}

#[cfg(test)]
fn probe_guarded_stages(
    state: &Arc<Mutex<AppState>>,
    frame: &Value,
    before_final: impl FnOnce(),
    probe: ProbeKind,
) -> (Value, usize) {
    let action = action(frame).expect("the fixture supplies an exact supported read");
    assert!(
        guarded::supports(&action),
        "only no-SQL catalog/roster reads"
    );
    let service = McpReadService::capture(&state.lock().unwrap());
    let (admitted, admission_probes) = probe.measure(true, || admit(&service, frame, &action));
    let Admitted {
        session,
        read,
        lease,
    } = match admitted {
        Ok(admitted) => admitted,
        Err(error) => return (super::mcp_action_response(Err(error)), admission_probes),
    };
    let PreparedRead { pool, query, scope } = read;
    assert!(pool.is_none(), "the probe cannot bypass SQL pool ownership");
    let (outcome, work_probes) = probe.measure(false, || match query(None) {
        Ok(work) => work.run(),
        Err(error) => job::Outcome::error(error),
    });
    before_final();
    let (answer, final_probes) = probe.measure(true, || {
        let _lease = lease;
        finish(&service, frame, &action, session, scope, outcome)
    });
    (answer, admission_probes + work_probes + final_probes)
}

async fn blocking<T: Send + 'static>(
    timer: Arc<FrameTimer>,
    work: impl FnOnce() -> T + Send + 'static,
) -> T {
    let waiting = WaitTimer::start();
    off_the_workers(move || {
        timer.record_pool_wait(waiting.elapsed());
        #[cfg(test)]
        return timer.sample_read_work(work);
        #[cfg(not(test))]
        work()
    })
    .await
}
