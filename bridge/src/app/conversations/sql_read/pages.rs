//! Bounded page rendering from one request transaction or its memory fallback.
use serde_json::Value;

use crate::store::ReadTransaction;
use crate::thread::{PageCut, Thread};

use super::super::read::{
    activity_page_limit, activity_span_params, thread_page_forward_limit, thread_page_limit,
};
use super::conversation_error;

pub(super) fn page(
    transaction: Option<&ReadTransaction<'_>>,
    thread: &Thread,
    params: &Value,
) -> Result<Value, String> {
    let before = params.get("before_sequence").and_then(Value::as_u64);
    let after = params.get("after_sequence").and_then(Value::as_u64);
    let newest = params
        .get("newest")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if before.is_some() && after.is_some() {
        return Err("provide exactly one of before_sequence and after_sequence".into());
    }
    if newest && after.is_none() {
        return Err("newest must be used with after_sequence".into());
    }
    let limit = if after.is_some() {
        thread_page_forward_limit(params)
    } else {
        thread_page_limit(params)
    };
    if params.get("activity").and_then(Value::as_str) == Some("newest") {
        return lean_page(transaction, thread, before, after, newest, limit);
    }
    match transaction {
        Some(transaction) => stored_page(transaction, thread, before, after, newest, limit),
        None => memory_page(thread, before, after, newest, limit),
    }
}

fn lean_page(
    transaction: Option<&ReadTransaction<'_>>,
    thread: &Thread,
    before: Option<u64>,
    after: Option<u64>,
    newest: bool,
    limit: usize,
) -> Result<Value, String> {
    if let Some(transaction) = transaction {
        let (cut, more) = transaction
            .thread_conversation_page_newest_activity(
                &thread.agent.id,
                after,
                before,
                limit,
                newest,
            )
            .map_err(conversation_error)?;
        return Ok(thread.wire_value_of_page(&cut, more));
    }
    if thread.lean_page_reaches_stored_history() {
        return Err("this conversation's history is not stored".into());
    }
    Ok(thread.wire_value_page_newest_activity(after, before, limit, newest))
}

fn stored_page(
    transaction: &ReadTransaction<'_>,
    thread: &Thread,
    before: Option<u64>,
    after: Option<u64>,
    newest: bool,
    limit: usize,
) -> Result<Value, String> {
    let Some(after) = after else {
        let (cut, more) = transaction
            .thread_conversation_page(&thread.agent.id, before, limit)
            .map_err(conversation_error)?;
        return Ok(thread.wire_value_of_page(&cut, more));
    };
    let mut items = if newest {
        transaction.thread_page(&thread.agent.id, None, limit + 1)
    } else {
        transaction.thread_page_after(&thread.agent.id, after, limit + 1)
    }
    .map_err(conversation_error)?;
    items.retain(|item| item.sequence() > after);
    let more = items.len() > limit;
    if newest && more {
        items.remove(0);
    }
    items.truncate(limit);
    Ok(thread.wire_value_of_page(
        &PageCut {
            items,
            digests: Vec::new(),
        },
        more,
    ))
}

fn memory_page(
    thread: &Thread,
    before: Option<u64>,
    after: Option<u64>,
    newest: bool,
    limit: usize,
) -> Result<Value, String> {
    if let Some(after) = after {
        if newest {
            return Ok(thread.wire_value_newest_page_after(after, limit));
        }
        if thread.forward_page_reaches_stored_history(after) {
            return Err("this conversation's history is not stored".into());
        }
        return Ok(thread.wire_value_page_after(after, limit));
    }
    if thread.page_reaches_stored_history(before, limit) {
        return Err("this conversation's history is not stored".into());
    }
    Ok(thread.wire_value_page(before, limit))
}

pub(super) fn activity(
    transaction: Option<&ReadTransaction<'_>>,
    thread: &Thread,
    params: &Value,
) -> Result<Value, String> {
    let (from, through) = activity_span_params(params, thread)?;
    let before = params.get("before_sequence").and_then(Value::as_u64);
    let limit = activity_page_limit(params);
    let rows = crate::thread::activity_rows_read(limit);
    if let Some(transaction) = transaction {
        let below = before.unwrap_or(u64::MAX).min(through.saturating_add(1));
        let items = transaction
            .thread_activity_range(&thread.agent.id, from.saturating_sub(1), below, rows)
            .map_err(conversation_error)?;
        return Ok(crate::thread::wire_value_activity_page(&items, limit));
    }
    if thread.resident_from_sequence() > from {
        return Err("this conversation's history is not stored".into());
    }
    Ok(crate::thread::wire_value_activity_page(
        &thread.activity_between(from, through, before, rows),
        limit,
    ))
}
