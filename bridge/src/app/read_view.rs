//! Committed, immutable projections for intake reads and push facts.
//!
//! Projection work happens before publication. The CAS only copies the map of
//! shared chunks, merging independent producers against the latest root.

pub(in crate::app) mod history;

use arc_swap::ArcSwap;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::app) struct ReadKey {
    pub method: &'static str,
    pub owner: String,
}

impl ReadKey {
    fn new(method: &'static str, owner: impl Into<String>) -> Self {
        Self {
            method,
            owner: owner.into(),
        }
    }
}

#[derive(Clone)]
struct VersionedChunk {
    version: u64,
    value: Option<Arc<Value>>,
}

#[derive(Clone)]
struct VersionedHistory {
    version: u64,
    value: Option<Arc<history::SharedHistory>>,
}

#[derive(Clone, Default)]
pub(in crate::app) struct ReadView {
    chunks: BTreeMap<ReadKey, VersionedChunk>,
    histories: BTreeMap<String, VersionedHistory>,
    pub facts: Arc<super::facts::PushFacts>,
    pub facts_version: u64,
    pub observations: Option<Arc<super::observation_reads::ObservationContext>>,
    observations_version: u64,
}

impl ReadView {
    pub fn get(&self, method: &'static str, owner: &str) -> Option<&Value> {
        self.chunks
            .get(&ReadKey::new(method, owner))?
            .value
            .as_deref()
    }

    pub fn history(&self, agent_id: &str) -> Option<&history::SharedHistory> {
        self.histories.get(agent_id)?.value.as_deref()
    }

    pub fn history_keys(&self) -> impl Iterator<Item = &str> {
        self.histories
            .iter()
            .filter(|(_, value)| value.value.is_some())
            .map(|(key, _)| key.as_str())
    }

    pub fn keys(&self) -> impl Iterator<Item = &ReadKey> {
        self.chunks
            .iter()
            .filter(|(_, chunk)| chunk.value.is_some())
            .map(|(key, _)| key)
    }

    fn accepts(&self, delta: &SnapshotDelta) -> bool {
        delta.updates.keys().all(|key| {
            self.chunks
                .get(key)
                .is_none_or(|chunk| chunk.version < delta.version)
        }) && delta.histories.keys().all(|key| {
            self.histories
                .get(key)
                .is_none_or(|chunk| chunk.version < delta.version)
        }) && (delta.facts.is_none() || self.facts_version < delta.version)
            && (delta.observations.is_none() || self.observations_version < delta.version)
            && delta.observation_patch.as_ref().is_none_or(|patch| {
                self.observations
                    .as_ref()
                    .is_some_and(|context| patch.accepts(context, delta.version))
            })
    }

    fn apply(&mut self, delta: &SnapshotDelta) {
        for (key, value) in &delta.updates {
            self.chunks.insert(
                key.clone(),
                VersionedChunk {
                    version: delta.version,
                    value: value.clone(),
                },
            );
        }
        for (key, value) in &delta.histories {
            self.histories.insert(
                key.clone(),
                VersionedHistory {
                    version: delta.version,
                    value: value.clone(),
                },
            );
        }
        if let Some(observations) = &delta.observations {
            self.observations = Some(Arc::new(observations.with_base_version(delta.version)));
            self.observations_version = delta.version;
        }
        if let Some(patch) = &delta.observation_patch {
            if let Some(context) = &self.observations {
                self.observations = Some(Arc::new(patch.apply(context, delta.version)));
                self.observations_version = self.observations_version.max(delta.version);
            }
        }
        if let Some(facts) = &delta.facts {
            self.facts = Arc::clone(facts);
            self.facts_version = delta.version;
        }
    }
}

pub(in crate::app) struct SnapshotDelta {
    version: u64,
    updates: BTreeMap<ReadKey, Option<Arc<Value>>>,
    facts: Option<Arc<super::facts::PushFacts>>,
    histories: BTreeMap<String, Option<Arc<history::SharedHistory>>>,
    observations: Option<Arc<super::observation_reads::ObservationContext>>,
    observation_patch: Option<super::observation_reads::ObservationPatch>,
    observation_receipts: Option<super::observation_reads::ObservationReceipts>,
}

