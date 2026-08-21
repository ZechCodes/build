//! Durable persistence for Issues, under the bridge state dir
//! (`~/.build/tasks/` by default, next to the identity file):
//!
//! ```text
//! issues/<issue_id>/record.json one aggregate: planning + implementation lineage
//! issues/<issue_id>/docs/…      canonical stage-plan docs
//! runs/<run_id>.json            planless adopted-worktree conversations only
//! plans/… / <task_id>.json      legacy formats; boot migrates and tombstones them
//! ```
//!
//! Every record holds the *durable core* of its entity — identity, project,
//! lifecycle state, worktree/branch bookkeeping, timestamps, and the last
//! `done` summary. Records are written atomically (tmp file + fsync + rename)
//! on every state transition, so a daemon crash never leaves a half-written
//! record, and read back in full on boot so a restart re-attaches every plan
//! and run instead of orphaning the worktrees that survived on disk.
//!
//! Plan docs live here as the **source of truth** (spec: Plan/Run Split):
//! the scratch docs dir is disposable, so `ingest_plan_docs` (scratch → store)
//! is fail-fast — a plan never advances with unpersisted docs — and
//! `materialize_plan_docs` (store → worktree) recreates the docs for run
//! dispatch and plan-revision sessions.
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
use crate::thread::Thread;

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
    /// The database was written by a newer bridge. Opening it read-write would
    /// corrupt state that build does not understand, so boot refuses.
    #[error("store schema version {found} is newer than this bridge supports ({supported}) — update build-bridge")]
    SchemaTooNew { found: i64, supported: i64 },
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
    /// record written before agents existed — [`Store::migrate_threads_to_agents`]
    /// (and `AgentRoster::restore` on reattach) fills it from `legacy_thread`.
    #[serde(default)]
    pub agents: Vec<Agent>,
    /// The pre-agent, entity-keyed conversation. Read once, by the migration
    /// that moves it onto the first agent, and empty on every record written
    /// since.
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
    item             TEXT NOT NULL,
    PRIMARY KEY (agent_id, sequence)
);
CREATE INDEX IF NOT EXISTS thread_items_cursor
    ON thread_items(agent_id, updated_sequence);

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

