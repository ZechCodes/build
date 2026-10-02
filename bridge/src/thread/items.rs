use super::{empty_agent, CompletionReport, ItemMetadata, WorktreeScope};
use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Plan,
    Diff,
    /// One stage document of a task. What a plan-doc comment anchors to: a
    /// passage of a named file, the same way a diff comment anchors to a
    /// passage of a hunk.
    Doc,
}

/// Which kind of conversation owner an agent belongs to, as a message names
/// it. The two an agent can be reached at: a workspace's conversation, or a
/// project's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOwnerKind {
    Workspace,
    Project,
}

/// The workspace or project an agent's conversation belongs to, stamped onto a
/// message so a client can draw "{workspace|project} > {conversation}" over it
/// and link both halves without a second read.
///
/// `id` is the workspace id or the project id — whichever `kind` says — and
/// `name` is its display name as it stood when the message was sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentOwnerRef {
    pub kind: AgentOwnerKind,
    pub id: String,
    pub name: String,
}

/// An agent, as a message names it: the id anything addressed to it uses, and
/// enough about where it lives to show and link the conversation it spoke from.
///
/// `owner` and `topic` are stamped when the bridge posts the message and are
/// absent everywhere else — on the conversation's own `agent`, and on every
/// record written before they existed — so a reader that has never heard of
/// them sees the id it always saw. `topic` may be the empty string: a
/// conversation that has not named itself yet was still stamped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentIdentity {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<AgentOwnerRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// What to CALL this agent — one or two words it or its maker chose. What
    /// a reader sees instead of "Agent 1" on every message it sent. Absent for
    /// an agent that has not been named, and for every message written before
    /// names existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// How full the SENDER's context was when it sent these words (#68).
    /// Stamped on a message an agent wrote, and only when its harness had
    /// reported a reading; absent on every other identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<super::ContextReading>,
}

impl AgentIdentity {
    /// An agent named by its id alone — what every conversation's own `agent`
    /// is, and what a sender is before the daemon stamps where it spoke from.
    pub fn new(id: impl Into<String>) -> Self {
        AgentIdentity {
            id: id.into(),
            owner: None,
            topic: None,
            name: None,
            context: None,
        }
    }
}

/// The task a message was handed over with, when assigning one is what sent
/// it (spec: Tasks → The envelope).
///
/// The way `from_agent` names the sender: the body already carries the task as
/// prose, so a harness that never learns this field still reads the whole
/// task, and the field is what lets a client draw the message as a task card
/// and link `#12` without a second read.
///
/// Absent on every other message, so a client that has never heard of it reads
/// a conversation exactly as it always has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEnvelope {
    pub task_id: String,
    /// Per-project and sequential — what `#12` is.
    pub number: u64,
    pub title: String,
    /// What the task is about, so the agent can reach the workspace, branch
    /// or conversation without asking.
    pub links: crate::tracker::TaskLinks,
}

/// What changed on a task somebody is TRACKING, on the notice Build posted
/// about it (spec: Tasks → Tracking).
///
/// The body says the same thing in one line, so a harness reads it either way.
/// This is for a client, which draws the notice as one line that deep-links
/// the task or the comment — and cannot do that from prose it would have to
/// parse back.
///
/// Distinct from [`TaskAction`], which is an agent saying what IT did in its
/// own conversation. A notice is Build telling somebody else what a third
/// party did, and the actor is therefore part of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskNotice {
    /// Who did it: `{kind: "user"}` or `{kind: "agent", agent_id}`, and the
    /// agent's `name` beside them when it has one.
    pub actor: NoticeActor,
    /// What they did: one of `commented`, `moved`, `assigned`, `unassigned`,
    /// `closed`, `reopened`, `edited`, `linked`. A slug, the way a column is,
    /// so a client renders the wording and the bridge does not decide it
    /// twice.
    pub action: String,
    /// The comment this notice is about, on `commented` only — what lets a
    /// client link the comment rather than the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment_id: Option<String>,
    /// The columns a `moved` went between, as slugs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Who an `assigned` handed it to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<crate::tracker::Assignee>,
    /// The target agent's durable name and harness on an assignment notice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee_identity: Option<crate::tracker::TaskAgentIdentity>,
}

