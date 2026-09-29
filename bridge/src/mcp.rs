//! The Build MCP server — the one way an agent talks *to* Build.
//!
//! Spawned per session over stdio (`build-bridge mcp --task <id>`), the owner id
//! baked into the transport: no shared server, no auth, no ambiguity. The
//! `--task` flag stays opaque across the plan/run split — the id is a plan id or
//! a run id, and the daemon routes each completion report by owner lookup (plans
//! map, then runs map). It exposes one conversation tool whose terminal
//! statuses double as the completion report. Everything here is hand-rolled
//! newline-delimited JSON-RPC 2.0 — the MCP stdio framing — so the surface stays
//! minimal and the parsing stays testable.

use std::io::{BufRead, Write};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::renamed_ids::current_id;

/// The one sentence every body a person reads ends with (#229): how to link a
/// Build thing in it. The shapes are spa/src/core/markdownRefs.js's; the
/// full note with an example of each is templates/notes/link_markup.md.
macro_rules! reference_shapes_note {
    () => {
        " Link Build things by reference: `#42`, `#42/c/<comment-id>`, `@agent:<agent-id>`, `@workspace:<name or id>`, `@project:<name or id>`, `[[<workspace>:<path>#L10]]`, `[[<workspace>:commit:<sha>]]`."
    };
}
#[cfg(test)]
const REFERENCE_SHAPES_NOTE: &str = reference_shapes_note!();

/// The tools whose `body` a person reads, and so carries the note above.
#[cfg(test)]
const READABLE_BODIES: &[&str] = &[
    "post_thread_message",
    "comment_task",
    "create_task",
    "message_agent",
    "message_workspace_agent",
];

mod compaction;

/// The protocol version this server advertises when a client omits one.
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

/// The agent's claim about how its turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DoneStatus {
    /// The objective is met (a claim, not a verdict — the human gate decides).
    Completed,
    /// The agent cannot proceed and is saying why.
    Blocked,
    /// The agent tried and asserts the approach did not work.
    Failed,
}

/// Whether a message ends the current turn or describes an in-progress one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
enum MessageStatus {
    #[serde(alias = "complete")]
    Complete,
    #[serde(alias = "blocked")]
    Blocked,
    #[serde(alias = "waiting")]
    Waiting,
    #[serde(alias = "working")]
    Working,
}

