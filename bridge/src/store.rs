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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::agent::{Agent, AgentRoster};
use crate::attention::Attention;
use crate::models::{AgentProvider, ModelChoice};
use crate::plan::{is_worktree_contained_path, PlanState, StageDoc};
use crate::run::{RunState, StageProgress};
use crate::thread::{Thread, ThreadItem};

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
}

/// The durable core of one plan — the project-scoped half of the split. Its
/// canonical docs live beside the record under `plans/<plan_id>/docs/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedPlan {
    pub id: String,
    pub goal: String,
    /// Canonical path of the project repo the plan belongs to. Stored as a
    /// path (not the in-memory project id) because project ids are re-minted
    /// on every boot.
    pub project_path: String,
    pub base_branch: String,
    pub state: PlanState,
    /// Durable archival metadata. This is deliberately not a `PlanState` arm:
    /// lifecycle and filing are independent, and canonical docs remain live.
    #[serde(default)]
    pub archived_at: Option<String>,
    #[serde(default)]
    pub implementation_intent: crate::plan::ImplementationIntent,
    #[serde(default)]
    pub implementation_activity: crate::plan::ImplementationActivity,
    /// Docs-dir-relative path of the single plan doc (`.build/plan.md`).
    pub plan_path: String,
    /// Stage docs: manifest metadata + plan-side review sub-state. Empty for
    /// single-doc plans.
    #[serde(default)]
    pub stages: Vec<StageDoc>,
    #[serde(default)]
    pub provider: AgentProvider,
    /// Model/effort the plan's agents run on (None = harness default).
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// This entity's agents, each owning its own conversation. Empty only on a
    /// record written before agents existed. `AgentRoster::restore` fills it
    /// from `legacy_thread` on reattach.
    #[serde(default)]
    pub agents: Vec<Agent>,
    /// The pre-agent, entity-keyed conversation. Retained so the JSON records
    /// the one-off import reads still deserialize; empty on everything written
    /// since, and never written by this store.
    #[serde(rename = "thread", default, skip_serializing_if = "Thread::is_empty")]
    pub legacy_thread: Thread,
    pub last_summary: Option<String>,
    /// The most recent thing that went wrong for this plan, shown on the card
    /// until the plan advances again.
    #[serde(default)]
    pub last_error: Option<String>,
    /// RFC 3339 UTC timestamps.
    pub created_at: String,
    pub updated_at: String,
    /// When the plan last changed *state* (vs `updated_at`, any mutation).
    /// `None` on records from before the field existed — restore falls back
    /// to `updated_at`.
    #[serde(default)]
    pub state_changed_at: Option<String>,
}

/// The durable core of one run — the worktree-scoped half of the split. A run
/// stores `plan_id` instead of plan docs; only an adopted (or v1-migrated) run
/// carries `plan_id: None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedRun {
    pub id: String,
    /// The plan this run implements. `None` = an adopted (or v1-migrated) run:
    /// no plan behind it, so no plan gate.
    #[serde(default)]
    pub plan_id: Option<String>,
    pub goal: String,
    /// Canonical path of the project repo (same rationale as on the plan).
    pub project_path: String,
    pub base_branch: String,
    pub state: RunState,
    pub branch: String,
    pub worktree_name: String,
    pub worktree_path: String,
    /// The "plan: <goal>" materialization commit recorded at dispatch — the
    /// baseline of the run's review diff, keeping materialized docs out of
    /// review noise. `None` for adopted and migrated runs (the diff falls back
    /// to the merge-base).
    #[serde(default)]
    pub base_sha: Option<String>,
    /// Run-side per-stage execution progress, keyed by the plan's stage ids.
    /// A progress record exists only once its stage has been dispatched.
    #[serde(default)]
    pub stages: Vec<StageProgress>,
    /// The stage whose build/fix/validate session is (or was last) in flight.
    #[serde(default)]
    pub current_stage_id: Option<String>,
    /// The stage a mid-run revision session is running for (disambiguates
    /// store write-back from post-review changes on `done(revise)`).
    #[serde(default)]
    pub revising_stage_id: Option<String>,
    /// "Run all": auto-dispatch the next approved stage when validation passes.
    #[serde(default)]
    pub auto_advance: bool,
    /// True for a run minted around a pre-existing (user-created) worktree.
    #[serde(default)]
    pub adopted: bool,
    /// Adoption's warm harness-continuation flag; consumed by the first
    /// session spawn after adoption, persisted so a restart in between keeps it.
    #[serde(default)]
    pub pending_continuation: bool,
    /// The last triage pass over this run's diff. Presentational, so it is
    /// persisted purely so a restart does not throw away an ordering the
    /// reviewer was reading; nothing waits on it and nothing reads it back into
    /// the lifecycle.
    #[serde(default)]
    pub triage: Option<crate::run::TriageReport>,
    /// Durable nonce-bound recovery journal. A started attempt survives daemon
    /// restart and can never be mistaken for a verified success.
    #[serde(default)]
    pub recovery: Option<crate::run::RecoveryAttempt>,
    /// Write-ahead journal for push/merge. It is cleared only after refs prove
    /// the candidate commit's publication (including after daemon restart).
    #[serde(default)]
    pub publication_attempt: Option<crate::run::PublicationAttempt>,
    #[serde(default)]
    pub provider: AgentProvider,
    /// Model/effort the run's agents run on (None = harness default).
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// This entity's agents, each owning its own conversation. A branch carries
    /// as many as the human adds; empty only on a pre-agent record.
    #[serde(default)]
    pub agents: Vec<Agent>,
    /// The pre-agent, entity-keyed conversation — see [`PersistedPlan::legacy_thread`].
    #[serde(rename = "thread", default, skip_serializing_if = "Thread::is_empty")]
    pub legacy_thread: Thread,
    pub last_summary: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// RFC 3339 UTC timestamps.
    pub created_at: String,
    pub updated_at: String,
    /// When the run last changed *state* (vs `updated_at`, any mutation).
    /// `None` on records from before the field existed — restore falls back
    /// to `updated_at`.
    #[serde(default)]
    pub state_changed_at: Option<String>,
}

impl PersistedPlan {
    pub fn model_choice(&self) -> ModelChoice {
        ModelChoice {
            provider: self.provider,
            model: self.model.clone(),
            effort: self.effort.clone(),
        }
    }

    /// This record's agents, with a pre-agent conversation folded onto the
    /// first one. The reattach path and the boot migration share it, so a
    /// record loaded before the migration ran reads the same either way.
    pub fn roster(&self) -> AgentRoster {
        AgentRoster::restore(
            &self.id,
            self.agents.clone(),
            self.legacy_thread.clone(),
            self.model_choice(),
            &self.created_at,
        )
    }
}

impl PersistedRun {
    pub fn model_choice(&self) -> ModelChoice {
        ModelChoice {
            provider: self.provider,
            model: self.model.clone(),
            effort: self.effort.clone(),
        }
    }

    /// See [`PersistedPlan::roster`].
    pub fn roster(&self) -> AgentRoster {
        AgentRoster::restore(
            &self.id,
            self.agents.clone(),
            self.legacy_thread.clone(),
            self.model_choice(),
            &self.created_at,
        )
    }
}

/// Canonical durable Issue aggregate. Planning state, stage-plan review,
/// implementation lineage, threads, and publication journals cross the crash
/// boundary as one record instead of being reconstructed from mutable halves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedIssue {
    pub issue: PersistedPlan,
    #[serde(default)]
    pub implementations: Vec<PersistedRun>,
}

/// The user-selected way an external worktree was finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorktreeFinishAction {
    Cleanup,
    Push,
    Merge,
    Delete,
}

/// Whether a durable finish record is protecting an in-progress destructive
/// operation or is ready to appear in Project Archive.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorktreeFinishStatus {
    Pending,
    #[default]
    Archived,
}

/// Durable intent and eventual history for an external worktree finished
/// through `worktree.finish`. Project ids are intentionally absent because they
/// are re-minted at boot; the canonical project path is the stable identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedArchivedWorktree {
    #[serde(default)]
    pub status: WorktreeFinishStatus,
    pub project_path: String,
    pub worktree_id: String,
    pub worktree_name: String,
    pub worktree_path: String,
    #[serde(default)]
    pub branch: Option<String>,
    pub head_sha: String,
    #[serde(default)]
    pub upstream: Option<String>,
    #[serde(default)]
    pub unpushed: Option<u64>,
    pub dirty_files: usize,
    pub uncommitted_files: usize,
    pub uncommitted_insertions: usize,
    pub uncommitted_deletions: usize,
    pub action: WorktreeFinishAction,
    #[serde(default)]
    pub archived_at: Option<String>,
}

