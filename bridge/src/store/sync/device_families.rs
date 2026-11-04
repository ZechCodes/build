//! Durable lifecycle and retained activity summaries. No transcript scans.
use super::{Entity, PublicationBatch};
use crate::store::StoreError;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};

pub(super) fn install(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS device_archive_projects(id TEXT PRIMARY KEY,project_path TEXT NOT NULL) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS device_archives_project ON device_archive_projects(project_path,id);
        CREATE TABLE IF NOT EXISTS device_retained_sessions(kind TEXT NOT NULL,key TEXT NOT NULL,last_activity_ms INTEGER NOT NULL,PRIMARY KEY(kind,key)) WITHOUT ROWID;
        CREATE TEMP TABLE device_family_touches(kind TEXT NOT NULL,key TEXT NOT NULL,PRIMARY KEY(kind,key)) WITHOUT ROWID;")?;
    Ok(())
}
fn installed(conn: &Connection) -> Result<bool, StoreError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_temp_schema WHERE name='device_family_touches')",
        [],
        |row| row.get(0),
    )?)
}
pub(in crate::store) fn touch_archive(conn: &Connection, id: &str) -> Result<(), StoreError> {
    if installed(conn)? {
        conn.prepare_cached(
            "INSERT OR IGNORE INTO temp.device_family_touches VALUES('archive',?1)",
        )?
        .execute([id])?;
    }
    Ok(())
}
pub(super) fn touch_retained(conn: &Connection, kind: &str, key: &str) -> Result<(), StoreError> {
    if installed(conn)? {
        conn.prepare_cached("INSERT OR IGNORE INTO temp.device_family_touches VALUES(?1,?2)")?
            .execute(params![kind, key])?;
    }
    Ok(())
}
pub(super) fn collect(
    tx: &Transaction<'_>,
    batch: &mut PublicationBatch,
) -> Result<(), StoreError> {
    let rows = tx
        .prepare_cached("SELECT kind,key FROM temp.device_family_touches ORDER BY kind,key")?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (kind, key) in rows {
        if kind == "archive" {
            stage_archive(tx, batch, &key)?;
        } else {
            stage_retained(tx, batch, &kind, &key)?;
        }
    }
    tx.execute("DELETE FROM temp.device_family_touches", [])?;
    Ok(())
}
pub(super) fn stage_archive(
    tx: &Transaction<'_>,
    batch: &mut PublicationBatch,
    id: &str,
) -> Result<(), StoreError> {
    let raw: Option<String> = tx
        .query_row(
            "SELECT record FROM archived_worktrees WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    let row=raw.map(|raw|->Result<Value,StoreError>{
        let record:crate::store::PersistedArchivedWorktree=serde_json::from_str(&raw).map_err(|source|StoreError::Corrupt{path:format!("archived_worktrees/{id}").into(),source})?;
        tx.execute("INSERT INTO device_archive_projects VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET project_path=?2",params![id,record.project_path])?;
        let project:Option<String>=tx.query_row("SELECT project_id FROM device_project_ids WHERE path=?1",[&record.project_path],|row|row.get(0)).optional()?;
        Ok(json!({"id":id,"project_id":project,"name":record.worktree_name,"path":record.worktree_path,"branch":record.branch,"head_sha":record.head_sha,"action":record.action,"status":record.status,"archived_at":record.archived_at}))
    }).transpose()?;
    super::stage_device(tx, batch, Entity::new("archived_worktree", id), row)
}
pub(super) fn stage_lifecycle(
    tx: &Transaction<'_>,
    batch: &mut PublicationBatch,
    id: &str,
    value: &Value,
) -> Result<(), StoreError> {
    let row = json!({"workspace_id":id,"pruned_bytes":value["pruned_bytes"],"pruned_at_ms":value["pruned_at_ms"],"noticed_at_ms":value["noticed_at_ms"],"size_bytes":value["size_bytes"],"size_measured_at_ms":value["size_measured_at_ms"]});
    super::stage_device(tx, batch, Entity::new("workspace_lifecycle", id), Some(row))
}
pub(in crate::store) fn lifecycle(
    tx: &Transaction<'_>,
    batch: &mut PublicationBatch,
    records: &std::collections::HashMap<String, crate::reclaim::LifecycleRecord>,
) -> Result<(), StoreError> {
    let previous = tx
        .prepare("SELECT key FROM sync_projection WHERE kind='workspace_lifecycle' AND deleted=0")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for id in previous {
        if !records.contains_key(&id) {
            super::stage_device(tx, batch, Entity::new("workspace_lifecycle", &id), None)?;
        }
    }
    for (id, record) in records {
        stage_lifecycle(
            tx,
            batch,
            id,
            &serde_json::to_value(record).expect("lifecycle record"),
        )?;
    }
    Ok(())
}
pub(super) fn retain_time(
    conn: &Connection,
    kind: &str,
    key: &str,
    at: i64,
) -> Result<(), StoreError> {
    conn.prepare_cached("INSERT INTO device_retained_sessions VALUES(?1,?2,?3) ON CONFLICT(kind,key) DO UPDATE SET last_activity_ms=MAX(last_activity_ms,?3)")?.execute(params![kind,key,at])?;
    touch_retained(conn, kind, key)
}
fn last_session(conn: &Connection, id: &str) -> Result<Option<i64>, StoreError> {
    Ok(conn
        .prepare_cached(
            "SELECT last_activity_ms FROM device_conversation_sessions WHERE conversation_id=?1",
        )?
        .query_row([id], |row| row.get(0))
        .optional()?
        .flatten())
}
pub(in crate::store) fn retain_agent(
    conn: &Connection,
    owner: &str,
    agent: &str,
) -> Result<(), StoreError> {
    if installed(conn)? {
        if let Some(at) = last_session(conn, agent)? {
            retain_time(conn, "owner", owner, at)?;
        }
    }
    Ok(())
}
pub(in crate::store) fn retain_owner(
    conn: &Connection,
    owner: &str,
    project: &str,
) -> Result<(), StoreError> {
    if !installed(conn)? {
        return Ok(());
    }
    let mut last: Option<i64> = conn
        .prepare_cached(
            "SELECT last_activity_ms FROM device_retained_sessions WHERE kind='owner' AND key=?1",
        )?
        .query_row([owner], |row| row.get(0))
        .optional()?;
    let agents = conn
        .prepare_cached("SELECT id FROM agents WHERE owner_id=?1")?
        .query_map([owner], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for agent in agents {
        last = last.max(last_session(conn, &agent)?);
    }
    if let Some(at) = last {
        retain_time(conn, "project", project, at)?;
    }
    Ok(())
}
pub(in crate::store) fn forget_owner(conn: &Connection, owner: &str) -> Result<(), StoreError> {
    if installed(conn)? {
        conn.prepare_cached("DELETE FROM device_retained_sessions WHERE kind='owner' AND key=?1")?
            .execute([owner])?;
        touch_retained(conn, "owner", owner)?;
        super::device_facts::forget_owner(conn, owner)?;
    }
    Ok(())
}
pub(in crate::store) fn clear_project(conn: &Connection, id: &str) -> Result<(), StoreError> {
    if installed(conn)? {
        conn.prepare_cached(
            "DELETE FROM device_retained_sessions WHERE kind='project' AND key=?1",
        )?
        .execute([id])?;
        touch_retained(conn, "project", id)?;
    }
    Ok(())
}
fn stage_retained(
    tx: &Transaction<'_>,
    batch: &mut PublicationBatch,
    kind: &str,
    key: &str,
) -> Result<(), StoreError> {
    let at: Option<i64> = tx
        .query_row(
            "SELECT last_activity_ms FROM device_retained_sessions WHERE kind=?1 AND key=?2",
            params![kind, key],
            |row| row.get(0),
        )
        .optional()?;
    let row=at.map(|at|->Result<Value,StoreError>{
        if kind=="project" {return Ok(json!({"project_id":key,"retained_last_activity_ms":at}));}
        let links:Option<(Option<String>,Option<String>)>=tx.query_row("SELECT p.project_id,w.workspace_id FROM device_owner_projects o LEFT JOIN device_project_ids p ON p.path=o.project_path LEFT JOIN device_workspace_roots w ON w.root=o.workspace_path WHERE o.owner_id=?1",[key],|row|Ok((row.get(0)?,row.get(1)?))).optional()?;
        let (project,workspace)=links.unwrap_or((None,None));
        Ok(json!({"owner_id":key,"project_id":project,"workspace_id":workspace,"retained_last_activity_ms":at}))
    }).transpose()?;
    super::stage_device(
        tx,
        batch,
        Entity::new(
            if kind == "project" {
                "project_session"
            } else {
                "owner_session"
            },
            key,
        ),
        row,
    )
}
