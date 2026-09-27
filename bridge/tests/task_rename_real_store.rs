//! Schema 10's rename (#190), run against a copy of a real store.
//!
//! Ignored by default: it needs a store, named by `BUILD_TASK_RENAME_FIXTURE`
//! — a COPY of `~/.build/tasks`, never the live one. The test copies the
//! database out of it again, so even the fixture is left as it was, and it
//! only ever opens a `Store`: nothing here starts an agent or delivers a
//! message against a real run id.
//!
//! ```text
//! cp -a ~/.build/tasks /tmp/store-copy
//! BUILD_TASK_RENAME_FIXTURE=/tmp/store-copy cargo test --test task_rename_real_store -- --ignored --nocapture
//! ```
//!
//! "Before" is read straight out of the v9 tables by their old names, never
//! through the code under test; "after" is read through the store's own API
//! into the renamed types. Every task, comment, event, link, label, tracker
//! and the Needs-you inputs (watched, read mark, dismissal, mentions) must
//! come through equal, and a second run must change nothing.

use std::collections::BTreeMap;
use std::path::Path;

use build_bridge::renamed_ids::current_id;
use build_bridge::store::Store;
use build_bridge::tracker::TimelineEntry;
use serde_json::{json, Value};

fn fixture() -> Option<std::path::PathBuf> {
    std::env::var_os("BUILD_TASK_RENAME_FIXTURE").map(std::path::PathBuf::from)
}

fn copy_database(from: &Path, to: &Path) {
    for name in ["build.db", "build.db-wal", "build.db-shm"] {
        if from.join(name).is_file() {
            std::fs::copy(from.join(name), to.join(name)).unwrap();
        }
    }
}

fn rows(conn: &rusqlite::Connection, sql: &str) -> Vec<Value> {
    let mut statement = conn.prepare(sql).unwrap();
    let found = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(|raw| serde_json::from_str(&raw.unwrap()).unwrap())
        .collect();
    found
}

fn count(conn: &rusqlite::Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

fn id_of(value: &Value) -> Option<String> {
    value.as_str().map(current_id)
}

fn ids_of(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(id_of).collect())
        .unwrap_or_default()
}

/// One task as the facts that must survive, read from a v9 record by the old
/// key names.
fn task_facts_v9(record: &Value) -> Value {
    let links = &record["links"];
    json!({
        "id": id_of(&record["id"]),
        "number": record["number"],
        "title": record["title"],
        "body": record["body"],
        "state": record["state"],
        "status": record["status"],
        "labels": record["labels"],
        "priority": record["priority"],
        "assignee": record["assignee"],
        "workspace_ids": links["workspace_ids"],
        "branches": links["branches"],
        "commits": links["commits"],
        "conversation_ids": ids_of(&links["conversation_ids"]),
        "parent": id_of(&links["parent_issue_id"]),
        "trackers": record["trackers"],
        "identities": record["identities"].as_object().map(|map| map.len()),
        "attachments": record["attachments"].as_array().map(Vec::len),
        "watched": record["watched"].as_bool().unwrap_or(false),
        "read_through": id_of(&record["read_through"]),
        "dismissed_through": id_of(&record["dismissed_through"]),
        "created_by": record["created_by"],
        "created_at": record["created_at"],
        "closed_at": record["closed_at"],
    })
}

/// The same facts, read through the store into the renamed type.
fn task_facts(task: &build_bridge::tracker::Task) -> Value {
    let record = serde_json::to_value(task).unwrap();
    json!({
        "id": task.id,
        "number": task.number,
        "title": task.title,
        "body": task.body,
        "state": record["state"],
        "status": task.status,
        "labels": task.labels,
        "priority": record["priority"],
        "assignee": record["assignee"],
        "workspace_ids": task.links.workspace_ids,
        "branches": task.links.branches,
        "commits": task.links.commits,
        "conversation_ids": task.links.conversation_ids,
        "parent": task.links.parent_task_id,
        "trackers": task.trackers,
        "identities": task.identities.len(),
        "attachments": task.attachments.len(),
        "watched": task.watched,
        "read_through": task.read_through,
        "dismissed_through": task.dismissed_through,
        "created_by": record["created_by"],
        "created_at": task.created_at,
        "closed_at": task.closed_at,
    })
}

