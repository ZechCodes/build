//! Durable conversation threads paired with plans and run diffs.
//!
//! A thread belongs to a stable logical agent identity (the Build-owned plan or
//! run), while individual harness processes are recorded as session lineage.
//! Messages cost agent tokens; events and revision links do not.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Plan,
    Diff,
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

    pub fn post_completion(&mut self, summary: impl Into<String>, now: impl Into<String>) {
        self.post_message(
            MessageRole::Agent,
            true,
            summary.into(),
            None,
            Vec::new(),
            now.into(),
        );
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
        let sequence = self.next();
        let id = format!("message-{sequence}");
        self.items.push(ThreadItem::Message(ThreadMessage {
            still_working,
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
    fn a_completion_message_reads_as_done_not_as_one_more_agent_message() {
        let mut thread = Thread::new("run-1");
        thread.post_completion("implemented the change", "2026-08-13T09:00:00Z");
        assert_eq!(thread.items[0].attention_reason(), Some("done"));
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
    fn done_is_a_flag_on_the_agent_message_and_follows_the_done_event() {
        let mut thread = Thread::new("run-done");
        thread.push_event(
            ThreadEventKind::Done,
            Some("Implemented the change".to_string()),
            None,
            None,
            "2026-07-24T12:00:00Z",
        );
        thread.post_completion("Implemented the change", "2026-07-24T12:00:00Z");

        let wire = thread.wire_value();
        assert_eq!(wire["items"][0]["type"], "event");
        assert_eq!(wire["items"][0]["data"]["event"], "done");
        assert_eq!(wire["items"][1]["type"], "message");
        assert_eq!(wire["items"][1]["data"]["body"], "Implemented the change");
        assert_eq!(wire["items"][1]["data"]["done"], true);
        assert!(wire["items"][1]["data"].get("source").is_none(), "{wire:?}");
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
