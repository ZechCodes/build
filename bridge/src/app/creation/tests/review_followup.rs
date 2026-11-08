use super::settlement::{git, workspace_attempt};
use super::*;
use crate::workspace::{DirectoryStatus, Workspace};
use std::path::PathBuf;

fn git_fixture(mixed: bool) -> (Fixture, PathBuf, String) {
    let f = Fixture::new();
    let repo = f._root.path().join("git-source");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    std::fs::write(repo.join("tracked"), b"source data").unwrap();
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
    let project = {
        let mut app = f.state.lock().unwrap();
        let project = app.add_project(repo.clone(), "main".into());
        app.persist();
        project
    };
    if mixed {
        let plain = f._root.path().join("plain");
        std::fs::create_dir(&plain).unwrap();
        std::fs::write(plain.join("keep"), b"plain directory data").unwrap();
        let config_path = f._root.path().join("config.json");
        let mut config: Value =
            serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
        let env_id = f
            .state
            .lock()
            .unwrap()
            .projects
            .get(&project)
            .unwrap()
            .environment_id
            .clone();
        let environment = config["environments"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|e| e["id"] == env_id)
            .unwrap();
        environment["sources"].as_array_mut().unwrap().push(json!({"id":"source-2","path":plain,"name":"plain","mount":"plain","base_branch":"main"}));
        environment["next_source"] = json!(3);
        std::fs::write(config_path, config.to_string()).unwrap();
    }
    (f.reopen(), repo, project)
}

fn stop_at(f: &Fixture, id: &str, at: Boundary) {
    let observer = observe_boundary(move |boundary| {
        if boundary == at {
            std::panic::panic_any(SimulatedCrash);
        }
    });
    let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f.run(id)));
    assert!(crash.unwrap_err().is::<SimulatedCrash>());
    drop(observer);
}

fn uncheckpointed_git_replay(path_absent: bool, create_project: bool) {
    let (f, repo, project) = git_fixture(false);
    let id = operation(190);
    if create_project {
        let environment = f
            .state
            .lock()
            .unwrap()
            .projects
            .get(&project)
            .unwrap()
            .environment_id
            .clone();
        admit(
            &f.state,
            "environment.create_project",
            json!({"environment_id":environment,"name":"new","workspace":{"name":"git"}}),
            WriteMetadata {
                operation_id: id.clone(),
                after: None,
            },
        )
        .unwrap();
    } else {
        workspace_attempt(&f, &id, &project);
    }
    stop_at(&f, &id, Boundary::BeforeDirectoryCheckpoint);
    let record = f.record(&id);
    assert_eq!(record.execution["effect_state"], "started");
    let workspace: Workspace =
        serde_json::from_value(record.execution["workspace"].clone()).unwrap();
    let directory = &workspace.directories[0];
    assert!(directory.branch.is_none());
    assert_eq!(directory.status, DirectoryStatus::Pending);
    assert!(directory.path.join("tracked").exists());
    let branches = git(&repo, &["branch", "--format=%(refname:short)"]);
    let branch = branches
        .lines()
        .find(|line| *line != "main")
        .unwrap()
        .to_owned();
    if path_absent {
        git(
            &repo,
            &[
                "worktree",
                "remove",
                "--force",
                directory.path.to_str().unwrap(),
            ],
        );
        assert!(!directory.path.exists());
    }
    let f = f.reopen();
    f.store.writer().recover_mutations().unwrap();
    crate::app::workspaces::journal_creation::fail_next_preparation_for_test(
        "replay preparation refused",
    );
    f.run(&id);
    let record = f.record(&id);
    assert!(
        matches!(record.state.as_str(), "unknown" | "reconciling"),
        "{record:?}"
    );
    assert_eq!(record.execution["effect_state"], "started");
    assert!(record.outcome.is_none());
    assert!(f.state.lock().unwrap().workspace_reserved(&workspace.id));
    assert!(!git(&repo, &["branch", "--list", &branch]).trim().is_empty());
}

#[test]
fn replay_uncheckpointed_git_checkout_never_proves_settlement() {
    uncheckpointed_git_replay(false, false);
}

#[test]
fn replay_uncheckpointed_git_ref_without_path_never_proves_settlement() {
    uncheckpointed_git_replay(true, false);
}