/// Who a notice says did the thing, as a client draws it.
///
/// The stored [`crate::tracker::Actor`] flattened, plus the agent's name. The
/// name is not on the Actor itself on purpose: an Actor is written into every
/// event and comment the tracker stores, and a name copied into all of them
/// would be a hundred stale copies the first time an agent renames itself.
/// Here it is resolved when the notice is written and read once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeActor {
    #[serde(flatten)]
    pub who: crate::tracker::Actor,
    /// What to call them, when they have a name. Absent for the user, and for
    /// an agent nobody has named — a client falls back to the ordinal or the
    /// id, as it did before names existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The author as this task knew them when the notice was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<crate::tracker::TaskAgentIdentity>,
}

/// What an agent did to a task, on the message it posted saying so (spec:
/// Tasks → An agent says what it did).
///
/// Distinct from [`TaskEnvelope`], which says a task was HANDED to somebody.
/// This says the agent acted on one. `action` is a slug rather than a label so
/// the client renders the wording and the bridge does not decide it twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskAction {
    pub action: String,
    pub task_id: String,
    pub number: u64,
    pub title: String,
    /// Present only on `commented_on`, and what lets a client deep-link the
    /// comment rather than the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment_id: Option<String>,
    /// Who the task went to, on `assigned` only — the same shape
    /// `task_notice.assignee` carries.
    ///
    /// Without it the agent's own line can say "assigned" and not to whom,
    /// which is the half a reader wants. Absent on an `unassigned`, which went
    /// to nobody, and on every other action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<crate::tracker::Assignee>,
    /// The column a `moved` went to, as a slug — what lets the agent's own
    /// line say "Moved #13 to “In review”" (#323). Absent on every other
    /// action, and from a bridge before wire 3.5.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
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

pub(super) fn is_false(value: &bool) -> bool {
    !*value
}

