use super::*;

// ---- boot recovery + migration -------------------------------------------

#[test]
fn working_plan_and_run_surface_interrupted_on_boot() {
    let (dir, repo) = init_repo();
    let (_b, run_wt) = init_repo();
    let store = crate::store::Store::new(dir.path().join("store")).expect("store opens");
    store
        .save_issue_plan(&drafting_plan("plan-1", &repo))
        .unwrap();
    store
        .save_run(&building_run("run-1", &repo, &run_wt))
        .unwrap();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.get", json!({ "plan_id": "plan-1" })));
    assert_eq!(plan["result"]["state"], "interrupted", "{plan:?}");
    let run = state.handle(req("run.get", json!({ "run_id": "run-1" })));
    assert_eq!(run["result"]["state"], "interrupted", "{run:?}");
    // Unread is event-driven, so the restart has to say on the conversation
    // that it killed the session, or a parked task goes quiet.
    assert_eq!(run["result"]["needs_attention"], true, "{run:?}");
    assert_eq!(run["result"]["unread_reason"], "interrupted", "{run:?}");
    assert_eq!(plan["result"]["unread_reason"], "interrupted", "{plan:?}");
}

#[test]
fn missing_run_worktree_abandons_and_missing_repo_too() {
    let (dir, repo) = init_repo();
    let store = crate::store::Store::new(dir.path().join("store")).expect("store opens");
    // Worktree gone, repo present → abandoned (branch kept).
    let mut gone = building_run("run-gone", &repo, std::path::Path::new("/tmp/nope-run"));
    gone.state = RunState::Building;
    store.save_run(&gone).unwrap();
    // Repo gone, worktree present, native → abandoned with a reason.
    let (_w, wt) = init_repo();
    let mut norepo = building_run("run-norepo", std::path::Path::new("/tmp/nope-repo"), &wt);
    norepo.state = RunState::Building;
    store.save_run(&norepo).unwrap();

    let mut state = qa_state(&repo, dir.path());
    let g = state.handle(req("run.get", json!({ "run_id": "run-gone" })));
    assert_eq!(g["result"]["state"], "abandoned", "{g:?}");
    let n = state.handle(req("run.get", json!({ "run_id": "run-norepo" })));
    assert_eq!(n["result"]["state"], "abandoned", "{n:?}");
    assert!(n["result"]["last_error"]
        .as_str()
        .unwrap()
        .contains("project repo missing"));
}

#[test]
fn boot_recovers_journaled_push_from_configured_remote_evidence() {
    let (dir, repo) = init_repo();
    let remote = dir.path().join("mirror.git");
    git_in(dir.path(), &["init", "--bare", remote.to_str().unwrap()]);
    git_in(
        &repo,
        &["remote", "add", "mirror", remote.to_str().unwrap()],
    );
    git_in(&repo, &["checkout", "-b", "build/journaled"]);
    std::fs::write(repo.join("published.txt"), "published\n").unwrap();
    git_in(&repo, &["add", "published.txt"]);
    git_in(&repo, &["commit", "-m", "published candidate"]);
    let candidate = git_stdout(&repo, &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();
    git_in(&repo, &["push", "-u", "mirror", "build/journaled"]);
    // Force recovery to refresh remote evidence rather than trusting a
    // convenient local remote-tracking ref left by setup.
    git_in(
        &repo,
        &["update-ref", "-d", "refs/remotes/mirror/build/journaled"],
    );

    let store = Store::new(dir.path().join("store")).expect("store opens");
    let mut record = building_run("run-journaled", &repo, &repo);
    record.state = RunState::Review;
    record.branch = "build/journaled".into();
    record.stages = vec![StageProgress {
        stage_id: "only".into(),
        state: StageProgressState::Validated { passed: true },
        start_sha: None,
        built_sha: Some(candidate.clone()),
        completion_sha: Some(candidate.clone()),
        publication: StagePublication::Local,
        invalidation_reason: None,
        validation: None,
    }];
    record.publication_attempt = Some(PublicationAttempt {
        action: "push".into(),
        candidate_sha: candidate,
        started_at: "2026-07-01T10:00:00Z".into(),
    });
    store.save_run(&record).unwrap();

    let mut state = qa_state(&repo, dir.path());
    let recovered = state.handle(req("run.get", json!({ "run_id": "run-journaled" })));
    assert_eq!(recovered["ok"], true, "{recovered:?}");
    assert_eq!(recovered["result"]["stages"][0]["publication"], "pushed");
    let persisted = store
        .load_all_runs()
        .unwrap()
        .into_iter()
        .find(|run| run.id == "run-journaled")
        .unwrap();
    assert!(persisted.publication_attempt.is_none());
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

fn building_run(id: &str, repo: &std::path::Path, worktree: &std::path::Path) -> PersistedRun {
    PersistedRun {
        id: id.into(),
        plan_id: None,
        goal: "building".into(),
        project_path: repo.display().to_string(),
        base_branch: "main".into(),
        state: RunState::Building,
        branch: format!("build/{id}"),
        worktree_name: id.into(),
        worktree_path: worktree.display().to_string(),
        base_sha: None,
        stages: Vec::new(),
        current_stage_id: None,
        revising_stage_id: None,
        auto_advance: false,
        adopted: false,
        triage: None,
        recovery: None,
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
