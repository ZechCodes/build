//! Synchronous producer hooks. Call these on the canonical mutation transaction.
//! Canonical SQL outside supported producers deliberately has no automatic hook.

use super::{query_projection, query_sources, sql, StoreError};
use crate::tracker::Task;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::OnceLock;

struct Statements {
    task: String,
    context: String,
    activity: String,
    append: String,
    card: String,
    metadata: String,
    dependencies: String,
    comment: String,
    event: String,
    identity: String,
    owner: String,
}

fn statements() -> &'static Statements {
    static SQL: OnceLock<Statements> = OnceLock::new();
    SQL.get_or_init(|| Statements {
        task: sql::refresh_task(),
        context: sql::refresh_context("?1"),
        activity: sql::refresh_activity("?1"),
        append: sql::append_update(),
        card: sql::patch_card("?1"),
        metadata: sql::refresh_metadata("?1"),
        dependencies: sql::refresh_dependencies("?1"),
        comment: sql::insert_facts("tracker_comments", "id=?1"),
        event: sql::insert_facts("tracker_events", "id=?1"),
        identity: sql::refresh_identities("a.id=?1"),
        owner: sql::refresh_identities("a.owner_id=?1"),
    })
}

fn ready(conn: &Connection) -> Result<bool, StoreError> {
    Ok(conn.prepare_cached("SELECT EXISTS(SELECT 1 FROM meta WHERE key='task_summary_v1_ready') AND NOT EXISTS(SELECT 1 FROM meta WHERE key='task_summary_rebuild')")?.query_row([],|row|row.get(0))?)
}

fn keyed(conn: &Connection, statement: &str, id: &str) -> Result<usize, StoreError> {
    Ok(conn.prepare_cached(statement)?.execute([id])?)
}

pub(crate) fn task_refresh(conn: &Connection, id: &str) -> Result<(), StoreError> {
    if ready(conn)? {
        task_refresh_unchecked(conn, id)?;
    }
    Ok(())
}

pub(super) fn task_refresh_unchecked(conn: &Connection, id: &str) -> Result<(), StoreError> {
    let record: Option<String> = conn
        .prepare_cached("SELECT record FROM tracker_tasks WHERE id=?1")?
        .query_row([id], |row| row.get(0))
        .optional()?;
    if let Some(record) = record {
        let task: Task = super::super::tracker::decode(&record, "tracker_tasks", id)?;
        task_written_unchecked(conn, &task)?;
    } else {
        task_remove(conn, id)?;
    }
    Ok(())
}

pub(crate) fn task_written(conn: &Connection, task: &Task) -> Result<(), StoreError> {
    if ready(conn)? {
        task_written_unchecked(conn, task)?;
    }
    Ok(())
}

pub(super) fn task_written_unchecked(conn: &Connection, task: &Task) -> Result<(), StoreError> {
    query_sources::task_written(conn, task)?;
    let number = i64::try_from(task.number).unwrap_or(i64::MAX);
    let record = compact_task(task).to_string();
    conn.prepare_cached(&statements().task)?.execute(params![
        task.id,
        task.project_path,
        number,
        task.state.as_str(),
        task.status,
        task.created_at,
        task.updated_at,
        record
    ])?;
    if keyed(conn, &statements().context, &task.id)? != 0 {
        keyed(conn, &statements().activity, &task.id)?;
    }
    keyed(conn, &statements().card, &task.id)?;
    keyed(conn, &statements().metadata, &task.id)?;
    keyed(conn, &statements().dependencies, &task.id)?;
    sql::refresh_choices(conn, &task.id)?;
    query_sources::metadata_refresh(conn, &task.id)?;
    query_projection::refresh(conn, &task.id)?;
    query_projection::refresh_relations(conn, task)?;
    query_projection::refresh_review_mode(conn, &task.id)
}

fn compact_task(task: &Task) -> Value {
    let identities: BTreeMap<_, _> = [
        task.created_by.agent_id(),
        task.assignee.as_ref().and_then(|a| a.agent_id()),
    ]
    .into_iter()
    .flatten()
    .filter_map(|id| task.identities.get(id).map(|identity| (id, identity)))
    .collect();
    json!({"title":task.title,"labels":task.labels,"priority":task.priority,
        "assignee":task.assignee,"created_by":task.created_by,"trackers":task.trackers,
        "links":{"parent_task_id":task.links.parent_task_id,"workspace_ids":task.links.workspace_ids},
        "attachment_count":task.attachments.len(),"identities":identities,
        "watched":task.watched,"read_through":task.read_through,"watch_started_after":task.watch_started_after,
        "dismissed_through":task.dismissed_through,"activity_session":task.activity_session,
        "closed_at":task.closed_at,"done_at":task.done_at})
}

