//! Every message Build sends into an agent's conversation.
//!
//! A message is composed once — its [`MessageKind`], a body, and the short
//! attachments it carries — and framed by the harness that receives it.
//! Each harness answers two questions about a message, and both are required
//! methods on [`Harness`], so a new kind or a new harness cannot compile until
//! it has answered them:
//!
//! - [`Harness::framing`]: whether a kind travels with its attachments or
//!   bare, byte for byte (a provider command the CLI must parse);
//! - [`Harness::wrap_attachment`]: how an attachment is marked as Build's
//!   instruction rather than the user's words on that harness.
//!
//! Attachments are typed ([`Attachment`]) and name Build's tools as
//! [`BuildTool`]s, so each harness spells them the way its model sees them
//! ([`Harness::build_tool_name`]) and says how to load them where they hide
//! behind tool search ([`Harness::tool_loading`]).
//!
//! Attachments ride in the turn's text, after the body. No harness has a
//! per-turn system channel, and the app server's receipts match the echoed
//! text, so the text is the one place every carrier already handles.
//!
//! The wording lives in `bridge/templates/messages/`, beside the role prompts.

#[cfg(test)]
pub(crate) mod golden;

use crate::harness::Harness;
use crate::templates::{
    MESSAGE_BUILD_INSTRUCTIONS, MESSAGE_REPLY_AGENT, MESSAGE_REPLY_TASK, MESSAGE_REPLY_THREAD,
    MESSAGE_SYSTEM_REMINDER, MESSAGE_TOOL_HINT_TOOL_SEARCH,
};

/// Every kind of message Build sends into an agent's conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageKind {
    /// Thread messages handed over as an operation: the user's, another
    /// agent's, or Build's notices.
    Delivery,
    /// A nudge to read new thread messages.
    ThreadNudge,
    /// The first turn of a fresh session: role prompt and conversation rules.
    ColdStart,
    /// A tracked task changed: a comment, a move, an assignment.
    TaskNotice,
    /// A task handed to the agent to work on.
    TaskAssignment,
    /// A task the agent holds was unblocked, released or re-blocked.
    DependencyNotice,
    /// Tasks the agent holds that have gone quiet.
    TaskReminder,
    /// Workspaces the agent owns that have sat idle.
    IdleWorkspaces,
    /// The bridge restarted under the agent.
    Restarted,
    /// The provider's usage limit lifted.
    UsageLimitResume,
    /// The agent's last turn failed.
    AgentFailure,
    /// Build could not recover the provider session.
    ProviderRecoveryFailure,
    /// The command that compacts the agent's context.
    Compaction,
    /// A provider command the user typed, which the CLI must parse as typed.
    ProviderCommand,
}

/// How a harness carries a message of one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Framing {
    /// The body, then each attachment wrapped by the harness.
    WithAttachments,
    /// The body alone, untouched: a command the provider parses.
    Bare,
}

/// A Build MCP tool a message tells the agent to use, by the name Build's MCP
/// server gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildTool {
    PostThreadMessage,
    CommentTask,
    ReadComment,
    MessageAgent,
    GetTask,
    MoveTask,
    MentionUser,
    SetName,
}

impl BuildTool {
    /// The tool's name on Build's MCP server.
    pub fn name(self) -> &'static str {
        match self {
            BuildTool::PostThreadMessage => "post_thread_message",
            BuildTool::CommentTask => "comment_task",
            BuildTool::ReadComment => "read_comment",
            BuildTool::MessageAgent => "message_agent",
            BuildTool::GetTask => "get_task",
            BuildTool::MoveTask => "move_task",
            BuildTool::MentionUser => "mention_user",
            BuildTool::SetName => "set_name",
        }
    }

    /// The name a CLI gives the tool when it reaches it through the MCP server
    /// configured as `build`: `mcp__build__<name>`.
    pub fn mcp_name(self) -> String {
        format!("mcp__build__{}", self.name())
    }
}

/// Whether a harness's model can call Build's tools straight away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolLoading {
    /// The tools are listed by name only and loaded with ToolSearch.
    ToolSearch,
    /// The tools are in the model's tool list from the start.
    Loaded,
}

/// Where the agent answers the words it was handed: the place they were said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyChannel {
    /// The user wrote in the agent's thread.
    Thread,
    /// Somebody commented on a task.
    Task { task_id: String },
    /// Another agent sent the words.
    Agent { agent_id: String },
}

impl ReplyChannel {
    /// The tool that answers on this channel.
    pub fn tool(&self) -> BuildTool {
        match self {
            ReplyChannel::Thread => BuildTool::PostThreadMessage,
            ReplyChannel::Task { .. } => BuildTool::CommentTask,
            ReplyChannel::Agent { .. } => BuildTool::MessageAgent,
        }
    }
}

/// One short instruction from Build riding on a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attachment {
    /// Where and with which tool to answer.
    Reply(ReplyChannel),
    /// How to load these tools, on a harness that hides them behind tool
    /// search. Says nothing where the tools are already loaded.
    ToolHint(Vec<BuildTool>),
}

