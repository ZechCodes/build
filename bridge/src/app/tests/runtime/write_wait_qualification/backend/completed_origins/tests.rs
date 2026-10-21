//! Real registry lifecycles and bounded cursor output, not fabricated success.
//! The sole active-snapshot misuse control is explicitly identified below.
use super::*;
use crate::timing::write_wait::archive::CompletedSnapshotsCursor;
use crate::timing::write_wait::{IntervalCategory, WriteCohort, WriteWaitRegistry};
use std::task::Poll;
use std::time::Duration;

const RPC_METHOD: &str = "thread.post";
const MCP_METHOD: &str = "mcp.control:post_thread_message";

fn consumer(original: &str, method: &str) -> Arc<CompletedOrigins> {
    CompletedOrigins::new([(original.to_owned(), method.to_owned())]).unwrap()
}

fn one_native_row(
    registry: &WriteWaitRegistry,
    cursor: &mut CompletedSnapshotsCursor,
) -> WriteSnapshot {
    let mut rows = registry.completed_snapshots_since(cursor, 1).unwrap();
    assert_eq!(rows.len(), 1, "one real terminal cursor row required");
    rows.pop().unwrap()
}

fn immediate_deadline() -> tokio::time::Instant {
    tokio::time::Instant::now()
}

fn assert_diagnostic(error: &str, diagnostic: &str) {
    assert!(
        error.contains(diagnostic),
        "missing specific {diagnostic} diagnostic: {error}"
    );
}

#[tokio::test]
async fn completed_original_preserves_actual_cursor_identity_method_and_completion_before_wait() {
    for (original, method) in [("native-rpc", RPC_METHOD), ("native-mcp", MCP_METHOD)] {
        let registry = WriteWaitRegistry::new();
        let mut cursor = registry.completed_snapshots_cursor();
        let root = registry.begin(original, method, registry.now());
        let id = root.id();
        root.admitted();
        root.acknowledged();
        root.complete();
        let row = one_native_row(&registry, &mut cursor);
        assert_eq!(row.id, id);
        assert_eq!(row.cohort, WriteCohort::SemanticWrite);
        assert!(row.root_finished && row.outstanding.is_empty() && row.active.is_empty());
        let completed_at = row.completed_at.expect("actual native terminal timestamp");

        let observed = consumer(original, method);
        observed
            .import(&row)
            .expect("import actual cursor completion");
        let proof = observed.wait(original, immediate_deadline()).await.unwrap();
        assert_eq!(proof.original_id(), original);
        assert_eq!(proof.method(), method);
        assert_eq!(proof.write_id(), id);
        assert_eq!(proof.completed_at(), completed_at);
        assert!(observed.audit().is_empty());
        assert!(registry
            .completed_snapshots_since(&mut cursor, 1)
            .unwrap()
            .is_empty());
    }
}

#[tokio::test]
async fn completed_original_wakes_registered_waiter_only_after_actual_child_and_interval_finish() {
    let registry = WriteWaitRegistry::new();
    let mut cursor = registry.completed_snapshots_cursor();
    let root = registry.begin("native-later", RPC_METHOD, registry.now());
    let id = root.id();
    let child = root.child();
    let interval = child.interval(IntervalCategory::OwnExternal);
    root.complete();
    assert!(registry
        .completed_snapshots_since(&mut cursor, 1)
        .unwrap()
        .is_empty());
    let observed = consumer("native-later", RPC_METHOD);
    let waiting = observed.wait(
        "native-later",
        tokio::time::Instant::now() + Duration::from_secs(1),
    );
    tokio::pin!(waiting);
    let before_completion = futures_util::poll!(waiting.as_mut());
    // Release the actual tokens even when the empty seam returned prematurely.
    child.complete();
    assert!(registry
        .completed_snapshots_since(&mut cursor, 1)
        .unwrap()
        .is_empty());
    drop(interval);
    let row = one_native_row(&registry, &mut cursor);
    assert!(
        matches!(before_completion, Poll::Pending),
        "a proof waiter must remain pending until real native work completes: {before_completion:?}"
    );
    observed
        .import(&row)
        .expect("import terminal native row after waiter registration");
    let proof = waiting
        .await
        .expect("actual cursor import wakes existing waiter");
    assert_eq!(proof.write_id(), id);
    assert_eq!(Some(proof.completed_at()), row.completed_at);
    assert!(observed.audit().is_empty());
}

