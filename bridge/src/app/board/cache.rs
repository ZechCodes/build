use crate::app::ActiveRun;

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::store::now_rfc3339;
use crate::worktree::{ExternalWorktree, WorktreeManager};

use super::super::{AppState, OffLockJob};

/// External-worktree scans are refreshed at most this often per project; the
/// board polls task.list every ~1.6 s and must never trigger a full rescan per
/// poll.
pub(in crate::app) const EXTERNAL_SCAN_INTERVAL: Duration = Duration::from_secs(10);

/// How long a task's `task.list` diffstat is served from cache before the next
/// poll recomputes it (same reasoning as the external-worktree scan interval).
pub(in crate::app) const TASK_STAT_TTL: Duration = Duration::from_secs(10);

/// How long the primary-checkout `task.list.primary_changes` summary is served
/// from cache before the next poll recomputes it (spec §5.3).
pub(in crate::app) const PRIMARY_SUMMARY_TTL: Duration = Duration::from_secs(10);

/// One project's cached external-worktree scan.
pub(in crate::app) struct ExternalScanCache {
    pub(in crate::app) scanned_at: std::time::Instant,
    pub(in crate::app) worktrees: Vec<ExternalWorktree>,
}

/// A claimed diff-cache refresh on its way to the blocking pool: the git work
/// and the test seam that watches it start.
pub(in crate::app) struct DiffRefreshJob {
    pub(in crate::app) refresh: DiffCacheRefresh,
    pub(in crate::app) observer: Option<DiffComputeObserver>,
}

impl OffLockJob for DiffRefreshJob {
    type Claim = DiffCacheKey;
    type Decided = Option<DiffCacheEntry>;

    fn claim(&self) -> DiffCacheKey {
        self.refresh.key()
    }

    fn decide(self) -> Option<DiffCacheEntry> {
        self.refresh.compute(self.observer.as_ref())
    }

    fn apply(state: &mut AppState, key: DiffCacheKey, entry: Option<DiffCacheEntry>) {
        state.publish_diff_refresh(&key, entry);
    }

    fn abandon(state: &mut AppState, key: DiffCacheKey) {
        state.release_diff_refresh(&key);
    }
}

impl DiffCacheRefresh {
    pub(in crate::app) fn key(&self) -> DiffCacheKey {
        match self {
            Self::RunStat { run_id, .. } => DiffCacheKey::RunStat(run_id.clone()),
            Self::ExternalScan { project_id, .. } => DiffCacheKey::ExternalScan(project_id.clone()),
            Self::PrimarySummary { project_id, .. } => {
                DiffCacheKey::PrimarySummary(project_id.clone())
            }
        }
    }

    /// The git work, on a thread that holds no lock. `None` means the compute
    /// failed and the cache keeps whatever it was already serving.
    pub(in crate::app) fn compute(
        &self,
        observer: Option<&DiffComputeObserver>,
    ) -> Option<DiffCacheEntry> {
        if let Some(observer) = observer {
            observer(&self.key());
        }
        match self {
            Self::RunStat {
                run_id,
                worktree,
                base_branch,
            } => Some(DiffCacheEntry::RunStat {
                run_id: run_id.clone(),
                stat: run_diffstat(worktree, base_branch),
            }),
            Self::ExternalScan {
                project_id,
                worktrees,
                base_branch,
                excluded,
            } => match worktrees.discover(base_branch, excluded) {
                Ok(scanned) => Some(DiffCacheEntry::ExternalScan {
                    project_id: project_id.clone(),
                    worktrees: scanned,
                }),
                Err(e) => {
                    eprintln!("external_worktrees {project_id}: {e}");
                    Some(DiffCacheEntry::ExternalScanUnreadable {
                        project_id: project_id.clone(),
                    })
                }
            },
            Self::PrimarySummary {
                project_id,
                repo_path,
                base_branch,
            } => primary_changes_summary(project_id, repo_path, base_branch).map(|summary| {
                DiffCacheEntry::PrimarySummary {
                    project_id: project_id.clone(),
                    summary,
                }
            }),
        }
    }
}

