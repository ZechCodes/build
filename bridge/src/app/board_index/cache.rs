use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use serde_json::Value;

use crate::store::now_rfc3339;
use crate::worktree::ExternalWorktree;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(in crate::app) enum DiffCacheKey {
    RunStat(String),
    ExternalScan(String),
    WorkspaceSummary(String, Vec<std::path::PathBuf>),
}

pub(in crate::app) enum DiffCacheEntry {
    RunStat {
        run_id: String,
        stat: Value,
    },
    ExternalScan {
        project_id: String,
        worktrees: Vec<ExternalWorktree>,
    },
    ExternalScanUnreadable {
        project_id: String,
    },
    WorkspaceSummary {
        workspace_id: String,
        repositories: Vec<std::path::PathBuf>,
        summary: Value,
    },
}

pub(in crate::app) struct ExternalScanCache {
    pub scanned_at: Instant,
    pub worktrees: Vec<ExternalWorktree>,
}

#[derive(Default)]
struct ProjectCache {
    external_scan: Option<ExternalScanCache>,
    external_scan_failed_at: Option<Instant>,
}

pub(in crate::app) struct CachedValue<'a> {
    pub computed_at: Instant,
    pub value: &'a Value,
}

pub(in crate::app) struct ExternalScanRead<'a> {
    pub worktrees: &'a [ExternalWorktree],
    pub settled_at: Option<Instant>,
    pub has_readable_scan: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::app) enum CacheEffect {
    BoardChanged,
}

pub(in crate::app) enum CachePublication {
    Settled(Vec<CacheEffect>),
    RunStatChanged(PendingRunStat),
}

#[cfg(test)]
impl CachePublication {
    pub(in crate::app) fn is_empty(&self) -> bool {
        matches!(self, Self::Settled(effects) if effects.is_empty())
    }
}

#[cfg(test)]
impl PartialEq<Vec<CacheEffect>> for CachePublication {
    fn eq(&self, effects: &Vec<CacheEffect>) -> bool {
        matches!(self, Self::Settled(actual) if actual == effects)
    }
}

#[cfg(test)]
impl std::fmt::Debug for CachePublication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Settled(effects) => formatter.debug_tuple("Settled").field(effects).finish(),
            Self::RunStatChanged(_) => formatter.write_str("RunStatChanged(..)"),
        }
    }
}

/// A changed run stat whose pre-insertion EntitySettled notification is still
/// owed by the adapter. Fields stay opaque so only DiffCache can commit it.
pub(in crate::app) struct PendingRunStat {
    run_id: String,
    stat: Value,
    now: Instant,
    files_changed_at: String,
}

impl PendingRunStat {
    pub(in crate::app) fn run_id(&self) -> &str {
        &self.run_id
    }
}

/// Opaque custody of one in-flight cache key. The runtime moves this token out
/// of the job before blocking work and passes it to existing apply/abandon paths.
pub(in crate::app) struct RefreshClaim {
    key: DiffCacheKey,
}

#[derive(Default)]
pub(in crate::app) struct DiffCache {
    run_stat_cache: HashMap<String, (Instant, Value)>,
    run_files_changed_at: HashMap<String, String>,
    project_cache: HashMap<String, ProjectCache>,
    workspace_summary_membership: HashMap<String, Vec<std::path::PathBuf>>,
    workspace_summary_cache: HashMap<String, (Instant, Vec<std::path::PathBuf>, Value)>,
    diff_refreshes_in_flight: HashSet<DiffCacheKey>,
    diff_refreshes_superseded: HashSet<DiffCacheKey>,
}

impl DiffCache {
    pub(in crate::app) fn register_project(&mut self, project_id: String) {
        self.project_cache.entry(project_id).or_default();
    }

    /// Removes only project-owned cache values. In-flight/superseded claims stay
    /// held until their existing computation settles.
    pub(in crate::app) fn remove_project(&mut self, project_id: &str) {
        self.project_cache.remove(project_id);
    }

