use super::super::task_summaries as projection;
use super::*;
use crate::tracker::{Actor, Task};

#[path = "task_summaries/cache.rs"]
mod cache;
#[path = "task_summaries/import_repair.rs"]
mod import_repair;
#[path = "task_summaries/lifecycle.rs"]
mod lifecycle;
#[path = "task_summaries/producer_reviews.rs"]
mod producer_reviews;
#[path = "task_summaries/producer_transactions.rs"]
mod producer_transactions;

#[test]
fn bounded_summary_omits_task_payload_and_carries_watch_count() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = Task::drafted("/summary", "a card", Actor::User, support::NOW);
    task.body = "secret large body".repeat(10_000);
    task.watched = true;
    let task = store.create_tracker_task(task, &[]).unwrap();
    let connection = store.connection();
    let reader = ReadTransaction::new(&connection);
    let rows = reader
        .list_task_summary_candidates("/summary", None, 10)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].row["id"], task.id);
    assert_eq!(rows[0].row["unread_count"], 0);
    assert!(rows[0].row.get("body").is_none());
    assert!(rows[0].row.get("attachments").is_none());
    assert!(rows[0].row.get("identities").is_none());
    assert!(rows[0].row.to_string().len() < 4096);
}

fn summary(store: &Store, project: &str) -> serde_json::Value {
    ReadTransaction::new(&store.connection())
        .list_task_summary_candidates(project, None, 1)
        .unwrap()
        .remove(0)
        .row
}

fn assert_matches_history(store: &Store, task_id: &str) {
    let task = store.load_tracker_task(task_id).unwrap().unwrap();
    let history = store.load_tracker_timeline(task_id).unwrap();
    let row = summary(store, &task.project_path);
    let unread = crate::tracker::summary::unread_since_mark(&task, &history);
    let session = crate::tracker::summary::task_activity_session(&task, &history);
    assert_eq!(row["unread_count"], unread);
    let after_mark = |entry: &&crate::tracker::TimelineEntry| {
        crate::tracker::summary::after(
            task.read_through.as_deref(),
            crate::tracker::summary::entry_id(entry),
        )
    };
    let agent_comments=history.iter().filter(after_mark).filter(|entry| matches!(entry,crate::tracker::TimelineEntry::Comment(comment) if matches!(comment.author,Actor::Agent{..}))).count();
    let mentions = history
        .iter()
        .filter(after_mark)
        .filter(|entry| match entry {
            crate::tracker::TimelineEntry::Comment(comment) => {
                matches!(comment.author, Actor::Agent { .. })
                    && (comment.mentions_user || comment.notifies_user)
            }
            crate::tracker::TimelineEntry::Event(event) => {
                event.kind == crate::tracker::TaskEventKind::Created
                    && matches!(event.actor, Actor::Agent { .. })
                    && event.mentions_user
            }
        })
        .count();
    assert_eq!(row["unread_agent_comments"], agent_comments);
    assert_eq!(row["unread_mentions"], mentions);
    assert_eq!(
        row["session_started_ms"],
        serde_json::json!(session.session_started_ms)
    );
    assert_eq!(
        row["last_activity_ms"],
        serde_json::json!(session.last_activity_ms)
    );
    assert_eq!(
        row["done_at"],
        serde_json::json!(task
            .done_at
            .clone()
            .or_else(|| crate::tracker::done_at_from_timeline(&task, &history)))
    );
}

fn said(task: &crate::tracker::Task, id: &str, at: &str) -> crate::tracker::TaskComment {
    serde_json::from_value(serde_json::json!({
        "id":id,"task_id":task.id,"author":{"kind":"agent","agent_id":"agent-one"},
        "body":"a note","created_at":at
    }))
    .unwrap()
}

#[test]
fn materialized_counts_sessions_completion_match_pure_history_after_mutations() {
    use crate::tracker::{TaskEvent, TaskEventKind};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/summary", "mutations", Actor::User, support::NOW);
    draft.watched = true;
    let mut task = store.create_tracker_task(draft, &[]).unwrap();
    let events = [
        (TaskEventKind::Created, serde_json::json!({})),
        (TaskEventKind::Assigned, serde_json::json!({})),
        (
            TaskEventKind::Moved,
            serde_json::json!({"by":"dispatch","to":"in_progress"}),
        ),
        (
            TaskEventKind::Tracked,
            serde_json::json!({"by":"assignment"}),
        ),
        (
            TaskEventKind::Untracked,
            serde_json::json!({"by":"finished"}),
        ),
        (
            TaskEventKind::Linked,
            serde_json::json!({"waiting_on":"other"}),
        ),
        (TaskEventKind::ReviewCompleted, serde_json::json!({})),
        (TaskEventKind::WorkspaceIdle, serde_json::json!({})),
        (TaskEventKind::Moved, serde_json::json!({"to":"done"})),
    ];
    for (i, (kind, payload)) in events.into_iter().enumerate() {
        let at = format!("2026-01-01T{:02}:00:00Z", i);
        let mut event = TaskEvent::new(
            &task.id,
            Actor::Agent {
                agent_id: "agent-one".into(),
            },
            kind,
            payload,
            &at,
        );
        event.id = format!("te-{:026}", i + 1);
        event.mentions_user = true;
        store
            .save_tracker_task_activity(&task, &[], &[event])
            .unwrap();
        assert_matches_history(&store, &task.id);
    }
    // An older timestamp arrives later: reconstruction must use chronological
    // meaningful activity, not insertion order, including across session gaps.
    let comments = [
        said(
            &task,
            "tc-00000000000000000000000030",
            "2026-01-02T01:00:00.123Z",
        ),
        said(
            &task,
            "tc-00000000000000000000000031",
            "2026-01-01T07:59:59Z",
        ),
    ];
    store
        .save_tracker_task_activity(&task, &comments, &[])
        .unwrap();
    task = store.load_tracker_task(&task.id).unwrap().unwrap();
    assert_matches_history(&store, &task.id);
    task.status = "done".into();
    task.done_at = None;
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_matches_history(&store, &task.id);
    // Direct SQL read/watch updates must share the same hooks as normal writes.
    store
        .connection()
        .execute_batch("SAVEPOINT import_change")
        .unwrap();
    store
        .connection()
        .execute(
            "UPDATE tracker_tasks SET record=json_set(record,'$.read_through',?1) WHERE id=?2",
            rusqlite::params![comments[0].id, task.id],
        )
        .unwrap();
    projection::task_refresh(&store.connection(), &task.id).unwrap();
    projection::history_repair(&store.connection(), &task.id).unwrap();
    store
        .connection()
        .execute_batch("RELEASE import_change")
        .unwrap();
    assert_matches_history(&store, &task.id);
    store
        .connection()
        .execute_batch("SAVEPOINT import_change")
        .unwrap();
    store.connection().execute("UPDATE tracker_tasks SET record=json_set(record,'$.watch_started_after',?1,'$.read_through',NULL) WHERE id=?2",rusqlite::params![comments[1].id,task.id]).unwrap();
    projection::task_refresh(&store.connection(), &task.id).unwrap();
    projection::history_repair(&store.connection(), &task.id).unwrap();
    store
        .connection()
        .execute_batch("RELEASE import_change")
        .unwrap();
    assert_matches_history(&store, &task.id);
    store
        .connection()
        .execute_batch("SAVEPOINT import_change")
        .unwrap();
    store
        .connection()
        .execute(
            "DELETE FROM tracker_comments WHERE id=?1",
            [&comments[0].id],
        )
        .unwrap();
    projection::task_refresh(&store.connection(), &task.id).unwrap();
    projection::history_repair(&store.connection(), &task.id).unwrap();
    store
        .connection()
        .execute_batch("RELEASE import_change")
        .unwrap();
    assert_matches_history(&store, &task.id);
}

