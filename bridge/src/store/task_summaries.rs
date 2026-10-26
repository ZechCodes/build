//! Bounded task cards. The persisted projection is updated by SQLite triggers,
//! including writes made by lifecycle paths that update task JSON directly.
//! Histories are reduced on writes, never decoded while serving a card page.

use super::{ReadTransaction, Store, StoreError};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::BTreeMap;

mod sql;

/// Hard bound on candidates, including one lookahead row.
pub const MAX_SUMMARY_CANDIDATES: usize = 2001;

pub struct TaskSummaryCandidate {
    pub row: Value,
    pub assignment_target: Option<Value>,
    /// Observed identities for the card's creator and assignee. The caller
    /// merges these into one shared page dictionary instead of duplicating them.
    pub identities: BTreeMap<String, Value>,
}

pub struct TaskFilterChoice {
    pub key: String,
    pub value: Value,
}

impl Store {
    pub fn task_summary_identity(
        &self,
        agent: &str,
    ) -> Result<Option<crate::tracker::TaskAgentIdentity>, StoreError> {
        ReadTransaction::new(&self.connection()).task_summary_identity(agent)
    }

    pub fn list_task_summary_candidates(
        &self,
        project: &str,
        below: Option<u64>,
        limit: usize,
    ) -> Result<Vec<TaskSummaryCandidate>, StoreError> {
        ReadTransaction::new(&self.connection()).list_task_summary_candidates(project, below, limit)
    }

    pub fn list_task_filter_choices(
        &self,
        project: &str,
        dimension: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<TaskFilterChoice>, StoreError> {
        ReadTransaction::new(&self.connection())
            .list_task_filter_choices(project, dimension, after, limit)
    }
}

impl ReadTransaction<'_> {
    /// Bounded durable identity from this request's snapshot. The materialized
    /// row is independent of owner record size and conversation history.
    pub fn task_summary_identity(
        &self,
        agent: &str,
    ) -> Result<Option<crate::tracker::TaskAgentIdentity>, StoreError> {
        let raw: Option<String> = self
            .connection()
            .query_row(
                "SELECT record FROM tracker_task_summary_identities WHERE agent_id=?1",
                [agent],
                |row| row.get(0),
            )
            .optional()?;
        raw.map(|raw| super::tracker::decode(&raw, "tracker_task_summary_identities", agent))
            .transpose()
    }

