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
pub struct ThreadMessage {
    pub id: String,
    pub sequence: u64,
    pub role: MessageRole,
    pub body: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<MessageAnchor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_by_revision: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadEventKind {
    SessionStarted,
    SessionEnded,
    RunStarted,
    RunFailed,
    IdleUnreported,
    Done,
    RevisionCreated,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ThreadItem {
    Message(ThreadMessage),
    Event(ThreadEvent),
}

impl ThreadItem {
    pub fn sequence(&self) -> u64 {
        match self {
            ThreadItem::Message(message) => message.sequence,
            ThreadItem::Event(event) => event.sequence,
        }
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
                .map(ThreadItem::sequence)
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
        self.post_message(MessageRole::User, body.into(), anchor, now.into())
    }

    pub fn post_agent(
        &mut self,
        body: impl Into<String>,
        anchor: Option<MessageAnchor>,
        now: impl Into<String>,
    ) -> String {
        self.post_message(MessageRole::Agent, body.into(), anchor, now.into())
    }

    fn post_message(
        &mut self,
        role: MessageRole,
        body: String,
        anchor: Option<MessageAnchor>,
        now: String,
    ) -> String {
        let sequence = self.next();
        let id = format!("message-{sequence}");
        self.items.push(ThreadItem::Message(ThreadMessage {
            id: id.clone(),
            sequence,
            role,
            body,
            created_at: now,
            seen_at: None,
            anchor,
            resolved_by_revision: None,
        }));
        id
    }

    pub fn read_unread(&mut self, now: &str) -> Vec<ThreadMessage> {
        let mut unread = Vec::new();
        for item in &mut self.items {
            let ThreadItem::Message(message) = item else {
                continue;
            };
            if message.role == MessageRole::User && message.seen_at.is_none() {
                message.seen_at = Some(now.to_string());
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
        let sequence = self.next();
        self.items.push(ThreadItem::Event(ThreadEvent {
            id: format!("event-{sequence}"),
            sequence,
            event,
            created_at: now.into(),
            summary,
            session_id,
            revision_id,
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
            "revisions": self.revisions.iter().map(|revision| json!({
                "id": revision.id,
                "artifact": revision.artifact,
                "content_hash": revision.content_hash,
                "created_at": revision.created_at,
                "snapshot_available": revision.snapshot.is_some(),
            })).collect::<Vec<_>>(),
            "last_completion": self.last_completion,
        })
    }

    pub fn catch_up_markdown(&self, limit: usize) -> String {
        let mut lines = Vec::new();
        for item in self.items.iter().rev().take(limit).rev() {
            match item {
                ThreadItem::Message(message) => lines.push(format!(
                    "- {}: {}",
                    message.role.as_str(),
                    message.body.replace('\n', " ")
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
