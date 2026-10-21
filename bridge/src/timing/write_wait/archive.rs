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
        // Test-first scaffold: completed rows are not consumed yet.
        Ok(Vec::new())
    }
    pub(crate) fn completed_intervals_since(
        &self,
        cursor: &mut CompletedIntervalsCursor,
        limit: usize,
    ) -> Result<Vec<CompletedInterval>, ArchiveCursorError> {
        let state = self.writer.lock().unwrap();
        cursor.0.validate(self, state.raw_intervals.len(), limit)?;
        // Test-first scaffold: completed intervals are not consumed yet.
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests;
