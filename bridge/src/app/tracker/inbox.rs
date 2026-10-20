//! Watched tasks as inbox rows (spec: Tasks → Watching).
//!
//! The user watches a task and it appears in the inbox beside the
//! conversations, ordered by when it last moved. One row per watched task,
//! carrying what the row draws and nothing else: the task itself is a click
//! away, and a row that carried the body would be the inbox re-rendering the
//! task page in miniature.
//!
//! The row's subtitle is the reader-facing voice of the same line an agent
//! gets as a notice ([`super::notices::notice_line`]). One composer, two
//! audiences: the facts cannot drift apart because there is one place that
//! decides them.

use super::notices::{notice_line, NoticeVoice};
use crate::app::AppState;
use crate::session_summary::{message_millis, SessionSummary};
use crate::store::TrackerTaskObservation;
use crate::tracker::{Assignee, Task, TimelineEntry};
use serde_json::{json, Value};

impl AppState {
    /// Every watched task, as inbox rows.
    ///
    /// Rows for tasks only: the conversations beside them are the board's own
    /// and are built where they always were. They land in the same list so the
    /// interleave by `anchor` costs nothing on either side.
    #[cfg(test)]
    pub(in crate::app) fn watched_task_rows(&self) -> Vec<Value> {
        let projects: Vec<String> = self
            .projects
            .iter()
            .map(|project| project.id.clone())
            .collect();
        projects
            .into_iter()
            .flat_map(|project| {
                self.watched_task_rows_for_project(&project)
                    .expect("watched task fixture is readable")
            })
            .collect()
    }

    pub(in crate::app) fn watched_task_rows_for_project(
        &self,
        project_id: &str,
    ) -> Result<Vec<Value>, String> {
        let Some(store) = &self.store else {
            return Ok(Vec::new());
        };
        let path = self.tracker_project_path(project_id)?;
        let observations = store
            .watched_tracker_task_observations(&path)
            .map_err(|error| error.to_string())?;
        Ok(observations
            .iter()
            .filter_map(|observation| self.watched_task_observation_row(project_id, observation))
            .collect())
    }

    /// Pure row construction from one committed transaction. An absent row
    /// means unwatching or no meaningful activity, never a failed SQL read.
    pub(super) fn watched_task_observation_row(
        &self,
        project_id: &str,
        observation: &TrackerTaskObservation,
    ) -> Option<Value> {
        let task = &observation.task;
        if !task.watched {
            return None;
        }
        let timeline = &observation.timeline;
        let last = timeline
            .iter()
            .rev()
            .find(|entry| !attention_bookkeeping(entry))?;
        let at = entry_at(last).to_string();
        let session = task_activity_session(task, timeline);
        let blocked = observation.dependencies.blocked;
        let assigned_to_user = matches!(task.assignee, Some(Assignee::User)) && !blocked;
        let mut row = json!({
            "kind": "tracker_task",
            "task_id": task.id,
            "number": task.number,
            "project_id": project_id,
            "title": task.title,
            "status": task.status,
            "assignee": task.assignee,
            "assigned_to_user": assigned_to_user,
            "last_event": {
                "text": self.reader_line_for(task, last),
                "actor": self.entry_actor_words(last).to_reader,
                "at": at,
            },
            // The two the inbox interleaves by. Both the same instant: a row's
            // anchor IS when it last moved, and the existing rows send both.
            "anchor": at,
            "last_activity": at,
            "session_started_ms": session.session_started_ms,
            "last_activity_ms": session.last_activity_ms,
            "unread": unread_since_mark(task, timeline),
            // Mute is unwatch here, so a row that exists is not muted — the
            // absence of the row is the whole of the answer.
            "muted": false,
            "done_until_next": self.task_is_done_until_next(task, timeline),
        });
        if let Some(summary) = &observation.review {
            row["review_summary"] = json!(summary);
        }
        extend_observed_waiting_summary(observation, &mut row);
        Some(row)
    }

    /// What the row's subtitle says: the reader's voice of the same line an
    /// agent would be sent about this event.
    fn reader_line_for(&self, task: &Task, entry: &TimelineEntry) -> String {
        let Some(notice) = super::notices::notice_of_entry(entry) else {
            // A timeline entry the notice vocabulary has no word for — a
            // tracking change, today. The row still needs a subtitle, and
            // when it last moved is the honest one.
            return "Updated".to_string();
        };
        notice_line(
            &notice,
            task,
            &self.entry_actor_words(entry),
            NoticeVoice::Reader,
        )
    }

