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
    let manifest = PathBuf::from(created["result"]["root"].as_str().unwrap())
        .join(crate::workspace::MANIFEST_FILE);
    let backup = directory.path().join("saved-workspace-manifest");
    std::fs::rename(&manifest, &backup).unwrap();
    std::fs::create_dir(&manifest).unwrap();
    let failed = call(
        &handler,
        "workspace.add_directory",
        json!({
            "workspace_id":workspace,"source_id":source
        }),
    );
    assert_eq!(failed["ok"], false, "{failed}");
    std::fs::remove_dir(&manifest).unwrap();
    std::fs::rename(&backup, &manifest).unwrap();
    let unrelated = call(
        &handler,
        "settings.set",
        json!({"watch_agent_filed_tasks":true}),
    );
    assert_eq!(unrelated["ok"], true, "{unrelated}");
    let missing = service.prepare(
        req(
            "fs.read",
            json!({
                "workspace_id":workspace,"source_id":source,"path":"note.txt"
            }),
        ),
        None,
    )();
    assert_eq!(missing["error_code"], "not_found", "{missing}");
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
        json!({"watch_agent_filed_tasks":false}),
    );
    assert_eq!(unrelated["ok"], true, "{unrelated}");
    let stale = stale();
    assert_eq!(stale["ok"], false, "{stale}");
    assert_eq!(stale["error_code"], "not_found", "{stale}");
    let fresh = prepare()();
    assert_eq!(fresh["error_code"], "not_found", "{fresh}");
}

#[test]
fn observation_project_source_failed_saves_never_change_retained_service_inputs() {
    for failure in [ConfigPersistStep::Write, ConfigPersistStep::Rename] {
        for remove in [false, true] {
            assert_project_source_failure_keeps_committed_inputs(failure, remove);
        }
    }
}

fn assert_project_source_failure_keeps_committed_inputs(failure: ConfigPersistStep, remove: bool) {
    let (directory, repository) = init_repo();
    let original = directory.path().join("committed-source");
    let candidate = directory.path().join("candidate-source");
    std::fs::create_dir(&original).unwrap();
    std::fs::create_dir(&candidate).unwrap();
    std::fs::write(original.join("note.txt"), b"committed\n").unwrap();
    std::fs::write(candidate.join("note.txt"), b"candidate\n").unwrap();
    let mut app = qa_state(&repository, directory.path())
        .with_config(directory.path().join("config.json"))
        .unwrap();
    let project = app.project_at(0).id.clone();
    let added = app.handle(req(
        "project.add_source",
        json!({
            "project_id":project,"path":original,"name":"Committed source"
        }),
    ));
    assert_eq!(added["ok"], true, "{added}");
    let source = added["result"]["sources"][1]["id"].clone();
    let state = app.shared();
    let handler = AppState::handler(state.clone());
    let service = state.lock().unwrap().observation_service();
    let params =
        json!({"project_id":project,"source_id":source,"source_path":original,"path":"note.txt"});
    let prepare = || service.prepare(req("fs.read", params.clone()), None);
    let before = prepare()();
    assert_eq!(before["ok"], true, "{before}");
    assert_eq!(before["result"]["content_b64"], b64encode(b"committed\n"));
    let pending = prepare();
    state.lock().unwrap().config_persist_failure = Some(failure);
    let (method, change) = if remove {
        (
            "project.remove_source",
            json!({"project_id":project,"source_id":source}),
        )
    } else {
        (
            "project.update_source",
            json!({"project_id":project,"source_id":source,"path":candidate}),
        )
    };
    let failed = call(&handler, method, change.clone());
    assert_eq!(failed["ok"], false, "{failed}");
    state.lock().unwrap().config_persist_failure = None;
    let unrelated = call(
        &handler,
        "settings.set",
        json!({"watch_agent_filed_tasks":true}),
    );
    assert_eq!(unrelated["ok"], true, "{unrelated}");
    assert_eq!(
        pending(),
        before,
        "failed candidate changed a prepared source fence"
    );
    assert_eq!(
        prepare()(),
        before,
        "failed candidate leaked through unrelated publication"
    );

    let pending = prepare();
    let saved = call(&handler, method, change);
    assert_eq!(saved["ok"], true, "{saved}");
    let expected = if remove { "not_found" } else { "conflict" };
    let old = pending();
    assert_eq!(old["error_code"], expected, "{old}");
    let old = prepare()();
    assert_eq!(old["error_code"], expected, "{old}");
    if !remove {
        let current = service.prepare(req("fs.read", json!({
            "project_id":project,"source_id":source,"source_path":candidate,"path":"note.txt"
        })), None)();
        assert_eq!(current["ok"], true, "{current}");
        assert_eq!(current["result"]["content_b64"], b64encode(b"candidate\n"));
    }
}
