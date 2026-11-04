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
    writer
        .reconcile_sync_dirty("project", "one", old, Some(json!({"v":1})), NOW)
        .unwrap();
    drop(writer);
    drop(store);
    let store = Store::new(directory.path()).unwrap();
    let writer = store.writer();
    let new = writer.mark_sync_dirty("project", "one", NOW + 1).unwrap();
    assert!(new > old, "clear/re-mark must not reuse a generation");
    assert!(writer
        .reconcile_sync_dirty("project", "one", old, Some(json!({"v":"stale"})), NOW + 2)
        .is_err());
    // Negative control: the current generation can publish and clear the fence.
    writer
        .reconcile_sync_dirty("project", "one", new, Some(json!({"v":2})), NOW + 2)
        .unwrap();
    writer
        .admit_device_lease("opening", "epoch", NOW + 2)
        .unwrap();
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
    let results = read
        .read(|tx| {
            let head = tx.sync_head(&scope, None, "epoch", NOW + 1);
            let events = tx.sync_events(&scope, Sequence::ZERO, Sequence::ZERO, 1);
            let page = tx.sync_device_snapshot(&lease.id, None, 1, "epoch", NOW + 1);
            (head, events, page)
        })
        .unwrap();
    assert!(
        matches!(results.0, Err(SyncError::Unavailable(ref message)) if message == "sync_reconciling"),
        "head must be fenced"
    );
    assert!(
        matches!(results.1, Err(SyncError::Unavailable(ref message)) if message == "sync_reconciling"),
        "events must be fenced"
    );
    assert!(
        matches!(results.2, Err(SyncError::Unavailable(ref message)) if message == "sync_reconciling"),
        "existing lease must be fenced"
    );
    // Negative control: task-backed reads do not depend on the non-SQL fence.
    let project = Scope::new(
        Stream::project("synthetic"),
        json!({"kind":"project_tasks"}),
    );
    read.head(&project, None, "epoch", NOW + 1).unwrap();
}

#[test]
fn stale_epoch_and_incarnation_leases_do_not_consume_opening_quota() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let writer = store.writer();
    for _ in 0..DEVICE_LEASES_PER_OPENING {
        writer
            .admit_device_lease("opening", "old-epoch", NOW)
            .unwrap();
    }
    writer
        .admit_device_lease("opening", "new-epoch", NOW + 1)
        .expect("old epoch cannot consume quota");
    store
        .writer_for_test()
        .execute("UPDATE sync_leases SET incarnation='retired'", [])
        .unwrap();
    for _ in 0..DEVICE_LEASES_PER_OPENING {
        writer
            .admit_device_lease("opening", "new-epoch", NOW + 2)
            .expect("old incarnation cannot consume quota");
    }
    // Negative control: current leases still enforce the opening quota.
    assert!(matches!(
        writer.admit_device_lease("opening", "new-epoch", NOW + 2),
        Err(SyncError::Unavailable(_))
    ));
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
        writer
            .reconcile_sync_dirty("project", key, generation, Some(json!({"key":key})), NOW)
            .unwrap();
    }
    let lease = writer.admit_device_lease("opening", "epoch", NOW).unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let first = read
        .read(|tx| tx.sync_device_snapshot(&lease.id, None, 1, "epoch", NOW))
        .unwrap()
        .unwrap();
    assert_eq!(
        first["records"][0]["key"], "z",
        "order must follow indexed first_seq,kind,key"
    );
    assert_eq!(first["next"]["first_seq"], "1");
    // Negative control: later keys cannot enter a frozen snapshot.
    let generation = writer.mark_sync_dirty("project", "0", NOW).unwrap();
    writer
        .reconcile_sync_dirty("project", "0", generation, Some(json!({"key":"new"})), NOW)
        .unwrap();
    let frozen = read
        .read(|tx| tx.sync_device_snapshot(&lease.id, None, 500, "epoch", NOW))
        .unwrap()
        .unwrap();
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
        writer
            .reconcile_sync_dirty(
                "project",
                &key,
                generation,
                Some(json!({"summary":"x".repeat(40_000)})),
                NOW,
            )
            .unwrap();
    }
    let lease = writer.admit_device_lease("opening", "epoch", NOW).unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let page = read
        .read(|tx| tx.sync_device_snapshot(&lease.id, None, 500, "epoch", NOW))
        .unwrap()
        .unwrap();
    assert!(serde_json::to_vec(&page).unwrap().len() <= MAX_RECOVERY_BYTES);
    assert!(
        page.get("next").is_some(),
        "budget must preserve a continuation"
    );
    // Negative control: a small complete page does not spuriously continue.
    let small = read
        .read(|tx| tx.sync_device_snapshot(&lease.id, None, 1, "epoch", NOW))
        .unwrap()
        .unwrap();
    assert_eq!(small["records"].as_array().unwrap().len(), 1);
    store
        .writer_for_test()
        .execute(
            "UPDATE sync_projection SET record=?1,record_bytes=?2 WHERE key='000'",
            rusqlite::params![
                json!({"summary":"x".repeat(MAX_RECOVERY_BYTES)}).to_string(),
                MAX_RECOVERY_BYTES as i64 + 14
            ],
        )
        .unwrap();
    let oversized = read
        .read(|tx| tx.sync_device_snapshot(&lease.id, None, 500, "epoch", NOW))
        .unwrap()
        .unwrap();
    assert_eq!(oversized["status"], "resync_required");
    assert_eq!(oversized["reason"], "over_budget");
    assert!(oversized.get("records").is_none());
}

