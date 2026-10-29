//! Exact query facts and additive counts maintained in canonical transactions.
use super::query_timestamps::{self, QueryTimestamp};
use super::{ReadTransaction, Store, StoreError};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(super) const TABLES: &str = r#"
CREATE TABLE IF NOT EXISTS tracker_task_query_facts (
 task_id TEXT PRIMARY KEY,project_key TEXT NOT NULL,number INTEGER NOT NULL,
 updated_ms INTEGER NOT NULL,anchor_ms INTEGER NOT NULL,priority_rank INTEGER NOT NULL,
 done_ms INTEGER,unfinished INTEGER NOT NULL,watched INTEGER NOT NULL,
 attention_asked INTEGER NOT NULL,attention_legacy INTEGER NOT NULL,blocked INTEGER NOT NULL,
 held INTEGER NOT NULL,undelivered INTEGER NOT NULL,review_mode TEXT,assignee_agent_id TEXT,
 unread_total INTEGER NOT NULL, contribution_agent TEXT,
 invalid_updated INTEGER NOT NULL DEFAULT 0, invalid_anchor INTEGER NOT NULL DEFAULT 0,invalid_done INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS tracker_task_query_number ON tracker_task_query_facts(project_key,number DESC);
CREATE INDEX IF NOT EXISTS tracker_task_query_updated ON tracker_task_query_facts(project_key,updated_ms DESC,number DESC);
CREATE INDEX IF NOT EXISTS tracker_task_query_priority ON tracker_task_query_facts(project_key,priority_rank DESC,number DESC);
CREATE INDEX IF NOT EXISTS tracker_task_query_anchor ON tracker_task_query_facts(project_key,anchor_ms ASC,number DESC);
CREATE INDEX IF NOT EXISTS tracker_task_query_done ON tracker_task_query_facts(project_key,done_ms DESC,number DESC);
CREATE INDEX IF NOT EXISTS tracker_task_query_invalid_updated ON tracker_task_query_facts(project_key) WHERE invalid_updated=1;
CREATE INDEX IF NOT EXISTS tracker_task_query_invalid_anchor ON tracker_task_query_facts(project_key) WHERE invalid_anchor=1;
CREATE INDEX IF NOT EXISTS tracker_task_query_invalid_done ON tracker_task_query_facts(project_key) WHERE invalid_done=1;
CREATE TABLE IF NOT EXISTS tracker_task_query_memberships (
 task_id TEXT NOT NULL,kind TEXT NOT NULL,value TEXT NOT NULL,project_key TEXT NOT NULL,number INTEGER NOT NULL,
 PRIMARY KEY(task_id,kind,value)
);
CREATE INDEX IF NOT EXISTS tracker_task_query_membership_lookup ON tracker_task_query_memberships(project_key,kind,value,number DESC);
CREATE TABLE IF NOT EXISTS tracker_task_query_counts (
 project_key TEXT NOT NULL,bucket TEXT NOT NULL,unfinished_open_count INTEGER NOT NULL,watched_unread_total INTEGER NOT NULL,
 PRIMARY KEY(project_key,bucket)
);
CREATE INDEX IF NOT EXISTS tracker_summary_meaningful ON tracker_task_summary_facts(task_id,ulid) WHERE meaningful=1;
INSERT OR IGNORE INTO meta(key,value) VALUES('task_query_generation','0');
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskCounts {
    pub unfinished_open_count: u64,
    pub watched_unread_total: u64,
}

impl Store {
    pub fn task_query_basis(&self) -> Result<String, StoreError> {
        ReadTransaction::new(&self.connection()).task_query_basis()
    }
    pub fn task_counts(
        &self,
        project: &str,
        agents: &[String],
    ) -> Result<Vec<TaskCounts>, StoreError> {
        ReadTransaction::new(&self.connection()).task_counts(project, agents)
    }
}
impl ReadTransaction<'_> {
    pub fn task_query_basis(&self) -> Result<String, StoreError> {
        let generation: String = self.connection().query_row(
            "SELECT value FROM meta WHERE key='task_query_generation'",
            [],
            |r| r.get(0),
        )?;
        let incarnation = format!("{:x}", Sha256::digest(self.tracker_list_key()?.as_bytes()));
        Ok(format!("{incarnation}:{generation}"))
    }
    pub fn task_counts(
        &self,
        project: &str,
        agents: &[String],
    ) -> Result<Vec<TaskCounts>, StoreError> {
        if agents.len() > 100 {
            return Err(StoreError::TaskSummaryInvalid(
                "at most 100 explicit agent buckets supported".into(),
            ));
        }
        let mut statement=self.connection().prepare_cached("SELECT unfinished_open_count,watched_unread_total FROM tracker_task_query_counts WHERE project_key=?1 AND bucket=?2")?;
        std::iter::once("")
            .chain(agents.iter().map(String::as_str))
            .map(|bucket| {
                Ok(statement
                    .query_row(params![project, bucket], |r| {
                        Ok(TaskCounts {
                            unfinished_open_count: r.get::<_, i64>(0)?.try_into().map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    0,
                                    rusqlite::types::Type::Integer,
                                    Box::new(e),
                                )
                            })?,
                            watched_unread_total: r.get::<_, i64>(1)?.try_into().map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    1,
                                    rusqlite::types::Type::Integer,
                                    Box::new(e),
                                )
                            })?,
                        })
                    })
                    .optional()?
                    .unwrap_or(TaskCounts {
                        unfinished_open_count: 0,
                        watched_unread_total: 0,
                    }))
            })
            .collect()
    }
}

