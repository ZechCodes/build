use serde_json::json;

use super::protocol::{ServerRequest, ServerRequestKind, ServerResponse};
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
        let id = request.id;
        match request.kind {
            ServerRequestKind::CommandApproval | ServerRequestKind::FileApproval => {
                report_decision(
                    ServerResponse::result(id, json!({"decision":"decline"})),
                    "Codex approval request declined",
                )
            }
            ServerRequestKind::LegacyCommandApproval => report_decision(
                ServerResponse::result(
                    id,
                    json!({"decision":{"denied":{"rejection":"Build does not approve commands"}}}),
                ),
                "Codex approval request declined",
            ),
            ServerRequestKind::LegacyPatchApproval => report_decision(
                ServerResponse::result(
                    id,
                    json!({"decision":{"denied":{"rejection":"Build does not approve file changes"}}}),
                ),
                "Codex approval request declined",
            ),
            ServerRequestKind::Elicitation => {
                decision(ServerResponse::result(id, json!({"action":"decline"})))
            }
            ServerRequestKind::CurrentTime => decision(ServerResponse::result(
                id,
                json!({"currentTimeAt":current_unix_seconds}),
            )),
            ServerRequestKind::UserInput => fail_turn(
                id,
                "Codex requested user input, which this carrier does not support",
            ),
            ServerRequestKind::Permissions => fail_turn(
                id,
                "Codex requested permissions, which this carrier cannot grant",
            ),
            ServerRequestKind::DynamicTool => {
                fail_turn(id, "Codex dynamic tools are not supported")
            }
            ServerRequestKind::RefreshAuth => fail_session(
                id,
                "Codex authentication expired; run `codex login` and start the agent again",
            ),
            ServerRequestKind::Attestation => fail_session(
                id,
                "Codex requested attestation that Build did not advertise",
            ),
            ServerRequestKind::Unknown => fail_session(
                id,
                format!("unsupported Codex server request {}", request.method),
            ),
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
