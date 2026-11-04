//! Restart-safe accounting upgrade; each committed chunk is bounded.
use crate::store::StoreError;
use rusqlite::{params, Connection};

pub(super) fn install(conn: &mut Connection) -> Result<(), StoreError> {
    let columns = conn
        .prepare("PRAGMA table_info(sync_state)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    for (name, definition) in [
        ("retained_version_bytes", "INTEGER NOT NULL DEFAULT 0"),
        ("projection_pressure_floor", "INTEGER NOT NULL DEFAULT 0"),
        ("gc_projection_seq", "INTEGER NOT NULL DEFAULT 0"),
        ("gc_projection_kind", "TEXT NOT NULL DEFAULT ''"),
        ("gc_projection_key", "TEXT NOT NULL DEFAULT ''"),
        ("retained_bytes_ready", "INTEGER NOT NULL DEFAULT 0"),
        ("retained_backfill_kind", "TEXT NOT NULL DEFAULT ''"),
        ("retained_backfill_key", "TEXT NOT NULL DEFAULT ''"),
        ("retained_backfill_seq", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        if !columns.iter().any(|column| column == name) {
            conn.execute_batch(&format!(
                "ALTER TABLE sync_state ADD COLUMN {name} {definition}"
            ))?;
        }
    }
    conn.execute_batch("CREATE INDEX IF NOT EXISTS sync_projection_tombstones ON sync_projection(from_seq,kind,key) WHERE deleted=1;")?;
    while !account_chunk(conn)? {}
    Ok(())
}

fn account_chunk(conn: &mut Connection) -> Result<bool, StoreError> {
    let tx = conn.transaction()?;
    let (ready,kind,key,seq):(bool,String,String,i64) = tx.query_row(
        "SELECT retained_bytes_ready,retained_backfill_kind,retained_backfill_key,retained_backfill_seq FROM sync_state WHERE id=1",[],
        |row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)))?;
    if ready {
        return Ok(true);
    }
    let rows=tx.prepare("SELECT kind,key,from_seq,COALESCE(length(CAST(record AS BLOB)),0)+length(CAST(kind AS BLOB))+length(CAST(key AS BLOB))+128 FROM sync_projection_versions WHERE (kind,key,from_seq)>(?1,?2,?3) ORDER BY kind,key,from_seq LIMIT 128")?
        .query_map(params![kind,key,seq],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,i64>(2)?,row.get::<_,i64>(3)?)))?.collect::<Result<Vec<_>,_>>()?;
    for (kind, key, seq, bytes) in &rows {
        tx.execute("UPDATE sync_projection_versions SET record_bytes=?4 WHERE kind=?1 AND key=?2 AND from_seq=?3",params![kind,key,seq,bytes])?;
    }
    if let Some((kind, key, seq, _)) = rows.last() {
        let bytes: i64 = rows.iter().map(|row| row.3).sum();
        tx.execute("UPDATE sync_state SET retained_version_bytes=retained_version_bytes+?1,retained_backfill_kind=?2,retained_backfill_key=?3,retained_backfill_seq=?4 WHERE id=1",params![bytes,kind,key,seq])?;
    } else {
        tx.execute("UPDATE sync_state SET retained_bytes_ready=1,projection_pressure_floor=CASE WHEN retained_version_bytes>?1 THEN head ELSE projection_pressure_floor END WHERE id=1",[super::retention::MAX_RETAINED_VERSION_BYTES])?;
    }
    tx.commit()?;
    Ok(rows.is_empty())
}
