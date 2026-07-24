//! The Build MCP server — the one way an agent talks *to* Build.
//!
//! Spawned per session over stdio (`build-bridge mcp --task <id>`), the owner id
//! baked into the transport: no shared server, no auth, no ambiguity. The
//! `--task` flag stays opaque across the plan/run split — the id is a plan id or
//! a run id, and the daemon routes each `done` report by owner lookup (plans
//! map, then runs map). It exposes scoped unread/reply tools plus `done`, by
//! which an agent reports the outcome of a phase. Everything here is hand-rolled
//! newline-delimited JSON-RPC 2.0 — the MCP stdio framing — so the surface stays
//! minimal and the parsing stays testable.

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
    /// An automated validation pass gating the next stage of a multi-stage plan.
    Validate,
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

/// One per-comment resolution from a stage plan-revision session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentResolution {
    pub comment_id: String,
    pub response: String,
}

/// Structured outputs a phase can report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoneOutputs {
    /// Required when `phase=plan` and `status=completed`: where the plan was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_path: Option<String>,
    /// Echo of `.build/plan/stages.json`. Presence of a non-empty array on
    /// phase=plan/completed marks the plan multi-stage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stages: Option<Vec<crate::plan::StageManifestEntry>>,
    /// Required when phase=validate and status=completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<crate::run::ValidationReport>,
    /// Optional on phase=revise/completed: per-comment resolutions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment_resolutions: Option<Vec<CommentResolution>>,
    /// Durable handoff context for reviewers and cold replacement sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_report: Option<crate::thread::CompletionReport>,
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
    #[error("outputs.validation is required when phase=validate and status=completed")]
    MissingValidationReport,
    #[error("invalid outputs.stages: {0}")]
    InvalidStages(String),
    #[error("plan_path {0:?} must be a plain relative path inside the worktree (no '..', no leading '/')")]
    PlanPathEscapesWorktree(String),
}

