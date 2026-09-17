use crate::app::ActiveRun;

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use crate::worktree::{ExternalWorktree, WorktreeManager};

use super::super::board_index::CachePublication;
pub(in crate::app) use super::super::board_index::{
    DiffCacheEntry, DiffCacheKey, ExternalScanCache,
};
use super::super::{AppState, CacheEffect, OffLockJob, RefreshClaim};

/// External-worktree scans are refreshed at most this often per project; the
/// board polls task.list every ~1.6 s and must never trigger a full rescan per
/// poll.
pub(in crate::app) const EXTERNAL_SCAN_INTERVAL: Duration = Duration::from_secs(10);

/// How long a task's `task.list` diffstat is served from cache before the next
/// poll recomputes it (same reasoning as the external-worktree scan interval).
pub(in crate::app) const TASK_STAT_TTL: Duration = Duration::from_secs(10);

pub(in crate::app) const WORKSPACE_SUMMARY_TTL: Duration = Duration::from_secs(10);

/// A claimed diff-cache refresh on its way to the blocking pool: the git work
/// and the test seam that watches it start.
pub(in crate::app) struct DiffRefreshJob {
    pub(in crate::app) refresh: DiffCacheRefresh,
    pub(in crate::app) claim: Option<RefreshClaim>,
    pub(in crate::app) observer: Option<DiffComputeObserver>,
}

impl OffLockJob for DiffRefreshJob {
    type Claim = RefreshClaim;
    type Decided = Option<DiffCacheEntry>;

    fn claim(&mut self) -> RefreshClaim {
        self.claim.take().expect("diff refresh claim taken once")
    }

    fn decide(self) -> Option<DiffCacheEntry> {
        self.refresh.compute(self.observer.as_ref())
    }

    fn apply(state: &mut AppState, claim: RefreshClaim, entry: Option<DiffCacheEntry>) {
        state.publish_diff_refresh(claim, entry);
    }

    fn abandon(state: &mut AppState, claim: RefreshClaim) {
        state.release_diff_refresh(claim);
    }
}