impl MessageStatus {
    /// The report a terminal message makes, or `None` for a progress note or
    /// a question: only `Complete` and `Blocked` hand the turn back.
    fn report(self) -> Option<DoneStatus> {
        match self {
            MessageStatus::Complete => Some(DoneStatus::Completed),
            MessageStatus::Blocked => Some(DoneStatus::Blocked),
            MessageStatus::Waiting | MessageStatus::Working => None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct SendMessageArgs {
    status: MessageStatus,
    body: String,
}

/// A terminal message — the typed completion event the lifecycle consumes.
///
/// It carries no phase: the session that sent it already is one. A plan
/// session's report is a plan report, a run's is a run report, a router's is
/// a routing report — the daemon routes by owner, so the agent never has to
/// say which, and cannot say the wrong thing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoneReport {
    pub status: DoneStatus,
    pub summary: String,
    /// Exact visible message carrying this report, filled after a successful
    /// post. Internal correlation only; never part of the MCP wire shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

impl DoneReport {
    pub fn new(status: DoneStatus, summary: impl Into<String>) -> DoneReport {
        DoneReport {
            status,
            summary: summary.into(),
            message_id: None,
        }
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
    PostThreadMessage {
        /// Whether the agent keeps working after this post (a progress note)
        /// rather than handing the turn back. See the tool description.
        still_working: bool,
        body: String,
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
    /// Say what to CALL this agent: one or two meaningful words that replace
    /// "Agent 1" everywhere it is drawn. Already normalized — trimmed, one
    /// space between words — by the tool boundary; the daemon still has the
    /// last word on whether it is taken.
    SetName {
        name: String,
    },
    /// Compact this agent's own session once its current turn ends, keeping
    /// what `instructions` names. Every agent with a session.
    CompactSelf {
        instructions: String,
    },
    /// Compact another agent of this project's session: now between its
    /// turns, else when its current one ends. Project only.
    CompactAgent {
        agent_id: String,
        instructions: String,
    },
    /// Every harness this bridge can run, with its models and efforts, and
    /// which of them are installed here. The lookup an agent asked for a model
    /// by name reads.
    ListHarnesses,
    /// Every project on this device. Router only.
    ListProjects,
    /// The branches and tasks in flight, as a digest. Router only.
    ListWork,
    /// One work item's conversation, read-only. Router only.
    ReadConversation {
        entity_id: String,
        agent_id: Option<String>,
        limit: usize,
    },
    /// Put an agent on a branch with an instruction. Router only.
    DispatchBranch {
        project_id: String,
        branch: Option<String>,
        name: String,
        instruction: String,
        rationale: Option<String>,
    },
    /// Ask the user the one question that unblocks a routing decision, with up
    /// to three concrete choices offered beside it. Router only.
    AskUser {
        question: String,
        options: Vec<crate::capture::CaptureOptionDraft>,
    },
    /// A router status message shown on the capture itself.
    RouterMessage {
        body: String,
        waiting: bool,
    },
    /// The workspaces of the project this agent belongs to. Project only, and
    /// read-only: which project is asked about comes from the agent's owner.
    ListWorkspaces,
    /// The agents on one of this project's workspaces. Project only, and
    /// read-only.
    ListWorkspaceAgents {
        workspace_id: String,
    },
    /// Cut a workspace in this agent's project. Project only. There is no
    /// project field: which project it is cut in comes from the owner binding,
    /// so the agent cannot ask for one in a project it is not the agent of.
    CreateWorkspace {
        name: String,
        isolation: Option<String>,
    },
    /// Put an agent on one of this project's workspaces. Project only. The
    /// workspace is checked against the owner binding, so the only workspaces
    /// reachable are this project's.
    AddWorkspaceAgent {
        workspace_id: String,
        /// Make the new agent watched, so its conversation appears in the user's inbox.
        notify_user: Option<bool>,
        /// The harness the agent runs on; the account's default when absent.
        harness: Option<String>,
        model: Option<String>,
        effort: Option<String>,
        /// What to call it. Whoever cuts an agent for a piece of work can name
        /// that work better than the agent can before it has read anything.
        name: Option<String>,
        /// What the agent is to BE, and — when it matters — how much
        /// direction it should need. The user has chosen a model for each
        /// role, so asking for one is how a caller gets that choice.
        role: Option<String>,
        capability: Option<String>,
    },
    /// Take an agent off one of this project's workspaces. Project only.
    RemoveWorkspaceAgent {
        workspace_id: String,
        agent_id: String,
    },
    /// Say something to another agent of this project, by its id. On every
    /// surface that has a conversation, because every agent has to be able to
    /// answer the ones that write to it.
    MessageAgent {
        agent_id: String,
        body: String,
    },
    /// Say something to an agent on one of this project's workspaces. Project
    /// only. Naming no agent is that workspace's primary one. A thin alias for
    /// [`BridgeAction::MessageAgent`] addressed by workspace rather than by id.
    MessageWorkspaceAgent {
        workspace_id: String,
        agent_id: Option<String>,
        body: String,
    },
    /// Take one of this project's workspaces away. Project only.
    DeleteWorkspace {
        workspace_id: String,
    },
    /// One more folder on this agent's project. Project only. There is no
    /// project field: which project gains it comes from the owner binding.
    AddProjectSource {
        path: Option<String>,
        remote: Option<String>,
        name: Option<String>,
        base_branch: Option<String>,
    },
    /// Take a folder off this agent's project. Project only.
    RemoveProjectSource {
        source_id: String,
    },
    /// Remove a workspace whose work is somewhere else and whose tasks are
    /// finished (#135). Project only: what becomes of a quiet workspace is
    /// the project agent's call.
    ReclaimWorkspace {
        workspace_id: String,
    },
    /// One more directory in one of this project's workspaces. Project only.
    AddWorkspaceDirectory {
        workspace_id: String,
        source_id: Option<String>,
        path: Option<String>,
        remote: Option<String>,
        name: Option<String>,
    },
    /// Take a directory out of one of this project's workspaces. Project only.
    RemoveWorkspaceDirectory {
        workspace_id: String,
        directory_id: String,
    },

    // ---------------------------------------------------- task tracker ---
    // The per-project task tracker (spec: Tasks), on the coding and project
    // surfaces alike. Every one of them is `Tracker`-prefixed, the name the
    // tracker's actions have carried since the router's own `CreateTask` went
    // with the retired plan flow.
    //
    // None of these takes a project or an author. The scope is the calling
    // agent's own project, and the author is the calling agent; a call that
    // carries either anyway is parsed as though it had not.
    /// The tasks of this agent's project, narrowed.
    TrackerListTasks {
        state: Option<String>,
        status: Option<String>,
        label: Option<String>,
    },
    /// One task and its whole timeline.
    TrackerGetTask {
        task_id: String,
    },
    /// One comment, by the id a notice named. What a notice deliberately does
    /// not carry, for an agent that decides it cares.
    TrackerReadComment {
        comment_id: String,
    },
    /// File one.
    TrackerCreateTask {
        title: String,
        body: Option<String>,
        status: Option<String>,
        labels: Vec<String>,
        priority: Option<String>,
        /// Files to file WITH the task, as `{path, name?}` — the same shape a
        /// message carries them in, so an agent hands on what the user sent it
        /// by passing the path it was given.
        attachments: Vec<Value>,
        /// And put the caller on the task's trackers. Absent means YES here
        /// and nowhere else: an agent that files a task almost always wants
        /// to know how it goes.
        track: Option<bool>,
        /// Watch it for the user, without asking them for a decision.
        notify_user: Option<bool>,
        /// File this task as a question for the user.
        mention_user: Option<bool>,
    },
    /// Say something on one, with typed references fenced by what it is about.
    TrackerCommentTask {
        task_id: String,
        body: String,
        /// The same, said with the comment.
        attachments: Vec<Value>,
        refs: Vec<crate::thread::ThreadLink>,
        track: Option<bool>,
        notify_user: Option<bool>,
        mention_user: Option<bool>,
    },
    /// Hand one over, which starts whoever gets it.
    ///
    /// The assignee is carried whole rather than as five sets of fields: the
    /// five kinds have different shapes, and the daemon's own parse is the one
    /// place that reads them and names each refusal.
    TrackerAssignTask {
        assignee: Value,
        task_id: String,
        note: Option<String>,
        track: Option<bool>,
        notify_user: Option<bool>,
    },
    /// Move one to another column.
    TrackerMoveTask {
        task_id: String,
        status: String,
        track: Option<bool>,
    },
    /// Add and remove labels on an existing task.
    TrackerLabelTask {
        task_id: String,
        add: Vec<String>,
        remove: Vec<String>,
        track: Option<bool>,
    },
    /// Close one.
    TrackerCloseTask {
        task_id: String,
        reason: Option<String>,
        track: Option<bool>,
    },
    /// Start or stop hearing about a task. Which agent is the CALLER: a
    /// tool cannot subscribe somebody else, the way it cannot sign a comment
    /// as somebody else, so neither carries an agent id.
    TrackerTrackTask {
        task_id: String,
    },
    TrackerUntrackTask {
        task_id: String,
    },
    /// Record what one is about.
    TrackerLinkTask {
        task_id: String,
        workspace_id: Option<String>,
        branch: Option<String>,
        commit: Option<String>,
        conversation_id: Option<String>,
        parent_task_id: Option<String>,
        track: Option<bool>,
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
            BridgeAction::PostThreadMessage { .. } => "post_thread_message",
            BridgeAction::SearchConversation { .. } => "search_conversation",
            BridgeAction::SetTopic { .. } => "set_topic",
            BridgeAction::SetName { .. } => "set_name",
            BridgeAction::CompactSelf { .. } => "compact_self",
            BridgeAction::CompactAgent { .. } => "compact_agent",
            BridgeAction::ListHarnesses => "list_harnesses",
            BridgeAction::ListProjects => "list_projects",
            BridgeAction::ListWork => "list_work",
            BridgeAction::ReadConversation { .. } => "read_conversation",
            BridgeAction::DispatchBranch { .. } => "dispatch_branch",
            BridgeAction::AskUser { .. } => "ask_user",
            BridgeAction::RouterMessage { .. } => "post_thread_message",
            BridgeAction::ListWorkspaces => "list_workspaces",
            BridgeAction::ListWorkspaceAgents { .. } => "list_workspace_agents",
            BridgeAction::CreateWorkspace { .. } => "create_workspace",
            BridgeAction::AddWorkspaceAgent { .. } => "add_workspace_agent",
            BridgeAction::RemoveWorkspaceAgent { .. } => "remove_workspace_agent",
            BridgeAction::MessageAgent { .. } => "message_agent",
            BridgeAction::MessageWorkspaceAgent { .. } => "message_workspace_agent",
            BridgeAction::DeleteWorkspace { .. } => "delete_workspace",
            BridgeAction::AddProjectSource { .. } => "add_project_source",
            BridgeAction::RemoveProjectSource { .. } => "remove_project_source",
            BridgeAction::ReclaimWorkspace { .. } => "reclaim_workspace",
            BridgeAction::AddWorkspaceDirectory { .. } => "add_workspace_directory",
            BridgeAction::RemoveWorkspaceDirectory { .. } => "remove_workspace_directory",
            // The task tracker's twelve, on both working surfaces.
            BridgeAction::TrackerListTasks { .. } => "list_tasks",
            BridgeAction::TrackerGetTask { .. } => "get_task",
            BridgeAction::TrackerReadComment { .. } => "read_comment",
            BridgeAction::TrackerCreateTask { .. } => "create_task",
            BridgeAction::TrackerCommentTask { .. } => "comment_task",
            BridgeAction::TrackerAssignTask { .. } => "assign_task",
            BridgeAction::TrackerMoveTask { .. } => "move_task",
            BridgeAction::TrackerLabelTask { .. } => "label_task",
            BridgeAction::TrackerCloseTask { .. } => "close_task",
            BridgeAction::TrackerLinkTask { .. } => "link_task",
            BridgeAction::TrackerTrackTask { .. } => "track_task",
            BridgeAction::TrackerUntrackTask { .. } => "untrack_task",
        }
    }

    /// Which surfaces carry this action. The socket enforces it against the
    /// session that sent it, so a harness cannot reach another surface's tools
    /// by writing the frame itself.
    ///
    /// A conversation tool is on every surface that HAS a conversation, and
    /// the workspace tools are on every surface whose agent is bound to a
    /// project — which is why this is a list. What belongs to exactly one
    /// surface is what [`surface_name`](Self::surface_name) names.
    pub fn surfaces(&self) -> &'static [McpSurface] {
        match self {
            BridgeAction::PostThreadMessage { .. }
            | BridgeAction::SearchConversation { .. }
            | BridgeAction::SetTopic { .. }
            | BridgeAction::SetName { .. }
            | BridgeAction::CompactSelf { .. }
            | BridgeAction::ListHarnesses
            | BridgeAction::MessageAgent { .. } => &[McpSurface::Coding, McpSurface::Project],
            BridgeAction::ListProjects
            | BridgeAction::ListWork
            | BridgeAction::ReadConversation { .. }
            | BridgeAction::DispatchBranch { .. }
            | BridgeAction::AskUser { .. }
            | BridgeAction::RouterMessage { .. } => &[McpSurface::Router],
            BridgeAction::ListWorkspaces
            | BridgeAction::ListWorkspaceAgents { .. }
            | BridgeAction::CreateWorkspace { .. }
            | BridgeAction::AddWorkspaceAgent { .. }
            | BridgeAction::RemoveWorkspaceAgent { .. }
            | BridgeAction::MessageWorkspaceAgent { .. }
            | BridgeAction::DeleteWorkspace { .. }
            | BridgeAction::AddWorkspaceDirectory { .. }
            | BridgeAction::RemoveWorkspaceDirectory { .. } => {
                &[McpSurface::Project, McpSurface::Coding]
            }
            // The folders a project is cut FROM stay with the project agent: an
            // agent in a checkout changes what its workspace holds, never what
            // the next workspace will be made of.
            BridgeAction::AddProjectSource { .. }
            | BridgeAction::RemoveProjectSource { .. }
            | BridgeAction::ReclaimWorkspace { .. } => &[McpSurface::Project],
            // Compacting ANOTHER agent is staffing, which is the project
            // agent's business; every agent compacts itself.
            BridgeAction::CompactAgent { .. } => &[McpSurface::Project],
            // The tracker is on every surface that WORKS a project: a coding
            // agent files and moves the tasks it is given, and a project
            // agent runs the board. The router has no project to be scoped to.
            BridgeAction::TrackerListTasks { .. }
            | BridgeAction::TrackerGetTask { .. }
            | BridgeAction::TrackerReadComment { .. }
            | BridgeAction::TrackerCreateTask { .. }
            | BridgeAction::TrackerCommentTask { .. }
            | BridgeAction::TrackerAssignTask { .. }
            | BridgeAction::TrackerMoveTask { .. }
            | BridgeAction::TrackerLabelTask { .. }
            | BridgeAction::TrackerCloseTask { .. }
            | BridgeAction::TrackerLinkTask { .. }
            | BridgeAction::TrackerTrackTask { .. }
            | BridgeAction::TrackerUntrackTask { .. } => &[McpSurface::Coding, McpSurface::Project],
        }
    }

    /// Whether a session on `surface` may call this action at all.
    pub fn allowed_on(&self, surface: McpSurface) -> bool {
        self.surfaces().contains(&surface)
    }

    /// The surface an error names when refusing this action: the first one it
    /// is on, which for everything that can be refused is its only one.
    pub fn surface_name(&self) -> &'static str {
        self.surfaces()
            .first()
            .copied()
            .unwrap_or(McpSurface::Coding)
            .as_str()
    }
}

/// Which set of tools a session gets.
///
/// Not a permission flag on one server: three surfaces, and a session is on
/// exactly one of them for its whole life. A coding agent never sees the
/// router's tools and a router never sees a coding agent's, so neither can
/// reach the other's by asking. The coding and project surfaces do share the
/// workspace tools, because both agents are bound to a project — and each is
/// held to its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpSurface {
    Coding,
    Router,
    /// The agent of a project's conversation owner: it reads the project's
    /// workspaces, staffs them and talks, and holds no checkout to change.
    Project,
}

impl McpSurface {
    /// The surface an owner id names. The id itself carries the answer — a
    /// router session's id is prefixed, and so is a project agent's — so the
    /// surface can never disagree with the session it was resolved for.
    pub fn for_owner(owner_id: &str) -> McpSurface {
        if crate::router::is_router_agent(owner_id) {
            McpSurface::Router
        } else if crate::agent::is_project_agent(owner_id) {
            McpSurface::Project
        } else {
            McpSurface::Coding
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            McpSurface::Coding => "coding",
            McpSurface::Router => "router",
            McpSurface::Project => "project",
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

    /// The router's message schema. A terminal message also reports routing status.
    fn router_message_input_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": { "type": "string", "enum": ["Complete", "Blocked", "Waiting", "Working"] },
                "body": { "type": "string", "description": "One concise sentence saying where the capture went and why. If you could not route it, say what stopped you." }
            },
            "required": ["status", "body"]
        })
    }

    /// The router's tools. Read broadly, dispatch branch work, or ask the user
    /// for the missing routing choice.
    fn router_tools() -> Value {
        json!([{
            "name": "list_projects",
            "description": "Every project on this device, with its id, name and repository path. Start here: a capture is routed to a project before it is routed to anything else.",
            "inputSchema": { "type": "object", "properties": {} }
        }, {
            "name": "list_work",
            "description": "The branches and tasks in flight across every project: what each one is, which project it belongs to, its state, and whether an agent is working it right now. This is what you check the capture against before you believe it continues existing work.",
            "inputSchema": { "type": "object", "properties": {} }
        }, {
            "name": "read_conversation",
            "description": "Read one work item's conversation, newest last. Read-only — you cannot post to it. Use it to confirm a capture really continues the work on a branch before dispatching to it; a branch whose conversation is about something else is not the destination.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entity_id": { "type": "string", "description": "The run_id or task_id from list_work." },
                    "agent_id": { "type": "string", "description": "Which agent's conversation, when the item has several. Omit for the item's own." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 200, "default": 40 }
                },
                "required": ["entity_id"]
            }
        }, {
            "name": "dispatch_branch",
            "description": "Put an agent on a branch with this instruction, creating or adopting the checkout as needed. Use ONLY when the capture names an existing branch or worktree, or unambiguously continues work already in flight on one — this starts an agent that changes code.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_id": { "type": "string" },
                    "branch": { "type": "string", "description": "The existing branch the work continues on, spelled exactly as it is — it is used as given, prefix and all. Omit only when the capture is new branch work whose name comes from the instruction." },
                    "name": { "type": "string", "description": "A short name for the new agent, required because you are creating it." },
                    "instruction": { "type": "string", "description": "What the agent should do, in the user's terms." },
                    "rationale": { "type": "string", "description": "One line on why this branch is the destination." }
                },
                "required": ["project_id", "name", "instruction"]
            }
        }, {
            "name": "ask_user",
            "description": "Ask the user the ONE question that would let you decide, and stop. Reserved for a capture whose project is ambiguous — asking is the friction capture exists to remove, so dispatch to the best-guess project when the choice is clear enough. The question reaches them as the capture's own inbox entry. Offer up to 3 options when you can name the destinations you are choosing between: each is one tap for the user, and the answer comes back naming the one they picked. They can always type an answer instead, so options are a shortcut and never the whole answer.",
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
                                "label": { "type": "string", "description": "What the user taps, in a few words: the destination, not the question again. e.g. \"Continue on Build\"." },
                                "project_id": { "type": "string", "description": "The project this choice routes to, from list_projects." },
                                "kind": { "type": "string", "enum": ["branch"], "description": "The branch destination this choice represents." },
                                "branch": { "type": "string", "description": "The existing branch this choice continues, spelled exactly as it is. Naming one makes the choice a branch." }
                            },
                            "required": ["label"]
                        }
                    }
                },
                "required": ["question"]
            }
        }, {
            "name": "post_thread_message",
            "description": "Send a message to the user. This is the only way the user sees what you say. Use status=Complete after dispatching, Blocked when routing cannot proceed, Waiting when you need the user's answer, or Working for a progress update.",
            "inputSchema": Self::router_message_input_schema()
        }])
    }

    /// The coding agent's tools: its conversation, and the workspaces of the
    /// project its checkout belongs to.
    fn coding_tools() -> Value {
        let mut tools = vec![
            json!({
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
                "description": "Send a message to the user. This tool is the only way the user can see your messages; terminal output and ordinary assistant responses are not visible to them. Every call needs a status: Complete when the objective is met, Blocked when an environment or implementation problem prevents progress, Waiting when you need a user response, or Working for a progress update while you continue. Always call it once with Complete or Blocked as the final outcome.",
                "inputSchema": Self::message_input_schema()
            }),
            json!({
                "name": "message_agent",
                "description": MESSAGE_AGENT_DESCRIPTION,
                "inputSchema": Self::message_agent_input_schema()
            }),
            json!({
                "name": "search_conversation",
                "description": SEARCH_CONVERSATION_DESCRIPTION,
                "inputSchema": Self::search_conversation_input_schema()
            }),
            json!({
                "name": "set_topic",
                "description": SET_TOPIC_DESCRIPTION,
                "inputSchema": Self::set_topic_input_schema()
            }),
            compaction::compact_self_tool(),
            json!({
                "name": "set_name",
                "description": SET_NAME_DESCRIPTION,
                "inputSchema": Self::set_name_input_schema()
            }),
        ];
        tools.extend(Self::workspace_tools());
        tools.extend(Self::task_tools());
        Value::Array(tools)
    }

    /// Who to write to and what to say — the whole of `message_agent`. There
    /// is no workspace and no project on it: the id is the address, and the
    /// scope is read off the sender.
    fn message_agent_input_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent_id": { "type": "string", "description": "The agent to write to. A message from an agent carries the id to answer it on; list_workspace_agents answers the ids on a workspace. An agent outside your project, an id that names nobody, and your own id are refused." },
                "body": { "type": "string", "description": concat!("What to say, in full. The other agent has none of your conversation, so say what it needs rather than pointing at what you were told.", reference_shapes_note!()) }
            },
            "required": ["agent_id", "body"]
        })
    }

    /// The conversation search schema, shared by every surface that has a
    /// conversation to search.
    fn search_conversation_input_schema() -> Value {
        json!({
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
        })
    }

    /// The topic schema, shared for the same reason.
    fn set_topic_input_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "topic": { "type": "string", "description": "The objective, in 2-4 words. Title-case the first word, no trailing period. Examples: \"Unify prompt delivery\", \"Fix login redirect\"." }
            },
            "required": ["topic"]
        })
    }

    /// The registry as a tool schema reads it: every harness this bridge can
    /// run, named by its wire id, with its label and whether it is installed
    /// here in the one line an agent sees beside the value.
    ///
    /// Built here rather than written down, so a harness added to the registry
    /// shows up in the schema without anybody remembering to copy it across.
    fn harness_enum() -> Value {
        let catalogs = crate::models::provider_catalogs();
        let described = catalogs
            .iter()
            .map(|catalog| {
                let installed = if catalog.installed {
                    String::new()
                } else {
                    format!(
                        " (not installed on this machine — `{}` is not on PATH)",
                        catalog.binary
                    )
                };
                format!("{} — {}{installed}", catalog.id.wire_id(), catalog.label)
            })
            .collect::<Vec<_>>()
            .join("; ");
        json!({
            "type": "string",
            "enum": catalogs.iter().map(|catalog| catalog.id.wire_id()).collect::<Vec<_>>(),
            "description": format!("What the agent runs on. {described}. Omit to use the device's choice for this task and scope."),
        })
    }

    /// The roles the user can have declared a model for, as a schema.
    fn role_enum() -> Value {
        json!({
            "type": "string",
            "enum": crate::models::AgentRole::ALL.map(|role| role.wire_id()),
            "description": format!(
                "What this agent is to be. The user has chosen which model fills each role, so asking for one is how you get their choice instead of guessing. {}. The answer tells you how much direction that model needs.",
                crate::models::AgentRole::ALL
                    .map(|role| format!("{} — {}", role.wire_id(), role.describes()))
                    .join("; ")
            ),
        })
    }

    /// How much direction the created agent should need. An input only when
    /// the caller knows what kind of brief it can write.
    fn capability_enum() -> Value {
        json!({
            "type": "string",
            "enum": crate::models::AgentCapability::ALL.map(|capability| capability.wire_id()),
            "description": format!(
                "Only when you need a particular kind: {}. Omit and you get the user's first choice for the role, whatever its capability — and are told which it is.",
                crate::models::AgentCapability::ALL
                    .map(|capability| format!("{} — {}", capability.wire_id(), capability.describes()))
                    .join("; ")
            ),
        })
    }

    /// Every effort any harness accepts. Not per harness: one schema is shown
    /// before the harness is chosen, and a value the chosen one does not take
    /// is refused by name when the call is made.
    fn effort_enum() -> Value {
        let mut efforts: Vec<&'static str> = crate::models::provider_catalogs()
            .iter()
            .flat_map(|catalog| catalog.efforts.iter().copied())
            .collect();
        efforts.dedup();
        let mut seen = Vec::new();
        for effort in efforts {
            if !seen.contains(&effort) {
                seen.push(effort);
            }
        }
        json!({
            "type": "string",
            "enum": seen,
            "description": "The reasoning effort, for a harness that takes one. This one is YOURS to judge — the user chooses the model for a role, you choose how hard it thinks about this piece of work. Not every harness accepts every value; list_harnesses says which.",
        })
    }

    /// The name schema. Separate from the topic's for the reason the tools are
    /// separate: they are asked for different things and refused on different
    /// grounds.
    fn set_name_input_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "One or two meaningful words for what you are: \"Tracker\", \"Rail scroll\", \"Transport\". At most three words and 24 characters, and not a name another agent on this conversation already has." }
            },
            "required": ["name"]
        })
    }

    // -------------------------------------------------- task tracker ---
    // The per-project task tracker (spec: Tasks). One block, shown by both
    // working surfaces, so a coding agent and a project agent are offered the
    // same twelve tools with the same words.

    /// The tracker's twelve, appended to whichever surface is being built.
    ///
    /// None takes a project: the scope is the calling agent's own, read off
    /// the session, so there is nothing to pass and no other project reachable.
    /// None takes an author either — the bridge knows who is calling, so a
    /// comment is signed by whoever wrote it and an event by whoever caused it.
    fn task_tools() -> Vec<Value> {
        let task_id = json!({
            "type": "string",
            "description": "From list_tasks, or the task you were handed."
        });
        let status = json!({
            "type": "string",
            "enum": ["backlog", "ready", "in_progress", "in_review", "done"],
            "description": "A column of the board."
        });
        // Every write carries it, so following a task is never a second
        // call. `create_task` describes its own default, which is the other
        // way round.
        let track = json!({
            "type": "boolean",
            "description": "And follow this task from now on: every later change to it arrives as a message here. Defaults to false; asking twice is not two trackers."
        });
        // Two kinds of file: one the user already sent you, passed back by the
        // `path` it arrived on, and one you made yourself, named by its full
        // path on this machine and copied into the store when the call lands
        // (#116). Either way the task holds the bytes, not a pointer.
        let attachments = json!({
            "type": "array",
            "maxItems": 10,
            "description": "Files to file with this task, shown on it to anyone reading, phone included. Use this for the screenshots, recordings and logs that prove a claim — a before/after, a failing run, a UI you changed — instead of listing their paths in the text, which nobody reading the task can open. Two kinds: an attachment you were sent (pass its `path` through unchanged), or a file you made, by its full path on this machine: images (png, jpg, webp, gif), videos (mp4, webm) and plain text or logs, up to 50 MB each. Build copies your file when the call lands, so you may delete it afterwards.",
            "items": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "An attachment's path exactly as you received it, or the full (absolute) path of a file you made." },
                    "name": { "type": "string", "description": "What to call it. Optional; the stored name is used otherwise." }
                },
                "required": ["path"]
            }
        });
        vec![
            json!({
                "name": "list_tasks",
                "description": "The tasks of your project, newest first: what each one is, who holds it, which column it is in and what it is about. Which project is read comes from who you are — there is nothing to pass, and no other project is reachable from here.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "state": { "type": "string", "enum": ["open", "closed"], "description": "Omit for both." },
                        "status": status,
                        "label": { "type": "string" }
                    }
                }
            }),
            json!({
                "name": "get_task",
                "description": "One task and its whole timeline: every comment and everything that has happened to it, oldest first. Read this before you act on a task somebody handed you — the body says what is wanted and the timeline says what has already been tried.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "task_id": task_id },
                    "required": ["task_id"]
                }
            }),
            json!({
                "name": "read_comment",
                "description": "One comment on one task, by the id a notice gave you. A notice is one line and carries no comment text on purpose — this is how you read the words when you decide you care, and not reading it costs nothing. Use get_task instead when you want the whole timeline rather than the one comment.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "comment_id": { "type": "string", "description": "From a notice, or from a task's timeline." }
                    },
                    "required": ["comment_id"]
                }
            }),
            json!({
                "name": "create_task",
                "description": "File a Build task: a card on your project's board, not an entry in your harness's own task or todo list. Two things it is for: work you have found and are NOT doing — a task is cheap, and something you noticed and did not write down exists only in this conversation — and work you ARE doing that runs to more than one step, filed and assigned to yourself so the user can see what is in progress without opening your conversation. It is filed, not started; assign it to start anyone on it, yourself included. If its body asks the user for a decision, pass mention_user: true so it reaches Needs you until they read it; a question in the body alone does not.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "One line saying what is wanted, in the user's terms." },
                        "body": { "type": "string", "description": concat!("Markdown. What you know: what happens, where you saw it, what you think is behind it.", reference_shapes_note!()) },
                        "status": status,
                        "labels": { "type": "array", "items": { "type": "string" }, "maxItems": 20 },
                        "priority": { "type": "string", "enum": ["none", "low", "medium", "high", "urgent"] },
                        "attachments": attachments.clone(),
                        "track": {
                            "type": "boolean",
                            "description": "Follow this task: every later change to it arrives as a message here. Defaults to TRUE — a task you filed is one you almost always want to hear about. Pass false for one you are filing for somebody else."
                        },
                        "notify_user": {
                            "type": "boolean",
                            "description": "Watch this task for the user. Watching alone does not put it in Needs you or ask them for an answer. To ask, pass mention_user: true when filing, comment later with mention_user: true, or assign the task to the user."
                        },
                        "mention_user": {
                            "type": "boolean",
                            "description": "Put this new task in the user's Needs you until they read it, and watch it for them. Use when the task body asks the user to read or answer a decision; leave it off for bookkeeping."
                        }
                    },
                    "required": ["title"]
                }
            }),
            json!({
                "name": "comment_task",
                "description": "Say something on a task. This is how progress on a task you were handed becomes visible: the conversation you are in is yours, and the task is where the user and the other agents look. It is also where you ANSWER: a comment on a task you hold is a question, and it is answered here rather than in your own thread — the user reads the task, not your conversation. The same goes for asking: a question about a task that came from outside your conversation goes here, because the assigner and the user both read the task and the answer comes back to you. For any decision the user must make, pass mention_user: true; that puts the task in Needs you until the user reads it. A question left only in the task body does not ask them; notify_user on create or assign only watches.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "task_id": task_id,
                        "body": { "type": "string", "description": concat!("Markdown.", reference_shapes_note!()) },
                        "track": track,
                        "attachments": attachments,
                        "notify_user": {
                            "type": "boolean",
                            "description": "Watch this task for the user and mark this comment as needing their attention; the unread comment puts it in Needs you until they read it. Use mention_user: true for a question or decision addressed to the user."
                        },
                        "mention_user": {
                            "type": "boolean",
                            "description": "Put this task in the user's Needs you until they read this comment, and watch it for them. Use for any decision the user must read or answer; leave it off for bookkeeping."
                        },
                        "refs": {
                            "type": "array",
                            "maxItems": 20,
                            "description": "Typed references. A file needs the task to link the workspace it is in; a commit must be one the task links.",
                            "items": { "type": "object", "properties": {
                                "kind": { "type": "string", "enum": ["file", "commit", "worktree"] },
                                "path": { "type": "string" },
                                "line_start": { "type": "integer", "minimum": 1 },
                                "line_end": { "type": "integer", "minimum": 1 },
                                "sha": { "type": "string" },
                                "worktree_id": { "type": "string" }
                            }, "required": ["kind"] }
                        }
                    },
                    "required": ["task_id", "body"]
                }
            }),
            json!({
                "name": "assign_task",
                "description": "Hand a task to somebody. Assigning IS dispatching: it delivers the task into that agent's conversation and starts it. This is how you hand work off — anything beyond a quick question or a one-line correction is filed and assigned rather than sent as a message, because the task is where the user and the other agents look and a brief sent as a message is a brief only its reader has. Assign it to yourself to plan and track work you are doing yourself. For a new_workspace or new_agent assignee, give the agent a short job name in agent_name.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "task_id": task_id,
                        "assignee": {
                            "type": "object",
                            "description": "Who gets it, and where the work runs. Pass null to unassign.",
                            "properties": {
                                "kind": { "type": "string", "enum": ["user", "project_agent", "agent", "new_workspace", "new_agent"] },
                                "agent_id": { "type": "string", "description": "With kind=agent. An agent of your project; one outside it is refused." },
                                "workspace_id": { "type": "string", "description": "With kind=new_agent: the workspace the new agent works in." },
                                "name": { "type": "string", "description": "With kind=new_workspace, the WORKSPACE name. The task's title when omitted." },
                                "agent_name": { "type": "string", "minLength": 1, "description": "Required with kind=new_workspace or new_agent. A short name describing the agent's job, like \"Flaky test fixer\"." },
                                "isolation": { "type": "string", "enum": ["worktree", "rift"], "description": "With kind=new_workspace. Omit for the project's own setting." },
                                "harness": { "type": "string", "description": "What a new agent runs on. Omit for the user's default." },
                                "model": { "type": "string" },
                                "effort": { "type": "string" },
                                "notify_user": { "type": "boolean", "description": "With kind=new_workspace or new_agent: watch the new agent and show its workspace in the USER's inbox. Pass true when the user asked to follow that agent's work. Otherwise the new agent is unwatched. This is separate from the top-level notify_user, which watches the task." }
                            },
                            "required": ["kind"],
                            "allOf": [
                                { "if": { "properties": { "kind": { "const": "new_workspace" } } }, "then": { "required": ["agent_name"] } },
                                { "if": { "properties": { "kind": { "const": "new_agent" } } }, "then": { "required": ["agent_name"] } }
                            ]
                        },
                        "note": { "type": "string", "description": "Extra instruction delivered under the task. The task's body is the task; this is what you would have said in a message." },
                        "track": track,
                        "notify_user": {
                            "type": "boolean",
                            "description": "Watch this task for the user. Watching alone does not put it in Needs you or ask them for an answer. Assign it to kind=user to ask them directly, or use mention_user on create_task or comment_task."
                        }
                    },
                    "required": ["task_id", "assignee"]
                }
            }),
            json!({
                "name": "move_task",
                "description": "Move a task to another column. Move it to In review when you report Complete: that says the work is ready to be looked at, not that it is accepted.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "task_id": task_id, "status": status, "track": track },
                    "required": ["task_id", "status"]
                }
            }),
            json!({
                "name": "label_task",
                "description": "Add or remove labels on an existing task. Labels are free text, including spaces. Returns the task and its labels after the change. Adding a label already present or removing one absent does nothing.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "task_id": task_id,
                        "add": { "type": "array", "items": { "type": "string" }, "maxItems": 20 },
                        "remove": { "type": "array", "items": { "type": "string" }, "maxItems": 20 },
                        "track": track
                    },
                    "required": ["task_id"]
                }
            }),
            json!({
                "name": "close_task",
                "description": "Close a task. Closing is not the Done column: one says whether anyone is still expected to act, the other says where the card is. Closing a task that is already closed is refused.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "task_id": task_id, "reason": { "type": "string", "description": "One line on why." }, "track": track },
                    "required": ["task_id"]
                }
            }),
            json!({
                "name": "link_task",
                "description": "Record what a task is about: the workspace being worked in, the branch, a commit, or the conversation working it. Assigning a task already links the assignee's workspace and conversation, so this is for the branch you cut for it, each commit that lands for it, and any second workspace. A link has to name something of your own project. Name at least one.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "task_id": task_id,
                        "workspace_id": { "type": "string" },
                        "branch": { "type": "string", "description": "Spelled exactly as it is." },
                        "commit": { "type": "string", "description": "A full 40-character sha." },
                        "conversation_id": { "type": "string", "description": "A conversation owner id of your project." },
                        "parent_task_id": { "type": "string", "description": "The task this one is part of. A cycle is refused." },
                        "track": track
                    },
                    "required": ["task_id"]
                }
            }),
            json!({
                "name": "track_task",
                "description": "Start hearing about a task. Every later change to it — a move, a comment, an assignment, an edit — arrives as a message in your conversation, and starts your turn if you are idle. Use it on a task you depend on or are collaborating around; you are tracked automatically on anything assigned to you. Your own changes are never echoed back to you.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "task_id": task_id.clone() },
                    "required": ["task_id"]
                }
            }),
            json!({
                "name": "untrack_task",
                "description": "Stop hearing about a task. Being unassigned does not do this on its own — handing work on is often exactly when you still want to know how it went — so say so when you no longer do.",
                "inputSchema": {
                    "type": "object",
                    "properties": { "task_id": task_id.clone() },
                    "required": ["task_id"]
                }
            }),
        ]
    }

    /// The tracker's `tools/call` arms, shared by both working surfaces.
    /// `None` is "not one of mine", which every other tool is.
    fn handle_task_tools_call(id: &Value, name: &str, params: Option<&Value>) -> Option<Handled> {
        // An id an agent read before the rename (#190) names the same task.
        let task = || required_argument(params, "task_id").map(|id| current_id(&id));
        Some(match name {
            "list_tasks" => acted(
                id.clone(),
                BridgeAction::TrackerListTasks {
                    state: optional_argument(params, "state"),
                    status: optional_argument(params, "status"),
                    label: optional_argument(params, "label"),
                },
            ),
            "read_comment" => match required_argument(params, "comment_id") {
                Ok(comment_id) => {
                    let comment_id = current_id(&comment_id);
                    acted(id.clone(), BridgeAction::TrackerReadComment { comment_id })
                }
                Err(message) => refused(id.clone(), message),
            },
            "get_task" => match task() {
                Ok(task_id) => acted(id.clone(), BridgeAction::TrackerGetTask { task_id }),
                Err(message) => refused(id.clone(), message),
            },
            "create_task" => match required_argument(params, "title") {
                Ok(title) => acted(
                    id.clone(),
                    BridgeAction::TrackerCreateTask {
                        title,
                        body: optional_argument(params, "body"),
                        status: optional_argument(params, "status"),
                        labels: string_list_argument(params, "labels"),
                        priority: optional_argument(params, "priority"),
                        attachments: value_list_argument(params, "attachments"),
                        track: optional_flag(params, "track"),
                        notify_user: optional_flag(params, "notify_user"),
                        mention_user: optional_flag(params, "mention_user"),
                    },
                ),
                Err(message) => refused(id.clone(), message),
            },
            "comment_task" => {
                match task().and_then(|task_id| Ok((task_id, required_argument(params, "body")?))) {
                    Ok((task_id, body)) => match task_refs(params) {
                        Ok(refs) => acted(
                            id.clone(),
                            BridgeAction::TrackerCommentTask {
                                task_id,
                                body,
                                refs,
                                attachments: value_list_argument(params, "attachments"),
                                track: optional_flag(params, "track"),
                                notify_user: optional_flag(params, "notify_user"),
                                mention_user: optional_flag(params, "mention_user"),
                            },
                        ),
                        Err(message) => refused(id.clone(), message),
                    },
                    Err(message) => refused(id.clone(), message),
                }
            }
            "assign_task" => match task().and_then(|task_id| {
                let assignee = argument(params, "assignee").unwrap_or(Value::Null);
                require_created_agent_name(&assignee)?;
                Ok((task_id, assignee))
            }) {
                Ok((task_id, assignee)) => acted(
                    id.clone(),
                    BridgeAction::TrackerAssignTask {
                        task_id,
                        // Absent and null are both unassignment, which is a
                        // legible thing to ask for.
                        assignee,
                        note: optional_argument(params, "note"),
                        track: optional_flag(params, "track"),
                        notify_user: optional_flag(params, "notify_user"),
                    },
                ),
                Err(message) => refused(id.clone(), message),
            },
            "move_task" => {
                match task().and_then(|task_id| Ok((task_id, required_argument(params, "status")?)))
                {
                    Ok((task_id, status)) => acted(
                        id.clone(),
                        BridgeAction::TrackerMoveTask {
                            task_id,
                            status,
                            track: optional_flag(params, "track"),
                        },
                    ),
                    Err(message) => refused(id.clone(), message),
                }
            }
            "label_task" => match task().and_then(|task_id| {
                Ok((
                    task_id,
                    label_list_argument(params, "add")?,
                    label_list_argument(params, "remove")?,
                ))
            }) {
                Ok((task_id, add, remove)) => acted(
                    id.clone(),
                    BridgeAction::TrackerLabelTask {
                        task_id,
                        add,
                        remove,
                        track: optional_flag(params, "track"),
                    },
                ),
                Err(message) => refused(id.clone(), message),
            },
            "close_task" => match task() {
                Ok(task_id) => acted(
                    id.clone(),
                    BridgeAction::TrackerCloseTask {
                        task_id,
                        reason: optional_argument(params, "reason"),
                        track: optional_flag(params, "track"),
                    },
                ),
                Err(message) => refused(id.clone(), message),
            },
            "track_task" => match task() {
                Ok(task_id) => acted(id.clone(), BridgeAction::TrackerTrackTask { task_id }),
                Err(message) => refused(id.clone(), message),
            },
            "untrack_task" => match task() {
                Ok(task_id) => acted(id.clone(), BridgeAction::TrackerUntrackTask { task_id }),
                Err(message) => refused(id.clone(), message),
            },
            "link_task" => match task() {
                Ok(task_id) => acted(
                    id.clone(),
                    BridgeAction::TrackerLinkTask {
                        task_id,
                        workspace_id: optional_argument(params, "workspace_id"),
                        branch: optional_argument(params, "branch"),
                        commit: optional_argument(params, "commit"),
                        conversation_id: optional_argument(params, "conversation_id"),
                        // And by the name it had before the rename (#190):
                        // dropped, it would leave the link half made.
                        parent_task_id: optional_argument(params, "parent_task_id")
                            .or_else(|| optional_argument(params, "parent_issue_id"))
                            .map(|id| current_id(&id)),
                        track: optional_flag(params, "track"),
                    },
                ),
                Err(message) => refused(id.clone(), message),
            },
            _ => return None,
        })
    }

    /// The tools one surface is shown, by name and in the order it shows them.
    ///
    /// Codex is handed its allow-list in argv and never asks `tools/list`, so
    /// the list it is given is built from this rather than written a second
    /// time (`harness/codex.rs`).
    pub fn tool_names_of(surface: McpSurface) -> Vec<String> {
        let server = DoneServer {
            owner_id: String::new(),
            surface,
        };
        server
            .tools()
            .as_array()
            .map(|tools| {
                tools
                    .iter()
                    .filter_map(|tool| tool["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The tools this session is shown. One inventory per surface, resolved
    /// from the id the session was opened with, so the list a harness is given
    /// and the calls the socket accepts can never be two different answers.
    fn tools(&self) -> Value {
        match self.surface {
            McpSurface::Coding => Self::coding_tools(),
            McpSurface::Router => Self::router_tools(),
            McpSurface::Project => Self::project_tools(),
        }
    }

    /// The project agent's message schema: a status and a sentence. No phase —
    /// it plans nothing, builds nothing and validates nothing, so there is no
    /// phase for it to claim.
    fn project_message_input_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": { "type": "string", "enum": ["Complete", "Blocked", "Waiting", "Working"] },
                "body": { "type": "string", "description": concat!("What you have to say, in the user's terms. Complete for an outcome or an answer, Waiting when the next step is their call, Working only while a long read on their question is still going, never as a progress report.", reference_shapes_note!()) }
            },
            "required": ["status", "body"]
        })
    }

    /// The workspace tools, in the order every surface that carries them shows
    /// them.
    ///
    /// One inventory, shared by the project agent and by the agents working in
    /// a checkout, because the scope rule is the same for both: the project is
    /// the one the ASKING agent is bound to, no tool takes a project, and a
    /// workspace outside that project is refused before anything runs. That is
    /// why none of these descriptions names a project — there is nothing for
    /// the caller to name.
    fn workspace_tools() -> Vec<Value> {
        vec![
            json!({
                "name": "list_workspaces",
                "description": "Every workspace in your project: its id and name, the branch it stands on and how its checkout is doing. Which project is read comes from who you are — there is nothing to pass, and no other project is reachable from here.",
                "inputSchema": { "type": "object", "properties": {} }
            }),
            json!({
                "name": "list_workspace_agents",
                "description": "The agents on one workspace's conversation, in rail order: who each one is, what it runs on, and whether it is working right now. Read-only — you cannot post to them.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workspace_id": { "type": "string", "description": "From list_workspaces. A workspace outside your project is refused." }
                    },
                    "required": ["workspace_id"]
                }
            }),
            json!({
                "name": "create_workspace",
                "description": "Cut a new workspace in your project: its own checkout of every source, on a branch of its own. This is how a separate checkout is obtained in Build — never `git worktree add`, never a manual clone or copy of the folder you are standing in, because a checkout Build did not cut is one nobody can see, review or clean up. It is made the way the project is configured (a git worktree, or a copy-on-write clone) unless you say otherwise, cut in your project — there is nothing to name — and nobody is working in it until you add an agent.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "What the workspace is for, in the user's words. It names the branch too." },
                        "isolation": { "type": "string", "enum": ["worktree", "rift"], "description": "How the checkout is made. Omit for the project's own setting." }
                    },
                    "required": ["name"]
                }
            }),
            json!({
                "name": "add_workspace_agent",
                "description": "Put a new agent on one of your workspaces, in its own conversation there. The workspace gets a conversation of its own if it has none yet. Nothing is said to the agent until you message it. The agent is unwatched unless you pass notify_user: true when the user asked to follow its work.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workspace_id": { "type": "string", "description": "From list_workspaces. A workspace outside your project is refused." },
                        "role": Self::role_enum(),
                        "capability": Self::capability_enum(),
                        "harness": Self::harness_enum(),
                        "model": { "type": "string", "description": "A model id, when the user named one. list_harnesses is where the ids are. Omit to take the user's own choice for the role." },
                        "effort": Self::effort_enum(),
                        "name": { "type": "string", "minLength": 1, "description": "Required. A short name describing the agent's job, like \"Flaky test fixer\". It is what you and the user will see instead of \"Agent 2\"." },
                        "notify_user": { "type": "boolean", "description": "Watch this new agent and show its workspace in the USER's inbox. Pass true when the user asked to follow its work; omit for an unwatched agent." }
                    },
                    "required": ["workspace_id", "name"]
                }
            }),
            json!({
                "name": "list_harnesses",
                "description": "Every harness this device can run an agent on, with its models, its reasoning efforts, which of them are installed here, and what the user has chosen for each kind of task. Read it when the user names a model — the ids are here — or when you want to know what you are choosing between. Prefer passing `task` and `scope` to a create tool over picking a model yourself: those are the user's own choices.",
                "inputSchema": { "type": "object", "properties": {} }
            }),
            json!({
                "name": "remove_workspace_agent",
                "description": "Take an agent off one of your workspaces. Its session ends and its conversation goes with it; the workspace and its files are untouched.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workspace_id": { "type": "string", "description": "From list_workspaces. A workspace outside your project is refused." },
                        "agent_id": { "type": "string", "description": "From list_workspace_agents." }
                    },
                    "required": ["workspace_id", "agent_id"]
                }
            }),
            json!({
                "name": "message_workspace_agent",
                "description": "Say something to an existing agent on one of your workspaces: what to work on, or a question about what it is doing. With no agent_id, this addresses the existing primary agent; an empty workspace roster is refused, so call add_workspace_agent with a name first. It arrives knowing you sent it and not the user, and its reply comes back to you. Use post_thread_message to talk to the user; this one talks to an agent.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workspace_id": { "type": "string", "description": "From list_workspaces. A workspace outside your project is refused." },
                        "agent_id": { "type": "string", "description": "From list_workspace_agents. Omit for the workspace's first agent." },
                        "body": { "type": "string", "description": concat!("What to say, in full. The agent has none of your conversation, so say what it needs rather than pointing at what you were told.", reference_shapes_note!()) }
                    },
                    "required": ["workspace_id", "body"]
                }
            }),
            json!({
                "name": "delete_workspace",
                "description": "Take one of your workspaces away: its agents and terminals stop, its checkouts are handed back to the repositories they were cut from, and the folder goes. Anything in it that is not committed and pushed is gone with it.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workspace_id": { "type": "string", "description": "From list_workspaces. A workspace outside your project is refused." }
                    },
                    "required": ["workspace_id"]
                }
            }),
            json!({
                "name": "add_workspace_directory",
                "description": "Put one more directory into a workspace that is already standing. A Git source arrives as its own checkout on the workspace's branch; anything else is copied in. This and create_workspace are how a separate checkout is obtained in Build — never `git worktree add`, never a manual clone or copy of the folder you are standing in. Name exactly one of source_id, path or remote.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workspace_id": { "type": "string", "description": "From list_workspaces. A workspace outside your project is refused." },
                        "source_id": { "type": "string", "description": "A source of your project this workspace was not cut with." },
                        "path": { "type": "string", "description": "A folder on this device, for a directory that is nobody's project source." },
                        "remote": { "type": "string", "description": "A clone url. The clone lands in the workspace and is its own repository." },
                        "name": { "type": "string", "description": "What to call it, and the folder it mounts under." }
                    },
                    "required": ["workspace_id"]
                }
            }),
            json!({
                "name": "remove_workspace_directory",
                "description": "Take one directory out of a workspace: its checkout is handed back to the repository it was cut from and the folder goes. The workspace and its other directories stay. Uncommitted work in that directory is gone with it.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "workspace_id": { "type": "string", "description": "From list_workspaces. A workspace outside your project is refused." },
                        "directory_id": { "type": "string", "description": "From list_workspaces' directories." }
                    },
                    "required": ["workspace_id", "directory_id"]
                }
            }),
        ]
    }

    /// The project agent's tools: the workspaces every project-bound agent
    /// shares, the two verbs that change what the project itself is made of,
    /// and the conversation. Every one of them is about the agent's OWN
    /// project: none takes a project, and the binding the agent was minted with
    /// is the only thing that says which it is.
    ///
    /// What is deliberately here and nowhere else: `add_project_source` and
    /// `remove_project_source`. They change the template every NEXT workspace
    /// is cut from, which is the project agent's business and no coding
    /// agent's.
    fn project_tools() -> Value {
        let mut tools = Self::workspace_tools();
        tools.extend([json!({
            "name": "add_project_source",
            "description": "Add a folder to your project. The project is the template every new workspace is cut from, so this changes what the NEXT workspace gets; use add_workspace_directory to put the folder into a workspace that already exists. Name exactly one of path or remote.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "A folder on this device. A Git repository or an ordinary folder." },
                    "remote": { "type": "string", "description": "A clone url. Build clones it into the project's own sources folder." },
                    "name": { "type": "string", "description": "What to call it. The folder's own name when omitted." },
                    "base_branch": { "type": "string", "description": "The branch workspaces are cut from. The repository's own when omitted." }
                }
            }
        }), json!({
            "name": "remove_project_source",
            "description": "Take a folder off your project. Nothing on disk moves and no workspace loses a directory: new workspaces stop being cut from it. A project keeps at least one source.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "source_id": { "type": "string", "description": "From list_workspaces' project sources." }
                },
                "required": ["source_id"]
            }
        }), json!({
            "name": "reclaim_workspace",
            "description": "Remove a workspace whose work is safe somewhere else: every commit is pushed, nothing is uncommitted, no agent is working in it, and every task linked to it is Done or closed. Its agents and terminals stop, its checkouts are handed back, the folder goes, and each linked task records the reclaim. The local branch each checkout carried goes too, unless it is a default branch, is checked out elsewhere or has commits no remote has; then it stays and the task says why. Refused, with the reasons, while anything still holds it. That refusal is the difference from delete_workspace, which removes whatever is there.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "workspace_id": { "type": "string", "description": "From list_workspaces. A workspace outside your project is refused." }
                },
                "required": ["workspace_id"]
            }
        }), compaction::compact_agent_tool(), json!({
            "name": "post_thread_message",
            "description": "Send a message to the user. This is the only way the user sees what you say. Use status=Complete for an outcome or an answer, Waiting when the next step is the user's call, Blocked when you cannot proceed without them, or Working only while a long read on their question is still going, never as a progress report.",
            "inputSchema": Self::project_message_input_schema()
        }), json!({
            "name": "message_agent",
            "description": MESSAGE_AGENT_DESCRIPTION,
            "inputSchema": Self::message_agent_input_schema()
        }), json!({
            "name": "search_conversation",
            "description": SEARCH_CONVERSATION_DESCRIPTION,
            "inputSchema": Self::search_conversation_input_schema()
        }), json!({
            "name": "set_topic",
            "description": SET_TOPIC_DESCRIPTION,
            "inputSchema": Self::set_topic_input_schema()
        }), compaction::compact_self_tool()]);
        tools.extend(Self::task_tools());
        Value::Array(tools)
    }

    /// The coding agent's message: what it says, how its turn stands, and the
    /// choices it offers. Nothing else — which phase a report closes is the
    /// session's to know, not the agent's to claim.
    fn message_input_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": { "type": "string", "enum": ["Complete", "Blocked", "Waiting", "Working"] },
                "body": { "type": "string", "description": SUMMARY_DESCRIPTION },
                "options": {
                    "type": "array", "maxItems": crate::thread::MAX_MESSAGE_OPTIONS,
                    "items": { "type": "object", "properties": {
                        "label": { "type": "string" }, "message": { "type": "string" }
                    }, "required": ["label"] }
                }
            },
            "required": ["status", "body"]
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
            "tools/list" => Handled {
                reply: Some(result(id, json!({ "tools": self.tools() }))),
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
        match self.surface {
            McpSurface::Router => self.handle_router_tools_call(id, name, params),
            McpSurface::Project => Self::handle_project_tools_call(id, name, params),
            McpSurface::Coding => self.handle_coding_tools_call(id, name, params),
        }
    }

    /// The project surface's `tools/call`: reads and writes of the project the
    /// agent belongs to, and the conversation tools every agent has.
    ///
    /// No arm reads a project out of the arguments. A call that carries one
    /// anyway is parsed as though it had not: the scope is the owner binding
    /// the daemon holds, and there is nothing here for an argument to widen.
    fn handle_project_tools_call(id: Value, name: &str, params: Option<&Value>) -> Handled {
        // The tracker first, then the workspace tools: both are shared by
        // the two working surfaces, so each is parsed in one place.
        if let Some(handled) = Self::handle_task_tools_call(&id, name, params) {
            return handled;
        }
        if let Some(handled) = workspace_tool_call(id.clone(), name, params) {
            return handled;
        }
        match name {
            "add_project_source" => acted(
                id,
                BridgeAction::AddProjectSource {
                    path: optional_argument(params, "path"),
                    remote: optional_argument(params, "remote"),
                    name: optional_argument(params, "name"),
                    base_branch: optional_argument(params, "base_branch"),
                },
            ),
            "remove_project_source" => match required_argument(params, "source_id") {
                Ok(source_id) => acted(id, BridgeAction::RemoveProjectSource { source_id }),
                Err(message) => refused(id, message),
            },
            "reclaim_workspace" => match required_argument(params, "workspace_id") {
                Ok(workspace_id) => acted(id, BridgeAction::ReclaimWorkspace { workspace_id }),
                Err(message) => refused(id, message),
            },
            "message_agent" => message_agent_action(id, params),
            "search_conversation" => search_action(id, params),
            "set_topic" => topic_action(id, params),
            "set_name" => name_action(id, params),
            "compact_agent" => compaction::compact_agent_action(id, params),
            "compact_self" => compaction::compact_self_action(id, params),
            "post_thread_message" => project_message(id, params),
            other => refused(id, unknown_tool(McpSurface::Project, other)),
        }
    }

    /// The coding surface's `tools/call`: its conversation, and the workspace
    /// tools it shares with the project surface.
    fn handle_coding_tools_call(&self, id: Value, name: &str, params: Option<&Value>) -> Handled {
        // The tracker first, then the workspace tools: both are shared by
        // the two working surfaces, so each is parsed in one place.
        if let Some(handled) = Self::handle_task_tools_call(&id, name, params) {
            return handled;
        }
        if let Some(handled) = workspace_tool_call(id.clone(), name, params) {
            return handled;
        }
        if name == "search_conversation" {
            return search_action(id, params);
        }
        if name == "set_topic" {
            return topic_action(id, params);
        }
        if name == "set_name" {
            return name_action(id, params);
        }
        if name == "compact_self" {
            return compaction::compact_self_action(id, params);
        }
        if name == "message_agent" {
            return message_agent_action(id, params);
        }
        if name == "post_thread_message" {
            let arguments = params
                .and_then(|p| p.get("arguments"))
                .cloned()
                .unwrap_or(Value::Null);
            let status: MessageStatus =
                match arguments.get("status").cloned().map(serde_json::from_value) {
                    Some(Ok(status)) => status,
                    Some(Err(error)) => {
                        return Handled {
                            reply: Some(tool_error(id, format!("invalid status: {error}"))),
                            ..Handled::default()
                        }
                    }
                    None => {
                        return Handled {
                            reply: Some(tool_error(id, "status is required".to_string())),
                            ..Handled::default()
                        }
                    }
                };
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
            let report = status
                .report()
                .map(|status| DoneReport::new(status, body.to_string()));
            return Handled {
                action: Some(BridgeAction::PostThreadMessage {
                    body: body.to_string(),
                    options,
                    still_working: status == MessageStatus::Working,
                }),
                action_id: Some(id),
                report,
                ..Handled::default()
            };
        }
        Handled {
            reply: Some(tool_error(id, unknown_tool(McpSurface::Coding, name))),
            ..Handled::default()
        }
    }

    /// The router surface's `tools/call`. Router reads and mutations are daemon
    /// actions; terminal messages also emit a lifecycle report after posting.
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
            "dispatch_branch" => required("project_id").and_then(|project_id| {
                Ok(BridgeAction::DispatchBranch {
                    project_id,
                    branch: text("branch"),
                    name: required_created_agent_name(params, "name")?,
                    instruction: required("instruction")?,
                    rationale: text("rationale"),
                })
            }),
            "ask_user" => required("question").and_then(|question| {
                let options = ask_options(&arguments)?;
                if options
                    .iter()
                    .any(|option| option.kind == Some(crate::capture::CaptureTarget::Task))
                {
                    return Err("router task destinations have been retired".to_string());
                }
                Ok(BridgeAction::AskUser { question, options })
            }),
            "post_thread_message" => {
                let args = match serde_json::from_value::<SendMessageArgs>(arguments) {
                    Ok(args) => args,
                    Err(error) => {
                        return Handled {
                            reply: Some(tool_error(
                                id,
                                format!("invalid message arguments: {error}"),
                            )),
                            ..Handled::default()
                        }
                    }
                };
                if args.body.trim().is_empty() {
                    return Handled {
                        reply: Some(tool_error(id, "body is required".to_string())),
                        ..Handled::default()
                    };
                }
                let Some(status) = args.status.report() else {
                    return Handled {
                        action: Some(BridgeAction::RouterMessage {
                            body: args.body,
                            waiting: args.status == MessageStatus::Waiting,
                        }),
                        action_id: Some(id),
                        ..Handled::default()
                    };
                };
                let report = DoneReport::new(status, args.body);
                return Handled {
                    reply: Some(tool_ok(id, &report.summary)),
                    report: Some(report),
                    ..Handled::default()
                };
            }
            other => Err(unknown_tool(McpSurface::Router, other)),
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
            let mut action_succeeded = true;
            let mut posted_message_id = None;
            if let (Some(id), Some(action)) = (handled.action_id, handled.action) {
                let reply = match on_action(action) {
                    Ok(value) => {
                        posted_message_id = value
                            .get("message_id")
                            .and_then(Value::as_str)
                            .map(str::to_string);
                        tool_ok(id, &value.to_string())
                    }
                    Err(message) => {
                        action_succeeded = false;
                        tool_error(id, message)
                    }
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
            if action_succeeded {
                if let Some(report) = handled.report {
                    let mut report = report;
                    report.message_id = posted_message_id;
                    on_report(report);
                }
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
/// A tool call that became the action the daemon will run.
fn acted(id: Value, action: BridgeAction) -> Handled {
    Handled {
        action: Some(action),
        action_id: Some(id),
        ..Handled::default()
    }
}

/// A tool call the parser refused, with the reason the agent can act on.
/// What a call to a tool this surface does not have is told. An agent resumed
/// across the task rename (#190) remembers the tools by their old names: when
/// the surface has the tool under its new name, the refusal says so.
fn unknown_tool(surface: McpSurface, name: &str) -> String {
    let renamed = name
        .strip_suffix("_issues")
        .map(|stem| format!("{stem}_tasks"))
        .or_else(|| {
            name.strip_suffix("_issue")
                .map(|stem| format!("{stem}_task"))
        })
        .filter(|now| DoneServer::tool_names_of(surface).contains(now));
    match renamed {
        Some(now) => format!("unknown tool: {name}; it is now called {now}"),
        None => format!("unknown tool: {name}"),
    }
}

fn refused(id: Value, message: impl Into<String>) -> Handled {
    Handled {
        reply: Some(tool_error(id, message.into())),
        ..Handled::default()
    }
}

/// One optional string argument, trimmed. Blank reads as absent, because a
/// harness filling a schema in reaches for "" long before it omits a key.
fn optional_argument(params: Option<&Value>, field: &str) -> Option<String> {
    params
        .and_then(|p| p.get("arguments"))
        .and_then(|arguments| arguments.get(field))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// One required string argument, trimmed, or why the call cannot be made.
fn required_argument(params: Option<&Value>, field: &str) -> Result<String, String> {
    params
        .and_then(|p| p.get("arguments"))
        .and_then(|arguments| arguments.get(field))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("{field} is required"))
}

/// Agent-originated creation always names its agent. The wire's `agent.add`
/// and `tasks.assign` remain optional so a user can create an unnamed agent.
fn created_agent_name(name: Option<&str>) -> Result<String, String> {
    let name = name
        .filter(|name| !name.trim().is_empty())
        .ok_or("Build cannot start an agent without a name.")?;
    crate::agent::agent_name_from(name)
}

fn required_created_agent_name(params: Option<&Value>, field: &str) -> Result<String, String> {
    created_agent_name(
        params
            .and_then(|p| p.get("arguments"))
            .and_then(|arguments| arguments.get(field))
            .and_then(Value::as_str),
    )
}

fn require_created_agent_name(assignee: &Value) -> Result<(), String> {
    if matches!(
        assignee.get("kind").and_then(Value::as_str),
        Some("new_workspace" | "new_agent")
    ) {
        created_agent_name(assignee.get("agent_name").and_then(Value::as_str))?;
    }
    Ok(())
}

/// One workspace tool call, on whichever surface asked for it. `None` is "not
/// one of theirs", which is how each surface goes on to its own tools.
///
/// The surfaces share this parser so that a tool a surface gains is parsed the
/// one way: the arguments a workspace tool takes, and the refusals for the ones
/// it was not given, cannot come out different depending on who called.
fn workspace_tool_call(id: Value, name: &str, params: Option<&Value>) -> Option<Handled> {
    Some(match workspace_tool_action(name, params)? {
        Ok(action) => acted(id, action),
        Err(message) => refused(id, message),
    })
}

/// The typed action behind one workspace tool name, or the argument the call is
/// missing. `None` when the name is not a workspace tool at all.
fn workspace_tool_action(
    name: &str,
    params: Option<&Value>,
) -> Option<Result<BridgeAction, String>> {
    let parsed = match name {
        "list_workspaces" => Ok(BridgeAction::ListWorkspaces),
        "list_harnesses" => Ok(BridgeAction::ListHarnesses),
        "list_workspace_agents" => required_argument(params, "workspace_id")
            .map(|workspace_id| BridgeAction::ListWorkspaceAgents { workspace_id }),
        "create_workspace" => {
            required_argument(params, "name").map(|name| BridgeAction::CreateWorkspace {
                name,
                isolation: optional_argument(params, "isolation"),
            })
        }
        "add_workspace_agent" => {
            required_argument(params, "workspace_id").and_then(|workspace_id| {
                let name = required_created_agent_name(params, "name")?;
                Ok(BridgeAction::AddWorkspaceAgent {
                    workspace_id,
                    notify_user: optional_flag(params, "notify_user"),
                    harness: optional_argument(params, "harness"),
                    model: optional_argument(params, "model"),
                    effort: optional_argument(params, "effort"),
                    name: Some(name),
                    role: optional_argument(params, "role"),
                    capability: optional_argument(params, "capability"),
                })
            })
        }
        "remove_workspace_agent" => {
            required_argument(params, "workspace_id").and_then(|workspace_id| {
                Ok(BridgeAction::RemoveWorkspaceAgent {
                    workspace_id,
                    agent_id: required_argument(params, "agent_id")?,
                })
            })
        }
        "message_workspace_agent" => {
            required_argument(params, "workspace_id").and_then(|workspace_id| {
                Ok(BridgeAction::MessageWorkspaceAgent {
                    workspace_id,
                    agent_id: optional_argument(params, "agent_id"),
                    body: required_argument(params, "body")?,
                })
            })
        }
        "delete_workspace" => required_argument(params, "workspace_id")
            .map(|workspace_id| BridgeAction::DeleteWorkspace { workspace_id }),
        "add_workspace_directory" => {
            required_argument(params, "workspace_id").map(|workspace_id| {
                BridgeAction::AddWorkspaceDirectory {
                    workspace_id,
                    source_id: optional_argument(params, "source_id"),
                    path: optional_argument(params, "path"),
                    remote: optional_argument(params, "remote"),
                    name: optional_argument(params, "name"),
                }
            })
        }
        "remove_workspace_directory" => {
            required_argument(params, "workspace_id").and_then(|workspace_id| {
                Ok(BridgeAction::RemoveWorkspaceDirectory {
                    workspace_id,
                    directory_id: required_argument(params, "directory_id")?,
                })
            })
        }
        _ => return None,
    };
    Some(parsed)
}

// ------------------------------------------------------ task tracker ---
// Three argument readers the tracker's tools need and the ones above do not:
// a whole value, a list of words, and a list of typed references.

/// One argument whole, for a tool whose argument is not a string.
/// One optional boolean argument. Absent and a non-boolean are both `None`,
/// so the caller's own default stands: a flag nobody set is not a flag set to
/// false.
fn optional_flag(params: Option<&Value>, field: &str) -> Option<bool> {
    params
        .and_then(|p| p.get("arguments"))
        .and_then(|arguments| arguments.get(field))
        .and_then(Value::as_bool)
}

fn argument(params: Option<&Value>, field: &str) -> Option<Value> {
    params
        .and_then(|p| p.get("arguments"))
        .and_then(|arguments| arguments.get(field))
        .cloned()
}

/// A list of words, with anything that is not one dropped. Absent is empty:
/// a tool that was given no labels was given no labels.
/// A list of objects an argument carries through untouched — the attachment
/// descriptors, which the tracker resolves and re-reads from disk. Nothing is
/// validated here: a path is a fence question, and the fence is where the file
/// is opened.
fn value_list_argument(params: Option<&Value>, field: &str) -> Vec<Value> {
    argument(params, field)
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.is_object())
        .collect()
}

fn string_list_argument(params: Option<&Value>, field: &str) -> Vec<String> {
    argument(params, field)
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// Unlike older list arguments, a malformed label list must be refused rather
/// than silently dropping a requested change.
fn label_list_argument(params: Option<&Value>, field: &str) -> Result<Vec<String>, String> {
    match argument(params, field) {
        None => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .into_iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("{field} labels must be strings"))
            })
            .collect(),
        Some(_) => Err(format!("{field} labels must be an array")),
    }
}

