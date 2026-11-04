//! Exercise supported canonical writers, rather than manually staging a batch.
use super::*;
use crate::store::Store;
use crate::tracker::{Actor, Task, TaskComment, TaskEvent, TaskEventKind};
use rusqlite::params;

const NOW: &str = "2026-01-01T00:00:00Z";
#[path = "producer_guard.rs"]
mod guard;
fn events(store: &Store) -> Vec<(i64, String, String)> {
    store
        .connection()
        .prepare("SELECT seq,entity_key,operation FROM sync_journal ORDER BY seq")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}
fn task_event_count(store: &Store) -> usize {
    store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM sync_journal WHERE entity_kind='task'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap() as usize
}
fn create(store: &Store) -> Task {
    store
        .create_tracker_task(
            Task::drafted("/synthetic-project", "one", Actor::User, NOW),
            &[],
        )
        .unwrap()
}

#[test]
fn canonical_create_edit_and_unchanged_save_publish_once_per_commit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = create(&store);
    assert_eq!(events(&store).len(), 1, "create must journal");
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_eq!(events(&store).len(), 1, "unchanged save must be quiet");
    task.title = "two".into();
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_eq!(events(&store).len(), 2);
    task.body = "a body edit with the same timestamp".into();
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_eq!(
        events(&store).len(),
        3,
        "task detail edits must invalidate the opened task"
    );
    let routes: i64 = store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM sync_routes WHERE stream_kind='task' AND stream_key=?1",
            [&task.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(routes, 3);
}

#[test]
fn ephemeral_identity_availability_does_not_publish_a_task_change() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = create(&store);
    task.identities.insert(
        "synthetic-agent".into(),
        crate::tracker::TaskAgentIdentity {
            agent_id: "synthetic-agent".into(),
            name: Some("Synthetic".into()),
            project_role: None,
            ordinal: Some(1),
            workspace_id: Some("synthetic-workspace".into()),
            workspace_name: Some("Synthetic workspace".into()),
            provider: Some("codex".into()),
            available: true,
        },
    );
    store.backfill_tracker_task(&task).unwrap();
    let durable = events(&store).len();

    task.identities
        .get_mut("synthetic-agent")
        .unwrap()
        .available = false;
    store.backfill_tracker_task(&task).unwrap();

    assert_eq!(
        events(&store).len(),
        durable,
        "process-lifetime availability must not enter the journal"
    );
}

#[test]
fn summary_repair_is_quiet_when_identical_and_publishes_history_only_change() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = create(&store);

    store
        .repair_task_summaries(std::slice::from_ref(&task.id))
        .unwrap();
    let after_first_repair = events(&store).len();
    store
        .repair_task_summaries(std::slice::from_ref(&task.id))
        .unwrap();
    assert_eq!(
        events(&store).len(),
        after_first_repair,
        "a second identical repair must not publish"
    );
    let card_before: String = store
        .writer_for_test()
        .query_row(
            "SELECT record FROM tracker_task_summaries WHERE task_id=?1",
            [&task.id],
            |row| row.get(0),
        )
        .unwrap();

    let mut imported = TaskEvent::new(
        &task.id,
        Actor::Build,
        TaskEventKind::DispatchDeferred,
        json!({"reason":"history-only import"}),
        "2026-01-02T00:00:00Z",
    );
    imported.id = "te-99999999999999999999999999".into();
    {
        let conn = store.writer_for_test();
        conn.execute(
            "INSERT INTO tracker_events(id,task_id,at,record) VALUES(?1,?2,?3,?4)",
            params![
                imported.id,
                imported.task_id,
                imported.at,
                serde_json::to_string(&imported).unwrap()
            ],
        )
        .unwrap();
    }
    store
        .repair_task_summaries(std::slice::from_ref(&task.id))
        .unwrap();
    assert_eq!(
        events(&store).len(),
        after_first_repair + 1,
        "a changed timeline head must publish even when its summary is unchanged"
    );
    let card_after: String = store
        .writer_for_test()
        .query_row(
            "SELECT record FROM tracker_task_summaries WHERE task_id=?1",
            [&task.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        card_after, card_before,
        "the imported event is history-only"
    );
}

#[test]
fn multiple_activity_appends_in_one_transaction_have_one_revision() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = create(&store);
    let comments:Vec<TaskComment>=(0..10).map(|n|serde_json::from_value(json!({"id":format!("synthetic-comment-{n}"),"task_id":task.id,"created_at":NOW,"author":{"kind":"user"},"body":"note"})).unwrap()).collect();
    store
        .save_tracker_task_activity(&task, &comments, &[])
        .unwrap();
    assert_eq!(
        events(&store).len(),
        2,
        "one final task publication per SQL transaction"
    );
    store
        .save_tracker_task_activity(&task, &comments, &[])
        .unwrap();
    assert_eq!(
        events(&store).len(),
        2,
        "duplicate comment ids must be quiet"
    );
}

#[test]
fn canonical_task_and_journal_both_rollback_on_append_failure() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = create(&store);
    store.connection().execute_batch("CREATE TRIGGER synthetic_reject_sync BEFORE INSERT ON sync_journal BEGIN SELECT RAISE(ABORT,'synthetic journal failure'); END;").unwrap();
    task.title = "must rollback".into();
    assert!(store.save_tracker_task_activity(&task, &[], &[]).is_err());
    assert_eq!(
        store.load_tracker_task(&task.id).unwrap().unwrap().title,
        "one"
    );
    assert_eq!(events(&store).len(), 1);
}

