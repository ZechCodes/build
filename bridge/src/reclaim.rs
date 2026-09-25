//! Workspace reclaim (#135): the measuring half of the workspace lifecycle.
//!
//! Workspaces outlive the conversations that made them, and each one carries
//! gigabytes of build output. The bridge does not decide what becomes of one
//! (Zech, 24 Sep 2026): once a workspace has gone a day without activity, it
//! tells the project agent, which merges it, deletes it, or puts it in front of
//! the user. The bridge measures every workspace without asking anybody. When
//! `BRIDGE_WORKSPACE_PRUNE` is on and nothing holds a quiet workspace, it also
//! drops that workspace's build output and keeps the source (tier 1). That is
//! off unless the switch is set. Removing a workspace (tier 2) is only ever
//! the explicit `workspace.reclaim`.
//!
//! This module has no app state. It holds what counts as activity, what counts
//! as build output, and when a notice is due. The service that runs it and
//! writes its verdict is `app/workspaces/reclaim.rs`.

pub mod artifacts;
mod budget;
mod git_probe;
mod measure;

pub use budget::{Budget, Unfinished};
pub use git_probe::{reading_line as git_reading_line, GitProbe};
pub use measure::{measure_repositories, newest_change_ms, RepositoryMeasure};

use artifacts::Artifact;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

/// How long a workspace goes without an agent turn, a commit or a file change
/// before the project agent hears about it. A user only looking at it is not
/// activity (Zech, #135).
pub const DEFAULT_IDLE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
/// How often the service measures every workspace.
pub const DEFAULT_SWEEP_EVERY: Duration = Duration::from_secs(60 * 60);
/// How long after startup the first sweep waits, so it does not compete with
/// resuming the agents a restart cut off.
pub const DEFAULT_FIRST_SWEEP_AFTER: Duration = Duration::from_secs(120);
/// How many directory entries measuring one workspace may visit, every walk
/// together. A large monorepo checkout with its build output is well under a
/// million.
pub const DEFAULT_MEASURE_ENTRIES: u64 = 2_000_000;
/// How long measuring one workspace may take.
pub const DEFAULT_MEASURE_TIME: Duration = Duration::from_secs(120);
/// How long the last look at a workspace's Git state may take, just before
/// its build output is moved. It is taken under the app mutex, so it is
/// short: a workspace too slow to read in it keeps its build output.
pub const DEFAULT_FINAL_CHECK_TIME: Duration = Duration::from_secs(5);

/// How every refusal of `workspace.reclaim` begins.
pub const REFUSAL: &str = "Build cannot reclaim ";
/// What a removal is told while other files are being moved.
pub const BUSY: &str = "another filesystem operation is still running";
/// What anything that would write inside a reserved workspace is told.
pub const RESERVED: &str = "Build is measuring this workspace. Try again in a moment.";

/// A linked issue that is neither Done nor closed.
pub const HOLD_ISSUE_OPEN: &str = "issue_open";
/// The linked issues could not be read, so none of them can be called Done.
pub const HOLD_ISSUES_UNREAD: &str = "issues_unread";
/// The workspace is still provisioning, or failed to.
pub const HOLD_NOT_READY: &str = "not_ready";
/// One of the user's terminals is open somewhere in the workspace.
pub const HOLD_TERMINAL_OPEN: &str = "terminal_open";
/// Measuring ran out of budget, so what it would have found is not known.
pub const HOLD_UNMEASURED: &str = "unmeasured";

/// When the service runs, what it calls idle, how much one measurement may
/// cost, how Git is read, and whether it may drop build output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReclaimPolicy {
    pub idle_after: Duration,
    pub sweep_every: Duration,
    pub first_sweep_after: Duration,
    pub measure_entries: u64,
    pub measure_time: Duration,
    pub final_check_time: Duration,
    /// How each repository's Git state is read, bounded by the budget.
    pub git: GitProbe,
    /// Tier 1: drop a quiet, unheld workspace's build output. Off unless
    /// `BRIDGE_WORKSPACE_PRUNE` turns it on.
    pub prune: bool,
}

impl Default for ReclaimPolicy {
    fn default() -> Self {
        Self {
            idle_after: DEFAULT_IDLE_AFTER,
            sweep_every: DEFAULT_SWEEP_EVERY,
            first_sweep_after: DEFAULT_FIRST_SWEEP_AFTER,
            measure_entries: DEFAULT_MEASURE_ENTRIES,
            measure_time: DEFAULT_MEASURE_TIME,
            final_check_time: DEFAULT_FINAL_CHECK_TIME,
            git: GitProbe::bridge(),
            prune: false,
        }
    }
}

