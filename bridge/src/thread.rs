//! Durable conversation threads paired with plans and run diffs.
//!
//! A thread belongs to a stable logical agent identity (the Build-owned plan or
//! run), while individual harness processes are recorded as session lineage.
//! Messages cost agent tokens; events and revision links do not.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Plan,
    Diff,
    /// One stage document of an issue. What a plan-doc comment anchors to: a
    /// passage of a named file, the same way a diff comment anchors to a
    /// passage of a hunk.
    Doc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentIdentity {
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Agent,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageSource {
    #[default]
    Chat,
    Completion,
}

impl MessageSource {
    fn is_chat(source: &Self) -> bool {
        *source == Self::Chat
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl MessageRole {
    pub fn as_str(self) -> &'static str {
        match self {
            MessageRole::User => "user",
            MessageRole::Agent => "agent",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageAnchor {
    pub artifact: ArtifactKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
    #[serde(default)]
    pub heading_path: Vec<String>,
    #[serde(default)]
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ThreadLink {
    File {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line_start: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line_end: Option<u32>,
    },
    /// Legacy stage reference retained for clients and records predating the
    /// Issue cutover. New lifecycle events emit `IssueStage`.
    PlanStage {
        plan_id: String,
        stage_id: String,
        path: String,
    },
    /// Canonical reference to one ordered stage-plan document owned by an Issue.
    IssueStage {
        issue_id: String,
        stage_id: String,
        path: String,
    },
    /// Legacy implementation reference retained as a wire alias.
    Run { run_id: String },
    /// Canonical implementation lineage reference, explicitly scoped to Issue.
    Implementation {
        issue_id: String,
        implementation_id: String,
    },
    /// Stable server-minted identity of a checkout.
    Worktree { worktree_id: String },
    /// Exact immutable commit boundary.
    Commit { sha: String },
    /// One verified recovery attempt/session.
    Recovery { recovery_id: String },
}

/// A file the reviewer sent with a message, already written to disk by
/// `thread.attach`. The message carries only the reference: the bytes live
/// where the agent's own tools can open them, and where a re-render costs a
/// read rather than another trip through the thread record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageAttachment {
    /// What the reviewer called it, for display.
    pub name: String,
    /// Where the agent opens it: worktree-relative under `.build/attachments/`
    /// when the conversation has a checkout, absolute when it does not.
    pub path: String,
    pub mime: String,
    pub size: u64,
}

/// What one item says about itself so a later search can find it.
///
/// Derived once, when the item is written, from the text and the links it
/// carries — never re-derived by a reader, so a search costs a scan of small
/// vectors rather than a re-parse of every body ever posted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemMetadata {
    /// Commit shas named here, lowercase, in the order they appear.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commits: Vec<String>,
    /// Worktree-relative paths that name real files in the checkout, plus the
    /// paths of links and anchors the bridge already validated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// Stage ids this item links to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stages: Vec<String>,
}

/// How many of each kind one item may claim. A message naming forty files is
/// a paste, not a reference, and the index is a pointer either way.
const MAX_METADATA_ENTRIES: usize = 20;

impl ItemMetadata {
    pub fn is_empty(&self) -> bool {
        self.commits.is_empty() && self.files.is_empty() && self.stages.is_empty()
    }

    /// Read an item's findability out of what it says and what it links to.
    fn derive(
        text: &str,
        links: &[ThreadLink],
        anchor: Option<&MessageAnchor>,
        scope: &WorktreeScope,
    ) -> ItemMetadata {
        let mut metadata = ItemMetadata::default();
        for token in prose_tokens(text) {
            if is_commit_sha(token) {
                push_unique(&mut metadata.commits, token.to_lowercase());
            } else if metadata.files.len() < MAX_METADATA_ENTRIES
                && looks_like_a_path(token)
                // Last, and only under the cap: this is the one check that
                // touches the disk, so a long paste cannot turn into a long
                // run of stat calls.
                && scope.names_a_real_file(token)
            {
                push_unique(&mut metadata.files, token.to_string());
            }
        }
        for link in links {
            match link {
                ThreadLink::File { path, .. } => push_unique(&mut metadata.files, path.clone()),
                ThreadLink::Commit { sha } => {
                    push_unique(&mut metadata.commits, sha.to_lowercase())
                }
                ThreadLink::PlanStage { stage_id, path, .. }
                | ThreadLink::IssueStage { stage_id, path, .. } => {
                    push_unique(&mut metadata.stages, stage_id.clone());
                    push_unique(&mut metadata.files, path.clone());
                }
                ThreadLink::Run { .. }
                | ThreadLink::Implementation { .. }
                | ThreadLink::Worktree { .. }
                | ThreadLink::Recovery { .. } => {}
            }
        }
        if let Some(path) = anchor.and_then(|anchor| anchor.path.as_ref()) {
            push_unique(&mut metadata.files, path.clone());
        }
        metadata
    }
}

/// Add a value the first time it is seen, up to the per-item cap.
fn push_unique(values: &mut Vec<String>, value: String) {
    if values.len() >= MAX_METADATA_ENTRIES || values.contains(&value) {
        return;
    }
    values.push(value);
}

/// The words of a body, with the punctuation people wrap them in removed:
/// backticks, quotes, brackets and sentence punctuation are how a sentence is
/// written, not part of the path or sha inside it. A leading dot IS part of a
/// path (`.build/plan/…`), so only trailing dots are stripped.
fn prose_tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split_whitespace()
        .map(|token| {
            token
                .trim_matches(|c: char| {
                    matches!(
                        c,
                        '`' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>'
                    ) || matches!(c, ',' | ';' | ':' | '!' | '?' | '*' | '|')
                })
                .trim_end_matches('.')
        })
        .filter(|token| !token.is_empty())
}

/// A commit sha: 7 to 40 hex characters carrying at least one digit.
///
/// The digit is what keeps all-hex English out of the index — "effaced" and
/// "defaced" are hex to the letter. A real sha with no digit at all is a
/// one-in-a-thousand accident, and a `Commit` link records the exact sha
/// whatever the prose looks like.
fn is_commit_sha(token: &str) -> bool {
    (7..=40).contains(&token.len())
        && token.bytes().all(|byte| byte.is_ascii_hexdigit())
        && token.bytes().any(|byte| byte.is_ascii_digit())
}

/// Whether a token is shaped like a path at all — a directory separator, or a
/// short extension. Shape alone never records a file; the checkout decides.
fn looks_like_a_path(token: &str) -> bool {
    if token.contains('/') {
        return true;
    }
    match token.rsplit_once('.') {
        Some((stem, extension)) => {
            !stem.is_empty()
                && (1..=8).contains(&extension.len())
                && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        }
        None => false,
    }
}

/// Where the conversation's checkout is, when the bridge has told the thread.
///
/// Deliberately outside equality and outside the persisted record: two
/// conversations holding the same items are the same conversation wherever
/// they happen to be checked out, and a path on this device means nothing to
/// the next one.
#[derive(Debug, Clone, Default)]
pub struct WorktreeScope(Option<PathBuf>);

impl PartialEq for WorktreeScope {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for WorktreeScope {}

impl WorktreeScope {
    /// Whether a token read out of prose names a real file inside the checkout.
    /// Without a checkout the thread cannot tell a path from a phrase, so it
    /// claims nothing.
    fn names_a_real_file(&self, candidate: &str) -> bool {
        let Some(root) = &self.0 else {
            return false;
        };
        crate::plan::is_worktree_contained_path(candidate) && root.join(candidate).is_file()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadMessage {
    pub id: String,
    pub sequence: u64,
    /// Drawn from the same counter as `sequence` and bumped whenever the
    /// message mutates in place (`seen_at`, `resolved_by_revision`), so the
    /// cursor protocol re-ships the newer copy of an already-held item.
    /// Defaults to 0 (never mutated) on records persisted before this field.
    #[serde(default)]
    pub updated_sequence: u64,
    pub role: MessageRole,
    /// Completion is metadata on an otherwise ordinary message. The timeline
    /// renders it like any other send after the separate `done` event.
    #[serde(default, skip_serializing_if = "is_false")]
    pub done: bool,
    /// Deprecated persisted shape. New completion sends use `done`; retaining
    /// this field lets older thread records deserialize without migration.
    #[serde(default, skip_serializing_if = "MessageSource::is_chat")]
    pub source: MessageSource,
    pub body: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<MessageAnchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_by_revision: Option<String>,
    /// The agent's answer to this message, when it is a plan-doc comment a
    /// revision reported addressed. Nothing else carries one, and no message
    /// written before doc comments were posts does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_reply: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<ThreadLink>,
    /// Files the reviewer sent with this message. Only a user message carries
    /// them today; the field defaults empty so records written before
    /// attachments existed load unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<MessageAttachment>,
    /// Whether the agent kept working after posting this. Only an agent message
    /// carries it, and only a progress note sets it: an ordinary reply hands the
    /// turn back, which is what ends the reviewer-facing "Working" line. Default
    /// false, so every message written before this existed reads as a handoff.
    #[serde(default, skip_serializing_if = "is_false")]
    pub still_working: bool,
    /// What this message referenced, derived when it was posted. Absent when it
    /// referenced nothing, and on every message written before findability
    /// existed.
    #[serde(default, skip_serializing_if = "ItemMetadata::is_empty")]
    pub metadata: ItemMetadata,
}

/// Where a plan-doc comment points inside a stage document: the passage the
/// reviewer selected, and where it sat when they selected it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocAnchor {
    /// The chain of enclosing heading *texts* (outermost first). Empty for a
    /// top-of-doc anchor.
    #[serde(default)]
    pub heading_path: Vec<String>,
    /// The selected passage, trimmed.
    #[serde(default)]
    pub snippet: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_start: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
}

impl DocAnchor {
    /// Whether this anchor names a passage at all. An anchor with no snippet,
    /// no headings and no lines is the document as a whole, which is what a
    /// general comment points at.
    fn names_a_passage(&self) -> bool {
        !self.snippet.trim().is_empty()
            || !self.heading_path.is_empty()
            || self.line_start.is_some()
    }
}

/// Whether a plan-doc comment is still waiting on the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocCommentState {
    Open,
    Addressed,
}

/// One reviewer comment on a stage document, as the conversation holds it.
///
/// Derived, never stored: the comment IS the anchored post, so there is one
/// place a comment can be and one place it can be deleted from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocComment {
    /// The id of the message that is this comment.
    pub id: String,
    pub stage_id: String,
    /// The stage document, worktree-relative.
    pub path: String,
    /// `None` for a comment on the document as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<DocAnchor>,
    pub body: String,
    pub state: DocCommentState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_reply: Option<String>,
    pub created_at: String,
}

/// Read one message as a plan-doc comment, or `None` when it is not one.
///
/// A comment is a reviewer message anchored to a doc and linked to the stage
/// that doc belongs to — both, so ordinary conversation about a stage is never
/// mistaken for review of it.
fn doc_comment_of(message: &ThreadMessage) -> Option<DocComment> {
    if message.role != MessageRole::User {
        return None;
    }
    let anchor = message.anchor.as_ref()?;
    if anchor.artifact != ArtifactKind::Doc {
        return None;
    }
    let (stage_id, path) = message.links.iter().find_map(|link| match link {
        ThreadLink::IssueStage { stage_id, path, .. }
        | ThreadLink::PlanStage { stage_id, path, .. } => Some((stage_id.clone(), path.clone())),
        _ => None,
    })?;
    let doc_anchor = DocAnchor {
        heading_path: anchor.heading_path.clone(),
        snippet: anchor.snippet.clone(),
        line_start: anchor.line_start,
        line_end: anchor.line_end,
    };
    Some(DocComment {
        id: message.id.clone(),
        stage_id,
        path: anchor.path.clone().unwrap_or(path),
        anchor: doc_anchor.names_a_passage().then_some(doc_anchor),
        body: message.body.clone(),
        state: match message.agent_reply {
            Some(_) => DocCommentState::Addressed,
            None => DocCommentState::Open,
        },
        agent_reply: message.agent_reply.clone(),
        created_at: message.created_at.clone(),
    })
}

/// What one item on a conversation does to the entry that owns it.
///
/// `Attention` pulls the human in: the entry goes unread and says why.
/// `Status` updates the entry underneath them and stays quiet. The class is
/// intrinsic to the item — it is decided here, once, rather than re-derived by
/// every surface that renders a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventClass {
    Attention,
    Status,
}

/// The unread reason an agent message carries. Not an event kind: the message
/// itself is the thing that needs reading.
pub const AGENT_MESSAGE_REASON: &str = "agent_message";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadEventKind {
    SessionStarted,
    SessionEnded,
    RunStarted,
    RunFailed,
    Blocked,
    ReviewBlocked,
    IdleUnreported,
    Done,
    RevisionCreated,
    Approved,
    StageApproved,
    StageStarted,
    StageFailed,
    ImplementationStarted,
    WorktreeCreated,
    /// Existing original checkout was verified and reused.
    WorktreeReused,
    /// Original checkout was recreated from its persisted branch lineage.
    WorktreeRecreated,
    /// Legacy recovery event retained for persisted compatibility.
    WorktreeRecovered,
    RecoveryStarted,
    RecoverySucceeded,
    RecoveryFailed,
    WorktreeDeleted,
    StageCompleted,
    ImplementationArchived,
    StageInvalidated,
    Committed,
    Pushed,
    Merged,
    Abandoned,
    /// A daemon restart killed the session mid-work. The entity is parked and
    /// waiting for the human to restart it.
    Interrupted,
}

impl ThreadEventKind {
    /// Every variant, so the wire-token and class rules can be checked over the
    /// whole enum instead of a sample of it.
    pub const ALL: [ThreadEventKind; 30] = [
        ThreadEventKind::SessionStarted,
        ThreadEventKind::SessionEnded,
        ThreadEventKind::RunStarted,
        ThreadEventKind::RunFailed,
        ThreadEventKind::Blocked,
        ThreadEventKind::ReviewBlocked,
        ThreadEventKind::IdleUnreported,
        ThreadEventKind::Done,
        ThreadEventKind::RevisionCreated,
        ThreadEventKind::Approved,
        ThreadEventKind::StageApproved,
        ThreadEventKind::StageStarted,
        ThreadEventKind::StageFailed,
        ThreadEventKind::ImplementationStarted,
        ThreadEventKind::WorktreeCreated,
        ThreadEventKind::WorktreeReused,
        ThreadEventKind::WorktreeRecreated,
        ThreadEventKind::WorktreeRecovered,
        ThreadEventKind::RecoveryStarted,
        ThreadEventKind::RecoverySucceeded,
        ThreadEventKind::RecoveryFailed,
        ThreadEventKind::WorktreeDeleted,
        ThreadEventKind::StageCompleted,
        ThreadEventKind::ImplementationArchived,
        ThreadEventKind::StageInvalidated,
        ThreadEventKind::Committed,
        ThreadEventKind::Pushed,
        ThreadEventKind::Merged,
        ThreadEventKind::Abandoned,
        ThreadEventKind::Interrupted,
    ];

    /// Whether this event needs the human, or merely tells them where things
    /// got to.
    ///
    /// Attention is what an agent hands back: it finished, it stopped, it went
    /// quiet, or the work reached an outcome that ends the entry. Status is the
    /// work happening — sessions opening and closing, stages moving, commits
    /// landing, revisions appearing, checkouts being made and recovered.
    pub fn class(self) -> EventClass {
        match self {
            ThreadEventKind::Done
            | ThreadEventKind::Blocked
            | ThreadEventKind::ReviewBlocked
            | ThreadEventKind::RunFailed
            | ThreadEventKind::StageFailed
            | ThreadEventKind::RecoveryFailed
            | ThreadEventKind::IdleUnreported
            | ThreadEventKind::Interrupted
            | ThreadEventKind::Merged
            | ThreadEventKind::Abandoned => EventClass::Attention,
            ThreadEventKind::SessionStarted
            | ThreadEventKind::SessionEnded
            | ThreadEventKind::RunStarted
            | ThreadEventKind::RevisionCreated
            | ThreadEventKind::Approved
            | ThreadEventKind::StageApproved
            | ThreadEventKind::StageStarted
            | ThreadEventKind::StageCompleted
            | ThreadEventKind::StageInvalidated
            | ThreadEventKind::ImplementationStarted
            | ThreadEventKind::ImplementationArchived
            | ThreadEventKind::WorktreeCreated
            | ThreadEventKind::WorktreeReused
            | ThreadEventKind::WorktreeRecreated
            | ThreadEventKind::WorktreeRecovered
            | ThreadEventKind::WorktreeDeleted
            | ThreadEventKind::RecoveryStarted
            | ThreadEventKind::RecoverySucceeded
            | ThreadEventKind::Committed
            | ThreadEventKind::Pushed => EventClass::Status,
        }
    }

    /// The stable wire token for this kind — byte-identical to how it
    /// serializes, so an unread reason and a rendered event name never drift.
    pub fn as_str(self) -> &'static str {
        match self {
            ThreadEventKind::SessionStarted => "session_started",
            ThreadEventKind::SessionEnded => "session_ended",
            ThreadEventKind::RunStarted => "run_started",
            ThreadEventKind::RunFailed => "run_failed",
            ThreadEventKind::Blocked => "blocked",
            ThreadEventKind::ReviewBlocked => "review_blocked",
            ThreadEventKind::IdleUnreported => "idle_unreported",
            ThreadEventKind::Done => "done",
            ThreadEventKind::RevisionCreated => "revision_created",
            ThreadEventKind::Approved => "approved",
            ThreadEventKind::StageApproved => "stage_approved",
            ThreadEventKind::StageStarted => "stage_started",
            ThreadEventKind::StageFailed => "stage_failed",
            ThreadEventKind::ImplementationStarted => "implementation_started",
            ThreadEventKind::WorktreeCreated => "worktree_created",
            ThreadEventKind::WorktreeReused => "worktree_reused",
            ThreadEventKind::WorktreeRecreated => "worktree_recreated",
            ThreadEventKind::WorktreeRecovered => "worktree_recovered",
            ThreadEventKind::RecoveryStarted => "recovery_started",
            ThreadEventKind::RecoverySucceeded => "recovery_succeeded",
            ThreadEventKind::RecoveryFailed => "recovery_failed",
            ThreadEventKind::WorktreeDeleted => "worktree_deleted",
            ThreadEventKind::StageCompleted => "stage_completed",
            ThreadEventKind::ImplementationArchived => "implementation_archived",
            ThreadEventKind::StageInvalidated => "stage_invalidated",
            ThreadEventKind::Committed => "committed",
            ThreadEventKind::Pushed => "pushed",
            ThreadEventKind::Merged => "merged",
            ThreadEventKind::Abandoned => "abandoned",
            ThreadEventKind::Interrupted => "interrupted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadEvent {
    pub id: String,
    pub sequence: u64,
    pub event: ThreadEventKind,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<ThreadLink>,
    /// The agent's structured handoff, on the `Done` event that reports it.
    /// Only a completion carries one, and only when the agent wrote one — so
    /// every other event, and every record written before reports existed,
    /// omits the field entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_report: Option<CompletionReport>,
    /// What this event referenced, derived when it was pushed. See
    /// [`ThreadMessage::metadata`].
    #[serde(default, skip_serializing_if = "ItemMetadata::is_empty")]
    pub metadata: ItemMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ThreadItem {
    Message(ThreadMessage),
    Event(ThreadEvent),
}

/// How much conversation a wire view carries: `Digest` for the polled list
/// surfaces (board.list / plan.list re-ship every entity every ~2.5s, so a
/// full thread there grows without bound), `Full` for the detail surfaces
/// that actually render the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadDetail {
    Digest,
    Full,
}

impl ThreadItem {
    pub fn sequence(&self) -> u64 {
        match self {
            ThreadItem::Message(message) => message.sequence,
            ThreadItem::Event(event) => event.sequence,
        }
    }

    /// The newest counter value this item has touched: its creation sequence,
    /// or a later in-place mutation bump. Events never mutate in place.
    pub fn latest_sequence(&self) -> u64 {
        match self {
            ThreadItem::Message(message) => message.sequence.max(message.updated_sequence),
            ThreadItem::Event(event) => event.sequence,
        }
    }

    /// Why this item needs the human, as a stable token — `None` when it does
    /// not.
    ///
    /// An agent message is the agent handing the turn back, so it needs
    /// reading; a progress note explicitly keeps the turn, so it does not. A
    /// message the human wrote themselves never needs their attention.
    pub fn attention_reason(&self) -> Option<&'static str> {
        match self {
            ThreadItem::Event(event) => match event.event.class() {
                EventClass::Attention => Some(event.event.as_str()),
                EventClass::Status => None,
            },
            ThreadItem::Message(message) => {
                if message.role != MessageRole::Agent || message.still_working {
                    return None;
                }
                Some(if message.done {
                    ThreadEventKind::Done.as_str()
                } else {
                    AGENT_MESSAGE_REASON
                })
            }
        }
    }

    pub fn class(&self) -> EventClass {
        match self.attention_reason() {
            Some(_) => EventClass::Attention,
            None => EventClass::Status,
        }
    }

    /// What this item referenced, as derived when it was written.
    pub fn metadata(&self) -> &ItemMetadata {
        match self {
            ThreadItem::Message(message) => &message.metadata,
            ThreadItem::Event(event) => &event.metadata,
        }
    }

    /// The text a search reads: what a person wrote, or what the bridge wrote
    /// about the work.
    pub fn searchable_text(&self) -> String {
        match self {
            ThreadItem::Message(message) => message.body.clone(),
            ThreadItem::Event(event) => completion_text(
                event.summary.as_deref().unwrap_or_default(),
                event.completion_report.as_ref(),
            ),
        }
    }

    /// Who this item is from: the two message roles, or `event` for what the
    /// bridge recorded on its own.
    pub fn role_token(&self) -> &'static str {
        match self {
            ThreadItem::Message(message) => message.role.as_str(),
            ThreadItem::Event(_) => EVENT_ROLE,
        }
    }

    pub fn created_at(&self) -> &str {
        match self {
            ThreadItem::Message(message) => &message.created_at,
            ThreadItem::Event(event) => &event.created_at,
        }
    }
}

/// The role token an event answers to in a query. Not a [`MessageRole`]: an
/// event has no author.
pub const EVENT_ROLE: &str = "event";

/// A summary and everything the report beside it says, as one block of text —
/// what both the index and the search read.
fn completion_text(summary: &str, report: Option<&CompletionReport>) -> String {
    let Some(report) = report else {
        return summary.to_string();
    };
    let mut text = String::from(summary);
    for line in report
        .critical_files
        .iter()
        .chain(&report.risk_notes)
        .chain(&report.decisions)
        .chain(&report.skips)
    {
        text.push('\n');
        text.push_str(line);
    }
    text
}

/// What a cold session asks its own conversation for. Every field narrows;
/// nothing widens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationQuery {
    /// Case-insensitive substring of the item's text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// A path, or the tail of one: matches any indexed file containing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// A sha of any length: matches an indexed sha that it prefixes, or that
    /// prefixes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// A stage id, exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// `user`, `agent` or `event`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Only items created after this sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_sequence: Option<u64>,
    /// How many hits at most, newest first.
    #[serde(default = "default_query_limit")]
    pub limit: usize,
}

