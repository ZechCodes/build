//! What the tracker's six store methods promise (spec: Tasks → Storage).

use super::support::NOW;
use super::*;
use crate::store::TaskFilter;
use crate::tracker::{
    Actor, Assignee, Task, TaskComment, TaskEvent, TaskEventKind, TaskPriority, TaskState,
    TimelineEntry, DEFAULT_STATUS,
};

const PROJECT: &str = "/repo";
const OTHER_PROJECT: &str = "/other-repo";

fn drafted(title: &str) -> Task {
    Task::drafted(PROJECT, title, Actor::User, NOW)
}

fn created(task: &Task) -> TaskEvent {
    TaskEvent::new(
        &task.id,
        task.created_by.clone(),
        TaskEventKind::Created,
        serde_json::json!({}),
        NOW,
    )
}

fn filed(store: &Store, title: &str) -> Task {
    let draft = drafted(title);
    let event = created(&draft);
    store
        .create_tracker_task(draft, &[event])
        .expect("a task is filed")
}

fn comment(task: &Task, body: &str, at: &str) -> TaskComment {
    TaskComment {
        id: crate::tracker::new_comment_id(),
        task_id: task.id.clone(),
        author: Actor::Agent {
            agent_id: "agent-1".into(),
        },
        body: body.into(),
        mentions_user: false,
        notifies_user: false,
        refs: Vec::new(),
        attachments: Vec::new(),
        created_at: at.into(),
        author_context: None,
    }
}

/// The number is the tracker's own counter, per project, minted by the store
/// because only the store can read the maximum and write past it atomically.
#[test]
fn numbers_run_from_one_per_project_and_two_projects_do_not_share_them() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();

    let first = filed(&store, "first");
    let second = filed(&store, "second");
    assert_eq!((first.number, second.number), (1, 2));

    let elsewhere = {
        let draft = Task::drafted(OTHER_PROJECT, "elsewhere", Actor::User, NOW);
        let event = created(&draft);
        store.create_tracker_task(draft, &[event]).unwrap()
    };
    assert_eq!(
        elsewhere.number, 1,
        "another project counts from its own start"
    );
    assert_eq!(filed(&store, "third").number, 3, "and this one carries on");
}

/// A closed task keeps its number forever, because nothing takes it away:
/// closing is a state, not a delete.
#[test]
fn closing_a_task_does_not_hand_its_number_to_the_next_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut first = filed(&store, "first");
    first.state = TaskState::Closed;
    first.closed_at = Some(NOW.into());
    store.save_tracker_task_activity(&first, &[], &[]).unwrap();

    assert_eq!(filed(&store, "second").number, 2);
    let reloaded = store.load_tracker_task(&first.id).unwrap().unwrap();
    assert_eq!(reloaded.state, TaskState::Closed);
    assert_eq!(reloaded.number, 1);
}

/// Every field survives the round trip, including the ones that only exist
/// inside the record — an assignee and a label list are not columns.
#[test]
fn a_task_reloads_exactly_as_it_was_written() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = filed(&store, "Kanban drag does not persist");
    task.body = "Dragging a card to In review puts it back.".into();
    task.status = "in_review".into();
    task.labels = vec!["bug".into(), "ui".into()];
    task.priority = TaskPriority::High;
    task.assignee = Some(Assignee::Agent {
        agent_id: "agent-1".into(),
    });
    task.links.workspace_ids.push("ws-1".into());
    task.links.conversation_ids.push("run-1".into());
    task.links.parent_task_id = Some("task-parent".into());
    task.updated_at = "2026-08-21T11:00:00Z".into();
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();

    drop(store);
    let reopened = Store::new(dir.path()).unwrap();
    assert_eq!(
        reopened.load_tracker_task(&task.id).unwrap().unwrap(),
        task,
        "a restart reads back what was written"
    );
}

