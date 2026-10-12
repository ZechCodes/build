//! What removing or resetting one worker may touch when every agent of the
//! project stands in the same folder: its own session, authority and queued
//! turns, and nothing of its siblings', the pin's or the sources'.

use super::super::project_agent_base::record_spawns;
use super::*;

/// The project's agents with live sessions, all started in the project's
/// own folder, each holding a known control-plane token.
struct LiveProject {
    _home: tempfile::TempDir,
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    project_id: String,
    owner: String,
    pin: String,
    shared: Arc<Mutex<AppState>>,
    scratch: PathBuf,
    first: String,
    second: String,
}

impl LiveProject {
    async fn start() -> Self {
        let mut project = WorkerProject::new();
        let first = project.worker("First");
        let second = project.worker("Second");
        let WorkerProject {
            _home,
            _tmp,
            repo,
            state,
            project_id,
            owner,
            pin,
        } = project;
        let scratch = AppState::canonical_root(&state.runs[&owner].worktree.path);
        let shared = state.shared();
        let spawns = record_spawns(&shared);
        for agent in [&pin, &first, &second] {
            ensure_agent_tab(
                &shared,
                &scratch,
                &owner,
                agent,
                &ModelChoice::default(),
                "start",
            )
            .unwrap_or_else(|error| panic!("{agent}: {error}"));
            shared
                .lock()
                .unwrap()
                .session_registry
                .test_install_token(agent.clone(), format!("{agent}-token"));
        }
        let cwds: Vec<_> = spawns
            .lock()
            .unwrap()
            .iter()
            .map(|spawned| spawned.cwd.clone())
            .collect();
        assert_eq!(cwds, vec![repo.clone(); 3], "every agent shares the folder");
        Self {
            _home,
            _tmp,
            repo,
            project_id,
            owner,
            pin,
            shared,
            scratch,
            first,
            second,
        }
    }

    fn live(&self, agent: &str) -> bool {
        let state = self.shared.lock().unwrap();
        state
            .session_registry
            .agent_snapshot(&TabKey::agent(&self.scratch, agent))
            .is_some()
            && state
                .session_registry
                .token_matches(agent, &format!("{agent}-token"))
    }

    fn handle(&self, method: &str, params: Value) -> Value {
        self.shared.lock().unwrap().handle(req(method, params))
    }

    fn post(&self, agent: &str, body: &str) {
        let posted = self.handle(
            "thread.post",
            json!({ "entity_id": self.owner, "agent_id": agent, "body": body }),
        );
        assert_eq!(posted["ok"], true, "{posted:?}");
    }

    fn history(&self, agent: &str) -> String {
        let state = self.shared.lock().unwrap();
        serde_json::to_string(
            &state
                .agent_conversation(&self.owner, Some(agent))
                .unwrap()
                .items,
        )
        .unwrap()
    }

    fn mcp_config(&self, agent: &str) -> PathBuf {
        self.scratch
            .join(crate::orchestrator::mcp_config_path(agent))
    }
}

/// Removing worker A and resetting worker B, while A, B and the pin all
/// stand in the project's folder: A's session and token go and B's reset
/// retires B's session alone; the pin keeps its session, token, history and
/// launch config, the survivors keep theirs, and an uncommitted edit in the
/// shared sources is untouched by either.
#[tokio::test]
async fn project_worker_removal_preserves_shared_cwd_siblings() {
    let live = LiveProject::start().await;
    let (owner, pin) = (live.owner.clone(), live.pin.clone());
    let edit = live.repo.join("shared-edit.txt");
    std::fs::write(&edit, "a human's uncommitted work\n").unwrap();
    live.post(&pin, "pin keeps this");
    live.post(&live.second, "second keeps this until reset");
    let pin_history = live.history(&pin);

    let removed = live.handle(
        "agent.remove",
        json!({ "entity_id": owner, "agent_id": live.first }),
    );
    assert_eq!(removed["ok"], true, "{removed:?}");
    assert!(!live.live(&live.first), "the removed worker's session went");
    assert!(live.live(&live.second), "a sibling's session stays");
    assert!(live.live(&pin), "the pin's session stays");
    let second_history = live.history(&live.second);
    assert!(second_history.contains("second keeps this"));

    let thread_id = live
        .shared
        .lock()
        .unwrap()
        .agent_conversation(&owner, Some(&live.second))
        .unwrap()
        .id
        .clone();
    let reset = live.handle(
        "conversation.reset",
        json!({
            "project_id": live.project_id, "entity_id": owner,
            "agent_id": live.second, "conversation_id": live.second,
            "expected_thread_id": thread_id,
        }),
    );
    assert_eq!(reset["ok"], true, "{reset:?}");
    assert!(!live.live(&live.second), "the reset worker's session went");
    assert!(!live.history(&live.second).contains("second keeps this"));
    assert!(
        live.live(&pin),
        "the pin's session survives a sibling's reset"
    );
    assert_eq!(live.history(&pin), pin_history);
    assert!(live.mcp_config(&pin).is_file());
    assert_eq!(
        std::fs::read_to_string(&edit).unwrap(),
        "a human's uncommitted work\n"
    );
    let state = live.shared.lock().unwrap();
    assert_eq!(state.project_pin(&owner), Some(pin.as_str()));
    let roster: Vec<_> = state
        .entity_agents(&owner)
        .unwrap()
        .iter()
        .map(|agent| agent.id.clone())
        .collect();
    assert_eq!(roster, vec![pin, live.second.clone()]);
}
