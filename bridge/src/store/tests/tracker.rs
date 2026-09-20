//! What the tracker's six store methods promise (spec: Issues → Storage).

use super::support::NOW;
use super::*;
use crate::store::IssueFilter;
use crate::tracker::{
    Actor, Assignee, Issue, IssueComment, IssueEvent, IssueEventKind, IssuePriority, IssueState,
    TimelineEntry, DEFAULT_STATUS,
};

const PROJECT: &str = "/repo";
const OTHER_PROJECT: &str = "/other-repo";

fn drafted(title: &str) -> Issue {
    Issue::drafted(PROJECT, title, Actor::User, NOW)
}

fn created(issue: &Issue) -> IssueEvent {
    IssueEvent::new(
        &issue.id,
        issue.created_by.clone(),
        IssueEventKind::Created,
        serde_json::json!({}),
        NOW,
    )
}

fn filed(store: &Store, title: &str) -> Issue {
    let draft = drafted(title);
    let event = created(&draft);
    store
        .create_tracker_issue(draft, &[event])
        .expect("an issue is filed")
}

fn comment(issue: &Issue, body: &str, at: &str) -> IssueComment {
    IssueComment {
        id: crate::tracker::new_comment_id(),
        issue_id: issue.id.clone(),
        author: Actor::Agent {
            agent_id: "agent-1".into(),
        },
        body: body.into(),
        refs: Vec::new(),
        created_at: at.into(),
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
        let draft = Issue::drafted(OTHER_PROJECT, "elsewhere", Actor::User, NOW);
        let event = created(&draft);
        store.create_tracker_issue(draft, &[event]).unwrap()
    };
    assert_eq!(
        elsewhere.number, 1,
        "another project counts from its own start"
    );
    assert_eq!(filed(&store, "third").number, 3, "and this one carries on");
}

/// A closed issue keeps its number forever, because nothing takes it away:
/// closing is a state, not a delete.
#[test]
fn closing_an_issue_does_not_hand_its_number_to_the_next_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut first = filed(&store, "first");
    first.state = IssueState::Closed;
    first.closed_at = Some(NOW.into());
    store.save_tracker_issue_activity(&first, &[], &[]).unwrap();

    assert_eq!(filed(&store, "second").number, 2);
    let reloaded = store.load_tracker_issue(&first.id).unwrap().unwrap();
    assert_eq!(reloaded.state, IssueState::Closed);
    assert_eq!(reloaded.number, 1);
}

/// Every field survives the round trip, including the ones that only exist
/// inside the record — an assignee and a label list are not columns.
#[test]
fn an_issue_reloads_exactly_as_it_was_written() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut issue = filed(&store, "Kanban drag does not persist");
    issue.body = "Dragging a card to In review puts it back.".into();
    issue.status = "in_review".into();
    issue.labels = vec!["bug".into(), "ui".into()];
    issue.priority = IssuePriority::High;
    issue.assignee = Some(Assignee::Agent {
        agent_id: "agent-1".into(),
    });
    issue.links.workspace_ids.push("ws-1".into());
    issue.links.conversation_ids.push("run-1".into());
    issue.links.parent_issue_id = Some("issue-parent".into());
    issue.updated_at = "2026-08-21T11:00:00Z".into();
    store.save_tracker_issue_activity(&issue, &[], &[]).unwrap();

    drop(store);
    let reopened = Store::new(dir.path()).unwrap();
    assert_eq!(
        reopened.load_tracker_issue(&issue.id).unwrap().unwrap(),
        issue,
        "a restart reads back what was written"
    );
}

/// The tracker list survives a restart, and a record written before tracking
/// existed still loads — with nobody watching, which is the true answer for it.
#[test]
fn the_tracker_list_reloads_and_a_record_without_one_still_opens() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut issue = filed(&store, "watched");
    issue.trackers = vec!["agent-1".into(), "agent-2".into()];
    store.save_tracker_issue_activity(&issue, &[], &[]).unwrap();

    // A record from before the field existed: the key is simply absent.
    let older = filed(&store, "from before");
    store
        .connection()
        .execute(
            "UPDATE tracker_issues SET record = json_remove(record, '$.trackers') WHERE id = ?1",
            [&older.id],
        )
        .unwrap();

    drop(store);
    let reopened = Store::new(dir.path()).unwrap();
    assert_eq!(
        reopened
            .load_tracker_issue(&issue.id)
            .unwrap()
            .unwrap()
            .trackers,
        vec!["agent-1".to_string(), "agent-2".into()]
    );
    assert!(
        reopened
            .load_tracker_issue(&older.id)
            .unwrap()
            .unwrap()
            .trackers
            .is_empty(),
        "an issue filed before tracking is watched by nobody, not unreadable"
    );
}

/// A comment and the event that explains it land in the same transaction as
/// the issue they are about — one refusal cannot leave a timeline claiming
/// something the record does not say.
#[test]
fn a_save_writes_the_record_its_comments_and_its_events_or_none_of_them() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut issue = filed(&store, "one");
    issue.status = "in_progress".into();
    let said = comment(&issue, "starting on this", "2026-08-21T10:01:00Z");
    let moved = IssueEvent::new(
        &issue.id,
        Actor::User,
        IssueEventKind::Moved,
        serde_json::json!({ "from": "backlog", "to": "in_progress" }),
        "2026-08-21T10:01:00Z",
    );

    store.fail_next_write();
    let refused = store.save_tracker_issue_activity(
        &issue,
        std::slice::from_ref(&said),
        std::slice::from_ref(&moved),
    );
    assert!(refused.is_err(), "the injected failure refuses the write");
    let untouched = store.load_tracker_issue(&issue.id).unwrap().unwrap();
    assert_eq!(untouched.status, DEFAULT_STATUS, "the record did not move");
    assert_eq!(
        store.load_tracker_timeline(&issue.id).unwrap().len(),
        1,
        "and nothing was said about a move that did not happen"
    );

    store
        .save_tracker_issue_activity(&issue, &[said], &[moved])
        .unwrap();
    assert_eq!(
        store.load_tracker_issue(&issue.id).unwrap().unwrap().status,
        "in_progress"
    );
    assert_eq!(store.load_tracker_timeline(&issue.id).unwrap().len(), 3);
}

