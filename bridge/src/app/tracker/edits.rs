//! Reading a tracker verb's params, and deciding what one write changes.
//!
//! Nothing here touches the store or the app. A verb resolves its task, hands
//! it to one of these, and writes what comes back — so what an update means is
//! in one place and testable without a database.

use super::TaskWrite;
use crate::app::{optional_nonempty_string, require_str};
use crate::tracker::{
    column_names, normalize_labels, normalize_status, Actor, Assignee, Task, TaskEventKind,
    TaskPriority, TaskState, DONE_STATUS, MAX_BODY_BYTES, MAX_TITLE_BYTES,
};
use serde_json::{json, Value};

/// A required string param, trimmed, non-empty, and under its cap.
pub(super) fn required_text(params: &Value, key: &str, cap: usize) -> Result<String, String> {
    let text = require_str(params, key)?.trim().to_string();
    if text.is_empty() {
        return Err(format!("{key} cannot be empty"));
    }
    if text.len() > cap {
        return Err(format!("{key} exceeds {cap} bytes"));
    }
    Ok(text)
}

/// The same, optional. Present-and-blank is still a refusal: a caller that
/// sent the key meant to say something.
fn optional_text(params: &Value, key: &str, cap: usize) -> Result<Option<String>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => required_text(params, key, cap).map(Some),
    }
}

/// A body, which unlike a title may legitimately be emptied.
fn optional_body(params: &Value) -> Result<Option<String>, String> {
    match params.get("body") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(body)) if body.len() > MAX_BODY_BYTES => {
            Err(format!("body exceeds {MAX_BODY_BYTES} bytes"))
        }
        Some(Value::String(body)) => Ok(Some(body.trim().to_string())),
        Some(_) => Err("body must be a string".to_string()),
    }
}

/// A column named by slug or by what it says, or why it names none.
pub(super) fn optional_status(params: &Value, key: &str) -> Result<Option<String>, String> {
    let Some(word) = optional_nonempty_string(params, key)? else {
        return Ok(None);
    };
    normalize_status(word)
        .map(|slug| Some(slug.to_string()))
        .ok_or_else(|| format!("unknown {key}: {word} — one of {}", column_names()))
}

pub(super) fn optional_state(params: &Value) -> Result<Option<TaskState>, String> {
    let Some(word) = optional_nonempty_string(params, "state")? else {
        return Ok(None);
    };
    TaskState::parse(word)
        .map(Some)
        .ok_or_else(|| format!("unknown state: {word} — one of open, closed"))
}

fn optional_priority(params: &Value) -> Result<Option<TaskPriority>, String> {
    let Some(word) = optional_nonempty_string(params, "priority")? else {
        return Ok(None);
    };
    TaskPriority::parse(word)
        .map(Some)
        .ok_or_else(|| format!("unknown priority: {word} — one of none, low, medium, high, urgent"))
}

fn optional_labels(params: &Value) -> Result<Option<Vec<String>>, String> {
    match params.get("labels") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(values)) => {
            let words: Vec<String> = values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(str::to_string)
                        .ok_or_else(|| "labels must be strings".to_string())
                })
                .collect::<Result<_, _>>()?;
            normalize_labels(&words).map(Some)
        }
        Some(_) => Err("labels must be an array".to_string()),
    }
}

/// A newly filed task, before the store mints its number.
pub(super) fn drafted_task(
    params: &Value,
    project_path: &str,
    created_by: Actor,
    now: &str,
) -> Result<Task, String> {
    let title = required_text(params, "title", MAX_TITLE_BYTES)?;
    let mut task = Task::drafted(project_path, &title, created_by, now);
    task.body = optional_body(params)?.unwrap_or_default();
    if let Some(status) = optional_status(params, "status")? {
        if status == DONE_STATUS {
            task.done_at = Some(now.to_string());
        }
        task.status = status;
    }
    if let Some(labels) = optional_labels(params)? {
        task.labels = labels;
    }
    if let Some(priority) = optional_priority(params)? {
        task.priority = priority;
    }
    Ok(task)
}

/// Apply the fields an update names, writing one event per change that the
/// timeline has to carry.
///
/// A title, a body and a priority write none: `updated_at` is the whole history
/// those need, and an event per keystroke would bury the ones that matter.
pub(super) fn apply_update(
    write: &mut TaskWrite,
    params: &Value,
    actor: &Actor,
    now: &str,
) -> Result<(), String> {
    if let Some(title) = optional_text(params, "title", MAX_TITLE_BYTES)? {
        write.task.title = title;
    }
    if let Some(body) = optional_body(params)? {
        write.task.body = body;
    }
    if let Some(priority) = optional_priority(params)? {
        write.task.priority = priority;
    }
    if let Some(labels) = optional_labels(params)? {
        apply_labels(write, labels, actor, now);
    }
    if let Some(status) = optional_status(params, "status")? {
        move_to(write, &status, actor, json!({}), now);
    }
    if let Some(state) = optional_state(params)? {
        apply_state(write, state, actor, now);
    }
    Ok(())
}

/// Move a task to a column, saying where it came from. Moving it where it
/// already is writes nothing: a timeline records changes, not requests.
pub(super) fn move_to(write: &mut TaskWrite, status: &str, actor: &Actor, extra: Value, now: &str) {
    if write.task.status == status {
        return;
    }
    let mut payload = json!({ "from": write.task.status, "to": status });
    if let (Some(payload), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
        payload.extend(extra.clone());
    }
    write.task.status = status.to_string();
    write.task.done_at = (status == DONE_STATUS).then(|| now.to_string());
    write.event(actor, TaskEventKind::Moved, payload, now);
}