fn comment_facts_v9(record: &Value) -> Value {
    json!({
        "id": id_of(&record["id"]),
        "task": id_of(&record["issue_id"]),
        "author": record["author"],
        "body": record["body"],
        "refs": record["refs"].as_array().map(Vec::len),
        "attachments": record["attachments"].as_array().map_or(0, Vec::len),
        "mentions_user": record["mentions_user"].as_bool().unwrap_or(false),
        "notifies_user": record["notifies_user"].as_bool().unwrap_or(false),
        "created_at": record["created_at"],
    })
}

fn event_facts_v9(record: &Value) -> Value {
    json!({
        "id": id_of(&record["id"]),
        "task": id_of(&record["issue_id"]),
        "kind": record["kind"],
        "actor": record["actor"],
        "at": record["at"],
        "mentions_user": record["mentions_user"].as_bool().unwrap_or(false),
        "payload": record["payload"].as_object().map(|map| map.len()),
    })
}

fn entry_facts(entry: &TimelineEntry) -> Value {
    match entry {
        TimelineEntry::Comment(comment) => {
            let record = serde_json::to_value(comment).unwrap();
            json!({
                "id": comment.id,
                "task": comment.task_id,
                "author": record["author"],
                "body": comment.body,
                "refs": comment.refs.len(),
                "attachments": comment.attachments.len(),
                "mentions_user": comment.mentions_user,
                "notifies_user": comment.notifies_user,
                "created_at": comment.created_at,
            })
        }
        TimelineEntry::Event(event) => {
            let record = serde_json::to_value(event).unwrap();
            json!({
                "id": event.id,
                "task": event.task_id,
                "kind": record["kind"],
                "actor": record["actor"],
                "at": event.at,
                "mentions_user": event.mentions_user,
                "payload": event.payload.as_object().map(|map| map.len()),
            })
        }
    }
}

/// Every row of every table, as text, for "the second run changed nothing".
fn every_row(path: &Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(path).unwrap();
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut out = Vec::new();
    for table in tables {
        let mut statement = conn
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .unwrap();
        let width = statement.column_count();
        let found: Vec<String> = statement
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
            .collect::<Result<_, _>>()
            .unwrap();
        out.extend(found);
    }
    out
}

/// The table names a v9 store and a v10 store give the same rows.
const TABLES: [(&str, &str); 15] = [
    ("meta", "meta"),
    ("issues", "tasks"),
    ("implementations", "implementations"),
    ("agents", "agents"),
    ("thread_items", "thread_items"),
    ("captures", "captures"),
    ("attention", "attention"),
    ("archived_worktrees", "archived_worktrees"),
    ("operations", "operations"),
    ("agent_migration_backups", "agent_migration_backups"),
    ("tracker_issues", "tracker_tasks"),
    ("tracker_comments", "tracker_comments"),
    ("tracker_events", "tracker_events"),
    ("inbox_retained_messages", "inbox_retained_messages"),
    ("inbox_retained_run_messages", "inbox_retained_run_messages"),
];

