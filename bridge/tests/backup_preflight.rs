use build_bridge::store::Store;
use rusqlite::Connection;
use std::{fs, process::Command};

#[test]
fn backup_refuses_stale_projection_without_changing_source() {
    let root = tempfile::tempdir().unwrap();
    let tasks = root.path().join("tasks");
    drop(Store::new(&tasks).unwrap());

    let database = tasks.join("build.db");
    let connection = Connection::open(&database).unwrap();
    let changed = connection
        .execute(
            "UPDATE meta SET value='stale-projection' WHERE key='task_summary_v1_ready'",
            [],
        )
        .unwrap();
    assert_eq!(changed, 1, "the fixture must require a projection rebuild");
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    drop(connection);
    let before = fs::read(&database).unwrap();
    let destination = root.path().join("backup.db");

    let output = Command::new(env!("CARGO_BIN_EXE_build-bridge"))
        .env_clear()
        .env("HOME", root.path())
        .env("BRIDGE_TASKS_DIR", &tasks)
        .arg("backup")
        .arg(&destination)
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "backup must refuse a source requiring projection maintenance; stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        !destination.exists(),
        "refusal must not create a backup destination"
    );
    assert_eq!(fs::read(&database).unwrap(), before, "refusal changed the source database");
}

#[test]
fn backup_stale_projection_does_not_mutate_database_bytes() {
    let root = tempfile::tempdir().unwrap();
    let tasks = root.path().join("tasks");
    drop(Store::new(&tasks).unwrap());
    let database = tasks.join("build.db");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "UPDATE meta SET value='stale-projection' WHERE key='task_summary_v1_ready'",
            [],
        )
        .unwrap();
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    drop(connection);
    let before = fs::read(&database).unwrap();

    let _output = Command::new(env!("CARGO_BIN_EXE_build-bridge"))
        .env_clear()
        .env("HOME", root.path())
        .env("BRIDGE_TASKS_DIR", &tasks)
        .arg("backup")
        .arg(root.path().join("backup.db"))
        .output()
        .unwrap();

    assert_eq!(fs::read(&database).unwrap(), before, "backup changed the source database");
}

#[test]
fn backup_of_ready_database_completes_and_keeps_source_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let tasks = root.path().join("tasks");
    drop(Store::new(&tasks).unwrap());
    let database = tasks.join("build.db");
    let before = fs::read(&database).unwrap();
    let destination = root.path().join("backup.db");

    let output = Command::new(env!("CARGO_BIN_EXE_build-bridge"))
        .env_clear()
        .env("HOME", root.path())
        .env("BRIDGE_TASKS_DIR", &tasks)
        .arg("backup")
        .arg(&destination)
        .output()
        .unwrap();

    assert!(output.status.success(), "backup failed: {}", String::from_utf8_lossy(&output.stderr));
    assert!(destination.is_file());
    assert_eq!(fs::read(&database).unwrap(), before, "backup changed the source database");
}
