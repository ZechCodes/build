use super::*;
use crate::app::observation_reads::{ObservationContext, ObservationPatch, ObservationReceipts};
use crate::app::snapshot_projection::incremental::GlobalProjection;
use crate::app::AppState;

fn fixture() -> (tempfile::TempDir, AppState) {
    let root = tempfile::tempdir().unwrap();
    let app = AppState::new_unrooted(
        root.path().join("worktrees"),
        "main",
        true,
        "/tmp/combined-publication-version.sock",
    );
    let observations = Arc::new(ObservationContext::capture(&app));
    assert!(app.read_view.publish_delta(
        SnapshotDelta::new(app.read_view.next_version())
            .set_observations(observations)
            .set_facts(Arc::default())
            .set_global(Arc::new(GlobalProjection::default()))
    ));
    (root, app)
}

fn owner_patch(app: &AppState, previous: &ReadView) -> ObservationPatch {
    let mut patch = ObservationPatch::default();
    // An absent owner still has a versioned input tombstone. No roster or wire
    // changes are needed to isolate its input-key acceptance guard.
    patch.owner(
        app,
        "retired-owner",
        previous.observations.as_ref().unwrap(),
    );
    assert!(!patch.is_empty());
    patch
}

fn combined_candidate(app: &AppState) -> SnapshotDelta {
    let previous = app.read_view.load();
    SnapshotDelta::new(app.read_view.next_version())
        .put_shared(
            "board.list",
            "",
            SharedJson::object([("candidate".into(), SharedJson::value(json!(true)))]),
        )
        .patch_observations(owner_patch(app, &previous))
        .set_observation_receipts(ObservationReceipts::default())
        .set_facts(Arc::default())
        .set_global(Arc::new(GlobalProjection::default()))
}

fn assert_candidate_rejected(app: &AppState, installed: &Arc<ReadView>, delta: SnapshotDelta) {
    assert!(!app.read_view.publish_delta(delta));
    let current = app.read_view.load();
    assert!(Arc::ptr_eq(installed, &current));
    assert!(Arc::ptr_eq(
        installed.observations.as_ref().unwrap(),
        current.observations.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        installed.global.as_ref().unwrap(),
        current.global.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(&installed.facts, &current.facts));
    assert!(current.get_shared("board.list", "").is_none());
    assert_eq!(current.global_version, installed.global_version);
    assert_eq!(current.observations_version, installed.observations_version);
    assert_eq!(current.facts_version, installed.facts_version);
}

#[test]
fn stale_global_cache_rejects_observation_patch_and_shared_wire_atomically() {
    let (_root, app) = fixture();
    let candidate = combined_candidate(&app);
    assert!(app.read_view.publish_delta(
        SnapshotDelta::new(app.read_view.next_version())
            .set_global(Arc::new(GlobalProjection::default()))
    ));
    let installed = app.read_view.load();
    assert!(installed.global_version > candidate.version);
    assert!(installed.observations_version < candidate.version);
    assert!(installed.facts_version < candidate.version);
    assert!(candidate
        .observation_patch
        .as_ref()
        .unwrap()
        .accepts(installed.observations.as_ref().unwrap(), candidate.version));
    assert_candidate_rejected(&app, &installed, candidate);
}

#[test]
fn stale_observation_key_rejects_global_cache_and_shared_wire_atomically() {
    let (_root, app) = fixture();
    let candidate = combined_candidate(&app);
    let previous = app.read_view.load();
    assert!(app.read_view.publish_delta(
        SnapshotDelta::new(app.read_view.next_version())
            .patch_observations(owner_patch(&app, &previous))
    ));
    let installed = app.read_view.load();
    assert!(installed.global_version < candidate.version);
    assert!(installed.facts_version < candidate.version);
    assert!(!candidate
        .observation_patch
        .as_ref()
        .unwrap()
        .accepts(installed.observations.as_ref().unwrap(), candidate.version));
    assert_candidate_rejected(&app, &installed, candidate);
}

#[test]
fn committed_history_receipt_retains_observation_root() {
    let (_root, mut app) = fixture();
    let mut thread = crate::thread::Thread::for_agent("receipt-owner");
    thread.post_user("receipt history", None, "before");
    assert!(app.read_view.publish_delta(
        SnapshotDelta::new(app.read_view.next_version()).put_history(
            "receipt-owner",
            history::SharedHistory::warm(&thread, None).unwrap(),
        )
    ));
    let before = app.read_view.load();
    let old_history = before.history("receipt-owner").unwrap().render(&json!({}));
    thread.set_legacy_delivery_status(1, 1, crate::thread::MessageDeliveryStatus::Sent);
    app.apply_committed_thread_items(&crate::store::CommittedThreadItems {
        conversation_id: thread.agent.id.clone(),
        thread_id: thread.id.clone(),
        generation_revision: thread.generation_revision,
        sequence: thread.last_sequence(),
        items: vec![thread.items[0].clone()],
    });
    let after = app.read_view.load();
    assert!(!Arc::ptr_eq(&before, &after));
    assert!(after.facts_version > before.facts_version);
    assert!(Arc::ptr_eq(
        before.observations.as_ref().unwrap(),
        after.observations.as_ref().unwrap()
    ));
    assert_eq!(after.observations_version, before.observations_version);
    let history = after.history("receipt-owner").unwrap().render(&json!({}));
    assert_eq!(history["items"][0]["data"]["delivery_status"], "sent");
    assert_eq!(
        before.history("receipt-owner").unwrap().render(&json!({})),
        old_history
    );
}
