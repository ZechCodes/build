use super::*;

#[test]
fn query_projection_counts_memberships_and_generation_follow_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let before = store.task_query_basis().unwrap();
    let mut draft = Task::drafted("/query", "query", Actor::User, support::NOW);
    draft.watched = true;
    draft.links.workspace_ids = (0..25).map(|n| format!("workspace-{n}")).collect();
    draft.trackers = (0..45).map(|n| format!("tracker-{n}")).collect();
    let mut task = store.create_tracker_task(draft, &[]).unwrap();
    assert_ne!(before, store.task_query_basis().unwrap());
    assert_eq!(
        store.task_counts("/query", &[]).unwrap()[0].unfinished_open_count,
        1
    );
    let memberships:i64 = store.connection().query_row("SELECT count(*) FROM tracker_task_query_memberships WHERE task_id=?1 AND kind='workspace'",[&task.id],|r|r.get(0)).unwrap();
    assert_eq!(memberships, 25);
    let trackers:i64=store.connection().query_row("SELECT count(*) FROM tracker_task_query_memberships WHERE task_id=?1 AND kind='tracker'",[&task.id],|r|r.get(0)).unwrap();
    assert_eq!(trackers, 45);
    let note = said(&task, "tc-00000000000000000000000091", support::NOW);
    store
        .save_tracker_task_activity(&task, &[note], &[])
        .unwrap();
    assert_eq!(
        store.task_counts("/query", &[]).unwrap()[0].watched_unread_total,
        1
    );
    {
        let mut conn = store.connection();
        let tx = conn.transaction().unwrap();
        // A history producer must not parse canonical task bodies or rebuild
        // relation lists. Deliberately poison that input in a rolled-back tx.
        tx.execute(
            "UPDATE tracker_tasks SET record='{invalid json' WHERE id=?1",
            [&task.id],
        )
        .unwrap();
        projection::history_append(
            &tx,
            projection::HistoryKind::Comment,
            &task.id,
            "tc-00000000000000000000000091",
        )
        .unwrap();
    }
    task.read_through = Some("tc-00000000000000000000000091".into());
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_eq!(
        store.task_counts("/query", &[]).unwrap()[0].watched_unread_total,
        0
    );
    task.status = "done".into();
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_eq!(
        store.task_counts("/query", &[]).unwrap()[0].unfinished_open_count,
        0
    );
    assert_eq!(
        store.task_counts("/query", &[]).unwrap()[0].watched_unread_total,
        0
    );
}

#[test]
fn aggregate_contributions_reassign_delete_and_rollback_atomically() {
    use crate::tracker::Assignee;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/counts", "counts", Actor::User, support::NOW);
    draft.assignee = Some(Assignee::Agent {
        agent_id: "agent-a".into(),
    });
    let mut task = store.create_tracker_task(draft, &[]).unwrap();
    let agents = vec!["agent-a".into(), "agent-b".into()];
    assert_eq!(
        store
            .task_counts("/counts", &agents)
            .unwrap()
            .iter()
            .map(|v| v.unfinished_open_count)
            .collect::<Vec<_>>(),
        vec![1, 1, 0]
    );
    task.assignee = Some(Assignee::Agent {
        agent_id: "agent-b".into(),
    });
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_eq!(
        store
            .task_counts("/counts", &agents)
            .unwrap()
            .iter()
            .map(|v| v.unfinished_open_count)
            .collect::<Vec<_>>(),
        vec![1, 0, 1]
    );
    let before = store.task_query_basis().unwrap();
    {
        let mut conn = store.connection();
        let tx = conn.transaction().unwrap();
        projection::task_remove(&tx, &task.id).unwrap();
        // Roll back all additive contributions and generation together.
    }
    assert_eq!(store.task_query_basis().unwrap(), before);
    assert_eq!(
        store.task_counts("/counts", &agents).unwrap()[2].unfinished_open_count,
        1
    );
    let conn = store.connection();
    let detail:String=conn.query_row("EXPLAIN QUERY PLAN SELECT unfinished_open_count,watched_unread_total FROM tracker_task_query_counts WHERE project_key=?1 AND bucket=?2",["/counts","agent-b"],|r|r.get(3)).unwrap();
    assert!(detail.contains("SEARCH"), "{detail}");
    drop(conn);
    assert!(store
        .task_counts("/counts", &vec!["agent".into(); 101])
        .is_err());
    store
        .delete_tracker_tasks_of_project("/counts", |_| Ok(()))
        .unwrap();
    assert_eq!(
        store.task_counts("/counts", &agents).unwrap()[0].unfinished_open_count,
        0
    );
}

