use super::*;

#[test]
fn named_nodes_survive_real_receipts_then_unrelated_publications_and_retained_roots() {
    let (_tmp, mut app) = fixture();
    let (canonical, alias) = bind_alias(&mut app);
    app.publish_all_snapshot_projection();
    change_topic(&mut app, "run-00", "receipt-shared named branch");
    let old = publish_incremental(&app, "run-00");
    let requests = requests(&app);
    let before = image(&old, &requests);
    let old_named = state_node(&old, "run-00");
    let committed = app
        .store
        .as_ref()
        .unwrap()
        .set_delivery_status(
            &canonical,
            None,
            1,
            1,
            crate::thread::MessageDeliveryStatus::Sent,
        )
        .unwrap();
    app.apply_committed_thread_items(&committed);
    let patched = app.read_view.load();
    let patched_before = image(&patched, &requests);
    assert_receipt(
        &patched_before,
        &canonical,
        &alias,
        &committed.thread_id,
        committed.generation_revision,
        committed.sequence,
    );
    // This real branch shape has no last_sequence field. Matching history,
    // summary chunks and push tips advance; its named payload stays unchanged.
    assert!(old_named.ptr_eq(&state_node(&patched, "run-00")));
    let mut unrelated = app.take_run("run-02").unwrap();
    unrelated.run.goal = "unrelated after named receipt".into();
    app.finish_run_mutation("run-02".into(), unrelated).unwrap();
    let next = app.read_view.load();
    assert_receipt(
        &image(&next, &requests),
        &canonical,
        &alias,
        &committed.thread_id,
        committed.generation_revision,
        committed.sequence,
    );
    assert_full_reference(&app, &next, &requests);
    // The full oracle intentionally installs independently seeded nodes.
    // Re-prime both alias owners through the real incremental producer before
    // testing that a task-only publication retains their shared nodes.
    assert_fast_eligible(&app, "run-00");
    let primed = publish_incremental(&app, "run-00");
    assert_eq!(image(&next, &requests), image(&primed, &requests));
    let project = app.projects.project_id_of("run-00").unwrap().to_owned();
    app.tasks_create(&json!({"project_id":project,"title":"task after named receipt"}))
        .unwrap();
    let task_root = app.read_view.load();
    assert_receipt(
        &image(&task_root, &requests),
        &canonical,
        &alias,
        &committed.thread_id,
        committed.generation_revision,
        committed.sequence,
    );
    assert_full_reference(&app, &task_root, &requests);
    assert_eq!(before, image(&old, &requests));
    assert_eq!(patched_before, image(&patched, &requests));
    for root in [&old, &patched, &next, &task_root] {
        assert_shared(root, "run-00");
        assert_shared(root, "run-01");
    }
}

#[test]
fn failed_save_and_rejected_candidate_cannot_install_a_private_named_node() {
    let (_tmp, mut app) = fixture();
    let requests = requests(&app);
    let old = app.read_view.load();
    let before = image(&old, &requests);
    let accepted_state = state_node(&old, "run-00");
    let selected = agent(&app, "run-00");
    let mut failed = app.take_run("run-00").unwrap();
    failed.agents.by_id_mut(&selected).unwrap().topic = Some("UNCOMMITTED named row".into());
    app.store.as_ref().unwrap().fail_next_write();
    assert!(app.finish_run_mutation("run-00".into(), failed).is_err());
    assert_eq!(before, image(&app.read_view.load(), &requests));
    assert!(accepted_state.ptr_eq(&state_node(&app.read_view.load(), "run-00")));
    change_topic(&mut app, "run-00", "STALE named row");
    let candidate = app.prepare_owner_snapshot_projection("run-00").unwrap();
    assert!(app.snapshot_ownership_context().is_none());
    app.runs
        .get_mut("run-00")
        .unwrap()
        .agents
        .by_id_mut(&selected)
        .unwrap()
        .topic = None;
    let version_only = SnapshotDelta::new(app.read_view.next_version())
        .set_global(old.global.as_ref().unwrap().clone());
    assert!(app.read_view.publish_delta(version_only));
    let advanced = app.read_view.load();
    app.publish_prepared_snapshot_projection(candidate);
    assert!(Arc::ptr_eq(&advanced, &app.read_view.load()));
    assert_eq!(before, image(&advanced, &requests));
    assert!(accepted_state.ptr_eq(&state_node(&advanced, "run-00")));
    let mut retry = app.take_run("run-00").unwrap();
    retry.agents.by_id_mut(&selected).unwrap().topic = Some("accepted named retry".into());
    app.finish_run_mutation("run-00".into(), retry).unwrap();
    let accepted = app.read_view.load();
    assert_literal(&accepted, "run-00", "accepted named retry");
    assert!(!values_contain(&accepted, "UNCOMMITTED"));
    assert!(!values_contain(&accepted, "STALE named"));
    assert_full_reference(&app, &accepted, &requests);
    assert_eq!(before, image(&old, &requests));
}

