//! Supported Store writes maintain the projection in their canonical transaction.
use super::*;

#[test]
fn store_opens_without_installed_task_projection_triggers() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let conn = store.connection();
    let names = conn
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type='trigger' AND
             (name GLOB 'tracker_summary_*' OR name GLOB 'tracker_choices_*'
              OR sql GLOB '*tracker_task_summar*' OR sql GLOB '*tracker_task_filter_*')
             ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        names.is_empty(),
        "supported producers must call Rust maintenance, installed triggers: {names:?}"
    );
}

#[test]
fn store_task_metadata_history_and_read_markers_match_canonical_records() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/summary", "before", Actor::User, support::NOW);
    draft.watched = true;
    draft.labels = vec!["Before".into()];
    let mut task = store.create_tracker_task(draft, &[]).unwrap();
    let note = said(&task, "tc-00000000000000000000000081", support::NOW);
    store
        .save_tracker_task_activity(&task, std::slice::from_ref(&note), &[])
        .unwrap();
    assert_matches_history(&store, &task.id);
    assert_eq!(summary(&store, "/summary")["unread_count"], 1);

    task.title = "committed metadata".into();
    task.labels = vec!["After".into()];
    task.read_through = Some(note.id);
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_matches_history(&store, &task.id);
    let canonical = store.load_tracker_task(&task.id).unwrap().unwrap();
    let card = summary(&store, "/summary");
    assert_eq!(card["title"], canonical.title);
    assert_eq!(card["labels"], serde_json::json!(canonical.labels));
    assert_eq!(card["unread_count"], 0);
    let choices = store
        .list_task_filter_choices("/summary", "label", None, 10)
        .unwrap();
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].key, "after");
}

#[test]
fn store_roster_owner_changes_and_deletion_keep_identity_consistent() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut run = support::run_record("OwnerCASE", None, support::NOW);
    run.worktree_name = "before".into();
    run.agents[0].name = Some("original".into());
    let agent_id = run.agents[0].id.clone();
    store.save_run(&run).unwrap();
    let identity = store.task_summary_identity(&agent_id).unwrap().unwrap();
    assert_eq!(identity.workspace_name.as_deref(), Some("before"));
    assert_eq!(identity.name.as_deref(), Some("original"));
    run.worktree_name = "after".into();
    run.agents[0].name = Some("renamed".into());
    store.save_run(&run).unwrap();
    let identity = store.task_summary_identity(&agent_id).unwrap().unwrap();
    assert_eq!(identity.workspace_name.as_deref(), Some("after"));
    assert_eq!(identity.name.as_deref(), Some("renamed"));
    store.connection().execute_batch("CREATE TRIGGER reject_identity BEFORE UPDATE ON tracker_task_summary_identities BEGIN SELECT RAISE(ABORT,'identity failure'); END;").unwrap();
    run.worktree_name = "rolled back".into();
    assert!(store.save_run(&run).is_err());
    assert_eq!(
        store
            .task_summary_identity(&agent_id)
            .unwrap()
            .unwrap()
            .workspace_name
            .as_deref(),
        Some("after")
    );
    assert_eq!(support::reload_run(&store, &run.id).worktree_name, "after");
    store
        .connection()
        .execute_batch("DROP TRIGGER reject_identity")
        .unwrap();
    run.agents.clear();
    run.worktree_name = "after".into();
    store.save_run(&run).unwrap();
    assert!(
        store.task_summary_identity(&agent_id).unwrap().is_none(),
        "removing the last roster member must remove its identity"
    );
    store.delete_run(&run.id).unwrap();
    assert!(store.task_summary_identity(&agent_id).unwrap().is_none());
}

#[test]
fn failed_project_deletion_restores_canonical_tasks_and_all_projection_memberships() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = Task::drafted("/summary", "delete rollback", Actor::User, support::NOW);
    task.labels = vec!["Kept".into()];
    let task = store.create_tracker_task(task, &[]).unwrap();
    let before = summary(&store, "/summary");
    store.connection().execute_batch("CREATE TRIGGER reject_task_delete BEFORE DELETE ON tracker_tasks BEGIN SELECT RAISE(ABORT,'deletion failure'); END;").unwrap();
    let result = store.delete_tracker_tasks_of_project("/summary", |_| Ok(()));
    assert!(result.is_err());
    assert!(store.load_tracker_task(&task.id).unwrap().is_some());
    assert_eq!(summary(&store, "/summary"), before);
    assert_eq!(
        store
            .list_task_filter_choices("/summary", "label", None, 10)
            .unwrap()[0]
            .key,
        "kept"
    );
}

