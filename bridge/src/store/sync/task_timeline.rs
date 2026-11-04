//! Bounded timeline windows and authenticated task-specific older-page cursors.
use super::schema;
use crate::store::{ReadTransaction, Store, StoreError};
use crate::tracker::TimelineEntry;
use aws_lc_rs::hmac;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const TASK_TIMELINE_DEFAULT_LIMIT: usize = 50;
pub const TASK_TIMELINE_MAX_LIMIT: usize = 100;
const ROW_BYTES: usize = 65_536;
const PAGE_BYTES: usize = 256 * 1024;
const CURSOR_BYTES: usize = 2048;

#[derive(Debug)]
pub struct TaskTimelinePage {
    pub timeline: Vec<TimelineEntry>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub timeline_incarnation: String,
    pub last_reviewed_snapshot_id: Option<String>,
    /// Highest canonical unread-relevant ULID, including rows outside this
    /// timestamp window. Read marks and unread facts use ULID order.
    pub timeline_read_through: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Position {
    purpose: String,
    incarnation: String,
    task: String,
    at: String,
    id: String,
    kind: String,
    inclusive: bool,
}

fn invalid_cursor() -> StoreError {
    StoreError::TaskTimelineCursorInvalid
}

fn encode(position: &Position, key: &[u8]) -> String {
    let bytes = serde_json::to_vec(position).expect("timeline cursor fields serialize");
    let signature = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), &bytes);
    format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(bytes),
        URL_SAFE_NO_PAD.encode(signature.as_ref())
    )
}

fn decode(raw: &str, task: &str, incarnation: &str, key: &[u8]) -> Result<Position, StoreError> {
    if raw.len() > CURSOR_BYTES {
        return Err(invalid_cursor());
    }
    let (payload, signature) = raw.split_once('.').ok_or_else(invalid_cursor)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| invalid_cursor())?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| invalid_cursor())?;
    hmac::verify(&hmac::Key::new(hmac::HMAC_SHA256, key), &bytes, &signature)
        .map_err(|_| invalid_cursor())?;
    let position: Position = serde_json::from_slice(&bytes).map_err(|_| invalid_cursor())?;
    if position.purpose != "task-timeline-v1"
        || position.incarnation != incarnation
        || position.task != task
        || !matches!(position.kind.as_str(), "comment" | "event")
        || position.at.len() > 128
        || position.id.len() > 256
    {
        return Err(invalid_cursor());
    }
    Ok(position)
}

