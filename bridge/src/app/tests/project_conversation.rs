//! `project.ensure_conversation`: the owner a project mints so it can be
//! talked to like a workspace, and the bridge-owned scratch directory that
//! owner's agents work in.

use super::*;

/// A context rooted in the test's own directory, so the project scratch this
/// cuts lands under `tmp` and never in the developer's `~/.build`.
fn context(state_root: &Path) -> HarnessContext {
    HarnessContext::resolved(state_root.join("mcp.sock"), state_root.to_path_buf()).unwrap()
}

fn rooted(state_root: &Path) -> AppState {
    AppState::new_unrooted_configured(
        state_root.join("worktrees"),
        "main",
        true,
        context(state_root),
    )
}

fn ensure(state: &mut AppState, project_id: &str) -> Value {
    state.handle(req(
        "project.ensure_conversation",
        json!({ "project_id": project_id }),
    ))
}

fn added_project(state: &mut AppState, repo: &Path) -> String {
    let project = state.handle(req("project.add", json!({ "path": repo })));
    assert_eq!(project["ok"], true, "{project:?}");
    project["result"]["project_id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn project_conversation_owns_a_bridge_scratch_root_and_is_idempotent() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);

    let first = ensure(&mut state, &project_id);
    let second = ensure(&mut state, &project_id);
    assert_eq!(first["ok"], true, "{first:?}");
    assert_eq!(second["result"], first["result"], "{second:?}");
    let run_id = first["result"]["run_id"].as_str().unwrap();
    assert_eq!(first["result"]["entity_id"], run_id, "{first:?}");
    assert_eq!(first["result"]["project_id"], project_id, "{first:?}");
    assert_eq!(state.runs.len(), 1);

    // The owner works in Build's own directory, not in the project's checkout:
    // the project IS the template workspaces are cut from, and an agent talking
    // about it must not be standing in it.
    let root = state.runs[run_id].worktree.path.clone();
    assert!(root.is_dir(), "{}", root.display());
    assert!(
        root.starts_with(state_root.join("project-scratch")),
        "{}",
        root.display()
    );
    assert_ne!(root, repo);
    assert!(!root.starts_with(&repo), "{}", root.display());

    let added = state.handle(req("agent.add", json!({ "entity_id": run_id })));
    assert_eq!(added["ok"], true, "{added:?}");
    assert_eq!(
        state.entity_agent_root(run_id).unwrap(),
        AppState::canonical_root(&root)
    );
}

#[test]
fn project_conversation_survives_a_restart_that_re_mints_the_project_id() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let config = state_root.join("config.json");
    let store = state_root.join("store");
    let worktrees = state_root.join("worktrees");
    let boot = || {
        AppState::new_unrooted_configured(&worktrees, "main", true, context(&state_root))
            .with_config(&config)
            .unwrap()
            .with_task_store(&store)
            .unwrap()
    };

    let (run_id, scratch) = {
        let mut state = boot();
        let project_id = added_project(&mut state, &repo);
        assert_eq!(project_id, "proj-1");
        let ensured = ensure(&mut state, &project_id);
        assert_eq!(ensured["ok"], true, "{ensured:?}");
        let run_id = ensured["result"]["run_id"].as_str().unwrap().to_string();
        let scratch = state.runs[&run_id].worktree.path.clone();
        (run_id, scratch)
    };

    // `proj-N` is not durable — a restore can hand the same repository another
    // one. The owner is keyed by the project's canonical path, so it survives.
    let mut stored: Value =
        serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
    stored["projects"][0]["id"] = json!("proj-7");
    std::fs::write(&config, serde_json::to_string(&stored).unwrap()).unwrap();

    let mut restarted = boot();
    assert!(restarted.projects.get("proj-7").is_some(), "the id moved");
    let ensured = ensure(&mut restarted, "proj-7");
    assert_eq!(ensured["result"]["run_id"], run_id, "{ensured:?}");
    assert_eq!(restarted.runs.len(), 1);
    assert!(scratch.is_dir(), "a project's scratch is never wiped");
}

#[test]
fn project_conversation_persistence_failure_leaves_no_owner_and_can_retry() {
    let (_repo_home, repo) = init_repo();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root)
        .with_task_store(state_root.join("store"))
        .unwrap();
    let project_id = added_project(&mut state, &repo);
    state.store.as_ref().unwrap().fail_next_write();

    let failed = ensure(&mut state, &project_id);
    assert_eq!(failed["ok"], false, "{failed:?}");
    assert!(failed["error"]
        .as_str()
        .unwrap()
        .contains("injected store failure"));
    assert!(state.runs.is_empty());

    let retried = ensure(&mut state, &project_id);
    assert_eq!(retried["ok"], true, "{retried:?}");
    assert_eq!(state.runs.len(), 1);
}

