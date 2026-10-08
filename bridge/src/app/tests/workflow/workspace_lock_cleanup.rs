use super::*;
use crate::app::tests::project_agent::workspace;

fn locked_workspace(state: &mut AppState, project_id: &str) -> crate::workspace::Workspace {
    let workspace_id = workspace(state, project_id, "locked-cleanup");
    let workspace = state.workspaces.get(&workspace_id).unwrap().clone();
    // Load the persisted user choice, as a restarted daemon would. The RPC's
    // validation and persistence are covered by the workspace lock tests.
    let manifest = workspace.root.join(crate::workspace::MANIFEST_FILE);
    let mut stored: Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    stored["locked"] = json!(true);
    std::fs::write(&manifest, serde_json::to_vec(&stored).unwrap()).unwrap();
    state.workspaces.reload().unwrap();
    workspace
}

fn run_in_checkout(
    state: &mut AppState,
    project_id: &str,
    workspace: &crate::workspace::Workspace,
) -> String {
    let directory = &workspace.directories[0];
    let run_id = "run-locked-checkout".to_string();
    let mut active = crate::orchestrator::ActiveRun::workspace_conversation(
        crate::run::RunId::new(&run_id),
        workspace.name.clone(),
        directory.path.clone(),
        crate::models::ModelChoice::default(),
    );
    // A persisted adopted implementation may own one checkout inside a
    // workspace, separately from the container's conversation owner.
    active.worktree.recorded_branch = directory.branch.clone().unwrap();
    active.worktree.base_branch = directory.base_branch.clone();
    state
        .projects
        .bind_entity(run_id.clone(), project_id.to_string());
    state.finish_run_mutation(run_id.clone(), active).unwrap();
    run_id
}

fn assert_locked(reply: &Value) {
    assert_eq!(reply["ok"], false, "{reply:?}");
    assert_eq!(reply["error_code"], "locked", "{reply:?}");
    assert_eq!(reply["retryable"], false, "{reply:?}");
}

#[test]
fn project_delete_preserves_locked_workspaces_and_their_project() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let workspace = locked_workspace(&mut state, &project_id);
    let owner = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace.id }),
    ))["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();

    let reply = state.handle(req(
        "project.delete",
        json!({ "project_id": project_id, "confirm": true }),
    ));

    assert_locked(&reply);
    assert!(workspace.root.exists());
    assert!(workspace.directories[0].path.exists());
    assert!(state.projects.get(&project_id).is_some());
    assert!(state.runs.contains_key(&owner));
    assert!(state.deferred_work.is_none());

    let unlocked = state.handle(req(
        "workspace.set_locked",
        json!({ "workspace_id": workspace.id, "locked": false }),
    ));
    assert_eq!(unlocked["ok"], true, "{unlocked:?}");
    let deleted = state.handle(req(
        "project.delete",
        json!({ "project_id": project_id, "confirm": true }),
    ));
    assert_eq!(deleted["ok"], true, "{deleted:?}");
    assert!(!workspace.root.exists());
    assert!(repo.exists());
}

#[test]
fn run_abandon_cannot_delete_a_locked_workspace_container() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let workspace = locked_workspace(&mut state, &project_id);
    let owner = state.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace.id }),
    ))["result"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();

    let reply = state.handle(req("run.abandon", json!({ "run_id": owner })));

    assert_locked(&reply);
    assert!(workspace.root.exists());
    assert!(state.runs.contains_key(&owner));
    assert_eq!(state.runs[&owner].run.state, crate::run::RunState::Review);
}

#[test]
fn run_abandon_cannot_delete_a_checkout_inside_a_locked_workspace() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let workspace = locked_workspace(&mut state, &project_id);
    let run_id = run_in_checkout(&mut state, &project_id, &workspace);

    let reply = state.handle(req("run.abandon", json!({ "run_id": run_id })));

    assert_locked(&reply);
    assert!(workspace.directories[0].path.exists());
    assert_eq!(state.runs[&run_id].run.state, crate::run::RunState::Review);
}

#[test]
fn merge_preserves_a_locked_checkout_when_cleanup_keeps_it() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let project_id = state.project_at(0).id.clone();
    let workspace = locked_workspace(&mut state, &project_id);
    let run_id = run_in_checkout(&mut state, &project_id, &workspace);
    let directory = &workspace.directories[0];
    std::fs::write(directory.path.join("merged-file.txt"), "work to retain\n").unwrap();

    let refused = state.handle(req(
        "run.git_action",
        json!({ "run_id": run_id, "action": "merge", "cleanup": "prune" }),
    ));
    assert_locked(&refused);
    assert!(
        !repo.join("merged-file.txt").exists(),
        "refusal precedes the merge"
    );
    assert!(directory.path.exists());

    let committed = state.handle(req(
        "run.git_action",
        json!({ "run_id": run_id, "action": "commit" }),
    ));
    assert_eq!(committed["ok"], true, "{committed:?}");
    assert!(directory.path.exists());

    let merged = state.handle(req(
        "run.git_action",
        json!({ "run_id": run_id, "action": "merge", "cleanup": "keep" }),
    ));
    assert_eq!(merged["ok"], true, "{merged:?}");
    assert!(repo.join("merged-file.txt").exists());
    assert!(directory.path.exists());
    assert!(state.workspaces.get(&workspace.id).is_some());
}

#[test]
fn workspace_lock_cannot_be_changed_through_an_agent_tool() {
    for owner in ["project-lock-audit", "run-lock-audit"] {
        let server = crate::mcp::DoneServer::for_owner(owner);
        let listed = server.handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        let listed: Value = serde_json::from_str(listed.reply.as_deref().unwrap()).unwrap();
        let tools = listed["result"]["tools"].as_array().unwrap();
        for name in [
            "workspace.set_locked",
            "set_workspace_locked",
            "unlock_workspace",
        ] {
            assert!(!tools.iter().any(|tool| tool["name"] == name));
            let refused = server.handle_message(&json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": name, "arguments": { "workspace_id": "ws-locked", "locked": false } },
            }).to_string());
            assert!(
                refused.action.is_none(),
                "{owner} received an unlock action"
            );
            let refused: Value = serde_json::from_str(refused.reply.as_deref().unwrap()).unwrap();
            let response = refused.to_string();
            assert!(response.contains("unknown tool"), "{refused:?}");
        }
    }
}
