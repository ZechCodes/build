//! Build output: the part of a workspace that a build makes again (tier 1).
//!
//! A directory counts only if all of these hold. It has one of the
//! conventional names. The repository it sits in ignores it and tracks nothing
//! inside it. It is inside the workspace, reached without a symlink. It holds
//! no repository of its own. A `dist/` a project commits, a `target/` it does
//! not ignore, a nested repository's committed `dist/`, or anything reached
//! through a link out of the workspace is somebody's source, and stays.
//!
//! Removing is two steps. Under the app mutex, each directory is renamed into
//! the workspace's own trash (`.build/reclaim`), which is one syscall and
//! takes it out of every tool's way at once: a build that starts afterwards
//! makes a fresh one. The trash is emptied afterwards with the mutex
//! released, on a budget, and whatever is left is emptied by the next sweep.

use super::budget::{Budget, Unfinished};
use std::path::{Path, PathBuf};

/// The directory names build output goes under.
pub const ARTIFACT_DIRS: [&str; 4] = ["node_modules", "target", ".venv", "dist"];

/// How deep below a repository's root to look. `bridge/target` and
/// `spa/node_modules` are one level down; nothing a build tool makes by
/// default sits deeper than this.
const MAX_DEPTH: usize = 4;

/// Where a workspace keeps what it is removing, below its root.
const TRASH: [&str; 2] = [".build", "reclaim"];

/// One build output directory, and what it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    /// Canonical: no component of it is a symlink.
    pub path: PathBuf,
    /// The Git directory whose index and ignore rules made it build output.
    pub repository: PathBuf,
    pub bytes: u64,
}

/// The build output directories inside one Git directory. `repository` must
/// be canonical. The walk stops at every nested repository or submodule,
/// whose files the outer index says nothing about.
pub fn find(repository: &Path, budget: &Budget) -> Result<Vec<PathBuf>, Unfinished> {
    let Ok(repo) = git2::Repository::open(repository) else {
        return Ok(Vec::new());
    };
    let Ok(index) = repo.index() else {
        return Ok(Vec::new());
    };
    let tracked: Vec<PathBuf> = index
        .iter()
        .map(|entry| PathBuf::from(String::from_utf8_lossy(&entry.path).into_owned()))
        .collect();
    let mut found = Vec::new();
    let mut pending = vec![(repository.to_path_buf(), 0usize)];
    while let Some((directory, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            budget.spend()?;
            // Never a symlink: what it points at is not this workspace's.
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) || entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            if holds_a_repository(&path) {
                continue;
            }
            let Ok(relative) = path.strip_prefix(repository) else {
                continue;
            };
            if is_build_output(&repo, relative, &tracked) {
                found.push(path);
            } else if depth + 1 < MAX_DEPTH {
                pending.push((path, depth + 1));
            }
        }
    }
    found.sort();
    Ok(found)
}

/// A nested repository, a submodule or a linked worktree: its own `.git`,
/// file or directory.
fn holds_a_repository(directory: &Path) -> bool {
    std::fs::symlink_metadata(directory.join(".git")).is_ok()
}

/// Whether `artifact` is still build output, asked again of the disk and a
/// freshly read index just before it is moved: still a real directory where
/// it was, still ignored, nothing inside it tracked since (a `git add -f`),
/// and no repository at its top. Cheap enough to ask under the app mutex:
/// one index read and one ignore lookup.
pub fn still_build_output(artifact: &Artifact) -> bool {
    let Ok(relative) = artifact.path.strip_prefix(&artifact.repository) else {
        return false;
    };
    let Ok(repo) = git2::Repository::open(&artifact.repository) else {
        return false;
    };
    let Ok(index) = repo.index() else {
        return false;
    };
    let tracked: Vec<PathBuf> = index
        .iter()
        .map(|entry| PathBuf::from(String::from_utf8_lossy(&entry.path).into_owned()))
        .collect();
    still_in_place(&artifact.path)
        && !holds_a_repository(&artifact.path)
        && is_build_output(&repo, relative, &tracked)
}

fn is_build_output(repo: &git2::Repository, relative: &Path, tracked: &[PathBuf]) -> bool {
    let named = relative
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| ARTIFACT_DIRS.contains(&name));
    named
        && repo.is_path_ignored(relative).unwrap_or(false)
        && !tracked.iter().any(|path| path.starts_with(relative))
}

