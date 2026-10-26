//! Persistent projection maintenance must also cover unwrapped SQL producers.
use super::*;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

fn card(conn: &Connection, id: &str) -> Value {
    let raw: String = conn
        .query_row(
            "SELECT record FROM tracker_task_summaries WHERE task_id=?1",
            [id],
            |row| row.get(0),
        )
        .unwrap();
    serde_json::from_str(&raw).unwrap()
}

fn pair(store: &Store) -> [Task; 2] {
    ["first", "second"].map(|name| {
        let mut draft = Task::drafted("/summary", name, Actor::User, support::NOW);
        draft.watched = true;
        draft.labels = vec!["Étiquette".into(), "LABEL".into(), "Label".into()];
        store.create_tracker_task(draft, &[]).unwrap()
    })
}

fn history_parity(store: &Store, id: &str) {
    let task = store.load_tracker_task(id).unwrap().unwrap();
    let history = store.load_tracker_timeline(id).unwrap();
    let row = card(&store.connection(), id);
    assert_eq!(
        row["unread_count"],
        crate::tracker::summary::unread_since_mark(&task, &history)
    );
    let session = crate::tracker::summary::task_activity_session(&task, &history);
    assert_eq!(row["last_activity_ms"], json!(session.last_activity_ms));
    assert_eq!(row["session_started_ms"], json!(session.session_started_ms));
}

#[test]
fn metadata_reparent_refreshes_old_and_new_cards_for_owned_and_raw_writers() {
    for raw in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path()).unwrap();
        let [first, second] = pair(&store);
        let external = Connection::open(dir.path().join("build.db")).unwrap();
        let owned = store.connection();
        let conn = if raw { &external } else { &owned };
        conn.execute(
            "INSERT INTO reviews(task_id,workspace_id,version,record) VALUES(?1,'workspace',1,?2)",
            params![
                first.id,
                json!({"mode":"pull_request","pull_request":{"status":"open"}}).to_string()
            ],
        )
        .unwrap();
        assert_eq!(card(conn, &first.id)["review_summary"]["status"], "open");
        let before = [card(conn, &first.id), card(conn, &second.id)];
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        conn.execute(
            "UPDATE reviews SET task_id=?1 WHERE task_id=?2",
            params![second.id, first.id],
        )
        .unwrap();
        assert!(
            card(conn, &first.id).get("review_summary").is_none(),
            "OLD review endpoint, raw={raw}"
        );
        assert_eq!(card(conn, &second.id)["review_summary"]["status"], "open");
        conn.execute_batch("ROLLBACK").unwrap();
        assert_eq!([card(conn, &first.id), card(conn, &second.id)], before);
        conn.execute(
            "UPDATE reviews SET task_id=?1 WHERE task_id=?2",
            params![second.id, first.id],
        )
        .unwrap();
        assert!(card(conn, &first.id).get("review_summary").is_none());
        assert_eq!(card(conn, &second.id)["review_summary"]["status"], "open");
        conn.execute("DELETE FROM reviews", []).unwrap();
        conn.execute("INSERT INTO tracker_delivery_intents(id,task_id,project_key,generation,operation_id,state,record) VALUES('intent',?1,'/summary',1,'operation','queued',?2)",params![first.id,json!({"target":{"kind":"agent","agent_id":"CaseSensitive"}}).to_string()]).unwrap();
        assert_eq!(
            card(conn, &first.id)["deferred_assignment"]["target"]["agent_id"],
            "CaseSensitive"
        );
        let before = [card(conn, &first.id), card(conn, &second.id)];
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        conn.execute(
            "UPDATE tracker_delivery_intents SET task_id=?1 WHERE task_id=?2",
            params![second.id, first.id],
        )
        .unwrap();
        assert!(
            card(conn, &first.id).get("deferred_assignment").is_none(),
            "OLD delivery endpoint, raw={raw}"
        );
        assert_eq!(
            card(conn, &second.id)["deferred_assignment"]["target"]["agent_id"],
            "CaseSensitive"
        );
        conn.execute_batch("ROLLBACK").unwrap();
        assert_eq!([card(conn, &first.id), card(conn, &second.id)], before);
        conn.execute(
            "UPDATE tracker_delivery_intents SET task_id=?1 WHERE task_id=?2",
            params![second.id, first.id],
        )
        .unwrap();
        assert!(card(conn, &first.id).get("deferred_assignment").is_none());
        assert_eq!(
            card(conn, &second.id)["deferred_assignment"]["target"]["agent_id"],
            "CaseSensitive"
        );
        conn.execute("UPDATE tracker_delivery_intents SET state='cancelled'", [])
            .unwrap();
        assert!(card(conn, &second.id).get("deferred_assignment").is_none());
        conn.execute("DELETE FROM tracker_delivery_intents", [])
            .unwrap();
    }
}

