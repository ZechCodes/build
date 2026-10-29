//! Behavioral parity with canonical history, not persisted projection booleans.
//! Sources: tracker/summary.rs unread_since_mark/after/attention_bookkeeping;
//! app/tracker/inbox.rs watched_task_observation_row/task_is_done_until_next;
//! spa/src/core/trackerAttentionModel.js hasUnreadAsk/reasonsOf/unreadAsks.
use super::*;
use crate::store::TaskQueryFilter;
use crate::tracker::summary::{after, attention_bookkeeping, entry_id, unread_since_mark};
use crate::tracker::{Assignee, TaskEvent, TaskEventKind, TaskState, TimelineEntry};
use serde_json::json;

fn assert_attention(store: &Store, id: &str, expected: (bool, bool), why: &str) {
    let task = store.load_tracker_task(id).unwrap().unwrap();
    let timeline = store.load_tracker_timeline(id).unwrap();
    let blocked = store
        .waiting_on_summary(&task.project_path, id)
        .unwrap()
        .blocked;
    let (comments, asks) = counts_after_read(&task, &timeline);
    let unfinished = task.state != TaskState::Closed && task.status != "done";
    let user = task.assignee == Some(Assignee::User) && !blocked;
    let unread = unread_since_mark(&task, &timeline);
    let meaningful: Vec<_> = timeline
        .iter()
        .filter(|entry| !attention_bookkeeping(entry))
        .collect();
    let cleared = task.dismissed_through.is_some()
        && !meaningful
            .iter()
            .any(|entry| after(task.dismissed_through.as_deref(), entry_id(entry)));
    let inbox = task.watched && !meaningful.is_empty() && !cleared && unread > 0;
    let canonical = (
        unfinished && (user || (task.watched && asks > 0)),
        unfinished && (task.status == "in_review" || user || (inbox && comments > 0)),
    );
    assert_eq!(canonical, expected, "canonical source: {why}");
    for (rule, want) in [("asked", canonical.0), ("legacy", canonical.1)] {
        let actual = store
            .task_query_matches(
                id,
                &TaskQueryFilter {
                    attention_rule: Some(rule.into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(actual, want, "{rule}: {why}");
    }
}
fn counts_after_read(task: &Task, timeline: &[TimelineEntry]) -> (usize, usize) {
    let mut comments = 0;
    let mut asks = 0;
    for entry in timeline
        .iter()
        .filter(|entry| after(task.read_through.as_deref(), entry_id(entry)))
    {
        match entry {
            TimelineEntry::Comment(comment) if matches!(comment.author, Actor::Agent { .. }) => {
                comments += 1;
                asks += usize::from(comment.mentions_user || comment.notifies_user);
            }
            TimelineEntry::Event(event)
                if event.kind == TaskEventKind::Created
                    && matches!(event.actor, Actor::Agent { .. }) =>
            {
                asks += usize::from(event.mentions_user)
            }
            _ => {}
        }
    }
    (comments, asks)
}
fn event(task: &Task, n: usize, kind: TaskEventKind, payload: serde_json::Value) -> TaskEvent {
    let mut event = TaskEvent::new(
        &task.id,
        Actor::Agent {
            agent_id: "agent-one".into(),
        },
        kind,
        payload,
        support::NOW,
    );
    event.id = format!("te-{n:026}");
    event
}
fn comment(task: &Task, n: usize, mentions: bool, notifies: bool) -> crate::tracker::TaskComment {
    let mut note = said(task, &format!("tc-{n:026}"), support::NOW);
    note.mentions_user = mentions;
    note.notifies_user = notifies;
    note
}

#[test]
fn attention_distinguishes_agent_traffic_requests_read_and_watch_floors() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/attention", "attention", Actor::User, support::NOW);
    draft.watched = true;
    let mut task = store.create_tracker_task(draft, &[]).unwrap();
    assert_attention(
        &store,
        &task.id,
        (false, false),
        "empty history has no inbox",
    );
    let mut own_note = comment(&task, 5, true, true);
    own_note.author = Actor::User;
    store
        .save_tracker_task_activity(&task, &[own_note], &[])
        .unwrap();
    assert_attention(
        &store,
        &task.id,
        (false, false),
        "user mentions are not requests from an agent",
    );
    store
        .save_tracker_task_activity(&task, &[comment(&task, 10, false, false)], &[])
        .unwrap();
    assert_attention(
        &store,
        &task.id,
        (false, true),
        "ordinary agent comment only legacy",
    );
    store
        .save_tracker_task_activity(&task, &[comment(&task, 20, true, false)], &[])
        .unwrap();
    assert_attention(&store, &task.id, (true, true), "explicit mention asks");
    task.watch_started_after = Some(format!("te-{:026}", 30));
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_attention(
        &store,
        &task.id,
        (true, false),
        "watch floor clears unread but does not advance read floor",
    );
    task.read_through = Some(format!("tc-{:026}", 20));
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_attention(
        &store,
        &task.id,
        (false, false),
        "read floor clears prior requests",
    );
    store
        .save_tracker_task_activity(&task, &[comment(&task, 40, false, true)], &[])
        .unwrap();
    assert_attention(&store, &task.id, (true, true), "notification also asks");
    task.watched = false;
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_attention(
        &store,
        &task.id,
        (false, false),
        "unwatch removes both history reasons",
    );
    task.watched = true;
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_attention(
        &store,
        &task.id,
        (true, true),
        "watch restores existing request above both floors",
    );
    task.read_through = Some(format!("tc-{:026}", 40));
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    let mut created = event(&task, 50, TaskEventKind::Created, json!({}));
    created.mentions_user = true;
    store
        .save_tracker_task_activity(&task, &[], &[created])
        .unwrap();
    assert_attention(
        &store,
        &task.id,
        (true, false),
        "agent creation request needs no unread agent comment",
    );
}

#[test]
fn dismissal_ignores_bookkeeping_until_meaningful_workspace_activity() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/attention", "dismissal", Actor::User, support::NOW);
    draft.watched = true;
    let mut task = store.create_tracker_task(draft, &[]).unwrap();
    store
        .save_tracker_task_activity(&task, &[comment(&task, 10, true, false)], &[])
        .unwrap();
    task.dismissed_through = Some(format!("tc-{:026}", 10));
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_attention(
        &store,
        &task.id,
        (true, false),
        "dismissal suppresses legacy only",
    );
    for (n, kind, payload) in [
        (20, TaskEventKind::Blocked, json!({})),
        (21, TaskEventKind::Dispatched, json!({})),
        (
            22,
            TaskEventKind::Linked,
            json!({"waiting_on":"task-other"}),
        ),
        (23, TaskEventKind::Tracked, json!({"by":"assignment"})),
    ] {
        store
            .save_tracker_task_activity(&task, &[], &[event(&task, n, kind, payload)])
            .unwrap();
        assert_attention(
            &store,
            &task.id,
            (true, false),
            "bookkeeping cannot clear dismissal",
        );
    }
    store
        .save_tracker_task_activity(
            &task,
            &[],
            &[event(&task, 30, TaskEventKind::WorkspaceIdle, json!({}))],
        )
        .unwrap();
    assert_attention(
        &store,
        &task.id,
        (true, true),
        "workspace bookkeeping remains meaningful to inbox despite session exclusion",
    );
    task.dismissed_through = Some(format!("te-{:026}", 30));
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_attention(
        &store,
        &task.id,
        (true, false),
        "new dismissal also clears workspace activity",
    );
    store
        .save_tracker_task_activity(
            &task,
            &[],
            &[event(
                &task,
                31,
                TaskEventKind::Linked,
                json!({"workspace_id":"workspace-1"}),
            )],
        )
        .unwrap();
    assert_attention(
        &store,
        &task.id,
        (true, true),
        "workspace links are meaningful unlike waiting-on links",
    );
}

#[test]
fn user_blocking_review_and_finished_reasons_match_source() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let blocker = store
        .create_tracker_task(
            Task::drafted("/attention", "blocker", Actor::User, support::NOW),
            &[],
        )
        .unwrap();
    let mut draft = Task::drafted("/attention", "user", Actor::User, support::NOW);
    draft.assignee = Some(Assignee::User);
    let mut task = store.create_tracker_task(draft, &[]).unwrap();
    assert_attention(
        &store,
        &task.id,
        (true, true),
        "unwatched user assignment independently qualifies",
    );
    store
        .edit_waiting_on(
            "/attention",
            &task.id,
            std::slice::from_ref(&blocker.id),
            &[],
            &Actor::User,
            support::NOW,
        )
        .unwrap();
    task = store.load_tracker_task(&task.id).unwrap().unwrap();
    assert_attention(
        &store,
        &task.id,
        (false, false),
        "blocked user assignment has no user-ready reason",
    );
    store
        .edit_waiting_on(
            "/attention",
            &task.id,
            &[],
            std::slice::from_ref(&blocker.id),
            &Actor::User,
            support::NOW,
        )
        .unwrap();
    task = store.load_tracker_task(&task.id).unwrap().unwrap();
    assert_attention(
        &store,
        &task.id,
        (true, true),
        "dependency removal restores user-ready reason",
    );
    task.assignee = None;
    task.status = "in_review".into();
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_attention(
        &store,
        &task.id,
        (false, true),
        "legacy review reason is independent of user assignment",
    );
    task.status = "done".into();
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_attention(
        &store,
        &task.id,
        (false, false),
        "done excludes all reasons",
    );
    task = store.load_tracker_task(&task.id).unwrap().unwrap();
    task.status = "in_review".into();
    task.state = TaskState::Closed;
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_attention(
        &store,
        &task.id,
        (false, false),
        "closed excludes review and assignment reasons",
    );
}

