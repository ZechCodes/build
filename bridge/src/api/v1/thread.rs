//! The thread family: `thread.*` and the roster verbs `agent.add`,
//! `agent.choose`, `agent.remove`. (`agent.attach` and
//! `agent.start` need the session or the shared handle and stay legacy.)
//!
//! Same shape as `git.rs`: typed params in, the existing implementation
//! underneath, and the shape that implementation answers in named as the
//! result type. Nothing about how a conversation is read or posted to changed
//! here.
//!
//! Two things this family has that the git one did not:
//!
//! * **Refusals with a name.** A conversation verb refuses for reasons the
//!   generic [`ApiError::classify`] cannot see — a `choice_revision` or a
//!   `conversation_id` that went stale while the request was in flight is a
//!   `conflict` whose `details.current` tells the client what to resend
//!   against, and an attachment or an operation that is not here is a
//!   `not_found`. [`refine`] re-codes those; everything else keeps whatever
//!   `classify` named, `internal` included.
//! * **Domain types reused as wire types.** `ThreadItem`, `SessionLineage`,
//!   `AgentIdentity` and `CompletionReport` already derive both halves of
//!   serde and ARE the wire shape, so the facade names them rather than
//!   copying them. Where the domain type serialises only one way
//!   ([`crate::thread::ActivityDigest`], [`crate::thread::LastToolCall`]) the
//!   wire mirror lives here.

use super::{answer, Answer, Handler, WireParams};
use crate::api::ApiError;
use crate::app::AppState;
use crate::thread::{
    AgentIdentity, CompletionReport, MessageAnchor, SessionLineage, ThreadItem, ViewingContext,
};
use crate::{v1_method, v1_methods};
use serde::{Deserialize, Serialize};
use serde_json::Number;
use std::collections::BTreeMap;

/// The verbs this family serves.
pub fn methods() -> &'static [(&'static str, Handler)] {
    v1_methods![
        v1_method!("thread.page", thread_page, ThreadPageParams, ThreadPage),
        v1_method!(
            "thread.revision",
            thread_revision,
            ThreadRevisionParams,
            ThreadRevision
        ),
        v1_method!(
            "thread.activity",
            thread_activity,
            ThreadActivityParams,
            ActivityPage
        ),
        v1_method!("thread.post", thread_post, ThreadPostParams, PostReceipt),
        v1_method!(
            "thread.operation",
            thread_operation,
            ThreadOperationParams,
            OperationReceipt
        ),
        v1_method!(
            "thread.attach",
            thread_attach,
            ThreadAttachParams,
            AttachmentUpload
        ),
        v1_method!(
            "thread.attachment",
            thread_attachment,
            ThreadAttachmentParams,
            AttachmentContent
        ),
        v1_method!("agent.add", agent_add, AgentAddParams, AgentAdded),
        v1_method!("agent.choose", agent_choose, AgentChooseParams, AgentChoice),
        v1_method!("agent.remove", agent_remove, AgentRemoveParams, AgentRoster),
        v1_method!(
            "conversation.watch",
            conversation_watch,
            AgentRemoveParams,
            ConversationWatch
        ),
        v1_method!(
            "conversation.unwatch",
            conversation_unwatch,
            AgentRemoveParams,
            ConversationWatch
        ),
        v1_method!(
            "conversation.reset",
            conversation_reset,
            ConversationResetParams,
            ConversationReset
        ),
        v1_method!(
            "conversation.settings",
            conversation_settings,
            ConversationSettingsParams,
            ConversationSettings
        ),
    ]
}

// ---------------------------------------------------------------- params ---

/// A page size as a client may spell it: the integer it is, or the string a
/// URL or a number-stringifying encoder leaves behind. Carried rather than
/// coerced, because the clamp and the fallback belong to the implementation
/// that has always owned them.
#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum PageLimit {
    Count(Number),
    Named(String),
}

/// Who owns the conversation a read is about. `entity_id` is the canonical
/// name; a detail surface's own id is accepted too, so a client paging the
/// view it is looking at does not have to rename the id it holds. Naming none
/// of them is `missing required param: entity_id`, from the implementation.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ConversationOwner {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