#[test]
fn snapshot_review_default_and_rebuild_keep_exact_counts() {
    let dir = tempfile::tempdir().unwrap();
    let task_id = {
        let store = Store::new(dir.path()).unwrap();
        let task = store
            .create_tracker_task(
                Task::drafted("/review-mode", "review", Actor::User, support::NOW),
                &[],
            )
            .unwrap();
        store
            .save_review_snapshot(
                &task.id,
                "workspace-1",
                0,
                crate::reviews::model::ReviewSnapshot {
                    publication: None,
                    id: "snapshot-query".into(),
                    number: 0,
                    created_at: support::NOW.into(),
                    author: Actor::User,
                    directories: vec![],
                },
            )
            .unwrap();
        let mode: String = store
            .connection()
            .query_row(
                "SELECT review_mode FROM tracker_task_query_facts WHERE task_id=?1",
                [&task.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(mode, "snapshot");
        task.id
    };
    for missing in [
        None,
        Some("tracker_task_query_facts"),
        Some("tracker_task_query_memberships"),
        Some("tracker_task_query_counts"),
    ] {
        if let Some(table) = missing {
            let conn = rusqlite::Connection::open(dir.path().join("build.db")).unwrap();
            conn.execute_batch(&format!("DROP TABLE {table}")).unwrap();
        }
        let store = Store::new(dir.path()).unwrap();
        assert_eq!(
            store.task_counts("/review-mode", &[]).unwrap()[0].unfinished_open_count,
            1,
            "repair {missing:?}"
        );
        let mode: String = store
            .connection()
            .query_row(
                "SELECT review_mode FROM tracker_task_query_facts WHERE task_id=?1",
                [&task_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(mode, "snapshot");
    }
}

#[test]
fn anchor_order_uses_valid_session_then_created_updated_and_missing() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = store
        .create_tracker_task(
            Task::drafted("/anchors", "anchors", Actor::User, support::NOW),
            &[],
        )
        .unwrap();
    let conn = store.connection();
    let created = crate::session_summary::message_millis(support::NOW).unwrap();
    let cases = [
        (
            serde_json::json!({"session_started_ms":null,"last_activity_ms":null}),
            created,
        ),
        (
            serde_json::json!({"session_started_ms":2,"last_activity_ms":3}),
            2,
        ),
        (
            serde_json::json!({"session_started_ms":4,"last_activity_ms":3}),
            created,
        ),
        (
            serde_json::json!({"session_started_ms":-9223372036854775808_i64,"last_activity_ms":3}),
            created,
        ),
        (
            serde_json::json!({"session_started_ms":null,"last_activity_ms":null,"created_at":"bad","updated_at":"2026-10-01T00:00:00Z"}),
            i64::MAX,
        ),
        (
            serde_json::json!({"session_started_ms":null,"last_activity_ms":null,"created_at":"bad","updated_at":"bad"}),
            i64::MAX,
        ),
        (
            serde_json::json!({"session_started_ms":null,"last_activity_ms":null,"created_at":"","updated_at":"2026-10-01T00:00:00Z"}),
            1790812800000,
        ),
        (
            serde_json::json!({"session_started_ms":null,"last_activity_ms":null,"created_at":"","updated_at":""}),
            i64::MAX,
        ),
        (
            serde_json::json!({"session_started_ms":2,"last_activity_ms":3,"created_at":"bad","updated_at":"bad"}),
            2,
        ),
    ];
    for (patch, want) in cases {
        conn.execute(
            "UPDATE tracker_task_summaries SET record=json_patch(record,?2) WHERE task_id=?1",
            rusqlite::params![task.id, patch.to_string()],
        )
        .unwrap();
        projection::dependency_refresh(&conn, &task.id).unwrap();
        let actual: i64 = conn
            .query_row(
                "SELECT anchor_ms FROM tracker_task_query_facts WHERE task_id=?1",
                [&task.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(actual, want, "patch={patch}");
        let invalid: bool = conn
            .query_row(
                "SELECT invalid_anchor FROM tracker_task_query_facts WHERE task_id=?1",
                [&task.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            invalid,
            patch["created_at"].as_str() == Some("bad") && want != 2,
            "patch={patch}"
        );
    }
}

#[test]
fn imported_project_reparent_moves_counts_and_all_relation_keys() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/before", "reparent", Actor::User, support::NOW);
    draft.links.workspace_ids = vec!["workspace-1".into()];
    let task = store.create_tracker_task(draft, &[]).unwrap();
    let mut conn = rusqlite::Connection::open(dir.path().join("build.db")).unwrap();
    let tx = conn.transaction().unwrap();
    tx.execute("UPDATE tracker_tasks SET project_key='/after',record=json_set(record,'$.project_path','/after') WHERE id=?1",[&task.id]).unwrap();
    projection::task_refresh(&tx, &task.id).unwrap();
    let reader = ReadTransaction::new(&tx);
    assert_eq!(
        reader.task_counts("/before", &[]).unwrap()[0].unfinished_open_count,
        0
    );
    assert_eq!(
        reader.task_counts("/after", &[]).unwrap()[0].unfinished_open_count,
        1
    );
    let project: String = tx
        .query_row(
            "SELECT project_key FROM tracker_task_query_memberships WHERE task_id=?1",
            [&task.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(project, "/after");
    tx.commit().unwrap();
    assert_eq!(
        store.task_counts("/after", &[]).unwrap()[0].unfinished_open_count,
        1
    );
}