pub(crate) fn task_remove(conn: &Connection, id: &str) -> Result<(), StoreError> {
    if !ready(conn)? {
        return Ok(());
    }
    task_dependents_refresh(conn, id)?;
    query_projection::remove(conn, id)?;
    keyed(
        conn,
        "DELETE FROM tracker_task_summaries WHERE task_id=?1",
        id,
    )?;
    keyed(
        conn,
        "DELETE FROM tracker_task_summary_context WHERE task_id=?1",
        id,
    )?;
    keyed(
        conn,
        "DELETE FROM tracker_task_summary_facts WHERE task_id=?1",
        id,
    )?;
    sql::refresh_choices(conn, id)
}

pub(crate) fn task_dependents_refresh(conn: &Connection, id: &str) -> Result<(), StoreError> {
    if !ready(conn)? {
        return Ok(());
    }
    let ids = conn
        .prepare_cached(
            "SELECT task_id FROM tracker_task_dependencies WHERE waiting_on_task_id=?1",
        )?
        .query_map([id], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for dependent in ids {
        keyed(conn, &statements().dependencies, &dependent)?;
        query_projection::refresh(conn, &dependent)?;
    }
    Ok(())
}

pub(crate) fn metadata_refresh(conn: &Connection, id: &str) -> Result<(), StoreError> {
    if ready(conn)? {
        keyed(conn, &statements().metadata, id)?;
        query_sources::metadata_refresh(conn, id)?;
        query_projection::refresh(conn, id)?;
        query_projection::refresh_review_mode(conn, id)?;
    }
    Ok(())
}

pub(crate) fn dependency_refresh(conn: &Connection, id: &str) -> Result<(), StoreError> {
    if ready(conn)? {
        keyed(conn, &statements().dependencies, id)?;
        query_projection::refresh(conn, id)?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(crate) enum HistoryKind {
    Comment,
    Event,
}

pub(crate) fn history_append(
    conn: &Connection,
    kind: HistoryKind,
    task_id: &str,
    id: &str,
) -> Result<(), StoreError> {
    if !ready(conn)? {
        return Ok(());
    }
    let replace: bool = conn
        .prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM tracker_task_summary_facts WHERE task_id=?1 AND id=?2)",
        )?
        .query_row(params![task_id, id], |row| row.get(0))?;
    let statement = match kind {
        HistoryKind::Comment => &statements().comment,
        HistoryKind::Event => &statements().event,
    };
    keyed(conn, statement, id)?;
    let incremental = if replace {
        0
    } else {
        conn.prepare_cached(&statements().append)?
            .execute(params![task_id, id])?
    };
    if incremental == 0 {
        keyed(conn, &statements().activity, task_id)?;
    }
    keyed(conn, &statements().card, task_id)?;
    query_projection::refresh(conn, task_id)?;
    Ok(())
}

pub(crate) fn history_repair(conn: &Connection, task_id: &str) -> Result<(), StoreError> {
    if !ready(conn)? {
        return Ok(());
    }
    keyed(
        conn,
        "DELETE FROM tracker_task_summary_facts WHERE task_id=?1",
        task_id,
    )?;
    let exists: bool = conn
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM tracker_tasks WHERE id=?1)")?
        .query_row([task_id], |row| row.get(0))?;
    if !exists {
        return Ok(());
    }
    for table in ["tracker_comments", "tracker_events"] {
        keyed(conn, &sql::insert_facts(table, "task_id=?1"), task_id)?;
    }
    keyed(conn, &statements().activity, task_id)?;
    keyed(conn, &statements().card, task_id)?;
    query_projection::refresh(conn, task_id)?;
    Ok(())
}

pub(crate) fn identity_refresh(conn: &Connection, id: &str) -> Result<(), StoreError> {
    if ready(conn)? {
        keyed(conn, &statements().identity, id)?;
    }
    Ok(())
}

pub(crate) fn identity_remove(conn: &Connection, id: &str) -> Result<(), StoreError> {
    if ready(conn)? {
        keyed(
            conn,
            "DELETE FROM tracker_task_summary_identities WHERE agent_id=?1",
            id,
        )?;
    }
    Ok(())
}

pub(crate) fn owner_refresh(conn: &Connection, id: &str) -> Result<(), StoreError> {
    if ready(conn)? {
        keyed(conn, &statements().owner, id)?;
    }
    Ok(())
}
