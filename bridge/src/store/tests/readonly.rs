use super::support::*;
use super::*;

#[tokio::test]
async fn readonly_history_matches_bounded_pages_and_restores_skeleton_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut record = run_record("run-readonly", None, NOW);
    let agent = &mut record.agents[0];
    agent.thread.post_user("first", None, NOW);
    for _ in 0..300 {
        agent.thread.push_event(
            crate::thread::ThreadEventKind::ToolUse,
            None,
            None,
            None,
            NOW,
        );
    }
    agent.thread.post_agent("last", None, NOW);
    let id = agent.id.clone();
    let count = agent.thread.total_item_count();
    let last = agent.thread.last_sequence();
    store.save_run(&record).unwrap();
    let before = store.total_changes();
    let expected = store
        .thread_conversation_page_newest_activity(&id, None, None, 3, false)
        .unwrap();
    let mut connection = store.read_pool().acquire().await;
    connection
        .read(|read| {
            let (owner, skeleton) = read.load_agent_skeleton(&id).unwrap().unwrap();
            assert_eq!(owner, record.id);
            assert!(skeleton.thread.items.is_empty());
            assert_eq!(skeleton.thread.total_item_count(), count);
            assert_eq!(skeleton.thread.last_sequence(), last);
            let actual = read
                .thread_conversation_page_newest_activity(&id, None, None, 3, false)
                .unwrap();
            assert_eq!(
                serde_json::to_value(&actual.0.items).unwrap(),
                serde_json::to_value(&expected.0.items).unwrap()
            );
            assert_eq!(
                serde_json::to_value(&actual.0.digests).unwrap(),
                serde_json::to_value(&expected.0.digests).unwrap()
            );
            assert_eq!(actual.1, expected.1);
        })
        .unwrap();
    assert_eq!(store.total_changes(), before);
}

#[tokio::test]
async fn readonly_binding_and_rows_stay_on_one_generation_through_reset() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut record = run_record("run-readonly-reset", None, NOW);
    record.agents[0].thread.post_user("old", None, NOW);
    store.save_run(&record).unwrap();
    let id = record.agents[0].id.clone();
    let pool = store.read_pool();
    let mut reader = pool.acquire().await;
    reader
        .read(|read| {
            let original = read.load_agent_skeleton(&id).unwrap().unwrap().1.thread;
            record.agents[0].thread.generation_revision += 1;
            record.agents[0].thread.items.clear();
            store
                .reset_conversation(&id, &record.agents, &HashMap::new(), &HashSet::new())
                .unwrap();
            assert_eq!(
                read.load_agent_skeleton(&id)
                    .unwrap()
                    .unwrap()
                    .1
                    .thread
                    .generation_revision,
                original.generation_revision
            );
            assert_eq!(read.thread_page(&id, None, 10).unwrap().len(), 1);
        })
        .unwrap();
    reader
        .read(|read| {
            let fresh = read.load_agent_skeleton(&id).unwrap().unwrap().1.thread;
            assert_eq!(fresh.generation_revision, 1);
            assert_eq!(fresh.total_item_count(), 0);
            assert!(read.thread_page(&id, None, 10).unwrap().is_empty());
        })
        .unwrap();
}

#[tokio::test]
async fn readonly_tasks_timeline_dependencies_and_cursor_key_use_one_snapshot() {
    use crate::tracker::{Actor, Task, TaskEvent, TaskEventKind};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let draft = Task::drafted("/repo", "original", Actor::User, NOW);
    let event = TaskEvent::new(
        &draft.id,
        Actor::User,
        TaskEventKind::Created,
        serde_json::json!({}),
        NOW,
    );
    let mut task = store.create_tracker_task(draft, &[event]).unwrap();
    let cursor_key = store.tracker_list_key().unwrap();
    let pool = store.read_pool();
    let mut reader = pool.acquire().await;
    let before = store.total_changes();
    reader
        .read(|read| {
            assert_eq!(read.tracker_list_key().unwrap(), cursor_key);
            let listed = read
                .list_tracker_tasks("/repo", TaskFilter::default())
                .unwrap();
            assert_eq!(listed.len(), 1);
            assert_eq!(read.load_tracker_timeline(&task.id).unwrap().len(), 1);
            assert!(!read.waiting_on_summary("/repo", &task.id).unwrap().blocked);
            assert_eq!(store.total_changes(), before);
            task.title = "committed later".into();
            store.save_tracker_task_activity(&task, &[], &[]).unwrap();
            assert_eq!(
                read.load_tracker_task(&task.id).unwrap().unwrap().title,
                "original"
            );
        })
        .unwrap();
    reader
        .read(|read| {
            assert_eq!(
                read.load_tracker_task(&task.id).unwrap().unwrap().title,
                "committed later"
            )
        })
        .unwrap();
}

