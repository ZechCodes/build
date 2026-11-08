//! Failed creation owns only checkouts that nobody else has referenced.
use super::*;

struct PreparedClone {
    _root: tempfile::TempDir,
    app: AppState,
    other: String,
    cloned: PathBuf,
    config: PathBuf,
    params: Value,
    done: crate::app::runtime::deferred::DeferredDone,
}

fn prepared_clone() -> PreparedClone {
    let root = tempfile::tempdir().unwrap();
    let upstream = init_repo_named(root.path(), "upstream");
    let home_a = root.path().join("home-a");
    let home_b = root.path().join("home-b");
    std::fs::create_dir(&home_a).unwrap();
    std::fs::create_dir(&home_b).unwrap();
    let projects = root.path().join("projects");
    let config = root.path().join("config.json");
    std::fs::write(&config, json!({"projects_dir":projects}).to_string()).unwrap();
    let context = crate::harness::HarnessContext::resolved(
        root.path().join("mcp.sock"),
        root.path().join("state"),
    )
    .unwrap();
    let mut app =
        AppState::new_unrooted_configured(root.path().join("workspaces"), "main", true, context)
            .with_config(&config)
            .unwrap();
    let other = app.add_project(home_b, "main".into());

    let params = json!({
        "name":"attempt",
        "sources":[{"path":home_a},{"remote":upstream,"name":"fresh"}]
    });
    let (admitted, work) = app.dispatch_deferring("project.create", &params);
    assert!(admitted.is_ok(), "{admitted:?}");
    let done = work
        .expect("opening and cloning run off the app lock")
        .run();
    let cloned = projects.join("attempt-sources/fresh");
    assert!(cloned.join(".git").is_dir());
    PreparedClone {
        _root: root,
        app,
        other,
        cloned,
        config,
        params,
        done,
    }
}

