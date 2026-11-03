//! Partial startup must refuse without repairing metadata or changing any rows.
use super::*;
use crate::store::Store;
use rusqlite::{types::Value as SqlValue, Connection};
use std::collections::BTreeMap;

type Rows = BTreeMap<String, Vec<Vec<SqlValue>>>;

fn rows(conn: &Connection) -> Rows {
    let names = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    names
        .into_iter()
        .map(|name| {
            let quoted = name.replace('"', "\"\"");
            let mut statement = conn
                .prepare(&format!("SELECT * FROM \"{quoted}\""))
                .unwrap();
            let columns = statement.column_count();
            let mut values = statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|i| row.get::<_, SqlValue>(i))
                        .collect::<Result<Vec<_>, _>>()
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            values.sort_by_key(|row| format!("{row:?}"));
            (name, values)
        })
        .collect()
}

fn refuses_without_repair(corrupt: &str) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path()).unwrap();
    store
        .writer()
        .sync_transaction(|_, batch| {
            batch.upsert(
                Entity::new("fixture", "startup"),
                serde_json::json!({"value":1}),
                &[Stream::device()],
            );
            Ok(())
        })
        .unwrap();
    drop(store);
    let path = root.path().join("build.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(corrupt).unwrap();
    let before = rows(&conn);
    drop(conn);
    let result = Store::new(root.path());
    assert!(
        matches!(result, Err(crate::store::StoreError::SyncInvalid(_))),
        "partial sync storage was accepted"
    );
    let after = Connection::open(path).unwrap();
    assert_eq!(
        before,
        rows(&after),
        "refusal must not change any stored row"
    );
}

#[test]
fn f1_missing_incarnation_refuses_without_repair() {
    refuses_without_repair("DELETE FROM meta WHERE key='sync_incarnation'");
}
#[test]
fn f1_missing_cursor_key_refuses_without_repair() {
    refuses_without_repair("DELETE FROM meta WHERE key='sync_cursor_key'");
}
#[test]
fn f1_missing_schema_refuses_without_repair() {
    refuses_without_repair("DELETE FROM meta WHERE key='sync_schema'");
}
#[test]
fn f1_missing_file_binding_refuses_without_repair() {
    refuses_without_repair("DELETE FROM meta WHERE key='sync_file_binding'");
}
#[test]
fn f1_missing_singleton_refuses_without_repair() {
    refuses_without_repair("DELETE FROM sync_state");
}
#[test]
fn f1_floor_ahead_of_head_refuses_without_repair() {
    refuses_without_repair("UPDATE sync_state SET floor=head+1");
}
#[test]
fn f1_head_behind_journal_refuses_without_repair() {
    refuses_without_repair("UPDATE sync_state SET head=0");
}
#[test]
fn f1_negative_counters_refuse_without_repair() {
    refuses_without_repair("UPDATE sync_state SET total_bytes=-1");
}
#[test]
fn f1_tables_without_meta_refuse_without_repair() {
    refuses_without_repair("DELETE FROM meta WHERE key IN ('sync_incarnation','sync_cursor_key','sync_schema','sync_file_binding')");
}
#[test]
fn f1_meta_with_missing_table_refuses_without_repair() {
    refuses_without_repair("DROP TABLE sync_routes");
}

#[test]
fn f1_pristine_upgrade_preserves_existing_data() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::new(root.path()).unwrap();
    drop(store);
    let conn = Connection::open(root.path().join("build.db")).unwrap();
    conn.execute_batch("DROP TABLE sync_routes; DROP TABLE sync_stream_state; DROP TABLE sync_revisions; DROP TABLE sync_journal; DROP TABLE sync_state; DELETE FROM meta WHERE key IN ('sync_incarnation','sync_cursor_key','sync_schema','sync_file_binding'); INSERT INTO meta(key,value) VALUES('synthetic_pre_sync','kept');").unwrap();
    drop(conn);
    let upgraded = Store::new(root.path()).unwrap();
    assert_eq!(
        upgraded
            .connection()
            .query_row(
                "SELECT value FROM meta WHERE key='synthetic_pre_sync'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "kept"
    );
    assert_eq!(
        upgraded
            .connection()
            .query_row("SELECT COUNT(*) FROM sync_state", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
}
