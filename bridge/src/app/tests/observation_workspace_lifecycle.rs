//! A retained observation service follows committed workspace directory changes.
use super::*;

#[test]
fn observation_workspace_directories_follow_acknowledged_changes_and_keep_saved_sources() {
    let (directory, repository) = init_repo();
    let extra = init_repo_named(directory.path(), "later-source");
    std::fs::write(extra.join("note.txt"), b"saved workspace source\n").unwrap();
    git_in(&extra, &["add", "note.txt"]);
    git_in(&extra, &["commit", "-q", "-m", "source contents"]);
    let app = qa_state(&repository, directory.path());
    let project = app.project_at(0).id.clone();
    let state = app.shared();
    let handler = AppState::handler(state.clone());
    let service = state.lock().unwrap().observation_service();
    let created = call(
        &handler,
        "workspace.create",
        json!({
            "project_id":project,"name":"incremental observation","isolation":"worktree"
        }),
    );
    assert_eq!(created["ok"], true, "{created}");
    let workspace = created["result"]["workspace_id"].clone();
    let appended = call(
        &handler,
        "project.add_source",
        json!({
            "project_id":project,"path":extra,"name":"Later source"
        }),
    );
    assert_eq!(appended["ok"], true, "{appended}");
    let source = appended["result"]["sources"][1]["id"].clone();
    let added = call(
        &handler,
        "workspace.add_directory",
        json!({
            "workspace_id":workspace,"source_id":source
        }),
    );
    assert_eq!(added["ok"], true, "{added}");
    let row = &added["result"]["directories"][1];
    let id = row["id"].clone();
    let saved_path = PathBuf::from(row["path"].as_str().unwrap());
    let params = json!({"workspace_id":workspace,"source_id":source,"path":"note.txt"});
    let prepare = || service.prepare(req("fs.read", params.clone()), None);
    let initial = prepare()();
    assert_eq!(initial["ok"], true, "{initial}");
    assert_eq!(
        initial["result"]["content_b64"],
        b64encode(b"saved workspace source\n")
    );
    let saved = prepare();
    let removed_source = call(
        &handler,
        "project.remove_source",
        json!({
            "project_id":project,"source_id":source
        }),
    );
    assert_eq!(removed_source["ok"], true, "{removed_source}");
    assert_eq!(
        saved(),
        initial,
        "a saved workspace outlives project source removal"
    );
    assert_eq!(prepare()(), initial);

    let stale = prepare();
    let removed_directory = call(
        &handler,
        "workspace.remove_directory",
        json!({
            "workspace_id":workspace,"directory_id":id
        }),
    );
    assert_eq!(removed_directory["ok"], true, "{removed_directory}");
    // Recreate the retired bytes so a stale immutable identity cannot be hidden
    // by a missing physical file. Only the committed directory fence may refuse.
    std::fs::create_dir_all(&saved_path).unwrap();
    std::fs::write(saved_path.join("note.txt"), b"saved workspace source\n").unwrap();
    let unrelated = call(
        &handler,
        "settings.set",
        json!({"watch_agent_filed_tasks":true}),
    );
    assert_eq!(unrelated["ok"], true, "{unrelated}");
    let stale = stale();
    assert_eq!(stale["ok"], false, "{stale}");
    assert_eq!(stale["error_code"], "not_found", "{stale}");
    let fresh = prepare()();
    assert_eq!(fresh["error_code"], "not_found", "{fresh}");
}