fn values_contain(view: &ReadView, needle: &str) -> bool {
    view.keys().any(|key| {
        view.get_owned(key.method, &key.owner)
            .unwrap()
            .to_string()
            .contains(needle)
    })
}

#[test]
fn named_rows_capture_fresh_runtime_status_and_elapsed_fields_on_each_guard() {
    let (_tmp, mut app) = fixture();
    let selected = agent(&app, "run-01");
    let root = app.runs["run-01"].worktree.path.clone();
    let status = crate::harness::SessionStatusSnapshot::new(crate::harness::AgentStatus::Working);
    let (send_status, status) = tokio::sync::watch::channel(status);
    let surfaces = AgentSurfaces {
        goal: Some(
            crate::harness::surfaces::SurfaceGoal::new(
                "fresh named goal",
                crate::harness::surfaces::GoalState::Active,
            )
            .0,
        ),
        ..AgentSurfaces::default()
    };
    app.session_registry.test_insert_tab(
        crate::app::TabKey::agent(&root, &selected),
        dictated_agent_tab(
            &root,
            "run-01",
            &selected,
            DictatedSession::reporting(crate::harness::AgentStatus::Working)
                .watching_status(status)
                .showing_surfaces(surfaces)
                .interruptible(),
        ),
    );
    app.runs
        .get_mut("run-01")
        .unwrap()
        .agents
        .by_id_mut(&selected)
        .unwrap()
        .working_since = Some("2999-01-01T00:00:00Z".into());
    app.publish_all_snapshot_projection();
    let old = publish_incremental(&app, "run-00");
    let requests = requests(&app);
    let before = image(&old, &requests);
    let prior = state_node(&old, "run-01");
    assert_eq!(prior.leaf().unwrap()["agents"][0]["working"], true);
    assert_eq!(prior.leaf().unwrap()["agents"][0]["can_interrupt"], true);
    send_status
        .send(crate::harness::SessionStatusSnapshot::new(
            crate::harness::AgentStatus::Waiting,
        ))
        .unwrap();
    // Before the status observer publishes its named owner, global/facts
    // already refresh this incidental active owner. Its separate agent.list
    // intentionally waits for that owner boundary; keep this root as evidence.
    let incidental = publish_incremental(&app, "run-00");
    let incidental_before = image(&incidental, &requests);
    let fresh = state_node(&incidental, "run-01");
    let row = fresh.leaf().unwrap();
    assert_eq!(row["agents"][0]["working"], false);
    assert_eq!(row["agents"][0]["can_interrupt"], false);
    assert_eq!(row["agents"][0]["working_time"]["seconds"], 0);
    assert!(row.to_string().contains("fresh named goal"));
    assert!(
        !prior.ptr_eq(&fresh),
        "accepted named memo leaked across publication guards"
    );
    // This is the same final owner-publication boundary used by StatusPump.
    // Drive it before requiring parity of all named reader chunks as well.
    let observed = publish_incremental(&app, "run-01");
    let observed_before = image(&observed, &requests);
    assert_eq!(
        observed.get_owned("agent.list", "run-01").unwrap()["agents"][0]["can_interrupt"],
        false
    );
    assert_full_reference(&app, &observed, &requests);
    let next = publish_incremental(&app, "run-00");
    assert_eq!(observed_before, image(&next, &requests));
    assert_full_reference(&app, &next, &requests);
    assert_eq!(before, image(&old, &requests));
    assert_eq!(incidental_before, image(&incidental, &requests));
    assert_eq!(observed_before, image(&observed, &requests));
}

