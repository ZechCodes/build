//! Identical public Store fixture for approved-base/candidate write comparisons.
use build_bridge::store::Store;
use build_bridge::tracker::{Actor, Task, TaskComment};
use rusqlite::{params, Connection};
use serde_json::json;
use std::time::Instant;

const ENTRIES: usize = 10_000;
const NOW: &str = "2026-01-01T00:00:00Z";

fn comment(task: &Task, number: usize, body: String) -> TaskComment {
    serde_json::from_value(json!({
        "id":format!("tc-{number:026}"),"task_id":task.id,
        "author":{"kind":"agent","agent_id":"agent-one"},
        "body":body,"created_at":"2026-01-01T01:00:00.123Z"
    }))
    .unwrap()
}

fn remove_projection(connection: &Connection) {
    let triggers: Vec<String> = connection.prepare("SELECT name FROM sqlite_schema WHERE type='trigger' AND (name LIKE 'tracker_summary_%' OR name LIKE 'tracker_choices_%')").unwrap()
        .query_map([],|row|row.get(0)).unwrap().collect::<Result<_,_>>().unwrap();
    for trigger in triggers {
        connection
            .execute_batch(&format!("DROP TRIGGER {trigger}"))
            .unwrap();
    }
    let tables: Vec<String> = connection.prepare("SELECT name FROM sqlite_schema WHERE type='table' AND (name LIKE 'tracker_task_summar%' OR name LIKE 'tracker_task_filter_%')").unwrap()
        .query_map([],|row|row.get(0)).unwrap().collect::<Result<_,_>>().unwrap();
    for table in tables {
        connection
            .execute_batch(&format!("DROP TABLE {table}"))
            .unwrap();
    }
    connection.execute_batch("DELETE FROM meta WHERE key LIKE 'task_summary_%'; UPDATE meta SET value='19' WHERE key='schema_version'").unwrap();
}

fn high_water_kib() -> String {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|raw| {
            raw.lines()
                .find(|line| line.starts_with("VmHWM:"))
                .map(str::to_string)
        })
        .unwrap_or_else(|| "VmHWM unavailable".into())
}

fn thread_cpu_micros() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // A valid stack pointer and Linux thread CPU clock; no process state changes.
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) },
        0
    );
    time.tv_sec as u64 * 1_000_000 + time.tv_nsec as u64 / 1_000
}

#[test]
fn identical_huge_history_write_fixture() {
    println!(
        "huge_fixture api_version={}",
        build_bridge::api::API_VERSION
    );
    if let Ok(expected) = std::env::var("TASK890_EXPECTED_API") {
        assert_eq!(
            build_bridge::api::API_VERSION,
            expected,
            "the comparison must link the expected checkout, not a shared cached library"
        );
    }
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path()).unwrap();
    let mut task = Task::drafted("/summary", "huge history", Actor::User, NOW);
    task.id = "task-00000000000000000000000001".into();
    task.body = "body must never enter summary".repeat(100_000);
    task.watched = true;
    let task = store.create_tracker_task(task, &[]).unwrap();
    drop(store);
    let mut connection = Connection::open(root.path().join("build.db")).unwrap();
    remove_projection(&connection);
    let seed_started = Instant::now();
    let tx = connection.transaction().unwrap();
    let mut insert = tx
        .prepare("INSERT INTO tracker_comments(id,task_id,created_at,record) VALUES(?1,?2,?3,?4)")
        .unwrap();
    let mut history_body_bytes = 0;
    let mut history_record_bytes = 0;
    for number in 0..ENTRIES {
        let body = if number == 0 {
            "large timeline payload".repeat(100_000)
        } else {
            "a note".into()
        };
        history_body_bytes += body.len();
        let note = comment(&task, number, body);
        let record = serde_json::to_string(&note).unwrap();
        history_record_bytes += record.len();
        insert
            .execute(params![note.id, note.task_id, note.created_at, record])
            .unwrap();
    }
    drop(insert);
    tx.commit().unwrap();
    drop(connection);
    println!("huge_fixture profile={} tasks=1 history_entries={ENTRIES} task_body_bytes={} history_body_bytes={history_body_bytes} total_body_bytes={} seed_us={} before_startup_hwm={}",if cfg!(debug_assertions){"debug"}else{"release"},task.body.len(),task.body.len()+history_body_bytes,seed_started.elapsed().as_micros(),high_water_kib());
    println!("huge_fixture task_record_bytes={} history_record_bytes={history_record_bytes} append_record_bytes={}", serde_json::to_vec(&task).unwrap().len(), serde_json::to_vec(&comment(&task, 100_000, "a note".into())).unwrap().len());
    let started = Instant::now();
    let cpu = thread_cpu_micros();
    let store = Store::new(root.path()).unwrap();
    println!(
        "huge_fixture startup_us={} startup_cpu_us={} startup_hwm={}",
        started.elapsed().as_micros(),
        thread_cpu_micros() - cpu,
        high_water_kib()
    );
    let task = store.load_tracker_task(&task.id).unwrap().unwrap();
    for sample in 0..5 {
        let note = comment(&task, 100_000 + sample, "a note".into());
        let started = Instant::now();
        let cpu = thread_cpu_micros();
        store
            .save_tracker_task_activity(&task, &[note], &[])
            .unwrap();
        println!("huge_fixture append_sample={sample} history_entries_before={} append_us={} append_cpu_us={} append_hwm={}",ENTRIES+sample,started.elapsed().as_micros(),thread_cpu_micros()-cpu,high_water_kib());
    }
    if std::env::var_os("TASK890_PROFILE_SQL").is_some() {
        profile_statement_cost(root.path(), &task);
    }
    drop(store);
    let started = Instant::now();
    let store = Store::new(root.path()).unwrap();
    println!(
        "huge_fixture migrated_reopen_us={} reopen_hwm={}",
        started.elapsed().as_micros(),
        high_water_kib()
    );
    assert!(store.load_tracker_task(&task.id).unwrap().is_some());
}

fn profile_statement_cost(root: &std::path::Path, task: &Task) {
    let mut conn = Connection::open(root.join("build.db")).unwrap();
    let tx = conn.transaction().unwrap();
    let sql =
        "INSERT OR IGNORE INTO tracker_comments(id,task_id,created_at,record) VALUES(?1,?2,?3,?4)";
    let started = Instant::now();
    let mut insert = tx.prepare(sql).unwrap();
    println!(
        "sql_profile comment_prepare_us={}",
        started.elapsed().as_micros()
    );
    for sample in 0..5 {
        let note = comment(task, 200_000 + sample, "a note".into());
        let record = serde_json::to_string(&note).unwrap();
        let started = Instant::now();
        insert
            .execute(params![note.id, note.task_id, note.created_at, record])
            .unwrap();
        println!(
            "sql_profile comment_execute_sample={sample} execute_us={}",
            started.elapsed().as_micros()
        );
    }
    let started = Instant::now();
    let mut update = tx
        .prepare("UPDATE tracker_tasks SET record=record WHERE id=?1")
        .unwrap();
    println!(
        "sql_profile unchanged_task_prepare_us={}",
        started.elapsed().as_micros()
    );
    for sample in 0..5 {
        let started = Instant::now();
        update.execute([&task.id]).unwrap();
        println!(
            "sql_profile unchanged_task_execute_sample={sample} execute_us={}",
            started.elapsed().as_micros()
        );
    }
}
