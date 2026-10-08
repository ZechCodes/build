use super::*;

const LOCKED: &str = "Workspace is locked. Unlock it to delete it.";

fn set_locked(state: &mut AppState, id: &str, locked: bool) -> Value {
    let reply = state.handle(req(
        "workspace.set_locked",
        json!({ "workspace_id": id, "locked": locked }),
    ));
    assert_eq!(reply["ok"], true, "{reply:?}");
    assert_eq!(reply["result"]["locked"], locked);
    reply["result"].clone()
}

#[test]
fn workspace_lock_refuses_every_removal_and_unlock_allows_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "source");
    let mut state = app(tmp.path());
    let project = state.add_project(repo, "main".into());
    let created = create_workspace(&mut state, &project, "protected");
    let id = created["workspace_id"].as_str().unwrap();
    assert_eq!(created["locked"], false);
    set_locked(&mut state, id, true);
    for method in ["workspace.delete", "workspace.finish", "workspace.reclaim"] {
        let reply = state.handle(req(method, json!({ "workspace_id": id })));
        assert_eq!(reply["ok"], false, "{reply:?}");
        assert_eq!(reply["error_code"], "locked", "{reply:?}");
        assert_eq!(reply["error"], LOCKED);
        assert_eq!(reply["retryable"], false);
        assert!(Path::new(created["root"].as_str().unwrap()).exists());
        assert!(state.deferred_work.is_none());
    }
    let renamed = state.handle(req(
        "workspace.rename",
        json!({"workspace_id": id, "name": "still editable"}),
    ));
    assert_eq!(renamed["ok"], true, "{renamed:?}");
    assert_eq!(renamed["result"]["locked"], true);
    set_locked(&mut state, id, false);
    let deleted = state.handle(req("workspace.delete", json!({ "workspace_id": id })));
    assert_eq!(deleted["ok"], true, "{deleted:?}");
    assert!(!Path::new(created["root"].as_str().unwrap()).exists());
}

#[test]
fn workspace_lock_survives_restart_and_lists_from_the_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "source");
    let mut state = app(tmp.path());
    let project = state.add_project(repo.clone(), "main".into());
    let created = create_workspace(&mut state, &project, "persistent");
    let id = created["workspace_id"].as_str().unwrap();
    set_locked(&mut state, id, true);
    drop(state);
    let mut restored = app(tmp.path());
    restored.add_project(repo, "main".into());
    let detail = restored.handle(req("workspace.get", json!({ "workspace_id": id })));
    assert_eq!(detail["result"]["locked"], true, "{detail:?}");
    let listed = restored.handle(req("workspace.list", json!({})));
    let row = listed["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id)
        .unwrap();
    assert_eq!(row["locked"], true);
    // Manifests written by older bridges default to unlocked.
    let manifest =
        Path::new(created["root"].as_str().unwrap()).join(crate::workspace::MANIFEST_FILE);
    let mut old: Value = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
    old.as_object_mut().unwrap().remove("locked");
    std::fs::write(&manifest, serde_json::to_vec(&old).unwrap()).unwrap();
    let mut legacy = app(tmp.path());
    let detail = legacy.handle(req("workspace.get", json!({"workspace_id": id})));
    assert_eq!(detail["result"]["locked"], false, "{detail:?}");
}

#[test]
fn workspace_lock_refuses_agent_delete_and_reclaim_and_project_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "source");
    let mut state = app(tmp.path());
    let project = state.add_project(repo, "main".into());
    let created = create_workspace(&mut state, &project, "protected");
    let id = created["workspace_id"].as_str().unwrap();
    let (owner, agent) = super::super::project_agent::project_agent(&mut state, &project);
    set_locked(&mut state, id, true);
    for action in [
        crate::mcp::BridgeAction::DeleteWorkspace {
            workspace_id: id.to_string(),
        },
        crate::mcp::BridgeAction::ReclaimWorkspace {
            workspace_id: id.to_string(),
        },
    ] {
        let error = state.agent_action(&owner, &agent, action).unwrap_err();
        assert_eq!(error, LOCKED);
    }
    let reply = state.handle(req(
        "project.delete",
        json!({ "project_id": project, "confirm": true }),
    ));
    assert_eq!(reply["ok"], false, "{reply:?}");
    assert_eq!(reply["error_code"], "locked", "{reply:?}");
    assert!(state.projects.get(&project).is_some());
    assert!(Path::new(created["root"].as_str().unwrap()).exists());
}

