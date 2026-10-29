//! Schema 11 (#200): the notification keys the bridge seals push content to —
//! one row per subscription, at most [`MAX_PUSH_KEYS`], the oldest evicted —
//! and a v10 store upgrading to hold them with everything it had intact.

use super::*;
use crate::store::push_keys::MAX_PUSH_KEYS;

const SCHEMA_V10: &str = include_str!("schema_v10.sql");

// Frozen whole-record shape from the schema-10 Task serializer at
// 3443235e2b5e523350d4917de20d3dd2850df548. Hoisted columns did not replace
// these required fields: even that bridge decoded Task from record alone.
// Keep this literal independent of the current serializer so newly added
// optional/defaulted fields remain absent in a genuine upgrade fixture.
const V10_TASK_RECORD: &str = r#"{"id":"task-1","project_path":"/p","number":1,"title":"kept","body":"legacy body","state":"open","status":"ready","labels":["legacy"],"priority":"high","assignee":{"kind":"user"},"links":{"workspace_ids":[],"branches":[],"commits":[],"conversation_ids":[],"parent_task_id":null},"trackers":[],"identities":{},"attachments":[],"created_by":{"kind":"user"},"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-02T00:00:00Z","closed_at":null}"#;

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
fn v10_store(dir: &std::path::Path) -> crate::tracker::Task {
    let task: crate::tracker::Task =
        serde_json::from_str(V10_TASK_RECORD).expect("the frozen v10 record is a complete task");
    let conn = rusqlite::Connection::open(dir.join("build.db")).unwrap();
    conn.execute_batch(SCHEMA_V10).unwrap();
    conn.execute_batch(
        "INSERT INTO meta (key, value) VALUES ('schema_version', '10');
         INSERT INTO meta (key, value) VALUES ('user_session', '{\"kept\":true}');
         INSERT INTO thread_items (agent_id, sequence, updated_sequence, message, item)
             VALUES ('agent-1', 1, 1, 1, '{\"type\":\"message\"}');",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO tracker_tasks
             (id, project_key, number, state, status, created_at, updated_at, record)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        rusqlite::params![
            task.id,
            task.project_path,
            task.number,
            task.state.as_str(),
            task.status,
            task.created_at,
            task.updated_at,
            V10_TASK_RECORD
        ],
    )
    .unwrap();
    task
}

fn assert_v10_data_intact(store: &Store, expected_task: &crate::tracker::Task) {
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
    assert_eq!(task, V10_TASK_RECORD, "the original task bytes survive");
    let items: Vec<(String, u64, u64, bool, String)> = conn
        .prepare("SELECT agent_id, sequence, updated_sequence, message, item FROM thread_items")
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        items,
        vec![(
            "agent-1".into(),
            1,
            1,
            true,
            "{\"type\":\"message\"}".into()
        )]
    );
    drop(conn);

    assert_eq!(
        store.load_tracker_task("task-1").unwrap().as_ref(),
        Some(expected_task),
        "the historical task remains readable through the canonical Store API"
    );
    let summaries = store.list_task_summary_candidates("/p", None, 10).unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].row["id"], expected_task.id);
    assert_eq!(summaries[0].row["title"], expected_task.title);
    assert_eq!(summaries[0].row["number"], expected_task.number);
    assert_eq!(summaries[0].row["status"], expected_task.status);
    assert_eq!(summaries[0].row["labels"], serde_json::json!(["legacy"]));
    assert_eq!(summaries[0].row["priority"], "high");
}

#[test]
fn a_v10_store_opens_at_current_schema_with_push_keys_and_its_data_intact() {
    let dir = tempfile::tempdir().unwrap();
    let expected_task = v10_store(dir.path());

    let store = Store::new(dir.path()).unwrap();
    assert_v10_data_intact(&store, &expected_task);
    assert!(
        !dir.path()
            .join(crate::store::task_rename::BACKUP_FILE)
            .exists(),
        "a v10 store is past the rename and takes no copy"
    );
    store.upsert_push_key(&sid(1), "key", 1).unwrap();
    assert_eq!(listed(&store).len(), 1, "the new table takes keys");
    drop(store);

    let reopened = Store::new(dir.path()).unwrap();
    assert_v10_data_intact(&reopened, &expected_task);
    assert_eq!(listed(&reopened), vec![(sid(1), "key".into())]);
}
