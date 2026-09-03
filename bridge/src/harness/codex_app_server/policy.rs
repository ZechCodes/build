use serde_json::json;

use super::protocol::{ServerRequest, ServerResponse};

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
}

pub struct ServerRequestPolicy;

impl ServerRequestPolicy {
    pub fn decide(request: ServerRequest, current_unix_seconds: i64) -> ServerRequestDecision {
        let id = request.id;
        match request.method.as_str() {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                decision(ServerResponse::result(id, json!({"decision":"decline"})))
            }
            "execCommandApproval" => decision(ServerResponse::result(
                id,
                json!({"decision":{"denied":{"rejection":"Build does not approve commands"}}}),
            )),
            "applyPatchApproval" => decision(ServerResponse::result(
                id,
                json!({"decision":{"denied":{"rejection":"Build does not approve file changes"}}}),
            )),
            "mcpServer/elicitation/request" => {
                decision(ServerResponse::result(id, json!({"action":"decline"})))
            }
            "currentTime/read" => decision(ServerResponse::result(
                id,
                json!({"currentTimeAt":current_unix_seconds}),
            )),
            "item/tool/requestUserInput" => fail_turn(
                id,
                "Codex requested user input, which this carrier does not support",
            ),
            "item/permissions/requestApproval" => fail_turn(
                id,
                "Codex requested permissions, which this carrier cannot grant",
            ),
            "item/tool/call" => fail_turn(id, "Codex dynamic tools are not supported"),
            "account/chatgptAuthTokens/refresh" => fail_session(
                id,
                "Codex authentication expired; run `codex login` and start the agent again",
            ),
            "attestation/generate" => fail_session(
                id,
                "Codex requested attestation that Build did not advertise",
            ),
            method => fail_session(id, format!("unsupported Codex server request {method}")),
        }
    }
}

fn decision(response: ServerResponse) -> ServerRequestDecision {
    ServerRequestDecision {
        response,
        after_response: AfterResponse::Continue,
    }
}

fn fail_turn(id: serde_json::Value, reason: impl Into<String>) -> ServerRequestDecision {
    ServerRequestDecision {
        response: ServerResponse::error(id, -32601, "method not supported"),
        after_response: AfterResponse::FailTurn(reason.into()),
    }
}

fn fail_session(id: serde_json::Value, reason: impl Into<String>) -> ServerRequestDecision {
    ServerRequestDecision {
        response: ServerResponse::error(id, -32601, "method not found"),
        after_response: AfterResponse::FailSession(reason.into()),
    }
}