/// What one candidate of `repository` holds, walked without following a
/// symlink. `None` when a repository is somewhere inside it: whatever that
/// repository has not pushed would go with it.
pub fn inspect(
    repository: &Path,
    directory: &Path,
    budget: &Budget,
) -> Result<Option<Artifact>, Unfinished> {
    let mut bytes = 0;
    let mut pending = vec![directory.to_path_buf()];
    while let Some(path) = pending.pop() {
        budget.spend()?;
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        bytes += allocated(&metadata);
        if !metadata.is_dir() {
            continue;
        }
        if path != directory && holds_a_repository(&path) {
            return Ok(None);
        }
        if let Ok(entries) = std::fs::read_dir(&path) {
            pending.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
    Ok(Some(Artifact {
        path: directory.to_path_buf(),
        repository: repository.to_path_buf(),
        bytes,
    }))
}

/// Whether `path` is still exactly what was inspected: a real directory, and
/// no component of it replaced by a symlink since. Cheap enough to ask under
/// the app mutex.
pub fn still_in_place(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
        && std::fs::canonicalize(path).is_ok_and(|resolved| resolved == path)
}

/// The workspace's trash directory, made if it is missing. `None` when it
/// cannot be made, or when something other than a real directory stands
/// where it goes.
pub fn trash_of(root: &Path) -> Option<PathBuf> {
    let mut trash = root.to_path_buf();
    for part in TRASH {
        trash.push(part);
        match std::fs::symlink_metadata(&trash) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return None,
            Err(_) => std::fs::create_dir(&trash).ok()?,
        }
    }
    still_in_place(&trash).then_some(trash)
}

/// Move each artifact into the trash, one rename each: bounded, so it runs
/// under the app mutex. An artifact that moved, turned into a link or is on
/// another filesystem is left where it is. Answers what was moved.
pub fn move_to_trash(artifacts: &[Artifact], trash: &Path) -> Vec<Artifact> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0);
    artifacts
        .iter()
        .enumerate()
        .filter(|(_, artifact)| still_in_place(&artifact.path))
        .filter_map(|(index, artifact)| {
            let name = artifact.path.file_name()?.to_string_lossy().into_owned();
            let target = trash.join(format!("{stamp}-{index}-{name}"));
            match std::fs::rename(&artifact.path, &target) {
                Ok(()) => Some(artifact.clone()),
                Err(error) => {
                    eprintln!(
                        "workspace reclaim: move {} aside: {error}",
                        artifact.path.display()
                    );
                    None
                }
            }
        })
        .collect()
}

/// Empty a workspace's trash, and say how many bytes went. Stops when the
/// budget does; the next sweep empties the rest.
pub fn empty_trash(root: &Path, budget: &Budget) -> u64 {
    let trash = TRASH
        .iter()
        .fold(root.to_path_buf(), |path, part| path.join(part));
    if !still_in_place(&trash) {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(&trash) else {
        return 0;
    };
    let mut freed = 0;
    for entry in entries.flatten() {
        let (bytes, finished) = remove_tree(&entry.path(), budget);
        freed += bytes;
        if !finished {
            break;
        }
    }
    freed
}

/// Remove a tree bottom-up without following a symlink, spending one entry
/// per path: `(bytes freed, whether it finished)`.
fn remove_tree(root: &Path, budget: &Budget) -> (u64, bool) {
    let mut freed = 0;
    // Directories are visited twice: once to list them, once to remove them
    // after everything they held.
    let mut pending = vec![(root.to_path_buf(), false)];
    while let Some((path, emptied)) = pending.pop() {
        if budget.spend().is_err() {
            return (freed, false);
        }
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_dir() {
            if std::fs::remove_file(&path).is_ok() {
                freed += allocated(&metadata);
            }
        } else if emptied {
            if std::fs::remove_dir(&path).is_ok() {
                freed += allocated(&metadata);
            }
        } else {
            pending.push((path.clone(), true));
            if let Ok(entries) = std::fs::read_dir(&path) {
                pending.extend(entries.flatten().map(|entry| (entry.path(), false)));
            }
        }
    }
    (freed, true)
}

/// The bytes a tree holds on disk: allocated blocks, not file lengths, without
/// following symlinks. Reflinked copies share blocks with their source, so on
/// a copy-on-write filesystem this counts more than removing it would free.
pub fn size_on_disk(root: &Path, budget: &Budget) -> Result<u64, Unfinished> {
    let mut total = 0;
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        budget.spend()?;
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        total += allocated(&metadata);
        if metadata.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&path) {
                pending.extend(entries.flatten().map(|entry| entry.path()));
            }
        }
    }
    Ok(total)
}

#[cfg(unix)]
fn allocated(metadata: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.blocks() * 512
}

#[cfg(not(unix))]
fn allocated(metadata: &std::fs::Metadata) -> u64 {
    metadata.len()
}
