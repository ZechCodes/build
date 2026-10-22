//! Explicit history reads retain committed identity and exact frozen targets.
use super::service::{Inputs, McpReadService};
use super::{PreparedRead, ReadSession, ReadWork};
use crate::app::conversations::sql_read::McpHistoryQuery;
use crate::app::read_context::FrozenMcpAddress;
use crate::mcp::BridgeAction;
use serde_json::{json, Value};
use std::sync::Arc;

pub(super) fn supports(action: &BridgeAction) -> bool {
    matches!(
        action,
        BridgeAction::SearchConversation { .. }
            | BridgeAction::ReadConversation { .. }
            | BridgeAction::TrackerReadComment { .. }
    )
}

pub(super) fn prepare(
    service: &McpReadService,
    inputs: &Arc<Inputs>,
    session: &ReadSession,
    action: &BridgeAction,
) -> Option<Result<PreparedRead, String>> {
    match action {
        BridgeAction::SearchConversation { query } => Some((|| {
            let caller = session.coding()?;
            let target = inputs
                .addresses
                .resolve(&caller.owner, Some(&caller.agent))?;
            Ok(prepare_history(
                service,
                inputs,
                target,
                McpHistoryQuery::Search(query.clone()),
            ))
        })()),
        BridgeAction::ReadConversation {
            entity_id,
            agent_id,
            limit,
        } => Some((|| {
            let target = inputs.addresses.resolve(entity_id, agent_id.as_deref())?;
            // Router history targets have no reset-reservation guard in legacy
            // behavior: accepted old history remains readable during retirement.
            Ok(prepare_history(
                service,
                inputs,
                target,
                McpHistoryQuery::CatchUp(*limit),
            ))
        })()),
        BridgeAction::TrackerReadComment { comment_id } => {
            Some(prepare_comment(service, inputs, session, comment_id))
        }
        _ => None,
    }
}

fn params(target: &FrozenMcpAddress) -> Value {
    json!({"entity_id":target.issued.owner,"agent_id":target.issued.agent,"conversation_id":target.issued.conversation,"thread_id":target.issued.thread_id})
}

fn prepare_history(
    service: &McpReadService,
    inputs: &Inputs,
    target: FrozenMcpAddress,
    query: McpHistoryQuery,
) -> PreparedRead {
    let context = service.read.mcp_history_context();
    let pool = service.pool.clone();
    let previous = inputs.view.clone();
    #[cfg(test)]
    let gate = service.gate.clone();
    PreparedRead::query(pool, move |connection| {
        #[cfg(test)]
        if let Some(gate) = gate {
            gate.arrive();
        }
        let params = params(&target);
        let fallback = previous
            .history(&target.issued.conversation)
            .map(|history| (history, target.resident_floor));
        let work = match connection {
            Some(connection) => connection
                .read(|transaction| {
                    context.history(
                        Some(transaction),
                        &target.issued.owner,
                        &params,
                        query,
                        fallback,
                    )
                })
                .map_err(|error| format!("conversation store: {error}"))??,
            None => context.history(None, &target.issued.owner, &params, query, fallback)?,
        };
        Ok(
            ReadWork::external(move || Ok(work.render())).finish_with(move |latest, value| {
                latest.addresses.validate(&target.issued)?;
                Ok(value)
            }),
        )
    })
}

fn prepare_comment(
    service: &McpReadService,
    inputs: &Inputs,
    session: &ReadSession,
    id: &str,
) -> Result<PreparedRead, String> {
    let unknown = format!("There is no comment {id} on this project's tasks.");
    let scope = session.project(inputs, unknown)?;
    let project = scope.id.clone();
    let reader = session.coding()?.agent.clone();
    let context = service.read.mcp_history_context();
    let read = service.read.clone();
    let pool = service.pool.clone();
    let id = id.to_owned();
    Ok(PreparedRead::query(pool, move |connection| {
        let connection = connection.ok_or("tasks need a durable store; this bridge has none")?;
        let (value, comment, fence) = connection
            .read(|transaction| context.comment(transaction, &project, &id))
            .map_err(|error| error.to_string())??;
        Ok(
            ReadWork::result(Ok(value)).finish_with(move |latest, mut value| {
                read.validate_mcp_comment(&fence)?;
                decorate_comment(latest, &reader, &comment, &mut value);
                Ok(value)
            }),
        )
    })
    .for_project(scope))
}

fn decorate_comment(
    inputs: &Inputs,
    reader: &str,
    comment: &crate::tracker::TaskComment,
    value: &mut Value,
) {
    if !inputs.addresses.is_pinned_coordinator(reader) {
        return;
    }
    let Some(reading) = &comment.author_context else {
        return;
    };
    let author = comment
        .author
        .agent_id()
        .and_then(|agent| inputs.addresses.display_name(agent))
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Agent {}", comment.author.agent_id().unwrap_or_default()));
    value["author_context_line"] = json!(reading.sentence(&author));
}