impl ReclaimPolicy {
    /// The defaults, with `BRIDGE_WORKSPACE_IDLE_SECS`,
    /// `BRIDGE_WORKSPACE_SWEEP_SECS` and `BRIDGE_WORKSPACE_PRUNE` read over
    /// them.
    pub fn from_env() -> Self {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    fn from_vars(var: impl Fn(&str) -> Option<String>) -> Self {
        let seconds = |name: &str| {
            var(name)
                .and_then(|raw| raw.trim().parse::<u64>().ok())
                .map(Duration::from_secs)
        };
        let defaults = Self::default();
        Self {
            idle_after: seconds("BRIDGE_WORKSPACE_IDLE_SECS").unwrap_or(defaults.idle_after),
            sweep_every: seconds("BRIDGE_WORKSPACE_SWEEP_SECS").unwrap_or(defaults.sweep_every),
            prune: var("BRIDGE_WORKSPACE_PRUNE").is_some_and(|raw| switched_on(&raw)),
            ..defaults
        }
    }

    fn idle_after_ms(&self) -> i64 {
        i64::try_from(self.idle_after.as_millis()).unwrap_or(i64::MAX)
    }

    /// A fresh budget for measuring one workspace.
    pub fn budget(&self, stop: Arc<AtomicBool>) -> Budget {
        Budget::new(self.measure_entries, self.measure_time, stop)
    }

    /// A fresh budget for the last look before build output is moved.
    pub fn final_check_budget(&self, stop: Arc<AtomicBool>) -> Budget {
        Budget::new(self.measure_entries, self.final_check_time, stop)
    }
}

fn switched_on(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

/// One issue that links a workspace, as the verdict names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkedIssue {
    pub issue_id: String,
    pub number: u64,
    pub title: String,
    pub status: String,
    /// `open` or `closed`.
    pub state: String,
}

impl LinkedIssue {
    /// Done or closed: nobody is still working toward it.
    pub fn finished(&self) -> bool {
        self.status == crate::tracker::DONE_STATUS || self.state == "closed"
    }
}

/// What the service last concluded about one workspace. It is kept between
/// sweeps and across restarts, and each workspace row carries it as
/// `lifecycle`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleRecord {
    pub measured_at_ms: i64,
    /// The newest of: a message in the workspace's conversation, a commit on
    /// one of its branches, a file changed in its tree, the workspace being
    /// made. `None` when none of them could be read.
    pub last_activity_ms: Option<i64>,
    /// No activity for the idle threshold.
    pub idle: bool,
    /// `workspace.reclaim` would take it: nothing in `holds`.
    pub reclaimable: bool,
    /// What keeps it from being reclaimed, in reading order.
    pub holds: Vec<String>,
    pub issues: Vec<LinkedIssue>,
    pub dirty_files: u64,
    pub unpushed_commits: u64,
    pub behind_commits: u64,
    /// Allocated bytes under the root. Measured when a quiet workspace is
    /// first seen, announced or pruned.
    pub size_bytes: Option<u64>,
    /// Build output dropped since the workspace last went idle.
    pub pruned_bytes: u64,
    pub pruned_at_ms: Option<i64>,
    /// When the project agent was last told about this workspace.
    pub noticed_at_ms: Option<i64>,
}

impl LifecycleRecord {
    /// Whether the project agent should hear about this workspace now.
    ///
    /// An idle workspace is announced once when it goes quiet. The notice is
    /// repeated once per idle period while it stays quiet. Activity after the
    /// last notice starts over, so going quiet again is news straight away.
    pub fn notice_due(&self, now_ms: i64, policy: &ReclaimPolicy) -> NoticeDue {
        if !self.idle {
            return NoticeDue::No;
        }
        match self.noticed_at_ms {
            Some(noticed) if self.last_activity_ms.is_some_and(|at| at <= noticed) => {
                if now_ms.saturating_sub(noticed) >= policy.idle_after_ms() {
                    NoticeDue::Again
                } else {
                    NoticeDue::No
                }
            }
            _ => NoticeDue::First,
        }
    }
}

impl LifecycleRecord {
    /// What the budget did not cover is not known: held as `unmeasured`,
    /// and neither idle nor reclaimable.
    pub fn unmeasured(&mut self) {
        self.idle = false;
        self.hold(&[HOLD_UNMEASURED]);
    }

