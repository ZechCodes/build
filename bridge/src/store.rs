//! Durable persistence for Issues, under the bridge state dir
//! (`~/.build/tasks/` by default, next to the identity file):
//!
//! ```text
//! build.db                 every record and every conversation
//! issues/<issue_id>/docs/… canonical stage-plan docs
//! ```
//!
//! One SQLite database, and one table in it carries the design: `thread_items`,
//! a row per conversation item. Everything else is a small, bounded record read
//! and written whole, so those rows keep their serde shape in a `record`
//! column. A conversation is the opposite — it grows without bound and is
//! appended to constantly — so appending one message writes one row, and paging
//! is a `LIMIT` rather than a full read.
//!
//! That is what this store replaced: one `record.json` per Issue holding the
//! Issue, every implementation inside it, and every thread on all of them,
//! rewritten whole on every state transition. 593 KB per transition on the
//! largest Issue in the one real installation.
//!
//! Writes are transactional: a record and the conversation rows that belong to
//! it land together or not at all. Everything is read back on boot so a restart
//! re-attaches every plan and run instead of orphaning the worktrees that
//! survived on disk.
//!
//! Plan docs live here as the **source of truth** (spec: Plan/Run Split):
//! the scratch docs dir is disposable, so `ingest_plan_docs` (scratch → store)
//! is fail-fast — a plan never advances with unpersisted docs — and
//! `materialize_plan_docs` (store → worktree) recreates the docs for run
//! dispatch and plan-revision sessions.
//!
//! **Backups have to stop the daemon.** WAL means the state dir holds three
//! live files — `build.db`, `build.db-wal`, `build.db-shm` — and a committed
//! change lives in the `-wal` until a checkpoint folds it back. Anything that
//! copies files one at a time while the bridge runs (Time Machine, Dropbox and
//! iCloud on `~`, `cp -r`, a `tar` of `~/.build`) can catch the database and
//! its log at different instants and produce a copy that will not open, or one
//! missing the last few state transitions. Restoring it looks like a bridge
//! that lost work rather than a bad backup, which is the dangerous part. The
//! answer is to stop the daemon first, so SQLite checkpoints and removes the
//! `-wal` on the last close and the copy is one consistent file — or, when
//! that lands, an explicit backup entry point here using SQLite's own online
//! backup API, which is the only way to take a consistent copy of a live
//! database.
//!
//! Live PTY output streams are intentionally **not** persisted: the terminal is
//! reconstructable observation, not state. What is durable is what the review
//! surfaces need — the lifecycle position and where the files live.

use rusqlite::Connection;
use rusqlite::OptionalExtension;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

mod conversations;
mod documents;
mod entities;
mod legacy;
mod migrations;
mod operations;
mod schema;

#[cfg(test)]
pub use conversations::items_decoded;
pub use conversations::RESIDENT_CONVERSATION_TAIL;
use conversations::{read_thread_page, stored_conversation_summary};
use entities::{migrate_agents_to_v6, write_issue, write_run};
pub use entities::{
    PersistedArchivedWorktree, PersistedIssue, PersistedPlan, PersistedRun, WorktreeFinishAction,
    WorktreeFinishStatus,
};
use legacy::{
    copy_tree, dir_contains_a_file, is_json_record, load_legacy_owner_context,
    remove_dir_if_present, remove_file_if_present, write_record_atomically, STAGE_PLAN_DIR,
};
use operations::ensure_operation_receipt_columns;
use schema::{
    HOISTED_ITEM_COLUMNS, SCHEMA, THREAD_ACTIVITY_COUNT_SQL, THREAD_ACTIVITY_RANGE_SQL,
    THREAD_CONVERSATION_FLOOR_SQL, THREAD_CURSOR_SQL, THREAD_FIRST_ATTENTION_AFTER_SQL,
    THREAD_ITEM_COUNT_SQL, THREAD_LAST_ATTENTION_SQL, THREAD_LAST_MESSAGE_SQL,
    THREAD_LAST_OWN_MESSAGE_SQL, THREAD_LAST_SEQUENCE_SQL, THREAD_MESSAGE_PAGE_SQL,
    THREAD_PAGE_SQL, THREAD_TOOL_CALL_COUNT_SQL,
};