#[test]
fn filter_choices_follow_explicit_task_import_updates_and_deletes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/summary", "labels", Actor::User, support::NOW);
    draft.labels = vec!["Backend".into(), "Zoo".into()];
    let first = store.create_tracker_task(draft.clone(), &[]).unwrap();
    draft.id = crate::tracker::new_task_id();
    draft.labels = vec!["backend".into(), "Apple".into()];
    let second = store.create_tracker_task(draft, &[]).unwrap();
    let conn = store.connection();
    let reader = ReadTransaction::new(&conn);
    let page = reader
        .list_task_filter_choices("/summary", "label", None, 2)
        .unwrap();
    assert_eq!(
        page.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(),
        ["apple", "backend"]
    );
    assert_eq!(
        reader
            .list_task_filter_choices("/summary", "label", Some(&page[1].key), 2)
            .unwrap()[0]
            .key,
        "zoo"
    );
    conn.execute_batch("SAVEPOINT import_change").unwrap();
    conn.execute(
        "UPDATE tracker_tasks SET record=json_set(record,'$.labels',json('[\"New\"]')) WHERE id=?1",
        [&first.id],
    )
    .unwrap();
    projection::task_refresh(&conn, &first.id).unwrap();
    conn.execute_batch("RELEASE import_change").unwrap();
    conn.execute_batch("SAVEPOINT import_change").unwrap();
    conn.execute("DELETE FROM tracker_tasks WHERE id=?1", [&second.id])
        .unwrap();
    projection::task_remove(&conn, &second.id).unwrap();
    conn.execute_batch("RELEASE import_change").unwrap();
    let page = reader
        .list_task_filter_choices("/summary", "label", None, 500)
        .unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].value, "New");
    assert!(reader
        .list_task_filter_choices("/other", "label", None, 500)
        .unwrap()
        .is_empty());
}

#[test]
fn persisted_silent_activity_checkpoint_can_span_more_than_one_gap() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = Task::drafted("/summary", "checkpoint", Actor::User, support::NOW);
    task.watched = true;
    task.activity_session = Some(crate::session_summary::SessionSummary {
        session_started_ms: Some(1000),
        last_activity_ms: Some(100_000_000),
    });
    let task = store.create_tracker_task(task, &[]).unwrap();
    assert_matches_history(&store, &task.id);
}

/// Build the old on-disk shape without creating live app or harness state.
fn remove_summary_projection(conn: &rusqlite::Connection) {
    let triggers: Vec<String> = conn.prepare("SELECT name FROM sqlite_schema WHERE type='trigger' AND (name LIKE 'tracker_summary_%' OR name LIKE 'tracker_choices_%')").unwrap()
        .query_map([], |r| r.get(0)).unwrap().collect::<Result<_,_>>().unwrap();
    for trigger in triggers {
        conn.execute_batch(&format!("DROP TRIGGER {trigger}"))
            .unwrap();
    }
    conn.execute_batch(
        "DROP TABLE tracker_task_summaries; DROP TABLE tracker_task_summary_facts;
        DROP TABLE tracker_task_filter_choices; DROP TABLE tracker_task_filter_values; DROP TABLE tracker_task_summary_identities;
        DELETE FROM meta WHERE key LIKE 'task_summary_v1_%';
        UPDATE meta SET value='19' WHERE key='schema_version';",
    )
    .unwrap();
}

#[test]
fn migration_reduces_huge_histories_before_reader_admission_and_reads_stay_small() {
    let dir = tempfile::tempdir().unwrap();
    let task_id;
    {
        let store = Store::new(dir.path()).unwrap();
        let mut task = Task::drafted(
            "/summary",
            "huge history",
            Actor::User,
            "2026-01-01T00:00:00Z",
        );
        task.body = "body must never enter summary".repeat(100_000);
        task.watched = true;
        let task = store.create_tracker_task(task, &[]).unwrap();
        task_id = task.id.clone();
        let mut conn = store.connection();
        remove_summary_projection(&conn);
        let tx = conn.transaction().unwrap();
        {
            let mut insert=tx.prepare("INSERT INTO tracker_comments(id,task_id,created_at,record) VALUES(?1,?2,?3,?4)").unwrap();
            for i in 0..10_000 {
                let mut comment = said(&task, &format!("tc-{i:026}"), "2026-01-01T01:00:00.123Z");
                if i == 0 {
                    comment.body = "large timeline payload".repeat(100_000);
                }
                insert
                    .execute(rusqlite::params![
                        comment.id,
                        comment.task_id,
                        comment.created_at,
                        serde_json::to_string(&comment).unwrap()
                    ])
                    .unwrap();
            }
        }
        tx.commit().unwrap();
    }
    let migration_started = std::time::Instant::now();
    let store = Store::new(dir.path()).unwrap();
    let migration_elapsed = migration_started.elapsed();
    let row = summary(&store, "/summary");
    assert_eq!(row["unread_count"], 10_000);
    assert_matches_history(&store, &task_id);
    let changes_before = store.connection().total_changes();
    let started = std::time::Instant::now();
    let mut bytes = 0;
    for _ in 0..100 {
        let row = summary(&store, "/summary");
        bytes = row.to_string().len();
        assert!(bytes < 4096);
    }
    let read_elapsed = started.elapsed();
    assert_eq!(
        store.connection().total_changes(),
        changes_before,
        "steady-state pages cannot enqueue or perform migration writes"
    );
    let task = store.load_tracker_task(&task_id).unwrap().unwrap();
    let started = std::time::Instant::now();
    store
        .save_tracker_task_activity(
            &task,
            &[said(
                &task,
                "tc-00000000000000000000100000",
                "2026-01-02T00:00:00Z",
            )],
            &[],
        )
        .unwrap();
    let mutation_elapsed = started.elapsed();
    eprintln!("summary fixture: 10000 history entries + multi-MiB task/comment; migration={migration_elapsed:?}, 100 reads={read_elapsed:?}, bytes={bytes}, append={mutation_elapsed:?}");
    // An independent reader has the same already-materialized count.
    assert_eq!(summary(&store, "/summary")["unread_count"], 10_001);
    drop(store);
    let started = std::time::Instant::now();
    let reopened = Store::new(dir.path()).unwrap();
    eprintln!(
        "summary fixture: already-migrated reopen={:?}",
        started.elapsed()
    );
    assert_eq!(summary(&reopened, "/summary")["unread_count"], 10_001);
}

