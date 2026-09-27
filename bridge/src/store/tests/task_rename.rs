//! Schema 10 (#190): a v9 store, written the way a v9 bridge wrote it, opens
//! with every issue spelled as a task — and opening it again changes nothing.

use super::*;
use crate::tracker::TimelineEntry;

const SCHEMA_V9: &str = include_str!("schema_v9.sql");

const FIRST: &str = "01M30C6A779B1R8VXRKT2C0GRM";
const PARENT: &str = "01M2ZKDW228EETGP8XEW32VP9P";
const COMMENT: &str = "01M2ZPY4QG8ECS2Y7KW8PBRNJE";
const EVENT: &str = "01M32HZAVM9S6VFQ0PCBKNS0DJ";
const PROJECT: &str = "/home/user/project";

fn issue_record(ulid: &str, number: i64, parent: Option<&str>) -> String {
    serde_json::json!({
        "id": format!("issue-{ulid}"),
        "project_path": PROJECT,
        "number": number,
        "title": "Issue creation should be inline",
        "body": "See issue-01M2ZKDW228EETGP8XEW32VP9P and #42/c/ic-7.",
        "state": "open",
        "status": "in_review",
        "labels": ["ui", "issues"],
        "priority": "medium",
        "assignee": { "kind": "agent", "agent_id": "agent-1" },
        "links": {
            "workspace_ids": ["ws-1"],
            "branches": ["build/issue-tabs"],
            "commits": [],
            "conversation_ids": ["run-1"],
            "parent_issue_id": parent.map(|ulid| format!("issue-{ulid}")),
        },
        "trackers": ["agent-1"],
        "identities": {},
        "attachments": [],
        "created_by": { "kind": "user" },
        "created_at": "2026-09-20T21:40:10Z",
        "updated_at": "2026-09-22T20:59:29Z",
        "closed_at": null,
        "watched": true,
        "read_through": format!("ic-{COMMENT}"),
    })
    .to_string()
}

/// A database exactly as a v9 bridge left it, with one of everything that
/// carries the old word.
fn v9_store(dir: &std::path::Path) {
    let conn = rusqlite::Connection::open(dir.join("build.db")).unwrap();
    conn.execute_batch(SCHEMA_V9).unwrap();
    conn.execute_batch("INSERT INTO meta (key, value) VALUES ('schema_version', '9')")
        .unwrap();
    for (ulid, number, parent) in [(PARENT, 1_i64, None), (FIRST, 2, Some(PARENT))] {
        conn.execute(
            "INSERT INTO tracker_issues (id, project_key, number, state, status, created_at, updated_at, record) \
             VALUES (?1, ?2, ?3, 'open', 'in_review', '2026-09-20T21:40:10Z', '2026-09-22T20:59:29Z', ?4)",
            rusqlite::params![format!("issue-{ulid}"), PROJECT, number, issue_record(ulid, number, parent)],
        )
        .unwrap();
    }
    let comment = serde_json::json!({
        "id": format!("ic-{COMMENT}"),
        "issue_id": format!("issue-{FIRST}"),
        "author": { "kind": "agent", "agent_id": "agent-1" },
        "body": "Is this an issue for GitHub?",
        "refs": [],
        "created_at": "2026-09-21T15:28:42Z",
        "mentions_user": true,
    });
    conn.execute(
        "INSERT INTO tracker_comments (id, issue_id, created_at, record) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            format!("ic-{COMMENT}"),
            format!("issue-{FIRST}"),
            "2026-09-21T15:28:42Z",
            comment.to_string()
        ],
    )
    .unwrap();
    let event = serde_json::json!({
        "id": format!("ie-{EVENT}"),
        "issue_id": format!("issue-{FIRST}"),
        "at": "2026-09-21T17:59:42Z",
        "actor": { "kind": "user" },
        "kind": "linked",
        "payload": { "parent_issue_id": format!("issue-{PARENT}") },
    });
    conn.execute(
        "INSERT INTO tracker_events (id, issue_id, at, record) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            format!("ie-{EVENT}"),
            format!("issue-{FIRST}"),
            "2026-09-21T17:59:42Z",
            event.to_string()
        ],
    )
    .unwrap();
    let notice = serde_json::json!({
        "type": "message",
        "data": {
            "id": "message-1",
            "from_issue": { "issue_id": format!("issue-{FIRST}"), "number": 2, "links": { "parent_issue_id": null } },
            "issue_notice": { "action": "commented", "comment_id": format!("ic-{COMMENT}") },
            "viewing_context": { "items": [{ "kind": "issue", "issue_id": format!("issue-{FIRST}") }] },
            "body": "New comment on issue #2",
        },
    });
    conn.execute(
        "INSERT INTO thread_items (agent_id, sequence, updated_sequence, message, item) VALUES ('agent-1', 1, 1, 1, ?1)",
        [notice.to_string()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO attention (entity_id, record) VALUES (?1, '{}'), ('row:proj-1:branch:build/issue-tabs', '{}')",
        [format!("issue-{FIRST}")],
    )
    .unwrap();
    let lifecycle = serde_json::json!({
        "ws-1": {
            "measured_at_ms": 1, "last_activity_ms": 1, "idle": false, "reclaimable": false,
            "holds": ["issue_open", "issues_unread"],
            "issues": [{ "issue_id": format!("issue-{FIRST}"), "number": 2, "title": "t", "status": "in_review", "state": "open" }],
            "dirty_files": 0, "unpushed_commits": 0, "behind_commits": 0, "size_bytes": null,
            "pruned_bytes": 0, "pruned_at_ms": null, "noticed_at_ms": null,
        },
    });
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('workspace_lifecycle', ?1)",
        [lifecycle.to_string()],
    )
    .unwrap();
}

