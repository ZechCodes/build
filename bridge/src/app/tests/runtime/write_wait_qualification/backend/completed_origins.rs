//! One checked result per immutable expected original, from native terminal cursors.
use crate::timing::write_wait::{WriteCohort, WriteId, WriteSnapshot};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::Notify;

/// Only this module may construct a checked native completion. Keep the
/// original native identity/method/timestamp, without copying an archive row.
#[derive(Clone, Debug)]
pub(super) struct NativeCompletion {
    original_id: String,
    method: String,
    write_id: WriteId,
    completed_at: Instant,
}

impl NativeCompletion {
    pub(super) fn original_id(&self) -> &str {
        &self.original_id
    }

    pub(super) fn method(&self) -> &str {
        &self.method
    }

    pub(super) fn write_id(&self) -> WriteId {
        self.write_id
    }

    pub(super) fn completed_at(&self) -> Instant {
        self.completed_at
    }
}

struct CompletionSlot {
    method: String,
    result: Mutex<Option<Result<NativeCompletion, String>>>,
    ready: Notify,
}

pub(super) struct CompletedOrigins {
    slots: BTreeMap<String, CompletionSlot>,
}

impl CompletedOrigins {
    pub(super) fn new(
        expected: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Arc<Self>, String> {
        let mut slots = BTreeMap::new();
        for (original, method) in expected {
            let slot = CompletionSlot {
                method,
                result: Mutex::new(None),
                ready: Notify::new(),
            };
            if slots.insert(original.clone(), slot).is_some() {
                return Err(format!("duplicate_expected_original: {original}"));
            }
        }
        Ok(Arc::new(Self { slots }))
    }

    pub(super) fn import(&self, row: &WriteSnapshot) -> Result<(), String> {
        let Some(slot) = self.slots.get(&row.original_id) else {
            // Background and unplanned rows remain in the native raw archive.
            return Ok(());
        };
        let mut stored = slot
            .result
            .lock()
            .map_err(|_| format!("completion_state_poisoned: {}", row.original_id))?;
        let result = if stored.is_some() {
            Err(format!(
                "duplicate_native_completion: {} native {:?}",
                row.original_id, row.id
            ))
        } else {
            checked_completion(row, &slot.method)
        };
        let status = result.as_ref().map(|_| ()).map_err(Clone::clone);
        // Construct at most one success. A replay or late distinct root replaces
        // success with a permanent failure, including after a waiter copied it.
        *stored = Some(result);
        drop(stored);
        slot.ready.notify_waiters();
        status
    }

    pub(super) async fn wait(
        &self,
        original: &str,
        cleanup_deadline: tokio::time::Instant,
    ) -> Result<NativeCompletion, String> {
        let slot = self
            .slots
            .get(original)
            .ok_or_else(|| format!("unknown_expected_original: {original}"))?;
        loop {
            let notified = slot.ready.notified();
            tokio::pin!(notified);
            // Register before checking the slot so import cannot lose a wake.
            notified.as_mut().enable();
            if let Some(result) = slot
                .result
                .lock()
                .map_err(|_| format!("completion_state_poisoned: {original}"))?
                .clone()
            {
                return result;
            }
            if tokio::time::timeout_at(cleanup_deadline, notified)
                .await
                .is_err()
            {
                // A missed deadline is not cancellation or a permanent missing
                // result: owned native work can still finish during cleanup.
                return Err(format!("missing_native_completion: {original}"));
            }
        }
    }

    pub(super) fn audit(&self) -> Vec<String> {
        self.slots
            .iter()
            .filter_map(|(original, slot)| match slot.result.lock() {
                Ok(stored) => match stored.as_ref() {
                    Some(Ok(_)) => None,
                    Some(Err(error)) => Some(error.clone()),
                    None => Some(format!("missing_native_completion: {original}")),
                },
                Err(_) => Some(format!("completion_state_poisoned: {original}")),
            })
            .collect()
    }
}

fn checked_completion(
    row: &WriteSnapshot,
    expected_method: &str,
) -> Result<NativeCompletion, String> {
    if row.method != expected_method {
        return Err(format!(
            "native_method_mismatch: {} expected {expected_method}, actual {}",
            row.original_id, row.method
        ));
    }
    if row.cohort != WriteCohort::SemanticWrite {
        return Err(format!(
            "native_cohort_mismatch: {} actual {:?}",
            row.original_id, row.cohort
        ));
    }
    if row.cancelled {
        return Err(format!("cancelled_native_completion: {}", row.original_id));
    }
    if row.abandoned {
        return Err(format!("abandoned_native_completion: {}", row.original_id));
    }
    if !row.root_finished || !row.outstanding.is_empty() || !row.active.is_empty() {
        return Err(format!("unfinished_native_completion: {}", row.original_id));
    }
    let completed_at = row.completed_at.ok_or_else(|| {
        format!(
            "unfinished_native_completion: {} missing terminal timestamp",
            row.original_id
        )
    })?;
    Ok(NativeCompletion {
        original_id: row.original_id.clone(),
        method: row.method.clone(),
        write_id: row.id,
        completed_at,
    })
}

#[cfg(test)]
#[path = "completed_origins/tests.rs"]
mod tests;