#[test]
fn legacy_oversized_filter_dimensions_fail_explicitly_and_titles_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = Task::drafted("/summary", &"🙂".repeat(10_000), Actor::User, support::NOW);
    task.labels = vec!["normal".into()];
    let task = store.create_tracker_task(task, &[]).unwrap();
    let row = summary(&store, "/summary");
    assert_eq!(row["title"].as_str().unwrap().len(), 4096);
    assert_eq!(row["summary_truncated"], true);
    store
        .connection()
        .execute_batch("SAVEPOINT import_change")
        .unwrap();
    store
        .connection()
        .execute(
            "UPDATE tracker_tasks SET record=json_set(record,'$.labels',json(?1)) WHERE id=?2",
            rusqlite::params![serde_json::json!(["x".repeat(10_000)]).to_string(), task.id],
        )
        .unwrap();
    projection::task_refresh(&store.connection(), &task.id).unwrap();
    store
        .connection()
        .execute_batch("RELEASE import_change")
        .unwrap();
    assert!(matches!(
        store.list_task_summary_candidates("/summary", None, 10),
        Err(StoreError::TaskSummaryInvalid(_))
    ));
    assert!(matches!(
        store.list_task_filter_choices("/summary", "label", None, 10),
        Err(StoreError::TaskSummaryInvalid(_))
    ));
}

#[test]
fn compact_dependencies_follow_edges_and_blocker_lifecycle_without_expansion() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let blocker = store
        .create_tracker_task(
            Task::drafted("/summary", "blocker", Actor::User, support::NOW),
            &[],
        )
        .unwrap();
    let dependent = store
        .create_tracker_task(
            Task::drafted("/summary", "dependent", Actor::User, support::NOW),
            &[],
        )
        .unwrap();
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
    let rows = store
        .list_task_summary_candidates("/summary", None, 10)
        .unwrap();
    assert_eq!(
        rows[0].row["dependencies"],
        serde_json::json!({"blocked":true,"waiting_on_count":1,"blocks_count":0})
    );
    assert_eq!(rows[1].row["dependencies"]["blocks_count"], 1);
    store
        .connection()
        .execute_batch("SAVEPOINT import_change")
        .unwrap();
    store.connection().execute("UPDATE tracker_tasks SET status='done',record=json_set(record,'$.status','done') WHERE id=?1",[&blocker.id]).unwrap();
    projection::task_refresh(&store.connection(), &blocker.id).unwrap();
    projection::task_dependents_refresh(&store.connection(), &blocker.id).unwrap();
    store
        .connection()
        .execute_batch("RELEASE import_change")
        .unwrap();
    assert_eq!(
        summary(&store, "/summary")["dependencies"]["blocked"],
        false
    );
    store
        .connection()
        .execute_batch("SAVEPOINT import_change")
        .unwrap();
    store.connection().execute("UPDATE tracker_tasks SET status='backlog',record=json_set(record,'$.status','backlog') WHERE id=?1",[&blocker.id]).unwrap();
    projection::task_refresh(&store.connection(), &blocker.id).unwrap();
    projection::task_dependents_refresh(&store.connection(), &blocker.id).unwrap();
    store
        .connection()
        .execute_batch("RELEASE import_change")
        .unwrap();
    assert_eq!(summary(&store, "/summary")["dependencies"]["blocked"], true);
    store
        .connection()
        .execute_batch("SAVEPOINT import_change")
        .unwrap();
    store
        .connection()
        .execute(
            "DELETE FROM tracker_task_dependencies WHERE task_id=?1",
            [&dependent.id],
        )
        .unwrap();
    projection::task_refresh(&store.connection(), &blocker.id).unwrap();
    projection::dependency_refresh(&store.connection(), &dependent.id).unwrap();
    projection::dependency_refresh(&store.connection(), &blocker.id).unwrap();
    store
        .connection()
        .execute_batch("RELEASE import_change")
        .unwrap();
    assert_eq!(
        summary(&store, "/summary")["dependencies"],
        serde_json::json!({"blocked":false,"waiting_on_count":0,"blocks_count":0})
    );
}

#[test]
fn projection_rolls_back_with_canonical_write_and_project_deletion_removes_all_memberships() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/summary", "rollback", Actor::User, support::NOW);
    draft.labels = vec!["Label".into(), "LABEL".into()]; // imported duplicate casing
    let task = store.create_tracker_task(draft, &[]).unwrap();
    assert_eq!(
        store
            .list_task_filter_choices("/summary", "label", None, 10)
            .unwrap()
            .len(),
        1
    );
    let before = summary(&store, "/summary");
    {
        let mut conn = store.connection();
        let tx = conn.transaction().unwrap();
        tx.execute_batch("SAVEPOINT import_change").unwrap();
        tx.execute("UPDATE tracker_tasks SET record=json_set(record,'$.title','rolled back','$.labels',json('[\"Temporary\"]')) WHERE id=?1",[&task.id]).unwrap();
        projection::task_refresh(&tx, &task.id).unwrap();
        tx.execute_batch("RELEASE import_change").unwrap();
        let reader = ReadTransaction::new(&tx);
        assert_eq!(
            reader
                .list_task_summary_candidates("/summary", None, 1)
                .unwrap()[0]
                .row["title"],
            "rolled back"
        );
        assert_eq!(
            reader
                .list_task_filter_choices("/summary", "label", None, 10)
                .unwrap()[0]
                .value,
            "Temporary"
        );
        tx.rollback().unwrap();
    }
    assert_eq!(summary(&store, "/summary"), before);
    assert_eq!(
        store
            .list_task_filter_choices("/summary", "label", None, 10)
            .unwrap()[0]
            .key,
        "label"
    );
    store
        .delete_tracker_tasks_of_project("/summary", |_| Ok(()))
        .unwrap();
    assert!(store
        .list_task_summary_candidates("/summary", None, 10)
        .unwrap()
        .is_empty());
    for table in [
        "tracker_task_summaries",
        "tracker_task_summary_facts",
        "tracker_task_filter_choices",
        "tracker_task_filter_values",
    ] {
        let count: i64 = store
            .connection()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "{table}");
    }
}

#[test]
fn missing_projection_rebuilds_even_when_completed_migration_checkpoints_remain() {
    let dir = tempfile::tempdir().unwrap();
    let task_id;
    {
        let store = Store::new(dir.path()).unwrap();
        let mut draft = Task::drafted("/summary", "rebuild", Actor::User, support::NOW);
        draft.watched = true;
        draft.labels = vec!["Restored".into()];
        let task = store.create_tracker_task(draft, &[]).unwrap();
        store
            .save_tracker_task_activity(
                &task,
                &[said(&task, "tc-00000000000000000000000100", support::NOW)],
                &[],
            )
            .unwrap();
        task_id = task.id;
    }
    {
        // Advance startup checkpoints, then lose only the display projection.
        let store = Store::new(dir.path()).unwrap();
        let conn = store.connection();
        let checkpoints: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM meta WHERE key LIKE 'task_summary_v1_%'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(checkpoints >= 2);
        conn.execute_batch("DROP TABLE tracker_task_summaries;")
            .unwrap();
    }
    let started = std::time::Instant::now();
    let store = Store::new(dir.path()).unwrap();
    eprintln!(
        "missing task-summary component: startup rebuild={:?}",
        started.elapsed()
    );
    assert_eq!(summary(&store, "/summary")["unread_count"], 1);
    assert_matches_history(&store, &task_id);
    let choices = store
        .list_task_filter_choices("/summary", "label", None, 10)
        .unwrap();
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].value, "Restored");
}

