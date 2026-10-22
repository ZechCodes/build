use super::{CommittedThreadItems, Store, StoreError};
use crate::agent::Agent;
use crate::thread::{MessageDeliveryStatus, MessageRole, ThreadItem, ThreadMessage};
use rusqlite::{params, OptionalExtension, Transaction};
use std::collections::BTreeSet;
use std::path::PathBuf;

impl Store {
    pub(crate) fn set_delivery_status(
        &self,
        conversation_id: &str,
        operation_id: Option<&str>,
        start_sequence: u64,
        end_sequence: u64,
        status: MessageDeliveryStatus,
    ) -> Result<CommittedThreadItems, StoreError> {
        self.in_transaction(|tx| {
            let mut agent = read_agent(tx, conversation_id)?;
            agent.thread.items = read_range(tx, conversation_id, start_sequence, end_sequence)?;
            match operation_id {
                Some(operation_id) => agent.thread.set_operation_delivery_status(
                    operation_id,
                    start_sequence,
                    end_sequence,
                    status,
                ),
                None => {
                    agent
                        .thread
                        .set_legacy_delivery_status(start_sequence, end_sequence, status)
                }
            }
            write_status(tx, conversation_id, agent)
        })
    }

    /// Frozen reviewer input can be sparse. Select each exact message inside
    /// one transaction, retaining the counter and status rules of range edits.
    pub(crate) fn set_frozen_legacy_delivery_status(
        &self,
        conversation_id: &str,
        thread_id: &str,
        generation_revision: u64,
        messages: &[ThreadMessage],
        status: MessageDeliveryStatus,
    ) -> Result<Option<CommittedThreadItems>, StoreError> {
        self.in_transaction(|tx| {
            let mut agent = read_agent(tx, conversation_id)?;
            if agent.thread.id != thread_id
                || agent.thread.generation_revision != generation_revision
            {
                return Ok(None);
            }
            agent.thread.items = read_frozen(tx, conversation_id, messages)?;
            if agent.thread.items.is_empty() {
                return Ok(None);
            }
            // Only the validated point selections are resident here. One pass
            // stamps that exact set in its captured order, including gaps.
            agent.thread.set_legacy_delivery_status(0, u64::MAX, status);
            write_status(tx, conversation_id, agent).map(Some)
        })
    }
}

fn read_agent(tx: &Transaction<'_>, conversation: &str) -> Result<Agent, StoreError> {
    let raw: String = tx.query_row(
        "SELECT record FROM agents WHERE id = ?1",
        [conversation],
        |row| row.get(0),
    )?;
    serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
        path: PathBuf::from(format!("agents/{conversation}")),
        source,
    })
}

fn read_range(
    tx: &Transaction<'_>,
    conversation: &str,
    first: u64,
    last: u64,
) -> Result<Vec<ThreadItem>, StoreError> {
    let mut statement = tx.prepare("SELECT item FROM thread_items WHERE agent_id = ?1 AND sequence BETWEEN ?2 AND ?3 ORDER BY sequence")?;
    let rows = statement.query_map(
        params![conversation, sql_sequence(first), sql_sequence(last)],
        |row| row.get::<_, String>(0),
    )?;
    rows.map(|row| decode_item(conversation, &row?)).collect()
}

fn read_frozen(
    tx: &Transaction<'_>,
    conversation: &str,
    messages: &[ThreadMessage],
) -> Result<Vec<ThreadItem>, StoreError> {
    let mut statement =
        tx.prepare("SELECT item FROM thread_items WHERE agent_id = ?1 AND sequence = ?2")?;
    let mut selected = Vec::new();
    let mut sequences = BTreeSet::new();
    for expected in messages {
        if sequences.contains(&expected.sequence) {
            continue;
        }
        let raw = statement
            .query_row(
                params![conversation, sql_sequence(expected.sequence)],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(raw) = raw else {
            continue;
        };
        let item = decode_item(conversation, &raw)?;
        if matches!(&item, ThreadItem::Message(message) if message.id == expected.id
            && message.sequence == expected.sequence && message.role == MessageRole::User
            && message.operation_id.is_none())
        {
            sequences.insert(expected.sequence);
            selected.push(item);
        }
    }
    Ok(selected)
}

fn decode_item(conversation: &str, raw: &str) -> Result<ThreadItem, StoreError> {
    serde_json::from_str(raw).map_err(|source| StoreError::Corrupt {
        path: PathBuf::from(format!("thread_items/{conversation}")),
        source,
    })
}

fn write_status(
    tx: &Transaction<'_>,
    conversation: &str,
    mut agent: Agent,
) -> Result<CommittedThreadItems, StoreError> {
    let sequence = agent.thread.last_sequence();
    for item in &agent.thread.items {
        tx.execute("UPDATE thread_items SET item = ?3, updated_sequence = ?4 WHERE agent_id = ?1 AND sequence = ?2",
            params![conversation, sql_sequence(item.sequence()), serde_json::to_string(item).expect("a thread item always serializes"), sql_sequence(item.latest_sequence())])?;
    }
    let items = std::mem::take(&mut agent.thread.items);
    tx.execute(
        "UPDATE agents SET record = ?2 WHERE id = ?1",
        params![
            conversation,
            serde_json::to_string(&agent).expect("an agent always serializes")
        ],
    )?;
    Ok(CommittedThreadItems {
        conversation_id: conversation.into(),
        thread_id: agent.thread.id,
        generation_revision: agent.thread.generation_revision,
        sequence,
        items,
    })
}

fn sql_sequence(sequence: u64) -> i64 {
    i64::try_from(sequence).unwrap_or(i64::MAX)
}
