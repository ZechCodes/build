use super::*;
use crate::tracker::{Actor, Task};

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
        .execute(
            "UPDATE tracker_tasks SET record=json_set(record,'$.read_through',?1) WHERE id=?2",
            rusqlite::params![comments[0].id, task.id],
        )
        .unwrap();
    assert_matches_history(&store, &task.id);
    store.connection().execute("UPDATE tracker_tasks SET record=json_set(record,'$.watch_started_after',?1,'$.read_through',NULL) WHERE id=?2",rusqlite::params![comments[1].id,task.id]).unwrap();
    assert_matches_history(&store, &task.id);
    store
        .connection()
        .execute(
            "DELETE FROM tracker_comments WHERE id=?1",
            [&comments[0].id],
        )
        .unwrap();
    assert_matches_history(&store, &task.id);
}

#[test]
fn filter_choices_are_indexed_distinct_and_follow_direct_task_updates_and_deletes() {
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
    assert!(reader
        .task_summary_has_label("/summary", &first.id, "BACKEND")
        .unwrap());
    conn.execute(
        "UPDATE tracker_tasks SET record=json_set(record,'$.labels',json('[\"New\"]')) WHERE id=?1",
        [&first.id],
    )
    .unwrap();
    conn.execute("DELETE FROM tracker_tasks WHERE id=?1", [&second.id])
        .unwrap();
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
        DROP TABLE tracker_task_filter_choices; DROP TABLE tracker_task_filter_values;
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
            for i in 0..4096 {
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
    assert_eq!(row["unread_count"], 4096);
    assert_matches_history(&store, &task_id);
    let started = std::time::Instant::now();
    let mut bytes = 0;
    for _ in 0..100 {
        let row = summary(&store, "/summary");
        bytes = row.to_string().len();
        assert!(bytes < 4096);
    }
    let read_elapsed = started.elapsed();
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
    eprintln!("summary fixture: 4096 history entries + multi-MiB task/comment; migration={migration_elapsed:?}, 100 reads={read_elapsed:?}, bytes={bytes}, append={mutation_elapsed:?}");
    // An independent reader has the same already-materialized count.
    assert_eq!(summary(&store, "/summary")["unread_count"], 4097);
    drop(store);
    let reopened = Store::new(dir.path()).unwrap();
    assert_eq!(summary(&reopened, "/summary")["unread_count"], 4097);
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
        .execute(
            "UPDATE tracker_tasks SET record=json_set(record,'$.labels',json(?1)) WHERE id=?2",
            rusqlite::params![serde_json::json!(["x".repeat(10_000)]).to_string(), task.id],
        )
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
    store.connection().execute("UPDATE tracker_tasks SET status='done',record=json_set(record,'$.status','done') WHERE id=?1",[&blocker.id]).unwrap();
    assert_eq!(
        summary(&store, "/summary")["dependencies"]["blocked"],
        false
    );
    store.connection().execute("UPDATE tracker_tasks SET status='backlog',record=json_set(record,'$.status','backlog') WHERE id=?1",[&blocker.id]).unwrap();
    assert_eq!(summary(&store, "/summary")["dependencies"]["blocked"], true);
    store
        .connection()
        .execute(
            "DELETE FROM tracker_task_dependencies WHERE task_id=?1",
            [&dependent.id],
        )
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
        tx.execute("UPDATE tracker_tasks SET record=json_set(record,'$.title','rolled back','$.labels',json('[\"Temporary\"]')) WHERE id=?1",[&task.id]).unwrap();
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
    let store = Store::new(dir.path()).unwrap();
    assert_eq!(summary(&store, "/summary")["unread_count"], 1);
    assert_matches_history(&store, &task_id);
    let choices = store
        .list_task_filter_choices("/summary", "label", None, 10)
        .unwrap();
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].value, "Restored");
}