/// A comment's typed references, parsed here so a malformed one refuses the
/// call rather than reaching the daemon as an empty list.
fn task_refs(params: Option<&Value>) -> Result<Vec<crate::thread::ThreadLink>, String> {
    match argument(params, "refs") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(values)) => values
            .into_iter()
            .map(|value| serde_json::from_value(value).map_err(|error| format!("refs: {error}")))
            .collect(),
        Some(_) => Err("refs must be an array".to_string()),
    }
}

/// `search_conversation`, on every surface that has a conversation.
fn search_action(id: Value, params: Option<&Value>) -> Handled {
    let arguments = params
        .and_then(|p| p.get("arguments"))
        .cloned()
        .unwrap_or(Value::Null);
    match conversation_query(&arguments) {
        Ok(query) => acted(id, BridgeAction::SearchConversation { query }),
        Err(message) => refused(id, message),
    }
}

/// `message_agent`, on every surface that has a conversation.
fn message_agent_action(id: Value, params: Option<&Value>) -> Handled {
    match required_argument(params, "agent_id")
        .and_then(|agent_id| Ok((agent_id, required_argument(params, "body")?)))
    {
        Ok((agent_id, body)) => acted(id, BridgeAction::MessageAgent { agent_id, body }),
        Err(message) => refused(id, message),
    }
}

