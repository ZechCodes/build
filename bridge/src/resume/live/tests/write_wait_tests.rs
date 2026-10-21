use super::*;
use crate::timing::write_wait::mutex::assert_blocked_acquisition;

#[test]
fn published_live_roster_records_slot_wait_and_preserves_poison_recovery() {
    let root = tempfile::tempdir().unwrap();
    let disk = Arc::new(RecordingDisk::default());
    let live = LiveRoster::start_with(root.path(), "test", disk, |_writer| {
        Err(std::io::Error::other("controlled no-writer fallback"))
    });
    let original = working_on("owner", "agent");
    assert_blocked_acquisition(&live.shared.slot, "live-roster-publication", || {
        live.set_entity("owner", vec![original.clone()]);
    });
    assert_eq!(live.slot().entities["owner"], vec![original]);
    std::thread::scope(|scope| {
        let slot = &live.shared.slot;
        let poison = scope.spawn(move || {
            let _guard = slot.lock().unwrap();
            panic!("poison the actual live-roster leaf");
        });
        assert!(poison.join().is_err());
    });
    let replacement = working_on("owner", "replacement");
    live.set_entity("owner", vec![replacement.clone()]);
    assert!(live.shared.slot.is_poisoned());
    assert_eq!(live.slot().entities["owner"], vec![replacement]);
    assert_eq!(live.slot().generation, 2);
}
