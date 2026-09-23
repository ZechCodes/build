//! The issues family: the per-project issue tracker (spec: Issues).
//!
//! NOT the `issue.*` verbs in [`lifecycle`](super::lifecycle). Those are the
//! retired plan-and-stages flow, which shares the English word and nothing
//! else. The two families are spelled apart — singular `issue.*` there, plural
//! `issues.*` here — so no name can be registered by both, and the test at the
//! bottom of `api/v1/mod.rs` proves it.
//!
//! Same shape as the workspace family: the implementations under
//! `app/tracker/` are untouched, each handler resolves its typed params, hands
//! them to the implementation, and names the shape that implementation answers
//! in. Nothing here defers — a tracker verb is a SQLite write and an in-memory
//! lookup, with no git to hand to the drain.
//!
//! On the nullable fields: an issue writes its absences as explicit `null`s
//! (`assignee`, `closed_at`, `links.parent_issue_id`) rather than leaving the
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
        v1_method!("issues.list", issues_list, IssuesListParams, IssuesList),
        v1_method!("issues.get", issues_get, IssueIdParams, IssueDetail),
        v1_method!(
            "issues.create",
            issues_create,
            IssuesCreateParams,
            IssueAssigned
        ),
        v1_method!(
            "issues.update",
            issues_update,
            IssuesUpdateParams,
            IssueAnswer
        ),
        v1_method!(
            "issues.comment",
            issues_comment,
            IssuesCommentParams,
            IssueComment
        ),
        v1_method!(
            "issues.assign",
            issues_assign,
            IssuesAssignParams,
            IssueAssigned
        ),
        v1_method!("issues.link", issues_link, IssuesLinkParams, IssueAnswer),
        v1_method!("issues.close", issues_close, IssuesCloseParams, IssueAnswer),
        v1_method!("issues.reopen", issues_reopen, IssueIdParams, IssueAnswer),
        v1_method!("issues.track", issues_track, IssuesTrackParams, IssueAnswer),
        v1_method!(
            "issues.untrack",
            issues_untrack,
            IssuesTrackParams,
            IssueAnswer
        ),
        v1_method!("issues.watch", issues_watch, IssueIdParams, IssueAnswer),
        v1_method!("issues.dismiss", issues_dismiss, IssueIdParams, IssueAnswer),
        v1_method!("issues.unwatch", issues_unwatch, IssueIdParams, IssueAnswer),
        v1_method!(
            "issues.read_through",
            issues_read_through,
            IssuesReadThroughParams,
            IssueAnswer
        ),
        v1_method!(
            "issues.for_agent",
            issues_for_agent,
            IssuesForAgentParams,
            IssuesForAgent
        ),
        v1_method!(
            "issues.columns",
            issues_columns,
            ProjectIdParams,
            IssueColumns
        ),
        v1_method!(
            "issues.attach",
            issues_attach,
            IssuesAttachParams,
            IssueAttachment
        ),
        v1_method!(
            "issues.attachment",
            issues_attachment,
            IssuesAttachmentParams,
            IssueAttachmentBytes
        ),
    ]
}

// ---------------------------------------------------------------- params ---

#[derive(Debug, Deserialize, Serialize)]
pub struct ProjectIdParams {
    pub project_id: String,
}

/// An issue named and nothing else asked of it: `issues.get`,
/// `issues.reopen`.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueIdParams {
    pub issue_id: String,
}

/// One project's issues, narrowed. Every filter is optional and they are ANDed.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct IssuesListParams {
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
}

/// One file, filed with an issue and BEFORE it.
///
/// Separate from the create for the reason `thread.attach` is separate from the
/// post: the bytes are on disk and verified before the record that names them
/// exists, so an issue can never point at an upload that failed halfway.
///
/// A project and not a conversation: an issue being created has neither, and
/// one filed unassigned never gets one.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssuesAttachParams {
    pub project_id: String,
    pub filename: String,
    pub content_b64: String,
}

