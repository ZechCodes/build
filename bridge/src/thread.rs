//! Durable conversation threads paired with plans and run diffs.
//!
//! A thread belongs to a stable logical agent identity (the Build-owned plan or
//! run), while individual harness processes are recorded as session lineage.
//! Messages cost agent tokens; events and revision links do not.

use std::borrow::Borrow;
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

/// What an agent reported through `done`, as a status on the message it posted.
///
/// The message is the whole record of an outcome — there is no companion event
/// — so this is what tells an outcome from an ordinary reply, both on the wire
/// and in the catch-up packet a replacement agent is handed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageOutcome {
    Completed,
    Blocked,
    Failed,
}

impl MessageOutcome {
    /// The stable wire token, byte-identical to how it serializes.
    pub fn as_str(self) -> &'static str {
        match self {
            MessageOutcome::Completed => "completed",
            MessageOutcome::Blocked => "blocked",
            MessageOutcome::Failed => "failed",
        }
    }

    /// Why this outcome needs the human, as the same token the event it
    /// replaced used to answer with — so an inbox row reads exactly as it did.
    fn attention_reason(self) -> &'static str {
        match self {
            MessageOutcome::Completed => ThreadEventKind::Done.as_str(),
            MessageOutcome::Blocked => ThreadEventKind::Blocked.as_str(),
            MessageOutcome::Failed => ThreadEventKind::RunFailed.as_str(),
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Never mutated, for the counter an event carries only once it has been: an
/// event that was written and never touched again serializes exactly as it did
/// before events could mutate at all.
fn is_zero(value: &u64) -> bool {
    *value == 0
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

/// One action an agent suggested the reviewer take in answer to its message.
///
/// `label` is the whole of what the chip says, and `message` is what the agent
/// is told when the chip is pressed — fuller than the label, so a choice still
/// carries its reasoning into a session that has forgotten the conversation.
/// Absent, the label is the message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl MessageOption {
    /// What choosing this option says to the agent.
    fn reply_text(&self) -> &str {
        self.message.as_deref().unwrap_or(&self.label)
    }
}

/// How many actions one message may suggest. A dozen chips is a menu, and a
/// menu is what the composer is for.
pub const MAX_MESSAGE_OPTIONS: usize = 6;

/// What the reviewer submitted: which offer, and which of its options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptionChoice {
    pub message_id: String,
    pub option_ids: Vec<String>,
}

/// An option as the agent offers it: everything but the id, which is Build's
/// to give — the same split the router's capture options make.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageOptionDraft {
    pub label: String,
    #[serde(default)]
    pub message: Option<String>,
}

/// The agent's offer, numbered and checked. Refused when there are too many,
/// when one carries no label, or when a label is too long to read on a chip.
pub fn numbered_message_options(
    drafts: &[MessageOptionDraft],
) -> Result<Vec<MessageOption>, String> {
    if drafts.len() > MAX_MESSAGE_OPTIONS {
        return Err(format!(
            "at most {MAX_MESSAGE_OPTIONS} options can be suggested with a message; {} were",
            drafts.len()
        ));
    }
    drafts
        .iter()
        .enumerate()
        .map(|(index, draft)| {
            let label = draft.label.trim();
            if label.is_empty() {
                return Err("an option with no label is nothing the user can choose".to_string());
            }
            if label.chars().count() > MAX_OPTION_LABEL_CHARS {
                return Err(format!(
                    "an option label is at most {MAX_OPTION_LABEL_CHARS} characters; {:?} is longer",
                    label
                ));
            }
            let message = draft
                .message
                .as_deref()
                .map(str::trim)
                .filter(|message| !message.is_empty())
                .map(str::to_string);
            if message.as_ref().is_some_and(|message| message.len() > MAX_OPTION_MESSAGE_BYTES) {
                return Err(format!(
                    "an option message exceeds {MAX_OPTION_MESSAGE_BYTES} bytes"
                ));
            }
            Ok(MessageOption {
                id: format!("option-{}", index + 1),
                label: label.to_string(),
                message,
            })
        })
        .collect()
}

/// How long a chip may be. Past this it is a paragraph wearing a button.
pub const MAX_OPTION_LABEL_CHARS: usize = 80;

/// How much an option may say to the agent when it is chosen.
pub const MAX_OPTION_MESSAGE_BYTES: usize = 4_000;

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
    /// Completion is metadata on an otherwise ordinary message. Set by a
    /// completed [`outcome`](Self::outcome) and by nothing else, so a client
    /// that knows only this field renders a completion exactly as it always
    /// has.
    #[serde(default, skip_serializing_if = "is_false")]
    pub done: bool,
    /// The outcome this message reports, when the agent's `done` is what wrote
    /// it. Absent on every ordinary message, and on every message written
    /// before outcomes were message statuses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<MessageOutcome>,
    /// The agent's structured handoff, on the message reporting the outcome it
    /// came with. Only an outcome carries one, and only when the agent wrote
    /// one — every other message omits the field entirely.
    ///
    /// Boxed: a report is four vectors and the rarest field on the largest kind
    /// of thread item, so every ordinary message would otherwise carry its bulk
    /// through every conversation the daemon holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_report: Option<Box<CompletionReport>>,
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
    /// Actions the agent suggested the reviewer take in answer to this message.
    /// Only an agent message carries them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<MessageOption>,
    /// Which of those options the reviewer submitted. Empty until they do, and
    /// what the chat renders as the selection afterwards — the choice is
    /// recorded here rather than on the reply, because the offer is the only
    /// place it is shown.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_options: Vec<String>,
    /// The offer this message answers, when it is an option submission. The
    /// chat draws the choice on those chips and nothing for this message, so
    /// pressing a suggestion reads as pressing it rather than as typing what it
    /// said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers_options_of: Option<String>,
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
    /// A triage pass classified the diff. Status, never attention: triage
    /// orders what the reviewer reads and asks nothing of them.
    Triaged,
    /// The reviewer disagreed with how a hunk was classified. Status: the
    /// agent is told, because miscalibration is only visible if it is said out
    /// loud, but nothing is being asked of anyone — the reviewer has already
    /// done the thing they wanted to do.
    TriageOverridden,
    /// A daemon restart killed the session mid-work. The entity is parked and
    /// waiting for the human to restart it.
    Interrupted,
    /// The agent thought out loud.
    Reasoning,
    /// The agent called a tool.
    ToolUse,
    /// A tool answered.
    ToolResult,
    /// The agent narrated. Distinct from a `post_thread_message`, which is the
    /// agent deliberately addressing the human.
    Narration,
    /// Background work the harness runs beyond the turn moved — started,
    /// finished, failed, or said something worth reading.
    ///
    /// The conversation is the only visibility a human has into a headless
    /// agent, and a task that outlives the turn that started it would otherwise
    /// be work nothing in the timeline says exists.
    TaskUpdate,
}

impl ThreadEventKind {
    /// Every variant, so the wire-token and class rules can be checked over the
    /// whole enum instead of a sample of it.
    pub const ALL: [ThreadEventKind; 37] = [
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
        ThreadEventKind::Triaged,
        ThreadEventKind::TriageOverridden,
        ThreadEventKind::Interrupted,
        ThreadEventKind::Reasoning,
        ThreadEventKind::ToolUse,
        ThreadEventKind::ToolResult,
        ThreadEventKind::Narration,
        ThreadEventKind::TaskUpdate,
    ];

    /// Whether this event is the agent working rather than something said or
    /// decided.
    ///
    /// Activity is what a conversation folds: the thinking, the tool calls and
    /// their answers, the narration, and the background tasks that outlive a
    /// turn. Everything else — a message, a lifecycle marker, a call for the
    /// human — ends a run of it. The web client folds the same five kinds, and
    /// a test over [`ALL`](Self::ALL) holds the two readings equal.
    pub fn is_activity(self) -> bool {
        matches!(
            self,
            ThreadEventKind::Reasoning
                | ThreadEventKind::ToolUse
                | ThreadEventKind::ToolResult
                | ThreadEventKind::Narration
                | ThreadEventKind::TaskUpdate
        )
    }

    /// Whether this event needs the human, or merely tells them where things
    /// got to.
    ///
    /// Attention is what an agent hands back: it finished, it stopped, it went
    /// quiet, or the work reached an outcome that ends the entry. Status is the
    /// work happening — sessions opening and closing, stages moving, commits
    /// landing, revisions appearing, checkouts being made and recovered, and
    /// the agent's own reasoning, tool calls and narration. An agent thinking
    /// out loud is the work happening, so it never marks the entry unread; the
    /// agent choosing to address the human is a message, which does.
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
            | ThreadEventKind::Triaged
            | ThreadEventKind::TriageOverridden
            | ThreadEventKind::Reasoning
            | ThreadEventKind::ToolUse
            | ThreadEventKind::ToolResult
            | ThreadEventKind::Narration
            | ThreadEventKind::TaskUpdate
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
            ThreadEventKind::Triaged => "triaged",
            ThreadEventKind::TriageOverridden => "triage_overridden",
            ThreadEventKind::Interrupted => "interrupted",
            ThreadEventKind::Reasoning => "reasoning",
            ThreadEventKind::ToolUse => "tool_use",
            ThreadEventKind::ToolResult => "tool_result",
            ThreadEventKind::Narration => "narration",
            ThreadEventKind::TaskUpdate => "task_update",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadEvent {
    pub id: String,
    pub sequence: u64,
    /// Drawn from the same counter as `sequence` and bumped when the event
    /// mutates in place — today that is a tool call's answer arriving — so the
    /// cursor protocol re-ships the newer copy of an already-held row.
    /// Defaults to 0 (never mutated) on every record persisted before this
    /// field, which leaves old rows untouched: `latest_sequence` is a max.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub updated_sequence: u64,
    pub event: ThreadEventKind,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// What a tool call's answer reported, on the `ToolUse` row it completes.
    /// Absent on every event that is not a completed tool call, and on every
    /// event written before calls and answers were one row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ToolCallOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<ThreadLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_sequence: Option<u64>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadEventDraft {
    pub event: ThreadEventKind,
    pub summary: Option<String>,
    pub session_id: Option<String>,
    pub revision_id: Option<String>,
    pub links: Vec<ThreadLink>,
    pub parent_sequence: Option<u64>,
}

/// How a tool call ended, on the row the call minted.
///
/// The conversation's own mirror of the harness enum, in the manner of the kind
/// mapping: what a client reads off the wire belongs to the thread, and the
/// harness stays free to name what it saw in its own words. `Unanswered` is a
/// terminal state of its own — no answer ever arrived and the boundary that
/// ended the call said so — never a failure, which the agent would have been
/// told about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallOutcome {
    Ok,
    Error,
    Unanswered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ThreadItem {
    Message(ThreadMessage),
    Event(ThreadEvent),
}

/// How much conversation a wire view carries: `Digest` for the polled list
/// surfaces (board.list / plan.list re-ship every entity every ~2.5s, so a
/// full thread there grows without bound), `Page` for a caller that said how
/// much it can hold — the newest items of the conversation, with everything
/// older a scroll-back away — and `Full` for one that said nothing.
///
/// `Full` is not a fallback, it is the older contract kept: a client written
/// before paging holds the conversation entire and checks every delta against
/// `thread_total`, so a window handed to it unasked is a count it can never
/// match again. Bounding is therefore the caller's to ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadDetail {
    Digest,
    Full,
    Page(usize),
}

