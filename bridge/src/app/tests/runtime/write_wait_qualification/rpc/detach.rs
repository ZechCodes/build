//! Explicit final audit before auxiliary native cleanup requests.
use super::*;

mod tests;

impl RpcHarness {
    /// Call after trial producers and their watchdog are joined. Failures are
    /// retained in reconciliation; a failed final reply is still terminal.
    pub(in crate::app::tests::runtime::write_wait_qualification) fn detach_client_watch(
        &self,
    ) -> Result<Option<Arc<FinalReconciliation>>, Vec<String>> {
        let mut attachment = self.watch.lock().unwrap();
        if let Some(saved) = &attachment.detached {
            return Ok(Some(Arc::clone(&saved.before_cleanup)));
        }
        let Some(watch) = attachment.active.clone() else {
            return Ok(None);
        };
        if attachment.receiving != 0 {
            return Err(vec![format!(
                "RPC receivers still in flight: {}",
                attachment.receiving
            )]);
        }
        let mut errors = Vec::new();
        let pending = self.pending();
        if !pending.is_empty() {
            errors.push(format!("RPC originals still pending: {pending:?}"));
        }
        let unfinished = watch.unfinished_final_originals();
        if !unfinished.is_empty() {
            errors.push(format!(
                "watch originals lack terminal receipts: {unfinished:?}"
            ));
        }
        for row in self.registry.snapshot() {
            if watch.contains(&row.original_id)
                && (!row.root_finished
                    || row.completed_at.is_none()
                    || !row.outstanding.is_empty()
                    || !row.active.is_empty())
            {
                errors.push(format!("server original still active: {}", row.original_id));
            }
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        let receipt_errors =
            cleanup::import_registry_receipts(&self.registry, &self.offered, &watch);
        let mut audit = watch.reconcile_final();
        audit.violations.extend(receipt_errors.iter().cloned());
        let before_cleanup = Arc::new(audit);
        attachment.detached = Some(DetachedWatch {
            watch: watch.clone(),
            before_cleanup: Arc::clone(&before_cleanup),
            receipt_errors,
        });
        attachment.active.take();
        Ok(Some(before_cleanup))
    }
}
