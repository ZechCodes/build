// Intended child of reclaim/sizes tests after initial rare RED.
use super::*;
use crate::timing::write_wait::WriteWaitRegistry;
use std::time::Instant;

#[test]
fn repeated_size_requests_keep_each_origin_through_one_actual_queue_completion() {
    let requests = SizeRequests::default();
    let registry = WriteWaitRegistry::new();
    let first = registry.begin("size-first", "workspace.measure_sizes", Instant::now());
    let first_id = first.id();
    {
        let _binding = first.context().bind_sync();
        assert_eq!(requests.request(["workspace".to_owned()]), ["workspace"]);
    }
    first.complete();
    assert_eq!(requests.next().as_deref(), Some("workspace"));
    let duplicate = registry.begin(
        "size-active-duplicate",
        "workspace.measure_sizes",
        Instant::now(),
    );
    let duplicate_id = duplicate.id();
    {
        let _binding = duplicate.context().bind_sync();
        assert!(requests.request(["workspace".to_owned()]).is_empty());
    }
    duplicate.complete();
    let before = registry.active_snapshot();
    requests.done("workspace");
    for id in [first_id, duplicate_id] {
        assert!(before
            .iter()
            .any(|row| row.id == id && row.root_finished && !row.outstanding.is_empty()));
        let done = registry
            .snapshot()
            .into_iter()
            .find(|row| row.id == id)
            .unwrap();
        assert!(done.root_finished && done.outstanding.is_empty() && done.active.is_empty());
        assert!(!done.abandoned && !done.cancelled);
    }
    assert!(requests.next().is_none());
}
