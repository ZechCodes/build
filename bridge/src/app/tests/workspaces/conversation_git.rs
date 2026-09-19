// A workspace conversation's run stands on the workspace root, which is not a
// repository: the workspace's git is in its git directory. Every run-scoped
// git read, and the git facts a push carries for that run, answer from there —
// which is what the client files under the workspace's entity.

use super::*;

fn conversation_run(state: &mut AppState, workspace_id: &str) -> String {
    let conversation = state.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace_id}),
    ));
    assert_eq!(conversation["ok"], true, "{conversation:?}");
    conversation["result"]["entity_id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn a_workspace_conversation_reads_git_from_its_git_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "code");
    let mut state = app(tmp.path());
    let added = state.handle(req("project.add", json!({"path": repo})));
    let project_id = added["result"]["project_id"].as_str().unwrap().to_string();
    let workspace = create_workspace(&mut state, &project_id, "work");
    let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();
    let git_dir = PathBuf::from(workspace["directories"][0]["path"].as_str().unwrap());
    let run_id = conversation_run(&mut state, &workspace_id);
    std::fs::write(git_dir.join("note.txt"), b"hello").unwrap();

    let status = state.handle(req("git.status", json!({"run_id": run_id})));
    assert_eq!(status["ok"], true, "{status:?}");
    let files: Vec<&str> = status["result"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file["path"].as_str().unwrap())
        .collect();
    assert_eq!(files, vec!["note.txt"]);

    let log = state.handle(req("git.log", json!({"run_id": run_id})));
    assert_eq!(log["ok"], true, "{log:?}");
    assert!(!log["result"]["commits"].as_array().unwrap().is_empty());

    let unpushed = state.handle(req("git.unpushed", json!({"run_id": run_id})));
    assert_eq!(unpushed["ok"], true, "{unpushed:?}");
    let diff = state.handle(req("run.diff", json!({"run_id": run_id})));
    assert_eq!(diff["ok"], true, "{diff:?}");
}

#[test]
fn a_workspace_conversation_is_a_git_subject_at_its_git_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = init_repo_named(tmp.path(), "code");
    let mut state = app(tmp.path());
    let added = state.handle(req("project.add", json!({"path": repo})));
    let project_id = added["result"]["project_id"].as_str().unwrap().to_string();
    let workspace = create_workspace(&mut state, &project_id, "work");
    let workspace_id = workspace["workspace_id"].as_str().unwrap().to_string();
    let git_dir = PathBuf::from(workspace["directories"][0]["path"].as_str().unwrap());
    let run_id = conversation_run(&mut state, &workspace_id);

    let roots = state.worktree_roots();
    assert_eq!(roots.get(&run_id), Some(&git_dir), "{roots:?}");
}
