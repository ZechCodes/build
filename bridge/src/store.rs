//! Durable task persistence: one JSON file per task under the bridge state dir
//! (`~/.build/tasks/<task_id>.json` by default, next to the identity file).
//!
//! The store holds the *durable core* of a task — id, goal, kind, project,
//! lifecycle state, branch, worktree path, plan path, timestamps, and the last
//! `done` summary. It is written atomically (tmp file + rename) on every state
//! transition, so a daemon crash never leaves a half-written record, and read
//! back in full on boot so a restart re-attaches every task instead of
//! orphaning the worktrees that survived on disk.
//!
//! Live PTY output streams are intentionally **not** persisted: the terminal is
//! reconstructable observation, not state. What is durable is what the review
//! surfaces need — the lifecycle position and where the files live.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::task::{Stage, StageComment, TaskKind, TaskState};

/// Things that can go wrong reading or writing the task store.
#[derive(Debug, thiserror::Error)]
pub enum TaskStoreError {
    #[error("task store io error: {0}")]
    Io(#[from] std::io::Error),
    /// A task file exists but cannot be parsed. Boot fails fast on this — a
    /// silently dropped task would orphan its worktree and lose the user's work
    /// without a trace.
    #[error("corrupt task file {path}: {source}")]
    Corrupt {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    /// A task file exists but is empty — the signature of a power loss between
    /// a rename becoming durable and the data blocks reaching disk. Named and
    /// actionable (the record held no recoverable data) rather than a bare
    /// JSON-parse error.
    #[error(
        "empty task file {path}: an interrupted write left no data — delete it to boot \
         (its worktree, if any, is untouched on disk)"
    )]
    Empty { path: PathBuf },
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
    /// RFC 3339 UTC timestamps.
    pub created_at: String,
    pub updated_at: String,
}

/// One-file-per-task JSON store with atomic writes.
pub struct TaskStore {
    dir: PathBuf,
}

impl TaskStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        TaskStore { dir: dir.into() }
    }

    /// Where a task's record lives.
    pub fn path_for(&self, task_id: &str) -> PathBuf {
        self.dir.join(format!("{task_id}.json"))
    }

    /// Persist one task record atomically **and durably**: write to a `.tmp`
    /// sibling, fsync it, rename over the final path, then fsync the directory.
    /// The fsyncs matter: rename-without-fsync is atomic against a process crash
    /// but not against power loss — the rename can become durable before the data
    /// blocks, leaving a zero-length record that blocks the next boot.
    pub fn save(&self, record: &PersistedTask) -> Result<(), TaskStoreError> {
        use std::io::Write;

        std::fs::create_dir_all(&self.dir)?;
        let final_path = self.path_for(&record.id);
        let tmp_path = self.dir.join(format!("{}.json.tmp", record.id));
        let json = serde_json::to_string_pretty(record).expect("a task record always serializes");
        let mut tmp_file = std::fs::File::create(&tmp_path)?;
        tmp_file.write_all(json.as_bytes())?;
        tmp_file.sync_all()?;
        drop(tmp_file);
        std::fs::rename(&tmp_path, &final_path)?;
        // Make the rename itself durable (best-effort where the platform allows
        // opening a directory read-only).
        if let Ok(dir_handle) = std::fs::File::open(&self.dir) {
            let _ = dir_handle.sync_all();
        }
        Ok(())
    }

    /// Load every task record in the store, ordered by creation time. A missing
    /// store dir means no tasks (first boot). A file that exists but does not
    /// parse is a hard error naming the file — never a silently dropped task.
    pub fn load_all(&self) -> Result<Vec<PersistedTask>, TaskStoreError> {
        if !self.dir.exists() {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let path = entry?.path();
            if !is_task_file(&path) {
                continue;
            }
            let text = std::fs::read_to_string(&path)?;
            if text.trim().is_empty() {
                return Err(TaskStoreError::Empty { path });
            }
            let record: PersistedTask =
                serde_json::from_str(&text).map_err(|source| TaskStoreError::Corrupt {
                    path: path.clone(),
                    source,
                })?;
            records.push(record);
        }
        records.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        Ok(records)
    }

    /// Delete a task's persisted record (and any leftover `.tmp` from an interrupted
    /// write). Removing a record that isn't there is not an error — delete is only
    /// ever called for a terminal task the board wants gone, and idempotency keeps a
    /// double-delete or a never-persisted task from failing the RPC.
    pub fn delete(&self, task_id: &str) -> Result<(), TaskStoreError> {
        for path in [
            self.path_for(task_id),
            self.dir.join(format!("{task_id}.json.tmp")),
        ] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(TaskStoreError::Io(e)),
            }
        }
        Ok(())
    }
}