#[tokio::test]
async fn device_expiry_check_is_after_all_snapshot_sql() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
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
    store.writer_for_test().execute_batch("ALTER TABLE sync_projection_keys RENAME TO expiry_keys;
        CREATE VIEW sync_projection_keys AS SELECT kind,key,device_scan_clock(first_seq) AS first_seq FROM expiry_keys;").unwrap();
    let checked = Arc::new(AtomicBool::new(false));
    let mut read = store.read_pool().acquire_sync().await;
    let result = read
        .read(|tx| {
            let observed = Arc::clone(&checked);
            tx.connection()
                .create_scalar_function(
                    "device_scan_clock",
                    1,
                    rusqlite::functions::FunctionFlags::SQLITE_UTF8,
                    move |context| {
                        if observed.load(Ordering::SeqCst) {
                            return Err(rusqlite::Error::UserFunctionError(Box::new(
                                std::io::Error::other("SQL after final expiry check"),
                            )));
                        }
                        context.get::<i64>(0)
                    },
                )
                .unwrap();
            tx.sync_device_snapshot_observed(&lease.id, "epoch", NOW, || {
                checked.store(true, Ordering::SeqCst);
                NOW
            })
        })
        .unwrap();
    assert!(
        result.is_ok(),
        "expiry recheck must follow lookahead and response assembly: {result:?}"
    );
    assert!(checked.load(Ordering::SeqCst));
}

#[tokio::test]
async fn device_followup_long_device_positions_remain_bounded_and_task_positions_stay_small() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    read.read(|tx| {
        let scope = Scope::new(Stream::device(), json!({"kind":"device"}));
        let position = json!({"key":"\0".repeat(4096)});
        let token = tx
            .sync_encode_continuation(&scope, "device_snapshot", &position)
            .expect("legal device keys must fit their bounded page token");
        assert!(token.len() <= 65536);
        let decoded: Continuation<serde_json::Value> = tx
            .sync_decode_continuation(&scope, "device_snapshot", &token)
            .unwrap();
        assert!(matches!(decoded,Continuation::Current(value) if value==position));
        assert!(tx
            .sync_encode_continuation(&scope, "snapshot", &position)
            .is_err());
    })
    .unwrap();
}

#[test]
fn device_followup_pressure_invalidated_leases_allow_replacement_after_gc() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    for _ in 0..DEVICE_LEASES_PER_OPENING {
        store
            .writer()
            .admit_device_lease("opening", "epoch", NOW)
            .unwrap();
    }
    store
        .writer_for_test()
        .execute(
            "UPDATE sync_state SET head=1,projection_pressure_floor=1",
            [],
        )
        .unwrap();
    for _ in 0..DEVICE_LEASES_PER_OPENING {
        store
            .writer()
            .admit_device_lease("opening", "epoch", NOW)
            .expect("invalidated leases do not consume quota");
    }
    assert!(store
        .writer()
        .admit_device_lease("opening", "epoch", NOW)
        .is_err());
}

#[test]
fn device_followup_repeated_epoch_changes_keep_lease_metadata_bounded() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    for epoch in 0..100 {
        store
            .writer()
            .admit_device_lease("opening", &format!("epoch-{epoch}"), NOW)
            .unwrap();
    }
    let rows: i64 = store
        .writer_for_test()
        .query_row("SELECT COUNT(*) FROM sync_leases", [], |row| row.get(0))
        .unwrap();
    assert!(rows <= 16, "invalid lease metadata accumulated: {rows}");
}

