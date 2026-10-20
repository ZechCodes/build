//! Small immutable catalog/roster reads; never call mutation dispatch or drain.
use super::{AppState, BridgeAction, PreparedRead, ReadSession, ReadWork};
use crate::app::mcp::McpConversationGeneration;
use serde_json::Value;

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
    let owner = &session.coding()?.entity_id;
    let scope = session.project(app, format!("unknown workspace_id: {workspace_id}"))?;
    let answer = app.project_agent_workspace_agents(owner, workspace_id)?;
    let selected = app
        .workspaces
        .get(workspace_id)
        .and_then(|workspace| app.workspace_snapshot_conversation_owner(workspace));
    if answer["entity_id"].as_str() != selected.as_deref() {
        return Err(format!("unknown workspace_id: {workspace_id}"));
    }
    let project = scope.id.clone();
    let workspace_id = workspace_id.to_string();
    let targets = targets(app, &answer)?;
    let work = ReadWork::result(Ok(answer)).finish_with(move |app, value| {
        let workspace = app
            .workspaces
            .get(&workspace_id)
            .filter(|workspace| workspace.project_id == project)
            .ok_or_else(|| format!("unknown workspace_id: {workspace_id}"))?;
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
    let answer = app.project_agent_project_agents(owner)?;
    let selected = app.project_snapshot_conversation_run(&project);
    if answer["entity_id"].as_str() != selected.as_deref() {
        return Err(format!("unknown project_id: {project}"));
    }
    let targets = targets(app, &answer)?;
    let work = ReadWork::result(Ok(answer)).finish_with(move |app, value| {
        if app.project_snapshot_conversation_run(&project) != selected {
            return Err(format!("unknown project_id: {project}"));
        }
        validate_targets(app, &targets)?;
        Ok(value)
    });
    Ok(PreparedRead::query(None, move |_| Ok(work)).for_project(scope))
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
