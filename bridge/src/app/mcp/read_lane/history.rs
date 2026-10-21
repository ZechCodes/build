//! Explicit history tools, using committed admission and exact frozen targets.
use super::{PreparedRead, ReadSession, ReadWork};
use crate::app::conversations::sql_read::McpHistoryQuery;
use crate::app::AppState;
use crate::mcp::BridgeAction;
use serde_json::{json, Value};

pub(super) fn supports(action: &BridgeAction) -> bool {
    matches!(
        action,
        BridgeAction::SearchConversation { .. }
            | BridgeAction::ReadConversation { .. }
            | BridgeAction::TrackerReadComment { .. }
    )
}

pub(super) fn prepare(
    app: &mut AppState,
    session: &ReadSession,
    action: &BridgeAction,
) -> Option<Result<PreparedRead, String>> {
    match action {
        BridgeAction::SearchConversation { query } => Some((|| {
            let endpoint = session.coding()?;
            let target = FrozenTarget::capture(app, &endpoint.entity_id, Some(&endpoint.agent_id))?;
            Ok(prepare_history(
                app,
                target,
                McpHistoryQuery::Search(query.clone()),
            ))
        })()),
        BridgeAction::ReadConversation {
            entity_id,
            agent_id,
            limit,
        } => Some((|| {
            let target = FrozenTarget::capture(app, entity_id, agent_id.as_deref())?;
            Ok(prepare_history(
                app,
                target,
                McpHistoryQuery::CatchUp(*limit),
            ))
        })()),
        BridgeAction::TrackerReadComment { comment_id } => {
            Some(prepare_comment(app, session, comment_id))
        }
        _ => None,
    }
}

struct FrozenTarget {
    owner: String,
    agent: String,
    conversation: String,
    thread: String,
    resident_floor: u64,
}

impl FrozenTarget {
    fn capture(app: &AppState, owner: &str, agent: Option<&str>) -> Result<Self, String> {
        let address = app.resolve_conversation_address(owner, agent)?;
        let thread = app.conversation_at(&address)?;
        Ok(Self {
            owner: owner.into(),
            agent: address.agent_id,
            conversation: address.conversation_id,
            thread: thread.id.clone(),
            resident_floor: thread.resident_from_sequence(),
        })
    }

    fn params(&self) -> Value {
        json!({"entity_id":self.owner,"agent_id":self.agent,"conversation_id":self.conversation,"thread_id":self.thread})
    }

    fn validate(&self, app: &AppState) -> Result<(), String> {
        app.resolve_conversation_params(&self.owner, &self.params())
            .map(|_| ())
    }
}

fn prepare_history(app: &AppState, target: FrozenTarget, query: McpHistoryQuery) -> PreparedRead {
    let context = app.read_service().mcp_history_context();
    let pool = app.store.as_ref().map(|store| store.read_pool());
    let previous = app.read_view.load();
    #[cfg(test)]
    let gate = app.off_lock_gate.clone();
    PreparedRead::query(pool, move |connection| {
        #[cfg(test)]
        if let Some(gate) = gate {
            gate.arrive();
        }
        let params = target.params();
        let fallback = previous
            .history(&target.conversation)
            .map(|history| (history, target.resident_floor));
        let work = match connection {
            Some(connection) => connection
                .read(|transaction| {
                    context.history(Some(transaction), &target.owner, &params, query, fallback)
                })
                .map_err(|error| format!("conversation store: {error}"))??,
            None => context.history(None, &target.owner, &params, query, fallback)?,
        };
        Ok(
            ReadWork::external(move || Ok(work.render())).finish_with(move |app, value| {
                target.validate(app)?;
                Ok(value)
            }),
        )
    })
}

fn prepare_comment(
    app: &AppState,
    session: &ReadSession,
    id: &str,
) -> Result<PreparedRead, String> {
    let unknown = format!("There is no comment {id} on this project's tasks.");
    let scope = session.project(app, unknown)?;
    let project = scope.id.clone();
    let reader = session.coding()?.agent_id.clone();
    let context = app.read_service().mcp_history_context();
    let service = app.read_service();
    let pool = app.store.as_ref().map(|store| store.read_pool());
    let id = id.to_owned();
    Ok(PreparedRead::query(pool, move |connection| {
        let connection = connection.ok_or("tasks need a durable store; this bridge has none")?;
        let (value, comment, fence) = connection
            .read(|transaction| context.comment(transaction, &project, &id))
            .map_err(|error| error.to_string())??;
        Ok(
            ReadWork::result(Ok(value)).finish_with(move |app, mut value| {
                service.validate_mcp_comment(&fence)?;
                app.decorate_comment_read(&reader, &comment, &mut value);
                Ok(value)
            }),
        )
    })
    .for_project(scope))
}
