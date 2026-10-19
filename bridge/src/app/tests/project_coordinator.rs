//! A project's pinned coordinator and the workers below it (#532).
//!
//! The project roster is reused: its owner persists exactly one pinned
//! coordinator, every other agent on it is a worker, and the bridge — not a
//! client, not an MCP frame, not a roster position — decides which is which.

use super::project_agent::{added_project, context, workspace};
use super::project_agent_base::{only_spawn, record_spawns};
use super::*;
use crate::mcp::{BridgeAction, McpSurface};
use crate::project_roster::ProjectRole;

/// A bridge over a store under `state_root`, booted the way the daemon boots.
pub(super) fn booted(state_root: &Path) -> AppState {
    AppState::new_unrooted_configured(
        state_root.join("worktrees"),
        "main",
        true,
        context(state_root),
    )
    .with_config(state_root.join("config.json"))
    .unwrap()
    .with_task_store(state_root.join("store"))
    .unwrap()
}

pub(super) fn ensure(state: &mut AppState, project_id: &str) -> String {
    let ensured = state.handle(req(
        "project.ensure_conversation",
        json!({ "project_id": project_id }),
    ));
    assert_eq!(ensured["ok"], true, "{ensured:?}");
    ensured["result"]["run_id"].as_str().unwrap().to_string()
}

pub(super) fn add_worker(state: &mut AppState, owner: &str, params: Value) -> String {
    let mut params = params;
    params["entity_id"] = json!(owner);
    let added = state.handle(req("agent.add", params));
    assert_eq!(added["ok"], true, "{added:?}");
    assert_eq!(
        added["result"]["agent"]["project_role"], "worker",
        "{added:?}"
    );
    added["result"]["agent"]["id"].as_str().unwrap().to_string()
}

pub(super) fn roles(state: &AppState, owner: &str) -> Vec<(String, Option<ProjectRole>)> {
    state
        .entity_agents(owner)
        .unwrap()
        .iter()
        .map(|agent| (agent.id.clone(), agent.project_role))
        .collect()
}

pub(super) fn coordinators(state: &AppState, owner: &str) -> Vec<String> {
    roles(state, owner)
        .into_iter()
        .filter(|(_, role)| *role == Some(ProjectRole::Coordinator))
        .map(|(id, _)| id)
        .collect()
}

/// Strip the pin and every purpose off a stored owner, leaving the record a
/// bridge from before pins wrote.
fn store_as_legacy(state_root: &Path, owner: &str, keep_agents: bool) {
    let store = crate::store::Store::new(state_root.join("store")).unwrap();
    let mut record = store
        .load_all_runs()
        .unwrap()
        .into_iter()
        .find(|run| run.id == owner)
        .expect("the owner is stored");
    record.pinned_agent_id = None;
    if keep_agents {
        for agent in &mut record.agents {
            agent.project_role = None;
        }
    } else {
        record.agents.clear();
    }
    store.save_run(&record).unwrap();
}

pub(super) fn a_repo() -> (tempfile::TempDir, PathBuf) {
    let (home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    (home, repo)
}

pub(super) fn a_state_root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    (tmp, root)
}

