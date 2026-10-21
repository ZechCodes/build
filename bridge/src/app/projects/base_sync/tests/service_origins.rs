// Intended child of app/projects/base_sync/tests.rs after initial rare RED.
use super::*;
use crate::timing::write_wait::WriteWaitRegistry;

#[test]
fn same_source_requests_keep_both_originals_until_actual_service_publication() {
    let mut fixture = fixture();
    let registry = WriteWaitRegistry::new();
    let source_id = fixture.source().id.clone();
    let revision = fixture.state.changes.board_revision();
    let mut ids = Vec::new();
    for original in ["source-first", "source-coalesced"] {
        let root = registry.begin(original, "project.sync_source", Instant::now());
        ids.push(root.id());
        let binding = root.context().bind_sync();
        fixture
            .state
            .request_source_sync(fixture.project_id.clone(), source_id.clone());
        drop(binding);
        root.complete();
    }
    assert_eq!(fixture.state.source_sync_requested.len(), 1);
    let before = registry.active_snapshot();
    let state = Arc::new(Mutex::new(fixture.state));
    AppState::sync_sources(&state, SyncPass::Requested, START_MS);
    let after = registry.snapshot();
    assert!(state.lock().unwrap().changes.board_revision() > revision);
    for id in ids {
        let retained = before.iter().find(|row| row.id == id);
        assert!(
            retained.is_some_and(|row| row.root_finished && !row.outstanding.is_empty()),
            "coalesced source must keep each completed request root's required child"
        );
        let done = after.iter().find(|row| row.id == id).unwrap();
        assert!(done.root_finished && done.outstanding.is_empty() && done.active.is_empty());
        assert!(!done.cancelled && !done.abandoned);
    }
}
