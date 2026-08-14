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
    /// A branch-lineage recovery agent reporting a nonce-bound result.
    Recover,
    /// A router session reporting where it sent a capture. The only phase on
    /// the router surface, and never on a coding agent's.
    Route,
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

/// A recovery agent's claim. The daemon accepts it only for its currently
/// persisted recovery nonce, then independently verifies branch, HEAD and the
/// restored worktree before unblocking implementation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryReport {
    pub recovery_id: String,
    pub recovered: bool,
    pub branch: String,
    pub head_sha: String,
    #[serde(default)]
    pub findings: String,
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
    /// Required when phase=recover and status=completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<RecoveryReport>,
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
    #[error("outputs.recovery is required when phase=recover and status=completed")]
    MissingRecoveryReport,
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
        if args.phase == DonePhase::Recover
            && args.status == DoneStatus::Completed
            && args.outputs.recovery.is_none()
        {
            return Err(DoneError::MissingRecoveryReport);
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
        /// Whether the agent keeps working after this post (a progress note)
        /// rather than handing the turn back. See the tool description.
        still_working: bool,
        body: String,
        anchor: Option<crate::thread::MessageAnchor>,
        links: Vec<crate::thread::ThreadLink>,
    },
    /// Ask the agent's own conversation history a question. Read-only, and
    /// scoped by the daemon to what this agent may read.
    SearchConversation {
        query: crate::thread::ConversationQuery,
    },
    /// Every project on this device. Router only.
    ListProjects,
    /// The branches and issues in flight, as a digest. Router only.
    ListWork,
    /// One work item's conversation, read-only. Router only.
    ReadConversation {
        entity_id: String,
        agent_id: Option<String>,
        limit: usize,
    },
    /// File an inert issue: a record, no worktree, no agent. Router only.
    CreateIssue {
        project_id: String,
        goal: String,
        rationale: Option<String>,
    },
    /// Put an agent on a branch with an instruction. Router only.
    DispatchBranch {
        project_id: String,
        branch: Option<String>,
        instruction: String,
        rationale: Option<String>,
    },
    /// Ask the user the one question that unblocks a routing decision. Router
    /// only.
    AskUser {
        question: String,
    },
}

/// How many conversation items `read_conversation` returns when the router does
/// not say.
const DEFAULT_CONVERSATION_LIMIT: usize = 40;

/// The most it will return however much the router asks for: the router is
/// triaging, not reading the whole history of a branch.
const MAX_CONVERSATION_LIMIT: usize = 200;

impl BridgeAction {
    /// The tool this action came from, for errors that have to name it.
    pub fn tool_name(&self) -> &'static str {
        match self {
            BridgeAction::ReadUnreadMessages => "read_unread_messages",
            BridgeAction::PostThreadMessage { .. } => "post_thread_message",
            BridgeAction::SearchConversation { .. } => "search_conversation",
            BridgeAction::ListProjects => "list_projects",
            BridgeAction::ListWork => "list_work",
            BridgeAction::ReadConversation { .. } => "read_conversation",
            BridgeAction::CreateIssue { .. } => "create_issue",
            BridgeAction::DispatchBranch { .. } => "dispatch_branch",
            BridgeAction::AskUser { .. } => "ask_user",
        }
    }

    /// Which surface this action belongs to. The socket enforces it against the
    /// session that sent it, so a harness cannot reach the other surface's tools
    /// by writing the frame itself.
    pub fn surface(&self) -> McpSurface {
        match self {
            BridgeAction::ReadUnreadMessages
            | BridgeAction::PostThreadMessage { .. }
            | BridgeAction::SearchConversation { .. } => McpSurface::Coding,
            BridgeAction::ListProjects
            | BridgeAction::ListWork
            | BridgeAction::ReadConversation { .. }
            | BridgeAction::CreateIssue { .. }
            | BridgeAction::DispatchBranch { .. }
            | BridgeAction::AskUser { .. } => McpSurface::Router,
        }
    }
}

/// Which set of tools a session gets.
///
/// Not a permission flag on one server: two surfaces, and a session is on
/// exactly one of them for its whole life. A coding agent never sees the
/// router's tools and a router never sees a coding agent's, so neither can
/// reach the other's by asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpSurface {
    Coding,
    Router,
}

impl McpSurface {
    /// The surface an owner id names. The id itself carries the answer — a
    /// router session's id is prefixed — so the surface can never disagree with
    /// the session it was resolved for.
    pub fn for_owner(owner_id: &str) -> McpSurface {
        if crate::router::is_router_agent(owner_id) {
            McpSurface::Router
        } else {
            McpSurface::Coding
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            McpSurface::Coding => "coding",
            McpSurface::Router => "router",
        }
    }
}

