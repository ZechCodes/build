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

#[cfg(unix)]
#[test]
fn backup_refuses_untrusted_state_ancestor_before_creating_destination() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let unsafe_parent = root.path().join("untrusted-parent");
    fs::create_dir(&unsafe_parent).unwrap();
    fs::set_permissions(&unsafe_parent, fs::Permissions::from_mode(0o777)).unwrap();
    let tasks = unsafe_parent.join("tasks");
    drop(Store::new(&tasks).unwrap());
    let destination = root.path().join("backup.db");

    let output = Command::new(env!("CARGO_BIN_EXE_build-bridge"))
        .env_clear()
        .env("HOME", root.path())
        .env("BRIDGE_TASKS_DIR", &tasks)
        .arg("backup")
        .arg(&destination)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "backup accepted an untrusted ancestor");
    assert!(
        stderr.contains("untrusted-parent"),
        "refusal must identify the rejected component: {stderr}"
    );
    assert!(!destination.exists(), "refusal must not create a destination");
}

#[cfg(unix)]
#[test]
fn backup_refuses_alias_to_untrusted_task_state_root() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let root = tempfile::tempdir().unwrap();
    let unsafe_parent = root.path().join("untrusted-target");
    fs::create_dir(&unsafe_parent).unwrap();
    fs::set_permissions(&unsafe_parent, fs::Permissions::from_mode(0o777)).unwrap();
    let actual = unsafe_parent.join("tasks");
    drop(Store::new(&actual).unwrap());
    let alias = root.path().join("linked-tasks");
    symlink(&actual, &alias).unwrap();
    let destination = root.path().join("backup.db");

    let output = Command::new(env!("CARGO_BIN_EXE_build-bridge"))
        .env_clear()
        .env("HOME", root.path())
        .env("BRIDGE_TASKS_DIR", &alias)
        .arg("backup")
        .arg(&destination)
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "backup accepted an alias to an untrusted target");
    assert!(stderr.contains("untrusted-target"), "refusal must name the untrusted target: {stderr}");
    assert!(!destination.exists(), "refusal must not create a destination");
}

#[cfg(unix)]
#[test]
fn backup_accepts_alias_to_trusted_task_state_root() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let actual = root.path().join("actual-tasks");
    drop(Store::new(&actual).unwrap());
    let alias = root.path().join("linked-tasks");
    symlink(&actual, &alias).unwrap();
    let destination = root.path().join("backup.db");
    let output = Command::new(env!("CARGO_BIN_EXE_build-bridge"))
        .env_clear()
        .env("HOME", root.path())
        .env("BRIDGE_TASKS_DIR", &alias)
        .arg("backup")
        .arg(&destination)
        .output()
        .unwrap();

    assert!(output.status.success(), "backup refused a trusted target: {}", String::from_utf8_lossy(&output.stderr));
    assert!(destination.is_file());
}
