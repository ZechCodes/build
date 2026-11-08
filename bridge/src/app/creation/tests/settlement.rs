use super::admission_recovery::SimulatedCrash;
use super::*;
use crate::workspace::Workspace;

fn workspace_attempt(f: &Fixture, id: &str, project: &str) -> MutationRecord {
    admit(
        &f.state,
        "workspace.create",
        json!({"project_id":project,"name":"settlement"}),
        WriteMetadata {
            operation_id: id.into(),
            after: None,
        },
    )
    .unwrap()
}

fn interrupt_settlement(f: &Fixture, id: &str) {
    let observer = observe_boundary(|boundary| {
        if boundary == Boundary::SettlementCheckpointed {
            std::panic::panic_any(SimulatedCrash);
        }
    });
    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.run(id)));
    if let Err(crash) = interrupted {
        assert!(crash.is::<SimulatedCrash>());
    }
    drop(observer);
    assert!(f.record(id).outcome.is_none());
    assert_eq!(f.record(id).execution["effect_state"], "settled");
}

fn assert_settled_restart(f: Fixture, id: &str, workspace: &Workspace, error: &str) {
    assert!(!workspace.root.exists());
    let f = f.reopen();
    f.store.writer().recover_mutations().unwrap();
    f.run(id);
    let record = f.record(id);
    assert_eq!(record.state, "failed", "{record:?}");
    assert_eq!(record.outcome.as_ref().unwrap()["error"]["message"], error);
    assert_eq!(record.outcome.unwrap()["effect"], "unapplied");
    assert!(!workspace.root.exists());
    assert!(f.state.lock().unwrap().workspaces.list(None).is_empty());
}

#[test]
fn crash_after_cleanup_settlement_finishes_failure_without_provisioning() {
    let (f, id, workspace) = super::failure_finalization::unknown_with_residue();
    interrupt_settlement(&f, &id);
    assert_settled_restart(f, &id, &workspace, "retained cleanup");
}

#[test]
fn crash_after_rollback_settlement_finishes_failure_without_provisioning() {
    let f = Fixture::new();
    let id = operation(171);
    let record = workspace_attempt(&f, &id, "proj-1");
    let workspace: Workspace =
        serde_json::from_value(record.execution["workspace"].clone()).unwrap();
    crate::app::workspaces::journal_creation::fail_next_preparation_for_test("preparation refused");
    interrupt_settlement(&f, &id);
    assert_settled_restart(f, &id, &workspace, "preparation refused");
}

#[test]
fn root_only_removal_does_not_prove_workspace_settlement() {
    let f = Fixture::new();
    let id = operation(174);
    let record = workspace_attempt(&f, &id, "proj-1");
    f.store.writer().claim_mutation(PRINCIPAL, &id).unwrap();
    let workspace: Workspace =
        serde_json::from_value(record.execution["workspace"].clone()).unwrap();
    f.state
        .lock()
        .unwrap()
        .workspaces
        .claim_creation_workspace(&workspace)
        .unwrap();
    let mut evidence = record.execution;
    evidence["effect_state"] = json!("started");
    f.store
        .writer()
        .checkpoint_creation(PRINCIPAL, &id, evidence)
        .unwrap();
    std::fs::remove_dir_all(&workspace.root).unwrap();
    f.store.fail_next_write();
    f.run(&id);
    let record = f.record(&id);
    assert_eq!(record.state, "unknown");
    assert!(record.outcome.is_none());
    assert!(f.state.lock().unwrap().workspace_reserved(&workspace.id));
}

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn interrupted_materialized_worktree_preparation_failure_rolls_back_every_effect() {
    let f = Fixture::new();
    let repo = f._root.path().join("git-source");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    std::fs::write(repo.join("tracked"), b"git source").unwrap();
    git(&repo, &["add", "tracked"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-m",
            "seed",
        ],
    );
    let env = {
        let mut app = f.state.lock().unwrap();
        let project = app.add_project(repo.clone(), "main".into());
        app.persist();
        app.projects.get(&project).unwrap().environment_id.clone()
    };
    let id = operation(172);
    admit(
        &f.state,
        "environment.create_project",
        json!({"environment_id":env,"name":"git child","workspace":{"name":"cut"}}),
        WriteMetadata {
            operation_id: id.clone(),
            after: None,
        },
    )
    .unwrap();
    let observer = observe_boundary(|boundary| {
        if boundary == Boundary::WorkspaceMaterialized {
            std::panic::panic_any(SimulatedCrash);
        }
    });
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.run(&id)));
    drop(observer);
    let record = f.record(&id);
    assert!(matches!(record.state.as_str(), "unknown" | "executing"));
    let workspace: Workspace =
        serde_json::from_value(record.execution["workspace"].clone()).unwrap();
    assert!(workspace.directories[0].path.join("tracked").exists());
    let branch = workspace.directories[0].branch.clone().unwrap();
    let f = f.reopen();
    f.store.writer().recover_mutations().unwrap();
    crate::app::workspaces::journal_creation::fail_next_preparation_for_test("base sync refused");
    f.run(&id);
    let record = f.record(&id);
    assert_eq!(record.state, "failed", "{record:?}");
    assert_eq!(record.outcome.unwrap()["effect"], "partial");
    assert!(!workspace.root.exists());
    assert!(!git(&repo, &["worktree", "list", "--porcelain"])
        .contains(workspace.directories[0].path.to_str().unwrap()));
    assert!(
        git(&repo, &["branch", "--list", &branch]).trim().is_empty(),
        "full rollback must remove the worktree branch"
    );
}

