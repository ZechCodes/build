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
//! `merge --ff-only --no-overwrite-ignore` moves it with its files. Anywhere
//! else (another branch, a detached HEAD) only the ref moves, by a
//! compare-and-swap `update-ref`, and no working tree is touched. Neither
//! happens while a rebase or bisect of the base, or a merge, cherry-pick or
//! revert on it, is part-way through in any worktree ([`in_progress`]). The
//! checkout, once started, is let finish ([`checkout`]).
//!
//! Every git it starts runs as nobody's command ([`git`]): no prompt of any
//! kind, no hooks, no maintenance, killed at its deadline. The url the fetch
//! uses is the one git resolves, checked by [`usable_remote_url`] before git
//! sees it, and the branch fetched is the one the base follows
//! ([`upstream`]).

mod checkout;
mod git;
mod in_progress;
mod reason;
#[cfg(test)]
mod tests;
mod upstream;

use crate::git_process::said_before_deadline;
use crate::lifecycle::refuse_unusable_branch_name;
use crate::remote_url::usable_remote_url;
use git::{sync_git, sync_git_within};
pub use reason::without_credentials;
use reason::{fetch_failure, fetch_timeout};
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
    /// The remote wanted something only a person can give, and git or ssh
    /// said so: a password, or a key whose passphrase nobody can type. The
    /// service stops asking until someone syncs the source by hand. A
    /// security key waiting for a touch says nothing Build can read (ssh
    /// shows that notice only on a terminal), so it reads as a timeout.
    pub needs_you: bool,
    /// The fetch did not finish within its deadline. Not a person's problem
    /// by itself: the service tries again.
    pub timed_out: bool,
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
    sync_base_capped(path, base_branch, fetch, checkout::CHECKOUT_CAP)
}

/// [`sync_base`], with the checkout's own cap named.
fn sync_base_capped(
    path: &Path,
    base_branch: &str,
    fetch: Fetch,
    checkout_cap: Duration,
) -> SyncReport {
    let mut report = SyncReport {
        outcome: SyncOutcome::UpToDate,
        ahead: 0,
        behind: 0,
        fetched: false,
    };
    report.outcome = match BaseSync::prepare(path, base_branch, checkout_cap)
        .and_then(|sync| sync.run(fetch, &mut report))
    {
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
        timed_out: false,
    })
}

fn open(path: &Path) -> Step<git2::Repository> {
    git2::Repository::open(path)
        .map_err(|error| failed(format!("Build cannot open {}: {error}", path.display())))
}

/// A sync whose branch, remote, remote branch and url have all been
/// checked.
struct BaseSync<'a> {
    path: &'a Path,
    base: &'a str,
    remote: String,
    /// The branch on `remote` the base follows.
    followed: String,
    /// How long the source checkout's fast-forward may run.
    checkout_cap: Duration,
}

impl<'a> BaseSync<'a> {
    fn prepare(path: &'a Path, base: &'a str, checkout_cap: Duration) -> Step<Self> {
        refuse_unusable_branch_name(base).map_err(failed)?;
        let repo = open(path)?;
        let upstream = upstream::upstream_of(&repo, base).ok_or(SyncOutcome::NoRemote)?;
        let remote = upstream.remote;
        if remote.starts_with('-') || !git2::Remote::is_valid_name(&remote) {
            return Err(failed(format!(
                "{remote} is not a remote name Git accepts."
            )));
        }
        refuse_unusable_branch_name(&upstream.branch).map_err(failed)?;
        let sync = Self {
            path,
            base,
            remote,
            followed: upstream.branch,
            checkout_cap,
        };
        sync.refuse_unusable_url()?;
        Ok(sync)
    }

