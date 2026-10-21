//! Close every real wire and join every receiver before asserting errors.
use super::*;

impl RpcHarness {
    pub async fn finish(self) {
        let report = self.finish_report().await;
        assert!(report.is_clean(), "RPC cleanup failed: {report:?}");
    }

    pub async fn finish_report(self) -> RpcFinishReport {
        let mut report = RpcFinishReport {
            errors: Vec::new(),
            pending_at_close: self.pending(),
            pending_after_join: Vec::new(),
            joined_receivers: 0,
            watch_before_cleanup: None,
            watch_after_cleanup: None,
        };
        let mut receivers = Vec::new();
        let mut evidence = Vec::new();
        // Close ALL openings before awaiting any receiver. No assertion or
        // individual error can skip cleanup of a later opening.
        for opening in self.openings {
            match Arc::try_unwrap(opening.wire) {
                Ok(wire) => wire.close(&self.intake),
                Err(_) => report
                    .errors
                    .push(format!("{}: wire has unjoined senders", opening.id)),
            }
            evidence.push((opening.waiting, opening.receive_errors));
            receivers.push((opening.id, opening.receiver));
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        for (id, mut receiver) in receivers {
            match tokio::time::timeout_at(deadline, &mut receiver).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => report
                    .errors
                    .push(format!("{id}: receiver join failed: {error}")),
                Err(_) => {
                    report.errors.push(format!("{id}: receiver timed out after wire close; stopping owned outbound consumer, accepted server work is not cancelled"));
                    receiver.abort();
                    if let Err(error) = receiver.await {
                        report
                            .errors
                            .push(format!("{id}: stopped receiver joined: {error}"));
                    }
                }
            }
            report.joined_receivers += 1;
        }
        for (waiting, errors) in evidence {
            report.pending_after_join.extend(
                waiting
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .keys()
                    .cloned(),
            );
            report.errors.extend(
                errors
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .iter()
                    .cloned(),
            );
        }
        // Archive only boundary timestamps in one final batch, not full replies
        // or a collector snapshot on every read. Teardown already completed.
        let attached = self
            .watch
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(watch) = attached {
            let imported = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                attach_registry_receipts(&self.registry, &self.offered, &watch)
            }));
            match imported {
                Ok(errors) => report.errors.extend(errors),
                Err(_) => report
                    .errors
                    .push("actual registry timestamp attachment panicked".into()),
            }
        }
        report
    }

    pub(super) fn pending(&self) -> Vec<String> {
        self.openings
            .iter()
            .flat_map(|opening| {
                opening
                    .waiting
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

fn attach_registry_receipts(
    registry: &WriteWaitRegistry,
    offered: &Mutex<BTreeSet<String>>,
    watch: &ClientWatch,
) -> Vec<String> {
    let offered = offered
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let mut imported = BTreeSet::new();
    let mut errors = Vec::new();
    for row in registry.snapshot() {
        if !offered.contains(&row.original_id) {
            continue;
        }
        let Some(schedule) = watch.schedule_lookup(&row.original_id) else {
            continue;
        };
        if schedule.intended_at.is_none() {
            continue;
        }
        if !imported.insert(row.original_id.clone()) {
            errors.push(format!("multiple server origins for {}", row.original_id));
            continue;
        }
        watch.server_registered_at(&row.original_id, row.intake);
        if let Some(admitted_at) = row.admitted_at {
            watch.admitted_at(&row.original_id, admitted_at);
        }
    }
    errors
}
