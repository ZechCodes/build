//! Small immutable catalog/roster reads; never call mutation dispatch or drain.
use super::{AppState, BridgeAction, PreparedRead, ReadSession, ReadWork};
use crate::app::mcp::McpConversationGeneration;
use serde_json::{json, Value};

pub(super) fn supports(action: &BridgeAction) -> bool {
    matches!(
        action,
        BridgeAction::ListHarnesses
            | BridgeAction::ListWorkspaceAgents { .. }
            | BridgeAction::ListProjectAgents
    )
}

pub(super) fn prepare(
    app: &mut AppState,
    session: &ReadSession,
    action: &BridgeAction,
) -> Option<Result<PreparedRead, String>> {
    match action {
        BridgeAction::ListHarnesses => Some(Ok(PreparedRead::ready(Ok(app.harness_table())))),
        BridgeAction::ListWorkspaceAgents { workspace_id } => {
            Some(workspace_agents(app, session, workspace_id))
        }
        BridgeAction::ListProjectAgents => Some(project_agents(app, session)),
        _ => None,
    }
}

fn workspace_agents(
    app: &mut AppState,
    session: &ReadSession,
    workspace_id: &str,
) -> Result<PreparedRead, String> {
    let scope = session.project(app, format!("unknown workspace_id: {workspace_id}"))?;
    let workspace = app
        .workspaces
        .get(workspace_id)
        .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))?;
    if workspace.project_id != scope.id {
        return Err(format!(
            "workspace {workspace_id} is not in project {}",
            scope.id
        ));
    }
    require_snapshot_inputs(app)?;
    let selected = app.workspace_snapshot_conversation_owner(workspace);
    let mut answer = match selected.as_deref() {
        Some(entity) => app.agent_list(&json!({"entity_id":entity}))?,
        None => json!({"entity_id":Value::Null,"agents":[]}),
    };
    answer["workspace_id"] = json!(workspace_id);
    let project = scope.id.clone();
    let workspace_id = workspace_id.to_string();
    let targets = targets(app, &answer)?;
    let work = ReadWork::result(Ok(answer)).finish_with(move |app, value| {
        let workspace = app
            .workspaces
            .get(&workspace_id)
            .filter(|workspace| workspace.project_id == project)
            .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))?;
        require_snapshot_inputs(app)?;
        if app.workspace_snapshot_conversation_owner(workspace) != selected {
            return Err(format!("unknown workspace_id: {workspace_id}"));
        }
        validate_targets(app, &targets)?;
        Ok(value)
    });
    Ok(PreparedRead::query(None, move |_| Ok(work)).for_project(scope))
}

fn project_agents(app: &mut AppState, session: &ReadSession) -> Result<PreparedRead, String> {
    let owner = &session.coding()?.entity_id;
    let project = app
        .projects
        .project_id_of(owner)
        .ok_or_else(|| format!("{owner} belongs to no project"))?;
    let scope = session.project(app, format!("unknown project_id: {project}"))?;
    let project = scope.id.clone();
    require_snapshot_inputs(app)?;
    let selected = app.project_snapshot_conversation_run(&project);
    let mut answer = match selected.as_deref() {
        Some(entity) => app.agent_list(&json!({
            "entity_id":entity,
            "include_project_workers":true,
        }))?,
        None => json!({"entity_id":Value::Null,"agents":[]}),
    };
    answer["project_id"] = json!(project);
    let targets = targets(app, &answer)?;
    let work = ReadWork::result(Ok(answer)).finish_with(move |app, value| {
        require_snapshot_inputs(app)?;
        if app.project_snapshot_conversation_run(&project) != selected {
            return Err(format!("unknown project_id: {project}"));
        }
        validate_targets(app, &targets)?;
        Ok(value)
    });
    Ok(PreparedRead::query(None, move |_| Ok(work)).for_project(scope))
}

// Observed helpers intentionally fall back to probing before initialization.
// A read adapter must refuse that cold path rather than initialize under guard.
fn require_snapshot_inputs(app: &AppState) -> Result<(), String> {
    if app.snapshot_projection_initialized() {
        Ok(())
    } else {
        Err("agent.list: snapshot was not initialized".into())
    }
}

fn targets(app: &AppState, answer: &Value) -> Result<Vec<McpConversationGeneration>, String> {
    let Some(owner) = answer["entity_id"].as_str() else {
        return Ok(Vec::new());
    };
    answer["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| row["id"].as_str())
        .map(|agent| app.mcp_conversation_generation(owner, agent))
        .collect()
}

fn validate_targets(app: &AppState, targets: &[McpConversationGeneration]) -> Result<(), String> {
    for target in targets {
        target.guard(app)?;
    }
    Ok(())
}
