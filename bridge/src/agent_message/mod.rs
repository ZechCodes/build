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
//! Attachments ride in the turn's text, after the body. No harness has a
//! per-turn system channel, and the app server's receipts match the echoed
//! text, so the text is the one place every carrier already handles.
//!
//! The wording lives in `bridge/templates/messages/`, beside the role prompts.

#[cfg(test)]
pub(crate) mod golden;

use crate::harness::Harness;
use crate::templates::without_final_newline;

/// A message template: its markdown file under `bridge/templates/messages/`,
/// compiled in.
macro_rules! message_template {
    ($file:literal) => {
        without_final_newline(include_str!(concat!("../../templates/messages/", $file)))
    };
}

/// An attachment marked the way Claude Code marks its own context.
const SYSTEM_REMINDER: &str = message_template!("wrap_system_reminder.md");

/// An attachment marked as Build's, on a harness with no convention of its own.
const BUILD_INSTRUCTIONS: &str = message_template!("wrap_build_instructions.md");

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

/// A message ready to be framed for whichever harness receives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedMessage {
    pub kind: MessageKind,
    pub body: String,
    /// Short instructions from Build, unwrapped; the harness marks them.
    pub attachments: Vec<String>,
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

    /// The text `harness` is handed: the body, then each attachment as the
    /// harness marks them, unless the harness takes this kind bare.
    pub fn framed_for(&self, harness: &dyn Harness) -> String {
        let mut text = self.body.clone();
        if harness.framing(self.kind) == Framing::Bare {
            return text;
        }
        for attachment in &self.attachments {
            text.push_str("\n\n");
            text.push_str(&harness.wrap_attachment(attachment));
        }
        text
    }
}

/// `text` inside a `<system-reminder>`, the tag Claude Code gives the context
/// it adds to a user turn.
pub fn system_reminder(text: &str) -> String {
    SYSTEM_REMINDER.replace("{text}", text)
}

/// `text` inside a block marked as Build's instructions.
pub fn build_instructions(text: &str) -> String {
    BUILD_INSTRUCTIONS.replace("{text}", text)
}

#[cfg(test)]
mod tests;
