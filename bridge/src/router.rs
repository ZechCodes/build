//! The router: the one agent that decides where a capture goes.
//!
//! Capture first, route after (spec: UX Redesign Decisions, "Capture and
//! router"). A router session is neither a plan nor a run. It owns no checkout,
//! changes no code, and lives exactly as long as one routing decision: it is
//! spawned reactively when a capture arrives, works in a bridge-owned scratch
//! directory that is deleted when it exits, and reaches Build through an MCP
//! surface of its own — the coding tools are not on it, and its tools are not on
//! theirs.
//!
//! Router = device-scoped v1: it can see every project on THIS bridge, which is
//! what "global" can honestly mean until multi-device aggregation exists.

use std::path::{Path, PathBuf};

use crate::models::{AgentProvider, ModelChoice};

/// What every router agent id starts with. The prefix is load-bearing, not
/// decoration: it is how the MCP control plane knows, from the id alone, that a
/// session gets the router's tools and not a coding agent's.
pub const ROUTER_AGENT_ID_PREFIX: &str = "router-";

/// The directory router scratch space lives under, inside Build's own state
/// directory. Never a repo checkout — a router that could reach a checkout
/// could change code, and deciding is its whole job.
pub const ROUTER_SCRATCH_DIR_NAME: &str = "router-scratch";

/// The effort a routing decision is worth. Triage is cheap thinking over a lot
/// of context; the expensive part is the work it dispatches.
pub const ROUTER_EFFORT: &str = "low";

/// A fresh router agent id.
pub fn new_router_agent_id() -> String {
    format!("{ROUTER_AGENT_ID_PREFIX}{}", uuid::Uuid::new_v4())
}

/// Whether an id names a router session rather than a coding agent.
pub fn is_router_agent(agent_id: &str) -> bool {
    agent_id.starts_with(ROUTER_AGENT_ID_PREFIX)
}

/// Where the router working on `capture_id` runs. One directory per capture, so
/// two routers can never read each other's scratch, and wiping one after it
/// exits can never take another's work.
pub fn scratch_dir(state_root: &Path, capture_id: &str) -> PathBuf {
    state_root.join(ROUTER_SCRATCH_DIR_NAME).join(capture_id)
}

/// What the router runs on: the account default provider at low effort, unless
/// the config file names something else. The provider is the account's because a
/// device with only one harness installed cannot route on the other one.
pub fn router_model_choice(
    default_provider: AgentProvider,
    configured: Option<&ModelChoice>,
) -> ModelChoice {
    match configured {
        Some(choice) => choice.clone(),
        None => ModelChoice {
            provider: default_provider,
            model: None,
            effort: Some(ROUTER_EFFORT.to_string()),
        },
    }
}

/// One live routing decision.
///
/// Keyed to the capture, not to a project or a checkout: the capture is the only
/// thing a router session is about, and the session ends when that capture has a
/// destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouterSession {
    pub capture_id: String,
    pub agent_id: String,
    pub scratch_dir: PathBuf,
    pub choice: ModelChoice,
    /// Whether a harness process was actually started for this session. Until
    /// one is, "no live process" means the spawn is still on its way, not that
    /// the router died — and the difference is a capture wrongly marked failed.
    pub started: bool,
}

impl RouterSession {
    /// A session about to be spawned for `capture_id`.
    pub fn new(capture_id: &str, state_root: &Path, choice: ModelChoice) -> RouterSession {
        RouterSession {
            capture_id: capture_id.to_string(),
            agent_id: new_router_agent_id(),
            scratch_dir: scratch_dir(state_root, capture_id),
            choice,
            started: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_router_id_says_what_kind_of_session_it_names() {
        let id = new_router_agent_id();
        assert!(is_router_agent(&id), "{id}");
        assert_ne!(id, new_router_agent_id());
        assert!(
            !is_router_agent(&crate::agent::new_agent_id()),
            "a coding agent is never mistaken for a router"
        );
    }

    /// Scratch is Build's, per capture, and outside every repository — the
    /// router has nothing to change even if it tried.
    #[test]
    fn scratch_is_one_directory_per_capture_under_builds_own_state() {
        let root = Path::new("/home/dev/.build");
        let one = scratch_dir(root, "capture-1");
        let two = scratch_dir(root, "capture-2");
        assert_eq!(one, root.join("router-scratch").join("capture-1"));
        assert_ne!(one, two);
        assert!(one.starts_with(root.join(ROUTER_SCRATCH_DIR_NAME)));
    }

    #[test]
    fn the_router_runs_on_the_account_default_at_low_effort() {
        let defaulted = router_model_choice(AgentProvider::Claude, None);
        assert_eq!(defaulted.provider, AgentProvider::Claude);
        assert_eq!(defaulted.effort.as_deref(), Some("low"));
        assert_eq!(
            defaulted.model, None,
            "the account's model, not a pinned one"
        );
        assert!(defaulted.validate().is_ok());

        assert_eq!(
            router_model_choice(AgentProvider::Codex, None).provider,
            AgentProvider::Codex
        );
    }

    #[test]
    fn a_configured_router_model_wins_over_the_default() {
        let configured = ModelChoice {
            provider: AgentProvider::Codex,
            model: Some("gpt-5.6-luna".to_string()),
            effort: Some("medium".to_string()),
        };
        assert_eq!(
            router_model_choice(AgentProvider::Claude, Some(&configured)),
            configured
        );
    }

    #[test]
    fn a_session_is_keyed_to_its_capture_and_gets_its_own_scratch() {
        let root = Path::new("/home/dev/.build");
        let session = RouterSession::new(
            "capture-1",
            root,
            router_model_choice(AgentProvider::Claude, None),
        );
        assert_eq!(session.capture_id, "capture-1");
        assert!(is_router_agent(&session.agent_id));
        assert_eq!(session.scratch_dir, scratch_dir(root, "capture-1"));
        assert!(!session.started, "nothing has been spawned yet");
    }
}