#[test]
fn snapshot_review_header_change_invalidates_task_even_without_a_card_delta() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = create(&store);
    let snapshot = crate::reviews::model::ReviewSnapshot {
        publication: None,
        id: "synthetic-review-snapshot".into(),
        number: 0,
        author: Actor::User,
        created_at: NOW.into(),
        directories: vec![],
    };
    store
        .save_review_snapshot(&task.id, "synthetic-workspace", 0, snapshot.clone())
        .unwrap();
    assert_eq!(
        events(&store).len(),
        2,
        "review headers also invalidate the opened task"
    );
    // Dev's new legacy marker is already true: a second retained snapshot
    // changes the review header while leaving that card marker unchanged.
    let mut second = snapshot;
    second.id = "synthetic-review-snapshot-two".into();
    store
        .save_review_snapshot(&task.id, "synthetic-workspace", 1, second)
        .unwrap();
    assert_eq!(events(&store).len(), 3);
}

#[test]
fn task_state_status_assignment_labels_and_read_watch_dismiss_writers_publish() {
    use crate::tracker::{Assignee, TaskState};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = create(&store);
    let mutations: [fn(&mut Task); 9] = [
        |t| t.state = TaskState::Closed,
        |t| {
            t.state = TaskState::Open;
            t.status = "ready".into();
        },
        |t| {
            t.assignee = Some(Assignee::Agent {
                agent_id: "Actual\0suffix".into(),
            })
        },
        |t| t.labels = vec!["API".into()],
        |t| t.watched = true,
        |t| t.read_through = Some("synthetic-read-marker".into()),
        |t| t.watched = false,
        |t| t.dismissed_through = Some("synthetic-dismiss-marker".into()),
        |t| t.trackers = vec!["synthetic-tracker".into()],
    ];
    for (n, mutate) in mutations.into_iter().enumerate() {
        mutate(&mut task);
        store.save_tracker_task_activity(&task, &[], &[]).unwrap();
        assert_eq!(task_event_count(&store), n + 2, "producer case {n}");
    }
}

#[test]
fn committed_delivery_summary_changes_publish_but_private_checkpoints_do_not() {
    use crate::store::DeliveryIntent;
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = create(&store);
    let intent = DeliveryIntent {
        id: String::new(),
        task_id: task.id.clone(),
        project_path: task.project_path.clone(),
        generation: 0,
        target: json!({"kind":"agent","agent_id":"synthetic-agent"}),
        actor: Actor::User,
        sender: None,
        note: None,
        operation_id: String::new(),
        state: "queued".into(),
        reservation: json!(null),
        admitted: None,
        error: None,
    };
    let intent = store.accept_delivery_intent(&task, &[], intent).unwrap();
    assert_eq!(events(&store).len(), 2);
    store
        .claim_delivery_intent(&intent.id, intent.generation)
        .unwrap();
    assert_eq!(
        events(&store).len(),
        2,
        "provisioning retains the same displayed queued summary"
    );
    store
        .checkpoint_delivery_intent(&intent.id, intent.generation, json!({}), None)
        .unwrap();
    assert_eq!(
        events(&store).len(),
        2,
        "private reservation checkpoints do not change the summary"
    );
    store
        .fail_delivery_intent(&intent.id, intent.generation, "synthetic failure")
        .unwrap();
    assert_eq!(events(&store).len(), 3);
}

#[test]
fn task_project_is_immutable_and_project_delete_keeps_its_tombstone() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = create(&store);
    task.project_path = "/synthetic-new-project".into();
    assert!(matches!(
        store.save_tracker_task_activity(&task, &[], &[]),
        Err(crate::store::StoreError::WaitingOnInvalid(_))
    ));
    assert_eq!(events(&store).len(), 1, "a refused reparent cannot publish");
    store
        .delete_tracker_tasks_of_project("/synthetic-project", |_| Ok(()))
        .unwrap();
    assert_eq!(events(&store).last().unwrap().2, "delete");
    let revision: (i64, bool) = store
        .connection()
        .query_row(
            "SELECT seq,deleted FROM sync_revisions WHERE entity_kind='task' AND entity_key=?1",
            [&task.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(revision, (2, true));
}

#[test]
fn dependencies_publish_both_endpoints_and_readiness_reconciliation() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let dependent = create(&store);
    let mut blocker = create(&store);
    store
        .edit_waiting_on(
            &dependent.project_path,
            &dependent.id,
            std::slice::from_ref(&blocker.id),
            &[],
            &Actor::User,
            NOW,
        )
        .unwrap();
    while store.reconcile_waiting_on(2).unwrap().pending {}
    let before = events(&store).last().unwrap().0;
    let summary = store
        .list_task_summary_candidates(&dependent.project_path, None, 10)
        .unwrap()
        .into_iter()
        .find(|r| r.row["id"] == dependent.id)
        .unwrap();
    assert_eq!(summary.row["dependencies"]["blocked"], true);
    blocker.status = "done".into();
    store
        .save_tracker_task_activity(&blocker, &[], &[])
        .unwrap();
    while store.reconcile_waiting_on(2).unwrap().pending {}
    let after = events(&store);
    for id in [&dependent.id, &blocker.id] {
        assert!(
            after.iter().any(|(seq, key, _)| seq > &before && key == id),
            "missing readiness/endpoint revision for {id}"
        );
    }
    let summary = store
        .list_task_summary_candidates(&dependent.project_path, None, 10)
        .unwrap()
        .into_iter()
        .find(|r| r.row["id"] == dependent.id)
        .unwrap();
    assert_eq!(summary.row["dependencies"]["blocked"], false);
    store
        .edit_waiting_on(
            &dependent.project_path,
            &dependent.id,
            &[],
            std::slice::from_ref(&blocker.id),
            &Actor::User,
            NOW,
        )
        .unwrap();
    let after_remove = events(&store);
    assert!(after_remove.last().unwrap().0 > after.last().unwrap().0);
}