#[test]
fn summary_identity_uses_committed_roster_metadata_and_owner_renames() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut conn = store.connection();
    // Only identity fields matter to this projection. A large unrelated owner
    // field proves the read need not fetch or parse the owner record.
    let owner =
        serde_json::json!({"worktree_name":"Original workspace","goal":"large".repeat(100_000)});
    conn.execute_batch("SAVEPOINT import_change").unwrap();
    conn.execute("INSERT INTO implementations(id,created_at,updated_at,record) VALUES('run-summary',?1,?1,?2)",rusqlite::params![support::NOW,owner.to_string()]).unwrap();
    projection::identity_refresh(&conn, "agent-summary").unwrap();
    conn.execute_batch("RELEASE import_change").unwrap();
    let agent = serde_json::json!({"id":"agent-summary","name":"Original name","choice":{"provider":"codex_app_server"},"project_role":null});
    conn.execute_batch("SAVEPOINT import_change").unwrap();
    conn.execute(
        "INSERT INTO agents(id,owner_id,ordinal,record) VALUES('agent-summary','run-summary',2,?1)",
        [agent.to_string()],
    )
    .unwrap();
    projection::identity_refresh(&conn, "agent-summary").unwrap();
    conn.execute_batch("RELEASE import_change").unwrap();
    {
        let tx = conn.transaction().unwrap();
        tx.execute_batch("SAVEPOINT import_change").unwrap();
        tx.execute("UPDATE agents SET record=json_set(record,'$.name','Uncommitted name') WHERE id='agent-summary'",[]).unwrap();
        projection::identity_refresh(&tx, "agent-summary").unwrap();
        tx.execute_batch("RELEASE import_change").unwrap();
        assert_eq!(
            ReadTransaction::new(&tx)
                .task_summary_identity("agent-summary")
                .unwrap()
                .unwrap()
                .name
                .as_deref(),
            Some("Uncommitted name")
        );
        tx.rollback().unwrap();
    }
    let reader = ReadTransaction::new(&conn);
    let identity = reader
        .task_summary_identity("agent-summary")
        .unwrap()
        .unwrap();
    assert_eq!(identity.name.as_deref(), Some("Original name"));
    assert_eq!(identity.provider.as_deref(), Some("codex_app_server"));
    assert_eq!(
        identity.workspace_name.as_deref(),
        Some("Original workspace")
    );
    conn.execute_batch("SAVEPOINT import_change").unwrap();
    conn.execute("UPDATE agents SET record=json_set(record,'$.name','Committed name') WHERE id='agent-summary'",[]).unwrap();
    projection::identity_refresh(&conn, "agent-summary").unwrap();
    conn.execute_batch("RELEASE import_change").unwrap();
    conn.execute_batch("SAVEPOINT import_change").unwrap();
    conn.execute("UPDATE implementations SET record=json_set(record,'$.worktree_name','Renamed workspace') WHERE id='run-summary'",[]).unwrap();
    projection::owner_refresh(&conn, "run-summary").unwrap();
    conn.execute_batch("RELEASE import_change").unwrap();
    let identity = reader
        .task_summary_identity("agent-summary")
        .unwrap()
        .unwrap();
    assert_eq!(identity.name.as_deref(), Some("Committed name"));
    assert_eq!(
        identity.workspace_name.as_deref(),
        Some("Renamed workspace")
    );
    conn.execute_batch("SAVEPOINT import_change").unwrap();
    conn.execute("DELETE FROM agents WHERE id='agent-summary'", [])
        .unwrap();
    projection::identity_remove(&conn, "agent-summary").unwrap();
    conn.execute_batch("RELEASE import_change").unwrap();
    assert!(reader
        .task_summary_identity("agent-summary")
        .unwrap()
        .is_none());
}

#[test]
fn assignee_projection_and_choices_preserve_tagged_filter_values_and_opaque_ids() {
    use crate::tracker::Assignee;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let assignees = [
        None,
        Some(Assignee::User),
        Some(Assignee::ProjectAgent),
        Some(Assignee::Agent {
            agent_id: "user".into(),
        }),
        Some(Assignee::Agent {
            agent_id: "none".into(),
        }),
        Some(Assignee::Agent {
            agent_id: "AgentCASE".into(),
        }),
        Some(Assignee::Agent {
            agent_id: "agentcase".into(),
        }),
    ];
    for (i, assignee) in assignees.into_iter().enumerate() {
        let mut task = Task::drafted(
            "/summary",
            &format!("holder-{i}"),
            Actor::User,
            support::NOW,
        );
        task.assignee = assignee.clone();
        store.create_tracker_task(task, &[]).unwrap();
        let row = summary(&store, "/summary");
        assert_eq!(row["assignee"], serde_json::to_value(&assignee).unwrap());
        assert_eq!(row["created_by"], serde_json::json!({"kind":"user"}));
    }
    let choices = store
        .list_task_filter_choices("/summary", "assignee", None, 100)
        .unwrap();
    assert_eq!(choices.len(), 7);
    let keys = choices
        .iter()
        .map(|choice| choice.key.clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(keys.len(), 7);
    for choice in choices {
        assert_eq!(choice.key, serde_json::to_string(&choice.value).unwrap());
        assert!(choice.value == serde_json::json!("none") || choice.value.is_object());
    }
}

#[test]
fn activity_millisecond_projection_matches_nanosecond_timestamp_truncation_and_offsets() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/summary", "precise time", Actor::User, support::NOW);
    draft.watched = true;
    let task = store.create_tracker_task(draft, &[]).unwrap();
    for (i, at) in [
        "2026-01-01T00:00:00.123500000Z",
        "2026-01-01T00:00:00.999999999Z",
        "2026-01-01T00:01:00.1Z",
        "2026-01-01T02:00:00.000999999+01:30",
        "1969-12-31T23:59:59.999999999Z",
    ]
    .into_iter()
    .enumerate()
    {
        store
            .save_tracker_task_activity(&task, &[said(&task, &format!("tc-{i:026}"), at)], &[])
            .unwrap();
        assert_matches_history(&store, &task.id);
    }
}

#[test]
fn invalid_filter_choices_fail_before_returning_an_unusable_page() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let cases = [
        ("label", "x".repeat(513), serde_json::json!("label")),
        ("label", "valid".into(), serde_json::json!("x".repeat(1025))),
        ("label", "valid".into(), serde_json::json!("x".repeat(41))),
        ("label", "valid".into(), serde_json::json!("")),
        ("label", "valid".into(), serde_json::json!(42)),
        (
            "assignee",
            "valid".into(),
            serde_json::json!({"kind":"agent","agent_id":"x".repeat(129)}),
        ),
        (
            "assignee",
            "valid".into(),
            serde_json::json!({"kind":"agent","agent_id":""}),
        ),
        (
            "assignee",
            "valid".into(),
            serde_json::json!({"kind":"user","agent_id":null}),
        ),
    ];
    for (dimension, key, value) in cases {
        let conn = store.connection();
        conn.execute("DELETE FROM tracker_task_filter_values", [])
            .unwrap();
        conn.execute("INSERT INTO tracker_task_filter_values(project_key,dimension,choice_key,value,refs) VALUES('/summary',?1,?2,?3,1)",rusqlite::params![dimension,key,value.to_string()]).unwrap();
        assert!(
            matches!(
                ReadTransaction::new(&conn)
                    .list_task_filter_choices("/summary", dimension, None, 1),
                Err(StoreError::TaskSummaryInvalid(_))
            ),
            "{dimension} choice {value}"
        );
    }
}