/// What an issue's attachment reads back as. Addressed through the ISSUE, so
/// no caller has to know (or can get wrong) where the file landed.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssuesAttachmentParams {
    pub issue_id: String,
    pub path: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct IssuesCreateParams {
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
    /// Who to hand it to. The same five kinds `issues.assign` takes, and the
    /// same consequence: filing an issue for somebody starts them on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<Value>,
    /// The files filed with it, each naming a path `issues.attach` answered.
    /// The same shape a message carries, because they are the same object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<IssueAttachmentRef>>,
    /// Extra instruction delivered under the issue, when it is assigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Only the fields present are applied. An absent field is not "set this to
/// nothing" — there is no verb here that empties a title.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssuesUpdateParams {
    pub issue_id: String,
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
pub struct IssuesCommentParams {
    pub issue_id: String,
    pub body: String,
    /// Typed references, fenced by shape and then by what this issue is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs: Option<Vec<crate::thread::ThreadLink>>,
    /// The files said with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<IssueAttachmentRef>>,
}

/// One attachment as a REQUEST names it: the path it was stored at, and
/// optionally what to call it. Name, mime and size are re-read from disk when
/// the record is written — the client's copy is a display hint.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueAttachmentRef {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// One or more of the five link keys. Naming none is refused.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct IssuesLinkParams {
    pub issue_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_issue_id: Option<String>,
}

/// Hand an issue to somebody, and start them on it.
///
/// One tagged `assignee` covering all five kinds — `user`, `project_agent`,
/// `agent`, `new_workspace`, `new_agent` — because assignment IS dispatch and a
/// second field beside it would be a second place for the same decision. `null`
/// unassigns. Untyped here for the reason a timeline entry is: the five arms
/// carry different fields, and `AssignTarget::parse` is the one place that
/// reads them, naming each refusal.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssuesAssignParams {
    pub issue_id: String,
    pub assignee: Value,
    /// Extra instruction delivered under the issue. Not stored on the issue:
    /// the body is the issue, and a hand-off note belongs in the conversation
    /// it was said in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One agent starts or stops watching one issue.
///
/// The wire verb names the agent because it is the USER's — a person on the
/// board may subscribe any agent of the issue's project. A tool cannot: it
/// forces the caller, the way it cannot sign a comment as somebody else.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssuesTrackParams {
    pub issue_id: String,
    pub agent_id: String,
}

/// How far the user has read one issue.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssuesReadThroughParams {
    pub issue_id: String,
    /// The last event the user has seen. Never moved backwards.
    pub event_id: String,
}

/// What one agent is on.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssuesForAgentParams {
    pub agent_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct IssuesCloseParams {
    pub issue_id: String,
    /// One line on why, kept on the `closed` event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// --------------------------------------------------------------- results ---

/// What an issue is about, in the repository and in Build.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueLinks {
    pub workspace_ids: Vec<String>,
    pub branches: Vec<String>,
    pub commits: Vec<String>,
    pub conversation_ids: Vec<String>,
    /// `null` for an issue under nothing.
    pub parent_issue_id: Option<String>,
}

/// One issue, as every tracker verb answers it.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueView {
    pub id: String,
    pub project_id: String,
    /// Per-project and sequential, for `#12`.
    pub number: u64,
    pub title: String,
    pub body: String,
    /// `open` or `closed`.
    pub state: String,
    /// The column slug — one of `issues.columns`' ids.
    pub status: String,
    pub labels: Vec<String>,
    /// `none`, `low`, `medium`, `high` or `urgent`.
    pub priority: String,
    /// `null` when nobody holds it.
    pub assignee: Option<Value>,
    pub links: IssueLinks,
    /// The agents watching this issue. Always present; `[]` for an issue
    /// nobody watches, so a client tells "nobody" from "this bridge is too old
    /// to answer it".
    pub trackers: Vec<String>,
    /// The files filed with it. Always present for the same reason `trackers`
    /// is: a client that sent files and got no key back is looking at a bridge
    /// that dropped them, and `[]` says it carried none.
    #[serde(default)]
    pub attachments: Vec<IssueAttachment>,
    /// Whether the USER is watching this issue — the inbox's flag, not
    /// `trackers`, which is the agents'. Absent means no, so an issue nobody
    /// watches says nothing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub watched: bool,
    /// The last event on this issue the user has read. `null` when they have
    /// read none of it, which is how an issue arrives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_through: Option<String>,
    /// The last event the user put down without reading — Done until the next
    /// one. `null` unless they have dismissed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dismissed_through: Option<String>,
    /// `{"kind":"user"}` or `{"kind":"agent","agent_id":…}`.
    pub created_by: Value,
    pub created_at: String,
    pub updated_at: String,
    /// `null` while the issue is open.
    pub closed_at: Option<String>,
}

