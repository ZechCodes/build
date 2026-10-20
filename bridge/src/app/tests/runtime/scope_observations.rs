use super::*;

#[test]
fn file_and_git_observations_answer_while_the_app_mutex_is_held() {
    let (directory, repository) = init_repo();
    std::fs::write(repository.join("observation.txt"), "visible\n").unwrap();
    let app = qa_state(&repository, directory.path());
    let project = app.project_at(0).id.clone();
    let state = app.shared();
    let handler = AppState::handler(Arc::clone(&state));
    let held = state.lock().unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let replies = [
            ("fs.tree", json!({"project_id": project})),
            (
                "fs.read",
                json!({"project_id": project, "path": "observation.txt"}),
            ),
            ("git.status", json!({"project_id": project})),
        ]
        .map(|(method, params)| {
            handler.call(
                SessionSender::detached("observation-reader"),
                req(method, params),
            )
        });
        send.send(replies).unwrap();
    });
    let answers = receive.recv_timeout(Duration::from_millis(500));
    drop(held);
    worker.join().unwrap();
    let answers = answers.expect("file and Git reads must finish before releasing the app lock");
    for answer in &answers {
        assert_eq!(answer["ok"], true, "{answer}");
    }
    assert_eq!(answers[1]["result"]["content_b64"], b64encode(b"visible\n"));
}

#[cfg(unix)]
#[test]
fn observation_file_reads_keep_symlink_fences_and_byte_bounds() {
    let (directory, repository) = init_repo();
    let outside = directory.path().join("outside.txt");
    std::fs::write(&outside, "outside").unwrap();
    std::os::unix::fs::symlink(&outside, repository.join("escape.txt")).unwrap();
    std::fs::write(repository.join("large.txt"), vec![b'x'; 1_048_577]).unwrap();
    let app = qa_state(&repository, directory.path());
    let project = app.project_at(0).id.clone();
    let handler = app.into_handler();
    let sender = SessionSender::detached("bounded-reader");
    for path in ["escape.txt", "../outside.txt"] {
        let answer = handler.call(
            sender.clone(),
            req(
                "fs.read",
                json!({
                    "project_id": project, "path": path
                }),
            ),
        );
        assert_eq!(answer["ok"], false, "{answer}");
    }
    let answer = handler.call(
        sender.clone(),
        req(
            "fs.read",
            json!({
                "project_id": project, "path": "large.txt"
            }),
        ),
    );
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["truncated"], true);
    let bytes =
        crate::encoding::b64decode(answer["result"]["content_b64"].as_str().unwrap()).unwrap();
    assert_eq!(bytes.len(), 1_048_576);
    let page = handler.call(sender, req("fs.read", json!({
        "project_id": project, "path": "large.txt", "range": {"offset": 4096, "bytes": 4096, "raw": true}
    })));
    assert_eq!(page["ok"], true, "{page}");
    assert_eq!(page["result"]["range"]["offset"], 4096);
    assert_eq!(page["result"]["range"]["end"], 8192);
    let bytes =
        crate::encoding::b64decode(page["result"]["content_b64"].as_str().unwrap()).unwrap();
    assert_eq!(bytes.len(), 4096);
}

struct ScopeFixture {
    directory: tempfile::TempDir,
    repository: PathBuf,
    project: String,
    state: Arc<Mutex<AppState>>,
    handler: crate::carrier::FrameHandler,
}

impl ScopeFixture {
    fn new() -> Self {
        let (directory, repository) = init_repo();
        let app = qa_state(&repository, directory.path())
            .with_config(directory.path().join("config.json"))
            .unwrap();
        let project = app.project_at(0).id.clone();
        let state = app.shared();
        let handler = AppState::handler(Arc::clone(&state));
        Self {
            directory,
            repository,
            project,
            state,
            handler,
        }
    }

    fn call(&self, method: &str, params: Value) -> Value {
        self.handler.call(
            SessionSender::detached("scope-observation"),
            req(method, params),
        )
    }

