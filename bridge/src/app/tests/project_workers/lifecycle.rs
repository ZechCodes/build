//! Reclaiming a named workspace and deleting the whole project, around the
//! project's own agents.

use super::super::project_agent_base::record_spawns;
use super::super::workspace_reclaim::{call, finish, impatient, linked_workspace, now_ms};
use super::*;
use crate::mcp::BridgeAction;

/// Start `agent` of `owner` on the recorded harness, in the project's folder.
fn start(state: &Arc<Mutex<AppState>>, owner: &str, agent: &str) -> PathBuf {
    let scratch = AppState::canonical_root(&state.lock().unwrap().runs[owner].worktree.path);
    let _spawns = record_spawns(state);
    ensure_agent_tab(
        state,
        &scratch,
        owner,
        agent,
        &ModelChoice::default(),
        "start",
    )
    .unwrap_or_else(|error| panic!("{agent}: {error}"));
    scratch
}

fn session_live(state: &Arc<Mutex<AppState>>, scratch: &Path, agent: &str) -> bool {
    state
        .lock()
        .unwrap()
        .session_registry
        .agent_snapshot(&TabKey::agent(scratch, agent))
        .is_some()
}

/// A named workspace's agent makes a project worker, the workspace goes
/// quiet and is swept and reclaimed: the project's owner is never a reclaim
/// subject, and the reclaim retires the workspace's agent alone — the pin,
/// the worker it made (still running) and the project's scratch stand.
#[tokio::test]
async fn workspace_reclaim_never_reclaims_project_agents() {
    let (_tmp, state, project_id, ws, task) = linked_workspace();
    let (owner, pin) =
        super::super::project_agent::project_agent(&mut state.lock().unwrap(), &project_id);
    let workspace_owner = call(
        &state,
        "workspace.ensure_conversation",
        json!({ "workspace_id": ws }),
    )["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let coder = call(&state, "agent.add", json!({ "entity_id": workspace_owner }))["result"]
        ["agent"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let made = state
        .lock()
        .unwrap()
        .on_agent_mcp_action(
            &workspace_owner,
            &coder,
            BridgeAction::AddProjectAgent {
                notify_user: None,
                harness: None,
                model: None,
                effort: None,
                name: Some("Workspace made".into()),
                role: None,
                capability: None,
            },
        )
        .unwrap();
    let worker = made["agent"]["id"].as_str().unwrap().to_string();
    let scratch = start(&state, &owner, &worker);

    finish(&state, &task);
    AppState::sweep_workspaces(&state, &impatient(), now_ms());
    let rows = call(&state, "workspace.list", json!({}));
    for row in rows["result"]["workspaces"].as_array().unwrap() {
        assert_ne!(row["workspace_id"], json!(owner), "{row:?}");
        let root = PathBuf::from(row["root"].as_str().unwrap());
        assert!(
            !crate::app::workspaces::same_path(&root, &scratch),
            "{row:?}"
        );
    }
    let refused = call(
        &state,
        "workspace.reclaim",
        json!({ "workspace_id": owner }),
    );
    assert_eq!(
        refused["ok"], false,
        "the owner is no workspace: {refused:?}"
    );

    let reclaimed = call(&state, "workspace.reclaim", json!({ "workspace_id": ws }));
    assert_eq!(reclaimed["ok"], true, "{reclaimed:?}");
    let app = state.lock().unwrap();
    assert!(!app.runs[&owner].run.state.is_terminal());
    assert_eq!(app.project_pin(&owner), Some(pin.as_str()));
    let roster: Vec<_> = app
        .entity_agents(&owner)
        .unwrap()
        .iter()
        .map(|agent| agent.id.clone())
        .collect();
    assert_eq!(roster, vec![pin, worker.clone()]);
    drop(app);
    assert!(scratch.is_dir(), "the project's scratch stands");
    assert!(
        session_live(&state, &scratch, &worker),
        "the worker runs on"
    );
}

/// Deleting the project is the one exit for its pin and workers: their
/// sessions end, their owner and Build's scratch for it go, and the
/// project's own folder — a worker's uncommitted edit included — stays.
#[tokio::test]
async fn project_deletion_retires_workers_and_preserves_sources() {
    let project = WorkerProject::new();
    let WorkerProject {
        _home,
        _tmp,
        repo,
        mut state,
        project_id,
        owner,
        pin: _,
    } = project;
    let worker = add_worker(&mut state, &owner, json!({ "name": "Editor" }));
    let state = state.shared();
    let scratch = start(&state, &owner, &worker);
    state
        .lock()
        .unwrap()
        .test_install_mcp_token(worker.clone(), "worker-token".into());
    let edit = repo.join("worker-edit.txt");
    std::fs::write(&edit, "uncommitted\n").unwrap();

    let deleted = call(
        &state,
        "project.delete",
        json!({ "project_id": project_id, "confirm": true }),
    );
    assert_eq!(deleted["ok"], true, "{deleted:?}");

    assert!(!session_live(&state, &scratch, &worker));
    let app = state.lock().unwrap();
    assert!(!app.session_registry.token_matches(&worker, "worker-token"));
    assert!(!app.runs.contains_key(&owner), "the owner went with it");
    assert_eq!(app.project_pin(&owner), None);
    assert!(app.projects.get(&project_id).is_none());
    drop(app);
    assert!(!scratch.exists(), "Build's scratch for the project went");
    assert!(repo.join(".git").is_dir(), "the source folder stays");
    assert_eq!(std::fs::read_to_string(&edit).unwrap(), "uncommitted\n");
}