/// Comments and events interleave by when they happened, and the id breaks a
/// tie — ids are time-ordered, so the order is the same for every reader.
#[test]
fn a_timeline_interleaves_comments_and_events_in_one_ascending_order() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let issue = filed(&store, "one");

    let later_comment = comment(&issue, "and now this", "2026-08-21T10:03:00Z");
    let middle_event = IssueEvent::new(
        &issue.id,
        Actor::User,
        IssueEventKind::Labelled,
        serde_json::json!({ "added": ["bug"], "removed": [] }),
        "2026-08-21T10:02:00Z",
    );
    let early_comment = comment(&issue, "first word", "2026-08-21T10:01:00Z");
    store
        .save_tracker_issue_activity(
            &issue,
            &[later_comment, early_comment],
            std::slice::from_ref(&middle_event),
        )
        .unwrap();

    let timeline = store.load_tracker_timeline(&issue.id).unwrap();
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

/// An append is idempotent by id, so a retry of a write whose answer was lost
/// adds nothing a second time.
#[test]
fn appending_the_same_comment_twice_leaves_one_of_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let issue = filed(&store, "one");
    let said = comment(&issue, "once", "2026-08-21T10:01:00Z");
    store
        .save_tracker_issue_activity(&issue, std::slice::from_ref(&said), &[])
        .unwrap();
    store
        .save_tracker_issue_activity(&issue, &[said], &[])
        .unwrap();
    assert_eq!(
        store.load_tracker_timeline(&issue.id).unwrap().len(),
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
        let draft = Issue::drafted(OTHER_PROJECT, "elsewhere", Actor::User, NOW);
        let event = created(&draft);
        store.create_tracker_issue(draft, &[event]).unwrap();
    }
    second.state = IssueState::Closed;
    second.status = "done".into();
    store
        .save_tracker_issue_activity(&second, &[], &[])
        .unwrap();

    let all = store
        .list_tracker_issues(PROJECT, IssueFilter::default())
        .unwrap();
    assert_eq!(
        all.iter().map(|issue| issue.number).collect::<Vec<_>>(),
        vec![3, 2, 1],
        "newest first, and no other project's"
    );
    assert!(all.iter().all(|issue| issue.project_path == PROJECT));

    let open = store
        .list_tracker_issues(
            PROJECT,
            IssueFilter {
                state: Some(IssueState::Open),
                status: None,
            },
        )
        .unwrap();
    assert_eq!(
        open.iter()
            .map(|issue| issue.id.clone())
            .collect::<Vec<_>>(),
        vec![third.id.clone(), first.id.clone()]
    );

    let done = store
        .list_tracker_issues(
            PROJECT,
            IssueFilter {
                state: None,
                status: Some("done"),
            },
        )
        .unwrap();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].id, second.id);

    let both = store
        .list_tracker_issues(
            PROJECT,
            IssueFilter {
                state: Some(IssueState::Open),
                status: Some("done"),
            },
        )
        .unwrap();
    assert!(both.is_empty(), "the two filters are ANDed: {both:?}");
}

/// Project deletion takes the project's whole tracker and leaves every other
/// project's alone.
#[test]
fn deleting_a_projects_issues_leaves_another_projects_standing() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let going = filed(&store, "going");
    store
        .save_tracker_issue_activity(
            &going,
            &[comment(&going, "said", "2026-08-21T10:01:00Z")],
            &[],
        )
        .unwrap();
    let staying = {
        let draft = Issue::drafted(OTHER_PROJECT, "staying", Actor::User, NOW);
        let event = created(&draft);
        store.create_tracker_issue(draft, &[event]).unwrap()
    };

    store.delete_tracker_issues_of_project(PROJECT).unwrap();

    assert!(store.load_tracker_issue(&going.id).unwrap().is_none());
    assert!(
        store.load_tracker_timeline(&going.id).unwrap().is_empty(),
        "its comments and events went with it"
    );
    assert!(store.load_tracker_issue(&staying.id).unwrap().is_some());
    assert_eq!(
        store.load_tracker_timeline(&staying.id).unwrap().len(),
        1,
        "the other project's timeline is untouched"
    );
}

/// An id nothing answers to is `None`, not an error: asking about an issue
/// that is not there is a legible question.
#[test]
fn an_unknown_issue_reads_as_absent() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    assert!(store.load_tracker_issue("issue-nobody").unwrap().is_none());
    assert!(store
        .load_tracker_timeline("issue-nobody")
        .unwrap()
        .is_empty());
    assert!(store
        .list_tracker_issues("/nowhere", IssueFilter::default())
        .unwrap()
        .is_empty());
}

/// A corrupt row fails the read by name rather than being dropped: a silently
/// missing issue is work the user filed that Build would say does not exist.
#[test]
fn a_corrupt_issue_row_names_itself_rather_than_vanishing() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let issue = filed(&store, "one");
    store
        .connection()
        .execute(
            "UPDATE tracker_issues SET record = '{' WHERE id = ?1",
            [&issue.id],
        )
        .unwrap();

    let refused = store.load_tracker_issue(&issue.id).unwrap_err();
    assert!(
        refused.to_string().contains(&issue.id),
        "the refusal names the row: {refused}"
    );
}