#[test]
fn named_rows_preserve_raw_candidates_exact_stats_and_nonrun_fallbacks() {
    let (tmp, mut app) = fixture();
    let project = app.projects.project_id_of("run-00").unwrap().to_owned();
    app.projects.at_mut(0).name.clear();
    {
        let _guard = app.begin_snapshot_ownership();
        let stat_a = json!({"insertions":3,"ignored_raw_key":"a"});
        let stat_b = json!({"insertions":9,"ignored_raw_key":"b"});
        let raw = app.branch_candidate_from_run("run-00", &stat_a);
        assert_eq!(raw.row["project"], "");
        assert_eq!(raw.row["stat"]["insertions"], 3);
        assert_eq!(
            app.branch_candidate_from_run("run-00", &stat_b).row["stat"]["insertions"],
            9
        );
        assert_eq!(
            app.branch_candidate_from_run("run-00", &stat_a).row,
            raw.row
        );
        let facts = app.push_facts_shared_snapshot(SharedJson::default());
        assert_eq!(
            facts.state_for_test("run-00").unwrap().leaf().unwrap()["project"],
            project
        );
        assert_eq!(
            app.branch_candidate_from_run("run-00", &Value::Null).row["project"],
            "",
            "normalizing the shared wire must not overwrite the raw fold candidate"
        );
    }
    for insertions in [7, 11] {
        app.store_run_stat(
            "run-00".into(),
            json!({"insertions":insertions}),
            std::time::Instant::now(),
        );
        let next = publish_incremental(&app, "run-00");
        assert_eq!(
            state_node(&next, "run-00").leaf().unwrap()["stat"]["insertions"],
            insertions
        );
        let requests = requests(&app);
        assert_full_reference(&app, &next, &requests);
    }
    assert_nonrun_fallbacks(&mut app, tmp.path());
}

fn assert_nonrun_fallbacks(app: &mut AppState, root: &Path) {
    app.projects.at_mut(0).is_git = true;
    let plan = file_legacy_task(app, "legacy named-state fallback");
    app.runs.get_mut("run-03").unwrap().run.state = crate::run::RunState::Archived;
    let project = app.projects.at(0).id.clone();
    let path = root.join("external-named-fallback");
    std::fs::create_dir(&path).unwrap();
    let external = crate::worktree::ExternalWorktree {
        id: crate::worktree::external_worktree_id(&path),
        name: "external fallback".into(),
        path,
        isolation: crate::isolation::Isolation::Worktree,
        branch: Some("external-branch".into()),
        head_sha: "abc".into(),
        head_subject: "external head".into(),
        head_age_seconds: 5,
        head_committed_at: None,
        dirty_files: 0,
        unpushed: Some(0),
        upstream: None,
        comparison_ref: Some("main".into()),
        ahead: Some(0),
        behind: Some(0),
        diffstat: Default::default(),
        uncommitted: Default::default(),
    };
    app.board.diff_mut().register_project(project.clone());
    app.board.diff_mut().store_entry_for_adapter(
        crate::app::board_index::DiffCacheEntry::ExternalScan {
            project_id: project,
            worktrees: vec![external.clone()],
        },
        std::time::Instant::now(),
    );
    app.board.attention_mut().restore_entity_clocks(
        external.id.clone(),
        CLOCK.into(),
        CLOCK.into(),
        CLOCK.into(),
        "review".into(),
    );
    app.publish_all_snapshot_projection();
    let root = app.read_view.load();
    assert!(root.facts.state_for_test("run-03").is_none());
    assert_eq!(
        state_node(&root, &plan).materialize(),
        app.entity_state_item(&plan).unwrap()
    );
    assert!(root
        .global
        .as_ref()
        .unwrap()
        .named_winner_for_test(&plan)
        .is_none());
    assert_eq!(
        state_node(&root, &external.id).materialize(),
        app.entity_state_item(&external.id).unwrap()
    );
    assert!(root
        .global
        .as_ref()
        .unwrap()
        .named_winner_for_test(&external.id)
        .is_none());
    let requests = requests(app);
    assert_full_reference(app, &root, &requests);
}
