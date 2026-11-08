use super::*;
use crate::app::projects::test_support::register_sibling;

fn rooted(root: &Path) -> AppState {
    let context = HarnessContext::resolved(root.join("mcp.sock"), root.to_path_buf()).unwrap();
    AppState::new_unrooted_configured(root.join("worktrees"), "main", true, context)
}

fn siblings(state: &mut AppState, home: &Path) -> (String, String) {
    let migrated = state.add_project(home.to_path_buf(), "main".into());
    let sibling = register_sibling(state, &migrated);
    (migrated, sibling)
}

fn run_record(id: &str, key: &str, root: &Path) -> PersistedRun {
    let mut record = fake_run_record(id);
    record.project_path = key.to_string();
    record.worktree_path = root.display().to_string();
    record.state = RunState::Interrupted;
    record.adopted = true;
    record
}

fn plan_record(id: &str, key: &str) -> PersistedPlan {
    PersistedPlan {
        id: id.into(),
        goal: "recover by project key".into(),
        project_path: key.into(),
        base_branch: "main".into(),
        state: PlanState::Interrupted,
        archived_at: None,
        implementation_intent: crate::plan::ImplementationIntent::None,
        implementation_activity: crate::plan::ImplementationActivity::Idle,
        plan_path: ".build/plan.md".into(),
        stages: Vec::new(),
        provider: AgentProvider::Claude,
        model: None,
        effort: None,
        agents: crate::agent::stored_agents(id),
        legacy_thread: crate::thread::Thread::default(),
        last_summary: None,
        last_error: None,
        created_at: "2026-07-01T10:00:00Z".into(),
        updated_at: "2026-07-01T10:00:00Z".into(),
        state_changed_at: None,
    }
}

#[test]
fn recovery_binds_runs_by_key_not_path() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("shared");
    std::fs::create_dir(&home).unwrap();
    let mut state = rooted(root.path());
    let (migrated, sibling) = siblings(&mut state, &home);
    let key = state.projects.get(&sibling).unwrap().project_key.clone();

    state
        .recover_run(run_record("run-sibling", &key, &home))
        .unwrap();

    assert_eq!(
        state.projects.project_id_of("run-sibling"),
        Some(sibling.as_str())
    );
    assert_ne!(
        state.projects.project_id_of("run-sibling"),
        Some(migrated.as_str())
    );
    assert_eq!(state.runs["run-sibling"].last_error, None);
    assert_eq!(state.projects.len(), 2);
}

#[test]
fn recovery_never_adds_project_from_record() {
    let root = tempfile::tempdir().unwrap();
    let orphan_home = root.path().join("unregistered");
    std::fs::create_dir(&orphan_home).unwrap();
    let unknown_key = orphan_home.display().to_string();
    let mut state = rooted(root.path());

    state
        .recover_plan(plan_record("plan-orphan", &unknown_key))
        .unwrap();
    state
        .recover_run(run_record("run-orphan", &unknown_key, &orphan_home))
        .unwrap();

    assert!(
        state.projects.is_empty(),
        "records must never register an existing folder"
    );
    assert_eq!(state.projects.project_id_of("plan-orphan"), None);
    assert_eq!(state.projects.project_id_of("run-orphan"), None);
    let expected = format!("project repo missing at {unknown_key}");
    assert_eq!(
        state.plans["plan-orphan"].last_error.as_deref(),
        Some(expected.as_str())
    );
    assert_eq!(
        state.runs["run-orphan"].last_error.as_deref(),
        Some(expected.as_str())
    );
    assert_eq!(state.runs["run-orphan"].run.state, RunState::Interrupted);

    let mut native = run_record(
        "run-native-orphan",
        &unknown_key,
        &root.path().join("gone-worktree"),
    );
    native.adopted = false;
    state.recover_run(native).unwrap();
    assert_eq!(
        state.runs["run-native-orphan"].run.state,
        RunState::Abandoned
    );
    assert_eq!(
        state.runs["run-native-orphan"].last_error.as_deref(),
        Some(expected.as_str())
    );
    assert!(state.projects.is_empty());
}