/// The schema this build writes. A stored value ahead of this one means the
/// database was written by a newer bridge; opening it read-write would corrupt
/// what that build knows, so the daemon refuses rather than guessing.
pub const SCHEMA_VERSION: i64 = 1;

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
        std::fs::create_dir_all(&dir)?;
        let conn = Connection::open(dir.join(DB_FILE))?;
        // WAL is what makes an append cheap: the reader keeps reading while the
        // writer commits, and a commit appends to the log instead of rewriting
        // the page it touched. `synchronous = NORMAL` under WAL fsyncs at
        // checkpoint rather than per commit — durable against process death,
        // which is the failure the old tmp+rename+fsync was guarding, and it
        // gives up only the very last commits to a power cut.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        let stored: Option<i64> = conn
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        match stored {
            Some(found) if found > SCHEMA_VERSION => {
                return Err(StoreError::SchemaTooNew {
                    found,
                    supported: SCHEMA_VERSION,
                })
            }
            Some(_) => {}
            None => {
                conn.execute(
                    "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)",
                    [SCHEMA_VERSION.to_string()],
                )?;
            }
        }
        Ok(Store {
            dir,
            conn: Arc::new(Mutex::new(conn)),
            #[cfg(test)]
            fail_next_write: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
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
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let out = work(&tx)?;
        tx.commit()?;
        Ok(out)
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
        self.conn
            .lock()
            .unwrap()
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
        self.conn
            .lock()
            .unwrap()
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
        self.conn
            .lock()
            .unwrap()
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
        let conn = self.conn.lock().unwrap();
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
    /// conversation is not: items are upserted by `(agent_id, sequence)`, so an
    /// append writes ONE row and a mutated item replaces ONE row. That is the
    /// whole reason this store exists — the JSON records it replaces rewrote
    /// every conversation on the Issue for every append.
    fn write_agents(
        tx: &rusqlite::Transaction,
        owner_id: &str,
        agents: &[Agent],
    ) -> Result<(), StoreError> {
        let keep: Vec<&str> = agents.iter().map(|agent| agent.id.as_str()).collect();
        let mut stale = tx.prepare("SELECT id FROM agents WHERE owner_id = ?1")?;
        let existing: Vec<String> = stale
            .query_map([owner_id], |row| row.get::<_, String>(0))?
            .flatten()
            .collect();
        drop(stale);
        for gone in existing.iter().filter(|id| !keep.contains(&id.as_str())) {
            tx.execute("DELETE FROM thread_items WHERE agent_id = ?1", [gone])?;
            tx.execute("DELETE FROM agents WHERE id = ?1", [gone])?;
        }

        let mut upsert_agent = tx.prepare(
            "INSERT INTO agents (id, owner_id, ordinal, record) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(id) DO UPDATE SET owner_id = ?2, ordinal = ?3, record = ?4",
        )?;
        let mut upsert_item = tx.prepare(
            "INSERT INTO thread_items (agent_id, sequence, updated_sequence, item)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(agent_id, sequence) DO UPDATE SET updated_sequence = ?3, item = ?4",
        )?;
        let mut held = tx.prepare("SELECT sequence FROM thread_items WHERE agent_id = ?1")?;
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
            for item in &items {
                upsert_item.execute(rusqlite::params![
                    agent.id,
                    item.sequence() as i64,
                    item.latest_sequence() as i64,
                    serde_json::to_string(item).expect("a thread item always serializes")
                ])?;
            }
            // An item can be deleted from a conversation (a withdrawn draft),
            // so sequences the roster no longer holds are dropped rather than
            // left behind to reappear on the next load.
            let live: Vec<i64> = items.iter().map(|item| item.sequence() as i64).collect();
            let orphans: Vec<i64> = held
                .query_map([&agent.id], |row| row.get::<_, i64>(0))?
                .flatten()
                .filter(|sequence| !live.contains(sequence))
                .collect();
            for orphan in orphans {
                tx.execute(
                    "DELETE FROM thread_items WHERE agent_id = ?1 AND sequence = ?2",
                    rusqlite::params![agent.id, orphan],
                )?;
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
        let mut items =
            conn.prepare("SELECT item FROM thread_items WHERE agent_id = ?1 ORDER BY sequence")?;
        let mut agents = Vec::with_capacity(rows.len());
        for (id, raw) in rows {
            let mut agent: Agent =
                serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                    path: PathBuf::from(format!("agents/{id}")),
                    source,
                })?;
            agent.thread.items = items
                .query_map([&id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<String>, _>>()?
                .into_iter()
                .map(|raw| serde_json::from_str(&raw))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|source| StoreError::Corrupt {
                    path: PathBuf::from(format!("thread_items/{id}")),
                    source,
                })?;
            agents.push(agent);
        }
        Ok(agents)
    }

    // ---- issues and implementations --------------------------------------

    /// Whether an Issue exists. The question `issue_record_path(..).is_file()`
    /// used to answer.
    pub fn issue_exists(&self, issue_id: &str) -> bool {
        self.conn
            .lock()
            .unwrap()
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

    /// Write one implementation and its agents.
    pub fn save_issue_implementation(&self, record: &PersistedRun) -> Result<(), StoreError> {
        if record.plan_id.is_none() {
            return Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "implementation has no issue_id",
            )));
        }
        self.save_run(record)
    }

    /// Every Issue with its implementations, oldest first.
    pub fn load_all_issues(&self) -> Result<Vec<PersistedIssue>, StoreError> {
        let conn = self.conn.lock().unwrap();
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

    /// Delete an Issue, its implementations and every conversation on them.
    pub fn delete_plan(&self, plan_id: &str) -> Result<(), StoreError> {
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
    /// Safe to run on every boot. It does nothing once the marker is set, and
    /// the JSON tree is RENAMED rather than deleted — a database that turns out
    /// to be wrong can be thrown away and rebuilt from what is still on disk.
    /// Returns how many records were imported.
    pub fn import_json_store(&self) -> Result<usize, StoreError> {
        const MARKER: &str = "json_import";
        {
            let conn = self.conn.lock().unwrap();
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
            if let Ok(raw) = std::fs::read_to_string(&attention_path) {
                if let Ok(map) = serde_json::from_str::<HashMap<String, Attention>>(&raw) {
                    let all: HashSet<String> = map.keys().cloned().collect();
                    self.save_attention(&map, &all)?;
                    imported += map.len();
                }
            }
        }

        // Park the imported tree beside the database rather than deleting it.
        // Plan docs and attachments are NOT parked — they stay where they are,
        // because they are still the live store for the things that must be
        // files.
        for parked in [
            "runs",
            "captures",
            "archived-worktrees",
            "attention",
            "plans",
        ] {
            let from = self.dir.join(parked);
            if from.exists() {
                let _ = std::fs::rename(&from, self.dir.join(format!("{parked}.imported")));
            }
        }
        if issues_dir.is_dir() {
            for entry in std::fs::read_dir(&issues_dir)? {
                let record_path = entry?.path().join("record.json");
                if record_path.is_file() {
                    let _ =
                        std::fs::rename(&record_path, record_path.with_extension("json.imported"));
                }
            }
        }

        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            rusqlite::params![MARKER, crate::store::now_rfc3339()],
        )?;
        Ok(imported)
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
