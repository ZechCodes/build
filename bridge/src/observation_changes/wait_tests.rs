use super::*;
use crate::timing::write_wait::mutex::assert_blocked_acquisition;

#[test]
fn marking_dirty_key_records_its_real_leaf_wait() {
    let keys = ChangeKeys::default();
    assert_blocked_acquisition(&keys.pending, "dirty-mark", || keys.mark("owner"));
    assert_eq!(keys.snapshot(), vec![("owner".into(), 1)]);
}

#[test]
fn capturing_dirty_keys_records_its_real_leaf_wait() {
    let keys = ChangeKeys::default();
    keys.mark("owner");
    let mut captured = Vec::new();
    assert_blocked_acquisition(&keys.pending, "dirty-capture", || {
        captured = keys.snapshot();
    });
    assert_eq!(captured, vec![("owner".into(), 1)]);
}

#[test]
fn acknowledging_dirty_keys_records_wait_and_preserves_newer_revisions() {
    let keys = ChangeKeys::default();
    keys.mark("owner");
    let first = keys.snapshot();
    keys.mark("owner");
    assert_blocked_acquisition(&keys.pending, "dirty-acknowledge", || {
        keys.acknowledge(&first);
    });
    assert_eq!(keys.snapshot(), vec![("owner".into(), 2)]);
}
