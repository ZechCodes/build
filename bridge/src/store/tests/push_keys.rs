//! Schema 11 (#200): the notification keys the bridge seals push content to —
//! one row per subscription, at most [`MAX_PUSH_KEYS`], the oldest evicted —
//! and a v10 store upgrading to hold them with everything it had intact.

use super::*;
use crate::store::push_keys::MAX_PUSH_KEYS;

const SCHEMA_V10: &str = include_str!("schema_v10.sql");

fn sid(n: usize) -> String {
    format!("{n:0>43}")
}

fn open_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    (dir, store)
}

fn listed(store: &Store) -> Vec<(String, String)> {
    let mut keys: Vec<_> = store
        .list_push_keys()
        .unwrap()
        .into_iter()
        .map(|key| (key.subscription_id, key.public_key))
        .collect();
    keys.sort();
    keys
}

#[test]
fn a_registered_key_is_listed_and_a_second_registration_replaces_it() {
    let (_dir, store) = open_store();
    store.upsert_push_key(&sid(1), "key-a", 1_000).unwrap();
    store.upsert_push_key(&sid(2), "key-b", 1_001).unwrap();
    store.upsert_push_key(&sid(1), "key-c", 1_002).unwrap();
    assert_eq!(
        listed(&store),
        vec![(sid(1), "key-c".into()), (sid(2), "key-b".into())]
    );
}

#[test]
fn a_revoked_key_is_gone_and_revoking_an_unknown_one_is_no_error() {
    let (_dir, store) = open_store();
    store.upsert_push_key(&sid(1), "key-a", 1_000).unwrap();
    store.delete_push_key(&sid(1)).unwrap();
    store.delete_push_key(&sid(9)).unwrap();
    assert!(listed(&store).is_empty());
}

#[test]
fn the_keys_the_api_reports_unknown_are_deleted_together() {
    let (_dir, store) = open_store();
    for n in 1..=4 {
        store.upsert_push_key(&sid(n), "key", n as i64).unwrap();
    }
    store.delete_push_keys(&[sid(1), sid(3), sid(99)]).unwrap();
    let left: Vec<String> = listed(&store).into_iter().map(|(id, _)| id).collect();
    assert_eq!(left, vec![sid(2), sid(4)]);
}

/// Storage is bounded: registering past the cap evicts the key registered
/// longest ago, and re-registering a key makes it the newest.
#[test]
fn registering_past_the_cap_evicts_the_oldest() {
    let (_dir, store) = open_store();
    for n in 0..MAX_PUSH_KEYS {
        store
            .upsert_push_key(&sid(n), "key", 1_000 + n as i64)
            .unwrap();
    }
    // Refreshed: sid(0) is now the newest, so sid(1) is the oldest.
    store.upsert_push_key(&sid(0), "key", 5_000).unwrap();
    store.upsert_push_key(&sid(100), "key", 5_001).unwrap();

    let left: Vec<String> = listed(&store).into_iter().map(|(id, _)| id).collect();
    assert_eq!(left.len(), MAX_PUSH_KEYS);
    assert!(left.contains(&sid(0)), "the refreshed key stays");
    assert!(left.contains(&sid(100)), "the new key is kept");
    assert!(!left.contains(&sid(1)), "the oldest is evicted");
}

/// A key's debug print never carries the key: it is what keeps forged
/// content out, so it stays out of every log.
#[test]
fn a_keys_debug_print_hides_the_key() {
    let (_dir, store) = open_store();
    store.upsert_push_key(&sid(1), "secret-point", 1).unwrap();
    let printed = format!("{:?}", store.list_push_keys().unwrap());
    assert!(!printed.contains("secret-point"), "{printed}");
}

/// A database exactly as a v10 bridge left it, with a task, a conversation
/// row and a meta record in it.
fn v10_store(dir: &std::path::Path) {
    let conn = rusqlite::Connection::open(dir.join("build.db")).unwrap();
    conn.execute_batch(SCHEMA_V10).unwrap();
    conn.execute_batch(
        "INSERT INTO meta (key, value) VALUES ('schema_version', '10');
         INSERT INTO meta (key, value) VALUES ('user_session', '{\"kept\":true}');
         INSERT INTO tracker_tasks (id, project_key, number, state, status, created_at, updated_at, record)
             VALUES ('task-1', '/p', 1, 'open', 'ready', 't0', 't1', '{\"title\":\"kept\"}');
         INSERT INTO thread_items (agent_id, sequence, updated_sequence, message, item)
             VALUES ('agent-1', 1, 1, 1, '{\"type\":\"message\"}');",
    )
    .unwrap();
}

#[test]
fn a_v10_store_opens_at_current_schema_with_push_keys_and_its_data_intact() {
    let dir = tempfile::tempdir().unwrap();
    v10_store(dir.path());

    let store = Store::new(dir.path()).unwrap();

    let conn = store.connection();
    let version: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION.to_string());
    let session: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'user_session'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(session, "{\"kept\":true}");
    let task: String = conn
        .query_row(
            "SELECT record FROM tracker_tasks WHERE id = 'task-1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(task, "{\"title\":\"kept\"}");
    let items: i64 = conn
        .query_row("SELECT COUNT(*) FROM thread_items", [], |row| row.get(0))
        .unwrap();
    assert_eq!(items, 1);
    assert!(
        !dir.path()
            .join(crate::store::task_rename::BACKUP_FILE)
            .exists(),
        "a v10 store is past the rename and takes no copy"
    );
    drop(conn);

    store.upsert_push_key(&sid(1), "key", 1).unwrap();
    assert_eq!(listed(&store).len(), 1, "the new table takes keys");
}