/// How many hits a query returns when it does not say.
pub const DEFAULT_QUERY_LIMIT: usize = 20;
/// The most any one query returns, however large a limit it asks for.
pub const MAX_QUERY_LIMIT: usize = 100;

fn default_query_limit() -> usize {
    DEFAULT_QUERY_LIMIT
}

impl Default for ConversationQuery {
    fn default() -> Self {
        ConversationQuery {
            text: None,
            file: None,
            commit: None,
            stage: None,
            role: None,
            since_sequence: None,
            limit: DEFAULT_QUERY_LIMIT,
        }
    }
}

impl ConversationQuery {
    /// The limit actually applied: never zero, never unbounded.
    pub fn effective_limit(&self) -> usize {
        self.limit.clamp(1, MAX_QUERY_LIMIT)
    }

    fn matches(&self, item: &ThreadItem) -> bool {
        if self
            .since_sequence
            .is_some_and(|since| item.sequence() <= since)
        {
            return false;
        }
        if self
            .role
            .as_deref()
            .is_some_and(|role| !role.eq_ignore_ascii_case(item.role_token()))
        {
            return false;
        }
        let metadata = item.metadata();
        if let Some(file) = &self.file {
            let file = file.to_lowercase();
            if !metadata
                .files
                .iter()
                .any(|indexed| indexed.to_lowercase().contains(&file))
            {
                return false;
            }
        }
        if let Some(commit) = &self.commit {
            let commit = commit.to_lowercase();
            if !metadata
                .commits
                .iter()
                .any(|indexed| indexed.starts_with(&commit) || commit.starts_with(indexed))
            {
                return false;
            }
        }
        if let Some(stage) = &self.stage {
            if !metadata
                .stages
                .iter()
                .any(|indexed| indexed.eq_ignore_ascii_case(stage))
            {
                return false;
            }
        }
        if let Some(text) = &self.text {
            if !item
                .searchable_text()
                .to_lowercase()
                .contains(&text.to_lowercase())
            {
                return false;
            }
        }
        true
    }
}

