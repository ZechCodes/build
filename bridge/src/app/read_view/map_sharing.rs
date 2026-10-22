//! Work-count controls for retained immutable roots, independent of JSON size.
use super::*;
use serde_json::json;
use std::cell::Cell;

const ENTRIES: usize = 4096;

#[derive(Clone, Copy, Debug, Default)]
struct Copies {
    chunks: usize,
    histories: usize,
}

thread_local! {
    static COPIES: Cell<Copies> = const { Cell::new(Copies { chunks: 0, histories: 0 }) };
}

pub(super) fn record_chunk_clone() {
    COPIES.with(|slot| {
        let mut copies = slot.get();
        copies.chunks += 1;
        slot.set(copies);
    });
}

pub(super) fn record_history_clone() {
    COPIES.with(|slot| {
        let mut copies = slot.get();
        copies.histories += 1;
        slot.set(copies);
    });
}

fn measure<T>(work: impl FnOnce() -> T) -> (T, Copies) {
    let previous = COPIES.replace(Copies::default());
    let result = work();
    (result, COPIES.replace(previous))
}

fn fixture() -> ReadViewSlot {
    let slot = ReadViewSlot::default();
    let thread = crate::thread::Thread::for_agent("shared-history-source");
    let history = Arc::new(history::SharedHistory::warm(&thread, None).unwrap());
    let mut delta = SnapshotDelta::new(slot.next_version());
    for index in 0..ENTRIES {
        let key = format!("entry-{index:04}");
        delta = delta
            .put("agent.list", &key, json!({"index":index}))
            .put_shared_history(&key, Arc::clone(&history));
    }
    assert!(slot.publish_delta(delta));
    slot
}

#[test]
fn cloning_retained_read_view_does_not_copy_chunk_or_history_entries() {
    let slot = fixture();
    let retained = slot.load();
    let (copied, copies) = measure(|| retained.as_ref().clone());
    assert_eq!(copied.keys().count(), ENTRIES);
    assert_eq!(copied.history_keys().count(), ENTRIES);
    assert!(copied
        .get_shared("agent.list", "entry-4095")
        .unwrap()
        .ptr_eq(retained.get_shared("agent.list", "entry-4095").unwrap()));
    assert_eq!(
        copies.chunks, 0,
        "root clone copied chunk entries: {copies:?}"
    );
    assert_eq!(
        copies.histories, 0,
        "root clone copied history entries: {copies:?}"
    );
}

#[test]
fn one_row_publication_copies_bounded_paths_and_preserves_retained_root() {
    let slot = fixture();
    let retained = slot.load();
    let mut thread = crate::thread::Thread::for_agent("replacement-history-source");
    thread.post_user("one changed history", None, "now");
    let replacement = Arc::new(history::SharedHistory::warm(&thread, None).unwrap());
    let delta = SnapshotDelta::new(slot.next_version())
        .put("agent.list", "entry-2048", json!({"changed":true}))
        .put_shared_history("entry-2048", Arc::clone(&replacement));
    let (accepted, copies) = measure(|| slot.publish_delta(delta));
    assert!(accepted);
    let current = slot.load();
    assert!(!Arc::ptr_eq(&retained, &current));
    assert_eq!(
        retained.get_owned("agent.list", "entry-2048"),
        Some(json!({"index":2048}))
    );
    assert_eq!(
        current.get_owned("agent.list", "entry-2048"),
        Some(json!({"changed":true}))
    );
    assert_eq!(
        retained.history("entry-2048").unwrap().render(&json!({}))["items"],
        json!([])
    );
    assert_eq!(
        current.history("entry-2048").unwrap().render(&json!({}))["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(Arc::ptr_eq(
        current.histories["entry-2048"].value.as_ref().unwrap(),
        &replacement
    ));
    for key in ["entry-0000", "entry-2047", "entry-2049", "entry-4095"] {
        assert!(retained
            .get_shared("agent.list", key)
            .unwrap()
            .ptr_eq(current.get_shared("agent.list", key).unwrap()));
        assert!(Arc::ptr_eq(
            retained.histories[key].value.as_ref().unwrap(),
            current.histories[key].value.as_ref().unwrap()
        ));
    }
    // A generous fraction excludes a whole-map copy without pinning the
    // persistent tree's node fanout, balancing policy or exact path length.
    assert!(
        copies.chunks < ENTRIES / 8,
        "small delta copied chunk map: {copies:?}"
    );
    assert!(
        copies.histories < ENTRIES / 8,
        "small delta copied history map: {copies:?}"
    );
}