#[test]
fn raw_history_bulk_reparent_edits_and_deletes_are_immediate_and_match_replay() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let [first, second] = pair(&store);
    let conn = Connection::open(dir.path().join("build.db")).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    for (number, at) in [
        (1, "2026-01-02T00:00:00.123456789Z"),
        (2, "2026-01-01T01:00:00Z"),
    ] {
        let note = said(&first, &format!("tc-{number:026}"), at);
        conn.execute(
            "INSERT INTO tracker_comments(id,task_id,created_at,record) VALUES(?1,?2,?3,?4)",
            params![
                note.id,
                note.task_id,
                note.created_at,
                serde_json::to_string(&note).unwrap()
            ],
        )
        .unwrap();
    }
    assert_eq!(card(&conn, &first.id)["unread_count"], 2);
    conn.execute(
        "UPDATE tracker_comments SET task_id=?1,record=json_set(record,'$.task_id',?1)",
        [&second.id],
    )
    .unwrap();
    assert_eq!(card(&conn, &first.id)["unread_count"], 0);
    assert_eq!(card(&conn, &second.id)["unread_count"], 2);
    conn.execute("UPDATE tracker_comments SET record=json_set(record,'$.author',json('{\"kind\":\"user\"}'))",[]).unwrap();
    assert_eq!(card(&conn, &second.id)["unread_count"], 0);
    let event = crate::tracker::TaskEvent::new(
        &first.id,
        Actor::Agent {
            agent_id: "AgentCASE".into(),
        },
        crate::tracker::TaskEventKind::Assigned,
        json!({}),
        "2026-01-03T00:00:00Z",
    );
    conn.execute(
        "INSERT INTO tracker_events(id,task_id,at,record) VALUES(?1,?2,?3,?4)",
        params![
            event.id,
            event.task_id,
            event.at,
            serde_json::to_string(&event).unwrap()
        ],
    )
    .unwrap();
    assert_eq!(card(&conn, &first.id)["unread_count"], 1);
    conn.execute(
        "INSERT OR IGNORE INTO tracker_events SELECT * FROM tracker_events",
        [],
    )
    .unwrap();
    assert_eq!(
        card(&conn, &first.id)["unread_count"],
        1,
        "ignored duplicate must not increment"
    );
    conn.execute(
        "UPDATE tracker_events SET task_id=?1,record=json_set(record,'$.task_id',?1)",
        [&second.id],
    )
    .unwrap();
    assert_eq!(card(&conn, &first.id)["unread_count"], 0);
    assert_eq!(card(&conn, &second.id)["unread_count"], 1);
    conn.execute_batch("COMMIT").unwrap();
    history_parity(&store, &first.id);
    history_parity(&store, &second.id);
    conn.execute_batch("DELETE FROM tracker_comments; DELETE FROM tracker_events;")
        .unwrap();
    history_parity(&store, &first.id);
    history_parity(&store, &second.id);
}