#[test]
fn failed_clone_settlement_keeps_directory_referenced_by_another_project() {
    let PreparedClone {
        _root,
        mut app,
        other,
        cloned,
        config,
        params,
        done,
    } = prepared_clone();
    let original = std::fs::read(cloned.join("README.md")).unwrap();

    // The clone exists, but the attempt has not published a project yet.
    // A different project can acquire an independent directory reference.
    let linked = app.handle(req(
        "project.add_source",
        json!({"project_id":other,"path":cloned,"name":"borrowed"}),
    ));
    assert_eq!(linked["ok"], true, "{linked}");
    let source_id = linked["result"]["sources"][1]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let committed = std::fs::read(&config).unwrap();

    app.config_persist_failure = Some(ConfigPersistStep::Write);
    let failed = app.apply_deferred("project.create", &params, done);
    assert!(
        failed.is_err(),
        "the original attempt must fail its durable write"
    );

    assert!(
        cloned.join(".git").is_dir(),
        "failed creation deleted a now-shared checkout"
    );
    assert_eq!(std::fs::read(cloned.join("README.md")).unwrap(), original);
    assert!(app
        .sources_for(&other)
        .unwrap()
        .iter()
        .any(|source| { source.id == source_id && source.path == cloned }));
    assert_eq!(std::fs::read(&config).unwrap(), committed);
    app.config_persist_failure = None;
    let listed = app.project_list();
    let other = listed["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["project_id"] == other)
        .unwrap();
    assert!(other["sources"].as_array().unwrap().iter().any(|source| {
        source["id"] == source_id && source["path"] == cloned.to_string_lossy().as_ref()
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clone_cleanup_claim_blocks_admission_and_prepared_settlement_during_delete() {
    let PreparedClone {
        _root,
        mut app,
        other,
        cloned,
        params,
        done,
        ..
    } = prepared_clone();
    let child = cloned.join("docs");
    std::fs::create_dir(&child).unwrap();
    let workspace = app.handle(req(
        "workspace.create",
        json!({"project_id":other,"name":"cleanup-fence","isolation":"worktree"}),
    ));
    assert_eq!(workspace["ok"], true, "{workspace}");
    let workspace_id = workspace["result"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    let local = json!({"project_id":other,"path":cloned,"name":"prepared"});
    let (admitted, pending) = app.dispatch_deferring("project.add_source", &local);
    assert!(admitted.is_ok(), "{admitted:?}");
    let pending = pending.expect("the local folder is opened off-lock").run();

    let state = app.shared();
    let (gate, gate_handle) = OffLockGate::new();
    {
        let mut app = state.lock().unwrap();
        // Only the failed attempt's cleanup gets the gate: both preparations
        // have already completed, and later admission must not inherit it.
        app.off_lock_gate = Some(gate);
        app.config_persist_failure = Some(ConfigPersistStep::Write);
        let failed = app.apply_deferred("project.create", &params, done);
        app.off_lock_gate = None;
        app.config_persist_failure = None;
        assert!(failed.is_err(), "the original project write must fail");
    }
    gate_handle.wait_for_arrival();

    {
        let mut app = state.lock().unwrap();
        // A child path avoids the unrelated exact-path pending reservation.
        let admission = app.project_add_source(&json!({"project_id":other,"path":child}));
        assert!(
            admission.is_err(),
            "admitted a source inside an active deletion"
        );
        let directory = app.handle(req(
            "workspace.add_directory",
            json!({"workspace_id":workspace_id,"path":child}),
        ));
        assert_eq!(
            directory["ok"], false,
            "admitted a workspace directory inside an active deletion: {directory}"
        );
        assert_eq!(
            app.workspaces.get(&workspace_id).unwrap().directories.len(),
            1
        );
        let settlement = app.apply_deferred("project.add_source", &local, pending);
        assert!(
            settlement.is_err(),
            "settled a source inside an active deletion"
        );
        assert_eq!(app.sources_for(&other).unwrap().len(), 1);
    }
    gate_handle.release();

    tokio::time::timeout(Duration::from_secs(10), async {
        while cloned.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Once cleanup applies, the same location may be registered afresh.
        std::fs::create_dir_all(&child).unwrap();
        loop {
            let accepted = state.lock().unwrap().handle(req(
                "project.add_source",
                json!({"project_id":other,"path":child}),
            ));
            if accepted["ok"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cleanup must delete its unreferenced clone and release the directory claim");
}

#[test]
fn failed_clone_settlement_keeps_directory_held_by_pending_workspace_add() {
    let PreparedClone {
        _root,
        mut app,
        other,
        cloned,
        params,
        done,
        ..
    } = prepared_clone();
    let workspace = app.handle(req(
        "workspace.create",
        json!({"project_id":other,"name":"pending-holder","isolation":"worktree"}),
    ));
    assert_eq!(workspace["ok"], true, "{workspace}");
    let workspace_id = workspace["result"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    // RPC admission waits behind the outstanding clone's filesystem job.
    // Seed real pending registry metadata to cover a previously recorded or
    // restored workspace holder independently of that admission guard.
    let pending = app
        .workspaces
        .begin_directory(
            &workspace_id,
            &crate::workspace::WorkspaceSource {
                id: "dir-borrowed".into(),
                name: "borrowed".into(),
                mount: "borrowed".into(),
                path: cloned.clone(),
                is_git: true,
                base_branch: "main".into(),
            },
        )
        .unwrap();
    assert!(app
        .workspaces
        .get(&workspace_id)
        .unwrap()
        .directories
        .iter()
        .any(|directory| directory.source_path == cloned));

    app.config_persist_failure = Some(ConfigPersistStep::Write);
    let failed = app.apply_deferred("project.create", &params, done);
    assert!(failed.is_err(), "the original project write must fail");
    assert!(
        cloned.join(".git").is_dir(),
        "cleanup deleted the source named by a pending workspace directory"
    );
    assert!(app
        .workspaces
        .get(&workspace_id)
        .unwrap()
        .directories
        .iter()
        .any(|directory| directory.id == pending.id && directory.source_path == cloned));
    let repository = git2::Repository::open(&cloned).unwrap();
    assert!(repository.head().unwrap().peel_to_commit().is_ok());
}
