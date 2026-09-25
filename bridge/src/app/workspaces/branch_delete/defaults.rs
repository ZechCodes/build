//! Whether a branch is a default branch, which Done never deletes: `main`,
//! `master`, a base the project was configured with, or the branch a remote's
//! `HEAD` names.
//!
//! What this repository remembers of a remote's `HEAD`
//! (`refs/remotes/<remote>/HEAD`) can be missing or stale, so it only ever
//! adds protection. The remote itself is asked once the checkout is gone,
//! off the app's lock: a remote that cannot be asked leaves the branch, since
//! nothing here can then say it is not that remote's default.

use crate::git_process::{run_command_with_deadline, run_git};
use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

/// Branch names Done never deletes, whatever the repository calls its default.
const PROTECTED_NAMES: [&str; 2] = ["main", "master"];

/// How long a remote has to say which branch its `HEAD` names.
const REMOTE_DEADLINE: Duration = Duration::from_secs(10);

const IS_DEFAULT: &str = "it is a default branch";
const UNCONFIRMED: &str = "Build could not confirm it is not the remote's default branch";

/// Whether the remotes are asked, or only what this repository remembers.
#[derive(Clone, Copy)]
pub(super) enum Remotes {
    Remembered,
    Asked,
}

/// Why `branch` in `repo` is a default branch, as the clause after "Build
/// cannot delete the branch X: ", or `None`. `configured` are the bases the
/// project and the directory were cut from.
pub(super) fn default_refusal(
    repo: &Path,
    branch: &str,
    configured: &[String],
    remotes: Remotes,
) -> Option<&'static str> {
    let named = PROTECTED_NAMES.contains(&branch)
        || configured.iter().any(|name| name == branch)
        || remembered_defaults(repo).iter().any(|name| name == branch);
    if named {
        return Some(IS_DEFAULT);
    }
    match remotes {
        Remotes::Remembered => None,
        Remotes::Asked => asked_refusal(repo, branch),
    }
}

/// What each remote answers for its `HEAD`: the refusal when one names
/// `branch`, and the refusal to guess when one cannot be asked.
fn asked_refusal(repo: &Path, branch: &str) -> Option<&'static str> {
    let Ok(remotes) = run_git(repo, &["remote"]) else {
        return Some(UNCONFIRMED);
    };
    remotes
        .lines()
        .find_map(|remote| match asked_default(repo, remote) {
            Ok(Some(name)) if name == branch => Some(IS_DEFAULT),
            Ok(_) => None,
            Err(()) => Some(UNCONFIRMED),
        })
}

/// The branch `remote`'s `HEAD` names, as the remote answers it now: `None`
/// for a remote whose `HEAD` names no branch, `Err` for one that did not
/// answer within [`REMOTE_DEADLINE`].
fn asked_default(repo: &Path, remote: &str) -> Result<Option<String>, ()> {
    if remote.starts_with('-') {
        return Err(());
    }
    let args = ["ls-remote", "--symref", remote, "HEAD"].map(OsStr::new);
    let answered = run_command_with_deadline(OsStr::new("git"), repo, &args, REMOTE_DEADLINE)
        .map_err(|_| ())?;
    if !answered.status.success() {
        return Err(());
    }
    Ok(String::from_utf8_lossy(&answered.stdout)
        .lines()
        .find_map(|line| {
            line.strip_prefix("ref: refs/heads/")?
                .strip_suffix("\tHEAD")
                .map(str::to_string)
        }))
}

/// The branch each remote's `HEAD` pointed at when this repository last
/// looked: `refs/remotes/origin/HEAD -> refs/remotes/origin/trunk` is `trunk`.
fn remembered_defaults(repo: &Path) -> Vec<String> {
    let Ok(listed) = run_git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname) %(symref)",
            "refs/remotes/",
        ],
    ) else {
        return Vec::new();
    };
    listed
        .lines()
        .filter_map(|line| {
            let (name, target) = line.split_once(' ')?;
            let remote = name.strip_suffix("HEAD")?;
            target.strip_prefix(remote).map(str::to_string)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_fixture::{git_in, init_repo_named};
    use std::path::PathBuf;

    /// `repo` whose `origin` has `main` and `trunk`, with origin's `HEAD` on
    /// `trunk` and nothing remembered of it here.
    fn origin_defaulting_to_trunk(parent: &Path) -> PathBuf {
        let repo = init_repo_named(parent, "repo");
        let origin = parent.join("origin.git");
        git_in(
            parent,
            &["init", "--bare", "-b", "main", origin.to_str().unwrap()],
        );
        git_in(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git_in(&repo, &["branch", "trunk"]);
        git_in(&repo, &["push", "-q", "origin", "main", "trunk"]);
        git_in(&origin, &["symbolic-ref", "HEAD", "refs/heads/trunk"]);
        git_in(&repo, &["fetch", "-q", "origin"]);
        let _ = run_git(&repo, &["symbolic-ref", "-d", "refs/remotes/origin/HEAD"]);
        assert!(remembered_defaults(&repo).is_empty());
        repo
    }

    #[test]
    fn a_remote_asked_names_its_default_when_nothing_is_remembered() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = origin_defaulting_to_trunk(tmp.path());
        let configured = ["main".to_string()];
        assert_eq!(
            default_refusal(&repo, "trunk", &configured, Remotes::Remembered),
            None
        );
        assert_eq!(
            default_refusal(&repo, "trunk", &configured, Remotes::Asked),
            Some(IS_DEFAULT)
        );
    }

    #[test]
    fn a_remote_asked_outranks_a_stale_memory_of_its_default() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = origin_defaulting_to_trunk(tmp.path());
        git_in(
            &repo,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        assert_eq!(
            default_refusal(&repo, "trunk", &[], Remotes::Remembered),
            None
        );
        assert_eq!(
            default_refusal(&repo, "trunk", &[], Remotes::Asked),
            Some(IS_DEFAULT)
        );
    }

    #[test]
    fn a_remote_that_cannot_be_asked_leaves_the_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = origin_defaulting_to_trunk(tmp.path());
        git_in(
            &repo,
            &[
                "remote",
                "set-url",
                "origin",
                tmp.path().join("gone.git").to_str().unwrap(),
            ],
        );
        assert_eq!(
            default_refusal(&repo, "feature", &[], Remotes::Asked),
            Some(UNCONFIRMED)
        );
    }

    #[test]
    fn a_branch_no_remote_calls_its_default_is_not_one() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = origin_defaulting_to_trunk(tmp.path());
        assert_eq!(default_refusal(&repo, "feature", &[], Remotes::Asked), None);
    }
}
