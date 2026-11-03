use super::*;
use crate::store::sync::{Continuation, CursorSequence, Sequence};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Run {
    origin: Sequence,
    after: Sequence,
    head: Sequence,
    events: u16,
    bytes: String,
}
impl Run {
    fn bytes(&self) -> Result<usize, SyncError> {
        if self.bytes.len() != 7 || !self.bytes.bytes().all(|b| b.is_ascii_digit()) {
            return Err(SyncError::MalformedCursor);
        }
        let bytes = self
            .bytes
            .parse::<usize>()
            .map_err(|_| SyncError::MalformedCursor)?;
        if bytes > MAX_RECOVERY_BYTES
            || self.events as usize > MAX_RECOVERY_EVENTS
            || self.after < self.origin
            || self.head < self.after
        {
            return Err(SyncError::MalformedCursor);
        }
        Ok(bytes)
    }
}
impl TaskSyncScope {
    pub fn changes(
        &self,
        read: &ReadTransaction<'_>,
        cursor: &str,
        continuation: Option<&str>,
        limit: usize,
        epoch: &str,
        now: i64,
    ) -> Result<Value, SyncError> {
        if !(1..=100).contains(&limit) {
            return Err(SyncError::InvalidRequest(
                "changes limit must be 1 to 100".into(),
            ));
        }
        let head = match self.checked_head(read, None, epoch, now)? {
            Ok(head) => head,
            Err(reset) => return Ok(reset),
        };
        let origin = match read.sync_cursor_sequence(&self.scope, cursor)? {
            CursorSequence::Current(origin) => origin,
            CursorSequence::Foreign => {
                return Ok(self.resync(&head, ResyncReason::IncarnationChanged))
            }
        };
        if origin > head.head {
            return Err(SyncError::FutureCursor);
        }
        self.ensure_supported(read)?;
        let run = match self.run_position(read, origin, continuation, &head)? {
            Ok(run) => run,
            Err(reset) => return Ok(reset),
        };
        if run.after < head.retention_floor {
            return Ok(self.resync(&head, ResyncReason::RetentionExpired));
        }
        let bytes = run.bytes()?;
        let events = read.sync_events(&self.scope, run.after, run.head, MAX_RECOVERY_EVENTS + 1)?;
        if run.events as usize + events.len() > MAX_RECOVERY_EVENTS {
            return Ok(self.resync(&head, ResyncReason::OverBudget));
        }
        let more = events.len() > limit;
        let batch = &events[..events.len().min(limit)];
        let Some(changes) = self.change_records(read, batch, MAX_RECOVERY_BYTES - bytes)? else {
            return Ok(self.resync(&head, ResyncReason::OverBudget));
        };
        let after = if more {
            batch.last().expect("nonempty batch").seq
        } else {
            run.head
        };
        let mut answer = json!({"status":"ok","scope":self.definition,"incarnation":head.incarnation,"head":run.head,"cursor":read.sync_cursor(&self.scope,after)?,"bridge_epoch":epoch,"changes":changes});
        if more {
            let next = Run {
                origin,
                after,
                head: run.head,
                events: run.events + answer["changes"].as_array().expect("changes").len() as u16,
                bytes: format!("{bytes:07}"),
            };
            answer["continuation"] =
                json!(read.sync_encode_continuation(&self.scope, "changes", &next)?);
            let total = bytes + encoded_size(&answer)?;
            if total > MAX_RECOVERY_BYTES {
                return Ok(self.resync(&head, ResyncReason::OverBudget));
            }
            // Fixed-width accounting leaves serialized token/envelope lengths
            // unchanged: the budget includes every returned envelope byte.
            let next = Run {
                bytes: format!("{total:07}"),
                ..next
            };
            answer["continuation"] =
                json!(read.sync_encode_continuation(&self.scope, "changes", next)?);
        } else if bytes + encoded_size(&answer)? > MAX_RECOVERY_BYTES {
            return Ok(self.resync(&head, ResyncReason::OverBudget));
        }
        Ok(answer)
    }
    fn change_records(
        &self,
        read: &ReadTransaction<'_>,
        events: &[crate::store::sync::Event],
        budget: usize,
    ) -> Result<Option<Vec<Value>>, SyncError> {
        let mut records = Vec::new();
        let mut cache = BTreeMap::new();
        let mut bytes = 0;
        for event in events {
            if event.entity.kind != "task" {
                return Err(SyncError::Unavailable(
                    "unexpected task recovery entity".into(),
                ));
            }
            // One current-row read per entity within this request, with no
            // retained replay state. Repeated task events cannot repeatedly
            // decode the same bounded timeline head.
            let base = match cache.entry(event.entity.key.clone()) {
                std::collections::btree_map::Entry::Occupied(row) => row.into_mut(),
                std::collections::btree_map::Entry::Vacant(row) => {
                    row.insert(self.record(read, &event.entity.key)?)
                }
            };
            let mut record = base.clone();
            record["seq"] = json!(event.seq);
            record["operation"] = json!(event.operation);
            bytes += encoded_size(&record)?;
            if bytes > budget {
                return Ok(None);
            }
            records.push(record);
        }
        Ok(Some(records))
    }
    fn run_position(
        &self,
        read: &ReadTransaction<'_>,
        origin: Sequence,
        token: Option<&str>,
        head: &Head,
    ) -> Result<Result<Run, Value>, SyncError> {
        let Some(token) = token else {
            return Ok(Ok(Run {
                origin,
                after: origin,
                head: head.head,
                events: 0,
                bytes: "0000000".into(),
            }));
        };
        match read.sync_decode_continuation::<Run>(&self.scope, "changes", token)? {
            Continuation::Foreign(run) => {
                run.bytes()?;
                Ok(Err(self.resync(head, ResyncReason::IncarnationChanged)))
            }
            Continuation::Current(run) => {
                run.bytes()?;
                if run.origin != origin {
                    return Err(SyncError::MalformedCursor);
                }
                if run.head > head.head {
                    return Err(SyncError::FutureCursor);
                }
                Ok(Ok(run))
            }
        }
    }
}
