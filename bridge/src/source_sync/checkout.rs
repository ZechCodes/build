//! Fast-forwarding the source's own checkout, files and all (#267, #268).
//!
//! The fetch is what can hang on the network, so it is what the short
//! deadlines bound. Once the fetch has landed, the checkout is let finish:
//! git killed part-way through writing the tree leaves `index.lock` behind,
//! which stops every later sync and every commit of the user's, and some of
//! the incoming files written beside an index that never moved. A filter can
//! make an honest checkout slow (git-lfs's smudge downloading a large file),
//! so the checkout's own cap is minutes, not seconds.
//!
//! A workspace cut never runs the checkout. It asks
//! [`refuse_what_would_stop`] instead, which reads what the checkout would
//! refuse without writing anything, so the cut can branch from the commit
//! the checkout is moving to (#271).
//!
//! Should even that cap pass, git is killed and what it left is cleaned up
//! as far as it is Build's: the index lock, when it was taken after this
//! checkout started. HEAD and the index never moved; files already written
//! are left as untracked files, which the next sync names rather than
//! overwrites.

use super::git::sync_git_within;
use super::reason::first_lines;
use super::{failed, without_credentials, Step, SyncOutcome};
use crate::git_process::git_failure;
use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How long the checkout's fast-forward may run once started.
pub(super) const CHECKOUT_CAP: Duration = Duration::from_secs(10 * 60);

/// How far a lock's mtime may read before the checkout started and still be
/// the checkout's: file times are coarser than the clock.
const MTIME_SLACK: Duration = Duration::from_secs(1);

/// Move the base the checkout at `path` stands on to `tracking`, when
/// nothing in it could be lost. Untracked files do not hold it back, and
/// nor do ignored ones: git refuses to overwrite either (an ignored `.env`
/// upstream starts tracking is still the user's), and says which.
pub(super) fn fast_forward_checkout(
    repo: &git2::Repository,
    path: &Path,
    tracking: &str,
    cap: Duration,
) -> Step<()> {
    refuse_uncommitted(repo)?;
    let merge = [
        "merge",
        "--ff-only",
        "--no-overwrite-ignore",
        "--quiet",
        tracking,
    ];
    let started = SystemTime::now();
    match sync_git_within(path, &merge, cap) {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let args: Vec<&OsStr> = merge.iter().map(OsStr::new).collect();
            Err(SyncOutcome::Skipped(format!(
                "Git would not fast-forward the base checkout: {}",
                first_lines(&without_credentials(
                    &git_failure(&args, &output).to_string()
                ))
            )))
        }
        Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
            remove_lock_taken_since(&repo.path().join("index.lock"), started);
            Err(failed(format!(
                "Build stopped fast-forwarding the base checkout after {} s, before it finished. The branch did not move; any files it had written are left untracked.",
                cap.as_secs()
            )))
        }
        Err(error) => Err(failed(format!("Build could not run git: {error}"))),
    }
}

/// What would stop [`fast_forward_checkout`] moving the checkout from `old`
/// to `new`, found without running it: uncommitted changes, or a file of
/// the user's, untracked or ignored, where `new` adds one or needs a
/// directory. Nothing is written, so it takes no lock.
///
/// It reads what git refuses on, not everything git could fail on: a
/// checkout that still fails (a filter that errors) leaves the base behind
/// and says so on the source's row, while the cut that asked has already
/// branched from `new`.
pub(super) fn refuse_what_would_stop(
    repo: &git2::Repository,
    old: git2::Oid,
    new: git2::Oid,
) -> Step<()> {
    refuse_uncommitted(repo)?;
    let in_the_way = files_in_the_way(repo, old, new).map_err(|error| {
        failed(format!(
            "Build cannot compare the base checkout with its remote: {error}"
        ))
    })?;
    if in_the_way.is_empty() {
        return Ok(());
    }
    let shown: Vec<String> = in_the_way
        .iter()
        .take(NAMED_AT_MOST)
        .map(|path| path.display().to_string())
        .collect();
    let more = in_the_way.len().saturating_sub(NAMED_AT_MOST);
    let more = if more > 0 {
        format!(" and {more} more")
    } else {
        String::new()
    };
    Err(SyncOutcome::Skipped(format!(
        "The base checkout has files of its own where its remote now has some, which Git will not overwrite: {}{more}.",
        shown.join(", ")
    )))
}

/// How many files in the way a reason names before it counts the rest.
const NAMED_AT_MOST: usize = 5;

