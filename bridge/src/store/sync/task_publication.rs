//! Explicit task-projection hooks, deduplicated at the sole writer boundary.
//!
//! The TEMP set belongs to the writer connection and rolls back with source
//! SQL. It is not a durable dirty marker or a second publication transaction.
use super::{journal, Entity, PublicationBatch, Stream};
use crate::store::StoreError;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};

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
    }
    tx.execute("DELETE FROM temp.sync_task_touches", [])?;
    Ok(())
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
    Ok(raw.map(|(project, row, target, source)| (project, json!([row, target, source]))))
}
