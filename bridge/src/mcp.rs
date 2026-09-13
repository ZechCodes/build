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
    /// A review-prioritization pass over the diff a build/revise just reported.
    /// It gates nothing — see [`crate::run::TriageReport`].
    Triage,
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
    /// Required when phase=triage and status=completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage: Option<crate::run::TriageReport>,
    /// Required when phase=recover and status=completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<RecoveryReport>,
    /// Optional on phase=revise/completed: per-comment resolutions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment_resolutions: Option<Vec<CommentResolution>>,
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
    #[error("outputs.triage is required when phase=triage and status=completed")]
    MissingTriageReport,
    #[error("outputs.triage classifies hunk {0:?} more than once")]
    DuplicateTriageHunk(String),
    #[error(
        "outputs.triage names hunks that are not in this diff: {}. The hunk ids of the current diff are: {}",
        unknown.join(", "),
        valid.join(", ")
    )]
    UnknownTriageHunks {
        unknown: Vec<String>,
        valid: Vec<String>,
    },
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
        if args.phase == DonePhase::Triage && args.status == DoneStatus::Completed {
            let Some(triage) = &args.outputs.triage else {
                return Err(DoneError::MissingTriageReport);
            };
            // A hunk classified twice has no classification. The ids are
            // checked against the actual diff where the diff exists — see
            // [`check_triage_hunk_ids`].
            let mut seen = std::collections::HashSet::new();
            for hunk in &triage.hunks {
                if !seen.insert(hunk.hunk_id.as_str()) {
                    return Err(DoneError::DuplicateTriageHunk(hunk.hunk_id.clone()));
                }
            }
        }
        Ok(DoneReport {
            phase: args.phase,
            status: args.status,
            summary: args.summary,
            outputs: args.outputs,
        })
    }
}