#[test]
fn interrupted_projection_repair_resumes_with_all_tables_present_and_old_checkpoints() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::new(dir.path()).unwrap();
        let mut task = Task::drafted("/summary", "canonical title", Actor::User, support::NOW);
        task.watched = true;
        task.labels = vec!["Canonical".into()];
        let task = store.create_tracker_task(task, &[]).unwrap();
        store
            .save_tracker_task_activity(
                &task,
                &[said(&task, "tc-00000000000000000000000001", support::NOW)],
                &[],
            )
            .unwrap();
    }
    {
        let store = Store::new(dir.path()).unwrap(); // complete old checkpoints
        let conn = store.connection();
        conn.execute(
            "INSERT INTO meta(key,value) VALUES('task_summary_rebuild','1')",
            [],
        )
        .unwrap();
        conn.execute("UPDATE tracker_task_summaries SET record=json_set(record,'$.title','partial stale projection')",[]).unwrap();
        conn.execute("DELETE FROM tracker_task_summary_facts", [])
            .unwrap();
    }
    let store = Store::new(dir.path()).unwrap();
    let row = summary(&store, "/summary");
    assert_eq!(row["title"], "canonical title");
    assert_eq!(row["unread_count"], 1);
    assert_eq!(
        store
            .list_task_filter_choices("/summary", "label", None, 1)
            .unwrap()[0]
            .value,
        "Canonical"
    );
    let marker: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM meta WHERE key='task_summary_rebuild'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(marker, 0);
}

fn forbid_projection_writes(conn: &rusqlite::Connection, tables: &[&str]) {
    for table in tables {
        for operation in ["INSERT", "UPDATE", "DELETE"] {
            conn.execute_batch(&format!("CREATE TRIGGER guard_{table}_{operation} BEFORE {operation} ON {table} BEGIN SELECT RAISE(ABORT,'unrelated projection write'); END;")).unwrap();
        }
    }
}

#[test]
fn history_and_read_marker_changes_do_not_rebuild_choices_or_dependency_rows() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/summary", "isolated activity", Actor::User, support::NOW);
    draft.body = "large unrelated body".repeat(100_000);
    draft.watched = true;
    draft.labels = vec!["Unchanged".into()];
    let task = store.create_tracker_task(draft, &[]).unwrap();
    let comment = said(&task, "tc-00000000000000000000000001", support::NOW);
    {
        let conn = store.connection();
        forbid_projection_writes(
            &conn,
            &[
                "tracker_task_filter_choices",
                "tracker_task_filter_values",
                "tracker_task_dependencies",
            ],
        );
    }
    store
        .save_tracker_task_activity(&task, std::slice::from_ref(&comment), &[])
        .unwrap();
    assert_matches_history(&store, &task.id);
    {
        let conn = store.connection();
        conn.execute_batch("SAVEPOINT import_change").unwrap();
        conn.execute(
            "UPDATE tracker_tasks SET record=json_set(record,'$.read_through',?1) WHERE id=?2",
            rusqlite::params![comment.id, task.id],
        )
        .unwrap();
        projection::task_refresh(&conn, &task.id).unwrap();
        conn.execute_batch("RELEASE import_change").unwrap();
        conn.execute_batch("SAVEPOINT import_change").unwrap();
        conn.execute("UPDATE tracker_comments SET record=json_set(record,'$.author.kind','user') WHERE id=?1",[&comment.id]).unwrap();
        projection::history_repair(&conn, &task.id).unwrap();
        conn.execute_batch("RELEASE import_change").unwrap();
    }
    assert_matches_history(&store, &task.id);
    store
        .connection()
        .execute_batch("SAVEPOINT import_change")
        .unwrap();
    store
        .connection()
        .execute("DELETE FROM tracker_comments WHERE id=?1", [&comment.id])
        .unwrap();
    projection::history_repair(&store.connection(), &task.id).unwrap();
    store
        .connection()
        .execute_batch("RELEASE import_change")
        .unwrap();
    assert_matches_history(&store, &task.id);
}

#[test]
fn review_metadata_patches_do_not_rebuild_activity_choices_or_dependencies() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = store
        .create_tracker_task(
            Task::drafted("/summary", "metadata", Actor::User, support::NOW),
            &[],
        )
        .unwrap();
    let conn = store.connection();
    forbid_projection_writes(
        &conn,
        &[
            "tracker_task_filter_choices",
            "tracker_task_filter_values",
            "tracker_task_dependencies",
            "tracker_task_summary_facts",
        ],
    );
    conn.execute_batch("SAVEPOINT import_change").unwrap();
    conn.execute("INSERT INTO reviews(task_id,workspace_id,version,record) VALUES(?1,'workspace',1,?2)",rusqlite::params![task.id,serde_json::json!({"mode":"pull_request","pull_request":{"originating_workspace_id":"workspace","status":"open"}}).to_string()]).unwrap();
    projection::metadata_refresh(&conn, &task.id).unwrap();
    conn.execute_batch("RELEASE import_change").unwrap();
    let reader = ReadTransaction::new(&conn);
    assert_eq!(
        reader
            .list_task_summary_candidates("/summary", None, 1)
            .unwrap()[0]
            .row["review_summary"]["status"],
        "open"
    );
    conn.execute_batch("SAVEPOINT import_change").unwrap();
    conn.execute("UPDATE reviews SET record=json_set(record,'$.pull_request.status','approved') WHERE task_id=?1",[&task.id]).unwrap();
    projection::metadata_refresh(&conn, &task.id).unwrap();
    conn.execute_batch("RELEASE import_change").unwrap();
    assert_eq!(
        reader
            .list_task_summary_candidates("/summary", None, 1)
            .unwrap()[0]
            .row["review_summary"]["status"],
        "approved"
    );
    conn.execute_batch("SAVEPOINT import_change").unwrap();
    conn.execute("DELETE FROM reviews WHERE task_id=?1", [&task.id])
        .unwrap();
    projection::metadata_refresh(&conn, &task.id).unwrap();
    conn.execute_batch("RELEASE import_change").unwrap();
    assert!(reader
        .list_task_summary_candidates("/summary", None, 1)
        .unwrap()[0]
        .row
        .get("review_summary")
        .is_none());
}