fn contribute(conn: &Connection, id: &str, sign: i64) -> Result<(), StoreError> {
    conn.execute("INSERT INTO tracker_task_query_counts(project_key,bucket,unfinished_open_count,watched_unread_total)
 SELECT project_key,bucket,?2*unfinished,?2*unread_total FROM
 (SELECT project_key,'' bucket,unfinished,unread_total FROM tracker_task_query_facts WHERE task_id=?1
 UNION ALL SELECT project_key,contribution_agent,unfinished,unread_total FROM tracker_task_query_facts WHERE task_id=?1 AND contribution_agent IS NOT NULL) WHERE unfinished<>0 OR unread_total<>0
 ON CONFLICT(project_key,bucket) DO UPDATE SET unfinished_open_count=unfinished_open_count+excluded.unfinished_open_count,watched_unread_total=watched_unread_total+excluded.watched_unread_total",params![id,sign])?;
    if sign < 0 {
        conn.execute("DELETE FROM tracker_task_query_counts WHERE project_key=(SELECT project_key FROM tracker_task_query_facts WHERE task_id=?1) AND bucket IN ('',COALESCE((SELECT contribution_agent FROM tracker_task_query_facts WHERE task_id=?1),'')) AND unfinished_open_count=0 AND watched_unread_total=0",[id])?;
    }
    Ok(())
}
fn advance(conn: &Connection) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE meta SET value=CAST(value AS INTEGER)+1 WHERE key='task_query_generation'",
        [],
    )?;
    Ok(())
}
pub(super) fn remove(conn: &Connection, id: &str) -> Result<(), StoreError> {
    contribute(conn, id, -1)?;
    let changed = conn.execute(
        "DELETE FROM tracker_task_query_facts WHERE task_id=?1",
        [id],
    )?;
    conn.execute(
        "DELETE FROM tracker_task_query_memberships WHERE task_id=?1",
        [id],
    )?;
    if changed > 0 {
        advance(conn)?;
    }
    Ok(())
}

