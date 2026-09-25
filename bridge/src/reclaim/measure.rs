//! What a workspace's activity and Git state measure as, read off the disk.

use super::artifacts::ARTIFACT_DIRS;
use super::budget::{Budget, Unfinished};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Names a walk for activity never enters. `.git` changes whenever anything
/// reads the repository; `.build` is Build's own per-agent configuration,
/// rewritten on every resume; build output changes on every build, which is not
/// somebody working.
const NOT_ACTIVITY: [&str; 2] = [".git", ".build"];

/// What the Git directories of one workspace say, summed across them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RepositoryMeasure {
    pub dirty_files: u64,
    pub unpushed_commits: u64,
    pub behind_commits: u64,
    pub newest_commit_ms: Option<i64>,
    /// `dirty`, `unpushed`, or `unknown` when a repository could not be read.
    pub holds: Vec<&'static str>,
    /// The budget ran out before every repository was read.
    pub unfinished: bool,
}

/// Measure every Git directory, with the same test Done uses for "the work is
/// somewhere else": [`crate::gitgui::work_summary`]. One repository's reading
/// cannot be interrupted, so the budget is checked between them.
pub fn measure_repositories(repositories: &[PathBuf], budget: &Budget) -> RepositoryMeasure {
    let mut measure = RepositoryMeasure::default();
    for repository in repositories {
        if budget.check().is_err() {
            measure.unfinished = true;
            break;
        }
        match crate::gitgui::work_summary(repository) {
            Ok(summary) => {
                measure.unpushed_commits += summary.pushes;
                measure.behind_commits += summary.behind;
                measure.dirty_files += dirty_file_count(repository);
                push_unique(
                    &mut measure.holds,
                    crate::workspace::summary_finish_blockers(&summary),
                );
            }
            Err(_) => push_unique(
                &mut measure.holds,
                vec![crate::workspace::FINISH_BLOCKER_UNKNOWN],
            ),
        }
        measure.newest_commit_ms = measure.newest_commit_ms.max(head_commit_ms(repository));
    }
    measure
}

fn push_unique(holds: &mut Vec<&'static str>, more: Vec<&'static str>) {
    for hold in more {
        if !holds.contains(&hold) {
            holds.push(hold);
        }
    }
}

/// How many paths `git status` would list: edits, staged or not, and untracked
/// files. Ignored files are not work.
fn dirty_file_count(repository: &Path) -> u64 {
    let Ok(repo) = git2::Repository::open(repository) else {
        return 0;
    };
    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);
    repo.statuses(Some(&mut options))
        .map(|statuses| statuses.len() as u64)
        .unwrap_or(0)
}

fn head_commit_ms(repository: &Path) -> Option<i64> {
    let repo = git2::Repository::open(repository).ok()?;
    let commit = repo.head().ok()?.peel_to_commit().ok()?;
    Some(commit.time().seconds().saturating_mul(1000))
}

/// The newest modification time of any file under `root`, in ms. `.git`,
/// Build's `.build`, the manifest and build output are skipped, and symlinks
/// are not followed. `Err` when the budget ran out first: the newest change
/// is then not known, and neither is whether the workspace is idle.
pub fn newest_change_ms(root: &Path, budget: &Budget) -> Result<Option<i64>, Unfinished> {
    let mut newest: Option<SystemTime> = None;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            budget.spend()?;
            if skipped_for_activity(&entry.file_name(), &directory, root) {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                let modified = entry.metadata().and_then(|metadata| metadata.modified());
                newest = newest.max(modified.ok());
            }
        }
    }
    Ok(newest.map(system_ms))
}

fn skipped_for_activity(name: &std::ffi::OsStr, parent: &Path, root: &Path) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    NOT_ACTIVITY.contains(&name)
        || ARTIFACT_DIRS.contains(&name)
        || (parent == root && name == crate::workspace::MANIFEST_FILE)
}

/// When a file was made, falling back to when it was last written on a
/// filesystem that does not keep birth times. The manifest is written when the
/// workspace is made, so this is the workspace's own age.
pub(super) fn created_ms(path: &Path) -> Option<i64> {
    let metadata = std::fs::metadata(path).ok()?;
    metadata
        .created()
        .or_else(|_| metadata.modified())
        .ok()
        .map(system_ms)
}

fn system_ms(at: SystemTime) -> i64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