#[test]
fn activity_context_backfills_old_card_schema_before_reader_admission() {
    let dir = tempfile::tempdir().unwrap();
    let task_id;
    {
        let store = Store::new(dir.path()).unwrap();
        let mut task = Task::drafted("/summary", "old card schema", Actor::User, support::NOW);
        task.watched = true;
        let task = store.create_tracker_task(task, &[]).unwrap();
        store
            .save_tracker_task_activity(
                &task,
                &[said(&task, "tc-00000000000000000000000001", support::NOW)],
                &[],
            )
            .unwrap();
        task_id = task.id;
    }
    {
        let store = Store::new(dir.path()).unwrap();
        let conn = store.connection();
        let names=conn.prepare("SELECT name FROM sqlite_schema WHERE type='trigger' AND name LIKE 'tracker_summary_%'").unwrap().query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        for name in names {
            conn.execute_batch(&format!("DROP TRIGGER {name}")).unwrap();
        }
        // The legacy shape predates shared helper views as well as context.
        let views=conn.prepare("SELECT name FROM sqlite_schema WHERE type='view' AND name GLOB 'tracker_summary_*'").unwrap().query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        for name in views {
            conn.execute_batch(&format!("DROP VIEW {name}")).unwrap();
        }
        conn.execute_batch("ALTER TABLE tracker_task_summaries DROP COLUMN context;")
            .unwrap();
    }
    let store = Store::new(dir.path()).unwrap();
    assert_matches_history(&store, &task_id);
    let size: i64 = store
        .connection()
        .query_row(
            "SELECT length(CAST(context AS BLOB)) FROM tracker_task_summaries WHERE task_id=?1",
            [&task_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(size <= 4096);
}

#[test]
fn history_projection_does_not_parse_the_canonical_task_record() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = Task::drafted("/summary", "isolated source", Actor::User, support::NOW);
    task.watched = true;
    let task = store.create_tracker_task(task, &[]).unwrap();
    let mut conn = store.connection();
    let tx = conn.transaction().unwrap();
    // Deliberately make the canonical JSON unreadable inside a rollback-only
    // transaction, retaining the already-committed compact card context.
    tx.execute(
        "UPDATE tracker_tasks SET record='unreadable canonical JSON' WHERE id=?1",
        [&task.id],
    )
    .unwrap();
    let comment = said(&task, "tc-00000000000000000000000001", support::NOW);
    tx.execute(
        "INSERT INTO tracker_comments(id,task_id,created_at,record) VALUES(?1,?2,?3,?4)",
        rusqlite::params![
            comment.id,
            comment.task_id,
            comment.created_at,
            serde_json::to_string(&comment).unwrap()
        ],
    )
    .unwrap();
    projection::history_append(&tx, projection::HistoryKind::Comment, &task.id, &comment.id)
        .unwrap();
    let reader = ReadTransaction::new(&tx);
    assert_eq!(
        reader
            .list_task_summary_candidates("/summary", None, 1)
            .unwrap()[0]
            .row["unread_count"],
        1
    );
    tx.rollback().unwrap();
}

#[test]
fn failed_migration_keeps_only_committed_batches_and_resumes_before_reader_admission() {
    let dir = tempfile::tempdir().unwrap();
    let task_id;
    let repaired_record;
    {
        let store = Store::new(dir.path()).unwrap();
        let mut task = Task::drafted("/summary", "resume batches", Actor::User, support::NOW);
        task.watched = true;
        let task = store.create_tracker_task(task, &[]).unwrap();
        task_id = task.id.clone();
        let mut connection = store.connection();
        remove_summary_projection(&connection);
        let tx = connection.transaction().unwrap();
        let mut insert = tx
            .prepare(
                "INSERT INTO tracker_comments(id,task_id,created_at,record) VALUES(?1,?2,?3,?4)",
            )
            .unwrap();
        let mut fixed = String::new();
        for number in 0..256 {
            let note = said(&task, &format!("tc-{number:026}"), "2026-01-01T01:00:00Z");
            let raw = serde_json::to_string(&note).unwrap();
            let saved = if number == 129 {
                fixed = raw.clone();
                "not-json".to_string()
            } else {
                raw
            };
            insert
                .execute(rusqlite::params![
                    note.id,
                    note.task_id,
                    note.created_at,
                    saved
                ])
                .unwrap();
        }
        repaired_record = fixed;
        drop(insert);
        tx.commit().unwrap();
    }
    assert!(
        Store::new(dir.path()).is_err(),
        "a partial migration must not publish a read pool"
    );
    let connection = rusqlite::Connection::open(dir.path().join("build.db")).unwrap();
    let checkpoint: String = connection
        .query_row(
            "SELECT value FROM meta WHERE key='task_summary_v1_tracker_comments'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(checkpoint, "128");
    let committed: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM tracker_task_summary_facts",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        committed, 128,
        "the failed second batch must roll back atomically"
    );
    connection
        .execute(
            "UPDATE tracker_comments SET record=?1 WHERE id=?2",
            rusqlite::params![repaired_record, format!("tc-{:026}", 129)],
        )
        .unwrap();
    drop(connection);
    let recovered = Store::new(dir.path()).unwrap();
    let row = summary(&recovered, "/summary");
    assert_eq!(row["unread_count"], 256);
    assert_eq!(
        recovered.load_tracker_timeline(&task_id).unwrap().len(),
        256
    );
    let count: i64 = recovered
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM tracker_task_summary_facts",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 256, "resume cannot duplicate the committed prefix");
}

#[test]
fn chronological_activity_increments_match_replay_across_marks_gaps_and_late_entries() {
    use crate::tracker::{TaskEvent, TaskEventKind};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = Task::drafted(
        "/summary",
        "incremental parity",
        Actor::User,
        "2026-01-01T00:00:00Z",
    );
    task.watched = true;
    task.read_through = Some("tc-00000000000000000000000001".into());
    task.watch_started_after = Some("te-00000000000000000000000002".into());
    task.activity_session = Some(crate::session_summary::SessionSummary {
        session_started_ms: crate::session_summary::message_millis("2026-01-01T00:00:00Z"),
        last_activity_ms: crate::session_summary::message_millis("2026-01-01T01:00:00Z"),
    });
    let mut task = store.create_tracker_task(task, &[]).unwrap();
    for (i, at) in [
        "2026-01-01T00:30:00Z",
        "2026-01-01T01:00:00Z",
        "2026-01-01T02:00:00Z",
        "2026-01-01T10:00:00Z",
        "2026-01-01T09:00:00Z",
        "2026-01-01T20:00:00Z",
    ]
    .into_iter()
    .enumerate()
    {
        let mut note = said(&task, &format!("tc-{i:026}"), at);
        note.mentions_user = true;
        if i == 3 {
            note.author = Actor::User;
        }
        store
            .save_tracker_task_activity(&task, &[note], &[])
            .unwrap();
        assert_matches_history(&store, &task.id);
    }
    // A legacy watch without an explicit cursor changes the effective floor.
    task.watch_started_after = None;
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    let mut watch = TaskEvent::new(
        &task.id,
        Actor::User,
        TaskEventKind::Watched,
        serde_json::json!({}),
        "2026-01-01T21:00:00Z",
    );
    watch.id = "te-00000000000000000000000030".into();
    store
        .save_tracker_task_activity(&task, &[], &[watch])
        .unwrap();
    assert_matches_history(&store, &task.id);
    store
        .save_tracker_task_activity(
            &task,
            &[said(
                &task,
                "tc-00000000000000000000000031",
                "2026-01-01T22:00:00Z",
            )],
            &[],
        )
        .unwrap();
    assert_matches_history(&store, &task.id);
    task.status = "done".into();
    task.done_at = None;
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    let mut done = TaskEvent::new(
        &task.id,
        Actor::Agent {
            agent_id: "agent-one".into(),
        },
        TaskEventKind::Moved,
        serde_json::json!({"to":"done","by":"report"}),
        "2026-01-01T23:00:00Z",
    );
    done.id = "te-00000000000000000000000032".into();
    store
        .save_tracker_task_activity(&task, &[], &[done])
        .unwrap();
    assert_matches_history(&store, &task.id);
}

fn assert_invalid_metadata_reference(reader: &ReadTransaction<'_>) {
    assert!(matches!(
        reader.list_task_summary_candidates("/summary", None, 1),
        Err(StoreError::TaskSummaryInvalid(_))
    ));
}

#[test]
fn oversized_metadata_references_fail_explicitly_and_valid_opaque_case_is_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = store
        .create_tracker_task(
            Task::drafted("/summary", "metadata refs", Actor::User, support::NOW),
            &[],
        )
        .unwrap();
    let conn = store.connection();
    conn.execute_batch("SAVEPOINT import_change").unwrap();
    conn.execute("INSERT INTO reviews(task_id,workspace_id,version,record) VALUES(?1,'WorkspaceCASE',1,?2)",rusqlite::params![task.id,serde_json::json!({"mode":"pull_request","pull_request":{"originating_workspace_id":"WorkspaceCASE","latest_published_snapshot_id":"SnapshotCASE","status":"open"}}).to_string()]).unwrap();
    projection::metadata_refresh(&conn, &task.id).unwrap();
    conn.execute_batch("RELEASE import_change").unwrap();
    let reader = ReadTransaction::new(&conn);
    let rows = reader
        .list_task_summary_candidates("/summary", None, 1)
        .unwrap();
    assert_eq!(
        rows[0].row["review_summary"]["workspace_id"],
        "WorkspaceCASE"
    );
    assert_eq!(
        rows[0].row["review_summary"]["latest_published_snapshot_id"],
        "SnapshotCASE"
    );
    assert!(rows[0].row.get("_summary_invalid_metadata_refs").is_none());
    for field in ["originating_workspace_id", "latest_published_snapshot_id"] {
        let path = format!("$.pull_request.{field}");
        conn.execute_batch("SAVEPOINT import_change").unwrap();
        conn.execute(
            "UPDATE reviews SET record=json_set(record,?1,?2) WHERE task_id=?3",
            rusqlite::params![path, "x".repeat(129), task.id],
        )
        .unwrap();
        projection::metadata_refresh(&conn, &task.id).unwrap();
        conn.execute_batch("RELEASE import_change").unwrap();
        assert_invalid_metadata_reference(&reader);
        conn.execute_batch("SAVEPOINT import_change").unwrap();
        conn.execute(
            "UPDATE reviews SET record=json_set(record,?1,'ValidCASE') WHERE task_id=?2",
            rusqlite::params![path, task.id],
        )
        .unwrap();
        projection::metadata_refresh(&conn, &task.id).unwrap();
        conn.execute_batch("RELEASE import_change").unwrap();
        assert!(reader
            .list_task_summary_candidates("/summary", None, 1)
            .is_ok());
    }
    conn.execute_batch("SAVEPOINT import_change").unwrap();
    conn.execute("DELETE FROM reviews WHERE task_id=?1", [&task.id])
        .unwrap();
    projection::metadata_refresh(&conn, &task.id).unwrap();
    conn.execute_batch("RELEASE import_change").unwrap();
    conn.execute_batch("SAVEPOINT import_change").unwrap();
    conn.execute("INSERT INTO tracker_delivery_intents(id,task_id,project_key,generation,operation_id,state,record) VALUES('IntentCASE',?1,'/summary',1,'metadata-op','queued',?2)",rusqlite::params![task.id,serde_json::json!({"target":{"kind":"agent","agent_id":"AgentCASE"}}).to_string()]).unwrap();
    projection::metadata_refresh(&conn, &task.id).unwrap();
    conn.execute_batch("RELEASE import_change").unwrap();
    let rows = reader
        .list_task_summary_candidates("/summary", None, 1)
        .unwrap();
    assert_eq!(rows[0].row["deferred_assignment"]["id"], "IntentCASE");
    assert_eq!(
        rows[0].assignment_target.as_ref().unwrap()["agent_id"],
        "AgentCASE"
    );
    conn.execute_batch("SAVEPOINT import_change").unwrap();
    conn.execute(
        "UPDATE tracker_delivery_intents SET id=?1 WHERE task_id=?2",
        rusqlite::params!["x".repeat(129), task.id],
    )
    .unwrap();
    projection::metadata_refresh(&conn, &task.id).unwrap();
    conn.execute_batch("RELEASE import_change").unwrap();
    assert_invalid_metadata_reference(&reader);
    conn.execute_batch("SAVEPOINT import_change").unwrap();
    conn.execute("UPDATE tracker_delivery_intents SET id='IntentCASE',record=json_set(record,'$.target.agent_id',?1) WHERE task_id=?2",rusqlite::params!["x".repeat(129),task.id]).unwrap();
    projection::metadata_refresh(&conn, &task.id).unwrap();
    conn.execute_batch("RELEASE import_change").unwrap();
    assert_invalid_metadata_reference(&reader);
    conn.execute_batch("SAVEPOINT import_change").unwrap();
    conn.execute(
        "DELETE FROM tracker_delivery_intents WHERE task_id=?1",
        [&task.id],
    )
    .unwrap();
    projection::metadata_refresh(&conn, &task.id).unwrap();
    conn.execute_batch("RELEASE import_change").unwrap();
    let rows = reader
        .list_task_summary_candidates("/summary", None, 1)
        .unwrap();
    assert!(rows[0].row.get("deferred_assignment").is_none());
    assert!(rows[0].assignment_target.is_none());
}