/// A fresh owner is born with its pin; a roster written before pins existed
/// pins its primary and keeps every other agent — id, history and settings —
/// as a worker; and neither a second boot nor repeated ensures move it.
#[test]
fn project_roster_migrates_one_stable_coordinator() {
    let (_home, repo) = a_repo();
    let (_tmp, state_root) = a_state_root();
    let (owner, pin, worker) = {
        let mut state = booted(&state_root);
        let project_id = added_project(&mut state, &repo);
        let owner = ensure(&mut state, &project_id);
        assert_eq!(coordinators(&state, &owner).len(), 1, "born with one pin");
        let pin = state.project_pin(&owner).unwrap().to_string();
        let worker = add_worker(
            &mut state,
            &owner,
            json!({ "name": "Rail scroll", "provider": "codex", "model": "gpt-5.5", "effort": "high" }),
        );
        let posted = state.handle(req(
            "thread.post",
            json!({ "entity_id": owner, "agent_id": worker, "body": "keep me" }),
        ));
        assert_eq!(posted["ok"], true, "{posted:?}");
        (owner, pin, worker)
    };

    store_as_legacy(&state_root, &owner, true);
    for boot in 0..2 {
        let mut state = booted(&state_root);
        assert_eq!(state.project_pin(&owner), Some(pin.as_str()), "boot {boot}");
        assert_eq!(
            roles(&state, &owner),
            vec![
                (pin.clone(), Some(ProjectRole::Coordinator)),
                (worker.clone(), Some(ProjectRole::Worker)),
            ],
            "boot {boot}"
        );
        let kept = state.entity_agents(&owner).unwrap().by_id(&worker).unwrap();
        assert_eq!(kept.name.as_deref(), Some("Rail scroll"));
        assert_eq!(kept.choice.model.as_deref(), Some("gpt-5.5"));
        assert!(
            state
                .agent_conversation(&owner, Some(&worker))
                .unwrap()
                .first_user_line(40)
                .is_some_and(|line| line.contains("keep me")),
            "the worker keeps its history"
        );
        let project_id = state.projects.project_id_of(&owner).unwrap().to_string();
        for _ in 0..2 {
            assert_eq!(ensure(&mut state, &project_id), owner);
        }
        assert_eq!(coordinators(&state, &owner), vec![pin.clone()]);
        assert_eq!(state.entity_agents(&owner).unwrap().len(), 2);
    }
}

/// An owner stored with nobody on it gains exactly one pin at boot, and
/// keeps that one across another restart and every later ensure.
#[test]
fn an_empty_legacy_project_roster_gains_a_single_pin() {
    let (_home, repo) = a_repo();
    let (_tmp, state_root) = a_state_root();
    let owner = {
        let mut state = booted(&state_root);
        let project_id = added_project(&mut state, &repo);
        ensure(&mut state, &project_id)
    };
    store_as_legacy(&state_root, &owner, false);

    let pin = {
        let state = booted(&state_root);
        assert_eq!(coordinators(&state, &owner).len(), 1);
        state.project_pin(&owner).unwrap().to_string()
    };
    let mut state = booted(&state_root);
    assert_eq!(state.project_pin(&owner), Some(pin.as_str()));
    let project_id = state.projects.project_id_of(&owner).unwrap().to_string();
    ensure(&mut state, &project_id);
    assert_eq!(coordinators(&state, &owner), vec![pin]);
}

/// Assert `answer` refused with `code`.
pub(super) fn refused_with(answer: &Value, code: &str) {
    assert_eq!(answer["ok"], false, "{answer:?}");
    assert_eq!(answer["error_code"], code, "{answer:?}");
}

/// A client cannot mint a coordinator, nor ask for a purpose on an owner
/// that is not a project's.
fn no_client_mints_a_coordinator(state: &mut AppState, project_id: &str, owner: &str) {
    for forged in ["coordinator", "pinned", ""] {
        let minted = state.handle(req(
            "agent.add",
            json!({ "entity_id": owner, "project_role": forged }),
        ));
        refused_with(&minted, "invalid_params");
    }
    let workspace_id = workspace(state, project_id, "elsewhere");
    let workspace_owner = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace_id }),
    ))["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let elsewhere = state.handle(req(
        "agent.add",
        json!({ "entity_id": workspace_owner, "project_role": "worker" }),
    ));
    refused_with(&elsewhere, "invalid_params");
}

/// Clearing the pin's conversation replaces its history, never its place.
fn a_reset_keeps_the_pin(state: &mut AppState, project_id: &str, owner: &str, pin: &str) {
    let thread_id = state
        .agent_conversation(owner, Some(pin))
        .unwrap()
        .id
        .clone();
    let reset = state.handle(req(
        "conversation.reset",
        json!({
            "project_id": project_id, "entity_id": owner, "agent_id": pin,
            "conversation_id": pin, "expected_thread_id": thread_id,
        }),
    ));
    assert_eq!(reset["ok"], true, "{reset:?}");
    assert_ne!(
        state.agent_conversation(owner, Some(pin)).unwrap().id,
        thread_id
    );
    assert_eq!(state.project_pin(owner), Some(pin));
}