#[tokio::test]
async fn completed_original_rejects_real_native_method_mismatch_and_keeps_final_diagnostic() {
    let registry = WriteWaitRegistry::new();
    let mut cursor = registry.completed_snapshots_cursor();
    registry
        .begin("native-wrong-method", MCP_METHOD, registry.now())
        .complete();
    let row = one_native_row(&registry, &mut cursor);
    let observed = consumer("native-wrong-method", RPC_METHOD);
    let error = observed
        .import(&row)
        .expect_err("wrong native method cannot prove expected RPC");
    assert_diagnostic(&error, "native_method_mismatch");
    let error = observed
        .wait("native-wrong-method", immediate_deadline())
        .await
        .unwrap_err();
    assert_diagnostic(&error, "native_method_mismatch");
    assert_diagnostic(&observed.audit().join("; "), "native_method_mismatch");
}

#[tokio::test]
async fn completed_original_does_not_credit_foreign_original_or_background_cursor_rows() {
    let registry = WriteWaitRegistry::new();
    let mut cursor = registry.completed_snapshots_cursor();
    registry
        .begin("different-native-original", RPC_METHOD, registry.now())
        .complete();
    registry
        .begin_background("native-background", RPC_METHOD, registry.now())
        .complete();
    let rows = registry.completed_snapshots_since(&mut cursor, 2).unwrap();
    assert_eq!(
        rows.len(),
        2,
        "retain both actual native rows outside the consumer"
    );
    let observed = consumer("missing-native-original", RPC_METHOD);
    for row in &rows {
        observed
            .import(row)
            .expect("unplanned native rows remain raw evidence without client credit");
    }
    let error = observed
        .wait("missing-native-original", immediate_deadline())
        .await
        .unwrap_err();
    assert_diagnostic(&error, "missing_native_completion");
    assert_diagnostic(&observed.audit().join("; "), "missing_native_completion");
    let error = observed
        .wait("different-native-original", immediate_deadline())
        .await
        .unwrap_err();
    assert_diagnostic(&error, "unknown_expected_original");
}

#[tokio::test]
async fn completed_original_rejects_matching_original_from_actual_background_cohort() {
    let registry = WriteWaitRegistry::new();
    let mut cursor = registry.completed_snapshots_cursor();
    registry
        .begin_background("wrong-native-cohort", RPC_METHOD, registry.now())
        .complete();
    let row = one_native_row(&registry, &mut cursor);
    assert_eq!(row.cohort, WriteCohort::BackgroundWork);
    let observed = consumer("wrong-native-cohort", RPC_METHOD);
    let error = observed
        .import(&row)
        .expect_err("background cannot satisfy a semantic client original");
    assert_diagnostic(&error, "native_cohort_mismatch");
    assert_diagnostic(&observed.audit().join("; "), "native_cohort_mismatch");
}

#[tokio::test]
async fn completed_original_keeps_missing_proof_without_cancelling_live_native_work() {
    let registry = WriteWaitRegistry::new();
    let mut cursor = registry.completed_snapshots_cursor();
    let root = registry.begin("still-native-live", RPC_METHOD, registry.now());
    assert!(registry
        .completed_snapshots_since(&mut cursor, 1)
        .unwrap()
        .is_empty());
    let observed = consumer("still-native-live", RPC_METHOD);
    let error = observed
        .wait("still-native-live", immediate_deadline())
        .await
        .unwrap_err();
    let missing = observed.audit();
    root.complete();
    let row = one_native_row(&registry, &mut cursor);
    assert!(
        !row.cancelled && !row.abandoned,
        "missing proof never cancels native work"
    );
    assert_diagnostic(&error, "missing_native_completion");
    assert_diagnostic(&missing.join("; "), "missing_native_completion");
    observed
        .import(&row)
        .expect("late real completion remains importable within owned cleanup");
    let proof = observed
        .wait("still-native-live", immediate_deadline())
        .await
        .unwrap();
    assert_eq!(proof.write_id(), row.id);
    assert!(observed.audit().is_empty());
}

#[test]
fn completed_original_rejects_actual_unfinished_snapshot_misrouted_to_cursor_consumer() {
    let registry = WriteWaitRegistry::new();
    let mut cursor = registry.completed_snapshots_cursor();
    let root = registry.begin("native-unfinished", RPC_METHOD, registry.now());
    let child = root.child();
    let interval = child.interval(IntervalCategory::OwnExternal);
    // One explicit negative API misuse input, from the real active registry.
    // Production integration only passes terminal rows from its bounded cursor.
    let rows = registry.snapshot();
    assert_eq!(rows.len(), 1);
    assert!(
        !rows[0].root_finished && !rows[0].outstanding.is_empty() && !rows[0].active.is_empty()
    );
    assert!(rows[0].completed_at.is_none());
    assert!(registry
        .completed_snapshots_since(&mut cursor, 1)
        .unwrap()
        .is_empty());
    root.complete();
    child.complete();
    drop(interval);
    let completed = one_native_row(&registry, &mut cursor);
    assert!(completed.completed_at.is_some());
    let observed = consumer("native-unfinished", RPC_METHOD);
    let error = observed
        .import(&rows[0])
        .expect_err("actual unfinished input is not completion evidence");
    assert_diagnostic(&error, "unfinished_native_completion");
    assert_diagnostic(&observed.audit().join("; "), "unfinished_native_completion");
}

