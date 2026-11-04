//! Explicit task-projection hooks, deduplicated at the sole writer boundary.
//!
//! The TEMP set belongs to the writer connection and rolls back with source
//! SQL. It is not a durable dirty marker or a second publication transaction.
use super::{journal, Entity, PublicationBatch, Stream};
use crate::store::StoreError;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(in crate::store) fn install(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("CREATE TEMP TABLE sync_task_touches(task_id TEXT PRIMARY KEY,force INTEGER NOT NULL) WITHOUT ROWID;")?;
    Ok(())
}

/// Capture the previous bounded projection before changing it. Canonical task
/// and activity writes force a revision even when the card did not change.
pub(in crate::store) fn touch(conn: &Connection, id: &str, force: bool) -> Result<(), StoreError> {
    // Startup backfill and explicitly owned external-connection repair run
    // before this StoreWriter exists. They cannot serve a sync scope yet.
    let installed: bool = conn
        .prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM sqlite_temp_schema WHERE name='sync_task_touches')",
        )?
        .query_row([], |r| r.get(0))?;
    if !installed {
        return Ok(());
    }
    let inserted = conn
        .prepare_cached("INSERT OR IGNORE INTO temp.sync_task_touches VALUES(?1,?2)")?
        .execute(params![id, force])?;
    if inserted != 0 {
        if let Some((project, value)) = projection(conn, id)? {
            journal::seed(
                conn,
                &Entity::new("task", id),
                &value,
                &routes(id, &project),
            )?;
        }
    } else if force {
        conn.prepare_cached("UPDATE temp.sync_task_touches SET force=1 WHERE task_id=?1")?
            .execute([id])?;
    }
    Ok(())
}

pub(in crate::store) fn collect(
    tx: &Transaction<'_>,
    batch: &mut PublicationBatch,
) -> Result<(), StoreError> {
    let touched = tx
        .prepare_cached("SELECT task_id,force FROM temp.sync_task_touches ORDER BY task_id")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    for (id, force) in touched {
        let entity = Entity::new("task", &id);
        match projection(tx, &id)? {
            Some((project, value)) => batch.upsert(entity.clone(), value, &routes(&id, &project)),
            None => batch.remove(entity.clone(), &[]),
        }
        if force {
            batch.force(&entity);
        }
        super::device::stage(
            tx,
            batch,
            Entity::new("inbox_task", &id),
            inbox_projection(tx, &id)?,
        )?;
    }
    tx.execute("DELETE FROM temp.sync_task_touches", [])?;
    Ok(())
}

pub(super) fn inbox_projection(conn: &Connection, id: &str) -> Result<Option<Value>, StoreError> {
    let raw: Option<(String, bool, bool, String)> = conn
        .prepare_cached(
            "SELECT s.record,json_extract(s.record,'$.watched')=1,q.pending_present,s.project_key
             FROM tracker_task_summaries s JOIN tracker_task_query_sources q ON q.task_id=s.task_id
             WHERE s.task_id=?1",
        )?
        .query_row([id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .optional()?;
    raw.filter(|(_, watched, _, _)| *watched)
        .map(|(raw, _, pending, project)| {
            let mut row: Value =
                serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                    path: std::path::PathBuf::from(format!("tracker_task_summaries/{id}")),
                    source,
                })?;
            let project_id: Option<String> = conn
                .query_row(
                    "SELECT project_id FROM device_project_ids WHERE path=?1",
                    [&project],
                    |row| row.get(0),
                )
                .optional()?;
            row["project_id"] = json!(project_id);
            if let Some(object) = row.as_object_mut() {
                object.remove("project_key");
                object.remove("project_path");
            }
            row.as_object_mut()
                .ok_or_else(|| {
                    StoreError::TaskSummaryInvalid("task summary is not an object".into())
                })?
                .insert("selection".into(), json!({"pending_present":pending}));
            Ok(row)
        })
        .transpose()
}

fn routes(id: &str, project: &str) -> [Stream; 2] {
    [Stream::task(id), Stream::project(project)]
}

fn projection(conn: &Connection, id: &str) -> Result<Option<(String, Value)>, StoreError> {
    let raw:Option<(String,String,Option<String>,String)>=conn.prepare_cached("SELECT s.project_key,s.record,s.assignment_target,
        json_object('status',q.status,'labels',q.labels_json,'assignee_kind',q.assignee_kind,'assignee_id',q.assignee_agent_id,
        'pending',q.pending_present,'held',q.pending_held,'creator_kind',q.creator_kind,'creator_id',q.creator_agent_id,
        'read',q.read_through,'watch',q.watch_started_after,'dismiss',q.dismissed_through,
        'updated',q.updated_source,'created',q.created_source,'done',q.input_done_source)
        FROM tracker_task_summaries s LEFT JOIN tracker_task_query_sources q ON q.task_id=s.task_id WHERE s.task_id=?1")?
        .query_row([id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    // Preserve opaque predicate bytes independently from lossy display fields.
    raw.map(|(project, row, target, source)| {
        let mut row: Value = serde_json::from_str(&row).map_err(|source| StoreError::Corrupt {
            path: std::path::PathBuf::from(format!("tracker_task_summaries/{id}")),
            source,
        })?;
        if let Some(identities) = row.get_mut("identities").and_then(Value::as_object_mut) {
            for identity in identities.values_mut() {
                if let Some(identity) = identity.as_object_mut() {
                    identity.remove("available");
                }
            }
        }
        Ok((
            project,
            json!([row, target, source, history_head(conn, id)?]),
        ))
    })
    .transpose()
}

/// Fingerprint the same bounded newest timeline window recovery can expose.
/// Each seek is index-backed and capped before records are loaded.
fn history_head(conn: &Connection, id: &str) -> Result<Value, StoreError> {
    let mut entries = Vec::with_capacity(102);
    for (table, at, kind) in [
        ("tracker_comments", "created_at", "comment"),
        ("tracker_events", "at", "event"),
    ] {
        let sql = format!(
            "SELECT {at},id,CASE WHEN length(CAST(record AS BLOB))<=65536 THEN record END \
             FROM {table} WHERE task_id=?1 ORDER BY {at} DESC,id DESC LIMIT 51"
        );
        let rows = conn
            .prepare(&sql)?
            .query_map([id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    kind,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        entries.extend(rows);
    }
    entries.sort_by(|a, b| (&b.0, &b.1).cmp(&(&a.0, &a.1)));
    let more = entries.len() > 50;
    entries.truncate(50);
    let mut digest = Sha256::new();
    for (at, id, kind, raw) in entries {
        for field in [at.as_bytes(), id.as_bytes(), kind.as_bytes()] {
            digest.update((field.len() as u64).to_be_bytes());
            digest.update(field);
        }
        match raw {
            Some(raw) => {
                digest.update([1]);
                digest.update((raw.len() as u64).to_be_bytes());
                digest.update(raw.as_bytes());
            }
            None => digest.update([0]),
        }
    }
    Ok(json!({"digest": digest.finalize().to_vec(), "more": more}))
}
