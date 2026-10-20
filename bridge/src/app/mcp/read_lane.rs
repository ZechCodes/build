//! Exact MCP reads bypass mutation turns while retaining session admission.
mod admission;
mod guarded;
mod history;
mod job;
mod mixed;
mod snapshots;
mod sql;

use crate::app::{off_the_workers, AppState};
use crate::mcp::{BridgeAction, DoneReport};
use crate::timing::{FrameTimer, WaitTimer};
use admission::{ProjectScope, ReadSession};
use job::{PreparedRead, ReadWork};
use serde_json::Value;
use std::sync::{Arc, Mutex};

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
    app: &mut AppState,
    session: &ReadSession,
    action: &BridgeAction,
) -> Result<PreparedRead, String> {
    sql::prepare(app, session, action)
        .or_else(|| guarded::prepare(app, session, action))
        .or_else(|| snapshots::prepare(app, session, action))
        .or_else(|| history::prepare(app, session, action))
        .or_else(|| mixed::prepare(app, session, action))
        .unwrap_or_else(|| Err("invalid MCP action request".into()))
}

struct Admitted {
    session: ReadSession,
    read: PreparedRead,
    lease: Option<crate::update::AdmissionLease>,
}

fn admit(
    state: &Arc<Mutex<AppState>>,
    frame: &Value,
    action: &BridgeAction,
    timer: &FrameTimer,
) -> Result<Admitted, String> {
    let mut app = timer.lock(state);
    let lease = app
        .update_admission()
        .map(|gate| {
            gate.try_enter()
                .ok_or_else(|| "bridge update handoff in progress".to_string())
        })
        .transpose()?;
    let session = ReadSession::authenticate(&app, frame, action)?;
    let read = prepare(&mut app, &session, action)?;
    Ok(Admitted {
        session,
        read,
        lease,
    })
}

pub(super) async fn answer(
    state: Arc<Mutex<AppState>>,
    frame: Value,
    action: BridgeAction,
    timer: FrameTimer,
) -> Value {
    #[cfg(test)]
    timer.sample_tool(action.tool_name());
    let timer = Arc::new(timer);
    let admission = {
        let state = Arc::clone(&state);
        let frame = frame.clone();
        let action = action.clone();
        let clock = Arc::clone(&timer);
        blocking(Arc::clone(&timer), move || {
            admit(&state, &frame, &action, &clock)
        })
        .await
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
    let clock = Arc::clone(&timer);
    blocking(Arc::clone(&timer), move || {
        let _lease = lease;
        let mut outcome = Some(outcome);
        let result = {
            let app = clock.lock(&state);
            session
                .validate(&app, &frame, &action)
                .and_then(|()| scope.as_ref().map_or(Ok(()), |scope| scope.validate(&app)))
                .and_then(|()| outcome.take().unwrap().finish(&app))
        };
        // Rejected payloads and response serialization are dropped/run after
        // the app guard, including when a final auth check rejected the read.
        drop(outcome);
        super::mcp_action_response(result)
    })
    .await
}

async fn blocking<T: Send + 'static>(
    timer: Arc<FrameTimer>,
    work: impl FnOnce() -> T + Send + 'static,
) -> T {
    let waiting = WaitTimer::start();
    off_the_workers(move || {
        timer.record_pool_wait(waiting.elapsed());
        work()
    })
    .await
}