/// Which of the owner's conversations. Absent `agent_id` is the primary one;
/// `conversation_id` is a guard, not an address — a binding captured before
/// the request crossed the network is refused rather than followed.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ConversationAddress {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ThreadPageParams {
    #[serde(flatten)]
    pub owner: ConversationOwner,
    #[serde(flatten)]
    pub address: ConversationAddress,
    /// The page walks backward from here; absent means the newest page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_sequence: Option<u64>,
    /// The page walks FORWARD from here instead, oldest first — the read a
    /// client holding a cached conversation makes. Mutually exclusive with
    /// `before_sequence`; both is `invalid_params`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_sequence: Option<u64>,
    /// With `after_sequence`, return the newest page of that delta rather than
    /// its oldest page. Omitted or false keeps the original forward walk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newest: Option<bool>,
    /// Clamped server-side to one page's worth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<PageLimit>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ThreadActivityParams {
    #[serde(flatten)]
    pub owner: ConversationOwner,
    #[serde(flatten)]
    pub address: ConversationAddress,
    /// The span the digest named, inclusive at both ends.
    pub from_sequence: u64,
    pub through_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<PageLimit>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ThreadRevisionParams {
    pub entity_id: String,
    pub revision_id: String,
    #[serde(flatten)]
    pub address: ConversationAddress,
}

/// One message of a batched post.
#[derive(Debug, Deserialize, Serialize)]
pub struct PostMessage {
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<MessageAnchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewing_context: Option<ViewingContext>,
}

