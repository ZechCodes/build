use super::*;
use serde_json::json;

mod shared_chunks;

#[test]
fn disjoint_deltas_merge_and_old_root_remains_immutable() {
    let slot = ReadViewSlot::default();
    let before = slot.load();
    let first = SnapshotDelta::new(slot.next_version()).put("agent.list", "a", json!({"n": 1}));
    let second = SnapshotDelta::new(slot.next_version()).put("agent.list", "b", json!({"n": 2}));
    slot.publish_delta(second);
    slot.publish_delta(first);
    assert_eq!(
        slot.load().get("agent.list", "a").as_deref(),
        Some(&json!({"n": 1}))
    );
    assert_eq!(
        slot.load().get("agent.list", "b").as_deref(),
        Some(&json!({"n": 2}))
    );
    assert!(before.get("agent.list", "a").is_none());
}

#[test]
fn deletion_tombstone_rejects_stale_refresh_and_multi_owner_delta_is_atomic() {
    let slot = ReadViewSlot::default();
    let stale = SnapshotDelta::new(slot.next_version())
        .put("agent.list", "a", json!("stale"))
        .put("agent.list", "b", json!("partial"));
    slot.publish_delta(SnapshotDelta::new(slot.next_version()).remove("agent.list", "a"));
    slot.publish_delta(stale);
    assert!(slot.load().get("agent.list", "a").is_none());
    assert!(slot.load().get("agent.list", "b").is_none());
}

#[test]
fn concurrent_delta_publication_retains_every_disjoint_owner() {
    let slot = ReadViewSlot::default();
    std::thread::scope(|scope| {
        for index in 0..16 {
            let slot = slot.clone();
            scope.spawn(move || {
                slot.publish_delta(SnapshotDelta::new(slot.next_version()).put(
                    "agent.list",
                    index.to_string(),
                    json!({"index": index}),
                ));
            });
        }
    });
    for index in 0..16 {
        assert_eq!(
            slot.load().get("agent.list", &index.to_string()).as_deref(),
            Some(&json!({"index":index}))
        );
    }
}

#[test]
fn current_builder_rebases_on_a_concurrent_publication() {
    use std::sync::{atomic::AtomicUsize, mpsc};
    use std::time::Duration;
    let slot = ReadViewSlot::default();
    slot.publish_delta(SnapshotDelta::new(slot.next_version()).put("agent.list", "a", json!(0)));
    let (entered, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let built = Arc::new(AtomicUsize::new(0));
    let writer = {
        let slot = slot.clone();
        let built = Arc::clone(&built);
        std::thread::spawn(move || {
            slot.publish_current(|current, version| {
                let previous = current.get("agent.list", "a").unwrap().as_u64().unwrap();
                if built.fetch_add(1, Ordering::SeqCst) == 0 {
                    entered.send(()).unwrap();
                    released.recv_timeout(Duration::from_secs(10)).unwrap();
                }
                Some(SnapshotDelta::new(version).put("agent.list", "a", json!(previous + 1)))
            })
        })
    };
    waiting.recv_timeout(Duration::from_secs(10)).unwrap();
    slot.publish_delta(SnapshotDelta::new(slot.next_version()).put("agent.list", "a", json!(10)));
    release.send(()).unwrap();
    writer.join().unwrap();
    assert_eq!(
        slot.load().get("agent.list", "a").as_deref(),
        Some(&json!(11))
    );
    assert_eq!(built.load(Ordering::SeqCst), 2);
}
