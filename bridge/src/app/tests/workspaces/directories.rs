//! `workspace.add_directory` / `workspace.remove_directory`: a workspace's
//! directories after it was cut.
//!
//! A workspace is cut from the project's sources as they stood that day. These
//! two verbs are how it gains one afterwards and how it loses one, without
//! going back to the project.

use super::*;

/// The directories of one workspace, as `workspace.get` answers them.
fn directories(state: &mut AppState, workspace_id: &str) -> Vec<Value> {
    let read = state.handle(req("workspace.get", json!({"workspace_id": workspace_id})));
    assert_eq!(read["ok"], true, "{read:?}");
    read["result"]["directories"]
        .as_array()
        .unwrap_or_else(|| panic!("{read:?}"))
        .clone()
}

fn head_of(path: &Path) -> String {
    crate::git_process::run_git(path, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string()
}

/// A Git source added to a live workspace lands the way the ones cut with it
/// did: its own checkout, on a branch of the workspace's own, standing where
/// the source's base branch stands.
#[test]
fn add_directory_cuts_a_git_source_onto_a_workspace_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "code");
    let extra = init_repo_named(tmp.path(), "docs");
    let mut state = app(tmp.path());
    let added = state.handle(req("project.add", json!({"path": repo})));
    let project_id = added["result"]["project_id"].as_str().unwrap().to_string();
    let workspace = create_workspace(&mut state, &project_id, "work");
    let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();
    let source = state.handle(req(
        "project.add_source",
        json!({"project_id": project_id, "path": extra, "name": "docs"}),
    ));
    assert_eq!(source["ok"], true, "{source:?}");
    let source_id = source["result"]["sources"][1]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let grown = state.handle(req(
        "workspace.add_directory",
        json!({"workspace_id": workspace_id, "source_id": source_id}),
    ));

    assert_eq!(grown["ok"], true, "{grown:?}");
    let listed = grown["result"]["directories"].as_array().unwrap();
    assert_eq!(listed.len(), 2, "{grown:?}");
    let added = &listed[1];
    assert_eq!(added["source_id"], source_id);
    assert_eq!(added["status"], "ready", "{grown:?}");
    assert_eq!(added["is_git"], true);
    let branch = added["branch"].as_str().unwrap().to_string();
    assert_ne!(branch, "main", "the checkout gets a branch of its own");
    let path = PathBuf::from(added["path"].as_str().unwrap());
    assert_eq!(
        path.parent().unwrap(),
        Path::new(workspace["root"].as_str().unwrap()),
        "the directory lands inside the workspace root"
    );
    assert_eq!(
        head_of(&path),
        head_of(&extra),
        "the branch is cut where the source's base branch stands"
    );
    // The read every client makes agrees with what the verb answered.
    assert_eq!(directories(&mut state, &workspace_id).len(), 2);
}

/// A folder that is nobody's project source is copied in under the name it was
/// given. Nothing about the project changes.
#[test]
fn add_directory_copies_a_folder_that_is_no_project_source() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "code");
    let assets = tmp.path().join("assets");
    std::fs::create_dir(&assets).unwrap();
    std::fs::write(assets.join("logo.svg"), b"<svg/>").unwrap();
    let mut state = app(tmp.path());
    let added = state.handle(req("project.add", json!({"path": repo})));
    let project_id = added["result"]["project_id"].as_str().unwrap().to_string();
    let workspace = create_workspace(&mut state, &project_id, "work");
    let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();

    let grown = state.handle(req(
        "workspace.add_directory",
        json!({"workspace_id": workspace_id, "path": assets, "name": "assets"}),
    ));

    assert_eq!(grown["ok"], true, "{grown:?}");
    let listed = grown["result"]["directories"].as_array().unwrap();
    assert_eq!(listed.len(), 2, "{grown:?}");
    assert_eq!(listed[1]["name"], "assets");
    assert_eq!(listed[1]["is_git"], false);
    let path = PathBuf::from(listed[1]["path"].as_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(path.join("logo.svg")).unwrap(),
        "<svg/>"
    );
    assert!(assets.is_dir(), "the folder it was copied from stays");
    let projects = state.handle(req("project.list", json!({})));
    assert_eq!(
        projects["result"]["projects"][0]["sources"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "a workspace directory is not a project source: {projects:?}"
    );
}

#[test]
fn add_directory_refuses_a_workspace_this_bridge_does_not_have() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "code");
    let mut state = app(tmp.path());
    state.handle(req("project.add", json!({"path": repo})));

    let refused = state.handle(req(
        "workspace.add_directory",
        json!({"workspace_id": "ws-nope", "path": tmp.path()}),
    ));

    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(refused["error_code"], "not_found", "{refused:?}");
}

/// Removing a directory hands its checkout back: the worktree registration in
/// the source repository goes, so the same name can be cut again, and the
/// folder leaves the workspace while its neighbour stays.
#[test]
fn remove_directory_unregisters_the_worktree_and_takes_the_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "code");
    let extra = init_repo_named(tmp.path(), "docs");
    let mut state = app(tmp.path());
    let project = state.handle(req(
        "project.add",
        json!({
            "name": "mixed",
            "sources": [{"name": "code", "path": repo}, {"name": "docs", "path": extra}],
        }),
    ));
    assert_eq!(project["ok"], true, "{project:?}");
    let project_id = project["result"]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let workspace = create_workspace(&mut state, &project_id, "work");
    let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();
    let listed = directories(&mut state, &workspace_id);
    let removed_path = PathBuf::from(listed[1]["path"].as_str().unwrap());
    let kept_path = PathBuf::from(listed[0]["path"].as_str().unwrap());
    let directory_id = listed[1]["id"].as_str().unwrap().to_string();
    let worktrees_before =
        crate::git_process::run_git(&extra, &["worktree", "list", "--porcelain"]).unwrap();
    assert!(worktrees_before.contains(removed_path.to_str().unwrap()));

    let shrunk = state.handle(req(
        "workspace.remove_directory",
        json!({"workspace_id": workspace_id, "directory_id": directory_id}),
    ));

    assert_eq!(shrunk["ok"], true, "{shrunk:?}");
    assert_eq!(
        shrunk["result"]["directories"].as_array().unwrap().len(),
        1,
        "{shrunk:?}"
    );
    assert!(!removed_path.exists(), "the directory is gone");
    assert!(kept_path.exists(), "its neighbour is untouched");
    assert!(extra.join(".git").is_dir(), "the source repository stays");
    assert!(
        !crate::git_process::run_git(&extra, &["worktree", "list", "--porcelain"])
            .unwrap()
            .contains(removed_path.to_str().unwrap()),
        "the worktree registration went with it"
    );
    assert_eq!(directories(&mut state, &workspace_id).len(), 1);
}

#[test]
fn remove_directory_refuses_a_directory_the_workspace_does_not_have() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "code");
    let mut state = app(tmp.path());
    let added = state.handle(req("project.add", json!({"path": repo})));
    let project_id = added["result"]["project_id"].as_str().unwrap().to_string();
    let workspace = create_workspace(&mut state, &project_id, "work");
    let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();

    let refused = state.handle(req(
        "workspace.remove_directory",
        json!({"workspace_id": workspace_id, "directory_id": "nope"}),
    ));

    assert_eq!(refused["ok"], false, "{refused:?}");
    assert_eq!(refused["error_code"], "not_found", "{refused:?}");
    assert_eq!(directories(&mut state, &workspace_id).len(), 1);
}