    /// The url git will fetch from, rewrites applied, held to the check every
    /// remote a client names is held to.
    fn refuse_unusable_url(&self) -> Step<()> {
        let url = sync_git(self.path, &["remote", "get-url", "--", &self.remote])
            .map_err(|error| failed(without_credentials(&error)))?;
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
        let local = oid(
            &repo,
            &self.local_ref(),
            format!("This checkout has no branch {}.", self.base),
        )?;
        let upstream = oid(&repo, &self.tracking_ref(), self.no_followed_branch())?;
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

    /// Where the fetch keeps the followed branch: the remote-tracking ref
    /// `git fetch` would write for it.
    fn tracking_ref(&self) -> String {
        format!("refs/remotes/{}/{}", self.remote, self.followed)
    }

    fn no_followed_branch(&self) -> String {
        format!("{} has no branch {}.", self.remote, self.followed)
    }

    /// Fetch the followed branch alone into its remote-tracking ref, and
    /// nothing else: no tags, no submodules. No `+`: a remote whose branch
    /// was rewritten is refused by git, not followed.
    fn fetch(&self, deadline: Duration) -> Step<()> {
        let refspec = format!("refs/heads/{}:{}", self.followed, self.tracking_ref());
        let args = [
            "fetch",
            "--no-tags",
            "--no-recurse-submodules",
            "--no-write-fetch-head",
            "--",
            &self.remote,
            &refspec,
        ];
        match sync_git_within(self.path, &args, deadline) {
            Ok(output) if output.status.success() => Ok(()),
            Ok(output) => Err(fetch_failure(
                &String::from_utf8_lossy(&output.stderr),
                &self.remote,
                self.no_followed_branch(),
            )),
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => Err(fetch_timeout(
                &String::from_utf8_lossy(said_before_deadline(&error).unwrap_or_default()),
                &self.remote,
                deadline,
            )),
            Err(error) => Err(failed(format!("Build could not run git: {error}"))),
        }
    }

    fn fast_forward(&self, repo: &git2::Repository, old: git2::Oid, new: git2::Oid) -> Step<()> {
        if let Some(unfinished) = in_progress::unfinished_on(repo, self.base) {
            return Err(SyncOutcome::Skipped(unfinished));
        }
        match Placement::of(self.path, &self.local_ref())? {
            Placement::Here => checkout::fast_forward_checkout(
                repo,
                self.path,
                &self.tracking_ref(),
                self.checkout_cap,
            ),
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
}

fn oid(repo: &git2::Repository, name: &str, missing: String) -> Step<git2::Oid> {
    repo.refname_to_id(name)
        .map_err(|_| SyncOutcome::Skipped(missing))
}

/// Move `refs/heads/<branch>` from `old` to `new` only if it still reads
/// `old`: git's compare-and-swap. A ref that moved in between keeps whatever
/// it moved to.
fn fast_forward_ref(path: &Path, branch: &str, old: &str, new: &str) -> Result<(), String> {
    let name = format!("refs/heads/{branch}");
    let message = format!("Build: fast-forward {branch} to its remote");
    sync_git(path, &["update-ref", "-m", &message, "--", &name, new, old])
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
        let listed = sync_git(path, &["worktree", "list", "--porcelain"])
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

/// One sync at a time per checkout: the service, a cut and the Sync now
/// button never fetch into the same repository together.
pub struct SyncLock {
    path: PathBuf,
}

fn held_checkouts() -> &'static (
    std::sync::Mutex<std::collections::HashSet<PathBuf>>,
    std::sync::Condvar,
) {
    static HELD: std::sync::OnceLock<(
        std::sync::Mutex<std::collections::HashSet<PathBuf>>,
        std::sync::Condvar,
    )> = std::sync::OnceLock::new();
    HELD.get_or_init(Default::default)
}

impl SyncLock {
    /// The checkout at `path`, once no other sync holds it, waiting at most
    /// `within`. The flag says whether another sync had it first, so a cut
    /// that waited can take what that sync fetched instead of fetching again.
    pub fn acquire(path: &Path, within: Duration) -> Option<(Self, bool)> {
        let (held, released) = held_checkouts();
        let expiry = std::time::Instant::now() + within;
        let mut guard = held.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut waited = false;
        while guard.contains(path) {
            waited = true;
            let left = expiry.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return None;
            }
            guard = released
                .wait_timeout(guard, left)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
        guard.insert(path.to_path_buf());
        Some((
            Self {
                path: path.to_path_buf(),
            },
            waited,
        ))
    }
}

impl Drop for SyncLock {
    fn drop(&mut self) {
        let (held, released) = held_checkouts();
        held.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&self.path);
        released.notify_all();
    }
}
