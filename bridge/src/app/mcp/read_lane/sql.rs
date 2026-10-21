use super::{AppState, BridgeAction, PreparedRead, ReadSession, ReadWork};
use crate::app::read_context::mcp_tasks::TaskQuery;

pub(super) fn supports(action: &BridgeAction) -> bool {
    TaskQuery::from_action(action).is_some()
}

pub(super) fn prepare(
    app: &mut AppState,
    session: &ReadSession,
    action: &BridgeAction,
) -> Option<Result<PreparedRead, String>> {
    let query = TaskQuery::from_action(action)?;
    Some(prepare_query(app, session, query))
}

fn prepare_query(
    app: &AppState,
    session: &ReadSession,
    query: TaskQuery,
) -> Result<PreparedRead, String> {
    let owner = &session.coding()?.entity_id;
    let project = app
        .projects
        .project_id_of(owner)
        .ok_or_else(|| format!("{owner} belongs to no project"))?;
    let scope = session.project(app, query.scope_error(project))?;
    let project = scope.id.clone();
    let service = app.read_service();
    let pool = app.store.as_ref().map(crate::store::Store::read_pool);
    #[cfg(test)]
    let gate = app.off_lock_gate.clone();
    Ok(PreparedRead::query(pool, move |connection| {
        #[cfg(test)]
        if let Some(gate) = gate {
            gate.arrive();
        }
        let answer = service.mcp_tasks(&project, &query, connection)?;
        Ok(ReadWork::result(Ok(answer)))
    })
    .for_project(scope))
}