#[test]
fn project_conversation_refuses_a_project_this_bridge_does_not_know() {
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);

    let refused = ensure(&mut state, "proj-9");
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(refused["error_code"], "not_found", "{refused:?}");
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("unknown project_id: proj-9"));
    assert!(state.runs.is_empty());
}

/// The owner decides which kind of agent is minted, so a project agent wears
/// the prefix that says so — and the same verbs on a workspace owner keep
/// minting coding agents.
#[test]
fn a_project_owners_agents_are_project_agents_and_a_workspaces_are_not() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root);
    let project_id = added_project(&mut state, &repo);
    let owner = ensure(&mut state, &project_id)["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();

    let added = state.handle(req("agent.add", json!({ "entity_id": owner })));
    let agent_id = added["result"]["agent"]["id"].as_str().unwrap();
    assert!(
        crate::agent::is_project_agent(agent_id),
        "{added:?} is on the project surface"
    );

    // The path that MUST be heard mints the same kind: a message to a project
    // conversation nobody is on is still the project's agent.
    let (_other_home, other_repo) = init_repo();
    let other = added_project(&mut state, &std::fs::canonicalize(&other_repo).unwrap());
    let other_owner = ensure(&mut state, &other)["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let posted = state.handle(req(
        "thread.post",
        json!({ "entity_id": other_owner, "body": "what is in here?" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let minted = state
        .entity_agents(&other_owner)
        .unwrap()
        .primary()
        .unwrap();
    assert!(crate::agent::is_project_agent(&minted.id), "{}", minted.id);

    // A workspace of the same project is an ordinary checkout, and its agents
    // are ordinary coding agents.
    let workspace = state.handle(req(
        "workspace.create",
        json!({ "project_id": project_id, "name": "work", "isolation": "worktree" }),
    ));
    assert_eq!(workspace["ok"], true, "{workspace:?}");
    let ensured = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace["result"]["workspace_id"] }),
    ));
    let workspace_owner = ensured["result"]["run_id"].as_str().unwrap();
    let workspace_agent = state.handle(req("agent.add", json!({ "entity_id": workspace_owner })));
    let workspace_agent = workspace_agent["result"]["agent"]["id"].as_str().unwrap();
    assert!(
        workspace_agent.starts_with(crate::agent::AGENT_ID_PREFIX),
        "{workspace_agent}"
    );
}

/// Abandoning the owner ends the conversation, never the directory it was
/// held in. The scratch is the project's, not the run's — it outlives every
/// session that works in it — so abandon lets go of the checkout the way it
/// does for a run standing in the project's repository.
#[test]
fn abandoning_a_project_conversation_owner_keeps_its_scratch() {
    let (_repo_home, repo) = init_repo();
    let repo = std::fs::canonicalize(&repo).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let state_root = std::fs::canonicalize(tmp.path()).unwrap();
    let mut state = rooted(&state_root)
        .with_task_store(state_root.join("store"))
        .unwrap();
    let project_id = added_project(&mut state, &repo);
    let ensured = ensure(&mut state, &project_id);
    let run_id = ensured["result"]["run_id"].as_str().unwrap().to_string();
    let scratch = state.runs[&run_id].worktree.path.clone();
    // What the project agent left behind: the scaffold it works out of.
    std::fs::write(scratch.join("NOTES.md"), "what the project agent wrote").unwrap();

    let abandoned = state.handle(req("run.abandon", json!({ "run_id": run_id })));

    assert_eq!(abandoned["ok"], true, "{abandoned:?}");
    assert!(scratch.is_dir(), "{}", scratch.display());
    assert_eq!(
        std::fs::read_to_string(scratch.join("NOTES.md")).unwrap(),
        "what the project agent wrote",
        "abandon took the project's durable scratch with it"
    );
    // And the next conversation is handed the same directory back.
    let again = ensure(&mut state, &project_id);
    assert_eq!(again["ok"], true, "{again:?}");
    let next = again["result"]["run_id"].as_str().unwrap();
    assert_ne!(next, run_id, "the abandoned owner is gone: {again:?}");
    assert_eq!(state.runs[next].worktree.path, scratch);
}
