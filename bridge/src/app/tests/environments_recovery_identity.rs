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
