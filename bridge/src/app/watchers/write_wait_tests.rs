use super::*;
use crate::timing::write_wait::mutex::assert_blocked_acquisition;

#[test]
fn board_moved_records_the_roots_snapshot_leaf_wait() {
    let root = tempfile::tempdir().unwrap();
    let watchers = WorktreeWatchers::new();
    let roots = WorktreeRoots::from([("owner".into(), root.path().to_path_buf())]);
    assert_blocked_acquisition(&watchers.roots, "watcher-roots-publication", || {
        watchers.board_moved(roots.clone());
    });
    assert_eq!(*watchers.roots.lock().unwrap(), roots);
}