#[test]
fn metadata_reference_guard_backfills_already_materialized_cards() {
    let dir = tempfile::tempdir().unwrap();
    let task_id;
    {
        let store = Store::new(dir.path()).unwrap();
        let task = store
            .create_tracker_task(
                Task::drafted("/summary", "old metadata", Actor::User, support::NOW),
                &[],
            )
            .unwrap();
        task_id = task.id;
    }
    {
        let store = Store::new(dir.path()).unwrap(); // task checkpoint already complete
        let conn = store.connection();
        conn.execute("INSERT INTO reviews(task_id,workspace_id,version,record) VALUES(?1,'workspace',1,?2)",rusqlite::params![task_id,serde_json::json!({"mode":"pull_request","pull_request":{"originating_workspace_id":"x".repeat(129),"status":"open"}}).to_string()]).unwrap();
        conn.execute("UPDATE tracker_task_summaries SET record=json_remove(record,'$._summary_invalid_metadata_refs') WHERE task_id=?1",[&task_id]).unwrap();
        conn.execute("DELETE FROM meta WHERE key IN ('task_summary_v1_metadata','task_summary_v1_metadata_complete')",[]).unwrap();
    }
    let store = Store::new(dir.path()).unwrap();
    assert!(matches!(
        store.list_task_summary_candidates("/summary", None, 1),
        Err(StoreError::TaskSummaryInvalid(_))
    ));
}