#[test]
fn explicit_import_repair_restores_consistency_after_an_unsupported_raw_write() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = store
        .create_tracker_task(
            Task::drafted("/summary", "before", Actor::User, support::NOW),
            &[],
        )
        .unwrap();
    let conn = rusqlite::Connection::open(dir.path().join("build.db")).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    conn.execute(
        "UPDATE tracker_tasks SET record=json_set(record,'$.title','imported') WHERE id=?1",
        [&task.id],
    )
    .unwrap();
    assert_eq!(
        ReadTransaction::new(&conn)
            .list_task_summary_candidates("/summary", None, 1)
            .unwrap()[0]
            .row["title"],
        "before",
        "arbitrary raw SQL no longer promises automatic maintenance"
    );
    projection::task_refresh(&conn, &task.id).unwrap();
    assert_eq!(
        ReadTransaction::new(&conn)
            .list_task_summary_candidates("/summary", None, 1)
            .unwrap()[0]
            .row["title"],
        "imported"
    );
    conn.execute_batch("COMMIT").unwrap();
    assert_eq!(
        summary(&store, "/summary")["title"],
        store.load_tracker_task(&task.id).unwrap().unwrap().title
    );
}
#[test]
fn import_repair_of_a_missing_task_removes_its_card_facts_and_memberships() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = Task::drafted("/summary", "removed import", Actor::User, support::NOW);
    task.labels = vec!["Last membership".into()];
    task.watched = true;
    let task = store.create_tracker_task(task, &[]).unwrap();
    store
        .save_tracker_task_activity(
            &task,
            &[said(&task, "tc-00000000000000000000000970", support::NOW)],
            &[],
        )
        .unwrap();
    let mut conn = store.connection();
    let tx = conn.transaction().unwrap();
    tx.execute("DELETE FROM tracker_tasks WHERE id=?1", [&task.id])
        .unwrap();
    projection::task_refresh(&tx, &task.id).unwrap();
    for table in [
        "tracker_task_summaries",
        "tracker_task_summary_context",
        "tracker_task_summary_facts",
        "tracker_task_filter_choices",
        "tracker_task_filter_values",
    ] {
        let count: i64 = tx
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0, "missing task repair retained {table}");
    }
    tx.rollback().unwrap();
    drop(conn);
    assert_matches_history(&store, &task.id);
    assert_eq!(summary(&store, "/summary")["unread_count"], 1);
}

