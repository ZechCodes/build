use super::*;
#[derive(Clone, Debug)]
pub struct ActiveTarget {
    pub owner: String,
    pub agent: String,
    pub thread: String,       // actual agent.thread.id generation guard
    pub conversation: String, // actual agent.conversation_id(), Store address
    pub workspace: String,
    pub source: String,
    pub path: PathBuf,
    pub project: usize,
}
#[derive(Clone, Debug)]
pub struct ProjectTarget {
    pub project_id: String,
    // ROOT APPROVED: two preexisting ordinary cards, selected by sibling0/1.
    // Neither is one of the reserved read-witness tasks. No new cards seeded.
    pub ordinary_tasks: [String; 2],
}
#[derive(Clone, Debug)]
pub struct RootGitRefs {
    // Created and verified BEFORE timing. Both point to initial_commit and
    // each ref is unique across all eight roots (including linked worktrees).
    pub full_refs: [String; 2],
    pub initial_commit: String,
    pub initial_head_ref: String,
}
#[derive(Clone, Debug)]
pub struct CommonTargets {
    pub active: [ActiveTarget; 8],
    pub projects: [ProjectTarget; 4],
    pub git: [RootGitRefs; 8],
    // Actual seeded editor bytes, verified during setup. No timed fs.read.
    pub initial_editor_bodies: [Vec<u8>; 8],
    pub reserved_read_tasks: BTreeSet<String>,
}
#[derive(Clone, Debug)]
pub enum ActualRequest {
    Rpc { method: &'static str, params: Value },
    Mcp { agent: String, action: BridgeAction },
}
#[derive(Clone, Debug)]
pub enum ActorExpectation {
    User,
    Agent(String),
}
#[derive(Clone, Debug)]
pub enum ExpectedEffect {
    Posted {
        owner: String,
        agent: String,
        thread: String,
        body: String,
        role: &'static str,
        operation_id: Option<String>,
    },
    Seen {
        owner: String,
        agent: String,
        thread: String,
    },
    UserPresent,
    TaskStatus {
        task: String,
        status: String,
        actor: ActorExpectation,
    },
    Comment {
        task: String,
        body: String,
        actor: ActorExpectation,
    },
    Topic {
        owner: String,
        agent: String,
        topic: String,
    },
    Name {
        owner: String,
        agent: String,
        name: String,
    },
    Handoff {
        caller: String,
        recipient_owner: String,
        recipient: String,
        thread: String,
        body: String,
    },
    File {
        owner: usize,
        path: PathBuf,
        bytes: Vec<u8>,
        revision: String,
    },
    GitHead {
        owner: usize,
        directory: PathBuf,
        full_ref: String,
        commit: String,
    },
}
#[derive(Clone, Debug)]
pub struct CommonRequest {
    pub original_id: String,
    pub caller: usize,
    pub owner: usize,
    pub request: ActualRequest,
    pub effect: ExpectedEffect,
    pub durable_operation_id: Option<String>,
    // Only approved true dependencies: per-file FS and per-root Git chains.
    // State slots, including all eight same-owner burst slots, have NONE.
    pub predecessor: Option<String>,
    pub finite_target_us: Option<u64>,
    pub finite_ceiling_us: Option<u64>,
}
/// Ephemeral witnesses loaded from REAL native storage/logs. They are consumed
/// by verify_saved and reduced to bounded typed evidence, never retained whole.
pub enum SavedEffect {
    Message {
        owner: String,
        agent: String,
        thread: String,
        item: Value,
        operation_status: Option<String>,
        matched_actual_session_turn: Option<String>,
        mandatory_children_complete: bool,
    },
    Seen {
        owner: String,
        agent: String,
        thread: String,
        persisted_through: u64,
        published_through: u64,
        actual_read_target_through: u64,
    },
    UserSession {
        persisted: crate::session_summary::UserSession,
        invocation_start_ms: i64,
        decoded_final_ms: i64,
    },
    TaskVersion {
        saved: crate::tracker::Task,
        moved_event: Option<crate::tracker::TaskEvent>,
    },
    Comment(crate::tracker::TaskComment),
    Agent {
        owner: String,
        saved: crate::agent::Agent,
    },
    File {
        actual_path: PathBuf,
        actual_bytes: Vec<u8>,
    },
    GitHead {
        actual_directory: PathBuf,
        actual_symbolic_head: String,
        actual_commit: String,
    },
}
