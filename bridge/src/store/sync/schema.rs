//! Startup-only installation and independently persisted sync incarnation.
use super::invalid;
use crate::store::StoreError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::BTreeMap;
use std::path::Path;

const DEFINITION: &str = "sync-journal-2";
const TABLES: &str = "
CREATE TABLE IF NOT EXISTS sync_state (
 id INTEGER PRIMARY KEY CHECK(id=1), head INTEGER NOT NULL DEFAULT 0,
 floor INTEGER NOT NULL DEFAULT 0, baseline INTEGER NOT NULL DEFAULT 0,
 total_bytes INTEGER NOT NULL DEFAULT 0, cumulative_bytes INTEGER NOT NULL DEFAULT 0,
 last_at_ms INTEGER NOT NULL DEFAULT 0);
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
CREATE TABLE IF NOT EXISTS sync_projection_keys (
 kind TEXT NOT NULL,key TEXT NOT NULL,first_seq INTEGER NOT NULL,
 PRIMARY KEY(kind,key)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS sync_projection_keyset ON sync_projection_keys(first_seq,kind,key);
CREATE TABLE IF NOT EXISTS sync_projection (
 kind TEXT NOT NULL,key TEXT NOT NULL,from_seq INTEGER NOT NULL,deleted INTEGER NOT NULL,
 record TEXT,record_bytes INTEGER NOT NULL,
 PRIMARY KEY(kind,key)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS sync_projection_versions (
 kind TEXT NOT NULL,key TEXT NOT NULL,from_seq INTEGER NOT NULL,to_seq INTEGER NOT NULL,
 deleted INTEGER NOT NULL,record TEXT,record_bytes INTEGER NOT NULL,
 PRIMARY KEY(kind,key,from_seq),CHECK(to_seq>from_seq)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS sync_projection_versions_gc ON sync_projection_versions(to_seq,kind,key,from_seq);
CREATE TABLE IF NOT EXISTS sync_leases (
 id TEXT PRIMARY KEY,scope_digest TEXT NOT NULL,opening TEXT NOT NULL,incarnation TEXT NOT NULL,
 epoch TEXT NOT NULL,seq INTEGER NOT NULL,expires_at_ms INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS sync_leases_expiry ON sync_leases(expires_at_ms,id);
CREATE INDEX IF NOT EXISTS sync_leases_seq ON sync_leases(seq);
CREATE TABLE IF NOT EXISTS sync_dirty (
 kind TEXT NOT NULL,key TEXT NOT NULL,generation INTEGER NOT NULL,marked_at_ms INTEGER NOT NULL,
 PRIMARY KEY(kind,key)) WITHOUT ROWID;
";

const V1_TABLES: [&str; 5] = [
    "sync_journal",
    "sync_revisions",
    "sync_routes",
    "sync_state",
    "sync_stream_state",
];
const V2_TABLES: [&str; 10] = [
    "sync_dirty",
    "sync_journal",
    "sync_leases",
    "sync_projection",
    "sync_projection_keys",
    "sync_projection_versions",
    "sync_revisions",
    "sync_routes",
    "sync_state",
    "sync_stream_state",
];

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
    let pristine = validate_startup(&tx)?;
    let definition = meta(&tx, "sync_schema")?;
    let previous_binding = meta(&tx, "sync_file_binding")?;
    let rotate = !pristine
        && (restored
            || definition.as_deref() != Some(DEFINITION)
            || previous_binding.as_deref() != Some(&binding));
    tx.execute_batch(TABLES)?;
    if pristine {
        tx.execute("INSERT INTO sync_state(id) VALUES(1)", [])?;
        put(
            &tx,
            "sync_cursor_key",
            &URL_SAFE_NO_PAD.encode(dryoc::rng::randombytes_buf(32)),
        )?;
    }
    if pristine || rotate {
        put(&tx, "sync_incarnation", &uuid::Uuid::new_v4().to_string())?;
        // Existing retained rows are physically reclaimed by ordinary GC.
        // No new cursor can acknowledge history from before this rebuild.
        tx.execute("UPDATE sync_state SET baseline=head,floor=head", [])?;
    }
    put(&tx, "sync_schema", DEFINITION)?;
    put(&tx, "sync_file_binding", &binding)?;
    tx.commit()?;
    super::task_publication::install(conn)?;
    super::device_producers::install(conn)?;
    Ok(())
}

/// Read-only preflight, also run before Store startup can migrate other rows.
/// True means an entirely pristine pre-sync database, never a partial install.
pub(in crate::store) fn validate_startup(conn: &Connection) -> Result<bool, StoreError> {
    inspect(conn)
}

fn incomplete() -> StoreError {
    invalid("sync storage is incomplete; restore an intact backup")
}

fn inspect(conn: &Connection) -> Result<bool, StoreError> {
    let tables = conn.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND name GLOB 'sync_*' ORDER BY name")?
        .query_map([],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    let has_meta: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='meta')",
        [],
        |r| r.get(0),
    )?;
    let metadata: BTreeMap<String, String> = if has_meta {
        conn.prepare("SELECT key,value FROM meta WHERE key GLOB 'sync_*' ORDER BY key")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?
    } else {
        BTreeMap::new()
    };
    if tables.is_empty() && metadata.is_empty() {
        return Ok(true);
    }
    let complete_v1 = tables == V1_TABLES
        && metadata
            .get("sync_schema")
            .is_some_and(|schema| schema == "sync-journal-1");
    let complete_v2 = tables == V2_TABLES;
    if (!complete_v1 && !complete_v2) || metadata.len() != 4 {
        return Err(incomplete());
    }
    for name in [
        "sync_incarnation",
        "sync_cursor_key",
        "sync_schema",
        "sync_file_binding",
    ] {
        if metadata.get(name).is_none_or(|value| value.is_empty()) {
            return Err(incomplete());
        }
    }
    let (incarnation, _) = identity(conn)?;
    if uuid::Uuid::parse_str(&incarnation)
        .ok()
        .map(|id| id.to_string())
        .as_deref()
        != Some(incarnation.as_str())
    {
        return Err(incomplete());
    }
    validate_state(conn)?;
    Ok(false)
}

fn validate_state(conn: &Connection) -> Result<(), StoreError> {
    let rows: i64 = conn.query_row("SELECT COUNT(*) FROM sync_state", [], |r| r.get(0))?;
    if rows != 1 {
        return Err(incomplete());
    }
    let (head,floor,baseline,bytes,cumulative,last_at):(i64,i64,i64,i64,i64,i64) = conn.query_row("SELECT head,floor,baseline,total_bytes,cumulative_bytes,last_at_ms FROM sync_state WHERE id=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
    let journal: i64 =
        conn.query_row("SELECT COALESCE(MAX(seq),0) FROM sync_journal", [], |r| {
            r.get(0)
        })?;
    if floor < 0
        || floor > head
        || baseline < 0
        || baseline > head
        || bytes < 0
        || cumulative < 0
        || last_at < 0
        || head < journal
    {
        return Err(incomplete());
    }
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
