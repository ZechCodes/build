//! Explicit touches for the SQL-owned device rows implemented in this slice.
use super::{stage_device, Entity, PublicationBatch};
use crate::attention::Attention;
use crate::store::StoreError;
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};

pub(in crate::store) fn install(conn: &Connection) -> Result<(), StoreError> {
    super::device_facts::install(conn)?;
    super::device_families::install(conn)?;
    super::device_thread::install(conn)?;
    conn.execute_batch(
        "CREATE TEMP TABLE sync_device_agent_touches(
            agent_id TEXT PRIMARY KEY
         ) WITHOUT ROWID;",
    )?;
    Ok(())
}

pub(in crate::store) fn touch_agent(conn: &Connection, id: &str) -> Result<(), StoreError> {
    let installed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_temp_schema WHERE name='sync_device_agent_touches')",
        [],
        |row| row.get(0),
    )?;
    if !installed {
        return Ok(());
    }
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
    super::device_thread::flush(tx)?;
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
    super::device_families::collect(tx, batch)?;
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
    let stored: Option<(String, Option<String>)> = conn
        .prepare_cached("SELECT a.owner_id,f.record FROM agents a LEFT JOIN device_agent_facts f ON f.agent_id=a.id WHERE a.id=?1")?
        .query_row([id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    let Some((owner, raw)) = stored else {
        return Ok(None);
    };
    let raw = raw.ok_or_else(|| super::invalid("device agent facts are incomplete"))?;
    let agent: super::device_facts::AgentFacts =
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
    let current = super::device_thread::generation(conn, &conversation)?;
    let last_row:Option<(i64,Option<String>)>=conn.prepare_cached("SELECT sequence,summary FROM device_summary_items WHERE conversation_id=?1 AND generation=?2 ORDER BY sequence DESC LIMIT 1")?.query_row(rusqlite::params![conversation,current],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
    let last = last_row.as_ref().map_or(0, |row| row.0);
    let last_summary = last_row.and_then(|row| row.1);
    let own:i64=conn.prepare_cached("SELECT sequence FROM device_summary_items WHERE conversation_id=?1 AND generation=?2 AND own=1 ORDER BY sequence DESC LIMIT 1")?.query_row(rusqlite::params![conversation,current],|row|row.get(0)).optional()?.unwrap_or(0);
    let floor = read.max(agent.watch_started_after);
    let unread = if agent.watched && !attention.muted {
        super::device_thread::unread(conn, &conversation, floor)?
    } else {
        0
    };
    let reason: Option<String> = if unread > 0 {
        conn.prepare_cached("SELECT reason FROM device_summary_items WHERE conversation_id=?1 AND generation=?2 AND attention=1 AND sequence>?3 ORDER BY sequence DESC LIMIT 1")?.query_row(rusqlite::params![conversation,current,i64::try_from(floor).unwrap_or(i64::MAX)],|row|row.get(0)).optional()?
    } else {
        None
    };
    let session=conn.prepare_cached("SELECT session_started_ms,last_activity_ms FROM device_conversation_sessions WHERE conversation_id=?1")?.query_row([&conversation],|row|Ok((row.get::<_,Option<i64>>(0)?,row.get::<_,Option<i64>>(1)?))).optional()?.unwrap_or((None,None));
    let thread:Option<(String,String)>=conn.prepare_cached("SELECT json_extract(record,'$.thread_id'),json_extract(record,'$.thread_generation_revision') FROM device_agent_facts WHERE agent_id=?1")?.query_row([&conversation],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
    let pending = pending(conn, &conversation)?;
    let project_path = owner_project_path(conn, &owner)?;
    let project_id = project_path
        .as_deref()
        .map(|path| {
            conn.query_row(
                "SELECT project_id FROM device_project_ids WHERE path=?1",
                [path],
                |row| row.get::<_, String>(0),
            )
            .optional()
        })
        .transpose()?
        .flatten();
    let workspace_id:Option<String>=conn.prepare_cached("SELECT w.workspace_id FROM device_owner_projects o JOIN device_workspace_roots w ON w.root=o.workspace_path WHERE o.owner_id=?1")?.query_row([&owner],|row|row.get(0)).optional()?;
    let provider = agent.choice.provider.wire_id();
    let agent_row = json!({
        "owner_id":owner,
        "project_id":project_id,
        "workspace_id":workspace_id,
        "id":agent.id,
        "conversation_id":conversation,
        "thread_id":thread.as_ref().map(|thread|&thread.0),
        "thread_generation_revision":thread.as_ref().map(|thread|&thread.1),
        "ordinal":agent.ordinal,
        "provider":provider,
        "model":agent.choice.model,
        "effort":agent.choice.effort,
        "watched":agent.watched,
        "choice_revision":agent.choice_revision,
        "created_at":agent.created_at,
        "title":agent.topic.as_ref().or(agent.name.as_ref()),
        "topic":agent.topic,
        "name":agent.name,
        "created_by":agent.created_by,
        "read_through_sequence":read.to_string(),
        "watch_started_after":agent.watch_started_after.to_string(),
        "last_sequence":last.to_string(),
        "unread_count":unread,
        "unread_reason":reason,
    });
    let conversation_row = agent.watched.then(|| {
        json!({
            "owner_id":owner,
            "agent_id":agent.id,
            "thread_id":thread.as_ref().map(|thread|&thread.0),
            "thread_generation_revision":thread.as_ref().map(|thread|&thread.1),
            "project_id":project_id,
            "workspace_id":workspace_id,
            "title":agent.topic.or(agent.name),
            "watched":true,
            "muted":attention.muted,
            "dismissed":attention.is_dismissed_for(&agent.id, agent.ordinal == 1, own as u64),
            "unread_count":unread,
            "read_through_sequence":read.to_string(),
            "last_sequence":last.to_string(),
            "watch_started_after":agent.watch_started_after.to_string(),
            "unread_reason":reason,
            "last_activity_ms":session.1,
            "session_started_ms":session.0,
            "last_message_summary":last_summary,
            "pending":pending,
            "last_interaction_at":attention.last_interaction_at,
            "resume_at":attention.resume_at,
        })
    });
    Ok(Some((agent_row, conversation_row)))
}

pub(super) fn owner_project_path(
    conn: &Connection,
    owner: &str,
) -> Result<Option<String>, StoreError> {
    Ok(conn
        .prepare_cached("SELECT project_path FROM device_owner_projects WHERE owner_id=?1")?
        .query_row([owner], |row| row.get(0))
        .optional()?)
}

pub(in crate::store) fn touch_conversation(conn: &Connection, id: &str) -> Result<(), StoreError> {
    let installed: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_temp_schema WHERE name='sync_device_agent_touches')",
        [],
        |row| row.get(0),
    )?;
    if !installed {
        return Ok(());
    }
    touch_agent(conn, id)?;
    conn.execute("INSERT OR IGNORE INTO temp.sync_device_agent_touches
        SELECT id FROM agents WHERE CASE WHEN json_valid(record) THEN json_extract(record,'$.conversation_id') END=?1",[id])?;
    Ok(())
}

pub(in crate::store) fn touch_project(conn: &Connection, path: &str) -> Result<(), StoreError> {
    conn.execute("INSERT OR IGNORE INTO temp.sync_device_agent_touches SELECT a.id FROM agents a JOIN device_owner_projects o ON o.owner_id=a.owner_id WHERE o.project_path=?1",[path])?;
    let owners = conn
        .prepare("SELECT owner_id FROM device_owner_projects WHERE project_path=?1")?
        .query_map([path], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for owner in owners {
        super::device_families::touch_retained(conn, "owner", &owner)?;
    }
    let archives = conn
        .prepare("SELECT id FROM device_archive_projects WHERE project_path=?1")?
        .query_map([path], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for id in archives {
        super::device_families::touch_archive(conn, &id)?;
    }
    let ids = conn
        .prepare("SELECT task_id FROM tracker_task_summaries WHERE project_key=?1")?
        .query_map([path], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for id in ids {
        super::touch_task(conn, &id, false)?;
    }
    Ok(())
}

fn pending(conn: &Connection, id: &str) -> Result<bool, StoreError> {
    let legacy=format!("SELECT EXISTS(SELECT 1 FROM thread_items INDEXED BY thread_items_pending_delivery WHERE agent_id=?1 AND message=1 AND {})",crate::store::schema::RETENTION_PENDING_ITEM_SQL);
    let pending: bool = conn.query_row(&legacy, [id], |row| row.get(0))?;
    if pending {
        return Ok(true);
    }
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM operations WHERE conversation_id=?1 AND status IN ('queued','claimed','uncertain'))",[id],|row|row.get(0))?)
}