/// The paths in the working tree that `new` would write over and `old` does
/// not track: a path `new` adds that is already there, and anything that is
/// not a directory where `new` needs one. A tracked path whose type `new`
/// changes (a file into a symlink, a file into a directory) is the
/// checkout's own to replace; a tracked directory `new` turns into a file is
/// too, unless it holds something `old` does not track, which git keeps.
fn files_in_the_way(
    repo: &git2::Repository,
    old: git2::Oid,
    new: git2::Oid,
) -> Result<Vec<PathBuf>, git2::Error> {
    let Some(workdir) = repo.workdir() else {
        return Ok(Vec::new());
    };
    let old_tree = repo.find_commit(old)?.tree()?;
    let new_tree = repo.find_commit(new)?.tree()?;
    let diff = repo.diff_tree_to_tree(Some(&old_tree), Some(&new_tree), None)?;
    let mut added = Vec::new();
    let mut removed = HashSet::new();
    for delta in diff.deltas() {
        match delta.status() {
            git2::Delta::Added => added.extend(delta.new_file().path().map(Path::to_path_buf)),
            git2::Delta::Deleted => removed.extend(delta.old_file().path().map(Path::to_path_buf)),
            _ => {}
        }
    }
    let mut in_the_way: Vec<PathBuf> = added
        .iter()
        .filter_map(|path| in_the_way_of(workdir, path, &removed))
        .collect();
    in_the_way.sort();
    in_the_way.dedup();
    Ok(in_the_way)
}

/// What in the working tree stops the checkout writing `path`, which `new`
/// adds, given the tracked paths `new` removes: a file of the user's above
/// it where it needs a directory, or at it.
fn in_the_way_of(workdir: &Path, path: &Path, removed: &HashSet<PathBuf>) -> Option<PathBuf> {
    let above = path.ancestors().skip(1).find(|ancestor| {
        !ancestor.as_os_str().is_empty()
            && !removed.contains(*ancestor)
            && std::fs::symlink_metadata(workdir.join(ancestor)).is_ok_and(|meta| !meta.is_dir())
    });
    if let Some(ancestor) = above {
        return Some(ancestor.to_path_buf());
    }
    let blocked = match std::fs::symlink_metadata(workdir.join(path)) {
        Ok(meta) if meta.is_dir() => holds_untracked(workdir, path, removed),
        Ok(_) => !removed.contains(path),
        Err(_) => false,
    };
    blocked.then(|| path.to_path_buf())
}

/// Whether the directory `dir` holds anything but the tracked files `new`
/// removes, however deep. One that cannot be read counts as holding
/// something.
fn holds_untracked(workdir: &Path, dir: &Path, removed: &HashSet<PathBuf>) -> bool {
    let Ok(entries) = std::fs::read_dir(workdir.join(dir)) else {
        return true;
    };
    entries.into_iter().any(|entry| {
        let Ok(entry) = entry else {
            return true;
        };
        let path = dir.join(entry.file_name());
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => holds_untracked(workdir, &path, removed),
            Ok(_) => !removed.contains(&path),
            Err(_) => true,
        }
    })
}

/// Remove the index lock at `lock` if it was taken after `started`, by the
/// git this checkout ran and has killed. One that was there before is
/// someone else's and stays.
///
/// The one lock this could take that is not Build's: the user's own git
/// taking it in the moment between the kill and the removal. Our git held it
/// until the kill, and a lock names no process, so that window is accepted
/// rather than closed.
fn remove_lock_taken_since(lock: &Path, started: SystemTime) {
    let taken = std::fs::metadata(lock).and_then(|meta| meta.modified());
    let since = started.checked_sub(MTIME_SLACK).unwrap_or(started);
    if taken.is_ok_and(|taken| taken >= since) {
        if let Err(error) = std::fs::remove_file(lock) {
            eprintln!("source sync: could not remove {}: {error}", lock.display());
        }
    }
}

fn refuse_uncommitted(repo: &git2::Repository) -> Step<()> {
    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(false)
        .include_ignored(false)
        .exclude_submodules(true);
    let statuses = repo.statuses(Some(&mut options)).map_err(|error| {
        failed(format!(
            "Build cannot read the base checkout's status: {error}"
        ))
    })?;
    if statuses.is_empty() {
        Ok(())
    } else {
        Err(SyncOutcome::Skipped(
            "The base checkout has uncommitted changes.".to_string(),
        ))
    }
}