#[test]
fn pre_step_failure_terminal_checkpoint_restarts_without_provisioning() {
    let f = Fixture::new();
    let id = operation(191);
    let record = workspace_attempt(&f, &id, "proj-1");
    let workspace: Workspace =
        serde_json::from_value(record.execution["workspace"].clone()).unwrap();
    f.store.writer().claim_mutation(PRINCIPAL, &id).unwrap();
    f.store.fail_next_write();
    stop_at(&f, &id, Boundary::BeforeFailureTerminal);
    assert!(f.record(&id).outcome.is_none());
    assert!(!workspace.root.exists());
    let f = f.reopen();
    f.store.writer().recover_mutations().unwrap();
    f.run(&id);
    let record = f.record(&id);
    assert_eq!(record.state, "failed", "{record:?}");
    assert_eq!(record.outcome.unwrap()["effect"], "unapplied");
    assert!(!workspace.root.exists());
    assert!(f.state.lock().unwrap().workspaces.list(None).is_empty());
}

#[test]
fn failed_worktree_rollback_preserves_later_plain_directory() {
    let (f, _repo, project) = git_fixture(true);
    let id = operation(192);
    workspace_attempt(&f, &id, &project);
    stop_at(&f, &id, Boundary::WorkspaceProvisioned);
    let mut record = f.record(&id);
    let workspace: Workspace =
        serde_json::from_value(record.execution["workspace"].clone()).unwrap();
    assert_eq!(workspace.directories.len(), 2);
    assert!(workspace.directories[0].branch.is_some());
    let plain = workspace.directories[1].path.join("keep");
    assert_eq!(std::fs::read(&plain).unwrap(), b"plain directory data");
    let admin = git2::Repository::open(&workspace.directories[0].path)
        .unwrap()
        .path()
        .to_path_buf();
    std::fs::write(
        admin.join(crate::isolation::BRANCH_TEARDOWN_MARKER),
        "unreadable teardown",
    )
    .unwrap();
    record.execution["residue"] = json!({"paths":[workspace.root],"error":"cleanup pending"});
    f.store
        .writer()
        .park_creation(PRINCIPAL, &id, "unknown", record.execution)
        .unwrap();
    let f = f.reopen();
    f.run(&id);
    assert_eq!(
        std::fs::read(&plain).unwrap(),
        b"plain directory data",
        "a worktree failure must prevent plain-directory deletion"
    );
    assert!(f.record(&id).outcome.is_none());
    assert_eq!(f.record(&id).execution["effect_state"], "started");
    assert!(f.state.lock().unwrap().workspace_reserved(&workspace.id));
}

#[test]
fn discard_guard_keeps_same_home_sibling_session_and_refuses_own_session() {
    for own_session in [false, true] {
        let f = Fixture::new();
        let id = operation(193);
        let record = f.admit(&id);
        f.store.writer().claim_mutation(PRINCIPAL, &id).unwrap();
        project_step(&f.state, &record).unwrap();
        f.store.writer().finish_creation(PRINCIPAL,&id,json!({"state":"failed","effect":"partial","error":{"code":"test","message":"workspace not begun","retryable":false}})).unwrap();
        let session = "retained-session";
        {
            let mut app = f.state.lock().unwrap();
            let owner = if own_session { "proj-2" } else { "proj-1" };
            app.projects.bind_entity(session.into(), owner.into());
            let active = crate::orchestrator::ActiveRun::workspace_conversation(
                crate::run::RunId::new(session),
                "retained".into(),
                f._root.path().join("session"),
                crate::models::ModelChoice::default(),
            );
            app.persist_run_record(session, &active).unwrap();
        }
        let before = f
            .store
            .load_all_runs()
            .unwrap()
            .into_iter()
            .find(|run| run.id == session)
            .unwrap();
        let result = f
            .state
            .lock()
            .unwrap()
            .project_discard_unused(&json!({"project_id":"proj-2","creation_operation_id":id}));
        assert_eq!(result.is_err(), own_session, "{result:?}");
        assert_eq!(
            f.store
                .load_all_runs()
                .unwrap()
                .into_iter()
                .find(|run| run.id == session)
                .unwrap(),
            before
        );
        assert!(f.state.lock().unwrap().projects.get("proj-1").is_some());
    }
}

#[test]
fn project_replay_preserves_started_workspace_evidence() {
    uncheckpointed_git_replay(false, true);
}

#[test]
fn fresh_git_preparation_failure_proves_net_zero_effects() {
    let (f, repo, project) = git_fixture(false);
    let id = operation(194);
    workspace_attempt(&f, &id, &project);
    crate::app::workspaces::journal_creation::fail_next_preparation_for_test(
        "fresh preparation refused",
    );
    f.run(&id);
    let record = f.record(&id);
    assert_eq!(record.state, "failed");
    assert_eq!(record.outcome.unwrap()["effect"], "unapplied");
    assert_eq!(
        git(&repo, &["branch", "--format=%(refname:short)"]).trim(),
        "main"
    );
}