/// Check a triage report's hunk ids against the diff it claims to describe.
///
/// The tool boundary can only check the report's shape; the vocabulary of hunk
/// ids belongs to a patch, which lives where the worktree is. So the daemon
/// calls this with `crate::diff::hunk_ids` of the run's current diff, and an
/// invented or mistyped id comes back to the agent WITH the ids it could have
/// used — a correctable tool error, not a silent drop.
pub fn check_triage_hunk_ids(
    report: &crate::run::TriageReport,
    valid_ids: &[String],
) -> Result<(), DoneError> {
    let valid: std::collections::HashSet<&str> = valid_ids.iter().map(String::as_str).collect();
    let unknown: Vec<String> = report
        .hunks
        .iter()
        .map(|hunk| hunk.hunk_id.clone())
        .filter(|id| !valid.contains(id.as_str()))
        .collect();
    if unknown.is_empty() {
        return Ok(());
    }
    Err(DoneError::UnknownTriageHunks {
        unknown,
        valid: valid_ids.to_vec(),
    })
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
    ReadOperationMessages {
        operation_id: String,
    },
    PostThreadMessage {
        /// Whether the agent keeps working after this post (a progress note)
        /// rather than handing the turn back. See the tool description.
        still_working: bool,
        body: String,
        anchor: Option<crate::thread::MessageAnchor>,
        links: Vec<crate::thread::ThreadLink>,
        /// Actions suggested beside the message, already numbered.
        options: Vec<crate::thread::MessageOption>,
    },
    /// Ask the agent's own conversation history a question. Read-only, and
    /// scoped by the daemon to what this agent may read.
    SearchConversation {
        query: crate::thread::ConversationQuery,
    },
    /// Name what this conversation is about: a 2-4 word objective the header
    /// wears in place of the harness name. Already normalized — trimmed, one
    /// space between words — by the tool boundary.
    SetTopic {
        topic: String,
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
    /// Ask the user the one question that unblocks a routing decision, with up
    /// to three concrete choices offered beside it. Router only.
    AskUser {
        question: String,
        options: Vec<crate::capture::CaptureOptionDraft>,
    },
}

/// The choices an `ask_user` call offered beside its question. Absent reads as
/// none — a question with no options is the question this surface started with.
/// Whether the offer is a legal one is Build's to say, not this parser's.
fn ask_options(arguments: &Value) -> Result<Vec<crate::capture::CaptureOptionDraft>, String> {
    match arguments.get("options") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(options) => {
            serde_json::from_value(options.clone()).map_err(|error| format!("options: {error}"))
        }
    }
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
            BridgeAction::ReadOperationMessages { .. } => "read_unread_messages",
            BridgeAction::PostThreadMessage { .. } => "post_thread_message",
            BridgeAction::SearchConversation { .. } => "search_conversation",
            BridgeAction::SetTopic { .. } => "set_topic",
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
            | BridgeAction::ReadOperationMessages { .. }
            | BridgeAction::PostThreadMessage { .. }
            | BridgeAction::SearchConversation { .. }
            | BridgeAction::SetTopic { .. } => McpSurface::Coding,
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
            "description": "File an issue on a project: the capture becomes its goal and a planning agent starts on the primary checkout to work out how it should be done. No branch, no worktree, no code touched. This is the default destination — a wrong guess costs the user one tap, and rerouting takes the issue and its agent back.",
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
                    "branch": { "type": "string", "description": "The existing branch the work continues on, spelled exactly as it is — it is used as given, prefix and all. Omit only when the capture is new branch work whose name comes from the instruction." },
                    "instruction": { "type": "string", "description": "What the agent should do, in the user's terms." },
                    "rationale": { "type": "string", "description": "One line on why this branch is the destination." }
                },
                "required": ["project_id", "instruction"]
            }
        }, {
            "name": "ask_user",
            "description": "Ask the user the ONE question that would let you decide, and stop. Reserved for a capture whose project is ambiguous — asking is the friction capture exists to remove, so a best-guess inert issue is nearly always better. The question reaches them as the capture's own inbox entry. Offer up to 3 options when you can name the destinations you are choosing between: each is one tap for the user, and the answer comes back naming the one they picked. They can always type an answer instead, so options are a shortcut and never the whole answer.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "question": { "type": "string" },
                    "options": {
                        "type": "array",
                        "maxItems": 3,
                        "description": "Up to 3 concrete choices, in the order the user should see them. Omit entirely when the question has no obvious candidate answers.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "label": { "type": "string", "description": "What the user taps, in a few words: the destination, not the question again. e.g. \"File as an issue on Build\"." },
                                "project_id": { "type": "string", "description": "The project this choice routes to, from list_projects." },
                                "kind": { "type": "string", "enum": ["issue", "branch"], "description": "What this choice would create: an inert issue, or a branch with an agent on it." },
                                "branch": { "type": "string", "description": "The existing branch this choice continues, spelled exactly as it is. Naming one makes the choice a branch." }
                            },
                            "required": ["label"]
                        }
                    }
                },
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
                "phase": { "type": "string", "enum": ["plan", "build", "revise", "validate", "triage", "recover"] },
                "status": { "type": "string", "enum": ["completed", "blocked", "failed"] },
                "summary": { "type": "string", "description": SUMMARY_DESCRIPTION },
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
                        "triage": {
                            "type": "object",
                            "description": "Required when phase=triage and status=completed. One entry per hunk id the triage prompt listed, and no others.",
                            "properties": {
                                "based_on": { "type": "string", "description": "The revision the triage prompt named, echoed back exactly." },
                                "hunks": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "hunk_id": { "type": "string", "description": "A hunk id from the prompt's list, verbatim." },
                                            "level": { "type": "string", "enum": ["critical", "normal", "low"], "description": "How much review this hunk needs." },
                                            "rationale": { "type": "string", "description": "One line. For a low hunk it is what the reviewer reads INSTEAD of the hunk; for a critical one, what to look at." },
                                            "group": { "type": "string", "description": "Required on low hunks: the short name several collapsed hunks share." }
                                        },
                                        "required": ["hunk_id", "level"]
                                    }
                                }
                            },
                            "required": ["based_on", "hunks"]
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
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "operation_id": { "type": "string", "description": "A durable thread.post operation to read exactly and idempotently." }
                                }
                            }
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
                                    "options": {
                                        "type": "array",
                                        "maxItems": crate::thread::MAX_MESSAGE_OPTIONS,
                                        "description": "Actions to suggest the reviewer take in answer to this message, shown as pressable chips under it. Offer them when the reply you need is a choice you can enumerate, not when it is prose. The reviewer may pick several and submits once; what comes back is an ordinary reviewer message. Anything said afterwards closes the offer, so the chips are only ever answered while they are the newest thing on the thread.",
                                        "items": {
                                            "type": "object",
                                            "properties": {
                                                "label": { "type": "string", "description": "What the chip says. Short — a few words." },
                                                "message": { "type": "string", "description": "What you are told when it is chosen, in place of the label. Write the full instruction here, so the choice still carries its context in a session that no longer remembers this message." }
                                            },
                                            "required": ["label"]
                                        }
                                    },
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
                        }, {
                            "name": "set_topic",
                            "description": SET_TOPIC_DESCRIPTION,
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "topic": { "type": "string", "description": "The objective, in 2-4 words. Title-case the first word, no trailing period. Examples: \"Unify prompt delivery\", \"Fix login redirect\"." }
                                },
                                "required": ["topic"]
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
            let operation_id = params
                .and_then(|params| params.get("arguments"))
                .and_then(|arguments| arguments.get("operation_id"));
            let action = match operation_id {
                None | Some(Value::Null) => BridgeAction::ReadUnreadMessages,
                Some(Value::String(operation_id))
                    if !operation_id.is_empty()
                        && operation_id.len() <= 128
                        && operation_id.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric()
                                || matches!(byte, b'-' | b'_' | b'.' | b':')
                        }) =>
                {
                    BridgeAction::ReadOperationMessages {
                        operation_id: operation_id.clone(),
                    }
                }
                Some(_) => {
                    return Handled {
                        reply: Some(tool_error(
                            id,
                            "operation_id must be a valid non-empty operation key".to_string(),
                        )),
                        ..Handled::default()
                    };
                }
            };
            return Handled {
                action: Some(action),
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
        if name == "set_topic" {
            let topic = params
                .and_then(|p| p.get("arguments"))
                .and_then(|arguments| arguments.get("topic"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            return match normalized_topic(topic) {
                Ok(topic) => Handled {
                    action: Some(BridgeAction::SetTopic { topic }),
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
            let options = match arguments.get("options") {
                None | Some(Value::Null) => Vec::new(),
                Some(value) => {
                    let drafts: Vec<crate::thread::MessageOptionDraft> =
                        match serde_json::from_value(value.clone()) {
                            Ok(drafts) => drafts,
                            Err(error) => {
                                return Handled {
                                    reply: Some(tool_error(
                                        id,
                                        format!("invalid options: {error}"),
                                    )),
                                    ..Handled::default()
                                }
                            }
                        };
                    match crate::thread::numbered_message_options(&drafts) {
                        Ok(options) => options,
                        Err(error) => {
                            return Handled {
                                reply: Some(tool_error(id, error)),
                                ..Handled::default()
                            }
                        }
                    }
                }
            };
            return Handled {
                action: Some(BridgeAction::PostThreadMessage {
                    body: body.to_string(),
                    anchor,
                    links,
                    options,
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
            "ask_user" => required("question").and_then(|question| {
                Ok(BridgeAction::AskUser {
                    question,
                    options: ask_options(&arguments)?,
                })
            }),
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
/// What `done` asks for in `summary`: the whole report, because it is the
/// one thing the reviewer reads. There used to be a structured
/// `completion_report` beside a one-sentence summary; the card it drew was
/// noise under a sentence too short to stand alone, and the detail lived in
/// the activity log nobody should have to open. Now the summary carries it.
const SUMMARY_DESCRIPTION: &str = "The full report of this phase, in markdown, written for a reviewer who will not open the activity log. Lead with the outcome in one sentence, then say what changed and where (the files that carry it and why), how you verified it and what you could not, the decisions a reviewer would otherwise have to reverse-engineer, and what you deliberately left out or that remains at risk. Leave a heading out rather than pad it. If blocked or failed, lead with what is needed instead.";

/// What `set_topic` says about itself on every `tools/list`. The cold prompt
/// asks for the call; this is what is still in context when the agent makes
/// it, so it carries the shape rule itself.
const SET_TOPIC_DESCRIPTION: &str = "Name what this conversation is about, in 2-4 words: the objective you are setting out to achieve, not the steps. The conversation header shows it in place of the harness name, and says \"Starting\" until you call this. Call it first thing in a new conversation, and again if the objective changes.";

/// The most bytes a topic may carry after normalization. Four words leave
/// room under this; it is the guard against one enormous \"word\".
const MAX_TOPIC_BYTES: usize = 80;

/// The topic a `set_topic` call names, normalized to one space between words,
/// or why the call is refused. Two to four words, because the header wears it
/// where a harness name used to fit.
fn normalized_topic(raw: &str) -> Result<String, String> {
    let words: Vec<&str> = raw.split_whitespace().collect();
    if !(2..=4).contains(&words.len()) {
        return Err(format!(
            "topic must be an objective in 2-4 words, got {} word(s): {:?}",
            words.len(),
            raw.trim()
        ));
    }
    let topic = words.join(" ");
    if topic.len() > MAX_TOPIC_BYTES {
        return Err(format!(
            "topic must be at most {MAX_TOPIC_BYTES} bytes, got {}",
            topic.len()
        ));
    }
    Ok(topic)
}

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
        assert_eq!(tools.len(), 5);
        assert_eq!(tools[0]["name"], "read_unread_messages");
        assert_eq!(tools[1]["name"], "post_thread_message");
        assert_eq!(tools[2]["name"], "done");
        assert_eq!(tools[3]["name"], "search_conversation");
        assert_eq!(tools[4]["name"], "set_topic");
        assert!(tools[2]["inputSchema"]["properties"]["phase"].is_object());
    }

    /// The topic is the agent's own word for what the conversation is about,
    /// and the header wears it in place of the harness name — so it has to
    /// be a short objective, not a sentence. Two to four words is the shape;
    /// anything else comes back as a correctable tool error naming the rule,
    /// never a silently truncated heading.
    #[test]
    fn set_topic_takes_a_two_to_four_word_objective_and_refuses_the_rest() {
        let call = |topic: &str| {
            server().handle_message(&format!(
                r#"{{"jsonrpc":"2.0","id":40,"method":"tools/call","params":{{"name":"set_topic","arguments":{{"topic":{}}}}}}}"#,
                serde_json::to_string(topic).unwrap()
            ))
        };
        let h = call("  Unify   prompt delivery ");
        assert!(
            h.reply.is_none(),
            "an accepted call is an action, not a reply"
        );
        assert_eq!(
            h.action,
            Some(BridgeAction::SetTopic {
                topic: "Unify prompt delivery".to_string()
            }),
            "trimmed, and the spaces between words folded to one"
        );
        assert_eq!(
            call("Add topic tool now").action,
            Some(BridgeAction::SetTopic {
                topic: "Add topic tool now".to_string()
            })
        );

        for (topic, why) in [
            ("", "empty"),
            ("Refactor", "one word"),
            ("Make the parser handle five", "five words"),
        ] {
            let h = call(topic);
            assert!(h.action.is_none(), "{why}: {topic:?}");
            let reply = parse(&h.reply.expect(why));
            let text = reply["result"]["content"][0]["text"].as_str().unwrap();
            assert!(
                reply["result"]["isError"] == true && text.contains("2") && text.contains("4"),
                "{why}: the refusal names the rule: {text}"
            );
        }
        let missing = server().handle_message(
            r#"{"jsonrpc":"2.0","id":41,"method":"tools/call","params":{"name":"set_topic","arguments":{}}}"#,
        );
        assert!(missing.action.is_none());
        assert_eq!(parse(&missing.reply.unwrap())["result"]["isError"], true);
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

    /// The summary IS the report: the one thing the reviewer reads, so the
    /// schema asks for the whole of it rather than a sentence with a card
    /// of lists under it. And it is the only place a report is asked for.
    #[test]
    fn summary_schema_asks_for_the_full_report_and_nothing_else_does() {
        let h = server().handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let v = parse(&h.reply.unwrap());
        let schema = &v["result"]["tools"][2]["inputSchema"];
        let desc = schema["properties"]["summary"]["description"]
            .as_str()
            .unwrap()
            .to_lowercase();
        assert!(desc.contains("full report"), "{desc}");
        assert!(desc.contains("markdown"), "{desc}");
        for asked in [
            "what changed and where",
            "verified",
            "reverse-engineer",
            "left out",
        ] {
            assert!(desc.contains(asked), "{asked}: {desc}");
        }
        assert!(desc.contains("activity log"), "{desc}");
        assert!(
            schema["properties"]["outputs"]["properties"]["completion_report"].is_null(),
            "no structured report beside the summary: {schema}"
        );
    }

    /// An agent on an older prompt still sends the structured report. It is
    /// ignored rather than refused: the `done` it rides is a real outcome.
    #[test]
    fn a_done_still_carrying_a_completion_report_is_accepted_without_it() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":25,"method":"tools/call","params":{"name":"done","arguments":{"phase":"revise","status":"completed","summary":"addressed the notes","outputs":{"completion_report":{"decisions":["kept the old name"]}}}}}"#,
        );
        let report = h.report.expect("the outcome is kept");
        assert_eq!(report.summary, "addressed the notes");
        assert_eq!(
            serde_json::to_value(&report.outputs).unwrap(),
            serde_json::json!({}),
            "nothing of the old report survives onto the record"
        );
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
        assert!(phases.iter().any(|p| p == "triage"));
        let outputs = &schema["properties"]["outputs"]["properties"];
        assert!(outputs["stages"].is_object());
        assert!(outputs["validation"].is_object());
        assert!(outputs["comment_resolutions"].is_object());
        let triage = &outputs["triage"];
        assert_eq!(triage["type"], "object");
        let levels = triage["properties"]["hunks"]["items"]["properties"]["level"]["enum"]
            .as_array()
            .expect("the level vocabulary is closed in the schema too");
        assert_eq!(levels, &vec!["critical", "normal", "low"]);
        assert_eq!(
            triage["required"].as_array().unwrap(),
            &vec!["based_on", "hunks"]
        );
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
    fn post_thread_message_numbers_the_actions_it_suggests() {
        let post = server().handle_message(
            r#"{"jsonrpc":"2.0","id":25,"method":"tools/call","params":{"name":"post_thread_message","arguments":{"body":"Which way?","options":[{"label":"Revert it","message":"Revert the commit that turned the tests red."},{"label":"  Fix forward  "}]}}}"#,
        );

        let Some(BridgeAction::PostThreadMessage { options, .. }) = post.action else {
            panic!("the suggestion rides the post");
        };
        assert_eq!(
            options,
            vec![
                crate::thread::MessageOption {
                    id: "option-1".to_string(),
                    label: "Revert it".to_string(),
                    message: Some("Revert the commit that turned the tests red.".to_string()),
                },
                crate::thread::MessageOption {
                    id: "option-2".to_string(),
                    label: "Fix forward".to_string(),
                    message: None,
                },
            ]
        );
        assert!(post.reply.is_none());
    }

    #[test]
    fn an_unchoosable_suggestion_is_refused_rather_than_posted() {
        for arguments in [
            r#"{"body":"Which way?","options":[{"label":"   "}]}"#,
            r#"{"body":"Which way?","options":[{"label":"a"},{"label":"b"},{"label":"c"},{"label":"d"},{"label":"e"},{"label":"f"},{"label":"g"}]}"#,
            r#"{"body":"Which way?","options":"revert it"}"#,
        ] {
            let post = server().handle_message(&format!(
                r#"{{"jsonrpc":"2.0","id":26,"method":"tools/call","params":{{"name":"post_thread_message","arguments":{arguments}}}}}"#,
            ));
            assert!(post.action.is_none(), "{arguments}");
            assert!(post.reply.is_some(), "{arguments}");
        }
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

    /// The typed triage report: the levels are a closed vocabulary, the
    /// optional per-hunk prose rides along, and `based_on` says which revision
    /// was read.
    #[test]
    fn done_triage_completed_carries_the_classified_hunks() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":40,"method":"tools/call","params":{"name":"done","arguments":{"phase":"triage","status":"completed","summary":"the crypto change carries the risk","outputs":{"triage":{"based_on":"rev-1","hunks":[{"hunk_id":"habc","level":"critical","rationale":"changes key derivation"},{"hunk_id":"hdef","level":"low","rationale":"version bump only","group":"version bumps"},{"hunk_id":"hghi","level":"normal"}]}}}}}"#,
        );
        assert_eq!(parse(&h.reply.unwrap())["result"]["isError"], false);
        let report = h.report.expect("a triage report is emitted");
        assert_eq!(report.phase, DonePhase::Triage);
        let triage = report.outputs.triage.expect("triage carried");
        assert_eq!(triage.based_on, "rev-1");
        assert_eq!(triage.hunks.len(), 3);
        assert_eq!(triage.hunks[0].level, crate::run::TriageLevel::Critical);
        assert_eq!(
            triage.hunks[1].group.as_deref(),
            Some("version bumps"),
            "a collapsed hunk names its group"
        );
        assert_eq!(triage.hunks[2].level, crate::run::TriageLevel::Normal);
        assert_eq!(triage.hunks[2].rationale, None);
    }

    #[test]
    fn done_triage_completed_without_a_triage_report_is_rejected() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":41,"method":"tools/call","params":{"name":"done","arguments":{"phase":"triage","status":"completed","summary":"triaged"}}}"#,
        );
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], true);
        assert_eq!(
            v["result"]["content"][0]["text"],
            DoneError::MissingTriageReport.to_string()
        );
        assert!(h.report.is_none());
    }

    #[test]
    fn a_level_outside_the_vocabulary_is_refused() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":42,"method":"tools/call","params":{"name":"done","arguments":{"phase":"triage","status":"completed","summary":"triaged","outputs":{"triage":{"based_on":"rev-1","hunks":[{"hunk_id":"habc","level":"urgent"}]}}}}}"#,
        );
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], true);
        let text = v["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("urgent"), "{text}");
        assert!(h.report.is_none());
    }

    /// A field the phase does not define is a misunderstanding of the contract,
    /// not a harmless extra: refuse it while the agent can still correct it.
    #[test]
    fn an_unknown_field_on_a_triage_hunk_is_refused() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":43,"method":"tools/call","params":{"name":"done","arguments":{"phase":"triage","status":"completed","summary":"triaged","outputs":{"triage":{"based_on":"rev-1","hunks":[{"hunk_id":"habc","level":"low","severity":"minor"}]}}}}}"#,
        );
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], true);
        assert!(v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("severity"));
        assert!(h.report.is_none());
    }

    #[test]
    fn one_hunk_classified_twice_is_refused() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":44,"method":"tools/call","params":{"name":"done","arguments":{"phase":"triage","status":"completed","summary":"triaged","outputs":{"triage":{"based_on":"rev-1","hunks":[{"hunk_id":"habc","level":"low"},{"hunk_id":"habc","level":"critical"}]}}}}}"#,
        );
        let v = parse(&h.reply.unwrap());
        assert_eq!(v["result"]["isError"], true);
        assert_eq!(
            v["result"]["content"][0]["text"],
            DoneError::DuplicateTriageHunk("habc".to_string()).to_string()
        );
        assert!(h.report.is_none());
    }

    #[test]
    fn done_triage_blocked_needs_no_triage_report() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":45,"method":"tools/call","params":{"name":"done","arguments":{"phase":"triage","status":"blocked","summary":"the worktree is gone"}}}"#,
        );
        assert_eq!(parse(&h.reply.unwrap())["result"]["isError"], false);
        assert!(h.report.is_some());
    }

    /// The id vocabulary belongs to the patch, so the check happens where the
    /// patch is — and the refusal hands back the ids the agent could have used.
    #[test]
    fn a_hunk_id_that_is_not_in_the_diff_is_refused_with_the_ones_that_are() {
        let report = crate::run::TriageReport {
            based_on: "rev-1".to_string(),
            hunks: vec![
                crate::run::TriageHunk {
                    hunk_id: "hpresent".to_string(),
                    level: crate::run::TriageLevel::Normal,
                    rationale: None,
                    group: None,
                },
                crate::run::TriageHunk {
                    hunk_id: "hinvented".to_string(),
                    level: crate::run::TriageLevel::Low,
                    rationale: Some("looks harmless".to_string()),
                    group: Some("noise".to_string()),
                },
            ],
            overrides: Vec::new(),
        };
        let valid = vec!["hpresent".to_string(), "hmissed".to_string()];
        let error = check_triage_hunk_ids(&report, &valid).expect_err("an invented id is refused");
        let message = error.to_string();
        assert!(message.contains("hinvented"), "{message}");
        assert!(
            message.contains("hpresent") && message.contains("hmissed"),
            "{message}"
        );

        // Classifying only some of the diff is allowed: the rest renders as it
        // does today, which is what an untriaged hunk means.
        let partial = crate::run::TriageReport {
            based_on: "rev-1".to_string(),
            hunks: vec![report.hunks[0].clone()],
            overrides: Vec::new(),
        };
        assert!(check_triage_hunk_ids(&partial, &valid).is_ok());
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
                "set_topic",
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
    #[allow(clippy::cognitive_complexity)] // ratchet: every_router_tool_emits_its_typed_action is at 17, threshold 15 — bring it under, then remove
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
            Some(BridgeAction::AskUser { ref question, ref options })
                if question == "which project?" && options.is_empty()
        ));
    }

    /// The options a router offers beside its question reach the daemon whole:
    /// what the user taps, and where that tap would send the capture.
    #[test]
    fn ask_user_carries_the_options_the_router_offered() {
        let handled = router().handle_message(
            r#"{"jsonrpc":"2.0","id":53,"method":"tools/call","params":{"name":"ask_user","arguments":{
                "question":"which project?",
                "options":[
                    {"label":"File as an issue on Build","project_id":"proj-1","kind":"issue"},
                    {"label":"New branch on Do","project_id":"proj-2","kind":"branch","branch":"do/login"}
                ]}}}"#,
        );
        let Some(BridgeAction::AskUser { question, options }) = handled.action else {
            panic!("expected an ask action: {:?}", handled.reply);
        };
        assert_eq!(question, "which project?");
        assert_eq!(
            options,
            vec![
                crate::capture::CaptureOptionDraft {
                    label: "File as an issue on Build".to_string(),
                    project_id: Some("proj-1".to_string()),
                    kind: Some(crate::capture::CaptureTarget::Issue),
                    branch: None,
                },
                crate::capture::CaptureOptionDraft {
                    label: "New branch on Do".to_string(),
                    project_id: Some("proj-2".to_string()),
                    kind: Some(crate::capture::CaptureTarget::Branch),
                    branch: Some("do/login".to_string()),
                },
            ]
        );
    }

    /// An offer the parser cannot read is a tool error the router can fix,
    /// never a question that reaches the user with a choice missing off it.
    #[test]
    fn an_option_the_parser_cannot_read_is_a_tool_error() {
        for arguments in [
            r#"{"question":"which?","options":[{"kind":"issue"}]}"#,
            r#"{"question":"which?","options":[{"label":"go","kind":"pull_request"}]}"#,
            r#"{"question":"which?","options":"the first one"}"#,
        ] {
            let handled = router().handle_message(&format!(
                r#"{{"jsonrpc":"2.0","id":54,"method":"tools/call","params":{{"name":"ask_user","arguments":{arguments}}}}}"#
            ));
            let reply = parse(&handled.reply.unwrap());
            assert_eq!(reply["result"]["isError"], true, "{arguments}");
            assert!(handled.action.is_none(), "{arguments}");
        }
    }

    /// The one shape that means "no options": a question with none offered is
    /// the question this surface started with.
    #[test]
    fn a_question_with_no_options_asks_the_same_way_it_always_did() {
        for arguments in [
            r#"{"question":"which?"}"#,
            r#"{"question":"which?","options":[]}"#,
            r#"{"question":"which?","options":null}"#,
        ] {
            let handled = router().handle_message(&format!(
                r#"{{"jsonrpc":"2.0","id":55,"method":"tools/call","params":{{"name":"ask_user","arguments":{arguments}}}}}"#
            ));
            assert!(
                matches!(handled.action, Some(BridgeAction::AskUser { ref options, .. }) if options.is_empty()),
                "{arguments}"
            );
        }
    }

    /// The tool the router reads has to say the offer is bounded and optional,
    /// or a router with four good ideas will send all four.
    #[test]
    fn the_ask_tool_states_the_shape_of_the_offer() {
        let tools = DoneServer::router_tools();
        let ask = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "ask_user")
            .expect("the router can ask");
        let options = &ask["inputSchema"]["properties"]["options"];
        assert_eq!(options["type"], "array");
        assert_eq!(options["maxItems"], crate::capture::MAX_CAPTURE_OPTIONS);
        assert_eq!(options["items"]["required"], json!(["label"]));
        assert_eq!(
            options["items"]["properties"]["kind"]["enum"],
            json!(["issue", "branch"])
        );
        assert_eq!(
            ask["inputSchema"]["required"],
            json!(["question"]),
            "a question with no options is still a question"
        );
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
                    options: Vec::new(),
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