/// No request may take the pin away or hand its purpose elsewhere: removing
/// it and ending its owner are refused, a client cannot mint a coordinator,
/// and the work a request names no agent for still reaches the pin. A worker
/// goes like any agent, and a reset keeps the pin's identity.
#[test]
fn project_coordinator_cannot_be_removed_or_reassigned() {
    let (_home, repo) = a_repo();
    let (_tmp, state_root) = a_state_root();
    let mut state = booted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let owner = ensure(&mut state, &project_id);
    let pin = state.project_pin(&owner).unwrap().to_string();
    let worker = add_worker(&mut state, &owner, json!({}));

    let removed = state.handle(req(
        "agent.remove",
        json!({ "entity_id": owner, "agent_id": pin }),
    ));
    refused_with(&removed, "conflict");
    assert_eq!(removed["error"], crate::app::PINNED_AGENT_STAYS);
    let abandoned = state.handle(req("run.abandon", json!({ "run_id": owner })));
    refused_with(&abandoned, "conflict");
    assert!(!state.runs[&owner].run.state.is_terminal());
    no_client_mints_a_coordinator(&mut state, &project_id, &owner);
    assert_eq!(coordinators(&state, &owner), vec![pin.clone()]);
    assert_eq!(state.entity_agents(&owner).unwrap().len(), 2);

    // A post naming no agent, and every path that must be heard, reach the
    // pin — never a worker, wherever the roster put it.
    state
        .runs
        .get_mut(&owner)
        .unwrap()
        .agents
        .move_to_front(&worker);
    assert_eq!(state.ensure_primary_agent(&owner).unwrap(), pin);
    assert_eq!(state.runs[&owner].agents.primary().unwrap().id, pin);

    a_reset_keeps_the_pin(&mut state, &project_id, &owner, &pin);
    let gone = state.handle(req(
        "agent.remove",
        json!({ "entity_id": owner, "agent_id": worker }),
    ));
    assert_eq!(gone["ok"], true, "{gone:?}");
    assert_eq!(
        roles(&state, &owner),
        vec![(pin.clone(), Some(ProjectRole::Coordinator))]
    );

    // And all of it was written down: a restart answers the same pin.
    drop(state);
    let state = booted(&state_root);
    assert_eq!(state.project_pin(&owner), Some(pin.as_str()));
}

/// A stale session cannot act for a worker that is gone, and an MCP frame a
/// pin authenticated before its conversation was cleared cannot write into
/// the cleared one.
#[tokio::test]
async fn stale_project_sessions_cannot_act_for_the_pin_or_a_removed_worker() {
    use crate::app::mcp::{authenticated_mcp_owner, handle_coding_mcp_frame, ControlPlane, Turn};
    let (_home, repo) = a_repo();
    let (_tmp, state_root) = a_state_root();
    let mut state = booted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let owner = ensure(&mut state, &project_id);
    let pin = state.project_pin(&owner).unwrap().to_string();
    let worker = add_worker(&mut state, &owner, json!({}));

    state
        .session_registry
        .test_install_token(worker.clone(), "worker-token".into());
    let removed = state.handle(req(
        "agent.remove",
        json!({ "entity_id": owner, "agent_id": worker }),
    ));
    assert_eq!(removed["ok"], true, "{removed:?}");
    let late = json!({ "task_id": worker, "session_token": "worker-token" });
    assert_eq!(
        authenticated_mcp_owner(&late, &state.session_registry),
        None
    );

    state
        .session_registry
        .test_install_token(pin.clone(), "pin-token".into());
    let frame = json!({
        "task_id": pin, "session_token": "pin-token",
        "request": BridgeAction::SetTopic { topic: "Old topic".into() },
    });
    let authenticated = authenticated_mcp_owner(&frame, &state.session_registry).unwrap();
    let Some(crate::app::mcp::AddressedSession::Coding(generation)) =
        state.addressed_session(authenticated.into())
    else {
        panic!("the pin is addressed as a conversation session");
    };
    let thread_id = state
        .agent_conversation(&owner, Some(&pin))
        .unwrap()
        .id
        .clone();
    let reset = state.handle(req(
        "conversation.reset",
        json!({
            "project_id": project_id, "entity_id": owner, "agent_id": pin,
            "conversation_id": pin, "expected_thread_id": thread_id,
        }),
    ));
    assert_eq!(reset["ok"], true, "{reset:?}");

    let shared = Arc::new(Mutex::new(state));
    let clock = Arc::clone(&shared.lock().unwrap().frame_clock);
    let plane = ControlPlane::new(Arc::clone(&clock));
    let timer = clock.frame(crate::app::mcp::MCP_CONTROL_METHOD);
    let mut turn = Turn::take(&plane, &timer).await;
    let answered = handle_coding_mcp_frame(&shared, &frame, &generation, &timer, &mut turn).await;
    assert_eq!(answered.unwrap()["ok"], false);
    let state = shared.lock().unwrap();
    assert_eq!(state.project_pin(&owner), Some(pin.as_str()));
    let topic = &state
        .entity_agents(&owner)
        .unwrap()
        .by_id(&pin)
        .unwrap()
        .topic;
    assert_eq!(topic.as_deref(), None, "the stale frame wrote nothing");
}

