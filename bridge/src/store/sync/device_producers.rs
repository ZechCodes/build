//! Explicit touches for the SQL-owned device rows implemented in this slice.
use super::{stage_device, Entity, PublicationBatch};
use crate::attention::Attention;
use crate::store::StoreError;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};

pub(in crate::store) fn install(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TEMP TABLE sync_device_agent_touches(
            agent_id TEXT PRIMARY KEY
         ) WITHOUT ROWID;",
    )?;
    Ok(())
}

pub(in crate::store) fn touch_agent(conn: &Connection, id: &str) -> Result<(), StoreError> {
    conn.prepare_cached("INSERT OR IGNORE INTO temp.sync_device_agent_touches VALUES(?1)")?
        .execute([id])?;
    Ok(())
}

pub(in crate::store) fn touch_owner(conn: &Connection, owner: &str) -> Result<(), StoreError> {
    conn.prepare_cached(
        "INSERT OR IGNORE INTO temp.sync_device_agent_touches
         SELECT id FROM agents WHERE owner_id=?1",
    )?
    .execute([owner])?;
    Ok(())
}

pub(in crate::store) fn collect(
    tx: &Transaction<'_>,
    batch: &mut PublicationBatch,
) -> Result<(), StoreError> {
    let ids = tx
        .prepare_cached("SELECT agent_id FROM temp.sync_device_agent_touches ORDER BY agent_id")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for id in ids {
        let projections = agent_projections(tx, &id)?;
        stage_device(
            tx,
            batch,
            Entity::new("agent", &id),
            projections.as_ref().map(|(agent, _)| agent.clone()),
        )?;
        stage_device(
            tx,
            batch,
            Entity::new("inbox_conversation", &id),
            projections.and_then(|(_, conversation)| conversation),
        )?;
        stage_device(
            tx,
            batch,
            Entity::new("identity", &id),
            identity_projection(tx, &id)?,
        )?;
    }
    tx.execute("DELETE FROM temp.sync_device_agent_touches", [])?;
    Ok(())
}

fn identity_projection(conn: &Connection, id: &str) -> Result<Option<Value>, StoreError> {
    let raw: Option<String> = conn
        .prepare_cached("SELECT record FROM tracker_task_summary_identities WHERE agent_id=?1")?
        .query_row([id], |row| row.get(0))
        .optional()?;
    raw.map(|raw| {
        let mut value: Value =
            serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                path: format!("tracker_task_summary_identities/{id}").into(),
                source,
            })?;
        if let Some(identity) = value.as_object_mut() {
            identity.remove("available");
        }
        Ok(value)
    })
    .transpose()
}

fn agent_projections(
    conn: &Connection,
    id: &str,
) -> Result<Option<(Value, Option<Value>)>, StoreError> {
    let stored: Option<(String, String)> = conn
        .prepare_cached("SELECT owner_id,record FROM agents WHERE id=?1")?
        .query_row([id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((owner, raw)) = stored else {
        return Ok(None);
    };
    let agent: crate::agent::Agent =
        serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
            path: format!("agents/{id}").into(),
            source,
        })?;
    let attention: Attention = conn
        .prepare_cached("SELECT record FROM attention WHERE entity_id=?1")?
        .query_row([&owner], |row| row.get::<_, String>(0))
        .optional()?
        .map(|raw| {
            serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                path: format!("attention/{owner}").into(),
                source,
            })
        })
        .transpose()?
        .unwrap_or_default();
    let conversation = agent.conversation_id().to_string();
    let read = attention.cursor_for(&agent.id);
    let (last, unread): (i64, i64) = conn.query_row(
        "SELECT COALESCE(MAX(sequence),0),
            COALESCE(SUM(attention=1 AND sequence>?2),0)
         FROM thread_items WHERE agent_id=?1",
        params![conversation, read as i64],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let project_path = owner_project_path(conn, &owner)?;
    let workspace_id: Option<String> = conn
        .prepare_cached("SELECT id FROM implementations WHERE id=?1")?
        .query_row([&owner], |row| row.get(0))
        .optional()?;
    let provider = agent.choice.provider.wire_id();
    let agent_row = json!({
        "owner_id":owner,
        "project_id":project_path,
        "workspace_id":workspace_id,
        "id":agent.id,
        "conversation_id":conversation,
        "ordinal":agent.ordinal,
        "provider":provider,
        "model":agent.choice.model,
        "effort":agent.choice.effort,
        "watched":agent.watched,
        "choice_revision":agent.choice_revision,
        "created_at":agent.created_at,
        "title":Value::Null,
        "topic":agent.topic,
        "name":agent.name,
        "created_by":agent.created_by,
        "read_through_sequence":read.to_string(),
        "watch_started_after":agent.watch_started_after.to_string(),
        "last_sequence":last.to_string(),
        "unread_count":unread,
        "unread_reason":Value::Null,
    });
    let conversation_row = agent.watched.then(|| {
        json!({
            "owner_id":owner,
            "agent_id":agent.id,
            "project_id":project_path,
            "workspace_id":workspace_id,
            "title":agent.topic.or(agent.name),
            "watched":true,
            "muted":attention.muted,
            "dismissed":attention.is_dismissed_for(&agent.id, agent.ordinal == 1, last as u64),
            "unread_count":unread,
            "read_through_sequence":read.to_string(),
            "last_sequence":last.to_string(),
            "watch_started_after":agent.watch_started_after.to_string(),
            "unread_reason":Value::Null,
            "last_activity_ms":Value::Null,
            "session_started_ms":Value::Null,
            "last_message_summary":Value::Null,
            "pending":false,
        })
    });
    Ok(Some((agent_row, conversation_row)))
}

fn owner_project_path(conn: &Connection, owner: &str) -> Result<Option<String>, StoreError> {
    let raw: Option<String> = conn
        .prepare_cached(
            "SELECT record FROM implementations WHERE id=?1
             UNION ALL SELECT record FROM tasks WHERE id=?1 LIMIT 1",
        )?
        .query_row([owner], |row| row.get(0))
        .optional()?;
    raw.map(|raw| {
        let value: Value = serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
            path: format!("owners/{owner}").into(),
            source,
        })?;
        Ok(value
            .get("project_path")
            .and_then(Value::as_str)
            .map(str::to_string))
    })
    .transpose()
    .map(Option::flatten)
}
