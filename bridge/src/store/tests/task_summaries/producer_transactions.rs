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