/// Every project agent wears the `project-` prefix; only the persisted
/// purpose decides what it is. The pin keeps the project tools and prompt; a
/// worker gets the coding tools, the worker prompt and the coding surface at
/// launch, and no copied prefix, flag or roster position changes that.
#[tokio::test]
async fn project_worker_tools_follow_persisted_purpose() {
    let (_home, repo) = a_repo();
    let (_tmp, state_root) = a_state_root();
    let mut state = booted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let owner = ensure(&mut state, &project_id);
    let pin = state.project_pin(&owner).unwrap().to_string();
    let worker = add_worker(&mut state, &owner, json!({}));
    assert!(crate::agent::is_project_agent(&worker), "{worker}");

    assert_eq!(state.agent_mcp_surface(&owner, &pin), McpSurface::Project);
    assert_eq!(state.agent_mcp_surface(&owner, &worker), McpSurface::Coding);

    let source_admin = || BridgeAction::RemoveProjectSource {
        source_id: "source-nope".into(),
    };
    let refused = state
        .on_agent_mcp_action(&owner, &worker, source_admin())
        .unwrap_err();
    assert!(
        refused.contains("this session is on the coding surface"),
        "{refused}"
    );
    let by_pin = state
        .on_agent_mcp_action(&owner, &pin, source_admin())
        .unwrap_err();
    assert!(
        !by_pin.contains("surface"),
        "the pin holds the tool: {by_pin}"
    );
    let staffing = state
        .on_agent_mcp_action(
            &owner,
            &worker,
            BridgeAction::CompactAgent {
                agent_id: pin.clone(),
                instructions: "keep the plan".into(),
            },
        )
        .unwrap_err();
    assert!(staffing.contains("coding surface"), "{staffing}");
    state
        .on_agent_mcp_action(&owner, &worker, BridgeAction::ListWorkspaces)
        .expect("a worker has the workspace tools");

    // The prompt each is started on.
    let worker_prompt = state.project_surface_prompt(&owner, &worker).unwrap();
    assert!(
        worker_prompt.contains("original sources"),
        "{worker_prompt}"
    );
    assert!(worker_prompt.contains(&repo.display().to_string()));
    assert!(
        !worker_prompt.contains("Never change it"),
        "{worker_prompt}"
    );
    let pin_prompt = state.project_surface_prompt(&owner, &pin).unwrap();
    assert!(pin_prompt.contains("Never change it"), "{pin_prompt}");
    assert!(state.claim_agent_name_request(&owner, &worker));
    assert!(!state.claim_agent_name_request(&owner, &pin));

    // What each launch is granted, in the project's folder.
    let scratch = AppState::canonical_root(&state.runs[&owner].worktree.path);
    let shared = state.shared();
    for (agent, granted) in [(&worker, McpSurface::Coding), (&pin, McpSurface::Project)] {
        let spawns = record_spawns(&shared);
        ensure_agent_tab(
            &shared,
            &scratch,
            &owner,
            agent,
            &ModelChoice::default(),
            "start",
        )
        .unwrap_or_else(|error| panic!("{agent}: {error}"));
        let options = only_spawn(&spawns);
        assert_eq!(options.mcp_surface, Some(granted), "{agent}");
        assert_eq!(options.cwd, repo, "{agent} starts in the project's folder");
    }
}

