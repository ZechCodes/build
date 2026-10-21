//! Catalogue work and accepted roster payloads, without application access.
use super::service::{Inputs, McpReadService};
use super::{BridgeAction, PreparedRead, ReadSession, ReadWork};
use serde_json::{json, Value};
use std::sync::Arc;

pub(super) fn supports(action: &BridgeAction) -> bool {
    matches!(
        action,
        BridgeAction::ListHarnesses
            | BridgeAction::ListWorkspaceAgents { .. }
            | BridgeAction::ListProjectAgents
    )
}

pub(super) fn prepare(
    service: &McpReadService,
    inputs: &Arc<Inputs>,
    session: &ReadSession,
    action: &BridgeAction,
) -> Option<Result<PreparedRead, String>> {
    match action {
        BridgeAction::ListHarnesses => {
            let inputs = inputs.clone();
            let readings = service.readings.clone();
            Some(Ok(PreparedRead::query(None, move |_| {
                Ok(ReadWork::external(move || {
                    // Settings decoding and existing TTL/executable resolution are work,
                    // never admission/final validation or writer publication.
                    let settings = inputs.harness_settings()?;
                    Ok(crate::app::model_catalog::harness_table_from(
                        settings.default_harness,
                        &settings.role_models,
                        &readings,
                    ))
                }))
            })))
        }
        BridgeAction::ListWorkspaceAgents { workspace_id } => {
            Some(workspace_agents(inputs, session, workspace_id))
        }
        BridgeAction::ListProjectAgents => Some(project_agents(inputs, session)),
        _ => None,
    }
}

fn workspace_agents(
    inputs: &Arc<Inputs>,
    session: &ReadSession,
    workspace: &str,
) -> Result<PreparedRead, String> {
    let scope = session.project(inputs, format!("unknown workspace_id: {workspace}"))?;
    let project = inputs.workspace(workspace)?;
    if project != scope.id {
        return Err(format!(
            "workspace {workspace} is not in project {}",
            scope.id
        ));
    }
    let selected = inputs.workspace_selection(workspace)?;
    let payload = capture_payload(inputs, selected.as_deref())?;
    let inputs = inputs.clone();
    let project = scope.id.clone();
    let workspace = workspace.to_owned();
    Ok(PreparedRead::query(None, move |_| {
        let (mut value, targets) = roster(&inputs, selected.as_deref(), payload, false)?;
        value["workspace_id"] = json!(workspace);
        Ok(
            ReadWork::result(Ok(value)).finish_with(move |latest, value| {
                if latest.workspace(&workspace)? != project {
                    return Err(format!("unknown workspace_id: {workspace}"));
                }
                if latest.workspace_selection(&workspace)? != selected {
                    return Err(format!("unknown workspace_id: {workspace}"));
                }
                validate_targets(latest, &targets)?;
                Ok(value)
            }),
        )
    })
    .for_project(scope))
}

fn project_agents(inputs: &Arc<Inputs>, session: &ReadSession) -> Result<PreparedRead, String> {
    let owner = &session.coding()?.owner;
    let (project, _) = inputs.project(owner)?;
    let scope = session.project(inputs, format!("unknown project_id: {project}"))?;
    let project = scope.id.clone();
    let selected = inputs.project_selection(&project)?;
    let payload = capture_payload(inputs, selected.as_deref())?;
    let inputs = inputs.clone();
    Ok(PreparedRead::query(None, move |_| {
        let (mut value, targets) = roster(&inputs, selected.as_deref(), payload, true)?;
        value["project_id"] = json!(project);
        Ok(
            ReadWork::result(Ok(value)).finish_with(move |latest, value| {
                if latest.project_selection(&project)? != selected {
                    return Err(format!("unknown project_id: {project}"));
                }
                validate_targets(latest, &targets)?;
                Ok(value)
            }),
        )
    })
    .for_project(scope))
}

fn capture_payload(
    inputs: &Inputs,
    selected: Option<&str>,
) -> Result<Option<crate::app::read_view::SharedJson>, String> {
    selected
        .map(|owner| {
            inputs
                .view
                .get_shared("agent.list", owner)
                .cloned()
                .ok_or_else(|| format!("agent.list: unknown entity {owner}"))
        })
        .transpose()
}

struct RosterTarget {
    owner: String,
    agent: String,
    conversation: String,
    thread_id: String,
    generation: u64,
}

fn roster(
    inputs: &Inputs,
    owner: Option<&str>,
    payload: Option<crate::app::read_view::SharedJson>,
    workers: bool,
) -> Result<(Value, Vec<RosterTarget>), String> {
    let Some(owner) = owner else {
        return Ok((json!({"entity_id":Value::Null,"agents":[]}), Vec::new()));
    };
    let mut answer = payload
        .expect("selected owner captured its accepted payload")
        .materialize();
    crate::app::read_view::filter_project_workers(
        &json!({"include_project_workers":workers}),
        &mut answer,
    );
    let targets = answer["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| {
            let field = |name| {
                row[name]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("agent.list: missing {name}"))
            };
            Ok(RosterTarget {
                owner: owner.into(),
                agent: field("id")?,
                conversation: field("conversation_id")?,
                thread_id: field("thread_id")?,
                generation: row["thread_generation_revision"]
                    .as_u64()
                    .ok_or("agent.list: missing thread_generation_revision")?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    // Every fence comes FROM this payload. Never attach a newer head to it.
    validate_targets(inputs, &targets)?;
    Ok((answer, targets))
}

fn validate_targets(inputs: &Inputs, targets: &[RosterTarget]) -> Result<(), String> {
    for target in targets {
        let actual = inputs
            .addresses
            .resolve(&target.owner, Some(&target.agent))?
            .issued;
        inputs.guard_reservation(&actual.conversation)?;
        if actual.thread_id != target.thread_id || actual.generation != target.generation {
            return Err(format!(
                "stale thread_id {}; current thread is {}",
                target.thread_id, actual.thread_id
            ));
        }
        let mut expected = actual;
        expected.conversation = target.conversation.clone();
        expected.thread_id = target.thread_id.clone();
        expected.generation = target.generation;
        inputs.addresses.validate(&expected)?;
    }
    Ok(())
}