/// The database schema. Applied on open; `schema_version` in `meta` is what a
/// future change reads to decide whether it has work to do.
///
/// One table carries the design: `thread_items`. Everything else is a small,
/// bounded record that is read and written whole, so those rows keep their
/// serde shape in a `record` column — normalizing them would buy nothing and
/// multiply the diff. A conversation is the opposite: it grows without bound
/// and is appended to constantly, so an item is a row, an append is one
/// `INSERT`, and a page is a `LIMIT`.
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS issues (
    id         TEXT PRIMARY KEY,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    record     TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS implementations (
    id         TEXT PRIMARY KEY,
    issue_id   TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    record     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS implementations_by_issue
    ON implementations(issue_id, created_at);

CREATE TABLE IF NOT EXISTS agents (
    id       TEXT PRIMARY KEY,
    owner_id TEXT NOT NULL,
    ordinal  INTEGER NOT NULL,
    record   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS agents_by_owner ON agents(owner_id, ordinal);

-- One conversation item. `sequence` orders the conversation and
-- `updated_sequence` carries an in-place mutation (seen, resolved), which is
-- what the client cursor compares against — so both are columns rather than
-- fields buried in the item JSON.
CREATE TABLE IF NOT EXISTS thread_items (
    agent_id         TEXT NOT NULL,
    sequence         INTEGER NOT NULL,
    updated_sequence INTEGER NOT NULL,
    -- 1 when this item calls the human: what an unread badge counts. Hoisted
    -- out of the item JSON because a conversation is loaded as its tail, so the
    -- items under it can only be counted by the database — and counting them
    -- by deserializing every one would undo the tail.
    attention        INTEGER NOT NULL DEFAULT 0,
    -- 1 when this item is a message, either role. Hoisted for the same reason
    -- `attention` is: what a page's limit buys and what a catch-up packet
    -- carries is conversation, and a conversation buried in activity can only
    -- be found under the tail by the database.
    message          INTEGER NOT NULL DEFAULT 0,
    item             TEXT NOT NULL,
    PRIMARY KEY (agent_id, sequence)
);
CREATE INDEX IF NOT EXISTS thread_items_attention
    ON thread_items(agent_id, attention, sequence);
CREATE INDEX IF NOT EXISTS thread_items_cursor
    ON thread_items(agent_id, updated_sequence);
-- The counted rule, indexed: an item the human reads as conversation. Partial,
-- so the index holds the conversation and not the activity between it, and
-- every statement that seeks down it repeats the predicate verbatim.
CREATE INDEX IF NOT EXISTS thread_items_conversation
    ON thread_items(agent_id, sequence) WHERE message = 1 OR attention = 1;

CREATE TABLE IF NOT EXISTS captures (
    id     TEXT PRIMARY KEY,
    record TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS attention (
    entity_id TEXT PRIMARY KEY,
    record    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS archived_worktrees (
    id     TEXT PRIMARY KEY,
    record TEXT NOT NULL
);
"#;

/// The two conversation reads that must never walk a whole conversation, held
/// here rather than inline so the test that checks their query plans checks
/// the statements that actually run.
///
/// `sequence DESC` with a `LIMIT` walks the primary key backward from the seek
/// point and stops, so the cost of a page is the page.
const THREAD_PAGE_SQL: &str = "SELECT item FROM thread_items \
     WHERE agent_id = ?1 AND sequence < ?2 \
     ORDER BY sequence DESC LIMIT ?3";

/// The messages of a conversation, newest-first from the end — what a resumed
/// agent's catch-up packet is built from when the tail it booted onto holds
/// only activity.
///
/// The partial index's predicate is repeated verbatim as a conjunct so the
/// planner's implication check is trivial, and `message = 1` then narrows the
/// seek to the words themselves.
const THREAD_MESSAGE_PAGE_SQL: &str = "SELECT item FROM thread_items \
     WHERE agent_id = ?1 AND (message = 1 OR attention = 1) AND message = 1 \
     ORDER BY sequence DESC LIMIT ?2";

/// Where a page of conversation reaches back to: the sequence of the
/// `limit`-th counted item below the seek, found by one seek down the partial
/// index. Nothing found means the conversation runs out above the page, and
/// the floor is the bottom.
const THREAD_CONVERSATION_FLOOR_SQL: &str = "SELECT sequence FROM thread_items \
     WHERE agent_id = ?1 AND (message = 1 OR attention = 1) AND sequence < ?2 \
     ORDER BY sequence DESC LIMIT 1 OFFSET ?3";

/// The page itself: every item in that span, newest-first under the ceiling —
/// so the activity between two messages travels with them, and an
/// all-activity stretch ends the page early instead of reading without bound.
const THREAD_CONVERSATION_PAGE_SQL: &str = "SELECT item FROM thread_items \
     WHERE agent_id = ?1 AND sequence < ?2 AND sequence >= ?3 \
     ORDER BY sequence DESC LIMIT ?4";

/// How much of a conversation a load reads and the daemon then holds.
///
/// The tail, never the whole: a conversation costs this process a constant
/// rather than its length, and everything older is one page read away. Far
/// above any page a client asks for or any catch-up packet an agent is handed,
/// so the bound is only ever felt by history nobody has scrolled back to.
///
/// The tail is the conversation's working set, and what is loaded is what the
/// daemon reasons over: the mailbox an agent is sent to, the unread count a
/// row carries. Both are about what has just been said, and a message with
/// this many items of conversation after it has been gone past rather than
/// left waiting. The two readers that are asked ABOUT history instead —
/// searching a conversation, and replaying an entity's anchor — go to the
/// store for the whole of it (`Store::thread_items`) rather than answering
/// off the tail.
pub const RESIDENT_CONVERSATION_TAIL: usize = 200;

/// How long a conversation is, asked of the primary key rather than of the
/// items: the load needs the total to say how much of a conversation it left
/// behind, and reading the items to count them would spend what paging saves.
const THREAD_ITEM_COUNT_SQL: &str = "SELECT COUNT(*) FROM thread_items WHERE agent_id = ?1";

/// The forward cursor, which `thread_items_cursor` covers: the seek is the
/// filter, so a poll that finds nothing new reads nothing.
const THREAD_CURSOR_SQL: &str = "SELECT item FROM thread_items \
     WHERE agent_id = ?1 AND updated_sequence > ?2 \
     ORDER BY updated_sequence";

/// The newest counter value anything in a conversation has reached, read off
/// the far end of `thread_items_cursor` rather than by looking at the items.
///
/// A load reads the tail, so it cannot see that an item under the tail was
/// mutated in place — a message marked seen, a comment resolved — before the
/// process before it stopped. This is how far a cursor has to have travelled
/// for the tail to be the whole answer to it.
const THREAD_LAST_SEQUENCE_SQL: &str =
    "SELECT COALESCE(MAX(updated_sequence), 0) FROM thread_items WHERE agent_id = ?1";

/// The schema this build writes. A stored value ahead of this one means the
/// database was written by a newer bridge; opening it read-write would corrupt
/// what that build knows, so the daemon refuses rather than guessing.
pub const SCHEMA_VERSION: i64 = 3;

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
        let conn = Connection::open(&database_path).map_err(|cause| StoreError::Unopenable {
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
        // The columns BEFORE the schema batch: `SCHEMA` indexes both of them,
        // and an older table has no such column for an index to name. A v1
        // database arrives here needing both, and reaches v3 in one open.
        if stored == Some(1) {
            Store::add_attention_column(&conn)?;
        }
        if matches!(stored, Some(1 | 2)) {
            Store::add_message_column(&conn)?;
        }
        conn.execute_batch(SCHEMA)?;
        if matches!(stored, Some(1 | 2)) {
            Store::classify_stored_items(&conn)?;
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

    /// Add the v2 `attention` column to a v1 table.
    ///
    /// `CREATE TABLE IF NOT EXISTS` does not alter a table that already exists,
    /// so a database written by v1 needs the column added by hand — and needs
    /// it before the schema batch, which indexes it.
    fn add_attention_column(conn: &Connection) -> Result<(), StoreError> {
        if conn
            .prepare("SELECT attention FROM thread_items LIMIT 1")
            .is_ok()
        {
            return Ok(());
        }
        conn.execute(
            "ALTER TABLE thread_items ADD COLUMN attention INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
        Ok(())
    }

    /// Add the v3 `message` column to an older table.
    ///
    /// The v1→v2 precedent exactly: `CREATE TABLE IF NOT EXISTS` does not alter
    /// a table that already exists, and the column has to be there before the
    /// schema batch, whose partial index names it.
    fn add_message_column(conn: &Connection) -> Result<(), StoreError> {
        if conn
            .prepare("SELECT message FROM thread_items LIMIT 1")
            .is_ok()
        {
            return Ok(());
        }
        conn.execute(
            "ALTER TABLE thread_items ADD COLUMN message INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
        Ok(())
    }

    /// Classify every stored item for the freshly added columns.
    ///
    /// The one place Build reads whole conversations on purpose: it runs once,
    /// on the upgrade, because a column added with a default says nothing about
    /// the items already under it. Both columns are written on every upgrade
    /// path — a v1 database gains them together, and rewriting `attention` with
    /// the value it already holds is what makes one classifier serve both.
    fn classify_stored_items(conn: &Connection) -> Result<(), StoreError> {
        let rows: Vec<(String, i64, String)> = {
            let mut statement =
                conn.prepare("SELECT agent_id, sequence, item FROM thread_items")?;
            let read = statement.query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get::<_, String>(2)?))
            })?;
            read.collect::<Result<_, _>>()?
        };
        let mut set = conn.prepare(
            "UPDATE thread_items SET attention = ?3, message = ?4 \
             WHERE agent_id = ?1 AND sequence = ?2",
        )?;
        for (agent_id, sequence, raw) in rows {
            let Ok(item) = serde_json::from_str::<ThreadItem>(&raw) else {
                continue;
            };
            set.execute(rusqlite::params![
                agent_id,
                sequence,
                i64::from(item.attention_reason().is_some()),
                i64::from(matches!(item, ThreadItem::Message(_)))
            ])?;
        }
        Ok(())
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

    /// Test-only: how many rows this connection has written since it opened.
    /// The only way to hold the store to its central promise — that appending
    /// one message writes one row rather than rewriting the conversation.
    #[cfg(test)]
    pub fn total_changes(&self) -> u64 {
        self.connection().total_changes()
    }

    /// Test-only: stamp a schema version, so the refusal path can be exercised
    /// without a second build of the bridge.
    #[cfg(test)]
    pub fn set_schema_version(&self, version: i64) {
        self.connection()
            .execute(
                "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = ?1",
                [version.to_string()],
            )
            .expect("the schema version is stamped");
    }

    /// Copy the database to `destination` as a consistent snapshot.
    ///
    /// The state dir holds a live `build.db` plus its `-wal` and `-shm`
    /// sidecars, and a file-at-a-time copy of that triple while the daemon is
    /// running takes a torn database — the JSON records it replaced could each
    /// be copied on their own, and this cannot. `VACUUM INTO` writes one
    /// self-contained file from a single consistent read, so a backup taken
    /// mid-write is a database rather than a puzzle.
    ///
    /// Refuses to overwrite: a backup that silently replaced the previous one
    /// is a backup that can be lost twice.
    pub fn backup_to(&self, destination: &Path) -> Result<(), StoreError> {
        if destination.exists() {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{} already exists", destination.display()),
            )));
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // A bound parameter is not accepted here (VACUUM INTO takes a literal),
        // so the path is quoted the way SQLite quotes a string literal.
        let quoted = destination.to_string_lossy().replace('\'', "''");
        self.connection()
            .execute_batch(&format!("VACUUM INTO '{quoted}'"))?;
        Ok(())
    }

    /// Test-only: strip the hoisted columns and stamp the version back, so the
    /// upgrade path can be exercised against a database this build wrote.
    #[cfg(test)]
    pub fn pretend_to_be_v1(&self) {
        let conn = self.connection();
        conn.execute_batch(
            "DROP INDEX IF EXISTS thread_items_attention;
             DROP INDEX IF EXISTS thread_items_conversation;
             ALTER TABLE thread_items DROP COLUMN attention;
             ALTER TABLE thread_items DROP COLUMN message;
             UPDATE meta SET value = '1' WHERE key = 'schema_version';",
        )
        .expect("the v1 shape is staged");
    }

    /// Test-only: the same for the v2 shape — attention hoisted, message not.
    #[cfg(test)]
    pub fn pretend_to_be_v2(&self) {
        let conn = self.connection();
        conn.execute_batch(
            "DROP INDEX IF EXISTS thread_items_conversation;
             ALTER TABLE thread_items DROP COLUMN message;
             UPDATE meta SET value = '2' WHERE key = 'schema_version';",
        )
        .expect("the v2 shape is staged");
    }

    /// Test-only: make the next write fail once, then behave normally.
    #[cfg(test)]
    pub fn fail_next_write(&self) {
        self.fail_next_write
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Test-only: put a row into `captures` that will not parse back, so the
    /// loader's fail-fast contract can be exercised against real corruption.
    #[cfg(test)]
    pub fn corrupt_capture_row(&self, capture_id: &str) {
        self.connection()
            .execute(
                "INSERT INTO captures (id, record) VALUES (?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET record = ?2",
                rusqlite::params![capture_id, "{ not json"],
            )
            .expect("the corrupt row is written");
    }

    /// Test-only: read an archived worktree's stored JSON, so a test can put
    /// the record back in a half-finished state the way an interrupted daemon
    /// would have left it.
    #[cfg(test)]
    pub fn archived_worktree_json(&self, worktree_id: &str) -> Option<String> {
        self.connection()
            .query_row(
                "SELECT record FROM archived_worktrees WHERE id = ?1",
                [worktree_id],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    /// Test-only counterpart of [`archived_worktree_json`](Self::archived_worktree_json).
    #[cfg(test)]
    pub fn set_archived_worktree_json(&self, worktree_id: &str, record: &str) {
        self.connection()
            .execute(
                "UPDATE archived_worktrees SET record = ?2 WHERE id = ?1",
                rusqlite::params![worktree_id, record],
            )
            .expect("the archived worktree row is updated");
    }

    // ---- attention --------------------------------------------------------

    /// The attention map. A row that will not parse is skipped rather than
    /// fatal: this is ordering and colour, never correctness, and losing one
    /// costs a badly sorted rail rather than a task.
    pub fn load_attention(&self) -> HashMap<String, Attention> {
        let conn = self.connection();
        let Ok(mut statement) = conn.prepare("SELECT entity_id, record FROM attention") else {
            return HashMap::new();
        };
        let Ok(rows) = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) else {
            return HashMap::new();
        };
        rows.flatten()
            .filter_map(|(id, raw)| serde_json::from_str(&raw).ok().map(|value| (id, value)))
            .collect()
    }

    /// Persist the attention map, pruned to `live` — ids that no longer exist
    /// (a deleted run, a removed worktree) drop out, so the table tracks the
    /// world rather than growing forever.
    pub fn save_attention(
        &self,
        attention: &HashMap<String, Attention>,
        live: &HashSet<String>,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            tx.execute("DELETE FROM attention", [])?;
            let mut insert =
                tx.prepare("INSERT INTO attention (entity_id, record) VALUES (?1, ?2)")?;
            for (id, record) in attention.iter().filter(|(id, _)| live.contains(*id)) {
                insert.execute(rusqlite::params![
                    id,
                    serde_json::to_string(record).expect("attention always serializes")
                ])?;
            }
            Ok(())
        })
    }

    // ---- agents and conversations ----------------------------------------

    /// Write one owner's agents and their conversations.
    ///
    /// The roster is replaced wholesale (an agent can be removed), but the
    /// conversation is NOT rewritten. The stored `(sequence, updated_sequence)`
    /// pairs are read first — integers off a covering index, never the item
    /// bodies — and only items that are new or have changed since are written.
    /// So appending one message writes one row, whatever the conversation
    /// already holds. That is the entire reason this store replaced a JSON
    /// aggregate that rewrote every conversation on the Issue for every append;
    /// writing all N items here would have moved the amplification rather than
    /// removed it.
    fn write_agents(
        tx: &rusqlite::Transaction,
        owner_id: &str,
        agents: &[Agent],
    ) -> Result<(), StoreError> {
        let keep: HashSet<&str> = agents.iter().map(|agent| agent.id.as_str()).collect();
        let mut roster = tx.prepare("SELECT id FROM agents WHERE owner_id = ?1")?;
        let existing: Vec<String> = roster
            .query_map([owner_id], |row| row.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        drop(roster);
        for gone in existing.iter().filter(|id| !keep.contains(id.as_str())) {
            tx.execute("DELETE FROM thread_items WHERE agent_id = ?1", [gone])?;
            tx.execute("DELETE FROM agents WHERE id = ?1", [gone])?;
        }

        let mut upsert_agent = tx.prepare(
            "INSERT INTO agents (id, owner_id, ordinal, record) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET owner_id = ?2, ordinal = ?3, record = ?4",
        )?;
        let mut upsert_item = tx.prepare(
            "INSERT INTO thread_items (agent_id, sequence, updated_sequence, attention, message, item)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(agent_id, sequence)
             DO UPDATE SET updated_sequence = ?3, attention = ?4, message = ?5, item = ?6",
        )?;
        let mut delete_item =
            tx.prepare("DELETE FROM thread_items WHERE agent_id = ?1 AND sequence = ?2")?;
        // The cursor columns alone, which `thread_items_cursor` covers: this is
        // what keeps the read cheap enough that skipping the writes is a win.
        // Bounded below by the tail the agent holds: a conversation loaded as
        // its tail knows nothing about the history under it, and an orphan
        // sweep that read that history would take it for items withdrawn from
        // a conversation that never had them.
        let mut stored_cursors = tx.prepare(
            "SELECT sequence, updated_sequence FROM thread_items \
             WHERE agent_id = ?1 AND sequence >= ?2",
        )?;

        for agent in agents {
            // The agent row carries everything about the conversation EXCEPT
            // its items — sessions, revisions, the last completion report —
            // because those are small, bounded and always read whole.
            let mut skeleton = agent.clone();
            let items = std::mem::take(&mut skeleton.thread.items);
            upsert_agent.execute(rusqlite::params![
                agent.id,
                owner_id,
                agent.ordinal,
                serde_json::to_string(&skeleton).expect("an agent always serializes")
            ])?;

            let mut stored: HashMap<i64, i64> = stored_cursors
                .query_map(
                    rusqlite::params![&agent.id, agent.thread.resident_from_sequence() as i64],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?
                .collect::<Result<_, _>>()?;
            for item in &items {
                let sequence = item.sequence() as i64;
                let updated = item.latest_sequence() as i64;
                // `remove` both answers "was it stored?" and leaves the map
                // holding exactly the sequences the roster no longer has.
                if stored.remove(&sequence) == Some(updated) {
                    continue;
                }
                upsert_item.execute(rusqlite::params![
                    agent.id,
                    sequence,
                    updated,
                    i64::from(item.attention_reason().is_some()),
                    i64::from(matches!(item, ThreadItem::Message(_))),
                    serde_json::to_string(item).expect("a thread item always serializes")
                ])?;
            }
            // An item can be taken off a conversation (a withdrawn draft), so
            // what is left over is deleted rather than left behind to reappear
            // on the next load.
            for orphan in stored.keys() {
                delete_item.execute(rusqlite::params![agent.id, orphan])?;
            }
        }
        Ok(())
    }

    /// Read one owner's agents back, conversations included, in rail order.
    fn read_agents(conn: &Connection, owner_id: &str) -> Result<Vec<Agent>, StoreError> {
        let mut statement =
            conn.prepare("SELECT id, record FROM agents WHERE owner_id = ?1 ORDER BY ordinal")?;
        let rows: Vec<(String, String)> = statement
            .query_map([owner_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<_, _>>()?;
        // The tail of each conversation, not the whole of it: a boot that read
        // every item of every conversation would spend, in one go and for the
        // life of the process, exactly what the paged reads exist to save.
        let mut tail = conn.prepare(THREAD_PAGE_SQL)?;
        let mut count = conn.prepare(THREAD_ITEM_COUNT_SQL)?;
        let mut last_sequence = conn.prepare(THREAD_LAST_SEQUENCE_SQL)?;
        let mut agents = Vec::with_capacity(rows.len());
        for (id, raw) in rows {
            let mut agent: Agent =
                serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                    path: PathBuf::from(format!("agents/{id}")),
                    source,
                })?;
            let held = count.query_row([&id], |row| row.get::<_, i64>(0))? as u64;
            let stored_last = last_sequence.query_row([&id], |row| row.get::<_, i64>(0))? as u64;
            let items = read_thread_page(&mut tail, &id, i64::MAX, RESIDENT_CONVERSATION_TAIL)?;
            agent.thread.adopt_stored_tail(
                items,
                held.saturating_sub(RESIDENT_CONVERSATION_TAIL as u64),
                stored_last,
            );
            agents.push(agent);
        }
        Ok(agents)
    }

    // ---- paged conversation reads ----------------------------------------

    /// One page of a conversation: the newest `limit` items strictly older
    /// than `before_sequence`, or the newest `limit` items when it is `None`.
    ///
    /// Handed back oldest-first even though SQL reads it newest-first, so the
    /// caller renders a page in the order it happened without reversing it
    /// again. This is what a first load asks for: a conversation of hundreds
    /// of items ships its tail, and the client walks backward from there.
    pub fn thread_page(
        &self,
        agent_id: &str,
        before_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Vec<ThreadItem>, StoreError> {
        // No bound means "from the newest", which the same seek expresses as a
        // point past every sequence there could be.
        let before = before_sequence
            .and_then(|sequence| i64::try_from(sequence).ok())
            .unwrap_or(i64::MAX);
        let connection = self.connection();
        let mut statement = connection.prepare(THREAD_PAGE_SQL)?;
        read_thread_page(&mut statement, agent_id, before, limit)
    }

    /// One page of a conversation, measured in conversation: the items down to
    /// and including the `limit`-th counted one below `before_sequence`,
    /// oldest-first, with whether anything at all remains below it.
    ///
    /// Three seeks and no scan. One finds where the page reaches back to, one
    /// reads that span under the ceiling, and one asks whether anything is
    /// left below what was shipped — which is what `has_more` means, for items
    /// of any kind, so a client walking `before_sequence = oldest_sequence`
    /// sees every row exactly once.
    pub fn thread_conversation_page(
        &self,
        agent_id: &str,
        before_sequence: Option<u64>,
        limit: usize,
    ) -> Result<(Vec<ThreadItem>, bool), StoreError> {
        let before = before_sequence
            .and_then(|sequence| i64::try_from(sequence).ok())
            .unwrap_or(i64::MAX);
        // A page of nothing is not a page; the callers clamp to at least one.
        let limit = limit.max(1);
        let connection = self.connection();
        let floor: i64 = connection
            .query_row(
                THREAD_CONVERSATION_FLOOR_SQL,
                rusqlite::params![agent_id, before, (limit - 1) as i64],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let mut page = {
            let mut statement = connection.prepare(THREAD_CONVERSATION_PAGE_SQL)?;
            let span = decode_thread_items(
                agent_id,
                statement.query_map(
                    rusqlite::params![
                        agent_id,
                        before,
                        floor,
                        crate::thread::page_span_ceiling(limit) as i64
                    ],
                    |row| row.get::<_, String>(0),
                )?,
            )?;
            span
        };
        page.reverse();
        // Off the page's own oldest item, never off the floor: the ceiling may
        // have stopped the read above it, and a page that claimed to reach the
        // floor it asked for would tell a client to skip what it never sent.
        let shipped_floor = match page.first() {
            Some(oldest) => oldest.sequence() as i64,
            None => return Ok((page, false)),
        };
        let has_more = connection
            .query_row(
                "SELECT 1 FROM thread_items WHERE agent_id = ?1 AND sequence < ?2 LIMIT 1",
                rusqlite::params![agent_id, shipped_floor],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        Ok((page, has_more))
    }

    /// The newest `limit` **messages** of a conversation, oldest-first — what
    /// a resumed agent's catch-up packet carries.
    ///
    /// Messages of either role, outcomes among them because an outcome is a
    /// message. The limit counts messages, so a session that emitted hundreds
    /// of tool calls before it stopped still hands its replacement what the
    /// human said: the seek runs down the conversation index and never reads
    /// the activity between the words at all.
    pub fn thread_message_page(
        &self,
        agent_id: &str,
        limit: usize,
    ) -> Result<Vec<ThreadItem>, StoreError> {
        let connection = self.connection();
        let mut statement = connection.prepare(THREAD_MESSAGE_PAGE_SQL)?;
        let mut page = decode_thread_items(
            agent_id,
            statement.query_map(rusqlite::params![agent_id, limit as i64], |row| {
                row.get::<_, String>(0)
            })?,
        )?;
        // Read newest-first off the seek, handed back in the order the
        // conversation happened — the way `read_thread_page` reverses.
        page.reverse();
        Ok(page)
    }

    /// How many items a conversation holds. A `COUNT(*)`, never a load — the
    /// client needs the total to know whether its cache is whole, and reading
    /// the items to count them would spend exactly what paging saves.
    /// How many attention-class items sit strictly between `cursor` and
    /// `floor` — the unread the human is owed from UNDER the resident tail.
    ///
    /// A conversation is loaded as its newest items, so a badge counted off
    /// what this process holds under-reports the moment the human has not read
    /// in a while. The count has to come from the database, and it is a count
    /// rather than a read: deserializing the history to size a badge would
    /// undo the tail it exists to keep.
    pub fn unread_attention_between(
        &self,
        agent_id: &str,
        cursor: u64,
        floor: u64,
    ) -> Result<u64, StoreError> {
        if floor == 0 || floor <= cursor {
            return Ok(0);
        }
        let count: i64 = self.connection().query_row(
            "SELECT COUNT(*) FROM thread_items
             WHERE agent_id = ?1 AND attention = 1 AND sequence > ?2 AND sequence < ?3",
            rusqlite::params![agent_id, cursor as i64, floor as i64],
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }

    pub fn thread_item_count(&self, agent_id: &str) -> Result<u64, StoreError> {
        let count: i64 = self
            .connection()
            .query_row(THREAD_ITEM_COUNT_SQL, [agent_id], |row| row.get(0))?;
        Ok(count as u64)
    }

    /// The forward cursor: everything that has happened on a conversation
    /// since `after_sequence`, oldest-first.
    ///
    /// It compares `updated_sequence`, not `sequence`, because an item mutated
    /// in place — a message marked seen, a comment resolved — is news to a
    /// client whose cursor is already past that item's creation. The
    /// `thread_items_cursor` index is on exactly that column, so the seek is
    /// the filter.
    pub fn thread_items_after(
        &self,
        agent_id: &str,
        after_sequence: u64,
    ) -> Result<Vec<ThreadItem>, StoreError> {
        let after = i64::try_from(after_sequence).unwrap_or(i64::MAX);
        let connection = self.connection();
        let mut statement = connection.prepare(THREAD_CURSOR_SQL)?;
        let mut delta = decode_thread_items(
            agent_id,
            statement.query_map(rusqlite::params![agent_id, after], |row| {
                row.get::<_, String>(0)
            })?,
        )?;
        // Index order is mutation order; the conversation's own order is
        // creation order, which is what a client merges its cache against.
        delta.sort_by_key(ThreadItem::sequence);
        Ok(delta)
    }

    /// The whole of one conversation, history included — the deliberate
    /// exception to paging.
    ///
    /// For the two readers that are answering ABOUT history rather than
    /// rendering it: a search of the conversation, and the anchor the boot
    /// migration replays out of everything the user ever said. Both are wrong
    /// if they only see the tail, and neither runs on a poll.
    pub fn thread_items(&self, agent_id: &str) -> Result<Vec<ThreadItem>, StoreError> {
        let connection = self.connection();
        let mut statement = connection
            .prepare("SELECT item FROM thread_items WHERE agent_id = ?1 ORDER BY sequence")?;
        let rows = statement.query_map([agent_id], |row| row.get::<_, String>(0))?;
        decode_thread_items(agent_id, rows)
    }

    // ---- issues and implementations --------------------------------------

    /// Whether an Issue exists. The question `issue_record_path(..).is_file()`
    /// used to answer.
    pub fn issue_exists(&self, issue_id: &str) -> bool {
        self.connection()
            .query_row("SELECT 1 FROM issues WHERE id = ?1", [issue_id], |_| Ok(()))
            .optional()
            .map(|found| found.is_some())
            .unwrap_or(false)
    }

    /// Write an Issue's own record and agents. Its implementations are separate
    /// rows and are not touched here — which is the point: saving an Issue no
    /// longer rewrites every implementation inside it.
    pub fn save_issue_plan(&self, record: &PersistedPlan) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            let mut skeleton = record.clone();
            let agents = std::mem::take(&mut skeleton.agents);
            tx.execute(
                "INSERT INTO issues (id, created_at, updated_at, record) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(id) DO UPDATE SET created_at = ?2, updated_at = ?3, record = ?4",
                rusqlite::params![
                    record.id,
                    record.created_at,
                    record.updated_at,
                    serde_json::to_string(&skeleton).expect("an Issue always serializes")
                ],
            )?;
            Store::write_agents(tx, &record.id, &agents)
        })
    }

    /// Every Issue with its implementations, oldest first.
    pub fn load_all_issues(&self) -> Result<Vec<PersistedIssue>, StoreError> {
        let conn = self.connection();
        let mut statement =
            conn.prepare("SELECT id, record FROM issues ORDER BY created_at, id")?;
        let rows: Vec<(String, String)> = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<_, _>>()?;
        drop(statement);
        let mut issues = Vec::with_capacity(rows.len());
        for (id, raw) in rows {
            let mut issue: PersistedPlan =
                serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                    path: PathBuf::from(format!("issues/{id}")),
                    source,
                })?;
            issue.agents = Store::read_agents(&conn, &id)?;
            let implementations = Store::read_runs(&conn, Some(&id))?;
            issues.push(PersistedIssue {
                issue,
                implementations,
            });
        }
        Ok(issues)
    }

    /// Delete an Issue, its implementations, every conversation on them, and
    /// its canonical plan docs.
    ///
    /// The docs are the filesystem half of the delete: they deliberately live
    /// outside the database, so removing only rows would leave the Issue's
    /// plan on disk forever with nothing referring to it.
    pub fn delete_plan(&self, plan_id: &str) -> Result<(), StoreError> {
        remove_dir_if_present(&self.issue_dir(plan_id))?;
        self.in_transaction(|tx| {
            let mut owned = tx.prepare("SELECT id FROM implementations WHERE issue_id = ?1")?;
            let runs: Vec<String> = owned
                .query_map([plan_id], |row| row.get::<_, String>(0))?
                .flatten()
                .collect();
            drop(owned);
            for owner in std::iter::once(plan_id.to_string()).chain(runs) {
                tx.execute(
                    "DELETE FROM thread_items WHERE agent_id IN
                     (SELECT id FROM agents WHERE owner_id = ?1)",
                    [&owner],
                )?;
                tx.execute("DELETE FROM agents WHERE owner_id = ?1", [&owner])?;
            }
            tx.execute("DELETE FROM implementations WHERE issue_id = ?1", [plan_id])?;
            tx.execute("DELETE FROM issues WHERE id = ?1", [plan_id])?;
            Ok(())
        })
    }

    // ---- runs -------------------------------------------------------------

    /// Write one run and its agents, whether or not it belongs to an Issue.
    pub fn save_run(&self, record: &PersistedRun) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            let mut skeleton = record.clone();
            let agents = std::mem::take(&mut skeleton.agents);
            tx.execute(
                "INSERT INTO implementations (id, issue_id, created_at, updated_at, record)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET
                     issue_id = ?2, created_at = ?3, updated_at = ?4, record = ?5",
                rusqlite::params![
                    record.id,
                    record.plan_id,
                    record.created_at,
                    record.updated_at,
                    serde_json::to_string(&skeleton).expect("a run always serializes")
                ],
            )?;
            Store::write_agents(tx, &record.id, &agents)
        })
    }

    /// Runs belonging to `issue_id`, or every run when it is `None`.
    fn read_runs(
        conn: &Connection,
        issue_id: Option<&str>,
    ) -> Result<Vec<PersistedRun>, StoreError> {
        let (sql, bind): (&str, Vec<&str>) = match issue_id {
            Some(id) => (
                "SELECT id, record FROM implementations WHERE issue_id = ?1
                 ORDER BY created_at, id",
                vec![id],
            ),
            None => (
                "SELECT id, record FROM implementations ORDER BY created_at, id",
                Vec::new(),
            ),
        };
        let mut statement = conn.prepare(sql)?;
        let rows: Vec<(String, String)> = statement
            .query_map(rusqlite::params_from_iter(bind), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<_, _>>()?;
        drop(statement);
        let mut runs = Vec::with_capacity(rows.len());
        for (id, raw) in rows {
            let mut run: PersistedRun =
                serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                    path: PathBuf::from(format!("implementations/{id}")),
                    source,
                })?;
            run.agents = Store::read_agents(conn, &id)?;
            runs.push(run);
        }
        Ok(runs)
    }

    /// Every run, oldest first — an Issue's implementations and the planless
    /// adopted ones alike. Boot reattaches from this one list.
    pub fn load_all_runs(&self) -> Result<Vec<PersistedRun>, StoreError> {
        let conn = self.connection();
        Store::read_runs(&conn, None)
    }

    /// Every Issue's own record, oldest first, without its implementations.
    pub fn load_all_plans(&self) -> Result<Vec<PersistedPlan>, StoreError> {
        Ok(self
            .load_all_issues()?
            .into_iter()
            .map(|issue| issue.issue)
            .collect())
    }

    pub fn delete_run(&self, run_id: &str) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            tx.execute(
                "DELETE FROM thread_items WHERE agent_id IN
                 (SELECT id FROM agents WHERE owner_id = ?1)",
                [run_id],
            )?;
            tx.execute("DELETE FROM agents WHERE owner_id = ?1", [run_id])?;
            tx.execute("DELETE FROM implementations WHERE id = ?1", [run_id])?;
            Ok(())
        })
    }

    // ---- captures and archived worktrees ---------------------------------

    pub fn save_capture(&self, record: &crate::capture::Capture) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            tx.execute(
                "INSERT INTO captures (id, record) VALUES (?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET record = ?2",
                rusqlite::params![
                    record.id,
                    serde_json::to_string(record).expect("a capture always serializes")
                ],
            )?;
            Ok(())
        })
    }

    pub fn delete_capture(&self, capture_id: &str) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            tx.execute("DELETE FROM captures WHERE id = ?1", [capture_id])?;
            Ok(())
        })
    }

    pub fn load_all_captures(&self) -> Result<Vec<crate::capture::Capture>, StoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare("SELECT id, record FROM captures")?;
        let rows: Vec<(String, String)> = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<_, _>>()?;
        let mut captures = Vec::with_capacity(rows.len());
        for (id, raw) in rows {
            captures.push(
                serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                    path: PathBuf::from(format!("captures/{id}")),
                    source,
                })?,
            );
        }
        captures.sort_by(|a: &crate::capture::Capture, b| a.created_at.cmp(&b.created_at));
        Ok(captures)
    }

    pub fn save_archived_worktree(
        &self,
        record: &PersistedArchivedWorktree,
    ) -> Result<(), StoreError> {
        self.in_transaction(|tx| {
            tx.execute(
                "INSERT INTO archived_worktrees (id, record) VALUES (?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET record = ?2",
                rusqlite::params![
                    record.worktree_id,
                    serde_json::to_string(record).expect("an archived worktree always serializes")
                ],
            )?;
            Ok(())
        })
    }

    pub fn load_all_archived_worktrees(
        &self,
    ) -> Result<Vec<PersistedArchivedWorktree>, StoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare("SELECT id, record FROM archived_worktrees")?;
        let rows: Vec<(String, String)> = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<_, _>>()?;
        let mut archived = Vec::with_capacity(rows.len());
        for (id, raw) in rows {
            archived.push(
                serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                    path: PathBuf::from(format!("archived_worktrees/{id}")),
                    source,
                })?,
            );
        }
        archived.sort_by(|a: &PersistedArchivedWorktree, b| {
            a.archived_at
                .cmp(&b.archived_at)
                .then(a.worktree_id.cmp(&b.worktree_id))
        });
        Ok(archived)
    }

    // ---- the one-off import from the JSON store --------------------------

    /// Import the JSON record tree this store replaced, once.
    ///
    /// Build has exactly one installation, so this is a one-way door rather
    /// than a compatibility layer: it reads the record shapes that were on disk
    /// at the cutover and nothing older. Records that predate those shapes were
    /// already migrated in place by the JSON store's own boot migrations, which
    /// is why none of them survive here.
    ///
    /// Safe to run on every boot: it does nothing once the marker is set.
    ///
    /// The JSON tree is left exactly where it is — not renamed, not deleted.
    /// That is what makes the recovery real rather than stated: the marker
    /// lives IN the database, so deleting `build.db` deletes the marker too,
    /// and the next boot imports the untouched records again. A database that
    /// turns out to be wrong is thrown away, not repaired. The JSON is the
    /// user's to delete once they are satisfied; Build never does.
    ///
    /// Returns how many records were imported.
    pub fn import_json_store(&self) -> Result<usize, StoreError> {
        const MARKER: &str = "json_import";
        {
            let conn = self.connection();
            let done: Option<String> = conn
                .query_row("SELECT value FROM meta WHERE key = ?1", [MARKER], |row| {
                    row.get(0)
                })
                .optional()?;
            if done.is_some() {
                return Ok(0);
            }
        }

        let mut imported = 0usize;
        // Issues, with the implementations nested inside each aggregate.
        let issues_dir = self.dir.join("issues");
        if issues_dir.is_dir() {
            for entry in std::fs::read_dir(&issues_dir)? {
                let record_path = entry?.path().join("record.json");
                if !record_path.is_file() {
                    continue;
                }
                let aggregate: PersistedIssue = read_record(&record_path)?;
                self.save_issue_plan(&aggregate.issue)?;
                imported += 1;
                for implementation in &aggregate.implementations {
                    self.save_run(implementation)?;
                    imported += 1;
                }
            }
        }
        // Planless runs, one file each.
        imported += self.import_dir("runs", |raw: PersistedRun| self.save_run(&raw))?;
        imported += self.import_dir("captures", |raw: crate::capture::Capture| {
            self.save_capture(&raw)
        })?;
        imported += self.import_dir("archived-worktrees", |raw: PersistedArchivedWorktree| {
            self.save_archived_worktree(&raw)
        })?;

        // Attention is one file holding the whole map. Nothing is pruned on
        // import: the live set is not known until the loaders have run, and the
        // next save prunes it anyway.
        let attention_path = self.dir.join("attention").join("map.json");
        if attention_path.is_file() {
            // Fatal, like every other record kind. Attention is what the inbox
            // is ordered by and what "unread" is measured against; importing
            // zero of it silently would present the user with every one of
            // their conversations unread and no way to tell why.
            let map: HashMap<String, Attention> = read_record(&attention_path)?;
            let all: HashSet<String> = map.keys().cloned().collect();
            self.save_attention(&map, &all)?;
            imported += map.len();
        }

        // Make the import durable BEFORE it is declared done. Ordinary writes
        // run at `synchronous = NORMAL`, which defers the fsync to the next
        // checkpoint — right for a daemon that can redo a lost transition, and
        // wrong for the one write that can never be redone. Checkpointing here
        // is what makes the marker a truthful record of what is on disk.
        {
            let conn = self.connection();
            conn.pragma_update(None, "wal_checkpoint", "TRUNCATE")?;
            conn.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)",
                rusqlite::params![MARKER, crate::store::now_rfc3339()],
            )?;
            conn.pragma_update(None, "wal_checkpoint", "TRUNCATE")?;
        }
        // Last, and only once the import is durable: the note doubles as the
        // timestamp the rollback check measures against, so it must not predate
        // the data it describes.
        self.write_superseded_note(imported)?;
        Ok(imported)
    }

    /// The name of the note left in the JSON tree once it has been imported.
    ///
    /// It is for a human reading the state dir, not for Build: an old bridge
    /// would ignore it. What protects against a rollback is
    /// [`refuse_a_rolled_back_store`](Self::refuse_a_rolled_back_store); this
    /// is what tells the person looking at the directory why there are two
    /// copies of their work in it.
    const SUPERSEDED_NOTE: &'static str = "SUPERSEDED-BY-build.db.md";

    fn write_superseded_note(&self, imported: usize) -> Result<(), StoreError> {
        let note = format!(
            "# These records were imported into `build.db`\n\n             {imported} records were read out of this tree and into the SQLite              database beside it. Build no longer reads them.\n\n             They are kept, not deleted, for two reasons:\n\n             - They are the backup of the migration. Deleting `build.db` makes              Build import them again from scratch, which is the whole recovery              if the database ever turns out to be wrong.\n             - They are yours to delete once you are satisfied. Build never will.\n\n             **Do not run an older build-bridge against this directory.** It              would read these files and serve state frozen at the moment of the              import, silently, and anything you did in the meantime would be              invisible. A build that understands the database refuses to start              if these files change after this point.\n"
        );
        std::fs::write(self.dir.join(Store::SUPERSEDED_NOTE), note)?;
        Ok(())
    }

    /// Refuse to start when the imported JSON has been written to since the
    /// import.
    ///
    /// The rollback hazard runs in one direction and is silent in both halves:
    /// an older bridge run against this directory reads the JSON tree, serves
    /// state frozen at the import, and writes its own changes back there — and
    /// then a newer bridge, coming forward again, reads only the database and
    /// never sees any of it. Neither half says anything.
    ///
    /// So the newer one checks. If a JSON record is newer than the note the
    /// import left, something wrote to a store Build stopped reading, and the
    /// honest answer is to stop and say which file rather than to quietly
    /// discard whichever copy is younger.
    pub fn refuse_a_rolled_back_store(&self) -> Result<(), StoreError> {
        let note = self.dir.join(Store::SUPERSEDED_NOTE);
        let Ok(imported_at) = std::fs::metadata(&note).and_then(|meta| meta.modified()) else {
            // No note: nothing was ever imported here, so there is no older
            // store to have been rolled back to.
            return Ok(());
        };
        let mut newer = Vec::new();
        let mut look = |path: PathBuf| {
            if let Ok(modified) = std::fs::metadata(&path).and_then(|meta| meta.modified()) {
                if modified > imported_at {
                    newer.push(path);
                }
            }
        };
        if let Ok(entries) = std::fs::read_dir(self.dir.join("issues")) {
            for entry in entries.flatten() {
                look(entry.path().join("record.json"));
            }
        }
        for dir in ["runs", "captures", "archived-worktrees"] {
            if let Ok(entries) = std::fs::read_dir(self.dir.join(dir)) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if is_json_record(&path) {
                        look(path);
                    }
                }
            }
        }
        look(self.dir.join("attention").join("map.json"));
        match newer.first() {
            None => Ok(()),
            Some(path) => Err(StoreError::RolledBack {
                path: path.clone(),
                count: newer.len(),
            }),
        }
    }

    /// Read every `*.json` in one store subdirectory and hand each record to
    /// `save`. A record that will not parse is fatal: dropping one silently
    /// would orphan its worktree and lose the user's work without a trace,
    /// which is the same rule the JSON store booted under.
    fn import_dir<T: serde::de::DeserializeOwned>(
        &self,
        name: &str,
        save: impl Fn(T) -> Result<(), StoreError>,
    ) -> Result<usize, StoreError> {
        let dir = self.dir.join(name);
        if !dir.is_dir() {
            return Ok(0);
        }
        let mut imported = 0usize;
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if !is_json_record(&path) {
                continue;
            }
            save(read_record(&path)?)?;
            imported += 1;
        }
        Ok(imported)
    }

    /// Where conversation attachments live when the entity that took them has
    /// no checkout to put them in. Its own directory, beside the records rather
    /// than among them: the store root is scanned for legacy task files.
    pub fn attachments_dir(&self) -> PathBuf {
        self.dir.join("attachments")
    }

    // ---- Canonical plan docs -----------------------------------------------
    //
    // These stay on disk, and deliberately: they are markdown an AGENT reads
    // and writes in a worktree. Build materializes them into a checkout and
    // ingests them back. A blob in a database that has to be written to a file
    // to be useful belongs in a file.

    fn issue_dir(&self, issue_id: &str) -> PathBuf {
        self.dir.join("issues").join(issue_id)
    }

    /// Where an Issue's canonical docs live (worktree-relative layout inside).
    fn plan_docs_dir(&self, plan_id: &str) -> PathBuf {
        self.issue_dir(plan_id).join("docs")
    }

    // ---- Runs (worktree-scoped records) ----

    // ---- Captures (what the user said, before anything routed it) ----------

    // ---- Archived external worktrees --------------------------------------

    // ---- Canonical plan-doc ops (worktree ⇄ store) ----

    /// Ingest a worktree's plan docs into the plan's canonical store docs —
    /// the single plan doc (`plan_path`, worktree-relative) and every file in
    /// the multi-stage plan dir (`.build/plan/`, a flat dir of docs plus the
    /// manifest). **Fail-fast**, unlike the legacy snapshot mirror: an
    /// escaping `plan_path` is rejected, IO errors propagate, and finding
    /// nothing at all to ingest is an error — the caller only ingests when a
    /// `done` report claimed docs exist, and the plan must not advance with
    /// unpersisted docs. A re-ingest overwrites with the latest contents.
    pub fn ingest_plan_docs(
        &self,
        plan_id: &str,
        worktree_path: &Path,
        plan_path: &str,
    ) -> Result<(), StoreError> {
        if !is_worktree_contained_path(plan_path) {
            return Err(StoreError::PathEscape {
                path: plan_path.to_string(),
            });
        }
        let docs_root = self.plan_docs_dir(plan_id);
        let mut ingested_files = 0usize;
        let plan_source = worktree_path.join(plan_path);
        if plan_source.is_file() {
            let dest = docs_root.join(plan_path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&plan_source, &dest)?;
            ingested_files += 1;
        }
        let stage_dir = worktree_path.join(STAGE_PLAN_DIR);
        if stage_dir.is_dir() {
            let dest_dir = docs_root.join(STAGE_PLAN_DIR);
            std::fs::create_dir_all(&dest_dir)?;
            let mut worktree_names = std::collections::HashSet::new();
            for entry in std::fs::read_dir(&stage_dir)? {
                let source = entry?.path();
                if !source.is_file() {
                    continue; // stage docs are a flat dir of markdown files
                }
                let Some(name) = source.file_name() else {
                    continue;
                };
                std::fs::copy(&source, dest_dir.join(name))?;
                worktree_names.insert(name.to_os_string());
                ingested_files += 1;
            }
            // The worktree's stage dir is the truth, deletions included: a
            // revision that drops or renames a stage doc must not leave the
            // stale file in the store, where the next run's materialization
            // would commit it (invisibly — the materialization commit is
            // excluded from review diffs via base_sha).
            for entry in std::fs::read_dir(&dest_dir)? {
                let stored = entry?.path();
                if !stored.is_file() {
                    continue;
                }
                let Some(name) = stored.file_name() else {
                    continue;
                };
                if !worktree_names.contains(name) {
                    remove_file_if_present(&stored)?;
                }
            }
        }
        if ingested_files == 0 {
            return Err(StoreError::NothingToIngest {
                plan_id: plan_id.to_string(),
                plan_path: plan_path.to_string(),
                worktree_path: worktree_path.to_path_buf(),
            });
        }
        Ok(())
    }

    /// Materialize a plan's canonical docs into a worktree, preserving the
    /// worktree-relative (`.build/…`) layout — the reverse of
    /// `ingest_plan_docs`; run dispatch commits the result. Erroring when the
    /// store holds no docs is deliberate: dispatching a planned run without
    /// its plan would silently build from nothing.
    pub fn materialize_plan_docs(
        &self,
        plan_id: &str,
        worktree_path: &Path,
    ) -> Result<(), StoreError> {
        let docs_root = self.plan_docs_dir(plan_id);
        if !docs_root.is_dir() {
            return Err(StoreError::NoStoredDocs {
                plan_id: plan_id.to_string(),
            });
        }
        let copied = copy_tree(&docs_root, worktree_path, &[])?;
        if copied == 0 {
            return Err(StoreError::NoStoredDocs {
                plan_id: plan_id.to_string(),
            });
        }
        Ok(())
    }

    /// Whether the canonical store holds any doc at all for this plan. False
    /// for a migrated plan whose docs were unrecoverable (worktree and branch
    /// both gone) — the UI gates doc reads and Implement on this instead of
    /// spinning on reads that can never succeed.
    pub fn has_plan_docs(&self, plan_id: &str) -> bool {
        dir_contains_a_file(&self.plan_docs_dir(plan_id))
    }

    /// Read one canonical plan doc by its worktree-relative path. `None` when
    /// the doc does not exist (or the path tries to escape the docs dir).
    pub fn read_plan_doc(&self, plan_id: &str, rel_path: &str) -> Option<String> {
        if !is_worktree_contained_path(rel_path) {
            return None;
        }
        std::fs::read_to_string(self.plan_docs_dir(plan_id).join(rel_path)).ok()
    }

    // ---- Legacy-task boot migration ----
}