type Row = (String, String, &'static str, usize, Option<String>);

impl Store {
    pub fn task_timeline_page(
        &self,
        task: &str,
        before: Option<&str>,
        limit: usize,
    ) -> Result<TaskTimelinePage, StoreError> {
        ReadTransaction::new(&self.connection()).task_timeline_page(task, before, limit)
    }
    pub fn task_timeline_anchor_page(
        &self,
        task: &str,
        anchor: &str,
        limit: usize,
    ) -> Result<TaskTimelinePage, StoreError> {
        ReadTransaction::new(&self.connection()).task_timeline_anchor_page(task, anchor, limit)
    }
}

impl ReadTransaction<'_> {
    pub fn task_timeline_page(
        &self,
        task: &str,
        before: Option<&str>,
        limit: usize,
    ) -> Result<TaskTimelinePage, StoreError> {
        validate_task(task)?;
        if !(1..=TASK_TIMELINE_MAX_LIMIT).contains(&limit) {
            return Err(StoreError::TaskSummaryInvalid(
                "timeline page limit must be 1 to 100".into(),
            ));
        }
        let (incarnation, key) = schema::identity(self.connection())?;
        let position = before
            .map(|raw| decode(raw, task, &incarnation, &key))
            .transpose()?;
        self.timeline_window(task, position, limit, incarnation, key)
    }

    pub fn task_timeline_anchor_page(
        &self,
        task: &str,
        anchor: &str,
        limit: usize,
    ) -> Result<TaskTimelinePage, StoreError> {
        validate_task(task)?;
        if !(1..=TASK_TIMELINE_MAX_LIMIT).contains(&limit) {
            return Err(StoreError::TaskSummaryInvalid(
                "timeline page limit must be 1 to 100".into(),
            ));
        }
        if anchor.is_empty() || anchor.len() > 256 {
            return Err(StoreError::TaskSummaryInvalid(
                "invalid timeline_anchor selector".into(),
            ));
        }
        let (incarnation, key) = schema::identity(self.connection())?;
        let point: Option<(Option<String>,String)> = self.connection().prepare_cached("SELECT CASE WHEN length(CAST(created_at AS BLOB))<=128 THEN created_at END, 'comment' FROM tracker_comments WHERE id=?1 AND task_id=?2 UNION ALL SELECT CASE WHEN length(CAST(at AS BLOB))<=128 THEN at END, 'event' FROM tracker_events WHERE id=?1 AND task_id=?2 LIMIT 1")?.query_row(params![anchor,task], |row|Ok((row.get(0)?,row.get(1)?))).optional()?;
        let (at, kind) = point.ok_or_else(|| {
            StoreError::TaskSummaryInvalid("unknown timeline_anchor for task".into())
        })?;
        let at = at.ok_or_else(|| {
            StoreError::TaskSummaryInvalid(
                "bounded task timeline ordering key exceeds its byte bound".into(),
            )
        })?;
        let position = Position {
            purpose: "task-timeline-v1".into(),
            incarnation: incarnation.clone(),
            task: task.into(),
            at,
            id: anchor.into(),
            kind,
            inclusive: true,
        };
        self.timeline_window(task, Some(position), limit, incarnation, key)
    }

    fn timeline_window(
        &self,
        task: &str,
        position: Option<Position>,
        limit: usize,
        incarnation: String,
        key: Vec<u8>,
    ) -> Result<TaskTimelinePage, StoreError> {
        let mut rows = Vec::new();
        for (table, at, kind) in [
            ("tracker_comments", "created_at", "comment"),
            ("tracker_events", "at", "event"),
        ] {
            rows.extend(self.timeline_rows(task, table, at, kind, position.as_ref(), limit + 1)?);
        }
        rows.sort_by(|a, b| (&b.0, &b.1, &b.2).cmp(&(&a.0, &a.1, &a.2)));
        let available = rows.len();
        let mut timeline = Vec::new();
        let mut bytes = 2;
        let mut last = None;
        for (at, id, kind, row_bytes, raw) in rows.into_iter().take(limit) {
            if !timeline.is_empty()
                && (row_bytes > ROW_BYTES || bytes + row_bytes + 32 > PAGE_BYTES)
            {
                break;
            }
            let raw = raw.ok_or_else(|| {
                StoreError::TaskSummaryInvalid(
                    "bounded task timeline row exceeds 65536 bytes".into(),
                )
            })?;
            let entry = decode_entry(&raw, kind)?;
            let entry_bytes = serde_json::to_vec(&entry)
                .expect("timeline entries serialize")
                .len();
            if !timeline.is_empty()
                && (entry_bytes > ROW_BYTES || bytes + entry_bytes + 1 > PAGE_BYTES)
            {
                break;
            }
            if entry_bytes > ROW_BYTES {
                return Err(StoreError::TaskSummaryInvalid(
                    "bounded task timeline row exceeds 65536 bytes".into(),
                ));
            }
            bytes += entry_bytes + 1;
            timeline.push(entry);
            last = Some(Position {
                purpose: "task-timeline-v1".into(),
                incarnation: incarnation.clone(),
                task: task.into(),
                at,
                id,
                kind: kind.into(),
                inclusive: false,
            });
        }
        let has_more = available > timeline.len();
        let next_cursor =
            has_more.then(|| encode(&last.expect("a nonempty bounded row fits the page"), &key));
        timeline.reverse();
        let timeline_read_through = self.timeline_read_watermark(task)?;
        let last_reviewed_snapshot_id = self.last_reviewed_snapshot(task)?;
        Ok(TaskTimelinePage {
            timeline,
            has_more,
            next_cursor,
            timeline_read_through,
            timeline_incarnation: incarnation,
            last_reviewed_snapshot_id,
        })
    }

    fn timeline_read_watermark(&self, task: &str) -> Result<Option<String>, StoreError> {
        let watermark: Option<Option<String>> = self.connection().prepare_cached("SELECT CASE WHEN length(CAST(id AS BLOB))<=256 THEN id END FROM tracker_task_summary_facts WHERE task_id=?1 AND unread=1 ORDER BY ulid DESC LIMIT 1")?.query_row([task],|row|row.get(0)).optional()?;
        match watermark {
            Some(Some(id)) => Ok(Some(id)),
            None => Ok(None),
            Some(None) => Err(StoreError::TaskSummaryInvalid(
                "bounded task timeline read watermark exceeds its byte bound".into(),
            )),
        }
    }

    fn last_reviewed_snapshot(&self, task: &str) -> Result<Option<String>, StoreError> {
        let opinion: Option<Option<String>> = self.connection().prepare_cached("SELECT CASE WHEN length(CAST(json_extract(record,'$.opinion.snapshot_id') AS BLOB))<=256 THEN json_extract(record,'$.opinion.snapshot_id') END FROM tracker_comments WHERE task_id=?1 AND CASE WHEN json_valid(record) THEN json_extract(record,'$.author.kind')='user' AND json_type(record,'$.opinion.snapshot_id')='text' ELSE 0 END ORDER BY created_at DESC,id DESC LIMIT 1")?.query_row([task],|row|row.get(0)).optional()?;
        match opinion {
            Some(Some(id)) => Ok(Some(id)),
            Some(None) => Err(StoreError::TaskSummaryInvalid(
                "bounded task review snapshot id exceeds 256 bytes".into(),
            )),
            None => Ok(None),
        }
    }

    fn timeline_rows(
        &self,
        task: &str,
        table: &str,
        at: &str,
        kind: &'static str,
        before: Option<&Position>,
        limit: usize,
    ) -> Result<Vec<Row>, StoreError> {
        let fields = format!("CASE WHEN length(CAST({at} AS BLOB))<=128 THEN {at} END,CASE WHEN length(CAST(id AS BLOB))<=256 THEN id END,length(CAST(record AS BLOB)),CASE WHEN length(CAST(record AS BLOB))<={ROW_BYTES} THEN record END");
        let read = |row: &rusqlite::Row<'_>| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                kind,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        };
        let rows = if let Some(before) = before {
            let comparison = if kind < before.kind.as_str()
                || before.inclusive && kind == before.kind.as_str()
            {
                "<="
            } else {
                "<"
            };
            let sql = format!("SELECT {fields} FROM {table} WHERE task_id=?1 AND ({at},id){comparison}(?2,?3) ORDER BY {at} DESC,id DESC LIMIT ?4");
            self.connection()
                .prepare_cached(&sql)?
                .query_map(params![task, before.at, before.id, limit as i64], read)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            let sql = format!(
                "SELECT {fields} FROM {table} WHERE task_id=?1 ORDER BY {at} DESC,id DESC LIMIT ?2"
            );
            self.connection()
                .prepare_cached(&sql)?
                .query_map(params![task, limit as i64], read)?
                .collect::<Result<Vec<_>, _>>()?
        };
        rows.into_iter()
            .map(|(at, id, kind, bytes, raw)| {
                let oversized = || {
                    StoreError::TaskSummaryInvalid(
                        "bounded task timeline ordering key exceeds its byte bound".into(),
                    )
                };
                Ok((
                    at.ok_or_else(oversized)?,
                    id.ok_or_else(oversized)?,
                    kind,
                    usize::try_from(bytes).map_err(|_| {
                        StoreError::TaskSummaryInvalid(
                            "invalid bounded task timeline row length".into(),
                        )
                    })?,
                    raw,
                ))
            })
            .collect()
    }
}

fn decode_entry(raw: &str, kind: &str) -> Result<TimelineEntry, StoreError> {
    let invalid = || StoreError::TaskSummaryInvalid("invalid bounded task timeline row".into());
    let mut value: Value = serde_json::from_str(raw).map_err(|_| invalid())?;
    value
        .as_object_mut()
        .ok_or_else(invalid)?
        .insert("type".into(), Value::String(kind.into()));
    serde_json::from_value(value).map_err(|_| invalid())
}

fn validate_task(task: &str) -> Result<(), StoreError> {
    if task.is_empty() || task.len() > 256 {
        return Err(StoreError::TaskSummaryInvalid(
            "invalid task timeline selector".into(),
        ));
    }
    Ok(())
}
