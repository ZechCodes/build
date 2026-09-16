use super::*;
use crate::isolation::Isolation;
use crate::workspace::{WorkspaceRegistry, WorkspaceSource};
use crate::worktree::WorktreeManager;

fn add_repo_project(state: &mut AppState, repo: &Path) -> String {
    let added = state.handle(req("project.add", json!({"path": repo})));
    assert_eq!(added["ok"], true, "{added:?}");
    added["result"]["project_id"].as_str().unwrap().to_string()
}

fn workspace_ids(state: &mut AppState, project_id: &str) -> Vec<String> {
    let listed = state.handle(req("workspace.list", json!({"project_id": project_id})));
    assert_eq!(listed["ok"], true, "{listed:?}");
    let mut ids = listed["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|workspace| workspace["workspace_id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

#[test]
fn listing_during_private_materialization_does_not_adopt_checkout_that_later_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "repo");
    let mut state = app(tmp.path());
    let project_id = add_repo_project(&mut state, &repo);
    let baseline = workspace_ids(&mut state, &project_id);
    let sources = state
        .sources_for(&project_id)
        .unwrap()
        .into_iter()
        .map(|source| WorkspaceSource {
            id: source.id,
            name: source.name,
            mount: source.mount,
            path: source.path,
            is_git: source.is_git,
            base_branch: source.base_branch,
        })
        .collect::<Vec<_>>();
    let mut registry = WorkspaceRegistry::load(state.workspaces.root()).unwrap();
    let workspace = registry
        .prepare_with_isolation(&project_id, "pending", &sources, Isolation::Worktree)
        .unwrap();
    let mut checkout = None;
    let result = registry.provision_unpublished(
        workspace.clone(),
        &sources,
        Isolation::Worktree,
        |source, destination, isolation| {
            let manager = WorktreeManager::new(&source.path, destination.parent().unwrap());
            checkout = Some(
                manager
                    .create_workspace_checkout(
                        "pending",
                        &source.base_branch,
                        destination,
                        isolation,
                    )
                    .unwrap()
                    .worktree,
            );
            // Interleave the read after Git registration but before publication,
            // exactly while the filesystem worker has released the app lock.
            assert_eq!(workspace_ids(&mut state, &project_id), baseline);
            Err("forced materialization failure".to_string())
        },
    );
    assert!(result.is_err());
    WorktreeManager::new(&repo, &workspace.root)
        .remove(&checkout.unwrap())
        .unwrap();
    std::fs::remove_dir_all(&workspace.root).unwrap();
    state.workspaces.reload().unwrap();
    assert_eq!(workspace_ids(&mut state, &project_id), baseline);
}

#[test]
fn listing_after_publication_before_settlement_does_not_adopt_workspace_mount() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "repo");
    let mut state = app(tmp.path());
    let project_id = add_repo_project(&mut state, &repo);
    let mut expected = workspace_ids(&mut state, &project_id);
    let params = json!({"project_id": project_id, "name": "published", "isolation": "worktree"});
    let (dispatched, deferred) = state.dispatch_deferring("workspace.create", &params);
    assert!(dispatched.is_ok());
    let done = deferred.expect("creation defers filesystem work").run();
    // The manifest now exists, but the app has not reloaded its registry.
    assert_eq!(workspace_ids(&mut state, &project_id), expected);
    let created = state
        .apply_deferred("workspace.create", &params, done)
        .unwrap();
    expected.push(created["workspace_id"].as_str().unwrap().to_string());
    expected.sort();
    assert_eq!(workspace_ids(&mut state, &project_id), expected);
}