    /// Seeks one immutable number range. No task bodies, timeline records,
    /// rosters, or dependency expansion are touched by this read.
    pub fn list_task_summary_candidates(
        &self,
        project: &str,
        below: Option<u64>,
        limit: usize,
    ) -> Result<Vec<TaskSummaryCandidate>, StoreError> {
        let mut statement = self.connection().prepare(
            "SELECT record, assignment_target, identities FROM tracker_task_summaries
             WHERE project_key = ?1 AND number < ?2 ORDER BY number DESC LIMIT ?3",
        )?;
        let records = statement.query_map(
            params![
                project,
                below
                    .and_then(|n| i64::try_from(n).ok())
                    .unwrap_or(i64::MAX),
                limit.min(MAX_SUMMARY_CANDIDATES) as i64
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;
        records
            .map(|record| {
                let (raw, target, identities) = record?;
                let mut row: Value =
                    super::tracker::decode(&raw, "tracker_task_summaries", project)?;
                if row.get("_summary_invalid_labels").and_then(Value::as_bool) == Some(true) {
                    return Err(StoreError::TaskSummaryInvalid(
                        "legacy task labels exceed supported summary limits".into(),
                    ));
                }
                if row.get("_summary_invalid_refs").and_then(Value::as_bool) == Some(true) {
                    return Err(StoreError::TaskSummaryInvalid(
                        "legacy task reference exceeds 128 bytes".into(),
                    ));
                }
                if row
                    .get("_summary_invalid_metadata_refs")
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    return Err(StoreError::TaskSummaryInvalid(
                        "legacy task metadata reference exceeds 128 bytes".into(),
                    ));
                }
                row.as_object_mut()
                    .expect("summary is an object")
                    .remove("_summary_invalid_metadata_refs");
                row.as_object_mut()
                    .expect("summary is an object")
                    .remove("_summary_invalid_refs");
                row.as_object_mut()
                    .expect("summary is an object")
                    .remove("_summary_invalid_labels");
                Ok(TaskSummaryCandidate {
                    row,
                    assignment_target: target
                        .map(|raw| {
                            super::tracker::decode(&raw, "tracker_task_summaries/target", project)
                        })
                        .transpose()?,
                    identities: super::tracker::decode(
                        &identities,
                        "tracker_task_summaries/identities",
                        project,
                    )?,
                })
            })
            .collect()
    }

    /// Distinct choices seek an index of materialized memberships, not task
    /// JSON. The key is returned separately so cursors bind the actual ordering.
    pub fn list_task_filter_choices(
        &self,
        project: &str,
        dimension: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<TaskFilterChoice>, StoreError> {
        let mut statement = self.connection().prepare(
            "SELECT CASE WHEN length(CAST(choice_key AS BLOB))<=512 THEN choice_key ELSE '' END, CASE WHEN length(CAST(choice_key AS BLOB))<=512 AND length(CAST(value AS BLOB))<=1024 THEN value END FROM tracker_task_filter_values
             WHERE project_key = ?1 AND dimension = ?2 AND choice_key > ?3
             ORDER BY choice_key LIMIT ?4",
        )?;
        let rows = statement.query_map(
            params![
                project,
                dimension,
                after.unwrap_or(""),
                limit.min(501) as i64
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )?;
        rows.map(|row| {
            let (key, raw) = row?;
            let raw = raw.ok_or_else(|| {
                StoreError::TaskSummaryInvalid(
                    "filter choice key exceeds 512 bytes or value exceeds 1024 bytes".into(),
                )
            })?;
            let value = super::tracker::decode(&raw, "tracker_task_filter_choices", project)?;
            validate_filter_choice(dimension, &value)?;
            Ok(TaskFilterChoice { key, value })
        })
        .collect()
    }
}

fn validate_filter_choice(dimension: &str, value: &Value) -> Result<(), StoreError> {
    let valid = match dimension {
        "label" => value.as_str().is_some_and(|label| {
            !label.is_empty()
                && label.trim() == label
                && label.len() <= crate::tracker::MAX_LABEL_BYTES
        }),
        "assignee" => valid_assignee_choice(value),
        "state" => value
            .as_str()
            .and_then(crate::tracker::TaskState::parse)
            .is_some(),
        "status" => value
            .as_str()
            .and_then(crate::tracker::normalize_status)
            .is_some(),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(StoreError::TaskSummaryInvalid(format!(
            "invalid {dimension} filter choice"
        )))
    }
}

fn valid_assignee_choice(value: &Value) -> bool {
    if value.as_str() == Some("none") {
        return true;
    }
    let Ok(assignee) = serde_json::from_value::<crate::tracker::Assignee>(value.clone()) else {
        return false;
    };
    let valid_id = assignee
        .agent_id()
        .is_none_or(|id| !id.is_empty() && id.len() <= 128);
    valid_id && serde_json::to_value(assignee).is_ok_and(|canonical| canonical == *value)
}

/// Startup precedes reader admission. Every transaction advances a durable
/// rowid checkpoint, so an interrupted migration resumes without restarting.
pub(super) fn initialize(conn: &mut Connection) -> Result<(), StoreError> {
    #[cfg(test)]
    let _phase = MigrationPhase::new("initialize");
    let existing: i64 = conn.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name IN ('tracker_task_summary_facts','tracker_task_summaries','tracker_task_filter_choices','tracker_task_filter_values','tracker_task_summary_identities')",[],|row|row.get(0))?;
    let has_context = has_context_column(conn)?;
    if existing != 5 || !has_context {
        // Commit repair intent before recreating a missing table: a crash
        // between DDL and reset must not make a partial unit look complete.
        conn.execute(
            "INSERT OR IGNORE INTO meta(key,value) VALUES('task_summary_rebuild','1')",
            [],
        )?;
    }
    conn.execute_batch(sql::TABLES)?;
    if !has_context_column(conn)? {
        conn.execute_batch("ALTER TABLE tracker_task_summaries ADD COLUMN context TEXT NOT NULL DEFAULT '{}' CHECK(length(CAST(context AS BLOB))<=4096);")?;
    }
    drop_projection_triggers(conn)?;
    let rebuilding: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM meta WHERE key='task_summary_rebuild')",
        [],
        |row| row.get(0),
    )?;
    if rebuilding {
        reset_projection(conn)?;
    }
    migrate_facts(conn, "tracker_comments")?;
    migrate_facts(conn, "tracker_events")?;
    migrate_metadata(conn)?;
    migrate_tasks(conn)?;
    migrate_identities(conn)?;
    conn.execute_batch(&sql::triggers())?;
    Ok(())
}

fn has_context_column(conn: &Connection) -> Result<bool, StoreError> {
    let columns = conn
        .prepare("PRAGMA table_info(tracker_task_summaries)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns.iter().any(|name| name == "context"))
}

fn drop_projection_triggers(conn: &Connection) -> Result<(), StoreError> {
    let names = conn
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type='trigger' AND name LIKE 'tracker_summary_%'",
        )?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for name in names {
        conn.execute_batch(&format!("DROP TRIGGER \"{}\"", name.replace('"', "\"\"")))?;
    }
    Ok(())
}

fn reset_projection(conn: &mut Connection) -> Result<(), StoreError> {
    #[cfg(test)]
    let _phase = MigrationPhase::new("reset_projection");
    for table in [
        "tracker_task_filter_choices",
        "tracker_task_filter_values",
        "tracker_task_summary_facts",
        "tracker_task_summaries",
        "tracker_task_summary_identities",
    ] {
        reset_table(conn, table)?;
    }
    // Rows are fully reset before dropping progress. Clearing the durable
    // repair marker and old migration checkpoints shares one commit.
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM meta WHERE key LIKE 'task_summary_v1_%' OR key='task_summary_rebuild'",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

fn reset_table(conn: &mut Connection, table: &str) -> Result<(), StoreError> {
    loop {
        let tx = conn.transaction()?;
        let changed=tx.execute(&format!("DELETE FROM {table} WHERE rowid IN (SELECT rowid FROM {table} ORDER BY rowid LIMIT 128)"),[])?;
        tx.commit()?;
        if changed == 0 {
            break;
        }
    }
    Ok(())
}

fn migrate_metadata(conn: &mut Connection) -> Result<(), StoreError> {
    let complete: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM meta WHERE key='task_summary_v1_metadata_complete')",
        [],
        |row| row.get(0),
    )?;
    if complete {
        return Ok(());
    }
    migrate_batches(
        conn,
        "task_summary_v1_metadata",
        "tracker_task_summaries",
        |tx, below, through| {
            let ids=tx.prepare("SELECT task_id FROM tracker_task_summaries WHERE rowid>?1 AND rowid<=?2 AND json_type(record,'$._summary_invalid_metadata_refs') IS NULL")?
            .query_map(params![below,through],|row|row.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
            for id in ids {
                tx.execute_batch(&sql::refresh_metadata(&format!(
                    "'{}'",
                    id.replace('\'', "''")
                )))?;
            }
            Ok(())
        },
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO meta(key,value) VALUES('task_summary_v1_metadata_complete','1')",
        [],
    )?;
    Ok(())
}

fn migrate_identities(conn: &mut Connection) -> Result<(), StoreError> {
    #[cfg(test)]
    let _phase = MigrationPhase::new("migrate_identities");
    migrate_batches(
        conn,
        "task_summary_v1_agents",
        "agents",
        |tx, below, through| {
            tx.execute_batch(&sql::refresh_identities(&format!(
                "a.rowid>{below} AND a.rowid<={through}"
            )))?;
            Ok(())
        },
    )
}

fn migrate_facts(conn: &mut Connection, table: &str) -> Result<(), StoreError> {
    #[cfg(test)]
    let _phase = MigrationPhase::new(format!("facts:{table}"));
    let key = format!("task_summary_v1_{table}");
    migrate_batches(conn, &key, table, |tx, below, through| {
        tx.execute_batch(&sql::insert_facts(
            table,
            &format!("rowid > {below} AND rowid <= {through}"),
        ))?;
        Ok(())
    })
}

fn migrate_tasks(conn: &mut Connection) -> Result<(), StoreError> {
    #[cfg(test)]
    let _phase = MigrationPhase::new("migrate_tasks");
    migrate_batches(
        conn,
        "task_summary_v1_tasks",
        "tracker_tasks",
        |tx, below, through| {
            let ids: Vec<String> = tx
                .prepare(
                    "SELECT id FROM tracker_tasks WHERE rowid>?1 AND rowid<=?2 ORDER BY rowid",
                )?
                .query_map(params![below, through], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            for id in ids {
                tx.execute_batch(&sql::rebuild_task(&format!("'{}'", id.replace('\'', "''"))))?;
            }
            Ok(())
        },
    )
}

#[cfg(test)]
fn thread_cpu_us() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // Diagnostic-only Linux thread CPU clock, independent of wall scheduling.
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) } != 0 {
        return 0;
    }
    (time.tv_sec as u64) * 1_000_000 + (time.tv_nsec as u64) / 1000
}

