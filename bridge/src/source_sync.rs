//! Keeping a project source's base branch in step with its remote (#267).
//!
//! A workspace is cut from the source's local base branch, so a base that
//! has fallen behind its remote hands every new workspace old code. One sync
//! fetches the base branch alone, then moves it forward only when that is a
//! fast-forward: never a merge, rebase, reset or force. A base that cannot be
//! moved without risking work (commits the remote lacks, uncommitted changes,
//! an operation in progress, the branch checked out in another worktree) is
//! left exactly as it is and the reason is reported.
//!
//! Where the base is checked out in the source's own checkout, git's
//! `merge --ff-only` moves it with its files. Anywhere else (another branch,
//! a detached HEAD) only the ref moves, by a compare-and-swap `update-ref`,
//! and no working tree is touched.
//!
//! The fetch runs unattended ([`run_git_unattended`]): no prompt of any kind,
//! killed at its deadline. The url it uses is the one git resolves, checked
//! by [`usable_remote_url`] before git sees it.

mod reason;
#[cfg(test)]
mod tests;

use crate::git_process::{run_git, run_git_unattended};
use crate::lifecycle::refuse_unusable_branch_name;
use crate::remote_url::usable_remote_url;
use crate::worktree::configured_remote_for_branch;
pub use reason::without_credentials;
use reason::{fetch_failure, first_lines};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a workspace cut waits on a source's fetch before it goes ahead
/// from the base as it stands.
pub const CUT_FETCH_DEADLINE: Duration = Duration::from_secs(10);

/// How long the service's and the Sync now button's fetch may run.
pub const SERVICE_FETCH_DEADLINE: Duration = Duration::from_secs(30);

/// Whether a sync asks the remote first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fetch {
    /// Fetch the base branch, killed at the deadline.
    Within(Duration),
    /// Move the base to what was fetched last, without the network.
    Skip,
}

/// A sync that could not reach or read the remote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// A sentence, with any credentials git echoed taken out.
    pub reason: String,
    /// The remote wanted something only a person can give (a password, a
    /// passphrase, a key touch) or never answered. The service stops asking
    /// until someone syncs the source by hand.
    pub needs_you: bool,
}

/// What one sync did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncOutcome {
    UpToDate,
    FastForwarded {
        commits: usize,
    },
    /// Left as it was, and why.
    Skipped(String),
    Failed(Failure),
    NoRemote,
}

/// One sync's outcome, and where the base stands against its remote after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncReport {
    pub outcome: SyncOutcome,
    /// Commits the base has that the remote does not.
    pub ahead: usize,
    /// Commits the remote has that the base does not.
    pub behind: usize,
    /// The remote answered a fetch in this sync.
    pub fetched: bool,
}

/// Sync the base branch `base_branch` of the checkout at `path` with its
/// remote. Never fails: what went wrong is the outcome.
pub fn sync_base(path: &Path, base_branch: &str, fetch: Fetch) -> SyncReport {
    let mut report = SyncReport {
        outcome: SyncOutcome::UpToDate,
        ahead: 0,
        behind: 0,
        fetched: false,
    };
    report.outcome =
        match BaseSync::prepare(path, base_branch).and_then(|sync| sync.run(fetch, &mut report)) {
            Ok(outcome) | Err(outcome) => outcome,
        };
    report
}

/// A step of a sync: the next thing it needs, or the outcome it ends on.
type Step<T> = Result<T, SyncOutcome>;

fn failed(reason: impl Into<String>) -> SyncOutcome {
    SyncOutcome::Failed(Failure {
        reason: reason.into(),
        needs_you: false,
    })
}

fn open(path: &Path) -> Step<git2::Repository> {
    git2::Repository::open(path)
        .map_err(|error| failed(format!("Build cannot open {}: {error}", path.display())))
}

/// A sync whose branch, remote and url have all been checked.
struct BaseSync<'a> {
    path: &'a Path,
    base: &'a str,
    remote: String,
}

