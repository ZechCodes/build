//! Indexed recovery primitives. Call only inside the request's read snapshot.
use super::{
    cursor, schema, Entity, Event, Operation, Scope, Sequence, SyncError, MAX_RECOVERY_EVENTS,
};
use crate::store::{ReadTransaction, TaskSummaryCandidate};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use std::collections::BTreeMap;

pub enum CursorSequence {
    Current(Sequence),
    Foreign,
}

impl ReadTransaction<'_> {
    pub fn task_project_key(&self, id: &str) -> Result<Option<String>, crate::store::StoreError> {
        Ok(self
            .connection()
            .query_row(
                "SELECT project_key FROM tracker_task_summaries WHERE task_id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn sync_cursor_sequence(
        &self,
        scope: &Scope,
        token: &str,
    ) -> Result<CursorSequence, SyncError> {
        let (incarnation, key) = schema::identity(self.connection())?;
        match cursor::decode(token, &incarnation, &key, scope)? {
            cursor::Position::Retained(seq) => Ok(CursorSequence::Current(seq)),
            cursor::Position::Foreign => Ok(CursorSequence::Foreign),
        }
    }

    pub fn sync_sequence(&self, scope: &Scope, token: &str) -> Result<Sequence, SyncError> {
        match self.sync_cursor_sequence(scope, token)? {
            CursorSequence::Current(seq) => Ok(seq),
            CursorSequence::Foreign => Err(SyncError::MalformedCursor),
        }
    }
    pub fn sync_revision(&self, entity: &Entity) -> Result<Sequence, SyncError> {
        let seq:i64=self.connection().query_row("SELECT MAX(COALESCE((SELECT seq FROM sync_revisions WHERE entity_kind=?1 AND entity_key=?2),0),baseline) FROM sync_state WHERE id=1",params![entity.kind,entity.key],|r|r.get(0))?;
        Ok(Sequence::new(seq)?)
    }
    /// At most 501 entries prove the 500-event catch-up limit without scanning
    /// an unbounded backlog. Routing is before filter membership evaluation.
    pub fn sync_events(
        &self,
        scope: &Scope,
        after: Sequence,
        head: Sequence,
        limit: usize,
    ) -> Result<Vec<Event>, SyncError> {
        let stream = scope.stream();
        let rows=self.connection().prepare("SELECT j.seq,j.entity_kind,j.entity_key,j.operation FROM sync_routes r JOIN sync_journal j ON j.seq=r.seq WHERE r.stream_kind=?1 AND r.stream_key=?2 AND r.seq>?3 AND r.seq<=?4 ORDER BY r.seq LIMIT ?5")?
            .query_map(params![stream.kind.as_str(),stream.key,after.get(),head.get(),limit.min(MAX_RECOVERY_EVENTS+1) as i64],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?.collect::<Result<Vec<_>,_>>()?;
        rows.into_iter()
            .map(|(seq, kind, key, operation)| {
                Ok(Event {
                    seq: Sequence::new(seq)?,
                    entity: Entity::new(&kind, &key),
                    operation: match operation.as_str() {
                        "upsert" => Operation::Upsert,
                        "delete" => Operation::Delete,
                        _ => {
                            return Err(SyncError::Unavailable("invalid journal operation".into()))
                        }
                    },
                })
            })
            .collect()
    }
    pub fn sync_task_summary(
        &self,
        id: &str,
    ) -> Result<Option<(String, TaskSummaryCandidate, bool, Sequence)>, SyncError> {
        let point: Option<(String, i64, bool, i64)> = self
            .connection()
            .query_row(
                "SELECT s.project_key,s.number,q.pending_present,MAX(COALESCE(r.seq,0),st.baseline)
             FROM tracker_task_summaries s
             JOIN tracker_task_query_sources q ON q.task_id=s.task_id
             LEFT JOIN sync_revisions r ON r.entity_kind='task' AND r.entity_key=s.task_id
             JOIN sync_state st ON st.id=1 WHERE s.task_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        if point.is_none() {
            let exists: bool = self.connection().query_row(
                "SELECT EXISTS(SELECT 1 FROM tracker_tasks WHERE id=?1)",
                [id],
                |r| r.get(0),
            )?;
            if exists {
                return Err(SyncError::Unavailable(
                    "task projection is incomplete".into(),
                ));
            }
        }
        point
            .map(|(project, number, pending, revision)| {
                let candidate = self
                    .task_summary_by_number(&project, number as u64)?
                    .ok_or_else(|| {
                        SyncError::Unavailable("task projection is incomplete".into())
                    })?;
                Ok((project, candidate, pending, Sequence::new(revision)?))
            })
            .transpose()
    }
    pub fn sync_task_metadata(
        &self,
        ids: &[String],
    ) -> Result<BTreeMap<String, (bool, Sequence)>, SyncError> {
        if ids.is_empty() {
            return Ok(BTreeMap::new());
        }
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT q.task_id,q.pending_present,MAX(COALESCE(r.seq,0),st.baseline)
             FROM tracker_task_query_sources q
             LEFT JOIN sync_revisions r ON r.entity_kind='task' AND r.entity_key=q.task_id
             JOIN sync_state st ON st.id=1 WHERE q.task_id IN ({placeholders})"
        );
        let rows = self
            .connection()
            .prepare(&sql)?
            .query_map(rusqlite::params_from_iter(ids), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(id, pending, revision)| Ok((id, (pending, Sequence::new(revision)?))))
            .collect()
    }
    pub fn sync_task_anchor_ms(&self, id: &str) -> Result<Option<i64>, SyncError> {
        let anchor: i64 = self.connection().query_row(
            "SELECT anchor_ms FROM tracker_task_query_facts WHERE task_id=?1",
            [id],
            |r| r.get(0),
        )?;
        Ok((anchor != i64::MAX).then_some(anchor))
    }
    /// Two indexed newest-head seeks, not an all-history UNION sort or decode.
    /// Oversized necessary rows fail explicitly before their bytes are loaded.
    pub fn sync_task_timeline_head(&self, id: &str) -> Result<(Vec<Value>, bool), SyncError> {
        let mut entries = Vec::new();
        for (table, at, kind) in [
            ("tracker_comments", "created_at", "comment"),
            ("tracker_events", "at", "event"),
        ] {
            let sql=format!("SELECT {at},id,CASE WHEN length(CAST(record AS BLOB))<=65536 THEN record END FROM {table} WHERE task_id=?1 ORDER BY {at} DESC,id DESC LIMIT 51");
            let rows = self
                .connection()
                .prepare(&sql)?
                .query_map([id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            for (at, id, raw) in rows {
                entries.push((at, id, kind, raw));
            }
        }
        entries.sort_by(|a, b| (&b.0, &b.1).cmp(&(&a.0, &a.1)));
        let more = entries.len() > 50;
        let mut timeline = entries
            .into_iter()
            .take(50)
            .map(|(_, _, kind, raw)| timeline_row(kind, raw))
            .collect::<Result<Vec<_>, _>>()?;
        timeline.reverse();
        Ok((timeline, more))
    }
}

fn timeline_row(kind: &str, raw: Option<String>) -> Result<Value, SyncError> {
    let raw =
        raw.ok_or_else(|| SyncError::Unavailable("timeline head row exceeds 65536 bytes".into()))?;
    let mut value: Value = serde_json::from_str(&raw)
        .map_err(|_| SyncError::Unavailable("invalid timeline head row".into()))?;
    value
        .as_object_mut()
        .ok_or_else(|| SyncError::Unavailable("invalid timeline head row".into()))?
        .insert("type".into(), Value::String(kind.into()));
    let entry: crate::tracker::TimelineEntry = serde_json::from_value(value)
        .map_err(|_| SyncError::Unavailable("invalid timeline head row".into()))?;
    serde_json::to_value(entry)
        .map_err(|_| SyncError::Unavailable("invalid timeline head row".into()))
}