    fn plain_source(&self) -> PathBuf {
        let path = self.directory.path().join("plain-source");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("note.txt"), "original\n").unwrap();
        let added = self.call(
            "project.add_source",
            json!({"project_id": self.project, "path": path}),
        );
        assert_eq!(added["ok"], true, "{added}");
        path
    }

    fn prepare(&self, method: &str, params: Value) -> crate::carrier::PreparedObservation {
        self.state
            .lock()
            .unwrap()
            .observation_service()
            .prepare(req(method, params), None)
    }

    fn context(&self) -> Arc<crate::app::observation_reads::ObservationContext> {
        self.state
            .lock()
            .unwrap()
            .read_view
            .load()
            .observations
            .clone()
            .unwrap()
    }

    fn inject_context(&self, context: Arc<crate::app::observation_reads::ObservationContext>) {
        let app = self.state.lock().unwrap();
        app.read_view.publish_delta(
            crate::app::read_view::SnapshotDelta::new(app.read_view.next_version())
                .set_observations(context),
        );
    }
}

#[test]
fn scope_source_move_fences_captured_reads_and_publishes_the_new_location() {
    let fixture = ScopeFixture::new();
    let original = fixture.plain_source();
    let old_scope = json!({"project_id": fixture.project, "source_id": "source-2", "source_path": original, "path": "note.txt"});
    let captured = fixture.prepare("fs.read", old_scope.clone());
    let without_publication = fixture.prepare("fs.read", old_scope.clone());
    let previous_context = fixture.context();
    let moved = fixture.directory.path().join("moved-source");
    std::fs::create_dir(&moved).unwrap();
    std::fs::write(moved.join("note.txt"), "moved\n").unwrap();
    let changed = fixture.call(
        "project.update_source",
        json!({
            "project_id": fixture.project, "source_id": "source-2", "path": moved
        }),
    );
    assert_eq!(changed["ok"], true, "{changed}");
    let current_context = fixture.context();
    // Fault injection through the existing immutable root: omitting the new
    // producer publication would let the old file through its old fence.
    fixture.inject_context(previous_context);
    let unfenced = without_publication();
    fixture.inject_context(current_context);
    assert_eq!(unfenced["ok"], true, "{unfenced}");
    assert_eq!(unfenced["result"]["content_b64"], b64encode(b"original\n"));
    let stale = captured();
    assert_eq!(stale["error_code"], "conflict", "{stale}");
    let stale = fixture.call("fs.read", old_scope);
    assert_eq!(stale["error_code"], "conflict", "{stale}");
    let current = fixture.call("fs.read", json!({
        "project_id": fixture.project, "source_id": "source-2", "source_path": moved, "path": "note.txt"
    }));
    assert_eq!(current["ok"], true, "{current}");
    assert_eq!(current["result"]["content_b64"], b64encode(b"moved\n"));
}

#[test]
fn scope_source_membership_and_non_git_errors_match_existing_contracts() {
    let fixture = ScopeFixture::new();
    let path = fixture.plain_source();
    let scope =
        json!({"project_id": fixture.project, "source_id": "source-2", "source_path": path});
    let plain_git = fixture.call("git.status", scope.clone());
    assert_eq!(plain_git["error_code"], "invalid_params", "{plain_git}");
    assert_eq!(
        plain_git["error"],
        crate::app::NOT_GIT_SOURCE,
        "{plain_git}"
    );
    let other = init_repo_named(fixture.directory.path(), "other-project");
    let added = fixture.call(
        "project.create",
        json!({"name": "Other", "sources": [{"path": other}]}),
    );
    assert_eq!(added["ok"], true, "{added}");
    let foreign = fixture.call("fs.tree", json!({
        "project_id": added["result"]["project_id"], "source_id": "source-2", "source_path": path
    }));
    assert_eq!(foreign["error_code"], "not_found", "{foreign}");
    let captured = fixture.prepare("fs.tree", scope.clone());
    let removed = fixture.call(
        "project.remove_source",
        json!({"project_id": fixture.project, "source_id": "source-2"}),
    );
    assert_eq!(removed["ok"], true, "{removed}");
    let old = captured();
    assert_eq!(old["error_code"], "not_found", "{old}");
    let fresh = fixture.call("fs.tree", scope);
    assert_eq!(fresh["error_code"], "not_found", "{fresh}");
}