/// One search result: enough to decide whether to go read the item, never the
/// item itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationHit {
    /// Which conversation this came out of — a search may span more than one.
    pub thread_id: String,
    pub sequence: u64,
    /// `user`, `agent` or `event`.
    pub role: String,
    /// The event kind, for an event. Absent on a message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub created_at: String,
    pub excerpt: String,
    #[serde(default, skip_serializing_if = "ItemMetadata::is_empty")]
    pub metadata: ItemMetadata,
}

/// How much of an item a hit shows. A pointer, not a transcript.
const EXCERPT_MAX_CHARS: usize = 200;

/// One line of the item, centred on what was asked for when that is somewhere
/// in the middle of a long body.
fn excerpt_around(text: &str, needle: Option<&str>) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let characters: Vec<char> = flat.chars().collect();
    if characters.len() <= EXCERPT_MAX_CHARS {
        return flat;
    }
    let lowered = flat.to_lowercase();
    let match_start = needle
        .map(str::to_lowercase)
        .and_then(|needle| lowered.find(&needle))
        .map(|byte| lowered[..byte].chars().count())
        .unwrap_or(0);
    let end = (match_start + EXCERPT_MAX_CHARS * 3 / 4).clamp(EXCERPT_MAX_CHARS, characters.len());
    let start = end - EXCERPT_MAX_CHARS;
    let mut excerpt = String::new();
    if start > 0 {
        excerpt.push('…');
    }
    excerpt.extend(&characters[start..end]);
    if end < characters.len() {
        excerpt.push('…');
    }
    excerpt
}

/// What an entry says about itself in the inbox: whether anything has needed
/// the human since they last read the conversation, how much, and why.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnreadSummary {
    pub count: u64,
    /// The newest attention item's kind. `None` exactly when `count` is 0.
    pub reason: Option<&'static str>,
}

