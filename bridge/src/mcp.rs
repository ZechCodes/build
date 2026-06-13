//! The Build MCP server — the one way an agent talks *to* Build.
//!
//! Spawned per session over stdio (`build-bridge mcp --task <id>`), task identity
//! baked into the transport: no shared server, no auth, no ambiguity. It exposes
//! exactly one tool in v1, `done`, by which an agent reports the outcome of a
//! phase. Everything here is hand-rolled newline-delimited JSON-RPC 2.0 — the MCP
//! stdio framing — so the surface stays minimal and the parsing stays testable.

use std::io::{BufRead, Write};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The protocol version this server advertises when a client omits one.
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

/// Which phase the agent is reporting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DonePhase {
    Plan,
    Build,
    Revise,
}

/// The agent's claim about how the phase ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DoneStatus {
    /// The phase objective is met (a claim, not a verdict — the human gate decides).
    Completed,
    /// The agent cannot proceed and is saying why.
    Blocked,
    /// The agent tried and asserts the approach did not work.
    Failed,
}

/// Structured outputs a phase can report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoneOutputs {
    /// Required when `phase=plan` and `status=completed`: where the plan was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_path: Option<String>,
}

/// The raw `done` arguments as they arrive over the wire, before validation.
#[derive(Debug, Deserialize)]
struct DoneArgs {
    phase: DonePhase,
    status: DoneStatus,
    summary: String,
    #[serde(default)]
    outputs: DoneOutputs,
}

/// A validated `done` report — the typed completion event the lifecycle consumes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoneReport {
    pub phase: DonePhase,
    pub status: DoneStatus,
    pub summary: String,
    pub outputs: DoneOutputs,
}

/// Why a `done` call was rejected (a tool-level error the agent can correct).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DoneError {
    #[error("plan_path is required when phase=plan and status=completed")]
    MissingPlanPath,
}

impl DoneReport {
    fn from_args(args: DoneArgs) -> Result<DoneReport, DoneError> {
        if args.phase == DonePhase::Plan
            && args.status == DoneStatus::Completed
            && args.outputs.plan_path.is_none()
        {
            return Err(DoneError::MissingPlanPath);
        }
        Ok(DoneReport {
            phase: args.phase,
            status: args.status,
            summary: args.summary,
            outputs: args.outputs,
        })
    }
}

/// The result of handling one JSON-RPC message: a line to write back (absent for
/// notifications) and a `done` report to forward to the daemon, if one was made.
#[derive(Debug, Default)]
pub struct Handled {
    /// The JSON-RPC reply to write to stdout, or `None` for a notification.
    pub reply: Option<String>,
    /// A validated `done` the lifecycle should consume, if this message was one.
    pub report: Option<DoneReport>,
}

/// The single-tool MCP server. Identity-scoped to one task.
pub struct DoneServer {
    task_id: String,
}

impl DoneServer {
    pub fn new(task_id: impl Into<String>) -> Self {
        DoneServer {
            task_id: task_id.into(),
        }
    }

    /// The JSON Schema for the `done` tool's arguments.
    fn done_input_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "phase": { "type": "string", "enum": ["plan", "build", "revise"] },
                "status": { "type": "string", "enum": ["completed", "blocked", "failed"] },
                "summary": { "type": "string", "description": "A short markdown summary for the human reviewer. Lead with a one-line outcome, then a few '- ' bullet points of the key changes — or, if blocked/failed, what is needed to proceed. Use markdown: bullets, **bold**, and `backticks` for paths and commands. Prefer scannable bullets over one long paragraph." },
                "outputs": {
                    "type": "object",
                    "properties": {
                        "plan_path": { "type": "string", "description": "Required when phase=plan and status=completed." }
                    }
                }
            },
            "required": ["phase", "status", "summary"]
        })
    }

    /// Handle one newline-delimited JSON-RPC message.
    pub fn handle_message(&self, line: &str) -> Handled {
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                return Handled {
                    reply: Some(error(Value::Null, -32700, "parse error")),
                    report: None,
                }
            }
        };

        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");

        // No id → a notification: act, never reply.
        let Some(id) = id else {
            return Handled::default();
        };

        match method {
            "initialize" => {
                let version = msg
                    .get("params")
                    .and_then(|p| p.get("protocolVersion"))
                    .and_then(Value::as_str)
                    .unwrap_or(DEFAULT_PROTOCOL_VERSION)
                    .to_string();
                Handled {
                    reply: Some(result(
                        id,
                        json!({
                            "protocolVersion": version,
                            "capabilities": { "tools": {} },
                            "serverInfo": { "name": format!("build-bridge[{}]", self.task_id), "version": env!("CARGO_PKG_VERSION") }
                        }),
                    )),
                    report: None,
                }
            }
            "tools/list" => Handled {
                reply: Some(result(
                    id,
                    json!({
                        "tools": [{
                            "name": "done",
                            "description": "Report the outcome of the current phase. Call with status=completed when the objective is met, status=blocked if you cannot proceed, or status=failed if the approach did not work.",
                            "inputSchema": Self::done_input_schema()
                        }]
                    }),
                )),
                report: None,
            },
            "tools/call" => self.handle_tools_call(id, msg.get("params")),
            _ => Handled {
                reply: Some(error(id, -32601, "method not found")),
                report: None,
            },
        }
    }

    fn handle_tools_call(&self, id: Value, params: Option<&Value>) -> Handled {
        let name = params
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if name != "done" {
            return Handled {
                reply: Some(tool_error(id, format!("unknown tool: {name}"))),
                report: None,
            };
        }

        let arguments = params
            .and_then(|p| p.get("arguments"))
            .cloned()
            .unwrap_or(Value::Null);
        let args: DoneArgs = match serde_json::from_value(arguments) {
            Ok(a) => a,
            Err(e) => {
                return Handled {
                    reply: Some(tool_error(id, format!("invalid done arguments: {e}"))),
                    report: None,
                }
            }
        };

        match DoneReport::from_args(args) {
            Ok(report) => Handled {
                reply: Some(tool_ok(id, &report.summary)),
                report: Some(report),
            },
            Err(e) => Handled {
                reply: Some(tool_error(id, e.to_string())),
                report: None,
            },
        }
    }

    /// Run the server over real stdio, forwarding each `done` to `on_report`.
    /// Blocks until stdin reaches EOF.
    pub fn run_stdio(
        &self,
        input: impl BufRead,
        mut output: impl Write,
        mut on_report: impl FnMut(DoneReport),
    ) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let handled = self.handle_message(&line);
            if let Some(reply) = handled.reply {
                output.write_all(reply.as_bytes())?;
                output.write_all(b"\n")?;
                output.flush()?;
            }
            if let Some(report) = handled.report {
                on_report(report);
            }
        }
        Ok(())
    }
}