#[test]
fn raw_transaction_readers_observe_atomic_cards_facts_and_choices() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let [task, _] = pair(&store);
    let writer = Connection::open(dir.path().join("build.db")).unwrap();
    let snapshot = Connection::open(dir.path().join("build.db")).unwrap();
    snapshot.execute_batch("BEGIN").unwrap();
    assert_eq!(card(&snapshot, &task.id)["title"], "first");
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    writer.execute("UPDATE tracker_tasks SET record=json_set(record,'$.title','inside transaction','$.labels',json('[\"Temporary\"]')) WHERE id=?1",[&task.id]).unwrap();
    let note = said(&task, "tc-00000000000000000000000100", support::NOW);
    writer
        .execute(
            "INSERT INTO tracker_comments(id,task_id,created_at,record) VALUES(?1,?2,?3,?4)",
            params![
                note.id,
                note.task_id,
                note.created_at,
                serde_json::to_string(&note).unwrap()
            ],
        )
        .unwrap();
    assert_eq!(card(&writer, &task.id)["title"], "inside transaction");
    assert_eq!(card(&writer, &task.id)["unread_count"], 1);
    assert_eq!(
        ReadTransaction::new(&writer)
            .list_task_filter_choices("/summary", "label", None, 10)
            .unwrap()
            .iter()
            .filter(|choice| choice.key == "temporary")
            .count(),
        1
    );
    assert_eq!(card(&snapshot, &task.id)["title"], "first");
    writer.execute_batch("COMMIT").unwrap();
    assert_eq!(
        card(&snapshot, &task.id)["title"],
        "first",
        "old WAL snapshot stays coherent"
    );
    snapshot.execute_batch("COMMIT").unwrap();
    assert_eq!(card(&snapshot, &task.id)["title"], "inside transaction");
    assert_eq!(card(&snapshot, &task.id)["unread_count"], 1);
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    writer
        .execute("DELETE FROM tracker_comments WHERE id=?1", [&note.id])
        .unwrap();
    writer.execute("UPDATE tracker_tasks SET record=json_set(record,'$.labels',json('[\"Rollback\"]')) WHERE id=?1",[&task.id]).unwrap();
    assert_eq!(card(&writer, &task.id)["unread_count"], 0);
    writer.execute_batch("ROLLBACK").unwrap();
    assert_eq!(card(&writer, &task.id)["unread_count"], 1);
    let keys = ReadTransaction::new(&writer)
        .list_task_filter_choices("/summary", "label", None, 10)
        .unwrap()
        .into_iter()
        .map(|choice| choice.key)
        .collect::<Vec<_>>();
    assert!(keys.contains(&"temporary".into()));
    assert!(!keys.contains(&"rollback".into()));
    history_parity(&store, &task.id);
}

