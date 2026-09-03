use super::protocol::{ServerRequest, ServerResponse};
use crate::harness::adk::{one_line, TOOL_SUMMARY_LIMIT};
use crate::harness::{ActivityReport, AgentActivity};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AfterResponse {
    Continue,
    FailTurn(String),
    FailSession(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerRequestDecision {
    pub response: ServerResponse,
    pub after_response: AfterResponse,
    pub report: Option<ActivityReport>,
}

pub struct ServerRequestPolicy;

impl ServerRequestPolicy {
    pub fn decide(request: ServerRequest, current_unix_seconds: i64) -> ServerRequestDecision {
        match request {
            ServerRequest::CommandApproval { id } | ServerRequest::FileApproval { id } => {
                report_decision(
                    ServerResponse::approval_declined(id),
                    "Codex approval request declined",
                )
            }
            ServerRequest::LegacyCommandApproval { id } => report_decision(
                ServerResponse::legacy_command_declined(id),
                "Codex approval request declined",
            ),
            ServerRequest::LegacyPatchApproval { id } => report_decision(
                ServerResponse::legacy_patch_declined(id),
                "Codex approval request declined",
            ),
            ServerRequest::Elicitation { id } => decision(ServerResponse::elicitation_declined(id)),
            ServerRequest::CurrentTime { id } => {
                decision(ServerResponse::current_time(id, current_unix_seconds))
            }
            ServerRequest::UserInput { id } => fail_turn(
                id,
                "Codex requested user input, which this carrier does not support",
            ),
            ServerRequest::Permissions { id } => fail_turn(
                id,
                "Codex requested permissions, which this carrier cannot grant",
            ),
            ServerRequest::DynamicTool { id } => {
                fail_turn(id, "Codex dynamic tools are not supported")
            }
            ServerRequest::RefreshAuth { id } => fail_session(
                id,
                "Codex authentication expired; run `codex login` and start the agent again",
            ),
            ServerRequest::Attestation { id } => fail_session(
                id,
                "Codex requested attestation that Build did not advertise",
            ),
            ServerRequest::Unknown { id, method } => {
                fail_session(id, format!("unsupported Codex server request {method}"))
            }
        }
    }
}

fn decision(response: ServerResponse) -> ServerRequestDecision {
    ServerRequestDecision {
        response,
        after_response: AfterResponse::Continue,
        report: None,
    }
}

fn report_decision(response: ServerResponse, summary: &str) -> ServerRequestDecision {
    ServerRequestDecision {
        response,
        after_response: AfterResponse::Continue,
        report: Some(ActivityReport::own_work(AgentActivity::TaskUpdate {
            summary: one_line(summary, TOOL_SUMMARY_LIMIT),
        })),
    }
}

fn fail_turn(id: serde_json::Value, reason: impl Into<String>) -> ServerRequestDecision {
    ServerRequestDecision {
        response: ServerResponse::error(id, -32601, "method not supported"),
        after_response: AfterResponse::FailTurn(reason.into()),
        report: None,
    }
}

fn fail_session(id: serde_json::Value, reason: impl Into<String>) -> ServerRequestDecision {
    ServerRequestDecision {
        response: ServerResponse::error(id, -32601, "method not found"),
        after_response: AfterResponse::FailSession(reason.into()),
        report: None,
    }
}