impl UnreadSummary {
    pub fn is_unread(&self) -> bool {
        self.count > 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLineage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    pub phase: String,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRevision {
    pub id: String,
    pub artifact: ArtifactKind,
    pub content_hash: String,
    pub created_at: String,
    /// Historical artifact content. Persisted locally, omitted from normal
    /// plan/run views and fetched only when the reviewer opens a revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionReport {
    #[serde(default)]
    pub critical_files: Vec<String>,
    #[serde(default)]
    pub risk_notes: Vec<String>,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub skips: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    #[serde(default)]
    pub id: String,
    #[serde(default = "empty_agent")]
    pub agent: AgentIdentity,
    #[serde(default)]
    pub sessions: Vec<SessionLineage>,
    #[serde(default)]
    pub items: Vec<ThreadItem>,
    #[serde(default)]
    pub revisions: Vec<ArtifactRevision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_completion: Option<CompletionReport>,
    #[serde(default)]
    next_sequence: u64,
    /// Where this conversation's checkout is, told to the thread by the daemon
    /// on its way to a mutation. Never persisted, never compared.
    #[serde(skip)]
    scope: WorktreeScope,
}

fn empty_agent() -> AgentIdentity {
    AgentIdentity { id: String::new() }
}

impl Default for AgentIdentity {
    fn default() -> Self {
        empty_agent()
    }
}

impl Thread {
    pub fn new(owner_id: &str) -> Self {
        Thread {
            id: format!("thread:{owner_id}"),
            agent: AgentIdentity {
                id: format!("agent:{owner_id}"),
            },
            ..Thread::default()
        }
    }

    /// The conversation of one agent. `thread:<agent_id>`, and the agent
    /// identity it carries is that same agent — there is exactly one
    /// conversation per agent, so the two can never name different things.
    pub fn for_agent(agent_id: &str) -> Self {
        Thread {
            id: format!("thread:{agent_id}"),
            agent: AgentIdentity {
                id: agent_id.to_string(),
            },
            ..Thread::default()
        }
    }

    /// Move an existing conversation onto `agent_id`, keeping every item.
    ///
    /// This is what the boot migration does to an entity-keyed thread, and what
    /// a reload does to an agent-keyed one — so it must be idempotent: a thread
    /// already keyed to this agent comes out unchanged.
    pub fn rekey_to_agent(&mut self, agent_id: &str) {
        self.id = format!("thread:{agent_id}");
        self.agent = AgentIdentity {
            id: agent_id.to_string(),
        };
        self.normalize(agent_id);
    }

    /// Say which checkout this conversation is about, so a path named in a
    /// message can be checked against real files before it is indexed. Set on
    /// the way to a mutation; a thread that was never told stays honest and
    /// records no files from prose.
    pub fn set_worktree_root(&mut self, root: impl Into<PathBuf>) {
        self.scope = WorktreeScope(Some(root.into()));
    }

    /// Whether anything has ever happened here. An entity created and never
    /// touched has an empty conversation, and nothing is lost by replacing it.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.sessions.is_empty() && self.revisions.is_empty()
    }

    /// Fill identity/counters when loading a record written before threads, or
    /// by an older build that did not persist `next_sequence`.
    pub fn normalize(&mut self, owner_id: &str) {
        if self.id.is_empty() {
            self.id = format!("thread:{owner_id}");
        }
        if self.agent.id.is_empty() {
            self.agent.id = format!("agent:{owner_id}");
        }
        self.next_sequence = self.next_sequence.max(
            self.items
                .iter()
                .map(ThreadItem::latest_sequence)
                .max()
                .unwrap_or(0),
        );
    }

    fn next(&mut self) -> u64 {
        self.next_sequence += 1;
        self.next_sequence
    }

    pub fn current_revision(&self, artifact: ArtifactKind) -> Option<&ArtifactRevision> {
        self.revisions.iter().rev().find(|r| r.artifact == artifact)
    }

    pub fn post_user(
        &mut self,
        body: impl Into<String>,
        mut anchor: Option<MessageAnchor>,
        now: impl Into<String>,
    ) -> String {
        if let Some(anchor) = &mut anchor {
            if anchor.revision_id.is_none() {
                anchor.revision_id = self.current_revision(anchor.artifact).map(|r| r.id.clone());
            }
        }
        self.post_message(
            MessageRole::User,
            false,
            body.into(),
            anchor,
            Vec::new(),
            now.into(),
        )
    }

    /// A reviewer message that came with files. The attachments are set on the
    /// message [`post_user`](Self::post_user) just pushed — the last item on the
    /// thread by construction — rather than threaded through the shared
    /// post_message arm, which every other caller would then have to pass empty.
    pub fn post_user_with_attachments(
        &mut self,
        body: impl Into<String>,
        anchor: Option<MessageAnchor>,
        attachments: Vec<MessageAttachment>,
        now: impl Into<String>,
    ) -> String {
        let id = self.post_user(body, anchor, now);
        if let Some(ThreadItem::Message(message)) = self.items.last_mut() {
            message.attachments = attachments;
        }
        id
    }

    /// Post a reviewer comment on one stage document.
    ///
    /// Plan-doc comments are conversation posts — the same anchored-message
    /// path diff comments take — so the agent reads them with the tool it
    /// already reads its messages with, and there is no second place a comment
    /// can go stale in. `anchor: None` comments the document as a whole.
    /// Returns the id of the comment (which is the id of the message).
    pub fn post_doc_comment(
        &mut self,
        issue_id: &str,
        stage_id: &str,
        path: &str,
        anchor: Option<DocAnchor>,
        body: impl Into<String>,
        now: impl Into<String>,
    ) -> String {
        let anchor = anchor.unwrap_or_default();
        let message_anchor = MessageAnchor {
            artifact: ArtifactKind::Doc,
            revision_id: None,
            path: Some(path.to_string()),
            side: None,
            line_start: anchor.line_start,
            line_end: anchor.line_end,
            heading_path: anchor.heading_path,
            snippet: anchor.snippet,
        };
        let links = vec![ThreadLink::IssueStage {
            issue_id: issue_id.to_string(),
            stage_id: stage_id.to_string(),
            path: path.to_string(),
        }];
        self.post_message(
            MessageRole::User,
            false,
            body.into(),
            Some(message_anchor),
            links,
            now.into(),
        )
    }

    /// Every plan-doc comment on this conversation, oldest first.
    pub fn doc_comments(&self) -> Vec<DocComment> {
        self.items
            .iter()
            .filter_map(|item| match item {
                ThreadItem::Message(message) => doc_comment_of(message),
                ThreadItem::Event(_) => None,
            })
            .collect()
    }

    /// The comments one stage is still waiting on, oldest first.
    pub fn open_doc_comments_for(&self, stage_id: &str) -> Vec<DocComment> {
        self.doc_comments()
            .into_iter()
            .filter(|comment| {
                comment.stage_id == stage_id && comment.state == DocCommentState::Open
            })
            .collect()
    }

    /// Record the agent's answer to one open comment, which closes it. `false`
    /// when no open comment has that id.
    pub fn resolve_doc_comment(&mut self, comment_id: &str, reply: &str) -> bool {
        let found = self.items.iter().position(|item| {
            matches!(item, ThreadItem::Message(message)
                if message.id == comment_id
                    && message.agent_reply.is_none()
                    && doc_comment_of(message).is_some())
        });
        let Some(index) = found else {
            return false;
        };
        // An in-place mutation of an already-sequenced item, like read_unread:
        // bump so the cursored polls re-ship the answered comment.
        let sequence = self.next();
        let ThreadItem::Message(message) = &mut self.items[index] else {
            return false;
        };
        message.agent_reply = Some(reply.to_string());
        message.updated_sequence = sequence;
        true
    }

    /// Delete one open comment — and with it the post, because the post is the
    /// comment. `None` when no OPEN comment has that id: an answered comment is
    /// conversation history.
    pub fn remove_doc_comment(&mut self, comment_id: &str) -> Option<DocComment> {
        let index = self.items.iter().position(|item| {
            matches!(item, ThreadItem::Message(message)
                if message.id == comment_id
                    && doc_comment_of(message).is_some_and(|comment| comment.state == DocCommentState::Open))
        })?;
        let ThreadItem::Message(message) = self.items.remove(index) else {
            return None;
        };
        doc_comment_of(&message)
    }

    pub fn post_agent(
        &mut self,
        body: impl Into<String>,
        anchor: Option<MessageAnchor>,
        now: impl Into<String>,
    ) -> String {
        self.post_agent_with_links(body, anchor, Vec::new(), now)
    }

    pub fn post_agent_with_links(
        &mut self,
        body: impl Into<String>,
        anchor: Option<MessageAnchor>,
        links: Vec<ThreadLink>,
        now: impl Into<String>,
    ) -> String {
        self.post_agent_with_links_working(body, anchor, links, now, false)
    }

    /// An agent message that says the agent is STILL working — a progress note
    /// rather than a handoff. Everything else about it is an ordinary post.
    pub fn post_agent_progress(
        &mut self,
        body: impl Into<String>,
        anchor: Option<MessageAnchor>,
        now: impl Into<String>,
    ) -> String {
        self.post_agent_with_links_working(body, anchor, Vec::new(), now, true)
    }

    pub fn post_agent_with_links_working(
        &mut self,
        body: impl Into<String>,
        anchor: Option<MessageAnchor>,
        links: Vec<ThreadLink>,
        now: impl Into<String>,
        still_working: bool,
    ) -> String {
        self.post_message_working(
            MessageRole::Agent,
            false,
            body.into(),
            anchor,
            links,
            now.into(),
            still_working,
        )
    }

    pub fn remember_completion(&mut self, report: &CompletionReport) {
        self.last_completion = Some(report.clone());
    }

    /// Record a completion: one `Done` event carrying the agent's summary and,
    /// when it wrote one, its structured report.
    ///
    /// The event IS the record. A completion used to also post an agent
    /// message repeating the summary, which put one hand-back on the
    /// conversation twice and made the entry unread twice for it.
    pub fn post_completion(
        &mut self,
        summary: impl Into<String>,
        report: Option<&CompletionReport>,
        now: impl Into<String>,
    ) {
        let summary = summary.into();
        // The report is the densest statement of what the change touched, so it
        // is indexed with the summary rather than beside it.
        let metadata =
            ItemMetadata::derive(&completion_text(&summary, report), &[], None, &self.scope);
        let sequence = self.next();
        self.items.push(ThreadItem::Event(ThreadEvent {
            id: format!("event-{sequence}"),
            sequence,
            event: ThreadEventKind::Done,
            created_at: now.into(),
            summary: Some(summary),
            session_id: None,
            revision_id: None,
            links: Vec::new(),
            completion_report: report.cloned(),
            metadata,
        }));
    }

    fn post_message(
        &mut self,
        role: MessageRole,
        done: bool,
        body: String,
        anchor: Option<MessageAnchor>,
        links: Vec<ThreadLink>,
        now: String,
    ) -> String {
        self.post_message_working(role, done, body, anchor, links, now, false)
    }

    #[allow(clippy::too_many_arguments)]
    fn post_message_working(
        &mut self,
        role: MessageRole,
        done: bool,
        body: String,
        anchor: Option<MessageAnchor>,
        links: Vec<ThreadLink>,
        now: String,
        still_working: bool,
    ) -> String {
        let metadata = ItemMetadata::derive(&body, &links, anchor.as_ref(), &self.scope);
        let sequence = self.next();
        let id = format!("message-{sequence}");
        self.items.push(ThreadItem::Message(ThreadMessage {
            still_working,
            metadata,
            id: id.clone(),
            sequence,
            updated_sequence: sequence,
            role,
            done,
            source: MessageSource::Chat,
            body,
            created_at: now,
            seen_at: None,
            anchor,
            resolved_by_revision: None,
            agent_reply: None,
            links,
            attachments: Vec::new(),
        }));
        id
    }

    /// Whether the reviewer has said anything the agent has not picked up yet.
    ///
    /// The read-only half of [`read_unread`](Self::read_unread), which marks
    /// what it returns as seen — so a caller deciding WHETHER to send the agent
    /// to the mailbox cannot use it without emptying the mailbox first.
    pub fn has_unread(&self) -> bool {
        self.items.iter().any(|item| {
            matches!(item, ThreadItem::Message(message)
                if message.role == MessageRole::User && message.seen_at.is_none())
        })
    }

    pub fn read_unread(&mut self, now: &str) -> Vec<ThreadMessage> {
        let mut unread = Vec::new();
        for item in &mut self.items {
            let ThreadItem::Message(message) = item else {
                continue;
            };
            if message.role == MessageRole::User && message.seen_at.is_none() {
                message.seen_at = Some(now.to_string());
                // An in-place mutation of an already-sequenced item: bump its
                // updated_sequence (inlined `next()` — the loop holds a borrow
                // of `self.items`) so cursored polls re-ship the seen state.
                self.next_sequence += 1;
                message.updated_sequence = self.next_sequence;
                unread.push(message.clone());
            }
        }
        unread
    }

    pub fn push_event(
        &mut self,
        event: ThreadEventKind,
        summary: Option<String>,
        session_id: Option<String>,
        revision_id: Option<String>,
        now: impl Into<String>,
    ) {
        self.push_event_with_links(event, summary, session_id, revision_id, Vec::new(), now);
    }

    pub fn push_event_with_links(
        &mut self,
        event: ThreadEventKind,
        summary: Option<String>,
        session_id: Option<String>,
        revision_id: Option<String>,
        links: Vec<ThreadLink>,
        now: impl Into<String>,
    ) {
        let metadata = ItemMetadata::derive(
            summary.as_deref().unwrap_or_default(),
            &links,
            None,
            &self.scope,
        );
        let sequence = self.next();
        self.items.push(ThreadItem::Event(ThreadEvent {
            id: format!("event-{sequence}"),
            sequence,
            event,
            created_at: now.into(),
            summary,
            session_id,
            revision_id,
            links,
            completion_report: None,
            metadata,
        }));
    }

    pub fn start_session(
        &mut self,
        provider: &str,
        model: Option<&str>,
        effort: Option<&str>,
        phase: &str,
        now: &str,
    ) -> String {
        let parent_session_id = self.sessions.last().map(|session| session.id.clone());
        let id = format!("session-{}", uuid::Uuid::new_v4());
        self.sessions.push(SessionLineage {
            id: id.clone(),
            parent_session_id,
            provider: provider.to_string(),
            model: model.map(str::to_string),
            effort: effort.map(str::to_string),
            phase: phase.to_string(),
            started_at: now.to_string(),
            ended_at: None,
        });
        self.push_event(
            ThreadEventKind::SessionStarted,
            Some(format!("{phase} session started")),
            Some(id.clone()),
            None,
            now,
        );
        id
    }

    pub fn finish_session(&mut self, session_id: &str, now: &str) {
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|session| session.id == session_id)
        {
            session.ended_at = Some(now.to_string());
        }
        self.push_event(
            ThreadEventKind::SessionEnded,
            None,
            Some(session_id.to_string()),
            None,
            now,
        );
    }

    pub fn add_revision(
        &mut self,
        artifact: ArtifactKind,
        contents: &str,
        now: &str,
    ) -> ArtifactRevision {
        let hash = format!("{:x}", Sha256::digest(contents.as_bytes()));
        if let Some(current) = self.current_revision(artifact) {
            if current.content_hash == hash {
                return current.clone();
            }
        }
        let number = self
            .revisions
            .iter()
            .filter(|r| r.artifact == artifact)
            .count()
            + 1;
        let revision = ArtifactRevision {
            id: format!("{}-revision-{number}-{}", artifact.as_str(), &hash[..8]),
            artifact,
            content_hash: hash,
            created_at: now.to_string(),
            snapshot: Some(snapshot_contents(contents, 1_000_000)),
        };
        for item in &mut self.items {
            let ThreadItem::Message(message) = item else {
                continue;
            };
            let Some(anchor) = &message.anchor else {
                continue;
            };
            if message.role == MessageRole::User
                && anchor.artifact == artifact
                && message.resolved_by_revision.is_none()
                && !anchor.snippet.trim().is_empty()
                && !contents.contains(anchor.snippet.trim())
            {
                message.resolved_by_revision = Some(revision.id.clone());
                // Same cursor rule as read_unread: resolution mutates an
                // already-sequenced item, so bump for the cursored polls.
                self.next_sequence += 1;
                message.updated_sequence = self.next_sequence;
            }
        }
        self.revisions.push(revision.clone());
        let snapshot_count = self
            .revisions
            .iter()
            .filter(|revision| revision.snapshot.is_some())
            .count();
        if snapshot_count > 20 {
            if let Some(oldest) = self
                .revisions
                .iter_mut()
                .find(|revision| revision.snapshot.is_some())
            {
                oldest.snapshot = None;
            }
        }
        self.push_event(
            ThreadEventKind::RevisionCreated,
            None,
            None,
            Some(revision.id.clone()),
            now,
        );
        revision
    }

    /// Public thread payload for the SPA. Historical snapshots are deliberately
    /// excluded so board polling never ships every old patch over E2EE.
    pub fn wire_value(&self) -> Value {
        json!({
            "id": self.id,
            "agent": self.agent,
            "sessions": self.sessions,
            "items": self.items,
            "revisions": self.revision_summaries(),
            "last_completion": self.last_completion,
        })
    }

    /// Snapshot-free revision listing shared by the full and cursored wire
    /// views.
    fn revision_summaries(&self) -> Vec<Value> {
        self.revisions
            .iter()
            .map(|revision| {
                json!({
                    "id": revision.id,
                    "artifact": revision.artifact,
                    "content_hash": revision.content_hash,
                    "created_at": revision.created_at,
                    "snapshot_available": revision.snapshot.is_some(),
                })
            })
            .collect()
    }

    /// The highest counter value any item has touched — creation or in-place
    /// mutation bump (0 for an empty thread) — the client's cursor high-water
    /// mark. Counting mutation bumps is what keeps the cursored polls from
    /// re-shipping a mutated item forever.
    pub fn last_sequence(&self) -> u64 {
        self.items
            .iter()
            .map(ThreadItem::latest_sequence)
            .max()
            .unwrap_or(0)
    }

