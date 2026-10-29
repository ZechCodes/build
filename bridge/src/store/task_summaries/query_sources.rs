//! Bounded, lossless predicate inputs captured before display projection.
use super::{
    query_timestamps::{self, QueryTimestamp},
    StoreError,
};
use crate::tracker::{Actor, Assignee, Task, TaskState};
use rusqlite::{params, Connection, OptionalExtension};

pub(super) const UNSUPPORTED: &str = "!unsupported-timestamp";
#[derive(Default)]
pub(super) struct Sources {
    pub updated: Option<String>,
    pub created: Option<String>,
    pub input_done: Option<String>,
    pub assignee_kind: Option<String>,
    pub assignee_agent_id: Option<String>,
    pub unfinished: bool,
    pub in_review: bool,
    pub status_done: bool,
    pub dismissed_ulid: Option<String>,
    pub invalid_markers: bool,
    pub pending_present: bool,
    pub pending_held: bool,
}
fn bounded_timestamp(value: &str) -> &str {
    if value.len() <= 38 {
        value
    } else {
        UNSUPPORTED
    }
}
fn marker_invalid(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.len() > 128)
}
fn assignee(task: &Task) -> (Option<&str>, Option<&str>) {
    match &task.assignee {
        None => (None, None),
        Some(Assignee::User) => (Some("user"), None),
        Some(Assignee::ProjectAgent) => (Some("project_agent"), None),
        Some(Assignee::Agent { agent_id }) => (
            Some("agent"),
            (agent_id.len() <= 128).then_some(agent_id.as_str()),
        ),
    }
}
pub(super) fn task_written(conn: &Connection, task: &Task) -> Result<(), StoreError> {
    let (kind, agent) = assignee(task);
    let (creator_kind, creator_agent_id) = match &task.created_by {
        Actor::User => ("user", None),
        Actor::Build => ("build", None),
        Actor::Agent { agent_id } => (
            "agent",
            (agent_id.len() <= 128).then_some(agent_id.as_str()),
        ),
    };
    let status_truncated = task.status.len() > 128;
    let status = if status_truncated {
        "\0unsupported-status"
    } else {
        task.status.as_str()
    };
    let labels = if task.labels.len() <= 20 && task.labels.iter().all(|label| label.len() <= 40) {
        serde_json::to_string(&task.labels).expect("string labels serialize")
    } else {
        "[]".into()
    };
    let invalid_markers = [
        task.read_through.as_deref(),
        task.watch_started_after.as_deref(),
        task.dismissed_through.as_deref(),
    ]
    .into_iter()
    .any(marker_invalid);
    let dismissed = task
        .dismissed_through
        .as_deref()
        .filter(|mark| mark.len() <= 128)
        .map(crate::tracker::summary::when);
    conn.execute("INSERT INTO tracker_task_query_sources(task_id,updated_source,created_source,input_done_source,assignee_kind,assignee_agent_id,status,status_truncated,labels_json,unfinished,in_review,status_done,dismissed_ulid,invalid_markers,creator_kind,creator_agent_id,read_through,watch_started_after,dismissed_through)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)
        ON CONFLICT(task_id) DO UPDATE SET updated_source=excluded.updated_source,created_source=excluded.created_source,input_done_source=excluded.input_done_source,assignee_kind=excluded.assignee_kind,assignee_agent_id=excluded.assignee_agent_id,status=excluded.status,status_truncated=excluded.status_truncated,labels_json=excluded.labels_json,unfinished=excluded.unfinished,in_review=excluded.in_review,status_done=excluded.status_done,dismissed_ulid=excluded.dismissed_ulid,invalid_markers=excluded.invalid_markers,creator_kind=excluded.creator_kind,creator_agent_id=excluded.creator_agent_id,read_through=excluded.read_through,watch_started_after=excluded.watch_started_after,dismissed_through=excluded.dismissed_through",
        params![task.id,bounded_timestamp(&task.updated_at),bounded_timestamp(&task.created_at),task.done_at.as_deref().map(bounded_timestamp),kind,agent,status,status_truncated,labels,task.state!=TaskState::Closed&&task.status!="done",task.status=="in_review",task.status=="done",dismissed,invalid_markers,creator_kind,creator_agent_id,task.read_through.as_deref().filter(|s|s.len()<=128),task.watch_started_after.as_deref().filter(|s|s.len()<=128),task.dismissed_through.as_deref().filter(|s|s.len()<=128)])?;
    Ok(())
}
pub(super) fn metadata_refresh(conn: &Connection, id: &str) -> Result<(), StoreError> {
    // Only metadata producers parse this canonical JSON; history writes reuse
    // two retained booleans, including JavaScript truthiness of arbitrary kind.
    conn.execute("UPDATE tracker_task_query_sources SET
        pending_present=COALESCE((SELECT state IN ('waiting','queued','provisioning','failed') FROM tracker_delivery_intents WHERE task_id=?1 AND state<>'cancelled' ORDER BY generation DESC LIMIT 1),0),
        pending_held=COALESCE((SELECT state IN ('waiting','queued','provisioning','failed') AND CASE json_type(record,'$.target.kind')
          WHEN 'text' THEN length(CAST(json_extract(record,'$.target.kind') AS BLOB))>0
          WHEN 'integer' THEN json_extract(record,'$.target.kind')<>0 WHEN 'real' THEN json_extract(record,'$.target.kind')<>0
          WHEN 'true' THEN 1 WHEN 'array' THEN 1 WHEN 'object' THEN 1 ELSE 0 END
        FROM tracker_delivery_intents WHERE task_id=?1 AND state<>'cancelled' ORDER BY generation DESC LIMIT 1),0) WHERE task_id=?1",[id])?;
    Ok(())
}
pub(super) fn load(conn: &Connection, id: &str) -> Result<Sources, StoreError> {
    Ok(conn.query_row("SELECT updated_source,created_source,input_done_source,assignee_kind,assignee_agent_id,unfinished,in_review,status_done,dismissed_ulid,invalid_markers,pending_present,pending_held FROM tracker_task_query_sources WHERE task_id=?1",[id],|r|Ok(Sources{updated:r.get(0)?,created:r.get(1)?,input_done:r.get(2)?,assignee_kind:r.get(3)?,assignee_agent_id:r.get(4)?,unfinished:r.get(5)?,in_review:r.get(6)?,status_done:r.get(7)?,dismissed_ulid:r.get(8)?,invalid_markers:r.get(9)?,pending_present:r.get(10)?,pending_held:r.get(11)?})).optional()?.unwrap_or_default())
}
pub(super) fn parse(value: Option<&str>) -> QueryTimestamp {
    query_timestamps::parse(value)
}
pub(super) fn done(
    conn: &Connection,
    id: &str,
    source: &Sources,
) -> Result<QueryTimestamp, StoreError> {
    if !source.status_done {
        return Ok(QueryTimestamp::Unknown);
    }
    if let Some(value) = &source.input_done {
        return Ok(parse(Some(value)));
    }
    // Same indexed winner and raw at/id ordering as canonical done fallback.
    // BLOB length sees bytes beyond NUL; no display substr or history replay.
    let event:Option<String>=conn.query_row("SELECT CASE WHEN typeof(at)='text' AND length(CAST(at AS BLOB))<=38 THEN at ELSE '!unsupported-timestamp' END FROM tracker_task_summary_facts WHERE task_id=?1 AND done_move=1 ORDER BY at DESC,id DESC LIMIT 1",[id],|r|r.get(0)).optional()?;
    Ok(parse(event.as_deref().or(source.created.as_deref())))
}