#[test]
fn legacy_record_binds_migrated_project_only() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("shared");
    std::fs::create_dir(&home).unwrap();
    let mut state = rooted(root.path());
    let (migrated, sibling) = siblings(&mut state, &home);
    let legacy_key = state.projects.get(&migrated).unwrap().project_key.clone();
    assert_ne!(
        legacy_key,
        state.projects.get(&sibling).unwrap().project_key
    );

    state
        .recover_plan(plan_record("plan-legacy", &legacy_key))
        .unwrap();
    state
        .recover_run(run_record("run-legacy", &legacy_key, &home))
        .unwrap();

    assert_eq!(
        state.projects.project_id_of("plan-legacy"),
        Some(migrated.as_str())
    );
    assert_eq!(
        state.projects.project_id_of("run-legacy"),
        Some(migrated.as_str())
    );
    assert_eq!(state.projects.len(), 2);

    state.projects.remove(&migrated);
    state
        .recover_plan(plan_record("plan-deleted-legacy", &legacy_key))
        .unwrap();
    state
        .recover_run(run_record("run-deleted-legacy", &legacy_key, &home))
        .unwrap();
    assert_eq!(state.projects.project_id_of("plan-deleted-legacy"), None);
    assert_eq!(state.projects.project_id_of("run-deleted-legacy"), None);
    assert_eq!(
        state.projects.len(),
        1,
        "a sibling does not revive the deleted identity"
    );
}

#[test]
fn restart_recovery_preserves_sibling_run_plan_and_scratch_ownership() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("shared");
    std::fs::create_dir(&home).unwrap();
    let config = root.path().join("config.json");
    let store_path = root.path().join("store");
    let (migrated, sibling, keys, scratches) = {
        let mut state = rooted(root.path()).with_config(&config).unwrap();
        let (migrated, sibling) = siblings(&mut state, &home);
        let keys =
            [&migrated, &sibling].map(|id| state.projects.get(id).unwrap().project_key.clone());
        let scratches = keys
            .each_ref()
            .map(|key| crate::app::projects::scratch_dir(root.path(), key));
        state
            .persist_config(&state.config_value(
                &state.projects_dir,
                state.default_harness,
                state.isolation,
            ))
            .unwrap();
        let store = crate::store::Store::new(&store_path).unwrap();
        for index in 0..2 {
            let plan_id = format!("plan-{index}");
            store
                .save_task_plan(&plan_record(&plan_id, &keys[index]))
                .unwrap();
            let mut record = run_record(&format!("run-{index}"), &keys[index], &scratches[index]);
            record.plan_id = Some(plan_id);
            store.save_run(&record).unwrap();
        }
        (migrated, sibling, keys, scratches)
    };

    let mut restarted = rooted(root.path())
        .with_config(&config)
        .unwrap()
        .with_task_store(&store_path)
        .unwrap();

    assert_ne!(scratches[0], scratches[1]);
    for (index, project_id) in [&migrated, &sibling].into_iter().enumerate() {
        let run_id = format!("run-{index}");
        let plan_id = format!("plan-{index}");
        assert_eq!(
            restarted.projects.project_id_of(&plan_id),
            Some(project_id.as_str())
        );
        assert_eq!(
            restarted.projects.project_id_of(&run_id),
            Some(project_id.as_str())
        );
        assert_eq!(restarted.runs[&run_id].worktree.path, scratches[index]);
        assert!(
            scratches[index].is_dir(),
            "each missing scratch is recut on restart"
        );
        assert_eq!(restarted.runs[&run_id].last_error, None);
        let mut active = restarted.runs.remove(&run_id).unwrap();
        active.last_summary = Some("persist after recovery".into());
        restarted
            .finish_run_mutation(run_id.clone(), active)
            .unwrap();
        let saved = restarted
            .store
            .as_ref()
            .unwrap()
            .load_all_runs()
            .unwrap()
            .into_iter()
            .find(|run| run.id == run_id)
            .unwrap();
        assert_eq!(
            saved.project_path, keys[index],
            "write-back preserves identity"
        );
        let mut plan = restarted.plans.remove(&plan_id).unwrap();
        plan.last_summary = Some("persist plan after recovery".into());
        restarted
            .finish_plan_mutation(plan_id.clone(), plan)
            .unwrap();
        let saved_plan = restarted
            .store
            .as_ref()
            .unwrap()
            .load_all_plans()
            .unwrap()
            .into_iter()
            .find(|plan| plan.id == plan_id)
            .unwrap();
        assert_eq!(
            saved_plan.project_path, keys[index],
            "plan write-back preserves identity"
        );
    }
    assert_eq!(restarted.projects.len(), 2);
}