impl<'a> BaseSync<'a> {
    fn prepare(path: &'a Path, base: &'a str) -> Step<Self> {
        refuse_unusable_branch_name(base).map_err(failed)?;
        let repo = open(path)?;
        let remote = configured_remote_for_branch(&repo, base).ok_or(SyncOutcome::NoRemote)?;
        if remote.starts_with('-') || !git2::Remote::is_valid_name(&remote) {
            return Err(failed(format!(
                "{remote} is not a remote name Git accepts."
            )));
        }
        let sync = Self { path, base, remote };
        sync.refuse_unusable_url()?;
        Ok(sync)
    }

    /// The url git will fetch from, rewrites applied, held to the check every
    /// remote a client names is held to.
    fn refuse_unusable_url(&self) -> Step<()> {
        let url = run_git(self.path, &["remote", "get-url", "--", &self.remote])
            .map_err(|error| failed(without_credentials(&error.to_string())))?;
        usable_remote_url(url.trim()).map(drop).map_err(|why| {
            failed(format!(
                "{} is not a location Build will fetch from. {}",
                self.remote,
                without_credentials(&why)
            ))
        })
    }

    fn run(&self, fetch: Fetch, report: &mut SyncReport) -> Step<SyncOutcome> {
        if let Fetch::Within(deadline) = fetch {
            self.fetch(deadline)?;
            report.fetched = true;
        }
        let repo = open(self.path)?;
        let local = self.oid(&repo, &self.local_ref(), "This checkout has no branch")?;
        let upstream = self.oid(
            &repo,
            &self.tracking_ref(),
            &format!("{} has no branch", self.remote),
        )?;
        let (ahead, behind) = repo.graph_ahead_behind(local, upstream).map_err(|error| {
            failed(format!(
                "Build cannot compare {} with {}: {error}",
                self.base, self.remote
            ))
        })?;
        (report.ahead, report.behind) = (ahead, behind);
        if behind == 0 {
            return Ok(SyncOutcome::UpToDate);
        }
        if ahead > 0 {
            let commits = if ahead == 1 { "commit" } else { "commits" };
            return Err(SyncOutcome::Skipped(format!(
                "{} has {ahead} {commits} {} does not.",
                self.base, self.remote
            )));
        }
        self.fast_forward(&repo, local, upstream)?;
        report.behind = 0;
        Ok(SyncOutcome::FastForwarded { commits: behind })
    }

    fn local_ref(&self) -> String {
        format!("refs/heads/{}", self.base)
    }

    fn tracking_ref(&self) -> String {
        format!("refs/remotes/{}/{}", self.remote, self.base)
    }

    fn oid(&self, repo: &git2::Repository, name: &str, missing: &str) -> Step<git2::Oid> {
        repo.refname_to_id(name)
            .map_err(|_| SyncOutcome::Skipped(format!("{missing} {}.", self.base)))
    }

    /// Fetch the base branch alone into its remote-tracking ref. No `+`: a
    /// remote whose branch was rewritten is refused by git, not followed.
    fn fetch(&self, deadline: Duration) -> Step<()> {
        let refspec = format!("{}:{}", self.local_ref(), self.tracking_ref());
        let args = [
            OsStr::new("fetch"),
            OsStr::new("--no-tags"),
            OsStr::new("--no-write-fetch-head"),
            OsStr::new("--"),
            OsStr::new(&self.remote),
            OsStr::new(&refspec),
        ];
        match run_git_unattended(self.path, &args, deadline) {
            Ok(output) if output.status.success() => Ok(()),
            Ok(output) => Err(fetch_failure(
                &String::from_utf8_lossy(&output.stderr),
                &self.remote,
                self.base,
            )),
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {
                Err(SyncOutcome::Failed(Failure {
                    reason: format!(
                        "{} did not answer within {} s.",
                        self.remote,
                        deadline.as_secs_f32().ceil()
                    ),
                    needs_you: true,
                }))
            }
            Err(error) => Err(failed(format!("Build could not run git: {error}"))),
        }
    }