/// A worker writes: with the project's folder gone it is refused a start
/// rather than started in Build's scratch, while the read-only pin still
/// starts there.
#[tokio::test]
async fn a_project_worker_does_not_start_without_its_sources() {
    let (_home, repo) = a_repo();
    let (_tmp, state_root) = a_state_root();
    let mut state = booted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let owner = ensure(&mut state, &project_id);
    let pin = state.project_pin(&owner).unwrap().to_string();
    let worker = add_worker(&mut state, &owner, json!({}));
    let scratch = AppState::canonical_root(&state.runs[&owner].worktree.path);
    std::fs::rename(&repo, repo.with_extension("moved")).unwrap();
    let shared = state.shared();
    let spawns = record_spawns(&shared);

    let refused = ensure_agent_tab(
        &shared,
        &scratch,
        &owner,
        &worker,
        &ModelChoice::default(),
        "start",
    )
    .unwrap_err();
    assert!(refused.contains("project folder"), "{refused}");
    assert!(spawns.lock().unwrap().is_empty(), "nothing was started");

    ensure_agent_tab(
        &shared,
        &scratch,
        &owner,
        &pin,
        &ModelChoice::default(),
        "start",
    )
    .expect("the pin still starts");
}

/// A client that predates workers reads a project owner as it always did:
/// one agent. Board rows, project rows, `agent.remove` and an `agent.list`
/// that does not ask for workers show the pin alone; asking shows the whole
/// roster with each agent's purpose and the pin by id.
#[test]
fn old_clients_see_only_the_pinned_coordinator() {
    let (_home, repo) = a_repo();
    let (_tmp, state_root) = a_state_root();
    let mut state = booted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let owner = ensure(&mut state, &project_id);
    let pin = state.project_pin(&owner).unwrap().to_string();
    let first = add_worker(&mut state, &owner, json!({}));
    let second = add_worker(&mut state, &owner, json!({ "project_role": "worker" }));
    let only_pin = |agents: &Value| -> Vec<String> {
        agents
            .as_array()
            .unwrap()
            .iter()
            .map(|agent| agent["id"].as_str().unwrap().to_string())
            .collect()
    };

    let board = state.handle(req("board.list", json!({})));
    let row = board["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["run_id"] == json!(owner))
        .unwrap()
        .clone();
    assert_eq!(only_pin(&row["agents"]), vec![pin.clone()], "{row:?}");
    assert_eq!(row["agents"][0]["project_role"], "coordinator");

    let projects = state.project_list();
    let conversations = projects["projects"][0]["conversations"].as_array().unwrap();
    assert_eq!(conversations.len(), 1, "{conversations:?}");
    assert_eq!(conversations[0]["conversation_id"], json!(pin));

    let legacy = state.handle(req("agent.list", json!({ "entity_id": owner })));
    assert_eq!(legacy["ok"], true, "{legacy:?}");
    assert_eq!(only_pin(&legacy["result"]["agents"]), vec![pin.clone()]);
    assert_eq!(legacy["result"]["pinned_agent_id"], json!(pin));

    let whole = state.handle(req(
        "agent.list",
        json!({ "entity_id": owner, "include_project_workers": true }),
    ));
    assert_eq!(
        only_pin(&whole["result"]["agents"]),
        vec![pin.clone(), first.clone(), second.clone()]
    );
    let purposes: Vec<&str> = whole["result"]["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|agent| agent["project_role"].as_str().unwrap())
        .collect();
    assert_eq!(purposes, ["coordinator", "worker", "worker"]);

    let removed = state.handle(req(
        "agent.remove",
        json!({ "entity_id": owner, "agent_id": first }),
    ));
    assert_eq!(only_pin(&removed["result"]["agents"]), vec![pin.clone()]);

    // Every other owner lists all of its agents, and names no pin.
    let workspace_id = workspace(&mut state, &project_id, "elsewhere");
    let workspace_owner = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace_id }),
    ))["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    for _ in 0..2 {
        state.handle(req("agent.add", json!({ "entity_id": workspace_owner })));
    }
    let listed = state.handle(req("agent.list", json!({ "entity_id": workspace_owner })));
    assert_eq!(listed["result"]["agents"].as_array().unwrap().len(), 2);
    assert!(
        listed["result"].get("pinned_agent_id").is_none(),
        "{listed:?}"
    );
    assert!(listed["result"]["agents"][0].get("project_role").is_none());
}

