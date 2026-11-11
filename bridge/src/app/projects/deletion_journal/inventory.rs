//! Remaining captured files may disappear during cleanup. New or changed files
//! prove the old plan is stale. Symlinks are hashed, never followed.
use crate::store::ProjectDeletionPlan;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn capture_inventory(
    roots: &[PathBuf],
    registry: &Path,
) -> Result<BTreeMap<PathBuf, String>, String> {
    let mut files = BTreeMap::new();
    for root in roots
        .iter()
        .map(PathBuf::as_path)
        .chain(std::iter::once(registry))
    {
        visit(root, &mut files)?;
    }
    Ok(files)
}
pub(super) fn capture_plan_inventory(
    plan: &ProjectDeletionPlan,
) -> Result<BTreeMap<PathBuf, String>, String> {
    let paths = plan
        .roots
        .iter()
        .chain(&plan.lock_markers)
        .cloned()
        .collect::<Vec<_>>();
    capture_inventory(&paths, &plan.rift_root)
}
fn visit(path: &Path, files: &mut BTreeMap<PathBuf, String>) -> Result<(), String> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("inspect deletion path {}: {e}", path.display())),
    };
    let proof = if meta.is_symlink() {
        format!(
            "link:{:?}",
            std::fs::read_link(path).map_err(|e| e.to_string())?
        )
    } else if meta.is_dir() {
        format!("directory:{}", identity(&meta))
    } else {
        format!("file:{}:{:x}", identity(&meta), file_digest(path)?)
    };
    files.insert(path.into(), proof);
    if meta.is_dir() {
        for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
            visit(&entry.map_err(|e| e.to_string())?.path(), files)?
        }
    }
    Ok(())
}
pub(super) fn inventory_unchanged(plan: &ProjectDeletionPlan) -> Result<bool, String> {
    for root in &plan.roots {
        if crate::worktree::canonical_planned_path(root) != *root {
            return Ok(false);
        }
    }
    for marker in &plan.lock_markers {
        let parent = marker.parent().ok_or("invalid workspace marker parent")?;
        if crate::worktree::canonical_planned_path(parent) != parent {
            return Ok(false);
        }
    }
    let current = capture_plan_inventory(plan)?;
    Ok(current
        .iter()
        .all(|(path, proof)| plan.inventory.get(path) == Some(proof)))
}

#[cfg(unix)]
fn identity(meta: &std::fs::Metadata) -> String {
    use std::os::unix::fs::MetadataExt;
    format!("{}:{}", meta.dev(), meta.ino())
}
#[cfg(not(unix))]
fn identity(meta: &std::fs::Metadata) -> String {
    format!("{:?}", meta.created().ok())
}

fn file_digest(path: &Path) -> Result<sha2::digest::Output<Sha256>, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash.finalize())
}
