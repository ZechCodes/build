//! Regressions for the device foundation entry criteria.
use super::*;
use crate::store::Store;
use serde_json::json;
const NOW: i64 = 1_800_000_000_000;

#[test]
fn dirty_clear_remark_rejects_delayed_generation_across_restart() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let writer = store.writer();
    let old = writer.mark_sync_dirty("project", "one", NOW).unwrap();
    writer.reconcile_sync_dirty("project", "one", old, Some(json!({"v":1})), NOW).unwrap();
    drop(writer);
    drop(store);
    let store = Store::new(directory.path()).unwrap();
    let writer = store.writer();
    let new = writer.mark_sync_dirty("project", "one", NOW + 1).unwrap();
    assert!(new > old, "clear/re-mark must not reuse a generation");
    assert!(writer.reconcile_sync_dirty("project", "one", old, Some(json!({"v":"stale"})), NOW + 2).is_err());
    // Negative control: the current generation can publish and clear the fence.
    writer.reconcile_sync_dirty("project", "one", new, Some(json!({"v":2})), NOW + 2).unwrap();
    writer.admit_device_lease("opening", "epoch", NOW + 2).unwrap();
}

#[tokio::test]
async fn device_dirty_fence_covers_existing_lease_head_events_and_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let writer = store.writer();
    let lease = writer.admit_device_lease("opening", "epoch", NOW).unwrap();
    writer.mark_sync_dirty("project", "one", NOW + 1).unwrap();
    let scope = Scope::new(Stream::device(), json!({"kind":"device"}));
    let mut read = store.read_pool().acquire_sync().await;
    let results = read.read(|tx| {
        let head = tx.sync_head(&scope, None, "epoch", NOW + 1);
        let events = tx.sync_events(&scope, Sequence::ZERO, Sequence::ZERO, 1);
        let page = tx.sync_device_snapshot(&lease.id, None, 1, "epoch", NOW + 1);
        (head, events, page)
    }).unwrap();
    assert!(matches!(results.0, Err(SyncError::Unavailable(ref message)) if message == "sync_reconciling"), "head must be fenced");
    assert!(matches!(results.1, Err(SyncError::Unavailable(ref message)) if message == "sync_reconciling"), "events must be fenced");
    assert!(matches!(results.2, Err(SyncError::Unavailable(ref message)) if message == "sync_reconciling"), "existing lease must be fenced");
    // Negative control: task-backed reads do not depend on the non-SQL fence.
    let project = Scope::new(Stream::project("synthetic"), json!({"kind":"project_tasks"}));
    read.head(&project, None, "epoch", NOW + 1).unwrap();
}

#[test]
fn stale_epoch_and_incarnation_leases_do_not_consume_opening_quota() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let writer = store.writer();
    for _ in 0..DEVICE_LEASES_PER_OPENING {
        writer.admit_device_lease("opening", "old-epoch", NOW).unwrap();
    }
    writer.admit_device_lease("opening", "new-epoch", NOW + 1).expect("old epoch cannot consume quota");
    store.writer_for_test().execute("UPDATE sync_leases SET incarnation='retired'", []).unwrap();
    for _ in 0..DEVICE_LEASES_PER_OPENING {
        writer.admit_device_lease("opening", "new-epoch", NOW + 2).expect("old incarnation cannot consume quota");
    }
    // Negative control: current leases still enforce the opening quota.
    assert!(matches!(writer.admit_device_lease("opening", "new-epoch", NOW + 2), Err(SyncError::Unavailable(_))));
}

#[test]
fn device_schema_is_version_23() {
    assert_eq!(crate::store::SCHEMA_VERSION, 23);
}

#[tokio::test]
async fn device_page_keyset_uses_first_sequence_and_frozen_upper_bound() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let writer = store.writer();
    for key in ["z", "a", "m"] {
        let generation = writer.mark_sync_dirty("project", key, NOW).unwrap();
        writer.reconcile_sync_dirty("project", key, generation, Some(json!({"key":key})), NOW).unwrap();
    }
    let lease = writer.admit_device_lease("opening", "epoch", NOW).unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let first = read.read(|tx| tx.sync_device_snapshot(&lease.id, None, 1, "epoch", NOW)).unwrap().unwrap();
    assert_eq!(first["records"][0]["key"], "z", "order must follow indexed first_seq,kind,key");
    assert_eq!(first["next"]["first_seq"], "1");
    // Negative control: later keys cannot enter a frozen snapshot.
    let generation = writer.mark_sync_dirty("project", "0", NOW).unwrap();
    writer.reconcile_sync_dirty("project", "0", generation, Some(json!({"key":"new"})), NOW).unwrap();
    let frozen = read.read(|tx| tx.sync_device_snapshot(&lease.id, None, 500, "epoch", NOW)).unwrap().unwrap();
    assert_eq!(frozen["records"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn device_page_budget_is_one_mib_and_oversized_rows_refuse_explicitly() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let writer = store.writer();
    for index in 0..30 {
        let key = format!("{index:03}");
        let generation = writer.mark_sync_dirty("project", &key, NOW).unwrap();
        writer.reconcile_sync_dirty("project", &key, generation, Some(json!({"summary":"x".repeat(40_000)})), NOW).unwrap();
    }
    let lease = writer.admit_device_lease("opening", "epoch", NOW).unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let page = read.read(|tx| tx.sync_device_snapshot(&lease.id, None, 500, "epoch", NOW)).unwrap().unwrap();
    assert!(serde_json::to_vec(&page).unwrap().len() <= MAX_RECOVERY_BYTES);
    assert!(page.get("next").is_some(), "budget must preserve a continuation");
    // Negative control: a small complete page does not spuriously continue.
    let small = read.read(|tx| tx.sync_device_snapshot(&lease.id, None, 1, "epoch", NOW)).unwrap().unwrap();
    assert_eq!(small["records"].as_array().unwrap().len(), 1);
    store.writer_for_test().execute("UPDATE sync_projection SET record=?1,record_bytes=?2 WHERE key='000'", rusqlite::params![json!({"summary":"x".repeat(MAX_RECOVERY_BYTES)}).to_string(),MAX_RECOVERY_BYTES as i64+14]).unwrap();
    let oversized = read.read(|tx| tx.sync_device_snapshot(&lease.id, None, 500, "epoch", NOW)).unwrap().unwrap();
    assert_eq!(oversized["status"], "resync_required");
    assert_eq!(oversized["reason"], "over_budget");
    assert!(oversized.get("records").is_none());
}
