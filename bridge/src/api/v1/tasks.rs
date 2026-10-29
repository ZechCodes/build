//! The tasks family: the per-project task tracker (spec: Tasks).
//!
//! NOT the `task.*` verbs in [`lifecycle`](super::lifecycle). Those are the
//! retired plan-and-stages flow, which shares the English word and nothing
//! else. The two families are spelled apart — singular `task.*` there, plural
//! `tasks.*` here — so no name can be registered by both, and the test at the
//! bottom of `api/v1/mod.rs` proves it.
//!
//! Each handler resolves typed params, invokes the tracker service and names
//! its answer shape. Assignment records durable delivery authority before
//! dispatch; checkout work uses the ordinary deferred Git drain.
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
        v1_method!("tasks.counts", tasks_counts, TasksCountsParams, TasksCounts),
        v1_method!("tasks.lookup", tasks_lookup, TasksLookupParams, TasksLookup),
        v1_method!(
            "tasks.filterChoices",
            tasks_filter_choices,
            TasksFilterChoicesParams,
            TasksFilterChoices
        ),
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
        v1_method!(
            "tasks.waiting_on.add",
            tasks_waiting_on_add,
            TasksWaitingOnParams,
            TaskAnswer
        ),
        v1_method!(
            "tasks.waiting_on.remove",
            tasks_waiting_on_remove,
            TasksWaitingOnParams,
            TaskAnswer
        ),
        v1_method!(
            "tasks.waiting_on.blocks",
            tasks_waiting_on_blocks,
            TasksBlocksParams,
            TaskBlocks
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
#[serde(try_from = "TasksListInput")]
pub struct TasksListParams {
    #[serde(flatten)]
    pub filters: TasksListFilters,
}

/// Typed decoding retains presence separately from a nullable scalar's value.
/// The validated public filters keep their established normalized wire shape.
#[derive(Deserialize)]
struct TasksListInput {
    #[serde(default, deserialize_with = "provided")]
    watched: Option<bool>,
    #[serde(default, deserialize_with = "provided")]
    attention_rule: Option<String>,
    #[serde(default, deserialize_with = "provided")]
    linked_workspace_id: Option<String>,
    #[serde(default, deserialize_with = "provided")]
    tracker_agent_id: Option<String>,
    #[serde(default, deserialize_with = "provided")]
    exclude_review_mode: Option<String>,
    #[serde(default, deserialize_with = "provided")]
    order: Option<String>,
    #[serde(default, deserialize_with = "provided")]
    done_since: Option<i64>,
    #[serde(default, deserialize_with = "provided")]
    unfinished: Option<bool>,
    #[serde(default, deserialize_with = "provided")]
    blocked: Option<bool>,
    #[serde(default, deserialize_with = "provided")]
    held: Option<bool>,
    #[serde(default, deserialize_with = "provided")]
    undelivered: Option<bool>,
    #[serde(default, deserialize_with = "nonnull_provided")]
    exclude_assignee: Option<Value>,

    project_id: String,
    #[serde(default)]
    summary: Option<bool>,
    #[serde(default, deserialize_with = "nullable_provided")]
    state: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable_provided")]
    status: Option<Option<String>>,
    #[serde(default, deserialize_with = "nullable_provided")]
    assignee: Option<Option<Value>>,
    #[serde(default, deserialize_with = "nullable_provided")]
    label: Option<Option<String>>,
    #[serde(default, deserialize_with = "provided")]
    states: Option<Vec<String>>,
    #[serde(default, deserialize_with = "provided")]
    statuses: Option<Vec<String>>,
    #[serde(default, deserialize_with = "provided")]
    labels: Option<Vec<String>>,
    #[serde(default, deserialize_with = "provided")]
    assignees: Option<Vec<Value>>,
    #[serde(default, deserialize_with = "nullable_provided")]
    limit: Option<Option<u64>>,
    #[serde(default, deserialize_with = "nullable_provided")]
    cursor: Option<Option<String>>,
}

fn nonnull_provided<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)?
        .map(Some)
        .ok_or_else(|| serde::de::Error::custom("selector cannot be null"))
}

fn nullable_provided<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::deserialize(deserializer).map(Some)
}

impl TryFrom<TasksListInput> for TasksListParams {
    type Error = String;

