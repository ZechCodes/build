//! Durable persistence for the plan/run split, under the bridge state dir
//! (`~/.build/tasks/` by default, next to the identity file):
//!
//! ```text
//! plans/<plan_id>/record.json   the plan's durable core
//! plans/<plan_id>/docs/…        the plan's canonical docs, in the worktree-
//!                               relative `.build/plan.md` / `.build/plan/*`
//!                               layout (ingest/materialize are straight copies)
//! runs/<run_id>.json            one file per run
//! <task_id>.json                legacy fused-task records (pre-split); boot
//!                               migration splits them and renames to
//!                               `<task_id>.json.migrated`
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
//! planning worktrees are disposable, so `ingest_plan_docs` (worktree → store)
//! is fail-fast — a plan never advances with unpersisted docs — and
//! `materialize_plan_docs` (store → worktree) recreates the docs for run
//! dispatch and plan-revision sessions.
//!
//! Live PTY output streams are intentionally **not** persisted: the terminal is
//! reconstructable observation, not state. What is durable is what the review
//! surfaces need — the lifecycle position and where the files live.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::attention::Attention;
use crate::legacy::{Phase, Stage, StageComment, StageState, TaskKind, TaskState};
use crate::models::AgentProvider;
use crate::plan::{is_worktree_contained_path, PlanState, StageDoc, StageDocState};
use crate::run::{RunState, StageProgress, StageProgressState};

