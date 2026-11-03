//! Collect only final committed summary facts; no partial/streaming item input.
use super::{invalid, retention, Entity, Event, Operation, Sequence, Stream, StreamKind};
use crate::store::{StoreError, StoreWriter};
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const MAX_ROUTES: usize = 4;

struct Publication {
    record: Option<Value>,
    streams: BTreeSet<Stream>,
}

/// One accumulator per canonical transaction. Repeated changes replace the
/// pending value; publication compares the final visible digest with the last
/// committed value. Partial/streaming transcript bytes must never be staged.
#[derive(Default)]
pub struct PublicationBatch(BTreeMap<Entity, Publication>);

impl PublicationBatch {
    pub fn upsert(&mut self, entity: Entity, visible: Value, streams: &[Stream]) {
        self.0.insert(
            entity,
            Publication {
                record: Some(visible),
                streams: streams.iter().cloned().collect(),
            },
        );
    }
    pub fn remove(&mut self, entity: Entity, streams: &[Stream]) {
        self.0.insert(
            entity,
            Publication {
                record: None,
                streams: streams.iter().cloned().collect(),
            },
        );
    }

    /// Low-level store hook. Production owners use sync_transaction to flush
    /// exactly once, after all canonical and derived SQL has succeeded.
    pub(in crate::store) fn publish(
        &self,
        tx: &Transaction<'_>,
        now: i64,
    ) -> Result<Vec<Event>, StoreError> {
        let (mut cumulative, mut at): (i64, i64) = tx.query_row(
            "SELECT cumulative_bytes,last_at_ms FROM sync_state WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        at = at.max(now);
        let mut events = Vec::new();
        let mut added_bytes = 0_i64;
        for (entity, publication) in &self.0 {
            if let Some((event, bytes)) = publish_one(tx, entity, publication, at, cumulative)? {
                cumulative = cumulative
                    .checked_add(bytes)
                    .ok_or_else(|| invalid("byte counter exhausted"))?;
                added_bytes += bytes;
                events.push(event);
            }
        }
        if let Some(last) = events.last() {
            tx.execute("UPDATE sync_state SET head=?1,cumulative_bytes=?2,total_bytes=total_bytes+?3,last_at_ms=?4 WHERE id=1",params![last.seq.get(),cumulative,added_bytes,at])?;
            let floor = retention::logical_floor(tx, now, retention::Policy::DEFAULT)?;
            tx.execute(
                "UPDATE sync_state SET floor=?1 WHERE id=1 AND floor<?1",
                [floor],
            )?;
        }
        Ok(events)
    }
}

impl StoreWriter {
    /// Commit state and its deduplicated summary publications together. The
    /// accumulator cannot escape the transaction, and no success is returned
    /// if a source write, append or commit fails. No async/ack boundary is added.
    pub fn sync_transaction<T>(
        &self,
        work: impl FnOnce(&Transaction<'_>, &mut PublicationBatch) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        self.transaction(|tx| {
            let mut publications = PublicationBatch::default();
            let result = work(tx, &mut publications)?;
            publications.publish(tx, super::now_ms())?;
            Ok(result)
        })
    }
}

fn publish_one(
    tx: &Transaction<'_>,
    entity: &Entity,
    publication: &Publication,
    at: i64,
    cumulative: i64,
) -> Result<Option<(Event, i64)>, StoreError> {
    validate(entity, &publication.streams)?;
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(
            publication
                .record
                .as_ref()
                .map(Value::to_string)
                .unwrap_or_default()
                .as_bytes()
        )
    );
    let deleted = publication.record.is_none();
    let previous: Option<(bool,String,String)> = tx.prepare_cached("SELECT deleted,fingerprint,streams FROM sync_revisions WHERE entity_kind=?1 AND entity_key=?2")?.query_row(params![entity.kind,entity.key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let old_streams: BTreeSet<Stream> = previous
        .as_ref()
        .map(|(_, _, raw)| {
            serde_json::from_str(raw).map_err(|_| invalid("corrupt revision routes"))
        })
        .transpose()?
        .unwrap_or_default();
    let current_streams = if deleted && publication.streams.is_empty() {
        &old_streams
    } else {
        &publication.streams
    };
    if previous
        .as_ref()
        .is_some_and(|(old_deleted, old_fingerprint, _)| {
            *old_deleted == deleted
                && *old_fingerprint == fingerprint
                && old_streams == *current_streams
        })
    {
        return Ok(None);
    }
    let routes: BTreeSet<_> = old_streams.union(current_streams).cloned().collect();
    validate(entity, &routes)?;
    let operation = if deleted {
        Operation::Delete
    } else {
        Operation::Upsert
    };
    let bytes = serde_json::to_vec(&(entity, &operation, &routes))
        .expect("bounded metadata serializes")
        .len() as i64
        + 64;
    let next = cumulative
        .checked_add(bytes)
        .ok_or_else(|| invalid("byte counter exhausted"))?;
    let seq: i64 = tx.prepare_cached("INSERT INTO sync_journal(at_ms,entity_kind,entity_key,operation,record_bytes,cumulative_bytes) VALUES(?1,?2,?3,?4,?5,?6) RETURNING seq")?.query_row(params![at,entity.kind,entity.key,operation.as_str(),bytes,next],|r|r.get(0))?;
    write_routes(tx, &routes, seq)?;
    let streams = serde_json::to_string(current_streams).expect("routes serialize");
    tx.prepare_cached("INSERT INTO sync_revisions(entity_kind,entity_key,seq,deleted,fingerprint,streams) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(entity_kind,entity_key) DO UPDATE SET seq=excluded.seq,deleted=excluded.deleted,fingerprint=excluded.fingerprint,streams=excluded.streams")?.execute(params![entity.kind,entity.key,seq,deleted,fingerprint,streams])?;
    Ok(Some((
        Event {
            seq: Sequence::new(seq)?,
            entity: entity.clone(),
            operation,
        },
        bytes,
    )))
}

fn validate(entity: &Entity, streams: &BTreeSet<Stream>) -> Result<(), StoreError> {
    if entity.kind.is_empty()
        || entity.kind.len() > 64
        || entity.key.is_empty()
        || entity.key.len() > 4096
        || streams.len() > MAX_ROUTES
    {
        return Err(invalid("publication identity or route budget exceeded"));
    }
    for stream in streams {
        if stream.key.len() > 4096 || (stream.key.is_empty() != (stream.kind == StreamKind::Device))
        {
            return Err(invalid("invalid publication stream"));
        }
    }
    Ok(())
}

fn write_routes(
    tx: &Transaction<'_>,
    routes: &BTreeSet<Stream>,
    seq: i64,
) -> Result<(), StoreError> {
    for route in routes {
        tx.prepare_cached("INSERT INTO sync_routes(stream_kind,stream_key,seq) VALUES(?1,?2,?3)")?
            .execute(params![route.kind.as_str(), route.key, seq])?;
        tx.prepare_cached("INSERT INTO sync_stream_state(stream_kind,stream_key,head) VALUES(?1,?2,?3) ON CONFLICT(stream_kind,stream_key) DO UPDATE SET head=excluded.head")?.execute(params![route.kind.as_str(),route.key,seq])?;
    }
    Ok(())
}
