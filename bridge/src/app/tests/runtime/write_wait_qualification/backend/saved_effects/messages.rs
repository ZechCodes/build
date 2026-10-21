//! OFFSOURCE implementation draft, not registered or executed.
//! Uses existing exact native storage APIs; no AppState access.
use super::*;
use crate::operation::{OperationReceipt, OperationStatus};
use crate::thread::{ThreadItem, ThreadMessage};

pub(super) async fn extract(
    request: &CommonRequest,
    reply: &ReplyProof,
    handles: &Handles,
    completion: &NativeCompletion,
) -> Result<SavedEffect, String> {
    let mandatory_children_complete = check_original_complete(request, reply, completion)?;
    let (agent, operation) = match &request.effect {
        ExpectedEffect::Posted {
            agent,
            operation_id,
            ..
        } => (agent, operation_id.clone()),
        ExpectedEffect::Handoff { recipient, .. } => (
            recipient,
            Some(
                reply.reply["result"]["operation_id"]
                    .as_str()
                    .ok_or("actual handoff receipt lacks operation_id")?
                    .to_owned(),
            ),
        ),
        _ => return Err("message extractor requires Posted/Handoff".into()),
    };
    if let ExpectedEffect::Posted {
        operation_id: Some(expected),
        ..
    } = &request.effect
    {
        if reply.reply["result"]["operation_id"].as_str() != Some(expected.as_str()) {
            return Err("actual Posted reply operation ID differs from immutable request".into());
        }
    }
    let address = handles.address(agent)?;
    handles.current_addresses().validate(&address)?;
    check_expected_address(request, &address)?;
    if let ExpectedEffect::Handoff { caller, .. } = &request.effect {
        handles
            .current_addresses()
            .validate(&handles.address(caller)?)?;
    }
    let (receipt, message) = match operation {
        Some(operation) => managed(handles, operation).await?,
        None => {
            let id = reply.reply["result"]["message_id"]
                .as_str()
                .ok_or("native MCP post lacks message_id")?
                .to_owned();
            let store = handles.store.clone();
            let conversation = address.conversation.clone();
            let message = tokio::task::spawn_blocking(move || {
                store
                    .thread_message_by_id(&conversation, &id)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "returned message ID not durably stored".to_owned())
            })
            .await
            .map_err(|e| e.to_string())??;
            (None, message)
        }
    };
    handles.current_addresses().validate(&address)?;
    check_message(request, &message)?;
    let matched_turn = if let Some(receipt) = &receipt {
        check_receipt(request, receipt, &address, handles)?;
        unique_actual_turn(handles, &receipt.agent_id, &message.body)?
    } else {
        None
    };
    Ok(SavedEffect::Message {
        owner: receipt
            .as_ref()
            .map_or_else(|| address.owner.clone(), |r| r.entity_id.clone()),
        agent: receipt
            .as_ref()
            .map_or_else(|| address.agent.clone(), |r| r.agent_id.clone()),
        thread: receipt.as_ref().map_or_else(
            || address.conversation.clone(),
            |r| r.conversation_id.clone(),
        ),
        item: serde_json::to_value(ThreadItem::Message(message)).map_err(|e| e.to_string())?,
        operation_status: receipt.map(|r| r.status.as_str().to_owned()),
        matched_actual_session_turn: matched_turn,
        // This is the checked actual completed original, never a reply-success flag.
        mandatory_children_complete,
    })
}