#[cfg(test)]
thread_local! {
    /// Conversation items this OS thread has put through serde on its way to a
    /// wire payload, since the process started.
    ///
    /// A payload that was built whole and then thrown away is byte-identical to
    /// one that was never built, so nothing about the answer can tell the two
    /// apart — only the count can. Paging exists to keep this from growing with
    /// conversation length on a steady-state poll, and the tests that hold that
    /// promise read it here. Per-OS-thread rather than global so tests running
    /// side by side do not read each other's work.
    static ITEMS_SERIALIZED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many conversation items this OS thread has serialized into wire
/// payloads — read before and after a call to measure what it cost.
#[cfg(test)]
pub fn items_serialized() -> usize {
    ITEMS_SERIALIZED.with(std::cell::Cell::get)
}

#[cfg(test)]
fn count_serialized_items(count: usize) {
    ITEMS_SERIALIZED.with(|counter| counter.set(counter.get() + count));
}

/// Nothing is counted outside the tests: only they ask what a payload cost.
#[cfg(not(test))]
fn count_serialized_items(_count: usize) {}

impl ThreadMessage {
    /// The outcome this message reports, reading a record written before the
    /// field existed too: such a record set `done` alone, which is a completion
    /// and has always been one.
    pub fn reported_outcome(&self) -> Option<MessageOutcome> {
        self.outcome
            .or_else(|| self.done.then_some(MessageOutcome::Completed))
    }
}

impl ThreadItem {
    pub fn sequence(&self) -> u64 {
        match self {
            ThreadItem::Message(message) => message.sequence,
            ThreadItem::Event(event) => event.sequence,
        }
    }

    /// The newest counter value this item has touched: its creation sequence,
    /// or a later in-place mutation bump.
    ///
    /// Both arms read the same way, and every cursor path in this file reads
    /// the mutation through this one place — an event bumped by its tool call's
    /// answer travels exactly as a message marked seen does.
    pub fn latest_sequence(&self) -> u64 {
        match self {
            ThreadItem::Message(message) => message.sequence.max(message.updated_sequence),
            ThreadItem::Event(event) => event.sequence.max(event.updated_sequence),
        }
    }

    /// Why this item needs the human, as a stable token — `None` when it does
    /// not.
    ///
    /// An agent message is the agent handing the turn back, so it needs
    /// reading; a progress note explicitly keeps the turn, so it does not. A
    /// message the human wrote themselves never needs their attention. A
    /// message reporting an outcome names it with the token the event it
    /// replaced answered with, so an entry says why it needs reading in the
    /// same words it always did.
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
                Some(match message.reported_outcome() {
                    Some(outcome) => outcome.attention_reason(),
                    None => AGENT_MESSAGE_REASON,
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

    /// Whether the human reads this item as conversation — the one predicate
    /// the two bounds count with.
    ///
    /// A message is conversation whoever wrote it, an outcome included since
    /// an outcome is a message. An event counts only when it is Build calling
    /// the human. Everything else rides free: the four activity kinds, and the
    /// quiet lifecycle markers with them. So a page's limit buys conversation,
    /// and a catch-up packet's limit buys what was said, however much work
    /// happened between two words.
    ///
    /// The store filters the same rule as `message = 1 OR attention = 1` over
    /// two hoisted columns, and a test holds the two readings equal across
    /// every kind.
    pub fn counted(&self) -> bool {
        matches!(self, ThreadItem::Message(_)) || self.attention_reason().is_some()
    }

    /// Whether this item is the agent working — the rule a page's activity
    /// runs are cut on.
    ///
    /// A message is never activity, whoever wrote it: a run of work ends the
    /// moment somebody says something.
    pub fn is_activity(&self) -> bool {
        match self {
            ThreadItem::Event(event) => event.event.is_activity(),
            ThreadItem::Message(_) => false,
        }
    }

    /// The tool call this item is, or `None` — the one place that answers
    /// "is this a tool call".
    ///
    /// A call and its answer are one row, so the call carries the outcome and
    /// there is nothing else to join it to.
    fn tool_call(&self) -> Option<&ThreadEvent> {
        match self {
            ThreadItem::Event(event) if event.event == ThreadEventKind::ToolUse => Some(event),
            _ => None,
        }
    }

    /// Whether this item is a tool call — what a folded run of activity counts,
    /// and what the store hoists into its `tool_call` column.
    pub fn is_tool_call(&self) -> bool {
        self.tool_call().is_some()
    }

    /// Whether this item spends a page's budget — the page measure, and only
    /// that.
    ///
    /// A page's limit buys MESSAGES, either role, an outcome among them since
    /// an outcome is a message. Everything else on a conversation rides free:
    /// the activity between two messages, and the lifecycle and attention
    /// events with it. So the page a reviewer opens on is always the last
    /// `limit` things anybody said, however much work and however many
    /// milestones happened between them.
    ///
    /// Distinct from [`counted`](Self::counted), which is what an unread badge
    /// and a catch-up packet measure: an attention event still calls the human
    /// even though it costs a page nothing.
    ///
    /// The store filters the same rule as `message = 1` over the hoisted
    /// column.
    pub fn counts_toward_page(&self) -> bool {
        matches!(self, ThreadItem::Message(_))
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
            ThreadItem::Message(message) => {
                completion_text(&message.body, message.completion_report.as_deref())
            }
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

/// How many MESSAGES a page carries when the caller asks for one without
/// saying how large.
///
/// The limit buys what was said, so twenty is twenty turns of conversation —
/// the sitting a reviewer opens onto — however much work happened between
/// them. Everything older is a page up, which is the point: a conversation of
/// hundreds no longer ships whole to show its last hour.
pub const DEFAULT_THREAD_PAGE: usize = 20;

/// The most conversation one page ships, however large a limit it asks for.
///
/// A scroll-back that asks for the whole conversation at once is the thing
/// paging exists to prevent, so the cap holds even when the caller means well.
pub const MAX_THREAD_PAGE: usize = 200;

/// How many items of any ONE run of activity a page ships.
///
/// A run is folded to a single row, so what the wire has to carry is the newest
/// of it — the last thing the agent did, and enough above it to read as work.
/// The digest beside the page carries the truth about the rest: how many calls
/// the run made, and which one was last. Items past the cap are omitted, which
/// is why a page is not a contiguous run of sequences.
pub const PAGE_ACTIVITY_RUN_CAP: usize = 100;

/// The newest tool call of an activity run, as a folded row prints it.
///
/// A fixed shape: `summary` and `outcome` serialize as `null` when the call
/// carries none, so the client falls back rather than reading around an absent
/// field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LastToolCall {
    pub sequence: u64,
    pub created_at: String,
    pub summary: Option<String>,
    pub outcome: Option<ToolCallOutcome>,
}

impl LastToolCall {
    fn of(event: &ThreadEvent) -> Self {
        LastToolCall {
            sequence: event.sequence,
            created_at: event.created_at.clone(),
            summary: event.summary.clone(),
            outcome: event.outcome,
        }
    }
}

/// What one folded run of activity amounts to, whatever a page shipped of it.
///
/// The count is the fact only the bridge can see: a client counts the rows it
/// was handed, and the cap means those are not all the rows there were. So the
/// span is named — `[from_sequence, through_sequence]`, the run's own first and
/// last item — and the count is exact over it, cap-omitted items included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActivityDigest {
    pub from_sequence: u64,
    pub through_sequence: u64,
    pub tool_calls: u64,
    pub last_tool_call: Option<LastToolCall>,
}

/// A page: what ships, and what the runs inside it mean. Held together because
/// they are one answer — items alone would say a run was a hundred calls long.
pub struct PageCut<T> {
    pub items: Vec<T>,
    pub digests: Vec<ActivityDigest>,
}

/// Cut a page out of the span it reaches over: keep everything that is not
/// activity, keep the newest [`PAGE_ACTIVITY_RUN_CAP`] of every run that is,
/// and answer for each run with a digest.
///
/// Generic over how the caller holds an item, so neither page path has to take
/// its span apart and put it back together: memory passes borrows off its
/// resident tail, the store passes the items it just decoded, and both get the
/// same cut back. Takes the span newest-first — the order both paths read in —
/// and hands the items back oldest-first, the order a conversation is rendered
/// in.
///
/// `tool_calls_between` is the census, and the only thing that differs between
/// reading memory and reading SQLite. It is asked for the run's whole span,
/// which both callers can answer exactly: a run on a page never reaches below
/// the span, because the span's oldest item is a message and a message ends a
/// run.
pub fn cut_activity_runs<T, E>(
    span_newest_first: Vec<T>,
    tool_calls_between: impl Fn(u64, u64) -> Result<u64, E>,
) -> Result<PageCut<T>, E>
where
    T: Borrow<ThreadItem>,
{
    let mut cut = PageCut {
        items: Vec::with_capacity(span_newest_first.len()),
        digests: Vec::new(),
    };
    let mut run: Vec<T> = Vec::new();
    for item in span_newest_first {
        if item.borrow().is_activity() {
            run.push(item);
            continue;
        }
        cut = fold_activity_run(cut, std::mem::take(&mut run), &tool_calls_between)?;
        cut.items.push(item);
    }
    let mut cut = fold_activity_run(cut, run, &tool_calls_between)?;
    cut.items.reverse();
    cut.digests.reverse();
    Ok(cut)
}

/// Close one run onto the cut: its digest, then the newest of its items. Both
/// the run and the cut being built are newest-first, so the caller reverses
/// once at the end rather than per run.
fn fold_activity_run<T, E>(
    mut cut: PageCut<T>,
    run_newest_first: Vec<T>,
    tool_calls_between: &impl Fn(u64, u64) -> Result<u64, E>,
) -> Result<PageCut<T>, E>
where
    T: Borrow<ThreadItem>,
{
    let (Some(newest), Some(oldest)) = (run_newest_first.first(), run_newest_first.last()) else {
        return Ok(cut);
    };
    let from_sequence = oldest.borrow().sequence();
    let through_sequence = newest.borrow().sequence();
    cut.digests.push(ActivityDigest {
        from_sequence,
        through_sequence,
        tool_calls: tool_calls_between(from_sequence, through_sequence)?,
        last_tool_call: run_newest_first
            .iter()
            .find_map(|item| item.borrow().tool_call().map(LastToolCall::of)),
    });
    cut.items
        .extend(run_newest_first.into_iter().take(PAGE_ACTIVITY_RUN_CAP));
    Ok(cut)
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
    /// How much of this conversation was left in the store, and where the part
    /// that was read of it starts.
    ///
    /// `items` is the tail of a conversation, not necessarily the whole of it:
    /// a load reads a bounded window and leaves the history behind until a
    /// page asks for it. Both are zero for a conversation held whole — one
    /// built in this process, or one short enough that its tail is all there
    /// is. Never persisted: they describe this process's view of the
    /// conversation, not the conversation.
    #[serde(skip)]
    earlier_item_count: u64,
    #[serde(skip)]
    resident_from_sequence: u64,
    /// The newest counter value the store held for this conversation when the
    /// load read its tail. Nothing under that tail can move again in this
    /// process — only the items it holds can — so this is the exact point past
    /// which the tail is the whole answer to a cursor.
    #[serde(skip)]
    stored_last_sequence: u64,
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
            body.into(),
            anchor,
            links,
            now.into(),
            still_working,
        )
    }

    /// An agent message that offers the reviewer a set of actions to take.
    ///
    /// The options ride the message rather than living beside it: they are the
    /// end of what the agent said, they go stale the moment anything else is
    /// said, and the reviewer's answer is drawn on them.
    #[allow(clippy::too_many_arguments)]
    pub fn post_agent_offering(
        &mut self,
        body: impl Into<String>,
        anchor: Option<MessageAnchor>,
        links: Vec<ThreadLink>,
        options: Vec<MessageOption>,
        now: impl Into<String>,
        still_working: bool,
    ) -> String {
        let id = self.post_agent_with_links_working(body, anchor, links, now, still_working);
        if let Some(ThreadItem::Message(message)) = self.items.last_mut() {
            message.options = options;
        }
        id
    }

    /// The message whose options the reviewer may still act on — the newest
    /// message on the thread, if it offered any and none were chosen.
    ///
    /// Newest MESSAGE, not newest item: an offer stands until somebody says
    /// something, and a commit landing is not somebody saying something.
    pub fn open_offer(&self) -> Option<&ThreadMessage> {
        self.items
            .iter()
            .rev()
            .find_map(|item| match item {
                ThreadItem::Message(message) => Some(message),
                ThreadItem::Event(_) => None,
            })
            .filter(|message| !message.options.is_empty() && message.selected_options.is_empty())
    }

    /// What a choice says to the agent, or why it cannot be sent.
    ///
    /// Read-only on purpose: the daemon validates a submission here, before it
    /// checks the entity out of its map, so a refusal costs nothing.
    pub fn option_reply_text(&self, choice: &OptionChoice) -> Result<String, String> {
        let offer = self
            .open_offer()
            .filter(|message| message.id == choice.message_id)
            .ok_or_else(|| "these options are no longer open".to_string())?;
        if choice.option_ids.is_empty() {
            return Err("choose at least one option".to_string());
        }
        let mut chosen = Vec::new();
        for id in &choice.option_ids {
            let option = offer
                .options
                .iter()
                .find(|option| &option.id == id)
                .ok_or_else(|| format!("no option {id} on {}", offer.id))?;
            if chosen.contains(&option.reply_text()) {
                return Err(format!("option {id} was chosen twice"));
            }
            chosen.push(option.reply_text());
        }
        Ok(chosen.join("\n\n"))
    }

    /// Record the reviewer's choice on the offer and post the reply it sends.
    ///
    /// One call, because the two halves must not come apart: a recorded choice
    /// the agent never heard reads as answered, and a reply with nothing marked
    /// leaves the chat with no record of what was pressed.
    pub fn post_option_reply(
        &mut self,
        choice: &OptionChoice,
        now: &str,
    ) -> Result<String, String> {
        let body = self.option_reply_text(choice)?;
        let sequence = self.next();
        for item in self.items.iter_mut() {
            let ThreadItem::Message(message) = item else {
                continue;
            };
            if message.id == choice.message_id {
                message.selected_options = choice.option_ids.clone();
                message.updated_sequence = sequence;
                break;
            }
        }
        let id = self.post_user(body, None, now);
        if let Some(ThreadItem::Message(reply)) = self.items.last_mut() {
            reply.answers_options_of = Some(choice.message_id.clone());
        }
        Ok(id)
    }

    pub fn remember_completion(&mut self, report: &CompletionReport) {
        self.last_completion = Some(report.clone());
    }

    /// Record an outcome the agent reported: its summary as an ordinary agent
    /// message carrying the outcome as a status, and the structured report
    /// attached when the agent wrote one.
    ///
    /// The message IS the record — no companion event stands beside it, free to
    /// disagree with it. An outcome is the agent addressing the human, so it
    /// needs reading exactly once, and a replacement agent reads why its
    /// predecessor stopped out of the same catch-up packet that carries what
    /// the human said.
    pub fn post_outcome(
        &mut self,
        outcome: MessageOutcome,
        summary: impl Into<String>,
        report: Option<&CompletionReport>,
        now: impl Into<String>,
    ) -> String {
        let summary = summary.into();
        // The report is the densest statement of what the change touched, so it
        // is indexed with the summary rather than beside it. Derived before the
        // post, which reads the scope this borrows.
        let metadata =
            ItemMetadata::derive(&completion_text(&summary, report), &[], None, &self.scope);
        let id = self.post_agent(summary, None, now);
        if let Some(ThreadItem::Message(message)) = self.items.last_mut() {
            message.outcome = Some(outcome);
            message.done = outcome == MessageOutcome::Completed;
            message.completion_report = report.cloned().map(Box::new);
            message.metadata = metadata;
        }
        id
    }

    fn post_message(
        &mut self,
        role: MessageRole,
        body: String,
        anchor: Option<MessageAnchor>,
        links: Vec<ThreadLink>,
        now: String,
    ) -> String {
        self.post_message_working(role, body, anchor, links, now, false)
    }

    /// Every message is posted here, and none of them is a completion: `done`
    /// is set by [`post_outcome`](Self::post_outcome) and by nothing else, so
    /// the flag cannot come apart from the outcome it stands for.
    fn post_message_working(
        &mut self,
        role: MessageRole,
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
            done: false,
            outcome: None,
            completion_report: None,
            source: MessageSource::Chat,
            body,
            created_at: now,
            seen_at: None,
            anchor,
            resolved_by_revision: None,
            agent_reply: None,
            links,
            attachments: Vec::new(),
            options: Vec::new(),
            selected_options: Vec::new(),
            answers_options_of: None,
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

    /// Mint one event and hand back the counter value it was minted at — the
    /// handle a later in-place update names it by. Callers with nothing to
    /// update ignore the value.
    pub fn push_event(
        &mut self,
        event: ThreadEventKind,
        summary: Option<String>,
        session_id: Option<String>,
        revision_id: Option<String>,
        now: impl Into<String>,
    ) -> u64 {
        self.push_event_with_links(event, summary, session_id, revision_id, Vec::new(), now)
    }

    pub fn push_event_with_links(
        &mut self,
        event: ThreadEventKind,
        summary: Option<String>,
        session_id: Option<String>,
        revision_id: Option<String>,
        links: Vec<ThreadLink>,
        now: impl Into<String>,
    ) -> u64 {
        self.push_drafted_event(
            ThreadEventDraft {
                event,
                summary,
                session_id,
                revision_id,
                links,
                parent_sequence: None,
            },
            now,
        )
    }

    pub fn push_drafted_event(&mut self, draft: ThreadEventDraft, now: impl Into<String>) -> u64 {
        let ThreadEventDraft {
            event,
            summary,
            session_id,
            revision_id,
            links,
            parent_sequence,
        } = draft;
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
            updated_sequence: 0,
            event,
            created_at: now.into(),
            summary,
            outcome: None,
            session_id,
            revision_id,
            links,
            parent_sequence,
            completion_report: None,
            metadata,
        }));
        sequence
    }

    /// Record a tool call's answer on the row the call minted: the answer
    /// arrives as a suffix line, the outcome as a field, and the row's
    /// `updated_sequence` is bumped so every cursor re-ships it.
    ///
    /// `false` when `sequence` names no resident tool-call row — a call minted
    /// before a reload left the tail, say — which sends the caller back to
    /// minting an answer of its own rather than losing it.
    ///
    /// The row's metadata is left as the call derived it: the answer is tool
    /// output, not the agent naming a file, and re-deriving would let a
    /// grep result's own text link the conversation somewhere the agent never
    /// looked.
    pub fn resolve_tool_call(
        &mut self,
        sequence: u64,
        outcome: ToolCallOutcome,
        answer: &str,
    ) -> bool {
        let found = self.items.iter().position(|item| {
            matches!(item, ThreadItem::Event(event)
                if event.sequence == sequence && event.event == ThreadEventKind::ToolUse)
        });
        let Some(index) = found else {
            return false;
        };
        // An in-place mutation of an already-sequenced item, like resolving a
        // comment: bump so the cursored polls re-ship the answered call.
        let bumped = self.next();
        let ThreadItem::Event(event) = &mut self.items[index] else {
            return false;
        };
        if !answer.is_empty() {
            let summary = event.summary.get_or_insert_with(String::new);
            summary.push_str("\n→ ");
            summary.push_str(answer);
        }
        event.outcome = Some(outcome);
        event.updated_sequence = bumped;
        true
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
        count_serialized_items(self.items.len());
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
    ///
    /// Counts the history the load left in the store as well, since a mutation
    /// down there is a counter value too: a mark the tail alone could name
    /// would sit below such a mutation forever, and the client handed it would
    /// ask for the same delta on every poll.
    pub fn last_sequence(&self) -> u64 {
        self.items
            .iter()
            .map(ThreadItem::latest_sequence)
            .max()
            .unwrap_or(0)
            .max(self.stored_last_sequence)
    }

    /// Take a conversation's tail as it was read from the store, told how many
    /// older items were left there and how far the whole conversation's counter
    /// had got by the time it was read.
    ///
    /// The counter the appends run off (`next_sequence`) travels with the
    /// agent's own record, not with the items, so a conversation carries on
    /// from where it really ended rather than from the end of its tail.
    pub fn adopt_stored_tail(
        &mut self,
        tail: Vec<ThreadItem>,
        earlier_item_count: u64,
        stored_last_sequence: u64,
    ) {
        self.stored_last_sequence = stored_last_sequence;
        // The floor is remembered rather than re-derived from `items`, which
        // moves: an append or a withdrawn draft would otherwise shift what
        // this process believes it read.
        self.resident_from_sequence = match earlier_item_count {
            0 => 0,
            _ => tail.first().map(ThreadItem::sequence).unwrap_or(0),
        };
        self.earlier_item_count = earlier_item_count;
        self.items = tail;
    }

    /// The oldest sequence this process read, or 0 when it read the whole
    /// conversation — the floor under which the stored history is not this
    /// process's to rewrite.
    pub fn resident_from_sequence(&self) -> u64 {
        self.resident_from_sequence
    }

    /// How long the whole conversation is, resident or not. What the client is
    /// told, because it is what the client's gap check means: "is my cache a
    /// window, or did it lose something?"
    pub fn total_item_count(&self) -> u64 {
        self.earlier_item_count + self.items.len() as u64
    }

    /// Whether the page asked for reaches under the tail this process holds,
    /// and so has to be read from the store instead of out of memory.
    ///
    /// Measured the way the page is: the tail answers when it holds the page's
    /// worth of MESSAGES below the seek. A tail of pure activity holds no page
    /// at all, however many items it holds — which is also what makes a page
    /// answered from memory able to count its own runs, since its oldest walked
    /// item is then a resident message.
    pub fn page_reaches_stored_history(&self, before_sequence: Option<u64>, limit: usize) -> bool {
        if self.earlier_item_count == 0 {
            return false;
        }
        let before = before_sequence.unwrap_or(u64::MAX);
        self.items
            .iter()
            .filter(|item| item.sequence() < before && item.counts_toward_page())
            .count()
            < limit
    }

    /// Whether a cursor this far back reaches under the tail this process
    /// holds, and so has to be completed out of the store.
    ///
    /// An item under the tail is one this process cannot mutate — it does not
    /// hold it — so the only news down there is a mutation the process before
    /// this one made, and every one of those is at or below the counter value
    /// the load read. A cursor past that mark has already been told everything
    /// the history has to say, which is what stops a caught-up client from
    /// asking the store anything on its steady-state polls.
    pub fn cursor_reaches_stored_history(&self, after_sequence: u64) -> bool {
        self.earlier_item_count > 0 && after_sequence < self.stored_last_sequence
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

    /// When the last thing on this conversation was said or happened.
    ///
    /// One of the three clocks the inbox's "last activity" is the latest of
    /// (the others being the checkout's files and the agent's terminal), and
    /// the cheapest: items are appended in order, so it is the tail.
    pub fn last_item_at(&self) -> Option<&str> {
        self.items.last().map(ThreadItem::created_at)
    }

    /// When the USER said each of the things they have said here, in order.
    ///
    /// The boot migration's input: an entity that predates anchors is anchored
    /// by replaying exactly these through the anchor rule, so its place in the
    /// inbox is the place it would always have had.
    pub fn user_message_times(&self) -> impl Iterator<Item = &str> {
        Thread::user_message_times_in(&self.items)
    }

    /// The same reading of items the caller read for itself. The migration is
    /// about everything the user ever said, so it looks at the conversation
    /// whole rather than at whatever tail a load left resident.
    pub fn user_message_times_in(items: &[ThreadItem]) -> impl Iterator<Item = &str> {
        items.iter().filter_map(|item| match item {
            ThreadItem::Message(message) if message.role == MessageRole::User => {
                Some(message.created_at.as_str())
            }
            _ => None,
        })
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

    /// Whether an unread attention-class item sits below `floor` — under the
    /// window a reader was actually shipped.
    ///
    /// A client reads a long conversation through a window on its newest
    /// items, so reaching the end of what it holds says nothing about the
    /// items beneath it. This is the question a read report has to answer
    /// before the cursor may jump to the end: is there anything down there the
    /// human is being called to and has never been sent?
    ///
    /// Creation sequence at both ends, for the reason
    /// [`unread_since`](Self::unread_since) reads it, and the floor itself is
    /// inside the window — it is the oldest item the reader holds.
    pub fn unread_attention_below(&self, floor: u64, cursor: u64) -> bool {
        self.items
            .iter()
            .filter(|item| item.sequence() > cursor && item.sequence() < floor)
            .any(|item| item.attention_reason().is_some())
    }

    /// The creation sequence of the newest item here that needed the human, or
    /// 0 when nothing ever has — the line a dismissal is measured against.
    ///
    /// Creation sequence, for the same reason [`unread_since`](Self::unread_since)
    /// reads it: marking a message seen bumps its `updated_sequence` and that is
    /// not the conversation speaking again. A status-only stretch after a
    /// dismissal leaves the row cleared, however long it runs.
    pub fn last_attention_sequence(&self) -> u64 {
        self.items
            .iter()
            .rev()
            .find(|item| item.attention_reason().is_some())
            .map(ThreadItem::sequence)
            .unwrap_or(0)
    }

    /// The items this query names, newest first, bounded by its limit.
    ///
    /// The point of the tool this serves: a session that lost its context asks
    /// what was decided about one thing, instead of replaying the whole log.
    pub fn search(&self, query: &ConversationQuery) -> Vec<ConversationHit> {
        self.search_items(&self.items, query)
    }

    /// The same search over items the caller read for itself — how a search
    /// reaches history no load left resident: the answer is about the whole
    /// conversation, so the tail is not enough to look through.
    pub fn search_items(
        &self,
        items: &[ThreadItem],
        query: &ConversationQuery,
    ) -> Vec<ConversationHit> {
        items
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
            "item_count": self.total_item_count(),
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
        self.wire_value_of_delta(&self.resident_after(after_sequence))
    }

    /// The same cursor view, completed with items read back out of the store:
    /// the history under the tail this process loaded, which memory has no
    /// answer for and which a client whose cursor predates a restart is still
    /// owed. Items the tail already holds are taken from the tail, not from
    /// `history` — memory is the fresher copy of those.
    pub fn wire_value_after_including_history(
        &self,
        after_sequence: u64,
        history: &[ThreadItem],
    ) -> Value {
        let mut delta: Vec<&ThreadItem> = history
            .iter()
            .filter(|item| {
                item.sequence() < self.resident_from_sequence
                    && item.latest_sequence() > after_sequence
            })
            .chain(self.resident_after(after_sequence))
            .collect();
        // The store hands its rows back in mutation order; a conversation's own
        // order is creation order, which is what a client merges against.
        delta.sort_by_key(|item| item.sequence());
        self.wire_value_of_delta(&delta)
    }

    /// The items this process holds that a cursor has not been told about.
    fn resident_after(&self, after_sequence: u64) -> Vec<&ThreadItem> {
        self.items
            .iter()
            .filter(|item| item.latest_sequence() > after_sequence)
            .collect()
    }

    /// The delta shape, around items the caller already chose.
    fn wire_value_of_delta(&self, delta: &[&ThreadItem]) -> Value {
        count_serialized_items(delta.len());
        json!({
            "id": self.id,
            "agent": self.agent,
            "sessions": self.sessions,
            "items": delta,
            "revisions": self.revision_summaries(),
            "last_completion": self.last_completion,
            "thread_total": self.total_item_count(),
            "thread_last_sequence": self.last_sequence(),
        })
    }

    /// Backward view for a first load and for scrolling up: the newest `limit`
    /// items strictly older than `before_sequence`, ascending, in the shape
    /// `wire_value_after` produces plus the two fields a backward walk needs —
    /// `oldest_sequence`, the seek for the next page up, and `has_more`,
    /// whether asking for one is worth it.
    ///
    /// Reads the tail this process holds, so `has_more` counts the history no
    /// load read as pages still to come: the caller answers those from the
    /// store through [`wire_value_of_page`](Self::wire_value_of_page).
    pub fn wire_value_page(&self, before_sequence: Option<u64>, limit: usize) -> Value {
        // No bound means "from the newest", which the same filter expresses as
        // a point past every sequence there could be.
        let before = before_sequence.unwrap_or(u64::MAX);
        let older: Vec<&ThreadItem> = self
            .items
            .iter()
            .filter(|item| item.sequence() < before)
            .collect();
        let reached = older.len() - page_span(older.iter().rev().copied(), limit);
        let span: Vec<&ThreadItem> = older[reached..].iter().rev().copied().collect();
        // The census is exact over the span every digest NAMES, because the
        // walk never leaves the resident tail: nothing in `[from, through]`
        // sits under it. A digest covers a WHOLE run when the walk reached a
        // message, which is what the page gate buys — a page memory answers
        // has `page_reaches_stored_history() == false`, so its limit-th
        // message is resident and a message ends the oldest run. A conversation
        // shorter than the limit is reached whole and has nothing below it.
        let census = |from: u64, through: u64| {
            Ok::<u64, std::convert::Infallible>(
                self.items
                    .iter()
                    .filter(|item| {
                        item.is_tool_call() && (from..=through).contains(&item.sequence())
                    })
                    .count() as u64,
            )
        };
        let cut = match cut_activity_runs(span, census) {
            Ok(cut) => cut,
            Err(impossible) => match impossible {},
        };
        // What is left below what shipped, plus the history no load read: both
        // are pages the client can still ask for.
        let outstanding = match cut.items.first().map(|item| item.sequence()) {
            Some(oldest) => older.iter().filter(|item| item.sequence() < oldest).count(),
            None => older.len(),
        } + self.earlier_item_count as usize;
        self.wire_value_of_page(&cut, outstanding > 0)
    }

    /// The page shape, around the cut the caller already made — off the tail
    /// this process holds, or off a page read back out of the store. Takes the
    /// cut whole, borrowed or owned, so items and digests cannot be assembled
    /// out of sync.
    ///
    /// A page is NOT guaranteed contiguous: the per-run cap omits items inside
    /// a run of activity, and `activity_digests` is what accounts for them —
    /// one per run the page touches, exact over the whole run. `has_more` is
    /// unchanged and means what it always did: something sits below the page's
    /// oldest SHIPPED item.
    ///
    /// `thread_total` still counts the whole conversation, not the page, so the
    /// client can tell "my cache is a bounded window" from "my cache lost
    /// something" — the gap check the forward cursor already relies on.
    pub fn wire_value_of_page<T: Borrow<ThreadItem>>(
        &self,
        cut: &PageCut<T>,
        has_more: bool,
    ) -> Value {
        let page: Vec<&ThreadItem> = cut.items.iter().map(Borrow::borrow).collect();
        count_serialized_items(page.len());
        json!({
            "id": self.id,
            "agent": self.agent,
            "sessions": self.sessions,
            "items": page,
            "activity_digests": cut.digests,
            "revisions": self.revision_summaries(),
            "last_completion": self.last_completion,
            "thread_total": self.total_item_count(),
            "thread_last_sequence": self.last_sequence(),
            "oldest_sequence": page.first().map(|item| item.sequence()),
            "has_more": has_more,
        })
    }

    /// What a resumed agent is handed to rebuild the conversation: the last
    /// `limit` **messages** to and from the agent, and nothing else on the
    /// thread.
    ///
    /// Events are left out by construction rather than by tuning a ratio. They
    /// are Build's observations about the agent, and the limit counts messages
    /// so that a session which emitted hundreds of tool calls before restarting
    /// still hands its replacement what the human said — the exact context the
    /// packet exists to carry.
    ///
    /// An outcome is not one of those observations: it is the agent's own
    /// report, so it is a message, and it is carried with the outcome named on
    /// its line. That is what tells a replacement why its predecessor blocked.
    pub fn catch_up_markdown(&self, limit: usize) -> String {
        catch_up_lines(self.items.iter(), limit)
    }

    /// Whether the packet has to be read from the store rather than off the
    /// tail — the sibling of [`page_reaches_stored_history`](Self::page_reaches_stored_history).
    ///
    /// True only when there is history under the tail AND the tail itself does
    /// not hold the packet's worth of messages. Both halves are answered off
    /// integers this process already has, so a conversation held whole — the
    /// common small case, and every storeless test daemon — hands a packet at
    /// today's speed and touches no SQL. The starved tail is the one that pays
    /// the read, and it is the one the packet exists for.
    pub fn catch_up_reaches_stored_history(&self, limit: usize) -> bool {
        self.earlier_item_count > 0
            && self
                .items
                .iter()
                .filter(|item| matches!(item, ThreadItem::Message(_)))
                .count()
                < limit
    }

    /// The same packet, completed with messages read back out of the store —
    /// the conversation under the tail an activity-heavy session left behind.
    ///
    /// Stored rows are admitted only below `resident_from_sequence`, the way
    /// the forward cursor admits them: the tail is the fresher copy of
    /// everything it still holds, so a message in both is carried once, from
    /// memory. The chain then runs exactly the filter-take-reverse the
    /// tail-only packet runs, so the limit still counts messages and still
    /// keeps the newest of them.
    pub fn catch_up_markdown_including_history(
        &self,
        history: &[ThreadItem],
        limit: usize,
    ) -> String {
        catch_up_lines(
            history
                .iter()
                .filter(|item| item.sequence() < self.resident_from_sequence)
                .chain(self.items.iter()),
            limit,
        )
    }
}

/// How far back a page reaches, walking newest→older: to the `limit`-th
/// message, and no further.
///
/// The whole span, not what ships: activity between two messages is the page's
/// too, and [`cut_activity_runs`] decides how much of each run the wire
/// carries. A conversation shorter than the limit is reached whole, which is
/// what makes the oldest walked item either a message or the start of the
/// conversation — and so what makes a run on a page always countable.
fn page_span<'a>(newest_first: impl Iterator<Item = &'a ThreadItem>, limit: usize) -> usize {
    let mut taken = 0;
    let mut counted = 0;
    for item in newest_first {
        if counted == limit {
            break;
        }
        taken += 1;
        if item.counts_toward_page() {
            counted += 1;
        }
    }
    taken
}

/// The packet's lines, off whatever conversation the caller assembled: the
/// newest `limit` messages, oldest-first.
fn catch_up_lines<'a>(
    items: impl DoubleEndedIterator<Item = &'a ThreadItem>,
    limit: usize,
) -> String {
    let mut lines: Vec<String> = items
        .rev()
        .filter_map(|item| match item {
            ThreadItem::Message(message) => Some(format!(
                "- {}{}: {}{}",
                message.role.as_str(),
                match message.reported_outcome() {
                    Some(outcome) => format!(" [{}]", outcome.as_str()),
                    None => String::new(),
                },
                message.body.replace('\n', " "),
                attachment_note(&message.attachments)
            )),
            ThreadItem::Event(_) => None,
        })
        .take(limit)
        .collect();
    lines.reverse();
    lines.join("\n")
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
    fn an_event_without_a_report_omits_the_field() {
        let mut thread = Thread::new("run-plain");
        thread.push_event(
            ThreadEventKind::RunStarted,
            None,
            None,
            None,
            "2026-08-13T09:01:00Z",
        );
        thread.push_event(
            ThreadEventKind::IdleUnreported,
            Some("Agent went quiet without reporting done".to_string()),
            None,
            None,
            "2026-08-13T09:02:00Z",
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

    /// The line a dismissal is measured against: where the conversation last
    /// needed the human, and nowhere else. Progress the agent reports after
    /// that must not move it, or a dismissed row would come back for work
    /// happening quietly.
    #[test]
    fn the_attention_line_is_the_newest_item_that_needed_the_human() {
        let mut thread = Thread::new("run-1");
        assert_eq!(
            thread.last_attention_sequence(),
            0,
            "nothing has ever asked"
        );

        thread.push_event(
            ThreadEventKind::RunStarted,
            None,
            None,
            None,
            "2026-08-13T09:00:00Z",
        );
        assert_eq!(thread.last_attention_sequence(), 0, "status is not asking");

        thread.post_agent("here is the answer", None, "2026-08-13T09:01:00Z");
        let asked_at = thread.last_sequence();
        assert_eq!(thread.last_attention_sequence(), asked_at);

        thread.push_event(
            ThreadEventKind::Committed,
            None,
            None,
            None,
            "2026-08-13T09:02:00Z",
        );
        thread.post_user("carry on", None, "2026-08-13T09:03:00Z");
        assert_eq!(
            thread.last_attention_sequence(),
            asked_at,
            "work happening and the human talking are not the work asking"
        );

        thread.push_event(
            ThreadEventKind::Blocked,
            None,
            None,
            None,
            "2026-08-13T09:04:00Z",
        );
        assert_eq!(thread.last_attention_sequence(), thread.last_sequence());
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

    /// The inbox's two readings of a conversation: when it last did anything,
    /// and when the user themselves last said something. Events and the agent's
    /// own words count for the first and never for the second — the anchor rule
    /// rests on the difference.
    #[test]
    fn a_conversation_reports_its_last_item_and_the_users_own_messages() {
        let mut thread = Thread::new("run-1");
        assert_eq!(thread.last_item_at(), None);
        assert_eq!(thread.user_message_times().count(), 0);

        thread.post_user("do the thing", None, "2026-08-13T09:00:00Z");
        thread.post_agent("on it", None, "2026-08-13T09:01:00Z");
        thread.post_user("and this too", None, "2026-08-14T22:00:00Z");
        thread.push_event(
            ThreadEventKind::Done,
            Some("finished".to_string()),
            None,
            None,
            "2026-08-14T22:05:00Z",
        );

        assert_eq!(thread.last_item_at(), Some("2026-08-14T22:05:00Z"));
        assert_eq!(
            thread.user_message_times().collect::<Vec<_>>(),
            vec!["2026-08-13T09:00:00Z", "2026-08-14T22:00:00Z"],
            "only what the user said, in the order they said it"
        );
    }
}

/// The five kinds an event-stream harness fills a conversation with, and the
/// packet a resumed agent is handed once they exist.
#[cfg(test)]
mod agent_activity_tests {
    use super::*;

    const ACTIVITY: [ThreadEventKind; 5] = [
        ThreadEventKind::Reasoning,
        ThreadEventKind::ToolUse,
        ThreadEventKind::ToolResult,
        ThreadEventKind::Narration,
        ThreadEventKind::TaskUpdate,
    ];

    /// The fifth kind is the four's equal in every rule the roster of kinds
    /// already carries: it is on `ALL`, it is `Status`, and its wire token is
    /// the snake_case of its name — so the class split, the Issue mirror and
    /// the counted predicate cover it with no new code.
    #[test]
    fn background_task_updates_join_the_activity_kinds() {
        assert_eq!(ThreadEventKind::TaskUpdate.as_str(), "task_update");
        assert_eq!(ThreadEventKind::TaskUpdate.class(), EventClass::Status);
        assert!(
            ThreadEventKind::ALL.contains(&ThreadEventKind::TaskUpdate),
            "a kind off ALL is a kind every rule tested over the roster misses"
        );
        assert_eq!(
            serde_json::to_value(ThreadEventKind::TaskUpdate).unwrap(),
            serde_json::json!("task_update"),
            "the token it serializes as is the token it names"
        );
    }

    /// The property that makes activity safe to put in the conversation: an
    /// agent thinking out loud updates the entry underneath the human and
    /// never marks it unread.
    #[test]
    fn agent_activity_is_status_and_moves_no_unread_count() {
        let mut thread = Thread::new("run-activity");
        let cursor = thread.last_sequence();
        for kind in ACTIVITY {
            assert_eq!(kind.class(), EventClass::Status, "{kind:?}");
            thread.push_event(
                kind,
                Some(format!("{} happened", kind.as_str())),
                None,
                None,
                "2026-08-23T09:00:00Z",
            );
        }

        assert_eq!(thread.items.len(), ACTIVITY.len());
        for item in &thread.items {
            assert_eq!(item.attention_reason(), None, "{item:?}");
        }
        assert_eq!(thread.unread_since(cursor), UnreadSummary::default());
        assert_eq!(thread.last_attention_sequence(), 0);
    }

    /// Activity rides the wire as an ordinary thread item — no new envelope,
    /// no new RPC, and a token that matches how the kind serializes.
    #[test]
    fn a_tool_use_rides_the_wire_as_an_ordinary_thread_item() {
        let mut thread = Thread::new("run-activity");
        thread.push_event(
            ThreadEventKind::ToolUse,
            Some("Read bridge/src/app.rs".to_string()),
            None,
            None,
            "2026-08-23T09:00:00Z",
        );

        let wire = thread.wire_value();
        assert_eq!(wire["items"][0]["type"], "event");
        assert_eq!(wire["items"][0]["data"]["event"], "tool_use");
        assert_eq!(
            wire["items"][0]["data"]["summary"],
            "Read bridge/src/app.rs"
        );
        assert_eq!(wire["items"][0]["data"]["sequence"], 1);
    }

    /// The packet exists to carry the conversation across a restart, so it
    /// carries messages and nothing else: a session that emitted activity all
    /// afternoon must still hand its replacement what the human said.
    #[test]
    fn the_catch_up_packet_carries_messages_and_no_events() {
        let mut thread = Thread::new("run-activity");
        thread.post_user("please rename the helper", None, "2026-08-23T09:00:00Z");
        for index in 0..3 {
            thread.push_event(
                ThreadEventKind::ToolUse,
                Some(format!("Read file-{index}.rs")),
                None,
                None,
                "2026-08-23T09:01:00Z",
            );
        }
        thread.post_agent("renamed it", None, "2026-08-23T09:02:00Z");
        // Build's own observations about the agent go the same way as activity.
        thread.push_event(
            ThreadEventKind::Blocked,
            Some("the test suite will not build".to_string()),
            None,
            None,
            "2026-08-23T09:03:00Z",
        );

        let catch_up = thread.catch_up_markdown(40);
        assert_eq!(
            catch_up, "- user: please rename the helper\n- agent: renamed it",
            "{catch_up}"
        );
    }

    /// The limit counts messages, not items. An agent that emitted more
    /// activity than the packet holds must still be handed what the human
    /// said — the case that made the packet messages-only in the first place.
    #[test]
    fn a_session_full_of_activity_still_hands_back_the_humans_words() {
        let mut thread = Thread::new("run-activity");
        thread.post_user("please rename the helper", None, "2026-08-23T09:00:00Z");
        for index in 0..100 {
            thread.push_event(
                ThreadEventKind::ToolUse,
                Some(format!("Read file-{index}.rs")),
                None,
                None,
                "2026-08-23T09:01:00Z",
            );
        }

        assert_eq!(
            thread.catch_up_markdown(40),
            "- user: please rename the helper"
        );
    }

    /// The limit still bounds the packet, and still keeps the newest.
    #[test]
    fn the_packet_keeps_the_newest_messages_up_to_its_limit() {
        let mut thread = Thread::new("run-activity");
        for index in 0..5 {
            thread.post_user(format!("ask {index}"), None, "2026-08-23T09:00:00Z");
            thread.push_event(
                ThreadEventKind::Reasoning,
                Some("thinking".to_string()),
                None,
                None,
                "2026-08-23T09:00:01Z",
            );
        }

        assert_eq!(thread.catch_up_markdown(2), "- user: ask 3\n- user: ask 4");
    }
}

/// The packet a resumed agent is handed when the tail it booted onto holds no
/// conversation — the failure §6.3 names first, and the one the messages-only
/// filter cannot fix on its own.
#[cfg(test)]
mod catch_up_history_tests {
    use super::*;

    const NOW: &str = "2026-08-29T09:00:00Z";

    /// A conversation stored whole, and the process that booted onto the last
    /// `tail` items of it — which is where an activity-heavy session leaves
    /// its replacement.
    fn stored_and_booted(tail: usize) -> (Vec<ThreadItem>, Thread) {
        let mut whole = Thread::new("run-restart");
        whole.post_user("please rename the helper", None, NOW);
        whole.post_agent("on it", None, NOW);
        for index in 0..8 {
            whole.push_event(
                ThreadEventKind::ToolUse,
                Some(format!("Read file-{index}.rs")),
                None,
                None,
                NOW,
            );
        }
        let stored = whole.items.clone();
        let mut booted = Thread::new("run-restart");
        booted.adopt_stored_tail(
            stored[stored.len() - tail..].to_vec(),
            (stored.len() - tail) as u64,
            whole.last_sequence(),
        );
        (stored, booted)
    }

    /// What the store hands the packet back: the conversation's messages,
    /// oldest-first.
    fn stored_messages(stored: &[ThreadItem]) -> Vec<ThreadItem> {
        stored
            .iter()
            .filter(|item| matches!(item, ThreadItem::Message(_)))
            .cloned()
            .collect()
    }

    /// The gate: only a starved tail pays a read. A conversation held whole,
    /// and a long one whose tail still holds the packet's worth of messages,
    /// are both answered out of memory.
    #[test]
    fn only_a_tail_short_of_its_messages_reaches_for_the_store() {
        let (_, booted) = stored_and_booted(5);
        assert!(
            booted.catch_up_reaches_stored_history(40),
            "a tail of pure activity has to read the store"
        );
        assert!(
            !booted.catch_up_reaches_stored_history(0),
            "a packet that asks for nothing needs nothing"
        );

        let mut whole = Thread::new("run-whole");
        whole.post_user("please rename the helper", None, NOW);
        assert!(
            !whole.catch_up_reaches_stored_history(40),
            "a conversation with no history under it never reads the store"
        );

        let (_, rich_tail) = stored_and_booted(10);
        assert!(
            !rich_tail.catch_up_reaches_stored_history(2),
            "a tail holding the packet's worth of messages answers from memory"
        );
    }

    /// The failure this exists for: the tail holds nothing but tool calls, so
    /// the messages-only filter over it yields an empty packet. Read through
    /// the store, the same packet carries what the human said.
    #[test]
    fn a_starved_tail_still_hands_over_the_conversation() {
        let (stored, booted) = stored_and_booted(5);
        assert_eq!(
            booted.catch_up_markdown(40),
            "",
            "the tail alone is the starved packet this replaces"
        );

        assert_eq!(
            booted.catch_up_markdown_including_history(&stored_messages(&stored), 40),
            "- user: please rename the helper\n- agent: on it"
        );
    }

    /// The merge rule, held to the precedent the forward cursor set: a stored
    /// row is admitted only below what this process read, so a message the
    /// tail still holds is carried once, from the tail.
    #[test]
    fn a_message_the_tail_still_holds_is_not_repeated() {
        let (stored, booted) = stored_and_booted(9);

        let packet = booted.catch_up_markdown_including_history(&stored_messages(&stored), 40);
        assert_eq!(packet, "- user: please rename the helper\n- agent: on it");
        assert_eq!(packet.matches("on it").count(), 1, "{packet}");
    }

    /// The limit still counts messages and still keeps the newest of them,
    /// across the join.
    #[test]
    fn the_merged_packet_keeps_the_newest_messages_up_to_its_limit() {
        let (stored, booted) = stored_and_booted(5);

        assert_eq!(
            booted.catch_up_markdown_including_history(&stored_messages(&stored), 1),
            "- agent: on it"
        );
    }
}

/// §6.3's second failure: a page measured in items shows a reviewer who opens
/// a conversation mid-session nothing but tool calls, with the last thing
/// anyone said somewhere below them.
#[cfg(test)]
mod counted_page_tests {
    use super::*;

    const NOW: &str = "2026-08-29T09:00:00Z";

    /// A working session as the thread records it: each message followed by
    /// the activity the agent emitted after it.
    fn conversation_with_activity(turns: usize, activity_per_turn: usize) -> Thread {
        let mut thread = Thread::new("run-busy");
        for turn in 0..turns {
            thread.post_user(format!("ask {turn}"), None, NOW);
            for index in 0..activity_per_turn {
                thread.push_event(
                    ThreadEventKind::ToolUse,
                    Some(format!("Read file-{turn}-{index}.rs")),
                    None,
                    None,
                    NOW,
                );
            }
        }
        thread
    }

    fn page_items(page: &Value) -> Vec<u64> {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["data"]["sequence"].as_u64().unwrap())
            .collect()
    }

    fn counted_in_page(page: &Value) -> usize {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "message")
            .count()
    }

    /// The limit buys conversation. A page opened on a session that emitted
    /// twenty tool calls per turn carries its five messages and the activity
    /// between them, rather than five tool calls and nothing said.
    #[test]
    fn a_pages_limit_buys_conversation_and_activity_rides_beside_it() {
        let thread = conversation_with_activity(10, 5);
        let page = thread.wire_value_page(None, 5);

        assert_eq!(counted_in_page(&page), 5, "the limit counts messages");
        let shipped = page_items(&page);
        assert!(
            shipped.len() > 5,
            "the activity between the messages travels with them: {shipped:?}"
        );
        // One contiguous run of sequences, ending at the newest item.
        assert_eq!(
            shipped,
            (shipped[0]..=thread.last_sequence()).collect::<Vec<u64>>()
        );
        assert_eq!(page["oldest_sequence"], shipped[0], "{page:?}");
        assert_eq!(page["has_more"], true);
        assert_eq!(page["thread_total"], 60, "the total counts every item");
    }

    /// A page's limit buys MESSAGES, and nothing else on a conversation spends
    /// it. An outcome, a block, a commit — everything Build records about the
    /// work — rides beside the words it belongs to, so the page a reviewer
    /// opens on is always the last twenty things anybody said.
    #[test]
    fn a_pages_limit_buys_messages_and_the_lifecycle_rides_beside_them() {
        let mut thread = Thread::new("run-outcomes");
        for turn in 0..6 {
            thread.post_user(format!("ask {turn}"), None, NOW);
            thread.push_event(
                ThreadEventKind::Done,
                Some(format!("finished {turn}")),
                None,
                None,
                NOW,
            );
        }

        let page = thread.wire_value_page(None, 3);
        let shipped = page_items(&page);
        assert_eq!(counted_in_page(&page), 3, "the limit counts messages");
        assert_eq!(
            shipped.len(),
            6,
            "each message's attention event rides with it: {shipped:?}"
        );
    }

    /// The page stops AT the limit-th counted item: activity older than it is
    /// the next page's, so a page is never padded with work nobody asked for.
    #[test]
    fn a_page_ends_on_its_oldest_message_not_on_the_activity_under_it() {
        let thread = conversation_with_activity(4, 3);
        let page = thread.wire_value_page(None, 2);

        let shipped = page_items(&page);
        let oldest = shipped[0];
        assert!(
            matches!(
                thread.items.iter().find(|item| item.sequence() == oldest),
                Some(ThreadItem::Message(_))
            ),
            "the page opens on a message: {shipped:?}"
        );
    }

    /// The cap. An all-activity stretch cannot make a page unbounded: the run
    /// ships its newest hundred and the digest beside it says how many there
    /// really were, so the reviewer is told a thousand without being sent one.
    #[test]
    fn an_all_activity_stretch_is_bounded_by_the_run_cap() {
        let mut thread = Thread::new("run-busy");
        thread.post_user("please rename the helper", None, NOW);
        for index in 0..1000 {
            thread.push_event(
                ThreadEventKind::ToolUse,
                Some(format!("Read file-{index}.rs")),
                None,
                None,
                NOW,
            );
        }

        let page = thread.wire_value_page(None, 5);
        let shipped = page_items(&page);
        assert_eq!(
            shipped.len(),
            1 + PAGE_ACTIVITY_RUN_CAP,
            "{}",
            shipped.len()
        );
        assert_eq!(counted_in_page(&page), 1, "the one thing said is on it");
        assert_eq!(
            page["has_more"], false,
            "nothing sits below the page's oldest item"
        );
        assert_eq!(page["oldest_sequence"], shipped[0]);

        let digests = page["activity_digests"].as_array().unwrap();
        assert_eq!(digests.len(), 1, "{digests:?}");
        assert_eq!(digests[0]["from_sequence"], 2);
        assert_eq!(digests[0]["through_sequence"], thread.last_sequence());
        assert_eq!(digests[0]["tool_calls"], 1000, "the omitted calls counted");
        assert_eq!(digests[0]["last_tool_call"]["sequence"], 1001);
        assert_eq!(
            digests[0]["last_tool_call"]["summary"], "Read file-999.rs",
            "{digests:?}"
        );
    }

    /// Every page carries its runs' digests; a forward delta carries none —
    /// a delta says what arrived, and what arrived is what the client holds.
    #[test]
    fn a_page_carries_activity_digests_and_a_delta_carries_none() {
        let thread = conversation_with_activity(4, 3);

        let page = thread.wire_value_page(None, 2);
        let digests = page["activity_digests"].as_array().unwrap();
        assert_eq!(digests.len(), 2, "one per run on the page: {digests:?}");
        assert!(
            digests
                .iter()
                .all(|digest| digest["tool_calls"] == 3 && digest["last_tool_call"].is_object()),
            "{digests:?}"
        );

        let delta = thread.wire_value_after(0);
        assert!(delta.get("activity_digests").is_none(), "{delta:?}");
    }

    /// A run of nothing but thinking folds to a row with nothing to claim, and
    /// the fixed shape says so rather than leaving the field out.
    #[test]
    fn a_run_without_tool_calls_ships_a_null_last_call() {
        let mut thread = Thread::new("run-quiet");
        thread.post_user("what do you make of it", None, NOW);
        for _ in 0..4 {
            thread.push_event(ThreadEventKind::Reasoning, None, None, None, NOW);
        }

        let page = thread.wire_value_page(None, 5);
        let digests = page["activity_digests"].as_array().unwrap();
        assert_eq!(digests[0]["tool_calls"], 0, "{digests:?}");
        assert!(digests[0]["last_tool_call"].is_null(), "{digests:?}");
    }

    /// The shape on the wire, whole. A client reads these four names and no
    /// others, and a digest always carries `last_tool_call` — the object when
    /// the run made a call, `null` when it made none — so nothing has to read
    /// around a field that is sometimes absent.
    #[test]
    fn an_activity_digest_ships_a_fixed_shape() {
        let mut thread = Thread::new("run-shape");
        thread.post_user("rename the helper", None, NOW);
        thread.push_event(ThreadEventKind::Reasoning, None, None, None, NOW);
        let call = thread.push_event(
            ThreadEventKind::ToolUse,
            Some("Bash(cargo test)".to_string()),
            None,
            None,
            NOW,
        );
        assert!(thread.resolve_tool_call(call, ToolCallOutcome::Ok, "1735 passed"));

        let page = thread.wire_value_page(None, 5);
        assert_eq!(
            page["activity_digests"][0],
            json!({
                "from_sequence": 2,
                "through_sequence": 3,
                "tool_calls": 1,
                "last_tool_call": {
                    "sequence": 3,
                    "created_at": NOW,
                    "summary": "Bash(cargo test)\n\u{2192} 1735 passed",
                    "outcome": "ok"
                }
            }),
            "{page:?}"
        );
    }

    /// The run still open at the end of a conversation is digested through the
    /// thread's last sequence, not through some item inside it: a client
    /// holding only the page can tell exactly which arrivals the digest has
    /// already counted and which it must add itself.
    #[test]
    fn the_open_tail_runs_digest_reaches_the_threads_last_sequence() {
        let thread = conversation_with_activity(6, 4);

        let page = thread.wire_value_page(None, 2);
        let digests = page["activity_digests"].as_array().unwrap();
        let tail = digests.last().expect("the open run has a digest");
        assert_eq!(tail["through_sequence"], thread.last_sequence(), "{tail:?}");
        assert_eq!(tail["tool_calls"], 4, "{tail:?}");
        assert_eq!(
            tail["last_tool_call"]["sequence"],
            thread.last_sequence(),
            "{tail:?}"
        );
    }

    /// What a sequence-paging client relies on: pages abut at their seeks, so
    /// walking `before_sequence = oldest_sequence` sees every item exactly
    /// once and skips none — activity included.
    #[test]
    fn paging_backward_over_an_activity_heavy_thread_sees_every_item_once() {
        let thread = conversation_with_activity(9, 7);

        let mut walked: Vec<u64> = Vec::new();
        let mut before = None;
        loop {
            let page = thread.wire_value_page(before, 2);
            let shipped = page_items(&page);
            assert!(!shipped.is_empty(), "{page:?}");
            walked.splice(0..0, shipped.clone());
            if !page["has_more"].as_bool().unwrap() {
                break;
            }
            before = Some(page["oldest_sequence"].as_u64().unwrap());
        }

        assert_eq!(walked, (1..=thread.last_sequence()).collect::<Vec<u64>>());
    }

    /// The gate that sends a page to the store is measured in messages too: a
    /// tail holding only activity cannot answer a page, however many items it
    /// holds, and a tail holding the page's worth of words answers it whatever
    /// else is under it.
    #[test]
    fn the_stored_page_gate_is_measured_in_messages() {
        let mut whole = Thread::new("run-busy");
        whole.post_user("please rename the helper", None, NOW);
        for index in 0..300 {
            whole.push_event(
                ThreadEventKind::ToolUse,
                Some(format!("Read file-{index}.rs")),
                None,
                None,
                NOW,
            );
        }
        let stored = whole.items.clone();

        let mut starved = Thread::new("run-busy");
        starved.adopt_stored_tail(
            stored[stored.len() - 20..].to_vec(),
            (stored.len() - 20) as u64,
            whole.last_sequence(),
        );
        assert!(
            starved.page_reaches_stored_history(None, 5),
            "a tail of pure activity holds no page of conversation"
        );
        assert!(
            starved.page_reaches_stored_history(None, 1),
            "not even one message: the words are under the tail"
        );

        whole.post_agent("renamed it", None, NOW);
        let spoken = whole.items.clone();
        let mut fed = Thread::new("run-busy");
        fed.adopt_stored_tail(
            spoken[spoken.len() - 20..].to_vec(),
            (spoken.len() - 20) as u64,
            whole.last_sequence(),
        );
        assert!(
            !fed.page_reaches_stored_history(None, 1),
            "a tail holding the page's words answers it from memory"
        );
    }
}

/// The one predicate both bounds count with: an item is counted when the human
/// reads it as conversation. Everything else — the four activity kinds and the
/// quiet lifecycle markers with them — rides free.
#[cfg(test)]
mod counted_item_tests {
    use super::*;

    /// The two readings of the rule, held equal over every kind there is: the
    /// Rust one here, and the `message = 1 OR attention = 1` the store filters
    /// with. A kind added later cannot make them disagree without failing
    /// here.
    #[test]
    fn a_counted_item_is_a_message_or_a_call_for_the_human() {
        let mut thread = Thread::new("run-counted");
        for kind in ThreadEventKind::ALL {
            thread.push_event(
                kind,
                Some(format!("{} happened", kind.as_str())),
                None,
                None,
                "2026-08-29T09:00:00Z",
            );
        }
        thread.post_user("please rename the helper", None, "2026-08-29T09:01:00Z");
        thread.post_agent("renamed it", None, "2026-08-29T09:02:00Z");
        thread.post_agent_progress("still going", None, "2026-08-29T09:03:00Z");
        thread.post_outcome(
            MessageOutcome::Blocked,
            "needs production credentials",
            None,
            "2026-08-29T09:04:00Z",
        );

        for item in &thread.items {
            let expected =
                matches!(item, ThreadItem::Message(_)) || item.attention_reason().is_some();
            assert_eq!(item.counted(), expected, "{item:?}");
        }
    }

    /// Both halves of the rule, said out loud rather than only as an
    /// equivalence: a progress note is conversation even though it asks
    /// nothing, and activity is not even though it is the agent talking.
    #[test]
    fn every_message_counts_and_no_activity_does() {
        let mut thread = Thread::new("run-counted");
        thread.post_agent_progress("still going", None, "2026-08-29T09:00:00Z");
        for kind in [
            ThreadEventKind::Reasoning,
            ThreadEventKind::ToolUse,
            ThreadEventKind::ToolResult,
            ThreadEventKind::Narration,
            ThreadEventKind::TaskUpdate,
            ThreadEventKind::Triaged,
        ] {
            thread.push_event(kind, None, None, None, "2026-08-29T09:01:00Z");
        }
        thread.push_event(
            ThreadEventKind::Interrupted,
            None,
            None,
            None,
            "2026-08-29T09:02:00Z",
        );

        let counted: Vec<bool> = thread.items.iter().map(ThreadItem::counted).collect();
        assert_eq!(
            counted,
            vec![true, false, false, false, false, false, false, true],
            "{:?}",
            thread.items
        );
    }
}

/// Activity is the agent working — what a conversation folds into one row
/// rather than showing line by line.
#[cfg(test)]
mod activity_item_tests {
    use super::*;

    /// The two readings of the rule, held equal over every kind there is: the
    /// Rust one here, and the set the web client folds with. A kind added
    /// later cannot make them disagree without failing here.
    #[test]
    fn activity_is_the_five_kinds_the_client_folds() {
        const FOLDED_BY_THE_CLIENT: [&str; 5] = [
            "reasoning",
            "tool_use",
            "tool_result",
            "narration",
            "task_update",
        ];

        for kind in ThreadEventKind::ALL {
            assert_eq!(
                kind.is_activity(),
                FOLDED_BY_THE_CLIENT.contains(&kind.as_str()),
                "{kind:?}"
            );
        }
    }

    /// A message is never activity, whoever wrote it and whatever it reports —
    /// a run of work ends the moment somebody says something.
    #[test]
    fn no_message_is_activity_and_every_activity_event_is() {
        let mut thread = Thread::new("run-activity");
        thread.post_user("please rename the helper", None, "2026-08-29T09:00:00Z");
        thread.push_event(
            ThreadEventKind::ToolUse,
            Some("Read src/thread.rs".to_string()),
            None,
            None,
            "2026-08-29T09:01:00Z",
        );
        thread.push_event(
            ThreadEventKind::Committed,
            None,
            None,
            None,
            "2026-08-29T09:02:00Z",
        );
        thread.post_agent_progress("still going", None, "2026-08-29T09:03:00Z");

        let folded: Vec<bool> = thread.items.iter().map(ThreadItem::is_activity).collect();
        assert_eq!(
            folded,
            vec![false, true, false, false],
            "{:?}",
            thread.items
        );
    }
}

/// The one cut both page paths ship under: maximal runs of activity, capped at
/// their newest, each answered for by a digest that is exact over the whole
/// run.
#[cfg(test)]
mod activity_cut_tests {
    use super::*;

    const NOW: &str = "2026-09-06T18:03:11.412Z";

    /// The cut as a page takes it: the whole conversation newest-first, with a
    /// census that counts the tool calls of a span exactly.
    fn cut_over(thread: &Thread) -> PageCut<&ThreadItem> {
        let span: Vec<&ThreadItem> = thread.items.iter().rev().collect();
        let census = |from: u64, through: u64| {
            Ok::<u64, std::convert::Infallible>(
                thread
                    .items
                    .iter()
                    .filter(|item| {
                        item.is_tool_call() && (from..=through).contains(&item.sequence())
                    })
                    .count() as u64,
            )
        };
        match cut_activity_runs(span, census) {
            Ok(cut) => cut,
            Err(impossible) => match impossible {},
        }
    }

    fn call(thread: &mut Thread, summary: &str) -> u64 {
        thread.push_event(
            ThreadEventKind::ToolUse,
            Some(summary.to_string()),
            None,
            None,
            NOW,
        )
    }

    /// The shape the fold exists for. A thousand calls ship as a hundred, and
    /// the digest carries the truth about the rest: how many there were, and
    /// which one was last.
    #[test]
    fn a_run_of_a_thousand_calls_ships_its_newest_hundred_and_counts_them_all() {
        let mut thread = Thread::new("run-busy");
        thread.post_user("rename the helper", None, NOW);
        for index in 0..1000 {
            call(&mut thread, &format!("Read file-{index}.rs"));
        }

        let cut = cut_over(&thread);
        let shipped: Vec<u64> = cut.items.iter().map(|item| item.sequence()).collect();
        assert_eq!(
            shipped.len(),
            1 + PAGE_ACTIVITY_RUN_CAP,
            "{}",
            shipped.len()
        );
        assert_eq!(
            shipped[0], 1,
            "the message is not activity and always ships"
        );
        assert_eq!(
            &shipped[1..],
            (902..=1001).collect::<Vec<u64>>(),
            "the newest of the run, oldest-first"
        );

        let digest = cut.digests.first().expect("the run has a digest");
        assert_eq!(digest.from_sequence, 2);
        assert_eq!(digest.through_sequence, 1001);
        assert_eq!(digest.tool_calls, 1000, "exact over the whole run");
        let last = digest
            .last_tool_call
            .as_ref()
            .expect("the run called tools");
        assert_eq!(last.sequence, 1001);
        assert_eq!(last.summary.as_deref(), Some("Read file-999.rs"));
        assert_eq!(last.created_at, NOW);
        assert!(last.outcome.is_none(), "{last:?}");
    }

    /// A run that only thought counts nothing and names no call, so the row it
    /// folds to has nothing to claim.
    #[test]
    fn a_run_of_pure_reasoning_counts_no_calls_and_names_none() {
        let mut thread = Thread::new("run-quiet");
        thread.post_user("what do you make of it", None, NOW);
        for _ in 0..5 {
            thread.push_event(
                ThreadEventKind::Reasoning,
                Some("thinking".to_string()),
                None,
                None,
                NOW,
            );
        }

        let cut = cut_over(&thread);
        let digest = cut.digests.first().expect("the run has a digest");
        assert_eq!(digest.tool_calls, 0);
        assert!(digest.last_tool_call.is_none(), "{digest:?}");
        assert_eq!(cut.items.len(), 6, "nothing was capped");
    }

    /// The newest call, wherever in the run it sits: the cap keeps the newest
    /// ITEMS, and a run that thought for a hundred steps after its last call
    /// still says which call that was.
    #[test]
    fn the_last_call_is_named_even_when_the_cap_left_it_off_the_wire() {
        let mut thread = Thread::new("run-thinky");
        thread.post_user("rename the helper", None, NOW);
        let called = call(&mut thread, "Bash(cargo test)");
        for _ in 0..PAGE_ACTIVITY_RUN_CAP + 20 {
            thread.push_event(ThreadEventKind::Reasoning, None, None, None, NOW);
        }

        let cut = cut_over(&thread);
        let shipped: Vec<u64> = cut.items.iter().map(|item| item.sequence()).collect();
        assert!(!shipped.contains(&called), "the cap left the call off");
        let digest = cut.digests.first().expect("the run has a digest");
        assert_eq!(digest.tool_calls, 1);
        assert_eq!(
            digest.last_tool_call.as_ref().map(|last| last.sequence),
            Some(called)
        );
    }

    /// A run is maximal, and anything that is not activity ends one: a message
    /// or a lifecycle marker both do.
    #[test]
    fn every_non_activity_item_ends_a_run() {
        let mut thread = Thread::new("run-mixed");
        thread.post_user("rename the helper", None, NOW);
        call(&mut thread, "Read one.rs");
        call(&mut thread, "Read two.rs");
        thread.push_event(ThreadEventKind::Committed, None, None, None, NOW);
        call(&mut thread, "Read three.rs");
        thread.post_agent("done", None, NOW);
        thread.push_event(ThreadEventKind::Narration, None, None, None, NOW);

        let cut = cut_over(&thread);
        let runs: Vec<(u64, u64, u64)> = cut
            .digests
            .iter()
            .map(|digest| {
                (
                    digest.from_sequence,
                    digest.through_sequence,
                    digest.tool_calls,
                )
            })
            .collect();
        assert_eq!(runs, vec![(2, 3, 2), (5, 5, 1), (7, 7, 0)], "{runs:?}");
        assert_eq!(
            cut.items
                .iter()
                .map(|item| item.sequence())
                .collect::<Vec<u64>>(),
            (1..=7).collect::<Vec<u64>>(),
            "nothing was capped, so the page is whole"
        );
    }

    /// The open run at the end of a conversation reaches the newest thing
    /// there is, so the client knows the digest speaks for everything it holds
    /// below it.
    #[test]
    fn the_open_tail_runs_through_sequence_is_the_last_sequence() {
        let mut thread = Thread::new("run-live");
        thread.post_user("rename the helper", None, NOW);
        for index in 0..300 {
            call(&mut thread, &format!("Read file-{index}.rs"));
        }

        let cut = cut_over(&thread);
        assert_eq!(
            cut.digests.last().expect("a run").through_sequence,
            thread.last_sequence()
        );
    }

    /// A conversation with no activity in it cuts to itself: every item ships,
    /// oldest-first, and there is nothing to fold.
    #[test]
    fn a_conversation_of_words_alone_cuts_to_itself() {
        let mut thread = Thread::new("run-talky");
        for turn in 0..4 {
            thread.post_user(format!("ask {turn}"), None, NOW);
            thread.post_agent(format!("answer {turn}"), None, NOW);
        }

        let cut = cut_over(&thread);
        assert!(cut.digests.is_empty(), "{:?}", cut.digests);
        assert_eq!(
            cut.items
                .iter()
                .map(|item| item.sequence())
                .collect::<Vec<u64>>(),
            (1..=8).collect::<Vec<u64>>()
        );
    }
}

/// What an agent reported through `done`, as a status on the message it
/// posted. The message is the whole record: there is no companion event, so
/// the outcome needs the human once and a resumed agent reads it out of the
/// same packet that carries what the human said.
#[cfg(test)]
mod outcome_message_tests {
    use super::*;

    fn report() -> CompletionReport {
        CompletionReport {
            critical_files: vec!["src/render.rs — the new draw path".to_string()],
            risk_notes: vec!["untested on the legacy screen".to_string()],
            decisions: vec!["kept the old entry point".to_string()],
            skips: vec!["no perf pass".to_string()],
        }
    }

    #[test]
    fn a_completion_is_one_agent_message_carrying_its_outcome() {
        let mut thread = Thread::new("run-outcome");
        thread.post_outcome(
            MessageOutcome::Completed,
            "implemented the change",
            Some(&report()),
            "2026-08-24T09:00:00Z",
        );

        assert_eq!(thread.items.len(), 1, "{:?}", thread.items);
        let ThreadItem::Message(message) = &thread.items[0] else {
            panic!("the outcome is a message: {:?}", thread.items);
        };
        assert_eq!(message.role, MessageRole::Agent);
        assert_eq!(message.body, "implemented the change");
        assert_eq!(message.outcome, Some(MessageOutcome::Completed));
        assert!(
            message.done,
            "a completed outcome keeps the flag an older client reads"
        );
        assert_eq!(message.completion_report.as_deref(), Some(&report()));
        assert!(!message.still_working, "an outcome hands the turn back");
    }

    /// The attention job the `Done` and `Blocked` events used to do, moved onto
    /// the message whole: one unread entry per outcome, naming which it was.
    #[test]
    fn every_outcome_needs_the_human_once_and_says_which_it_was() {
        for (outcome, reason) in [
            (MessageOutcome::Completed, "done"),
            (MessageOutcome::Blocked, "blocked"),
            (MessageOutcome::Failed, "run_failed"),
        ] {
            let mut thread = Thread::new("run-outcome");
            thread.post_user("do the thing", None, "2026-08-24T09:00:00Z");
            thread.read_unread("2026-08-24T09:00:01Z");
            let cursor = thread.last_sequence();
            thread.post_outcome(
                outcome,
                "the agent's own words",
                None,
                "2026-08-24T09:01:00Z",
            );

            let unread = thread.unread_since(cursor);
            assert_eq!(unread.count, 1, "{outcome:?}");
            assert_eq!(unread.reason, Some(reason), "{outcome:?}");
            assert_eq!(
                thread.working_since(),
                None,
                "an outcome ends the turn: {outcome:?}"
            );
        }
    }

    /// Additive: `outcome` is new, `done` keeps its exact meaning, and the
    /// report the `Done` event carried rides the message instead.
    #[test]
    fn the_outcome_and_its_report_ride_the_message_on_the_wire() {
        let mut thread = Thread::new("run-outcome");
        thread.post_outcome(
            MessageOutcome::Blocked,
            "needs production credentials",
            Some(&report()),
            "2026-08-24T09:00:00Z",
        );
        thread.post_outcome(
            MessageOutcome::Completed,
            "implemented the change",
            None,
            "2026-08-24T09:02:00Z",
        );

        let wire = thread.wire_value();
        let blocked = &wire["items"][0];
        assert_eq!(blocked["type"], "message");
        assert_eq!(blocked["data"]["outcome"], "blocked");
        assert_eq!(blocked["data"]["role"], "agent");
        assert!(
            blocked["data"].get("done").is_none(),
            "only a completion sets done: {wire:?}"
        );
        assert_eq!(
            blocked["data"]["completion_report"]["risk_notes"][0],
            "untested on the legacy screen"
        );
        let completed = &wire["items"][1];
        assert_eq!(completed["data"]["outcome"], "completed");
        assert_eq!(completed["data"]["done"], true);
        assert!(
            completed["data"].get("completion_report").is_none(),
            "an outcome with no report omits the field: {wire:?}"
        );
    }

    #[test]
    fn an_ordinary_message_carries_neither_field() {
        let mut thread = Thread::new("run-outcome");
        thread.post_agent("here is what I found", None, "2026-08-24T09:00:00Z");

        let wire = thread.wire_value();
        assert!(
            wire["items"][0]["data"].get("outcome").is_none(),
            "{wire:?}"
        );
        assert!(
            wire["items"][0]["data"].get("completion_report").is_none(),
            "{wire:?}"
        );
        assert_eq!(
            thread.items[0].attention_reason(),
            Some(AGENT_MESSAGE_REASON)
        );
    }

    /// The report is the densest statement of what a change touched, so a
    /// search reads it with the summary — as it did off the `Done` event.
    #[test]
    fn a_search_reads_the_report_with_the_summary() {
        let mut thread = Thread::new("run-outcome");
        thread.post_outcome(
            MessageOutcome::Completed,
            "implemented the change",
            Some(&report()),
            "2026-08-24T09:00:00Z",
        );

        let text = thread.items[0].searchable_text();
        assert!(text.contains("implemented the change"), "{text}");
        assert!(text.contains("src/render.rs — the new draw path"), "{text}");
    }

    /// The gap this exists to close: a replacement agent is told why its
    /// predecessor blocked, out of the packet that carries the human's words.
    #[test]
    fn the_catch_up_packet_carries_the_outcome_a_predecessor_reported() {
        let mut thread = Thread::new("run-outcome");
        thread.post_user("please rename the helper", None, "2026-08-24T09:00:00Z");
        thread.post_outcome(
            MessageOutcome::Blocked,
            "needs production credentials",
            None,
            "2026-08-24T09:01:00Z",
        );
        thread.push_event(
            ThreadEventKind::ToolUse,
            Some("Read src/app.rs".to_string()),
            None,
            None,
            "2026-08-24T09:02:00Z",
        );

        assert_eq!(
            thread.catch_up_markdown(40),
            "- user: please rename the helper\n- agent [blocked]: needs production credentials",
        );
    }

    /// Every outcome is in the packet, each prefixed with which it was — and no
    /// event line is re-admitted with them.
    #[test]
    fn the_packet_names_each_outcome_and_still_carries_no_events() {
        let mut thread = Thread::new("run-outcome");
        for (outcome, summary) in [
            (MessageOutcome::Completed, "implemented the change"),
            (MessageOutcome::Blocked, "needs production credentials"),
            (MessageOutcome::Failed, "the migration will not run"),
        ] {
            thread.post_outcome(outcome, summary, None, "2026-08-24T09:00:00Z");
        }
        thread.push_event(
            ThreadEventKind::IdleUnreported,
            Some("Agent went quiet without reporting done".to_string()),
            None,
            None,
            "2026-08-24T09:03:00Z",
        );

        assert_eq!(
            thread.catch_up_markdown(40),
            "- agent [completed]: implemented the change\n\
             - agent [blocked]: needs production credentials\n\
             - agent [failed]: the migration will not run",
        );
    }

    /// A thread persisted before outcomes existed: a `Done` event carrying the
    /// report, and the companion completion message an older bridge wrote
    /// beside it. It loads, it still needs the human where it did, and the
    /// event still carries what it always carried — no migration.
    fn pre_step_7_thread() -> Thread {
        let raw = serde_json::json!({
            "id": "thread:run-old",
            "agent": { "id": "agent:run-old" },
            "items": [
                { "type": "message", "data": {
                    "id": "message-1", "sequence": 1, "role": "user",
                    "body": "please rename the helper",
                    "created_at": "2026-07-24T12:00:00Z", "seen_at": "2026-07-24T12:00:30Z" } },
                { "type": "message", "data": {
                    "id": "message-2", "sequence": 2, "role": "agent", "done": true,
                    "source": "completion", "body": "Implemented the change",
                    "created_at": "2026-07-24T12:01:00Z" } },
                { "type": "event", "data": {
                    "id": "event-3", "sequence": 3, "event": "done",
                    "created_at": "2026-07-24T12:01:00Z",
                    "summary": "Implemented the change",
                    "completion_report": { "critical_files": ["src/render.rs"] } } },
                { "type": "event", "data": {
                    "id": "event-4", "sequence": 4, "event": "blocked",
                    "created_at": "2026-07-24T12:02:00Z",
                    "summary": "needs production credentials" } }
            ],
            "next_sequence": 4
        });
        serde_json::from_value(raw).expect("a pre-outcome thread loads")
    }

    #[test]
    fn a_thread_written_before_outcomes_loads_and_reads_as_it_did() {
        let thread = pre_step_7_thread();

        let reasons: Vec<Option<&str>> = thread
            .items
            .iter()
            .map(ThreadItem::attention_reason)
            .collect();
        assert_eq!(
            reasons,
            vec![None, Some("done"), Some("done"), Some("blocked")],
            "{:?}",
            thread.items
        );
        let ThreadItem::Message(completion) = &thread.items[1] else {
            panic!("{:?}", thread.items);
        };
        assert_eq!(
            completion.outcome, None,
            "an old record carries no outcome field"
        );
        assert_eq!(completion.source, MessageSource::Completion);
        let ThreadItem::Event(done) = &thread.items[2] else {
            panic!("{:?}", thread.items);
        };
        assert_eq!(
            done.completion_report
                .as_ref()
                .map(|report| report.critical_files.clone()),
            Some(vec!["src/render.rs".to_string()]),
            "the old event still carries the report it was written with"
        );
        assert_eq!(thread.unread_since(0).count, 3);
    }

    /// The packet reads an old completion message as the completion it was:
    /// `done` without an `outcome` is a completed outcome.
    #[test]
    fn an_old_completion_message_reads_as_a_completed_outcome() {
        let thread = pre_step_7_thread();

        assert_eq!(
            thread.catch_up_markdown(40),
            "- user: please rename the helper\n- agent [completed]: Implemented the change",
        );
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
        thread.post_outcome(
            MessageOutcome::Completed,
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

    /// A conversation holding one open tool call, and the sequence that call
    /// was minted at — the handle the pump keeps and the answer comes back on.
    fn thread_with_an_open_tool_call() -> (Thread, u64) {
        let mut thread = Thread::new("run-1");
        let sequence = thread.push_event(
            ThreadEventKind::ToolUse,
            Some("Read bridge/src/app.rs".to_string()),
            None,
            None,
            "2026-08-30T09:00:00Z",
        );
        (thread, sequence)
    }

    fn event_at(thread: &Thread, sequence: u64) -> &ThreadEvent {
        thread
            .items
            .iter()
            .find_map(|item| match item {
                ThreadItem::Event(event) if event.sequence == sequence => Some(event),
                _ => None,
            })
            .expect("the row the call minted")
    }

    /// The call and its answer are ONE row: the answer lands on the row the
    /// call minted, as a suffix line and an outcome, and mints nothing beside
    /// it.
    #[test]
    fn a_tool_calls_answer_updates_the_row_the_call_minted() {
        let (mut thread, call) = thread_with_an_open_tool_call();
        assert_eq!(thread.items.len(), 1);

        assert!(thread.resolve_tool_call(call, ToolCallOutcome::Ok, "fn main() {}"));

        assert_eq!(thread.items.len(), 1, "no second row: {:?}", thread.items);
        let row = event_at(&thread, call);
        assert_eq!(
            row.summary.as_deref(),
            Some("Read bridge/src/app.rs\n→ fn main() {}")
        );
        assert_eq!(row.outcome, Some(ToolCallOutcome::Ok));
        assert!(row.updated_sequence > row.sequence, "{row:?}");
        assert_eq!(
            ThreadItem::Event(row.clone()).latest_sequence(),
            row.updated_sequence,
            "the bump is what every cursor path reads"
        );
    }

    /// An answer with nothing in it still closes the row: the outcome carries
    /// the state, and an empty suffix line would say nothing.
    #[test]
    fn an_empty_answer_closes_the_row_without_a_suffix() {
        let (mut thread, call) = thread_with_an_open_tool_call();

        assert!(thread.resolve_tool_call(call, ToolCallOutcome::Unanswered, ""));

        let row = event_at(&thread, call);
        assert_eq!(row.summary.as_deref(), Some("Read bridge/src/app.rs"));
        assert_eq!(row.outcome, Some(ToolCallOutcome::Unanswered));
        assert!(row.updated_sequence > row.sequence, "{row:?}");
    }

    /// A sequence that names no resident tool call is refused rather than
    /// guessed at, which is what sends the caller back to minting a row of its
    /// own.
    #[test]
    fn resolving_a_call_no_resident_row_holds_is_refused() {
        let (mut thread, call) = thread_with_an_open_tool_call();
        thread.push_event(
            ThreadEventKind::Narration,
            Some("dropped the index".to_string()),
            None,
            None,
            "2026-08-30T09:00:01Z",
        );
        let before = thread.last_sequence();

        assert!(!thread.resolve_tool_call(call + 1, ToolCallOutcome::Ok, "answer"));
        assert!(!thread.resolve_tool_call(9_999, ToolCallOutcome::Ok, "answer"));

        assert_eq!(
            thread.last_sequence(),
            before,
            "a refusal spends no counter value"
        );
    }

    /// The event mirror of
    /// [`wire_value_after_reships_a_message_marked_seen_after_the_cursor`]: a
    /// client whose cursor sits past the call's creation is still owed the
    /// answer, and the same bump that ships it moves the high-water mark so the
    /// row is shipped once.
    #[test]
    fn wire_value_after_reships_a_tool_call_its_answer_completed() {
        let (mut thread, call) = thread_with_an_open_tool_call();
        let cursor = thread.last_sequence();

        assert!(thread.resolve_tool_call(call, ToolCallOutcome::Ok, "fn main() {}"));

        let delta = thread.wire_value_after(cursor);
        let items = delta["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0]["data"]["sequence"], json!(call));
        assert_eq!(items[0]["data"]["outcome"], "ok");
        assert_eq!(
            items[0]["data"]["summary"],
            "Read bridge/src/app.rs\n→ fn main() {}"
        );
        let bumped = delta["thread_last_sequence"].as_u64().unwrap();
        assert!(bumped > cursor, "{delta:?}");
        let drained = thread.wire_value_after(bumped);
        assert_eq!(drained["items"].as_array().unwrap().len(), 0, "{drained:?}");
    }

    /// §6.3 invariance. An answer arriving is the agent working, never the
    /// agent addressing anyone — so the two hoisted columns, the counted
    /// predicate and the unread rule read exactly as they did before it landed.
    #[test]
    fn an_answer_landing_moves_neither_the_counted_predicate_nor_attention() {
        let (mut thread, call) = thread_with_an_open_tool_call();
        thread.post_user("drop the index", None, "2026-08-30T09:00:01Z");
        let read_to = thread.last_sequence();
        let before = event_at(&thread, call).clone();
        let attention_line = thread.last_attention_sequence();

        assert!(thread.resolve_tool_call(call, ToolCallOutcome::Error, "no such file"));

        let after = ThreadItem::Event(event_at(&thread, call).clone());
        let before = ThreadItem::Event(before);
        assert_eq!(before.counted(), after.counted());
        assert_eq!(before.attention_reason(), after.attention_reason());
        assert_eq!(after.attention_reason(), None, "activity asks for nothing");
        assert!(!after.counted(), "and buys no slot against either bound");
        assert_eq!(thread.unread_since(read_to).count, 0);
        assert_eq!(thread.last_attention_sequence(), attention_line);
    }

    /// A row written before events could mutate loads with the machinery it
    /// predates absent, and goes back to the store byte for byte as it came.
    #[test]
    fn an_event_written_before_events_mutated_loads_and_round_trips_unchanged() {
        let stored = r#"{"type":"event","data":{"id":"event-7","sequence":7,"event":"tool_use","created_at":"2026-08-01T09:00:00Z","summary":"Read bridge/src/app.rs"}}"#;

        let item: ThreadItem = serde_json::from_str(stored).expect("an old row still loads");
        let ThreadItem::Event(event) = &item else {
            panic!("{item:?}");
        };
        assert_eq!(event.updated_sequence, 0, "never mutated");
        assert_eq!(event.outcome, None, "and never answered");
        assert_eq!(item.latest_sequence(), 7, "so the max is its creation");
        assert_eq!(
            serde_json::to_string(&item).unwrap(),
            stored,
            "old rows are untouched by machinery they predate"
        );
    }

    fn thread_with_long_conversation(item_count: usize) -> Thread {
        let mut thread = Thread::new("plan-long");
        for turn in 0..item_count {
            thread.post_user(format!("ask number {turn}"), None, "2026-08-20T09:00:00Z");
        }
        thread
    }

    fn page_sequences(page: &Value) -> Vec<u64> {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["data"]["sequence"].as_u64().unwrap())
            .collect()
    }

    #[test]
    fn wire_value_page_ships_only_its_limit_but_reports_the_whole_conversation() {
        let thread = thread_with_long_conversation(200);
        let page = thread.wire_value_page(None, 25);

        let sequences = page_sequences(&page);
        assert_eq!(sequences.len(), 25, "{sequences:?}");
        // The tail of the conversation, ascending, so the client renders it in
        // the order it happened.
        assert_eq!(sequences, (176..=200).collect::<Vec<u64>>());
        assert_eq!(page["oldest_sequence"], 176);
        assert_eq!(page["has_more"], true);
        assert_eq!(page["thread_total"], 200);
        assert_eq!(page["thread_last_sequence"], 200);
        // The small bounded companions still ship in full, exactly as the
        // forward cursor ships them.
        assert_eq!(page["id"], "thread:plan-long");
        assert_eq!(page["agent"]["id"], "agent:plan-long");
        assert!(page["sessions"].is_array());
        assert!(page["revisions"].is_array());
        assert!(page.get("last_completion").is_some(), "{page:?}");
    }

    #[test]
    fn wire_value_page_names_the_conversations_newest_counter_value_not_the_pages() {
        // The idle state a reviewer opens a long conversation in: the last
        // thing the counter moved for was an in-place bump on an old item — a
        // long-queued question marked seen, a plan comment resolved — with
        // nothing posted after it. The bump lands far below the newest page.
        let mut thread = Thread::new("plan-long");
        thread.post_user("please rename the helper", None, "2026-08-20T09:00:00Z");
        for turn in 0..200 {
            thread.post_agent(format!("progress {turn}"), None, "2026-08-20T09:01:00Z");
        }
        thread.read_unread("2026-08-20T10:00:00Z");

        let page = thread.wire_value_page(None, DEFAULT_THREAD_PAGE);
        let sequences = page_sequences(&page);
        assert_eq!(sequences.len(), DEFAULT_THREAD_PAGE, "{sequences:?}");
        assert!(!sequences.contains(&1), "the bumped item is below the page");
        // One meaning for one field: the newest counter value in the whole
        // conversation, exactly as the forward cursor reports it. A page that
        // named its own top instead would leave the client asking for a
        // cursor the daemon has already moved past, so the bump would re-ship
        // on every poll for the life of the view. The client knows a page
        // delivers only its own window and reads its cursor off the items.
        assert_eq!(page["thread_last_sequence"], 202);
        assert_eq!(page["thread_total"], 201);
        assert_eq!(*sequences.last().unwrap(), 201);
    }

    #[test]
    fn wire_value_page_has_more_only_while_older_items_remain() {
        let thread = thread_with_long_conversation(30);

        let oldest_page = thread.wire_value_page(Some(11), 10);
        assert_eq!(page_sequences(&oldest_page), (1..=10).collect::<Vec<u64>>());
        assert_eq!(oldest_page["oldest_sequence"], 1);
        assert_eq!(oldest_page["has_more"], false);

        let middle_page = thread.wire_value_page(Some(21), 10);
        assert_eq!(
            page_sequences(&middle_page),
            (11..=20).collect::<Vec<u64>>()
        );
        assert_eq!(middle_page["has_more"], true);
    }

    #[test]
    fn a_conversation_shorter_than_the_page_ships_whole_and_says_so() {
        let thread = thread_with_conversation();
        let page = thread.wire_value_page(None, DEFAULT_THREAD_PAGE);

        assert_eq!(page_sequences(&page), vec![1, 2, 3]);
        assert_eq!(page["oldest_sequence"], 1);
        assert_eq!(page["has_more"], false);
        assert_eq!(page["thread_total"], 3);
    }

    #[test]
    fn paging_backward_from_oldest_sequence_walks_the_whole_conversation() {
        let thread = thread_with_long_conversation(97);

        let mut walked: Vec<u64> = Vec::new();
        let mut before = None;
        loop {
            let page = thread.wire_value_page(before, 20);
            let sequences = page_sequences(&page);
            assert!(!sequences.is_empty(), "{page:?}");
            // Prepending keeps the walk in conversation order, which is how the
            // client grows its cache upward.
            walked.splice(0..0, sequences);
            if !page["has_more"].as_bool().unwrap() {
                break;
            }
            before = Some(page["oldest_sequence"].as_u64().unwrap());
        }

        assert_eq!(walked, (1..=97).collect::<Vec<u64>>());
    }

    #[test]
    fn wire_value_page_of_an_empty_conversation_is_empty_and_final() {
        let page = Thread::new("plan-empty").wire_value_page(None, DEFAULT_THREAD_PAGE);

        assert_eq!(page["items"].as_array().unwrap().len(), 0, "{page:?}");
        assert!(page["oldest_sequence"].is_null(), "{page:?}");
        assert_eq!(page["has_more"], false);
        assert_eq!(page["thread_total"], 0);
        assert_eq!(page["thread_last_sequence"], 0);
    }

    #[test]
    fn paging_before_the_oldest_item_is_an_empty_final_page() {
        let thread = thread_with_conversation();
        let page = thread.wire_value_page(Some(1), DEFAULT_THREAD_PAGE);

        assert_eq!(page["items"].as_array().unwrap().len(), 0, "{page:?}");
        assert!(page["oldest_sequence"].is_null(), "{page:?}");
        assert_eq!(page["has_more"], false);
        assert_eq!(page["thread_total"], 3);
    }

    #[test]
    fn the_default_page_bounds_a_first_load_without_hiding_a_sitting() {
        // A sitting of the default's worth of messages opens whole, so the
        // default is not a bound the reviewer feels.
        let one_sitting = thread_with_long_conversation(DEFAULT_THREAD_PAGE);
        let sitting_page = one_sitting.wire_value_page(None, DEFAULT_THREAD_PAGE);
        assert_eq!(sitting_page["has_more"], false, "{sitting_page:?}");
        assert_eq!(page_sequences(&sitting_page).len(), DEFAULT_THREAD_PAGE);

        // Everything past it is paged, not shipped.
        let long = thread_with_long_conversation(DEFAULT_THREAD_PAGE * 4);
        let page = long.wire_value_page(None, DEFAULT_THREAD_PAGE);
        assert_eq!(page_sequences(&page).len(), DEFAULT_THREAD_PAGE);
        assert_eq!(page["has_more"], true, "{page:?}");
    }

    #[test]
    fn the_outcome_message_is_the_whole_wire_record_of_a_completion() {
        let mut thread = Thread::new("run-done");
        thread.post_agent("here is what I found", None, "2026-07-24T11:00:00Z");
        thread.post_outcome(
            MessageOutcome::Completed,
            "Implemented the change",
            None,
            "2026-07-24T12:00:00Z",
        );

        let wire = thread.wire_value();
        assert_eq!(wire["items"][0]["type"], "message");
        assert!(wire["items"][0]["data"].get("source").is_none(), "{wire:?}");
        assert_eq!(wire["items"][1]["type"], "message");
        assert_eq!(wire["items"][1]["data"]["outcome"], "completed");
        assert_eq!(wire["items"][1]["data"]["done"], true);
        assert_eq!(wire["items"][1]["data"]["body"], "Implemented the change");
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
    #[allow(clippy::cognitive_complexity)] // ratchet: a_doc_comment_is_an_anchored_post_on_the_conversation is at 19, threshold 15 — bring it under, then remove
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

/// The suggested actions an agent offers with a message, and what answering
/// one does to the conversation.
#[cfg(test)]
mod option_tests {
    use super::*;

    fn option(id: &str, label: &str, message: Option<&str>) -> MessageOption {
        MessageOption {
            id: id.to_string(),
            label: label.to_string(),
            message: message.map(str::to_string),
        }
    }

    fn offered() -> Thread {
        let mut thread = Thread::new("run-1");
        thread.post_user("the tests are red", None, "2026-08-19T09:00:00Z");
        thread.post_agent_offering(
            "Two ways out. Which?",
            None,
            Vec::new(),
            vec![
                option(
                    "option-1",
                    "Revert it",
                    Some("Revert the commit that turned the tests red."),
                ),
                option("option-2", "Fix forward", None),
            ],
            "2026-08-19T09:01:00Z",
            false,
        );
        thread
    }

    #[test]
    fn an_answer_sends_the_options_longer_text_and_falls_back_to_the_label() {
        let mut thread = offered();
        let choice = OptionChoice {
            message_id: "message-2".to_string(),
            option_ids: vec!["option-1".to_string(), "option-2".to_string()],
        };

        assert_eq!(
            thread.option_reply_text(&choice).unwrap(),
            "Revert the commit that turned the tests red.\n\nFix forward"
        );
        thread
            .post_option_reply(&choice, "2026-08-19T09:02:00Z")
            .unwrap();
        let ThreadItem::Message(reply) = thread.items.last().unwrap() else {
            panic!("the reply is a message");
        };
        assert_eq!(reply.role, MessageRole::User);
        assert_eq!(reply.answers_options_of.as_deref(), Some("message-2"));
        assert!(reply.body.contains("Fix forward"));
    }

    /// The chat's only record of what was chosen, so it has to be on the
    /// message that offered it rather than on the reply.
    #[test]
    fn the_choice_is_recorded_on_the_message_that_offered_it() {
        let mut thread = offered();
        thread
            .post_option_reply(
                &OptionChoice {
                    message_id: "message-2".to_string(),
                    option_ids: vec!["option-2".to_string()],
                },
                "2026-08-19T09:02:00Z",
            )
            .unwrap();

        let ThreadItem::Message(offer) = &thread.items[1] else {
            panic!("the offer is a message");
        };
        assert_eq!(offer.selected_options, vec!["option-2".to_string()]);
        // An in-place mutation of an already-sequenced item, so a cursored poll
        // re-ships it with the selection on it.
        assert!(offer.updated_sequence > offer.sequence, "{offer:?}");
    }

    #[test]
    fn an_option_nobody_offered_is_refused_and_leaves_the_thread_alone() {
        let mut thread = offered();
        let choice = OptionChoice {
            message_id: "message-2".to_string(),
            option_ids: vec!["option-9".to_string()],
        };

        assert!(thread.option_reply_text(&choice).is_err());
        assert!(thread
            .post_option_reply(&choice, "2026-08-19T09:02:00Z")
            .is_err());
        assert_eq!(thread.items.len(), 2);
    }

    #[test]
    fn answering_an_empty_set_is_refused() {
        let thread = offered();
        assert!(thread
            .option_reply_text(&OptionChoice {
                message_id: "message-2".to_string(),
                option_ids: Vec::new(),
            })
            .is_err());
    }

    #[test]
    fn options_are_answered_once() {
        let mut thread = offered();
        let choice = OptionChoice {
            message_id: "message-2".to_string(),
            option_ids: vec!["option-1".to_string()],
        };
        thread
            .post_option_reply(&choice, "2026-08-19T09:02:00Z")
            .unwrap();

        assert!(thread
            .post_option_reply(&choice, "2026-08-19T09:03:00Z")
            .is_err());
    }

    /// What the reviewer sees disabled, the daemon refuses: a newer message —
    /// from either side — closes the offer, and the race where one lands
    /// between the render and the press must not send a stale answer.
    #[test]
    fn a_newer_message_closes_the_offer_and_an_event_does_not() {
        let mut thread = offered();
        let choice = OptionChoice {
            message_id: "message-2".to_string(),
            option_ids: vec!["option-1".to_string()],
        };
        thread.push_event(
            ThreadEventKind::Committed,
            Some("Changes committed".to_string()),
            None,
            None,
            "2026-08-19T09:02:00Z",
        );
        assert!(thread.option_reply_text(&choice).is_ok());

        thread.post_agent(
            "actually, I found a third way",
            None,
            "2026-08-19T09:03:00Z",
        );
        assert!(thread.option_reply_text(&choice).is_err());
    }

    #[test]
    fn a_message_with_no_options_cannot_be_answered() {
        let thread = offered();
        assert!(thread
            .option_reply_text(&OptionChoice {
                message_id: "message-1".to_string(),
                option_ids: vec!["option-1".to_string()],
            })
            .is_err());
    }
}
