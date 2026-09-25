//! Done with the branch deleted: `branch.finish` with `action: "delete"`
//! removes the workspace, then the local branch the finish resolved — and
//! refuses before anything is touched when the branch is a default branch,
//! is checked out somewhere else or holds commits no remote has.

use super::*;

/// A workspace on `repo`'s project, and the branch its checkout carries.
fn workspace_on_branch(state: &mut AppState, repo: &Path, name: &str) -> (String, String) {
    let added = state.handle(req("project.add", json!({"path": repo})));
    assert_eq!(added["ok"], true, "{added:?}");
    let project_id = added["result"]["project_id"].as_str().unwrap().to_string();
    let workspace = create_workspace(state, &project_id, name);
    let branch = workspace["directories"][0]["branch"]
        .as_str()
        .unwrap()
        .to_string();
    (project_id, branch)
}

fn local_branches(repo: &Path) -> Vec<String> {
    let out = std::process::Command::new("git")
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(repo)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn done_with_delete_removes_the_local_branch_it_finished() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, _) = repo_with_origin(tmp.path(), "repo");
    let mut state = app(tmp.path());
    let (project_id, branch) = workspace_on_branch(&mut state, &repo, "delete-me");
    assert!(local_branches(&repo).contains(&branch));

    let finished = state.handle(req(
        "branch.finish",
        json!({"project_id": project_id, "branch": branch, "action": "delete"}),
    ));
    assert_eq!(finished["ok"], true, "{finished:?}");
    assert_eq!(finished["result"]["deleted"], true, "{finished:?}");
    assert!(
        !local_branches(&repo).contains(&branch),
        "{branch} is still in {:?}",
        local_branches(&repo)
    );
}

#[test]
fn done_without_delete_keeps_the_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, _) = repo_with_origin(tmp.path(), "repo");
    let mut state = app(tmp.path());
    let (project_id, branch) = workspace_on_branch(&mut state, &repo, "keep-me");

    let finished = state.handle(req(
        "branch.finish",
        json!({"project_id": project_id, "branch": branch}),
    ));
    assert_eq!(finished["ok"], true, "{finished:?}");
    assert_eq!(finished["result"]["deleted"], true, "{finished:?}");
    assert!(
        finished["result"].get("branch_deleted").is_none(),
        "{finished:?}"
    );
    assert!(local_branches(&repo).contains(&branch));
}

/// The user's own checkout moved onto the branch: Done refuses as a whole,
/// before a file or an agent is touched, and says where the branch is.
#[test]
fn done_with_delete_refuses_a_branch_checked_out_elsewhere_and_touches_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, _) = repo_with_origin(tmp.path(), "repo");
    let mut state = app(tmp.path());
    let (project_id, branch) = workspace_on_branch(&mut state, &repo, "in-use");
    git_in(
        &repo,
        &["switch", "-q", "--ignore-other-worktrees", &branch],
    );
    let listed = state.handle(req("workspace.list", json!({"project_id": project_id})));
    let workspace_id = listed["result"]["workspaces"][0]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();

    let refused = state.handle(req(
        "branch.finish",
        json!({"project_id": project_id, "branch": branch, "action": "delete"}),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    let error = refused["error"].as_str().unwrap();
    assert!(
        error.starts_with(&format!(
            "Build cannot delete the branch {branch}: it is checked out at "
        )),
        "{error}"
    );
    assert_eq!(refused["error_code"], "conflict", "{refused:?}");
    let still = state.handle(req("workspace.get", json!({"workspace_id": workspace_id})));
    assert_eq!(still["ok"], true, "the workspace is untouched: {still:?}");
    assert!(local_branches(&repo).contains(&branch));
}

/// A workspace switched onto `main`: finishing it by that name with delete
/// refuses as a whole, and local `main` stays.
#[test]
fn done_with_delete_never_deletes_main() {
    let tmp = tempfile::tempdir().unwrap();
    let (repo, _) = repo_with_origin(tmp.path(), "repo");
    let mut state = app(tmp.path());
    let added = state.handle(req("project.add", json!({"path": repo})));
    let project_id = added["result"]["project_id"].as_str().unwrap().to_string();
    let workspace = create_workspace(&mut state, &project_id, "on-main");
    let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();
    let checkout = PathBuf::from(workspace["directories"][0]["path"].as_str().unwrap());
    git_in(&repo, &["switch", "-q", "--detach"]);
    git_in(&checkout, &["switch", "-q", "main"]);
    let refreshed = state.handle(req("workspace.get", json!({"workspace_id": workspace_id})));
    assert_eq!(
        refreshed["result"]["directories"][0]["branch"], "main",
        "{refreshed:?}"
    );

    let refused = state.handle(req(
        "branch.finish",
        json!({"project_id": project_id, "branch": "main", "action": "delete"}),
    ));
    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(
        refused["error"],
        "Build cannot delete the branch main: it is a default branch."
    );
    assert_eq!(refused["error_code"], "conflict", "{refused:?}");
    let still = state.handle(req("workspace.get", json!({"workspace_id": workspace_id})));
    assert_eq!(still["ok"], true, "the workspace is untouched: {still:?}");
    assert!(local_branches(&repo).contains(&"main".to_string()));
}
