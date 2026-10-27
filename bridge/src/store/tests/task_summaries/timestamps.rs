use super::*;
use crate::tracker::{TaskEvent, TaskEventKind};

const TIMESTAMPS: &[&str] = &[
    "2026-01-01t00:00:00.123z",
    "2016-12-31T23:59:60Z",
    "2016-12-31t23:59:60.123456789z",
    "2026-01-01T04:30:00.123456789+04:30",
    "2026-01-01T00:00:00.999999999-03:30",
    "1969-12-31T23:59:59.123456789Z",
    "1969-12-31t23:59:59.999999999z",
];

fn timestamp_task(store: &Store, at: &str, event: bool) -> Task {
    let mut task = Task::drafted("/summary", "canonical timestamp", Actor::User, at);
    task.watched = true;
    let task = store.create_tracker_task(task, &[]).unwrap();
    if event {
        let event = TaskEvent::new(
            &task.id,
            Actor::Agent {
                agent_id: "agent-one".into(),
            },
            TaskEventKind::Assigned,
            serde_json::json!({}),
            at,
        );
        store
            .save_tracker_task_activity(&task, &[], &[event])
            .unwrap();
    } else {
        let note = said(&task, "tc-00000000000000000000000001", at);
        store
            .save_tracker_task_activity(&task, &[note], &[])
            .unwrap();
    }
    task
}

fn assert_timestamp(store: &Store, task: &Task, at: &str) {
    let expected = crate::session_summary::message_millis(at).unwrap();
    let history = store.load_tracker_timeline(&task.id).unwrap();
    assert_eq!(history.len(), 1, "a later entry must never mask {at}");
    let legacy = crate::tracker::summary::task_activity_session(task, &history);
    assert_eq!(legacy.last_activity_ms, Some(expected));
    assert_eq!(legacy.session_started_ms, Some(expected));
    let row = ReadTransaction::new(&store.connection())
        .list_task_summary_candidates("/summary", None, 100)
        .unwrap()
        .into_iter()
        .find(|row| row.row["id"] == task.id)
        .unwrap()
        .row;
    assert_eq!(row["last_activity_ms"], expected, "last activity: {at}");
    assert_eq!(row["session_started_ms"], expected, "session start: {at}");
    let fact: Option<i64> = store
        .connection()
        .query_row(
            "SELECT ts FROM tracker_task_summary_facts WHERE task_id=?1",
            [&task.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fact, Some(expected), "stored fact: {at}");
}

#[test]
fn native_comment_and_event_timestamps_match_canonical_parser_in_isolation() {
    for event in [false, true] {
        for at in TIMESTAMPS {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::new(dir.path()).unwrap();
            let task = timestamp_task(&store, at, event);
            assert_timestamp(&store, &task, at);
        }
    }
}

#[test]
fn explicit_repair_reparses_canonical_comment_and_event_timestamps() {
    for event in [false, true] {
        for at in TIMESTAMPS {
            let dir = tempfile::tempdir().unwrap();
            let store = Store::new(dir.path()).unwrap();
            let task = timestamp_task(&store, at, event);
            store
                .connection()
                .execute(
                    "UPDATE tracker_task_summary_facts SET ts=NULL WHERE task_id=?1",
                    [&task.id],
                )
                .unwrap();
            store
                .repair_task_summaries(std::slice::from_ref(&task.id))
                .unwrap();
            assert_timestamp(&store, &task, at);
        }
    }
}

#[test]
fn old_definition_rebuild_reparses_existing_incorrect_timestamp_facts() {
    for event in [false, true] {
        for at in TIMESTAMPS {
            let dir = tempfile::tempdir().unwrap();
            let task = {
                let store = Store::new(dir.path()).unwrap();
                let task = timestamp_task(&store, at, event);
                let conn = store.connection();
                conn.execute("UPDATE tracker_task_summary_facts SET ts=NULL", [])
                    .unwrap();
                conn.execute("UPDATE meta SET value='pre-canonical-timestamp-parser' WHERE key='task_summary_v1_definition'", []).unwrap();
                conn.execute("UPDATE meta SET value='pre-canonical-timestamp-parser:' || (SELECT schema_version FROM pragma_schema_version) WHERE key='task_summary_v1_ready'", []).unwrap();
                task
            };
            let store = Store::new(dir.path()).unwrap();
            assert_timestamp(&store, &task, at);
        }
    }
}