async fn managed(
    handles: &Handles,
    operation: String,
) -> Result<(Option<OperationReceipt>, ThreadMessage), String> {
    let mut connection = handles.pool.acquire().await;
    tokio::task::spawn_blocking(move || {
        connection
            .read(|tx| -> Result<_, String> {
                let receipt = tx
                    .operation(&operation)
                    .map_err(|e| e.to_string())?
                    .ok_or("operation receipt missing")?;
                if receipt.operation_id != operation {
                    return Err("native operation ID differs from requested immutable key".into());
                }
                let prior = receipt
                    .posted_sequence
                    .checked_sub(1)
                    .ok_or("zero posted sequence")?;
                let mut items = tx
                    .thread_page_after(&receipt.conversation_id, prior, 1)
                    .map_err(|e| e.to_string())?;
                let item = items.pop().ok_or("operation's exact posted item missing")?;
                let ThreadItem::Message(message) = item else {
                    return Err("posted item is not message".into());
                };
                if message.sequence != receipt.posted_sequence
                    || message.operation_id.as_deref() != Some(operation.as_str())
                {
                    return Err("receipt/message sequence or operation mismatch".into());
                }
                Ok((Some(receipt), message))
            })
            .map_err(|e| e.to_string())?
    })
    .await
    .map_err(|e| e.to_string())?
}

fn check_receipt(
    request: &CommonRequest,
    receipt: &OperationReceipt,
    address: &IssuedCodingAddress,
    handles: &Handles,
) -> Result<(), String> {
    if receipt.method != crate::operation::THREAD_POST_METHOD
        || receipt.status != OperationStatus::Delivered
        || receipt.execution_error.is_some()
        || receipt.entity_id != address.owner
        || receipt.agent_id != address.agent
        || receipt.conversation_id != address.conversation
    {
        return Err("native receipt is not successful delivery to the captured address".into());
    }
    if let ExpectedEffect::Handoff { caller, .. } = &request.effect {
        let caller_address = handles.address(caller)?;
        handles.current_addresses().validate(&caller_address)?;
        if !receipt.requested_by.as_ref().is_some_and(|who| {
            who.agent_id == caller_address.agent
                && who.entity_id == caller_address.owner
                && who.conversation_id == caller_address.conversation
        }) {
            return Err("native handoff receipt requester canonical address differs".into());
        }
    } else if receipt.requested_by.is_some() {
        return Err("native human post receipt has an unexpected agent requester".into());
    }
    Ok(())
}
fn unique_actual_turn(
    handles: &Handles,
    agent: &str,
    body: &str,
) -> Result<Option<String>, String> {
    let turns = handles
        .logs
        .get(agent)
        .ok_or("actual recipient SessionLog absent")?
        .turns();
    let mut matches = turns.into_iter().filter(|turn| turn.contains(body));
    let turn = matches
        .next()
        .ok_or("actual provider did not receive saved body")?;
    if matches.next().is_some() {
        return Err("body matched multiple actual provider turns".into());
    }
    Ok(Some(turn))
}

fn check_expected_address(
    request: &CommonRequest,
    address: &IssuedCodingAddress,
) -> Result<(), String> {
    let (owner, agent, conversation) = match &request.effect {
        ExpectedEffect::Posted {
            owner,
            agent,
            thread,
            ..
        } => (owner, agent, thread),
        ExpectedEffect::Handoff {
            recipient_owner,
            recipient,
            thread,
            ..
        } => (recipient_owner, recipient, thread),
        _ => return Err("message effect absent".into()),
    };
    if owner != &address.owner || agent != &address.agent || conversation != &address.conversation {
        return Err("native message target differs from request expectation".into());
    }
    Ok(())
}

fn check_message(request: &CommonRequest, message: &ThreadMessage) -> Result<(), String> {
    let (body, role, caller) = match &request.effect {
        ExpectedEffect::Posted { body, role, .. } => (body, *role, None),
        ExpectedEffect::Handoff { body, caller, .. } => (body, "user", Some(caller)),
        _ => return Err("message effect absent".into()),
    };
    if body != &message.body
        || serde_json::to_value(message.role).map_err(|error| error.to_string())? != role
        || caller.is_some_and(|caller| {
            message
                .from_agent
                .as_ref()
                .is_none_or(|sender| sender.id != *caller)
        })
    {
        return Err(
            "native saved message body/role/sender differs before SessionLog matching".into(),
        );
    }
    Ok(())
}