#[test]
fn recovery_uses_home_path_for_a_resolved_project_with_a_missing_repo() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("missing-home");
    std::fs::create_dir(&home).unwrap();
    let mut state = rooted(root.path());
    let (_, sibling) = siblings(&mut state, &home);
    let key = state.projects.get(&sibling).unwrap().project_key.clone();
    let checkout = root.path().join("surviving-checkout");
    std::fs::create_dir(&checkout).unwrap();
    std::fs::remove_dir(&home).unwrap();

    state
        .recover_plan(plan_record("plan-missing", &key))
        .unwrap();
    state
        .recover_run(run_record("run-missing", &key, &checkout))
        .unwrap();

    let expected = format!("project repo missing at {}", home.display());
    assert_eq!(
        state.plans["plan-missing"].last_error.as_deref(),
        Some(expected.as_str())
    );
    assert_eq!(
        state.runs["run-missing"].last_error.as_deref(),
        Some(expected.as_str())
    );
    assert_eq!(state.projects.project_id_of("plan-missing"), None);
    assert_eq!(state.projects.project_id_of("run-missing"), None);
}

#[test]
fn archived_boot_recovery_keeps_plan_and_run_records_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let mut state = rooted(root.path())
        .with_task_store(root.path().join("store"))
        .unwrap();
    let project = state.add_project(home.clone(), "main".into());
    let key = state.project(&project).unwrap().project_key.clone();
    let store = state.store.clone().unwrap();
    let plan = plan_record("plan-archived", &key);
    let run = run_record("run-archived", &key, &root.path().join("missing-checkout"));
    store.save_task_plan(&plan).unwrap();
    store.save_run(&run).unwrap();
    assert_eq!(
        state.handle(req("project.archive", json!({"project_id":project})))["ok"],
        true
    );
    std::fs::remove_dir(&home).unwrap();
    state.recover_plan(plan.clone()).unwrap();
    state.recover_run(run.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(store.load_all_plans().unwrap()[0].clone()).unwrap(),
        serde_json::to_value(&plan).unwrap()
    );
    assert_eq!(
        serde_json::to_value(store.load_all_runs().unwrap()[0].clone()).unwrap(),
        serde_json::to_value(&run).unwrap()
    );
    assert_eq!(state.plans[&plan.id].last_error, None);
    assert_eq!(state.runs[&run.id].last_error, None);
    assert!(state.projects.project_id_of(&plan.id).is_some());
    assert!(state.projects.project_id_of(&run.id).is_some());
}

#[test]
fn archived_idle_monitor_waits_for_restore() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let checkout = root.path().join("checkout");
    std::fs::create_dir(&checkout).unwrap();
    let mut state = rooted(root.path())
        .with_task_store(root.path().join("store"))
        .unwrap();
    let project = state.add_project(home, "main".into());
    let key = state.project(&project).unwrap().project_key.clone();
    state
        .recover_run(run_record("run-idle-archived", &key, &checkout))
        .unwrap();
    let mut active = state.take_run("run-idle-archived").unwrap();
    active.run.state = RunState::Building;
    state
        .finish_run_mutation("run-idle-archived".into(), active)
        .unwrap();
    assert_eq!(
        state.handle(req("project.archive", json!({"project_id":project})))["ok"],
        true
    );
    assert!(state.mark_idle_tasks(Duration::ZERO).is_empty());
    assert_eq!(
        state.runs["run-idle-archived"].run.state,
        RunState::Building
    );
    assert_eq!(
        state.handle(req("project.restore", json!({"project_id":project})))["ok"],
        true
    );
    assert_eq!(state.mark_idle_tasks(Duration::ZERO), ["run-idle-archived"]);
}

