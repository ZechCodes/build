use super::*;
use crate::store::Store;
use serde_json::json;

const NOW: i64 = 1_800_000_000_000;

#[tokio::test]
async fn leased_snapshot_keeps_copy_on_write_rows_through_update_and_delete() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let writer = store.writer();
    let first = writer.mark_sync_dirty("project", "one", NOW).unwrap();
    writer
        .reconcile_sync_dirty("project", "one", first, Some(json!({"name":"before"})), NOW)
        .unwrap();
    let second = writer.mark_sync_dirty("project", "two", NOW).unwrap();
    writer
        .reconcile_sync_dirty(
            "project",
            "two",
            second,
            Some(json!({"name":"deleted later"})),
            NOW,
        )
        .unwrap();
    let lease = writer
        .admit_device_lease("opening-a", "epoch-a", NOW)
        .unwrap();

    let changed = writer.mark_sync_dirty("project", "one", NOW + 1).unwrap();
    writer
        .reconcile_sync_dirty(
            "project",
            "one",
            changed,
            Some(json!({"name":"after"})),
            NOW + 1,
        )
        .unwrap();
    let deleted = writer.mark_sync_dirty("project", "two", NOW + 1).unwrap();
    writer
        .reconcile_sync_dirty("project", "two", deleted, None, NOW + 1)
        .unwrap();

    let mut read = store.read_pool().acquire_sync().await;
    let old = read
        .read(|tx| tx.sync_device_snapshot(&lease.id, None, 100, "epoch-a", NOW + 2))
        .unwrap()
        .unwrap();
    assert_eq!(old["records"][0]["row"]["name"], "before");
    assert_eq!(old["records"][1]["row"]["name"], "deleted later");
    let current = writer
        .admit_device_lease("opening-a", "epoch-a", NOW + 2)
        .unwrap();
    let current = read
        .read(|tx| tx.sync_device_snapshot(&current.id, None, 100, "epoch-a", NOW + 2))
        .unwrap()
        .unwrap();
    assert_eq!(current["records"].as_array().unwrap().len(), 1);
    assert_eq!(current["records"][0]["row"]["name"], "after");
}

#[tokio::test]
async fn mutation_during_device_paging_keeps_the_leased_keyset_and_versions() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let writer = store.writer();
    for key in ["a", "b", "c"] {
        let generation = writer.mark_sync_dirty("project", key, NOW).unwrap();
        writer
            .reconcile_sync_dirty("project", key, generation, Some(json!({"name":key})), NOW)
            .unwrap();
    }
    let lease = writer.admit_device_lease("opening", "epoch", NOW).unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let first = read
        .read(|tx| tx.sync_device_snapshot(&lease.id, None, 1, "epoch", NOW + 1))
        .unwrap()
        .unwrap();
    assert_eq!(first["records"][0]["row"]["name"], "a");

    let changed = writer.mark_sync_dirty("project", "b", NOW + 2).unwrap();
    writer
        .reconcile_sync_dirty(
            "project",
            "b",
            changed,
            Some(json!({"name":"new-b"})),
            NOW + 2,
        )
        .unwrap();
    let deleted = writer.mark_sync_dirty("project", "c", NOW + 2).unwrap();
    writer
        .reconcile_sync_dirty("project", "c", deleted, None, NOW + 2)
        .unwrap();
    let created = writer.mark_sync_dirty("project", "aa", NOW + 2).unwrap();
    writer
        .reconcile_sync_dirty(
            "project",
            "aa",
            created,
            Some(json!({"name":"created-after-watermark"})),
            NOW + 2,
        )
        .unwrap();

    let next = (
        first["next"]["first_seq"].as_str().unwrap().parse::<i64>().unwrap(),
        first["next"]["kind"].as_str().unwrap(),
        first["next"]["key"].as_str().unwrap(),
    );
    let second = read
        .read(|tx| tx.sync_device_snapshot(&lease.id, Some(next), 1, "epoch", NOW + 3))
        .unwrap()
        .unwrap();
    assert_eq!(second["records"][0]["row"]["name"], "b");
    let next = (
        second["next"]["first_seq"].as_str().unwrap().parse::<i64>().unwrap(),
        second["next"]["kind"].as_str().unwrap(),
        second["next"]["key"].as_str().unwrap(),
    );
    let third = read
        .read(|tx| tx.sync_device_snapshot(&lease.id, Some(next), 10, "epoch", NOW + 3))
        .unwrap()
        .unwrap();
    let names = third["records"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| record["row"]["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(names, ["c"]);
}

#[tokio::test]
async fn dirty_markers_survive_restart_and_generation_races_refuse_stale_reads() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let marker = store
        .writer()
        .mark_sync_dirty("workspace", "one", NOW)
        .unwrap();
    drop(store);
    let reopened = Store::new(directory.path()).unwrap();
    assert!(matches!(
        reopened
            .writer()
            .admit_device_lease("opening", "epoch", NOW),
        Err(SyncError::Unavailable(_))
    ));
    let newer = reopened
        .writer()
        .mark_sync_dirty("workspace", "one", NOW + 1)
        .unwrap();
    assert!(reopened
        .writer()
        .reconcile_sync_dirty(
            "workspace",
            "one",
            marker,
            Some(json!({"name":"stale"})),
            NOW + 1,
        )
        .is_err());
    reopened
        .writer()
        .reconcile_sync_dirty(
            "workspace",
            "one",
            newer,
            Some(json!({"name":"current"})),
            NOW + 1,
        )
        .unwrap();
    reopened
        .writer()
        .admit_device_lease("opening", "epoch", NOW + 1)
        .unwrap();
}

