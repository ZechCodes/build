//! Schema readiness is structural; no latency threshold or hidden timed warmup.
use super::*;
use crate::store::{Store, DB_FILE};

fn schema_bytes(connection: &Connection) -> i32 {
    let mut current = 0;
    let mut high_water = 0;
    // SAFETY: this live connection is exclusively borrowed by this test. The
    // handle cannot close during the call, output pointers refer to valid local
    // integers, and reset=0 reads SQLite's counters without changing the schema.
    let code = unsafe {
        rusqlite::ffi::sqlite3_db_status(
            connection.handle(),
            rusqlite::ffi::SQLITE_DBSTATUS_SCHEMA_USED,
            &mut current,
            &mut high_water,
            0,
        )
    };
    assert_eq!(code, rusqlite::ffi::SQLITE_OK);
    current
}

#[derive(Debug)]
struct Witness {
    before_bytes: i32,
    after_bytes: i32,
    before_autocommit: bool,
    after_autocommit: bool,
}

fn tracker_witness(connection: &Connection) -> Witness {
    let before_bytes = schema_bytes(connection);
    let before_autocommit = connection.is_autocommit();
    let statement = connection
        .prepare("SELECT record FROM tracker_tasks WHERE id = ?1")
        .unwrap();
    drop(statement);
    Witness {
        before_bytes,
        after_bytes: schema_bytes(connection),
        before_autocommit,
        after_autocommit: connection.is_autocommit(),
    }
}

#[test]
fn raw_reader_negative_control_allocates_schema_on_first_table_prepare() {
    let dir = tempfile::tempdir().unwrap();
    let _store = Store::new(dir.path()).unwrap();
    let connection = open_read_connection(&dir.path().join(DB_FILE)).unwrap();
    let witness = tracker_witness(&connection);
    assert!(witness.before_autocommit && witness.after_autocommit);
    assert!(witness.after_bytes > witness.before_bytes, "{witness:?}");
}

#[tokio::test]
async fn all_pooled_readers_have_loaded_schemas_before_first_table_prepare() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let pool = store.read_pool();
    let mut connections = Vec::with_capacity(READ_CONNECTIONS);
    for _ in 0..READ_CONNECTIONS {
        connections.push(pool.acquire().await);
    }
    // Holding every borrower prevents a serial test from reusing one warmed
    // connection while leaving the other seven readers cold.
    let witnesses: Vec<_> = connections
        .iter()
        .map(|connection| tracker_witness(connection.connection.as_ref().unwrap()))
        .collect();
    assert!(
        witnesses.iter().all(|witness| {
            witness.before_bytes > 0
                && witness.before_bytes == witness.after_bytes
                && witness.before_autocommit
                && witness.after_autocommit
        }),
        "pool must load every schema during startup: {witnesses:?}"
    );
}