/// The tracker list survives a restart, and a record written before tracking
/// existed still loads — with nobody watching, which is the true answer for it.
#[test]
fn the_tracker_list_reloads_and_a_record_without_one_still_opens() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = filed(&store, "watched");
    task.trackers = vec!["agent-1".into(), "agent-2".into()];
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();

    // A record from before the field existed: the key is simply absent.
    let older = filed(&store, "from before");
    store
        .connection()
        .execute(
            "UPDATE tracker_tasks SET record = json_remove(record, '$.trackers') WHERE id = ?1",
            [&older.id],
        )
        .unwrap();

    drop(store);
    let reopened = Store::new(dir.path()).unwrap();
    assert_eq!(
        reopened
            .load_tracker_task(&task.id)
            .unwrap()
            .unwrap()
            .trackers,
        vec!["agent-1".to_string(), "agent-2".into()]
    );
    assert!(
        reopened
            .load_tracker_task(&older.id)
            .unwrap()
            .unwrap()
            .trackers
            .is_empty(),
        "a task filed before tracking is watched by nobody, not unreadable"
    );
}

/// A comment and the event that explains it land in the same transaction as
/// the task they are about — one refusal cannot leave a timeline claiming
/// something the record does not say.
#[test]
fn a_save_writes_the_record_its_comments_and_its_events_or_none_of_them() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut task = filed(&store, "one");
    task.status = "in_progress".into();
    let said = comment(&task, "starting on this", "2026-08-21T10:01:00Z");
    let moved = TaskEvent::new(
        &task.id,
        Actor::User,
        TaskEventKind::Moved,
        serde_json::json!({ "from": "backlog", "to": "in_progress" }),
        "2026-08-21T10:01:00Z",
    );

    store.fail_next_write();
    let refused = store.save_tracker_task_activity(
        &task,
        std::slice::from_ref(&said),
        std::slice::from_ref(&moved),
    );
    assert!(refused.is_err(), "the injected failure refuses the write");
    let untouched = store.load_tracker_task(&task.id).unwrap().unwrap();
    assert_eq!(untouched.status, DEFAULT_STATUS, "the record did not move");
    assert_eq!(
        store.load_tracker_timeline(&task.id).unwrap().len(),
        1,
        "and nothing was said about a move that did not happen"
    );

    store
        .save_tracker_task_activity(&task, &[said], &[moved])
        .unwrap();
    assert_eq!(
        store.load_tracker_task(&task.id).unwrap().unwrap().status,
        "in_progress"
    );
    assert_eq!(store.load_tracker_timeline(&task.id).unwrap().len(), 3);
}

/// Comments and events interleave by when they happened, and the id breaks a
/// tie — ids are time-ordered, so the order is the same for every reader.
#[test]
fn a_timeline_interleaves_comments_and_events_in_one_ascending_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = filed(&store, "one");

    let later_comment = comment(&task, "and now this", "2026-08-21T10:03:00Z");
    let middle_event = TaskEvent::new(
        &task.id,
        Actor::User,
        TaskEventKind::Labelled,
        serde_json::json!({ "added": ["bug"], "removed": [] }),
        "2026-08-21T10:02:00Z",
    );
    let early_comment = comment(&task, "first word", "2026-08-21T10:01:00Z");
    store
        .save_tracker_task_activity(
            &task,
            &[later_comment, early_comment],
            std::slice::from_ref(&middle_event),
        )
        .unwrap();

    let timeline = store.load_tracker_timeline(&task.id).unwrap();
    let kinds: Vec<String> = timeline
        .iter()
        .map(|entry| match entry {
            TimelineEntry::Comment(said) => format!("comment:{}", said.body),
            TimelineEntry::Event(happened) => format!("event:{}", happened.kind.as_str()),
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            "event:created".to_string(),
            "comment:first word".to_string(),
            "event:labelled".to_string(),
            "comment:and now this".to_string(),
        ]
    );
}

/// Several tasks with timelines of their own: comments and events out of
/// insertion order, a comment and an event stamped in the same second, and a
/// task with nothing said on it beyond its creation.
fn tasks_with_timelines(store: &Store, count: usize) -> Vec<Task> {
    (0..count)
        .map(|index| {
            let task = filed(store, &format!("task {index}"));
            let minute = |m: usize| format!("2026-08-21T10:{:02}:00Z", (index + m) % 60);
            let comments: Vec<TaskComment> = (0..index % 4)
                .rev()
                .map(|n| comment(&task, &format!("said {n}"), &minute(n * 2 + 1)))
                .collect();
            let events: Vec<TaskEvent> = (0..index % 3)
                .map(|n| {
                    TaskEvent::new(
                        &task.id,
                        Actor::User,
                        TaskEventKind::Labelled,
                        serde_json::json!({ "added": [format!("l{n}")], "removed": [] }),
                        &minute(n * 3 + 1),
                    )
                })
                .collect();
            store
                .save_tracker_task_activity(&task, &comments, &events)
                .unwrap();
            task
        })
        .collect()
}

