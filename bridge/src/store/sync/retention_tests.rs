use super::*;
use crate::store::Store;
use serde_json::json;

const NOW: i64 = 1_800_000_000_000;

fn publish(store: &Store, key: &str, value: Option<serde_json::Value>) {
    let generation = store.writer().mark_sync_dirty("fixture", key, NOW).unwrap();
    store
        .writer()
        .reconcile_sync_dirty("fixture", key, generation, value, NOW)
        .unwrap();
}

fn plan(conn: &Connection, sql: &str) -> String {
    conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap()
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join("\n")
}

#[test]
fn tombstone_candidates_seek_index_and_bound_work_before_version_filtering() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    let conn = store.writer_for_test();
    let query = "SELECT from_seq,kind,key FROM sync_projection WHERE deleted=1 AND (from_seq,kind,key)>(0,'','') AND from_seq<=999 ORDER BY from_seq,kind,key LIMIT 32";
    let indexed = plan(&conn, query);
    assert!(indexed.contains("sync_projection_tombstones"), "{indexed}");
    assert!(!indexed.contains("USE TEMP B-TREE"), "{indexed}");
    conn.execute_batch("DROP INDEX sync_projection_tombstones")
        .unwrap();
    let control = plan(&conn, query);
    assert!(
        !control.contains("sync_projection_tombstones"),
        "negative control must lose indexed seek"
    );
}

#[test]
fn retained_version_bytes_survive_restart_and_decrease_with_bounded_gc() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    publish(&store, "one", Some(json!({"version":1})));
    let bytes: i64 = store
        .writer_for_test()
        .query_row(
            "SELECT record_bytes FROM sync_projection WHERE key='one'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    publish(&store, "one", Some(json!({"version":2})));
    let retained: i64 = store
        .writer_for_test()
        .query_row("SELECT retained_version_bytes FROM sync_state", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(retained, bytes);
    drop(store);
    let store = Store::new(directory.path()).unwrap();
    let retained: i64 = store
        .writer_for_test()
        .query_row("SELECT retained_version_bytes FROM sync_state", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(retained, bytes);
    let progress = store.writer().sync_gc_batch(NOW + 1).unwrap();
    assert!(progress.changed_rows <= GC_ROWS as u64);
    let retained: i64 = store
        .writer_for_test()
        .query_row("SELECT retained_version_bytes FROM sync_state", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(retained, 0);
}

#[test]
fn retained_version_pressure_advances_durable_lease_fence_but_not_below_cap() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    publish(&store, "one", Some(json!({"version":1})));
    let lease = store
        .writer()
        .admit_device_lease("opening", "epoch", NOW)
        .unwrap();
    publish(&store, "one", Some(json!({"version":2})));
    let floor: i64 = store
        .writer_for_test()
        .query_row(
            "SELECT projection_pressure_floor FROM sync_state",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        floor, 0,
        "negative control: ordinary retention preserves active leases"
    );
    store
        .writer_for_test()
        .execute("UPDATE sync_state SET retained_version_bytes=67108864", [])
        .unwrap();
    publish(&store, "one", Some(json!({"version":3})));
    let floor: i64 = store
        .writer_for_test()
        .query_row(
            "SELECT projection_pressure_floor FROM sync_state",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(floor > lease.watermark.get());
    drop(store);
    let store = Store::new(directory.path()).unwrap();
    let survived: i64 = store
        .writer_for_test()
        .query_row(
            "SELECT projection_pressure_floor FROM sync_state",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(survived, floor);
}

#[test]
fn blocked_tombstone_prefix_yields_and_resumes_without_skipping_later_candidates() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::new(directory.path()).unwrap();
    store
        .writer()
        .admit_device_lease("opening", "epoch", NOW)
        .unwrap();
    {
        let conn = store.writer_for_test();
        conn.execute("UPDATE sync_state SET head=1000,floor=1000", [])
            .unwrap();
        for index in 0..140 {
            let key = format!("blocked-{index:03}");
            conn.execute(
                "INSERT INTO sync_projection VALUES('fixture',?1,100,1,NULL,0)",
                [&key],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO sync_projection_keys VALUES('fixture',?1,1)",
                [&key],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO sync_projection_versions VALUES('fixture',?1,1,100,0,'{}',2)",
                [&key],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO sync_projection VALUES('fixture','later',101,1,NULL,0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sync_projection_keys VALUES('fixture','later',101)",
            [],
        )
        .unwrap();
    }
    let first = store.writer().sync_gc_batch(NOW + 1).unwrap();
    assert!(first.changed_rows <= GC_ROWS as u64);
    assert_eq!(
        first.removed_projection_rows, 0,
        "blocked candidates consume the scan cap before the later eligible tombstone"
    );
    let cursor: i64 = store
        .writer_for_test()
        .query_row("SELECT gc_projection_seq FROM sync_state", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(cursor > 0);
    let mut removed = 0;
    for _ in 0..4 {
        let step = store.writer().sync_gc_batch(NOW + 1).unwrap();
        assert!(step.changed_rows <= GC_ROWS as u64);
        removed += step.removed_projection_rows;
    }
    assert_eq!(
        removed, 1,
        "the bounded seek must resume beyond blocked candidates"
    );
    assert_eq!(
        store
            .writer_for_test()
            .query_row(
                "SELECT COUNT(*) FROM sync_projection WHERE key LIKE 'blocked-%'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        140,
        "negative control: leased tombstones remain protected"
    );
}

#[test]
fn device_followup_gc_lease_seek_does_not_walk_an_expired_prefix() {
    use std::cell::Cell;
    thread_local!{static LEASE_STEPS:Cell<u64>=const{Cell::new(0)};}
    fn trace(event:rusqlite::trace::TraceEvent<'_>) {
        if let rusqlite::trace::TraceEvent::Profile(statement,_)=event {
            let sql=statement.sql();
            if sql.contains("SELECT MAX(projection_pressure_floor") {LEASE_STEPS.with(|steps|steps.set(statement.get_status(rusqlite::StatementStatus::VmStep) as u64));}
        }
    }
    let directory=tempfile::tempdir().unwrap();let store=Store::new(directory.path()).unwrap();
    {let conn=store.writer_for_test();conn.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<5000) INSERT INTO sync_leases SELECT CAST(x AS TEXT),'s','o','i','e',x,0 FROM n",[]).unwrap();
    conn.execute("INSERT INTO sync_leases VALUES('active','s','o','i','e',5001,?1)",[NOW+100]).unwrap();
    conn.trace_v2(rusqlite::trace::TraceEventCodes::SQLITE_TRACE_PROFILE,Some(trace));}
    store.writer().sync_gc_batch(NOW).unwrap();
    let steps=LEASE_STEPS.with(Cell::get);
    assert!(steps>0 && steps<200,"oldest lease seek walked expired history: {steps}");
}