fn apply_labels(write: &mut TaskWrite, labels: Vec<String>, actor: &Actor, now: &str) {
    let added: Vec<&String> = labels
        .iter()
        .filter(|label| !write.task.labels.contains(label))
        .collect();
    let removed: Vec<&String> = write
        .task
        .labels
        .iter()
        .filter(|label| !labels.contains(label))
        .collect();
    if added.is_empty() && removed.is_empty() {
        return;
    }
    let payload = json!({ "added": added, "removed": removed });
    write.task.labels = labels;
    write.event(actor, TaskEventKind::Labelled, payload, now);
}

fn apply_state(write: &mut TaskWrite, state: TaskState, actor: &Actor, now: &str) {
    if write.task.state == state {
        return;
    }
    match state {
        TaskState::Closed => close(write, actor, None, now),
        TaskState::Open => {
            write.task.state = TaskState::Open;
            write.task.closed_at = None;
            write.event(actor, TaskEventKind::Reopened, json!({}), now);
        }
    }
}

/// Close a task and say why, if there is a why.
///
/// It does NOT move the task to Done. One says where the card is on the board,
/// the other whether anyone is still expected to do something about it, and
/// collapsing them would lose a task closed as "not doing this".
pub(super) fn close(write: &mut TaskWrite, actor: &Actor, reason: Option<String>, now: &str) {
    write.task.state = TaskState::Closed;
    write.task.closed_at = Some(now.to_string());
    let payload = match reason {
        Some(reason) => json!({ "reason": reason }),
        None => json!({}),
    };
    write.event(actor, TaskEventKind::Closed, payload, now);
}

/// Whether a task carries the label a filter asked for, case-insensitively —
/// labels are deduped that way, so they are matched that way.
pub(super) fn carries_label(task: &Task, label: Option<&str>) -> bool {
    match label {
        None => true,
        Some(label) => task
            .labels
            .iter()
            .any(|carried| carried.eq_ignore_ascii_case(label)),
    }
}

/// What an `assignee` filter asked for. `any` and `none` are words rather than
/// shapes because neither names an assignee.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) enum AssigneeFilter {
    #[default]
    Unfiltered,
    Unassigned,
    Assigned,
    Exactly(Assignee),
}

impl AssigneeFilter {
    pub(super) fn matches(&self, task: &Task) -> bool {
        match self {
            AssigneeFilter::Unfiltered => true,
            AssigneeFilter::Unassigned => task.assignee.is_none(),
            AssigneeFilter::Assigned => task.assignee.is_some(),
            AssigneeFilter::Exactly(wanted) => task.assignee.as_ref() == Some(wanted),
        }
    }

    /// The filter as one value, the same however it was spelled on the wire:
    /// what a list cursor is made over.
    pub(super) fn key(&self) -> Value {
        match self {
            AssigneeFilter::Unfiltered => Value::Null,
            AssigneeFilter::Unassigned => Value::from("none"),
            AssigneeFilter::Assigned => Value::from("any"),
            AssigneeFilter::Exactly(wanted) => serde_json::to_value(wanted).unwrap_or_default(),
        }
    }
}

pub(super) fn optional_assignee_filter(params: &Value) -> Result<AssigneeFilter, String> {
    match params.get("assignee") {
        None | Some(Value::Null) => Ok(AssigneeFilter::Unfiltered),
        Some(Value::String(word)) if word == "none" => Ok(AssigneeFilter::Unassigned),
        Some(Value::String(word)) if word == "any" => Ok(AssigneeFilter::Assigned),
        Some(Value::String(word)) => Err(format!(
            "unknown assignee filter: {word} — a filter is \"none\", \"any\", or an assignee"
        )),
        Some(value) => serde_json::from_value(value.clone())
            .map(AssigneeFilter::Exactly)
            .map_err(|error| format!("assignee: {error}")),
    }
}

/// The links one `tasks.link` call asked for, in the order the verb applies
/// them. Built as one value so a call naming three of the five is one walk.
#[derive(Debug, Default)]
pub(in crate::app) struct AskedLinks {
    asked: Vec<(&'static str, String)>,
}

impl AskedLinks {
    pub(in crate::app) fn entries(&self) -> impl Iterator<Item = (&'static str, &str)> {
        self.asked
            .iter()
            .map(|(kind, value)| (*kind, value.as_str()))
    }

    pub(in crate::app) fn is_empty(&self) -> bool {
        self.asked.is_empty()
    }

    pub(in crate::app) fn push(&mut self, kind: &'static str, value: impl Into<String>) {
        self.asked.push((kind, value.into()));
    }
}

/// The five link keys, read off one call. Naming none of them is a refusal:
/// a link call that links nothing is a caller that meant something.
pub(super) fn asked_links(params: &Value) -> Result<AskedLinks, String> {
    let mut asked = AskedLinks::default();
    for key in [
        "workspace_id",
        "branch",
        "commit",
        "conversation_id",
        "parent_task_id",
    ] {
        if let Some(value) = optional_nonempty_string(params, key)? {
            asked.push(key, value);
        }
    }
    if asked.is_empty() {
        return Err(
            "tasks.link: name a workspace_id, branch, commit, conversation_id or parent_task_id"
                .to_string(),
        );
    }
    Ok(asked)
}