/// Only `*.json` files are records; `.tmp` leftovers from an interrupted
/// write are ignored (the rename never happened, so the previous record — or
/// no record — is the truth), and so are `.migrated` legacy files.
fn is_json_record(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("json")
}

/// The worktree-relative dir multi-stage plan docs live in.
const STAGE_PLAN_DIR: &str = ".build/plan";

/// [`write_record_atomically`] for a JSON file that is not a store record.
///
/// The one caller is `.build/review-rules.json`, which lives in the user's own
/// checkout rather than the store — and wants exactly the same durability, for
/// exactly the same reason: a half-written file is one a human has to repair.
pub(crate) fn write_json_atomically(path: &Path, json: &str) -> Result<(), StoreError> {
    write_record_atomically(path, json)
}

/// Persist one JSON record atomically **and durably**: write to a `.tmp`
/// sibling, fsync it, rename over the final path, then fsync the directory.
/// The fsyncs matter: rename-without-fsync is atomic against a process crash
/// but not against power loss — the rename can become durable before the data
/// blocks, leaving a zero-length record that blocks the next boot.
fn write_record_atomically(final_path: &Path, json: &str) -> Result<(), StoreError> {
    use std::io::Write;

    let dir = final_path
        .parent()
        .expect("record paths always sit inside a store dir");
    std::fs::create_dir_all(dir)?;
    let mut tmp_name = final_path
        .file_name()
        .expect("record paths always name a file")
        .to_os_string();
    tmp_name.push(".tmp");
    let tmp_path = dir.join(tmp_name);
    let mut tmp_file = std::fs::File::create(&tmp_path)?;
    tmp_file.write_all(json.as_bytes())?;
    tmp_file.sync_all()?;
    drop(tmp_file);
    std::fs::rename(&tmp_path, final_path)?;
    // Make the rename itself durable (best-effort where the platform allows
    // opening a directory read-only).
    if let Ok(dir_handle) = std::fs::File::open(dir) {
        let _ = dir_handle.sync_all();
    }
    Ok(())
}