#[test]
fn scope_files_and_git_observe_acknowledged_mutations() {
    let fixture = ScopeFixture::new();
    std::fs::write(fixture.repository.join("note.txt"), "before\n").unwrap();
    let scope = json!({"project_id": fixture.project, "path": "note.txt"});
    let before = fixture.call("fs.read", scope.clone());
    assert_eq!(before["ok"], true, "{before}");
    let written = fixture.call("fs.write", json!({
        "project_id": fixture.project, "path": "note.txt", "content_b64": b64encode(b"after\n"), "expected_revision": before["result"]["revision"]
    }));
    assert_eq!(written["ok"], true, "{written}");
    let after = fixture.call("fs.read", scope);
    assert_eq!(
        after["result"]["content_b64"],
        b64encode(b"after\n"),
        "{after}"
    );
    let staged = fixture.call(
        "git.stage",
        json!({"project_id": fixture.project, "paths": ["note.txt"]}),
    );
    assert_eq!(staged["ok"], true, "{staged}");
    let status = fixture.call("git.status", json!({"project_id": fixture.project}));
    assert_eq!(status["ok"], true, "{status}");
    assert_eq!(
        status["result"]["status_key"], staged["result"]["status_key"],
        "{status}"
    );
    assert_eq!(status["result"]["files"][0]["staged"], "full", "{status}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gated_scope_file_and_git_jobs_leave_sql_snapshot_and_control_traffic_free() {
    for method in ["fs.read", "git.status"] {
        gated_scope_job_leaves_other_reads_free(method).await;
    }
}

async fn gated_scope_job_leaves_other_reads_free(method: &'static str) {
    use crate::carrier::testing::{client_request, session_init, ChannelWire};
    use crate::carrier::{FrameIntake, OutboundEnvelope};
    let (directory, repository) = init_repo();
    let mut app = qa_state(&repository, directory.path());
    let project = app.project_at(0).id.clone();
    let task = crate::app::tests::tracker::filed(&mut app, &project, "independent SQL");
    let (gate, permits) = OffLockGate::new();
    app.off_lock_gate = Some(gate);
    let state = app.shared();
    let handler = AppState::handler(Arc::clone(&state));
    let clock = Arc::clone(&state.lock().unwrap().frame_clock);
    let transport = crate::transport::generate_transport_keypair();
    let intake = FrameIntake::new(handler.clone(), transport.clone());
    let mut wire = ChannelWire::open();
    let key = crate::transport::generate_session_key();
    wire.open_session(
        &intake,
        "scope",
        &session_init("scope", &transport.public_key_b64, &key),
    )
    .unwrap();
    assert!(matches!(
        wire.outbound.try_recv(),
        Ok(OutboundEnvelope::SessionAccept { .. })
    ));
    let held = std::thread::spawn(move || {
        handler.call(
            SessionSender::detached("held-scope"),
            req(method, json!({"project_id": project, "path": "README.md"})),
        )
    });
    permits.wait_for_arrival();
    for (id, method, params) in [
        (101, "tasks.get", json!({"task_id": task["id"]})),
        (102, "board.list", json!({})),
        (103, "ping", json!({})),
    ] {
        wire.accept(
            &intake,
            client_request(
                &key,
                "scope",
                "data",
                json!({"id":id,"method":method,"params":params}),
            ),
        )
        .await
        .unwrap();
        let answer = tokio::time::timeout(
            Duration::from_millis(500),
            scope_wire_answer(&mut wire, &key),
        )
        .await
        .expect("SQL, snapshots and control must answer before releasing the observation job");
        assert_eq!(answer["id"], id, "{answer}");
        assert_eq!(answer["ok"], true, "{answer}");
    }
    permits.release();
    let answer = held.join().unwrap();
    assert_eq!(answer["ok"], true, "{answer}");
    let stats = clock.stats();
    for observed in [method, "tasks.get", "board.list"] {
        assert_eq!(
            stats["methods"][observed]["lock_wait"]["sum_ms"], 0.0,
            "{stats}"
        );
        assert_eq!(stats["methods"][observed]["held"]["sum_ms"], 0.0, "{stats}");
    }
}

async fn scope_wire_answer(wire: &mut crate::carrier::testing::ChannelWire, key: &str) -> Value {
    loop {
        let envelope = wire.outbound.recv().await.unwrap();
        let answer = SessionSender::decrypt_push(key, &envelope);
        if answer.get("accepted").is_none() {
            return answer;
        }
    }
}

fn seed_scope_stage(fixture: &ScopeFixture) -> (String, String, PathBuf) {
    let mut app = fixture.state.lock().unwrap();
    let task = file_legacy_task(&mut app, "retained observation stage");
    let owner = adopted_run(
        &mut app,
        &fixture.repository,
        fixture.directory.path(),
        "retained-scope",
    );
    let mut run = app.take_run(&owner).unwrap();
    let checkout = run.worktree.path.clone();
    let sha = git2::Repository::open(&checkout)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap()
        .to_string();
    let mut stage = crate::run::StageProgress::dispatched("stage-one");
    stage.start_sha = Some(sha.clone());
    stage.completion_sha = Some(sha.clone());
    run.run.plan_id = Some(crate::plan::PlanId::new(&task));
    run.base_sha = Some(sha);
    run.stages = vec![stage];
    app.finish_run_mutation(owner.clone(), run).unwrap();
    (task, owner, checkout)
}

#[test]
fn scope_failed_run_path_base_and_stage_candidate_stays_out_of_later_reads() {
    let fixture = ScopeFixture::new();
    let (task, owner, _) = seed_scope_stage(&fixture);
    let params = json!({"task_id": task, "stage_id": "stage-one"});
    let stage_before = fixture.call("task.stage_diff", params.clone());
    let diff_before = fixture.call("run.diff", json!({"run_id": owner}));
    assert_eq!(stage_before["ok"], true, "{stage_before}");
    assert_eq!(diff_before["ok"], true, "{diff_before}");
    let poisoned = {
        let mut app = fixture.state.lock().unwrap();
        let mut candidate = app.take_run(&owner).unwrap();
        candidate.worktree.path = fixture.directory.path().join("failed-checkout");
        candidate.base_sha = Some("1111111111111111111111111111111111111111".into());
        candidate.worktree.base_branch = "failed-base".into();
        candidate.stages[0].completion_sha =
            Some("2222222222222222222222222222222222222222".into());
        app.runs.insert(owner.clone(), candidate);
        let poisoned = Arc::new(crate::app::observation_reads::ObservationContext::capture(
            &app,
        ));
        let candidate = app.runs.remove(&owner).unwrap();
        app.store.as_ref().unwrap().fail_next_write();
        assert!(app.finish_run_mutation(owner.clone(), candidate).is_err());
        file_legacy_task(&mut app, "unrelated successful publication");
        app.publish_read_metadata();
        poisoned
    };
    let committed = fixture.context();
    // A failed candidate really would change both reads if a producer were
    // allowed to install it. Restore the real root before the final assertions.
    fixture.inject_context(poisoned);
    let poisoned_stage = fixture.call("task.stage_diff", params.clone());
    let poisoned_diff = fixture.call("run.diff", json!({"run_id": owner}));
    fixture.inject_context(committed);
    assert_eq!(poisoned_stage["ok"], false, "{poisoned_stage}");
    assert_eq!(poisoned_diff["ok"], false, "{poisoned_diff}");
    let stage_after = fixture.call("task.stage_diff", params);
    let diff_after = fixture.call("run.diff", json!({"run_id": owner}));
    assert_eq!(
        stage_after, stage_before,
        "failed boundary leaked into immutable scope inputs"
    );
    assert_eq!(
        diff_after, diff_before,
        "failed checkout or base leaked into immutable scope inputs"
    );
}

#[test]
fn scope_stage_reads_choose_completed_retained_lineage_and_its_original_repository() {
    let fixture = ScopeFixture::new();
    let (task, first, checkout) = seed_scope_stage(&fixture);
    {
        let mut app = fixture.state.lock().unwrap();
        let later = adopted_run(
            &mut app,
            &fixture.repository,
            fixture.directory.path(),
            "later-scope",
        );
        let mut run = app.take_run(&later).unwrap();
        run.run.plan_id = Some(crate::plan::PlanId::new(&task));
        run.stages = vec![crate::run::StageProgress::dispatched("stage-one")];
        app.finish_run_mutation(later, run).unwrap();
        // Recovery retains this exact repository text after its live project
        // route is parked. The historical object database must survive it.
        app.projects
            .retain_entity_path(first.clone(), fixture.repository.display().to_string());
        app.projects.unbind_live_entity(&first);
        app.publish_read_metadata();
    }
    git_in(
        &fixture.repository,
        &["worktree", "remove", "--force", checkout.to_str().unwrap()],
    );
    let answer = fixture.call(
        "task.stage_diff",
        json!({"task_id": task, "stage_id": "stage-one"}),
    );
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["result"]["run_id"], first, "{answer}");
    assert_eq!(answer["result"]["status"], "available", "{answer}");
}