/// Things that can go wrong reading or writing the store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("store io error: {0}")]
    Io(#[from] std::io::Error),
    /// A record file exists but cannot be parsed. Boot fails fast on this — a
    /// silently dropped record would orphan its worktree and lose the user's
    /// work without a trace.
    #[error("store database error: {0}")]
    Db(#[from] rusqlite::Error),
    /// The store directory or its database file could not be opened at all.
    /// Distinct from a bare io/rusqlite error because this is the failure the
    /// daemon dies on, and under a KeepAlive supervisor it dies on it over and
    /// over — so the message has to name the file the user must go look at
    /// rather than leaving them a cause with no location.
    #[error("cannot open the store at {path}: {cause}")]
    Unopenable { path: PathBuf, cause: String },
    /// The database was written by a newer bridge. Opening it read-write would
    /// corrupt state that build does not understand, so boot refuses.
    #[error("store schema version {found} is newer than this bridge supports ({supported}) — update build-bridge")]
    SchemaTooNew { found: i64, supported: i64 },
    /// The JSON records this store imported have been written to since. An
    /// older bridge was run against this directory, and the two halves of the
    /// user's work now live in different places.
    #[error("{path} was written after this store was imported into build.db ({count} record(s) changed) — an older build-bridge has been run against this directory. Work now lives in two places: sort them out before starting, or move the JSON tree aside if the database is the copy you want")]
    RolledBack { path: PathBuf, count: usize },
    #[error("corrupt store record {path}: {source}")]
    Corrupt {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    /// A record file exists but is empty — the signature of a power loss
    /// between a rename becoming durable and the data blocks reaching disk.
    /// Named and actionable (the record held no recoverable data) rather than
    /// a bare JSON-parse error.
    #[error(
        "empty store record {path}: an interrupted write left no data — delete it to boot \
         (its worktree, if any, is untouched on disk)"
    )]
    Empty { path: PathBuf },
    /// An agent-supplied doc path tried to escape its containment dir (the
    /// worktree on ingest, the docs dir on reads). Fail fast: never read or
    /// write through such a path.
    #[error("plan doc path escapes its containment dir: {path}")]
    PathEscape { path: String },
    /// `ingest_plan_docs` found no docs at all. The caller only ingests when a
    /// `done` report claimed docs exist, so an empty ingest means the agent
    /// misreported (or the worktree vanished) — the plan must not advance with
    /// unpersisted docs.
    #[error(
        "no plan docs to ingest for {plan_id}: neither {plan_path} nor .build/plan/ \
         exists under {worktree_path}"
    )]
    NothingToIngest {
        plan_id: String,
        plan_path: String,
        worktree_path: PathBuf,
    },
    /// `materialize_plan_docs` found no canonical docs to copy. Dispatching a
    /// planned run (or a revision session) without its docs would silently
    /// build from nothing — an error, never a no-op.
    #[error("plan {plan_id} has no docs in the store: nothing to materialize")]
    NoStoredDocs { plan_id: String },
    #[error("operation {operation_id} was already used with a different request or target")]
    OperationConflict { operation_id: String },
}

/// The schema this build writes. A stored value ahead of this one means the
/// database was written by a newer bridge; opening it read-write would corrupt
/// what that build knows, so the daemon refuses rather than guessing.
pub const SCHEMA_VERSION: i64 = 7;

/// The database file, inside the store directory beside the docs it does not
/// hold.
const DB_FILE: &str = "build.db";

/// The bridge's record store: one SQLite database for entity state and
/// conversations, plus the directories holding the things that have to be
/// files — canonical plan docs an agent reads and writes in a worktree, and
/// conversation attachments handed to agents by path.
///
/// Cloneable on purpose: the lock-free half of a mutation carries its own
/// handle rather than borrowing the daemon's. Clones share one connection
/// behind a mutex, which is what SQLite wants for a single writer.
#[derive(Clone)]
pub struct Store {
    dir: PathBuf,
    conn: Arc<Mutex<Connection>>,
    /// Test-only: make the next write fail.
    ///
    /// Several contracts are about what happens when the store REFUSES —
    /// a worktree is not removed until its archive record is durable, most
    /// of all. The JSON store let a test force that by putting a file where
    /// a directory belonged; a database has no such accident to stage, so
    /// the refusal is injected here instead of simulated.
    #[cfg(test)]
    fail_next_write: Arc<std::sync::atomic::AtomicBool>,
}

