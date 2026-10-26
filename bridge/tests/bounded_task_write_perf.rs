//! Identical public Store fixture for approved-base/candidate write comparisons.
use build_bridge::store::Store;
use build_bridge::tracker::{Actor, Task, TaskComment};
use rusqlite::{params, Connection};
use serde_json::json;
use std::time::Instant;

const ENTRIES: usize = 10_000;
const REOPEN_SAMPLES: usize = 5;
const WRITES_PER_REOPEN: usize = 3;
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
    // A pre-summary store has no helpers referring to projection tables.
    let views: Vec<String> = connection
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type='view' AND name LIKE 'tracker_summary_%'",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for view in views {
        connection
            .execute_batch(&format!("DROP VIEW \"{}\"", view.replace('"', "\"\"")))
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

fn assert_api_version(fixture: &str) {
    println!("{fixture} api_version={}", build_bridge::api::API_VERSION);
    if let Ok(expected) = std::env::var("TASK890_EXPECTED_API") {
        assert_eq!(
            build_bridge::api::API_VERSION,
            expected,
            "the comparison must link the expected checkout, not a shared cached library"
        );
    }
}

fn completed_reopen_samples(
    root: &std::path::Path,
    task_id: &str,
    fixture: &str,
    history_entries: usize,
) {
    for reopen_sample in 0..REOPEN_SAMPLES {
        let started = Instant::now();
        let cpu = thread_cpu_micros();
        let store = Store::new(root).unwrap();
        println!(
            "{fixture} migrated_reopen_us={} migrated_reopen_cpu_us={} completed_reopen_sample={reopen_sample} reopen_hwm={}",
            started.elapsed().as_micros(),
            thread_cpu_micros() - cpu,
            high_water_kib()
        );
        let task = store.load_tracker_task(task_id).unwrap().unwrap();
        for sample in 0..WRITES_PER_REOPEN {
            let offset = reopen_sample * WRITES_PER_REOPEN + sample;
            let note = comment(&task, 100_005 + offset, "a note".into());
            let kind = if sample == 0 { "first" } else { "warm" };
            let started = Instant::now();
            let cpu = thread_cpu_micros();
            store
                .save_tracker_task_activity(&task, &[note], &[])
                .unwrap();
            println!(
                "{fixture} completed_reopen_sample={reopen_sample} post_reopen_append_sample={sample} post_reopen_write_kind={kind} history_entries_before={} post_reopen_write_us={} post_reopen_write_cpu_us={} append_hwm={}",
                history_entries + offset,
                started.elapsed().as_micros(),
                thread_cpu_micros() - cpu,
                high_water_kib()
            );
        }
        drop(store);
    }
}

#[test]
fn identical_huge_history_write_fixture() {
    assert_api_version("huge_fixture");
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
    completed_reopen_samples(root.path(), &task.id, "huge_fixture", ENTRIES + 5);
}

#[test]
fn identical_small_task_write_fixture() {
    assert_api_version("small_fixture");
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path()).unwrap();
    let mut task = Task::drafted("/summary", "small task", Actor::User, NOW);
    task.id = "task-00000000000000000000000002".into();
    task.body = "a small task body".into();
    task.watched = true;
    let task = store.create_tracker_task(task, &[]).unwrap();
    drop(store);
    let connection = Connection::open(root.path().join("build.db")).unwrap();
    remove_projection(&connection);
    drop(connection);
    println!(
        "small_fixture profile={} tasks=1 history_entries=0 task_body_bytes={} history_body_bytes=0 total_body_bytes={} before_startup_hwm={}",
        if cfg!(debug_assertions) { "debug" } else { "release" },
        task.body.len(),
        task.body.len(),
        high_water_kib()
    );
    println!(
        "small_fixture task_record_bytes={} history_record_bytes=0 append_record_bytes={}",
        serde_json::to_vec(&task).unwrap().len(),
        serde_json::to_vec(&comment(&task, 100_000, "a note".into()))
            .unwrap()
            .len()
    );
    let started = Instant::now();
    let cpu = thread_cpu_micros();
    let store = Store::new(root.path()).unwrap();
    println!(
        "small_fixture startup_us={} startup_cpu_us={} startup_hwm={}",
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
        println!(
            "small_fixture append_sample={sample} history_entries_before={sample} append_us={} append_cpu_us={} append_hwm={}",
            started.elapsed().as_micros(),
            thread_cpu_micros() - cpu,
            high_water_kib()
        );
    }
    drop(store);
    completed_reopen_samples(root.path(), &task.id, "small_fixture", 5);
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

// Supplementary cases deliberately change projection inputs on every write.
// The original fixture functions above remain the unchanged comparison series.
struct MetadataFixture {
    root: tempfile::TempDir,
    task: Task,
    prefix: &'static str,
    initial_history: usize,
    sample: usize,
}