#[test]
fn scope_reads_never_adopt_legacy_records_or_schedule_external_scans() {
    let fixture = ScopeFixture::new();
    let legacy = format!("legacy-{}", fixture.project);
    {
        let mut app = fixture.state.lock().unwrap();
        assert!(app.workspaces.get(&legacy).is_some());
        app.workspaces.forget(&legacy);
        app.clear_external_scan_for_test(&fixture.project);
        app.publish_read_metadata();
    }
    let workspace = fixture.call(
        "fs.tree",
        json!({"workspace_id": legacy, "source_id": "source-1"}),
    );
    assert_eq!(workspace["error_code"], "not_found", "{workspace}");
    let external = fixture.call(
        "worktree.diff",
        json!({"project_id": fixture.project, "worktree_id": "unknown"}),
    );
    assert_eq!(external["error_code"], "not_found", "{external}");
    let app = fixture.state.lock().unwrap();
    assert!(
        app.workspaces.get(&legacy).is_none(),
        "read adopted an absent legacy record"
    );
    assert!(
        !app.diff_refresh_is_running(&crate::app::DiffCacheKey::ExternalScan(
            fixture.project.clone()
        )),
        "read scheduled a scan"
    );
}

#[test]
fn scope_workspace_delete_retires_captured_workspace_and_run_addresses() {
    let fixture = ScopeFixture::new();
    let created = fixture.call(
        "workspace.create",
        json!({"project_id": fixture.project, "name": "removed scope", "isolation": "worktree"}),
    );
    assert_eq!(created["ok"], true, "{created}");
    let workspace = created["result"]["workspace_id"].as_str().unwrap();
    let source = created["result"]["directories"][0]["source_id"]
        .as_str()
        .unwrap();
    let conversation = fixture.call(
        "workspace.ensure_conversation",
        json!({"workspace_id": workspace}),
    );
    assert_eq!(conversation["ok"], true, "{conversation}");
    let owner = conversation["result"]["entity_id"].as_str().unwrap();
    let file = fixture.prepare(
        "fs.tree",
        json!({"workspace_id": workspace, "source_id": source}),
    );
    let git = fixture.prepare("git.status", json!({"run_id": owner}));
    let deleted = fixture.call("workspace.delete", json!({"workspace_id": workspace}));
    assert_eq!(deleted["ok"], true, "{deleted}");
    assert_eq!(file()["ok"], false);
    assert_eq!(git()["ok"], false);
    let fresh_file = fixture.call(
        "fs.tree",
        json!({"workspace_id": workspace, "source_id": source}),
    );
    let fresh_git = fixture.call("git.status", json!({"run_id": owner}));
    assert_eq!(fresh_file["error_code"], "not_found", "{fresh_file}");
    assert_eq!(fresh_git["error_code"], "not_found", "{fresh_git}");
    let app = fixture.state.lock().unwrap();
    assert!(app.workspaces.get(workspace).is_none());
    assert!(!app.runs.contains_key(owner));
}
