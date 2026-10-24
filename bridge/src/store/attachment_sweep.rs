//! Garbage collection for the bridge's content-addressed attachment copies.

use super::{Store, StoreError, DB_FILE};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::SystemTime;

static ATTACHMENT_ADMISSION: Mutex<()> = Mutex::new(());

/// Serialize file admission/refresh with the final bounded unlink phase.
pub(crate) fn attachment_admission() -> MutexGuard<'static, ()> {
    ATTACHMENT_ADMISSION
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

/// An attachment validated before its record commits receives the same grace
/// period as a new upload. Call while holding `attachment_admission`.
pub(crate) fn refresh_attachment_admission(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path)?.set_modified(SystemTime::now())
}

/// A bounded deletion plan. Prepare and apply outside application locks. Concurrent durable writes invalidate it.
pub struct AttachmentSweep {
    observed_changes: u64,
    observed_data_version: i64,
    candidates: Vec<PathBuf>,
    older_than: SystemTime,
}

const SWEEP_BATCH_SIZE: usize = 128;

impl Store {
    /// Read the durable reference snapshot and enumerate flat attachment homes
    /// without taking the application lock or holding the writer during scans.
    pub fn prepare_attachment_sweep(
        &self,
        homes: &[PathBuf],
        older_than: SystemTime,
    ) -> Result<AttachmentSweep, StoreError> {
        let (observed_changes, observed_data_version) = {
            let writer = self.connection();
            (
                writer.total_changes(),
                writer.pragma_query_value(None, "data_version", |row| row.get(0))?,
            )
        };
        let mut reader =
            Connection::open_with_flags(self.dir.join(DB_FILE), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let snapshot = reader.transaction()?;
        let referenced = referenced_leaves(&snapshot)?;
        let mut candidates = Vec::new();
        for home in homes {
            collect_candidates(home, &referenced, older_than, &mut candidates)?;
            if candidates.len() == SWEEP_BATCH_SIZE {
                break;
            }
        }
        Ok(AttachmentSweep {
            observed_changes,
            observed_data_version,
            candidates,
            older_than,
        })
    }

    /// Apply outside application locks. Holding the writer
    /// fences new durable references; the short bounded batch does no SQL scan.
    /// A changed database is retried on a later maintenance tick.
    pub fn apply_attachment_sweep(&self, sweep: AttachmentSweep) -> Result<usize, StoreError> {
        self.in_transaction(|connection| {
            let data_version: i64 =
                connection.pragma_query_value(None, "data_version", |row| row.get(0))?;
            if connection.total_changes() != sweep.observed_changes
                || data_version != sweep.observed_data_version
            {
                return Ok(0);
            }
            let _admission = attachment_admission();
            let mut deleted = 0;
            for path in sweep.candidates {
                if !old_regular_file(&path, sweep.older_than)? {
                    continue;
                }
                match std::fs::remove_file(path) {
                    Ok(()) => deleted += 1,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(deleted)
        })
    }

    /// Sweep one bounded batch outside application locks.
    pub fn sweep_unreferenced_attachments(
        &self,
        homes: &[PathBuf],
        older_than: SystemTime,
    ) -> Result<usize, StoreError> {
        self.apply_attachment_sweep(self.prepare_attachment_sweep(homes, older_than)?)
    }
}

fn referenced_leaves(connection: &Connection) -> Result<HashSet<String>, StoreError> {
    let mut retained = HashSet::new();
    // Every durable JSON document participates, including nested review and
    // journal payloads. JSON atoms are compared as paths, never substrings of
    // serialized JSON or prose. Schema discovery keeps new record tables safe.
    let mut columns = connection.prepare(
        "SELECT s.name, p.name FROM sqlite_schema s, pragma_table_info(s.name) p
         WHERE s.type = 'table' AND p.name IN ('record', 'item', 'delivery', 'intent')",
    )?;
    let documents = columns.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for document in documents {
        let (table, column) = document?;
        let sql = format!(
            "SELECT j.atom FROM {} d, json_tree(d.{}) j WHERE j.type = 'text' AND j.key = 'path'",
            quoted_identifier(&table),
            quoted_identifier(&column),
        );
        let mut paths = connection.prepare(&sql)?;
        for path in paths.query_map([], |row| row.get::<_, String>(0))? {
            retain_leaf(&mut retained, &path?);
        }
    }
    let mut uploads = connection.prepare("SELECT path FROM conversation_attachment_uploads")?;
    for path in uploads.query_map([], |row| row.get::<_, String>(0))? {
        retain_leaf(&mut retained, &path?);
    }
    Ok(retained)
}

fn quoted_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn retain_leaf(retained: &mut HashSet<String>, path: &str) {
    if let Some(leaf) = Path::new(path).file_name().and_then(|name| name.to_str()) {
        retained.insert(leaf.to_owned());
    }
}

fn collect_candidates(
    home: &Path,
    retained: &HashSet<String>,
    older_than: SystemTime,
    candidates: &mut Vec<PathBuf>,
) -> Result<(), StoreError> {
    let metadata = match home.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    // Never descend into symlinked roots or subdirectories.
    if !metadata.file_type().is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(home)? {
        let entry = entry?;
        if !retained.contains(&entry.file_name().to_string_lossy().into_owned())
            && old_regular_file(&entry.path(), older_than)?
        {
            candidates.push(entry.path());
            if candidates.len() == SWEEP_BATCH_SIZE {
                break;
            }
        }
    }
    Ok(())
}

fn old_regular_file(path: &Path, older_than: SystemTime) -> Result<bool, StoreError> {
    match path.symlink_metadata() {
        Ok(metadata) => Ok(metadata.file_type().is_file() && metadata.modified()? < older_than),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::Store;
    use rusqlite::params;
    use serde_json::json;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn old_file(home: &std::path::Path, leaf: &str) -> PathBuf {
        std::fs::create_dir_all(home).unwrap();
        let path = home.join(leaf);
        std::fs::write(&path, b"attachment").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(100))
            .unwrap();
        path
    }

    #[test]
    fn sweep_preserves_exact_references_across_durable_documents_and_uploads() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let local = store.attachments_dir();
        let worktree = dir.path().join("checkout/.build/attachments");
        let references = [
            "thread.png",
            "task.png",
            "comment.png",
            "review.png",
            "operation.png",
            "journal.png",
            "upload.png",
        ];
        {
            let conn = store.connection();
            conn.execute("INSERT INTO thread_items (agent_id, sequence, updated_sequence, item) VALUES ('a', 1, 1, ?1)", [json!({"data":{"attachments":[{"path":".build/attachments/thread.png"}]}}).to_string()]).unwrap();
            conn.execute("INSERT INTO tracker_tasks (id, project_key, number, state, status, created_at, updated_at, record) VALUES ('t', 'p', 1, 'open', 'todo', '', '', ?1)", [json!({"attachments":[{"path":local.join("task.png") }]}).to_string()]).unwrap();
            conn.execute("INSERT INTO tracker_comments (id, task_id, created_at, record) VALUES ('c', 't', '', ?1)", [json!({"attachments":[{"path":".build/attachments/comment.png"}]}).to_string()]).unwrap();
            conn.execute("INSERT INTO reviews (task_id, workspace_id, version, record) VALUES ('t', 'w', 1, ?1)", [json!({"nested":{"attachments":[{"path":local.join("review.png")}]}}).to_string()]).unwrap();
            conn.execute("INSERT INTO operations (operation_id, method, entity_id, agent_id, conversation_id, choice_revision, request_hash, posted_sequence, message_start_sequence, status, delivery, created_at, updated_at) VALUES ('o', 'thread.post', 'e', 'a', 'a', 1, '', 1, 1, 'queued', ?1, '', '')", [json!({"payload":{"messages":[{"attachments":[{"path":".build/attachments/operation.png"}]}]}}).to_string()]).unwrap();
            conn.execute("INSERT INTO mutation_commands (principal, operation_id, state, accepted_at, intent, record) VALUES ('p', 'o', 'accepted', 1, ?1, '{}')", [json!({"attachments":[{"path":".build/attachments/journal.png"}]}).to_string()]).unwrap();
            conn.execute("INSERT INTO conversation_attachment_uploads (conversation_id, thread_id, path) VALUES ('a', 'g', ?1)", [".build/attachments/upload.png"]).unwrap();
        }
        for leaf in references {
            old_file(&local, leaf);
            old_file(&worktree, leaf);
        }
        let orphan = old_file(&local, "orphan.png");
        let orphan_copy = old_file(&worktree, "orphan.png");
        let result = store
            .sweep_unreferenced_attachments(
                &[local.clone(), worktree.clone()],
                SystemTime::UNIX_EPOCH + Duration::from_secs(200),
            )
            .unwrap();
        assert_eq!(result, 2);
        assert!(!orphan.exists());
        assert!(!orphan_copy.exists());
        for leaf in references {
            assert!(local.join(leaf).exists(), "{leaf}");
            assert!(worktree.join(leaf).exists(), "{leaf}");
        }
    }

    #[test]
    fn sweep_ignores_substrings_new_uploads_directories_and_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let home = store.attachments_dir();
        let orphan = old_file(&home, "orphan.png");
        let fresh = old_file(&home, "fresh.png");
        std::fs::File::options()
            .write(true)
            .open(&fresh)
            .unwrap()
            .set_modified(SystemTime::now())
            .unwrap();
        std::fs::create_dir_all(home.join("folder")).unwrap();
        let nested = old_file(&home.join("folder"), "untouched.png");
        store.connection().execute("INSERT INTO reviews (task_id, workspace_id, version, record) VALUES ('t', 'w', 1, ?1)", params![json!({"body":"mentions .build/attachments/orphan.png", "attachments":[{"path":".build/attachments/orphan.png.extra"}]}).to_string()]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&orphan, home.join("link.png")).unwrap();
        assert_eq!(
            store
                .sweep_unreferenced_attachments(
                    &[home.clone(), dir.path().join("missing")],
                    SystemTime::UNIX_EPOCH + Duration::from_secs(200)
                )
                .unwrap(),
            1
        );
        assert!(!orphan.exists());
        assert!(fresh.exists());
        assert!(nested.exists());
        #[cfg(unix)]
        assert!(home
            .join("link.png")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
    }
    #[test]
    fn apply_retries_if_a_reference_was_committed_after_the_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let home = store.attachments_dir();
        let file = old_file(&home, "late.png");
        let plan = store
            .prepare_attachment_sweep(&[home], SystemTime::now())
            .unwrap();
        store.connection().execute("INSERT INTO reviews (task_id, workspace_id, version, record) VALUES ('t', 'w', 1, ?1)", [json!({"attachments":[{"path":".build/attachments/late.png"}]}).to_string()]).unwrap();
        assert_eq!(store.apply_attachment_sweep(plan).unwrap(), 0);
        assert!(file.exists());
    }

    #[test]
    fn sweep_is_bounded_and_rechecks_file_age_at_apply() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let home = store.attachments_dir();
        for index in 0..140 {
            old_file(&home, &format!("{index}.png"));
        }
        let cutoff = SystemTime::UNIX_EPOCH + Duration::from_secs(200);
        assert_eq!(
            store
                .sweep_unreferenced_attachments(&[home.clone()], cutoff)
                .unwrap(),
            128
        );
        assert_eq!(
            store
                .sweep_unreferenced_attachments(&[home.clone()], cutoff)
                .unwrap(),
            12
        );
        let refreshed = old_file(&home, "refreshed.png");
        let plan = store.prepare_attachment_sweep(&[home], cutoff).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&refreshed)
            .unwrap()
            .set_modified(SystemTime::now())
            .unwrap();
        assert_eq!(store.apply_attachment_sweep(plan).unwrap(), 0);
        assert!(refreshed.exists());
    }

    #[test]
    fn apply_detects_references_committed_by_another_connection() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let home = store.attachments_dir();
        let file = old_file(&home, "external.png");
        let plan = store
            .prepare_attachment_sweep(&[home], SystemTime::now())
            .unwrap();
        let external = rusqlite::Connection::open(dir.path().join("build.db")).unwrap();
        external.execute("INSERT INTO reviews (task_id, workspace_id, version, record) VALUES ('t', 'w', 1, ?1)", [json!({"attachments":[{"path":".build/attachments/external.png"}]}).to_string()]).unwrap();
        assert_eq!(store.apply_attachment_sweep(plan).unwrap(), 0);
        assert!(file.exists());
    }

    #[test]
    fn corrupt_reference_documents_abort_before_any_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let home = store.attachments_dir();
        let file = old_file(&home, "keep.png");
        store.connection().execute("INSERT INTO tracker_comments (id, task_id, created_at, record) VALUES ('c', 't', '', '{')", []).unwrap();
        assert!(store
            .sweep_unreferenced_attachments(&[home], SystemTime::now())
            .is_err());
        assert!(file.exists());
    }
}