    pub(in crate::app) fn run_stat(&self, run_id: &str) -> Option<CachedValue<'_>> {
        self.run_stat_cache
            .get(run_id)
            .map(|(computed_at, value)| CachedValue {
                computed_at: *computed_at,
                value,
            })
    }

    #[cfg(test)]
    pub(in crate::app) fn run_files_changed_at(&self, run_id: &str) -> Option<&str> {
        self.run_files_changed_at.get(run_id).map(String::as_str)
    }

    pub(in crate::app) fn workspace_summary(
        &self,
        workspace_id: &str,
        repositories: &[std::path::PathBuf],
    ) -> Option<CachedValue<'_>> {
        self.workspace_summary_cache
            .get(workspace_id)
            .filter(|(_, cached_repositories, _)| cached_repositories == repositories)
            .map(|(computed_at, _, value)| CachedValue {
                computed_at: *computed_at,
                value,
            })
    }

    pub(in crate::app) fn sync_workspace_summaries(
        &mut self,
        memberships: &[(String, Vec<std::path::PathBuf>)],
    ) {
        let current = memberships.iter().cloned().collect::<HashMap<_, _>>();
        self.workspace_summary_cache
            .retain(|workspace_id, (_, repositories, _)| {
                current.get(workspace_id) == Some(repositories)
            });
        let obsolete = self
            .diff_refreshes_in_flight
            .iter()
            .filter(|key| match key {
                DiffCacheKey::WorkspaceSummary(workspace_id, repositories) => {
                    current.get(workspace_id) != Some(repositories)
                }
                _ => false,
            })
            .cloned()
            .collect::<Vec<_>>();
        for key in obsolete {
            self.supersede(&key);
        }
        self.workspace_summary_membership = current;
    }

    pub(in crate::app) fn external_scan(&self, project_id: &str) -> ExternalScanRead<'_> {
        let project = self.project_cache.get(project_id);
        let worktrees = project
            .and_then(|cache| cache.external_scan.as_ref())
            .map(|scan| scan.worktrees.as_slice())
            .unwrap_or_default();
        ExternalScanRead {
            worktrees,
            has_readable_scan: project
                .and_then(|cache| cache.external_scan.as_ref())
                .is_some(),
            settled_at: project.and_then(|cache| {
                cache
                    .external_scan
                    .as_ref()
                    .map(|scan| scan.scanned_at)
                    .max(cache.external_scan_failed_at)
            }),
        }
    }

    pub(in crate::app) fn external_scan_cache(
        &self,
        project_id: &str,
    ) -> Option<&ExternalScanCache> {
        self.project_cache
            .get(project_id)
            .and_then(|cache| cache.external_scan.as_ref())
    }

    pub(in crate::app) fn claim_refresh(&mut self, key: DiffCacheKey) -> Option<RefreshClaim> {
        self.diff_refreshes_in_flight
            .insert(key.clone())
            .then_some(RefreshClaim { key })
    }

    pub(in crate::app) fn supersede(&mut self, key: &DiffCacheKey) {
        if self.diff_refreshes_in_flight.contains(key) {
            self.diff_refreshes_superseded.insert(key.clone());
        }
    }

    pub(in crate::app) fn publish_refresh(
        &mut self,
        claim: RefreshClaim,
        entry: Option<DiffCacheEntry>,
        now: Instant,
    ) -> CachePublication {
        let key = claim.key;
        let claimed = self.diff_refreshes_in_flight.remove(&key);
        let superseded = self.diff_refreshes_superseded.remove(&key);
        if !claimed || superseded {
            return CachePublication::Settled(Vec::new());
        }
        match entry {
            Some(DiffCacheEntry::RunStat { run_id, stat }) => {
                self.prepare_run_stat(run_id, stat, now)
            }
            Some(entry) => CachePublication::Settled(self.store_entry_for_adapter(entry, now)),
            None => CachePublication::Settled(Vec::new()),
        }
    }

    /// Commit only after the adapter has emitted EntitySettled for this run.
    pub(in crate::app) fn commit_changed_run_stat(&mut self, pending: PendingRunStat) {
        self.run_files_changed_at
            .insert(pending.run_id.clone(), pending.files_changed_at);
        self.run_stat_cache
            .insert(pending.run_id, (pending.now, pending.stat));
    }

    pub(in crate::app) fn prepare_run_stat(
        &mut self,
        run_id: String,
        stat: Value,
        now: Instant,
    ) -> CachePublication {
        if self
            .run_stat_cache
            .get(&run_id)
            .is_some_and(|(_, previous)| previous != &stat)
        {
            CachePublication::RunStatChanged(PendingRunStat {
                run_id,
                stat,
                now,
                files_changed_at: now_rfc3339(),
            })
        } else {
            CachePublication::Settled(self.store_run_stat_for_adapter(run_id, stat, now))
        }
    }

    pub(in crate::app) fn release_refresh(&mut self, claim: RefreshClaim) {
        self.diff_refreshes_in_flight.remove(&claim.key);
        self.diff_refreshes_superseded.remove(&claim.key);
    }

    #[cfg(test)]
    pub(in crate::app) fn refresh_is_running(&self, key: &DiffCacheKey) -> bool {
        self.diff_refreshes_in_flight.contains(key)
    }

    #[cfg(test)]
    pub(in crate::app) fn refresh_is_superseded(&self, key: &DiffCacheKey) -> bool {
        self.diff_refreshes_superseded.contains(key)
    }

    #[cfg(test)]
    pub(in crate::app) fn has_run_stat(&self, run_id: &str) -> bool {
        self.run_stat_cache.contains_key(run_id)
    }

    #[cfg(test)]
    pub(in crate::app) fn clear_run_stats(&mut self) {
        self.run_stat_cache.clear();
    }

    #[cfg(test)]
    pub(in crate::app) fn seed_run_stat(&mut self, run_id: String, stat: Value) {
        self.run_stat_cache.insert(run_id, (Instant::now(), stat));
    }

    #[cfg(test)]
    pub(in crate::app) fn clear_external_scan(&mut self, project_id: &str) {
        self.project_cache
            .get_mut(project_id)
            .expect("registered project cache")
            .external_scan = None;
    }

    #[cfg(test)]
    pub(in crate::app) fn age_workspace_summary(
        &mut self,
        workspace_id: &str,
        age: std::time::Duration,
    ) {
        if let Some((computed_at, _, _)) = self.workspace_summary_cache.get_mut(workspace_id) {
            *computed_at -= age;
        }
    }

    #[cfg(test)]
    pub(in crate::app) fn age_external_scan(&mut self, project_id: &str, age: std::time::Duration) {
        self.project_cache
            .get_mut(project_id)
            .and_then(|project| project.external_scan.as_mut())
            .expect("a scan to age")
            .scanned_at -= age;
    }

    pub(in crate::app) fn store_entry_for_adapter(
        &mut self,
        entry: DiffCacheEntry,
        now: Instant,
    ) -> Vec<CacheEffect> {
        match entry {
            DiffCacheEntry::RunStat { run_id, stat } => {
                self.store_run_stat_for_adapter(run_id, stat, now)
            }
            DiffCacheEntry::ExternalScan {
                project_id,
                worktrees,
            } => self.store_external_scan(&project_id, worktrees, now),
            DiffCacheEntry::ExternalScanUnreadable { project_id } => {
                self.store_scan_failure(&project_id, now)
            }
            DiffCacheEntry::WorkspaceSummary {
                workspace_id,
                repositories,
                summary,
            } => {
                if self.workspace_summary_membership.get(&workspace_id) != Some(&repositories) {
                    return Vec::new();
                }
                let changed = self.workspace_summary_cache.get(&workspace_id).is_none_or(
                    |(_, previous_repositories, previous)| {
                        previous_repositories != &repositories || previous != &summary
                    },
                );
                self.workspace_summary_cache
                    .insert(workspace_id, (now, repositories, summary));
                if changed {
                    vec![CacheEffect::BoardChanged]
                } else {
                    Vec::new()
                }
            }
        }
    }

    pub(in crate::app) fn store_run_stat_for_adapter(
        &mut self,
        run_id: String,
        stat: Value,
        now: Instant,
    ) -> Vec<CacheEffect> {
        let first = !self.run_stat_cache.contains_key(&run_id);
        self.run_stat_cache.insert(run_id, (now, stat));
        if first {
            vec![CacheEffect::BoardChanged]
        } else {
            Vec::new()
        }
    }

    fn store_external_scan(
        &mut self,
        project_id: &str,
        worktrees: Vec<ExternalWorktree>,
        now: Instant,
    ) -> Vec<CacheEffect> {
        let Some(project) = self.project_cache.get_mut(project_id) else {
            return Vec::new();
        };
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
            vec![CacheEffect::BoardChanged]
        } else {
            Vec::new()
        }
    }

    fn store_scan_failure(&mut self, project_id: &str, now: Instant) -> Vec<CacheEffect> {
        let Some(project) = self.project_cache.get_mut(project_id) else {
            return Vec::new();
        };
        let settling = project.external_scan_failed_at.is_none();
        project.external_scan_failed_at = Some(now);
        if settling {
            vec![CacheEffect::BoardChanged]
        } else {
            Vec::new()
        }
    }

    pub(in crate::app) fn invalidate_run_stat(&mut self, run_id: &str) {
        self.run_stat_cache.remove(run_id);
        self.supersede(&DiffCacheKey::RunStat(run_id.to_string()));
    }

    pub(in crate::app) fn remove_run_files_changed_at(&mut self, run_id: &str) {
        self.run_files_changed_at.remove(run_id);
    }

    /// Exact-path upsert: remove only the first equal path, push, then apply the
    /// existing checkout sorter. No deduplication or canonicalization.
    pub(in crate::app) fn note_worktree_appeared(
        &mut self,
        project_id: &str,
        worktree: ExternalWorktree,
    ) -> Vec<CacheEffect> {
        let Some(scan) = self
            .project_cache
            .get_mut(project_id)
            .and_then(|project| project.external_scan.as_mut())
        else {
            return Vec::new();
        };
        let replaced = scan
            .worktrees
            .iter()
            .position(|known| known.path == worktree.path)
            .map(|index| scan.worktrees.remove(index));
        let changed = replaced.as_ref() != Some(&worktree);
        scan.worktrees.push(worktree);
        crate::worktree::sort_checkouts(&mut scan.worktrees);
        if !changed {
            return Vec::new();
        }
        self.supersede(&DiffCacheKey::ExternalScan(project_id.to_string()));
        vec![CacheEffect::BoardChanged]
    }

    /// The adapter supplies the already-canonical path. No filesystem work is
    /// performed by this component.
    pub(in crate::app) fn note_worktree_gone(
        &mut self,
        project_id: &str,
        canonical_path: &Path,
    ) -> Vec<CacheEffect> {
        let Some(scan) = self
            .project_cache
            .get_mut(project_id)
            .and_then(|project| project.external_scan.as_mut())
        else {
            return Vec::new();
        };
        let before = scan.worktrees.len();
        scan.worktrees.retain(|known| known.path != canonical_path);
        if before == scan.worktrees.len() {
            return Vec::new();
        }
        self.supersede(&DiffCacheKey::ExternalScan(project_id.to_string()));
        vec![CacheEffect::BoardChanged]
    }
}