/// Many tasks' timelines in one read answer, for every task, exactly what
/// reading that task's timeline alone answers — the same entries in the same
/// order — and an id nothing answers to reads as an empty timeline.
#[test]
fn many_timelines_read_together_match_each_read_alone() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let tasks = tasks_with_timelines(&store, 12);
    let mut ids: Vec<String> = tasks.iter().map(|task| task.id.clone()).collect();
    ids.push("task-nobody".into());
    ids.push(tasks[3].id.clone());

    let together = store.load_tracker_timelines(&ids).unwrap();

    assert_eq!(together.len(), tasks.len() + 1, "one timeline per id asked");
    for id in &ids {
        assert_eq!(
            together[id],
            store.load_tracker_timeline(id).unwrap(),
            "{id} reads the same together as alone"
        );
    }
    assert!(together["task-nobody"].is_empty());
    assert!(
        together.values().any(|timeline| timeline.len() > 4),
        "the fixture interleaves more than a creation event"
    );
}

/// A list longer than one statement may bind is read in chunks, and a task
/// on either side of a chunk boundary still gets its whole timeline.
#[test]
fn many_timelines_read_across_the_parameter_limit() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let tasks = tasks_with_timelines(&store, 6);
    let mut ids: Vec<String> = (0..1_200).map(|n| format!("task-absent-{n}")).collect();
    for (slot, task) in [0, 499, 500, 501, 999, 1_199].into_iter().zip(&tasks) {
        ids[slot] = task.id.clone();
    }

    let together = store.load_tracker_timelines(&ids).unwrap();

    assert_eq!(together.len(), ids.len());
    for task in &tasks {
        assert_eq!(
            together[&task.id],
            store.load_tracker_timeline(&task.id).unwrap()
        );
    }
}

/// An append is idempotent by id, so a retry of a write whose answer was lost
/// adds nothing a second time.
#[test]
fn appending_the_same_comment_twice_leaves_one_of_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = filed(&store, "one");
    let said = comment(&task, "once", "2026-08-21T10:01:00Z");
    store
        .save_tracker_task_activity(&task, std::slice::from_ref(&said), &[])
        .unwrap();
    store
        .save_tracker_task_activity(&task, &[said], &[])
        .unwrap();
    assert_eq!(
        store.load_tracker_timeline(&task.id).unwrap().len(),
        2,
        "the created event and the one comment"
    );
}

/// The list is one project's, newest first, narrowed by the two filters the
/// store answers in SQL.
#[test]
fn the_list_is_one_projects_newest_first_and_the_filters_narrow_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let first = filed(&store, "first");
    let mut second = filed(&store, "second");
    let third = filed(&store, "third");
    {
        let draft = Task::drafted(OTHER_PROJECT, "elsewhere", Actor::User, NOW);
        let event = created(&draft);
        store.create_tracker_task(draft, &[event]).unwrap();
    }
    second.state = TaskState::Closed;
    second.status = "done".into();
    store.save_tracker_task_activity(&second, &[], &[]).unwrap();

    let all = store
        .list_tracker_tasks(PROJECT, TaskFilter::default())
        .unwrap();
    assert_eq!(
        all.iter().map(|task| task.number).collect::<Vec<_>>(),
        vec![3, 2, 1],
        "newest first, and no other project's"
    );
    assert!(all.iter().all(|task| task.project_path == PROJECT));

    let open = store
        .list_tracker_tasks(
            PROJECT,
            TaskFilter {
                state: Some(TaskState::Open),
                status: None,
            },
        )
        .unwrap();
    assert_eq!(
        open.iter().map(|task| task.id.clone()).collect::<Vec<_>>(),
        vec![third.id.clone(), first.id.clone()]
    );

    let done = store
        .list_tracker_tasks(
            PROJECT,
            TaskFilter {
                state: None,
                status: Some("done"),
            },
        )
        .unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].id, second.id);

    let both = store
        .list_tracker_tasks(
            PROJECT,
            TaskFilter {
                state: Some(TaskState::Open),
                status: Some("done"),
            },
        )
        .unwrap();
    assert!(both.is_empty(), "the two filters are ANDed: {both:?}");
}