/// The board's checkout rows, and whether every project behind them has been
/// scanned at least once. A board that has not finished looking says so rather
/// than shipping an empty rail as the answer.
#[derive(Default)]
pub(in crate::app) struct ExternalWorktreeRows {
    pub(in crate::app) rows: Vec<Value>,
    pub(in crate::app) scanning: bool,
}

/// What a reader gets back from a project's checkout scan: the last list, and
/// whether any scan attempt has settled — one that landed a list, or one that
/// found a repository this daemon could not read. An empty list with nothing
/// behind it is a board still waiting, not a project with no worktrees, and the
/// two render differently.
#[derive(Default)]
pub(in crate::app) struct ScanRead {
    pub(in crate::app) worktrees: Vec<ExternalWorktree>,
    pub(in crate::app) settled: bool,
}

/// Why a checkout a caller named might not be in the last scan — the one
/// sentence every such refusal ends with, so "nothing has looked yet" and
/// "there is no such checkout" stop being the same answer. Both resolve on the
/// scan the missed read has already claimed.
pub(in crate::app) fn scan_may_yet_show_it(settled: bool) -> &'static str {
    if settled {
        "a checkout made outside Build since the last scan is resolvable once the scan now \
         running lands"
    } else {
        "no scan of this project's checkouts has landed yet, and the scan now running settles it"
    }
}

/// One entry of the diff caches the poll surfaces read: a run's diffstat, a
/// project's external-worktree scan, a project's primary-checkout summary.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(in crate::app) enum DiffCacheKey {
    RunStat(String),
    ExternalScan(String),
    PrimarySummary(String),
}

/// A diff-cache entry to compute, carrying every input the git work needs.
///
/// It borrows nothing from [`AppState`] on purpose: a refresh runs on a thread
/// that holds no lock. That is the whole point — the 2026-08-13 wedge was a
/// poll sitting inside libgit2 for seconds with the app mutex in its hand,
/// which stopped the relay read loop and got the device declared dead.
#[derive(Clone, Debug)]
pub(in crate::app) enum DiffCacheRefresh {
    RunStat {
        run_id: String,
        worktree: std::path::PathBuf,
        base_branch: String,
    },
    ExternalScan {
        project_id: String,
        /// The project's own checkout seam, cloned off the app mutex so the
        /// scan that walks every checkout runs without it.
        worktrees: WorktreeManager,
        base_branch: String,
        excluded: std::collections::HashSet<std::path::PathBuf>,
    },
    PrimarySummary {
        project_id: String,
        repo_path: std::path::PathBuf,
        base_branch: String,
    },
}

/// What a refresh computed, on its way back into the cache.
pub(in crate::app) enum DiffCacheEntry {
    RunStat {
        run_id: String,
        stat: Value,
    },
    ExternalScan {
        project_id: String,
        worktrees: Vec<ExternalWorktree>,
    },
    /// The scan ran and could not read the repository. Stored so a project
    /// whose repo is gone settles instead of being walked again by every poll.
    ExternalScanUnreadable {
        project_id: String,
    },
    PrimarySummary {
        project_id: String,
        summary: Value,
    },
}

/// Called on the thread that is about to compute a diff-cache entry, before the
/// git work. The seam the stale-while-revalidate tests observe: it is how they
/// hold a compute open on purpose and check the app mutex is free while it
/// runs. `None` in production — nothing outside tests ever sets it.
pub(in crate::app) type DiffComputeObserver = Arc<dyn Fn(&DiffCacheKey) + Send + Sync>;

