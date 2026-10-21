//! Immutable producer snapshots for maintenance-shaped MCP list tools.
use super::service::{Inputs, McpReadService};
use super::{PreparedRead, ReadSession, ReadWork};
use crate::app::read_view::SharedJson;
use crate::mcp::BridgeAction;
use serde_json::{json, Value};
use std::sync::Arc;

pub(super) fn supports(action: &BridgeAction) -> bool {
    matches!(
        action,
        BridgeAction::ListProjects | BridgeAction::ListWork | BridgeAction::ListWorkspaces
    )
}

pub(super) fn prepare(
    _service: &McpReadService,
    inputs: &Arc<Inputs>,
    session: &ReadSession,
    action: &BridgeAction,
) -> Option<Result<PreparedRead, String>> {
    match action {
        BridgeAction::ListProjects | BridgeAction::ListWork | BridgeAction::ListWorkspaces => {
            Some(capture(inputs, session, action))
        }
        _ => None,
    }
}

fn capture(
    inputs: &Inputs,
    session: &ReadSession,
    action: &BridgeAction,
) -> Result<PreparedRead, String> {
    let (method, owner, scope) = match action {
        BridgeAction::ListProjects => ("project.list", String::new(), None),
        BridgeAction::ListWork => ("board.list", String::new(), None),
        BridgeAction::ListWorkspaces => {
            let caller = &session.coding()?.owner;
            let (project, _) = inputs.project(caller)?;
            let scope = session.project(inputs, format!("unknown project_id: {project}"))?;
            ("workspace.list", scope.id.clone(), Some(scope))
        }
        _ => unreachable!("only exact snapshot list actions reach capture"),
    };
    // Retain just the accepted immutable fragment. Materializing a large list
    // and formatting the router digest happens in independent work.
    let value = inputs.view.get_shared(method, &owner).cloned();
    let value = match value {
        Some(value) => value,
        None if method == "workspace.list" => SharedJson::value(json!({"workspaces":[]})),
        None => return Err(format!("{method}: snapshot was not initialized")),
    };
    let work_digest = matches!(action, BridgeAction::ListWork);
    let prepared = PreparedRead::query(None, move |_| {
        Ok(ReadWork::external(move || {
            let value = {
                #[cfg(test)]
                let _phase = crate::timing::phase_samples::phase("snapshot.materialize");
                value.materialize()
            };
            Ok(if work_digest {
                router_work_digest(value)
            } else {
                value
            })
        }))
    });
    Ok(match scope {
        Some(scope) => prepared.for_project(scope),
        None => prepared,
    })
}

/// Match the router's existing compact wire shape and capture exclusion. The
/// complete board is supplied by its normal producer; this performs no refresh.
fn router_work_digest(board: Value) -> Value {
    #[cfg(test)]
    let _phase = crate::timing::phase_samples::phase("snapshot.work_digest");
    let work = board["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| row["kind"] != "capture")
        .map(|row| {
            json!({
                "kind": row["kind"],
                "entity_id": row["run_id"].as_str()
                    .or_else(|| row["task_id"].as_str())
                    .or_else(|| row["worktree_id"].as_str()),
                "project_id": row["project_id"],
                "project": row["project"],
                "branch": row["branch"],
                "task_id": row["task_id"],
                "title": row["title"],
                "state": row["state"],
                "working": row["working"],
            })
        })
        .collect::<Vec<_>>();
    json!({"work":work})
}