    /// When the turn the agent is working started, or `None` when nothing is
    /// in flight — the authoritative source of working time.
    ///
    /// A turn starts when the agent READS a reviewer message (that read is what
    /// stamps `seen_at`) and ends when the agent hands the turn back: an
    /// ordinary reply hands back, a progress note keeps it, and any
    /// attention-class event ends it outright. Only the newest such message
    /// carries the turn — one agent, one thing being worked.
    pub fn working_since(&self) -> Option<&str> {
        for item in self.items.iter().rev() {
            match item {
                ThreadItem::Message(message) => {
                    if message.role == MessageRole::User {
                        if let Some(seen_at) = message.seen_at.as_deref() {
                            return Some(seen_at);
                        }
                    } else if message.role == MessageRole::Agent && !message.still_working {
                        return None;
                    }
                }
                ThreadItem::Event(event) => {
                    if event.event.class() == EventClass::Attention {
                        return None;
                    }
                }
            }
        }
        None
    }

    /// The attention-class items created after `cursor` — the whole of the
    /// unread rule.
    ///
    /// Creation sequence, never the in-place mutation bump: marking a message
    /// seen or resolving a comment moves an item's `updated_sequence`, and
    /// neither is new news. A status-only stretch of conversation leaves the
    /// entry read, however long it is.
    pub fn unread_since(&self, cursor: u64) -> UnreadSummary {
        let unread = self
            .items
            .iter()
            .filter(|item| item.sequence() > cursor)
            .filter_map(ThreadItem::attention_reason);
        let mut summary = UnreadSummary::default();
        for reason in unread {
            summary.count += 1;
            summary.reason = Some(reason);
        }
        summary
    }

    /// The items this query names, newest first, bounded by its limit.
    ///
    /// The point of the tool this serves: a session that lost its context asks
    /// what was decided about one thing, instead of replaying the whole log.
    pub fn search(&self, query: &ConversationQuery) -> Vec<ConversationHit> {
        self.items
            .iter()
            .rev()
            .filter(|item| query.matches(item))
            .take(query.effective_limit())
            .map(|item| ConversationHit {
                thread_id: self.id.clone(),
                sequence: item.sequence(),
                role: item.role_token().to_string(),
                kind: match item {
                    ThreadItem::Event(event) => Some(event.event.as_str().to_string()),
                    ThreadItem::Message(_) => None,
                },
                created_at: item.created_at().to_string(),
                excerpt: excerpt_around(&item.searchable_text(), query.text.as_deref()),
                metadata: item.metadata().clone(),
            })
            .collect()
    }

    /// Bounded summary for list surfaces: identity plus counters and the
    /// latest event, never message bodies or the item array, so the polled
    /// board payload stops growing with conversation length.
    pub fn digest_value(&self) -> Value {
        let latest_event = self.items.iter().rev().find_map(|item| match item {
            ThreadItem::Event(event) => Some(event),
            ThreadItem::Message(_) => None,
        });
        json!({
            "id": self.id,
            "agent": self.agent,
            "item_count": self.items.len(),
            "last_sequence": self.last_sequence(),
            "last_event": latest_event.map(|event| json!({
                "event": event.event,
                "created_at": event.created_at,
            })),
        })
    }

    /// Cursor view for the detail polls: only items created — or mutated in
    /// place — strictly after `after_sequence`, plus `thread_total` /
    /// `thread_last_sequence` so the client can detect a gap (bridge restart,
    /// dropped delta) and refetch in full. Built directly (never by trimming a
    /// full `wire_value`) so the per-poll serialization cost is bounded like
    /// the wire. Sessions, revisions and last_completion are small and
    /// bounded, so they always ship whole.
    pub fn wire_value_after(&self, after_sequence: u64) -> Value {
        let newer: Vec<&ThreadItem> = self
            .items
            .iter()
            .filter(|item| item.latest_sequence() > after_sequence)
            .collect();
        json!({
            "id": self.id,
            "agent": self.agent,
            "sessions": self.sessions,
            "items": newer,
            "revisions": self.revision_summaries(),
            "last_completion": self.last_completion,
            "thread_total": self.items.len(),
            "thread_last_sequence": self.last_sequence(),
        })
    }

    pub fn catch_up_markdown(&self, limit: usize) -> String {
        let mut lines = Vec::new();
        for item in self.items.iter().rev().take(limit).rev() {
            match item {
                ThreadItem::Message(message)
                    if message.done || message.source == MessageSource::Completion => {}
                ThreadItem::Message(message) => lines.push(format!(
                    "- {}: {}{}",
                    message.role.as_str(),
                    message.body.replace('\n', " "),
                    attachment_note(&message.attachments)
                )),
                ThreadItem::Event(event) => {
                    if let Some(summary) = &event.summary {
                        lines.push(format!("- event/{:?}: {summary}", event.event));
                    }
                }
            }
        }
        lines.join("\n")
    }
}

/// The trailer that names a message's files in prose form. The catch-up packet
/// is markdown, not JSON, so a path only reaches a resumed agent if it is
/// written into the line.
fn attachment_note(attachments: &[MessageAttachment]) -> String {
    if attachments.is_empty() {
        return String::new();
    }
    let paths: Vec<&str> = attachments
        .iter()
        .map(|attachment| attachment.path.as_str())
        .collect();
    format!(" [attached files, open them: {}]", paths.join(", "))
}

fn snapshot_contents(contents: &str, max_bytes: usize) -> String {
    if contents.len() <= max_bytes {
        return contents.to_string();
    }
    let mut boundary = max_bytes;
    while !contents.is_char_boundary(boundary) {
        boundary -= 1;
    }
    let mut snapshot = contents[..boundary].to_string();
    snapshot.push_str("\n\n[Snapshot truncated by Build]");
    snapshot
}

impl ArtifactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ArtifactKind::Plan => "plan",
            ArtifactKind::Diff => "diff",
            ArtifactKind::Doc => "doc",
        }
    }
}

#[cfg(test)]
mod attention_class_tests {
    use super::*;

    #[test]
    fn every_event_kind_names_itself_the_way_it_serializes() {
        for kind in ThreadEventKind::ALL {
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                json!(kind.as_str()),
                "{kind:?}"
            );
        }
    }

    /// The split the whole inbox rests on: an agent handing back needs the
    /// human, the work happening does not.
    #[test]
    fn handing_back_is_attention_and_working_is_status() {
        for kind in [
            ThreadEventKind::Done,
            ThreadEventKind::Blocked,
            ThreadEventKind::ReviewBlocked,
            ThreadEventKind::RunFailed,
            ThreadEventKind::StageFailed,
            ThreadEventKind::RecoveryFailed,
            ThreadEventKind::IdleUnreported,
            ThreadEventKind::Interrupted,
            ThreadEventKind::Merged,
            ThreadEventKind::Abandoned,
        ] {
            assert_eq!(kind.class(), EventClass::Attention, "{kind:?}");
        }
        for kind in [
            ThreadEventKind::RunStarted,
            ThreadEventKind::SessionStarted,
            ThreadEventKind::SessionEnded,
            ThreadEventKind::Committed,
            ThreadEventKind::Pushed,
            ThreadEventKind::RevisionCreated,
            ThreadEventKind::StageStarted,
            ThreadEventKind::StageCompleted,
            ThreadEventKind::StageApproved,
            ThreadEventKind::StageInvalidated,
            ThreadEventKind::ImplementationStarted,
            ThreadEventKind::WorktreeCreated,
            ThreadEventKind::RecoveryStarted,
        ] {
            assert_eq!(kind.class(), EventClass::Status, "{kind:?}");
        }
    }

    #[test]
    fn an_agent_reply_needs_reading_and_its_own_words_never_do() {
        let mut thread = Thread::new("run-1");
        thread.post_user("please rename the helper", None, "2026-08-13T09:00:00Z");
        thread.post_agent_progress("still digging", None, "2026-08-13T09:01:00Z");
        thread.post_agent("renamed it, here is why", None, "2026-08-13T09:02:00Z");

        let reasons: Vec<Option<&str>> = thread
            .items
            .iter()
            .map(ThreadItem::attention_reason)
            .collect();
        assert_eq!(reasons, vec![None, None, Some(AGENT_MESSAGE_REASON)]);
    }

    #[test]
    fn a_completion_is_one_done_event_and_no_companion_message() {
        let mut thread = Thread::new("run-1");
        thread.post_completion("implemented the change", None, "2026-08-13T09:00:00Z");

        assert_eq!(thread.items.len(), 1, "{:?}", thread.items);
        assert!(matches!(
            &thread.items[0],
            ThreadItem::Event(event)
                if event.event == ThreadEventKind::Done
                    && event.summary.as_deref() == Some("implemented the change")
        ));
        assert_eq!(thread.items[0].attention_reason(), Some("done"));
    }

    #[test]
    fn the_done_event_carries_the_completion_report_onto_the_wire() {
        let mut thread = Thread::new("run-report");
        let report = CompletionReport {
            critical_files: vec!["src/thread.rs — the event now carries the report".to_string()],
            risk_notes: vec!["older records have no report".to_string()],
            decisions: vec!["kept last_completion for cold sessions".to_string()],
            skips: vec!["no SPA card yet".to_string()],
        };

        thread.post_completion(
            "implemented the change",
            Some(&report),
            "2026-08-13T09:00:00Z",
        );

        let wire = thread.wire_value();
        let carried = &wire["items"][0]["data"]["completion_report"];
        assert_eq!(
            carried["critical_files"][0],
            "src/thread.rs — the event now carries the report"
        );
        assert_eq!(carried["risk_notes"][0], "older records have no report");
        assert_eq!(
            carried["decisions"][0],
            "kept last_completion for cold sessions"
        );
        assert_eq!(carried["skips"][0], "no SPA card yet");
    }

    #[test]
    fn an_event_without_a_report_omits_the_field() {
        let mut thread = Thread::new("run-plain");
        thread.post_completion("implemented the change", None, "2026-08-13T09:00:00Z");
        thread.push_event(
            ThreadEventKind::RunStarted,
            None,
            None,
            None,
            "2026-08-13T09:01:00Z",
        );

        let wire = thread.wire_value();
        for index in 0..2 {
            assert!(
                wire["items"][index]["data"]
                    .get("completion_report")
                    .is_none(),
                "{wire:?}"
            );
        }
    }

    #[test]
    fn unread_counts_only_attention_items_past_the_cursor() {
        let mut thread = Thread::new("run-1");
        thread.push_event(
            ThreadEventKind::RunStarted,
            None,
            None,
            None,
            "2026-08-13T09:00:00Z",
        );
        let cursor = thread.last_sequence();
        thread.push_event(
            ThreadEventKind::Committed,
            None,
            None,
            None,
            "2026-08-13T09:01:00Z",
        );
        thread.post_user("a note of my own", None, "2026-08-13T09:02:00Z");
        assert_eq!(thread.unread_since(cursor), UnreadSummary::default());
        assert!(!thread.unread_since(cursor).is_unread());

        thread.post_agent("here is the answer", None, "2026-08-13T09:03:00Z");
        thread.push_event(
            ThreadEventKind::Blocked,
            None,
            None,
            None,
            "2026-08-13T09:04:00Z",
        );
        let unread = thread.unread_since(cursor);
        assert_eq!(unread.count, 2);
        assert_eq!(unread.reason, Some("blocked"), "the newest one says why");
        assert!(unread.is_unread());

        // Reading through the whole conversation empties it.
        assert_eq!(
            thread.unread_since(thread.last_sequence()),
            UnreadSummary::default()
        );
    }

    /// A message marked seen bumps its `updated_sequence`; that is bookkeeping
    /// for the cursored polls, not news, and must not resurrect an unread badge.
    #[test]
    fn marking_a_message_seen_does_not_make_the_entry_unread_again() {
        let mut thread = Thread::new("run-1");
        thread.post_agent("here is the answer", None, "2026-08-13T09:00:00Z");
        thread.post_user("thanks", None, "2026-08-13T09:01:00Z");
        let cursor = thread.last_sequence();
        thread.read_unread("2026-08-13T09:02:00Z");
        assert_eq!(thread.unread_since(cursor), UnreadSummary::default());
    }
}