#[test]
fn raw_task_import_bulk_choices_and_dependency_endpoints_are_synchronous() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let [first, second] = pair(&store);
    let mut imported = Task::drafted("/summary", "raw import", Actor::User, support::NOW);
    imported.number = 99;
    imported.labels = vec!["Label".into(), "LABEL".into()];
    let conn = Connection::open(dir.path().join("build.db")).unwrap();
    conn.execute("INSERT INTO tracker_tasks(id,project_key,number,state,status,created_at,updated_at,record) VALUES(?1,'/summary',99,'open','backlog',?2,?2,?3)",params![imported.id,support::NOW,serde_json::to_string(&imported).unwrap()]).unwrap();
    assert_eq!(card(&conn, &imported.id)["title"], "raw import");
    let refs: i64=conn.query_row("SELECT refs FROM tracker_task_filter_values WHERE project_key='/summary' AND dimension='label' AND choice_key='label'",[],|row|row.get(0)).unwrap();
    assert_eq!(
        refs, 3,
        "duplicate spellings contribute one membership per task"
    );
    conn.execute("INSERT INTO tracker_task_dependencies(task_id,waiting_on_task_id,position,task_number) VALUES(?1,?2,0,?3)",params![second.id,first.id,first.number as i64]).unwrap();
    assert_eq!(card(&conn, &second.id)["dependencies"]["blocked"], true);
    assert_eq!(card(&conn, &first.id)["dependencies"]["blocks_count"], 1);
    conn.execute(
        "UPDATE tracker_task_dependencies SET task_id=?1,waiting_on_task_id=?2,task_number=?3",
        params![first.id, imported.id, imported.number as i64],
    )
    .unwrap();
    assert_eq!(
        card(&conn, &second.id)["dependencies"]["waiting_on_count"],
        0
    );
    assert_eq!(card(&conn, &first.id)["dependencies"]["blocks_count"], 0);
    assert_eq!(card(&conn, &first.id)["dependencies"]["blocked"], true);
    assert_eq!(card(&conn, &imported.id)["dependencies"]["blocks_count"], 1);
    conn.execute("UPDATE tracker_tasks SET state='closed',record=json_set(record,'$.state','closed') WHERE id=?1",[&imported.id]).unwrap();
    assert_eq!(card(&conn, &first.id)["dependencies"]["blocked"], false);
    conn.execute("DELETE FROM tracker_task_dependencies", [])
        .unwrap();
    assert_eq!(card(&conn, &imported.id)["dependencies"]["blocks_count"], 0);
    conn.execute(
        "UPDATE tracker_tasks SET record=json_set(record,'$.labels',json('[\"Replacement\"]'))",
        [],
    )
    .unwrap();
    let choices = ReadTransaction::new(&conn)
        .list_task_filter_choices("/summary", "label", None, 10)
        .unwrap();
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].key, "replacement");
    conn.execute("UPDATE tracker_tasks SET record=record", [])
        .unwrap();
    let refs: i64 = conn
        .query_row(
            "SELECT refs FROM tracker_task_filter_values WHERE dimension='label'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(refs, 3);
    conn.execute("DELETE FROM tracker_tasks", []).unwrap();
    for table in [
        "tracker_task_summaries",
        "tracker_task_summary_facts",
        "tracker_task_filter_choices",
        "tracker_task_filter_values",
    ] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "raw bulk delete left {table}");
    }
}

#[test]
fn raw_task_deletion_refreshes_counterparts_without_foreign_key_cascades() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let [blocker, dependent] = pair(&store);
    let conn = Connection::open(dir.path().join("build.db")).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE")
        .unwrap();
    conn.execute("INSERT INTO tracker_task_dependencies(task_id,waiting_on_task_id,position,task_number) VALUES(?1,?2,0,?3)",params![dependent.id,blocker.id,blocker.number as i64]).unwrap();
    assert_eq!(card(&conn, &dependent.id)["dependencies"]["blocked"], true);
    conn.execute("DELETE FROM tracker_tasks WHERE id=?1", [&blocker.id])
        .unwrap();
    // The raw connection retains its canonical dangling edge, whose join has
    // no unresolved blocker. Projection counts still describe those rows.
    assert_eq!(
        card(&conn, &dependent.id)["dependencies"],
        json!({"blocked":false,"waiting_on_count":1,"blocks_count":0})
    );
    conn.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        card(&conn, &dependent.id)["dependencies"]["waiting_on_count"],
        0
    );
    assert_eq!(card(&conn, &blocker.id)["title"], "first");
}