    fn entry_actor_words(&self, entry: &TimelineEntry) -> super::notices::ActorWords {
        self.actor_words(entry_actor(entry))
    }

    /// Whether the user cleared this row and nothing has happened since.
    fn task_is_done_until_next(&self, task: &Task, timeline: &[TimelineEntry]) -> bool {
        let Some(cleared) = task.dismissed_through.as_deref() else {
            return false;
        };
        !timeline
            .iter()
            .filter(|entry| !attention_bookkeeping(entry))
            .any(|entry| after(Some(cleared), entry_id(entry)))
    }
}

pub(super) fn extend_observed_waiting_summary(
    observation: &TrackerTaskObservation,
    row: &mut Value,
) {
    row["dependencies"] = json!(observation.dependencies);
    row["dependencies"]["blocks"] = json!(observation.blocks.blocks);
    if let Some(cursor) = &observation.blocks.next_cursor {
        row["dependencies"]["blocks_next_cursor"] = json!(cursor);
    }
    if let Some(intent) = observation.delivery.as_ref().filter(|intent| {
        matches!(
            intent.state.as_str(),
            "waiting" | "queued" | "provisioning" | "failed"
        )
    }) {
        let state = match intent.state.as_str() {
            "provisioning" => "queued",
            state => state,
        };
        row["deferred_assignment"] =
            json!({"id":intent.id,"target":intent.target,"state":state,"error":intent.error});
    }
}

/// The task's visible activity streak, shared by task reads and inbox rows.
/// Silent edits retain their whole activity interval in the task record;
/// later timeline entries extend it. A task predating that checkpoint
/// replays its meaningful timeline, so reads and restarted bridges agree.
pub(super) fn task_activity_session(task: &Task, timeline: &[TimelineEntry]) -> SessionSummary {
    let saved = task.activity_session.unwrap_or_default();
    let replay_after = saved.last_activity_ms.unwrap_or(i64::MIN);
    let mut times: Vec<i64> = timeline
        .iter()
        .filter(|entry| !attention_bookkeeping(entry))
        .filter(|entry| !workspace_bookkeeping(entry))
        .filter_map(|entry| message_millis(entry_at(entry)))
        .filter(|ts| *ts > replay_after)
        .collect();
    times.sort_unstable();
    times.into_iter().fold(saved, SessionSummary::updated)
}

/// Reclaim facts describe a linked workspace, not activity on the task.
/// `record_quiet_event` preserves the task record for the same reason.
fn workspace_bookkeeping(entry: &TimelineEntry) -> bool {
    use crate::tracker::TaskEventKind as Kind;
    matches!(entry, TimelineEntry::Event(event) if matches!(event.kind,
        Kind::WorkspaceIdle | Kind::WorkspacePruned | Kind::WorkspaceReclaimed
        | Kind::BranchDeleted | Kind::BranchKept))
}

/// Whether an entry is Build ending tracking because the task finished
/// (#444). Bookkeeping the inbox does not see: it lands beside the move that
/// finished the task, which is what the row should say, or — from the boot
/// pass — on its own, where it must not bring back a row the user cleared or
/// move it up the list.
fn attention_bookkeeping(entry: &TimelineEntry) -> bool {
    let TimelineEntry::Event(event) = entry else {
        return false;
    };
    use crate::tracker::TaskEventKind as Kind;
    match event.kind {
        Kind::Blocked
        | Kind::Unblocked
        | Kind::DispatchDeferred
        | Kind::DispatchFailed
        | Kind::Dispatched => true,
        Kind::Linked | Kind::Unlinked => event.payload.get("waiting_on").is_some(),
        Kind::Tracked => event.payload["by"] == "assignment",
        Kind::Moved => matches!(event.payload["by"].as_str(), Some("dispatch" | "report")),
        _ => ended_tracking(entry),
    }
}

fn ended_tracking(entry: &TimelineEntry) -> bool {
    let TimelineEntry::Event(event) = entry else {
        return false;
    };
    event.kind == crate::tracker::TaskEventKind::Untracked
        && event.payload.get("by").and_then(Value::as_str) == Some(crate::tracker::FINISHED_UNTRACK)
}

