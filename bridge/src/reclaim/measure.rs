//! What a workspace's activity and Git state measure as, read off the disk.

use super::artifacts::ARTIFACT_DIRS;
use super::budget::{Budget, Unfinished};
use super::git_probe::{GitProbe, GitReading};
#[cfg(unix)]
use std::os::fd::{AsRawFd, OwnedFd};
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
    /// The budget ran out, or the daemon stopped, before every repository
    /// was read.
    pub unfinished: bool,
}

/// Measure every Git directory, with the same test Done uses for "the work is
/// somewhere else": [`crate::gitgui::work_summary`], read by `probe` in a
/// process of its own so the budget bounds it. The budget is checked before
/// and after every repository, the last one included: a reading that ran
/// over, or was killed, leaves the measure unfinished.
pub fn measure_repositories(
    repositories: &[PathBuf],
    budget: &Budget,
    probe: &GitProbe,
) -> RepositoryMeasure {
    let mut measure = RepositoryMeasure::default();
    for repository in repositories {
        let Ok(reading) = probe.read(repository, budget) else {
            measure.unfinished = true;
            return measure;
        };
        measure.add(&reading);
    }
    measure.unfinished = budget.check().is_err();
    measure
}

/// Read each checkout through a descriptor held below the managed root, so
/// replacing its path cannot redirect the child to another repository.
#[cfg(unix)]
pub fn measure_repositories_pinned(
    repositories: &[(PathBuf, OwnedFd)],
    budget: &Budget,
    probe: &GitProbe,
) -> RepositoryMeasure {
    let mut measure = RepositoryMeasure::default();
    for (_, checkout) in repositories {
        let Ok(reading) = probe.read_pinned(checkout.as_raw_fd(), budget) else {
            measure.unfinished = true;
            return measure;
        };
        measure.add(&reading);
    }
    measure.unfinished = budget.check().is_err();
    measure
}

impl RepositoryMeasure {
    fn add(&mut self, reading: &GitReading) {
        use crate::workspace::{
            FINISH_BLOCKER_DIRTY, FINISH_BLOCKER_UNKNOWN, FINISH_BLOCKER_UNPUSHED,
        };
        if !reading.read {
            push_unique(&mut self.holds, vec![FINISH_BLOCKER_UNKNOWN]);
            return;
        }
        self.unpushed_commits += reading.pushes;
        self.behind_commits += reading.behind;
        self.dirty_files += reading.dirty_files;
        self.newest_commit_ms = self.newest_commit_ms.max(reading.newest_commit_ms);
        let mut found = Vec::new();
        if reading.dirty {
            found.push(FINISH_BLOCKER_DIRTY);
        }
        if reading.pushes > 0 {
            found.push(FINISH_BLOCKER_UNPUSHED);
        }
        push_unique(&mut self.holds, found);
    }
}

fn push_unique(holds: &mut Vec<&'static str>, more: Vec<&'static str>) {
    for hold in more {
        if !holds.contains(&hold) {
            holds.push(hold);
        }
    }
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

fn system_ms(at: SystemTime) -> i64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
