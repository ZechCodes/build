mod attention;
mod cache;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::attention::Attention;
use crate::store::PersistedArchivedWorktree;

pub(in crate::app) use attention::AttentionIndex;
#[cfg(test)]
pub(in crate::app) use attention::EntityClock;
pub(in crate::app) use cache::{
    CacheEffect, CachePublication, DiffCache, DiffCacheEntry, DiffCacheKey, ExternalScanCache,
    RefreshClaim,
};

/// Board-owned indexes and caches. Callers supply already-resolved domain facts;
/// filesystem work, persistence, notifications, and ChangeBus publication stay
/// in the application/runtime adapters.
pub(in crate::app) struct BoardIndex {
    attention: AttentionIndex,
    diff: DiffCache,
    archived_worktrees: HashMap<String, PersistedArchivedWorktree>,
}

impl BoardIndex {
    pub(in crate::app) fn new(
        attention: HashMap<String, Attention>,
        archived_worktrees: HashMap<String, PersistedArchivedWorktree>,
    ) -> Self {
        Self {
            attention: AttentionIndex::new(attention),
            diff: DiffCache::default(),
            archived_worktrees,
        }
    }

    pub(in crate::app) fn attention(&self) -> &AttentionIndex {
        &self.attention
    }

    pub(in crate::app) fn attention_mut(&mut self) -> &mut AttentionIndex {
        &mut self.attention
    }

    pub(in crate::app) fn diff(&self) -> &DiffCache {
        &self.diff
    }

    pub(in crate::app) fn diff_mut(&mut self) -> &mut DiffCache {
        &mut self.diff
    }

    pub(in crate::app) fn archived(&self, worktree_id: &str) -> Option<&PersistedArchivedWorktree> {
        self.archived_worktrees.get(worktree_id)
    }

    pub(in crate::app) fn archived_values(
        &self,
    ) -> impl Iterator<Item = &PersistedArchivedWorktree> {
        self.archived_worktrees.values()
    }

    pub(in crate::app) fn insert_archived(
        &mut self,
        record: PersistedArchivedWorktree,
    ) -> Option<PersistedArchivedWorktree> {
        self.archived_worktrees
            .insert(record.worktree_id.clone(), record)
    }

    /// Boot hydration replaces only the durable archive index, after Store has
    /// been installed. Attention clocks, caches, and refresh claims are untouched.
    pub(in crate::app) fn replace_archived(
        &mut self,
        archived: HashMap<String, PersistedArchivedWorktree>,
    ) {
        self.archived_worktrees = archived;
    }
}