#[tokio::test]
async fn completed_original_rejects_actual_cancelled_terminal_cursor_row() {
    let registry = WriteWaitRegistry::new();
    let mut cursor = registry.completed_snapshots_cursor();
    registry
        .begin("native-cancelled", RPC_METHOD, registry.now())
        .cancel();
    let row = one_native_row(&registry, &mut cursor);
    assert!(row.cancelled && row.completed_at.is_some());
    assert_terminal_rejected("native-cancelled", &row, "cancelled_native_completion").await;
}

#[tokio::test]
async fn completed_original_rejects_actual_abandoned_terminal_cursor_row() {
    let registry = WriteWaitRegistry::new();
    let mut cursor = registry.completed_snapshots_cursor();
    drop(registry.begin("native-abandoned", RPC_METHOD, registry.now()));
    let row = one_native_row(&registry, &mut cursor);
    assert!(row.abandoned && row.completed_at.is_some());
    assert_terminal_rejected("native-abandoned", &row, "abandoned_native_completion").await;
}

async fn assert_terminal_rejected(original: &str, row: &WriteSnapshot, diagnostic: &str) {
    let observed = consumer(original, RPC_METHOD);
    let error = observed
        .import(row)
        .expect_err("native failed terminal outcome is never success");
    assert_diagnostic(&error, diagnostic);
    let error = observed
        .wait(original, immediate_deadline())
        .await
        .unwrap_err();
    assert_diagnostic(&error, diagnostic);
    assert_diagnostic(&observed.audit().join("; "), diagnostic);
}

#[tokio::test]
async fn completed_original_late_second_native_root_invalidates_previously_consumed_proof() {
    let registry = WriteWaitRegistry::new();
    let mut cursor = registry.completed_snapshots_cursor();
    let observed = consumer("native-duplicate", RPC_METHOD);
    registry
        .begin("native-duplicate", RPC_METHOD, registry.now())
        .complete();
    let first = one_native_row(&registry, &mut cursor);
    observed.import(&first).unwrap();
    let earlier_proof = observed
        .wait("native-duplicate", immediate_deadline())
        .await
        .unwrap();
    assert_eq!(earlier_proof.write_id(), first.id);
    assert!(observed.audit().is_empty());

    registry
        .begin("native-duplicate", RPC_METHOD, registry.now())
        .complete();
    let later = one_native_row(&registry, &mut cursor);
    assert_ne!(first.id, later.id, "two actual independent native roots");
    let error = observed
        .import(&later)
        .expect_err("late native duplicate cannot be silently ignored");
    assert_diagnostic(&error, "duplicate_native_completion");
    assert_diagnostic(&observed.audit().join("; "), "duplicate_native_completion");
    let error = observed
        .wait("native-duplicate", immediate_deadline())
        .await
        .unwrap_err();
    assert_diagnostic(&error, "duplicate_native_completion");
}

#[test]
fn completed_original_rejects_replayed_actual_cursor_row_instead_of_overwriting_it() {
    let registry = WriteWaitRegistry::new();
    let mut cursor = registry.completed_snapshots_cursor();
    registry
        .begin("native-replayed", RPC_METHOD, registry.now())
        .complete();
    let row = one_native_row(&registry, &mut cursor);
    let observed = consumer("native-replayed", RPC_METHOD);
    observed.import(&row).unwrap();
    let error = observed
        .import(&row)
        .expect_err("one cursor row may be consumed only once");
    assert_diagnostic(&error, "duplicate_native_completion");
    assert_diagnostic(&observed.audit().join("; "), "duplicate_native_completion");
}

#[test]
fn completed_original_rejects_duplicate_immutable_expected_originals() {
    let error = match CompletedOrigins::new([
        ("same-expected".to_owned(), RPC_METHOD.to_owned()),
        ("same-expected".to_owned(), MCP_METHOD.to_owned()),
    ]) {
        Ok(_) => panic!("duplicate expected original must not overwrite its native method"),
        Err(error) => error,
    };
    assert_diagnostic(&error, "duplicate_expected_original");
}