/// Events the user has not read: everything after the mark that is news,
/// minus their own.
///
/// Their own are excluded because a count that went up when the user
/// commented would be telling them about themselves — and the inbox badge is
/// a list of things asking for their attention. The inbox row says it as
/// `unread`, and `tasks.list` as each watched task's `unread_count` (#104).
pub(in crate::app) fn unread_since_mark(task: &Task, timeline: &[TimelineEntry]) -> usize {
    if !task.watched {
        return 0;
    }
    let baseline = watch_started_after(task, timeline);
    timeline
        .iter()
        .filter(|entry| after(task.read_through.as_deref(), entry_id(entry)))
        .filter(|entry| after(baseline, entry_id(entry)))
        .filter(|entry| !matches!(entry_actor(entry), crate::tracker::Actor::User))
        .filter(|entry| counts_as_unread(entry))
        .count()
}

/// Records saved before the watch cursor existed still carry the watch's
/// timeline event. A task watched from creation has neither and starts at the
/// origin; an automatic watch's stored cursor can precede its asking comment.
fn watch_started_after<'a>(task: &'a Task, timeline: &'a [TimelineEntry]) -> Option<&'a str> {
    task.watch_started_after.as_deref().or_else(|| {
        timeline
            .iter()
            .filter_map(|entry| match entry {
                TimelineEntry::Event(event)
                    if event.kind == crate::tracker::TaskEventKind::Watched =>
                {
                    Some(event.id.as_str())
                }
                _ => None,
            })
            .max_by_key(|id| when(id))
    })
}

/// Whether a timeline entry is news to the user (#183): something said, a
/// change to who holds the task or where it stands, or an agent-created
/// task that asks the user to read it. Other bookkeeping — filing without an
/// ask, tracking, linking, labelling, dispatching, watching, and what Build
/// records about branches and workspaces — is not. The SPA's
/// fallback count reads the same list (`spa/src/core/trackerUnread.js`), and
/// `app::tests::tracker_unread_kinds` prints the cases both sides are held to.
pub(in crate::app) fn counts_as_unread(entry: &TimelineEntry) -> bool {
    match entry {
        TimelineEntry::Comment(_) => true,
        TimelineEntry::Event(event) => event_counts_as_unread(event),
    }
}

/// The event half of [`counts_as_unread`]: whether one event is news. Browser
/// push asks the same question of the events a write carries (#191).
pub(in crate::app) fn event_counts_as_unread(event: &crate::tracker::TaskEvent) -> bool {
    use crate::tracker::{Actor, TaskEventKind as Kind};
    if event.kind == Kind::Moved
        && matches!(event.payload["by"].as_str(), Some("dispatch" | "report"))
    {
        return false;
    }
    match event.kind {
        Kind::Assigned
        | Kind::Unassigned
        | Kind::Moved
        | Kind::Closed
        | Kind::Reopened
        | Kind::ReviewCompleted => true,
        Kind::Created => event.mentions_user && matches!(&event.actor, Actor::Agent { .. }),
        Kind::Labelled
        | Kind::Linked
        | Kind::Unlinked
        | Kind::Blocked
        | Kind::Unblocked
        | Kind::Dispatched
        | Kind::DispatchDeferred
        | Kind::DispatchFailed
        | Kind::Tracked
        | Kind::Untracked
        | Kind::Watched
        | Kind::Unwatched
        | Kind::BranchDeleted
        | Kind::BranchKept
        | Kind::WorkspaceIdle
        | Kind::WorkspacePruned
        | Kind::WorkspaceReclaimed => false,
    }
}

/// Whether `id` is newer than a mark.
///
/// Compared without the `tc-`/`te-` in front: the rest is a ULID and so is
/// time-ordered, but the prefixes are not — every comment id sorts below
/// every event id, and a mark left on an event would hide every comment made
/// after it.
pub(in crate::app) fn after(mark: Option<&str>, id: &str) -> bool {
    mark.is_none_or(|mark| when(id) > when(mark))
}

/// One id without its kind: `tc-01K5Z…` and `te-01K5Z…` are comparable, and
/// with the prefix on they are not.
pub(in crate::app) fn when(id: &str) -> &str {
    id.split_once('-').map_or(id, |(_, ulid)| ulid)
}

pub(super) fn entry_id(entry: &TimelineEntry) -> &str {
    match entry {
        TimelineEntry::Comment(comment) => &comment.id,
        TimelineEntry::Event(event) => &event.id,
    }
}

fn entry_at(entry: &TimelineEntry) -> &str {
    match entry {
        TimelineEntry::Comment(comment) => &comment.created_at,
        TimelineEntry::Event(event) => &event.at,
    }
}

fn entry_actor(entry: &TimelineEntry) -> &crate::tracker::Actor {
    match entry {
        TimelineEntry::Comment(comment) => &comment.author,
        TimelineEntry::Event(event) => &event.actor,
    }
}
