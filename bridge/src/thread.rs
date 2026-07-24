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
    /// Drawn from the same counter as `sequence` and bumped whenever the
    /// message mutates in place (`seen_at`, `resolved_by_revision`), so the
    /// cursor protocol re-ships the newer copy of an already-held item.
    /// Defaults to 0 (never mutated) on records persisted before this field.
    #[serde(default)]
    pub updated_sequence: u64,
    pub role: MessageRole,
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
}

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
    ImplementationStarted,
    Committed,
    Pushed,
    Merged,
    Abandoned,
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
            MessageSource::Chat,
            body.into(),
            anchor,
            now.into(),
        )
    }

    pub fn post_agent(
        &mut self,
        body: impl Into<String>,
        anchor: Option<MessageAnchor>,
        now: impl Into<String>,
    ) -> String {
        self.post_message(
            MessageRole::Agent,
            MessageSource::Chat,
            body.into(),
            anchor,
            now.into(),
        )
    }

    pub fn post_completion(&mut self, report: &CompletionReport, now: impl Into<String>) -> String {
        self.last_completion = Some(report.clone());
        self.post_message(
            MessageRole::Agent,
            MessageSource::Completion,
            completion_report_markdown(report),
            None,
            now.into(),
        )
    }

    fn post_message(
        &mut self,
        role: MessageRole,
        source: MessageSource,
        body: String,
        anchor: Option<MessageAnchor>,
        now: String,
    ) -> String {
        let sequence = self.next();
        let id = format!("message-{sequence}");
        self.items.push(ThreadItem::Message(ThreadMessage {
            id: id.clone(),
            sequence,
            updated_sequence: sequence,
            role,
            source,
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
                ThreadItem::Message(message) if message.source == MessageSource::Completion => {}
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

fn completion_report_markdown(report: &CompletionReport) -> String {
    let mut sections = vec!["**Completion report**".to_string()];
    for (label, values) in [
        ("Critical files", &report.critical_files),
        ("Risks", &report.risk_notes),
        ("Decisions", &report.decisions),
        ("Skipped", &report.skips),
    ] {
        if values.is_empty() {
            continue;
        }
        sections.push(format!(
            "**{label}**\n{}",
            values
                .iter()
                .map(|value| format!("- {value}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    sections.join("\n\n")
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
}
