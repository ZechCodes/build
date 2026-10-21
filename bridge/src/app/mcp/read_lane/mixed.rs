//! Saved review reads own SQL metadata and then independent external work.
use super::{PreparedRead, ReadSession, ReadWork};
use crate::app::AppState;
use crate::mcp::BridgeAction;
use serde_json::json;

pub(super) fn supports(action: &BridgeAction) -> bool {
    matches!(
        action,
        BridgeAction::TrackerGetReview { .. } | BridgeAction::TrackerReadReview { .. }
    )
}

pub(super) fn prepare(
    app: &mut AppState,
    session: &ReadSession,
    action: &BridgeAction,
) -> Option<Result<PreparedRead, String>> {
    if !supports(action) {
        return None;
    }
    Some(prepare_review(app, session, action))
}

fn prepare_review(
    app: &AppState,
    session: &ReadSession,
    action: &BridgeAction,
) -> Result<PreparedRead, String> {
    if app.project_deletion_in_progress {
        return Err(crate::app::rpc::PROJECT_DELETION_IN_PROGRESS.into());
    }
    let (task, method, params) = match action {
        BridgeAction::TrackerGetReview { task_id } => {
            (task_id, "tasks.review.get", json!({"task_id":task_id}))
        }
        BridgeAction::TrackerReadReview {
            task_id,
            snapshot_id,
            directory_id,
            mode,
            path,
            paths,
            patch,
            range,
        } => {
            let params = crate::api::v1::reviews::ReviewDiffParams {
                task_id: task_id.clone(),
                snapshot_id: snapshot_id.clone(),
                directory_id: directory_id.clone(),
                read: crate::reviews::read::ReviewReadRequest {
                    mode: *mode,
                    path: path.clone(),
                    paths: paths.clone(),
                    patch: *patch,
                    range: *range,
                },
            };
            (
                task_id,
                "tasks.review.diff",
                serde_json::to_value(params).expect("review read params serialize"),
            )
        }
        _ => unreachable!("review allowlist checked"),
    };
    let unknown = format!("unknown task_id: {task}");
    let owner = &session.coding()?.entity_id;
    // Review tools' existing no-project refusal differs from plain task tools.
    app.projects
        .project_id_of(owner)
        .ok_or_else(|| unknown.clone())?;
    let scope = session.project(app, unknown)?;
    let project = scope.id.clone();
    let view = app.read_view.clone();
    let context = view
        .load()
        .observations
        .clone()
        .expect("observation inputs initialized before MCP admission");
    let pool = app.store.as_ref().map(|store| store.read_pool());
    Ok(PreparedRead::query(pool, move |connection| {
        let connection = connection.ok_or("tasks need a durable store; this bridge has none")?;
        let job = connection
            .read(|transaction| {
                context
                    .mixed
                    .prepare_mcp_review(&project, method, &params, transaction)
            })
            .map_err(|error| error.to_string())?
            .map_err(raw_error)?;
        let fence = job.fence();
        Ok(
            ReadWork::external(move || job.run().map_err(raw_error)).finish_with(
                move |_, value| {
                    let latest = view.load();
                    let context = latest
                        .observations
                        .as_ref()
                        .expect("initialized inputs remain published");
                    context.mixed.validate(&fence).map_err(raw_error)?;
                    Ok(value)
                },
            ),
        )
    })
    .for_project(scope))
}

fn raw_error(error: crate::api::ApiError) -> String {
    match error.details() {
        Some(details) => {
            crate::api::v1::reviews::errors::encode(error.code(), error.message(), details.clone())
        }
        None => error.message().into(),
    }
}
