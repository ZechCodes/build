//! The tasks family: the per-project task tracker (spec: Tasks).
//!
//! NOT the `task.*` verbs in [`lifecycle`](super::lifecycle). Those are the
//! retired plan-and-stages flow, which shares the English word and nothing
//! else. The two families are spelled apart — singular `task.*` there, plural
//! `tasks.*` here — so no name can be registered by both, and the test at the
//! bottom of `api/v1/mod.rs` proves it.
//!
//! Same shape as the workspace family: the implementations under
//! `app/tracker/` are untouched, each handler resolves its typed params, hands
//! them to the implementation, and names the shape that implementation answers
//! in. Nothing here defers — a tracker verb is a SQLite write and an in-memory
//! lookup, with no git to hand to the drain.
//!
//! On the nullable fields: a task writes its absences as explicit `null`s
//! (`assignee`, `closed_at`, `links.parent_task_id`) rather than leaving the
//! key out, so those optionals carry no `skip_serializing_if`. A client reads
//! `assignee === null` as unassigned, and a missing key would read as a bridge
//! too old to answer it.

use super::{answer, Answer, Handler, WireParams};
use crate::api::ApiError;
use crate::app::AppState;
use crate::{v1_method, v1_methods};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The verbs this family serves.
pub fn methods() -> &'static [(&'static str, Handler)] {
    v1_methods![
        v1_method!("tasks.list", tasks_list, TasksListParams, TasksList),
        v1_method!("tasks.get", tasks_get, TaskIdParams, TaskDetail),
        v1_method!(
            "tasks.create",
            tasks_create,
            TasksCreateParams,
            TaskAssigned
        ),
        v1_method!("tasks.update", tasks_update, TasksUpdateParams, TaskAnswer),
        v1_method!(
            "tasks.comment",
            tasks_comment,
            TasksCommentParams,
            TaskComment
        ),
        v1_method!(
            "tasks.assign",
            tasks_assign,
            TasksAssignParams,
            TaskAssigned
        ),
        v1_method!("tasks.close", tasks_close, TasksCloseParams, TaskAnswer),
        v1_method!("tasks.reopen", tasks_reopen, TaskIdParams, TaskAnswer),
        v1_method!("tasks.watch", tasks_watch, TaskIdParams, TaskAnswer),
        v1_method!("tasks.unwatch", tasks_unwatch, TaskIdParams, TaskAnswer),
        v1_method!(
            "tasks.read_through",
            tasks_read_through,
            TasksReadThroughParams,
            TaskAnswer
        ),
        v1_method!("tasks.columns", tasks_columns, ProjectIdParams, TaskColumns),
        v1_method!(
            "tasks.attach",
            tasks_attach,
            TasksAttachParams,
            TaskAttachment
        ),
        v1_method!(
            "tasks.attachment",
            tasks_attachment,
            TasksAttachmentParams,
            TaskAttachmentBytes
        ),
    ]
}

// ---------------------------------------------------------------- params ---

#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectIdParams {
    pub project_id: String,
}

/// A task named and nothing else asked of it: `tasks.get`,
/// `tasks.reopen`.
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskIdParams {
    #[serde(deserialize_with = "crate::renamed_ids::current")]
    pub task_id: String,
}

/// One project's tasks, narrowed. Every filter is optional and they are ANDed.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct TasksListParams {
    pub project_id: String,
    /// `open` or `closed`; both when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// A column slug or its display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// An assignee object, or the words `"none"` (unassigned) and `"any"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<Value>,
    /// One label, matched the way labels are deduped: case-insensitively.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// At most this many tasks, 1 to 500 (1.25.0, `tasks.listPaged`). Absent
    /// is the whole list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// Where the page starts: a `next_cursor` an earlier page of the SAME
    /// filter answered (1.25.0). Absent is the top of the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// One file, filed with a task and BEFORE it.
///
/// Separate from the create for the reason `thread.attach` is separate from the
/// post: the bytes are on disk and verified before the record that names them
/// exists, so a task can never point at an upload that failed halfway.
///
/// A project and not a conversation: a task being created has neither, and
/// one filed unassigned never gets one.
#[derive(Debug, Deserialize, Serialize)]
pub struct TasksAttachParams {
    pub project_id: String,
    pub filename: String,
    pub content_b64: String,
}