#[test]
fn metadata_guard_repair_resumes_failed_batch_and_skips_completed_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut last_id = String::new();
    {
        let store = Store::new(dir.path()).unwrap();
        for number in 0..129 {
            last_id = store
                .create_tracker_task(
                    Task::drafted(
                        "/summary",
                        &format!("metadata repair {number}"),
                        Actor::User,
                        support::NOW,
                    ),
                    &[],
                )
                .unwrap()
                .id;
        }
    }
    {
        // Complete the ordinary task checkpoint before simulating a metadata-only upgrade.
        let store = Store::new(dir.path()).unwrap();
        let conn = store.connection();
        conn.execute("UPDATE tracker_task_summaries SET record=json_remove(record,'$._summary_invalid_metadata_refs')",[]).unwrap();
        conn.execute("DELETE FROM meta WHERE key IN ('task_summary_v1_metadata','task_summary_v1_metadata_complete')",[]).unwrap();
        conn.execute_batch(&format!("CREATE TRIGGER metadata_stop BEFORE UPDATE ON tracker_task_summaries WHEN NEW.task_id='{}' AND json_type(OLD.record,'$._summary_invalid_metadata_refs') IS NULL AND json_type(NEW.record,'$._summary_invalid_metadata_refs') IS NOT NULL BEGIN SELECT RAISE(ABORT,'stop metadata repair'); END",last_id.replace('\'',"''"))).unwrap();
    }
    assert!(
        Store::new(dir.path()).is_err(),
        "failed metadata repair must not admit a reader pool"
    );
    {
        let conn = rusqlite::Connection::open(dir.path().join("build.db")).unwrap();
        let checkpoint: String = conn
            .query_row(
                "SELECT value FROM meta WHERE key='task_summary_v1_metadata'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(checkpoint, "128");
        let repaired: i64 = conn.query_row("SELECT COUNT(*) FROM tracker_task_summaries WHERE json_type(record,'$._summary_invalid_metadata_refs') IS NOT NULL",[],|row|row.get(0)).unwrap();
        assert_eq!(repaired, 128, "failed batch rolls back as a unit");
        let complete: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM meta WHERE key='task_summary_v1_metadata_complete')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!complete, "failure cannot publish the complete marker");
        conn.execute_batch("DROP TRIGGER metadata_stop").unwrap();
    }
    let started = std::time::Instant::now();
    {
        let store = Store::new(dir.path()).unwrap();
        eprintln!(
            "metadata repair resumed startup elapsed={:?}",
            started.elapsed()
        );
        assert_eq!(
            store
                .list_task_summary_candidates("/summary", None, 2001)
                .unwrap()
                .len(),
            129
        );
        let repaired: i64 = store.connection().query_row("SELECT COUNT(*) FROM tracker_task_summaries WHERE json_type(record,'$._summary_invalid_metadata_refs') IS NOT NULL",[],|row|row.get(0)).unwrap();
        assert_eq!(repaired, 129);
    }
    {
        let conn = rusqlite::Connection::open(dir.path().join("build.db")).unwrap();
        let complete: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM meta WHERE key='task_summary_v1_metadata_complete')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(complete);
        conn.execute_batch("CREATE TRIGGER metadata_rerun BEFORE UPDATE ON tracker_task_summaries BEGIN SELECT RAISE(ABORT,'completed metadata repair ran again'); END").unwrap();
    }
    let started = std::time::Instant::now();
    let store = Store::new(dir.path()).unwrap();
    eprintln!(
        "metadata repair completed reopen elapsed={:?}",
        started.elapsed()
    );
    assert_eq!(
        store
            .list_task_summary_candidates("/summary", None, 2001)
            .unwrap()
            .len(),
        129
    );
}

#[test]
fn completed_reopen_preserves_schema_and_does_no_projection_writes() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::new(dir.path()).unwrap();
        let task = store
            .create_tracker_task(
                Task::drafted("/summary", "after migration", Actor::User, support::NOW),
                &[],
            )
            .unwrap();
        store
            .save_tracker_task_activity(
                &task,
                &[said(&task, "tc-00000000000000000000000999", support::NOW)],
                &[],
            )
            .unwrap();
    }
    let mut conn = rusqlite::Connection::open(dir.path().join("build.db")).unwrap();
    let schema: i64 = conn
        .query_row("PRAGMA schema_version", [], |row| row.get(0))
        .unwrap();
    let writes = conn.total_changes();
    super::super::task_summaries::initialize(&mut conn).unwrap();
    assert_eq!(
        conn.total_changes(),
        writes,
        "completed projection must not replay migration or rewrite markers"
    );
    assert_eq!(
        conn.query_row("PRAGMA schema_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        schema,
        "completed projection must preserve its schema"
    );
    drop(conn);
    let store = Store::new(dir.path()).unwrap();
    assert_eq!(
        store
            .connection()
            .query_row("PRAGMA schema_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        schema
    );
    assert_eq!(summary(&store, "/summary")["title"], "after migration");
}

#[test]
fn completed_projection_repairs_missing_indexes_before_admission() {
    let name = "tracker_summaries_number";
    let dir = tempfile::tempdir().unwrap();
    let task = {
        let store = Store::new(dir.path()).unwrap();
        let mut task = Task::drafted("/summary", "repair schema", Actor::User, support::NOW);
        task.watched = true;
        store.create_tracker_task(task, &[]).unwrap()
    };
    {
        let conn = rusqlite::Connection::open(dir.path().join("build.db")).unwrap();
        conn.execute_batch("DROP INDEX tracker_summaries_number")
            .unwrap();
    }
    let store = Store::new(dir.path()).unwrap();
    let exists: bool = store
        .connection()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name=?1)",
            [name],
            |row| row.get(0),
        )
        .unwrap();
    assert!(exists, "required index must be repaired");
    store
        .save_tracker_task_activity(
            &task,
            &[said(&task, "tc-00000000000000000000000999", support::NOW)],
            &[],
        )
        .unwrap();
    assert_eq!(summary(&store, "/summary")["unread_count"], 1);
}

#[test]
fn unrelated_schema_change_revalidates_without_replaying_completed_history() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = Store::new(dir.path()).unwrap();
        let task = store
            .create_tracker_task(
                Task::drafted("/summary", "no replay", Actor::User, support::NOW),
                &[],
            )
            .unwrap();
        store
            .save_tracker_task_activity(
                &task,
                &[said(&task, "tc-00000000000000000000000999", support::NOW)],
                &[],
            )
            .unwrap();
    }
    let conn = rusqlite::Connection::open(dir.path().join("build.db")).unwrap();
    conn.execute_batch("CREATE TABLE unrelated_migration(id TEXT); CREATE TRIGGER forbid_replay BEFORE INSERT ON tracker_task_summary_facts BEGIN SELECT RAISE(ABORT,'completed history replayed'); END;").unwrap();
    drop(conn);
    let store = Store::new(dir.path()).unwrap();
    assert_eq!(summary(&store, "/summary")["title"], "no replay");
}
