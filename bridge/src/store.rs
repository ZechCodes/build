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

use crate::task::{TaskKind, TaskState};

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

    /// Persist one task record atomically: write to a `.tmp` sibling, then
    /// rename over the final path, so readers never observe a torn file.
    pub fn save(&self, record: &PersistedTask) -> Result<(), TaskStoreError> {
        std::fs::create_dir_all(&self.dir)?;
        let final_path = self.path_for(&record.id);
        let tmp_path = self.dir.join(format!("{}.json.tmp", record.id));
        let json = serde_json::to_string_pretty(record).expect("a task record always serializes");
        std::fs::write(&tmp_path, json)?;
        std::fs::rename(&tmp_path, &final_path)?;
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
    use crate::task::{Phase, TaskKind, TaskState};

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