/// Read and parse one JSON record, failing fast — with the file named — on
/// the two torn-record shapes: empty (power loss after the rename) and
/// unparseable.
fn read_record<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, StoreError> {
    let text = std::fs::read_to_string(path)?;
    if text.trim().is_empty() {
        return Err(StoreError::Empty {
            path: path.to_path_buf(),
        });
    }
    serde_json::from_str(&text).map_err(|source| StoreError::Corrupt {
        path: path.to_path_buf(),
        source,
    })
}

/// True iff the directory exists and holds at least one file, at any depth.
fn dir_contains_a_file(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        path.is_file() || (path.is_dir() && dir_contains_a_file(&path))
    })
}

/// Remove a file, treating "already gone" as success (delete idempotency).
/// Remove a directory and everything under it, if it is there. Absent is not
/// an error: deleting twice is the same as deleting once.
fn remove_dir_if_present(path: &Path) -> Result<(), StoreError> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StoreError::Io(error)),
    }
}

fn remove_file_if_present(path: &Path) -> Result<(), StoreError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(StoreError::Io(e)),
    }
}

/// Recursively copy every file under `source_root` into `dest_root`,
/// preserving the relative layout. Entries named in `top_level_excludes` are
/// skipped at the top level only (migration copies a snapshot into a `docs/`
/// subdir of itself and must not recurse into the destination). Returns how
/// many files were copied.
fn copy_tree(
    source_root: &Path,
    dest_root: &Path,
    top_level_excludes: &[&str],
) -> Result<usize, StoreError> {
    std::fs::create_dir_all(dest_root)?;
    // Collect before copying: the destination may be created inside the
    // source, and a live read_dir cursor must not observe it.
    let mut sources: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(source_root)? {
        sources.push(entry?.path());
    }
    let mut copied = 0usize;
    for source in sources {
        let Some(name) = source.file_name() else {
            continue;
        };
        if top_level_excludes
            .iter()
            .any(|excluded| name == std::ffi::OsStr::new(excluded))
        {
            continue;
        }
        let dest = dest_root.join(name);
        if source.is_dir() {
            copied += copy_tree(&source, &dest, &[])?;
        } else if source.is_file() {
            std::fs::copy(&source, &dest)?;
            copied += 1;
        }
    }
    Ok(copied)
}

