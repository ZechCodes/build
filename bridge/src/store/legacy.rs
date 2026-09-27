use super::{
    PersistedArchivedWorktree, PersistedPlan, PersistedRun, PersistedTask, Store, StoreError,
};
use crate::attention::Attention;
use crate::models::ModelChoice;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

impl Store {
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
        // Tasks, with the implementations nested inside each aggregate.
        let tasks_dir = self.dir.join(Store::PLANS_DIR);
        if tasks_dir.is_dir() {
            for entry in std::fs::read_dir(&tasks_dir)? {
                let record_path = entry?.path().join("record.json");
                if !record_path.is_file() {
                    continue;
                }
                let aggregate: PersistedTask = read_record(&record_path)?;
                self.save_task_plan(&aggregate.task)?;
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
    /// Read every `*.json` in one store subdirectory and hand each record to
    /// `save`. A record that will not parse is fatal: dropping one silently
    /// would orphan its worktree and lose the user's work without a trace,
    /// which is the same rule the JSON store booted under.
    pub(super) fn import_dir<T: serde::de::DeserializeOwned>(
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
}

/// Only `*.json` files are records; `.tmp` leftovers from an interrupted
/// write are ignored (the rename never happened, so the previous record — or
/// no record — is the truth), and so are `.migrated` legacy files.
pub(super) fn is_json_record(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("json")
}

/// The worktree-relative dir multi-stage plan docs live in.
pub(super) const STAGE_PLAN_DIR: &str = ".build/plan";

/// Read and parse one JSON record, failing fast — with the file named — on
/// the two torn-record shapes: empty (power loss after the rename) and
/// unparseable.
pub(super) fn read_record<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, StoreError> {
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
pub(super) fn dir_contains_a_file(dir: &Path) -> bool {
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
pub(super) fn remove_dir_if_present(path: &Path) -> Result<(), StoreError> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StoreError::Io(error)),
    }
}

pub(super) fn remove_file_if_present(path: &Path) -> Result<(), StoreError> {
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
pub(super) fn copy_tree(
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

pub(super) struct LegacyOwnerContext {
    pub(super) choice: ModelChoice,
    pub(super) task_id: Option<String>,
}

pub(super) fn load_legacy_owner_context(
    conn: &Connection,
) -> Result<HashMap<String, LegacyOwnerContext>, StoreError> {
    let mut owners = HashMap::new();
    let mut tasks = conn.prepare("SELECT id, record FROM tasks")?;
    let task_rows: Vec<(String, String)> = tasks
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    drop(tasks);
    for (id, raw) in task_rows {
        let record: PersistedPlan =
            serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                path: PathBuf::from(format!("{}/{id}", Store::PLANS_DIR)),
                source,
            })?;
        owners.insert(
            id,
            LegacyOwnerContext {
                choice: ModelChoice {
                    provider: record.provider,
                    model: record.model,
                    effort: record.effort,
                },
                task_id: None,
            },
        );
    }
    let mut runs = conn.prepare("SELECT id, record FROM implementations")?;
    let run_rows: Vec<(String, String)> = runs
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    drop(runs);
    for (id, raw) in run_rows {
        let record: PersistedRun =
            serde_json::from_str(&raw).map_err(|source| StoreError::Corrupt {
                path: PathBuf::from(format!("implementations/{id}")),
                source,
            })?;
        owners.insert(
            id,
            LegacyOwnerContext {
                choice: ModelChoice {
                    provider: record.provider,
                    model: record.model,
                    effort: record.effort,
                },
                task_id: record.plan_id,
            },
        );
    }
    Ok(owners)
}