#[test]
fn raw_identity_owner_mutations_and_rollback_are_immediately_visible() {
    let dir = tempfile::tempdir().unwrap();
    let _store = Store::new(dir.path()).unwrap();
    let conn = Connection::open(dir.path().join("build.db")).unwrap();
    conn.execute(
        "INSERT INTO implementations(id,created_at,updated_at,record) VALUES('OwnerCASE',?1,?1,?2)",
        params![support::NOW, json!({"worktree_name":"before"}).to_string()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO agents(id,owner_id,ordinal,record) VALUES('AgentCASE','OwnerCASE',4,?1)",
        [json!({"name":"original","choice":{"provider":"codex_app_server"}}).to_string()],
    )
    .unwrap();
    let identity = |id: &str| {
        ReadTransaction::new(&conn)
            .task_summary_identity(id)
            .unwrap()
    };
    assert_eq!(
        identity("AgentCASE").unwrap().workspace_name.as_deref(),
        Some("before")
    );
    assert!(identity("agentcase").is_none(), "opaque case-sensitive key");
    conn.execute_batch("CREATE TRIGGER forbid_transcript_identity_write BEFORE UPDATE ON tracker_task_summary_identities BEGIN SELECT RAISE(ABORT,'transcript-only identity write'); END;").unwrap();
    conn.execute(
        "UPDATE agents SET record=json_set(record,'$.conversation_id','new transcript')",
        [],
    )
    .unwrap();
    conn.execute_batch("DROP TRIGGER forbid_transcript_identity_write")
        .unwrap();
    conn.execute_batch("BEGIN").unwrap();
    conn.execute(
        "UPDATE implementations SET record=json_set(record,'$.worktree_name','inside')",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE agents SET record=json_set(record,'$.name','inside')",
        [],
    )
    .unwrap();
    assert_eq!(
        identity("AgentCASE").unwrap().workspace_name.as_deref(),
        Some("inside")
    );
    assert_eq!(
        identity("AgentCASE").unwrap().name.as_deref(),
        Some("inside")
    );
    conn.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        identity("AgentCASE").unwrap().name.as_deref(),
        Some("original")
    );
    conn.execute("DELETE FROM implementations", []).unwrap();
    assert_eq!(identity("AgentCASE").unwrap().workspace_name, None);
    conn.execute(
        "UPDATE agents SET id='RenamedCASE' WHERE id='AgentCASE'",
        [],
    )
    .unwrap();
    assert!(
        identity("AgentCASE").is_none(),
        "OLD agent key must be removed"
    );
    assert_eq!(
        identity("RenamedCASE").unwrap().name.as_deref(),
        Some("original")
    );
    conn.execute("DELETE FROM agents", []).unwrap();
    assert!(identity("RenamedCASE").is_none());
}

#[test]
fn injected_projection_failure_rolls_back_raw_canonical_statement() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let [task, _] = pair(&store);
    let conn = Connection::open(dir.path().join("build.db")).unwrap();
    let before = card(&conn, &task.id);
    conn.execute_batch("CREATE TRIGGER reject_fact BEFORE INSERT ON tracker_task_summary_facts BEGIN SELECT RAISE(ABORT,'injected failure'); END; BEGIN;").unwrap();
    let note = said(&task, "tc-00000000000000000000000101", support::NOW);
    let failed = conn.execute(
        "INSERT INTO tracker_comments(id,task_id,created_at,record) VALUES(?1,?2,?3,?4)",
        params![
            note.id,
            note.task_id,
            note.created_at,
            serde_json::to_string(&note).unwrap()
        ],
    );
    assert!(failed.is_err());
    assert_eq!(card(&conn, &task.id), before);
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM tracker_comments WHERE id=?1",
            [&note.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 0,
        "canonical insert must roll back with its projection"
    );
    conn.execute_batch("COMMIT; DROP TRIGGER reject_fact;")
        .unwrap();
    assert_eq!(card(&store.connection(), &task.id), before);
}

#[test]
fn unchanged_task_activity_save_avoids_rewriting_the_canonical_task() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let [task, _] = pair(&store);
    store.connection().execute_batch("CREATE TRIGGER reject_unchanged_task BEFORE UPDATE ON tracker_tasks BEGIN SELECT RAISE(ABORT,'unchanged task rewritten'); END;").unwrap();
    store
        .save_tracker_task_activity(
            &task,
            &[said(&task, "tc-00000000000000000000000105", support::NOW)],
            &[],
        )
        .unwrap();
    history_parity(&store, &task.id);
    store
        .connection()
        .execute_batch("DROP TRIGGER reject_unchanged_task")
        .unwrap();
    let mut changed = task.clone();
    changed.title = "real change".into();
    changed.status = "done".into();
    changed.labels = vec!["Changed".into()];
    store
        .save_tracker_task_activity(&changed, &[], &[])
        .unwrap();
    assert_eq!(card(&store.connection(), &task.id)["title"], "real change");
    assert_eq!(card(&store.connection(), &task.id)["status"], "done");
    assert_eq!(
        store.load_tracker_task(&task.id).unwrap().unwrap().labels,
        vec!["Changed"]
    );
    history_parity(&store, &task.id);
}

