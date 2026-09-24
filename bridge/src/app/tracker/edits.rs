//! Reading a tracker verb's params, and deciding what one write changes.
//!
//! Nothing here touches the store or the app. A verb resolves its issue, hands
//! it to one of these, and writes what comes back — so what an update means is
//! in one place and testable without a database.

use super::IssueWrite;
use crate::app::{optional_nonempty_string, require_str};
use crate::tracker::{
    column_names, normalize_labels, normalize_status, Actor, Assignee, Issue, IssueEventKind,
    IssuePriority, IssueState, DONE_STATUS, MAX_BODY_BYTES, MAX_TITLE_BYTES,
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

pub(super) fn optional_state(params: &Value) -> Result<Option<IssueState>, String> {
    let Some(word) = optional_nonempty_string(params, "state")? else {
        return Ok(None);
    };
    IssueState::parse(word)
        .map(Some)
        .ok_or_else(|| format!("unknown state: {word} — one of open, closed"))
}

fn optional_priority(params: &Value) -> Result<Option<IssuePriority>, String> {
    let Some(word) = optional_nonempty_string(params, "priority")? else {
        return Ok(None);
    };
    IssuePriority::parse(word)
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

/// A newly filed issue, before the store mints its number.
pub(super) fn drafted_issue(
    params: &Value,
    project_path: &str,
    created_by: Actor,
    now: &str,
) -> Result<Issue, String> {
    let title = required_text(params, "title", MAX_TITLE_BYTES)?;
    let mut issue = Issue::drafted(project_path, &title, created_by, now);
    issue.body = optional_body(params)?.unwrap_or_default();
    if let Some(status) = optional_status(params, "status")? {
        if status == DONE_STATUS {
            issue.done_at = Some(now.to_string());
        }
        issue.status = status;
    }
    if let Some(labels) = optional_labels(params)? {
        issue.labels = labels;
    }
    if let Some(priority) = optional_priority(params)? {
        issue.priority = priority;
    }
    Ok(issue)
}

/// Apply the fields an update names, writing one event per change that the
/// timeline has to carry.
///
/// A title, a body and a priority write none: `updated_at` is the whole history
/// those need, and an event per keystroke would bury the ones that matter.
pub(super) fn apply_update(
    write: &mut IssueWrite,
    params: &Value,
    actor: &Actor,
    now: &str,
) -> Result<(), String> {
    if let Some(title) = optional_text(params, "title", MAX_TITLE_BYTES)? {
        write.issue.title = title;
    }
    if let Some(body) = optional_body(params)? {
        write.issue.body = body;
    }
    if let Some(priority) = optional_priority(params)? {
        write.issue.priority = priority;
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

/// Move an issue to a column, saying where it came from. Moving it where it
/// already is writes nothing: a timeline records changes, not requests.
pub(super) fn move_to(
    write: &mut IssueWrite,
    status: &str,
    actor: &Actor,
    extra: Value,
    now: &str,
) {
    if write.issue.status == status {
        return;
    }
    let mut payload = json!({ "from": write.issue.status, "to": status });
    if let (Some(payload), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
        payload.extend(extra.clone());
    }
    write.issue.status = status.to_string();
    write.issue.done_at = (status == DONE_STATUS).then(|| now.to_string());
    write.event(actor, IssueEventKind::Moved, payload, now);
}

fn apply_labels(write: &mut IssueWrite, labels: Vec<String>, actor: &Actor, now: &str) {
    let added: Vec<&String> = labels
        .iter()
        .filter(|label| !write.issue.labels.contains(label))
        .collect();
    let removed: Vec<&String> = write
        .issue
        .labels
        .iter()
        .filter(|label| !labels.contains(label))
        .collect();
    if added.is_empty() && removed.is_empty() {
        return;
    }
    let payload = json!({ "added": added, "removed": removed });
    write.issue.labels = labels;
    write.event(actor, IssueEventKind::Labelled, payload, now);
}

fn apply_state(write: &mut IssueWrite, state: IssueState, actor: &Actor, now: &str) {
    if write.issue.state == state {
        return;
    }
    match state {
        IssueState::Closed => close(write, actor, None, now),
        IssueState::Open => {
            write.issue.state = IssueState::Open;
            write.issue.closed_at = None;
            write.event(actor, IssueEventKind::Reopened, json!({}), now);
        }
    }
}

/// Close an issue and say why, if there is a why.
///
/// It does NOT move the issue to Done. One says where the card is on the board,
/// the other whether anyone is still expected to do something about it, and
/// collapsing them would lose an issue closed as "not doing this".
pub(super) fn close(write: &mut IssueWrite, actor: &Actor, reason: Option<String>, now: &str) {
    write.issue.state = IssueState::Closed;
    write.issue.closed_at = Some(now.to_string());
    let payload = match reason {
        Some(reason) => json!({ "reason": reason }),
        None => json!({}),
    };
    write.event(actor, IssueEventKind::Closed, payload, now);
}

/// Whether an issue carries the label a filter asked for, case-insensitively —
/// labels are deduped that way, so they are matched that way.
pub(super) fn carries_label(issue: &Issue, label: Option<&str>) -> bool {
    match label {
        None => true,
        Some(label) => issue
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
    pub(super) fn matches(&self, issue: &Issue) -> bool {
        match self {
            AssigneeFilter::Unfiltered => true,
            AssigneeFilter::Unassigned => issue.assignee.is_none(),
            AssigneeFilter::Assigned => issue.assignee.is_some(),
            AssigneeFilter::Exactly(wanted) => issue.assignee.as_ref() == Some(wanted),
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

/// The links one `issues.link` call asked for, in the order the verb applies
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
        "parent_issue_id",
    ] {
        if let Some(value) = optional_nonempty_string(params, key)? {
            asked.push(key, value);
        }
    }
    if asked.is_empty() {
        return Err(
            "issues.link: name a workspace_id, branch, commit, conversation_id or parent_issue_id"
                .to_string(),
        );
    }
    Ok(asked)
}