#[test]
fn imported_unknown_event_notify_flag_does_not_create_a_canonical_ask() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/attention", "legacy event", Actor::User, support::NOW);
    draft.watched = true;
    let task = store.create_tracker_task(draft, &[]).unwrap();
    let created = event(&task, 10, TaskEventKind::Created, json!({}));
    store
        .save_tracker_task_activity(&task, &[], std::slice::from_ref(&created))
        .unwrap();
    // Explicit legacy import fixture: TaskEvent has no notifies_user field.
    // Unknown JSON extensions are ignored by canonical typed timeline reads;
    // the repair producer must retain that same semantic behavior.
    store.connection().execute("UPDATE tracker_events SET record=json_set(record,'$.notifies_user',json('true')) WHERE id=?1",[&created.id]).unwrap();
    store
        .repair_task_summaries(std::slice::from_ref(&task.id))
        .unwrap();
    assert_attention(
        &store,
        &task.id,
        (false, false),
        "unknown legacy event extensions are not canonical asks",
    );
}

#[test]
fn old_event_ask_projection_rebuilds_all_derived_layers_without_editing_canonical_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let (task_id, event_id, canonical_task, canonical_event, basis) = {
        let store = Store::new(dir.path()).unwrap();
        let mut draft = Task::drafted("/attention", "pre-fix event", Actor::User, support::NOW);
        draft.watched = true;
        let task = store.create_tracker_task(draft, &[]).unwrap();
        let created = event(&task, 10, TaskEventKind::Created, json!({}));
        store
            .save_tracker_task_activity(&task, &[], std::slice::from_ref(&created))
            .unwrap();
        let basis = store.task_query_basis().unwrap();
        let conn = store.connection();
        // Explicit imported extension ignored by canonical TaskEvent decoding.
        // Only derived layers below emulate the previous faulty producer.
        conn.execute("UPDATE tracker_events SET record=json_set(record,'$.notifies_user',json('true')) WHERE id=?1",[&created.id]).unwrap();
        conn.execute(
            "UPDATE tracker_task_summary_facts SET mentions=1 WHERE task_id=?1 AND id=?2",
            rusqlite::params![task.id, created.id],
        )
        .unwrap();
        conn.execute(
            "UPDATE tracker_task_summary_context SET unread_mentions=1 WHERE task_id=?1",
            [&task.id],
        )
        .unwrap();
        conn.execute("UPDATE tracker_task_summaries SET record=json_set(record,'$.unread_mentions',1) WHERE task_id=?1",[&task.id]).unwrap();
        conn.execute(
            "UPDATE tracker_task_query_facts SET attention_asked=1 WHERE task_id=?1",
            [&task.id],
        )
        .unwrap();
        conn.execute(
            "UPDATE meta SET value='pre-fix-event-mentions' WHERE key='task_summary_v1_definition'",
            [],
        )
        .unwrap();
        conn.execute("UPDATE meta SET value='pre-fix-event-mentions:' || (SELECT schema_version FROM pragma_schema_version) WHERE key='task_summary_v1_ready'",[]).unwrap();
        let canonical_task: String = conn
            .query_row(
                "SELECT record FROM tracker_tasks WHERE id=?1",
                [&task.id],
                |r| r.get(0),
            )
            .unwrap();
        let canonical_event: String = conn
            .query_row(
                "SELECT record FROM tracker_events WHERE id=?1",
                [&created.id],
                |r| r.get(0),
            )
            .unwrap();
        (task.id, created.id, canonical_task, canonical_event, basis)
    };
    let store = Store::new(dir.path()).unwrap();
    assert_attention(
        &store,
        &task_id,
        (false, false),
        "old producer derived rows must converge after fingerprint invalidation",
    );
    assert_ne!(
        store.task_query_basis().unwrap(),
        basis,
        "rebuilt facts advance the persisted projection generation"
    );
    let conn = store.connection();
    let layers:(i64,i64,i64,bool)=conn.query_row("SELECT f.mentions,c.unread_mentions,json_extract(s.record,'$.unread_mentions'),q.attention_asked FROM tracker_task_summary_facts f JOIN tracker_task_summary_context c USING(task_id) JOIN tracker_task_summaries s USING(task_id) JOIN tracker_task_query_facts q USING(task_id) WHERE f.task_id=?1 AND f.id=?2",rusqlite::params![task_id,event_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(layers, (0, 0, 0, false));
    let after_task: String = conn
        .query_row(
            "SELECT record FROM tracker_tasks WHERE id=?1",
            [&task_id],
            |r| r.get(0),
        )
        .unwrap();
    let after_event: String = conn
        .query_row(
            "SELECT record FROM tracker_events WHERE id=?1",
            [&event_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(after_task, canonical_task);
    assert_eq!(after_event, canonical_event);
}
