//! Startup-only installation and independently persisted sync incarnation.
use super::invalid;
use crate::store::StoreError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

const DEFINITION: &str = "sync-journal-1";
const TABLES: &str = "
CREATE TABLE IF NOT EXISTS sync_state (
 id INTEGER PRIMARY KEY CHECK(id=1), head INTEGER NOT NULL DEFAULT 0,
 floor INTEGER NOT NULL DEFAULT 0, baseline INTEGER NOT NULL DEFAULT 0,
 total_bytes INTEGER NOT NULL DEFAULT 0, cumulative_bytes INTEGER NOT NULL DEFAULT 0,
 last_at_ms INTEGER NOT NULL DEFAULT 0);
INSERT OR IGNORE INTO sync_state(id) VALUES(1);
CREATE TABLE IF NOT EXISTS sync_journal (
 seq INTEGER PRIMARY KEY AUTOINCREMENT,at_ms INTEGER NOT NULL,
 entity_kind TEXT NOT NULL,entity_key TEXT NOT NULL,operation TEXT NOT NULL,
 record_bytes INTEGER NOT NULL,cumulative_bytes INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS sync_journal_time ON sync_journal(at_ms,seq);
CREATE INDEX IF NOT EXISTS sync_journal_bytes ON sync_journal(cumulative_bytes,seq);
CREATE TABLE IF NOT EXISTS sync_stream_state (
 stream_kind TEXT NOT NULL,stream_key TEXT NOT NULL,head INTEGER NOT NULL,floor INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(stream_kind,stream_key)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS sync_routes (
 stream_kind TEXT NOT NULL,stream_key TEXT NOT NULL,seq INTEGER NOT NULL,
 PRIMARY KEY(stream_kind,stream_key,seq)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS sync_routes_gc ON sync_routes(seq);
CREATE TABLE IF NOT EXISTS sync_revisions (
 entity_kind TEXT NOT NULL,entity_key TEXT NOT NULL,seq INTEGER NOT NULL,
 deleted INTEGER NOT NULL,fingerprint TEXT NOT NULL,streams TEXT NOT NULL,
 PRIMARY KEY(entity_kind,entity_key)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS sync_deleted_revisions ON sync_revisions(seq) WHERE deleted=1;
";

fn meta(conn: &Connection, key: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key=?1", [key], |r| r.get(0))
        .optional()?)
}
fn put(conn: &Connection, key: &str, value: &str) -> Result<(), StoreError> {
    conn.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value])?;
    Ok(())
}

pub(in crate::store) fn initialize(
    conn: &mut Connection,
    path: &Path,
    restored: bool,
) -> Result<(), StoreError> {
    let binding = file_binding(path)?;
    let tx = conn.transaction()?;
    let incarnation = meta(&tx, "sync_incarnation")?;
    let definition = meta(&tx, "sync_schema")?;
    let previous_binding = meta(&tx, "sync_file_binding")?;
    let existing: i64 = tx.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE type='table' AND name IN ('sync_state','sync_journal','sync_stream_state','sync_routes','sync_revisions')",[],|r|r.get(0))?;
    if incarnation.is_some() && existing != 5 {
        return Err(invalid(
            "sync storage is incomplete; restore an intact backup",
        ));
    }
    let rotate = incarnation.is_some()
        && (restored
            || definition.as_deref() != Some(DEFINITION)
            || previous_binding.as_deref() != Some(&binding));
    tx.execute_batch(TABLES)?;
    if incarnation.is_none() || rotate {
        put(&tx, "sync_incarnation", &uuid::Uuid::new_v4().to_string())?;
        // Existing retained rows are physically reclaimed by ordinary GC.
        // No new cursor can acknowledge history from before this rebuild.
        tx.execute("UPDATE sync_state SET baseline=head,floor=head", [])?;
    }
    if meta(&tx, "sync_cursor_key")?.is_none() {
        put(
            &tx,
            "sync_cursor_key",
            &URL_SAFE_NO_PAD.encode(dryoc::rng::randombytes_buf(32)),
        )?;
    }
    put(&tx, "sync_schema", DEFINITION)?;
    put(&tx, "sync_file_binding", &binding)?;
    tx.commit()?;
    Ok(())
}

#[cfg(unix)]
fn file_binding(path: &Path) -> Result<String, StoreError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(path)?;
    Ok(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
}
#[cfg(not(unix))]
fn file_binding(path: &Path) -> Result<String, StoreError> {
    Ok(format!("path:{}", std::fs::canonicalize(path)?.display()))
}

pub(super) fn identity(conn: &Connection) -> Result<(String, Vec<u8>), StoreError> {
    let incarnation =
        meta(conn, "sync_incarnation")?.ok_or_else(|| invalid("missing incarnation"))?;
    let key = meta(conn, "sync_cursor_key")?.ok_or_else(|| invalid("missing cursor key"))?;
    let key = URL_SAFE_NO_PAD
        .decode(key)
        .map_err(|_| invalid("invalid cursor key"))?;
    if key.len() != 32 {
        return Err(invalid("invalid cursor key"));
    }
    Ok((incarnation, key))
}