#[test]
fn device_followup_authoritative_reconcile_preserves_newer_per_entity_markers() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let writer = store.writer();
    writer.mark_sync_dirty("project", "one", NOW).unwrap();
    let generation = writer
        .mark_sync_dirty("registry", "lifecycle", NOW)
        .unwrap();
    let markers = writer.device_registry_markers().unwrap();
    let newer = writer.mark_sync_dirty("project", "one", NOW + 1).unwrap();
    writer
        .reconcile_device_registry(
            generation,
            &std::collections::BTreeMap::new(),
            &std::collections::BTreeMap::new(),
            &markers,
        )
        .unwrap();
    let current: i64 = store
        .writer_for_test()
        .query_row(
            "SELECT generation FROM sync_dirty WHERE kind='project' AND key='one'",
            [],
            |row| row.get(0),
        )
        .expect("newer marker must remain fenced");
    assert_eq!(current, newer);
    assert!(writer
        .admit_device_lease("opening", "epoch", NOW + 1)
        .is_err());
}

#[test]
fn device_followup_busy_writer_refuses_lease_admission_instead_of_waiting() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let held = store.writer_for_test();
    let writer = store.writer();
    let (sent, received) = std::sync::mpsc::channel();
    let caller = std::thread::spawn(move || {
        sent.send(writer.admit_device_lease("opening", "epoch", NOW))
            .unwrap()
    });
    let response = received.recv_timeout(std::time::Duration::from_secs(3));
    drop(held);
    caller.join().unwrap();
    assert!(
        matches!(response, Ok(Err(SyncError::Unavailable(_)))),
        "busy admission must refuse before the held writer is released: {response:?}"
    );
    store
        .writer()
        .admit_device_lease("opening", "epoch", NOW)
        .unwrap();
}

#[tokio::test]
async fn device_followup_dirty_fence_covers_device_continuation_helpers() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let scope = Scope::new(Stream::device(), json!({"kind":"device"}));
    let mut read = store.read_pool().acquire_sync().await;
    let token = read
        .read(|tx| tx.sync_encode_continuation(&scope, "device_snapshot", json!({"position":1})))
        .unwrap()
        .unwrap();
    store
        .writer()
        .mark_sync_dirty("registry", "lifecycle", NOW)
        .unwrap();
    read.read(|tx| {
        assert!(matches!(
            tx.sync_encode_continuation(&scope, "device_snapshot", json!({"position":1})),
            Err(SyncError::Unavailable(_))
        ));
        assert!(matches!(
            tx.sync_decode_continuation::<serde_json::Value>(&scope, "device_snapshot", &token),
            Err(SyncError::Unavailable(_))
        ));
        let task = Scope::new(
            Stream::task("task-synthetic"),
            json!({"kind":"task","task_id":"task-synthetic"}),
        );
        tx.sync_encode_continuation(&task, "snapshot", json!({"position":1}))
            .unwrap();
    })
    .unwrap();
}

#[test]
fn device_followup_external_writer_lock_refuses_metadata_admission_without_changing_write_policy() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let external = rusqlite::Connection::open(directory.path().join("build.db")).unwrap();
    external.execute_batch("BEGIN IMMEDIATE").unwrap();
    let writer = store.writer();
    let (sent, received) = std::sync::mpsc::channel();
    let caller = std::thread::spawn(move || {
        sent.send(writer.admit_device_lease("opening", "epoch", NOW))
            .unwrap()
    });
    let response = received.recv_timeout(std::time::Duration::from_secs(3));
    external.execute_batch("COMMIT").unwrap();
    caller.join().unwrap();
    assert!(
        matches!(response, Ok(Err(SyncError::Unavailable(_)))),
        "lease metadata must fail retryably while an external writer is held: {response:?}"
    );
    let timeout: i64 = store
        .writer_for_test()
        .pragma_query_value(None, "busy_timeout", |row| row.get(0))
        .unwrap();
    assert_eq!(
        timeout, 5000,
        "ordinary writes retain their existing wait policy"
    );
    store
        .writer()
        .admit_device_lease("opening", "epoch", NOW)
        .unwrap();
}

#[tokio::test]
async fn device_candidate_budget_yields_through_a_tombstone_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let writer = store.writer();
    writer
        .sync_transaction(|_, batch| {
            for index in 0..2200 {
                batch.upsert(
                    Entity::new("candidate", &format!("{index:04}")),
                    json!({"name":"temporary"}),
                    &[Stream::device()],
                );
            }
            batch.upsert(
                Entity::new("candidate", "survivor"),
                json!({"name":"live"}),
                &[Stream::device()],
            );
            Ok(())
        })
        .unwrap();
    writer
        .sync_transaction(|_, batch| {
            for index in 0..2200 {
                batch.remove(
                    Entity::new("candidate", &format!("{index:04}")),
                    &[Stream::device()],
                );
            }
            Ok(())
        })
        .unwrap();
    let lease = writer
        .admit_device_lease("candidate-budget", "epoch", NOW)
        .unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let small = read
        .read(|tx| tx.sync_device_snapshot(&lease.id, None, 1, "epoch", NOW))
        .unwrap()
        .unwrap();
    assert!(small["records"].as_array().unwrap().is_empty());
    assert_eq!(
        small["next"]["first_seq"], "4",
        "small pages have a proportional candidate budget"
    );
    let large = read
        .read(|tx| tx.sync_device_snapshot(&lease.id, None, 500, "epoch", NOW))
        .unwrap()
        .unwrap();
    assert!(large["records"].as_array().unwrap().is_empty());
    assert_eq!(
        large["next"]["first_seq"], "2000",
        "a sparse prefix must yield before scanning history"
    );
    let rest = read
        .read(|tx| {
            tx.sync_device_snapshot(
                &lease.id,
                Some((2000, "candidate", "1999")),
                500,
                "epoch",
                NOW,
            )
        })
        .unwrap()
        .unwrap();
    assert_eq!(rest["records"][0]["key"], "survivor");
    assert!(rest.get("next").is_none());
}