/// Things that can go wrong reading or writing the store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("store io error: {0}")]
    Io(#[from] std::io::Error),
    /// A record file exists but cannot be parsed. Boot fails fast on this — a
    /// silently dropped record would orphan its worktree and lose the user's
    /// work without a trace.
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

/// The durable core of one task, exactly what boot recovery needs to re-attach
/// it. Everything else (PTY sessions, output streams) is rebuilt or lost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedTask {
    pub id: String,
    pub goal: String,
    pub kind: TaskKind,
    /// Canonical path of the project repo the task was dispatched to. Stored as
    /// a path (not the in-memory project id) because project ids are re-minted
    /// on every boot.
    pub project_path: String,
    pub base_branch: String,
    pub state: TaskState,
    pub branch: String,
    pub worktree_name: String,
    pub worktree_path: String,
    pub plan_path: String,
    pub last_summary: Option<String>,
    /// Model/effort the task's agents run on (None = harness default). Added
    /// after the first release: defaulted so pre-existing task files load.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// The most recent thing that went wrong for this task (merge failure, harness
    /// crash), shown on the card until the task advances again. Defaulted so task
    /// files written before this field load as `None`.
    #[serde(default)]
    pub last_error: Option<String>,
    /// Multi-stage: manifest + per-stage sub-state + validation reports. Empty
    /// for legacy single-plan tasks and Quick tasks — empty means "legacy path".
    #[serde(default)]
    pub stages: Vec<Stage>,
    /// The stage whose build/fix/validate session is (or was last) in flight.
    #[serde(default)]
    pub current_stage_id: Option<String>,
    /// The stage a plan-revision session is running for (routes
    /// `Interrupted(Plan)` recovery).
    #[serde(default)]
    pub revising_stage_id: Option<String>,
    /// "Run all": auto-dispatch the next approved stage when validation passes.
    #[serde(default)]
    pub auto_advance: bool,
    /// Persisted per-stage plan comments (flat; each carries its `stage_id`).
    #[serde(default)]
    pub comments: Vec<StageComment>,
    /// True for a task minted around a pre-existing (user-created) worktree.
    /// Defaulted so task files written before adoption existed load as native.
    #[serde(default)]
    pub adopted: bool,
    /// Adoption's warm harness-continuation flag; consumed by the first
    /// session spawn after adoption, persisted so a restart in between keeps it.
    #[serde(default)]
    pub pending_continuation: bool,
    /// RFC 3339 UTC timestamps.
    pub created_at: String,
    pub updated_at: String,
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
    /// The disposable planning worktree, while one is alive (kept warm through
    /// the notes/revision loop). `None` once torn down (approve/abandon) or
    /// before one exists — the store docs are canonical either way.
    #[serde(default)]
    pub worktree_name: Option<String>,
    #[serde(default)]
    pub worktree_path: Option<String>,
    /// The planning worktree's branch (`plan/<slug>`), torn down with it.
    #[serde(default)]
    pub branch: Option<String>,
    /// Worktree-relative path of the single plan doc (`.build/plan.md`).
    pub plan_path: String,
    /// Stage docs: manifest metadata + plan-side review sub-state. Empty for
    /// single-doc plans.
    #[serde(default)]
    pub stages: Vec<StageDoc>,
    /// Persisted per-stage plan comments (flat; each carries its `stage_id`).
    #[serde(default)]
    pub comments: Vec<crate::plan::StageComment>,
    #[serde(default)]
    pub provider: AgentProvider,
    /// Model/effort the plan's agents run on (None = harness default).
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// Durable conversation paired with this plan review artifact.
    #[serde(default)]
    pub thread: crate::thread::Thread,
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
    #[serde(default)]
    pub provider: AgentProvider,
    /// Model/effort the run's agents run on (None = harness default).
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// Durable conversation paired with this run's evolving diff.
    #[serde(default)]
    pub thread: crate::thread::Thread,
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

/// The bridge's JSON record store: plans (record + canonical docs per dir),
/// runs (one file each), and — until the final cutover stage — legacy fused
/// tasks. All writes are atomic and fsync'd.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Store { dir: dir.into() }
    }

    /// Where a task's record lives.
    pub fn path_for(&self, task_id: &str) -> PathBuf {
        self.dir.join(format!("{task_id}.json"))
    }

    /// Where the attention map lives: ONE file for runs, plans and worktrees
    /// alike. A bare worktree has no record of its own — it is discovered by
    /// scanning, not persisted — so attention cannot live on the entity, and
    /// splitting it across two homes would mean two prune rules and two round
    /// trips for one fact.
    fn attention_path(&self) -> PathBuf {
        // In its own directory, like runs/ and plans/: the store ROOT is scanned
        // for legacy task records, and a store-level file sitting there would be
        // read as a corrupt task on every boot.
        self.dir.join("attention").join("map.json")
    }

    /// The attention map. A missing or unparseable file reads as empty: this is
    /// ordering and colour, never correctness, and losing it costs one badly
    /// sorted rail rather than a task.
    pub fn load_attention(&self) -> HashMap<String, Attention> {
        std::fs::read_to_string(self.attention_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// Persist the attention map atomically, pruned to `live` — ids that no
    /// longer exist (a deleted run, a removed worktree) drop out, so the file
    /// tracks the world rather than growing forever.
    pub fn save_attention(
        &self,
        attention: &HashMap<String, Attention>,
        live: &HashSet<String>,
    ) -> Result<(), StoreError> {
        let kept: HashMap<&String, &Attention> = attention
            .iter()
            .filter(|(id, _)| live.contains(*id))
            .collect();
        let json = serde_json::to_string_pretty(&kept).expect("attention always serializes");
        write_record_atomically(&self.attention_path(), &json)
    }

    /// Persist one legacy task record atomically and durably.
    pub fn save(&self, record: &PersistedTask) -> Result<(), StoreError> {
        let json = serde_json::to_string_pretty(record).expect("a task record always serializes");
        write_record_atomically(&self.path_for(&record.id), &json)
    }

    /// Load every legacy task record in the store, ordered by creation time. A
    /// missing store dir means no tasks (first boot). A file that exists but
    /// does not parse is a hard error naming the file — never a silently
    /// dropped task. Migrated (`.json.migrated`) files are ignored.
    pub fn load_all(&self) -> Result<Vec<PersistedTask>, StoreError> {
        if !self.dir.exists() {
            return Ok(Vec::new());
        }
        let mut records: Vec<PersistedTask> = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if !is_json_record(&path) {
                continue;
            }
            records.push(read_record(&path)?);
        }
        records.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        Ok(records)
    }

    /// Delete a task's persisted record (and any leftover `.tmp` from an interrupted
    /// write), plus its plan snapshot. Removing a record that isn't there is not an
    /// error — delete is only ever called for a terminal task the board wants gone,
    /// and idempotency keeps a double-delete or a never-persisted task from failing
    /// the RPC.
    pub fn delete(&self, task_id: &str) -> Result<(), StoreError> {
        for path in [
            self.path_for(task_id),
            self.dir.join(format!("{task_id}.json.tmp")),
        ] {
            remove_file_if_present(&path)?;
        }
        remove_dir_if_present(&self.plan_snapshot_dir(task_id))?;
        Ok(())
    }

    /// Where a task's plan-doc snapshot lives. The same dir the plan/run split
    /// uses for the plan itself — the legacy snapshot was this layout's
    /// ancestor, with the docs sitting directly in the dir instead of `docs/`.
    fn plan_snapshot_dir(&self, task_id: &str) -> PathBuf {
        self.plan_dir(task_id)
    }

    /// Mirror the worktree's plan docs into the store — the single plan file
    /// (`plan_path`, worktree-relative) and the whole multi-stage plan dir
    /// (`.build/plan/`) — so a task whose worktree the user deletes keeps its
    /// plans readable as archived history. Missing sources are quiet no-ops; a
    /// re-snapshot overwrites with the latest contents.
    pub fn snapshot_plan_docs(
        &self,
        task_id: &str,
        worktree_path: &Path,
        plan_path: &str,
    ) -> Result<(), StoreError> {
        let snapshot_root = self.plan_snapshot_dir(task_id);
        if snapshot_relative_path_escapes(plan_path) {
            return Ok(()); // the callers fence plan_path already; never mirror an escapee
        }
        let plan_source = worktree_path.join(plan_path);
        if plan_source.is_file() {
            let dest = snapshot_root.join(plan_path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&plan_source, &dest)?;
        }
        let stage_dir = worktree_path.join(".build/plan");
        if stage_dir.is_dir() {
            let dest_dir = snapshot_root.join(".build/plan");
            std::fs::create_dir_all(&dest_dir)?;
            for entry in std::fs::read_dir(&stage_dir)? {
                let source = entry?.path();
                if !source.is_file() {
                    continue; // stage docs are a flat dir of markdown files
                }
                let Some(name) = source.file_name() else {
                    continue;
                };
                std::fs::copy(&source, dest_dir.join(name))?;
            }
        }
        Ok(())
    }

    /// Read one plan doc from a task's snapshot by its worktree-relative path.
    /// None when never snapshotted (or the path tries to escape the snapshot).
    pub fn read_plan_snapshot(&self, task_id: &str, rel_path: &str) -> Option<String> {
        if snapshot_relative_path_escapes(rel_path) {
            return None;
        }
        std::fs::read_to_string(self.plan_snapshot_dir(task_id).join(rel_path)).ok()
    }

    // ---- Plans (project-scoped records + canonical docs) ----

    /// Where a plan's record and docs live.
    fn plan_dir(&self, plan_id: &str) -> PathBuf {
        self.dir.join("plans").join(plan_id)
    }

    /// Where a plan's durable record lives.
    pub fn plan_record_path(&self, plan_id: &str) -> PathBuf {
        self.plan_dir(plan_id).join("record.json")
    }

    /// Where a plan's canonical docs live (worktree-relative layout inside).
    fn plan_docs_dir(&self, plan_id: &str) -> PathBuf {
        self.plan_dir(plan_id).join("docs")
    }

    /// Persist one plan record atomically and durably.
    pub fn save_plan(&self, record: &PersistedPlan) -> Result<(), StoreError> {
        let json = serde_json::to_string_pretty(record).expect("a plan record always serializes");
        write_record_atomically(&self.plan_record_path(&record.id), &json)
    }

    /// Load every plan record, ordered by creation time. A missing `plans/`
    /// dir means no plans. A record that exists but does not parse is a hard
    /// error naming the file — never a silently dropped plan. A plan dir
    /// without a `record.json` is not a plan: it is a legacy doc snapshot for
    /// a task that migrated without one (quick tasks) — skipped, files kept.
    pub fn load_all_plans(&self) -> Result<Vec<PersistedPlan>, StoreError> {
        let plans_dir = self.dir.join("plans");
        if !plans_dir.exists() {
            return Ok(Vec::new());
        }
        let mut records: Vec<PersistedPlan> = Vec::new();
        for entry in std::fs::read_dir(&plans_dir)? {
            let plan_dir = entry?.path();
            if !plan_dir.is_dir() {
                continue;
            }
            let record_path = plan_dir.join("record.json");
            if !record_path.is_file() {
                continue;
            }
            records.push(read_record(&record_path)?);
        }
        records.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        Ok(records)
    }

    /// Delete a plan's record **and** its canonical docs. Idempotent for the
    /// same reason as `delete`: only ever called for a plan the board wants
    /// gone, and a double-delete must not fail the RPC.
    pub fn delete_plan(&self, plan_id: &str) -> Result<(), StoreError> {
        remove_dir_if_present(&self.plan_dir(plan_id))
    }

    // ---- Runs (worktree-scoped records) ----

    /// Where a run's durable record lives.
    pub fn run_record_path(&self, run_id: &str) -> PathBuf {
        self.dir.join("runs").join(format!("{run_id}.json"))
    }

    /// Persist one run record atomically and durably.
    pub fn save_run(&self, record: &PersistedRun) -> Result<(), StoreError> {
        let json = serde_json::to_string_pretty(record).expect("a run record always serializes");
        write_record_atomically(&self.run_record_path(&record.id), &json)
    }

    /// Load every run record, ordered by creation time — same discipline as
    /// `load_all_plans`: missing dir means none, unparseable means fail fast.
    pub fn load_all_runs(&self) -> Result<Vec<PersistedRun>, StoreError> {
        let runs_dir = self.dir.join("runs");
        if !runs_dir.exists() {
            return Ok(Vec::new());
        }
        let mut records: Vec<PersistedRun> = Vec::new();
        for entry in std::fs::read_dir(&runs_dir)? {
            let path = entry?.path();
            if !is_json_record(&path) {
                continue;
            }
            records.push(read_record(&path)?);
        }
        records.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        Ok(records)
    }

    /// Delete a run's record (and any leftover `.tmp`). Idempotent.
    pub fn delete_run(&self, run_id: &str) -> Result<(), StoreError> {
        let record_path = self.run_record_path(run_id);
        remove_file_if_present(&record_path)?;
        remove_file_if_present(&record_path.with_extension("json.tmp"))?;
        Ok(())
    }

    // ---- Archived external worktrees --------------------------------------

    fn archived_worktree_path(&self, worktree_id: &str) -> PathBuf {
        self.dir
            .join("archived-worktrees")
            .join(format!("{worktree_id}.json"))
    }

    /// Persist one finished external worktree by its stable server id. Re-saving
    /// the same record atomically replaces it, making repeated finish requests
    /// idempotent without duplicating history.
    pub fn save_archived_worktree(
        &self,
        record: &PersistedArchivedWorktree,
    ) -> Result<(), StoreError> {
        let json = serde_json::to_string_pretty(record)
            .expect("an archived worktree record always serializes");
        write_record_atomically(&self.archived_worktree_path(&record.worktree_id), &json)
    }

    /// Load every finished external worktree, ordered by archive time and id.
    /// Corruption is a boot error: silently dropping archive history would make
    /// a destructive finish action illegible after restart.
    pub fn load_all_archived_worktrees(
        &self,
    ) -> Result<Vec<PersistedArchivedWorktree>, StoreError> {
        let archive_dir = self.dir.join("archived-worktrees");
        if !archive_dir.exists() {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        for entry in std::fs::read_dir(archive_dir)? {
            let path = entry?.path();
            if is_json_record(&path) {
                records.push(read_record(&path)?);
            }
        }
        records.sort_by(|a: &PersistedArchivedWorktree, b| {
            a.archived_at
                .cmp(&b.archived_at)
                .then(a.worktree_id.cmp(&b.worktree_id))
        });
        Ok(records)
    }

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

    /// Boot migration: split every legacy fused-task record (`<task_id>.json`)
    /// into plan and/or run records per the Plan/Run Split spec's mapping —
    /// quick task → run only; standard task never past planning → plan only;
    /// past planning → `Approved` plan + run; terminal states map to terminal
    /// run states with the plan kept — then rename the legacy file to
    /// `<task_id>.json.migrated` (kept: no data is deleted; ignored by every
    /// loader). Legacy doc snapshots are promoted into `plans/<id>/docs/`.
    /// Must run before `load_all_plans`/`load_all_runs` on boot.
    ///
    /// Idempotent and crash-resumable: an existing new-format record is never
    /// overwritten (its presence marks that half migrated, so a crash between
    /// the record writes and the rename re-runs and only fills the gaps), and
    /// a store with no legacy files is a no-op. A legacy record that does not
    /// parse fails the migration loudly — exactly like boot loading — because
    /// a silently skipped task would orphan its worktree.
    ///
    /// Returns how many legacy records were migrated.
    pub fn migrate_legacy_tasks(&self) -> Result<usize, StoreError> {
        if !self.dir.exists() {
            return Ok(0);
        }
        // Collect first: the loop renames files while iterating.
        let mut legacy_paths: Vec<PathBuf> = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if path.is_file() && is_json_record(&path) {
                legacy_paths.push(path);
            }
        }
        legacy_paths.sort();

        let mut migrated = 0usize;
        for legacy_path in legacy_paths {
            let task: PersistedTask = read_record(&legacy_path)?;
            if let Some(plan) = plan_record_from_legacy(&task) {
                if !self.plan_record_path(&plan.id).exists() {
                    // Docs before record: the record's existence marks the
                    // plan fully migrated, so a crash in between re-runs the
                    // (overwrite-safe) copy.
                    self.promote_snapshot_docs(&plan.id)?;
                    // The legacy snapshot mirror was best-effort (and younger
                    // than some records): when it never ran, the live worktree
                    // is the only copy of the docs — ingest from it, or the
                    // migrated plan is unreadable despite the files existing.
                    if !dir_contains_a_file(&self.plan_docs_dir(&plan.id)) {
                        let worktree = Path::new(&task.worktree_path);
                        if worktree.is_dir() {
                            match self.ingest_plan_docs(&plan.id, worktree, &task.plan_path) {
                                Ok(()) | Err(StoreError::NothingToIngest { .. }) => {}
                                Err(other) => return Err(other),
                            }
                        }
                    }
                    self.save_plan(&plan)?;
                }
            }
            if let Some(run) = run_record_from_legacy(&task) {
                if !self.run_record_path(&run.id).exists() {
                    self.save_run(&run)?;
                }
            }
            std::fs::rename(&legacy_path, legacy_path.with_extension("json.migrated"))?;
            migrated += 1;
        }
        Ok(migrated)
    }

    /// Promote a legacy plan snapshot (docs sitting directly in
    /// `plans/<task_id>/`) into the canonical `plans/<task_id>/docs/`
    /// location. Copies, never moves — no data is deleted by migration. A
    /// task that never snapshotted docs is fine: the plan simply has none.
    fn promote_snapshot_docs(&self, plan_id: &str) -> Result<(), StoreError> {
        let snapshot_root = self.plan_dir(plan_id); // the legacy snapshot dir *is* the plan dir
        if !snapshot_root.is_dir() {
            return Ok(());
        }
        copy_tree(
            &snapshot_root,
            &self.plan_docs_dir(plan_id),
            &["record.json", "docs"],
        )?;
        Ok(())
    }
}