fn names_in(store: &Store, kind: &str) -> Vec<String> {
    let conn = store.connection();
    let mut statement = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = ?1 ORDER BY name")
        .unwrap();
    let names = statement
        .query_map([kind], |row| row.get(0))
        .unwrap()
        .collect::<Result<Vec<String>, _>>()
        .unwrap();
    names
}

/// Every row of every table, as text: what "nothing changed" is compared on.
fn every_row(store: &Store) -> Vec<String> {
    let tables = names_in(store, "table");
    let conn = store.connection();
    let mut rows = Vec::new();
    for table in tables {
        let mut statement = conn
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .unwrap();
        let width = statement.column_count();
        let found = statement
            .query_map([], |row| {
                let values = (0..width)
                    .map(|index| {
                        row.get::<_, rusqlite::types::Value>(index)
                            .map(|value| format!("{value:?}"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(format!("{table}: {}", values.join(" | ")))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows.extend(found);
    }
    rows
}

/// A v9 store, opened once by this build.
fn migrated() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    v9_store(dir.path());
    let store = Store::new(dir.path()).unwrap();
    (dir, store)
}

#[test]
fn a_v9_store_opens_with_its_tables_and_indexes_named_for_tasks() {
    let (_dir, store) = migrated();
    let tables = names_in(&store, "table");
    assert!(tables.contains(&"tracker_tasks".to_string()), "{tables:?}");
    assert!(
        tables.iter().all(|name| !name.contains("issue")),
        "{tables:?}"
    );
    let indexes = names_in(&store, "index");
    assert!(
        indexes.iter().all(|name| !name.contains("issue")),
        "{indexes:?}"
    );
    assert!(
        indexes.contains(&"tracker_comments_by_task".to_string()),
        "{indexes:?}"
    );
    let version: String = store
        .connection()
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, SCHEMA_VERSION.to_string());
}

#[test]
fn an_issue_reads_back_as_a_task_with_its_own_words_untouched() {
    let (_dir, store) = migrated();
    let task = store
        .load_tracker_task(&format!("task-{FIRST}"))
        .unwrap()
        .expect("the task is there under its new id");
    assert_eq!(task.number, 2);
    assert_eq!(
        task.labels,
        vec!["ui", "issues"],
        "a label is somebody's word, not ours"
    );
    assert_eq!(task.links.parent_task_id, Some(format!("task-{PARENT}")));
    assert_eq!(
        task.links.branches,
        vec!["build/issue-tabs"],
        "a branch name is git's, not ours"
    );
    assert!(
        task.body.contains("issue-01M2ZKDW228EETGP8XEW32VP9P"),
        "prose keeps what it quoted"
    );
    assert_eq!(task.read_through, Some(format!("tc-{COMMENT}")));
    assert!(task.watched);
}

#[test]
fn a_timeline_reads_back_under_the_new_comment_and_event_ids() {
    let (_dir, store) = migrated();
    let timeline = store
        .load_tracker_timeline(&format!("task-{FIRST}"))
        .unwrap();
    let ids: Vec<&str> = timeline
        .iter()
        .map(|entry| match entry {
            TimelineEntry::Comment(comment) => comment.id.as_str(),
            TimelineEntry::Event(event) => event.id.as_str(),
        })
        .collect();
    assert_eq!(ids, vec![format!("tc-{COMMENT}"), format!("te-{EVENT}")]);
}

#[test]
fn a_workspace_verdict_holds_and_lists_tasks() {
    let (_dir, store) = migrated();
    let lifecycle = store.load_workspace_lifecycle().unwrap();
    assert_eq!(lifecycle["ws-1"].holds, vec!["task_open", "tasks_unread"]);
    assert_eq!(lifecycle["ws-1"].tasks[0].task_id, format!("task-{FIRST}"));
}

#[test]
fn a_conversation_item_takes_the_new_keys_and_keeps_its_words() {
    let (_dir, store) = migrated();
    let raw: String = store
        .connection()
        .query_row("SELECT item FROM thread_items", [], |row| row.get(0))
        .unwrap();
    let item: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let data = &item["data"];
    assert_eq!(data["from_task"]["task_id"], format!("task-{FIRST}"));
    assert!(data["from_task"]["links"].get("parent_task_id").is_some());
    assert_eq!(data["task_notice"]["comment_id"], format!("tc-{COMMENT}"));
    assert_eq!(data["viewing_context"]["items"][0]["kind"], "task");
    assert_eq!(
        data["body"], "New comment on issue #2",
        "a message's words are its own"
    );
}

#[test]
fn attention_moves_to_the_new_id_and_leaves_a_branch_row_alone() {
    let (_dir, store) = migrated();
    let attention: Vec<String> = store
        .connection()
        .prepare("SELECT entity_id FROM attention ORDER BY entity_id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        attention,
        vec![
            "row:proj-1:branch:build/issue-tabs".to_string(),
            format!("task-{FIRST}")
        ]
    );
}

#[test]
fn the_copy_is_of_the_store_before_anything_was_renamed() {
    let (dir, _store) = migrated();
    let backup =
        rusqlite::Connection::open(dir.path().join(crate::store::task_rename::BACKUP_FILE))
            .unwrap();
    let old_rows: i64 = backup
        .query_row("SELECT COUNT(*) FROM tracker_issues", [], |row| row.get(0))
        .unwrap();
    assert_eq!(old_rows, 2);
}

#[test]
fn running_the_rename_again_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    v9_store(dir.path());
    let store = Store::new(dir.path()).unwrap();
    let migrated = every_row(&store);
    let backup = std::fs::read(dir.path().join(crate::store::task_rename::BACKUP_FILE)).unwrap();

    // Stamped back, so the open runs the rename a second time rather than
    // skipping it by version.
    store.set_schema_version(crate::store::task_rename::TASK_RENAME_VERSION - 1);
    drop(store);
    let reopened = Store::new(dir.path()).unwrap();

    assert_eq!(every_row(&reopened), migrated);
    assert_eq!(
        std::fs::read(dir.path().join(crate::store::task_rename::BACKUP_FILE)).unwrap(),
        backup,
        "the first copy is kept, never replaced by a copy of the migrated store"
    );
}

#[test]
fn a_new_store_has_nothing_to_rename_and_takes_no_copy() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    assert!(names_in(&store, "table").contains(&"tracker_tasks".to_string()));
    assert!(!dir
        .path()
        .join(crate::store::task_rename::BACKUP_FILE)
        .exists());
}