impl MetadataFixture {
    fn new(huge: bool, prefix: &'static str) -> (Self, Store) {
        assert_api_version(prefix);
        let setup = Instant::now();
        let cpu = thread_cpu_micros();
        let root = tempfile::tempdir().unwrap();
        let store = Store::new(root.path()).unwrap();
        let mut task = Task::drafted(
            "/summary",
            if huge { "huge history" } else { "small task" },
            Actor::User,
            NOW,
        );
        task.id = if huge {
            "task-00000000000000000000000001"
        } else {
            "task-00000000000000000000000002"
        }
        .into();
        task.body = if huge {
            "body must never enter summary".repeat(100_000)
        } else {
            "a small task body".into()
        };
        task.watched = true;
        let task = store.create_tracker_task(task, &[]).unwrap();
        drop(store);
        let mut connection = Connection::open(root.path().join("build.db")).unwrap();
        remove_projection(&connection);
        let initial_history = if huge { ENTRIES } else { 0 };
        let tx = connection.transaction().unwrap();
        let mut insert = tx
            .prepare(
                "INSERT INTO tracker_comments(id,task_id,created_at,record) VALUES(?1,?2,?3,?4)",
            )
            .unwrap();
        let mut history_body_bytes = 0;
        for number in 0..initial_history {
            let body = if number == 0 {
                "large timeline payload".repeat(100_000)
            } else {
                "a note".into()
            };
            history_body_bytes += body.len();
            let note = comment(&task, number, body);
            insert
                .execute(params![
                    note.id,
                    note.task_id,
                    note.created_at,
                    serde_json::to_string(&note).unwrap()
                ])
                .unwrap();
        }
        drop(insert);
        tx.commit().unwrap();
        drop(connection);
        let store = Store::new(root.path()).unwrap();
        let task = store.load_tracker_task(&task.id).unwrap().unwrap();
        println!(
            "{prefix} profile={} tasks=1 history_entries={initial_history} task_body_bytes={} history_body_bytes={history_body_bytes} total_body_bytes={} setup_us={} setup_cpu_us={} measured_writes=20",
            if cfg!(debug_assertions) { "debug" } else { "release" },
            task.body.len(), task.body.len() + history_body_bytes,
            setup.elapsed().as_micros(), thread_cpu_micros() - cpu
        );
        (
            Self {
                root,
                task,
                prefix,
                initial_history,
                sample: 0,
            },
            store,
        )
    }

    fn write(&mut self, phase: &str, cycle: usize, save: impl FnOnce(&Task, &TaskComment)) {
        let sample = self.sample;
        self.task.title = format!("Actual metadata change {sample:03}");
        self.task.labels = vec![format!("metadata-{sample:03}")];
        self.task.status = if sample % 2 == 0 {
            "in_progress"
        } else {
            "in_review"
        }
        .into();
        self.task.updated_at = format!("2026-01-01T02:00:{sample:02}Z");
        let note = comment(&self.task, 300_000 + sample, "a note".into());
        let started = Instant::now();
        let cpu = thread_cpu_micros();
        save(&self.task, &note);
        let elapsed = started.elapsed();
        let cpu_us = thread_cpu_micros() - cpu;
        println!(
            "{} phase={phase} cycle={cycle} sample={sample} history_entries_before={} task_body_bytes={} status={} write_us={} write_cpu_us={cpu_us} write_hwm={}",
            self.prefix, self.initial_history + sample, self.task.body.len(), self.task.status,
            elapsed.as_micros(), high_water_kib()
        );
        self.sample += 1;
    }

    fn verify_metadata(&self, store: &Store) {
        let actual = store.load_tracker_task(&self.task.id).unwrap().unwrap();
        assert_eq!(actual.title, self.task.title);
        assert_eq!(actual.labels, self.task.labels);
        assert_eq!(actual.status, self.task.status);
        assert_eq!(actual.updated_at, self.task.updated_at);
        assert_eq!(actual.body, self.task.body);
    }
}