#[cfg(test)]
mod working_flag_tests {
    use super::*;

    /// "Working" has to end when the agent hands back, and only the agent knows
    /// which of its messages is a progress note and which is the handoff. A
    /// posted reply hands back by default — a stuck "Working" outlives the work
    /// it describes, while a progress note that clears the line early costs
    /// nothing.
    #[test]
    fn an_agent_message_hands_back_unless_it_says_it_is_still_working() {
        let mut thread = Thread::new("run-1");
        let handoff = thread.post_agent("here is what I found", None, "2026-08-08T03:00:00Z");
        let update = thread.post_agent_progress("still digging", None, "2026-08-08T03:01:00Z");

        let message = |id: &str| {
            thread
                .items
                .iter()
                .find_map(|item| match item {
                    ThreadItem::Message(m) if m.id == id => Some(m.clone()),
                    _ => None,
                })
                .expect("the message is on the thread")
        };
        assert!(
            !message(&handoff).still_working,
            "an ordinary reply gives the turn back"
        );
        assert!(
            message(&update).still_working,
            "a progress note keeps the agent working"
        );
    }

    /// Working time is derived from the turn, not from a timer: it starts when
    /// the agent READS what the human said and ends when the agent hands the
    /// turn back. Same rule the MCP tool descriptions already teach agents.
    #[test]
    fn a_turn_runs_from_the_read_until_the_agent_hands_it_back() {
        let mut thread = Thread::new("run-working");
        assert_eq!(thread.working_since(), None, "an empty thread is idle");

        thread.post_user("do the thing", None, "2026-08-13T10:00:00Z");
        assert_eq!(
            thread.working_since(),
            None,
            "a message nobody has read yet is not work in flight"
        );

        thread.read_unread("2026-08-13T10:00:05Z");
        assert_eq!(thread.working_since(), Some("2026-08-13T10:00:05Z"));

        thread.post_agent_progress("halfway", None, "2026-08-13T10:01:00Z");
        assert_eq!(
            thread.working_since(),
            Some("2026-08-13T10:00:05Z"),
            "a progress note keeps the same turn running"
        );
        thread.push_event(
            ThreadEventKind::Committed,
            None,
            None,
            None,
            "2026-08-13T10:02:00Z",
        );
        assert_eq!(
            thread.working_since(),
            Some("2026-08-13T10:00:05Z"),
            "status is the work happening, not the work ending"
        );

        thread.post_agent("here is what I did", None, "2026-08-13T10:03:00Z");
        assert_eq!(thread.working_since(), None, "an ordinary reply hands back");
    }

    /// An agent that reports `done` without saying anything has still handed
    /// back — the event is the record.
    #[test]
    fn an_attention_event_ends_the_turn_with_nothing_said() {
        let mut thread = Thread::new("run-done");
        thread.post_user("ship it", None, "2026-08-13T11:00:00Z");
        thread.read_unread("2026-08-13T11:00:01Z");
        thread.push_event(
            ThreadEventKind::Done,
            None,
            None,
            None,
            "2026-08-13T11:05:00Z",
        );
        assert_eq!(thread.working_since(), None);

        // A second message read after the handoff starts a new turn.
        thread.post_user("one more thing", None, "2026-08-13T11:06:00Z");
        thread.read_unread("2026-08-13T11:06:02Z");
        assert_eq!(thread.working_since(), Some("2026-08-13T11:06:02Z"));
    }
}

#[cfg(test)]
mod findability_tests {
    use super::*;

    /// A checkout holding exactly the files a test names, so "this path is real"
    /// is decided by the same filesystem the bridge would ask.
    fn checkout(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("a temp checkout");
        for file in files {
            let path = dir.path().join(file);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("dirs");
            std::fs::write(path, "").expect("a file");
        }
        dir
    }

    fn thread_in(dir: &tempfile::TempDir) -> Thread {
        let mut thread = Thread::new("run-1");
        thread.set_worktree_root(dir.path());
        thread
    }

    fn message_metadata(thread: &Thread) -> ItemMetadata {
        thread
            .items
            .last()
            .expect("an item was posted")
            .metadata()
            .clone()
    }

    /// The rule that keeps prose out of the file index: a path is recorded only
    /// when the worktree really has that file. "and/or" and a plausible-looking
    /// path nobody created are prose, not files.
    #[test]
    fn a_path_counts_only_when_the_checkout_really_has_that_file() {
        let dir = checkout(&["src/parser.rs", "docs/design.md"]);
        let mut thread = thread_in(&dir);
        thread.post_user(
            "Rework `src/parser.rs` and/or docs/design.md, but not src/imaginary.rs. \
             See main.rs too.",
            None,
            "2026-08-13T09:00:00Z",
        );

        let metadata = message_metadata(&thread);
        assert_eq!(metadata.files, vec!["src/parser.rs", "docs/design.md"]);
    }

    /// Punctuation is how people write, not part of the path.
    #[test]
    fn a_path_wrapped_in_prose_punctuation_is_still_the_path() {
        let dir = checkout(&["src/parser.rs"]);
        let mut thread = thread_in(&dir);
        thread.post_agent(
            "I touched (src/parser.rs), then re-read \"src/parser.rs\".",
            None,
            "2026-08-13T09:00:00Z",
        );
        assert_eq!(message_metadata(&thread).files, vec!["src/parser.rs"]);
    }

    /// A conversation with no checkout behind it cannot tell a path from a
    /// phrase, so it claims no files from prose — but a link the bridge already
    /// validated is a file whatever the thread knows about disk.
    #[test]
    fn without_a_checkout_only_validated_links_name_files() {
        let mut thread = Thread::new("run-rootless");
        thread.post_agent_with_links(
            "Look at src/parser.rs and/or elsewhere.",
            None,
            vec![ThreadLink::File {
                path: "src/lexer.rs".to_string(),
                line_start: None,
                line_end: None,
            }],
            "2026-08-13T09:00:00Z",
        );
        assert_eq!(message_metadata(&thread).files, vec!["src/lexer.rs"]);
    }

    /// A commit sha is 7-40 hex characters. Requiring a digit is what keeps
    /// all-hex English ("effaced", "deface") out of the commit index; a real
    /// sha without a single digit is a one-in-a-thousand accident, and a link
    /// carries the exact sha anyway.
    #[test]
    fn a_commit_is_hex_of_the_right_length_with_a_digit_in_it() {
        let mut thread = Thread::new("run-commits");
        thread.post_user(
            "a1b2c3d effaced deadbeef 0123456789abcdef0123456789abcdef01234567 \
             abc123 0123456789abcdef0123456789abcdef012345678",
            None,
            "2026-08-13T09:00:00Z",
        );
        assert_eq!(
            message_metadata(&thread).commits,
            vec!["a1b2c3d", "0123456789abcdef0123456789abcdef01234567"],
            "short, digit-free and over-long tokens are not shas"
        );
    }

    #[test]
    fn a_commit_link_is_a_commit_however_the_body_reads() {
        let mut thread = Thread::new("run-commit-link");
        thread.push_event_with_links(
            ThreadEventKind::Committed,
            Some("committed the parser fix".to_string()),
            None,
            None,
            vec![ThreadLink::Commit {
                sha: "A".repeat(40),
            }],
            "2026-08-13T09:00:00Z",
        );
        assert_eq!(
            thread.items.last().unwrap().metadata().commits,
            vec!["a".repeat(40)],
            "a sha is indexed lowercase whatever case it arrived in"
        );
    }

    #[test]
    fn a_stage_link_makes_the_item_findable_by_stage() {
        let mut thread = Thread::new("run-stages");
        thread.push_event_with_links(
            ThreadEventKind::StageStarted,
            Some("Started stage Parser".to_string()),
            None,
            None,
            vec![
                ThreadLink::IssueStage {
                    issue_id: "issue-1".to_string(),
                    stage_id: "parser".to_string(),
                    path: ".build/plan/01-parser.md".to_string(),
                },
                ThreadLink::PlanStage {
                    plan_id: "issue-1".to_string(),
                    stage_id: "lexer".to_string(),
                    path: ".build/plan/02-lexer.md".to_string(),
                },
            ],
            "2026-08-13T09:00:00Z",
        );
        assert_eq!(
            thread.items.last().unwrap().metadata().stages,
            vec!["parser", "lexer"]
        );
    }

    /// The completion report is the densest statement of what a change touched,
    /// so it is indexed like any other text the agent wrote.
    #[test]
    fn a_completion_report_makes_its_critical_files_findable() {
        let dir = checkout(&["src/parser.rs"]);
        let mut thread = thread_in(&dir);
        thread.post_completion(
            "rewrote the parser",
            Some(&CompletionReport {
                critical_files: vec!["src/parser.rs — now streams tokens".to_string()],
                risk_notes: vec!["reverts cleanly at a1b2c3d".to_string()],
                decisions: Vec::new(),
                skips: Vec::new(),
            }),
            "2026-08-13T09:00:00Z",
        );
        let metadata = thread.items.last().unwrap().metadata().clone();
        assert_eq!(metadata.files, vec!["src/parser.rs"]);
        assert_eq!(metadata.commits, vec!["a1b2c3d"]);
    }

