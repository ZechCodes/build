//! A retained observation service follows committed workspace directory changes.
use super::*;

fn assert_observation_bytes(reply: &Value, expected: &[u8]) {
    assert_eq!(reply["ok"], true, "{reply}");
    assert_eq!(reply["result"]["content_b64"], b64encode(expected));
}

fn publish_unrelated_watch_setting(handler: &crate::carrier::FrameHandler, watched: bool) {
    let unrelated = call(
        handler,
        "settings.set",
        json!({"watch_agent_filed_tasks":watched}),
    );
    assert_eq!(unrelated["ok"], true, "{unrelated}");
}

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
    publish_unrelated_watch_setting(&handler, true);
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
    assert_observation_bytes(&initial, b"saved workspace source\n");
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
    publish_unrelated_watch_setting(&handler, false);
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
    assert_observation_bytes(&before, b"committed\n");
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
    publish_unrelated_watch_setting(&handler, true);
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
        assert_observation_bytes(&current, b"candidate\n");
    }
}

struct WorkspaceReassociationFixture {
    _directory: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    service: crate::app::observation_reads::ObservationService,
    workspace: String,
    project: String,
    root: PathBuf,
    git: PathBuf,
    owners: Vec<String>,
}

impl WorkspaceReassociationFixture {
    fn new(owners: usize) -> Self {
        let (directory, repository) = init_repo();
        let mut app = qa_state(&repository, directory.path());
        let project = app.project_at(0).id.clone();
        let created = app.handle(req(
            "workspace.create",
            json!({
                "project_id":project,"name":"ownership fallback","isolation":"worktree"
            }),
        ));
        assert_eq!(created["ok"], true, "{created}");
        let workspace = created["result"]["workspace_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let root = PathBuf::from(created["result"]["root"].as_str().unwrap());
        let git = PathBuf::from(
            created["result"]["directories"][0]["path"]
                .as_str()
                .unwrap(),
        );
        assert!(!root.join(".git").exists());
        std::fs::write(git.join("ownership-marker.txt"), b"selected source\n").unwrap();
        let owners = (0..owners)
            .map(|index| format!("fallback-{index}"))
            .collect::<Vec<_>>();
        for owner in &owners {
            let active = crate::orchestrator::ActiveRun::workspace_conversation(
                crate::run::RunId::new(owner),
                "fallback".into(),
                root.clone(),
                crate::models::ModelChoice::default(),
            );
            app.projects.bind_entity(owner.clone(), project.clone());
            app.finish_run_mutation(owner.clone(), active).unwrap();
        }
        app.delivery_queue.clear_queued();
        let state = app.shared();
        let _handler = AppState::handler(state.clone());
        let service = state.lock().unwrap().observation_service();
        Self {
            _directory: directory,
            state,
            service,
            workspace,
            project,
            root,
            git,
            owners,
        }
    }

    fn assert_git_parity(&self, owner: &str) -> Value {
        let params = json!({"run_id":owner});
        let expected = {
            let mut app = self.state.lock().unwrap();
            app.resolve_git_scope(&params)
                .and_then(|scope| scope.read("git.status", &params))
        };
        let frame = req("git.status", params);
        let expected = crate::api::reply(
            frame.payload["id"].clone(),
            expected.map_err(crate::api::ApiError::classify),
        );
        let actual = self.service.prepare(frame, None)();
        assert_eq!(
            actual, expected,
            "retained observation disagrees with legacy root for {owner}"
        );
        actual
    }

    fn assert_no_workspace_change(&self) {
        assert!(
            self.state
                .lock()
                .unwrap()
                .workspaces
                .observation_changes()
                .snapshot()
                .is_empty(),
            "this transition must be driven only by owner inputs"
        );
    }
}

#[test]
fn observation_terminal_owner_reassigns_the_shared_workspace_git_root() {
    let fixture = WorkspaceReassociationFixture::new(2);
    let first = {
        let app = fixture.state.lock().unwrap();
        app.workspace_snapshot_conversation_owner(app.workspaces.get(&fixture.workspace).unwrap())
            .unwrap()
    };
    let next = fixture
        .owners
        .iter()
        .find(|owner| **owner != first)
        .unwrap();
    assert_eq!(fixture.assert_git_parity(&first)["ok"], true);
    assert_eq!(fixture.assert_git_parity(next)["ok"], false);
    fixture.assert_no_workspace_change();
    {
        let mut app = fixture.state.lock().unwrap();
        let mut candidate = app.take_run(&first).unwrap();
        candidate.run.state = RunState::Archived;
        app.finish_run_mutation(first, candidate).unwrap();
        let root = app
            .resolve_git_scope(&json!({"run_id":next}))
            .unwrap()
            .repo_path;
        assert_eq!(
            root, fixture.git,
            "legacy resolver must promote the remaining owner"
        );
    }
    fixture.assert_no_workspace_change();
    let promoted = fixture.assert_git_parity(next);
    assert_eq!(promoted["ok"], true);
    assert!(promoted["result"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file["path"] == "ownership-marker.txt"));
}

#[test]
fn observation_direct_workspace_owner_retires_the_previous_fallback_git_root() {
    let fixture = WorkspaceReassociationFixture::new(1);
    let fallback = &fixture.owners[0];
    assert_eq!(fixture.assert_git_parity(fallback)["ok"], true);
    let pending = fixture
        .service
        .prepare(req("git.status", json!({"run_id":fallback})), None);
    fixture.assert_no_workspace_change();
    {
        let mut app = fixture.state.lock().unwrap();
        let direct = crate::orchestrator::ActiveRun::workspace_conversation(
            crate::run::RunId::new(&fixture.workspace),
            "direct owner".into(),
            fixture.root.clone(),
            crate::models::ModelChoice::default(),
        );
        app.projects
            .bind_entity(fixture.workspace.clone(), fixture.project.clone());
        app.finish_run_mutation(fixture.workspace.clone(), direct)
            .unwrap();
        let root = app
            .resolve_git_scope(&json!({"run_id":fallback}))
            .unwrap()
            .repo_path;
        assert_eq!(
            root, fixture.root,
            "direct ownership must displace the old fallback"
        );
    }
    fixture.assert_no_workspace_change();
    let stale = pending();
    assert_eq!(stale["error_code"], "conflict", "{stale}");
    assert_eq!(fixture.assert_git_parity(fallback)["ok"], false);
    assert_eq!(fixture.assert_git_parity(&fixture.workspace)["ok"], true);
}