/// One attachment as every ANSWER carries it — what `issues.attach` says it
/// stored, and what an issue or a comment says it holds.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueAttachment {
    pub name: String,
    /// Where the bytes are. Opaque to a browser, which reads them back with
    /// `issues.attachment`; an agent opens it.
    pub path: String,
    pub mime: String,
    pub size: u64,
}

/// One attachment's bytes, for a surface that cannot reach the disk.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueAttachmentBytes {
    pub path: String,
    pub size: u64,
    pub mime: String,
    pub content_b64: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct IssuesList {
    pub project_id: String,
    pub issues: Vec<IssueView>,
}

/// What every mutating verb answers: the issue as it now stands.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueAnswer {
    pub issue: IssueView,
}

/// One comment on one issue.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueCommentView {
    pub id: String,
    pub issue_id: String,
    pub author: Value,
    pub body: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub mentions_user: bool,
    pub refs: Vec<crate::thread::ThreadLink>,
    #[serde(default)]
    pub attachments: Vec<IssueAttachment>,
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
pub struct IssueComment {
    pub issue: IssueView,
    pub comment: IssueCommentView,
}

/// Where an assignment put the work, or `null` when it dispatched nothing —
/// which is `{"kind":"user"}` and unassignment.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueDispatch {
    /// Which of the five kinds was asked for.
    pub kind: String,
    /// The workspace the work runs in, when the dispatch names or makes one.
    pub workspace_id: Option<String>,
    /// The conversation owner the issue was delivered into.
    pub entity_id: String,
    /// The agent now holding the issue.
    pub agent_id: String,
    /// The receipt for the turn the delivery queued.
    pub operation_id: Value,
}

/// What `issues.assign` answers: the issue, and where the work went.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueAssigned {
    pub issue: IssueView,
    pub dispatch: Option<IssueDispatch>,
}

/// One timeline entry: the record itself, with `type` naming which it is.
///
/// Untyped past the discriminator on purpose. A comment and an event have
/// different fields and an event's `payload` is open by design — it says what
/// its own kind needs — so pinning the union here would mean one struct per
/// event kind and a fixture per struct, to describe something whose whole point
/// is that a client reads `type` and then reads the record.
pub type IssueTimelineEntry = Value;

#[derive(Debug, Deserialize, Serialize)]
pub struct IssueDetail {
    pub issue: IssueView,
    pub timeline: Vec<IssueTimelineEntry>,
}

/// One issue as a list somebody scans shows it: enough to recognise and to
/// order by, and not the body.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueDigest {
    pub issue_id: String,
    pub number: u64,
    pub title: String,
    pub state: String,
    pub status: String,
    pub updated_at: String,
}

/// What one agent holds and what it watches. An assigned issue is in both:
/// the two questions are different.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssuesForAgent {
    pub agent_id: String,
    pub assigned: Vec<IssueDigest>,
    pub tracking: Vec<IssueDigest>,
}