    fn try_from(input: TasksListInput) -> Result<Self, Self::Error> {
        if input.summary != Some(true)
            && [
                input.watched.is_some(),
                input.attention_rule.is_some(),
                input.linked_workspace_id.is_some(),
                input.tracker_agent_id.is_some(),
                input.exclude_review_mode.is_some(),
                input.order.is_some(),
                input.done_since.is_some(),
                input.unfinished.is_some(),
                input.blocked.is_some(),
                input.held.is_some(),
                input.undelivered.is_some(),
                input.exclude_assignee.is_some(),
            ]
            .into_iter()
            .any(|present| present)
        {
            return Err("task selectors require summary:true".into());
        }
        if input.summary == Some(true) {
            for (name, null) in [
                ("limit", input.limit == Some(None)),
                ("cursor", input.cursor == Some(None)),
            ] {
                if null {
                    return Err(format!("{name} cannot be null in summary mode"));
                }
            }
        }
        for (scalar, plural, scalar_present, plural_present) in [
            (
                "state",
                "states",
                input.state.is_some(),
                input.states.is_some(),
            ),
            (
                "status",
                "statuses",
                input.status.is_some(),
                input.statuses.is_some(),
            ),
            (
                "label",
                "labels",
                input.label.is_some(),
                input.labels.is_some(),
            ),
            (
                "assignee",
                "assignees",
                input.assignee.is_some(),
                input.assignees.is_some(),
            ),
        ] {
            if scalar_present && plural_present {
                return Err(format!("provide only one of {scalar} and {plural}"));
            }
        }
        Ok(Self {
            filters: TasksListFilters {
                watched: input.watched,
                attention_rule: input.attention_rule,
                linked_workspace_id: input.linked_workspace_id,
                tracker_agent_id: input.tracker_agent_id,
                exclude_review_mode: input.exclude_review_mode,
                order: input.order,
                done_since: input.done_since,
                unfinished: input.unfinished,
                blocked: input.blocked,
                held: input.held,
                undelivered: input.undelivered,
                exclude_assignee: input.exclude_assignee,

                project_id: input.project_id,
                summary: input.summary,
                state: input.state.flatten(),
                status: input.status.flatten(),
                assignee: input.assignee.flatten(),
                label: input.label.flatten(),
                states: input.states,
                statuses: input.statuses,
                labels: input.labels,
                assignees: input.assignees,
                limit: input.limit.flatten(),
                cursor: input.cursor.flatten(),
            },
        })
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct TasksListFilters {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watched: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention_rule: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_review_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_since: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unfinished: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undelivered: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_assignee: Option<Value>,

    pub project_id: String,
    /// Lightweight, bounded pages (3.26.0, `tasks.listSummaries`). Defaults
    /// to false, preserving legacy tasks.list rows and unbounded reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<bool>,
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
    /// OR within one selection, AND across dimensions. An empty selection or
    /// both scalar and plural forms is invalid; omit the dimension for all.
    #[serde(
        default,
        deserialize_with = "provided",
        skip_serializing_if = "Option::is_none"
    )]
    pub states: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "provided",
        skip_serializing_if = "Option::is_none"
    )]
    pub statuses: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "provided",
        skip_serializing_if = "Option::is_none"
    )]
    pub labels: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "provided",
        skip_serializing_if = "Option::is_none"
    )]
    pub assignees: Option<Vec<Value>>,
    /// At most this many tasks, 1 to 500 (1.25.0, `tasks.listPaged`). Absent
    /// is the whole legacy list, or 100 rows when summary is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// Where the page starts: a `next_cursor` an earlier page of the SAME
    /// filter answered (1.25.0). Absent is the top of the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Exact project aggregates, with at most 100 explicit agent buckets.