/// A project with its owner, pin and one worker, written down and closed, for
/// a test that changes the disk under it before the next boot.
fn a_stored_project(state_root: &Path, repo: &Path) -> (String, String, String, PathBuf) {
    let mut state = booted(state_root);
    let project_id = added_project(&mut state, repo);
    let owner = ensure(&mut state, &project_id);
    let pin = state.project_pin(&owner).unwrap().to_string();
    let worker = add_worker(&mut state, &owner, json!({}));
    let scratch = state.runs[&owner].worktree.path.clone();
    (owner, pin, worker, scratch)
}

/// The owner a boot restored still stands with its pin, and the project's
/// `project.ensure_conversation` answers that owner rather than a new one.
fn the_owner_stands(state: &mut AppState, owner: &str, pin: &str) {
    assert!(
        !state.runs[owner].run.state.is_terminal(),
        "{:?}",
        state.runs[owner].run.state
    );
    assert_eq!(state.project_pin(owner), Some(pin));
    let project_id = state.projects.project_id_of(owner).unwrap().to_string();
    assert_eq!(ensure(state, &project_id), owner);
    assert_eq!(coordinators(state, owner), vec![pin.to_string()]);
}

/// A project's scratch directory is Build's own, so one deleted under a
/// stopped bridge is cut again at boot: the owner, its pin and its workers
/// stand, and no later ensure mints a second owner beside them.
#[test]
fn a_missing_project_scratch_is_recreated_and_keeps_the_pin() {
    let (_home, repo) = a_repo();
    let (_tmp, state_root) = a_state_root();
    let (owner, pin, worker, scratch) = a_stored_project(&state_root, &repo);
    std::fs::remove_dir_all(&scratch).unwrap();

    for boot in 0..2 {
        let mut state = booted(&state_root);
        assert!(scratch.is_dir(), "boot {boot} cut the scratch again");
        the_owner_stands(&mut state, &owner, &pin);
        assert_eq!(
            roles(&state, &owner),
            vec![
                (pin.clone(), Some(ProjectRole::Coordinator)),
                (worker.clone(), Some(ProjectRole::Worker)),
            ],
            "boot {boot}"
        );
        assert_eq!(state.runs.len(), 1, "boot {boot}: no second owner");
    }
}

