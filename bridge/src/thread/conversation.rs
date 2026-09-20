use super::{
    completion_text, doc_comment_of, snapshot_contents, AgentIdentity, ArtifactKind, DocAnchor,
    DocComment, DocCommentState, EventClass, ItemMetadata, MessageAnchor, MessageAttachment,
    MessageDeliveryStatus, MessageOption, MessageOutcome, MessageRole, MessageSource, OptionChoice,
    ThreadEvent, ThreadEventDraft, ThreadEventKind, ThreadItem, ThreadLink, ThreadMessage,
    ToolCallOutcome, UnreadSummary, WorktreeScope,
};
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLineage {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub entity_id: String,
    /// The durable agent whose process this session was. Conversations may be
    /// shared explicitly, so the thread holding the lineage is not enough to
    /// recover the actor.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub conversation_id: String,
    /// The checkout this exact process was assigned. Older records did not
    /// carry it and remain readable without inventing one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout: Option<String>,
    /// Provider-owned conversation id used to resume this exact lineage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_session_id: Option<String>,
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

/// The exact session process a lifecycle callback belongs to.
///
/// This is deliberately more than an id: a delayed callback must retain the
/// agent attribution captured when the process started instead of resolving a
/// current/default actor from shared history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInstance {
    pub id: String,
    pub entity_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub checkout: String,
}

/// Immutable facts recorded when one process session starts.
pub struct SessionStart<'a> {
    pub entity_id: &'a str,
    pub agent_id: &'a str,
    pub checkout: &'a str,
    pub provider: &'a str,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub phase: &'a str,
    pub now: &'a str,
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
    /// The structured report the last `done` carried, on threads written
    /// while `done` had one. Nothing writes it any more — the summary is the
    /// whole report — and it is kept only so those records still load.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_completion: Option<CompletionReport>,
    #[serde(default)]
    next_sequence: u64,
    /// Durable inbox summary. Conversations are loaded as a bounded tail, so
    /// these cannot be derived from `items` without losing an older message
    /// behind a long run of tool activity.
    #[serde(default)]
    pub(super) last_message_sequence_summary: u64,
    /// The same, counting only what this conversation's own two parties said.
    /// A hand-off another agent wrote is words the human is not being spoken
    /// to with, and the line a dismissal is judged against is drawn here.
    #[serde(default)]
    pub(super) last_own_message_sequence_summary: u64,
    /// What the own-message line held before the newest message, so
    /// [`wear_sender`](Self::wear_sender) can put it back when that message
    /// turns out to be another agent's. Never persisted: the sender is worn
    /// in the same breath as the post.
    #[serde(skip)]
    own_message_line_before_newest: u64,
    #[serde(default)]
    pub(super) last_attention_sequence_summary: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) conversation_activity_at_summary: Option<String>,
    /// Whether the newest message left a turn in flight. This is persisted so
    /// a stopping attention event can close work even when that message is
    /// below the resident tail after restart.
    #[serde(default)]
    pub(super) conversation_working: bool,
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
    pub(super) earlier_item_count: u64,
    #[serde(skip)]
    pub(super) resident_from_sequence: u64,
    /// The newest counter value the store held for this conversation when the
    /// load read its tail. Nothing under that tail can move again in this
    /// process — only the items it holds can — so this is the exact point past
    /// which the tail is the whole answer to a cursor.
    #[serde(skip)]
    pub(super) stored_last_sequence: u64,
}

pub(super) fn empty_agent() -> AgentIdentity {
    AgentIdentity::new(String::new())
}

