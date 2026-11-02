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
    ComposedMessage::new(kind, "fix the parser").with(TurnAttachments {
        replies: vec![ReplyChannel::Thread],
        tools: Vec::new(),
    })
}

/// The exact text each harness is handed for a message the user wrote in the
/// thread: the reply tool as that harness's model types it, and on the Claude
/// Code harnesses, which keep Build's tools behind ToolSearch, how to load it.
#[test]
fn each_harness_names_the_reply_tool_its_own_way() {
    let claude = "fix the parser\n\n\
<system-reminder>\nThe user wrote this in the thread. Reply there with `mcp__build__post_thread_message`.\n</system-reminder>\n\n\
<system-reminder>\nBuild tools load on demand: call ToolSearch with `select:mcp__build__post_thread_message` before you use them.\n</system-reminder>";
    let codex = "fix the parser\n\n\
<build_instructions>\nThe user wrote this in the thread. Reply there with `mcp__build__post_thread_message`.\n</build_instructions>";
    let pi = "fix the parser\n\n\
<build_instructions>\nThe user wrote this in the thread. Reply there with `post_thread_message`.\n</build_instructions>";
    let expected = [
        (AgentProvider::ClaudeAdk, claude),
        (AgentProvider::Claude, claude),
        (AgentProvider::Codex, codex),
        (AgentProvider::CodexAppServer, codex),
        (AgentProvider::Pi, pi),
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

/// A comment on a task is answered on that task, and another agent's words
/// are answered to that agent, each by its exact id.
#[test]
fn a_reply_names_the_task_or_agent_it_goes_back_to() {
    let message = ComposedMessage::new(MessageKind::Delivery, "body").with(TurnAttachments {
        replies: vec![
            ReplyChannel::Task {
                task_id: "task-42".to_string(),
            },
            ReplyChannel::Agent {
                agent_id: "agent-3".to_string(),
            },
        ],
        tools: Vec::new(),
    });
    assert_eq!(
        message.framed_for(harness_for(AgentProvider::Pi)),
        "body\n\n\
<build_instructions>\nThis is about task `task-42`. Reply on the task with `comment_task`.\n</build_instructions>\n\n\
<build_instructions>\nAgent `agent-3` sent this, not the user. Reply to it with `message_agent`.\n</build_instructions>"
    );
}

/// One tool hint for the whole turn: the reply tools first, then the tools
/// the words name, each once.
#[test]
fn the_tool_hint_names_each_tool_once_reply_tools_first() {
    let message = ComposedMessage::new(MessageKind::Delivery, "body").with(TurnAttachments {
        replies: vec![ReplyChannel::Task {
            task_id: "task-42".to_string(),
        }],
        tools: vec![BuildTool::ReadComment, BuildTool::CommentTask],
    });
    assert_eq!(
        message.attachments.last(),
        Some(&Attachment::ToolHint(vec![
            BuildTool::CommentTask,
            BuildTool::ReadComment
        ]))
    );
    assert!(message
        .framed_for(harness_for(AgentProvider::Claude))
        .ends_with(
            "call ToolSearch with `select:mcp__build__comment_task,mcp__build__read_comment` \
             before you use them.\n</system-reminder>"
        ));
}

/// Every attachment, on every harness, is a sentence or two: the turn's own
/// words stay the bulk of what the agent reads.
#[test]
fn every_attachment_is_a_sentence_or_two() {
    let attachments = [
        Attachment::Reply(ReplyChannel::Thread),
        Attachment::Reply(ReplyChannel::Task {
            task_id: "task-01M6EK32EA914912VZT05W8R8G".to_string(),
        }),
        Attachment::Reply(ReplyChannel::Agent {
            agent_id: "agent-01M6EN9NVJ8WS82KE6MCPNGA8B".to_string(),
        }),
        Attachment::ToolHint(vec![
            BuildTool::PostThreadMessage,
            BuildTool::GetTask,
            BuildTool::SetName,
        ]),
    ];
    for provider in PROVIDERS {
        for attachment in &attachments {
            let Some(text) = attachment.text(harness_for(provider)) else {
                continue;
            };
            let sentences = text.matches(". ").count() + usize::from(text.ends_with('.'));
            assert!(
                (1..=2).contains(&sentences) && !text.contains('\n'),
                "{provider:?}: {text}"
            );
        }
    }
}

/// The tool hint says nothing where Build's tools are already loaded.
#[test]
fn the_tool_hint_is_silent_where_tools_are_loaded() {
    let hint = Attachment::ToolHint(vec![BuildTool::GetTask]);
    for provider in [
        AgentProvider::Codex,
        AgentProvider::CodexAppServer,
        AgentProvider::Pi,
    ] {
        assert_eq!(hint.text(harness_for(provider)), None, "{provider:?}");
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

/// Which queued texts reach each harness as a command it parses, and so
/// travel bare: the commands its `requires_unadorned_command` names, and
/// nothing else. Codex and Pi take `/compact` inside the delivery today.
#[test]
fn a_turn_is_a_provider_command_only_where_the_harness_parses_it() {
    let expected = [
        (
            AgentProvider::ClaudeAdk,
            "/compact",
            MessageKind::ProviderCommand,
        ),
        (
            AgentProvider::Claude,
            "/compact",
            MessageKind::ProviderCommand,
        ),
        (
            AgentProvider::Claude,
            "/clear",
            MessageKind::ProviderCommand,
        ),
        (AgentProvider::Codex, "/clear", MessageKind::ProviderCommand),
        (AgentProvider::Codex, "/compact", MessageKind::Delivery),
        (
            AgentProvider::CodexAppServer,
            "/compact",
            MessageKind::ProviderCommand,
        ),
        (AgentProvider::Pi, "/compact", MessageKind::Delivery),
    ];
    for (provider, text, kind) in expected {
        assert_eq!(
            turn_message(harness_for(provider), text.to_string()).kind,
            kind,
            "{provider:?} {text}"
        );
    }
    for provider in PROVIDERS {
        assert_eq!(
            turn_message(harness_for(provider), "fix the parser".to_string()).kind,
            MessageKind::Delivery,
            "{provider:?}"
        );
    }
}