#[tokio::test]
async fn readonly_unbounded_task_list_reports_checkout_and_transaction_duration() {
    use crate::tracker::{Actor, Task, TaskEvent, TaskEventKind};
    use std::time::Instant;

    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    for number in 0..501 {
        let task = Task::drafted("/repo", &format!("task {number}"), Actor::User, NOW);
        let event = TaskEvent::new(
            &task.id,
            Actor::User,
            TaskEventKind::Created,
            serde_json::json!({}),
            NOW,
        );
        store.create_tracker_task(task, &[event]).unwrap();
    }
    let writes_before = store.total_changes();
    let pool = store.read_pool();
    let checkout_started = Instant::now();
    let mut reader = pool.acquire().await;
    let checkout_us = checkout_started.elapsed().as_micros();
    let transaction_started = Instant::now();
    let (count, timeline_rows, dependency_rows) = reader
        .read(|read| -> Result<_, StoreError> {
            let tasks = read.list_tracker_tasks("/repo", TaskFilter::default())?;
            let ids: Vec<_> = tasks.iter().map(|task| task.id.clone()).collect();
            let timelines = read.load_tracker_timelines(&ids)?;
            let dependencies = read.waiting_on_summaries("/repo", &ids)?;
            Ok((
                tasks.len(),
                timelines.values().map(Vec::len).sum::<usize>(),
                dependencies.len(),
            ))
        })
        .unwrap()
        .unwrap();
    let transaction_us = transaction_started.elapsed().as_micros();
    assert_eq!((count, timeline_rows, dependency_rows), (501, 501, 501));
    assert_eq!(store.total_changes(), writes_before);
    println!(
        "sql_read_sizing verb=tasks.list count={count} checkout_us={checkout_us} transaction_us={transaction_us}"
    );
}

#[tokio::test]
async fn readonly_legacy_thread_identity_matches_roster_restore() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut record = run_record("run-legacy-thread-identity", None, NOW);
    record.agents[0]
        .thread
        .post_user("history remains addressable", None, NOW);
    let id = record.agents[0].id.clone();
    store.save_run(&record).unwrap();
    store.connection().execute(
        "UPDATE agents SET record = json_set(record, '$.thread.id', '', '$.thread.agent.id', 'legacy-owner') WHERE id = ?1",
        [&id],
    ).unwrap();
    let writes = store.total_changes();
    let pool = store.read_pool();
    let mut reader = pool.acquire().await;
    reader
        .read(|read| {
            let (_, restored) = read.load_agent_skeleton(&id).unwrap().unwrap();
            assert_eq!(restored.thread.agent.id, id);
            assert_eq!(restored.thread.id, format!("thread:{id}"));
            assert_eq!(
                read.thread_page(&restored.thread.agent.id, None, 10)
                    .unwrap()
                    .len(),
                1
            );
        })
        .unwrap();
    assert_eq!(store.total_changes(), writes);

    let reset_id = format!("thread-reset:{id}");
    record.agents[0].thread.id = reset_id.clone();
    record.agents[0].thread.generation_revision = 2;
    store.save_run(&record).unwrap();
    let writes = store.total_changes();
    reader
        .read(|read| {
            let (_, restored) = read.load_agent_skeleton(&id).unwrap().unwrap();
            assert_eq!(restored.thread.agent.id, id);
            assert_eq!(restored.thread.id, reset_id);
            assert_eq!(restored.thread.generation_revision, 2);
        })
        .unwrap();
    assert_eq!(store.total_changes(), writes);
}
