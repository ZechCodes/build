//! Durable pooled sessions use the same update rule as live list snapshots.
//! Only new committed messages enter a pool; edits and unchanged timestamps
//! neither rewrite the summary nor publish another device revision.
use crate::session_summary::SessionSummary;
use crate::store::{Store, StoreError};
use rusqlite::{params, Connection, OptionalExtension};

pub(super) fn install(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS device_pooled_sessions(kind TEXT NOT NULL,key TEXT NOT NULL,session_started_ms INTEGER,last_activity_ms INTEGER,PRIMARY KEY(kind,key)) WITHOUT ROWID;
        CREATE TABLE IF NOT EXISTS device_session_project_ids(path TEXT PRIMARY KEY,project_id TEXT NOT NULL) WITHOUT ROWID;")?;
    super::device_session_backfill::install(conn)
}

pub(super) fn summary(
    conn: &Connection,
    kind: &str,
    key: &str,
) -> Result<Option<SessionSummary>, StoreError> {
    Ok(conn.prepare_cached("SELECT session_started_ms,last_activity_ms FROM device_pooled_sessions WHERE kind=?1 AND key=?2")?
        .query_row(params![kind,key], |row| Ok(SessionSummary { session_started_ms: row.get(0)?, last_activity_ms: row.get(1)? })).optional()?)
}

fn replace_summary(
    conn: &Connection,
    kind: &str,
    key: &str,
    held: Option<SessionSummary>,
    next: Option<SessionSummary>,
) -> Result<(), StoreError> {
    if next == held {
        return Ok(());
    }
    if let Some(next) = next {
        conn.prepare_cached("INSERT INTO device_pooled_sessions VALUES(?1,?2,?3,?4) ON CONFLICT(kind,key) DO UPDATE SET session_started_ms=?3,last_activity_ms=?4")?
            .execute(params![kind,key,next.session_started_ms,next.last_activity_ms])?;
    } else {
        conn.prepare_cached("DELETE FROM device_pooled_sessions WHERE kind=?1 AND key=?2")?
            .execute(params![kind, key])?;
    }
    if kind == "owner" {
        super::device_families::touch_retained(conn, "owner", key)?;
    } else {
        touch_project(conn, key)?;
    }
    Ok(())
}
pub(super) fn update(conn: &Connection, kind: &str, key: &str, at: i64) -> Result<(), StoreError> {
    let held = summary(conn, kind, key)?;
    replace_summary(
        conn,
        kind,
        key,
        held,
        Some(held.unwrap_or_default().updated(at)),
    )
}