#[test]
fn archived_vanished_run_settlement_waits_for_restore() {
    use crate::app::runtime::recovery::DecidedVanishedRun;
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let checkout = root.path().join("checkout");
    std::fs::create_dir(&checkout).unwrap();
    let mut state = rooted(root.path())
        .with_task_store(root.path().join("store"))
        .unwrap();
    let project = state.add_project(home, "main".into());
    let key = state.project(&project).unwrap().project_key.clone();
    state
        .recover_run(run_record("run-vanished-archived", &key, &checkout))
        .unwrap();
    assert_eq!(
        state.handle(req("project.archive", json!({"project_id":project})))["ok"],
        true
    );
    std::fs::remove_dir(&checkout).unwrap();
    let decide = || {
        vec![DecidedVanishedRun {
            run_id: "run-vanished-archived".into(),
            published: Default::default(),
        }]
    };
    state.archive_vanished_runs(decide());
    assert_eq!(
        state.runs["run-vanished-archived"].run.state,
        RunState::Interrupted
    );
    assert_eq!(
        state.handle(req("project.restore", json!({"project_id":project})))["ok"],
        true
    );
    state.archive_vanished_runs(decide());
    assert_eq!(
        state.runs["run-vanished-archived"].run.state,
        RunState::Archived
    );
}

fn deferred_run_fixture(root: &Path, adopted: bool) -> (AppState, String, PersistedRun) {
    let (mut state, project) = super::tracker::tracked_with_origin(root);
    let checkout = state.project(&project).unwrap().orch.worktrees()
        .create("restore-obligation", "main", crate::isolation::Isolation::Worktree).unwrap().worktree;
    let mut record = run_record("run-restore-obligation", &state.project(&project).unwrap().project_key, &checkout.path);
    record.branch = checkout.branch();
    record.worktree_name = checkout.name;
    record.adopted = adopted;
    state.store.as_ref().unwrap().save_run(&record).unwrap();
    state.projects.at_mut(0).archived_at = Some(1);
    (state, project, record)
}

#[test]
fn restore_resumes_boot_deferred_publication_with_existing_checkout() {
    let root = tempfile::tempdir().unwrap();
    let (mut state, project, mut record) = deferred_run_fixture(root.path(), true);
    let checkout = Path::new(&record.worktree_path);
    crate::git_fixture::git_in(checkout, &["push", "-u", "origin", &record.branch]);
    let head = git2::Repository::open(checkout).unwrap().head().unwrap().target().unwrap().to_string();
    record.publication_attempt = Some(crate::run::PublicationAttempt {action:"push".into(),candidate_sha:head,started_at:record.updated_at.clone()});
    state.store.as_ref().unwrap().save_run(&record).unwrap();
    state.recover_run(record.clone()).unwrap();
    assert!(state.runs[&record.id].publication_attempt.is_some());
    assert_eq!(state.handle(req("project.restore", json!({"project_id":project})))["ok"], true);
    assert!(state.runs[&record.id].publication_attempt.is_none(), "restore must discharge the skipped publication journal");
    assert_eq!(state.runs[&record.id].last_error, None);
    let before = serde_json::to_value(state.store.as_ref().unwrap().load_all_runs().unwrap()).unwrap();
    state.handle(req("project.restore", json!({"project_id":project})));
    assert_eq!(serde_json::to_value(state.store.as_ref().unwrap().load_all_runs().unwrap()).unwrap(), before);
}

#[test]
fn restore_resumes_boot_deferred_native_checkout_before_vanished_sweep() {
    let root = tempfile::tempdir().unwrap();
    let (mut state, project, record) = deferred_run_fixture(root.path(), false);
    std::fs::remove_dir_all(&record.worktree_path).unwrap();
    state.recover_run(record.clone()).unwrap();
    assert!(!Path::new(&record.worktree_path).exists());
    assert_eq!(state.handle(req("project.restore", json!({"project_id":project})))["ok"], true);
    state.sweep_vanished_runs();
    assert!(Path::new(&record.worktree_path).is_dir(), "restore must remake the native checkout from its retained branch");
    assert_eq!(state.runs[&record.id].run.state, RunState::Interrupted);
}
