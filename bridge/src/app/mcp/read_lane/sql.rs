use super::service::{Inputs, McpReadService};
use super::{BridgeAction, PreparedRead, ReadSession, ReadWork};
use crate::app::read_context::mcp_tasks::TaskQuery;
use std::sync::Arc;

pub(super) fn supports(action: &BridgeAction) -> bool {
    TaskQuery::from_action(action).is_some()
}

pub(super) fn prepare(
    service: &McpReadService,
    inputs: &Arc<Inputs>,
    session: &ReadSession,
    action: &BridgeAction,
) -> Option<Result<PreparedRead, String>> {
    let query = TaskQuery::from_action(action)?;
    Some(prepare_query(service, inputs, session, query))
}

fn prepare_query(
    service: &McpReadService,
    inputs: &Inputs,
    session: &ReadSession,
    query: TaskQuery,
) -> Result<PreparedRead, String> {
    let owner = &session.coding()?.owner;
    let (project, _) = inputs.project(owner)?;
    let scope = session.project(inputs, query.scope_error(project))?;
    let project = scope.id.clone();
    let read = service.read.clone();
    let pool = service.pool.clone();
    #[cfg(test)]
    let gate = service.gate.clone();
    Ok(PreparedRead::query(pool, move |connection| {
        #[cfg(test)]
        if let Some(gate) = gate {
            gate.arrive();
        }
        let answer = read.mcp_tasks(&project, &query, connection)?;
        Ok(ReadWork::result(Ok(answer)))
    })
    .for_project(scope))
}
