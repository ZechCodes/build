//! A definition change invalidates only the new durable device facts. Cleanup
//! checkpoints at most 128 source keys per transaction before rebuilding them.
use super::{Entity, PublicationBatch};
use crate::store::StoreError;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

const MARKER: &str = "device_334_cleanup_position";
const CHUNK: i64 = 128;

#[derive(Default, Deserialize, Serialize)]
struct Position {
    phase: u8,
    key: String,
}

/// Called in the schema-definition transaction, before any reader can trust
/// the old completion markers. A normal reopen never calls this function.
pub(super) fn prepare(conn: &Connection) -> Result<(), StoreError> {
    conn.execute("DELETE FROM meta WHERE key IN ('device_owner_facts_334_position','device_sessions_334_position')", [])?;
    conn.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![MARKER, serde_json::to_string(&Position::default()).expect("cleanup position")])?;
    Ok(())
}

pub(super) fn drain(conn: &mut Connection) -> Result<(), StoreError> {
    loop {
        let tx = conn.transaction()?;
        let raw: Option<String> = tx
            .query_row("SELECT value FROM meta WHERE key=?1", [MARKER], |row| {
                row.get(0)
            })
            .optional()?;
        let Some(raw) = raw else {
            return Ok(());
        };
        let mut position: Position =
            serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                path: MARKER.into(),
                source,
            })?;
        let mut batch = PublicationBatch::default();
        if step(&tx, &mut position, &mut batch)? {
            tx.execute("DELETE FROM meta WHERE key=?1", [MARKER])?;
            tx.commit()?;
            return Ok(());
        }
        batch.publish(&tx, super::now_ms())?;
        tx.execute(
            "UPDATE meta SET value=?2 WHERE key=?1",
            params![
                MARKER,
                serde_json::to_string(&position).expect("cleanup position")
            ],
        )?;
        tx.commit()?;
    }
}

fn advance(position: &mut Position) {
    position.phase += 1;
    position.key.clear();
}

fn step(
    tx: &Transaction<'_>,
    position: &mut Position,
    batch: &mut PublicationBatch,
) -> Result<bool, StoreError> {
    let removed = match position.phase {
        0 => tx.execute("DELETE FROM device_owner_pins WHERE owner_id IN (SELECT owner_id FROM device_owner_pins ORDER BY owner_id LIMIT 128)", [])?,
        1 => tx.execute("DELETE FROM device_pooled_sessions WHERE (kind,key) IN (SELECT kind,key FROM device_pooled_sessions ORDER BY kind,key LIMIT 128)", [])?,
        2 => tx.execute("DELETE FROM device_session_replay WHERE (source,source_key,sequence) IN (SELECT source,source_key,sequence FROM device_session_replay ORDER BY source,source_key,sequence LIMIT 128)", [])?,
        3 => tx.execute("DELETE FROM device_session_project_ids WHERE path IN (SELECT path FROM device_session_project_ids ORDER BY path LIMIT 128)", [])?,
        4 | 5 => { projection(tx, position, batch)?; return Ok(false); },
        6 => return Ok(true),
        _ => return Err(super::invalid("invalid device fact cleanup phase")),
    };
    if removed == 0 {
        advance(position);
    }
    Ok(false)
}

fn projection(
    tx: &Transaction<'_>,
    position: &mut Position,
    batch: &mut PublicationBatch,
) -> Result<(), StoreError> {
    let kind = if position.phase == 4 {
        "owner_session"
    } else {
        "project_session"
    };
    // Seek over source keys before filtering tombstones. Even a long deleted
    // prefix advances the persisted cursor after no more than 128 candidates.
    let rows = tx.prepare_cached("SELECT key,deleted FROM sync_projection WHERE kind=?1 AND key>?2 ORDER BY key LIMIT ?3")?
        .query_map(params![kind, position.key, CHUNK], |row| Ok((row.get::<_,String>(0)?, row.get::<_,bool>(1)?)))?
        .collect::<Result<Vec<_>,_>>()?;
    for (key, deleted) in &rows {
        if !deleted {
            super::stage_device(tx, batch, Entity::new(kind, key), None)?;
        }
    }
    if let Some((key, _)) = rows.last() {
        position.key = key.clone();
    } else {
        advance(position);
    }
    Ok(())
}
