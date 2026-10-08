use super::{OpenReviewRequest, ReviewerDispatch};
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub(super) fn lock(request: &OpenReviewRequest) -> Result<File, String> {
    let root = crate::worktree::canonical_planned_path(&request.receiver_root);
    let workspace = crate::worktree::canonical_planned_path(&request.workspace.root);
    if root.starts_with(&workspace) {
        return Err("review receivers must be outside disposable workspaces".into());
    }
    std::fs::create_dir_all(root.join("operations")).map_err(|error| error.to_string())?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("operations").join("opening.lock"))
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(format!("busy: review opening lock: {error}")),
        }
    }
}

pub(super) fn path(root: &Path, task_id: &str, kind: &str) -> PathBuf {
    let identity = format!("{:x}", Sha256::digest(task_id.as_bytes()));
    root.join("operations")
        .join(format!("{identity}-{kind}.json"))
}

pub(super) fn read<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| error.to_string())
}

pub(super) fn write<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec(value)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        File::open(path.parent().expect("journal path has parent"))?.sync_all()
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result.map_err(|error: std::io::Error| error.to_string())
}

pub(super) fn dispatch(root: &Path, task_id: &str) -> Result<ReviewerDispatch, String> {
    read(&path(root, task_id, "dispatch"))
        .map(|saved| saved.unwrap_or(ReviewerDispatch::NotRequested))
}
