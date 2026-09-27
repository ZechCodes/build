use super::*;

/// Retiring task creation does not make existing records disposable. A
/// restart must still load their metadata through the read-only compatibility
/// surface and leave an associated checkout on disk.
#[test]
fn legacy_task_and_run_load_without_removing_their_checkout() {
    let (dir, repo) = init_repo();
    let checkout = add_external_worktree(&repo, dir.path(), "legacy-run", "legacy-run");
    let store = crate::store::Store::new(dir.path().join("store")).expect("store opens");
    store
        .save_task_plan(&drafting_plan("plan-1", &repo))
        .unwrap();
    store
        .save_run(&building_run("run-1", &repo, &checkout))
        .unwrap();

    let mut state = qa_state(&repo, dir.path());
    let task = state.handle(req("task.get", json!({ "task_id": "plan-1" })));
    assert_eq!(task["ok"], true, "{task:?}");
    assert_eq!(task["result"]["task_id"], "plan-1", "{task:?}");
    assert_eq!(task["result"]["goal"], "drafting", "{task:?}");

    let tasks = state.handle(req("task.list", json!({})));
    assert!(
        tasks["result"]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["task_id"] == "plan-1"),
        "{tasks:?}"
    );
    assert!(
        checkout.is_dir(),
        "loading legacy state preserves its checkout"
    );
}

#[test]
fn corrupt_run_record_fails_boot_naming_the_file() {
    let (dir, repo) = init_repo();
    let runs_dir = dir.path().join("store").join("runs");
    std::fs::create_dir_all(&runs_dir).unwrap();
    std::fs::write(runs_dir.join("run-bad.json"), "{ not json").unwrap();
    let context =
        HarnessContext::resolved(dir.path().join("test-mcp.sock"), dir.path().to_path_buf())
            .unwrap();
    let err = AppState::new_configured(repo.clone(), dir.path().join("wt"), "main", true, context)
        .with_task_store(dir.path().join("store"))
        .err()
        .expect("boot should fail on a corrupt record");
    assert!(err.contains("run-bad.json"), "{err}");
}

fn drafting_plan(id: &str, repo: &std::path::Path) -> PersistedPlan {
    PersistedPlan {
        id: id.into(),
        goal: "drafting".into(),
        project_path: repo.display().to_string(),
        base_branch: "main".into(),
        state: PlanState::Drafting,
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

pub(in crate::app::tests) fn building_run(
    id: &str,
    repo: &std::path::Path,
    worktree: &std::path::Path,
) -> PersistedRun {
    PersistedRun {
        id: id.into(),
        plan_id: Some("plan-1".into()),
        goal: "building".into(),
        project_path: repo.display().to_string(),
        base_branch: "main".into(),
        state: RunState::Building,
        branch: "legacy-run".into(),
        worktree_name: id.into(),
        worktree_path: worktree.display().to_string(),
        base_sha: None,
        stages: Vec::new(),
        current_stage_id: None,
        revising_stage_id: None,
        auto_advance: false,
        adopted: false,
        publication_attempt: None,
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