/// The conversation-aware MCP server. Identity-scoped to one owner (a plan or a run);
/// `owner_id` is opaque here — the daemon disambiguates it by owner lookup.
pub struct DoneServer {
    owner_id: String,
    surface: McpSurface,
}

impl DoneServer {
    /// The coding-agent surface, for a session owned by a plan or a run.
    pub fn new(owner_id: impl Into<String>) -> Self {
        DoneServer {
            owner_id: owner_id.into(),
            surface: McpSurface::Coding,
        }
    }

    /// The surface `owner_id` names. The one constructor the stdio entry point
    /// uses, so which tools a session gets is decided by who it is rather than
    /// by a flag someone has to remember to pass.
    pub fn for_owner(owner_id: impl Into<String>) -> Self {
        let owner_id = owner_id.into();
        let surface = McpSurface::for_owner(&owner_id);
        DoneServer { owner_id, surface }
    }

    pub fn surface(&self) -> McpSurface {
        self.surface
    }

    /// The JSON Schema for the router's `done`. One phase, because a router has
    /// one: it routed, or it could not.
    fn router_done_input_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "phase": { "type": "string", "enum": ["route"] },
                "status": { "type": "string", "enum": ["completed", "failed"] },
                "summary": { "type": "string", "description": "One concise sentence saying where the capture went and why. If you could not route it, say what stopped you." }
            },
            "required": ["phase", "status", "summary"]
        })
    }

    /// The router's tools. Read broadly, write in exactly two places (an inert
    /// issue, a branch dispatch), and one way to ask the user something.
    fn router_tools() -> Value {
        json!([{
            "name": "list_projects",
            "description": "Every project on this device, with its id, name and repository path. Start here: a capture is routed to a project before it is routed to anything else.",
            "inputSchema": { "type": "object", "properties": {} }
        }, {
            "name": "list_work",
            "description": "The branches and issues in flight across every project: what each one is, which project it belongs to, its state, and whether an agent is working it right now. This is what you check the capture against before you believe it continues existing work.",
            "inputSchema": { "type": "object", "properties": {} }
        }, {
            "name": "read_conversation",
            "description": "Read one work item's conversation, newest last. Read-only — you cannot post to it. Use it to confirm a capture really continues the work on a branch before dispatching to it; a branch whose conversation is about something else is not the destination.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entity_id": { "type": "string", "description": "The run_id or issue_id from list_work." },
                    "agent_id": { "type": "string", "description": "Which agent's conversation, when the item has several. Omit for the item's own." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 40 }
                },
                "required": ["entity_id"]
            }
        }, {
            "name": "create_issue",
            "description": "File an inert issue on a project: a record with the capture as its goal, no worktree and no agent until the user opens it. This is the default destination — a wrong guess costs the user one tap, and they can reroute it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_id": { "type": "string" },
                    "goal": { "type": "string", "description": "What the user wants done, in their terms. Keep their words; do not turn a sentence into a specification." },
                    "rationale": { "type": "string", "description": "One line on why this project and why an issue. The user reads it when deciding whether you got it right." }
                },
                "required": ["project_id", "goal"]
            }
        }, {
            "name": "dispatch_branch",
            "description": "Put an agent on a branch with this instruction, creating or adopting the checkout as needed. Use ONLY when the capture names an existing branch or worktree, or unambiguously continues work already in flight on one — this starts an agent that changes code.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_id": { "type": "string" },
                    "branch": { "type": "string", "description": "The existing branch the work continues on. Omit only when the capture is new branch work whose name comes from the instruction." },
                    "instruction": { "type": "string", "description": "What the agent should do, in the user's terms." },
                    "rationale": { "type": "string", "description": "One line on why this branch is the destination." }
                },
                "required": ["project_id", "instruction"]
            }
        }, {
            "name": "ask_user",
            "description": "Ask the user the ONE question that would let you decide, and stop. Reserved for a capture whose project is ambiguous — asking is the friction capture exists to remove, so a best-guess inert issue is nearly always better. The question reaches them as the capture's own inbox entry.",
            "inputSchema": {
                "type": "object",
                "properties": { "question": { "type": "string" } },
                "required": ["question"]
            }
        }, {
            "name": "done",
            "description": "Report the routing outcome and end the session. Call it after create_issue, dispatch_branch or ask_user — or with status=failed when nothing let you decide.",
            "inputSchema": Self::router_done_input_schema()
        }])
    }

    /// The JSON Schema for the `done` tool's arguments.
    fn done_input_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "phase": { "type": "string", "enum": ["plan", "build", "revise", "validate", "recover"] },
                "status": { "type": "string", "enum": ["completed", "blocked", "failed"] },
                "summary": { "type": "string", "description": "One concise sentence stating what was completed. If blocked or failed, state what is needed instead. No file list, changelog, test log, links, or process narration." },
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
                        "recovery": {
                            "type": "object",
                            "description": "Required when phase=recover and status=completed. Echo the recovery_id nonce from the recovery prompt.",
                            "properties": {
                                "recovery_id": { "type": "string" },
                                "recovered": { "type": "boolean" },
                                "branch": { "type": "string" },
                                "head_sha": { "type": "string" },
                                "findings": { "type": "string" }
                            },
                            "required": ["recovery_id", "recovered", "branch", "head_sha", "findings"]
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
                            "description": "Expected when phase=build or phase=revise and status=completed: the handoff a reviewer reads before the diff, and the only durable context a replacement session gets. One short line per entry; leave a list out rather than padding it.",
                            "properties": {
                                "critical_files": { "type": "array", "items": { "type": "string" }, "description": "The few files that carry this change, each with why it matters — \"path — what it now does\"." },
                                "risk_notes": { "type": "array", "items": { "type": "string" }, "description": "What could break and where it would show, including anything you could not verify." },
                                "decisions": { "type": "array", "items": { "type": "string" }, "description": "Choices a reviewer would otherwise have to reverse-engineer, each with its reason." },
                                "skips": { "type": "array", "items": { "type": "string" }, "description": "What you deliberately did not do, and why." }
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
                            "serverInfo": { "name": format!("build-{}[{}]", self.surface.as_str(), self.owner_id), "version": env!("CARGO_PKG_VERSION") }
                        }),
                    )),
                    report: None,
                    ..Handled::default()
                }
            }
            "tools/list" if self.surface == McpSurface::Router => Handled {
                reply: Some(result(id, json!({ "tools": Self::router_tools() }))),
                ..Handled::default()
            },
            "tools/list" => Handled {
                reply: Some(result(
                    id,
                    json!({
                        "tools": [{
                            "name": "read_unread_messages",
                            "description": "Read unread reviewer messages in your current Build conversation thread. Reading atomically marks them seen, which starts the reviewer's \"Working\" indicator and its timer — post_thread_message stops it (see the `working` field on the result). A message may carry files under `attachments` — open every `path` it names before acting on that message.",
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
                            "description": "Reply in the current Build conversation thread. Post only for a question, necessary pushback or clarification, an explicit request for a response, or a reviewer message that reads as either a question or a directive — for that last case post a one-line clarifying reply rather than silently changing code. Implementing an unambiguous directive needs no reply: the next revision is the acknowledgment. Do not post bare acknowledgments or diff recaps. Posting hands the turn back to the reviewer; set still_working=true when you are only reporting progress and will keep going without waiting for an answer.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "body": { "type": "string" },
                                    "still_working": { "type": "boolean", "description": "True when this is a progress note and you are continuing without waiting for a reply. Omit (false) for an ordinary reply, which hands the turn back." },
                                    "anchor": { "type": "object", "description": "Optional structured plan/diff anchor copied from the reviewer message." },
                                    "links": {
                                        "type": "array",
                                        "maxItems": 20,
                                        "description": "Optional links to worktree files. Use kind=file with a worktree-relative path and optional line_start/line_end.",
                                        "items": {
                                            "type": "object",
                                            "properties": {
                                                "kind": { "type": "string", "enum": ["file"] },
                                                "path": { "type": "string" },
                                                "line_start": { "type": "integer", "minimum": 1 },
                                                "line_end": { "type": "integer", "minimum": 1 }
                                            },
                                            "required": ["kind", "path"]
                                        }
                                    }
                                },
                                "required": ["body"]
                            }
                        }, {
                            "name": "done",
                            "description": "Report the outcome of the current phase. Call with status=completed when the objective is met, status=blocked if you cannot proceed, or status=failed if the approach did not work.",
                            "inputSchema": Self::done_input_schema()
                        }, {
                            "name": "search_conversation",
                            // The one tool a session with no memory of the work
                            // needs to know exists, so the description says what
                            // to do INSTEAD of scrolling: ask a question.
                            "description": "Search your Build conversation history — every past message and event, including the ones from sessions before yours. Use it whenever you need context you do not have: what was decided about a file, why a commit was made, what the reviewer already asked for. Search rather than replay: never scroll the terminal or re-read the whole conversation to find something. Filters combine, results are newest first, and each hit is an excerpt with its sequence number, not the full item.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "query": { "type": "string", "description": "Case-insensitive text to look for in message bodies and event summaries." },
                                    "file": { "type": "string", "description": "A worktree-relative path, or the tail of one: finds items that referenced that file." },
                                    "commit": { "type": "string", "description": "A commit sha, short or full: finds items that referenced it." },
                                    "stage": { "type": "string", "description": "A stage id: finds items linked to that stage." },
                                    "role": { "type": "string", "enum": ["user", "agent", "event"], "description": "Only what the reviewer wrote, only what an agent wrote, or only what the bridge recorded." },
                                    "since_sequence": { "type": "integer", "minimum": 0, "description": "Only items after this sequence number — page back through a conversation you have already partly read." },
                                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }
                                }
                            }
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
        if self.surface == McpSurface::Router {
            return self.handle_router_tools_call(id, name, params);
        }
        if name == "read_unread_messages" {
            return Handled {
                action: Some(BridgeAction::ReadUnreadMessages),
                action_id: Some(id),
                ..Handled::default()
            };
        }
        if name == "search_conversation" {
            let arguments = params
                .and_then(|p| p.get("arguments"))
                .cloned()
                .unwrap_or(Value::Null);
            return match conversation_query(&arguments) {
                Ok(query) => Handled {
                    action: Some(BridgeAction::SearchConversation { query }),
                    action_id: Some(id),
                    ..Handled::default()
                },
                Err(message) => Handled {
                    reply: Some(tool_error(id, message)),
                    ..Handled::default()
                },
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
            let links = match arguments.get("links") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(values)) if values.len() <= 20 => {
                    match values
                        .iter()
                        .cloned()
                        .map(serde_json::from_value)
                        .collect::<Result<Vec<crate::thread::ThreadLink>, _>>()
                    {
                        Ok(links) => links,
                        Err(error) => {
                            return Handled {
                                reply: Some(tool_error(id, format!("invalid links: {error}"))),
                                ..Handled::default()
                            }
                        }
                    }
                }
                Some(Value::Array(_)) => {
                    return Handled {
                        reply: Some(tool_error(
                            id,
                            "links must contain at most 20 entries".to_string(),
                        )),
                        ..Handled::default()
                    }
                }
                Some(_) => {
                    return Handled {
                        reply: Some(tool_error(id, "links must be an array".to_string())),
                        ..Handled::default()
                    }
                }
            };
            return Handled {
                action: Some(BridgeAction::PostThreadMessage {
                    body: body.to_string(),
                    anchor,
                    links,
                    still_working: arguments
                        .get("still_working")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
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

    /// The router surface's `tools/call`. Every tool but `done` is a daemon
    /// action: the router asks Build, and Build — not this parser — decides
    /// whether the answer is allowed.
    fn handle_router_tools_call(&self, id: Value, name: &str, params: Option<&Value>) -> Handled {
        let arguments = params
            .and_then(|p| p.get("arguments"))
            .cloned()
            .unwrap_or(Value::Null);
        let text = |field: &str| {
            arguments
                .get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        };
        let required = |field: &'static str| text(field).ok_or(format!("{field} is required"));
        let action = match name {
            "list_projects" => Ok(BridgeAction::ListProjects),
            "list_work" => Ok(BridgeAction::ListWork),
            "read_conversation" => {
                required("entity_id").map(|entity_id| BridgeAction::ReadConversation {
                    entity_id,
                    agent_id: text("agent_id"),
                    limit: arguments
                        .get("limit")
                        .and_then(Value::as_u64)
                        .map(|limit| limit as usize)
                        .unwrap_or(DEFAULT_CONVERSATION_LIMIT)
                        .clamp(1, MAX_CONVERSATION_LIMIT),
                })
            }
            "create_issue" => required("project_id").and_then(|project_id| {
                Ok(BridgeAction::CreateIssue {
                    project_id,
                    goal: required("goal")?,
                    rationale: text("rationale"),
                })
            }),
            "dispatch_branch" => required("project_id").and_then(|project_id| {
                Ok(BridgeAction::DispatchBranch {
                    project_id,
                    branch: text("branch"),
                    instruction: required("instruction")?,
                    rationale: text("rationale"),
                })
            }),
            "ask_user" => required("question").map(|question| BridgeAction::AskUser { question }),
            "done" => {
                return match serde_json::from_value::<DoneArgs>(arguments)
                    .map_err(|error| format!("invalid done arguments: {error}"))
                    .and_then(|args| {
                        if args.phase == DonePhase::Route {
                            Ok(args)
                        } else {
                            Err("a router reports phase=\"route\"".to_string())
                        }
                    })
                    .and_then(|args| DoneReport::from_args(args).map_err(|e| e.to_string()))
                {
                    Ok(report) => Handled {
                        reply: Some(tool_ok(id, &report.summary)),
                        report: Some(report),
                        ..Handled::default()
                    },
                    Err(message) => Handled {
                        reply: Some(tool_error(id, message)),
                        ..Handled::default()
                    },
                }
            }
            other => Err(format!("unknown tool: {other}")),
        };
        match action {
            Ok(action) => Handled {
                action: Some(action),
                action_id: Some(id),
                ..Handled::default()
            },
            Err(message) => Handled {
                reply: Some(tool_error(id, message)),
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

/// Read a `search_conversation` call's arguments into the typed query the
/// daemon runs. A filter the daemon could not act on (an author nobody is) is
/// rejected here rather than silently returning nothing.
fn conversation_query(arguments: &Value) -> Result<crate::thread::ConversationQuery, String> {
    let text = |field: &str| {
        arguments
            .get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let role = text("role");
    if let Some(role) = &role {
        if !["user", "agent", crate::thread::EVENT_ROLE].contains(&role.as_str()) {
            return Err(format!(
                "role must be one of user, agent, {}",
                crate::thread::EVENT_ROLE
            ));
        }
    }
    Ok(crate::thread::ConversationQuery {
        text: text("query"),
        file: text("file"),
        commit: text("commit"),
        stage: text("stage"),
        role,
        since_sequence: arguments.get("since_sequence").and_then(Value::as_u64),
        limit: arguments
            .get("limit")
            .and_then(Value::as_u64)
            .map(|limit| limit as usize)
            .unwrap_or(crate::thread::DEFAULT_QUERY_LIMIT),
    })
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
        assert_eq!(tools.len(), 4);
        assert_eq!(tools[0]["name"], "read_unread_messages");
        assert_eq!(tools[1]["name"], "post_thread_message");
        assert_eq!(tools[2]["name"], "done");
        assert_eq!(tools[3]["name"], "search_conversation");
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
    fn summary_schema_asks_for_one_concise_outcome() {
        let h = server().handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let v = parse(&h.reply.unwrap());
        let desc = v["result"]["tools"][2]["inputSchema"]["properties"]["summary"]["description"]
            .as_str()
            .unwrap()
            .to_lowercase();
        assert!(desc.contains("one concise sentence"), "{desc}");
        assert!(desc.contains("no file list"), "{desc}");
        assert!(!desc.contains("bullet"), "{desc}");
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
    }

    #[test]
    fn tools_list_schema_asks_for_the_completion_report_on_build_and_revise() {
        let h = server().handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let v = parse(&h.reply.unwrap());
        let completion = &v["result"]["tools"][2]["inputSchema"]["properties"]["outputs"]
            ["properties"]["completion_report"];

        assert_eq!(completion["type"], "object");
        let described = completion["description"].as_str().unwrap();
        assert!(described.contains("build"), "{described}");
        assert!(described.contains("revise"), "{described}");
        for field in ["critical_files", "risk_notes", "decisions", "skips"] {
            assert_eq!(completion["properties"][field]["type"], "array", "{field}");
            assert_eq!(
                completion["properties"][field]["items"]["type"], "string",
                "{field}"
            );
            assert!(
                completion["properties"][field]["description"].is_string(),
                "{field} says what belongs in it"
            );
        }
    }

    #[test]
    fn a_partial_completion_report_leaves_the_other_lists_empty() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":25,"method":"tools/call","params":{"name":"done","arguments":{"phase":"revise","status":"completed","summary":"addressed the notes","outputs":{"completion_report":{"decisions":["kept the old name"]}}}}}"#,
        );
        let report = h.report.unwrap().outputs.completion_report.unwrap();
        assert_eq!(report.decisions, vec!["kept the old name"]);
        assert!(report.critical_files.is_empty());
        assert!(report.risk_notes.is_empty());
        assert!(report.skips.is_empty());
    }

    #[test]
    fn a_malformed_completion_report_is_a_tool_error_not_a_silent_drop() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":26,"method":"tools/call","params":{"name":"done","arguments":{"phase":"build","status":"completed","summary":"done","outputs":{"completion_report":{"critical_files":"src/main.rs"}}}}}"#,
        );
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], true);
        assert!(v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("invalid done arguments"));
        assert!(h.report.is_none());
    }

    #[test]
    fn tools_list_offers_search_and_tells_a_cold_session_to_use_it() {
        let h = server().handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let v = parse(&h.reply.unwrap());
        let tools = v["result"]["tools"].as_array().unwrap();
        let search = tools
            .iter()
            .find(|tool| tool["name"] == "search_conversation")
            .expect("the history query tool is advertised");

        let description = search["description"].as_str().unwrap().to_lowercase();
        // The whole reason the tool exists: a session that lost its context asks
        // a question instead of replaying the log.
        assert!(description.contains("search"), "{description}");
        assert!(
            description.contains("replay") || description.contains("re-read"),
            "the description must say what to do INSTEAD of replaying: {description}"
        );
        let properties = &search["inputSchema"]["properties"];
        for field in [
            "query",
            "file",
            "commit",
            "stage",
            "role",
            "since_sequence",
            "limit",
        ] {
            assert!(properties[field].is_object(), "{field} is missing");
        }
        assert_eq!(properties["limit"]["default"], 20);
        let roles = properties["role"]["enum"].as_array().unwrap();
        assert_eq!(roles, &vec![json!("user"), json!("agent"), json!("event")]);
    }

    #[test]
    fn a_search_call_emits_a_typed_query() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":30,"method":"tools/call","params":{"name":"search_conversation","arguments":{"query":"rename","file":"src/parser.rs","commit":"a1b2c3d","stage":"lexer","role":"agent","since_sequence":4,"limit":5}}}"#,
        );
        let Some(BridgeAction::SearchConversation { query }) = h.action else {
            panic!("expected a search action, got {:?}", h.action);
        };
        assert_eq!(query.text.as_deref(), Some("rename"));
        assert_eq!(query.file.as_deref(), Some("src/parser.rs"));
        assert_eq!(query.commit.as_deref(), Some("a1b2c3d"));
        assert_eq!(query.stage.as_deref(), Some("lexer"));
        assert_eq!(query.role.as_deref(), Some("agent"));
        assert_eq!(query.since_sequence, Some(4));
        assert_eq!(query.limit, 5);
        assert!(
            h.reply.is_none(),
            "the daemon answers a search, not the parser"
        );
    }

    #[test]
    fn a_search_with_no_arguments_is_the_recent_tail() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":31,"method":"tools/call","params":{"name":"search_conversation","arguments":{}}}"#,
        );
        let Some(BridgeAction::SearchConversation { query }) = h.action else {
            panic!("expected a search action");
        };
        assert_eq!(query, crate::thread::ConversationQuery::default());
        assert_eq!(query.limit, 20);
    }

    #[test]
    fn a_search_with_a_role_nobody_has_is_a_tool_error() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":32,"method":"tools/call","params":{"name":"search_conversation","arguments":{"role":"reviewer"}}}"#,
        );
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], true);
        assert!(v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("role"));
        assert!(h.action.is_none());
    }

    #[test]
    fn a_search_limit_is_clamped_rather_than_honoured_without_bound() {
        for (asked, applied) in [(0, 1), (5_000, 100)] {
            let h = server().handle_message(&format!(
                r#"{{"jsonrpc":"2.0","id":33,"method":"tools/call","params":{{"name":"search_conversation","arguments":{{"limit":{asked}}}}}}}"#
            ));
            let Some(BridgeAction::SearchConversation { query }) = h.action else {
                panic!("expected a search action");
            };
            assert_eq!(query.effective_limit(), applied, "limit={asked}");
        }
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
            Some(BridgeAction::PostThreadMessage { ref body, anchor: None, ref links, .. })
                if body == "Which name should I use?" && links.is_empty()
        ));
        assert!(post.reply.is_none());
    }

    #[test]
    fn post_thread_message_accepts_typed_file_links() {
        let post = server().handle_message(
            r#"{"jsonrpc":"2.0","id":24,"method":"tools/call","params":{"name":"post_thread_message","arguments":{"body":"See the parser.","links":[{"kind":"file","path":"src/parser.rs","line_start":12,"line_end":18}]}}}"#,
        );

        assert!(matches!(
            post.action,
            Some(BridgeAction::PostThreadMessage { ref links, .. })
                if links == &vec![crate::thread::ThreadLink::File {
                    path: "src/parser.rs".to_string(),
                    line_start: Some(12),
                    line_end: Some(18),
                }]
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
    fn done_recover_completed_requires_a_verified_recovery_report() {
        let missing = server().handle_message(
            r#"{"jsonrpc":"2.0","id":91,"method":"tools/call","params":{"name":"done","arguments":{"phase":"recover","status":"completed","summary":"recovered"}}}"#,
        );
        assert_eq!(parse(&missing.reply.unwrap())["result"]["isError"], true);
        assert!(missing.report.is_none());

        let complete = server().handle_message(
            r#"{"jsonrpc":"2.0","id":92,"method":"tools/call","params":{"name":"done","arguments":{"phase":"recover","status":"completed","summary":"recovered","outputs":{"recovery":{"recovery_id":"recovery-nonce","recovered":true,"branch":"build/fix","head_sha":"0123456789012345678901234567890123456789","findings":"branch restored"}}}}}"#,
        );
        let report = complete.report.expect("verified recovery report");
        assert_eq!(report.phase, DonePhase::Recover);
        assert_eq!(
            report.outputs.recovery.unwrap().recovery_id,
            "recovery-nonce"
        );
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

    // ==== the router surface ================================================

    fn router() -> DoneServer {
        DoneServer::for_owner("router-abc")
    }

    fn tool_names(server: &DoneServer) -> Vec<String> {
        let h = server.handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        parse(&h.reply.unwrap())["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect()
    }

    /// The id decides the surface, so a session cannot be handed the wrong tool
    /// set by anything forgetting to say which one it is.
    #[test]
    fn the_owner_id_decides_which_surface_a_session_is_on() {
        assert_eq!(
            DoneServer::for_owner("router-abc").surface(),
            McpSurface::Router
        );
        assert_eq!(
            DoneServer::for_owner("agent-01H").surface(),
            McpSurface::Coding
        );
        assert_eq!(DoneServer::new("router-abc").surface(), McpSurface::Coding);
    }

    /// Two surfaces, and nothing on both but `done`. A coding agent never sees
    /// a router tool and a router never sees a coding one.
    #[test]
    fn the_two_surfaces_share_only_done() {
        assert_eq!(
            tool_names(&router()),
            vec![
                "list_projects",
                "list_work",
                "read_conversation",
                "create_issue",
                "dispatch_branch",
                "ask_user",
                "done",
            ]
        );
        assert_eq!(
            tool_names(&server()),
            vec![
                "read_unread_messages",
                "post_thread_message",
                "done",
                "search_conversation",
            ],
            "the coding surface is unchanged by the router's arrival"
        );
    }

    /// The router's `done` reports the one phase a router has, so the schema
    /// cannot invite it to claim it built something.
    #[test]
    fn the_routers_done_reports_only_routing() {
        let h = router().handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let v = parse(&h.reply.unwrap());
        let schema = &v["result"]["tools"][6]["inputSchema"];
        assert_eq!(schema["properties"]["phase"]["enum"], json!(["route"]));
        assert_eq!(
            schema["properties"]["status"]["enum"],
            json!(["completed", "failed"])
        );

        let routed = router().handle_message(
            r#"{"jsonrpc":"2.0","id":40,"method":"tools/call","params":{"name":"done","arguments":{"phase":"route","status":"completed","summary":"filed an issue on the bridge"}}}"#,
        );
        let report = routed.report.expect("a routing report");
        assert_eq!(report.phase, DonePhase::Route);
        assert_eq!(report.summary, "filed an issue on the bridge");

        let wrong_phase = router().handle_message(
            r#"{"jsonrpc":"2.0","id":41,"method":"tools/call","params":{"name":"done","arguments":{"phase":"build","status":"completed","summary":"built it"}}}"#,
        );
        assert_eq!(
            parse(&wrong_phase.reply.unwrap())["result"]["isError"],
            true
        );
        assert!(wrong_phase.report.is_none());
    }

    /// The parser's whole job on this surface: turn a tool call into the typed
    /// action the daemon executes, and refuse one it cannot act on.
    #[test]
    fn every_router_tool_emits_its_typed_action() {
        let call = |name: &str, arguments: &str| {
            router().handle_message(&format!(
                r#"{{"jsonrpc":"2.0","id":50,"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#
            ))
        };

        assert!(matches!(
            call("list_projects", "{}").action,
            Some(BridgeAction::ListProjects)
        ));
        assert!(matches!(
            call("list_work", "{}").action,
            Some(BridgeAction::ListWork)
        ));
        assert!(matches!(
            call("read_conversation", r#"{"entity_id":"run-1","agent_id":"agent-2","limit":5}"#).action,
            Some(BridgeAction::ReadConversation { ref entity_id, ref agent_id, limit })
                if entity_id == "run-1" && agent_id.as_deref() == Some("agent-2") && limit == 5
        ));
        assert!(matches!(
            call("create_issue", r#"{"project_id":"proj-1","goal":"fix the redirect","rationale":"no branch names it"}"#).action,
            Some(BridgeAction::CreateIssue { ref project_id, ref goal, ref rationale })
                if project_id == "proj-1" && goal == "fix the redirect"
                    && rationale.as_deref() == Some("no branch names it")
        ));
        assert!(matches!(
            call("dispatch_branch", r#"{"project_id":"proj-1","branch":"build/login","instruction":"finish the toast"}"#).action,
            Some(BridgeAction::DispatchBranch { ref project_id, ref branch, ref instruction, rationale: None })
                if project_id == "proj-1" && branch.as_deref() == Some("build/login")
                    && instruction == "finish the toast"
        ));
        assert!(matches!(
            call("ask_user", r#"{"question":"which project?"}"#).action,
            Some(BridgeAction::AskUser { ref question }) if question == "which project?"
        ));
    }

    #[test]
    fn a_router_tool_call_missing_what_it_needs_is_a_tool_error() {
        for (name, arguments, wanted) in [
            ("read_conversation", "{}", "entity_id"),
            ("create_issue", r#"{"project_id":"proj-1"}"#, "goal"),
            ("dispatch_branch", r#"{"instruction":"go"}"#, "project_id"),
            ("ask_user", r#"{"question":"   "}"#, "question"),
        ] {
            let h = router().handle_message(&format!(
                r#"{{"jsonrpc":"2.0","id":51,"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#
            ));
            let v = parse(&h.reply.unwrap());
            assert_eq!(v["result"]["isError"], true, "{name}");
            assert!(
                v["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains(wanted),
                "{name}: {v}"
            );
            assert!(h.action.is_none(), "{name}");
        }
    }

    #[test]
    fn a_read_conversation_limit_is_clamped_rather_than_honoured_without_bound() {
        for (asked, applied) in [(0, 1), (5_000, 200)] {
            let h = router().handle_message(&format!(
                r#"{{"jsonrpc":"2.0","id":52,"method":"tools/call","params":{{"name":"read_conversation","arguments":{{"entity_id":"run-1","limit":{asked}}}}}}}"#
            ));
            let Some(BridgeAction::ReadConversation { limit, .. }) = h.action else {
                panic!("expected a read action");
            };
            assert_eq!(limit, applied, "limit={asked}");
        }
    }

    /// Neither surface can reach the other's tools by naming them.
    #[test]
    fn each_surface_refuses_the_others_tools() {
        let coding_asking_for_router = server().handle_message(
            r#"{"jsonrpc":"2.0","id":60,"method":"tools/call","params":{"name":"list_projects","arguments":{}}}"#,
        );
        assert_eq!(
            parse(&coding_asking_for_router.reply.unwrap())["result"]["isError"],
            true
        );
        assert!(coding_asking_for_router.action.is_none());

        let router_asking_for_coding = router().handle_message(
            r#"{"jsonrpc":"2.0","id":61,"method":"tools/call","params":{"name":"read_unread_messages","arguments":{}}}"#,
        );
        assert_eq!(
            parse(&router_asking_for_coding.reply.unwrap())["result"]["isError"],
            true
        );
        assert!(router_asking_for_coding.action.is_none());
    }

    /// The socket enforces the same split on the frames themselves, so it needs
    /// each action to say which surface it belongs to.
    #[test]
    fn every_action_names_its_tool_and_its_surface() {
        for (action, name, surface) in [
            (
                BridgeAction::ReadUnreadMessages,
                "read_unread_messages",
                McpSurface::Coding,
            ),
            (
                BridgeAction::ListProjects,
                "list_projects",
                McpSurface::Router,
            ),
            (BridgeAction::ListWork, "list_work", McpSurface::Router),
            (
                BridgeAction::AskUser {
                    question: "which?".to_string(),
                },
                "ask_user",
                McpSurface::Router,
            ),
        ] {
            assert_eq!(action.tool_name(), name);
            assert_eq!(action.surface(), surface, "{name}");
        }
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