/// The pin's purpose is the record's: a boot that cannot reach the project's
/// primary source neither retires the owner nor forgets which agent is its
/// coordinator. The pin keeps the project surface and every refusal, a worker
/// still cannot start without the sources, and once the folder is back —
/// found again within the boot, or at the next — the project answers the
/// same owner and the same pin.
#[tokio::test]
async fn a_boot_without_the_primary_source_keeps_the_pins_purpose() {
    let (_home, repo) = a_repo();
    let (_tmp, state_root) = a_state_root();
    let (owner, pin, worker, scratch) = a_stored_project(&state_root, &repo);
    let away = repo.with_extension("away");
    std::fs::rename(&repo, &away).unwrap();

    {
        let state = booted(&state_root);
        assert!(!state.runs[&owner].run.state.is_terminal());
        the_pin_keeps_its_purpose_unbound(state, &owner, &pin, &worker, &scratch).await;
    }

    {
        let mut state = booted(&state_root);
        std::fs::rename(&away, &repo).unwrap();
        // Nothing is added again: the list finds the folder back (#533).
        let listed = state.handle(req("project.list", json!({})));
        assert_eq!(listed["ok"], true, "{listed:?}");
        let project_id = state.projects.project_id_of(&owner).unwrap().to_string();
        assert_eq!(ensure(&mut state, &project_id), owner);
        assert_eq!(state.project_pin(&owner), Some(pin.as_str()));
    }
    let mut state = booted(&state_root);
    the_owner_stands(&mut state, &owner, &pin);
    assert_eq!(state.agent_mcp_surface(&owner, &pin), McpSurface::Project);
    assert_eq!(state.runs.len(), 1, "no second owner");

    // The same worker starts again in the sources, with nothing repaired.
    let shared = state.shared();
    let spawns = record_spawns(&shared);
    ensure_agent_tab(
        &shared,
        &AppState::canonical_root(&scratch),
        &owner,
        &worker,
        &ModelChoice::default(),
        "start",
    )
    .expect("the worker starts once its folder is back");
    assert_eq!(only_spawn(&spawns).cwd, repo);
}

/// What a boot without the project's sources still owes the pin.
async fn the_pin_keeps_its_purpose_unbound(
    mut state: AppState,
    owner: &str,
    pin: &str,
    worker: &str,
    scratch: &Path,
) {
    assert_eq!(state.project_pin(owner), Some(pin));
    assert_eq!(state.agent_mcp_surface(owner, pin), McpSurface::Project);
    assert_eq!(state.agent_mcp_surface(owner, worker), McpSurface::Coding);
    let removed = state.handle(req(
        "agent.remove",
        json!({ "entity_id": owner, "agent_id": pin }),
    ));
    refused_with(&removed, "conflict");
    let abandoned = state.handle(req("run.abandon", json!({ "run_id": owner })));
    refused_with(&abandoned, "conflict");
    let forged = state.handle(req(
        "agent.add",
        json!({ "entity_id": owner, "project_role": "coordinator" }),
    ));
    refused_with(&forged, "invalid_params");
    assert_eq!(coordinators(&state, owner), vec![pin.to_string()]);

    // No project is loaded to start anything with; the refusal is the
    // worker's own, read off the record's path, before any spawn is tried.
    let shared = state.shared();
    let scratch = AppState::canonical_root(scratch);
    let refused = ensure_agent_tab(
        &shared,
        &scratch,
        owner,
        worker,
        &ModelChoice::default(),
        "start",
    )
    .unwrap_err();
    assert!(refused.contains("project folder"), "{refused}");
}

/// The live sweep that archives a run whose checkout was deleted never
/// retires a project's owner over its scratch: the board's sweep cuts the
/// directory again, and an archive the sweep decided on still passes the
/// owner by.
#[test]
fn a_scratch_deleted_while_running_never_archives_the_owner() {
    use crate::app::runtime::recovery::DecidedVanishedRun;
    let (_home, repo) = a_repo();
    let (_tmp, state_root) = a_state_root();
    let mut state = booted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let owner = ensure(&mut state, &project_id);
    let pin = state.project_pin(&owner).unwrap().to_string();
    let scratch = state.runs[&owner].worktree.path.clone();

    std::fs::remove_dir_all(&scratch).unwrap();
    let board = state.handle(req("board.list", json!({})));
    assert_eq!(board["ok"], true, "{board:?}");
    assert!(scratch.is_dir(), "the sweep cut the scratch again");

    std::fs::remove_dir_all(&scratch).unwrap();
    state.archive_vanished_runs(vec![DecidedVanishedRun {
        run_id: owner.clone(),
        published: Default::default(),
    }]);
    the_owner_stands(&mut state, &owner, &pin);
    drop(state);
    let mut state = booted(&state_root);
    the_owner_stands(&mut state, &owner, &pin);
}
