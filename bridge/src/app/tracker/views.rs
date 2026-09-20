//! What a client reads.
//!
//! The record holds `project_path`, because a `proj-N` id is minted per boot
//! and is not durable. The wire holds `project_id`, because that is what every
//! other verb is addressed by. This module is the one place the swap happens,
//! so no surface above it ever sees a path and no record below it ever sees an
//! id.

use crate::tracker::{Issue, TimelineEntry, COLUMNS};
use serde_json::{json, Value};

/// One issue as every tracker verb answers it.
pub(in crate::app) fn issue_json(project_id: &str, issue: &Issue) -> Value {
    let mut value = serde_json::to_value(issue).expect("an issue always serializes");
    let object = value
        .as_object_mut()
        .expect("an issue serializes as object");
    object.remove("project_path");
    object.insert("project_id".to_string(), json!(project_id));
    value
}

/// An issue and its whole timeline — what `issues.get` answers.
///
/// A timeline entry is the record itself with one more key naming which it is
/// (the spread form), so a client reads a comment's own fields off the entry
/// rather than reaching through a wrapper.
pub(in crate::app) fn issue_with_timeline_json(
    project_id: &str,
    issue: &Issue,
    timeline: &[TimelineEntry],
) -> Value {
    json!({
        "issue": issue_json(project_id, issue),
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
