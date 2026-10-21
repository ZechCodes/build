//! Section 8 qualification, distinct from the short #709/#719 diagnostic.
//!
//! The schedule counts intended work before caller credits, and completion
//! never erases a missed offer. Long trials must use actual encrypted RPC/MCP
//! execution and the logical write-wait registry; these pure controls alone
//! do not qualify the production paths.
mod admission;
mod client_watch;
mod coverage;
mod finite_watch;
mod fixture;
mod history_fill;
mod post_reply;
mod rare;
mod read_and_exempt;
pub(super) mod read_recipes;
mod rpc;
mod saved_intent;
mod scheduler;

const SPA_CALLERS: usize = 4;
const MCP_CALLERS: usize = 8;
const CALLERS: usize = SPA_CALLERS + MCP_CALLERS;
const ACTIVE_WORKSPACES: usize = 8;
const WRITES_PER_SECOND: usize = 40;
const EXEMPT_PER_SECOND: usize = 100;
const MAX_OUTSTANDING_PER_CALLER: usize = 2;
const TRIAL_SECONDS: usize = 600;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkClass {
    State,
    FilesystemProcess,
    GitProvider,
}

impl WorkClass {
    fn external_bound_ms(self) -> u64 {
        match self {
            Self::State => 0,
            Self::FilesystemProcess => 250,
            Self::GitProvider => 500,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct WriteSlot {
    id: usize,
    due_micros: u64,
    caller: usize,
    owner: usize,
    class: WorkClass,
}