/// Never mutated, for the counter an event carries only once it has been: an
/// event that was written and never touched again serializes exactly as it did
/// before events could mutate at all.
pub(super) fn is_zero(value: &u64) -> bool {
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
    /// Legacy: agents used to attach file links to their messages. Nothing
    /// writes one now; the variant stays so stored conversations still load.
    File {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line_start: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line_end: Option<u32>,
    },
    /// Legacy stage reference retained for clients and records predating the
    /// Task cutover. New lifecycle events emit `TaskStage`.
    PlanStage {
        plan_id: String,
        stage_id: String,
        path: String,
    },
    /// Canonical reference to one ordered stage-plan document owned by a Task.
    TaskStage {
        task_id: String,
        stage_id: String,
        path: String,
    },
    /// Legacy implementation reference retained as a wire alias.
    Run { run_id: String },
    /// Canonical implementation lineage reference, explicitly scoped to Task.
    Implementation {
        task_id: String,
        implementation_id: String,
    },
    /// Stable server-minted identity of a checkout.
    Worktree { worktree_id: String },
    /// Exact immutable commit boundary.
    Commit { sha: String },
    /// Legacy: the retired branch-recovery agent's attempt. Nothing writes
    /// one now; the variant stays so stored conversations still load.
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
    pub(super) fn reply_text(&self) -> &str {
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

/// How many of each kind one item may claim. A message naming forty files is
/// a paste, not a reference, and the index is a pointer either way.
pub(super) const MAX_METADATA_ENTRIES: usize = 20;

impl ItemMetadata {
    pub fn is_empty(&self) -> bool {
        self.commits.is_empty() && self.files.is_empty() && self.stages.is_empty()
    }

    /// Read an item's findability out of what it says and what it links to.
    pub(super) fn derive(
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
                | ThreadLink::TaskStage { stage_id, path, .. } => {
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
pub(super) fn push_unique(values: &mut Vec<String>, value: String) {
    if values.len() >= MAX_METADATA_ENTRIES || values.contains(&value) {
        return;
    }
    values.push(value);
}

/// The words of a body, with the punctuation people wrap them in removed:
/// backticks, quotes, brackets and sentence punctuation are how a sentence is
/// written, not part of the path or sha inside it. A leading dot IS part of a
/// path (`.build/plan/…`), so only trailing dots are stripped.
pub(super) fn prose_tokens(text: &str) -> impl Iterator<Item = &str> {
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
pub(super) fn is_commit_sha(token: &str) -> bool {
    (7..=40).contains(&token.len())
        && token.bytes().all(|byte| byte.is_ascii_hexdigit())
        && token.bytes().any(|byte| byte.is_ascii_digit())
}

/// Whether a token is shaped like a path at all — a directory separator, or a
/// short extension. Shape alone never records a file; the checkout decides.
pub(super) fn looks_like_a_path(token: &str) -> bool {
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
    /// The agent that sent this message, when an agent sent it and not the
    /// human.
    ///
    /// [`role`](Self::role) says which side of the conversation a message is
    /// on, not who wrote it: an instruction one agent hands another lands on
    /// the inbound side, exactly where the human's words land, and names its
    /// sender here. Absent on everything the human said, and on every record
    /// written before agents could speak to each other — so a client that has
    /// never heard of the field reads those messages exactly as it always has.
    ///
    /// Boxed for the reason [`completion_report`](Self::completion_report) is:
    /// almost every message on almost every conversation is the human's, and
    /// the identity of the rare sender must not cost the rest of them a word.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_agent: Option<Box<AgentIdentity>>,
    /// The agent this message was sent TO, when it is the record of a send
    /// rather than something said here.
    ///
    /// The other half of [`from_agent`](Self::from_agent): the recipient's
    /// conversation gets the words wearing the sender, and the sender's own
    /// gets them wearing the recipient, so both ends of a hand-off are on a
    /// page and either can be drawn and linked. The role is the agent's,
    /// because the agent wrote them.
    ///
    /// It is a message, so a page shows it and a run of activity is cut around
    /// it. It is not a hand-off — nobody handed this conversation anything —
    /// and it calls nobody: the human was not written to.
    ///
    /// Boxed for the reason `from_agent` is, and absent on everything that went
    /// nowhere, which is every message written before agents could write to
    /// each other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_to: Option<Box<AgentIdentity>>,
    /// Whether BUILD wrote this message rather than the human or an agent.
    ///
    /// One thing says it today: the notice a resumed agent is given after the
    /// daemon restarted under it. The role is the human's for the reason
    /// [`from_agent`](Self::from_agent) explains — an instruction lands on the
    /// inbound side whoever wrote it — and this is what stops the agent, and
    /// the human reading over its shoulder, taking Build's words for the
    /// user's and answering a question nobody asked.
    ///
    /// Not an [`AgentIdentity`]: Build is not an agent, has no conversation and
    /// cannot be written back to, and giving it a borrowed agent id would make
    /// every `from_agent` reader believe in an agent that does not exist.
    ///
    /// Absent on every other message and on every record written before it,
    /// so a client that has never heard of it reads those exactly as it has.
    #[serde(default, skip_serializing_if = "is_false")]
    pub from_build: bool,
    /// The task this message handed over, when assigning one is what sent it.
    ///
    /// Boxed for the reason `from_agent` is: almost every message on almost
    /// every conversation is somebody talking, and a task's title, body and
    /// links must not cost the rest of them a word.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_task: Option<Box<TaskEnvelope>>,
    /// What this agent did to a task, when the message is the agent saying
    /// so. Boxed for the reason `from_task` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_action: Option<Box<TaskAction>>,
    /// What changed on a tracked task, when the message is Build's notice
    /// about it. Boxed for the reason `from_task` is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_notice: Option<Box<TaskNotice>>,
    /// Client mutation whose durable delivery owns this reviewer message.
    /// Managed messages are read through that exact operation and never by
    /// the legacy catch-all unread mailbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    /// Durable progress of an operation-managed reviewer message through the
    /// provider handoff. Legacy and unmanaged messages omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery_status: Option<MessageDeliveryStatus>,
    /// The UI state the reviewer deliberately sent with these words. Kept as
    /// structured message metadata so rendering can show it without rewriting
    /// the body, and old records remain byte-compatible when it is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewing_context: Option<Box<ViewingContext>>,
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
    /// What this message is a comment ON, when it is one: a passage of a diff,
    /// of a plan, or of a stage document.
    ///
    /// Boxed for the reason [`completion_report`](Self::completion_report) is.
    /// An anchor is six fields wide and the rarest of them on a conversation
    /// that is mostly prose, so carrying it inline would cost every ordinary
    /// message its bulk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Box<MessageAnchor>>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageDeliveryStatus {
    Queued,
    /// Reserved for native handoff and awaiting a correlated input receipt.
    /// The UI continues to show Queued; recovery must not replay this input.
    Submitted,
    Sent,
    Seen,
    Uncertain,
    Failed,
}

pub const MAX_VIEWING_CONTEXT_ITEMS: usize = 100;
pub const MAX_VIEWING_CONTEXT_PATH_BYTES: usize = 4 * 1024;
pub const MAX_VIEWING_CONTEXT_EXCERPT_BYTES: usize = 32 * 1024;
/// How long a word naming something may be — a workspace's id, a workspace's
/// name. It reaches the agent as one line of prose, not as a document.
pub const MAX_VIEWING_CONTEXT_LABEL_BYTES: usize = 512;

/// What the reviewer was looking at when they wrote. Deliberately WITHOUT
/// `deny_unknown_fields`: this rides v1 request paths (`thread.post`,
/// `run.message`), where a newer SPA may
/// name a field this bridge predates and must not be refused for it. An
/// unknown field is ignored here and dropped on the way back out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewingContext {
    pub version: u8,
    pub items: Vec<ViewingContextItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffViewingMode {
    Uncommitted,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionSide {
    Old,
    New,
}

/// One thing on screen. Ignores an unknown field for the same reason
/// [`ViewingContext`] does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ViewingContextItem {
    File {
        path: String,
    },
    Commit {
        sha: String,
    },
    Diff {
        path: String,
        mode: DiffViewingMode,
    },
    /// The workspace the user was standing in when they wrote. Not a thing on
    /// screen the way the others are: the project's conversation is reachable
    /// from every workspace's rail, so a message sent from one carries which
    /// one it came from, and "this workspace" has an answer.
    Workspace {
        workspace_id: String,
        name: String,
    },
    /// The task the user had open when they wrote. Not a thing in the
    /// checkout the way a file is: the board is its own surface, and "this
    /// task" is a question an agent is asked while looking at neither.
    ///
    /// Carries what it takes to NAME the task and no more. The body is not
    /// here on purpose — a task changes after the message is sent, and a
    /// copy frozen into viewing context would go stale while reading as
    /// current. `get_task` is how the agent reads it.
    Task {
        task_id: String,
        number: u64,
        title: String,
    },
    Selection {
        path: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line_start: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line_end: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        side: Option<SelectionSide>,
        #[serde(default, skip_serializing_if = "is_false")]
        unsaved: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        truncated: bool,
    },
}

impl ViewingContext {
    /// The workspace this message was sent from, when it says: its name and its
    /// id, for the line that tells the agent where the user was standing. The
    /// rail leads the context with it and sends at most one, so the first is
    /// the answer.
    pub fn workspace(&self) -> Option<(&str, &str)> {
        self.items.iter().find_map(|item| match item {
            ViewingContextItem::Workspace { workspace_id, name } => {
                Some((workspace_id.as_str(), name.as_str()))
            }
            _ => None,
        })
    }

    /// The task the user had open when they wrote, when the message says:
    /// its number, its title and its id, for the line that tells the agent
    /// what is on screen. The board sends at most one, so the first is the
    /// answer — the same rule the workspace item follows.
    pub fn task(&self) -> Option<(u64, &str, &str)> {
        self.items.iter().find_map(|item| match item {
            ViewingContextItem::Task {
                task_id,
                number,
                title,
            } => Some((*number, title.as_str(), task_id.as_str())),
            _ => None,
        })
    }

    pub fn normalize(mut self) -> Result<Self, String> {
        for item in &mut self.items {
            if let ViewingContextItem::Commit { sha } = item {
                sha.make_ascii_lowercase();
            }
        }
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("viewing_context.version must be 1".to_string());
        }
        if self.items.is_empty() {
            return Err("viewing_context.items must not be empty".to_string());
        }
        if self.items.len() > MAX_VIEWING_CONTEXT_ITEMS {
            return Err(format!(
                "viewing_context.items must contain at most {MAX_VIEWING_CONTEXT_ITEMS} entries"
            ));
        }
        let mut excerpt_bytes = 0usize;
        for item in &self.items {
            match item {
                ViewingContextItem::File { path } | ViewingContextItem::Diff { path, .. } => {
                    validate_viewing_path(path)?
                }
                ViewingContextItem::Commit { sha } => {
                    if !matches!(sha.len(), 40 | 64)
                        || !sha.bytes().all(|byte| byte.is_ascii_hexdigit())
                    {
                        return Err("viewing_context commit sha must be a full 40 or 64 character object id".to_string());
                    }
                }
                ViewingContextItem::Workspace { workspace_id, name } => {
                    validate_viewing_workspace(workspace_id, name)?
                }
                ViewingContextItem::Task { task_id, title, .. } => {
                    validate_viewing_task(task_id, title)?
                }
                ViewingContextItem::Selection {
                    path,
                    text,
                    line_start,
                    line_end,
                    ..
                } => {
                    validate_viewing_path(path)?;
                    excerpt_bytes = excerpt_bytes.saturating_add(text.len());
                    if text.is_empty() {
                        return Err("viewing_context selection text must not be empty".to_string());
                    }
                    if line_start.is_some_and(|line| line == 0)
                        || line_end.is_some_and(|line| line == 0)
                        || (line_start.is_none() && line_end.is_some())
                        || matches!((line_start, line_end), (Some(start), Some(end)) if start > end)
                    {
                        return Err("viewing_context selection line range is invalid".to_string());
                    }
                }
            }
        }
        if excerpt_bytes > MAX_VIEWING_CONTEXT_EXCERPT_BYTES {
            return Err(format!("viewing_context selection excerpts exceed {MAX_VIEWING_CONTEXT_EXCERPT_BYTES} bytes"));
        }
        Ok(())
    }
}

/// A workspace item names one: both words are for the agent reading them, so
/// neither may be empty, and neither is a path this bridge will open.
fn validate_viewing_workspace(workspace_id: &str, name: &str) -> Result<(), String> {
    if workspace_id.is_empty()
        || name.is_empty()
        || workspace_id.len() > MAX_VIEWING_CONTEXT_LABEL_BYTES
        || name.len() > MAX_VIEWING_CONTEXT_LABEL_BYTES
    {
        return Err(format!("viewing_context workspace must carry an id and a name of at most {MAX_VIEWING_CONTEXT_LABEL_BYTES} bytes"));
    }
    Ok(())
}

/// A task item names one, and the same way a workspace item does: both words
/// are for the agent reading them, so neither may be empty and neither may be
/// the size of a document. The number needs no check — every `u64` is one.
fn validate_viewing_task(task_id: &str, title: &str) -> Result<(), String> {
    if task_id.is_empty()
        || title.is_empty()
        || task_id.len() > MAX_VIEWING_CONTEXT_LABEL_BYTES
        || title.len() > MAX_VIEWING_CONTEXT_LABEL_BYTES
    {
        return Err(format!("viewing_context task must carry an id and a title of at most {MAX_VIEWING_CONTEXT_LABEL_BYTES} bytes"));
    }
    Ok(())
}

fn validate_viewing_path(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.len() > MAX_VIEWING_CONTEXT_PATH_BYTES
        || !crate::plan::is_worktree_contained_path(path)
    {
        return Err(format!("viewing_context path must be a scope-relative path of at most {MAX_VIEWING_CONTEXT_PATH_BYTES} bytes"));
    }
    Ok(())
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
pub(super) fn doc_comment_of(message: &ThreadMessage) -> Option<DocComment> {
    if message.role != MessageRole::User {
        return None;
    }
    let anchor = message.anchor.as_ref()?;
    if anchor.artifact != ArtifactKind::Doc {
        return None;
    }
    let (stage_id, path) = message.links.iter().find_map(|link| match link {
        ThreadLink::TaskStage { stage_id, path, .. }
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
    /// Legacy: a stage's validation failed. The validation gate is retired;
    /// the kind stays so stored conversations still load.
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
    /// Legacy: the retired branch-recovery agent started. Retained for
    /// persisted compatibility.
    RecoveryStarted,
    /// Legacy: the retired branch-recovery agent succeeded. Retained for
    /// persisted compatibility.
    RecoverySucceeded,
    /// Build could not restore an implementation's worktree.
    RecoveryFailed,
    WorktreeDeleted,
    StageCompleted,
    ImplementationArchived,
    StageInvalidated,
    Committed,
    Pushed,
    Merged,
    Abandoned,
    /// Legacy: a triage pass classified the diff. Triage is retired; the
    /// kind stays so stored conversations still load.
    Triaged,
    /// Legacy: the reviewer overrode a triage level. Retained for persisted
    /// compatibility.
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
    /// The provider is compacting its context. Completion updates this row.
    Compaction,
}

impl ThreadEventKind {
    /// Every variant, so the wire-token and class rules can be checked over the
    /// whole enum instead of a sample of it.
    pub const ALL: [ThreadEventKind; 38] = [
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
        ThreadEventKind::Compaction,
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
            | ThreadEventKind::Compaction
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
            ThreadEventKind::Compaction => "compaction",
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
    /// What a compaction Build asked for was focused on, and the context either
    /// side of it, on the `Compaction` row it produced. Absent on every other
    /// event, on compactions Build did not ask for, and on every record
    /// written before compactions were measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CompactionDetail>,
    /// What this event referenced, derived when it was pushed. See
    /// [`ThreadMessage::metadata`].
    #[serde(default, skip_serializing_if = "ItemMetadata::is_empty")]
    pub metadata: ItemMetadata,
}

/// One compaction as Build asked for it and saw it land.
///
/// Every part is optional because each arrives on its own clock: the focus and
/// the context before it are known when the command is sent, the context after
/// it only once the harness next reports one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionDetail {
    /// The focus the summary was asked to keep. Absent for an automatic
    /// compaction, and for a harness that compacts without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// The context, in tokens, the agent's last turn left before it was sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_before: Option<u64>,
    /// The first context the harness reported after it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_after: Option<u64>,
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
pub(super) fn count_serialized_items(count: usize) {
    ITEMS_SERIALIZED.with(|counter| counter.set(counter.get() + count));
}

/// Nothing is counted outside the tests: only they ask what a payload cost.
#[cfg(not(test))]
pub(super) fn count_serialized_items(_count: usize) {}

impl ThreadMessage {
    /// Whether a legacy delivery still owes the agent this message: the
    /// human's, carried by no operation, not seen, and never sent or only
    /// queued. `Store::waiting_legacy_messages` asks the database the same.
    pub fn awaits_legacy_delivery(&self) -> bool {
        self.role == MessageRole::User
            && self.operation_id.is_none()
            && self.seen_at.is_none()
            && matches!(
                self.delivery_status,
                None | Some(MessageDeliveryStatus::Queued)
            )
    }

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
    pub(super) fn tool_call(&self) -> Option<&ThreadEvent> {
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

    /// Whether this item is a message one agent handed to another: the user's
    /// role, because that is the side an instruction arrives on whoever wrote
    /// it, and `from_agent` saying a machine wrote it.
    ///
    /// The one thing that reads differently from everything else a
    /// conversation holds: it is words, and it is not the human being spoken
    /// to. So it never crosses the line a dismissal drew.
    ///
    /// The store filters the same rule as `handoff = 1` over the hoisted
    /// column.
    pub fn is_handoff(&self) -> bool {
        matches!(self, ThreadItem::Message(message) if message.from_agent.is_some())
    }

    /// Whether this item is one agent writing to another, in either direction:
    /// a message handed to this conversation, or the record of one sent out of
    /// it.
    ///
    /// Neither is this conversation's own two parties speaking, so neither
    /// draws the line a dismissal is judged against — a row the human cleared
    /// stays cleared while the agents work.
    pub fn is_agent_traffic(&self) -> bool {
        matches!(
            self,
            ThreadItem::Message(message)
                if message.from_agent.is_some() || message.sent_to.is_some()
        )
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
pub(super) fn completion_text(summary: &str, report: Option<&CompletionReport>) -> String {
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

/// How much of an item a hit shows. A pointer, not a transcript.
pub(super) const EXCERPT_MAX_CHARS: usize = 200;

/// One line of the item, centred on what was asked for when that is somewhere
/// in the middle of a long body.
pub(super) fn excerpt_around(text: &str, needle: Option<&str>) -> String {
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

impl Default for AgentIdentity {
    fn default() -> Self {
        empty_agent()
    }
}

#[cfg(test)]
mod tests;