pub(super) fn note_message(
    conn: &Connection,
    conversation: &str,
    at: i64,
) -> Result<(), StoreError> {
    let links: Option<(String,Option<String>)> = conn.prepare_cached("SELECT a.owner_id,o.project_path FROM agents a LEFT JOIN device_owner_projects o ON o.owner_id=a.owner_id WHERE a.id=?1")?
        .query_row([conversation], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
    let Some((owner, project)) = links else {
        return Ok(());
    };
    update(conn, "owner", &owner, at)?;
    if let Some(project) = project {
        update(conn, "project", &project, at)?;
    }
    Ok(())
}

pub(super) fn project_key(conn: &Connection, id: &str) -> Result<String, StoreError> {
    Ok(conn
        .prepare_cached("SELECT path FROM device_project_ids WHERE project_id=?1 LIMIT 1")?
        .query_row([id], |row| row.get(0))
        .optional()?
        .unwrap_or_else(|| format!("@registry:{id}")))
}

fn project_id(conn: &Connection, path: &str) -> Result<Option<String>, StoreError> {
    if let Some(id) = path.strip_prefix("@registry:") {
        return Ok(Some(id.to_owned()));
    }
    Ok(conn
        .prepare_cached("SELECT project_id FROM device_project_ids WHERE path=?1")?
        .query_row([path], |row| row.get(0))
        .optional()?)
}

fn touch_project(conn: &Connection, path: &str) -> Result<(), StoreError> {
    if let Some(id) = project_id(conn, path)? {
        super::device_families::touch_retained(conn, "project", &id)?;
    }
    Ok(())
}

/// Registry IDs can change independently from the stable path that owns a
/// pool. Retire the previous device key and publish the newly resolved one.
pub(super) fn project_mapping_changed(conn: &Connection, path: &str) -> Result<(), StoreError> {
    let previous: Option<String> = conn
        .prepare_cached("SELECT project_id FROM device_session_project_ids WHERE path=?1")?
        .query_row([path], |row| row.get(0))
        .optional()?;
    let current = project_id(conn, path)?;
    if let Some(id) = &current {
        if summary(conn, "project", &format!("@registry:{id}"))?.is_some() {
            // An old store can discover its registry mapping after bootstrap.
            // Replay exact remaining timestamps; two latest intervals cannot
            // recover an older interval bridged by the other source.
            clear_project(conn, id)?;
        }
    }
    if previous == current {
        return Ok(());
    }
    for id in previous.iter().chain(current.iter()) {
        super::device_families::touch_retained(conn, "project", id)?;
    }
    match current {
        Some(id) => {
            conn.prepare_cached("INSERT INTO device_session_project_ids VALUES(?1,?2) ON CONFLICT(path) DO UPDATE SET project_id=?2")?.execute(params![path,id])?;
        }
        None => {
            conn.prepare_cached("DELETE FROM device_session_project_ids WHERE path=?1")?
                .execute([path])?;
        }
    }
    Ok(())
}

pub(super) fn forget_owner(conn: &Connection, id: &str) -> Result<(), StoreError> {
    conn.prepare_cached("DELETE FROM device_pooled_sessions WHERE kind='owner' AND key=?1")?
        .execute([id])?;
    Ok(())
}

fn message_times(conn: &Connection, sql: &str, key: &str) -> Result<Vec<i64>, StoreError> {
    let raw = conn
        .prepare_cached(sql)?
        .query_map([key], |row| row.get::<_, Option<String>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(raw
        .into_iter()
        .flatten()
        .filter_map(|at| crate::session_summary::message_millis(&at))
        .collect())
}
fn retained_times(conn: &Connection, sql: &str, key: &str) -> Result<Vec<i64>, StoreError> {
    Ok(conn
        .prepare_cached(sql)?
        .query_map([key], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?)
}
fn replay(mut times: Vec<i64>) -> Option<SessionSummary> {
    if times.is_empty() {
        return None;
    }
    times.sort_unstable();
    Some(
        times
            .into_iter()
            .fold(SessionSummary::default(), SessionSummary::updated),
    )
}
fn rebuild_owner(conn: &Connection, key: &str) -> Result<(), StoreError> {
    let mut times=message_times(conn,"SELECT json_extract(t.item,'$.data.created_at') FROM thread_items t JOIN agents a ON a.id=t.agent_id WHERE a.owner_id=?1 AND t.message=1 AND COALESCE(json_extract(t.item,'$.data.from_build'),0)=0",key)?;
    times.extend(retained_times(
        conn,
        "SELECT ts_ms FROM inbox_retained_run_messages WHERE run_id=?1",
        key,
    )?);
    replace_summary(
        conn,
        "owner",
        key,
        summary(conn, "owner", key)?,
        replay(times),
    )
}
fn rebuild_project(conn: &Connection, key: &str, id: Option<&str>) -> Result<(), StoreError> {
    let mut times=message_times(conn,"SELECT json_extract(t.item,'$.data.created_at') FROM thread_items t JOIN agents a ON a.id=t.agent_id JOIN device_owner_projects o ON o.owner_id=a.owner_id WHERE o.project_path=?1 AND t.message=1 AND COALESCE(json_extract(t.item,'$.data.from_build'),0)=0",key)?;
    times.extend(retained_times(conn,"SELECT r.ts_ms FROM inbox_retained_run_messages r JOIN device_owner_projects o ON o.owner_id=r.run_id WHERE o.project_path=?1",key)?);
    if let Some(id) = id {
        times.extend(retained_times(
            conn,
            "SELECT ts_ms FROM inbox_retained_messages WHERE project_id=?1",
            id,
        )?);
    }
    replace_summary(
        conn,
        "project",
        key,
        summary(conn, "project", key)?,
        replay(times),
    )
}
/// Registry migration and explicit history clearing can replay affected
/// timestamp facts. Neither this work nor reset enters the append path.
pub(super) fn clear_project(conn: &Connection, id: &str) -> Result<(), StoreError> {
    let key = project_key(conn, id)?;
    rebuild_project(conn, &key, Some(id))?;
    if !key.starts_with("@registry:") {
        conn.prepare_cached("DELETE FROM device_pooled_sessions WHERE kind='project' AND key=?1")?
            .execute([format!("@registry:{id}")])?;
    }
    Ok(())
}
/// Conversation reset replaces the affected owner/project authority in the
/// same canonical transaction. Other owners retain their compact boundaries.
pub(in crate::store) fn reset(
    conn: &Connection,
    owners: &std::collections::HashSet<String>,
) -> Result<(), StoreError> {
    let mut projects = std::collections::HashSet::new();
    for owner in owners {
        let path: Option<String> = conn
            .prepare_cached("SELECT project_path FROM device_owner_projects WHERE owner_id=?1")?
            .query_row([owner], |row| row.get(0))
            .optional()?;
        if let Some(path) = path {
            projects.insert(path);
        }
        rebuild_owner(conn, owner)?;
    }
    for path in projects {
        rebuild_project(conn, &path, project_id(conn, &path)?.as_deref())?;
    }
    Ok(())
}

pub(crate) type SessionFact = (String, String, SessionSummary);
impl Store {
    /// Startup reads compact pool authority after the bounded migration has
    /// completed. The app separately restores message-seen cursors and any
    /// not-yet-resolved registry pools from its startup history replay.
    pub(crate) fn device_session_summaries(&self) -> Result<Vec<SessionFact>, StoreError> {
        Ok(self.connection().prepare("SELECT kind,key,session_started_ms,last_activity_ms FROM device_pooled_sessions ORDER BY kind,key")?
            .query_map([], |row| Ok((row.get(0)?,row.get(1)?,SessionSummary { session_started_ms:row.get(2)?,last_activity_ms:row.get(3)? })))?
            .collect::<Result<Vec<_>,_>>()?)
    }
}