/// What a task's attachment reads back as. Addressed through the TASK, so
/// no caller has to know (or can get wrong) where the file landed.
#[derive(Debug, Deserialize, Serialize)]
pub struct TasksAttachmentParams {
    #[serde(deserialize_with = "crate::renamed_ids::current")]
    pub task_id: String,
    pub path: String,
    /// Where the piece starts (1.19). Absent is the start of the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    /// How many bytes to read from there, at most one piece (1.19). Absent is
    /// one piece.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TasksCreateParams {
    pub project_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// The column it starts in; `backlog` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    /// Who to hand it to. The same five kinds `tasks.assign` takes, and the
    /// same consequence: filing a task for somebody starts them on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<Value>,
    /// The files filed with it, each naming a path `tasks.attach` answered.
    /// The same shape a message carries, because they are the same object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<TaskAttachmentRef>>,
    /// Extra instruction delivered under the task, when it is assigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Only the fields present are applied. An absent field is not "set this to
/// nothing" — there is no verb here that empties a title.
#[derive(Debug, Deserialize, Serialize)]
pub struct TasksUpdateParams {
    #[serde(deserialize_with = "crate::renamed_ids::current")]
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TasksCommentParams {
    #[serde(deserialize_with = "crate::renamed_ids::current")]
    pub task_id: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<crate::tracker::ReviewCommentAnchorInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opinion: Option<crate::tracker::ReviewOpinionInput>,
    /// Typed references, fenced by shape and then by what this task is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs: Option<Vec<crate::thread::ThreadLink>>,
    /// The files said with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<TaskAttachmentRef>>,
}

/// One attachment as a REQUEST names it: the path it was stored at, and
/// optionally what to call it. Name, mime and size are re-read from disk when
/// the record is written — the client's copy is a display hint.
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskAttachmentRef {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Hand a task to somebody, and start them on it.
///
/// One tagged `assignee` covering all five kinds — `user`, `project_agent`,
/// `agent`, `new_workspace`, `new_agent` — because assignment IS dispatch and a
/// second field beside it would be a second place for the same decision. `null`
/// unassigns. Untyped here for the reason a timeline entry is: the five arms
/// carry different fields, and `AssignTarget::parse` is the one place that
/// reads them, naming each refusal.
#[derive(Debug, Deserialize, Serialize)]
pub struct TasksAssignParams {
    #[serde(deserialize_with = "crate::renamed_ids::current")]
    pub task_id: String,
    pub assignee: Value,
    /// Extra instruction delivered under the task. Not stored on the task:
    /// the body is the task, and a hand-off note belongs in the conversation
    /// it was said in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// How far the user has read one task.
#[derive(Debug, Deserialize, Serialize)]
pub struct TasksReadThroughParams {
    #[serde(deserialize_with = "crate::renamed_ids::current")]
    pub task_id: String,
    /// The last event the user has seen. Never moved backwards.
    #[serde(deserialize_with = "crate::renamed_ids::current")]
    pub event_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TasksCloseParams {
    #[serde(deserialize_with = "crate::renamed_ids::current")]
    pub task_id: String,
    /// One line on why, kept on the `closed` event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// --------------------------------------------------------------- results ---

/// What a task is about, in the repository and in Build.
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskLinks {
    pub workspace_ids: Vec<String>,
    pub branches: Vec<String>,
    pub commits: Vec<String>,
    pub conversation_ids: Vec<String>,
    /// `null` for a task under nothing.
    pub parent_task_id: Option<String>,
}

/// One task, as every tracker verb answers it.
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskView {
    pub id: String,
    pub project_id: String,
    /// Per-project and sequential, for `#12`.
    pub number: u64,
    pub title: String,
    pub body: String,
    /// `open` or `closed`.
    pub state: String,
    /// The column slug — one of `tasks.columns`' ids.
    pub status: String,
    pub labels: Vec<String>,
    /// `none`, `low`, `medium`, `high` or `urgent`.
    pub priority: String,
    /// `null` when nobody holds it.
    pub assignee: Option<Value>,
    pub links: TaskLinks,
    /// The agents watching this task. Always present; `[]` for a task
    /// nobody watches, so a client tells "nobody" from "this bridge is too old
    /// to answer it".
    pub trackers: Vec<String>,
    /// Last known identities of agents this task mentions, keyed by agent id.
    /// Captured on writes so a finished workspace still names its agents.
    #[serde(default)]
    pub identities: std::collections::BTreeMap<String, crate::tracker::TaskAgentIdentity>,
    /// The files filed with it. Always present for the same reason `trackers`
    /// is: a client that sent files and got no key back is looking at a bridge
    /// that dropped them, and `[]` says it carried none.
    #[serde(default)]
    pub attachments: Vec<TaskAttachment>,
    /// Whether the USER is watching this task — the inbox's flag, not
    /// `trackers`, which is the agents'. Absent means no, so a task nobody
    /// watches says nothing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub watched: bool,
    /// The last event on this task the user has read. `null` when they have
    /// read none of it, which is how a task arrives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_through: Option<String>,
    /// The last event the user put down without reading — Done until the next
    /// one. `null` unless they have dismissed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed_through: Option<String>,
    /// How many timeline entries after `read_through` are not the user's own
    /// (1.29.0). `tasks.list` and `tasks.get` carry it on a watched task:
    /// a task nobody watches never shows a count, so it carries none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread_count: Option<u64>,
    /// `{"kind":"user"}` or `{"kind":"agent","agent_id":…}`.
    pub created_by: Value,
    pub created_at: String,
    pub updated_at: String,
    /// `null` while the task is open.
    pub closed_at: Option<String>,
    /// When it last moved into Done, while it is there. Absent anywhere else,
    /// and from a bridge that does not announce `tasks.doneSinceLeft`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_at: Option<String>,
}

/// One attachment as every ANSWER carries it — what `tasks.attach` says it
/// stored, and what a task or a comment says it holds.
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskAttachment {
    pub name: String,
    /// Where the bytes are. Opaque to a browser, which reads them back with
    /// `tasks.attachment`; an agent opens it.
    pub path: String,
    pub mime: String,
    pub size: u64,
}

/// One attachment's bytes, for a surface that cannot reach the disk — or one
/// piece of them (1.19): `size` is always the whole file's, and `offset` says
/// where this piece starts.
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskAttachmentBytes {
    pub path: String,
    pub size: u64,
    pub mime: String,
    #[serde(default)]
    pub offset: u64,
    pub content_b64: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TasksList {
    pub project_id: String,
    pub tasks: Vec<TaskView>,
    /// The user's session on this bridge, device-wide rather than this
    /// project's: what the dashboard's "Done since you left" measures from.
    pub user_session: UserSessionView,
    /// Where the next page starts, handed back as `cursor` with the same
    /// filter (1.25.0). Present only on a page with more rows after it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// The user's session: epoch milliseconds, each `null` until there is one.
/// Only the user's own actions move it; an agent never does.
#[derive(Debug, Deserialize, Serialize)]
pub struct UserSessionView {
    /// The first action after the last silence of `gap_ms` or more.
    pub session_started_ms: Option<i64>,
    pub last_activity_ms: Option<i64>,
    /// The last action before that silence: where "since you left" starts.
    pub previous_session_ended_ms: Option<i64>,
    /// The silence that ends a session.
    pub gap_ms: i64,
    /// This bridge's clock when it answered. A client measures silences
    /// against this, plus the time since it arrived, and never against its
    /// own clock.
    pub now_ms: i64,
}

/// What every mutating verb answers: the task as it now stands.
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskAnswer {
    pub task: TaskView,
}

/// One comment on one task.
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskCommentView {
    pub id: String,
    pub task_id: String,
    pub author: Value,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<crate::tracker::ReviewCommentAnchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opinion: Option<crate::tracker::ReviewOpinion>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub mentions_user: bool,
    /// An agent's comment written with `notify_user` (#144). Absent otherwise.
    #[serde(default, skip_serializing_if = "is_false")]
    pub notifies_user: bool,
    pub refs: Vec<crate::thread::ThreadLink>,
    #[serde(default)]
    pub attachments: Vec<TaskAttachment>,
    pub created_at: String,
    /// How full the authoring agent's context was as it wrote (#68). Absent
    /// on the user's comments and wherever the agent had no reading.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_context: Option<crate::thread::ContextReading>,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TaskComment {
    pub task: TaskView,
    pub comment: TaskCommentView,
}

/// Where an assignment put the work, or `null` when it dispatched nothing —
/// which is `{"kind":"user"}` and unassignment.
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskDispatch {
    /// Which of the five kinds was asked for.
    pub kind: String,
    /// The workspace the work runs in, when the dispatch names or makes one.
    pub workspace_id: Option<String>,
    /// The conversation owner the task was delivered into.
    pub entity_id: String,
    /// The agent now holding the task.
    pub agent_id: String,
    /// The receipt for the turn the delivery queued.
    pub operation_id: Value,
}

/// What `tasks.assign` answers: the task, and where the work went.
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskAssigned {
    pub task: TaskView,
    pub dispatch: Option<TaskDispatch>,
}

/// One timeline entry: the record itself, with `type` naming which it is.
///
/// Untyped past the discriminator on purpose. A comment and an event have
/// different fields and an event's `payload` is open by design — it says what
/// its own kind needs — so pinning the union here would mean one struct per
/// event kind and a fixture per struct, to describe something whose whole point
/// is that a client reads `type` and then reads the record.
pub type TaskTimelineEntry = Value;

#[derive(Debug, Deserialize, Serialize)]
pub struct TaskDetail {
    pub task: TaskView,
    pub timeline: Vec<TaskTimelineEntry>,
}

/// One kanban column: the slug that is stored, the name that is shown.
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskColumn {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TaskColumns {
    pub project_id: String,
    pub columns: Vec<TaskColumn>,
}

// ----------------------------------------------------------------- codes ---

/// The request was legible and the task's own state said no.
const CONFLICT: [&str; 7] = [
    "already has the most trackers it can carry",
    "is already closed",
    "is already open",
    "a parent link cannot close a loop",
    // A dispatch that cuts a checkout waits for the filesystem, the way every
    // other checkout Build makes does.
    "another filesystem operation is still running",
    "is not in project",
    "workspace must finish provisioning successfully",
];

/// A word in the request is not one this bridge knows, or a value is out of
/// shape. `unknown <thing>` otherwise reads as a missing entity, and half of
/// these are about a word rather than a thing.
const INVALID: [&str; 22] = [
    "unknown assignee kind:",
    "assignee: name a kind",
    "assignee new_agent: name a",
    "unknown status:",
    "unknown state:",
    "unknown priority:",
    "unknown assignee filter:",
    "tasks.link: name a",
    "cannot be empty",
    "exceeds",
    "must be a string",
    "must be an array",
    "labels must be strings",
    "a task cannot be its own parent",
    "a task carries at most",
    // Attachments: a file too big, an empty one, and a list that is not one.
    "attachments must be an array",
    "each attachment needs a path",
    "attachment is empty",
    // A page of `tasks.list`: a limit out of bounds, a cursor that is not
    // one, and a cursor made for another filter.
    "a page holds 1 to",
    "Build cannot read this cursor",
    "the cursor was made for a different filter",
    "the cursor was made for another project or on another device",
];

/// The store is not there, or a reference did not survive its fencing. Neither
/// is the caller's fault in a way retrying would fix, and neither is a missing
/// entity.
const UNAVAILABLE: [&str; 1] = ["tasks need a durable store"];

/// Name the refusal this family raised, where the sentence says what
/// [`ApiError::classify`] cannot read from its general rules. Anything
/// unmatched keeps the code classify gave it; the message is never rewritten.
///
/// [`ApiError::classify`]: crate::api::ApiError::classify
fn refine(error: ApiError) -> ApiError {
    let message = error.message().to_string();
    if UNAVAILABLE.iter().any(|marker| message.contains(marker)) {
        return ApiError::unavailable(message);
    }
    if CONFLICT.iter().any(|marker| message.contains(marker)) {
        return ApiError::conflict(message, None);
    }
    if INVALID.iter().any(|marker| message.contains(marker)) {
        return ApiError::invalid_params(message);
    }
    error
}

// -------------------------------------------------------------- handlers ---

fn tasks_list(app: &mut AppState, params: TasksListParams) -> Result<Answer<TasksList>, ApiError> {
    answer(app.tasks_list(&params.wire())).map_err(refine)
}

fn tasks_get(app: &mut AppState, params: TaskIdParams) -> Result<Answer<TaskDetail>, ApiError> {
    answer(app.tasks_get(&params.wire())).map_err(refine)
}

fn tasks_create(
    app: &mut AppState,
    params: TasksCreateParams,
) -> Result<Answer<TaskAssigned>, ApiError> {
    answer(
        app.tasks_create(&params.wire())
            .map(super::deferral_placeholder),
    )
    .map_err(refine)
}

fn tasks_attach(
    app: &mut AppState,
    params: TasksAttachParams,
) -> Result<Answer<TaskAttachment>, ApiError> {
    answer(app.tasks_attach(&params.wire())).map_err(refine)
}

fn tasks_attachment(
    app: &mut AppState,
    params: TasksAttachmentParams,
) -> Result<Answer<TaskAttachmentBytes>, ApiError> {
    answer(app.tasks_attachment(&params.wire())).map_err(refine)
}

fn tasks_update(
    app: &mut AppState,
    params: TasksUpdateParams,
) -> Result<Answer<TaskAnswer>, ApiError> {
    answer(app.tasks_update(&params.wire())).map_err(refine)
}

fn tasks_comment(
    app: &mut AppState,
    params: TasksCommentParams,
) -> Result<Answer<TaskComment>, ApiError> {
    answer(app.tasks_comment(&params.wire())).map_err(refine)
}

fn tasks_assign(
    app: &mut AppState,
    params: TasksAssignParams,
) -> Result<Answer<TaskAssigned>, ApiError> {
    answer(
        app.tasks_assign(&params.wire())
            .map(super::deferral_placeholder),
    )
    .map_err(refine)
}

fn tasks_close(
    app: &mut AppState,
    params: TasksCloseParams,
) -> Result<Answer<TaskAnswer>, ApiError> {
    answer(app.tasks_close(&params.wire())).map_err(refine)
}

fn tasks_reopen(app: &mut AppState, params: TaskIdParams) -> Result<Answer<TaskAnswer>, ApiError> {
    answer(app.tasks_reopen(&params.wire())).map_err(refine)
}

fn tasks_watch(app: &mut AppState, params: TaskIdParams) -> Result<Answer<TaskAnswer>, ApiError> {
    answer(app.tasks_watch(&params.wire())).map_err(refine)
}

fn tasks_unwatch(app: &mut AppState, params: TaskIdParams) -> Result<Answer<TaskAnswer>, ApiError> {
    answer(app.tasks_unwatch(&params.wire())).map_err(refine)
}

fn tasks_read_through(
    app: &mut AppState,
    params: TasksReadThroughParams,
) -> Result<Answer<TaskAnswer>, ApiError> {
    answer(app.tasks_read_through(&params.wire())).map_err(refine)
}

fn tasks_columns(
    app: &mut AppState,
    params: ProjectIdParams,
) -> Result<Answer<TaskColumns>, ApiError> {
    answer(app.tasks_columns(&params.wire())).map_err(refine)
}

#[cfg(test)]
mod tests {
    use super::super::testing::fixture_round_trips;
    use super::*;

    #[test]
    fn every_fixture_holds_to_the_types_this_family_declares() {
        for (method, _) in methods() {
            fixture_round_trips(methods(), method);
        }
    }

    /// The tracker and the retired plan flow are spelled apart, so no name can
    /// be served by both however either grows.
    #[test]
    fn no_tracker_verb_can_be_read_as_a_plan_verb() {
        for (method, _) in methods() {
            assert!(method.starts_with("tasks."), "{method}");
            assert!(!method.starts_with("task."), "{method}");
        }
    }
}
