//! Pure task history summaries, shared by list projections and inbox reads.

use super::{Task, TimelineEntry};
use crate::session_summary::{message_millis, SessionSummary};
use serde_json::Value;

/// The task's visible activity streak, shared by task reads and inbox rows.
/// Silent edits retain their whole activity interval in the task record;
/// later timeline entries extend it. A task predating that checkpoint
/// replays its meaningful timeline, so reads and restarted bridges agree.
pub(crate) fn task_activity_session(task: &Task, timeline: &[TimelineEntry]) -> SessionSummary {
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
pub(crate) fn attention_bookkeeping(entry: &TimelineEntry) -> bool {
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
pub(crate) fn unread_since_mark(task: &Task, timeline: &[TimelineEntry]) -> usize {
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
pub(crate) fn counts_as_unread(entry: &TimelineEntry) -> bool {
    match entry {
        TimelineEntry::Comment(_) => true,
        TimelineEntry::Event(event) => event_counts_as_unread(event),
    }
}

/// The event half of [`counts_as_unread`]: whether one event is news. Browser
/// push asks the same question of the events a write carries (#191).
pub(crate) fn event_counts_as_unread(event: &crate::tracker::TaskEvent) -> bool {
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
pub(crate) fn after(mark: Option<&str>, id: &str) -> bool {
    mark.is_none_or(|mark| when(id) > when(mark))
}

/// One id without its kind: `tc-01K5Z…` and `te-01K5Z…` are comparable, and
/// with the prefix on they are not.
pub(crate) fn when(id: &str) -> &str {
    id.split_once('-').map_or(id, |(_, ulid)| ulid)
}

pub(crate) fn entry_id(entry: &TimelineEntry) -> &str {
    match entry {
        TimelineEntry::Comment(comment) => &comment.id,
        TimelineEntry::Event(event) => &event.id,
    }
}

pub(crate) fn entry_at(entry: &TimelineEntry) -> &str {
    match entry {
        TimelineEntry::Comment(comment) => &comment.created_at,
        TimelineEntry::Event(event) => &event.at,
    }
}

pub(crate) fn entry_actor(entry: &TimelineEntry) -> &crate::tracker::Actor {
    match entry {
        TimelineEntry::Comment(comment) => &comment.author,
        TimelineEntry::Event(event) => &event.actor,
    }
}
