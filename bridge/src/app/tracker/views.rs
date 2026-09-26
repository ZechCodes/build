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

/// One issue as a read that holds its timeline answers it — `issues.get`, and
/// each row of `issues.list`: `issue_json`, and on a watched issue the
/// `unread_count` its inbox row says (#104), so the Issues tab and the rail's
/// badges read it off the list without a timeline each. An issue nobody
/// watches never shows a count, so it carries none.
pub(in crate::app) fn read_issue_json(
    project_id: &str,
    issue: &Issue,
    timeline: &[TimelineEntry],
) -> Value {
    let mut value = issue_json(project_id, issue);
    if issue.watched {
        value["unread_count"] = Value::from(super::inbox::unread_since_mark(issue, timeline));
    }
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
        "issue": read_issue_json(project_id, issue, timeline),
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