/// One kanban column: the slug that is stored, the name that is shown.
#[derive(Debug, Deserialize, Serialize)]
pub struct IssueColumn {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct IssueColumns {
    pub project_id: String,
    pub columns: Vec<IssueColumn>,
}

// ----------------------------------------------------------------- codes ---

/// The request was legible and the issue's own state said no.
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
const INVALID: [&str; 18] = [
    "unknown assignee kind:",
    "assignee: name a kind",
    "assignee new_agent: name a",
    "unknown status:",
    "unknown state:",
    "unknown priority:",
    "unknown assignee filter:",
    "issues.link: name a",
    "cannot be empty",
    "exceeds",
    "must be a string",
    "must be an array",
    "labels must be strings",
    "an issue cannot be its own parent",
    "an issue carries at most",
    // Attachments: a file too big, an empty one, and a list that is not one.
    "attachments must be an array",
    "each attachment needs a path",
    "attachment is empty",
];

/// The store is not there, or a reference did not survive its fencing. Neither
/// is the caller's fault in a way retrying would fix, and neither is a missing
/// entity.
const UNAVAILABLE: [&str; 1] = ["issues need a durable store"];

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

fn issues_list(
    app: &mut AppState,
    params: IssuesListParams,
) -> Result<Answer<IssuesList>, ApiError> {
    answer(app.issues_list(&params.wire())).map_err(refine)
}

fn issues_get(app: &mut AppState, params: IssueIdParams) -> Result<Answer<IssueDetail>, ApiError> {
    answer(app.issues_get(&params.wire())).map_err(refine)
}

fn issues_create(
    app: &mut AppState,
    params: IssuesCreateParams,
) -> Result<Answer<IssueAssigned>, ApiError> {
    answer(
        app.issues_create(&params.wire())
            .map(super::deferral_placeholder),
    )
    .map_err(refine)
}

fn issues_attach(
    app: &mut AppState,
    params: IssuesAttachParams,
) -> Result<Answer<IssueAttachment>, ApiError> {
    answer(app.issues_attach(&params.wire())).map_err(refine)
}

fn issues_attachment(
    app: &mut AppState,
    params: IssuesAttachmentParams,
) -> Result<Answer<IssueAttachmentBytes>, ApiError> {
    answer(app.issues_attachment(&params.wire())).map_err(refine)
}

fn issues_update(
    app: &mut AppState,
    params: IssuesUpdateParams,
) -> Result<Answer<IssueAnswer>, ApiError> {
    answer(app.issues_update(&params.wire())).map_err(refine)
}

fn issues_comment(
    app: &mut AppState,
    params: IssuesCommentParams,
) -> Result<Answer<IssueComment>, ApiError> {
    answer(app.issues_comment(&params.wire())).map_err(refine)
}

fn issues_assign(
    app: &mut AppState,
    params: IssuesAssignParams,
) -> Result<Answer<IssueAssigned>, ApiError> {
    answer(
        app.issues_assign(&params.wire())
            .map(super::deferral_placeholder),
    )
    .map_err(refine)
}

fn issues_link(
    app: &mut AppState,
    params: IssuesLinkParams,
) -> Result<Answer<IssueAnswer>, ApiError> {
    answer(app.issues_link(&params.wire())).map_err(refine)
}

fn issues_close(
    app: &mut AppState,
    params: IssuesCloseParams,
) -> Result<Answer<IssueAnswer>, ApiError> {
    answer(app.issues_close(&params.wire())).map_err(refine)
}

fn issues_reopen(
    app: &mut AppState,
    params: IssueIdParams,
) -> Result<Answer<IssueAnswer>, ApiError> {
    answer(app.issues_reopen(&params.wire())).map_err(refine)
}

fn issues_watch(
    app: &mut AppState,
    params: IssueIdParams,
) -> Result<Answer<IssueAnswer>, ApiError> {
    answer(app.issues_watch(&params.wire())).map_err(refine)
}

fn issues_dismiss(
    app: &mut AppState,
    params: IssueIdParams,
) -> Result<Answer<IssueAnswer>, ApiError> {
    answer(app.issues_dismiss(&params.wire())).map_err(refine)
}

fn issues_unwatch(
    app: &mut AppState,
    params: IssueIdParams,
) -> Result<Answer<IssueAnswer>, ApiError> {
    answer(app.issues_unwatch(&params.wire())).map_err(refine)
}

fn issues_read_through(
    app: &mut AppState,
    params: IssuesReadThroughParams,
) -> Result<Answer<IssueAnswer>, ApiError> {
    answer(app.issues_read_through(&params.wire())).map_err(refine)
}

fn issues_track(
    app: &mut AppState,
    params: IssuesTrackParams,
) -> Result<Answer<IssueAnswer>, ApiError> {
    answer(app.issues_track(&params.wire())).map_err(refine)
}

fn issues_untrack(
    app: &mut AppState,
    params: IssuesTrackParams,
) -> Result<Answer<IssueAnswer>, ApiError> {
    answer(app.issues_untrack(&params.wire())).map_err(refine)
}

fn issues_for_agent(
    app: &mut AppState,
    params: IssuesForAgentParams,
) -> Result<Answer<IssuesForAgent>, ApiError> {
    answer(app.issues_for_agent(&params.wire())).map_err(refine)
}

fn issues_columns(
    app: &mut AppState,
    params: ProjectIdParams,
) -> Result<Answer<IssueColumns>, ApiError> {
    answer(app.issues_columns(&params.wire())).map_err(refine)
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
            assert!(method.starts_with("issues."), "{method}");
            assert!(!method.starts_with("issue."), "{method}");
        }
    }
}