#[cfg(test)]
struct MigrationPhase {
    name: String,
    started: std::time::Instant,
    started_cpu_us: u64,
}

#[cfg(test)]
impl MigrationPhase {
    fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            started: std::time::Instant::now(),
            started_cpu_us: thread_cpu_us(),
        }
    }
}

#[cfg(test)]
impl Drop for MigrationPhase {
    fn drop(&mut self) {
        eprintln!(
            "task_summary_migration phase={} elapsed_us={} cpu_us={}",
            self.name,
            self.started.elapsed().as_micros(),
            thread_cpu_us().saturating_sub(self.started_cpu_us)
        );
    }
}

fn migrate_batches(
    conn: &mut Connection,
    key: &str,
    table: &str,
    apply: impl Fn(&rusqlite::Transaction<'_>, i64, i64) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    #[cfg(test)]
    let mut counts = (0_usize, 0_usize, 0_usize);
    loop {
        #[cfg(test)]
        let batch_started = std::time::Instant::now();
        let tx = conn.transaction()?;
        let below: i64 = tx
            .query_row(
                "SELECT COALESCE((SELECT value FROM meta WHERE key=?1),'0')",
                [key],
                |r| r.get::<_, String>(0),
            )?
            .parse()
            .unwrap_or(0);
        let through: Option<i64> = tx.query_row(&format!("SELECT MAX(rowid) FROM (SELECT rowid FROM {table} WHERE rowid>?1 ORDER BY rowid LIMIT 128)"), [below], |r| r.get(0))?;
        let Some(through) = through else {
            tx.commit()?;
            break;
        };
        #[cfg(test)]
        let batch_rows = {
            let rows: i64 = tx.query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE rowid>?1 AND rowid<=?2"),
                params![below, through],
                |r| r.get(0),
            )?;
            let rows = rows as usize;
            counts = (counts.0 + 1, counts.1 + rows, counts.2.max(rows));
            rows
        };
        #[cfg(test)]
        let setup_us = batch_started.elapsed().as_micros();
        #[cfg(test)]
        let apply_started = std::time::Instant::now();
        #[cfg(test)]
        let apply_cpu_started = thread_cpu_us();
        apply(&tx, below, through)?;
        #[cfg(test)]
        let apply_us = apply_started.elapsed().as_micros();
        #[cfg(test)]
        let apply_cpu_us = thread_cpu_us().saturating_sub(apply_cpu_started);
        tx.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, through.to_string()])?;
        #[cfg(test)]
        let commit_started = std::time::Instant::now();
        #[cfg(test)]
        let commit_cpu_started = thread_cpu_us();
        tx.commit()?;
        #[cfg(test)]
        if counts.0 <= 5 || counts.0.is_multiple_of(10) || batch_rows < 128 {
            eprintln!("task_summary_migration_batch table={table} batch={} below={below} through={through} rows={batch_rows} setup_us={setup_us} apply_us={apply_us} apply_cpu_us={apply_cpu_us} commit_us={} commit_cpu_us={}",counts.0,commit_started.elapsed().as_micros(),thread_cpu_us().saturating_sub(commit_cpu_started));
        }
    }
    #[cfg(test)]
    eprintln!(
        "task_summary_migration table={table} batches={} rows={} max_batch_rows={}",
        counts.0, counts.1, counts.2
    );
    Ok(())
}
