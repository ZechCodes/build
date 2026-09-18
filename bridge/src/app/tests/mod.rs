use super::*;
use crate::git_fixture::{git_in, init_repo, init_repo_named};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::harness::stream_fixtures::{
    recorded_workflow_surfaces, SUBAGENT_SPAWNING_CALL_ID, SUBAGENT_TASK_ID, WORKFLOW_TASK_ID,
};
use crate::harness::surfaces::{AgentSurfaces, SurfaceRevision};
use crate::harness::{AgentSession, HarnessError, Turn};
use crate::pty::{PtySession, AGENT_WORKING_WINDOW};
use crate::store::PersistedArchivedWorktree;
use crate::timing::{recording_clock, SLOW_FRAME};
use serde_json::Value;

/// The frame path as a test that is not measuring a frame calls it.
///
/// Every acquisition the delivery path makes belongs to the frame that
/// asked for it, so it takes that frame's timer. A test calling it directly
/// has no frame, so it gets one of its own and these shadow the real
/// functions for the rest of the module. A test that IS about the timing
/// calls `crate::app::` and passes the timer it means.
///
/// The activity pump makes no acquisition at all: it is handed its session's
/// revision channel with the rest of the session output. A test that put a
/// dictated tab in the registry has no session output, so its shadow reads
/// the channel back off the tab — under a bare lock, which no frame holds
/// here because no frame is running.
mod untimed {
    use super::*;

    pub(super) fn a_frame(state: &Arc<Mutex<AppState>>) -> FrameTimer {
        Arc::clone(&state.lock().unwrap().frame_clock).frame("test")
    }

    /// Start a tab's pumps the way a verb does, for a test that put the
    /// tab in the registry by hand: the real function is handed what the
    /// tab holds, which a verb takes off the tab before it hands it over.
    pub(super) fn spawn_tab_pumps(
        state: &Arc<Mutex<AppState>>,
        key: TabKey,
        output: SessionOutput,
    ) {
        let pumps = state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(&key)
            .unwrap()
            .pumps(output);
        super::super::spawn_tab_pumps(state, key, pumps)
    }

    pub(super) fn spawn_activity_pump(
        state: &Arc<Mutex<AppState>>,
        key: TabKey,
        rx: Option<broadcast::Receiver<crate::harness::ActivityReport>>,
    ) {
        let Some((session, instance, surfaces_changed)) = state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(&key)
            .map(|tab| {
                (
                    Arc::clone(&tab.session),
                    tab.session_instance.clone(),
                    tab.session.surfaces_changed(),
                )
            })
        else {
            return;
        };
        super::super::spawn_activity_pump(state, key, session, instance, rx, surfaces_changed)
    }

    pub(super) fn ensure_agent_tab(
        state: &Arc<Mutex<AppState>>,
        root: &std::path::Path,
        owner: &str,
        agent_id: &str,
        model_choice: &ModelChoice,
        phase: &str,
    ) -> Result<(String, Spawned), String> {
        let conversation_id = state
            .lock()
            .unwrap()
            .resolve_conversation_address(owner, Some(agent_id))?
            .conversation_id;
        super::super::ensure_agent_tab(
            state,
            root,
            AgentSpawnRequest {
                owner,
                agent_id,
                conversation_id: &conversation_id,
                model_choice,
                force_fresh: false,
                phase,
            },
            &a_frame(state),
        )
        .map(|opened| opened.expect("the owner still has a session"))
    }

    pub(super) fn deliver(
        state: &Arc<Mutex<AppState>>,
        root: &std::path::Path,
        owner: &str,
        agent_id: &str,
        model_choice: &ModelChoice,
        phase: &'static str,
        [cold, warm]: [&str; 2],
    ) -> Result<(String, Spawned), String> {
        let say = TurnText {
            cold: cold.to_string(),
            warm: warm.to_string(),
        };
        let (conversation_id, choice_revision) = {
            let app = state.lock().unwrap();
            let address = app.resolve_conversation_address(owner, Some(agent_id))?;
            let choice_revision = app
                .entity_agents(owner)?
                .by_id(agent_id)
                .expect("the resolved address names this agent")
                .choice_revision;
            (address.conversation_id, choice_revision)
        };
        super::super::deliver(
            state,
            &PendingAgentTurn {
                operation_id: None,
                root: AppState::canonical_root(root),
                owner: owner.to_string(),
                agent_id: agent_id.to_string(),
                conversation_id,
                model_choice: model_choice.clone(),
                choice_revision,
                interrupt: false,
                phase,
                say: Some(say),
                wants_catch_up: false,
                survives_refusal: false,
            },
            &a_frame(state),
        )
        .map(|delivered| match delivered {
            DeliveryOutcome::Delivered(Some(delivered)) => delivered,
            DeliveryOutcome::Delivered(None) => panic!("the owner still has a session"),
            DeliveryOutcome::Deferred => panic!("the test turn is immediately eligible"),
        })
    }

    /// Take the queue and deliver it here and now, the way a test with no
    /// runtime under it has to.
    pub(super) fn deliver_pending_agent_turns(state: &Arc<Mutex<AppState>>) {
        let turns = state.lock().unwrap().take_pending_turns();
        DeliveryRunner::run(state, turns)
    }
}
use untimed::{
    a_frame, deliver, deliver_pending_agent_turns, ensure_agent_tab, spawn_activity_pump,
    spawn_tab_pumps,
};

fn test_build_agent(mcp_socket: impl Into<std::path::PathBuf>) -> Agent {
    build_agent(
        false,
        HarnessContext::resolved(mcp_socket.into(), default_state_root()).unwrap(),
    )
}

fn test_bridge_exe() -> PathBuf {
    std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap()
}

/// The grid a tab's terminal paints into. Every tab a test spawns has one:
/// only a hand-built terminal-free session does not, and no test that
/// speaks about a screen owns one of those.
fn screen_of(tab: &Tab) -> &ScreenHandle {
    tab.screen
        .as_ref()
        .expect("a tab spawned in a PTY has a screen")
}

fn req(method: &str, params: Value) -> Frame {
    Frame {
        session_id: "s".into(),
        message_id: "m".into(),
        frame_type: "data".into(),
        sender: "client".into(),
        created_at: "t".into(),
        payload: json!({ "method": method, "id": "1", "params": params }),
    }
}

mod support;
use support::*;

mod agent_messages;
mod api_facade;
mod board;
mod configuration;
mod conversations;
mod filesystem;
mod git;
mod harness_models;
mod lifecycle_compat;
mod merge_regressions;
mod project_agent;
mod project_conversation;
mod protocol;
mod push;
mod routing;
mod rtc;
mod runtime;
mod shell;
mod workflow;
mod workspaces;