#[test]
fn workspace_lock_requires_a_boolean_and_known_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = app(tmp.path());
    for params in [
        json!({ "workspace_id": "missing" }),
        json!({ "workspace_id": "missing", "locked": "true" }),
    ] {
        let reply = state.handle(req("workspace.set_locked", params));
        assert_eq!(reply["error_code"], "invalid_params", "{reply:?}");
    }
    let reply = state.handle(req(
        "workspace.set_locked",
        json!({ "workspace_id": "missing", "locked": true }),
    ));
    assert_eq!(reply["error_code"], "not_found", "{reply:?}");
}

#[test]
fn workspace_lock_reports_a_hold_to_the_automatic_sweep() {
    use super::super::workspace_reclaim::{call, lifecycle, linked_workspace, now_ms, pruning};
    let (_tmp, shared, _project, id, _task) = linked_workspace();
    let locked = call(
        &shared,
        "workspace.set_locked",
        json!({"workspace_id": id, "locked": true}),
    );
    assert_eq!(locked["ok"], true, "{locked:?}");
    AppState::sweep_workspaces(&shared, &pruning(), now_ms());
    let verdict = lifecycle(&shared, &id);
    assert_eq!(verdict["reclaimable"], false);
    assert!(
        verdict["holds"]
            .as_array()
            .unwrap()
            .contains(&json!("locked")),
        "{verdict:?}"
    );
    assert_eq!(verdict["pruned_bytes"], 0);
    assert!(shared
        .lock()
        .unwrap()
        .workspaces
        .get(&id)
        .unwrap()
        .root
        .exists());
}

#[test]
fn workspace_lock_cannot_change_a_reserved_or_removing_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "source");
    let mut state = app(tmp.path());
    let project = state.add_project(repo, "main".into());
    let created = create_workspace(&mut state, &project, "reserved");
    let id = created["workspace_id"].as_str().unwrap();
    state.reserve_workspace_for_test(id);
    let reply = state.handle(req(
        "workspace.set_locked",
        json!({ "workspace_id": id, "locked": true }),
    ));
    assert_eq!(reply["error_code"], "busy", "{reply:?}");
    assert_eq!(
        state.workspace_get(&json!({"workspace_id": id})).unwrap()["locked"],
        false
    );
}

#[test]
fn workspace_lock_on_adopted_checkout_survives_re_adoption_without_writing_into_it() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "source");
    let mut state = app(tmp.path());
    let project = state.add_project(repo.clone(), "main".into());
    let listed = state.workspace_list(&json!({})).unwrap();
    drop(listed);
    let id = format!("legacy-{project}");
    set_locked(&mut state, &id, true);
    assert!(!repo.join(crate::workspace::MANIFEST_FILE).exists());
    drop(state);
    let mut restored = app(tmp.path());
    let restored_project = restored.add_project(repo.clone(), "main".into());
    assert_eq!(project, restored_project);
    let detail = restored.handle(req("workspace.get", json!({"workspace_id": id})));
    assert_eq!(detail["result"]["locked"], true, "{detail:?}");
    set_locked(&mut restored, &id, false);
    assert!(!repo.join(crate::workspace::MANIFEST_FILE).exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_lock_push_carries_the_cached_row() {
    use super::super::push::{greeted_push_session, pushes_until};
    let (dir, repo) = init_repo();
    let (shared, handler, sender, mut receiver, key) = greeted_push_session(&repo, dir.path());
    let subscribed = handler.call(
        sender.clone(),
        req(
            "changes.subscribe",
            json!({
                "subscription_id": "lock-board", "scope": {"kind": "all"}, "kinds": ["state"]
            }),
        ),
    );
    assert_eq!(subscribed["ok"], true, "{subscribed:?}");
    let project = shared.lock().unwrap().project_at(0).id.clone();
    let created = handler.call(
        sender.clone(),
        req(
            "workspace.create",
            json!({
                "project_id": project, "name": "push lock", "isolation": "worktree"
            }),
        ),
    );
    let id = created["result"]["workspace_id"].as_str().unwrap();
    let locked = handler.call(
        sender.clone(),
        req(
            "workspace.set_locked",
            json!({"workspace_id": id, "locked": true}),
        ),
    );
    assert_eq!(locked["ok"], true, "{locked:?}");
    let carries_lock = |pushes: &[Value]| {
        pushes
            .iter()
            .filter(|push| push["type"] == "changes")
            .flat_map(|push| push["items"].as_array().cloned().unwrap_or_default())
            .filter_map(|item| item["state"]["workspaces"].as_array().cloned())
            .flatten()
            .any(|row| row["workspace_id"] == id && row["locked"] == true)
    };
    let pushes = pushes_until(&mut receiver, &key, carries_lock).await;
    assert!(carries_lock(&pushes), "{pushes:?}");
}
