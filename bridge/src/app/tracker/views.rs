//! What a client reads.
//!
//! The record holds `project_path`, because a `proj-N` id is minted per boot
//! and is not durable. The wire holds `project_id`, because that is what every
//! other verb is addressed by. This module is the one place the swap happens,
//! so no surface above it ever sees a path and no record below it ever sees an
//! id.

use crate::tracker::{Task, TimelineEntry, COLUMNS};
use serde_json::{json, Value};

/// One task as every tracker verb answers it.
pub(in crate::app) fn task_json(project_id: &str, task: &Task) -> Value {
    let mut value = serde_json::to_value(task).expect("a task always serializes");
    let object = value.as_object_mut().expect("a task serializes as object");
    object.remove("project_path");
    object.insert("project_id".to_string(), json!(project_id));
    value
}

/// One task as a read that holds its timeline answers it — `tasks.get`, and
/// each row of `tasks.list`: `task_json`, and on a watched task the
/// `unread_count` its inbox row says (#104), so the Tasks tab and the rail's
/// badges read it off the list without a timeline each. A task nobody
/// watches never shows a count, so it carries none.
pub(in crate::app) fn read_task_json(
    project_id: &str,
    task: &Task,
    timeline: &[TimelineEntry],
) -> Value {
    let mut value = task_json(project_id, task);
    if task.watched {
        value["unread_count"] = Value::from(super::inbox::unread_since_mark(task, timeline));
    }
    value
}

/// A task and its whole timeline — what `tasks.get` answers.
///
/// A timeline entry is the record itself with one more key naming which it is
/// (the spread form), so a client reads a comment's own fields off the entry
/// rather than reaching through a wrapper.
pub(in crate::app) fn task_with_timeline_json(
    project_id: &str,
    task: &Task,
    timeline: &[TimelineEntry],
) -> Value {
    json!({
        "task": read_task_json(project_id, task, timeline),
        "timeline": timeline
            .iter()
            .map(|entry| serde_json::to_value(entry).expect("a timeline entry serializes"))
            .collect::<Vec<_>>(),
    })
}

/// The board's columns, in board order.
pub(in crate::app) fn columns_json(project_id: &str) -> Value {
    json!({
        "project_id": project_id,
        "columns": COLUMNS
            .iter()
            .map(|column| json!({ "id": column.id, "name": column.name }))
            .collect::<Vec<_>>(),
    })
}