#[test]
fn native_delivery_changes_and_projection_failure_remain_atomic() {
    use crate::store::DeliveryIntent;
    use crate::tracker::{Assignee, TaskEvent, TaskEventKind};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = store
        .create_tracker_task(
            Task::drafted("/summary", "delivery", Actor::User, support::NOW),
            &[],
        )
        .unwrap();
    task.watched = true;
    task.assignee = Some(Assignee::Agent {
        agent_id: "AgentCASE".into(),
    });
    let intent = DeliveryIntent {
        id: String::new(),
        task_id: task.id.clone(),
        project_path: task.project_path.clone(),
        generation: 0,
        target: serde_json::json!({"kind":"agent","agent_id":"AgentCASE"}),
        actor: Actor::User,
        sender: None,
        note: None,
        operation_id: String::new(),
        state: "queued".into(),
        reservation: serde_json::Value::Null,
        admitted: None,
        error: None,
    };
    let assigned = TaskEvent::new(
        &task.id,
        Actor::User,
        TaskEventKind::Assigned,
        serde_json::json!({}),
        support::NOW,
    );
    let intent = store
        .accept_delivery_intent(&task, &[assigned], intent)
        .unwrap();
    assert_eq!(
        summary(&store, "/summary")["deferred_assignment"]["target"]["agent_id"],
        "AgentCASE"
    );
    assert_eq!(
        summary(&store, "/summary")["deferred_assignment"]["state"],
        intent.state
    );
    assert_matches_history(&store, &task.id);
    let before = summary(&store, "/summary");
    store.connection().execute_batch("CREATE TRIGGER reject_metadata BEFORE UPDATE ON tracker_task_summaries BEGIN SELECT RAISE(ABORT,'delivery metadata failed'); END;").unwrap();
    assert!(store
        .fail_delivery_intent(&intent.id, intent.generation, "must rollback")
        .is_err());
    assert_eq!(
        store
            .load_delivery_intent(&intent.id)
            .unwrap()
            .unwrap()
            .state,
        intent.state
    );
    assert_eq!(summary(&store, "/summary"), before);
    store
        .connection()
        .execute_batch("DROP TRIGGER reject_metadata")
        .unwrap();
    store
        .fail_delivery_intent(&intent.id, intent.generation, "failed")
        .unwrap();
    assert_eq!(
        summary(&store, "/summary")["deferred_assignment"]["state"],
        "failed"
    );
    assert_eq!(
        summary(&store, "/summary")["deferred_assignment"]["error"],
        "failed"
    );
    assert_matches_history(&store, &task.id);
    let retried = store.retry_delivery_intent(&intent.id).unwrap();
    assert_eq!(
        summary(&store, "/summary")["deferred_assignment"]["state"],
        retried.state
    );
    task.assignee = None;
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert!(summary(&store, "/summary")
        .get("deferred_assignment")
        .is_none());
    assert_matches_history(&store, &task.id);
}
#[test]
fn public_repair_batch_is_atomic_and_rebuilds_canonical_history_without_duplicate_counts() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let tasks = ["first", "second"].map(|title| {
        let mut task = Task::drafted("/summary", title, Actor::User, support::NOW);
        task.watched = true;
        store.create_tracker_task(task, &[]).unwrap()
    });
    let conn = store.connection();
    let before: Vec<String> = conn
        .prepare("SELECT record FROM tracker_task_summaries ORDER BY task_id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    for (i, task) in tasks.iter().enumerate() {
        conn.execute(
            "UPDATE tracker_tasks SET record=json_set(record,'$.title',?1) WHERE id=?2",
            rusqlite::params![format!("imported-{i}"), task.id],
        )
        .unwrap();
        let note = said(task, &format!("tc-{:026}", i + 900), support::NOW);
        conn.execute(
            "INSERT INTO tracker_comments(id,task_id,created_at,record) VALUES(?1,?2,?3,?4)",
            rusqlite::params![
                note.id,
                note.task_id,
                note.created_at,
                serde_json::to_string(&note).unwrap()
            ],
        )
        .unwrap();
    }
    conn.execute_batch("COMMIT").unwrap();
    let mut ordered_ids = tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<Vec<_>>();
    ordered_ids.sort_unstable();
    conn.execute_batch(&format!("CREATE TRIGGER reject_repair BEFORE UPDATE ON tracker_task_summaries WHEN NEW.task_id='{}' BEGIN SELECT CASE WHEN NOT EXISTS(SELECT 1 FROM tracker_task_summaries WHERE task_id='{}' AND json_extract(record,'$.title') GLOB 'imported-*') THEN RAISE(ABORT,'first repair was not applied') END; SELECT RAISE(ABORT,'second repair fails'); END;",ordered_ids[1],ordered_ids[0])).unwrap();
    drop(conn);
    let ids = tasks.iter().map(|task| task.id.clone()).collect::<Vec<_>>();
    let error = store.repair_task_summaries(&ids).unwrap_err();
    assert!(
        error.to_string().contains("second repair fails"),
        "first task must be visibly repaired before failure on the second: {error}"
    );
    let conn = store.connection();
    let after: Vec<String> = conn
        .prepare("SELECT record FROM tracker_task_summaries ORDER BY task_id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        after, before,
        "failed second repair must roll back first task's projection too"
    );
    conn.execute_batch("DROP TRIGGER reject_repair").unwrap();
    drop(conn);
    store.repair_task_summaries(&ids).unwrap();
    store.repair_task_summaries(&ids).unwrap();
    let rows = store
        .list_task_summary_candidates("/summary", None, 10)
        .unwrap();
    for (i, task) in tasks.iter().enumerate() {
        let row = &rows
            .iter()
            .find(|row| row.row["id"] == task.id)
            .unwrap()
            .row;
        assert_eq!(row["title"], format!("imported-{i}"));
        assert_eq!(
            row["unread_count"], 1,
            "repeating explicit repair must not double count"
        );
        let facts: i64 = store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM tracker_task_summary_facts WHERE task_id=?1",
                [&task.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(facts, 1);
    }
}