fn store_metadata_series(huge: bool, prefix: &'static str) {
    let (mut fixture, store) = MetadataFixture::new(huge, prefix);
    let save = |task: &Task, note: &TaskComment| {
        store
            .save_tracker_task_activity(task, std::slice::from_ref(note), &[])
            .unwrap();
    };
    for sample in 0..5 {
        fixture.write(if sample == 0 { "first" } else { "warm" }, 0, save);
    }
    for cycle in 0..5 {
        // Independent DDL invalidates the real Store writer's cached bytecode.
        // Setup is outside the write timer and leaves the installed schema intact.
        let connection = Connection::open(fixture.root.path().join("build.db")).unwrap();
        let before: i64 = connection
            .query_row("PRAGMA schema_version", [], |row| row.get(0))
            .unwrap();
        let started = Instant::now();
        let cpu = thread_cpu_micros();
        connection.execute_batch("CREATE TABLE bounded_perf_invalidation(id INTEGER); DROP TABLE bounded_perf_invalidation;").unwrap();
        let setup_us = started.elapsed().as_micros();
        let setup_cpu_us = thread_cpu_micros() - cpu;
        let after: i64 = connection
            .query_row("PRAGMA schema_version", [], |row| row.get(0))
            .unwrap();
        assert_ne!(before, after);
        drop(connection);
        println!("{prefix} invalidation_cycle={cycle} schema_before={before} schema_after={after} invalidation_setup_us={setup_us} invalidation_setup_cpu_us={setup_cpu_us}");
        for sample in 0..3 {
            fixture.write(
                if sample == 0 {
                    "invalidated_first"
                } else {
                    "invalidated_warm"
                },
                cycle,
                save,
            );
        }
    }
    fixture.verify_metadata(&store);
}

#[test]
fn identical_small_metadata_write_fixture() {
    store_metadata_series(false, "small_metadata_fixture");
}

#[test]
fn identical_huge_metadata_write_fixture() {
    store_metadata_series(true, "huge_metadata_fixture");
}

// The base Store has no public writer-cache access. These explicitly raw SQL
// cases probe cache pressure, separately from the public Store measurements.
const CACHE_UPDATE: &str = "UPDATE tracker_tasks SET
    project_key = ?2, number = ?3, state = ?4, status = ?5,
    created_at = ?6, updated_at = ?7, record = ?8
    WHERE id = ?1 AND (project_key IS NOT ?2 OR number IS NOT ?3 OR state IS NOT ?4
        OR status IS NOT ?5 OR created_at IS NOT ?6 OR updated_at IS NOT ?7 OR record IS NOT ?8)";
const CACHE_COMMENT: &str =
    "INSERT OR IGNORE INTO tracker_comments(id,task_id,created_at,record) VALUES(?1,?2,?3,?4)";

fn cached_sql_write(connection: &mut Connection, task: &Task, note: &TaskComment) {
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let record = serde_json::to_string(task).unwrap();
    tx.prepare_cached(CACHE_UPDATE)
        .unwrap()
        .execute(params![
            task.id,
            task.project_path,
            task.number as i64,
            task.state.as_str(),
            task.status,
            task.created_at,
            task.updated_at,
            record,
        ])
        .unwrap();
    tx.prepare_cached(CACHE_COMMENT)
        .unwrap()
        .execute(params![
            note.id,
            note.task_id,
            note.created_at,
            serde_json::to_string(note).unwrap(),
        ])
        .unwrap();
    tx.commit().unwrap();
}

fn cache_eviction_series(huge: bool, prefix: &'static str) {
    let (mut fixture, store) = MetadataFixture::new(huge, prefix);
    drop(store);
    let mut connection = Connection::open(fixture.root.path().join("build.db")).unwrap();
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;",
        )
        .unwrap();
    connection.set_prepared_statement_cache_capacity(16);
    println!("{prefix} component=raw_cached_sql statement=changed_only_update cache_capacity=16 pressure_statements=64 store_writer_cache=false");
    for sample in 0..5 {
        fixture.write(
            if sample == 0 { "first" } else { "warm" },
            0,
            |task, note| cached_sql_write(&mut connection, task, note),
        );
    }
    for cycle in 0..5 {
        let started = Instant::now();
        let cpu = thread_cpu_micros();
        for number in 0..64_i64 {
            let value: i64 = connection
                .prepare_cached(&format!("SELECT {number} AS pressure_{number}"))
                .unwrap()
                .query_row([], |row| row.get(0))
                .unwrap();
            assert_eq!(value, number);
        }
        println!("{prefix} eviction_cycle={cycle} cache_capacity=16 pressure_statements=64 pressure_setup_us={} pressure_setup_cpu_us={}",started.elapsed().as_micros(),thread_cpu_micros()-cpu);
        for sample in 0..3 {
            fixture.write(
                if sample == 0 {
                    "evicted_first"
                } else {
                    "evicted_warm"
                },
                cycle,
                |task, note| cached_sql_write(&mut connection, task, note),
            );
        }
    }
    drop(connection);
    // Verification runs outside all write measurements.
    fixture.verify_metadata(&Store::new(fixture.root.path()).unwrap());
}

#[test]
fn identical_small_cache_eviction_fixture() {
    cache_eviction_series(false, "small_cache_fixture");
}

#[test]
fn identical_huge_cache_eviction_fixture() {
    cache_eviction_series(true, "huge_cache_fixture");
}