/// One run's diffstat against its base, plus what sits uncommitted in its tree.
///
/// Counts only: this poll surface ships numbers, so it must never pay to render
/// (or even load) the worktree's patch text.
pub(in crate::app) fn run_diffstat(worktree: &std::path::Path, base_branch: &str) -> Value {
    let git_state = git2::Repository::open(worktree).ok().and_then(|repo| {
        let head_ref = repo.head().ok()?;
        let checked_out_branch = head_ref.shorthand().map(str::to_string);
        let head = head_ref.peel_to_commit().ok()?;
        let head_committed_at = crate::worktree::rfc3339_from_unix(head.time().seconds());
        let comparison = crate::worktree::branch_comparison(
            &repo,
            &head,
            checked_out_branch.as_deref(),
            base_branch,
        );
        Some((checked_out_branch, comparison, head_committed_at))
    });
    let checked_out_branch = git_state
        .as_ref()
        .and_then(|(branch, _, _)| branch.as_deref());
    let comparison = git_state.as_ref().map(|(_, comparison, _)| comparison);
    let head_committed_at = git_state
        .as_ref()
        .and_then(|(_, _, committed_at)| committed_at.as_deref());
    let uncommitted = crate::diff::stat_uncommitted(worktree)
        .map(|stat| stat.to_json())
        .unwrap_or(Value::Null);
    crate::diff::stat_against_base(worktree, base_branch)
        .map(|stat| {
            json!({
                "files_changed": stat.files_changed,
                "insertions": stat.insertions,
                "deletions": stat.deletions,
                "branch": checked_out_branch,
                "comparison_ref": comparison.and_then(|value| value.reference.as_deref()),
                "upstream": comparison.and_then(|value| value.upstream.as_deref()),
                "ahead": comparison.and_then(|value| value.ahead),
                "behind": comparison.and_then(|value| value.behind),
                // When this branch last got a commit. The inbox's floor for how
                // recently the work moved, computed in the cached walk that has
                // the commit in its hand already.
                "head_committed_at": head_committed_at,
                "uncommitted": uncommitted,
            })
        })
        .unwrap_or(Value::Null)
}

/// One project's primary-checkout changes summary, minus the run ownership
/// stamped on at serve time. `None` on a failure (unborn HEAD, fs error): it
/// logs and the cache keeps what it had.
pub(in crate::app) fn primary_changes_summary(
    project_id: &str,
    repo_path: &std::path::Path,
    base_branch: &str,
) -> Option<Value> {
    type SyncCounts = (Option<String>, Option<String>, Option<u64>, Option<u64>);
    fn head_sync_counts(repo: &git2::Repository, base_branch: &str) -> SyncCounts {
        const NONE: SyncCounts = (None, None, None, None);
        let head = match repo.head() {
            Ok(head) if head.is_branch() => head,
            _ => return NONE,
        };
        let Some(branch) = head.shorthand() else {
            return NONE;
        };
        let Ok(commit) = head.peel_to_commit() else {
            return NONE;
        };
        let comparison =
            crate::worktree::branch_comparison(repo, &commit, Some(branch), base_branch);
        (
            comparison.upstream,
            comparison.reference,
            comparison.ahead,
            comparison.behind,
        )
    }

    let repo = git2::Repository::open(repo_path);
    let branch = repo
        .as_ref()
        .ok()
        .and_then(|r| r.head().ok())
        .and_then(|h| h.shorthand().map(str::to_string))
        .unwrap_or_else(|| "HEAD".to_string());
    // What this checkout's history looks like right now. A bare checkout has no
    // conversation and no lifecycle, so its own commits are all the inbox has —
    // to date it by (`head_committed_at`), and to tell whether it has said
    // anything since the human cleared its row (`head_sha`). Both are computed
    // HERE, inside the cached walk, never on the poll path.
    let head_commit = repo
        .as_ref()
        .ok()
        .and_then(|repo| repo.head().ok())
        .and_then(|head| head.peel_to_commit().ok());
    let head_sha = head_commit.as_ref().map(|commit| commit.id().to_string());
    let head_committed_at = head_commit
        .as_ref()
        .and_then(|commit| crate::worktree::rfc3339_from_unix(commit.time().seconds()));
    let (upstream, comparison_ref, ahead, behind) = repo
        .as_ref()
        .ok()
        .map(|repo| head_sync_counts(repo, base_branch))
        .unwrap_or((None, None, None, None));
    match crate::diff::stat_against_head(repo_path) {
        Ok(stat) => Some(json!({
            "project_id": project_id,
            "branch": branch,
            "upstream": upstream,
            "comparison_ref": comparison_ref,
            "ahead": ahead,
            "behind": behind,
            "head_sha": head_sha,
            "head_committed_at": head_committed_at,
            "files_changed": stat.files_changed,
            "insertions": stat.insertions,
            "deletions": stat.deletions,
        })),
        Err(e) => {
            eprintln!("primary_changes {project_id}: {e}");
            None
        }
    }
}

