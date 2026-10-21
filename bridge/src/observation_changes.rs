//! Producer dirty keys. Snapshots own no candidate values and survive refusals.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

#[derive(Debug, Default)]
pub(crate) struct ChangeKeys {
    sequence: AtomicU64,
    pending: Mutex<BTreeMap<String, u64>>,
}

impl ChangeKeys {
    pub(crate) fn mark(&self, key: impl Into<String>) {
        let mut pending = crate::timing::resource_lock!(self.pending).unwrap();
        let revision = self.sequence.fetch_add(1, Ordering::Relaxed) + 1;
        pending.insert(key.into(), revision);
    }

    pub(crate) fn snapshot(&self) -> Vec<(String, u64)> {
        crate::timing::resource_lock!(self.pending)
            .unwrap()
            .iter()
            .map(|(key, revision)| (key.clone(), *revision))
            .collect()
    }

    pub(crate) fn acknowledge(&self, captured: &[(String, u64)]) {
        let mut pending = crate::timing::resource_lock!(self.pending).unwrap();
        for (key, revision) in captured {
            if pending.get(key) == Some(revision) {
                pending.remove(key);
            }
        }
    }
}

#[cfg(test)]
mod wait_tests;