impl DiffCacheRefresh {
    pub(in crate::app) fn key(&self) -> DiffCacheKey {
        match self {
            Self::RunStat { run_id, .. } => DiffCacheKey::RunStat(run_id.clone()),
            Self::ExternalScan { project_id, .. } => DiffCacheKey::ExternalScan(project_id.clone()),
            Self::WorkspaceSummary {
                workspace_id,
                repositories,
            } => DiffCacheKey::WorkspaceSummary(workspace_id.clone(), repositories.clone()),
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
            Self::WorkspaceSummary {
                workspace_id,
                repositories,
            } => {
                let summary = workspace_work_summary(repositories);
                Some(DiffCacheEntry::WorkspaceSummary {
                    workspace_id: workspace_id.clone(),
                    repositories: repositories.clone(),
                    summary,
                })
            }
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
    WorkspaceSummary {
        workspace_id: String,
        repositories: Vec<std::path::PathBuf>,
    },
}

fn workspace_work_summary(repositories: &[std::path::PathBuf]) -> Value {
    if repositories.is_empty() {
        return Value::Null;
    }
    let Ok(summary) = crate::gitgui::aggregate_work_summary(repositories) else {
        return Value::Null;
    };
    json!({
        "pushes": summary.pushes,
        "behind": summary.behind,
        "additions": summary.additions,
        "deletions": summary.deletions,
        "clean": summary.clean,
    })
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

impl AppState {
    /// Drop a run's cached diffstat — the mutation that calls this just changed
    /// the tree it described. Any refresh in flight is superseded with it.
    pub(in crate::app) fn invalidate_run_stat(&mut self, run_id: &str) {
        self.board.diff_mut().invalidate_run_stat(run_id);
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

    /// The project's external worktrees, as the last scan left them, plus
    /// whether a scan has ever landed. A read never scans: it serves what it
    /// has and claims the rescan it needs, which runs off every lock and
    /// invalidates the browser when it lands.
    pub(in crate::app) fn external_worktrees(&mut self, project_id: &str) -> ScanRead {
        if let Some(refresh) = self.external_scan_refresh(project_id) {
            let settled_at = self.board.diff().external_scan(project_id).settled_at;
            self.refresh_if_stale(settled_at, EXTERNAL_SCAN_INTERVAL, refresh);
        }
        let scan = self.board.diff().external_scan(project_id);
        ScanRead {
            worktrees: scan.worktrees.to_vec(),
            settled: scan.settled_at.is_some(),
        }
    }

    /// A checkout Build just put on disk, or handed back: it joins the last
    /// scan rather than emptying it, so the very next board poll shows it.
    pub(in crate::app) fn note_worktree_appeared(
        &mut self,
        project_id: &str,
        worktree: ExternalWorktree,
    ) {
        let effects = self
            .board
            .diff_mut()
            .note_worktree_appeared(project_id, worktree);
        self.apply_cache_effects(effects);
    }

    /// A checkout that is gone, or that a run has taken ownership of: it leaves
    /// the last scan, which is what the rail lists as unbound. A checkout bound
    /// to a run was never in the list, so this is routinely a no-op.
    pub(in crate::app) fn note_worktree_gone(&mut self, project_id: &str, path: &std::path::Path) {
        let canonical = Self::canonical_root(path);
        let effects = self
            .board
            .diff_mut()
            .note_worktree_gone(project_id, &canonical);
        self.apply_cache_effects(effects);
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

    pub(in crate::app) fn workspace_summary_of(
        &self,
        workspace_id: &str,
        repositories: &[std::path::PathBuf],
    ) -> Option<(std::time::Instant, &Value)> {
        self.board
            .diff()
            .workspace_summary(workspace_id, repositories)
            .map(|cached| (cached.computed_at, cached.value))
    }

    pub(in crate::app) fn sync_workspace_summaries(
        &mut self,
        memberships: &[(String, Vec<std::path::PathBuf>)],
    ) {
        self.board.diff_mut().sync_workspace_summaries(memberships);
    }

    pub(in crate::app) fn workspace_summary_refresh(
        &self,
        workspace_id: &str,
        repositories: Vec<std::path::PathBuf>,
    ) -> DiffCacheRefresh {
        DiffCacheRefresh::WorkspaceSummary {
            workspace_id: workspace_id.to_string(),
            repositories,
        }
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

    /// Whether a refresh of this entry is running right now. Nothing in the
    /// daemon asks — a claim is taken and released where it is made — but a
    /// test that holds a compute open has no other way to see it.
    #[cfg(test)]
    pub(in crate::app) fn diff_refresh_is_running(&self, key: &DiffCacheKey) -> bool {
        self.board.diff().refresh_is_running(key)
    }

    #[cfg(test)]
    pub(in crate::app) fn claim_diff_refresh_for_test(
        &mut self,
        key: DiffCacheKey,
    ) -> RefreshClaim {
        self.board
            .diff_mut()
            .claim_refresh(key)
            .expect("test refresh claim must be free")
    }

    #[cfg(test)]
    pub(in crate::app) fn diff_refresh_is_superseded(&self, key: &DiffCacheKey) -> bool {
        self.board.diff().refresh_is_superseded(key)
    }

    #[cfg(test)]
    pub(in crate::app) fn has_cached_run_stat(&self, run_id: &str) -> bool {
        self.board.diff().has_run_stat(run_id)
    }

    #[cfg(test)]
    pub(in crate::app) fn clear_external_scan_for_test(&mut self, project_id: &str) {
        self.board.diff_mut().clear_external_scan(project_id);
    }

    #[cfg(test)]
    pub(in crate::app) fn age_external_scan_for_test(&mut self, project_id: &str, age: Duration) {
        self.board.diff_mut().age_external_scan(project_id, age);
    }

    /// Recompute this entry behind whatever the caller is about to answer with.
    ///
    /// Single-flight and non-blocking: a refresh already running absorbs this
    /// call, and one that starts here runs on a thread that holds nothing. No
    /// age test — the caller has already decided it wants the git work done.
    pub(in crate::app) fn refresh_now(&mut self, refresh: DiffCacheRefresh) {
        let Some(claim) = self.board.diff_mut().claim_refresh(refresh.key()) else {
            return;
        };
        self.run_off_lock(DiffRefreshJob {
            refresh,
            claim: Some(claim),
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
        claim: RefreshClaim,
        entry: Option<DiffCacheEntry>,
    ) {
        match self
            .board
            .diff_mut()
            .publish_refresh(claim, entry, std::time::Instant::now())
        {
            CachePublication::Settled(effects) => self.apply_cache_effects(effects),
            CachePublication::RunStatChanged(pending) => {
                self.note_entity_settled(pending.run_id());
                self.board.diff_mut().commit_changed_run_stat(pending);
            }
        }
    }

    /// Let a claim go without publishing anything.
    pub(in crate::app) fn release_diff_refresh(&mut self, claim: RefreshClaim) {
        self.board.diff_mut().release_refresh(claim);
    }

    /// Write a computed entry into the cache it belongs to. The one place a
    /// kind of entry names the cache it settles in; each arm below is that
    /// cache's own write, and an entry whose run or project has since gone is
    /// dropped by it.
    #[cfg(test)]
    pub(in crate::app) fn store_diff_entry(&mut self, entry: DiffCacheEntry) {
        match entry {
            DiffCacheEntry::RunStat { run_id, stat } => {
                self.store_run_stat(run_id, stat, std::time::Instant::now());
            }
            entry => {
                let effects = self
                    .board
                    .diff_mut()
                    .store_entry_for_adapter(entry, std::time::Instant::now());
                self.apply_cache_effects(effects);
            }
        }
    }

    /// A run's diffstat, as of `now`.
    #[cfg(test)]
    pub(in crate::app) fn store_run_stat(
        &mut self,
        run_id: String,
        stat: Value,
        now: std::time::Instant,
    ) {
        let publication = self.board.diff_mut().prepare_run_stat(run_id, stat, now);
        self.apply_cache_publication(publication);
    }

    /// A run's diffstat for the `board.list` poll surface, held for
    /// [`TASK_STAT_TTL`] and then served stale while it refreshes. `None` until
    /// the first refresh lands, and for a terminal run (worktree pruned or
    /// about to be) forever.
    pub(in crate::app) fn run_stat(&mut self, run_id: &str) -> Option<Value> {
        if let Some(refresh) = self.run_stat_refresh(run_id) {
            let computed_at = self
                .board
                .diff()
                .run_stat(run_id)
                .map(|cached| cached.computed_at);
            self.refresh_if_stale(computed_at, TASK_STAT_TTL, refresh);
        }
        self.board
            .diff()
            .run_stat(run_id)
            .map(|cached| cached.value.clone())
    }

    /// When this project's last scan attempt settled, whether it landed a list
    /// or gave up on a repository it could not read. What the interval is
    /// measured from, so a broken repo is not walked again by every poll.
    pub(in crate::app) fn scan_settled_at(&self, project_id: &str) -> Option<std::time::Instant> {
        self.board.diff().external_scan(project_id).settled_at
    }

    /// The last scan of a project's checkouts, if one has ever landed.
    pub(in crate::app) fn external_scan_of(&self, project_id: &str) -> Option<&ExternalScanCache> {
        self.board.diff().external_scan_cache(project_id)
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

    fn apply_cache_effects(&self, effects: Vec<CacheEffect>) {
        for effect in effects {
            match effect {
                CacheEffect::BoardChanged => self.note_board_changed(),
            }
        }
    }

    #[cfg(test)]
    fn apply_cache_publication(&mut self, publication: CachePublication) {
        match publication {
            CachePublication::Settled(effects) => self.apply_cache_effects(effects),
            CachePublication::RunStatChanged(pending) => {
                self.note_entity_settled(pending.run_id());
                self.board.diff_mut().commit_changed_run_stat(pending);
            }
        }
    }
}
