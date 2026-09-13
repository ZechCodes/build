use super::protocol::{ACTIVITY_TEXT_LIMIT, TOOL_SUMMARY_LIMIT};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Voice {
    Assistant,
    User,
}

/// What the agent actually said in a content block, or `None` for a block with
/// nothing in it — an empty block is not something the agent said.
pub(super) fn spoken(text: Option<&str>) -> Option<String> {
    let text = text.unwrap_or_default().trim();
    match text.is_empty() {
        true => None,
        false => Some(bounded_activity_text(text)),
    }
}

/// Expandable activity text, preserving whitespace and clipping only unusually
/// large provider events. Compact status surfaces use [`one_line`] separately.
pub(crate) fn bounded_activity_text(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= ACTIVITY_TEXT_LIMIT {
        return text.to_string();
    }
    let mut clipped: String = text.chars().take(ACTIVITY_TEXT_LIMIT).collect();
    clipped.push('…');
    clipped
}

/// The one argument each tool is worth reading by — its meat key, matched on
/// the name the protocol calls the tool.
///
/// One table, read by one function: a row is one quiet line, so a call named
/// here mints this field and drops everything else it carried. `Bash`'s
/// `description` is dropped deliberately — it is the model's paraphrase where
/// the command is the record, and two claims about one act are worse than one.
const TOOL_MEAT_KEYS: &[(&str, &str)] = &[
    ("Bash", "command"),
    ("Read", "file_path"),
    ("Write", "file_path"),
    ("Edit", "file_path"),
    ("NotebookEdit", "notebook_path"),
    ("Glob", "pattern"),
    ("Grep", "pattern"),
    ("WebFetch", "url"),
    ("WebSearch", "query"),
    ("Task", "description"),
];

/// A tool call on one line: the tool's name, plus the thing it acted on —
/// `Bash cargo test`, `Read bridge/src/app.rs`.
///
/// The name still leads, because the row's icon says only "a tool call" and
/// `Edit foo.rs` against `Read foo.rs` is a distinction worth five characters.
pub(super) fn tool_call_summary(tool: &str, input: &Value) -> String {
    let meat = tool_call_meat(tool, input);
    let summary = match meat.trim().is_empty() {
        true => tool.to_string(),
        false => format!("{tool} {meat}"),
    };
    bounded_activity_text(&summary)
}

/// What a call is worth reading: its tool's meat key when [`TOOL_MEAT_KEYS`]
/// names one and the call carried it as a string, else the first string-valued
/// field the input holds, else nothing at all.
///
/// Never JSON. A tool the table has not heard of — an MCP tool, or one newer
/// than this table — is guessed at rather than rendered as an object, because a
/// truncated-but-human line beats a line of punctuation nobody can scan: the row
/// is a scent, and the fold body and the diff are the record. Fields iterate in
/// key order, so which one a guess lands on is a property of the call rather
/// than of how the child happened to spell it.
fn tool_call_meat(tool: &str, input: &Value) -> String {
    let Some(fields) = input.as_object() else {
        return input.as_str().unwrap_or_default().to_string();
    };
    TOOL_MEAT_KEYS
        .iter()
        .find(|(named, _)| *named == tool)
        .and_then(|(_, key)| fields.get(*key)?.as_str())
        .or_else(|| fields.values().find_map(Value::as_str))
        .unwrap_or_default()
        .to_string()
}

/// What a tool answered. The protocol allows both shapes — a plain string, or
/// the content blocks a richer tool returns — so both are read.
pub(super) fn unix_millis_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_millis() as u64
}

pub(crate) fn tool_result_text(block: &Value) -> String {
    match &block["content"] {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|inner| inner["text"].as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// What a task goes by in the timeline: the description the child gave it,
/// falling back to its id. A row reading `bi1jfa1kd — started` says less than
/// one naming the work, and far more than ` — started`.
pub(super) fn task_description(event: &Value, id: &str) -> String {
    let described = event["description"].as_str().unwrap_or_default().trim();
    match described.is_empty() {
        false => described.to_string(),
        true => id.to_string(),
    }
}

/// Whether a reported task status means the work is over.
///
/// Named rather than inferred from the absence of a running status, so an
/// unrecognised status leaves the task in the set instead of closing it — where
/// the roster, when one is coming, will close it on the child's own word.
///
/// `completed`, `failed`, `killed` and `stopped` are the four the live probes
/// turned up; the rest are the shapes their names imply, recognised so a task
/// ending under one of them is not held open waiting for a roster that, for a
/// foreground task, never comes.
pub(crate) fn task_status_is_terminal(status: &str) -> bool {
    matches!(
        status,
        "completed"
            | "failed"
            | "error"
            | "cancelled"
            | "canceled"
            | "killed"
            | "stopped"
            | "timed_out"
    )
}

pub(crate) fn task_status_failed(status: &str) -> bool {
    matches!(status, "failed" | "error" | "timed_out")
}

/// The row a task's ending mints: `failed` when the event that ended it said
/// so, with the error it named, and `finished` otherwise. A task that was
/// cancelled, killed or stopped did not fail — something ended it, which is not
/// the same thing to read.
///
/// `ending` is whichever event carried the terminal status: a `task_updated`'s
/// patch, or a `task_notification` itself, which carries its status at the top
/// level and — as the probes recorded it — no error text at all.
pub(super) fn ended_summary(status: &str, description: &str, ending: &Value) -> String {
    if !task_status_failed(status) {
        return format!("{description} — finished");
    }
    let reported = ending["error"]
        .as_str()
        .or_else(|| ending["result"].as_str())
        .unwrap_or_default()
        .trim();
    match reported.is_empty() {
        true => format!("{description} — failed"),
        false => format!("{description} — failed: {reported}"),
    }
}

/// The error text a failed result carried, falling back to its subtype: an
/// epitaph naming `error_max_turns` explains more than an empty string does.
pub(super) fn result_error_text(event: &Value) -> String {
    let reported = event["result"]
        .as_str()
        .or_else(|| event["error"].as_str())
        .unwrap_or_default()
        .trim();
    match reported.is_empty() {
        false => one_line(reported, TOOL_SUMMARY_LIMIT),
        true => event["subtype"]
            .as_str()
            .unwrap_or("the session failed without saying why")
            .to_string(),
    }
}

/// `text` collapsed onto one line and clipped to `limit` characters. Clipped by
/// characters rather than bytes: tool output is arbitrary UTF-8, and a byte
/// truncation would split one.
pub(crate) fn one_line(text: &str, limit: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= limit {
        return collapsed;
    }
    let mut clipped: String = collapsed.chars().take(limit).collect();
    clipped.push('…');
    clipped
}