/// One page off an already-prepared [`THREAD_PAGE_SQL`] — shared by the load,
/// which pages every conversation of an owner off one statement, and by
/// [`Store::thread_page`], which prepares its own. Turns the seek's
/// newest-first read into the order the conversation happened in.
fn read_thread_page(
    statement: &mut rusqlite::Statement<'_>,
    agent_id: &str,
    before: i64,
    limit: usize,
) -> Result<Vec<ThreadItem>, StoreError> {
    let mut page = decode_thread_items(
        agent_id,
        statement.query_map(rusqlite::params![agent_id, before, limit as i64], |row| {
            row.get::<_, String>(0)
        })?,
    )?;
    page.reverse();
    Ok(page)
}

/// Turn stored item rows into conversation items, naming the conversation in
/// the error so a corrupt row says which agent's history stopped parsing.
fn decode_thread_items(
    agent_id: &str,
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<String>>,
) -> Result<Vec<ThreadItem>, StoreError> {
    rows.collect::<Result<Vec<String>, _>>()?
        .into_iter()
        .map(|raw| serde_json::from_str(&raw))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| StoreError::Corrupt {
            path: PathBuf::from(format!("thread_items/{agent_id}")),
            source,
        })
}

// ---- Legacy → split mapping (pure; the migration's translation table) ----