#[derive(Debug, Deserialize, Serialize)]
pub struct TasksCountsParams {
    pub project_id: String,
    #[serde(
        default,
        deserialize_with = "provided",
        skip_serializing_if = "Option::is_none"
    )]
    pub agent_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TaskAgentCounts {
    pub agent_id: String,
    pub unfinished_open_count: u64,
    pub watched_unread_total: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TasksCounts {
    pub project_id: String,
    pub unfinished_open_count: u64,
    pub watched_unread_total: u64,
    pub agents: Vec<TaskAgentCounts>,
    pub basis: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TasksLookupParams {
    pub project_id: String,
    pub number: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TasksLookup {
    pub project_id: String,
    pub number: u64,
    pub task: Option<TaskSummaryView>,
    pub identities: std::collections::BTreeMap<String, crate::tracker::TaskAgentIdentity>,
    pub basis: String,
}

/// Distinct facets: category is `label`; workflow column is `status`.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskFilterDimension {
    Label,
    Assignee,
    Status,
    State,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TasksFilterChoicesParams {
    pub project_id: String,
    pub dimension: TaskFilterDimension,
    #[serde(
        default,
        deserialize_with = "provided",
        skip_serializing_if = "Option::is_none"
    )]
    pub limit: Option<u64>,
    #[serde(
        default,
        deserialize_with = "provided",
        skip_serializing_if = "Option::is_none"
    )]
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TasksFilterChoices {
    pub project_id: String,
    pub dimension: TaskFilterDimension,
    /// Label strings, status/state slugs, or assignee filters (`"none"`
    /// names unassigned tasks). Choices span the project, independently of
    /// any task page or selection currently displayed by the client.
    pub choices: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
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
    pub waiting_on: Option<Vec<String>>,
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
    /// Since 3.10.0 (`tasks.bodyPrecondition`): refuse the entire update if
    /// the saved body's SHA-256 differs. Lowercase hex of its exact UTF-8;
    /// only meaningful alongside `body`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_body_hash: Option<String>,
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
/// One tagged `assignee` covering all six kinds — `user`, `project_agent`,
/// `agent`, `new_workspace`, `new_agent`, `new_project_agent` (since 3.21.0,
/// announced as `tasks.assignProjectAgent`) — because assignment IS dispatch and a
/// second field beside it would be a second place for the same decision. `null`
/// unassigns. Untyped here for the reason a timeline entry is: the five arms
/// carry different fields, and `AssignTarget::parse` is the one place that
/// reads them, naming each refusal.
#[derive(Debug, Deserialize)]
#[serde(try_from = "TasksAssignWire")]
pub struct TasksAssignParams {
    pub task_id: String,
    pub assignee: Value,
    pub retry_deferred_id: Option<String>,
    pub note: Option<String>,
    pub operation_id: Option<String>,
}

/// Preserve field presence until mutually exclusive retry inputs are validated.
#[derive(Deserialize)]
struct TasksAssignWire {
    #[serde(deserialize_with = "crate::renamed_ids::current")]
    pub task_id: String,
    #[serde(
        default,
        deserialize_with = "provided",
        skip_serializing_if = "Option::is_none"
    )]
    pub assignee: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_deferred_id: Option<String>,
    /// Extra instruction delivered under the task. Not stored on the task:
    /// the body is the task, and a hand-off note belongs in the conversation
    /// it was said in.
    #[serde(
        default,
        deserialize_with = "provided",
        skip_serializing_if = "Option::is_none"
    )]
    pub note: Option<Option<String>>,
    /// The client's id for this assignment (since 3.21.0, announced as
    /// `tasks.assignProjectAgent`). The delivery is queued under it, and
    /// asking again with the same id answers the dispatch it already made
    /// instead of delivering twice.
    #[serde(
        default,
        deserialize_with = "provided",
        skip_serializing_if = "Option::is_none"
    )]
    pub operation_id: Option<Option<String>>,
}

impl TryFrom<TasksAssignWire> for TasksAssignParams {
    type Error = &'static str;

    fn try_from(wire: TasksAssignWire) -> Result<Self, Self::Error> {
        if wire.retry_deferred_id.is_some()
            && (wire.assignee.is_some() || wire.note.is_some() || wire.operation_id.is_some())
        {
            return Err(
                "retry_deferred_id is mutually exclusive with assignee, note and operation_id",
            );
        }
        if wire.retry_deferred_id.is_none() && wire.assignee.is_none() {
            return Err("assignee is required");
        }
        Ok(Self {
            task_id: wire.task_id,
            assignee: wire.assignee.unwrap_or(Value::Null),
            retry_deferred_id: wire.retry_deferred_id,
            note: wire.note.flatten(),
            operation_id: wire.operation_id.flatten(),
        })
    }
}

impl Serialize for TasksAssignParams {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut wire = serializer.serialize_map(None)?;
        wire.serialize_entry("task_id", &self.task_id)?;
        if let Some(id) = &self.retry_deferred_id {
            wire.serialize_entry("retry_deferred_id", id)?;
        } else {
            wire.serialize_entry("assignee", &self.assignee)?;
        }
        if let Some(note) = &self.note {
            wire.serialize_entry("note", note)?;
        }
        if let Some(id) = &self.operation_id {
            wire.serialize_entry("operation_id", id)?;
        }
        wire.end()
    }
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