#[cfg(unix)]
#[test]
fn rollback_and_residue_journal_failures_keep_landed_project_workspace_fenced() {
    let f = Fixture::new();
    let second = f._root.path().join("second");
    std::fs::create_dir(&second).unwrap();
    let config_path = f._root.path().join("config.json");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
    config["environments"][0]["sources"].as_array_mut().unwrap().push(json!({"id":"source-2","path":second,"name":"second","mount":"second","base_branch":"main"}));
    config["environments"][0]["next_source"] = json!(3);
    std::fs::write(config_path, json!(config).to_string()).unwrap();
    let f = f.reopen();
    let id = operation(173);
    f.admit(&id);
    let state = f.state.clone();
    let store = f.store.clone();
    let foreign = f._root.path().join("foreign");
    std::fs::create_dir(&foreign).unwrap();
    std::fs::write(foreign.join("keep"), b"foreign owned").unwrap();
    let foreign_for_hook = foreign.clone();
    let mut materialized = false;
    let residue_attempted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let residue_for_hook = residue_attempted.clone();
    let hook_id = id.clone();
    let observer = observe_boundary(move |boundary| match boundary {
        Boundary::WorkspaceMaterialized if !materialized => {
            materialized = true;
            let execution = store
                .writer()
                .creation_record(PRINCIPAL, &hook_id)
                .unwrap()
                .unwrap()
                .execution;
            let workspace: Workspace =
                serde_json::from_value(execution["workspace"].clone()).unwrap();
            assert_eq!(workspace.directories.len(), 2);
            std::fs::remove_dir_all(&workspace.directories[0].path).unwrap();
            // A file where rollback expects a directory makes removal fail
            // independently of permissions or symlink-removal behavior.
            std::fs::write(&workspace.directories[0].path, b"retained residue").unwrap();
            assert!(foreign_for_hook.join("keep").exists());
            std::fs::remove_dir_all(&second).unwrap();
        }
        Boundary::BeforeResidueCheckpoint => {
            residue_for_hook.store(true, std::sync::atomic::Ordering::SeqCst);
            store.fail_next_write();
        }
        _ => {}
    });
    f.run(&id);
    drop(observer);
    assert!(
        residue_attempted.load(std::sync::atomic::Ordering::SeqCst),
        "the failed rollback must attempt its residue checkpoint"
    );
    let record = f.record(&id);
    assert!(
        matches!(record.state.as_str(), "unknown" | "reconciling"),
        "{record:?}"
    );
    assert!(record.outcome.is_none());
    assert_eq!(record.execution["effect_state"], "started");
    let workspace: Workspace =
        serde_json::from_value(record.execution["workspace"].clone()).unwrap();
    assert!(state.lock().unwrap().workspace_reserved(&workspace.id));
    assert!(workspace.root.exists());
    assert_eq!(
        std::fs::read(foreign.join("keep")).unwrap(),
        b"foreign owned"
    );
}