    #[test]
    fn metadata_ships_on_the_wire_and_an_item_without_any_omits_it() {
        let dir = checkout(&["src/parser.rs"]);
        let mut thread = thread_in(&dir);
        thread.post_user("nothing to see here", None, "2026-08-13T09:00:00Z");
        thread.post_agent("fixed src/parser.rs", None, "2026-08-13T09:01:00Z");

        let wire = thread.wire_value();
        assert!(
            wire["items"][0]["data"].get("metadata").is_none(),
            "{wire:?}"
        );
        assert_eq!(
            wire["items"][1]["data"]["metadata"]["files"][0],
            "src/parser.rs"
        );

        let reloaded: Thread = serde_json::from_value(serde_json::to_value(&thread).unwrap())
            .expect("a thread with metadata reloads");
        assert_eq!(reloaded.items, thread.items);
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;

    fn conversation() -> Thread {
        let mut thread = Thread::new("run-search");
        thread.post_user(
            "Rename the helper in the parser",
            None,
            "2026-08-13T09:00:00Z",
        );
        thread.post_agent_with_links(
            "Renamed it; the parser now streams tokens.",
            None,
            vec![ThreadLink::File {
                path: "src/parser.rs".to_string(),
                line_start: None,
                line_end: None,
            }],
            "2026-08-13T09:01:00Z",
        );
        thread.push_event_with_links(
            ThreadEventKind::Committed,
            Some("committed the rename".to_string()),
            None,
            None,
            vec![ThreadLink::Commit {
                sha: "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678".to_string(),
            }],
            "2026-08-13T09:02:00Z",
        );
        thread.push_event_with_links(
            ThreadEventKind::StageStarted,
            Some("Started stage Lexer".to_string()),
            None,
            None,
            vec![ThreadLink::IssueStage {
                issue_id: "issue-1".to_string(),
                stage_id: "lexer".to_string(),
                path: ".build/plan/02-lexer.md".to_string(),
            }],
            "2026-08-13T09:03:00Z",
        );
        thread
    }

    fn sequences(hits: &[ConversationHit]) -> Vec<u64> {
        hits.iter().map(|hit| hit.sequence).collect()
    }

    #[test]
    fn an_empty_query_returns_the_whole_conversation_newest_first() {
        let hits = conversation().search(&ConversationQuery::default());
        assert_eq!(sequences(&hits), vec![4, 3, 2, 1]);
        assert_eq!(hits[0].role, "event");
        assert_eq!(hits[0].kind.as_deref(), Some("stage_started"));
        assert_eq!(hits[0].created_at, "2026-08-13T09:03:00Z");
        assert_eq!(hits[3].role, "user");
        assert_eq!(hits[3].kind, None);
        assert_eq!(hits[3].excerpt, "Rename the helper in the parser");
        assert_eq!(hits[3].thread_id, "thread:run-search");
    }

    #[test]
    fn text_matching_is_case_insensitive_substring() {
        // Substring, so "RENAME" also finds "Renamed" — a cold session asking
        // about a word should not have to guess which form was written.
        let hits = conversation().search(&ConversationQuery {
            text: Some("RENAME".to_string()),
            ..ConversationQuery::default()
        });
        assert_eq!(sequences(&hits), vec![3, 2, 1]);
        assert!(conversation()
            .search(&ConversationQuery {
                text: Some("lexer".to_string()),
                ..ConversationQuery::default()
            })
            .iter()
            .all(|hit| hit.sequence == 4));
    }

    #[test]
    fn each_metadata_filter_narrows_to_the_items_carrying_it() {
        let thread = conversation();

        let by_file = thread.search(&ConversationQuery {
            file: Some("parser.rs".to_string()),
            ..ConversationQuery::default()
        });
        assert_eq!(sequences(&by_file), vec![2]);

        // A short sha finds the full one it prefixes, and the reverse.
        let by_short_commit = thread.search(&ConversationQuery {
            commit: Some("a1b2c3d".to_string()),
            ..ConversationQuery::default()
        });
        assert_eq!(sequences(&by_short_commit), vec![3]);
        let by_full_commit = thread.search(&ConversationQuery {
            commit: Some("a1b2c3d4e5f60718293a4b5c6d7e8f9012345678".to_string()),
            ..ConversationQuery::default()
        });
        assert_eq!(sequences(&by_full_commit), vec![3]);
        assert!(thread
            .search(&ConversationQuery {
                commit: Some("f".repeat(7),),
                ..ConversationQuery::default()
            })
            .is_empty());

        let by_stage = thread.search(&ConversationQuery {
            stage: Some("lexer".to_string()),
            ..ConversationQuery::default()
        });
        assert_eq!(sequences(&by_stage), vec![4]);
    }

    #[test]
    fn role_selects_who_said_it_and_events_are_their_own_role() {
        let thread = conversation();
        for (role, expected) in [("user", vec![1]), ("agent", vec![2]), ("event", vec![4, 3])] {
            let hits = thread.search(&ConversationQuery {
                role: Some(role.to_string()),
                ..ConversationQuery::default()
            });
            assert_eq!(sequences(&hits), expected, "role={role}");
        }
    }

    #[test]
    fn since_sequence_and_limit_bound_the_answer() {
        let thread = conversation();
        let since = thread.search(&ConversationQuery {
            since_sequence: Some(2),
            ..ConversationQuery::default()
        });
        assert_eq!(sequences(&since), vec![4, 3]);

        let limited = thread.search(&ConversationQuery {
            limit: 2,
            ..ConversationQuery::default()
        });
        assert_eq!(sequences(&limited), vec![4, 3]);
    }

    #[test]
    fn filters_combine_rather_than_widen() {
        let hits = conversation().search(&ConversationQuery {
            text: Some("rename".to_string()),
            role: Some("user".to_string()),
            ..ConversationQuery::default()
        });
        assert_eq!(sequences(&hits), vec![1]);
    }

    /// A hit is a pointer, not a transcript: the excerpt is bounded and centred
    /// on what was asked for, so a cold session can decide what to read next
    /// without paying for the whole conversation.
    #[test]
    fn a_long_body_is_excerpted_around_the_match() {
        let mut thread = Thread::new("run-long");
        let body = format!(
            "{} the decisive sentence {}",
            "x ".repeat(400),
            "y ".repeat(400)
        );
        thread.post_agent(body, None, "2026-08-13T09:00:00Z");

        let hits = thread.search(&ConversationQuery {
            text: Some("decisive".to_string()),
            ..ConversationQuery::default()
        });
        assert_eq!(hits.len(), 1);
        let excerpt = &hits[0].excerpt;
        assert!(excerpt.contains("the decisive sentence"), "{excerpt}");
        assert!(
            excerpt.chars().count() <= 210,
            "{}",
            excerpt.chars().count()
        );
    }

    #[test]
    fn a_hit_carries_the_metadata_that_made_it_findable() {
        let hits = conversation().search(&ConversationQuery {
            file: Some("src/parser.rs".to_string()),
            ..ConversationQuery::default()
        });
        assert_eq!(hits[0].metadata.files, vec!["src/parser.rs"]);
    }
}

#[cfg(test)]
mod attachment_tests {
    use super::*;

    fn image() -> MessageAttachment {
        MessageAttachment {
            name: "screenshot.png".to_string(),
            path: ".build/attachments/ab12cd34-screenshot.png".to_string(),
            mime: "image/png".to_string(),
            size: 4096,
        }
    }

    /// The agent reads its mail as JSON, so a file the reviewer attached only
    /// exists for it if the path rides the message it was sent with.
    #[test]
    fn an_attached_file_rides_the_message_the_agent_reads() {
        let mut thread = Thread::new("run-1");
        thread.post_user_with_attachments(
            "look at this",
            None,
            vec![image()],
            "2026-08-09T13:00:00Z",
        );

        let unread = thread.read_unread("2026-08-09T13:00:01Z");
        assert_eq!(unread.len(), 1);
        assert_eq!(unread[0].attachments, vec![image()]);
        let wire = serde_json::to_value(&unread[0]).expect("a message serializes");
        assert_eq!(
            wire["attachments"][0]["path"],
            ".build/attachments/ab12cd34-screenshot.png"
        );
    }

    /// A resumed agent rebuilds the conversation from the catch-up packet, not
    /// from its mailbox — so a file sent to a previous session has to be named
    /// there too, or it silently stops existing across a restart.
    #[test]
    fn the_catch_up_packet_still_names_the_files_a_message_carried() {
        let mut thread = Thread::new("run-1");
        thread.post_user_with_attachments(
            "look at this",
            None,
            vec![image()],
            "2026-08-09T13:00:00Z",
        );
        let catch_up = thread.catch_up_markdown(40);
        assert!(
            catch_up.contains(".build/attachments/ab12cd34-screenshot.png"),
            "{catch_up}"
        );
    }