/// Project deletion takes the project's whole tracker and leaves every other
/// project's alone.
#[test]
fn deleting_a_projects_tasks_leaves_another_projects_standing() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let going = filed(&store, "going");
    store
        .save_tracker_task_activity(
            &going,
            &[comment(&going, "said", "2026-08-21T10:01:00Z")],
            &[],
        )
        .unwrap();
    let staying = {
        let draft = Task::drafted(OTHER_PROJECT, "staying", Actor::User, NOW);
        let event = created(&draft);
        store.create_tracker_task(draft, &[event]).unwrap()
    };

    store
        .delete_tracker_tasks_of_project(PROJECT, |_| Ok(()))
        .unwrap();

    assert!(store.load_tracker_task(&going.id).unwrap().is_none());
    assert!(
        store.load_tracker_timeline(&going.id).unwrap().is_empty(),
        "its comments and events went with it"
    );
    assert!(store.load_tracker_task(&staying.id).unwrap().is_some());
    assert_eq!(
        store.load_tracker_timeline(&staying.id).unwrap().len(),
        1,
        "the other project's timeline is untouched"
    );
}

/// An id nothing answers to is `None`, not an error: asking about a task
/// that is not there is a legible question.
#[test]
fn an_unknown_task_reads_as_absent() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    assert!(store.load_tracker_task("task-nobody").unwrap().is_none());
    assert!(store
        .load_tracker_timeline("task-nobody")
        .unwrap()
        .is_empty());
    assert!(store
        .list_tracker_tasks("/nowhere", TaskFilter::default())
        .unwrap()
        .is_empty());
}

/// A corrupt row fails the read by name rather than being dropped: a silently
/// missing task is work the user filed that Build would say does not exist.
#[test]
fn a_corrupt_task_row_names_itself_rather_than_vanishing() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let task = filed(&store, "one");
    store
        .connection()
        .execute(
            "UPDATE tracker_tasks SET record = '{' WHERE id = ?1",
            [&task.id],
        )
        .unwrap();

    let refused = store.load_tracker_task(&task.id).unwrap_err();
    assert!(
        refused.to_string().contains(&task.id),
        "the refusal names the row: {refused}"
    );
}

/// A list page reads no row it throws away in SQL (#85): whatever the list
/// is narrowed by, every column is an equality of the index seek in front of
/// `number`, so the scan bound on a page bounds the whole read. Narrowed by
/// state and status through an index on only one of them, a page of the
/// closed tasks in a column full of open ones would step over every open
/// one before finding nothing.
#[test]
fn every_list_page_seeks_straight_to_the_rows_it_may_answer() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let connection = store.connection();
    let filters = [
        (TaskFilter::default(), vec![]),
        (
            TaskFilter {
                state: Some(TaskState::Closed),
                status: None,
            },
            vec!["state=?"],
        ),
        (
            TaskFilter {
                state: None,
                status: Some("backlog"),
            },
            vec!["status=?"],
        ),
        (
            TaskFilter {
                state: Some(TaskState::Closed),
                status: Some("backlog"),
            },
            vec!["state=?", "status=?"],
        ),
    ];
    for (filter, narrowed) in filters {
        for below in [None, Some(40)] {
            let (statement, _) = crate::store::tracker::stretch_query(PROJECT, filter, below);
            let plan = super::support::query_plan(&connection, &statement);
            let seek = plan
                .iter()
                .find(|step| step.contains("SEARCH tracker_tasks USING"))
                .unwrap_or_else(|| panic!("{statement} does not seek: {plan:?}"));
            let mut constraints = vec!["project_key=?"];
            constraints.extend(narrowed.iter());
            if below.is_some() {
                constraints.push("number<?");
            }
            for constraint in constraints {
                assert!(
                    seek.contains(constraint),
                    "{statement} reads rows it throws away, {constraint} is not in the seek: {plan:?}"
                );
            }
            assert!(
                !plan
                    .iter()
                    .any(|step| step.contains("SCAN") || step.contains("TEMP B-TREE")),
                "{statement} walks or sorts the list: {plan:?}"
            );
        }
    }
}