impl Attachment {
    /// The attachment's words on `harness`, unwrapped, or `None` when it has
    /// nothing to say there.
    pub fn text(&self, harness: &dyn Harness) -> Option<String> {
        match self {
            Attachment::Reply(channel) => {
                let tool = harness.build_tool_name(channel.tool());
                Some(match channel {
                    ReplyChannel::Thread => MESSAGE_REPLY_THREAD.replace("{tool}", &tool),
                    ReplyChannel::Task { task_id } => MESSAGE_REPLY_TASK
                        .replace("{task_id}", task_id)
                        .replace("{tool}", &tool),
                    ReplyChannel::Agent { agent_id } => MESSAGE_REPLY_AGENT
                        .replace("{agent_id}", agent_id)
                        .replace("{tool}", &tool),
                })
            }
            Attachment::ToolHint(tools) => match harness.tool_loading() {
                ToolLoading::ToolSearch => {
                    let names: Vec<String> = tools
                        .iter()
                        .map(|tool| harness.build_tool_name(*tool))
                        .collect();
                    Some(MESSAGE_TOOL_HINT_TOOL_SEARCH.replace("{tools}", &names.join(",")))
                }
                ToolLoading::Loaded => None,
            },
        }
    }
}

/// What a turn asks of the agent beyond its words: where to answer, and the
/// other Build tools the words tell it to use. Decided where the delivered
/// messages are known, and turned into attachments by
/// [`ComposedMessage::with`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnAttachments {
    pub replies: Vec<ReplyChannel>,
    pub tools: Vec<BuildTool>,
}

impl TurnAttachments {
    /// Add what `other` asks, keeping the first of anything asked twice.
    pub fn merge(&mut self, other: TurnAttachments) {
        for reply in other.replies {
            if !self.replies.contains(&reply) {
                self.replies.push(reply);
            }
        }
        for tool in other.tools {
            if !self.tools.contains(&tool) {
                self.tools.push(tool);
            }
        }
    }
}

/// A message ready to be framed for whichever harness receives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedMessage {
    pub kind: MessageKind,
    pub body: String,
    /// Short instructions from Build; the harness words and marks them.
    pub attachments: Vec<Attachment>,
}

impl ComposedMessage {
    /// A message of `kind` saying `body`, with no attachments yet.
    pub fn new(kind: MessageKind, body: impl Into<String>) -> ComposedMessage {
        ComposedMessage {
            kind,
            body: body.into(),
            attachments: Vec::new(),
        }
    }

    /// This message carrying a reply attachment per channel in `asks`, then
    /// one tool hint naming every tool the turn uses, reply tools first.
    pub fn with(mut self, asks: TurnAttachments) -> ComposedMessage {
        let mut tools: Vec<BuildTool> = Vec::new();
        for tool in asks
            .replies
            .iter()
            .map(ReplyChannel::tool)
            .chain(asks.tools)
        {
            if !tools.contains(&tool) {
                tools.push(tool);
            }
        }
        self.attachments
            .extend(asks.replies.into_iter().map(Attachment::Reply));
        if !tools.is_empty() {
            self.attachments.push(Attachment::ToolHint(tools));
        }
        self
    }

    /// The text `harness` is handed: the body, then each attachment as the
    /// harness words and marks it, unless the harness takes this kind bare.
    pub fn framed_for(&self, harness: &dyn Harness) -> String {
        let mut text = self.body.clone();
        if harness.framing(self.kind) == Framing::Bare {
            return text;
        }
        for attachment in &self.attachments {
            if let Some(words) = attachment.text(harness) {
                text.push_str("\n\n");
                text.push_str(&harness.wrap_attachment(&words));
            }
        }
        text
    }
}

/// The message a queued turn's text makes on `harness`: a provider command
/// the harness must receive as typed, or a delivery.
pub fn turn_message(harness: &dyn Harness, text: String) -> ComposedMessage {
    let kind = if harness.requires_unadorned_command(&text) {
        MessageKind::ProviderCommand
    } else {
        MessageKind::Delivery
    };
    ComposedMessage::new(kind, text)
}

/// What `harness` is handed for a queued turn's `text`, a message of `kind`
/// carrying what `asks` attaches.
pub fn turn_text(
    harness: &dyn Harness,
    text: String,
    kind: MessageKind,
    asks: TurnAttachments,
) -> String {
    ComposedMessage::new(kind, text)
        .with(asks)
        .framed_for(harness)
}

/// `text` inside a `<system-reminder>`, the tag Claude Code gives the context
/// it adds to a user turn.
pub fn system_reminder(text: &str) -> String {
    MESSAGE_SYSTEM_REMINDER.replace("{text}", text)
}

/// `text` inside a block marked as Build's instructions.
pub fn build_instructions(text: &str) -> String {
    MESSAGE_BUILD_INSTRUCTIONS.replace("{text}", text)
}

#[cfg(test)]
mod tests;