impl Store {
    /// Open (creating if absent) the store under `dir`.
    ///
    /// Fails rather than degrading: a store that cannot be opened is a daemon
    /// that would silently orphan every worktree it cannot see.
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self, StoreError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).map_err(|cause| StoreError::Unopenable {
            path: dir.clone(),
            cause: cause.to_string(),
        })?;
        let database_path = dir.join(DB_FILE);
        let mut conn =
            Connection::open(&database_path).map_err(|cause| StoreError::Unopenable {
                path: database_path,
                cause: cause.to_string(),
            })?;
        // Refuse BEFORE writing anything. A store written by a newer bridge
        // must not receive this build's pragmas or DDL on the way to being
        // rejected — the refusal exists to leave it untouched.
        let stored: Option<i64> = conn
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap_or(None);
        if let Some(found) = stored {
            if found > SCHEMA_VERSION {
                return Err(StoreError::SchemaTooNew {
                    found,
                    supported: SCHEMA_VERSION,
                });
            }
        }
        // WAL is what makes an append cheap: the reader keeps reading while the
        // writer commits, and a commit appends to the log instead of rewriting
        // the page it touched. `synchronous = NORMAL` under WAL fsyncs at
        // checkpoint rather than per commit — durable against process death,
        // which is the failure the old tmp+rename+fsync was guarding, and it
        // gives up only the very last commits to a power cut.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // A contended write waits instead of failing outright. Clones of this
        // Store share one connection, but the daemon is not the only process
        // that may ever open the file (a backup, a shell).
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // The columns BEFORE the schema batch: `SCHEMA` indexes them, and an
        // older table has no such column for an index to name. A v1 database
        // arrives here needing all five, and reaches the current version in
        // one open.
        for (arrived_in, column) in HOISTED_ITEM_COLUMNS {
            if stored.is_some_and(|found| found < arrived_in) {
                Store::add_hoisted_column(&conn, column)?;
            }
        }
        conn.execute_batch(SCHEMA)?;
        ensure_operation_receipt_columns(&conn)?;
        if stored.is_some_and(|found| found < SCHEMA_VERSION) {
            Store::classify_stored_items(&conn)?;
        }
        if stored.unwrap_or(0) < 6 {
            migrate_agents_to_v6(&mut conn)?;
        }
        if stored.unwrap_or(0) < SCHEMA_VERSION {
            conn.execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = ?1",
                [SCHEMA_VERSION.to_string()],
            )?;
        }
        Ok(Store {
            dir,
            conn: Arc::new(Mutex::new(conn)),
            #[cfg(test)]
            fail_next_write: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }
    /// The connection, whatever an earlier panic left behind.
    ///
    /// A poisoned mutex says only that some call panicked while it held the
    /// guard — never that the connection is torn. SQLite rolls an uncommitted
    /// transaction back when its handle drops, so the database is already at
    /// its last commit by the time the guard is released, and the next caller
    /// finds exactly the state a clean failure would have left. Treating the
    /// poison as fatal would turn one panic into a permanent one: every later
    /// store call would panic too, and the daemon would sit there up and
    /// connected while it could neither read nor write state.
    fn connection(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    /// Run `work` inside one transaction. Every save is all-or-nothing: a
    /// record and the conversation rows that belong to it land together or not
    /// at all, which is the guarantee the old tmp-file-and-rename bought one
    /// file at a time.
    fn in_transaction<T>(
        &self,
        work: impl FnOnce(&rusqlite::Transaction) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        #[cfg(test)]
        if self
            .fail_next_write
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            return Err(StoreError::Io(std::io::Error::other(
                "injected store failure",
            )));
        }
        let mut conn = self.connection();
        // IMMEDIATE, not the default DEFERRED: every caller here writes, and
        // several read first. A deferred transaction takes its write lock on
        // the first write, and a failed upgrade raises SQLITE_BUSY_SNAPSHOT,
        // which SQLite does NOT route through the busy handler — so the
        // timeout above would not cover it.
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let out = work(&tx)?;
        tx.commit()?;
        Ok(out)
    }
}

/// [`write_record_atomically`] for a file that is not a store record but lives
/// in the user's own checkout — `.build/review-rules.json`, the repository's
/// `.git/info/exclude` — and wants exactly the same guarantee, for exactly the
/// same reason: a half-written file is one a human has to repair. Safe for
/// writers running at once: each stages its own sibling, so a reader sees the
/// old file whole or the new one whole, never a prefix of either.
pub(crate) fn write_file_atomically(path: &Path, contents: &str) -> Result<(), StoreError> {
    write_record_atomically(path, contents)
}

/// The current time as an RFC 3339 UTC string (the store's timestamp format).
pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("UTC now formats as RFC 3339")
}

#[cfg(test)]
mod tests;
