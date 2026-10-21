//! Attach only after completed bootstrap, retaining every prior original ID.
use super::*;

pub(super) fn install(harness: &mut RpcHarness, watch: ClientWatch) {
    let occupied = {
        let attachment = harness.watch.lock().unwrap();
        attachment.active.is_some() || attachment.detached.is_some()
    };
    assert!(!occupied, "one watch per RPC harness");
    let offered = harness.offered.lock().unwrap().clone();
    let errors = bootstrap_errors(harness, &offered, &watch);
    // Every inspection guard is gone before rejection. A caught invalid attach
    // cannot poison attachment/history locks or skip later receiver cleanup.
    assert!(
        errors.is_empty(),
        "bootstrap is not ready for trial watch attachment: {errors:?}"
    );
    harness.watch.lock().unwrap().active = Some(watch);
}

fn bootstrap_errors(
    harness: &RpcHarness,
    offered: &BTreeSet<String>,
    watch: &ClientWatch,
) -> Vec<String> {
    let mut errors = Vec::new();
    for id in offered {
        if watch.contains(id) {
            errors.push(format!(
                "prior bootstrap original is already in the trial watch: {id}"
            ));
        }
    }
    for opening in &harness.openings {
        inspect_receiver(opening, &mut errors);
    }
    // Actual reads may have no registry origin. Successful final transport is
    // proved by offered-minus-pending and clean receivers, not a fabricated
    // registry row. Every matching real origin that does exist must be closed.
    for row in harness.registry.snapshot() {
        if offered.contains(&row.original_id) && !terminal(&row) {
            errors.push(format!(
                "bootstrap server origin is not terminal: {} ({:?})",
                row.original_id, row.id
            ));
        }
    }
    errors
}

fn inspect_receiver(opening: &Opening, errors: &mut Vec<String>) {
    let pending: Vec<_> = opening.waiting.lock().unwrap().keys().cloned().collect();
    if !pending.is_empty() {
        errors.push(format!(
            "{}: bootstrap originals still pending: {pending:?}",
            opening.id
        ));
    }
    let received = opening.receive_errors.lock().unwrap().clone();
    if !received.is_empty() {
        errors.push(format!(
            "{}: bootstrap receiver errors: {received:?}",
            opening.id
        ));
    }
    if opening.receiver.is_finished() {
        errors.push(format!(
            "{}: bootstrap receiver no longer running",
            opening.id
        ));
    }
}

fn terminal(row: &crate::timing::write_wait::WriteSnapshot) -> bool {
    row.root_finished
        && row.outstanding.is_empty()
        && row.active.is_empty()
        && !row.cancelled
        && !row.abandoned
}