/// A snapshot path must stay inside the task's snapshot dir: plain relative
/// components only — no roots, no prefixes, no `..`.
fn snapshot_relative_path_escapes(rel_path: &str) -> bool {
    let path = Path::new(rel_path);
    path.components().any(|c| {
        !matches!(
            c,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    })
}

/// Only `*.json` files are records; `.tmp` leftovers from an interrupted
/// write are ignored (the rename never happened, so the previous record — or
/// no record — is the truth), and so are `.migrated` legacy files.
fn is_json_record(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("json")
}

/// The worktree-relative dir multi-stage plan docs live in.
const STAGE_PLAN_DIR: &str = ".build/plan";

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

/// Remove a dir tree, treating "already gone" as success (delete idempotency).
fn remove_dir_if_present(path: &Path) -> Result<(), StoreError> {
    match std::fs::remove_dir_all(path) {
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

/// The spec's migration split: did this fused task ever progress past the
/// plan gate? Build-phase and terminal states did trivially. A plan-phase
/// state with any run-side stage progress is a mid-run stage revision or the
/// between-stages gate (the fused machine reused `PlanReview` for it) — also
/// past planning.
fn legacy_task_progressed_past_planning(task: &PersistedTask) -> bool {
    match &task.state {
        TaskState::Building
        | TaskState::Review
        | TaskState::Merged
        | TaskState::Abandoned
        | TaskState::Archived => true,
        TaskState::Blocked(phase)
        | TaskState::Failed(phase)
        | TaskState::IdleUnreported(phase)
        | TaskState::Interrupted(phase) => {
            *phase == Phase::Build || task.stages.iter().any(legacy_stage_has_run_progress)
        }
        TaskState::Created | TaskState::Planning | TaskState::PlanReview => {
            task.stages.iter().any(legacy_stage_has_run_progress)
        }
    }
}

/// Run-side progress on a legacy stage means it was dispatched at least once.
fn legacy_stage_has_run_progress(stage: &Stage) -> bool {
    matches!(
        stage.state,
        StageState::Building
            | StageState::Built
            | StageState::Validating
            | StageState::Validated { .. }
    )
}

/// The plan half of a legacy task. `None` for quick tasks — they never had a
/// plan gate, so they migrate to a run with `plan_id: None`.
fn plan_record_from_legacy(task: &PersistedTask) -> Option<PersistedPlan> {
    if task.kind == TaskKind::Quick {
        return None;
    }
    let past_planning = legacy_task_progressed_past_planning(task);
    // While the plan was still being authored, the fused task's worktree *was*
    // the planning worktree; past planning it belongs to the run.
    let (worktree_name, worktree_path, branch) = if past_planning {
        (None, None, None)
    } else {
        (
            Some(task.worktree_name.clone()),
            Some(task.worktree_path.clone()),
            Some(task.branch.clone()),
        )
    };
    Some(PersistedPlan {
        id: task.id.clone(),
        goal: task.goal.clone(),
        project_path: task.project_path.clone(),
        base_branch: task.base_branch.clone(),
        state: migrated_plan_state(task, past_planning),
        archived_at: None,
        worktree_name,
        worktree_path,
        branch,
        plan_path: task.plan_path.clone(),
        stages: task.stages.iter().map(stage_doc_from_legacy).collect(),
        comments: task.comments.iter().map(plan_comment_from_legacy).collect(),
        provider: AgentProvider::Claude,
        model: task.model.clone(),
        effort: task.effort.clone(),
        thread: crate::thread::Thread::new(&task.id),
        last_summary: task.last_summary.clone(),
        last_error: task.last_error.clone(),
        created_at: task.created_at.clone(),
        updated_at: task.updated_at.clone(),
        state_changed_at: None,
    })
}

/// Plan-side state for a migrated legacy task. Past planning the plan is
/// `Approved` — the human approved it to get there, and terminal tasks keep
/// their plan per the spec ("terminal fused states map to terminal run states
/// with the plan kept"). Otherwise the fused plan-phase state maps 1:1.
fn migrated_plan_state(task: &PersistedTask, past_planning: bool) -> PlanState {
    if past_planning {
        return PlanState::Approved;
    }
    match &task.state {
        TaskState::Created => PlanState::Created,
        TaskState::Planning => PlanState::Drafting,
        TaskState::PlanReview => PlanState::PlanReview,
        TaskState::Blocked(_) => PlanState::Blocked,
        TaskState::Failed(_) => PlanState::Failed,
        TaskState::IdleUnreported(_) => PlanState::IdleUnreported,
        TaskState::Interrupted(_) => PlanState::Interrupted,
        TaskState::Building
        | TaskState::Review
        | TaskState::Merged
        | TaskState::Abandoned
        | TaskState::Archived => {
            unreachable!("{:?} is past planning by definition", task.state)
        }
    }
}

/// The run half of a legacy task. `None` for a standard task that never
/// progressed past planning — there was never an implementation attempt.
fn run_record_from_legacy(task: &PersistedTask) -> Option<PersistedRun> {
    let plan_id = match task.kind {
        TaskKind::Quick => None,
        TaskKind::Standard => {
            if !legacy_task_progressed_past_planning(task) {
                return None;
            }
            Some(task.id.clone())
        }
    };
    // When a legacy task splits into both halves, the plan keeps the task id
    // (its docs dir is already keyed by it) and the run takes a derived,
    // disjoint id — done-report routing and every entity map assume no id is
    // ever both a plan and a run. Quick tasks have no plan half, so their id
    // carries over untouched.
    let run_id = match plan_id {
        Some(_) => format!("run-{}", task.id),
        None => task.id.clone(),
    };
    Some(PersistedRun {
        id: run_id.clone(),
        plan_id,
        goal: task.goal.clone(),
        project_path: task.project_path.clone(),
        base_branch: task.base_branch.clone(),
        state: migrated_run_state(task),
        branch: task.branch.clone(),
        worktree_name: task.worktree_name.clone(),
        worktree_path: task.worktree_path.clone(),
        // Legacy runs never recorded a materialization commit; their review
        // diffs fall back to the merge-base, exactly like adopted runs.
        base_sha: None,
        stages: task
            .stages
            .iter()
            .filter_map(stage_progress_from_legacy)
            .collect(),
        current_stage_id: task.current_stage_id.clone(),
        revising_stage_id: task.revising_stage_id.clone(),
        auto_advance: task.auto_advance,
        adopted: task.adopted,
        pending_continuation: task.pending_continuation,
        provider: AgentProvider::Claude,
        model: task.model.clone(),
        effort: task.effort.clone(),
        thread: crate::thread::Thread::new(&run_id),
        last_summary: task.last_summary.clone(),
        last_error: task.last_error.clone(),
        created_at: task.created_at.clone(),
        updated_at: task.updated_at.clone(),
        state_changed_at: None,
    })
}

/// Run-side state for a migrated legacy task (only called when a run record
/// is produced at all).
fn migrated_run_state(task: &PersistedTask) -> RunState {
    match &task.state {
        // A quick task that never dispatched.
        TaskState::Created => RunState::Created,
        TaskState::Building => RunState::Building,
        TaskState::Review => RunState::Review,
        TaskState::Blocked(_) => RunState::Blocked,
        TaskState::Failed(_) => RunState::Failed,
        TaskState::IdleUnreported(_) => RunState::IdleUnreported,
        TaskState::Interrupted(_) => RunState::Interrupted,
        TaskState::Merged => RunState::Merged,
        TaskState::Abandoned => RunState::Abandoned,
        TaskState::Archived => RunState::Archived,
        // The fused machine reused PlanReview as the between-stages board
        // (and parked there while mid-run stage revisions ran): with stage
        // progress in play — the only way these reach a run record — that
        // position is the run's StageGate.
        TaskState::Planning | TaskState::PlanReview => RunState::StageGate,
    }
}

/// Plan-side view of a legacy stage: manifest metadata + doc review state.
fn stage_doc_from_legacy(stage: &Stage) -> StageDoc {
    StageDoc {
        id: stage.id.clone(),
        title: stage.title.clone(),
        path: stage.path.clone(),
        summary: stage.summary.clone(),
        // Any run-side progress implies the human approved the doc to
        // dispatch it.
        state: match stage.state {
            StageState::Planned => StageDocState::Planned,
            _ => StageDocState::Approved,
        },
    }
}

/// Run-side view of a legacy stage: execution progress, present only once the
/// stage was dispatched (plan-review-only stages live on the plan alone).
fn stage_progress_from_legacy(stage: &Stage) -> Option<StageProgress> {
    let state = match stage.state {
        StageState::Planned | StageState::Approved => return None,
        StageState::Building => StageProgressState::Building,
        StageState::Built => StageProgressState::Built,
        StageState::Validating => StageProgressState::Validating,
        StageState::Validated { passed } => StageProgressState::Validated { passed },
    };
    Some(StageProgress {
        stage_id: stage.id.clone(),
        state,
        start_sha: stage.start_sha.clone(),
        validation: stage
            .validation
            .as_ref()
            .map(|report| crate::run::ValidationReport {
                passed: report.passed,
                findings: report.findings.clone(),
                notes_for_next_stage: report.notes_for_next_stage.clone(),
            }),
    })
}

/// A legacy comment carries over to the plan record field-for-field.
fn plan_comment_from_legacy(comment: &StageComment) -> crate::plan::StageComment {
    crate::plan::StageComment {
        id: comment.id.clone(),
        stage_id: comment.stage_id.clone(),
        anchor: comment
            .anchor
            .as_ref()
            .map(|anchor| crate::plan::CommentAnchor {
                heading_path: anchor.heading_path.clone(),
                snippet: anchor.snippet.clone(),
            }),
        body: comment.body.clone(),
        state: match comment.state {
            crate::legacy::CommentState::Open => crate::plan::CommentState::Open,
            crate::legacy::CommentState::Addressed => crate::plan::CommentState::Addressed,
        },
        agent_reply: comment.agent_reply.clone(),
    }
}

/// The current time as an RFC 3339 UTC string (the store's timestamp format).
pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("UTC now formats as RFC 3339")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy::{
        CommentAnchor, CommentState, Phase, Stage, StageComment, StageState, TaskKind, TaskState,
        ValidationReport,
    };

    #[test]
    fn task_files_from_before_model_choice_still_load() {
        let dir = tempfile::tempdir().unwrap();
        // A pre-model-choice file is exactly today's serialization minus the
        // new keys — build it that way so the fixture never drifts from the
        // real wire format.
        let mut legacy = serde_json::to_value(record("task-1", TaskState::PlanReview)).unwrap();
        let map = legacy.as_object_mut().unwrap();
        map.remove("model");
        map.remove("effort");
        map.remove("last_error");
        std::fs::write(
            dir.path().join("task-1.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let store = Store::new(dir.path());
        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].model, None);
        assert_eq!(loaded[0].effort, None);
        assert_eq!(loaded[0].last_error, None);
    }

    /// The binding spec's pinned legacy rule (§5): a record with no multi-stage
    /// keys at all — i.e. any file written before this feature — loads with
    /// `stages: []` and every other new field at its zero value, so the legacy
    /// single-plan code path (`plan_path`, `task.plan`, `task.approve_plan`,
    /// `task.send_notes`) keeps working untouched.
    #[test]
    fn pre_multi_stage_task_files_still_load_on_the_legacy_path() {
        let dir = tempfile::tempdir().unwrap();
        let mut legacy = serde_json::to_value(record("task-1", TaskState::PlanReview)).unwrap();
        let map = legacy.as_object_mut().unwrap();
        map.remove("stages");
        map.remove("current_stage_id");
        map.remove("revising_stage_id");
        map.remove("auto_advance");
        map.remove("comments");
        std::fs::write(
            dir.path().join("task-1.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let store = Store::new(dir.path());
        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].stages, Vec::new());
        assert_eq!(loaded[0].current_stage_id, None);
        assert_eq!(loaded[0].revising_stage_id, None);
        assert!(!loaded[0].auto_advance);
        assert_eq!(loaded[0].comments, Vec::new());
    }

    /// Full round-trip of the new multi-stage shape: two stages (one still
    /// failing validation, carrying a report and a `start_sha`), two comments
    /// (one anchored/open, one general/addressed with an agent reply),
    /// `auto_advance` on, and both stage-tracking ids set.
    #[test]
    fn multi_stage_record_round_trips_every_new_field() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let mut rec = record("task-1", TaskState::PlanReview);
        rec.stages = vec![
            Stage {
                id: "database-schema".into(),
                title: "Database schema".into(),
                path: ".build/plan/01-database-schema.md".into(),
                summary: "Create the tables and the migration.".into(),
                state: StageState::Validated { passed: false },
                start_sha: Some("deadbeef".into()),
                validation: Some(ValidationReport {
                    passed: false,
                    findings: "missing the soft-delete column".into(),
                    notes_for_next_stage: "".into(),
                }),
            },
            Stage {
                id: "api-endpoints".into(),
                title: "API endpoints".into(),
                path: ".build/plan/02-api-endpoints.md".into(),
                summary: "CRUD routes over the new tables.".into(),
                state: StageState::Planned,
                start_sha: None,
                validation: None,
            },
        ];
        rec.current_stage_id = Some("database-schema".into());
        rec.revising_stage_id = Some("api-endpoints".into());
        rec.auto_advance = true;
        rec.comments = vec![
            StageComment {
                id: "c-1".into(),
                stage_id: "database-schema".into(),
                anchor: Some(CommentAnchor {
                    heading_path: vec!["Database schema".into(), "Tables".into()],
                    snippet: "users table gets a soft-delete column".into(),
                }),
                body: "use a deleted_at timestamp, not a boolean".into(),
                state: CommentState::Open,
                agent_reply: None,
            },
            StageComment {
                id: "c-2".into(),
                stage_id: "database-schema".into(),
                anchor: None,
                body: "this stage feels too big".into(),
                state: CommentState::Addressed,
                agent_reply: Some("split into two migrations".into()),
            },
        ];
        store.save(&rec).unwrap();

        let loaded = store.load_all().unwrap();
        assert_eq!(loaded, vec![rec]);
    }

    /// Adoption's pinned legacy rule (spec §4): any task file written before
    /// worktree adoption existed has neither key and must load as a native
    /// task — `adopted: false`, `pending_continuation: false`.
    #[test]
    fn pre_adoption_task_files_load_as_native() {
        let dir = tempfile::tempdir().unwrap();
        let mut legacy = serde_json::to_value(record("task-1", TaskState::Review)).unwrap();
        let map = legacy.as_object_mut().unwrap();
        map.remove("adopted");
        map.remove("pending_continuation");
        std::fs::write(
            dir.path().join("task-1.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();
        let store = Store::new(dir.path());
        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(!loaded[0].adopted);
        assert!(!loaded[0].pending_continuation);
    }

    /// An adopted record round-trips both adoption flags.
    #[test]
    fn adopted_record_round_trips_both_flags() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let mut rec = record("task-1", TaskState::Review);
        rec.adopted = true;
        rec.pending_continuation = true;
        store.save(&rec).unwrap();
        assert_eq!(store.load_all().unwrap(), vec![rec]);
    }

    #[test]
    fn delete_removes_the_record_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        store.save(&record("task-1", TaskState::Merged)).unwrap();
        assert_eq!(store.load_all().unwrap().len(), 1);

        store.delete("task-1").unwrap();
        assert_eq!(store.load_all().unwrap(), Vec::new(), "record gone");
        // Deleting again (or a task that never persisted) is not an error.
        store.delete("task-1").unwrap();
        store.delete("never-existed").unwrap();
    }

    #[test]
    fn plan_snapshot_mirrors_plan_doc_and_stage_docs() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let worktree = dir.path().join("wt");
        std::fs::create_dir_all(worktree.join(".build/plan")).unwrap();
        std::fs::write(worktree.join(".build/plan.md"), "# the plan").unwrap();
        std::fs::write(worktree.join(".build/plan/01-first.md"), "stage one").unwrap();
        std::fs::write(worktree.join(".build/plan/02-second.md"), "stage two").unwrap();

        store
            .snapshot_plan_docs("task-1", &worktree, ".build/plan.md")
            .unwrap();
        assert_eq!(
            store
                .read_plan_snapshot("task-1", ".build/plan.md")
                .as_deref(),
            Some("# the plan")
        );
        assert_eq!(
            store
                .read_plan_snapshot("task-1", ".build/plan/01-first.md")
                .as_deref(),
            Some("stage one")
        );
        assert_eq!(
            store
                .read_plan_snapshot("task-1", ".build/plan/02-second.md")
                .as_deref(),
            Some("stage two")
        );
    }

    #[test]
    fn plan_snapshot_tracks_updates_and_tolerates_missing_sources() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let worktree = dir.path().join("wt");
        // Nothing to snapshot yet (no worktree at all) — a quiet no-op.
        store
            .snapshot_plan_docs("task-1", &worktree, ".build/plan.md")
            .unwrap();
        assert_eq!(store.read_plan_snapshot("task-1", ".build/plan.md"), None);

        std::fs::create_dir_all(worktree.join(".build")).unwrap();
        std::fs::write(worktree.join(".build/plan.md"), "v1").unwrap();
        store
            .snapshot_plan_docs("task-1", &worktree, ".build/plan.md")
            .unwrap();
        std::fs::write(worktree.join(".build/plan.md"), "v2 revised").unwrap();
        store
            .snapshot_plan_docs("task-1", &worktree, ".build/plan.md")
            .unwrap();
        assert_eq!(
            store
                .read_plan_snapshot("task-1", ".build/plan.md")
                .as_deref(),
            Some("v2 revised"),
            "a re-snapshot overwrites with the latest contents"
        );
    }

    #[test]
    fn plan_snapshot_read_refuses_traversal_and_absolute_paths() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        assert_eq!(
            store.read_plan_snapshot("task-1", "../task-2/plan.md"),
            None
        );
        assert_eq!(store.read_plan_snapshot("task-1", "/etc/hostname"), None);
    }

    #[test]
    fn delete_removes_the_plan_snapshot_too() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        store.save(&record("task-1", TaskState::Merged)).unwrap();
        let worktree = dir.path().join("wt");
        std::fs::create_dir_all(worktree.join(".build")).unwrap();
        std::fs::write(worktree.join(".build/plan.md"), "# plan").unwrap();
        store
            .snapshot_plan_docs("task-1", &worktree, ".build/plan.md")
            .unwrap();

        store.delete("task-1").unwrap();
        assert_eq!(store.read_plan_snapshot("task-1", ".build/plan.md"), None);
    }

    fn record(id: &str, state: TaskState) -> PersistedTask {
        PersistedTask {
            id: id.into(),
            goal: "add a greeting".into(),
            kind: TaskKind::Standard,
            project_path: "/home/u/code/proj".into(),
            base_branch: "main".into(),
            state,
            branch: "build/add-a-greeting".into(),
            worktree_name: "add-a-greeting".into(),
            worktree_path: "/home/u/.build/worktrees/add-a-greeting".into(),
            plan_path: ".build/plan.md".into(),
            last_summary: Some("planned it".into()),
            model: Some("claude-opus-4-8".into()),
            effort: Some("xhigh".into()),
            last_error: None,
            stages: Vec::new(),
            current_stage_id: None,
            revising_stage_id: None,
            auto_advance: false,
            comments: Vec::new(),
            adopted: false,
            pending_continuation: false,
            created_at: "2026-07-01T10:00:00Z".into(),
            updated_at: "2026-07-01T10:05:00Z".into(),
        }
    }

    #[test]
    fn save_then_load_round_trips_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let rec = record("task-1", TaskState::PlanReview);
        store.save(&rec).unwrap();

        let loaded = store.load_all().unwrap();
        assert_eq!(loaded, vec![rec]);
    }

    #[test]
    fn save_overwrites_atomically_leaving_no_tmp_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let mut rec = record("task-1", TaskState::Planning);
        store.save(&rec).unwrap();
        rec.state = TaskState::PlanReview;
        rec.updated_at = "2026-07-01T10:10:00Z".into();
        store.save(&rec).unwrap();

        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 1, "an update replaces, never duplicates");
        assert_eq!(loaded[0].state, TaskState::PlanReview);
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("tasks"))
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "no .tmp files after a save");
    }

    #[test]
    fn load_all_orders_by_creation_time() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let mut newer = record("task-2", TaskState::Building);
        newer.created_at = "2026-07-01T11:00:00Z".into();
        let older = record("task-1", TaskState::Merged);
        store.save(&newer).unwrap();
        store.save(&older).unwrap();

        let ids: Vec<String> = store
            .load_all()
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, vec!["task-1", "task-2"]);
    }

    #[test]
    fn missing_store_dir_is_no_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("never-created"));
        assert_eq!(store.load_all().unwrap(), Vec::new());
    }

    #[test]
    fn corrupt_task_file_is_a_hard_error_naming_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        store.save(&record("task-1", TaskState::Review)).unwrap();
        std::fs::write(tasks.join("task-2.json"), "{ not json").unwrap();

        let err = store.load_all().expect_err("corrupt file must fail loudly");
        let message = err.to_string();
        assert!(
            message.contains("task-2.json"),
            "error names the corrupt file: {message}"
        );
        assert!(matches!(err, StoreError::Corrupt { .. }));
    }

    #[test]
    fn empty_task_file_is_a_named_actionable_error() {
        // A power loss can make the rename durable before the data blocks: the
        // record exists but is zero-length. The error must say exactly which file
        // and that deleting it is safe — not a bare JSON parse error.
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        store.save(&record("task-1", TaskState::Review)).unwrap();
        std::fs::write(tasks.join("task-2.json"), "").unwrap();

        let err = store.load_all().expect_err("empty file must fail loudly");
        assert!(matches!(err, StoreError::Empty { .. }));
        let message = err.to_string();
        assert!(message.contains("task-2.json"), "names the file: {message}");
        assert!(message.contains("delete"), "actionable: {message}");
    }

    #[test]
    fn interrupted_write_leftover_tmp_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        store.save(&record("task-1", TaskState::Review)).unwrap();
        // A crash between write and rename leaves a torn .tmp behind.
        std::fs::write(tasks.join("task-1.json.tmp"), "{ torn").unwrap();

        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].state, TaskState::Review);
    }

    #[test]
    fn phase_states_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        for (i, state) in [
            TaskState::Blocked(Phase::Build),
            TaskState::Failed(Phase::Plan),
            TaskState::IdleUnreported(Phase::Build),
            TaskState::Interrupted(Phase::Plan),
        ]
        .into_iter()
        .enumerate()
        {
            let rec = record(&format!("task-{i}"), state.clone());
            store.save(&rec).unwrap();
        }
        let loaded = store.load_all().unwrap();
        assert_eq!(loaded[3].state, TaskState::Interrupted(Phase::Plan));
        assert_eq!(loaded.len(), 4);
    }

    #[test]
    fn now_rfc3339_looks_like_a_timestamp() {
        let now = now_rfc3339();
        assert!(now.contains('T') && now.ends_with('Z'), "got {now}");
    }

    // ================== Plan records (the split's project-scoped half) ==================

    use crate::plan::{PlanState, StageDoc, StageDocState};
    use crate::run::{RunState, StageProgress, StageProgressState};

    fn plan_record(id: &str, state: PlanState) -> PersistedPlan {
        PersistedPlan {
            id: id.into(),
            goal: "plan the greeting".into(),
            project_path: "/home/u/code/proj".into(),
            base_branch: "main".into(),
            state,
            archived_at: None,
            worktree_name: Some("plan-greeting".into()),
            worktree_path: Some("/home/u/.build/worktrees/plan-greeting".into()),
            branch: Some("plan/greeting".into()),
            plan_path: ".build/plan.md".into(),
            stages: vec![StageDoc {
                id: "database-schema".into(),
                title: "Database schema".into(),
                path: ".build/plan/01-database-schema.md".into(),
                summary: "Tables and migration.".into(),
                state: StageDocState::Planned,
            }],
            comments: vec![crate::plan::StageComment {
                id: "c-1".into(),
                stage_id: "database-schema".into(),
                anchor: Some(crate::plan::CommentAnchor {
                    heading_path: vec!["Database schema".into()],
                    snippet: "users table".into(),
                }),
                body: "use a deleted_at timestamp".into(),
                state: crate::plan::CommentState::Open,
                agent_reply: None,
            }],
            provider: AgentProvider::Claude,
            model: Some("claude-opus-4-8".into()),
            effort: Some("xhigh".into()),
            thread: crate::thread::Thread::new(id),
            last_summary: Some("planned it".into()),
            last_error: None,
            created_at: "2026-07-01T10:00:00Z".into(),
            updated_at: "2026-07-01T10:05:00Z".into(),
            state_changed_at: None,
        }
    }

    fn archived_worktree_record(id: &str, project_path: &str) -> PersistedArchivedWorktree {
        PersistedArchivedWorktree {
            status: WorktreeFinishStatus::Archived,
            project_path: project_path.into(),
            worktree_id: id.into(),
            worktree_name: "feature-one".into(),
            worktree_path: "/home/u/.build/worktrees/feature-one".into(),
            branch: Some("build/feature-one".into()),
            head_sha: "0123456789abcdef".into(),
            upstream: Some("origin/build/feature-one".into()),
            unpushed: Some(2),
            dirty_files: 3,
            uncommitted_files: 2,
            uncommitted_insertions: 14,
            uncommitted_deletions: 4,
            action: WorktreeFinishAction::Push,
            archived_at: Some("2026-07-29T12:00:00Z".into()),
        }
    }

    fn run_record(id: &str, state: RunState) -> PersistedRun {
        PersistedRun {
            id: id.into(),
            plan_id: Some("plan-1".into()),
            goal: "implement the greeting".into(),
            project_path: "/home/u/code/proj".into(),
            base_branch: "main".into(),
            state,
            branch: "build/greeting".into(),
            worktree_name: "greeting".into(),
            worktree_path: "/home/u/.build/worktrees/greeting".into(),
            base_sha: Some("f00dcafe".into()),
            stages: vec![StageProgress {
                stage_id: "database-schema".into(),
                state: StageProgressState::Validated { passed: false },
                start_sha: Some("deadbeef".into()),
                validation: Some(crate::run::ValidationReport {
                    passed: false,
                    findings: "missing the soft-delete column".into(),
                    notes_for_next_stage: "".into(),
                }),
            }],
            current_stage_id: Some("database-schema".into()),
            revising_stage_id: None,
            auto_advance: true,
            adopted: false,
            pending_continuation: false,
            provider: AgentProvider::Claude,
            model: Some("claude-fable-5".into()),
            effort: Some("high".into()),
            thread: crate::thread::Thread::new(id),
            last_summary: Some("stage one built".into()),
            last_error: None,
            created_at: "2026-07-01T11:00:00Z".into(),
            updated_at: "2026-07-01T11:05:00Z".into(),
            state_changed_at: None,
        }
    }

    #[test]
    fn plan_save_then_load_round_trips_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let mut rec = plan_record("plan-1", PlanState::PlanReview);
        rec.provider = AgentProvider::Codex;
        rec.model = Some("gpt-5.6-sol".into());
        rec.effort = Some("ultra".into());
        store.save_plan(&rec).unwrap();
        assert_eq!(store.load_all_plans().unwrap(), vec![rec]);
    }

    #[test]
    fn plan_archived_at_round_trips_and_defaults_for_legacy_json() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let mut archived = plan_record("plan-archived", PlanState::Approved);
        archived.archived_at = Some("2026-07-29T12:00:00Z".into());
        store.save_plan(&archived).unwrap();
        assert_eq!(store.load_all_plans().unwrap(), vec![archived]);

        let legacy_path = store.plan_record_path("plan-legacy");
        let mut legacy =
            serde_json::to_value(plan_record("plan-legacy", PlanState::Approved)).unwrap();
        legacy.as_object_mut().unwrap().remove("archived_at");
        write_record_atomically(&legacy_path, &serde_json::to_string(&legacy).unwrap()).unwrap();
        let loaded = store.load_all_plans().unwrap();
        assert_eq!(loaded[1].id, "plan-legacy");
        assert_eq!(loaded[1].archived_at, None);
    }

    #[test]
    fn archived_worktrees_round_trip_by_stable_id_and_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let record = archived_worktree_record("wt-0123456789ab", "/home/u/code/proj");
        Store::new(&tasks).save_archived_worktree(&record).unwrap();

        let reopened = Store::new(&tasks);
        assert_eq!(
            reopened.load_all_archived_worktrees().unwrap(),
            vec![record]
        );
        assert!(tasks
            .join("archived-worktrees/wt-0123456789ab.json")
            .is_file());
    }

    #[test]
    fn archived_worktree_save_is_idempotent_and_projects_remain_filterable() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let first = archived_worktree_record("wt-aaaaaaaaaaaa", "/projects/one");
        let second = archived_worktree_record("wt-bbbbbbbbbbbb", "/projects/two");
        store.save_archived_worktree(&first).unwrap();
        store.save_archived_worktree(&first).unwrap();
        store.save_archived_worktree(&second).unwrap();

        let loaded = store.load_all_archived_worktrees().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(
            loaded
                .iter()
                .filter(|record| record.project_path == "/projects/one")
                .count(),
            1
        );
    }

    #[test]
    fn plan_record_lives_at_record_json_inside_the_plan_dir() {
        // The layout is spec-pinned: plans/<plan_id>/record.json, with the
        // canonical docs as siblings under plans/<plan_id>/docs/.
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        store
            .save_plan(&plan_record("plan-1", PlanState::Drafting))
            .unwrap();
        assert!(tasks.join("plans/plan-1/record.json").is_file());
    }

    #[test]
    fn plan_worktree_fields_persist_as_absent_after_teardown() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let mut rec = plan_record("plan-1", PlanState::Approved);
        rec.worktree_name = None;
        rec.worktree_path = None;
        rec.branch = None;
        store.save_plan(&rec).unwrap();
        assert_eq!(store.load_all_plans().unwrap(), vec![rec]);
    }

    #[test]
    fn run_save_then_load_round_trips_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let mut rec = run_record("run-1", RunState::StageGate);
        rec.provider = AgentProvider::Codex;
        rec.model = Some("gpt-5.6-terra".into());
        rec.effort = Some("max".into());
        store.save_run(&rec).unwrap();
        assert_eq!(store.load_all_runs().unwrap(), vec![rec]);
    }

    #[test]
    fn run_record_lives_as_one_json_file_under_runs() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        store
            .save_run(&run_record("run-1", RunState::Building))
            .unwrap();
        assert!(tasks.join("runs/run-1.json").is_file());
    }

    #[test]
    fn plan_less_run_round_trips_without_a_plan_link() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let mut rec = run_record("run-1", RunState::Building);
        rec.plan_id = None;
        rec.base_sha = None;
        rec.stages = Vec::new();
        rec.current_stage_id = None;
        store.save_run(&rec).unwrap();
        assert_eq!(store.load_all_runs().unwrap(), vec![rec]);
    }

    #[test]
    fn missing_plans_and_runs_dirs_mean_no_records() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("never-created"));
        assert_eq!(store.load_all_plans().unwrap(), Vec::new());
        assert_eq!(store.load_all_runs().unwrap(), Vec::new());
    }

    #[test]
    fn plans_and_runs_load_ordered_by_creation_time() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let mut newer_plan = plan_record("plan-2", PlanState::Drafting);
        newer_plan.created_at = "2026-07-02T10:00:00Z".into();
        store.save_plan(&newer_plan).unwrap();
        store
            .save_plan(&plan_record("plan-1", PlanState::Approved))
            .unwrap();
        let plan_ids: Vec<String> = store
            .load_all_plans()
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(plan_ids, vec!["plan-1", "plan-2"]);

        let mut newer_run = run_record("run-2", RunState::Building);
        newer_run.created_at = "2026-07-02T11:00:00Z".into();
        store.save_run(&newer_run).unwrap();
        store
            .save_run(&run_record("run-1", RunState::Merged))
            .unwrap();
        let run_ids: Vec<String> = store
            .load_all_runs()
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(run_ids, vec!["run-1", "run-2"]);
    }

    #[test]
    fn plan_save_overwrites_atomically_leaving_no_tmp_file() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        let mut rec = plan_record("plan-1", PlanState::Drafting);
        store.save_plan(&rec).unwrap();
        rec.state = PlanState::PlanReview;
        store.save_plan(&rec).unwrap();

        let loaded = store.load_all_plans().unwrap();
        assert_eq!(loaded.len(), 1, "an update replaces, never duplicates");
        assert_eq!(loaded[0].state, PlanState::PlanReview);
        let leftovers: Vec<_> = std::fs::read_dir(tasks.join("plans/plan-1"))
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "no .tmp files after a save");
    }

    #[test]
    fn corrupt_plan_record_is_a_hard_error_naming_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        std::fs::create_dir_all(tasks.join("plans/plan-1")).unwrap();
        std::fs::write(tasks.join("plans/plan-1/record.json"), "{ not json").unwrap();

        let err = store
            .load_all_plans()
            .expect_err("corrupt record must fail loudly");
        assert!(matches!(err, StoreError::Corrupt { .. }));
        assert!(err.to_string().contains("plan-1"), "names the plan: {err}");
    }

    #[test]
    fn empty_run_record_is_a_named_actionable_error() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        std::fs::create_dir_all(tasks.join("runs")).unwrap();
        std::fs::write(tasks.join("runs/run-1.json"), "").unwrap();

        let err = store
            .load_all_runs()
            .expect_err("empty record must fail loudly");
        assert!(matches!(err, StoreError::Empty { .. }));
        let message = err.to_string();
        assert!(message.contains("run-1.json"), "names the file: {message}");
        assert!(message.contains("delete"), "actionable: {message}");
    }

    #[test]
    fn run_load_ignores_interrupted_write_tmp_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        store
            .save_run(&run_record("run-1", RunState::Review))
            .unwrap();
        std::fs::write(tasks.join("runs/run-1.json.tmp"), "{ torn").unwrap();

        let loaded = store.load_all_runs().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].state, RunState::Review);
    }

    #[test]
    fn docs_only_plan_dirs_are_not_plan_records() {
        // A plans/<id>/ dir with docs but no record.json is a legacy snapshot
        // for a task that migrated to a run-only record (quick tasks): the
        // files stay, but there is no plan to load.
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        std::fs::create_dir_all(tasks.join("plans/task-old/.build")).unwrap();
        std::fs::write(tasks.join("plans/task-old/.build/plan.md"), "# old").unwrap();
        assert_eq!(store.load_all_plans().unwrap(), Vec::new());
    }

    #[test]
    fn delete_plan_removes_record_and_docs_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        store
            .save_plan(&plan_record("plan-1", PlanState::Approved))
            .unwrap();
        let worktree = dir.path().join("wt");
        std::fs::create_dir_all(worktree.join(".build")).unwrap();
        std::fs::write(worktree.join(".build/plan.md"), "# plan").unwrap();
        store
            .ingest_plan_docs("plan-1", &worktree, ".build/plan.md")
            .unwrap();

        store.delete_plan("plan-1").unwrap();
        assert_eq!(store.load_all_plans().unwrap(), Vec::new());
        assert_eq!(store.read_plan_doc("plan-1", ".build/plan.md"), None);
        // Deleting again (or a plan that never persisted) is not an error.
        store.delete_plan("plan-1").unwrap();
        store.delete_plan("never-existed").unwrap();
    }

    #[test]
    fn delete_run_removes_the_record_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        store
            .save_run(&run_record("run-1", RunState::Merged))
            .unwrap();
        store.delete_run("run-1").unwrap();
        assert_eq!(store.load_all_runs().unwrap(), Vec::new());
        store.delete_run("run-1").unwrap();
        store.delete_run("never-existed").unwrap();
    }

    // ================== Canonical doc ops (ingest / materialize / read) ==================

    /// A worktree with the standard plan layout: the single plan doc plus a
    /// multi-stage dir with a manifest and two stage docs.
    fn worktree_with_plan_docs(root: &Path) -> PathBuf {
        let worktree = root.join("wt");
        std::fs::create_dir_all(worktree.join(".build/plan")).unwrap();
        std::fs::write(worktree.join(".build/plan.md"), "# the plan").unwrap();
        std::fs::write(worktree.join(".build/plan/stages.json"), r#"[{"id":"s1"}]"#).unwrap();
        std::fs::write(worktree.join(".build/plan/01-first.md"), "stage one").unwrap();
        std::fs::write(worktree.join(".build/plan/02-second.md"), "stage two").unwrap();
        worktree
    }

    #[test]
    fn ingest_then_read_plan_doc_round_trips_the_docs() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let worktree = worktree_with_plan_docs(dir.path());

        store
            .ingest_plan_docs("plan-1", &worktree, ".build/plan.md")
            .unwrap();
        assert_eq!(
            store.read_plan_doc("plan-1", ".build/plan.md").as_deref(),
            Some("# the plan")
        );
        assert_eq!(
            store
                .read_plan_doc("plan-1", ".build/plan/01-first.md")
                .as_deref(),
            Some("stage one")
        );
        assert_eq!(
            store
                .read_plan_doc("plan-1", ".build/plan/stages.json")
                .as_deref(),
            Some(r#"[{"id":"s1"}]"#)
        );
    }

    #[test]
    fn reingest_overwrites_with_the_latest_docs() {
        // The revision loop: every plan/revise `done` re-ingests, and the
        // store copy always reflects the latest session's docs.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let worktree = worktree_with_plan_docs(dir.path());
        store
            .ingest_plan_docs("plan-1", &worktree, ".build/plan.md")
            .unwrap();
        std::fs::write(worktree.join(".build/plan.md"), "# revised").unwrap();
        store
            .ingest_plan_docs("plan-1", &worktree, ".build/plan.md")
            .unwrap();
        assert_eq!(
            store.read_plan_doc("plan-1", ".build/plan.md").as_deref(),
            Some("# revised")
        );
    }

    #[test]
    fn ingest_with_only_stage_docs_still_succeeds() {
        // A multi-stage plan may have no single plan.md; the stage dir alone
        // is a valid doc set.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let worktree = dir.path().join("wt");
        std::fs::create_dir_all(worktree.join(".build/plan")).unwrap();
        std::fs::write(worktree.join(".build/plan/01-only.md"), "only stage").unwrap();
        store
            .ingest_plan_docs("plan-1", &worktree, ".build/plan.md")
            .unwrap();
        assert_eq!(
            store
                .read_plan_doc("plan-1", ".build/plan/01-only.md")
                .as_deref(),
            Some("only stage")
        );
    }

    #[test]
    fn has_plan_docs_reflects_the_canonical_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        assert!(!store.has_plan_docs("plan-1"), "no docs dir yet");
        let worktree = dir.path().join("wt");
        std::fs::create_dir_all(worktree.join(".build")).unwrap();
        std::fs::write(worktree.join(".build/plan.md"), "# plan").unwrap();
        store
            .ingest_plan_docs("plan-1", &worktree, ".build/plan.md")
            .unwrap();
        assert!(store.has_plan_docs("plan-1"));
        assert!(!store.has_plan_docs("plan-2"), "scoped per plan");
    }

    #[test]
    fn re_ingest_mirrors_stage_doc_deletions_and_renames() {
        // A revision that drops or renames a stage doc must not leave the
        // stale file in the store — the next run would materialize and merge
        // it invisibly.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let worktree = dir.path().join("wt");
        std::fs::create_dir_all(worktree.join(".build/plan")).unwrap();
        std::fs::write(worktree.join(".build/plan/01-keep.md"), "keep").unwrap();
        std::fs::write(worktree.join(".build/plan/02-drop.md"), "drop").unwrap();
        store
            .ingest_plan_docs("plan-1", &worktree, ".build/plan.md")
            .unwrap();

        std::fs::remove_file(worktree.join(".build/plan/02-drop.md")).unwrap();
        std::fs::write(worktree.join(".build/plan/02-renamed.md"), "renamed").unwrap();
        store
            .ingest_plan_docs("plan-1", &worktree, ".build/plan.md")
            .unwrap();

        assert_eq!(
            store.read_plan_doc("plan-1", ".build/plan/01-keep.md"),
            Some("keep".into())
        );
        assert_eq!(
            store.read_plan_doc("plan-1", ".build/plan/02-renamed.md"),
            Some("renamed".into())
        );
        assert_eq!(
            store.read_plan_doc("plan-1", ".build/plan/02-drop.md"),
            None,
            "the deleted doc is gone from the store"
        );
    }

    #[test]
    fn ingest_fails_fast_when_the_worktree_has_no_docs() {
        // Unlike the legacy snapshot (a quiet mirror), ingest is the canonical
        // write: the done report claimed docs exist, so finding none is an
        // error and the plan must not advance.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let worktree = dir.path().join("wt-empty");
        std::fs::create_dir_all(&worktree).unwrap();

        let err = store
            .ingest_plan_docs("plan-1", &worktree, ".build/plan.md")
            .expect_err("empty ingest must fail");
        assert!(matches!(err, StoreError::NothingToIngest { .. }));
        assert!(err.to_string().contains("plan-1"), "names the plan: {err}");
    }

    #[test]
    fn ingest_refuses_an_escaping_plan_path() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let worktree = worktree_with_plan_docs(dir.path());
        for escapee in ["../outside.md", "/etc/passwd", ".build/../../etc/passwd"] {
            let err = store
                .ingest_plan_docs("plan-1", &worktree, escapee)
                .expect_err("escaping path must be rejected");
            assert!(matches!(err, StoreError::PathEscape { .. }), "{escapee}");
        }
    }

    #[test]
    fn materialize_recreates_the_worktree_layout() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        let source_worktree = worktree_with_plan_docs(dir.path());
        store
            .ingest_plan_docs("plan-1", &source_worktree, ".build/plan.md")
            .unwrap();

        let fresh_worktree = dir.path().join("wt-fresh");
        std::fs::create_dir_all(&fresh_worktree).unwrap();
        store
            .materialize_plan_docs("plan-1", &fresh_worktree)
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(fresh_worktree.join(".build/plan.md")).unwrap(),
            "# the plan"
        );
        assert_eq!(
            std::fs::read_to_string(fresh_worktree.join(".build/plan/02-second.md")).unwrap(),
            "stage two"
        );
        assert_eq!(
            std::fs::read_to_string(fresh_worktree.join(".build/plan/stages.json")).unwrap(),
            r#"[{"id":"s1"}]"#
        );
    }

    #[test]
    fn materialize_fails_fast_when_the_store_has_no_docs() {
        // Dispatching a planned run without its plan docs would silently build
        // from nothing — an error, never a no-op.
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        let worktree = dir.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();

        let err = store
            .materialize_plan_docs("plan-1", &worktree)
            .expect_err("no docs dir must fail");
        assert!(matches!(err, StoreError::NoStoredDocs { .. }));

        // An existing-but-empty docs dir is just as empty a plan.
        std::fs::create_dir_all(tasks.join("plans/plan-1/docs")).unwrap();
        let err = store
            .materialize_plan_docs("plan-1", &worktree)
            .expect_err("empty docs dir must fail");
        assert!(matches!(err, StoreError::NoStoredDocs { .. }));
    }

    #[test]
    fn read_plan_doc_refuses_traversal_and_absolute_paths() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("tasks"));
        assert_eq!(store.read_plan_doc("plan-1", "../plan-2/record.json"), None);
        assert_eq!(store.read_plan_doc("plan-1", "/etc/hostname"), None);
        assert_eq!(store.read_plan_doc("plan-1", ""), None);
        assert_eq!(store.read_plan_doc("plan-1", ".build/plan.md"), None);
    }

    // ================== Legacy-task boot migration ==================

    /// The exact JSON a legacy (fused-task) daemon wrote to disk. Built as raw
    /// JSON — not by serializing `PersistedTask` — so migration stays pinned
    /// to the real historical wire format even as the code evolves.
    fn legacy_task_json(id: &str, kind: &str, state: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "goal": "add a greeting",
            "kind": kind,
            "project_path": "/home/u/code/proj",
            "base_branch": "main",
            "state": state,
            "branch": "build/add-a-greeting",
            "worktree_name": "add-a-greeting",
            "worktree_path": "/home/u/.build/worktrees/add-a-greeting",
            "plan_path": ".build/plan.md",
            "last_summary": "worked on it",
            "model": "claude-opus-4-8",
            "effort": "xhigh",
            "last_error": null,
            "stages": [],
            "current_stage_id": null,
            "revising_stage_id": null,
            "auto_advance": false,
            "comments": [],
            "adopted": false,
            "pending_continuation": false,
            "created_at": "2026-07-01T10:00:00Z",
            "updated_at": "2026-07-01T10:05:00Z"
        })
    }

    fn write_legacy_task(store_dir: &Path, fixture: &serde_json::Value) {
        std::fs::create_dir_all(store_dir).unwrap();
        let id = fixture["id"].as_str().unwrap();
        std::fs::write(
            store_dir.join(format!("{id}.json")),
            serde_json::to_vec_pretty(fixture).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn migrate_quick_mid_build_becomes_a_run_only() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        write_legacy_task(
            &tasks,
            &legacy_task_json("task-q", "Quick", "Building".into()),
        );

        assert_eq!(store.migrate_legacy_tasks().unwrap(), 1);

        let runs = store.load_all_runs().unwrap();
        assert_eq!(runs.len(), 1);
        let run = &runs[0];
        assert_eq!(run.id, "task-q");
        assert_eq!(run.plan_id, None, "a quick task is a plan-less run");
        assert_eq!(run.state, RunState::Building);
        assert_eq!(run.base_sha, None, "legacy runs never recorded a base sha");
        assert_eq!(run.branch, "build/add-a-greeting");
        assert_eq!(run.worktree_path, "/home/u/.build/worktrees/add-a-greeting");
        assert_eq!(run.model.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(run.created_at, "2026-07-01T10:00:00Z");
        assert_eq!(store.load_all_plans().unwrap(), Vec::new());

        // The legacy file is renamed, not deleted, and ignored by the loader.
        assert!(tasks.join("task-q.json.migrated").is_file());
        assert!(!tasks.join("task-q.json").exists());
        assert_eq!(store.load_all().unwrap(), Vec::new());
    }

    #[test]
    fn migrate_standard_in_plan_review_becomes_a_plan_only() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        let mut fixture = legacy_task_json("task-s", "Standard", "PlanReview".into());
        fixture["stages"] = serde_json::json!([
            {
                "id": "s1", "title": "Stage one", "path": ".build/plan/01-s1.md",
                "summary": "first", "state": "approved", "start_sha": null, "validation": null
            },
            {
                "id": "s2", "title": "Stage two", "path": ".build/plan/02-s2.md",
                "summary": "second", "state": "planned", "start_sha": null, "validation": null
            }
        ]);
        fixture["comments"] = serde_json::json!([
            {
                "id": "c-1", "stage_id": "s1",
                "anchor": { "heading_path": ["Stage one"], "snippet": "the tables" },
                "body": "tighten this", "state": "open", "agent_reply": null
            }
        ]);
        write_legacy_task(&tasks, &fixture);
        // The legacy snapshot dir is already in the right place; migration
        // promotes it to plans/<id>/docs/.
        std::fs::create_dir_all(tasks.join("plans/task-s/.build/plan")).unwrap();
        std::fs::write(tasks.join("plans/task-s/.build/plan.md"), "# plan").unwrap();
        std::fs::write(tasks.join("plans/task-s/.build/plan/01-s1.md"), "s1 doc").unwrap();

        assert_eq!(store.migrate_legacy_tasks().unwrap(), 1);

        assert_eq!(store.load_all_runs().unwrap(), Vec::new());
        let plans = store.load_all_plans().unwrap();
        assert_eq!(plans.len(), 1);
        let plan = &plans[0];
        assert_eq!(plan.id, "task-s");
        assert_eq!(plan.state, PlanState::PlanReview);
        // Never past planning: the task's worktree was the planning worktree.
        assert_eq!(plan.worktree_name.as_deref(), Some("add-a-greeting"));
        assert_eq!(plan.branch.as_deref(), Some("build/add-a-greeting"));
        assert_eq!(plan.plan_path, ".build/plan.md");
        assert_eq!(plan.stages.len(), 2);
        assert_eq!(plan.stages[0].state, StageDocState::Approved);
        assert_eq!(plan.stages[1].state, StageDocState::Planned);
        assert_eq!(plan.comments.len(), 1);
        assert_eq!(plan.comments[0].body, "tighten this");
        assert_eq!(
            plan.comments[0].anchor.as_ref().unwrap().heading_path,
            vec!["Stage one".to_string()]
        );
        // Snapshot docs were promoted to the canonical location.
        assert_eq!(
            store.read_plan_doc("task-s", ".build/plan.md").as_deref(),
            Some("# plan")
        );
        assert_eq!(
            store
                .read_plan_doc("task-s", ".build/plan/01-s1.md")
                .as_deref(),
            Some("s1 doc")
        );
    }

    #[test]
    fn migrate_multi_stage_mid_run_parks_the_run_at_the_stage_gate() {
        // The fused machine reused PlanReview as the between-stages board;
        // with any run-side stage progress that position is the run's
        // StageGate, and the plan (approved to get there) rests at Approved.
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        let mut fixture = legacy_task_json("task-m", "Standard", "PlanReview".into());
        fixture["stages"] = serde_json::json!([
            {
                "id": "s1", "title": "Stage one", "path": ".build/plan/01-s1.md",
                "summary": "first", "state": { "validated": { "passed": true } },
                "start_sha": "deadbeef",
                "validation": { "passed": true, "findings": "all good", "notes_for_next_stage": "careful" }
            },
            {
                "id": "s2", "title": "Stage two", "path": ".build/plan/02-s2.md",
                "summary": "second", "state": "approved", "start_sha": null, "validation": null
            }
        ]);
        fixture["current_stage_id"] = "s1".into();
        fixture["auto_advance"] = true.into();
        write_legacy_task(&tasks, &fixture);

        assert_eq!(store.migrate_legacy_tasks().unwrap(), 1);

        let plans = store.load_all_plans().unwrap();
        assert_eq!(plans.len(), 1);
        let plan = &plans[0];
        assert_eq!(plan.state, PlanState::Approved);
        assert_eq!(
            plan.worktree_name, None,
            "past planning: the worktree belongs to the run"
        );
        // A dispatched stage's doc was necessarily approved.
        assert_eq!(plan.stages[0].state, StageDocState::Approved);
        assert_eq!(plan.stages[1].state, StageDocState::Approved);

        let runs = store.load_all_runs().unwrap();
        assert_eq!(runs.len(), 1);
        let run = &runs[0];
        assert_eq!(run.plan_id.as_deref(), Some("task-m"));
        assert_eq!(run.state, RunState::StageGate);
        assert_eq!(run.worktree_name, "add-a-greeting");
        // Only dispatched stages have run-side progress records.
        assert_eq!(run.stages.len(), 1);
        assert_eq!(run.stages[0].stage_id, "s1");
        assert_eq!(
            run.stages[0].state,
            StageProgressState::Validated { passed: true }
        );
        assert_eq!(run.stages[0].start_sha.as_deref(), Some("deadbeef"));
        let validation = run.stages[0].validation.as_ref().unwrap();
        assert!(validation.passed);
        assert_eq!(validation.notes_for_next_stage, "careful");
        assert_eq!(run.current_stage_id.as_deref(), Some("s1"));
        assert!(run.auto_advance);
    }

    #[test]
    fn migrate_standard_mid_build_splits_into_approved_plan_and_building_run() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        write_legacy_task(
            &tasks,
            &legacy_task_json("task-b", "Standard", "Building".into()),
        );

        assert_eq!(store.migrate_legacy_tasks().unwrap(), 1);
        let plans = store.load_all_plans().unwrap();
        assert_eq!(plans[0].state, PlanState::Approved);
        assert_eq!(plans[0].id, "task-b");
        let runs = store.load_all_runs().unwrap();
        assert_eq!(runs[0].state, RunState::Building);
        assert_eq!(runs[0].plan_id.as_deref(), Some("task-b"));
        // The two halves must NOT share an id: done-report routing, agent
        // screens, and the entity maps all rely on plan/run ids being
        // crate-wide disjoint.
        assert_eq!(runs[0].id, "run-task-b");
    }

    #[test]
    fn migrate_ingests_docs_from_the_live_worktree_when_no_snapshot_exists() {
        // Records older than the snapshot mirror (or whose best-effort mirror
        // silently failed) hold their only docs in the worktree.
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        let worktree = dir.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::create_dir_all(worktree.join(".build")).unwrap();
        std::fs::write(worktree.join(".build/plan.md"), "# from the worktree").unwrap();
        let mut fixture = legacy_task_json("task-w", "Standard", "PlanReview".into());
        fixture["worktree_path"] = serde_json::json!(worktree.to_str().unwrap());
        write_legacy_task(&tasks, &fixture);

        assert_eq!(store.migrate_legacy_tasks().unwrap(), 1);
        assert_eq!(
            store.read_plan_doc("task-w", ".build/plan.md").as_deref(),
            Some("# from the worktree")
        );
    }

    #[test]
    fn migrate_blocked_during_planning_is_a_blocked_plan_only() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        write_legacy_task(
            &tasks,
            &legacy_task_json(
                "task-p",
                "Standard",
                serde_json::json!({ "Blocked": "Plan" }),
            ),
        );

        assert_eq!(store.migrate_legacy_tasks().unwrap(), 1);
        let plans = store.load_all_plans().unwrap();
        assert_eq!(plans[0].state, PlanState::Blocked);
        assert_eq!(store.load_all_runs().unwrap(), Vec::new());
    }

    #[test]
    fn migrate_terminal_tasks_map_terminal_runs_with_the_plan_kept() {
        for (id, legacy_state, run_state) in [
            ("task-merged", "Merged", RunState::Merged),
            ("task-abandoned", "Abandoned", RunState::Abandoned),
            ("task-archived", "Archived", RunState::Archived),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let tasks = dir.path().join("tasks");
            let store = Store::new(&tasks);
            write_legacy_task(
                &tasks,
                &legacy_task_json(id, "Standard", legacy_state.into()),
            );

            assert_eq!(store.migrate_legacy_tasks().unwrap(), 1);
            let plans = store.load_all_plans().unwrap();
            assert_eq!(plans.len(), 1, "{legacy_state}: the plan is kept");
            assert_eq!(plans[0].state, PlanState::Approved);
            let runs = store.load_all_runs().unwrap();
            assert_eq!(runs[0].state, run_state, "from legacy {legacy_state}");
        }
    }

    #[test]
    fn migrate_empty_store_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        // A store dir that never existed (first boot).
        let store = Store::new(dir.path().join("never-created"));
        assert_eq!(store.migrate_legacy_tasks().unwrap(), 0);
        // An existing but empty store dir.
        let tasks = dir.path().join("tasks");
        std::fs::create_dir_all(&tasks).unwrap();
        let store = Store::new(&tasks);
        assert_eq!(store.migrate_legacy_tasks().unwrap(), 0);
        assert_eq!(store.load_all_plans().unwrap(), Vec::new());
        assert_eq!(store.load_all_runs().unwrap(), Vec::new());
    }

    #[test]
    fn migrate_is_idempotent_after_a_successful_run() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        write_legacy_task(
            &tasks,
            &legacy_task_json("task-q", "Quick", "Building".into()),
        );

        assert_eq!(store.migrate_legacy_tasks().unwrap(), 1);
        assert_eq!(store.migrate_legacy_tasks().unwrap(), 0, "nothing left");
        assert_eq!(store.load_all_runs().unwrap().len(), 1);
    }

    #[test]
    fn migrate_never_clobbers_existing_new_format_records() {
        // A crash between the record writes and the legacy-file rename re-runs
        // the migration; records written (and possibly since updated) by the
        // new world must survive untouched.
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = Store::new(&tasks);
        let fixture = legacy_task_json("task-q", "Quick", "Building".into());
        write_legacy_task(&tasks, &fixture);
        assert_eq!(store.migrate_legacy_tasks().unwrap(), 1);

        // The new world moves the run on; then the legacy file "reappears"
        // (the crash-window shape: record present, rename missing).
        let mut moved_on = store.load_all_runs().unwrap().remove(0);
        moved_on.state = RunState::Merged;
        store.save_run(&moved_on).unwrap();
        write_legacy_task(&tasks, &fixture);

        assert_eq!(store.migrate_legacy_tasks().unwrap(), 1);
        let runs = store.load_all_runs().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(
            runs[0].state,
            RunState::Merged,
            "the migrated record was not overwritten"
        );
        assert!(!tasks.join("task-q.json").exists(), "stray legacy renamed");
    }

    #[test]
    fn migrate_fails_fast_on_a_corrupt_legacy_record() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        std::fs::create_dir_all(&tasks).unwrap();
        let store = Store::new(&tasks);
        std::fs::write(tasks.join("task-x.json"), "{ not json").unwrap();

        let err = store
            .migrate_legacy_tasks()
            .expect_err("corrupt legacy record must fail loudly");
        assert!(matches!(err, StoreError::Corrupt { .. }));
        assert!(err.to_string().contains("task-x.json"));
        assert!(
            tasks.join("task-x.json").is_file(),
            "the corrupt file is left in place for the human"
        );
    }

    /// Attention is ordering and colour, never correctness: it round-trips, it
    /// prunes to the world that still exists, and a corrupt file costs a badly
    /// sorted rail rather than a task.
    #[test]
    fn attention_round_trips_and_prunes_to_the_living() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        assert!(
            store.load_attention().is_empty(),
            "nothing until something is stamped"
        );

        let mut map = HashMap::new();
        let mut run = Attention::default();
        run.interact("2026-07-27T09:00:00Z");
        run.see("2026-07-27T09:00:00Z");
        map.insert("run-1".to_string(), run.clone());
        let mut gone = Attention::default();
        gone.interact("2026-07-20T09:00:00Z");
        map.insert("wt-deleted".to_string(), gone);

        let live: HashSet<String> = ["run-1".to_string()].into_iter().collect();
        store.save_attention(&map, &live).unwrap();

        let loaded = store.load_attention();
        assert_eq!(
            loaded.len(),
            1,
            "the deleted worktree dropped out: {loaded:?}"
        );
        assert_eq!(loaded.get("run-1"), Some(&run));
    }

    #[test]
    fn a_corrupt_attention_file_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        std::fs::create_dir_all(dir.path()).unwrap();
        std::fs::create_dir_all(dir.path().join("attention")).unwrap();
        std::fs::write(dir.path().join("attention").join("map.json"), "{not json").unwrap();
        assert!(store.load_attention().is_empty());
    }
}
