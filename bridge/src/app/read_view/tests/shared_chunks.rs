use super::*;
use crate::app::snapshot_projection::incremental::GlobalProjection;

#[test]
fn shared_publication_retains_unaffected_leaves_and_old_root() {
    let slot = ReadViewSlot::default();
    let row = SharedJson::value(json!({"body": "retained ".repeat(128)}));
    let board = SharedJson::object([("items".into(), SharedJson::array([row.clone()]))]);
    slot.publish_delta(SnapshotDelta::new(slot.next_version()).put_shared("board.list", "", board));
    let old = slot.load();
    let board = SharedJson::object([(
        "items".into(),
        SharedJson::array([row.clone(), SharedJson::value(json!({"body":"new"}))]),
    )]);
    slot.publish_delta(SnapshotDelta::new(slot.next_version()).put_shared("board.list", "", board));
    let next = slot.load();
    let items = next
        .get_shared("board.list", "")
        .unwrap()
        .field("items")
        .unwrap();
    assert!(items.elements().unwrap()[0].ptr_eq(&row));
    assert_eq!(
        old.get_owned("board.list", "").unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        next.get_owned("board.list", "").unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn direct_global_updates_invalidate_cache_but_disjoint_and_facts_updates_preserve_it() {
    let slot = ReadViewSlot::default();
    let cache = Arc::new(GlobalProjection::default());
    slot.publish_delta(SnapshotDelta::new(slot.next_version()).set_global(cache.clone()));
    slot.publish_delta(SnapshotDelta::new(slot.next_version()).put("agent.list", "a", json!([])));
    slot.publish_delta(SnapshotDelta::new(slot.next_version()).set_facts(Arc::default()));
    assert!(Arc::ptr_eq(slot.load().global.as_ref().unwrap(), &cache));
    slot.publish_delta(SnapshotDelta::new(slot.next_version()).put(
        "board.list",
        "",
        json!({"injected":true}),
    ));
    assert!(slot.load().global.is_none());
    assert_eq!(
        slot.load().get_owned("board.list", ""),
        Some(json!({"injected":true}))
    );
}

#[test]
fn older_disjoint_global_update_merges_and_stale_cache_cannot_resurrect() {
    let slot = ReadViewSlot::default();
    let disjoint =
        SnapshotDelta::new(slot.next_version()).put("project.list", "", json!({"projects":[]}));
    let stale_version = slot.next_version();
    let stale = SnapshotDelta::new(stale_version)
        .put("agent.list", "stale", json!({"bad":true}))
        .set_global(Arc::default());
    let latest_version = slot.next_version();
    slot.publish_delta(SnapshotDelta::new(latest_version).set_global(Arc::default()));
    slot.publish_delta(disjoint);
    assert!(slot.load().get_shared("project.list", "").is_some());
    assert!(slot.load().global.is_none());
    assert_eq!(slot.load().global_version, latest_version);
    slot.publish_delta(stale);
    assert!(slot.load().global.is_none());
    assert!(slot.load().get_shared("agent.list", "stale").is_none());
}

#[test]
fn shared_chunk_tombstone_rejects_entire_older_candidate() {
    let slot = ReadViewSlot::default();
    let stale = SnapshotDelta::new(slot.next_version())
        .put_shared("board.list", "", SharedJson::object([]))
        .put("agent.list", "partial", json!(true))
        .set_global(Arc::default());
    slot.publish_delta(SnapshotDelta::new(slot.next_version()).remove("board.list", ""));
    slot.publish_delta(stale);
    assert!(slot.load().get_shared("board.list", "").is_none());
    assert!(slot.load().get_shared("agent.list", "partial").is_none());
    assert!(slot.load().global.is_none());
}
