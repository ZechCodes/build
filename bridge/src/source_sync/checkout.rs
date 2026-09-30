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
//! Should even that cap pass, git is killed and what it left is cleaned up
//! as far as it is Build's: the index lock, when it was taken after this
//! checkout started. HEAD and the index never moved; files already written
//! are left as untracked files, which the next sync names rather than
//! overwrites.

use super::git::sync_git_within;
use super::reason::first_lines;
use super::{failed, without_credentials, Step, SyncOutcome};
use crate::git_process::git_failure;
use std::ffi::OsStr;
use std::path::Path;
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