    /// Every message written before attachments existed must still load, and a
    /// message without them must not pay for the field on the wire.
    #[test]
    fn a_message_without_attachments_carries_no_attachment_field() {
        let mut thread = Thread::new("run-1");
        thread.post_user("no files here", None, "2026-08-09T13:00:00Z");
        let wire = serde_json::to_value(&thread).expect("a thread serializes");
        assert!(!wire.to_string().contains("attachments"), "{wire:?}",);
        let reloaded: Thread = serde_json::from_value(wire).expect("a thread reloads");
        assert_eq!(reloaded, thread);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thread_with_conversation() -> Thread {
        let mut thread = Thread::new("plan-1");
        thread.post_user("please rename the helper", None, "2026-07-24T12:00:00Z");
        thread.post_agent("Which name do you prefer?", None, "2026-07-24T12:01:00Z");
        thread.push_event(
            ThreadEventKind::Done,
            Some("Agent reported done".to_string()),
            None,
            None,
            "2026-07-24T12:02:00Z",
        );
        thread
    }

    #[test]
    fn digest_value_is_bounded_and_omits_bodies() {
        let mut thread = thread_with_conversation();
        thread.post_user("a second unread ask", None, "2026-07-24T12:03:00Z");
        let digest = thread.digest_value();

        assert_eq!(digest["id"], "thread:plan-1");
        assert_eq!(digest["agent"]["id"], "agent:plan-1");
        assert_eq!(digest["item_count"], 4);
        assert_eq!(digest["last_sequence"], 4);
        assert_eq!(digest["last_event"]["event"], "done");
        assert_eq!(digest["last_event"]["created_at"], "2026-07-24T12:02:00Z");
        // The bounded contract: no item array, no message bodies anywhere, and
        // no counters nothing consumes (unseen_user_messages was dead payload).
        assert!(digest.get("unseen_user_messages").is_none(), "{digest:?}");
        assert!(digest.get("items").is_none(), "{digest:?}");
        let serialized = digest.to_string();
        assert!(!serialized.contains("please rename the helper"));
        assert!(!serialized.contains("a second unread ask"));
    }

    #[test]
    fn digest_value_of_an_empty_thread_has_zero_counters_and_no_event() {
        let digest = Thread::new("plan-empty").digest_value();
        assert_eq!(digest["item_count"], 0);
        assert_eq!(digest["last_sequence"], 0);
        assert!(digest["last_event"].is_null(), "{digest:?}");
    }

    #[test]
    fn wire_value_after_ships_only_newer_items_with_totals() {
        let thread = thread_with_conversation();
        let delta = thread.wire_value_after(1);

        let items = delta["items"].as_array().unwrap();
        assert_eq!(items.len(), 2, "{items:?}");
        assert_eq!(items[0]["data"]["sequence"], 2);
        assert_eq!(items[1]["data"]["sequence"], 3);
        assert_eq!(delta["thread_total"], 3);
        assert_eq!(delta["thread_last_sequence"], 3);
        // The small bounded companions still ship in full.
        assert!(delta["sessions"].is_array());
        assert!(delta["revisions"].is_array());
    }

    #[test]
    fn wire_value_after_past_the_end_is_an_empty_delta_not_an_error() {
        let thread = thread_with_conversation();
        let delta = thread.wire_value_after(9_999);
        assert_eq!(delta["items"].as_array().unwrap().len(), 0);
        assert_eq!(delta["thread_total"], 3);
        assert_eq!(delta["thread_last_sequence"], 3);
    }

    #[test]
    fn wire_value_after_reships_a_message_marked_seen_after_the_cursor() {
        let mut thread = Thread::new("plan-1");
        thread.post_user("please rename the helper", None, "2026-07-24T12:00:00Z");
        let cursor = thread.last_sequence();
        thread.read_unread("2026-07-24T12:05:00Z");

        let delta = thread.wire_value_after(cursor);
        let items = delta["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0]["data"]["seen_at"], "2026-07-24T12:05:00Z");
        // The mutation advances the high-water mark so the client's next
        // cursor moves past it instead of re-requesting the item forever.
        let bumped = delta["thread_last_sequence"].as_u64().unwrap();
        assert!(bumped > cursor, "{delta:?}");
        let drained = thread.wire_value_after(bumped);
        assert_eq!(drained["items"].as_array().unwrap().len(), 0, "{drained:?}");
    }

    #[test]
    fn wire_value_after_reships_a_message_resolved_by_a_later_revision() {
        let mut thread = Thread::new("plan-1");
        let anchor = MessageAnchor {
            artifact: ArtifactKind::Plan,
            revision_id: None,
            path: None,
            side: None,
            line_start: None,
            line_end: None,
            heading_path: Vec::new(),
            snippet: "old wording".to_string(),
        };
        thread.post_user("tighten this", Some(anchor), "2026-07-24T12:00:00Z");
        let cursor = thread.last_sequence();
        let revision = thread.add_revision(ArtifactKind::Plan, "rewritten", "2026-07-24T12:06:00Z");

        let delta = thread.wire_value_after(cursor);
        let items = delta["items"].as_array().unwrap();
        assert!(
            items
                .iter()
                .any(|item| item["data"]["resolved_by_revision"] == json!(revision.id)),
            "{items:?}"
        );
        let bumped = delta["thread_last_sequence"].as_u64().unwrap();
        assert!(bumped > cursor, "{delta:?}");
        let drained = thread.wire_value_after(bumped);
        assert_eq!(drained["items"].as_array().unwrap().len(), 0, "{drained:?}");
    }

    #[test]
    fn the_done_event_is_the_whole_wire_record_of_a_completion() {
        let mut thread = Thread::new("run-done");
        thread.post_agent("here is what I found", None, "2026-07-24T11:00:00Z");
        thread.post_completion("Implemented the change", None, "2026-07-24T12:00:00Z");

        let wire = thread.wire_value();
        assert_eq!(wire["items"][0]["type"], "message");
        assert!(wire["items"][0]["data"].get("source").is_none(), "{wire:?}");
        assert_eq!(wire["items"][1]["type"], "event");
        assert_eq!(wire["items"][1]["data"]["event"], "done");
        assert_eq!(
            wire["items"][1]["data"]["summary"],
            "Implemented the change"
        );
        assert_eq!(wire["items"].as_array().unwrap().len(), 2, "{wire:?}");
    }

    #[test]
    fn message_and_event_links_round_trip_on_the_wire() {
        let mut thread = Thread::new("run-linked");
        thread.post_agent_with_links(
            "The parser and its tests changed.",
            None,
            vec![ThreadLink::File {
                path: "src/parser.rs".to_string(),
                line_start: Some(12),
                line_end: Some(24),
            }],
            "2026-07-24T12:00:00Z",
        );
        thread.push_event_with_links(
            ThreadEventKind::StageStarted,
            Some("Started stage Parser".to_string()),
            None,
            None,
            vec![ThreadLink::PlanStage {
                plan_id: "plan-1".to_string(),
                stage_id: "parser".to_string(),
                path: ".build/plan/01-parser.md".to_string(),
            }],
            "2026-07-24T12:01:00Z",
        );

        let wire = thread.wire_value();
        assert_eq!(wire["items"][0]["data"]["links"][0]["kind"], "file");
        assert_eq!(
            wire["items"][0]["data"]["links"][0]["path"],
            "src/parser.rs"
        );
        assert_eq!(wire["items"][1]["data"]["links"][0]["kind"], "plan_stage");
        assert_eq!(wire["items"][1]["data"]["links"][0]["stage_id"], "parser");

        let canonical = vec![
            ThreadLink::IssueStage {
                issue_id: "issue-1".into(),
                stage_id: "parser".into(),
                path: ".build/plan/01-parser.md".into(),
            },
            ThreadLink::Implementation {
                issue_id: "issue-1".into(),
                implementation_id: "run-1".into(),
            },
            ThreadLink::Worktree {
                worktree_id: "wt-0123456789ab".into(),
            },
            ThreadLink::Commit {
                sha: "a".repeat(40),
            },
            ThreadLink::Recovery {
                recovery_id: "recovery-1".into(),
            },
        ];
        let value = serde_json::to_value(&canonical).unwrap();
        assert_eq!(value[0]["kind"], "issue_stage");
        assert_eq!(value[1]["kind"], "implementation");
        assert_eq!(value[2]["kind"], "worktree");
        assert_eq!(value[3]["kind"], "commit");
        assert_eq!(value[4]["kind"], "recovery");
        assert_eq!(
            serde_json::from_value::<Vec<ThreadLink>>(value).unwrap(),
            canonical
        );
    }
}

#[cfg(test)]
mod doc_comment_tests {
    use super::*;

    const NOW: &str = "2026-08-13T09:00:00Z";
    const STAGE_PATH: &str = ".build/plan/01-database-schema.md";

    fn passage() -> DocAnchor {
        DocAnchor {
            heading_path: vec!["Database schema".to_string(), "Tables".to_string()],
            snippet: "users table gets a soft-delete column".to_string(),
            line_start: Some(12),
            line_end: Some(14),
        }
    }

    fn commented_stage() -> Thread {
        let mut thread = Thread::for_agent("agent-1");
        thread.post_doc_comment(
            "issue-1",
            "database-schema",
            STAGE_PATH,
            Some(passage()),
            "use a deleted_at timestamp, not a boolean",
            NOW,
        );
        thread
    }

    /// A plan-doc comment is a post like any other: an anchored user message on
    /// the conversation, not a record beside it.
    #[test]
    fn a_doc_comment_is_an_anchored_post_on_the_conversation() {
        let thread = commented_stage();
        assert_eq!(thread.items.len(), 1, "{:?}", thread.items);
        let ThreadItem::Message(message) = &thread.items[0] else {
            panic!("a comment is a message: {:?}", thread.items);
        };
        assert_eq!(message.role, MessageRole::User);
        let anchor = message.anchor.as_ref().expect("an anchored comment");
        assert_eq!(anchor.artifact, ArtifactKind::Doc);
        assert_eq!(anchor.path.as_deref(), Some(STAGE_PATH));
        assert_eq!(anchor.line_start, Some(12));
        assert_eq!(anchor.line_end, Some(14));
        assert_eq!(anchor.snippet, passage().snippet);
        assert!(message.links.contains(&ThreadLink::IssueStage {
            issue_id: "issue-1".to_string(),
            stage_id: "database-schema".to_string(),
            path: STAGE_PATH.to_string(),
        }));
        // Posting it derives the same findability as any other item.
        assert_eq!(message.metadata.stages, vec!["database-schema".to_string()]);
        assert_eq!(message.metadata.files, vec![STAGE_PATH.to_string()]);

        let comments = thread.doc_comments();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].id, message.id);
        assert_eq!(comments[0].stage_id, "database-schema");
        assert_eq!(comments[0].path, STAGE_PATH);
        assert_eq!(
            comments[0].body,
            "use a deleted_at timestamp, not a boolean"
        );
        assert_eq!(comments[0].state, DocCommentState::Open);
        assert_eq!(comments[0].anchor.as_ref(), Some(&passage()));
        assert_eq!(comments[0].created_at, NOW);
    }

    /// A comment on the stage as a whole anchors to the document, not a passage
    /// — and it is still that stage's comment.
    #[test]
    fn a_general_comment_points_at_the_document_rather_than_a_passage() {
        let mut thread = Thread::for_agent("agent-1");
        thread.post_doc_comment(
            "issue-1",
            "database-schema",
            STAGE_PATH,
            None,
            "this stage is too big",
            NOW,
        );
        let comments = thread.doc_comments();
        assert_eq!(comments.len(), 1);
        assert!(comments[0].anchor.is_none(), "{:?}", comments[0]);
        assert_eq!(comments[0].path, STAGE_PATH);
        assert_eq!(
            thread.open_doc_comments_for("database-schema").len(),
            1,
            "a general comment is still open work on the stage"
        );
    }

    /// Ordinary conversation is not a comment, and one stage's comments are not
    /// another's.
    #[test]
    fn open_comments_are_scoped_to_their_stage() {
        let mut thread = commented_stage();
        thread.post_doc_comment(
            "issue-1",
            "api-surface",
            ".build/plan/02-api-surface.md",
            None,
            "name the endpoint after the resource",
            NOW,
        );
        thread.post_user("unrelated direction", None, NOW);
        thread.post_agent("and an answer", None, NOW);

        assert_eq!(thread.doc_comments().len(), 2);
        let open = thread.open_doc_comments_for("api-surface");
        assert_eq!(open.len(), 1, "{open:?}");
        assert_eq!(open[0].stage_id, "api-surface");
    }

    /// The agent's answer lands on the comment it answers, which closes it —
    /// and the mutation bumps the cursor so a polling client re-ships it.
    #[test]
    fn resolving_a_comment_records_the_reply_and_closes_it() {
        let mut thread = commented_stage();
        let id = thread.doc_comments()[0].id.clone();
        let before = thread.last_sequence();

        assert!(thread.resolve_doc_comment(&id, "Switched to deleted_at."));
        let comment = &thread.doc_comments()[0];
        assert_eq!(comment.state, DocCommentState::Addressed);
        assert_eq!(
            comment.agent_reply.as_deref(),
            Some("Switched to deleted_at.")
        );
        assert!(thread.open_doc_comments_for("database-schema").is_empty());
        assert!(thread.last_sequence() > before, "the cursor moves");

        assert!(
            !thread.resolve_doc_comment("message-404", "nothing to answer"),
            "an unknown comment resolves nothing"
        );
        assert!(
            !thread.resolve_doc_comment(&id, "again"),
            "an addressed comment is not re-answered"
        );
    }

    /// Deleting a comment deletes the post: there is nowhere else it lives.
    #[test]
    fn removing_an_open_comment_takes_the_post_with_it() {
        let mut thread = commented_stage();
        let id = thread.doc_comments()[0].id.clone();

        let removed = thread.remove_doc_comment(&id).expect("an open comment");
        assert_eq!(removed.id, id);
        assert!(thread.items.is_empty(), "{:?}", thread.items);
        assert!(thread.remove_doc_comment(&id).is_none(), "already gone");

        let mut thread = commented_stage();
        let id = thread.doc_comments()[0].id.clone();
        thread.resolve_doc_comment(&id, "done");
        assert!(
            thread.remove_doc_comment(&id).is_none(),
            "an answered comment is history, not a draft"
        );
    }

    #[test]
    fn a_doc_anchor_says_doc_on_the_wire() {
        assert_eq!(ArtifactKind::Doc.as_str(), "doc");
        assert_eq!(
            serde_json::to_value(ArtifactKind::Doc).unwrap(),
            json!("doc")
        );
        let thread = commented_stage();
        let wire = thread.wire_value();
        assert_eq!(wire["items"][0]["data"]["anchor"]["artifact"], "doc");
        assert_eq!(wire["items"][0]["data"]["anchor"]["line_start"], 12);
        let reloaded: Thread = serde_json::from_value(json!({
            "id": thread.id,
            "agent": thread.agent,
            "items": thread.items,
        }))
        .expect("a conversation carrying a doc comment reloads");
        assert_eq!(reloaded.doc_comments(), thread.doc_comments());
    }

    /// Every message written before doc comments existed loads, and none of
    /// them pays for the reply field.
    #[test]
    fn a_message_written_before_replies_existed_carries_none() {
        let mut thread = Thread::for_agent("agent-1");
        thread.post_user("plain direction", None, NOW);
        let wire = thread.wire_value();
        assert!(
            wire["items"][0]["data"].get("agent_reply").is_none(),
            "{wire:?}"
        );
        assert!(thread.doc_comments().is_empty());
    }
}