#[test]
#[ignore = "needs BUILD_TASK_RENAME_FIXTURE: a copy of a real store"]
fn a_real_store_renames_to_tasks_with_nothing_lost_and_renames_once() {
    let Some(source) = fixture() else {
        panic!("set BUILD_TASK_RENAME_FIXTURE to a copy of ~/.build/tasks");
    };
    let work = tempfile::tempdir().unwrap();
    copy_database(&source, work.path());
    let database = work.path().join("build.db");

    // ---- before: the v9 tables, by their old names ----
    let (tasks_before, comments_before, events_before, counts_before) = {
        let conn = rusqlite::Connection::open(&database).unwrap();
        let version: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, "9", "the fixture is a v9 store");
        let tasks: BTreeMap<String, Value> = rows(&conn, "SELECT record FROM tracker_issues")
            .iter()
            .map(|record| (id_of(&record["id"]).unwrap(), task_facts_v9(record)))
            .collect();
        let comments: Vec<Value> = rows(&conn, "SELECT record FROM tracker_comments")
            .iter()
            .map(comment_facts_v9)
            .collect();
        let events: Vec<Value> = rows(&conn, "SELECT record FROM tracker_events")
            .iter()
            .map(event_facts_v9)
            .collect();
        let counts: Vec<i64> = TABLES.iter().map(|(old, _)| count(&conn, old)).collect();
        (tasks, comments, events, counts)
    };

    // ---- the rename ----
    let store = Store::new(work.path()).expect("the v9 store opens");

    // ---- after: through the store, into the renamed types ----
    let mut tasks_after = BTreeMap::new();
    let mut timeline_after = Vec::new();
    for id in tasks_before.keys() {
        let task = store
            .load_tracker_task(id)
            .unwrap()
            .unwrap_or_else(|| panic!("{id} is there under its new id"));
        tasks_after.insert(id.clone(), task_facts(&task));
        timeline_after.extend(
            store
                .load_tracker_timeline(id)
                .unwrap()
                .iter()
                .map(entry_facts),
        );
    }
    drop(store);
    let conn = rusqlite::Connection::open(&database).unwrap();
    let counts_after: Vec<i64> = TABLES.iter().map(|(_, new)| count(&conn, new)).collect();
    drop(conn);

    for (id, before) in &tasks_before {
        assert_eq!(&tasks_after[id], before, "task {id}");
    }
    let sorted = |mut facts: Vec<Value>| {
        facts.sort_by_key(|fact| fact["id"].as_str().unwrap_or_default().to_string());
        facts
    };
    let mut timeline_before = comments_before.clone();
    timeline_before.extend(events_before.iter().cloned());
    assert_eq!(
        sorted(timeline_after),
        sorted(timeline_before),
        "every comment and event"
    );
    assert_eq!(
        counts_after, counts_before,
        "every table keeps every row: {TABLES:?}"
    );

    let sum = |key: &str| -> usize {
        tasks_before
            .values()
            .map(|facts| facts[key].as_array().map_or(0, Vec::len))
            .sum()
    };
    let links = sum("workspace_ids")
        + sum("branches")
        + sum("commits")
        + sum("conversation_ids")
        + tasks_before
            .values()
            .filter(|facts| !facts["parent"].is_null())
            .count();
    let watched = tasks_before
        .values()
        .filter(|facts| facts["watched"] == true)
        .count();
    let marks = tasks_before
        .values()
        .filter(|facts| !facts["read_through"].is_null())
        .count();
    let mentions = comments_before
        .iter()
        .chain(&events_before)
        .filter(|facts| facts["mentions_user"] == true || facts["notifies_user"] == true)
        .count();
    println!(
        "renamed: {} tasks, {} comments, {} events, {links} links, {} labels, {} trackers; \
         Needs-you inputs: {watched} watched, {marks} read marks, {mentions} mentions",
        tasks_before.len(),
        comments_before.len(),
        events_before.len(),
        sum("labels"),
        sum("trackers"),
    );
    for ((old, new), rows) in TABLES.iter().zip(&counts_after) {
        println!("  {old} -> {new}: {rows} rows");
    }

    // ---- the second run: stamped back to 9, so it runs rather than skips ----
    let migrated = every_row(&database);
    let backup = work.path().join("build.db.before-tasks");
    let backup_bytes = std::fs::read(&backup).expect("the first open took a copy");
    rusqlite::Connection::open(&database)
        .unwrap()
        .execute(
            "UPDATE meta SET value = '9' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();
    drop(Store::new(work.path()).expect("the store opens a second time"));
    assert_eq!(
        every_row(&database),
        migrated,
        "a second run changes nothing"
    );
    assert_eq!(
        std::fs::read(&backup).unwrap(),
        backup_bytes,
        "and keeps the first copy"
    );
    println!("second run: {} rows, unchanged", migrated.len());
}