/// A file already uploaded through `thread.attach`, referenced by the path
/// that upload answered with.
#[derive(Debug, Deserialize, Serialize)]
pub struct PostAttachment {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// The reviewer pressing one of an agent's suggested actions.
#[derive(Debug, Deserialize, Serialize)]
pub struct OptionReply {
    pub message_id: String,
    pub option_ids: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ThreadPostParams {
    pub entity_id: String,
    #[serde(flatten)]
    pub address: ConversationAddress,
    /// Names the durable mutation, so a retry answers the first receipt
    /// instead of posting twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    /// The agent's choice revision this post was composed against; a stale one
    /// is a `conflict`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice_revision: Option<u64>,
    /// The single message: `body` (+ optional `anchor`), or `messages` for a
    /// batch. A post carrying attachments may have neither.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<MessageAnchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewing_context: Option<ViewingContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<Vec<PostMessage>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<PostAttachment>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_reply: Option<OptionReply>,
    /// Stop the turn in flight before saying this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupt: Option<bool>,
    /// How much conversation the answer carries back. Read projection only:
    /// neither field is part of the request an `operation_id` is bound to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_limit: Option<PageLimit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_after_sequence: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ThreadOperationParams {
    pub entity_id: String,
    #[serde(flatten)]
    pub address: ConversationAddress,
    pub operation_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ThreadAttachParams {
    pub entity_id: String,
    #[serde(flatten)]
    pub address: ConversationAddress,
    pub filename: String,
    pub content_b64: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ThreadAttachmentParams {
    pub entity_id: String,
    #[serde(flatten)]
    pub address: ConversationAddress,
    pub path: String,
    /// Where the piece starts (1.30). Absent is the start of the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    /// How many bytes to read, capped at 5 MiB (1.30). When both range fields
    /// are absent, the legacy whole-file answer is preserved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
}

/// What an agent runs on. Absent everywhere means the entity's own choice on
/// `agent.add`, and no change on `agent.choose`.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ModelChoiceParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AgentAddParams {
    pub entity_id: String,
    /// The client's name for this creation, so a retried add answers the agent
    /// the first one made rather than making a second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creation_id: Option<String>,
    /// What to CALL the agent: one or two meaningful words, replacing "Agent
    /// 1" everywhere it is drawn. Optional — an agent nobody names is asked to
    /// name itself the first time the user writes to it — and refused when
    /// another agent on the conversation already has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What the agent is to BE. The device has a model declared for each
    /// role; anything named in `choice` is laid over that answer field by
    /// field, and the effort is always the caller's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// How much direction it should need, when that matters to the caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    /// An AGENT asked for this one, not the person. Its conversation is that
    /// agent's business and stays out of the user's inbox (spec: Tasks →
    /// Watching) unless `notify_user` says otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub made_by_agent: Option<bool>,
    /// Put this conversation in the user's inbox whoever asked for it — the
    /// agent saying "you will want to see this one".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_user: Option<bool>,
    #[serde(flatten)]
    pub choice: ModelChoiceParams,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AgentChooseParams {
    pub entity_id: String,
    #[serde(flatten)]
    pub address: ConversationAddress,
    /// An agent is locked to the harness it was created on, so naming a
    /// provider here is refused: model and effort only.
    #[serde(flatten)]
    pub choice: ModelChoiceParams,
    /// The revision this menu was drawn from; a stale one is a `conflict`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_choice_revision: Option<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AgentRemoveParams {
    pub entity_id: String,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

/// `conversation.settings`. `max_context_tokens` must be named: a number of
/// tokens (0 never compacts), or `null` to follow the device again.
#[derive(Debug, Deserialize, Serialize)]
pub struct ConversationSettingsParams {
    pub entity_id: String,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(
        default,
        deserialize_with = "super::board::named",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_context_tokens: super::board::Named<u64>,
}

/// Clear one exact generation, keeping the durable agent and owner identity.
#[derive(Debug, Deserialize, Serialize)]
pub struct ConversationResetParams {
    pub project_id: String,
    pub entity_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub expected_thread_id: String,
    #[serde(flatten)]
    pub choice: ModelChoiceParams,
    #[serde(
        default,
        deserialize_with = "super::board::named",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_context_tokens: super::board::Named<u64>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ConversationReset {
    pub entity_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub previous_thread_id: String,
    pub thread_id: String,
    pub thread_generation_revision: u64,
    pub agent: AgentDigest,
    pub thread: ThreadPage,
}

// --------------------------------------------------------------- results ---

/// The newest tool call of a folded run, as the digest prints it.
#[derive(Debug, Deserialize, Serialize)]
pub struct LastToolCall {
    pub sequence: u64,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// `ok`, `error`, or `unanswered`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

/// What one folded run of activity amounts to. The counts are exact over
/// `[from_sequence, through_sequence]` whatever the page shipped of it, which
/// is what `thread.activity` is then asked for.
#[derive(Debug, Deserialize, Serialize)]
pub struct ActivityDigest {
    pub from_sequence: u64,
    pub through_sequence: u64,
    pub tool_calls: u64,
    pub rows: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_tool_call: Option<LastToolCall>,
}

/// One artifact revision, without the snapshot body `thread.revision` fetches.
#[derive(Debug, Deserialize, Serialize)]
pub struct RevisionSummary {
    pub id: String,
    /// `plan` or `diff`.
    pub artifact: String,
    pub content_hash: String,
    pub created_at: String,
    pub snapshot_available: bool,
}

/// One page of a conversation, walking backward.
#[derive(Debug, Deserialize, Serialize)]
pub struct ThreadPage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_generation_revision: Option<u64>,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    pub agent: AgentIdentity,
    pub sessions: Vec<SessionLineage>,
    pub items: Vec<ThreadItem>,
    /// One per run of activity this page folded away.
    pub activity_digests: Vec<ActivityDigest>,
    pub revisions: Vec<RevisionSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_completion: Option<CompletionReport>,
    pub thread_total: u64,
    pub thread_last_sequence: u64,
    /// The seek for the next page up; absent when the page is empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_sequence: Option<u64>,
    pub has_more: bool,
}

/// One page of one folded run's activity. No digests: the client asked for
/// this span because it already holds the digest that named it.
#[derive(Debug, Deserialize, Serialize)]
pub struct ActivityPage {
    pub items: Vec<ThreadItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_sequence: Option<u64>,
    pub has_more: bool,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ThreadRevision {
    pub revision_id: String,
    /// `plan` or `diff`.
    pub artifact: String,
    pub created_at: String,
    pub contents: String,
}

/// The durable record of one `thread.post`, as `thread.operation` reads it
/// back. `status` is `queued`, `claimed`, `delivered` or `uncertain`.
#[derive(Debug, Deserialize, Serialize)]
pub struct OperationReceipt {
    pub operation_id: String,
    pub method: String,
    pub entity_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub choice_revision: u64,
    pub posted_sequence: u64,
    pub message_start_sequence: u64,
    pub status: String,
    /// A known terminal delivery failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_error: Option<String>,
}

/// The entity view a post answers with — the same plan/run detail the
/// lifecycle family serves, repainted after the append. Carried as fields
/// rather than typed here: the shape belongs to the task and run views, and
/// naming it twice would be two things to keep equal.
pub type EntityView = BTreeMap<String, serde_json::Value>;

/// What `thread.post` answers: the receipt fields beside the repainted entity
/// view. A post with no `operation_id` carries no receipt, and a post that
/// deferred its work carries neither (the drain answers).
#[derive(Debug, Deserialize, Serialize)]
pub struct PostReceipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    /// The receipt's `status`, renamed where it rides a view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice_revision: Option<u64>,
    /// Where the reviewer's words landed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub posted_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_start_sequence: Option<u64>,
    #[serde(flatten)]
    pub view: EntityView,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AttachmentUpload {
    pub name: String,
    /// Where a `thread.post` references the file from.
    pub path: String,
    pub mime: String,
    pub size: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AttachmentContent {
    pub path: String,
    pub size: u64,
    pub mime: String,
    /// The first byte carried by this answer; absent on bridges before 1.30.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    pub content_b64: String,
}

/// How long the agent has been working, when it is.
#[derive(Debug, Deserialize, Serialize)]
pub struct WorkingTime {
    pub since: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<u64>,
}

/// Whatever the live session is showing, when the caller asked for detail.
/// Session-owned and open by construction, so it is carried rather than typed.
pub type AgentSurfaces = BTreeMap<String, serde_json::Value>;

/// One bubble of the agent rail: who the agent is, what it runs on, whether it
/// is live, and how much of its conversation is waiting for the human.
#[derive(Debug, Deserialize, Serialize)]
pub struct AgentDigest {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_generation_revision: Option<u64>,
    pub conversation_id: String,
    /// 1-based, stable for the agent's life.
    pub ordinal: u64,
    pub provider: String,
    /// What the NEXT start spends; empty for the harness default.
    pub model: String,
    pub effort: String,
    /// What the live session actually runs, falling back to `model`.
    pub active_model: String,
    pub state: String,
    pub unread_count: u64,
    /// The newest attention item's kind; absent exactly when nothing is
    /// unread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unread_reason: Option<String>,
    pub read_through_sequence: u64,
    pub working: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_time: Option<WorkingTime>,
    pub choice_revision: u64,
    /// Whether the rail offers this agent a basement.
    pub has_terminal: bool,
    /// Whether the composer offers "Interrupt & send".
    pub can_interrupt: bool,
    /// Why the last turn queued for this agent never reached a harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_error: Option<String>,
    pub created_at: String,
    /// What to CALL this conversation in a list: the agent's own topic, or —
    /// until it sets one — the first line the human opened with. Null for a
    /// conversation nothing has been said in yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The agent's own word for what this conversation is about, which the
    /// header wears in place of the harness name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// What to CALL this agent, everywhere it used to be "Agent 1". Absent
    /// until somebody names it, and the ordinal is the fallback for exactly
    /// that long.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The agent whose Build MCP call made this one (since 3.1.0, announced
    /// as `agents.createdBy`). Absent for an agent the user made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    /// Tokens in context on the agent's last reported turn. Absent until a
    /// harness reports one, and again once a compaction has been asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_context_tokens: Option<u64>,
    /// When that reading was recorded, RFC3339; absent exactly when it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_context_at: Option<String>,
    /// Cache-read tokens the agent's current session has spent in all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_cache_read_tokens: Option<u64>,
    /// The conversation's own compaction threshold; absent follows the
    /// device's, 0 never compacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<u64>,
    /// The threshold in effect: the conversation's, else the device's. 0 is
    /// never.
    pub compact_at_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surfaces: Option<AgentSurfaces>,
}

/// One conversation's bounded message sessions on a workspace or project
/// list row. The list lets the inbox pool conversations without fetching them.
#[derive(Debug, Deserialize, Serialize)]
pub struct ConversationActivity {
    pub conversation_id: String,
}

/// Whether one conversation is in the user's inbox.
#[derive(Debug, Deserialize, Serialize)]
pub struct ConversationWatch {
    pub agent_id: String,
    pub watched: bool,
}

/// `conversation.settings`: the context size one conversation compacts at.
#[derive(Debug, Deserialize, Serialize)]
pub struct ConversationSettings {
    pub agent_id: String,
    /// The conversation's own threshold as stored; absent follows the device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<u64>,
    /// The threshold in effect. 0 is never.
    pub compact_at_tokens: u64,
}

/// `agent.add`. `created` is false when a `creation_id` matched an agent this
/// entity already has — the retry, answered with what the first call made.
#[derive(Debug, Deserialize, Serialize)]
pub struct AgentAdded {
    pub entity_id: String,
    pub created: bool,
    pub agent: AgentDigest,
    /// How much direction the model this role resolved to wants, when the
    /// caller asked for a role. Absent when it named the model itself, or
    /// when the device has declared nothing for that role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    /// The same thing said as the instruction it is, for the agent about to
    /// write this one's brief.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AgentChoice {
    pub entity_id: String,
    pub agent_id: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub choice_revision: u64,
}

/// The entity's agents in rail order: what `agent.remove` answers with,
/// naming the agent that went.
#[derive(Debug, Deserialize, Serialize)]
pub struct AgentRoster {
    pub entity_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    pub agents: Vec<AgentDigest>,
}

// -------------------------------------------------------------- refusals ---

/// The current value a stale-revision refusal names, as `details.current`.
/// Both spellings the implementations use end in the value, so the tail of
/// the message is the value.
fn stale_current(message: &str) -> Option<&str> {
    message.rsplit(' ').next().filter(|tail| !tail.is_empty())
}

/// Whether a refusal is about a request naming something that is not here.
fn names_nothing(message: &str) -> bool {
    message.contains("unknown ")
        || message.contains("does not exist")
        || message.starts_with("not an attachment on this conversation")
        // "agent agent-2 is not on run-7": the entity is real and the agent is
        // not one of its own, which is the same answer as never having heard
        // of it — not a fault of this bridge.
        || message.contains(" is not on ")
}

/// Whether a refusal is about the request itself rather than the state it met.
fn names_bad_input(message: &str) -> bool {
    message.starts_with("invalid ")
        || message.contains("operation_id exceeds")
        || message.contains("operation_id contains")
        || message.contains("creation_id is too long")
        || message.contains("is empty")
        || message.contains("exceeds 32000 bytes")
        || message.contains("requires a non-empty body")
}

/// Name the code a thread refusal deserves, where the message makes it
/// obvious. Everything [`ApiError::classify`] already named correctly is left
/// alone, so this only sees what came back as `internal`.
pub(super) fn refine(error: ApiError) -> ApiError {
    if !matches!(error, ApiError::Internal { .. }) {
        return error;
    }
    let message = error.message().to_string();
    if message.contains("session delivery is in progress") {
        return ApiError::busy(message);
    }
    if message.contains("conversation does not belong to project") {
        return ApiError::invalid_params(message);
    }
    if message.contains("stale ") {
        let details = stale_current(&message).map(|current| match current.parse::<u64>() {
            Ok(revision) => serde_json::json!({ "current": revision }),
            Err(_) => serde_json::json!({ "current": current }),
        });
        return ApiError::conflict(message, details);
    }
    if names_nothing(&message) {
        return ApiError::not_found(message);
    }
    if names_bad_input(&message) {
        return ApiError::invalid_params(message);
    }
    ApiError::internal(message)
}

// -------------------------------------------------------------- handlers ---

fn conversation_reset(
    app: &mut AppState,
    params: ConversationResetParams,
) -> Result<Answer<ConversationReset>, ApiError> {
    answer(app.conversation_reset(&params.wire())).map_err(refine)
}

fn thread_page(
    app: &mut AppState,
    params: ThreadPageParams,
) -> Result<Answer<ThreadPage>, ApiError> {
    answer(app.thread_page(&params.wire())).map_err(refine)
}

fn thread_revision(
    app: &mut AppState,
    params: ThreadRevisionParams,
) -> Result<Answer<ThreadRevision>, ApiError> {
    answer(app.thread_revision(&params.wire())).map_err(refine)
}

fn thread_activity(
    app: &mut AppState,
    params: ThreadActivityParams,
) -> Result<Answer<ActivityPage>, ApiError> {
    answer(app.thread_activity(&params.wire())).map_err(refine)
}

fn thread_post(
    app: &mut AppState,
    params: ThreadPostParams,
) -> Result<Answer<PostReceipt>, ApiError> {
    answer(app.thread_post(&params.wire())).map_err(refine)
}

fn thread_operation(
    app: &mut AppState,
    params: ThreadOperationParams,
) -> Result<Answer<OperationReceipt>, ApiError> {
    answer(app.thread_operation(&params.wire())).map_err(refine)
}

fn thread_attach(
    app: &mut AppState,
    params: ThreadAttachParams,
) -> Result<Answer<AttachmentUpload>, ApiError> {
    answer(app.thread_attach(&params.wire())).map_err(refine)
}

fn thread_attachment(
    app: &mut AppState,
    params: ThreadAttachmentParams,
) -> Result<Answer<AttachmentContent>, ApiError> {
    answer(app.thread_attachment(&params.wire())).map_err(refine)
}

fn agent_add(app: &mut AppState, params: AgentAddParams) -> Result<Answer<AgentAdded>, ApiError> {
    answer(app.agent_add(&params.wire())).map_err(refine)
}

fn agent_choose(
    app: &mut AppState,
    params: AgentChooseParams,
) -> Result<Answer<AgentChoice>, ApiError> {
    answer(app.agent_choose(&params.wire())).map_err(refine)
}

fn guard_conversation_mutation(
    app: &AppState,
    owner: &str,
    params: &serde_json::Value,
) -> Result<(), ApiError> {
    app.guard_conversation_mutation_params(owner, params)
        .map_err(ApiError::classify)
        .map_err(refine)
}

fn conversation_watch(
    app: &mut AppState,
    params: AgentRemoveParams,
) -> Result<Answer<ConversationWatch>, ApiError> {
    guard_conversation_mutation(app, &params.entity_id, &params.wire())?;
    answer(app.set_conversation_watched(&params.entity_id, &params.agent_id, true)).map_err(refine)
}

fn conversation_unwatch(
    app: &mut AppState,
    params: AgentRemoveParams,
) -> Result<Answer<ConversationWatch>, ApiError> {
    guard_conversation_mutation(app, &params.entity_id, &params.wire())?;
    answer(app.set_conversation_watched(&params.entity_id, &params.agent_id, false)).map_err(refine)
}

fn conversation_settings(
    app: &mut AppState,
    params: ConversationSettingsParams,
) -> Result<Answer<ConversationSettings>, ApiError> {
    guard_conversation_mutation(app, &params.entity_id, &params.wire())?;
    let Some(max_context_tokens) = params.max_context_tokens else {
        return Err(ApiError::invalid_params(
            "missing required param: max_context_tokens (a number of tokens, or null for the device's)",
        ));
    };
    answer(app.set_conversation_context_limit(
        &params.entity_id,
        &params.agent_id,
        max_context_tokens,
    ))
    .map_err(refine)
}

fn agent_remove(
    app: &mut AppState,
    params: AgentRemoveParams,
) -> Result<Answer<AgentRoster>, ApiError> {
    answer(app.agent_remove(&params.wire())).map_err(refine)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::api::v1::testing::fixture;

    fn round_trips(method: &str) {
        crate::api::v1::testing::fixture_round_trips(methods(), method);
    }

    #[test]
    fn the_thread_page_fixture_round_trips() {
        round_trips("thread.page");
    }

    #[test]
    fn the_thread_revision_fixture_round_trips() {
        round_trips("thread.revision");
    }

    #[test]
    fn the_thread_activity_fixture_round_trips() {
        round_trips("thread.activity");
    }

    #[test]
    fn the_thread_post_fixture_round_trips() {
        round_trips("thread.post");
    }

    #[test]
    fn the_thread_operation_fixture_round_trips() {
        round_trips("thread.operation");
    }

    #[test]
    fn the_thread_attach_fixture_round_trips() {
        round_trips("thread.attach");
    }

    #[test]
    fn the_thread_attachment_fixture_round_trips() {
        round_trips("thread.attachment");
    }

    #[test]
    fn the_agent_add_fixture_round_trips() {
        round_trips("agent.add");
    }

    #[test]
    fn the_agent_choose_fixture_round_trips() {
        round_trips("agent.choose");
    }

    #[test]
    fn the_agent_remove_fixture_round_trips() {
        round_trips("agent.remove");
    }

    #[test]
    fn the_conversation_settings_fixture_round_trips() {
        round_trips("conversation.settings");
    }

    /// `fixtures/chat_operation_contract.json` folded in here, under
    /// `operations`: one file, two consumers (this bridge and
    /// `spa/test/chatRepository.test.js`), and the receipt vocabulary it names
    /// is held to the result beside it.
    #[test]
    fn the_thread_post_fixture_carries_the_operation_contract() {
        let posted = fixture("thread.post");
        let operations = &posted["operations"];
        assert_eq!(operations["post_method"], "thread.post");
        assert_eq!(operations["status_method"], "thread.operation");
        assert_eq!(operations["operation_id"]["max_bytes"], 128);
        assert_eq!(
            operations["statuses"],
            serde_json::json!(["queued", "claimed", "delivered", "uncertain"])
        );
        for field in operations["post_receipt_fields"]
            .as_array()
            .expect("the contract lists the receipt fields")
        {
            let field = field.as_str().expect("a field name");
            assert!(
                posted["result"].get(field).is_some(),
                "the thread.post result does not carry {field}"
            );
        }
        for field in operations["status_receipt_fields"]
            .as_array()
            .expect("the contract lists the status fields")
        {
            let field = field.as_str().expect("a field name");
            let receipt = fixture("thread.operation");
            assert!(
                receipt["result"].get(field).is_some(),
                "the thread.operation result does not carry {field}"
            );
        }
    }

    #[test]
    fn a_stale_revision_is_a_conflict_naming_what_to_resend_against() {
        let refused = refine(ApiError::internal(
            "stale choice_revision 3; agent agent-1 is at 4",
        ));
        assert_eq!(refused.code(), "conflict");
        assert_eq!(
            refused.details(),
            Some(&serde_json::json!({ "current": 4 }))
        );
        let bound = refine(ApiError::internal(
            "agent.choose: stale conversation_id conv-old; agent agent-1 is bound to conv-2",
        ));
        assert_eq!(bound.code(), "conflict");
        assert_eq!(
            bound.details(),
            Some(&serde_json::json!({ "current": "conv-2" }))
        );
        assert!(!bound.retryable());
    }

    #[test]
    fn a_missing_entity_is_not_found_and_a_bad_body_is_invalid_params() {
        assert_eq!(
            refine(ApiError::internal("agent.add: unknown entity run-9")).code(),
            "not_found"
        );
        assert_eq!(
            refine(ApiError::internal("unknown operation_id: op-9")).code(),
            "not_found"
        );
        assert_eq!(
            refine(ApiError::internal(
                "not an attachment on this conversation: /etc/passwd"
            ))
            .code(),
            "not_found"
        );
        assert_eq!(
            refine(ApiError::internal("invalid viewing_context: bad")).code(),
            "invalid_params"
        );
        assert_eq!(
            refine(ApiError::internal("attachment is empty")).code(),
            "invalid_params"
        );
    }

    #[test]
    fn a_refusal_already_named_keeps_its_code() {
        assert_eq!(
            refine(ApiError::not_found("unknown id")).code(),
            "not_found"
        );
        assert_eq!(
            refine(ApiError::invalid_params(
                "missing required param: entity_id"
            ))
            .code(),
            "invalid_params"
        );
        assert_eq!(
            refine(ApiError::internal("thread.post: run is done")).code(),
            "internal"
        );
    }
}