impl AppState {
    /// A project's primary-checkout summary, as of `now`.
    pub(in crate::app) fn store_primary_summary(
        &mut self,
        project_id: &str,
        summary: Value,
        now: std::time::Instant,
    ) {
        let Some(project) = self.project_mut(project_id) else {
            return;
        };
        let changed = project
            .primary_summary
            .as_ref()
            .is_none_or(|(_, previous)| previous != &summary);
        project.primary_summary = Some((now, summary));
        if changed {
            self.note_board_changed();
        }
    }

    /// Drop a run's cached diffstat — the mutation that calls this just changed
    /// the tree it described. Any refresh in flight is superseded with it.
    pub(in crate::app) fn invalidate_run_stat(&mut self, run_id: &str) {
        self.run_stat_cache.remove(run_id);
        self.supersede_diff_refresh(&DiffCacheKey::RunStat(run_id.to_string()));
    }

    /// Drop a project's cached primary-checkout summary, same reasoning.
    pub(in crate::app) fn invalidate_primary_summary(&mut self, project_id: &str) {
        if let Some(project) = self.project_mut(project_id) {
            project.primary_summary = None;
        }
        self.supersede_diff_refresh(&DiffCacheKey::PrimarySummary(project_id.to_string()));
    }

    /// The runs of a project that have not finished, with the id each is
    /// keyed by. Reading a run's branch costs a HEAD read on disk, so callers
    /// that want one branch stop at it rather than describing them all.
    pub(in crate::app) fn live_runs_of<'a>(
        &'a self,
        project_id: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a ActiveRun)> {
        self.runs.iter().filter(move |(run_id, active)| {
            !active.run.state.is_terminal()
                && self.projects.project_id_of(run_id) == Some(project_id)
        })
    }

    /// The live run that owns a branch in a project, if one does.
    pub(in crate::app) fn run_on_branch(&self, project_id: &str, branch: &str) -> Option<String> {
        self.live_runs_of(project_id)
            .find(|(_, active)| active.worktree.branch() == branch)
            .map(|(run_id, _)| run_id.clone())
    }

    /// The project's external worktrees, as the last scan left them, plus
    /// whether a scan has ever landed. A read never scans: it serves what it
    /// has and claims the rescan it needs, which runs off every lock and
    /// invalidates the browser when it lands.
    pub(in crate::app) fn external_worktrees(&mut self, project_id: &str) -> ScanRead {
        if let Some(refresh) = self.external_scan_refresh(project_id) {
            let settled_at = self.scan_settled_at(project_id);
            self.refresh_if_stale(settled_at, EXTERNAL_SCAN_INTERVAL, refresh);
        }
        ScanRead {
            worktrees: self
                .external_scan_of(project_id)
                .map(|cache| cache.worktrees.clone())
                .unwrap_or_default(),
            settled: self.scan_settled_at(project_id).is_some(),
        }
    }

    /// A checkout Build just put on disk, or handed back: it joins the last
    /// scan rather than emptying it, so the very next board poll shows it.
    pub(in crate::app) fn note_worktree_appeared(
        &mut self,
        project_id: &str,
        worktree: ExternalWorktree,
    ) {
        self.amend_external_scan(project_id, |worktrees| {
            let replaced = worktrees
                .iter()
                .position(|known| known.path == worktree.path)
                .map(|index| worktrees.remove(index));
            let changed = replaced.as_ref() != Some(&worktree);
            worktrees.push(worktree);
            crate::worktree::sort_checkouts(worktrees);
            changed
        });
    }

    /// A checkout that is gone, or that a run has taken ownership of: it leaves
    /// the last scan, which is what the rail lists as unbound. A checkout bound
    /// to a run was never in the list, so this is routinely a no-op.
    pub(in crate::app) fn note_worktree_gone(&mut self, project_id: &str, path: &std::path::Path) {
        let canonical = Self::canonical_root(path);
        self.amend_external_scan(project_id, |worktrees| {
            let before = worktrees.len();
            worktrees.retain(|known| known.path != canonical);
            before != worktrees.len()
        });
    }

    /// The one checkout of a project that `is_it` names, or the refusal that
    /// says why there is none.
    ///
    /// A miss claims a scan and the refusal promises it, because both ways to
    /// miss are worth retrying: a checkout made outside Build since the last
    /// scan, and a project whose checkouts nothing has looked at yet. `refusal`
    /// is what the caller was asking for, in its own words; how the scan bears
    /// on it is [`scan_may_yet_show_it`], which is the same sentence wherever a
    /// checkout is missed.
    pub(in crate::app) fn find_checkout(
        &mut self,
        project_id: &str,
        refusal: &str,
        is_it: impl Fn(&ExternalWorktree) -> bool,
    ) -> Result<ExternalWorktree, String> {
        let scan = self.external_worktrees(project_id);
        if let Some(checkout) = scan.worktrees.into_iter().find(|c| is_it(c)) {
            return Ok(checkout);
        }
        self.rescan_external_worktrees(project_id);
        Err(format!(
            "{refusal} ({})",
            scan_may_yet_show_it(scan.settled)
        ))
    }

    /// Resolve a client-supplied `worktree_id` against the discovered list
    /// only — a raw path is never accepted.
    pub(in crate::app) fn resolve_external_worktree(
        &mut self,
        project_id: &str,
        worktree_id: &str,
    ) -> Result<ExternalWorktree, String> {
        self.find_checkout(
            project_id,
            &format!("unknown worktree_id: {worktree_id}"),
            |checkout| checkout.id == worktree_id,
        )
    }

    /// The last walk of a project's primary checkout, if one has ever landed.
    pub(in crate::app) fn primary_summary_of(
        &self,
        project_id: &str,
    ) -> Option<&(std::time::Instant, Value)> {
        self.projects
            .iter()
            .find(|p| p.id == project_id)?
            .primary_summary
            .as_ref()
    }

    // ---- the poll surfaces' diff caches (stale-while-revalidate) -------------

    /// Whether an entry stamped at `computed_at` has aged out of its window.
    pub(in crate::app) fn diff_cache_is_stale(
        &self,
        computed_at: std::time::Instant,
        ttl: Duration,
    ) -> bool {
        #[cfg(test)]
        if self.force_stale_diff_caches {
            return true;
        }
        computed_at.elapsed() >= ttl
    }

    /// The refresh that recomputes one run's diffstat. `None` when the run has
    /// no diff to take: it is gone, terminal, or its worktree is already pruned.
    pub(in crate::app) fn run_stat_refresh(&self, run_id: &str) -> Option<DiffCacheRefresh> {
        let active = self.runs.get(run_id)?;
        if active.run.state.is_terminal() || !active.worktree.path.exists() {
            return None;
        }
        Some(DiffCacheRefresh::RunStat {
            run_id: run_id.to_string(),
            worktree: active.worktree.path.clone(),
            base_branch: active.worktree.base_branch.clone(),
        })
    }

    /// The refresh that rescans one project's external worktrees.
    pub(in crate::app) fn external_scan_refresh(
        &self,
        project_id: &str,
    ) -> Option<DiffCacheRefresh> {
        let project = self.project(project_id)?;
        Some(DiffCacheRefresh::ExternalScan {
            project_id: project.id.clone(),
            worktrees: project.orch.worktrees().clone(),
            base_branch: project.base_branch.clone(),
            excluded: self.bound_worktree_paths(),
        })
    }

    /// The refresh that recomputes one project's primary-checkout summary.
    pub(in crate::app) fn primary_summary_refresh(
        &self,
        project_id: &str,
    ) -> Option<DiffCacheRefresh> {
        let project = self.project(project_id)?;
        Some(DiffCacheRefresh::PrimarySummary {
            project_id: project.id.clone(),
            repo_path: project.repo_path.clone(),
            base_branch: project.base_branch.clone(),
        })
    }

    /// Whether a refresh of this entry is running right now. Nothing in the
    /// daemon asks — a claim is taken and released where it is made — but a
    /// test that holds a compute open has no other way to see it.
    #[cfg(test)]
    pub(in crate::app) fn diff_refresh_is_running(&self, key: &DiffCacheKey) -> bool {
        self.diff_refreshes_in_flight.contains(key)
    }

    /// Recompute this entry behind whatever the caller is about to answer with.
    ///
    /// Single-flight and non-blocking: a refresh already running absorbs this
    /// call, and one that starts here runs on a thread that holds nothing. No
    /// age test — the caller has already decided it wants the git work done.
    pub(in crate::app) fn refresh_now(&mut self, refresh: DiffCacheRefresh) {
        if !self.diff_refreshes_in_flight.insert(refresh.key()) {
            return;
        }
        self.run_off_lock(DiffRefreshJob {
            refresh,
            observer: self.diff_compute_observer.clone(),
        });
    }

    /// [`AppState::refresh_now`] unless what it would replace is younger than
    /// `ttl`. The stamp comes from the caller because the caller has just read
    /// it: nothing here looks a timestamp up by which cache it belongs to.
    pub(in crate::app) fn refresh_if_stale(
        &mut self,
        computed_at: Option<std::time::Instant>,
        ttl: Duration,
        refresh: DiffCacheRefresh,
    ) {
        let stale = computed_at.is_none_or(|at| self.diff_cache_is_stale(at, ttl));
        if stale {
            self.refresh_now(refresh);
        }
    }

    /// Store what a refresh computed and let its claim go. A refresh that was
    /// superseded while it ran describes a tree the daemon has since changed
    /// on purpose, so what it computed is dropped and only the claim goes back.
    pub(in crate::app) fn publish_diff_refresh(
        &mut self,
        key: &DiffCacheKey,
        entry: Option<DiffCacheEntry>,
    ) {
        let claimed = self.diff_refreshes_in_flight.remove(key);
        let superseded = self.diff_refreshes_superseded.remove(key);
        if !claimed || superseded {
            return;
        }
        if let Some(entry) = entry {
            self.store_diff_entry(entry);
        }
    }

    /// Let a claim go without publishing anything.
    pub(in crate::app) fn release_diff_refresh(&mut self, key: &DiffCacheKey) {
        self.diff_refreshes_in_flight.remove(key);
        self.diff_refreshes_superseded.remove(key);
    }

    /// Overtake whatever refresh of this entry is running: the caller has just
    /// written something newer than that refresh can possibly know about, so
    /// its result is dropped when it lands.
    ///
    /// The claim is deliberately kept until then. Releasing it instead — which
    /// is what the caches did before — lets the very next read start a second
    /// compute of the same thing behind the first, and then lets the first,
    /// pre-edit one land on top of the edit and discard the second's answer.
    pub(in crate::app) fn supersede_diff_refresh(&mut self, key: &DiffCacheKey) {
        if self.diff_refreshes_in_flight.contains(key) {
            self.diff_refreshes_superseded.insert(key.clone());
        }
    }

    /// Write a computed entry into the cache it belongs to. The one place a
    /// kind of entry names the cache it settles in; each arm below is that
    /// cache's own write, and an entry whose run or project has since gone is
    /// dropped by it.
    pub(in crate::app) fn store_diff_entry(&mut self, entry: DiffCacheEntry) {
        let now = std::time::Instant::now();
        match entry {
            DiffCacheEntry::RunStat { run_id, stat } => self.store_run_stat(run_id, stat, now),
            DiffCacheEntry::ExternalScan {
                project_id,
                worktrees,
            } => self.store_external_scan(&project_id, worktrees, now),
            DiffCacheEntry::ExternalScanUnreadable { project_id } => {
                self.store_scan_failure(&project_id, now)
            }
            DiffCacheEntry::PrimarySummary {
                project_id,
                summary,
            } => self.store_primary_summary(&project_id, summary, now),
        }
    }

    /// A run's diffstat, as of `now`.
    pub(in crate::app) fn store_run_stat(
        &mut self,
        run_id: String,
        stat: Value,
        now: std::time::Instant,
    ) {
        // Two computes that disagree are files that changed. Only when there
        // was something to disagree with: an invalidated entry recomputes from
        // nothing, and that is a mutation, not a filesystem event.
        let (first, changed) = match self.run_stat_cache.get(&run_id) {
            Some((_, previous)) => (false, previous != &stat),
            None => (true, false),
        };
        if changed {
            self.run_files_changed_at
                .insert(run_id.clone(), now_rfc3339());
            // This cache IS the git watcher: two computes that disagree are
            // files that landed in the checkout, which is exactly what an
            // entity's diff surface is showing. It fires as fast as an agent
            // writes files, so the entity's own event is paced.
            self.note_entity_settled(&run_id);
        }
        self.run_stat_cache.insert(run_id, (now, stat));
        // A board answered `stat: null` for this run and claimed this refresh;
        // nothing else will ever tell it the number arrived.
        if first {
            self.note_board_changed();
        }
    }

    /// A project's checkouts, as one walk of its repository found them.
    pub(in crate::app) fn store_external_scan(
        &mut self,
        project_id: &str,
        worktrees: Vec<ExternalWorktree>,
        now: std::time::Instant,
    ) {
        let Some(project) = self.project_mut(project_id) else {
            return;
        };
        // A board answered "still scanning", or answered from a list this one
        // disagrees with. Either way the rows the browser is holding are not
        // the rows this daemon would send now, so it is told to ask again.
        let changed = project
            .external_scan
            .as_ref()
            .is_none_or(|cache| cache.worktrees != worktrees);
        project.external_scan = Some(ExternalScanCache {
            scanned_at: now,
            worktrees,
        });
        project.external_scan_failed_at = None;
        if changed {
            self.note_board_changed();
        }
    }

    /// A repository this daemon could not read, so the interval is measured
    /// from the attempt rather than from a list that never arrived.
    pub(in crate::app) fn store_scan_failure(&mut self, project_id: &str, now: std::time::Instant) {
        let Some(project) = self.project_mut(project_id) else {
            return;
        };
        let settling = project.external_scan_failed_at.is_none();
        project.external_scan_failed_at = Some(now);
        if settling {
            self.note_board_changed();
        }
    }

    /// A run's diffstat for the `board.list` poll surface, held for
    /// [`TASK_STAT_TTL`] and then served stale while it refreshes. `None` until
    /// the first refresh lands, and for a terminal run (worktree pruned or
    /// about to be) forever.
    pub(in crate::app) fn run_stat(&mut self, run_id: &str) -> Option<Value> {
        if let Some(refresh) = self.run_stat_refresh(run_id) {
            let computed_at = self.run_stat_cache.get(run_id).map(|(at, _)| *at);
            self.refresh_if_stale(computed_at, TASK_STAT_TTL, refresh);
        }
        self.run_stat_cache
            .get(run_id)
            .map(|(_, stat)| stat.clone())
    }

    /// When this project's last scan attempt settled, whether it landed a list
    /// or gave up on a repository it could not read. What the interval is
    /// measured from, so a broken repo is not walked again by every poll.
    pub(in crate::app) fn scan_settled_at(&self, project_id: &str) -> Option<std::time::Instant> {
        let project = self.project(project_id)?;
        project
            .external_scan
            .as_ref()
            .map(|cache| cache.scanned_at)
            .max(project.external_scan_failed_at)
    }

    /// The last scan of a project's checkouts, if one has ever landed.
    pub(in crate::app) fn external_scan_of(&self, project_id: &str) -> Option<&ExternalScanCache> {
        self.projects
            .iter()
            .find(|p| p.id == project_id)?
            .external_scan
            .as_ref()
    }

    /// Scan one project's checkouts here and now, with the app mutex in hand.
    ///
    /// Tests only, and nothing else: every verb that has to decide against the
    /// checkouts that exist — a dispatch, an adoption, an implementation handed
    /// a card — asks for them in the lock-free run phase of a
    /// [`WorktreeLifecycleJob`]. What is left here is the tests' way to settle
    /// the cache before they read an id out of it.
    #[cfg(test)]
    pub(in crate::app) fn scan_external_worktrees_now(
        &mut self,
        project_id: &str,
    ) -> Result<Vec<ExternalWorktree>, String> {
        let excluded = self.bound_worktree_paths();
        let base = self.base_for(project_id)?;
        let worktrees = self.orch_for(project_id)?.worktrees().clone();
        match worktrees.discover(&base, &excluded) {
            Ok(scanned) => {
                self.store_diff_entry(DiffCacheEntry::ExternalScan {
                    project_id: project_id.to_string(),
                    worktrees: scanned.clone(),
                });
                Ok(scanned)
            }
            Err(e) => {
                eprintln!("external_worktrees {project_id}: {e}");
                Err(e.to_string())
            }
        }
    }

    /// Rescan a project's checkouts behind whatever the board is serving: Build
    /// just changed something about this repository that the cached list cannot
    /// be amended for.
    pub(in crate::app) fn rescan_external_worktrees(&mut self, project_id: &str) {
        if let Some(refresh) = self.external_scan_refresh(project_id) {
            self.refresh_now(refresh);
        }
    }

    /// Edit a project's last scan in place. A scan in flight described the
    /// repository as it was before this change, so the edit supersedes it and
    /// whatever it finds is dropped — the amended list is the newer truth. A
    /// project that has never been scanned is left alone, and so is the scan it
    /// has running: there is nothing here that scan is out of date about, and
    /// its first list is what shows the checkout.
    ///
    /// `amend` answers whether it changed the list. An amendment that changed
    /// nothing is not an edit: it neither overtakes the running scan nor
    /// tells the browser about a board that is as it was.
    pub(in crate::app) fn amend_external_scan(
        &mut self,
        project_id: &str,
        amend: impl FnOnce(&mut Vec<ExternalWorktree>) -> bool,
    ) {
        let amended = self
            .project_mut(project_id)
            .and_then(|project| project.external_scan.as_mut())
            // The stamp is not touched: this edit knows about one checkout, and
            // the rest of the list is exactly as old as it was.
            .is_some_and(|cache| amend(&mut cache.worktrees));
        if !amended {
            return;
        }
        self.supersede_diff_refresh(&DiffCacheKey::ExternalScan(project_id.to_string()));
        self.note_board_changed();
    }
}