    /// Add holds found after the measurement, which makes the workspace not
    /// reclaimable.
    pub fn hold(&mut self, more: &[&str]) {
        for hold in more {
            if !self.holds.iter().any(|held| held == hold) {
                self.holds.push((*hold).to_string());
            }
        }
        self.holds.sort_by_key(|hold| hold_order(hold));
        self.reclaimable = self.holds.is_empty();
    }
}

/// See [`LifecycleRecord::notice_due`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeDue {
    No,
    /// The workspace just went quiet. The linked issues' timelines record it.
    First,
    /// Still quiet a whole idle period after the last notice.
    Again,
}

/// What the service reads of one workspace under the app mutex, so the slow
/// part can run without it.
#[derive(Clone, Debug)]
pub struct Subject {
    pub workspace_id: String,
    pub project_id: String,
    pub name: String,
    pub root: PathBuf,
    /// `(path, branch)` of each Git directory.
    pub repositories: Vec<(PathBuf, Option<String>)>,
    /// Holds read under the mutex: an agent working, a linked issue still
    /// open, the workspace not ready or not Build's.
    pub holds: Vec<&'static str>,
    pub issues: Vec<LinkedIssue>,
    /// The newest message in the workspace's conversation.
    pub conversation_activity_ms: Option<i64>,
    pub previous: Option<LifecycleRecord>,
}

impl Subject {
    /// Measure everything the mutex was not needed for. Nothing is removed
    /// here: dropping build output is [`Subject::build_output`] and the
    /// service's reservation around it.
    ///
    /// Any part the budget did not cover (a repository's Git state, the
    /// activity walk, the size walk) or a budget found spent at the end
    /// holds the workspace as `unmeasured`: not idle, not reclaimable.
    pub fn measure(&self, now_ms: i64, policy: &ReclaimPolicy, budget: &Budget) -> LifecycleRecord {
        let paths = self.repository_paths();
        let git = measure_repositories(&paths, budget, &policy.git);
        let changed = newest_change_ms(&self.root, budget);
        let last_activity_ms = [
            self.conversation_activity_ms,
            git.newest_commit_ms,
            changed.unwrap_or(None),
            measure::created_ms(&self.root.join(crate::workspace::MANIFEST_FILE)),
        ]
        .into_iter()
        .flatten()
        .max();
        let mut record = LifecycleRecord {
            measured_at_ms: now_ms,
            last_activity_ms,
            idle: last_activity_ms
                .is_some_and(|at| now_ms.saturating_sub(at) >= policy.idle_after_ms()),
            holds: self.holds_with(&git.holds),
            issues: self.issues.clone(),
            dirty_files: git.dirty_files,
            unpushed_commits: git.unpushed_commits,
            behind_commits: git.behind_commits,
            ..self.carried_forward(last_activity_ms)
        };
        record.reclaimable = record.holds.is_empty();
        // Sizing walks the whole tree, build output included, so it runs when
        // the number is about to be read rather than every sweep.
        let wants_size = record.idle
            && (record.size_bytes.is_none() || record.notice_due(now_ms, policy) != NoticeDue::No);
        let sized = !wants_size || self.resize(&mut record, budget);
        if git.unfinished || changed.is_err() || !sized || budget.check().is_err() {
            record.unmeasured();
        }
        record
    }

    /// Measure the size on disk again. `false` when the budget ran out or the
    /// daemon stopped first; the last size is kept.
    pub fn resize(&self, record: &mut LifecycleRecord, budget: &Budget) -> bool {
        match artifacts::size_on_disk(&self.root, budget) {
            Ok(size) => {
                record.size_bytes = Some(size);
                true
            }
            Err(Unfinished) => false,
        }
    }

    /// Read the Git state once more, as the last look before build output is
    /// moved, and fold what it finds into `record`. Answers whether it still
    /// finds nothing at stake: every tree clean, every commit pushed, and all
    /// of it read within the budget.
    pub fn confirm_git(
        &self,
        record: &mut LifecycleRecord,
        policy: &ReclaimPolicy,
        budget: &Budget,
    ) -> bool {
        let git = measure_repositories(&self.repository_paths(), budget, &policy.git);
        if git.unfinished {
            record.unmeasured();
            return false;
        }
        if git.holds.is_empty() {
            return true;
        }
        record.dirty_files = git.dirty_files;
        record.unpushed_commits = git.unpushed_commits;
        record.hold(&git.holds);
        false
    }