#[tokio::test]
async fn device_334_pending_coverage_fences_reads_continuations_and_lease_admission() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let scope = Scope::new(Stream::device(), json!({"kind":"device"}));
    let lease = store
        .writer()
        .admit_device_lease("opening", "epoch", NOW)
        .unwrap();
    let mut read = store.read_pool().acquire_sync().await;
    let token = read
        .read(|tx| tx.sync_encode_continuation(&scope, "device_snapshot", json!({"position":1})))
        .unwrap()
        .unwrap();
    store
        .writer_for_test()
        .execute(
            "INSERT INTO meta(key,value) VALUES('device_334_cleanup_position','{}')",
            [],
        )
        .unwrap();
    read.read(|tx| {
        let refused = |result: Result<(), SyncError>| assert!(matches!(result, Err(SyncError::Unavailable(ref reason)) if reason=="sync_reconciling"));
        refused(tx.sync_head(&scope, None, "epoch", NOW).map(|_| ()));
        refused(tx.sync_events(&scope, Sequence::ZERO, Sequence::ZERO, 1).map(|_| ()));
        refused(tx.sync_device_snapshot(&lease.id, None, 1, "epoch", NOW).map(|_| ()));
        refused(tx.sync_encode_continuation(&scope, "device_snapshot", json!({"position":1})).map(|_| ()));
        refused(tx.sync_decode_continuation::<serde_json::Value>(&scope, "device_snapshot", &token).map(|_| ()));
        let task = Scope::new(Stream::task("task-synthetic"), json!({"kind":"task","task_id":"task-synthetic"}));
        tx.sync_head(&task, None, "epoch", NOW).unwrap();
    }).unwrap();
    for _ in 0..20 {
        assert!(
            matches!(store.writer().admit_device_lease("opening", "epoch", NOW), Err(SyncError::Unavailable(ref reason)) if reason=="sync_reconciling")
        );
    }
    let leases: i64 = store
        .writer_for_test()
        .query_row("SELECT COUNT(*) FROM sync_leases", [], |row| row.get(0))
        .unwrap();
    assert_eq!(leases, 1, "fenced retries cannot consume lease quota");
    store
        .writer_for_test()
        .execute(
            "DELETE FROM meta WHERE key='device_334_cleanup_position'",
            [],
        )
        .unwrap();
    store
        .writer()
        .admit_device_lease("opening", "epoch", NOW)
        .unwrap();
}

#[test]
fn device_334_incomplete_coverage_fences_admission_without_cleanup_marker() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    for (key, incomplete, complete) in [
        (
            "device_owner_facts_334_position",
            r#"{"key":"","done":false}"#,
            r#"{"key":"","done":true}"#,
        ),
        (
            "device_sessions_334_position",
            r#"{"phase":2,"key":"","seq":0,"at":0,"source":0}"#,
            r#"{"phase":6,"key":"","seq":0,"at":0,"source":0}"#,
        ),
    ] {
        store
            .writer_for_test()
            .execute("UPDATE meta SET value=?2 WHERE key=?1", [key, incomplete])
            .unwrap();
        assert!(
            matches!(store.writer().admit_device_lease("opening", "epoch", NOW), Err(SyncError::Unavailable(ref reason)) if reason=="sync_reconciling"),
            "incomplete coverage must fence device reads: {key}"
        );
        store
            .writer_for_test()
            .execute("DELETE FROM meta WHERE key=?1", [key])
            .unwrap();
        assert!(
            matches!(store.writer().admit_device_lease("opening", "epoch", NOW), Err(SyncError::Unavailable(ref reason)) if reason=="sync_reconciling"),
            "missing coverage must fence device reads: {key}"
        );
        store
            .writer_for_test()
            .execute("INSERT INTO meta VALUES(?1,?2)", [key, complete])
            .unwrap();
    }
    store
        .writer()
        .admit_device_lease("opening", "epoch", NOW)
        .unwrap();
}