#[test]
fn dirty_reconcile_rolls_back_projection_journal_and_marker_clear_together() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let writer = store.writer();
    let generation = writer.mark_sync_dirty("project", "one", NOW).unwrap();
    store
        .writer_for_test()
        .execute_batch(
            "CREATE TRIGGER reject_device_journal BEFORE INSERT ON sync_journal
             BEGIN SELECT RAISE(ABORT,'synthetic device journal failure'); END;",
        )
        .unwrap();
    assert!(writer
        .reconcile_sync_dirty(
            "project",
            "one",
            generation,
            Some(json!({"name":"one"})),
            NOW,
        )
        .is_err());
    let conn = store.writer_for_test();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM sync_dirty WHERE kind='project' AND key='one'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM sync_projection", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM sync_journal", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
}

#[tokio::test]
async fn expired_and_old_epoch_leases_require_resync_and_admission_is_capped() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let writer = store.writer();
    let lease = writer
        .admit_device_lease("opening-a", "epoch-a", NOW)
        .unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    for (epoch, now) in [
        ("epoch-b", NOW + 1),
        ("epoch-a", NOW + DEVICE_LEASE_TTL_MS + 1),
    ] {
        let answer = read
            .read(|tx| tx.sync_device_snapshot(&lease.id, None, 10, epoch, now))
            .unwrap()
            .unwrap();
        assert_eq!(answer["status"], "resync_required");
        assert_eq!(answer["reason"], "lease_expired");
    }
    for _ in 1..DEVICE_LEASES_PER_OPENING {
        writer
            .admit_device_lease("opening-a", "epoch-a", NOW + 1)
            .unwrap();
    }
    assert!(matches!(
        writer.admit_device_lease("opening-a", "epoch-a", NOW + 1),
        Err(SyncError::Unavailable(_))
    ));
}

#[tokio::test]
async fn lease_expiry_during_snapshot_is_rechecked_before_reply() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let writer = store.writer();
    let generation = writer.mark_sync_dirty("project", "one", NOW).unwrap();
    writer
        .reconcile_sync_dirty(
            "project",
            "one",
            generation,
            Some(json!({"name":"one"})),
            NOW,
        )
        .unwrap();
    let lease = writer.admit_device_lease("opening", "epoch", NOW).unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let answer = read
        .read(|tx| {
            tx.sync_device_snapshot_with_recheck(
                &lease.id,
                None,
                10,
                "epoch",
                NOW,
                lease.expires_at_ms.saturating_add(1),
            )
        })
        .unwrap()
        .unwrap();
    assert_eq!(answer["status"], "resync_required");
    assert_eq!(answer["reason"], "lease_expired");
    assert!(answer.get("records").is_none());
}

#[test]
fn device_gc_preserves_leased_versions_then_reclaims_expired_lease_and_history() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let writer = store.writer();
    let first = writer.mark_sync_dirty("project", "one", NOW).unwrap();
    writer
        .reconcile_sync_dirty("project", "one", first, Some(json!({"version":1})), NOW)
        .unwrap();
    let lease = writer
        .admit_device_lease("opening-a", "epoch-a", NOW)
        .unwrap();
    for version in 2..=3 {
        let generation = writer
            .mark_sync_dirty("project", "one", NOW + version)
            .unwrap();
        writer
            .reconcile_sync_dirty(
                "project",
                "one",
                generation,
                Some(json!({"version":version})),
                NOW + version,
            )
            .unwrap();
    }

    let active = writer.sync_gc_batch(NOW + 4).unwrap();
    assert_eq!(active.removed_leases, 0);
    assert_eq!(active.removed_projection_versions, 0);
    assert_eq!(
        store
            .writer_for_test()
            .query_row("SELECT COUNT(*) FROM sync_projection_versions", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        2,
        "the lease at the first watermark still needs both prior versions"
    );

    let expired = writer
        .sync_gc_batch(lease.expires_at_ms.saturating_add(1))
        .unwrap();
    assert_eq!(expired.removed_leases, 1);
    assert_eq!(expired.removed_projection_versions, 2);
    assert_eq!(
        store
            .writer_for_test()
            .query_row("SELECT COUNT(*) FROM sync_projection_versions", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    assert!(expired.changed_rows <= super::retention::GC_ROWS as u64);
}