/// `set_topic`, on every surface that has a conversation.
fn topic_action(id: Value, params: Option<&Value>) -> Handled {
    let topic = params
        .and_then(|p| p.get("arguments"))
        .and_then(|arguments| arguments.get("topic"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    match normalized_topic(topic) {
        Ok(topic) => acted(id, BridgeAction::SetTopic { topic }),
        Err(message) => refused(id, message),
    }
}

/// `set_name`, on every surface that has a conversation. Shape is checked
/// here; whether the name is already taken is the daemon's, because only it
/// knows who else is on the conversation.
fn name_action(id: Value, params: Option<&Value>) -> Handled {
    let name = params
        .and_then(|p| p.get("arguments"))
        .and_then(|arguments| arguments.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    match crate::agent::agent_name_from(name) {
        Ok(name) => acted(id, BridgeAction::SetName { name }),
        Err(message) => refused(id, message),
    }
}

/// A project agent's message: a status and a sentence, and no phase to report.
/// Complete and Blocked end the turn the way they do everywhere else; they end
/// no phase, because a project agent runs none.
fn project_message(id: Value, params: Option<&Value>) -> Handled {
    let arguments = params
        .and_then(|p| p.get("arguments"))
        .cloned()
        .unwrap_or(Value::Null);
    let status = match arguments
        .get("status")
        .cloned()
        .map(serde_json::from_value::<MessageStatus>)
    {
        Some(Ok(status)) => status,
        Some(Err(error)) => return refused(id, format!("invalid status: {error}")),
        None => return refused(id, "status is required"),
    };
    let body = arguments
        .get("body")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|body| !body.is_empty());
    let Some(body) = body else {
        return refused(id, "body is required");
    };
    acted(
        id,
        BridgeAction::PostThreadMessage {
            still_working: status == MessageStatus::Working,
            body: body.to_string(),
            options: Vec::new(),
        },
    )
}

const SUMMARY_DESCRIPTION: &str = concat!("The full report of this turn, in markdown, written for a reviewer who will not open the activity log. Lead with the outcome in one sentence, then say what changed and where (the files that carry it and why), how you verified it and what you could not, the decisions a reviewer would otherwise have to reverse-engineer, and what you deliberately left out or that remains at risk. Leave a heading out rather than pad it. If blocked or failed, lead with what is needed instead.", reference_shapes_note!());

/// What `set_topic` says about itself on every `tools/list`. The cold prompt
/// asks for the call; this is what is still in context when the agent makes
/// it, so it carries the shape rule itself.
/// The one tool a session with no memory of the work needs to know exists, so
/// the description says what to do INSTEAD of scrolling: ask a question.
const MESSAGE_AGENT_DESCRIPTION: &str = "Say something to another agent working this project: a question for whoever is on the piece you depend on, or work to hand over. It arrives knowing you sent it and not the user, and when that agent finishes the turn its report comes back to you as a message. You cannot message yourself, and no agent outside this project is reachable. This talks to an agent; post_thread_message talks to the user, and is still the only thing the user sees.";

const SEARCH_CONVERSATION_DESCRIPTION: &str = "Search your Build conversation history — every past message and event, including the ones from sessions before yours. Use it whenever you need context you do not have: what was decided about a file, why a commit was made, what the reviewer already asked for. Search rather than replay: never scroll the terminal or re-read the whole conversation to find something. Filters combine, results are newest first, and each hit is an excerpt with its sequence number, not the full item.";

const SET_NAME_DESCRIPTION: &str = "Say what to call you: one or two meaningful words for what you are working on — \"Tracker\", \"Rail scroll\", \"Transport\". This is your NAME, not your topic: the topic is what you are doing now and changes with the work, the name is who you are and replaces \"Agent 1\" in the rail, in every message you send and everywhere else you are named. Pick something the user would recognise from across a list of agents, and set it once.";

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
    fn tools_list_exposes_message_search_and_topic_tools() {
        let h = server().handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let v = parse(&h.reply.unwrap());
        let tools = v["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 6 + WORKSPACE_TOOLS.len() + TASK_TOOLS.len());
        assert_eq!(tools[0]["name"], "post_thread_message");
        assert_eq!(tools[1]["name"], "message_agent");
        assert_eq!(tools[2]["name"], "search_conversation");
        assert_eq!(tools[3]["name"], "set_topic");
        assert_eq!(tools[4]["name"], "compact_self");
        assert_eq!(tools[5]["name"], "set_name");
        assert_eq!(
            tools[0]["inputSchema"]["properties"]["status"]["enum"],
            json!(["Complete", "Blocked", "Waiting", "Working"])
        );
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
        let description = v["result"]["tools"][0]["description"].as_str().unwrap();
        let lowered = description.to_lowercase();
        assert!(
            lowered.contains("only way the user can see")
                && lowered.contains("always call it once"),
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

    /// The message body IS the report: the one thing the reviewer reads, so the
    /// schema asks for the whole of it rather than a sentence with a card
    /// of lists under it. And it is the only place a report is asked for.
    #[test]
    fn message_body_schema_asks_for_the_full_report_and_nothing_else_does() {
        let h = server().handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let v = parse(&h.reply.unwrap());
        let schema = &v["result"]["tools"][0]["inputSchema"];
        let desc = schema["properties"]["body"]["description"]
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
    }

    /// An agent on an older prompt still sends a phase, structured outputs,
    /// an anchor or links. They are ignored rather than refused: the message
    /// they ride is a real outcome, and nothing of them reaches the record.
    #[test]
    fn a_message_still_carrying_retired_fields_is_accepted_without_them() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":25,"method":"tools/call","params":{"name":"post_thread_message","arguments":{"phase":"plan","status":"Complete","body":"addressed the notes","outputs":{"plan_path":".build/plan.md","completion_report":{"decisions":["kept the old name"]}},"anchor":{"artifact":"diff"},"links":[{"kind":"file","path":"src/lib.rs"}]}}}"#,
        );
        let report = h.report.expect("the outcome is kept");
        assert_eq!(
            report,
            DoneReport::new(DoneStatus::Completed, "addressed the notes")
        );
        assert!(matches!(
            h.action,
            Some(BridgeAction::PostThreadMessage { ref body, still_working: false, .. })
                if body == "addressed the notes"
        ));
    }

    #[test]
    fn a_blocked_message_reports_blocked() {
        let h = server().handle_message(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"post_thread_message","arguments":{"status":"Blocked","body":"missing credentials"}}}"#,
        );
        assert!(
            h.reply.is_none(),
            "a posted message is completed by the daemon"
        );
        assert_eq!(
            h.report.unwrap(),
            DoneReport::new(DoneStatus::Blocked, "missing credentials")
        );
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

    /// The whole coding schema: what the agent says, how its turn stands, and
    /// the choices it offers. Nothing about phases, outputs, anchors or links.
    #[test]
    fn the_message_schema_carries_status_body_and_options_only() {
        let h = server().handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let v = parse(&h.reply.unwrap());
        let schema = &v["result"]["tools"][0]["inputSchema"];
        let mut fields: Vec<&str> = schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        fields.sort_unstable();
        assert_eq!(fields, ["body", "options", "status"]);
        assert_eq!(schema["required"], json!(["status", "body"]));
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
    fn retired_tools_are_rejected_and_reply_emits_a_scoped_bridge_action() {
        let read = server().handle_message(
            r#"{"jsonrpc":"2.0","id":21,"method":"tools/call","params":{"name":"read_unread_messages","arguments":{}}}"#,
        );
        assert!(read.action.is_none());
        assert_eq!(parse(&read.reply.unwrap())["result"]["isError"], true);

        let done = server().handle_message(
            r#"{"jsonrpc":"2.0","id":23,"method":"tools/call","params":{"name":"done","arguments":{}}}"#,
        );
        assert!(done.action.is_none());
        assert_eq!(parse(&done.reply.unwrap())["result"]["isError"], true);

        let post = server().handle_message(
            r#"{"jsonrpc":"2.0","id":22,"method":"tools/call","params":{"name":"post_thread_message","arguments":{"status":"Waiting","body":"Which name should I use?"}}}"#,
        );
        assert!(matches!(
            post.action,
            Some(BridgeAction::PostThreadMessage { ref body, .. })
                if body == "Which name should I use?"
        ));
        assert!(post.reply.is_none());
    }

    #[test]
    fn complete_message_posts_and_reports_completion() {
        let sent = server().handle_message(
            r#"{"jsonrpc":"2.0","id":24,"method":"tools/call","params":{"name":"post_thread_message","arguments":{"status":"Complete","body":"Shipped it."}}}"#,
        );

        assert!(matches!(
            sent.action,
            Some(BridgeAction::PostThreadMessage { ref body, still_working: false, .. })
                if body == "Shipped it."
        ));
        let report = sent.report.expect("Complete advances the lifecycle");
        assert_eq!(report.status, DoneStatus::Completed);
        assert_eq!(report.summary, "Shipped it.");
    }

    #[test]
    fn working_message_keeps_working_without_completing() {
        let sent = server().handle_message(
            r#"{"jsonrpc":"2.0","id":24,"method":"tools/call","params":{"name":"post_thread_message","arguments":{"status":"Working","body":"Running the migration tests."}}}"#,
        );

        assert!(matches!(
            sent.action,
            Some(BridgeAction::PostThreadMessage {
                still_working: true,
                ..
            })
        ));
        assert!(sent.report.is_none());
    }

    #[test]
    fn post_thread_message_numbers_the_actions_it_suggests() {
        let post = server().handle_message(
            r#"{"jsonrpc":"2.0","id":25,"method":"tools/call","params":{"name":"post_thread_message","arguments":{"status":"Waiting","body":"Which way?","options":[{"label":"Revert it","message":"Revert the commit that turned the tests red."},{"label":"  Fix forward  "}]}}}"#,
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
        assert_eq!(
            DoneServer::for_owner("project-01H").surface(),
            McpSurface::Project
        );
        assert_eq!(DoneServer::new("router-abc").surface(), McpSurface::Coding);
    }

    /// The workspace tools are the ONE inventory shared between two surfaces:
    /// the same names in the same order on the coding surface and the project
    /// one, because both agents are bound to a project. Everything else is a
    /// surface's own — a coding agent never sees a router tool, a router never
    /// sees a coding one, and the two verbs that change what a project is made
    /// of are the project agent's alone.
    const WORKSPACE_TOOLS: [&str; 10] = [
        "list_workspaces",
        "list_workspace_agents",
        "create_workspace",
        "add_workspace_agent",
        "list_harnesses",
        "remove_workspace_agent",
        "message_workspace_agent",
        "delete_workspace",
        "add_workspace_directory",
        "remove_workspace_directory",
    ];

    /// The task tracker's twelve, the OTHER inventory shared between the two
    /// working surfaces — and for the same reason: both agents are bound to a
    /// project, and a project has one board.
    const TASK_TOOLS: [&str; 12] = [
        "list_tasks",
        "get_task",
        "read_comment",
        "create_task",
        "comment_task",
        "assign_task",
        "move_task",
        "label_task",
        "close_task",
        "link_task",
        "track_task",
        "untrack_task",
    ];

    /// Every body a person reads says how to link a Build thing in it (#229):
    /// an agent writing a message, a comment or a task sees the shapes where
    /// it is writing, whatever prompt it was started on.
    #[test]
    fn every_readable_body_teaches_the_reference_shapes() {
        for owner in ["agent-01H", "project-01H"] {
            let tools = DoneServer::for_owner(owner).tools();
            let tools = tools.as_array().unwrap();
            let mut taught = 0;
            for tool in tools {
                let name = tool["name"].as_str().unwrap();
                if !READABLE_BODIES.contains(&name) {
                    continue;
                }
                let body = tool["inputSchema"]["properties"]["body"]["description"]
                    .as_str()
                    .unwrap();
                assert!(
                    body.ends_with(REFERENCE_SHAPES_NOTE),
                    "{owner} {name}: {body}"
                );
                taught += 1;
            }
            assert!(taught >= 4, "{owner} taught only {taught} bodies");
        }
    }

    /// The shapes named once, so the sentence cannot drift from what
    /// spa/src/core/markdownRefs.js parses.
    #[test]
    fn the_reference_shapes_note_names_every_shape() {
        for shape in [
            "`#42`",
            "`#42/c/<comment-id>`",
            "`@agent:<agent-id>`",
            "`@workspace:<name or id>`",
            "`@project:<name or id>`",
            "`[[<workspace>:<path>#L10]]`",
            "`[[<workspace>:commit:<sha>]]`",
        ] {
            assert!(REFERENCE_SHAPES_NOTE.contains(shape), "{shape}");
        }
    }

    /// `create_task` files a card on the board, not an entry in the harness's
    /// own task list (#190): the description says so first.
    #[test]
    fn create_task_says_it_files_a_build_task_on_the_board() {
        for owner in ["agent-01H", "project-01H"] {
            let tools = DoneServer::for_owner(owner).tools();
            let create = tools
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == "create_task")
                .unwrap();
            let description = create["description"].as_str().unwrap();
            assert!(
                description.starts_with(
                    "File a Build task: a card on your project's board, not an entry in your \
                     harness's own task or todo list."
                ),
                "{description}"
            );
        }
    }

    #[test]
    fn both_task_surfaces_explain_how_a_new_task_asks_the_user() {
        for owner in ["agent-01H", "project-01H"] {
            let tools = DoneServer::for_owner(owner).tools();
            let tools = tools.as_array().unwrap();
            let create = tools
                .iter()
                .find(|tool| tool["name"] == "create_task")
                .unwrap();
            let comment = tools
                .iter()
                .find(|tool| tool["name"] == "comment_task")
                .unwrap();
            let properties = &create["inputSchema"]["properties"];
            assert_eq!(properties["mention_user"]["type"], "boolean");
            assert!(properties["mention_user"]["description"]
                .as_str()
                .unwrap()
                .contains("Needs you"));
            assert!(properties["notify_user"]["description"]
                .as_str()
                .unwrap()
                .contains("Watching alone does not put it in Needs you"));
            let assign = tools
                .iter()
                .find(|tool| tool["name"] == "assign_task")
                .unwrap();
            assert!(
                assign["inputSchema"]["properties"]["notify_user"]["description"]
                    .as_str()
                    .unwrap()
                    .contains("Watching alone does not put it in Needs you")
            );
            assert!(
                comment["inputSchema"]["properties"]["notify_user"]["description"]
                    .as_str()
                    .unwrap()
                    .contains("unread comment puts it in Needs you")
            );
            assert!(
                comment["inputSchema"]["properties"]["mention_user"]["description"]
                    .as_str()
                    .unwrap()
                    .contains("until they read")
            );
        }
    }

    /// Every write the tracker offers takes `track`, so following a task is
    /// never a second call — and `create_task` says its default is the other
    /// way round, because a flag that defaults differently in one place has to
    /// say so where it is read.
    #[test]
    fn every_task_write_offers_the_track_flag() {
        let tools = server().tools().as_array().unwrap().clone();
        let named = |name: &str| {
            tools
                .iter()
                .find(|tool| tool["name"] == name)
                .unwrap_or_else(|| panic!("{name} is offered"))
                .clone()
        };
        for name in [
            "create_task",
            "comment_task",
            "assign_task",
            "move_task",
            "label_task",
            "close_task",
            "link_task",
        ] {
            let tool = named(name);
            assert_eq!(
                tool["inputSchema"]["properties"]["track"]["type"], "boolean",
                "{name} does not offer it"
            );
            assert!(
                !tool["inputSchema"]["required"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("track")),
                "{name} must not require it"
            );
        }
        let filing = named("create_task")["inputSchema"]["properties"]["track"]["description"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(filing.contains("Defaults to TRUE"), "{filing}");
        let moving = named("move_task")["inputSchema"]["properties"]["track"]["description"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(moving.contains("Defaults to false"), "{moving}");

        // The reads take no such flag: nothing to follow is written by them.
        for name in ["list_tasks", "get_task"] {
            assert!(
                named(name)["inputSchema"]["properties"]["track"].is_null(),
                "{name} offers a flag it cannot honour"
            );
        }
    }

    /// `link_task` says what assignment does NOT do for you, so an agent that
    /// read "assignment links your workspace" does not conclude the tool is
    /// redundant and leave every commit unlinked.
    #[test]
    fn link_task_says_what_assignment_leaves_for_it() {
        let described = server()
            .tools()
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "link_task")
            .expect("link_task is offered")
            .clone();
        let description = described["description"].as_str().unwrap();
        assert!(
            description.contains("Assigning a task already links"),
            "{description}"
        );
        assert!(
            description.contains("each commit that lands for it"),
            "{description}"
        );
    }

    #[test]
    fn every_surface_advertises_its_exact_tool_inventory() {
        assert_eq!(
            tool_names(&router()),
            vec![
                "list_projects",
                "list_work",
                "read_conversation",
                "dispatch_branch",
                "ask_user",
                "post_thread_message",
            ]
        );
        let conversation = [
            "post_thread_message",
            "message_agent",
            "search_conversation",
            "set_topic",
            "compact_self",
        ];
        assert_eq!(
            tool_names(&server()),
            [
                &conversation[..],
                // A coding agent names ITSELF; the project's agent is named by
                // its project and is offered no such tool.
                &["set_name"][..],
                &WORKSPACE_TOOLS[..],
                &TASK_TOOLS[..]
            ]
            .concat(),
            "a coding agent has its conversation, the workspaces of its project, and its board"
        );
        assert_eq!(
            tool_names(&project()),
            [
                &WORKSPACE_TOOLS[..],
                &[
                    "add_project_source",
                    "remove_project_source",
                    "reclaim_workspace",
                    "compact_agent"
                ][..],
                &conversation[..],
                &TASK_TOOLS[..],
            ]
            .concat(),
            "and the project agent has the same three, plus the project's own sources"
        );
        for surface in [McpSurface::Coding, McpSurface::Project] {
            for tool in WORKSPACE_TOOLS.iter().chain(TASK_TOOLS.iter()) {
                assert!(
                    DoneServer::tool_names_of(surface).contains(&tool.to_string()),
                    "{tool} missing from {surface:?}"
                );
            }
        }
        for project_only in [
            "add_project_source",
            "remove_project_source",
            "reclaim_workspace",
            "compact_agent",
        ] {
            assert!(
                !tool_names(&server()).contains(&project_only.to_string()),
                "{project_only} is the project agent's alone"
            );
        }
        // The router works no project, so it carries no board. `create_task`
        // is the exception it always was: the router's own, and the plan
        // flow's, sharing a name on a surface nothing else here reaches.
        for tool in TASK_TOOLS.iter().filter(|tool| **tool != "create_task") {
            assert!(
                !tool_names(&router()).contains(&tool.to_string()),
                "the router is shown {tool}"
            );
        }
    }

    /// The two tools that hand out a checkout say, in the description itself,
    /// that this is how a checkout is obtained in Build and that git's own
    /// worktrees are not.
    ///
    /// The coding prompt says it too (`WORKSPACE_NOTE` in `templates.rs`), and
    /// that is the point of saying it twice: a description is re-sent on every
    /// tools/list, so it is still in front of the agent after the compaction
    /// that ate the cold prompt — which is exactly when an agent reaches for
    /// the worktree command it already knows.
    #[test]
    fn the_checkout_tools_forbid_git_worktree_in_the_description_itself() {
        for surface in [&server(), &project()] {
            let listed =
                surface.handle_message(r#"{"jsonrpc":"2.0","id":74,"method":"tools/list"}"#);
            let value = parse(&listed.reply.unwrap());
            for tool in ["create_workspace", "add_workspace_directory"] {
                let description = value["result"]["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|listed| listed["name"] == tool)
                    .unwrap_or_else(|| panic!("{tool} is advertised"))["description"]
                    .as_str()
                    .unwrap()
                    .to_string();
                assert!(
                    description.contains("how a separate checkout is obtained in Build"),
                    "{tool} does not say where a checkout comes from: {description}"
                );
                assert!(
                    description.contains("never `git worktree add`"),
                    "{tool} does not forbid git's own worktrees: {description}"
                );
            }
        }
    }

    /// The three task tools whose descriptions had to change say, in the
    /// description itself, when to reach for them: file and assign rather than
    /// message, file and self-assign to plan your own work, and ask on the
    /// task you were handed.
    ///
    /// Said here as well as in the prompt for the reason the checkout tools say
    /// their rule twice — a description survives the compaction that eats a
    /// cold prompt, and the moment an agent reaches for a message instead of a
    /// task is long after that prompt is gone.
    #[test]
    fn the_task_tools_say_when_to_reach_for_them_in_the_description_itself() {
        for surface in [&server(), &project()] {
            let listed =
                surface.handle_message(r#"{"jsonrpc":"2.0","id":75,"method":"tools/list"}"#);
            let value = parse(&listed.reply.unwrap());
            let described = |tool: &str| -> String {
                value["result"]["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|listed| listed["name"] == tool)
                    .unwrap_or_else(|| panic!("{tool} is advertised"))["description"]
                    .as_str()
                    .unwrap()
                    .to_string()
            };
            for (tool, said) in [
                // Rule 1: the threshold and the reason.
                (
                    "assign_task",
                    "anything beyond a quick question or a one-line correction is filed and assigned rather than sent as a message",
                ),
                // Rule 2: your own multi-step work is a task too.
                (
                    "create_task",
                    "work you ARE doing that runs to more than one step, filed and assigned to yourself",
                ),
                // Rule 3: the task is where you ask, not only where you report.
                (
                    "comment_task",
                    "a question about a task that came from outside your conversation goes here",
                ),
            ] {
                let description = described(tool);
                assert!(
                    description.contains(said),
                    "{tool} no longer says, verbatim: {said}\n\nit says: {description}"
                );
            }
        }
    }

    /// A workspace tool call parses to the same action whoever sent it: one
    /// parser, so the arguments and the refusals cannot drift between surfaces.
    #[test]
    fn a_workspace_tool_parses_the_same_way_on_both_surfaces() {
        for tool in WORKSPACE_TOOLS {
            assert!(
                workspace_tool_action(tool, None).is_some(),
                "{tool} is not parsed as a workspace tool"
            );
        }
        assert!(workspace_tool_action("add_project_source", None).is_none());

        let arguments = r#"{"workspace_id":"ws-1","body":"start on the rail"}"#;
        let frame = format!(
            r#"{{"jsonrpc":"2.0","id":73,"method":"tools/call","params":{{"name":"message_workspace_agent","arguments":{arguments}}}}}"#
        );
        let from_project = project().handle_message(&frame).action;
        let from_coding = server().handle_message(&frame).action;
        assert_eq!(
            format!("{from_project:?}"),
            format!("{from_coding:?}"),
            "the surfaces disagree about what the call means"
        );
        assert!(matches!(
            from_coding,
            Some(BridgeAction::MessageWorkspaceAgent { ref workspace_id, agent_id: None, ref body })
                if workspace_id == "ws-1" && body == "start on the rail"
        ));
    }

    // ==== the project surface ===============================================

    fn project() -> DoneServer {
        DoneServer::for_owner("project-01H")
    }

    /// One project tool call, as a harness writes it.
    fn project_call(name: &str, arguments: &str) -> Handled {
        project().handle_message(&format!(
            r#"{{"jsonrpc":"2.0","id":70,"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#
        ))
    }

    /// What the surface reads of the project, and the conversation tools every
    /// agent with a conversation has.
    #[test]
    fn every_project_read_emits_its_typed_action() {
        let call = project_call;

        assert!(matches!(
            call("list_workspaces", "{}").action,
            Some(BridgeAction::ListWorkspaces)
        ));
        assert!(matches!(
            call("list_workspace_agents", r#"{"workspace_id":"ws-1"}"#).action,
            Some(BridgeAction::ListWorkspaceAgents { ref workspace_id }) if workspace_id == "ws-1"
        ));
        assert!(matches!(
            call("set_topic", r#"{"topic":"Cut a workspace"}"#).action,
            Some(BridgeAction::SetTopic { ref topic }) if topic == "Cut a workspace"
        ));
        assert!(matches!(
            call("search_conversation", r#"{"query":"workspace"}"#).action,
            Some(BridgeAction::SearchConversation { .. })
        ));
    }

    /// What the surface changes about the project.
    #[test]
    fn every_project_write_emits_its_typed_action() {
        let call = project_call;

        assert!(matches!(
            call("create_workspace", r#"{"name":"read the router","isolation":"rift"}"#).action,
            Some(BridgeAction::CreateWorkspace { ref name, ref isolation })
                if name == "read the router" && isolation.as_deref() == Some("rift")
        ));
        assert!(matches!(
            call("remove_workspace_agent", r#"{"workspace_id":"ws-1","agent_id":"agent-2"}"#).action,
            Some(BridgeAction::RemoveWorkspaceAgent { ref workspace_id, ref agent_id })
                if workspace_id == "ws-1" && agent_id == "agent-2"
        ));
        assert!(matches!(
            call("message_workspace_agent", r#"{"workspace_id":"ws-1","body":"start on the rail"}"#).action,
            Some(BridgeAction::MessageWorkspaceAgent { ref workspace_id, agent_id: None, ref body })
                if workspace_id == "ws-1" && body == "start on the rail"
        ));
    }

    /// No write names a project. A project id in the arguments is not a field
    /// of any of these actions, so it cannot travel past the parser: the owner
    /// binding is the only thing that says which project a write lands in.
    #[test]
    fn a_project_id_in_a_write_call_goes_no_further_than_the_parser() {
        let call = project_call;

        assert!(matches!(
            call("create_workspace", r#"{"name":"one","project_id":"proj-9"}"#).action,
            Some(BridgeAction::CreateWorkspace { ref name, isolation: None }) if name == "one"
        ));
        assert!(matches!(
            call("add_workspace_agent", r#"{"workspace_id":"ws-1","harness":"codex","project_id":"proj-9","name":"Flaky test fixer"}"#).action,
            Some(BridgeAction::AddWorkspaceAgent { ref workspace_id, ref harness, model: None, effort: None, name: Some(ref name), .. })
                if workspace_id == "ws-1" && harness.as_deref() == Some("codex") && name == "Flaky test fixer"
        ));
    }

    #[test]
    fn every_agent_creation_tool_requires_a_name() {
        let tools = project().tools();
        let tools = tools.as_array().unwrap();
        let named = |name: &str| tools.iter().find(|tool| tool["name"] == name).unwrap();
        assert!(named("add_workspace_agent")["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("name")));
        let assignee = &named("assign_task")["inputSchema"]["properties"]["assignee"];
        assert_eq!(
            assignee["allOf"][0]["then"]["required"],
            json!(["agent_name"])
        );
        assert_eq!(
            assignee["allOf"][1]["then"]["required"],
            json!(["agent_name"])
        );

        for (tool, args) in [
            ("add_workspace_agent", r#"{"workspace_id":"ws-1"}"#),
            (
                "add_workspace_agent",
                r#"{"workspace_id":"ws-1","name":"  "}"#,
            ),
            (
                "assign_task",
                r#"{"task_id":"task-1","assignee":{"kind":"new_agent","workspace_id":"ws-1"}}"#,
            ),
            (
                "assign_task",
                r#"{"task_id":"task-1","assignee":{"kind":"new_agent","workspace_id":"ws-1","agent_name":" "}}"#,
            ),
            (
                "assign_task",
                r#"{"task_id":"task-1","assignee":{"kind":"new_workspace","name":"workspace"}}"#,
            ),
            (
                "assign_task",
                r#"{"task_id":"task-1","assignee":{"kind":"new_workspace","agent_name":" "}}"#,
            ),
        ] {
            for server in [project(), server()] {
                let refused = server.handle_message(&format!(
                    r#"{{"jsonrpc":"2.0","id":71,"method":"tools/call","params":{{"name":"{tool}","arguments":{args}}}}}"#
                ));
                assert!(refused.action.is_none(), "{tool}: {args}");
                let reply = parse(&refused.reply.unwrap());
                assert_eq!(reply["result"]["isError"], true, "{tool}: {args}");
                assert_eq!(
                    reply["result"]["content"][0]["text"],
                    "Build cannot start an agent without a name."
                );
            }
        }
    }

    /// A call the parser cannot act on is a tool error and no action, so the
    /// daemon is never asked to guess what the agent meant.
    #[test]
    fn a_project_tool_missing_an_argument_is_refused_before_the_daemon_sees_it() {
        for (tool, arguments, why) in [
            (
                "list_workspace_agents",
                "{}",
                "a workspace read with no workspace named is a tool error",
            ),
            (
                "create_workspace",
                r#"{"project_id":"proj-9"}"#,
                "a workspace with no name is a tool error, project id or not",
            ),
            (
                "add_workspace_agent",
                r#"{"harness":"codex"}"#,
                "an agent added to no workspace is a tool error",
            ),
            (
                "remove_workspace_agent",
                r#"{"workspace_id":"ws-1"}"#,
                "removing nobody in particular is a tool error",
            ),
            (
                "message_workspace_agent",
                r#"{"workspace_id":"ws-1","agent_id":"agent-2"}"#,
                "a message with nothing in it is a tool error",
            ),
        ] {
            let missing = project_call(tool, arguments);
            assert_eq!(
                parse(&missing.reply.unwrap())["result"]["isError"],
                true,
                "{why}"
            );
            assert!(missing.action.is_none(), "{why}");
        }
    }

    /// The project agent has no phases — it builds nothing — so its message
    /// asks for none, and a terminal one reports no lifecycle outcome.
    #[test]
    fn a_project_message_carries_a_status_and_never_a_phase() {
        let h = project().handle_message(r#"{"jsonrpc":"2.0","id":71,"method":"tools/list"}"#);
        let v = parse(&h.reply.unwrap());
        let message = v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "post_thread_message")
            .expect("the project surface carries the message tool")
            .clone();
        let schema = &message["inputSchema"];
        assert!(schema["properties"]["phase"].is_null(), "{schema}");
        assert_eq!(
            schema["properties"]["status"]["enum"],
            json!(["Complete", "Blocked", "Waiting", "Working"])
        );

        let done = project().handle_message(
            r#"{"jsonrpc":"2.0","id":72,"method":"tools/call","params":{"name":"post_thread_message","arguments":{"status":"Complete","body":"two workspaces, both idle"}}}"#,
        );
        assert!(done.report.is_none(), "a project agent reports no phase");
        assert!(matches!(
            done.action,
            Some(BridgeAction::PostThreadMessage { still_working: false, ref body, .. })
                if body == "two workspaces, both idle"
        ));

        let working = project().handle_message(
            r#"{"jsonrpc":"2.0","id":73,"method":"tools/call","params":{"name":"post_thread_message","arguments":{"status":"Working","body":"reading the workspaces"}}}"#,
        );
        assert!(matches!(
            working.action,
            Some(BridgeAction::PostThreadMessage {
                still_working: true,
                ..
            })
        ));

        let empty = project().handle_message(
            r#"{"jsonrpc":"2.0","id":74,"method":"tools/call","params":{"name":"post_thread_message","arguments":{"status":"Complete","body":"  "}}}"#,
        );
        assert_eq!(parse(&empty.reply.unwrap())["result"]["isError"], true);
        assert!(empty.action.is_none());
    }

    /// The router's completion message reports the one phase a router has, so the schema
    /// cannot invite it to claim it built something.
    #[test]
    fn the_routers_completion_message_reports_only_routing() {
        let h = router().handle_message(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        let v = parse(&h.reply.unwrap());
        let schema = &v["result"]["tools"][5]["inputSchema"];
        assert!(schema["properties"]["phase"].is_null(), "{schema}");
        assert_eq!(
            schema["properties"]["status"]["enum"],
            json!(["Complete", "Blocked", "Waiting", "Working"])
        );

        let routed = router().handle_message(
            r#"{"jsonrpc":"2.0","id":40,"method":"tools/call","params":{"name":"post_thread_message","arguments":{"status":"Complete","body":"filed a task on the bridge"}}}"#,
        );
        let report = routed.report.expect("a routing report");
        assert_eq!(
            report,
            DoneReport::new(DoneStatus::Completed, "filed a task on the bridge")
        );
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
            call("dispatch_branch", r#"{"project_id":"proj-1","branch":"build/login","name":"Branch Worker","instruction":"finish the toast"}"#).action,
            Some(BridgeAction::DispatchBranch { ref project_id, ref branch, ref name, ref instruction, rationale: None })
                if project_id == "proj-1" && branch.as_deref() == Some("build/login")
                    && name == "Branch Worker" && instruction == "finish the toast"
        ));
        for name in [None, Some(""), Some("x")] {
            let arguments = serde_json::json!({
                "project_id": "proj-1", "name": name, "instruction": "finish the toast"
            });
            let refused = router().handle_message(
                &serde_json::json!({
                    "jsonrpc": "2.0", "id": 51, "method": "tools/call",
                    "params": { "name": "dispatch_branch", "arguments": arguments }
                })
                .to_string(),
            );
            assert!(refused.action.is_none(), "{name:?}");
            assert_eq!(parse(&refused.reply.unwrap())["result"]["isError"], true);
        }
        assert!(matches!(
            call("ask_user", r#"{"question":"which project?"}"#).action,
            Some(BridgeAction::AskUser { ref question, ref options })
                if question == "which project?" && options.is_empty()
        ));
        let retired = call(
            "create_task",
            r#"{"project_id":"proj-1","goal":"fix the redirect"}"#,
        );
        assert!(retired.action.is_none());
        assert_eq!(parse(&retired.reply.unwrap())["result"]["isError"], true);
    }

    /// The options a router offers beside its question reach the daemon whole:
    /// what the user taps, and where that tap would send the capture.
    #[test]
    fn ask_user_carries_the_options_the_router_offered() {
        let handled = router().handle_message(
            r#"{"jsonrpc":"2.0","id":53,"method":"tools/call","params":{"name":"ask_user","arguments":{
                "question":"which project?",
                "options":[
                    {"label":"New branch on Build","project_id":"proj-1","kind":"branch"},
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
                    label: "New branch on Build".to_string(),
                    project_id: Some("proj-1".to_string()),
                    kind: Some(crate::capture::CaptureTarget::Branch),
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
            r#"{"question":"which?","options":[{"kind":"task"}]}"#,
            r#"{"question":"which?","options":[{"label":"file it","kind":"task"}]}"#,
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
            json!(["branch"])
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
            r#"{"jsonrpc":"2.0","id":61,"method":"tools/call","params":{"name":"search_conversation","arguments":{}}}"#,
        );
        assert_eq!(
            parse(&router_asking_for_coding.reply.unwrap())["result"]["isError"],
            true
        );
        assert!(router_asking_for_coding.action.is_none());

        for (server, tool) in [
            (project(), "dispatch_branch"),
            (project(), "list_projects"),
            (server(), "add_project_source"),
            (server(), "remove_project_source"),
            (server(), "reclaim_workspace"),
            (router(), "list_workspace_agents"),
        ] {
            let refused = server.handle_message(&format!(
                r#"{{"jsonrpc":"2.0","id":62,"method":"tools/call","params":{{"name":"{tool}","arguments":{{}}}}}}"#
            ));
            assert_eq!(
                parse(&refused.reply.unwrap())["result"]["isError"],
                true,
                "{tool}"
            );
            assert!(refused.action.is_none(), "{tool}");
        }
    }

    /// An agent resumed across the rename (#190) calls the names its
    /// transcript remembers. The refusal names the tool as it is called now,
    /// on a surface that has it, rather than leave the agent to guess.
    #[test]
    fn a_tool_from_before_tasks_were_renamed_is_answered_with_its_new_name() {
        let refusal = |server: DoneServer, name: &str| {
            let called = server.handle_message(&format!(
                r#"{{"jsonrpc":"2.0","id":64,"method":"tools/call","params":{{"name":"{name}","arguments":{{}}}}}}"#
            ));
            parse(&called.reply.expect("a refusal"))["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string()
        };
        for server in [server as fn() -> DoneServer, project] {
            assert_eq!(
                refusal(server(), "comment_issue"),
                "unknown tool: comment_issue; it is now called comment_task"
            );
            assert_eq!(
                refusal(server(), "list_issues"),
                "unknown tool: list_issues; it is now called list_tasks"
            );
            assert_eq!(refusal(server(), "open_issue"), "unknown tool: open_issue");
        }
        assert_eq!(
            refusal(router(), "comment_issue"),
            "unknown tool: comment_issue",
            "the router has no task tools to point at"
        );
    }

    /// A tool a surface lists is a tool its dispatcher routes. `set_name` sat
    /// on the coding surface's list for a whole release answering "unknown
    /// tool", because the list and the dispatcher are two places to add it.
    #[test]
    fn every_listed_tool_is_one_its_surface_dispatches() {
        for (surface, server) in [
            ("coding", server as fn() -> DoneServer),
            ("project", project),
            ("router", router),
        ] {
            for tool in tool_names(&server()) {
                let called = server().handle_message(&format!(
                    r#"{{"jsonrpc":"2.0","id":63,"method":"tools/call","params":{{"name":"{tool}","arguments":{{}}}}}}"#
                ));
                if let Some(reply) = called.reply {
                    let text = parse(&reply)["result"]["content"][0]["text"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    assert!(
                        !text.starts_with("unknown tool"),
                        "{surface} lists {tool} but does not route it"
                    );
                }
            }
        }
    }

    /// A workspace agent names itself from the coding surface, and the name
    /// is shaped there the way it is on the project surface.
    #[test]
    fn set_name_on_the_coding_surface_is_a_rename() {
        let named = server().handle_message(
            r#"{"jsonrpc":"2.0","id":64,"method":"tools/call","params":{"name":"set_name","arguments":{"name":"  Rail   scroll "}}}"#,
        );
        assert!(named.reply.is_none());
        let Some(BridgeAction::SetName { name }) = named.action else {
            panic!("expected a rename");
        };
        assert_eq!(name, "Rail scroll");

        let empty = server().handle_message(
            r#"{"jsonrpc":"2.0","id":65,"method":"tools/call","params":{"name":"set_name","arguments":{"name":""}}}"#,
        );
        assert!(empty.action.is_none());
        let refused = parse(&empty.reply.unwrap());
        assert_eq!(refused["result"]["isError"], true);
        assert!(refused["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("An agent's name needs at least"));
    }

    /// The socket enforces the same split on the frames themselves, so it needs
    /// each action to say which surface it belongs to.
    #[test]
    fn every_action_names_its_tool_and_its_surface() {
        // A conversation tool is on every surface that has a conversation, and
        // a workspace tool on every surface whose agent is bound to a project;
        // what is on exactly one surface is the rest.
        for (action, surface, allowed) in [
            (
                BridgeAction::SetTopic { topic: "T".into() },
                McpSurface::Coding,
                true,
            ),
            (
                BridgeAction::SetTopic { topic: "T".into() },
                McpSurface::Project,
                true,
            ),
            (
                BridgeAction::SetTopic { topic: "T".into() },
                McpSurface::Router,
                false,
            ),
            (BridgeAction::ListWorkspaces, McpSurface::Project, true),
            (BridgeAction::ListWorkspaces, McpSurface::Coding, true),
            (BridgeAction::ListWorkspaces, McpSurface::Router, false),
            (
                BridgeAction::AddProjectSource {
                    path: None,
                    remote: None,
                    name: None,
                    base_branch: None,
                },
                McpSurface::Project,
                true,
            ),
            (
                BridgeAction::AddProjectSource {
                    path: None,
                    remote: None,
                    name: None,
                    base_branch: None,
                },
                McpSurface::Coding,
                false,
            ),
            (BridgeAction::ListProjects, McpSurface::Project, false),
        ] {
            assert_eq!(
                action.allowed_on(surface),
                allowed,
                "{} on {}",
                action.tool_name(),
                surface.as_str()
            );
        }
        assert_eq!(BridgeAction::ListWorkspaces.tool_name(), "list_workspaces");
        assert_eq!(
            BridgeAction::ListWorkspaceAgents {
                workspace_id: "ws-1".into()
            }
            .tool_name(),
            "list_workspace_agents"
        );
        assert_eq!(BridgeAction::ListWorkspaces.surface_name(), "project");

        for (action, name, surface) in [
            (
                BridgeAction::SetTopic {
                    topic: "routing".to_string(),
                },
                "set_topic",
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
            assert!(action.allowed_on(surface), "{name}");
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
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"post_thread_message","arguments":{"status":"Complete","body":"addressed comments"}}}"#,
            "\n",
        );
        let mut out = Vec::new();
        let mut reports = Vec::new();
        server()
            .run_stdio(
                input.as_bytes(),
                &mut out,
                |r| reports.push(r),
                |_| Ok(json!({ "message_id": "m-1" })),
            )
            .unwrap();

        // One report (the completion message), and two response lines (initialize + tools/call;
        // the notification produces none).
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].summary, "addressed comments");
        assert_eq!(reports[0].message_id.as_deref(), Some("m-1"));
        let lines: Vec<&str> = std::str::from_utf8(&out).unwrap().lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn run_stdio_does_not_complete_when_the_message_cannot_be_posted() {
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"post_thread_message","arguments":{"status":"Complete","body":"finished"}}}"#,
            "\n",
        );
        let mut out = Vec::new();
        let mut reports = Vec::new();
        server()
            .run_stdio(
                input.as_bytes(),
                &mut out,
                |report| reports.push(report),
                |_| Err("invalid link".into()),
            )
            .unwrap();

        assert!(reports.is_empty());
        let reply: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(reply["result"]["isError"], true);
    }
}