/// A JSON-RPC success response line.
fn result(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

/// A JSON-RPC error response line (protocol-level failure).
fn error(id: Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// An MCP tool result reporting success (`isError: false`).
fn tool_ok(id: Value, text: &str) -> String {
    result(
        id,
        json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
    )
}

/// An MCP tool result reporting a tool-level error the agent can correct.
fn tool_error(id: Value, text: String) -> String {
    result(
        id,
        json!({ "content": [{ "type": "text", "text": text }], "isError": true }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> DoneServer {
        DoneServer::new("task-123")
    }

    fn parse(reply: &str) -> Value {
        serde_json::from_str(reply).expect("reply is valid JSON")
    }

    #[test]
    fn initialize_reports_capabilities_and_echoes_version() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
        );
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["id"], 1);
        assert_eq!(v["result"]["protocolVersion"], "2024-11-05");
        assert!(v["result"]["capabilities"]["tools"].is_object());
        assert!(v["result"]["serverInfo"]["name"]
            .as_str()
            .unwrap()
            .contains("task-123"));
        assert!(h.report.is_none());
    }

    #[test]
    fn tools_list_exposes_only_done() {
        let h = server().handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let v = parse(&h.reply.unwrap());
        let tools = v["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "done");
        assert!(tools[0]["inputSchema"]["properties"]["phase"].is_object());
    }

    #[test]
    fn summary_schema_asks_for_markdown_bullets() {
        let h = server().handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let v = parse(&h.reply.unwrap());
        let desc = v["result"]["tools"][0]["inputSchema"]["properties"]["summary"]["description"]
            .as_str()
            .unwrap()
            .to_lowercase();
        assert!(desc.contains("markdown"), "summary should ask for markdown");
        assert!(desc.contains("bullet"), "summary should ask for bullets");
    }

    #[test]
    fn done_plan_completed_with_plan_path_emits_report() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"done","arguments":{"phase":"plan","status":"completed","summary":"plan ready","outputs":{"plan_path":".build/plan.md"}}}}"#,
        );
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], false);
        let report = h.report.expect("a report should be emitted");
        assert_eq!(report.phase, DonePhase::Plan);
        assert_eq!(report.status, DoneStatus::Completed);
        assert_eq!(report.summary, "plan ready");
        assert_eq!(report.outputs.plan_path.as_deref(), Some(".build/plan.md"));
    }

    #[test]
    fn done_plan_completed_without_plan_path_is_rejected() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"done","arguments":{"phase":"plan","status":"completed","summary":"oops"}}}"#,
        );
        let v = parse(&h.reply.unwrap());
        assert_eq!(
            v["result"]["isError"], true,
            "missing plan_path is a tool error"
        );
        assert!(h.report.is_none(), "no report when validation fails");
    }

    #[test]
    fn done_build_blocked_needs_no_plan_path() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"done","arguments":{"phase":"build","status":"blocked","summary":"missing credentials"}}}"#,
        );
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], false);
        let report = h.report.unwrap();
        assert_eq!(report.phase, DonePhase::Build);
        assert_eq!(report.status, DoneStatus::Blocked);
    }

    #[test]
    fn notification_initialized_yields_no_reply() {
        let h =
            server().handle_message(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        assert!(h.reply.is_none());
        assert!(h.report.is_none());
    }

    #[test]
    fn unknown_method_is_a_protocol_error() {
        let h = server().handle_message(r#"{"jsonrpc":"2.0","id":6,"method":"resources/list"}"#);
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["error"]["code"], -32601);
        assert!(h.report.is_none());
    }

    #[test]
    fn unknown_tool_is_a_tool_error() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"ask_user","arguments":{}}}"#,
        );
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], true);
        assert!(h.report.is_none());
    }

    #[test]
    fn malformed_json_is_a_parse_error() {
        let h = server().handle_message("{not json");
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["error"]["code"], -32700);
    }

    #[test]
    fn run_stdio_forwards_reports_and_skips_blank_lines() {
        let input = concat!(
            "\n",
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"done","arguments":{"phase":"revise","status":"completed","summary":"addressed comments"}}}"#,
            "\n",
        );
        let mut out = Vec::new();
        let mut reports = Vec::new();
        server()
            .run_stdio(input.as_bytes(), &mut out, |r| reports.push(r))
            .unwrap();

        // One report (the done), and two response lines (initialize + tools/call;
        // the notification produces none).
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].phase, DonePhase::Revise);
        let lines: Vec<&str> = std::str::from_utf8(&out).unwrap().lines().collect();
        assert_eq!(lines.len(), 2);
    }
}
