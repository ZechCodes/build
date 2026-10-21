//! Bounded finite-trial archive export; never used by the active watchdog.
use super::*;
use std::sync::Weak;

pub(crate) const MAX_ARCHIVE_BATCH: usize = 256;

struct Cursor {
    registry: Weak<WriteWaitRegistry>,
    next: usize,
}
pub(crate) struct CompletedSnapshotsCursor(Cursor);
pub(crate) struct CompletedIntervalsCursor(Cursor);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArchiveCursorError {
    ForeignRegistry,
    InvalidPosition,
    EmptyBatchLimit,
    OversizedBatchLimit,
}
impl Cursor {
    fn new(registry: &Arc<WriteWaitRegistry>) -> Self {
        Self {
            registry: Arc::downgrade(registry),
            next: 0,
        }
    }
    fn take_batch<T: Clone>(&mut self, rows: &[T], limit: usize) -> Vec<T> {
        let end = self.next.saturating_add(limit).min(rows.len());
        let batch = rows[self.next..end].to_vec();
        self.next = end;
        batch
    }
    fn validate(
        &self,
        registry: &WriteWaitRegistry,
        length: usize,
        limit: usize,
    ) -> Result<(), ArchiveCursorError> {
        if !std::ptr::eq(self.registry.as_ptr(), registry) {
            return Err(ArchiveCursorError::ForeignRegistry);
        }
        if self.next > length {
            return Err(ArchiveCursorError::InvalidPosition);
        }
        if limit == 0 {
            return Err(ArchiveCursorError::EmptyBatchLimit);
        }
        if limit > MAX_ARCHIVE_BATCH {
            return Err(ArchiveCursorError::OversizedBatchLimit);
        }
        Ok(())
    }
}
impl WriteWaitRegistry {
    pub(crate) fn completed_snapshots_cursor(self: &Arc<Self>) -> CompletedSnapshotsCursor {
        CompletedSnapshotsCursor(Cursor::new(self))
    }
    pub(crate) fn completed_intervals_cursor(self: &Arc<Self>) -> CompletedIntervalsCursor {
        CompletedIntervalsCursor(Cursor::new(self))
    }
    pub(crate) fn completed_snapshots_since(
        &self,
        cursor: &mut CompletedSnapshotsCursor,
        limit: usize,
    ) -> Result<Vec<WriteSnapshot>, ArchiveCursorError> {
        let state = self.writer.lock().unwrap();
        cursor.0.validate(self, state.archive.len(), limit)?;
        Ok(cursor.0.take_batch(&state.archive, limit))
    }
    pub(crate) fn completed_intervals_since(
        &self,
        cursor: &mut CompletedIntervalsCursor,
        limit: usize,
    ) -> Result<Vec<CompletedInterval>, ArchiveCursorError> {
        let state = self.writer.lock().unwrap();
        cursor.0.validate(self, state.raw_intervals.len(), limit)?;
        Ok(cursor.0.take_batch(&state.raw_intervals, limit))
    }
}

#[cfg(test)]
mod tests;