/// The current time as an RFC 3339 UTC string (the store's timestamp format).
pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("UTC now formats as RFC 3339")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{Capture, CaptureRouting, CaptureState, CaptureTarget};

    use crate::agent::AgentRoster;
    use crate::models::ModelChoice;

    const NOW: &str = "2026-08-21T10:00:00Z";

    fn run_record(id: &str, plan_id: Option<&str>, created_at: &str) -> PersistedRun {
        PersistedRun {
            id: id.into(),
            plan_id: plan_id.map(str::to_string),
            goal: format!("goal for {id}"),
            project_path: "/repo".into(),
            base_branch: "main".into(),
            state: RunState::Building,
            branch: format!("build/{id}"),
            worktree_name: id.into(),
            worktree_path: format!("/wt/{id}"),
            base_sha: None,
            stages: Vec::new(),
            current_stage_id: None,
            revising_stage_id: None,
            auto_advance: false,
            adopted: false,
            pending_continuation: false,
            triage: None,
            recovery: None,
            publication_attempt: None,
            provider: Default::default(),
            model: None,
            effort: None,
            agents: AgentRoster::with_first(id, ModelChoice::default(), created_at)
                .agents()
                .to_vec(),
            legacy_thread: Default::default(),
            last_summary: None,
            last_error: None,
            created_at: created_at.into(),
            updated_at: created_at.into(),
            state_changed_at: None,
        }
    }

    fn plan_record(id: &str) -> PersistedPlan {
        PersistedPlan {
            id: id.into(),
            goal: format!("goal for {id}"),
            project_path: "/repo".into(),
            base_branch: "main".into(),
            state: PlanState::Drafting,
            archived_at: None,
            implementation_intent: Default::default(),
            implementation_activity: Default::default(),
            plan_path: ".build/plan.md".into(),
            stages: Vec::new(),
            provider: Default::default(),
            model: None,
            effort: None,
            agents: AgentRoster::with_first(id, ModelChoice::default(), NOW)
                .agents()
                .to_vec(),
            legacy_thread: Default::default(),
            last_summary: None,
            last_error: None,
            created_at: NOW.into(),
            updated_at: NOW.into(),
            state_changed_at: None,
        }
    }

    fn reload_run(store: &Store, run_id: &str) -> PersistedRun {
        store
            .load_all_runs()
            .expect("runs load")
            .into_iter()
            .find(|run| run.id == run_id)
            .unwrap_or_else(|| panic!("{run_id} is missing after a reload"))
    }

    /// The conversation is the one thing that cannot be re-derived, and saving
    /// splits it off the record into its own rows. It has to come back — from a
    /// store opened again from scratch, not from the process that wrote it.
    #[test]
    fn a_runs_conversation_survives_a_store_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("tasks");
        let mut record = run_record("run-1", Some("plan-1"), NOW);
        let agent_id = record.agents[0].id.clone();
        record.agents[0]
            .thread
            .post_user("build the thing", None, NOW);
        record.agents[0].thread.post_agent("on it", None, NOW);

        Store::new(&root)
            .expect("store opens")
            .save_run(&record)
            .expect("the run saves");

        let reloaded = reload_run(&Store::new(&root).expect("store reopens"), "run-1");
        assert_eq!(reloaded.agents.len(), 1);
        assert_eq!(reloaded.agents[0].id, agent_id);
        assert_eq!(
            reloaded.agents[0].thread.items, record.agents[0].thread.items,
            "every conversation item comes back unchanged"
        );
    }

    /// Appending one message writes ONE row. This is the whole reason the store
    /// changed: the JSON records it replaced rewrote every conversation on the
    /// Issue for every append, and a store that upserted all N items per save
    /// would have moved that cost rather than removed it.
    #[test]
    fn appending_one_message_writes_one_row_however_long_the_conversation_is() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        let mut record = run_record("run-1", None, NOW);
        for n in 0..60 {
            record.agents[0]
                .thread
                .post_user(format!("message {n}"), None, NOW);
        }
        store.save_run(&record).expect("the first save writes");

        let before = store.total_changes();
        record.agents[0].thread.post_user("one more", None, NOW);
        store.save_run(&record).expect("the append saves");
        let written = store.total_changes() - before;

        // One thread item, plus the agent row and the run row that always
        // carry the entity's own state. Never the 61 items already stored.
        assert!(
            written <= 3,
            "an append wrote {written} rows — the conversation is being rewritten"
        );
        assert_eq!(reload_run(&store, "run-1").agents[0].thread.items.len(), 61);
    }

    /// A conversation comes back in the order it happened, and an item taken
    /// off it is gone rather than resurrected by the next load.
    #[test]
    fn conversation_items_keep_their_order_and_a_removed_item_stays_removed() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        let mut record = run_record("run-1", None, NOW);
        for n in 0..5 {
            record.agents[0]
                .thread
                .post_user(format!("message {n}"), None, NOW);
        }
        store.save_run(&record).expect("the run saves");

        let ordered: Vec<u64> = reload_run(&store, "run-1").agents[0]
            .thread
            .items
            .iter()
            .map(|item| item.sequence())
            .collect();
        assert!(
            ordered.windows(2).all(|pair| pair[0] < pair[1]),
            "items came back out of order: {ordered:?}"
        );

        record.agents[0].thread.items.remove(2);
        store.save_run(&record).expect("the shortened run saves");
        let after = reload_run(&store, "run-1");
        assert_eq!(after.agents[0].thread.items.len(), 4);
        assert_eq!(
            after.agents[0].thread.items, record.agents[0].thread.items,
            "the removed item did not come back"
        );
    }

    /// An in-place mutation — marking a message seen — reaches the store even
    /// though the item's creation sequence has not moved. The cursor column is
    /// what makes that visible, so a save that compared creation sequences
    /// alone would silently drop it.
    #[test]
    fn a_message_mutated_in_place_is_written_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        let mut record = run_record("run-1", None, NOW);
        record.agents[0].thread.post_user("read me", None, NOW);
        store.save_run(&record).expect("the run saves");

        let read = record.agents[0].thread.read_unread(NOW);
        assert!(!read.is_empty(), "there is an unread message to mark seen");
        store.save_run(&record).expect("the mutation saves");

        let reloaded = reload_run(&store, "run-1");
        assert_eq!(
            reloaded.agents[0].thread.items, record.agents[0].thread.items,
            "the in-place mutation reached the store"
        );
    }

    /// A conversation of `count` messages, saved, with the id of the agent
    /// holding it — the shape every paging test starts from.
    fn store_with_conversation(root: &Path, count: usize) -> (Store, String) {
        let store = Store::new(root).expect("store opens");
        let mut record = run_record("run-1", None, NOW);
        for n in 0..count {
            record.agents[0]
                .thread
                .post_user(format!("message {n}"), None, NOW);
        }
        store.save_run(&record).expect("the conversation saves");
        (store, record.agents[0].id.clone())
    }

    fn sequences(items: &[ThreadItem]) -> Vec<u64> {
        items.iter().map(ThreadItem::sequence).collect()
    }

    /// What paging is for, said at the load: a conversation costs the daemon
    /// its tail, not its length. A boot that reads every item of every
    /// conversation back into memory pays the whole cost the paged reads
    /// exist to avoid, however carefully those reads seek.
    #[test]
    fn a_load_reads_the_tail_of_a_conversation_not_all_of_it() {
        let dir = tempfile::tempdir().unwrap();
        let held = RESIDENT_CONVERSATION_TAIL + 60;
        let (store, _agent_id) = store_with_conversation(&dir.path().join("tasks"), held);

        let reloaded = reload_run(&store, "run-1");
        let thread = &reloaded.agents[0].thread;
        assert_eq!(
            thread.items.len(),
            RESIDENT_CONVERSATION_TAIL,
            "the load is bounded by the tail, not by the conversation"
        );
        assert_eq!(
            thread.items.first().map(ThreadItem::sequence),
            Some(61),
            "the tail is the newest items, so the load starts past the first 60"
        );
        assert_eq!(
            thread.total_item_count(),
            held as u64,
            "a conversation still knows how long it is, whatever was read of it"
        );
        assert_eq!(
            thread.last_sequence(),
            held as u64,
            "appends carry on from the end of the conversation, not the end of the tail"
        );
    }

    /// The history a load left in the store is not history this process may
    /// throw away. Saving a conversation whose tail is all the daemon read
    /// must not read the missing items as items that were taken off it.
    #[test]
    fn saving_a_tail_leaves_the_history_it_never_read_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let held = RESIDENT_CONVERSATION_TAIL + 60;
        let (store, agent_id) = store_with_conversation(&dir.path().join("tasks"), held);

        let mut reloaded = reload_run(&store, "run-1");
        reloaded.agents[0].thread.post_user("one more", None, NOW);
        store.save_run(&reloaded).expect("the append saves");

        assert_eq!(
            store.thread_item_count(&agent_id).expect("the count reads"),
            held as u64 + 1,
            "the conversation lost the history the daemon never read"
        );
        assert_eq!(
            store
                .thread_page(&agent_id, Some(2), 1)
                .expect("a page reads")
                .first()
                .map(ThreadItem::sequence),
            Some(1),
            "the oldest item is still there"
        );
    }

    /// The whole point of paging: a client can walk a long conversation
    /// backward a page at a time and see every item exactly once, in order,
    /// without ever asking for the conversation whole.
    #[test]
    fn paging_backward_reaches_every_item_exactly_once_and_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let (store, agent_id) = store_with_conversation(&dir.path().join("tasks"), 60);

        let mut walked: Vec<u64> = Vec::new();
        let mut before = None;
        loop {
            let page = store
                .thread_page(&agent_id, before, 17)
                .expect("a page reads");
            if page.is_empty() {
                break;
            }
            let page_sequences = sequences(&page);
            assert!(
                page_sequences.windows(2).all(|pair| pair[0] < pair[1]),
                "a page came back out of order: {page_sequences:?}"
            );
            before = page_sequences.first().copied();
            // Pages arrive newest-first, so the walk builds the conversation
            // from the front.
            walked.splice(0..0, page_sequences);
        }

        let every_sequence: Vec<u64> = (1..=60).collect();
        assert_eq!(
            walked, every_sequence,
            "the backward walk missed, repeated or reordered items"
        );
    }

    /// A short conversation is not a special case — a page wider than the
    /// conversation is simply the whole of it.
    #[test]
    fn a_page_wider_than_the_conversation_returns_all_of_it() {
        let dir = tempfile::tempdir().unwrap();
        let (store, agent_id) = store_with_conversation(&dir.path().join("tasks"), 4);

        let page = store
            .thread_page(&agent_id, None, 500)
            .expect("a page reads");
        assert_eq!(sequences(&page), vec![1, 2, 3, 4]);
    }

    /// The end of the walk. Asking for what precedes the oldest item is the
    /// ordinary way a client learns the conversation has no more history, so
    /// it answers empty rather than failing or wrapping around.
    #[test]
    fn paging_before_the_oldest_item_returns_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (store, agent_id) = store_with_conversation(&dir.path().join("tasks"), 10);

        let oldest = store
            .thread_page(&agent_id, None, 10)
            .expect("a page reads")
            .first()
            .map(ThreadItem::sequence)
            .expect("the conversation has an oldest item");
        let page = store
            .thread_page(&agent_id, Some(oldest), 10)
            .expect("a page reads");
        assert!(page.is_empty(), "{:?}", sequences(&page));
    }

    /// The forward cursor is not a `sequence > ?` filter. Marking a message
    /// seen moves its `updated_sequence` and leaves its creation sequence
    /// where it was, and the client polling from a cursor past that creation
    /// sequence still has to be told.
    #[test]
    fn the_forward_cursor_returns_a_message_mutated_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("tasks");
        let store = Store::new(&root).expect("store opens");
        let mut record = run_record("run-1", None, NOW);
        record.agents[0].thread.post_user("read me", None, NOW);
        record.agents[0].thread.post_agent("on it", None, NOW);
        store.save_run(&record).expect("the conversation saves");
        let agent_id = record.agents[0].id.clone();
        let cursor = record.agents[0].thread.last_sequence();

        assert!(
            store
                .thread_items_after(&agent_id, cursor)
                .expect("the cursor reads")
                .is_empty(),
            "nothing has happened since the cursor yet"
        );

        let seen = record.agents[0].thread.read_unread(NOW);
        assert_eq!(seen.len(), 1, "there is one unread message to mark seen");
        store.save_run(&record).expect("the mutation saves");

        let delta = store
            .thread_items_after(&agent_id, cursor)
            .expect("the cursor reads");
        assert_eq!(
            sequences(&delta),
            vec![1],
            "the message whose updated_sequence moved did not come back"
        );
    }

    /// How long a conversation is, without reading it. The client needs the
    /// total to know whether it holds the whole thing; loading the items to
    /// count them would undo the paging it pays for.
    #[test]
    fn counting_a_conversation_does_not_load_it() {
        let dir = tempfile::tempdir().unwrap();
        let (store, agent_id) = store_with_conversation(&dir.path().join("tasks"), 60);

        assert_eq!(
            store.thread_item_count(&agent_id).expect("the count reads"),
            60
        );
        assert_eq!(
            store
                .thread_page(&agent_id, None, 5)
                .expect("a page reads")
                .len(),
            5,
            "the count is not the size of what a read returns"
        );
        assert_eq!(
            store
                .thread_item_count("no-such-agent")
                .expect("the count reads"),
            0
        );
    }

    /// The contract underneath every paging test above: both reads seek into
    /// the conversation rather than walking it. A predicate SQLite cannot
    /// answer from an index — or an `ORDER BY` it has to satisfy with a sort —
    /// reads every row of a 600-item conversation to hand back ten of them,
    /// and the returned page looks identical either way.
    #[test]
    fn the_conversation_reads_seek_instead_of_walking() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        let connection = store.connection();
        for statement in [
            THREAD_PAGE_SQL,
            THREAD_CURSOR_SQL,
            THREAD_MESSAGE_PAGE_SQL,
            THREAD_CONVERSATION_FLOOR_SQL,
            THREAD_CONVERSATION_PAGE_SQL,
        ] {
            let mut explain = connection
                .prepare(&format!("EXPLAIN QUERY PLAN {statement}"))
                .expect("the statement prepares");
            // The plan does not depend on what the parameters hold, only on
            // how many there are.
            let placeholders = vec![1_i64; explain.parameter_count()];
            let plan: Vec<String> = explain
                .query_map(rusqlite::params_from_iter(placeholders), |row| {
                    row.get::<_, String>(3)
                })
                .expect("the plan reads")
                .collect::<Result<_, _>>()
                .expect("the plan reads");

            assert!(
                plan.iter().any(|step| step.contains("SEARCH")),
                "{statement} does not seek: {plan:?}"
            );
            assert!(
                !plan
                    .iter()
                    .any(|step| step.contains("SCAN") || step.contains("TEMP B-TREE")),
                "{statement} walks or sorts the conversation: {plan:?}"
            );
        }
    }

    /// A panic anywhere under the connection lock poisons the mutex, and a
    /// store that treats poison as fatal answers every later call with a panic
    /// of its own. The daemon would stay up and connected while it could
    /// neither read nor write a thing, which is a far worse failure than the
    /// one panic that started it.
    #[test]
    fn a_panic_under_the_connection_lock_does_not_wedge_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        store
            .save_run(&run_record("run-1", None, NOW))
            .expect("the run saves");

        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = store.connection();
            panic!("a store call panicked while it held the connection");
        }));
        std::panic::set_hook(previous_hook);
        assert!(panicked.is_err(), "the test did not poison the mutex");

        store
            .save_run(&run_record("run-2", None, NOW))
            .expect("a write after the poisoning still lands");
        assert_eq!(
            store
                .load_all_runs()
                .expect("a read after the poisoning still runs")
                .len(),
            2
        );
    }

    /// The forward cursor's index by name. `thread_items_cursor` exists only
    /// for this query — the primary key already covers `agent_id` — so a plan
    /// that no longer names it means the index is dead weight on every write.
    #[test]
    fn the_forward_cursor_reads_through_its_own_index() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        let connection = store.connection();
        let mut explain = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {THREAD_CURSOR_SQL}"))
            .expect("the statement prepares");
        let placeholders = vec![1_i64; explain.parameter_count()];
        let plan: Vec<String> = explain
            .query_map(rusqlite::params_from_iter(placeholders), |row| {
                row.get::<_, String>(3)
            })
            .expect("the plan reads")
            .collect::<Result<_, _>>()
            .expect("the plan reads");
        assert!(
            plan.iter().any(|step| step.contains("thread_items_cursor")),
            "the forward cursor does not use thread_items_cursor: {plan:?}"
        );
    }

    /// The conversation index by name. The primary key would answer this
    /// statement too — by walking every tool call between the words, which is
    /// the whole cost the partial index exists to skip — and the page it
    /// returned would look identical either way. So the plan is pinned to the
    /// index, not merely to a seek.
    #[test]
    fn the_message_page_reads_through_the_conversation_index() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        let connection = store.connection();
        let mut explain = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {THREAD_MESSAGE_PAGE_SQL}"))
            .expect("the statement prepares");
        let placeholders = vec![1_i64; explain.parameter_count()];
        let plan: Vec<String> = explain
            .query_map(rusqlite::params_from_iter(placeholders), |row| {
                row.get::<_, String>(3)
            })
            .expect("the plan reads")
            .collect::<Result<_, _>>()
            .expect("the plan reads");
        assert!(
            plan.iter()
                .any(|step| step.contains("thread_items_conversation")),
            "the message page does not use thread_items_conversation: {plan:?}"
        );

        // The same for the seek that finds where a page reaches back to: on
        // the primary key it would count every tool call on the way down.
        let mut explain = connection
            .prepare(&format!(
                "EXPLAIN QUERY PLAN {THREAD_CONVERSATION_FLOOR_SQL}"
            ))
            .expect("the statement prepares");
        let placeholders = vec![1_i64; explain.parameter_count()];
        let plan: Vec<String> = explain
            .query_map(rusqlite::params_from_iter(placeholders), |row| {
                row.get::<_, String>(3)
            })
            .expect("the plan reads")
            .collect::<Result<_, _>>()
            .expect("the plan reads");
        assert!(
            plan.iter()
                .any(|step| step.contains("thread_items_conversation")),
            "the page floor does not use thread_items_conversation: {plan:?}"
        );
    }

    /// The failure the daemon dies on. Under a KeepAlive supervisor it dies on
    /// it once a second, so the message is the only thing standing between the
    /// user and a silent restart loop: it has to name the file.
    #[test]
    fn a_store_that_cannot_be_opened_names_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let occupied = dir.path().join("tasks");
        std::fs::write(&occupied, "not a directory").unwrap();

        let Err(error) = Store::new(&occupied) else {
            panic!("a file where the store directory belongs must fail to open");
        };
        let message = error.to_string();
        assert!(
            message.contains(&occupied.display().to_string()),
            "the failure does not name the store: {message}"
        );
    }

    /// A run that belongs to an Issue is filed under it by `save_run` alone —
    /// there is no second write path for implementations, and the `issue_id`
    /// column comes off `plan_id` whichever way the run got here.
    #[test]
    fn save_run_files_a_run_under_the_issue_its_plan_id_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        store
            .save_issue_plan(&plan_record("plan-1"))
            .expect("the Issue saves");
        store
            .save_run(&run_record("run-1", Some("plan-1"), NOW))
            .expect("the run saves");

        let issues = store.load_all_issues().expect("issues load");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].implementations.len(), 1);
        assert_eq!(issues[0].implementations[0].id, "run-1");
    }

    /// Removing an agent from the roster takes its conversation with it — a
    /// left-behind row would reappear on the next load as an agent the entity
    /// no longer has.
    #[test]
    fn removing_an_agent_removes_its_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        let mut record = run_record("run-1", None, NOW);
        let mut roster = AgentRoster::restore(
            "run-1",
            record.agents.clone(),
            Default::default(),
            ModelChoice::default(),
            NOW,
        );
        let second = roster.add("run-1", ModelChoice::default(), NOW).id.clone();
        roster
            .by_id_mut(&second)
            .expect("the second agent is on the roster")
            .thread
            .post_user("only agent two hears this", None, NOW);
        record.agents = roster.agents().to_vec();
        store.save_run(&record).expect("both agents save");
        assert_eq!(reload_run(&store, "run-1").agents.len(), 2);

        roster
            .remove(&second)
            .expect("the second agent is removable");
        record.agents = roster.agents().to_vec();
        store.save_run(&record).expect("the shortened roster saves");

        let after = reload_run(&store, "run-1");
        assert_eq!(after.agents.len(), 1);
        assert!(after.agents.iter().all(|agent| agent.id != second));
    }

    /// Boot reattaches in creation order, so the loaders have to hand records
    /// back oldest first — a plan is recovered before the run that reads its
    /// record.
    #[test]
    fn records_load_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        for (id, created) in [
            ("run-late", "2026-08-21T12:00:00Z"),
            ("run-early", "2026-08-21T08:00:00Z"),
            ("run-middle", "2026-08-21T10:00:00Z"),
        ] {
            store
                .save_run(&run_record(id, None, created))
                .expect("the run saves");
        }
        let order: Vec<String> = store
            .load_all_runs()
            .expect("runs load")
            .into_iter()
            .map(|run| run.id)
            .collect();
        assert_eq!(order, vec!["run-early", "run-middle", "run-late"]);
    }

    /// Deleting an Issue takes its canonical plan docs with it. They live
    /// outside the database on purpose, so deleting only rows would leave the
    /// plan on disk forever with nothing referring to it.
    #[test]
    fn deleting_an_issue_removes_its_canonical_docs_too() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("tasks");
        let store = Store::new(&root).expect("store opens");
        let docs = root.join("issues").join("plan-1").join("docs");
        std::fs::create_dir_all(docs.join(".build/plan")).unwrap();
        std::fs::write(docs.join(".build/plan/01-stage.md"), "# stage").unwrap();

        store.delete_plan("plan-1").expect("the delete succeeds");
        assert!(
            !root.join("issues").join("plan-1").exists(),
            "the Issue's docs outlived the Issue"
        );
        store
            .delete_plan("plan-1")
            .expect("deleting twice is the same as deleting once");
    }

    /// A conversation is loaded as its newest items, so the badge has to count
    /// what is under them. This is the query that does it — and it counts only
    /// what calls the human, only between the cursor and the tail.
    #[test]
    fn unread_under_the_tail_is_counted_in_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        let mut record = run_record("run-1", None, NOW);
        let agent_id = record.agents[0].id.clone();
        // Agent messages call the human; status events do not.
        for n in 0..10 {
            record.agents[0]
                .thread
                .post_agent(format!("said {n}"), None, NOW);
            record.agents[0].thread.push_event(
                crate::thread::ThreadEventKind::Triaged,
                None,
                None,
                None,
                NOW,
            );
        }
        store.save_run(&record).expect("the run saves");
        let floor = record.agents[0].thread.last_sequence() + 1;

        assert_eq!(
            store
                .unread_attention_between(&agent_id, 0, floor)
                .expect("the count runs"),
            10,
            "only the items that call the human are counted"
        );
        // A cursor inside the conversation counts only what is above it.
        let midpoint = record.agents[0].thread.items[9].sequence();
        let above = store
            .unread_attention_between(&agent_id, midpoint, floor)
            .expect("the count runs");
        assert!(above < 10 && above > 0, "counted {above} above the cursor");
        // A conversation held whole has no history under it to ask about.
        assert_eq!(
            store
                .unread_attention_between(&agent_id, 0, 0)
                .expect("the count runs"),
            0
        );
    }

    /// A conversation buried in activity still hands its messages back, and
    /// hands back nothing else: the query the catch-up packet reads through,
    /// against exactly the thread that starved it.
    #[test]
    fn the_message_page_reads_past_the_activity_between_the_words() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        let mut record = run_record("run-1", None, NOW);
        record.agents[0]
            .thread
            .post_user("please rename the helper", None, NOW);
        for index in 0..300 {
            record.agents[0].thread.push_event(
                crate::thread::ThreadEventKind::ToolUse,
                Some(format!("Read file-{index}.rs")),
                None,
                None,
                NOW,
            );
        }
        record.agents[0].thread.post_outcome(
            crate::thread::MessageOutcome::Blocked,
            "needs production credentials",
            None,
            NOW,
        );
        record.agents[0].thread.push_event(
            crate::thread::ThreadEventKind::Interrupted,
            Some("the daemon restarted".to_string()),
            None,
            None,
            NOW,
        );
        store.save_run(&record).expect("the conversation saves");
        let agent_id = record.agents[0].id.clone();

        let messages = store
            .thread_message_page(&agent_id, 40)
            .expect("the message page reads");
        let bodies: Vec<String> = messages
            .iter()
            .map(|item| match item {
                ThreadItem::Message(message) => message.body.clone(),
                ThreadItem::Event(event) => panic!("an event came back: {event:?}"),
            })
            .collect();
        assert_eq!(
            bodies,
            vec![
                "please rename the helper".to_string(),
                "needs production credentials".to_string(),
            ],
            "the words are handed back oldest-first, activity and observations left behind"
        );
    }

    /// The limit counts messages and keeps the newest of them, whatever sits
    /// between.
    #[test]
    fn the_message_page_keeps_the_newest_messages_up_to_its_limit() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        let mut record = run_record("run-1", None, NOW);
        for index in 0..10 {
            record.agents[0]
                .thread
                .post_user(format!("ask {index}"), None, NOW);
            record.agents[0].thread.push_event(
                crate::thread::ThreadEventKind::Reasoning,
                Some("thinking".to_string()),
                None,
                None,
                NOW,
            );
        }
        store.save_run(&record).expect("the conversation saves");

        let messages = store
            .thread_message_page(&record.agents[0].id, 3)
            .expect("the message page reads");
        assert_eq!(sequences(&messages), vec![15, 17, 19], "{messages:?}");
    }

    /// The two readings of the counted rule — `ThreadItem::counted()` and the
    /// store's `message = 1 OR attention = 1` — held equal over every kind
    /// there is, so the hoisted columns cannot drift from the enum they were
    /// written off.
    #[test]
    fn the_hoisted_columns_agree_with_the_counted_rule() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        let mut record = run_record("run-1", None, NOW);
        for kind in crate::thread::ThreadEventKind::ALL {
            record.agents[0].thread.push_event(
                kind,
                Some(kind.as_str().to_string()),
                None,
                None,
                NOW,
            );
        }
        record.agents[0].thread.post_user("a question", None, NOW);
        record.agents[0].thread.post_agent("an answer", None, NOW);
        record.agents[0]
            .thread
            .post_agent_progress("still going", None, NOW);
        store.save_run(&record).expect("the conversation saves");

        let counted_in_sql: Vec<u64> = {
            let connection = store.connection();
            let mut statement = connection
                .prepare(
                    "SELECT sequence FROM thread_items \
                     WHERE agent_id = ?1 AND (message = 1 OR attention = 1) ORDER BY sequence",
                )
                .expect("the predicate prepares");
            statement
                .query_map([&record.agents[0].id], |row| row.get::<_, i64>(0))
                .expect("the predicate reads")
                .map(|sequence| sequence.expect("a row reads") as u64)
                .collect()
        };
        let counted_in_rust: Vec<u64> = record.agents[0]
            .thread
            .items
            .iter()
            .filter(|item| item.counted())
            .map(ThreadItem::sequence)
            .collect();

        assert!(!counted_in_rust.is_empty(), "the fixture counts nothing");
        assert_eq!(counted_in_sql, counted_in_rust);
    }

    /// A stored page is measured the same way a resident one is: its limit
    /// buys conversation, the activity between two messages travels with them,
    /// and `has_more` answers for items of any kind below what was shipped.
    #[test]
    fn a_stored_page_is_measured_in_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        let mut record = run_record("run-1", None, NOW);
        for turn in 0..10 {
            record.agents[0]
                .thread
                .post_user(format!("ask {turn}"), None, NOW);
            for index in 0..5 {
                record.agents[0].thread.push_event(
                    crate::thread::ThreadEventKind::ToolUse,
                    Some(format!("Read file-{turn}-{index}.rs")),
                    None,
                    None,
                    NOW,
                );
            }
        }
        store.save_run(&record).expect("the conversation saves");
        let agent_id = record.agents[0].id.clone();

        let (page, has_more) = store
            .thread_conversation_page(&agent_id, None, 3)
            .expect("a page reads");
        let shipped = sequences(&page);
        assert_eq!(
            page.iter().filter(|item| item.counted()).count(),
            3,
            "the limit counts conversation: {shipped:?}"
        );
        assert!(
            shipped.len() > 3,
            "the activity between the messages rides with them: {shipped:?}"
        );
        assert_eq!(
            shipped,
            (*shipped.first().unwrap()..=60).collect::<Vec<u64>>(),
            "a page is one contiguous run, oldest-first"
        );
        assert!(has_more, "there is history below this page");

        // Pages abut at their seeks: the walk sees every item exactly once.
        let mut walked = shipped;
        let mut before = walked.first().copied();
        loop {
            let (page, has_more) = store
                .thread_conversation_page(&agent_id, before, 3)
                .expect("a page reads");
            let shipped = sequences(&page);
            assert!(
                !shipped.is_empty(),
                "a page below {before:?} came back empty"
            );
            walked.splice(0..0, shipped);
            if !has_more {
                break;
            }
            before = walked.first().copied();
        }
        assert_eq!(walked, (1..=60).collect::<Vec<u64>>());
    }

    /// The ceiling holds in SQL too: an all-activity stretch ends the page
    /// early rather than reading an unbounded span of it.
    #[test]
    fn a_stored_page_of_pure_activity_stops_at_the_ceiling() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        let mut record = run_record("run-1", None, NOW);
        record.agents[0].thread.post_user("rename it", None, NOW);
        for index in 0..500 {
            record.agents[0].thread.push_event(
                crate::thread::ThreadEventKind::ToolUse,
                Some(format!("Read file-{index}.rs")),
                None,
                None,
                NOW,
            );
        }
        store.save_run(&record).expect("the conversation saves");

        let (page, has_more) = store
            .thread_conversation_page(&record.agents[0].id, None, 4)
            .expect("a page reads");
        assert_eq!(page.len(), 4 * crate::thread::THREAD_PAGE_SPAN_FACTOR);
        assert!(has_more, "the walk stopped early and says so");
    }

    /// A v2 database gains the message column and is classified in place, the
    /// way v1 gained attention. Nobody's stored conversation has to be
    /// rewritten for the packet to read it.
    #[test]
    fn a_v2_database_is_migrated_and_its_messages_classified() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("tasks");
        let agent_id;
        {
            let store = Store::new(&root).expect("store opens");
            let mut record = run_record("run-1", None, NOW);
            agent_id = record.agents[0].id.clone();
            record.agents[0].thread.post_user("said before", None, NOW);
            record.agents[0].thread.push_event(
                crate::thread::ThreadEventKind::ToolUse,
                Some("Read a file".to_string()),
                None,
                None,
                NOW,
            );
            store.save_run(&record).expect("the run saves");
            store.pretend_to_be_v2();
        }
        let migrated = Store::new(&root).expect("a v2 store opens");

        assert_eq!(
            sequences(
                &migrated
                    .thread_message_page(&agent_id, 40)
                    .expect("the message page reads")
            ),
            vec![1],
            "the backfill classified the items already stored"
        );
    }

    /// A v1 database gains the attention column and is classified in place —
    /// the one real installation is a v1 database, so this path is the only one
    /// that will ever run on it.
    #[test]
    fn a_v1_database_is_migrated_and_its_items_classified() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("tasks");
        let agent_id;
        let floor;
        {
            let store = Store::new(&root).expect("store opens");
            let mut record = run_record("run-1", None, NOW);
            agent_id = record.agents[0].id.clone();
            record.agents[0]
                .thread
                .post_agent("look at this", None, NOW);
            store.save_run(&record).expect("the run saves");
            floor = record.agents[0].thread.last_sequence() + 1;
            // Put it back the way a v1 store looks: no attention column, and a
            // schema version that says so.
            store.pretend_to_be_v1();
        }
        let migrated = Store::new(&root).expect("a v1 store opens");
        assert_eq!(
            migrated
                .unread_attention_between(&agent_id, 0, floor)
                .expect("the count runs"),
            1,
            "the backfill classified the items already stored"
        );
        assert_eq!(
            sequences(
                &migrated
                    .thread_message_page(&agent_id, 40)
                    .expect("the message page reads")
            ),
            vec![1],
            "a v1 store arrives at v3, so both classifiers ran on it"
        );
        assert_eq!(
            migrated.load_all_runs().expect("runs load")[0].agents[0]
                .thread
                .items
                .len(),
            1,
            "the migration did not disturb the conversation"
        );
    }

    /// A backup is one self-contained file taken from a consistent read, so a
    /// copy made while the daemon is writing is a database rather than a torn
    /// one — the thing a file-at-a-time tool can no longer do for itself.
    #[test]
    fn a_backup_is_a_whole_store_and_never_silently_replaces_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        let mut record = run_record("run-1", None, NOW);
        record.agents[0].thread.post_user("keep me", None, NOW);
        store.save_run(&record).expect("the run saves");

        let backup = dir.path().join("backups").join("store.db");
        store.backup_to(&backup).expect("the backup is written");
        assert!(backup.is_file(), "the backup created its parent directory");

        // The copy stands on its own: opened as a store, it holds the work.
        let restored = Store::new(dir.path().join("restored")).expect("store opens");
        drop(restored);
        std::fs::copy(&backup, dir.path().join("restored").join("build.db")).unwrap();
        let restored = Store::new(dir.path().join("restored")).expect("the backup opens");
        let runs = restored.load_all_runs().expect("runs load");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].agents[0].thread.items.len(), 1);

        // Overwriting is refused: a backup that replaced the previous one
        // silently is one that can be lost twice.
        let refused = store.backup_to(&backup).expect_err("the second is refused");
        assert!(refused.to_string().contains("already exists"), "{refused}");
    }

    /// The attention map is pruned to the entities that still exist, so it
    /// tracks the world rather than growing forever.
    #[test]
    fn saving_attention_prunes_entities_that_no_longer_exist() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        let mut map = HashMap::new();
        map.insert("run-live".to_string(), Attention::default());
        map.insert("run-gone".to_string(), Attention::default());
        let live: HashSet<String> = ["run-live".to_string()].into_iter().collect();

        store.save_attention(&map, &live).expect("attention saves");
        let loaded = store.load_attention();
        assert!(loaded.contains_key("run-live"));
        assert!(!loaded.contains_key("run-gone"), "a dead id was kept");
    }

    /// A database written by a newer bridge is refused, and refused WITHOUT
    /// being written to — the point of the guard is to leave it untouched.
    #[test]
    fn a_newer_schema_is_refused_rather_than_downgraded() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("tasks");
        {
            let store = Store::new(&root).expect("store opens");
            store.set_schema_version(SCHEMA_VERSION + 1);
        }
        match Store::new(&root) {
            Err(StoreError::SchemaTooNew { found, supported }) => {
                assert_eq!(found, SCHEMA_VERSION + 1);
                assert_eq!(supported, SCHEMA_VERSION);
            }
            Err(other) => panic!("wrong refusal: {other}"),
            Ok(_) => panic!("a store from a newer bridge was opened anyway"),
        }
    }

    /// The import runs once, imports everything, and leaves the JSON where it
    /// was — which is what makes throwing the database away a real recovery.
    /// Runs on every `cargo test`: the one-way door is the change's riskiest
    /// step, so it cannot be covered only by a test that needs a real store.
    #[test]
    fn the_json_import_runs_once_and_leaves_the_records_it_read() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("tasks");
        let mut issue_record = run_record("run-in-issue", Some("plan-1"), NOW);
        issue_record.agents[0]
            .thread
            .post_user("carried across", None, NOW);
        std::fs::create_dir_all(root.join("issues/plan-1")).unwrap();
        std::fs::write(
            root.join("issues/plan-1/record.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "issue": plan_record("plan-1"),
                "implementations": [issue_record],
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(root.join("runs")).unwrap();
        std::fs::write(
            root.join("runs/run-planless.json"),
            serde_json::to_string_pretty(&run_record("run-planless", None, NOW)).unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(root.join("attention")).unwrap();
        std::fs::write(
            root.join("attention/map.json"),
            serde_json::to_string(&HashMap::from([(
                "plan-1".to_string(),
                Attention::default(),
            )]))
            .unwrap(),
        )
        .unwrap();

        let store = Store::new(&root).expect("store opens");
        assert_eq!(store.import_json_store().expect("the import runs"), 4);
        assert_eq!(store.load_all_issues().expect("issues load").len(), 1);
        assert_eq!(store.load_all_runs().expect("runs load").len(), 2);
        assert!(store.load_attention().contains_key("plan-1"));
        assert_eq!(
            store.load_all_runs().expect("runs load")[0].agents[0]
                .thread
                .items
                .len(),
            1,
            "the imported conversation came with its record"
        );

        assert_eq!(
            store.import_json_store().expect("a second import runs"),
            0,
            "the import is one-way"
        );
        assert!(
            root.join("issues/plan-1/record.json").is_file(),
            "the import moved the records it read"
        );

        // Deleting the database deletes the marker with it, so the untouched
        // JSON rebuilds the store. This is the documented recovery.
        drop(store);
        for sidecar in ["build.db", "build.db-wal", "build.db-shm"] {
            let _ = std::fs::remove_file(root.join(sidecar));
        }
        let rebuilt = Store::new(&root).expect("store reopens");
        assert_eq!(rebuilt.import_json_store().expect("the rebuild imports"), 4);
        assert_eq!(rebuilt.load_all_runs().expect("runs load").len(), 2);
    }

    /// A capture is durable before anything is decided about it: what the user
    /// said survives a store that is opened again from scratch, routing and
    /// all.
    #[test]
    fn a_capture_round_trips_with_everything_decided_about_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        let mut capture = Capture::new(
            "capture-1",
            "fix the login redirect",
            "2026-08-13T10:00:00Z",
        );
        capture.state = CaptureState::Routed;
        capture.routing = Some(CaptureRouting {
            project_id: "p1".to_string(),
            kind: CaptureTarget::Issue,
            target_id: "plan-7".to_string(),
            routed_at: "2026-08-13T10:00:05Z".to_string(),
            rationale: Some("no branch names this work".to_string()),
        });
        store.save_capture(&capture).unwrap();

        let reopened = Store::new(dir.path().join("tasks")).expect("store opens");
        assert_eq!(reopened.load_all_captures().unwrap(), vec![capture]);
    }

    /// Re-saving a capture replaces it in place rather than filing a second
    /// copy: one capture, one record, however many times routing touches it.
    #[test]
    fn saving_a_capture_again_replaces_the_record() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        let mut capture = Capture::new("capture-1", "ship it", "2026-08-13T10:00:00Z");
        store.save_capture(&capture).unwrap();
        capture.state = CaptureState::Routing;
        store.save_capture(&capture).unwrap();

        let loaded = store.load_all_captures().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].state, CaptureState::Routing);
    }

    /// Captures come back oldest first, so the order they were said in is the
    /// order they are read in.
    #[test]
    fn captures_load_in_the_order_they_were_said() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        for (id, said_at) in [
            ("capture-b", "2026-08-13T10:00:02Z"),
            ("capture-a", "2026-08-13T10:00:01Z"),
            ("capture-c", "2026-08-13T10:00:03Z"),
        ] {
            store
                .save_capture(&Capture::new(id, "something", said_at))
                .unwrap();
        }
        let ids: Vec<String> = store
            .load_all_captures()
            .unwrap()
            .into_iter()
            .map(|capture| capture.id)
            .collect();
        assert_eq!(ids, vec!["capture-a", "capture-b", "capture-c"]);
    }

    /// A capture the user abandoned is gone, and gone across a reboot: the one
    /// deletion this store does, and it happens only when they asked for it.
    #[test]
    fn a_cancelled_capture_is_forgotten_and_stays_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        store
            .save_capture(&Capture::new(
                "capture-1",
                "ship it",
                "2026-08-13T10:00:00Z",
            ))
            .unwrap();
        store
            .save_capture(&Capture::new(
                "capture-2",
                "and this",
                "2026-08-13T10:00:01Z",
            ))
            .unwrap();

        store.delete_capture("capture-1").unwrap();
        let ids: Vec<String> = store
            .load_all_captures()
            .unwrap()
            .into_iter()
            .map(|capture| capture.id)
            .collect();
        assert_eq!(ids, vec!["capture-2"], "only the one asked for");

        store
            .delete_capture("capture-1")
            .expect("forgetting what is already forgotten is not an error");
    }

    /// No captures dir means no captures — a first boot is not an error.
    #[test]
    fn a_store_with_no_captures_yet_loads_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        assert_eq!(store.load_all_captures().unwrap(), Vec::new());
    }

    /// A capture record that will not parse fails the boot that read it. The
    /// text is the one thing the user cannot re-derive, so dropping it quietly
    /// is the one thing the store must never do.
    #[test]
    fn an_unreadable_capture_record_fails_fast() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks")).expect("store opens");
        store.corrupt_capture_row("capture-1");
        assert!(matches!(
            store.load_all_captures(),
            Err(StoreError::Corrupt { .. })
        ));
    }
}