/// Only `*.json` files are task records; `.tmp` leftovers from an interrupted
/// write are ignored (the rename never happened, so the previous record — or
/// no record — is the truth).
fn is_task_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("json")
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
    use crate::task::{
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
        let store = TaskStore::new(dir.path());
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
        let store = TaskStore::new(dir.path());
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
        let store = TaskStore::new(dir.path().join("tasks"));
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

    #[test]
    fn delete_removes_the_record_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::new(dir.path().join("tasks"));
        store.save(&record("task-1", TaskState::Merged)).unwrap();
        assert_eq!(store.load_all().unwrap().len(), 1);

        store.delete("task-1").unwrap();
        assert_eq!(store.load_all().unwrap(), Vec::new(), "record gone");
        // Deleting again (or a task that never persisted) is not an error.
        store.delete("task-1").unwrap();
        store.delete("never-existed").unwrap();
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
            created_at: "2026-07-01T10:00:00Z".into(),
            updated_at: "2026-07-01T10:05:00Z".into(),
        }
    }

    #[test]
    fn save_then_load_round_trips_every_field() {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::new(dir.path().join("tasks"));
        let rec = record("task-1", TaskState::PlanReview);
        store.save(&rec).unwrap();

        let loaded = store.load_all().unwrap();
        assert_eq!(loaded, vec![rec]);
    }

    #[test]
    fn save_overwrites_atomically_leaving_no_tmp_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::new(dir.path().join("tasks"));
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
        let store = TaskStore::new(dir.path().join("tasks"));
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
        let store = TaskStore::new(dir.path().join("never-created"));
        assert_eq!(store.load_all().unwrap(), Vec::new());
    }

    #[test]
    fn corrupt_task_file_is_a_hard_error_naming_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = TaskStore::new(&tasks);
        store.save(&record("task-1", TaskState::Review)).unwrap();
        std::fs::write(tasks.join("task-2.json"), "{ not json").unwrap();

        let err = store.load_all().expect_err("corrupt file must fail loudly");
        let message = err.to_string();
        assert!(
            message.contains("task-2.json"),
            "error names the corrupt file: {message}"
        );
        assert!(matches!(err, TaskStoreError::Corrupt { .. }));
    }

    #[test]
    fn empty_task_file_is_a_named_actionable_error() {
        // A power loss can make the rename durable before the data blocks: the
        // record exists but is zero-length. The error must say exactly which file
        // and that deleting it is safe — not a bare JSON parse error.
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = TaskStore::new(&tasks);
        store.save(&record("task-1", TaskState::Review)).unwrap();
        std::fs::write(tasks.join("task-2.json"), "").unwrap();

        let err = store.load_all().expect_err("empty file must fail loudly");
        assert!(matches!(err, TaskStoreError::Empty { .. }));
        let message = err.to_string();
        assert!(message.contains("task-2.json"), "names the file: {message}");
        assert!(message.contains("delete"), "actionable: {message}");
    }

    #[test]
    fn interrupted_write_leftover_tmp_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let tasks = dir.path().join("tasks");
        let store = TaskStore::new(&tasks);
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
        let store = TaskStore::new(dir.path().join("tasks"));
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
}
