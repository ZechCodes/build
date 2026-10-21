//! TEST-FIRST consumer boundary over actual bounded completion cursor rows.
//! No completion proof is produced until these native controls establish RED.
use crate::timing::write_wait::{WriteId, WriteSnapshot};
use std::sync::Arc;
use std::time::Instant;

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

/// Empty TEST-FIRST scaffold. The later implementation may index one checked
/// result per immutable expected original; it must not add a registry or clock.
pub(super) struct CompletedOrigins;

impl CompletedOrigins {
    pub(super) fn new(
        _expected: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self))
    }

    pub(super) fn import(&self, _row: &WriteSnapshot) -> Result<(), String> {
        Err("completion consumer TESTFIRST pending".into())
    }

    pub(super) async fn wait(
        &self,
        _original: &str,
        _cleanup_deadline: tokio::time::Instant,
    ) -> Result<NativeCompletion, String> {
        Err("completion consumer TESTFIRST pending".into())
    }

    pub(super) fn audit(&self) -> Vec<String> {
        vec!["completion consumer TESTFIRST pending".into()]
    }
}

#[cfg(test)]
#[path = "completed_origins/tests.rs"]
mod tests;