#[test]
fn typed_task_maintenance_keeps_compact_fields_and_attachment_count_exact() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = Task::drafted("/summary", "compact fields", Actor::User, support::NOW);
    task.labels = vec!["One".into(), "Two".into()];
    task.attachments = (0..2)
        .map(|i| crate::thread::MessageAttachment {
            name: format!("file-{i}"),
            path: format!(".build/attachments/file-{i}"),
            mime: "text/plain".into(),
            size: 1024,
        })
        .collect();
    let mut task = store.create_tracker_task(task, &[]).unwrap();
    let row = summary(&store, "/summary");
    assert_eq!(row["attachment_count"], 2);
    assert_eq!(row["labels"], serde_json::json!(task.labels));
    assert!(row.get("attachments").is_none());
    task.attachments.pop();
    task.title = "updated compact fields".into();
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    let row = summary(&store, "/summary");
    assert_eq!(row["attachment_count"], 1);
    assert_eq!(row["title"], task.title);
    assert!(row.get("attachments").is_none());
}

#[test]
fn native_blocker_lifecycle_refreshes_counterparts_and_rolls_back_on_projection_failure() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut tasks = ["blocker", "dependent"].map(|title| {
        let mut task = Task::drafted("/summary", title, Actor::User, support::NOW);
        task.watched = true;
        store.create_tracker_task(task, &[]).unwrap()
    });
    let [blocker, dependent] = &mut tasks;
    store
        .edit_waiting_on(
            "/summary",
            &dependent.id,
            std::slice::from_ref(&blocker.id),
            &[],
            &Actor::User,
            support::NOW,
        )
        .unwrap();
    let dependency = |id: &str| {
        store
            .list_task_summary_candidates("/summary", None, 10)
            .unwrap()
            .into_iter()
            .find(|row| row.row["id"] == id)
            .unwrap()
            .row["dependencies"]
            .clone()
    };
    assert_eq!(dependency(&dependent.id)["blocked"], true);
    assert_eq!(dependency(&blocker.id)["blocks_count"], 1);
    let before = store.load_tracker_task(&blocker.id).unwrap().unwrap();
    let before_card = dependency(&dependent.id);
    store.connection().execute_batch(&format!("CREATE TRIGGER reject_counterpart BEFORE UPDATE ON tracker_task_summaries WHEN NEW.task_id='{}' BEGIN SELECT RAISE(ABORT,'counterpart projection failure'); END;",dependent.id)).unwrap();
    blocker.state = crate::tracker::TaskState::Closed;
    blocker.closed_at = Some(support::NOW.into());
    assert!(store.save_tracker_task_activity(blocker, &[], &[]).is_err());
    assert_eq!(
        store.load_tracker_task(&blocker.id).unwrap().unwrap(),
        before
    );
    assert_eq!(dependency(&dependent.id), before_card);
    store
        .connection()
        .execute_batch("DROP TRIGGER reject_counterpart")
        .unwrap();
    store.save_tracker_task_activity(blocker, &[], &[]).unwrap();
    assert_eq!(dependency(&dependent.id)["blocked"], false);
    assert_eq!(dependency(&dependent.id)["waiting_on_count"], 1);
    store
        .edit_waiting_on(
            "/summary",
            &dependent.id,
            &[],
            std::slice::from_ref(&blocker.id),
            &Actor::User,
            support::NOW,
        )
        .unwrap();
    assert_eq!(dependency(&dependent.id)["waiting_on_count"], 0);
    assert_eq!(dependency(&blocker.id)["blocks_count"], 0);
}