#[test]
fn task_metadata_changes_reuse_retained_activity_until_context_inputs_change() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let [mut task, _] = pair(&store);
    let note = said(&task, "tc-00000000000000000000000106", support::NOW);
    store
        .save_tracker_task_activity(&task, std::slice::from_ref(&note), &[])
        .unwrap();
    let conn = Connection::open(dir.path().join("build.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_counter_replay BEFORE UPDATE OF unread_count,unread_agent_comments,unread_mentions,session_started_ms,last_activity_ms,done_at ON tracker_task_summary_context BEGIN SELECT RAISE(ABORT,'unrelated counter replay'); END;").unwrap();
    task.title = "changed title".into();
    task.updated_at = "2026-01-02T00:00:00Z".into();
    task.body = "new unrelated payload".repeat(1000);
    task.labels = vec!["Changed".into()];
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_eq!(card(&conn, &task.id)["title"], "changed title");
    assert_eq!(card(&conn, &task.id)["unread_count"], 1);
    history_parity(&store, &task.id);
    conn.execute_batch("DROP TRIGGER reject_counter_replay")
        .unwrap();
    task.read_through = Some(note.id);
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_eq!(card(&conn, &task.id)["unread_count"], 0);
    history_parity(&store, &task.id);
}

#[test]
fn shared_helper_view_changes_reconcile_missed_writes_before_admission() {
    let dir = tempfile::tempdir().unwrap();
    let task = {
        let store = Store::new(dir.path()).unwrap();
        let [first, _] = pair(&store);
        first
    };
    {
        let conn = Connection::open(dir.path().join("build.db")).unwrap();
        let columns = conn
            .prepare("PRAGMA table_info(tracker_summary_task)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            !columns.is_empty(),
            "shared task definition must be persistent"
        );
        let empty = columns
            .iter()
            .map(|column| format!("NULL AS \"{}\"", column.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(",");
        conn.execute_batch(&format!("DROP VIEW tracker_summary_task; CREATE VIEW tracker_summary_task AS SELECT {empty} WHERE 0;")).unwrap();
        conn.execute("UPDATE tracker_tasks SET record=json_set(record,'$.title','changed under broken helper') WHERE id=?1",[&task.id]).unwrap();
    }
    let repaired = Store::new(dir.path()).unwrap();
    assert_eq!(
        card(&repaired.connection(), &task.id)["title"],
        "changed under broken helper"
    );
    let note = said(&task, "tc-00000000000000000000000102", support::NOW);
    repaired
        .save_tracker_task_activity(&task, &[note], &[])
        .unwrap();
    history_parity(&repaired, &task.id);
}

#[test]
fn missing_shared_views_and_typed_context_rebuild_before_readers_are_published() {
    for (kind, name) in [
        ("VIEW", "tracker_summary_activity"),
        ("TABLE", "tracker_task_summary_context"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let task = {
            let store = Store::new(dir.path()).unwrap();
            let [first, _] = pair(&store);
            store
                .save_tracker_task_activity(
                    &first,
                    &[said(&first, "tc-00000000000000000000000103", support::NOW)],
                    &[],
                )
                .unwrap();
            first
        };
        Connection::open(dir.path().join("build.db"))
            .unwrap()
            .execute_batch(&format!("DROP {kind} {name}"))
            .unwrap();
        let repaired = Store::new(dir.path()).unwrap();
        assert_eq!(
            card(&repaired.connection(), &task.id)["unread_count"],
            1,
            "{kind} {name}"
        );
        history_parity(&repaired, &task.id);
        repaired
            .save_tracker_task_activity(
                &task,
                &[said(&task, "tc-00000000000000000000000104", support::NOW)],
                &[],
            )
            .unwrap();
        history_parity(&repaired, &task.id);
    }
}