/// A stable kebab-case slug: lowercase alphanumerics in hyphen-separated runs,
/// no leading/trailing/doubled hyphens.
fn is_kebab_slug(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate.split('-').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// Validate a manifest echo per the spec's ordered rule list, naming the first
/// offense found (checked in manifest order, one rule at a time per entry).
fn validate_stages(entries: &[crate::plan::StageManifestEntry]) -> Result<(), String> {
    if entries.is_empty() {
        return Err("stages must be non-empty".to_string());
    }
    let mut seen_ids = std::collections::HashSet::new();
    for entry in entries {
        if !is_kebab_slug(&entry.id) {
            return Err(format!("id \"{}\" is not a kebab-case slug", entry.id));
        }
        if !seen_ids.insert(entry.id.clone()) {
            return Err(format!("duplicate id \"{}\"", entry.id));
        }
        if !entry.path.starts_with(".build/plan/") {
            return Err(format!(
                "path \"{}\" must start with .build/plan/",
                entry.path
            ));
        }
        // The prefix check alone accepts `.build/plan/../../..` — the joined
        // path must also be traversal-free so it can never leave the plan dir.
        if !crate::plan::is_worktree_contained_path(&entry.path) {
            return Err(format!(
                "path \"{}\" must not contain traversal segments",
                entry.path
            ));
        }
        if entry.title.trim().is_empty() {
            return Err(format!("stage \"{}\" has an empty title", entry.id));
        }
    }
    Ok(())
}

impl DoneReport {
    fn from_args(args: DoneArgs) -> Result<DoneReport, DoneError> {
        if args.phase == DonePhase::Plan
            && args.status == DoneStatus::Completed
            && args.outputs.plan_path.is_none()
        {
            return Err(DoneError::MissingPlanPath);
        }
        // A reported plan_path is later joined under the worktree and read back
        // over RPC, so an escaping path would exfiltrate arbitrary host files.
        if let Some(path) = &args.outputs.plan_path {
            if !crate::plan::is_worktree_contained_path(path) {
                return Err(DoneError::PlanPathEscapesWorktree(path.clone()));
            }
        }
        if args.phase == DonePhase::Plan && args.status == DoneStatus::Completed {
            if let Some(entries) = &args.outputs.stages {
                validate_stages(entries).map_err(DoneError::InvalidStages)?;
            }
        }
        if args.phase == DonePhase::Validate
            && args.status == DoneStatus::Completed
            && args.outputs.validation.is_none()
        {
            return Err(DoneError::MissingValidationReport);
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
    /// A thread operation the stdio adapter executes against the owning daemon
    /// entity before it can write the JSON-RPC reply.
    pub action: Option<BridgeAction>,
    action_id: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BridgeAction {
    ReadUnreadMessages,
    PostThreadMessage {
        body: String,
        anchor: Option<crate::thread::MessageAnchor>,
    },
}

/// The conversation-aware MCP server. Identity-scoped to one owner (a plan or a run);
/// `owner_id` is opaque here — the daemon disambiguates it by owner lookup.
pub struct DoneServer {
    owner_id: String,
}

impl DoneServer {
    pub fn new(owner_id: impl Into<String>) -> Self {
        DoneServer {
            owner_id: owner_id.into(),
        }
    }

    /// The JSON Schema for the `done` tool's arguments.
    fn done_input_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "phase": { "type": "string", "enum": ["plan", "build", "revise", "validate"] },
                "status": { "type": "string", "enum": ["completed", "blocked", "failed"] },
                "summary": { "type": "string", "description": "A short markdown summary for the human reviewer. Lead with a one-line outcome, then a few '- ' bullet points of the key changes — or, if blocked/failed, what is needed to proceed. Use markdown: bullets, **bold**, and `backticks` for paths and commands. Prefer scannable bullets over one long paragraph." },
                "outputs": {
                    "type": "object",
                    "properties": {
                        "plan_path": { "type": "string", "description": "Required when phase=plan and status=completed. For a multi-stage plan, the manifest path .build/plan/stages.json." },
                        "stages": {
                            "type": "array",
                            "description": "Echo of .build/plan/stages.json, in execution order. Required when phase=plan, status=completed and the plan is multi-stage.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "description": "Stable kebab-case slug; never changes across revisions." },
                                    "title": { "type": "string" },
                                    "path": { "type": "string", "description": "Worktree-relative, under .build/plan/." },
                                    "summary": { "type": "string" }
                                },
                                "required": ["id", "title", "path"]
                            }
                        },
                        "validation": {
                            "type": "object",
                            "description": "Required when phase=validate and status=completed.",
                            "properties": {
                                "passed": { "type": "boolean" },
                                "findings": { "type": "string", "description": "Markdown: what the diff did and did not satisfy from the stage doc." },
                                "notes_for_next_stage": { "type": "string", "description": "Markdown notes the next stage's builder should know. Empty string if none." }
                            },
                            "required": ["passed", "findings", "notes_for_next_stage"]
                        },
                        "comment_resolutions": {
                            "type": "array",
                            "description": "When phase=revise and the prompt listed [c-N] comment ids: one entry per comment saying how it was addressed.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "comment_id": { "type": "string" },
                                    "response": { "type": "string" }
                                },
                                "required": ["comment_id", "response"]
                            }
                        },
                        "completion_report": {
                            "type": "object",
                            "description": "Structured handoff for the reviewer and any cold replacement session.",
                            "properties": {
                                "critical_files": { "type": "array", "items": { "type": "string" } },
                                "risk_notes": { "type": "array", "items": { "type": "string" } },
                                "decisions": { "type": "array", "items": { "type": "string" } },
                                "skips": { "type": "array", "items": { "type": "string" } }
                            }
                        }
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
                    ..Handled::default()
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
                            "serverInfo": { "name": format!("build-bridge[{}]", self.owner_id), "version": env!("CARGO_PKG_VERSION") }
                        }),
                    )),
                    report: None,
                    ..Handled::default()
                }
            }
            "tools/list" => Handled {
                reply: Some(result(
                    id,
                    json!({
                        "tools": [{
                            "name": "read_unread_messages",
                            "description": "Read unread reviewer messages in your current Build conversation thread. Reading atomically marks them seen.",
                            "inputSchema": { "type": "object", "properties": {} }
                        }, {
                            "name": "post_thread_message",
                            // MUST agree with the "Build conversation protocol"
                            // block in `conversation_prompt` (orchestrator.rs),
                            // which is canonical — change both together.
                            //
                            // Deliberately self-contained rather than a pointer at
                            // that block: this description is re-sent on every
                            // tools/list and so outlives context compaction, which
                            // means it is the ONE statement guaranteed to still be
                            // in context when an ambiguous message actually arrives.
                            // A pointer would resolve to nothing exactly then.
                            "description": "Reply in the current Build conversation thread. Post only for a question, necessary pushback or clarification, an explicit request for a response, or a reviewer message that reads as either a question or a directive — for that last case post a one-line clarifying reply rather than silently changing code. Implementing an unambiguous directive needs no reply: the next revision is the acknowledgment. Do not post bare acknowledgments or diff recaps.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "body": { "type": "string" },
                                    "anchor": { "type": "object", "description": "Optional structured plan/diff anchor copied from the reviewer message." }
                                },
                                "required": ["body"]
                            }
                        }, {
                            "name": "done",
                            "description": "Report the outcome of the current phase. Call with status=completed when the objective is met, status=blocked if you cannot proceed, or status=failed if the approach did not work.",
                            "inputSchema": Self::done_input_schema()
                        }]
                    }),
                )),
                report: None,
                ..Handled::default()
            },
            "tools/call" => self.handle_tools_call(id, msg.get("params")),
            _ => Handled {
                reply: Some(error(id, -32601, "method not found")),
                report: None,
                ..Handled::default()
            },
        }
    }

    fn handle_tools_call(&self, id: Value, params: Option<&Value>) -> Handled {
        let name = params
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if name == "read_unread_messages" {
            return Handled {
                action: Some(BridgeAction::ReadUnreadMessages),
                action_id: Some(id),
                ..Handled::default()
            };
        }
        if name == "post_thread_message" {
            let arguments = params
                .and_then(|p| p.get("arguments"))
                .cloned()
                .unwrap_or(Value::Null);
            let body = arguments
                .get("body")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|body| !body.is_empty());
            let Some(body) = body else {
                return Handled {
                    reply: Some(tool_error(id, "body is required".to_string())),
                    ..Handled::default()
                };
            };
            let anchor = match arguments.get("anchor") {
                None | Some(Value::Null) => None,
                Some(value) => match serde_json::from_value(value.clone()) {
                    Ok(anchor) => Some(anchor),
                    Err(error) => {
                        return Handled {
                            reply: Some(tool_error(id, format!("invalid anchor: {error}"))),
                            ..Handled::default()
                        }
                    }
                },
            };
            return Handled {
                action: Some(BridgeAction::PostThreadMessage {
                    body: body.to_string(),
                    anchor,
                }),
                action_id: Some(id),
                ..Handled::default()
            };
        }
        if name != "done" {
            return Handled {
                reply: Some(tool_error(id, format!("unknown tool: {name}"))),
                report: None,
                ..Handled::default()
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
                    ..Handled::default()
                }
            }
        };

        match DoneReport::from_args(args) {
            Ok(report) => Handled {
                reply: Some(tool_ok(id, &report.summary)),
                report: Some(report),
                ..Handled::default()
            },
            Err(e) => Handled {
                reply: Some(tool_error(id, e.to_string())),
                report: None,
                ..Handled::default()
            },
        }
    }

    /// Run the server over real stdio, forwarding each `done` to `on_report`
    /// and each thread operation to the scoped daemon callback.
    /// Blocks until stdin reaches EOF.
    pub fn run_stdio(
        &self,
        input: impl BufRead,
        mut output: impl Write,
        mut on_report: impl FnMut(DoneReport),
        mut on_action: impl FnMut(BridgeAction) -> Result<Value, String>,
    ) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let handled = self.handle_message(&line);
            if let (Some(id), Some(action)) = (handled.action_id, handled.action) {
                let reply = match on_action(action) {
                    Ok(value) => tool_ok(id, &value.to_string()),
                    Err(message) => tool_error(id, message),
                };
                output.write_all(reply.as_bytes())?;
                output.write_all(b"\n")?;
                output.flush()?;
            }
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
    fn tools_list_exposes_thread_tools_and_done() {
        let h = server().handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let v = parse(&h.reply.unwrap());
        let tools = v["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0]["name"], "read_unread_messages");
        assert_eq!(tools[1]["name"], "post_thread_message");
        assert_eq!(tools[2]["name"], "done");
        assert!(tools[2]["inputSchema"]["properties"]["phase"].is_object());
    }

    /// This description outlives context compaction (it rides every
    /// `tools/list`) while the spawn prompt's protocol block does not — so it is
    /// the statement still in context when an ambiguous message lands, and it
    /// has to carry the rule rather than point at one.
    #[test]
    fn post_thread_message_description_carries_the_policy_it_must_survive_on() {
        let h = server().handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let v = parse(&h.reply.unwrap());
        let description = v["result"]["tools"][1]["description"].as_str().unwrap();
        let lowered = description.to_lowercase();
        assert!(
            lowered.contains("either a question or a directive")
                && lowered.contains("one-line clarifying reply"),
            "the ambiguity carve-out must be stated here, not deferred to a block \
             that compaction removes: {description}"
        );
        // The contradiction this replaced: a blanket ban on replying to anything
        // read as a directive, which suppresses the carve-out above. Assert the
        // MEANING is gated on ambiguity, not one former spelling of the ban.
        for banned in [
            "not acknowledge directives",
            "never acknowledge directives",
            "do not reply to directives",
        ] {
            assert!(
                !lowered.contains(banned),
                "an unqualified directive ban contradicts the ambiguity carve-out: {description}"
            );
        }
    }

    #[test]
    fn summary_schema_asks_for_markdown_bullets() {
        let h = server().handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let v = parse(&h.reply.unwrap());
        let desc = v["result"]["tools"][2]["inputSchema"]["properties"]["summary"]["description"]
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
    fn tools_list_schema_enumerates_validate_phase_and_new_outputs() {
        let h = server().handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let v = parse(&h.reply.unwrap());
        let schema = &v["result"]["tools"][2]["inputSchema"];
        let phases = schema["properties"]["phase"]["enum"].as_array().unwrap();
        assert!(phases.iter().any(|p| p == "validate"));
        let outputs = &schema["properties"]["outputs"]["properties"];
        assert!(outputs["stages"].is_object());
        assert!(outputs["validation"].is_object());
        assert!(outputs["comment_resolutions"].is_object());
        assert!(outputs["completion_report"].is_object());
    }

    #[test]
    fn unread_and_reply_calls_emit_scoped_bridge_actions() {
        let read = server().handle_message(
            r#"{"jsonrpc":"2.0","id":21,"method":"tools/call","params":{"name":"read_unread_messages","arguments":{}}}"#,
        );
        assert!(matches!(
            read.action,
            Some(BridgeAction::ReadUnreadMessages)
        ));
        assert!(read.reply.is_none());

        let post = server().handle_message(
            r#"{"jsonrpc":"2.0","id":22,"method":"tools/call","params":{"name":"post_thread_message","arguments":{"body":"Which name should I use?"}}}"#,
        );
        assert!(matches!(
            post.action,
            Some(BridgeAction::PostThreadMessage { ref body, anchor: None })
                if body == "Which name should I use?"
        ));
        assert!(post.reply.is_none());
    }

    #[test]
    fn done_carries_the_structured_completion_report() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":23,"method":"tools/call","params":{"name":"done","arguments":{"phase":"build","status":"completed","summary":"done","outputs":{"completion_report":{"critical_files":["src/main.rs"],"risk_notes":["migration"],"decisions":["kept API"],"skips":["load test"]}}}}}"#,
        );
        let report = h.report.unwrap().outputs.completion_report.unwrap();
        assert_eq!(report.critical_files, vec!["src/main.rs"]);
        assert_eq!(report.skips, vec!["load test"]);
    }

    #[test]
    fn done_validate_completed_without_validation_is_rejected() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"done","arguments":{"phase":"validate","status":"completed","summary":"looks good"}}}"#,
        );
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], true);
        assert_eq!(
            v["result"]["content"][0]["text"],
            DoneError::MissingValidationReport.to_string()
        );
        assert!(h.report.is_none());
    }

    #[test]
    fn done_validate_completed_with_validation_emits_report() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"done","arguments":{"phase":"validate","status":"completed","summary":"pass","outputs":{"validation":{"passed":true,"findings":"all good","notes_for_next_stage":"none"}}}}}"#,
        );
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], false);
        let report = h.report.expect("a report should be emitted");
        assert_eq!(report.phase, DonePhase::Validate);
        let validation = report.outputs.validation.expect("validation carried");
        assert!(validation.passed);
        assert_eq!(validation.findings, "all good");
        assert_eq!(validation.notes_for_next_stage, "none");
    }

    #[test]
    fn done_validate_blocked_needs_no_validation_report() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"done","arguments":{"phase":"validate","status":"blocked","summary":"cannot run tests"}}}"#,
        );
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], false);
        assert!(h.report.is_some());
    }

    fn plan_done_message(id: i64, stages_json: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"done","arguments":{{"phase":"plan","status":"completed","summary":"plan ready","outputs":{{"plan_path":".build/plan/stages.json","stages":{stages_json}}}}}}}}}"#
        )
    }

    #[test]
    fn done_plan_completed_with_good_manifest_echo_carries_entries() {
        let stages = r#"[{"id":"database-schema","title":"Database schema","path":".build/plan/01-database-schema.md","summary":"Create the tables."},{"id":"api-endpoints","title":"API endpoints","path":".build/plan/02-api-endpoints.md","summary":""}]"#;
        let h = server().handle_message(&plan_done_message(11, stages));
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], false);
        let report = h.report.expect("a report should be emitted");
        let entries = report.outputs.stages.expect("stages carried");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].id, "database-schema");
        assert_eq!(entries[1].summary, "");
    }

    #[test]
    fn done_plan_completed_with_duplicate_stage_ids_is_rejected() {
        let stages = r#"[{"id":"dup","title":"A","path":".build/plan/01-a.md"},{"id":"dup","title":"B","path":".build/plan/02-b.md"}]"#;
        let h = server().handle_message(&plan_done_message(12, stages));
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], true);
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("duplicate id"), "got: {text}");
        assert!(h.report.is_none());
    }

    #[test]
    fn done_plan_completed_with_bad_id_chars_is_rejected() {
        let stages = r#"[{"id":"Not_Kebab","title":"A","path":".build/plan/01-a.md"}]"#;
        let h = server().handle_message(&plan_done_message(13, stages));
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], true);
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("kebab-case"), "got: {text}");
        assert!(h.report.is_none());
    }

    #[test]
    fn done_plan_completed_with_path_outside_plan_dir_is_rejected() {
        let stages = r#"[{"id":"a","title":"A","path":"src/a.md"}]"#;
        let h = server().handle_message(&plan_done_message(14, stages));
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], true);
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains(".build/plan/"), "got: {text}");
        assert!(h.report.is_none());
    }

    #[test]
    fn done_plan_completed_with_traversal_stage_path_is_rejected() {
        // `.build/plan/../../..` satisfies a naive prefix check but escapes the
        // plan dir (and the worktree) when joined — it must be rejected.
        let stages =
            r#"[{"id":"a","title":"A","path":".build/plan/../../../../../../etc/passwd"}]"#;
        let h = server().handle_message(&plan_done_message(19, stages));
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], true);
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("traversal"), "got: {text}");
        assert!(h.report.is_none());
    }

    #[test]
    fn done_plan_completed_with_traversal_plan_path_is_rejected() {
        for bad_path in ["../../outside.md", "/etc/passwd"] {
            let message = format!(
                r#"{{"jsonrpc":"2.0","id":20,"method":"tools/call","params":{{"name":"done","arguments":{{"phase":"plan","status":"completed","summary":"plan ready","outputs":{{"plan_path":"{bad_path}"}}}}}}}}"#
            );
            let h = server().handle_message(&message);
            let v = parse(&h.reply.unwrap());
            assert_eq!(v["result"]["isError"], true, "path {bad_path:?} accepted");
            assert!(h.report.is_none());
        }
    }

    #[test]
    fn done_plan_completed_with_empty_title_is_rejected() {
        let stages = r#"[{"id":"a","title":"","path":".build/plan/01-a.md"}]"#;
        let h = server().handle_message(&plan_done_message(15, stages));
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], true);
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("empty title"), "got: {text}");
        assert!(h.report.is_none());
    }

    #[test]
    fn done_plan_completed_with_empty_stages_array_is_rejected() {
        let h = server().handle_message(&plan_done_message(16, "[]"));
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], true);
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("non-empty"), "got: {text}");
        assert!(h.report.is_none());
    }

    #[test]
    fn done_revise_completed_with_comment_resolutions_round_trips() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":17,"method":"tools/call","params":{"name":"done","arguments":{"phase":"revise","status":"completed","summary":"addressed comments","outputs":{"comment_resolutions":[{"comment_id":"c-1","response":"switched to a timestamp"}]}}}}"#,
        );
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], false);
        let report = h.report.expect("a report should be emitted");
        let resolutions = report
            .outputs
            .comment_resolutions
            .expect("resolutions carried");
        assert_eq!(resolutions.len(), 1);
        assert_eq!(resolutions[0].comment_id, "c-1");
        assert_eq!(resolutions[0].response, "switched to a timestamp");
    }

    #[test]
    fn done_build_completed_ignores_stray_stages_and_validation_outputs() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":18,"method":"tools/call","params":{"name":"done","arguments":{"phase":"build","status":"completed","summary":"done","outputs":{"stages":[],"validation":{"passed":true,"findings":"","notes_for_next_stage":""}}}}}"#,
        );
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], false);
        assert!(h.report.is_some());
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
            .run_stdio(
                input.as_bytes(),
                &mut out,
                |r| reports.push(r),
                |_| Err("no thread action expected".into()),
            )
            .unwrap();

        // One report (the done), and two response lines (initialize + tools/call;
        // the notification produces none).
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].phase, DonePhase::Revise);
        let lines: Vec<&str> = std::str::from_utf8(&out).unwrap().lines().collect();
        assert_eq!(lines.len(), 2);
    }
}
