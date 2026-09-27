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
use crate::store::TaskFilter;
use crate::tracker::{Assignee, Task, TimelineEntry};
use serde_json::{json, Value};

impl AppState {
    /// Every watched task, as inbox rows.
    ///
    /// Rows for tasks only: the conversations beside them are the board's own
    /// and are built where they always were. They land in the same list so the
    /// interleave by `anchor` costs nothing on either side.
    pub(in crate::app) fn watched_task_rows(&mut self) -> Vec<Value> {
        let projects: Vec<String> = self
            .projects
            .iter()
            .map(|project| project.id.clone())
            .collect();
        let mut rows = Vec::new();
        for project_id in projects {
            let Ok(path) = self.tracker_project_path(&project_id) else {
                continue;
            };
            let Ok(tasks) = self.tracker_store().and_then(|store| {
                store
                    .list_tracker_tasks(&path, TaskFilter::default())
                    .map_err(|error| error.to_string())
            }) else {
                continue;
            };
            for task in tasks.into_iter().filter(|task| task.watched) {
                if let Some(row) = self.watched_task_row(&project_id, &task) {
                    rows.push(row);
                }
            }
        }
        rows
    }

    /// One row, or `None` for a task whose timeline could not be read —
    /// which is a row the inbox is better off without than wrong about.
    fn watched_task_row(&mut self, project_id: &str, task: &Task) -> Option<Value> {
        let timeline = self
            .tracker_store()
            .ok()?
            .load_tracker_timeline(&task.id)
            .ok()?;
        let last = timeline.last()?;
        let at = entry_at(last).to_string();
        let assigned_to_user = matches!(task.assignee, Some(Assignee::User));
        Some(json!({
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
            "unread": unread_since_mark(task, &timeline),
            // Mute is unwatch here, so a row that exists is not muted — the
            // absence of the row is the whole of the answer.
            "muted": false,
            "done_until_next": self.task_is_done_until_next(task, &timeline),
        }))
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
            .any(|entry| after(Some(cleared), entry_id(entry)))
    }
}

/// Events the user has not read: everything after the mark that is news,
/// minus their own.
///
/// Their own are excluded because a count that went up when the user
/// commented would be telling them about themselves — and the inbox badge is
/// a list of things asking for their attention. The inbox row says it as
/// `unread`, and `tasks.list` as each watched task's `unread_count` (#104).
pub(in crate::app) fn unread_since_mark(task: &Task, timeline: &[TimelineEntry]) -> usize {
    timeline
        .iter()
        .filter(|entry| after(task.read_through.as_deref(), entry_id(entry)))
        .filter(|entry| !matches!(entry_actor(entry), crate::tracker::Actor::User))
        .filter(|entry| counts_as_unread(entry))
        .count()
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
    match event.kind {
        Kind::Assigned | Kind::Unassigned | Kind::Moved | Kind::Closed | Kind::Reopened => true,
        Kind::Created => event.mentions_user && matches!(&event.actor, Actor::Agent { .. }),
        Kind::Labelled
        | Kind::Linked
        | Kind::Dispatched
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

fn entry_id(entry: &TimelineEntry) -> &str {
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
