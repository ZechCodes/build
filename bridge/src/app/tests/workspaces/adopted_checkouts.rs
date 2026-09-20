//! What Build does with checkouts it did not make: it shows the ones that are
//! there and takes away the rows of the ones that are not.

use super::*;

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

/// The one id the checkout added since `baseline`.
fn adopted_id(state: &mut AppState, project_id: &str, baseline: &[String]) -> String {
    let ids = workspace_ids(state, project_id);
    let mut added = ids
        .iter()
        .filter(|id| !baseline.contains(id))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(added.len(), 1, "expected one adopted row, got {ids:?}");
    added.pop().unwrap()
}

#[test]
fn an_adopted_checkout_row_goes_when_its_folder_does() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "repo");
    let mut state = app(tmp.path());
    let project_id = add_repo_project(&mut state, &repo);
    let baseline = workspace_ids(&mut state, &project_id);
    let checkout = tmp.path().join("hotfix");
    git_in(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "hotfix",
            checkout.to_str().unwrap(),
        ],
    );
    let adopted = adopted_id(&mut state, &project_id, &baseline);

    // The folder goes; Git's worktree registry still names it, as it does
    // until somebody prunes it.
    std::fs::remove_dir_all(&checkout).unwrap();

    assert!(
        !workspace_ids(&mut state, &project_id).contains(&adopted),
        "a checkout that is not on disk is not a workspace"
    );
}

#[test]
fn an_adopted_checkout_row_goes_when_git_stops_listing_it() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "repo");
    let mut state = app(tmp.path());
    let project_id = add_repo_project(&mut state, &repo);
    let baseline = workspace_ids(&mut state, &project_id);
    let checkout = tmp.path().join("hotfix");
    git_in(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "hotfix",
            checkout.to_str().unwrap(),
        ],
    );
    let adopted = adopted_id(&mut state, &project_id, &baseline);

    // Removed as a worktree, and the folder left standing as an ordinary
    // directory: the path exists, the registry entry does not.
    git_in(
        &repo,
        &["worktree", "remove", "--force", checkout.to_str().unwrap()],
    );
    std::fs::create_dir_all(&checkout).unwrap();

    assert!(
        !workspace_ids(&mut state, &project_id).contains(&adopted),
        "a folder Git no longer calls a worktree is not a workspace"
    );
}
