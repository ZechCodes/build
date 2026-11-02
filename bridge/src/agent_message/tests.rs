use super::*;
use crate::harness::harness_for;
use crate::models::AgentProvider;

const PROVIDERS: [AgentProvider; 5] = [
    AgentProvider::ClaudeAdk,
    AgentProvider::Claude,
    AgentProvider::Codex,
    AgentProvider::CodexAppServer,
    AgentProvider::Pi,
];

fn with_reply_note(kind: MessageKind) -> ComposedMessage {
    let mut message = ComposedMessage::new(kind, "fix the parser");
    message
        .attachments
        .push("Answer with post_thread_message.".to_string());
    message
}

/// The exact text each harness is handed for a message carrying one
/// attachment: Claude Code's own tag where the harness is Claude Code, a
/// marked Build block elsewhere.
#[test]
fn each_harness_marks_an_attachment_its_own_way() {
    let reminder =
        "fix the parser\n\n<system-reminder>\nAnswer with post_thread_message.\n</system-reminder>";
    let build = "fix the parser\n\n<build_instructions>\nAnswer with post_thread_message.\n</build_instructions>";
    let expected = [
        (AgentProvider::ClaudeAdk, reminder),
        (AgentProvider::Claude, reminder),
        (AgentProvider::Codex, build),
        (AgentProvider::CodexAppServer, build),
        (AgentProvider::Pi, build),
    ];
    let message = with_reply_note(MessageKind::Delivery);
    for (provider, text) in expected {
        assert_eq!(
            message.framed_for(harness_for(provider)),
            text,
            "{provider:?}"
        );
    }
}

/// A command the provider parses reaches it as typed on every harness, even
/// when something tried to attach a note to it.
#[test]
fn bare_kinds_drop_their_attachments_everywhere() {
    for kind in [MessageKind::ProviderCommand, MessageKind::Compaction] {
        let message = with_reply_note(kind);
        for provider in PROVIDERS {
            assert_eq!(
                message.framed_for(harness_for(provider)),
                "fix the parser",
                "{provider:?} {kind:?}"
            );
        }
    }
}

/// A message with nothing attached is its body, untouched, on every harness
/// and for every kind: what lets the delivery path move onto the composer
/// without changing a byte.
#[test]
fn a_message_without_attachments_is_its_body() {
    let kinds = [
        MessageKind::Delivery,
        MessageKind::ThreadNudge,
        MessageKind::ColdStart,
        MessageKind::TaskNotice,
        MessageKind::TaskAssignment,
        MessageKind::DependencyNotice,
        MessageKind::TaskReminder,
        MessageKind::IdleWorkspaces,
        MessageKind::Restarted,
        MessageKind::UsageLimitResume,
        MessageKind::AgentFailure,
        MessageKind::ProviderRecoveryFailure,
        MessageKind::Compaction,
        MessageKind::ProviderCommand,
    ];
    for kind in kinds {
        let message = ComposedMessage::new(kind, "line one\nline two");
        for provider in PROVIDERS {
            assert_eq!(
                message.framed_for(harness_for(provider)),
                "line one\nline two"
            );
        }
    }
}