impl Thread {
    pub fn new(owner_id: &str) -> Self {
        Thread {
            id: format!("thread:{owner_id}"),
            agent: AgentIdentity::new(format!("agent:{owner_id}")),
            ..Thread::default()
        }
    }
    /// The conversation of one agent. `thread:<agent_id>`, and the agent
    /// identity it carries is that same agent — there is exactly one
    /// conversation per agent, so the two can never name different things.
    pub fn for_agent(agent_id: &str) -> Self {
        Thread {
            id: format!("thread:{agent_id}"),
            agent: AgentIdentity::new(agent_id.to_string()),
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
        self.agent = AgentIdentity::new(agent_id.to_string());
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
        self.refresh_conversation_summary_from_resident();
    }
    /// Install the durable/indexed summary read alongside a stored tail.
    pub fn adopt_conversation_summary(
        &mut self,
        last_message_sequence: u64,
        last_own_message_sequence: u64,
        last_attention_sequence: u64,
        activity_at: Option<String>,
        working: bool,
    ) {
        self.last_message_sequence_summary = last_message_sequence;
        self.last_own_message_sequence_summary = last_own_message_sequence;
        self.last_attention_sequence_summary = last_attention_sequence;
        self.conversation_activity_at_summary = activity_at;
        self.conversation_working = working;
        self.refresh_conversation_summary_from_resident();
    }
    pub(super) fn next(&mut self) -> u64 {
        self.next_sequence += 1;
        self.next_sequence
    }
    pub fn current_revision(&self, artifact: ArtifactKind) -> Option<&ArtifactRevision> {
        self.revisions.iter().rev().find(|r| r.artifact == artifact)
    }
    pub fn post_user(
        &mut self,
        body: impl Into<String>,
        anchor: Option<MessageAnchor>,
        now: impl Into<String>,
    ) -> String {
        self.post_user_with_context(body, anchor, None, now)
    }

    /// One agent's words in another agent's conversation.
    ///
    /// The role is the human's, because that is the side of the conversation
    /// an instruction arrives on whoever wrote it; `from_agent` is who wrote
    /// it. The two together are what let the agent reading it — and the human
    /// watching — tell a hand-off from the user speaking.
    pub fn post_user_from_agent(
        &mut self,
        body: impl Into<String>,
        from_agent: AgentIdentity,
        now: impl Into<String>,
    ) -> String {
        let id = self.post_user(body, None, now);
        self.wear_sender(from_agent);
        id
    }

    /// Say who wrote the message just posted — the last item on the thread by
    /// construction, the way the attachments and the viewing context are set.
    /// For the post paths that carry an anchor, a viewing context or files and
    /// so cannot go through [`post_user_from_agent`](Self::post_user_from_agent).
    pub fn wear_sender(&mut self, from_agent: AgentIdentity) {
        let Some(ThreadItem::Message(message)) = self.items.last_mut() else {
            return;
        };
        message.from_agent = Some(Box::new(from_agent));
        // It is a hand-off, so the line a dismissal is judged against goes back
        // where it was.
        self.unclaim_own_message_line();
    }

    /// Say which issue the message just posted handed over, set the same way
    /// the sender is and for the same reason: the post paths that carry an
    /// anchor or a viewing context cannot take one more argument each.
    ///
    /// It does NOT unclaim the own-message line the way wearing a sender does.
    /// An issue is not a sender: the human assigning one IS the human speaking,
    /// and a row they cleared should come back for it. Where an AGENT did the
    /// assigning, the sender it also wears is what moves the line.
    pub fn wear_issue(&mut self, from_issue: super::IssueEnvelope) {
        let Some(ThreadItem::Message(message)) = self.items.last_mut() else {
            return;
        };
        message.from_issue = Some(Box::new(from_issue));
    }

    pub fn post_user_with_context(
        &mut self,
        body: impl Into<String>,
        mut anchor: Option<MessageAnchor>,
        viewing_context: Option<super::ViewingContext>,
        now: impl Into<String>,
    ) -> String {
        if let Some(anchor) = &mut anchor {
            if anchor.revision_id.is_none() {
                anchor.revision_id = self.current_revision(anchor.artifact).map(|r| r.id.clone());
            }
        }
        let id = self.post_message(
            MessageRole::User,
            body.into(),
            anchor,
            Vec::new(),
            now.into(),
        );
        if let Some(ThreadItem::Message(message)) = self.items.last_mut() {
            message.viewing_context = viewing_context.map(Box::new);
        }
        id
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
        self.post_user_with_context_and_attachments(body, anchor, None, attachments, now)
    }

    pub fn post_user_with_context_and_attachments(
        &mut self,
        body: impl Into<String>,
        anchor: Option<MessageAnchor>,
        viewing_context: Option<super::ViewingContext>,
        attachments: Vec<MessageAttachment>,
        now: impl Into<String>,
    ) -> String {
        let id = self.post_user_with_context(body, anchor, viewing_context, now);
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
        self.post_doc_comment_with_context(issue_id, stage_id, path, anchor, body, None, now)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn post_doc_comment_with_context(
        &mut self,
        issue_id: &str,
        stage_id: &str,
        path: &str,
        anchor: Option<DocAnchor>,
        body: impl Into<String>,
        viewing_context: Option<super::ViewingContext>,
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
        let id = self.post_message(
            MessageRole::User,
            body.into(),
            Some(message_anchor),
            links,
            now.into(),
        );
        if let Some(ThreadItem::Message(message)) = self.items.last_mut() {
            message.viewing_context = viewing_context.map(Box::new);
        }
        id
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
        options: Vec<MessageOption>,
        now: impl Into<String>,
        still_working: bool,
    ) -> String {
        let id = self.post_agent_with_links_working(body, None, Vec::new(), now, still_working);
        if let Some(ThreadItem::Message(message)) = self.items.last_mut() {
            message.options = options;
        }
        id
    }
    /// Record a message this agent sent to another agent's conversation.
    ///
    /// Its own words on its own side of the conversation, naming who they went
    /// to — so the page the human reads shows what was sent and where. It
    /// leaves the turn open (`still_working`), because calling a tool is not
    /// handing the turn back, and that is also what keeps it from calling the
    /// human: the agent wrote to another agent, not to them.
    pub fn post_agent_sent(
        &mut self,
        body: impl Into<String>,
        sent_to: AgentIdentity,
        now: impl Into<String>,
    ) -> String {
        let id = self.post_agent_with_links_working(body, None, Vec::new(), now, true);
        if let Some(ThreadItem::Message(message)) = self.items.last_mut() {
            message.sent_to = Some(Box::new(sent_to));
        }
        self.unclaim_own_message_line();
        id
    }

    /// Take the message just posted back off the line a dismissal is judged
    /// against. For the two kinds of agent-to-agent traffic: words handed in,
    /// and the record of words sent out. A message is posted before it is
    /// signed, so the post counted it as this conversation's own.
    fn unclaim_own_message_line(&mut self) {
        let Some(ThreadItem::Message(message)) = self.items.last() else {
            return;
        };
        if self.last_own_message_sequence_summary == message.sequence {
            self.last_own_message_sequence_summary = self.own_message_line_before_newest;
        }
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

    /// Attach lifecycle status to the exact message that carried it.
    pub fn mark_agent_message_outcome(
        &mut self,
        message_id: &str,
        outcome: MessageOutcome,
        body: &str,
        report: Option<&CompletionReport>,
    ) -> bool {
        let Some(index) = self.items.iter().position(|item| {
            matches!(item, ThreadItem::Message(message) if message.id == message_id && message.role == MessageRole::Agent)
        }) else {
            return false;
        };
        let sequence = self.next();
        let ThreadItem::Message(existing) = &self.items[index] else {
            unreachable!()
        };
        let metadata = ItemMetadata::derive(
            &completion_text(body, report),
            &existing.links,
            existing.anchor.as_deref(),
            &self.scope,
        );
        let ThreadItem::Message(message) = &mut self.items[index] else {
            unreachable!()
        };
        message.outcome = Some(outcome);
        message.body = body.to_string();
        message.done = outcome == MessageOutcome::Completed;
        message.completion_report = report.cloned().map(Box::new);
        message.metadata = metadata;
        message.updated_sequence = sequence;
        self.refresh_conversation_summary_from_resident();
        true
    }
    pub(super) fn post_message(
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
    pub(super) fn post_message_working(
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
            from_agent: None,
            sent_to: None,
            from_issue: None,
            done: false,
            outcome: None,
            completion_report: None,
            source: MessageSource::Chat,
            operation_id: None,
            delivery_status: None,
            viewing_context: None,
            body,
            created_at: now,
            seen_at: None,
            anchor: anchor.map(Box::new),
            resolved_by_revision: None,
            agent_reply: None,
            links,
            attachments: Vec::new(),
            options: Vec::new(),
            selected_options: Vec::new(),
            answers_options_of: None,
        }));
        self.last_message_sequence_summary = sequence;
        self.own_message_line_before_newest = self.last_own_message_sequence_summary;
        self.last_own_message_sequence_summary = sequence;
        self.conversation_activity_at_summary = self
            .items
            .last()
            .map(ThreadItem::created_at)
            .map(str::to_string);
        self.conversation_working = match role {
            MessageRole::User => false,
            MessageRole::Agent => still_working,
        };
        if role == MessageRole::Agent && !still_working {
            self.last_attention_sequence_summary = sequence;
        }
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
                if message.role == MessageRole::User
                    && message.operation_id.is_none()
                    && message.seen_at.is_none())
        })
    }
    pub fn read_unread(&mut self, now: &str) -> Vec<ThreadMessage> {
        let mut unread = Vec::new();
        for item in &mut self.items {
            let ThreadItem::Message(message) = item else {
                continue;
            };
            if message.role == MessageRole::User
                && message.operation_id.is_none()
                && message.seen_at.is_none()
            {
                message.seen_at = Some(now.to_string());
                if message.delivery_status.is_some() {
                    message.delivery_status = Some(MessageDeliveryStatus::Seen);
                }
                // An in-place mutation of an already-sequenced item: bump its
                // updated_sequence (inlined `next()` — the loop holds a borrow
                // of `self.items`) so cursored polls re-ship the seen state.
                self.next_sequence += 1;
                message.updated_sequence = self.next_sequence;
                unread.push(message.clone());
            }
        }
        if !unread.is_empty() {
            self.conversation_working = true;
        }
        unread
    }
    /// Bind the reviewer messages appended after `previous_sequence` to one
    /// durable operation and return immutable snapshots for its delivery.
    pub fn bind_operation_messages(
        &mut self,
        operation_id: &str,
        previous_sequence: u64,
        through_sequence: u64,
    ) -> Vec<ThreadMessage> {
        self.items
            .iter_mut()
            .filter_map(|item| match item {
                ThreadItem::Message(message)
                    if message.role == MessageRole::User
                        && message.sequence > previous_sequence
                        && message.sequence <= through_sequence =>
                {
                    message.operation_id = Some(operation_id.to_string());
                    message.delivery_status = Some(MessageDeliveryStatus::Queued);
                    Some(message.clone())
                }
                _ => None,
            })
            .collect()
    }
    /// Read one accepted operation, independent of every other unread cursor.
    /// Repeating the scoped read returns the same messages and only the first
    /// read advances their seen metadata.
    pub fn read_operation_messages(
        &mut self,
        operation_id: &str,
        from_sequence: u64,
        through_sequence: u64,
        now: &str,
    ) -> Vec<ThreadMessage> {
        self.read_operation_messages_with_working(
            operation_id,
            from_sequence,
            through_sequence,
            now,
            true,
        )
    }
    /// Native delivery acknowledgement stamps the same message metadata but
    /// must not reopen work after an agent reply already closed the turn.
    pub fn read_native_operation_messages(
        &mut self,
        operation_id: &str,
        from_sequence: u64,
        through_sequence: u64,
        now: &str,
    ) -> Vec<ThreadMessage> {
        self.read_operation_messages_with_working(
            operation_id,
            from_sequence,
            through_sequence,
            now,
            false,
        )
    }
    /// Receipt for a native turn sent through the legacy, operationless path.
    /// It updates only reviewer messages in the exact range and leaves the
    /// conversation's working summary untouched.
    pub fn read_native_legacy_messages(
        &mut self,
        from_sequence: u64,
        through_sequence: u64,
        now: &str,
    ) -> Vec<ThreadMessage> {
        let mut messages = Vec::new();
        for item in &mut self.items {
            let ThreadItem::Message(message) = item else {
                continue;
            };
            if message.role != MessageRole::User
                || message.operation_id.is_some()
                || message.sequence < from_sequence
                || message.sequence > through_sequence
            {
                continue;
            }
            let mut changed = false;
            if message.delivery_status != Some(MessageDeliveryStatus::Seen) {
                message.delivery_status = Some(MessageDeliveryStatus::Seen);
                changed = true;
            }
            if message.seen_at.is_none() {
                message.seen_at = Some(now.to_string());
                changed = true;
            }
            if changed {
                self.next_sequence += 1;
                message.updated_sequence = self.next_sequence;
            }
            messages.push(message.clone());
        }
        messages
    }
    fn read_operation_messages_with_working(
        &mut self,
        operation_id: &str,
        from_sequence: u64,
        through_sequence: u64,
        now: &str,
        mark_working: bool,
    ) -> Vec<ThreadMessage> {
        let mut messages = Vec::new();
        for item in &mut self.items {
            let ThreadItem::Message(message) = item else {
                continue;
            };
            if message.operation_id.as_deref() != Some(operation_id)
                || message.sequence < from_sequence
                || message.sequence > through_sequence
            {
                continue;
            }
            let mut changed = false;
            if message.delivery_status != Some(MessageDeliveryStatus::Seen) {
                message.delivery_status = Some(MessageDeliveryStatus::Seen);
                changed = true;
            }
            if message.seen_at.is_none() {
                message.seen_at = Some(now.to_string());
                changed = true;
            }
            if changed {
                self.next_sequence += 1;
                message.updated_sequence = self.next_sequence;
            }
            messages.push(message.clone());
        }
        if mark_working && !messages.is_empty() {
            self.conversation_working = true;
        }
        messages
    }
    /// Update only the messages owned by an exact operation and sequence
    /// range. Seen is terminal: delayed handoff callbacks cannot move it back.
    pub fn set_operation_delivery_status(
        &mut self,
        operation_id: &str,
        from_sequence: u64,
        through_sequence: u64,
        status: MessageDeliveryStatus,
    ) {
        for item in &mut self.items {
            let ThreadItem::Message(message) = item else {
                continue;
            };
            if message.operation_id.as_deref() != Some(operation_id)
                || message.sequence < from_sequence
                || message.sequence > through_sequence
                || message.delivery_status == Some(status)
                || message.delivery_status == Some(MessageDeliveryStatus::Seen)
                || (status == MessageDeliveryStatus::Uncertain
                    && matches!(
                        message.delivery_status,
                        Some(MessageDeliveryStatus::Sent | MessageDeliveryStatus::Failed)
                    ))
            {
                continue;
            }
            message.delivery_status = Some(status);
            self.next_sequence += 1;
            message.updated_sequence = self.next_sequence;
        }
    }
    /// Update an exact range of legacy reviewer messages. Operation-managed
    /// messages are excluded so the two delivery paths cannot cross streams.
    pub fn set_legacy_delivery_status(
        &mut self,
        from_sequence: u64,
        through_sequence: u64,
        status: MessageDeliveryStatus,
    ) {
        for item in &mut self.items {
            let ThreadItem::Message(message) = item else {
                continue;
            };
            if message.role != MessageRole::User
                || message.operation_id.is_some()
                || message.sequence < from_sequence
                || message.sequence > through_sequence
                || message.delivery_status == Some(status)
                || message.delivery_status == Some(MessageDeliveryStatus::Seen)
                || (status == MessageDeliveryStatus::Uncertain
                    && matches!(
                        message.delivery_status,
                        Some(MessageDeliveryStatus::Sent | MessageDeliveryStatus::Failed)
                    ))
            {
                continue;
            }
            message.delivery_status = Some(status);
            self.next_sequence += 1;
            message.updated_sequence = self.next_sequence;
        }
    }
    /// Record the working-state half of an authorized operation read when its
    /// exact messages live below this process's bounded resident tail.
    pub fn note_operation_read(&mut self, now: &str) {
        self.conversation_working = true;
        self.conversation_activity_at_summary = Some(now.to_string());
    }
    /// Merge a sequence minted by a narrow store-side historical mutation into
    /// this bounded resident view so a later append cannot reuse it.
    pub fn advance_sequence_to(&mut self, sequence: u64) {
        self.next_sequence = self.next_sequence.max(sequence);
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
        let now = now.into();
        if self.conversation_working && event.class() == EventClass::Attention {
            self.conversation_activity_at_summary = Some(now.clone());
            self.conversation_working = false;
        }
        let sequence = self.next();
        if event.class() == EventClass::Attention {
            self.last_attention_sequence_summary = sequence;
        }
        self.items.push(ThreadItem::Event(ThreadEvent {
            id: format!("event-{sequence}"),
            sequence,
            updated_sequence: 0,
            event,
            created_at: now,
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

    /// Finish the newest active compaction in place, causing cursored clients
    /// to receive the updated row through its bumped sequence.
    pub fn resolve_compaction(&mut self, session_id: Option<&str>) -> bool {
        let found = self.items.iter().rposition(|item| {
            matches!(item, ThreadItem::Event(event)
                if event.event == ThreadEventKind::Compaction
                    && event.session_id.as_deref() == session_id
                    && event.summary.as_deref() == Some("Compacting"))
        });
        let Some(index) = found else {
            return false;
        };
        let bumped = self.next();
        let ThreadItem::Event(event) = &mut self.items[index] else {
            return false;
        };
        event.summary = Some("Compacted".to_string());
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
        let agent_id = self.agent.id.clone();
        self.start_agent_session(SessionStart {
            entity_id: "",
            agent_id: &agent_id,
            checkout: "",
            provider,
            model,
            effort,
            phase,
            now,
        })
        .id
    }
    /// Open one exact agent process in this conversation.
    ///
    /// A conversation can be intentionally shared, so ancestry is selected by
    /// explicit agent identity rather than by the newest row in the thread.
    pub fn start_agent_session(&mut self, start: SessionStart<'_>) -> SessionInstance {
        let SessionStart {
            entity_id,
            agent_id,
            checkout,
            provider,
            model,
            effort,
            phase,
            now,
        } = start;
        let parent_session_id = self
            .sessions
            .iter()
            .rev()
            .find(|session| session.agent_id == agent_id)
            .map(|session| session.id.clone());
        let id = format!("session-{}", uuid::Uuid::new_v4());
        let conversation_id = self.agent.id.clone();
        self.sessions.push(SessionLineage {
            id: id.clone(),
            entity_id: entity_id.to_string(),
            agent_id: agent_id.to_string(),
            conversation_id: conversation_id.clone(),
            checkout: (!checkout.is_empty()).then(|| checkout.to_string()),
            resume_session_id: None,
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
        SessionInstance {
            id,
            entity_id: entity_id.to_string(),
            agent_id: agent_id.to_string(),
            conversation_id,
            checkout: checkout.to_string(),
        }
    }
    pub fn finish_session(&mut self, session_id: &str, now: &str) {
        let Some(session) = self
            .sessions
            .iter()
            .find(|session| session.id == session_id)
        else {
            return;
        };
        let instance = SessionInstance {
            id: session.id.clone(),
            entity_id: session.entity_id.clone(),
            agent_id: session.agent_id.clone(),
            conversation_id: session.conversation_id.clone(),
            checkout: session.checkout.clone().unwrap_or_default(),
        };
        self.finish_session_instance(&instance, now);
    }
    /// Close only the process captured by `instance`.
    ///
    /// Returns false for stale, mismatched, or already-ended callbacks and
    /// records no synthetic end event for them.
    pub fn finish_session_instance(&mut self, instance: &SessionInstance, now: &str) -> bool {
        let Some(session) = self.sessions.iter_mut().find(|session| {
            session.id == instance.id
                && session.entity_id == instance.entity_id
                && session.agent_id == instance.agent_id
                && session.conversation_id == instance.conversation_id
                && session.checkout.as_deref().unwrap_or_default() == instance.checkout
                && session.ended_at.is_none()
        }) else {
            return false;
        };
        session.ended_at = Some(now.to_string());
        self.push_event(
            ThreadEventKind::SessionEnded,
            None,
            Some(instance.id.clone()),
            None,
            now,
        );
        true
    }
    /// The newest still-open process explicitly attributed to `agent_id`.
    pub fn open_session_instance(&self, agent_id: &str) -> Option<SessionInstance> {
        self.sessions
            .iter()
            .rev()
            .find(|session| session.agent_id == agent_id && session.ended_at.is_none())
            .map(|session| SessionInstance {
                id: session.id.clone(),
                entity_id: session.entity_id.clone(),
                agent_id: session.agent_id.clone(),
                conversation_id: session.conversation_id.clone(),
                checkout: session.checkout.clone().unwrap_or_default(),
            })
    }
    /// Attach the provider's resume id to the exact process that announced it.
    pub fn name_session_instance(
        &mut self,
        instance: &SessionInstance,
        resume_session_id: &str,
    ) -> bool {
        let Some(session) = self.sessions.iter_mut().find(|session| {
            session.id == instance.id
                && session.entity_id == instance.entity_id
                && session.agent_id == instance.agent_id
                && session.conversation_id == instance.conversation_id
                && session.checkout.as_deref().unwrap_or_default() == instance.checkout
        }) else {
            return false;
        };
        session.resume_session_id = Some(resume_session_id.to_string());
        true
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
    /// 0 when nothing ever has.
    ///
    /// Creation sequence, for the same reason [`unread_since`](Self::unread_since)
    /// reads it: marking a message seen bumps its `updated_sequence` and that is
    /// not the conversation speaking again. A status-only stretch after a
    /// dismissal leaves the row cleared, however long it runs.
    pub fn last_attention_sequence(&self) -> u64 {
        self.last_attention_sequence_summary.max(
            self.items
                .iter()
                .rev()
                .find(|item| item.attention_reason().is_some())
                .map(ThreadItem::sequence)
                .unwrap_or(0),
        )
    }
    /// The newest user or agent message sequence. Tool and lifecycle events
    /// never cross a message-based dismissal line.
    pub fn last_message_sequence(&self) -> u64 {
        self.last_message_sequence_summary.max(
            self.items
                .iter()
                .rev()
                .find(|item| matches!(item, ThreadItem::Message(_)))
                .map(ThreadItem::sequence)
                .unwrap_or(0),
        )
    }

    /// The same line with the hand-offs taken out: the newest message this
    /// conversation's own two parties spoke, the human's or this agent's.
    ///
    /// This is what a dismissal is drawn at and judged against. A project
    /// agent staffing a workspace, and the answer travelling back, are the
    /// work happening rather than the row calling the human — so they must
    /// not put a cleared row back on the list.
    pub fn last_own_message_sequence(&self) -> u64 {
        self.last_own_message_sequence_summary.max(
            self.items
                .iter()
                .rev()
                .find(|item| matches!(item, ThreadItem::Message(_)) && !item.is_agent_traffic())
                .map(ThreadItem::sequence)
                .unwrap_or(0),
        )
    }
}

#[cfg(test)]
mod tests;
