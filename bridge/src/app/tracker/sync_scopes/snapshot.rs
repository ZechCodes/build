use super::*;
use crate::store::{
    sync::{Continuation, Sequence},
    TaskQueryPosition,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    watermark: Sequence,
    after: TaskQueryPosition,
}
impl Page {
    fn validate(&self) -> Result<(), SyncError> {
        if self.after.number == 0 || self.after.number > i64::MAX as u64 {
            return Err(SyncError::MalformedCursor);
        }
        Ok(())
    }
}

impl TaskSyncScope {
    pub fn removed_snapshot(
        &self,
        read: &ReadTransaction<'_>,
        page: &str,
        epoch: &str,
        now: i64,
    ) -> Result<Value, SyncError> {
        let position = read.sync_decode_continuation::<Page>(&self.scope, "snapshot", page)?;
        let head = match self.checked_head(read, None, epoch, now)? {
            Ok(head) => head,
            Err(reset) => return Ok(reset),
        };
        let reason = match position {
            Continuation::Foreign(page) => {
                page.validate()?;
                ResyncReason::IncarnationChanged
            }
            Continuation::Current(page) => {
                page.validate()?;
                if page.watermark > head.head {
                    return Err(SyncError::FutureCursor);
                }
                ResyncReason::ScopeRemoved
            }
        };
        Ok(self.resync(&head, reason))
    }

    pub fn snapshot(
        &self,
        read: &ReadTransaction<'_>,
        page: Option<&str>,
        limit: usize,
        epoch: &str,
        now: i64,
    ) -> Result<Value, SyncError> {
        if !(1..=500).contains(&limit) {
            return Err(SyncError::InvalidRequest(
                "snapshot limit must be 1 to 500".into(),
            ));
        }
        if self.task.is_some() && page.is_some() {
            return Err(SyncError::InvalidRequest(
                "task snapshot is a single read".into(),
            ));
        }
        let (watermark, after) = match self.snapshot_position(read, page, epoch, now)? {
            Ok(position) => position,
            Err(resync) => return Ok(resync),
        };
        let cursor = read.sync_cursor(&self.scope, watermark)?;
        let head = match self.checked_head(read, Some(&cursor), epoch, now)? {
            Ok(head) => head,
            Err(reset) => return Ok(reset),
        };
        self.ensure_supported(read)?;
        let mut answer = json!({"status":"ok","scope":self.definition,"incarnation":head.incarnation,"watermark":watermark,"cursor":cursor,"bridge_epoch":epoch,"consistency":if self.task.is_some(){"single_read"}else{"live_keyset"},"records":[]});
        let row_budget = MAX_RECOVERY_BYTES.saturating_sub(encoded_size(&answer)? + 2050);
        let (records, next) = if let Some(id) = &self.task {
            let mut record = self.record(read, id)?;
            record.as_object_mut().expect("record").remove("member");
            (
                if record["row"].is_null() {
                    vec![]
                } else {
                    vec![record]
                },
                None,
            )
        } else {
            self.snapshot_rows(read, watermark, after, limit, row_budget)?
        };
        answer["records"] = json!(records);
        if let Some(next) = next {
            answer["next_page"] = json!(next);
        }
        if encoded_size(&answer)? > MAX_RECOVERY_BYTES {
            return Err(SyncError::Unavailable(
                "snapshot envelope exceeds 1 MiB".into(),
            ));
        }
        Ok(answer)
    }
    fn snapshot_position(
        &self,
        read: &ReadTransaction<'_>,
        page: Option<&str>,
        epoch: &str,
        now: i64,
    ) -> Result<Result<(Sequence, Option<TaskQueryPosition>), Value>, SyncError> {
        let head = match self.checked_head(read, None, epoch, now)? {
            Ok(head) => head,
            Err(reset) => return Ok(Err(reset)),
        };
        let Some(page) = page else {
            return Ok(Ok((head.head, None)));
        };
        match read.sync_decode_continuation::<Page>(&self.scope, "snapshot", page)? {
            Continuation::Foreign(page) => {
                page.validate()?;
                Ok(Err(self.resync(&head, ResyncReason::IncarnationChanged)))
            }
            Continuation::Current(page) => {
                page.validate()?;
                if page.watermark > head.head {
                    return Err(SyncError::FutureCursor);
                }
                Ok(Ok((page.watermark, Some(page.after))))
            }
        }
    }
    fn snapshot_rows(
        &self,
        read: &ReadTransaction<'_>,
        watermark: Sequence,
        after: Option<TaskQueryPosition>,
        limit: usize,
        row_budget: usize,
    ) -> Result<(Vec<Value>, Option<String>), SyncError> {
        // Bounded work even for sparse predicates or a requested 500-row page.
        let candidates = read.list_task_query_candidates(
            &self.project,
            self.query.order,
            after,
            (4 * limit + 1).min(128),
        )?;
        let ids = candidates
            .iter()
            .map(|(candidate, _)| {
                candidate.row["id"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| SyncError::Unavailable("missing task id".into()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut metadata = read.sync_task_metadata(&ids)?;
        if metadata.len() != ids.len() {
            return Err(SyncError::Unavailable(
                "task projection is incomplete".into(),
            ));
        }
        let mut rows = Vec::new();
        let mut last = after;
        let mut bytes = 0;
        for (candidate, position) in candidates {
            if rows.len() == limit {
                break;
            }
            let id = candidate.row["id"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| SyncError::Unavailable("missing task id".into()))?;
            let (pending, revision) = metadata
                .remove(&id)
                .ok_or_else(|| SyncError::Unavailable("task projection is incomplete".into()))?;
            let mut record =
                self.record_candidate(read, &id, &self.project, candidate, pending, revision)?;
            if record["member"] == true {
                record.as_object_mut().expect("record").remove("member");
                let size = encoded_size(&record)?;
                if bytes + size > row_budget {
                    break;
                }
                bytes += size + 1;
                rows.push(record);
            }
            last = Some(position);
        }
        // A full scan window can end without filling a sparse page. Advancing
        // its keyset is valid; its watermark remains the first read's W.
        if let Some(last_position) = last {
            let lookahead = read.list_task_query_candidates(
                &self.project,
                self.query.order,
                Some(last_position),
                1,
            )?;
            if !lookahead.is_empty() {
                return Ok((
                    rows,
                    Some(read.sync_encode_continuation(
                        &self.scope,
                        "snapshot",
                        Page {
                            watermark,
                            after: last_position,
                        },
                    )?),
                ));
            }
        }
        Ok((rows, None))
    }
}