    /// The build output this workspace could lose, each directory inspected.
    ///
    /// Only what is inside the workspace counts: the root and every checkout
    /// are resolved through their links first, and a checkout that resolves
    /// anywhere but strictly inside the root makes the whole workspace
    /// somebody else's. `Err` when the budget ran out first, and then nothing
    /// is removed.
    pub fn build_output(&self, budget: &Budget) -> Result<Vec<Artifact>, Unfinished> {
        let Some(root) = self.canonical_root() else {
            return Ok(Vec::new());
        };
        let mut repositories = Vec::new();
        for path in self.repository_paths() {
            match std::fs::canonicalize(&path) {
                Ok(resolved) if resolved.starts_with(&root) && resolved != root => {
                    repositories.push(resolved)
                }
                _ => {
                    eprintln!(
                        "workspace reclaim: {} resolves outside {}; its build output stays",
                        path.display(),
                        root.display()
                    );
                    return Ok(Vec::new());
                }
            }
        }
        let mut found = Vec::new();
        for repository in repositories {
            for candidate in artifacts::find(&repository, budget)? {
                if let Some(artifact) = artifacts::inspect(&repository, &candidate, budget)? {
                    found.push(artifact);
                }
            }
        }
        Ok(found)
    }

    /// The root with its links resolved: where the trash goes, and what every
    /// checkout has to be inside.
    pub fn canonical_root(&self) -> Option<PathBuf> {
        std::fs::canonicalize(&self.root).ok()
    }

    fn repository_paths(&self) -> Vec<PathBuf> {
        self.repositories
            .iter()
            .map(|(path, _)| path.clone())
            .collect()
    }

    /// The previous record's notice and pruning, for as long as they belong to
    /// the same quiet stretch. Activity since either one starts a new stretch.
    fn carried_forward(&self, last_activity_ms: Option<i64>) -> LifecycleRecord {
        let Some(previous) = &self.previous else {
            return LifecycleRecord::default();
        };
        let still_quiet =
            |at: Option<i64>| at.is_some_and(|at| last_activity_ms.is_some_and(|last| last <= at));
        let pruned = still_quiet(previous.pruned_at_ms);
        LifecycleRecord {
            noticed_at_ms: previous.noticed_at_ms,
            pruned_bytes: if pruned { previous.pruned_bytes } else { 0 },
            pruned_at_ms: if pruned { previous.pruned_at_ms } else { None },
            size_bytes: previous.size_bytes,
            ..LifecycleRecord::default()
        }
    }

    fn holds_with(&self, measured: &[&'static str]) -> Vec<String> {
        let mut holds: Vec<&'static str> = self.holds.clone();
        for hold in measured {
            if !holds.contains(hold) {
                holds.push(hold);
            }
        }
        holds.sort_by_key(|hold| hold_order(hold));
        holds.into_iter().map(str::to_string).collect()
    }
}

/// The trash a subject's build output is moved into, below its canonical root.
pub fn trash_of(root: &Path) -> Option<PathBuf> {
    artifacts::trash_of(root)
}

/// One order for every hold, whoever assembled the list: what is live, then
/// what only this workspace has, then the issues, then not knowing.
pub fn hold_order(hold: &str) -> usize {
    use crate::workspace::{
        FINISH_BLOCKER_AGENT_WORKING, FINISH_BLOCKER_DIRTY, FINISH_BLOCKER_PLAIN_DIRECTORY,
        FINISH_BLOCKER_UNKNOWN, FINISH_BLOCKER_UNPUSHED,
    };
    const ORDER: [&str; 10] = [
        HOLD_NOT_READY,
        FINISH_BLOCKER_AGENT_WORKING,
        HOLD_TERMINAL_OPEN,
        FINISH_BLOCKER_DIRTY,
        FINISH_BLOCKER_UNPUSHED,
        FINISH_BLOCKER_PLAIN_DIRECTORY,
        HOLD_ISSUE_OPEN,
        HOLD_ISSUES_UNREAD,
        FINISH_BLOCKER_UNKNOWN,
        HOLD_UNMEASURED,
    ];
    ORDER
        .iter()
        .position(|known| *known == hold)
        .unwrap_or(ORDER.len())
}

/// How a hold reads in a sentence: the refusal `workspace.reclaim` gives and
/// the notice the project agent reads.
pub fn hold_sentence(hold: &str) -> &'static str {
    match hold {
        HOLD_ISSUE_OPEN => "an issue linked to it is not Done",
        HOLD_ISSUES_UNREAD => "Build could not read the issues linked to it",
        HOLD_NOT_READY => "it is not ready",
        HOLD_TERMINAL_OPEN => "a terminal is open in it",
        HOLD_UNMEASURED => "Build could not finish measuring it",
        other => crate::workspace::blocker_sentence(other),
    }
}

/// A byte count as a person reads it: `17.2 GB`, `640 MB`, `12 KB`.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 || value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests;