    fn fast_forward(&self, repo: &git2::Repository, old: git2::Oid, new: git2::Oid) -> Step<()> {
        match Placement::of(self.path, &self.local_ref())? {
            Placement::Here => self.fast_forward_checkout(repo),
            Placement::Elsewhere(path) => Err(SyncOutcome::Skipped(format!(
                "{} is checked out in {}.",
                self.base,
                path.display()
            ))),
            Placement::Nowhere => {
                fast_forward_ref(self.path, self.base, &old.to_string(), &new.to_string())
                    .map_err(SyncOutcome::Skipped)
            }
        }
    }

    /// Move the base the source's own checkout stands on, files and all, when
    /// nothing in it could be lost. Untracked files do not hold it back; git
    /// refuses to overwrite one, and says which.
    fn fast_forward_checkout(&self, repo: &git2::Repository) -> Step<()> {
        refuse_unfinished(repo)?;
        refuse_uncommitted(repo)?;
        let tracking = self.tracking_ref();
        run_git(self.path, &["merge", "--ff-only", "--quiet", &tracking])
            .map(drop)
            .map_err(|error| {
                SyncOutcome::Skipped(format!(
                    "Git would not fast-forward the base checkout: {}",
                    first_lines(&without_credentials(&error.to_string()))
                ))
            })
    }
}

fn refuse_unfinished(repo: &git2::Repository) -> Step<()> {
    match repo.state() {
        git2::RepositoryState::Clean => Ok(()),
        state => Err(SyncOutcome::Skipped(format!(
            "A {} is in progress in the base checkout.",
            unfinished_name(state)
        ))),
    }
}

fn unfinished_name(state: git2::RepositoryState) -> &'static str {
    use git2::RepositoryState::*;
    match state {
        Merge => "merge",
        Revert | RevertSequence => "revert",
        CherryPick | CherryPickSequence => "cherry-pick",
        Bisect => "bisect",
        Rebase | RebaseInteractive | RebaseMerge => "rebase",
        _ => "git operation",
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

/// Move `refs/heads/<branch>` from `old` to `new` only if it still reads
/// `old`: git's compare-and-swap. A ref that moved in between keeps whatever
/// it moved to.
fn fast_forward_ref(path: &Path, branch: &str, old: &str, new: &str) -> Result<(), String> {
    let name = format!("refs/heads/{branch}");
    let message = format!("Build: fast-forward {branch} to its remote");
    run_git(path, &["update-ref", "-m", &message, "--", &name, new, old])
        .map(drop)
        .map_err(|_| {
            format!("{branch} moved while Build was fast-forwarding it, so it was left as it is.")
        })
}

/// Where the base branch is checked out, if anywhere.
enum Placement {
    /// In the checkout being synced.
    Here,
    /// In another worktree of the same repository.
    Elsewhere(PathBuf),
    Nowhere,
}

impl Placement {
    fn of(path: &Path, local_ref: &str) -> Step<Self> {
        let listed = run_git(path, &["worktree", "list", "--porcelain"])
            .map_err(|error| failed(format!("Build cannot list the worktrees: {error}")))?;
        let Some(holder) = worktree_holding(&listed, local_ref) else {
            return Ok(Self::Nowhere);
        };
        if same_directory(&holder, path) {
            Ok(Self::Here)
        } else {
            Ok(Self::Elsewhere(holder))
        }
    }
}

/// The worktree `git worktree list --porcelain` says has `local_ref` checked
/// out.
fn worktree_holding(listed: &str, local_ref: &str) -> Option<PathBuf> {
    listed.split("\n\n").find_map(|entry| {
        let mut path = None;
        let mut holds = false;
        for line in entry.lines() {
            if let Some(worktree) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(worktree));
            }
            holds |= line.strip_prefix("branch ") == Some(local_ref);
        }
        path.filter(|_| holds)
    })
}

fn same_directory(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}