impl SnapshotDelta {
    pub fn new(version: u64) -> Self {
        Self {
            version,
            updates: BTreeMap::new(),
            facts: None,
            histories: BTreeMap::new(),
            observations: None,
            observation_patch: None,
            observation_receipts: None,
        }
    }

    pub fn put(mut self, method: &'static str, owner: impl Into<String>, value: Value) -> Self {
        self.updates
            .insert(ReadKey::new(method, owner), Some(Arc::new(value)));
        self
    }

    pub fn remove(mut self, method: &'static str, owner: impl Into<String>) -> Self {
        self.updates.insert(ReadKey::new(method, owner), None);
        self
    }

    pub fn put_history(self, agent_id: impl Into<String>, value: history::SharedHistory) -> Self {
        self.put_shared_history(agent_id, Arc::new(value))
    }

    pub fn put_shared_history(
        mut self,
        agent_id: impl Into<String>,
        value: Arc<history::SharedHistory>,
    ) -> Self {
        self.histories.insert(agent_id.into(), Some(value));
        self
    }

    pub fn remove_history(mut self, agent_id: impl Into<String>) -> Self {
        self.histories.insert(agent_id.into(), None);
        self
    }

    pub fn set_observations(
        mut self,
        observations: Arc<super::observation_reads::ObservationContext>,
    ) -> Self {
        self.observations = Some(observations);
        self
    }

    pub(in crate::app) fn patch_observations(
        mut self,
        patch: super::observation_reads::ObservationPatch,
    ) -> Self {
        if !patch.is_empty() {
            self.observation_patch = Some(patch);
        }
        self
    }
    pub(in crate::app) fn set_observation_receipts(
        mut self,
        receipts: super::observation_reads::ObservationReceipts,
    ) -> Self {
        self.observation_receipts = Some(receipts);
        self
    }
    pub(in crate::app) fn take_observation_receipts(
        &mut self,
    ) -> Option<super::observation_reads::ObservationReceipts> {
        self.observation_receipts.take()
    }

    pub fn set_facts(mut self, facts: Arc<super::facts::PushFacts>) -> Self {
        self.facts = Some(facts);
        self
    }
}

#[derive(Clone, Default)]
pub(in crate::app) struct ReadViewSlot {
    root: Arc<ArcSwap<ReadView>>,
    sequence: Arc<AtomicU64>,
}

impl ReadViewSlot {
    pub fn next_version(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn load(&self) -> Arc<ReadView> {
        self.root.load_full()
    }

    /// Rebuild an incremental committed update if another producer publishes
    /// while the candidate is being prepared. Builders must have no effects.
    pub fn publish_current(&self, builder: impl Fn(&ReadView, u64) -> Option<SnapshotDelta>) {
        loop {
            let current = self.root.load_full();
            let Some(delta) = builder(&current, self.next_version()) else {
                return;
            };
            if !current.accepts(&delta) {
                return;
            }
            let mut next = (*current).clone();
            next.apply(&delta);
            let replaced = self.root.compare_and_swap(&current, Arc::new(next));
            if Arc::ptr_eq(&current, &replaced) {
                return;
            }
        }
    }

    /// Publish every owner together or reject the entire stale candidate.
    /// Tombstones retain their versions, so old refreshes cannot resurrect rows.
    pub fn publish_delta(&self, delta: SnapshotDelta) -> bool {
        loop {
            let previous = self.root.load_full();
            if !previous.accepts(&delta) {
                return false;
            }
            let mut next = (*previous).clone();
            next.apply(&delta);
            let replaced = self.root.compare_and_swap(&previous, Arc::new(next));
            if Arc::ptr_eq(&previous, &replaced) {
                return true;
            }
        }
    }
}

mod service;

#[cfg(test)]
mod tests;