#[derive(Debug, Deserialize, Serialize)]
pub struct TasksWaitingOnParams {
    #[serde(deserialize_with = "crate::renamed_ids::current")]
    pub task_id: String,
    pub waiting_on: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TasksBlocksParams {
    #[serde(deserialize_with = "crate::renamed_ids::current")]
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TaskBlocks {
    pub blocks: Vec<crate::store::DependencyDigest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TaskDependenciesView {
    pub blocked: bool,
    pub waiting_on: Vec<crate::store::DependencyDigest>,
    pub blocks_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<crate::store::DependencyDigest>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks_next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DeferredAssignmentView {
    pub id: String,
    pub target: Value,
    pub state: DeferredAssignmentState,
    pub error: Option<String>,
}

fn provided<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeferredAssignmentState {
    Waiting,
    Queued,
    Failed,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<TaskDependenciesView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred_assignment: Option<DeferredAssignmentView>,
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
    /// Captured on writes so a finished workspace still names its agents,
    /// and an unnamed project worker keeps its purpose after removal.
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
    /// Timeline floor of the current watch, separate from the read cursor.
    /// Since wire 3.19.0 (`unread.watchScoped`); absent for an origin watch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_started_after: Option<String>,
    /// The last event on this task the user has read. `null` when they have
    /// read none of it, which is how a task arrives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_through: Option<String>,
    /// The last event the user put down without reading — Done until the next
    /// one. `null` unless they have dismissed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed_through: Option<String>,
    /// How many timeline entries after both `read_through` and the current
    /// watch baseline are not the user's own
    /// (1.29.0). `tasks.list` and `tasks.get` carry it on a watched task:
    /// a task nobody watches never shows a count, so it carries none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread_count: Option<u64>,
    /// A watched task's activity streak, split by a silence of 8h or more
    /// (3.21.0, `tasks.activitySessions`). Bookkeeping does not extend it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_started_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_ms: Option<i64>,
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
    /// Durable PR discovery link (3.15.0), retained after the review closes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_summary: Option<crate::reviews::records::ReviewSummary>,
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
#[serde(untagged, from = "TasksListResponse")]
pub enum TasksList {
    Summaries(TaskSummaries),
    Legacy(LegacyTasksList),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TasksListResponse {
    Summaries(TaskSummaries),
    Legacy(LegacyTasksListResponse),
}

/// A present summary discriminator belongs exclusively to the summary shape,
/// even when the value or the rest of that shape is malformed.
#[derive(Deserialize)]
struct LegacyTasksListResponse {
    #[serde(
        rename = "summary",
        default,
        deserialize_with = "reject_summary_marker"
    )]
    _summary: (),
    #[serde(flatten)]
    list: LegacyTasksList,
}

fn reject_summary_marker<'de, D: serde::Deserializer<'de>>(
    _deserializer: D,
) -> Result<(), D::Error> {
    Err(serde::de::Error::custom("summary must be true"))
}

impl From<TasksListResponse> for TasksList {
    fn from(input: TasksListResponse) -> Self {
        match input {
            TasksListResponse::Summaries(summary) => Self::Summaries(summary),
            TasksListResponse::Legacy(legacy) => Self::Legacy(legacy.list),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct LegacyTasksList {
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

/// An explicit discriminator preserves the shape of empty summary pages.
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskSummaries {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<String>,
    #[serde(deserialize_with = "summary_true")]
    pub summary: bool,
    pub project_id: String,
    pub tasks: Vec<TaskSummaryView>,
    pub identities: std::collections::BTreeMap<String, crate::tracker::TaskAgentIdentity>,
    pub user_session: UserSessionView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

fn summary_true<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    if bool::deserialize(deserializer)? {
        Ok(true)
    } else {
        Err(serde::de::Error::custom("summary must be true"))
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TaskSummaryLinks {
    pub workspace_ids: Vec<String>,
    pub parent_task_id: Option<String>,
}

/// Counts only: graph details use the bounded dependency detail APIs.
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskSummaryDependencies {
    pub blocked: bool,
    pub waiting_on_count: u64,
    pub blocks_count: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TaskMatchedMemberships {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker_agent_id: Option<String>,
}

/// Summary-only additive mode; canonical legacy review summaries stay unchanged.
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskSummaryReviewView {
    #[serde(flatten)]
    pub summary: crate::reviews::records::ReviewSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

/// List/card/badge/navigation data only. Detail content stays on tasks.get.
#[derive(Debug, Deserialize, Serialize)]
pub struct TaskSummaryView {
    #[serde(
        default,
        deserialize_with = "nullable_provided",
        skip_serializing_if = "Option::is_none"
    )]
    pub order_anchor_ms: Option<Option<i64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_memberships: Option<TaskMatchedMemberships>,
    pub id: String,
    pub project_id: String,
    pub number: u64,
    pub title: String,
    pub state: String,
    pub status: String,
    pub labels: Vec<String>,
    pub priority: String,
    pub assignee: Option<Value>,
    pub created_by: Value,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
    pub done_at: Option<String>,
    pub watched: bool,
    pub trackers: Vec<String>,
    pub links: TaskSummaryLinks,
    pub attachment_count: u64,
    pub summary_truncated: bool,
    pub dependencies: TaskSummaryDependencies,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_through: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_started_after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed_through: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread_agent_comments: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread_mentions: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_started_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_summary: Option<TaskSummaryReviewView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deferred_assignment: Option<DeferredAssignmentView>,
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
const CONFLICT: [&str; 9] = [
    "conflict: unresolved waiting-on dependencies",
    "already has the most trackers it can carry",
    // A retried assignment's id names a different assignment of the task.
    "was already spent on a different assignment",
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
const INVALID: [&str; 40] = [
    "invalid cursor",
    "invalid task cursor",
    "states must contain",
    "statuses must contain",
    "labels must contain",
    "assignees must contain",
    "cannot be supplied together",
    "unknown field in assignee filter",
    "assignee agent_id must",
    "a task cannot wait on itself",
    "waiting_on",
    "reverse cursor",
    "limit must",
    "retry_deferred_id",
    "only the current failed",
    "delivery intent",
    "unknown assignee kind",
    "operation_id exceeds",
    "operation_id contains",
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
const UNAVAILABLE: [&str; 7] = [
    "tasks need a durable store",
    "task summary exceeds",
    "filter choice exceeds",
    "invalid task summary",
    "invalid summary identity",
    "bounded task response exceeds",
    "task point response exceeds",
];

/// Name the refusal this family raised, where the sentence says what
/// [`ApiError::classify`] cannot read from its general rules. Anything
/// unmatched keeps the code classify gave it; the message is never rewritten.
///
/// [`ApiError::classify`]: crate::api::ApiError::classify
pub(crate) fn refine(error: ApiError) -> ApiError {
    if error.message().starts_with("invalid task query selector:") {
        return ApiError::invalid_params(error.message());
    }
    if let Some(encoded) = error
        .message()
        .strip_prefix("task_query_timestamp_unsupported:")
        .or_else(|| {
            error
                .message()
                .strip_prefix("task_query_marker_unsupported:")
        })
    {
        if let Ok(details) = serde_json::from_str::<Value>(encoded) {
            return ApiError::Unavailable {
                message: error.message().to_string(),
                details: Some(details),
            };
        }
    }
    if error.message().starts_with("task_query_basis_changed") {
        return ApiError::conflict(
            error.message(),
            Some(serde_json::json!({"reason":"task_query_basis_changed","restart":true})),
        );
    }
    let message = error.message().to_string();
    if message == "conflict: operation_id already used with different assignment" {
        return ApiError::conflict(message, None);
    }
    if message.contains("only the current failed delivery")
        || message.contains("retry_deferred_id does not belong")
    {
        return ApiError::conflict(message, None);
    }
    if message.starts_with("stale_body:") {
        return ApiError::StaleBody {
            message,
            details: None,
        };
    }
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

fn tasks_waiting_on_add(
    app: &mut AppState,
    params: TasksWaitingOnParams,
) -> Result<Answer<TaskAnswer>, ApiError> {
    let wire = params.wire();
    answer(app.tasks_waiting_on_edit(&wire, true)).map_err(|error| {
        refine_waiting(app, error, Some(&params.task_id), None, &params.waiting_on)
    })
}
fn tasks_waiting_on_remove(
    app: &mut AppState,
    params: TasksWaitingOnParams,
) -> Result<Answer<TaskAnswer>, ApiError> {
    let wire = params.wire();
    answer(app.tasks_waiting_on_edit(&wire, false)).map_err(|error| {
        refine_waiting(app, error, Some(&params.task_id), None, &params.waiting_on)
    })
}
fn tasks_waiting_on_blocks(
    app: &mut AppState,
    params: TasksBlocksParams,
) -> Result<Answer<TaskBlocks>, ApiError> {
    answer(app.tasks_waiting_on_blocks(&params.wire())).map_err(refine)
}

/// Add facts only after the implementation fenced every dependency endpoint.
fn refine_waiting(
    app: &AppState,
    error: ApiError,
    task_id: Option<&str>,
    project_id: Option<&str>,
    proposed: &[String],
) -> ApiError {
    let error = refine(error);
    let message = error.message().to_string();
    let blocked = message.contains("conflict: unresolved waiting-on dependencies");
    let cycle = message.contains("waiting_on would create a cycle")
        || message.contains("a task cannot wait on itself");
    if !blocked && !cycle {
        return error;
    }
    let Some(task_id) = task_id else {
        return refine_waiting_create(app, error, project_id, proposed);
    };
    let Some(task) = waiting_task_read(app, task_id) else {
        return error;
    };
    let details = if blocked {
        let waiting_on =
            scoped_waiting_blockers(app, &task.project_id, task.dependencies.as_ref(), proposed);
        Some(serde_json::json!({"task_id":task_id,"waiting_on":waiting_on}))
    } else {
        Some(serde_json::json!({"task_id":task_id,"waiting_on":proposed}))
    };
    if blocked {
        ApiError::conflict(message, details)
    } else {
        ApiError::InvalidParams { message, details }
    }
}

/// Rejected creation has no saved task; scope facts to its validated project.
fn refine_waiting_create(
    app: &AppState,
    error: ApiError,
    project_id: Option<&str>,
    proposed: &[String],
) -> ApiError {
    let Some(project_id) = project_id else {
        return error;
    };
    let waiting_on = scoped_waiting_blockers(app, project_id, None, proposed);
    ApiError::conflict(
        error.message(),
        Some(serde_json::json!({"project_id":project_id,"waiting_on":waiting_on})),
    )
}

/// A refused new edge is absent from storage; include its scoped proposal too.
fn scoped_waiting_blockers(
    app: &AppState,
    project_id: &str,
    dependencies: Option<&TaskDependenciesView>,
    proposed: &[String],
) -> Vec<crate::store::DependencyDigest> {
    let mut blockers = dependencies
        .map(|summary| {
            summary
                .waiting_on
                .iter()
                .filter(|edge| !edge.resolved)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for id in proposed {
        if blockers.iter().any(|blocker| blocker.task_id == *id) {
            continue;
        }
        if let Some(blocker) = proposed_waiting_blocker(app, project_id, id) {
            blockers.push(blocker)
        }
    }
    blockers
}

fn waiting_task_read(app: &AppState, id: &str) -> Option<TaskView> {
    let read = app.tasks_get(&serde_json::json!({"task_id":id})).ok()?;
    serde_json::from_value::<TaskDetail>(read)
        .ok()
        .map(|detail| detail.task)
}

fn proposed_waiting_blocker(
    app: &AppState,
    project_id: &str,
    id: &str,
) -> Option<crate::store::DependencyDigest> {
    let prerequisite = waiting_task_read(app, id)?;
    if prerequisite.project_id != project_id
        || prerequisite.state != "open"
        || prerequisite.status == "done"
    {
        return None;
    }
    Some(crate::store::DependencyDigest {
        task_id: id.to_string(),
        number: prerequisite.number,
        title: prerequisite.title,
        state: crate::tracker::TaskState::Open,
        status: prerequisite.status,
        resolved: false,
        resolved_by: None,
    })
}

// -------------------------------------------------------------- handlers ---

fn tasks_list(app: &mut AppState, params: TasksListParams) -> Result<Answer<TasksList>, ApiError> {
    answer(app.tasks_list(&params.wire())).map_err(refine)
}

fn tasks_counts(
    app: &mut AppState,
    params: TasksCountsParams,
) -> Result<Answer<TasksCounts>, ApiError> {
    answer(app.tasks_counts(&params.wire())).map_err(refine)
}

fn tasks_lookup(
    app: &mut AppState,
    params: TasksLookupParams,
) -> Result<Answer<TasksLookup>, ApiError> {
    answer(app.tasks_lookup(&params.wire())).map_err(refine)
}

fn tasks_filter_choices(
    app: &mut AppState,
    params: TasksFilterChoicesParams,
) -> Result<Answer<TasksFilterChoices>, ApiError> {
    answer(app.tasks_filter_choices(&params.wire())).map_err(refine)
}

fn tasks_get(app: &mut AppState, params: TaskIdParams) -> Result<Answer<TaskDetail>, ApiError> {
    answer(app.tasks_get(&params.wire())).map_err(refine)
}

fn tasks_create(
    app: &mut AppState,
    params: TasksCreateParams,
) -> Result<Answer<TaskAssigned>, ApiError> {
    let wire = params.wire();
    answer(app.tasks_create(&wire).map(super::deferral_placeholder)).map_err(|error| {
        refine_waiting(
            app,
            error,
            None,
            Some(&params.project_id),
            params.waiting_on.as_deref().unwrap_or_default(),
        )
    })
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
    let wire = params.wire();
    answer(app.tasks_update(&wire))
        .map_err(|error| refine_waiting(app, error, Some(&params.task_id), None, &[]))
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
    let wire = params.wire();
    answer(app.tasks_assign(&wire).map(super::deferral_placeholder))
        .map_err(|error| refine_waiting(app, error, Some(&params.task_id), None, &[]))
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
    fn query_selector_validation_is_invalid_params_without_reclassifying_other_failures() {
        for message in [
            "attention_rule must be asked or legacy",
            "exclude_review_mode must be pull_request or snapshot",
            "exclude_assignee must be an exact valid assignee object",
            "membership id must contain 1 to 128 bytes",
            "invalid task query selector: invalid type: string, expected a boolean",
        ] {
            let error = refine(ApiError::classify(message.into()));
            assert_eq!(error.code(), "invalid_params", "{message}");
            assert!(!error.retryable(), "{message}");
        }
        assert_eq!(
            refine(ApiError::classify(
                "unexpected legacy tracker failure".into()
            ))
            .code(),
            "internal"
        );
    }

    #[test]
    fn unsupported_order_timestamp_preserves_structured_refusal() {
        let details = serde_json::json!({"reason":"task_query_timestamp_unsupported","scope":"project_order","order":"updated","project_id":"proj-1","domain":"ecmascript-iso-v1","field":"updated_at"});
        let message = format!("task_query_timestamp_unsupported:{details}");
        let reply = refine(ApiError::classify(message)).into_reply(serde_json::json!("r-ordered"));
        assert_eq!(reply["error_code"], "unavailable");
        assert_eq!(reply["retryable"], false);
        assert_eq!(reply["details"], details);
        assert!(reply.get("result").is_none());
        assert!(reply.get("next_cursor").is_none());
    }

    #[test]
    fn query_basis_change_is_a_restartable_conflict() {
        let reply = refine(ApiError::classify(
            "task_query_basis_changed: restart this query".into(),
        ))
        .into_reply(serde_json::json!("r-basis"));
        assert_eq!(reply["error_code"], "conflict");
        assert_eq!(reply["retryable"], false);
        assert_eq!(
            reply["details"],
            serde_json::json!({"reason":"task_query_basis_changed","restart":true})
        );
    }

    #[test]
    fn bounded_read_refusals_are_typed() {
        for message in [
            "invalid cursor: restart this list with its original filters",
            "invalid task cursor",
            "states must contain 1 to 100 selections",
            "statuses must contain 1 to 100 selections",
            "labels must contain 1 to 100 selections",
            "assignees must contain 1 to 100 selections",
            "state and states cannot be supplied together",
            "unknown field in assignee filter",
            "unknown assignee kind",
            "assignee agent_id must contain 1 to 128 bytes",
        ] {
            let error = refine(ApiError::classify(message.into()));
            assert_eq!(error.code(), "invalid_params", "{message}");
        }
        for message in [
            "task summary exceeds 65536 bytes; repair the stored row before continuing",
            "filter choice exceeds 65536 bytes; repair the stored choice before continuing",
            "invalid task summary: legacy task labels exceed supported summary limits",
            "invalid task summary data: legacy task reference exceeds 128 bytes",
            "bounded task response exceeds 1048576 bytes",
        ] {
            let error = refine(ApiError::classify(message.into()));
            assert_eq!(error.code(), "unavailable", "{message}");
            assert!(!error.retryable());
        }
    }

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