pub(super) fn refresh(conn: &Connection, id: &str) -> Result<(), StoreError> {
    let record: Option<String> = conn
        .query_row(
            "SELECT record FROM tracker_task_summaries WHERE task_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    let record: Value = record
        .map(|raw| super::super::tracker::decode(&raw, "tracker_task_summaries", id))
        .transpose()?
        .unwrap_or(Value::Null);
    let updated = timestamp(&record, "updated_at");
    let anchor = anchor(&record, updated);
    let done = if record["status"].as_str() == Some("done") {
        timestamp(&record, "done_at")
    } else {
        QueryTimestamp::Unknown
    };
    let review_mode: Option<String> = conn
        .query_row(
            "SELECT review_mode FROM tracker_task_query_facts WHERE task_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    // The old contribution stays available until all subtracts have landed.
    contribute(conn, id, -1)?;
    conn.execute(
        "DELETE FROM tracker_task_query_facts WHERE task_id=?1",
        [id],
    )?;
    conn.execute(r#"INSERT INTO tracker_task_query_facts
 SELECT task_id,project_key,number,COALESCE(?3,0),
 ?4,
 CASE json_extract(record,'$.priority') WHEN 'urgent' THEN 4 WHEN 'high' THEN 3 WHEN 'medium' THEN 2 WHEN 'low' THEN 1 ELSE 0 END,
 ?5,unfinished,watched,
 unfinished AND (user_ready OR (watched AND COALESCE(json_extract(record,'$.unread_mentions'),0)>0)),
 unfinished AND (json_extract(record,'$.status')='in_review' OR user_ready OR
 (watched AND COALESCE(json_extract(record,'$.unread_count'),0)>0 AND COALESCE(json_extract(record,'$.unread_agent_comments'),0)>0
 AND EXISTS(SELECT 1 FROM tracker_task_summary_facts f WHERE f.task_id=s.task_id AND f.meaningful=1
 AND f.ulid>COALESCE(substr(json_extract(record,'$.dismissed_through'),instr(json_extract(record,'$.dismissed_through'),'-')+1),'')))),
 blocked,COALESCE(json_extract(record,'$.assignee.kind'),'')<>'' OR (undelivered AND COALESCE(json_extract(record,'$.deferred_assignment.target.kind'),'')<>''),
 undelivered,?2,
 json_extract(record,'$.assignee.agent_id'),
 CASE WHEN unfinished THEN COALESCE(json_extract(record,'$.unread_count'),0) ELSE 0 END,
 CASE WHEN json_extract(record,'$.assignee.kind')='agent' THEN json_extract(record,'$.assignee.agent_id') END,?6,?7,?8
 FROM (SELECT *,json_extract(record,'$.state')<>'closed' AND json_extract(record,'$.status')<>'done' unfinished,
 COALESCE(json_extract(record,'$.watched'),0) watched,COALESCE(json_extract(record,'$.dependencies.blocked'),0) blocked,
 COALESCE(json_extract(record,'$.assignee.kind')='user',0) AND NOT COALESCE(json_extract(record,'$.dependencies.blocked'),0) user_ready,
 COALESCE(json_extract(record,'$.deferred_assignment.state') IN ('waiting','queued','provisioning','failed'),0) undelivered
 FROM tracker_task_summaries WHERE task_id=?1) s"#,params![id,review_mode,millis(updated).unwrap_or(0),millis(anchor).unwrap_or(i64::MAX),millis(done),invalid(updated),invalid(anchor),invalid(done)])?;
    contribute(conn, id, 1)?;
    advance(conn)
}

/// Canonical relations are reduced only on task writes, using the typed task
/// already available to the producer. History/metadata writes never parse bodies.
pub(super) fn refresh_relations(
    conn: &Connection,
    task: &crate::tracker::Task,
) -> Result<(), StoreError> {
    conn.execute(
        "DELETE FROM tracker_task_query_memberships WHERE task_id=?1",
        [&task.id],
    )?;
    let mut statement=conn.prepare_cached("INSERT OR IGNORE INTO tracker_task_query_memberships(task_id,kind,value,project_key,number) VALUES(?1,?2,?3,?4,?5)")?;
    for (kind, values) in [
        ("workspace", &task.links.workspace_ids),
        ("tracker", &task.trackers),
    ] {
        for value in values {
            statement.execute(params![
                task.id,
                kind,
                value,
                task.project_path,
                i64::try_from(task.number).unwrap_or(i64::MAX)
            ])?;
        }
    }
    Ok(())
}
pub(super) fn refresh_review_mode(conn: &Connection, id: &str) -> Result<(), StoreError> {
    conn.execute("UPDATE tracker_task_query_facts SET review_mode=(SELECT COALESCE(json_extract(record,'$.mode'),'snapshot') FROM reviews WHERE task_id=?1) WHERE task_id=?1",[id])?;
    Ok(())
}

fn timestamp(record: &Value, field: &str) -> QueryTimestamp {
    match record.get(field) {
        None | Some(Value::Null) => QueryTimestamp::Unknown,
        Some(Value::String(value)) => query_timestamps::parse(Some(value)),
        _ => QueryTimestamp::Unsupported,
    }
}
fn anchor(record: &Value, updated: QueryTimestamp) -> QueryTimestamp {
    let pair = record["session_started_ms"]
        .as_i64()
        .zip(record["last_activity_ms"].as_i64());
    if let Some((start, _last)) = pair.filter(|(start, last)| {
        (-9007199254740991..=9007199254740991).contains(start)
            && (-9007199254740991..=9007199254740991).contains(last)
            && start <= last
    }) {
        return QueryTimestamp::Known(start);
    }
    match timestamp(record, "created_at") {
        QueryTimestamp::Unknown => updated,
        other => other,
    }
}
fn millis(timestamp: QueryTimestamp) -> Option<i64> {
    match timestamp {
        QueryTimestamp::Known(millis) => Some(millis),
        _ => None,
    }
}
fn invalid(timestamp: QueryTimestamp) -> bool {
    matches!(timestamp, QueryTimestamp::Unsupported)
}
