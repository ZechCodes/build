//! Step 4 paired observation measurements over real encrypted intake.
//! All reads name real temporary resources; gh is an injected local script.
mod benchmark;

use super::*;
use std::os::unix::fs::PermissionsExt;

struct IoSample {
    sample: Sample,
    requests: Vec<(&'static str, Value)>,
    pending: std::collections::BTreeMap<u64, &'static str>,
    setup: Vec<Value>,
    worker_starts: Arc<std::sync::atomic::AtomicUsize>,
    sql_worker_starts: Arc<std::sync::atomic::AtomicUsize>,
    read_pool_initialization: Value,
}

impl IoSample {
    fn new() -> Self {
        Self::new_with_clock(FrameClock::new())
    }

    fn new_with_clock(clock: Arc<FrameClock>) -> Self {
        Self::new_with_clock_and_sql_startup(clock, false)
    }

    fn new_with_clock_and_sql_startup(clock: Arc<FrameClock>, eager_sql: bool) -> Self {
        let mut setup = Vec::new();
        let mut start = std::time::Instant::now();
        let mut sample = Sample::new_with_clock(1, None, clock);
        benchmark::setup_mark(&mut setup, "helper_fixture_and_unused_intake", &mut start);
        let (requests, read_pool_initialization) = {
            let mut app = sample.state.lock().unwrap();
            let requests = fixture_requests(&mut app, &sample);
            app.refresh_snapshot_observations();
            app.publish_all_snapshot_projection();
            let initialization = app
                .store
                .as_ref()
                .unwrap()
                .read_pool()
                .initialization_timing();
            let read_pool_initialization = json!({
                "total_ns":initialization.total.as_nanos(),
                "schema_total_ns":initialization.schema_total.as_nanos(),
                "schema_count":initialization.schema_count,
                "semantics":"natural ReadPool::open total is nested inside helper_fixture_and_unused_intake; schema subtotal is nested inside that total. Neither is additional client latency. No benchmark warmup query."
            });
            (requests, read_pool_initialization)
        };
        benchmark::setup_mark(&mut setup, "extra_fixture_seed", &mut start);
        let mut handler = AppState::handler(Arc::clone(&sample.state));
        // Set only the lazy-start policy; actual startup remains in the timed first request.
        handler.set_observation_sql_startup_eager(eager_sql);
        let sql_worker_starts = handler.sql_worker_starts();
        let worker_starts = handler.observation_worker_starts();
        benchmark::setup_mark(&mut setup, "production_handler", &mut start);
        let transport = crate::transport::generate_transport_keypair();
        sample.intake = FrameIntake::with_pool(handler, transport.clone(), 128, 1);
        benchmark::setup_mark(&mut setup, "production_intake", &mut start);
        sample.wire = ChannelWire::open();
        sample
            .wire
            .open_session(
                &sample.intake,
                SESSION,
                &session_init(SESSION, &transport.public_key_b64, &sample.key),
            )
            .unwrap();
        assert!(matches!(
            sample.wire.outbound.try_recv(),
            Ok(OutboundEnvelope::SessionAccept { .. })
        ));
        benchmark::setup_mark(&mut setup, "session_open", &mut start);
        Self {
            read_pool_initialization,
            sql_worker_starts,
            worker_starts,
            setup,
            sample,
            requests,
            pending: Default::default(),
        }
    }

    async fn reads(&mut self, serial: bool) {
        for (method, params) in self.requests.clone() {
            self.pending.insert(self.sample.next_id, method);
            self.sample.send(method, params).await;
            if serial {
                self.answers(1).await;
            }
        }
    }

    async fn answers(&mut self, count: usize) {
        for _ in 0..count {
            loop {
                let answer = SessionSender::decrypt_push(
                    &self.sample.key,
                    &within_patience(self.sample.wire.outbound.recv()).await,
                );
                if answer.get("accepted").is_some() {
                    continue;
                }
                assert_eq!(answer["ok"], true, "{answer}");
                let id = answer["id"].as_u64().unwrap();
                if let Some(method) = self.pending.remove(&id) {
                    validate_payload(method, &answer["result"]);
                }
                break;
            }
        }
        assert!(
            self.pending.is_empty(),
            "every observation reply was verified"
        );
    }

    fn report(&self, scenario: &str, gate: &str) {
        assert!(
            self.pending.is_empty(),
            "every observation reply was verified"
        );
        let stats = self.sample.clock.stats();
        let methods: serde_json::Map<_, _> = self
            .requests
            .iter()
            .map(|(method, _)| {
                let metric = &stats["methods"][method];
                assert_eq!(metric["served"], SAMPLE_COUNT, "{method}: {stats}");
                for phase in ["queued", "lock_wait", "held"] {
                    assert_eq!(metric[phase]["count"], SAMPLE_COUNT, "{method}.{phase}");
                }
                (method.to_string(), metric.clone())
            })
            .collect();
        emit_baseline(
            &baseline_output_dir(self.sample._state_root.path()),
            scenario,
            gate,
            1,
            &json!(methods),
            &json!({}),
        );
    }
}

fn fixture_requests(app: &mut AppState, sample: &Sample) -> Vec<(&'static str, Value)> {
    let repo = app.project_at(0).repo_path.clone();
    let hash = git2::Repository::open(&repo)
        .unwrap()
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    let workspace =
        crate::app::tests::project_agent::workspace(app, &sample.project_id, "I/O timing");
    let source = app.workspaces.get(&workspace).unwrap().directories[0]
        .source_id
        .clone();
    let mut requests = vec![
        ("git.log", json!({"project_id":sample.project_id})),
        (
            "git.show",
            json!({"project_id":sample.project_id,"hash":hash}),
        ),
        ("git.status", json!({"project_id":sample.project_id})),
        (
            "git.diff",
            json!({"project_id":sample.project_id,"paths":["README.md"]}),
        ),
        ("git.refs", json!({"project_id":sample.project_id})),
        ("git.unpushed", json!({"project_id":sample.project_id})),
        (
            "git.changeset_diff",
            json!({"project_id":sample.project_id,"paths":["README.md"]}),
        ),
        ("fs.list", json!({"path":repo})),
        ("fs.tree", json!({"project_id":sample.project_id})),
        (
            "fs.read",
            json!({"project_id":sample.project_id,"path":"README.md"}),
        ),
        (
            "workspace.git_init_options",
            json!({"workspace_id":workspace,"source_id":source}),
        ),
    ];
    requests.extend(review_requests(app, sample, &workspace));
    requests.extend(legacy_requests(app, sample, &repo));
    requests.extend(attachment_requests(app, sample));
    let gh = sample._state_root.path().join("fixture-gh");
    std::fs::write(&gh, r##"#!/bin/sh
case "$1" in
  repo) echo '[{"nameWithOwner":"fixture/repository","description":"temporary timing fixture","sshUrl":"git@example.invalid:fixture/repository.git","url":"https://example.invalid/fixture/repository","isPrivate":true}]' ;;
  org) exit 0 ;;
esac
"##).unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    app.github = crate::github::GithubCli::at(gh, "temporary-fixture");
    std::fs::write(
        repo.join("README.md"),
        "# project\nTiming observation edit\n",
    )
    .unwrap();
    requests.push(("github.repos", json!({})));
    requests.push(("archived.list", json!({})));
    requests
}

fn review_requests(
    app: &mut AppState,
    sample: &Sample,
    workspace: &str,
) -> Vec<(&'static str, Value)> {
    let task = &sample.task_ids[0];
    let saved = app.handle(req(
        "tasks.review.snapshot",
        json!({
        "task_id":task,"workspace_id":workspace,"expected_version":0}),
    ));
    assert_eq!(saved["ok"], true, "{saved}");
    let snapshot = &saved["result"]["review"]["snapshots"][0];
    vec![
        ("tasks.review.get", json!({"task_id":task})),
        (
            "tasks.review.diff",
            json!({"task_id":task,"snapshot_id":snapshot["id"],
            "directory_id":snapshot["directories"][0]["id"],"mode":"blob","path":"README.md"}),
        ),
    ]
}

fn legacy_requests(app: &mut AppState, sample: &Sample, repo: &Path) -> Vec<(&'static str, Value)> {
    let single = file_legacy_task(app, "single timing document");
    let path = app.plans.get(&single).unwrap().plan_path.clone();
    let source = sample._state_root.path().join("document-source");
    std::fs::create_dir_all(source.join(&path).parent().unwrap()).unwrap();
    std::fs::write(source.join(&path), "# Timing document\nStored contents\n").unwrap();
    app.require_store()
        .unwrap()
        .ingest_plan_docs(&single, &source, &path)
        .unwrap();
    let (plan, run) = planned_run_in_review(app, "staged timing document");
    // The scripted helper queues a legacy stage turn; observation fixtures
    // retain its saved lineage without starting a background delivery.
    app.delivery_queue.clear_queued();
    let stage = app.plans.get(&plan).unwrap().stages[0].id.clone();
    app.plans.get_mut(&single).unwrap().plan.archived_at = Some("2026-10-20T12:00:00Z".into());
    add_external_worktree(
        repo,
        sample._state_root.path(),
        "timing-external",
        "timing-external",
    );
    let external = app
        .scan_external_worktrees_now(&sample.project_id)
        .unwrap()
        .into_iter()
        .find(|worktree| worktree.branch.as_deref() == Some("timing-external"))
        .unwrap();
    vec![
        ("task.doc", json!({"task_id":single})),
        ("task.stage_doc", json!({"task_id":plan,"stage_id":stage})),
        ("task.stage_diff", json!({"task_id":plan,"stage_id":stage})),
        ("run.diff", json!({"run_id":run})),
        (
            "worktree.diff",
            json!({"project_id":sample.project_id,"worktree_id":external.id}),
        ),
    ]
}

fn attachment_requests(app: &mut AppState, sample: &Sample) -> Vec<(&'static str, Value)> {
    let bytes = crate::encoding::b64encode(b"Timing attachment payload\n");
    let task = app.handle(req(
        "tasks.attach",
        json!({"project_id":sample.project_id,
        "filename":"timing.txt","content_b64":bytes}),
    ));
    assert_eq!(task["ok"], true, "{task}");
    let thread = app.handle(req(
        "thread.attach",
        json!({"entity_id":sample.history.owner,
        "agent_id":sample.history.agent,"filename":"thread.txt","content_b64":bytes}),
    ));
    assert_eq!(thread["ok"], true, "{thread}");
    vec![
        (
            "tasks.attachment",
            json!({"task_id":sample.task_ids[0],"path":task["result"]["path"],"offset":0,"length":12}),
        ),
        (
            "thread.attachment",
            json!({"entity_id":sample.history.owner,"agent_id":sample.history.agent,
            "path":thread["result"]["path"],"offset":0,"length":12}),
        ),
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn isolated_io_observations_publish_a_file_backed_baseline() {
    let mut sample = IoSample::new();
    for _ in 0..SAMPLE_COUNT {
        sample.reads(true).await;
    }
    sample.report("io_isolated", "none");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn io_observations_measure_an_app_held_write() {
    let mut sample = IoSample::new();
    for _ in 0..SAMPLE_COUNT {
        let state = Arc::clone(&sample.sample.state);
        let (entered, acquired) = mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _guard = state.lock().unwrap();
            entered.send(()).unwrap();
            std::thread::sleep(GATE);
        });
        acquired.recv_timeout(PATIENCE).unwrap();
        sample.reads(false).await;
        sample.answers(sample.requests.len()).await;
        holder.join().unwrap();
    }
    sample.report("io_app_held_write", "app mutex held 75ms per batch");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn io_observations_measure_an_ordinary_git_worker() {
    let mut sample = IoSample::new();
    for _ in 0..SAMPLE_COUNT {
        let (gate, handle) = crate::app::OffLockGate::new();
        sample.sample.state.lock().unwrap().off_lock_gate = Some(gate);
        sample
            .sample
            .send(
                "git.stage",
                json!({"project_id":sample.sample.project_id,"paths":["README.md"]}),
            )
            .await;
        handle.wait_for_arrival();
        sample.sample.state.lock().unwrap().off_lock_gate = None;
        let holder = std::thread::spawn(move || {
            std::thread::sleep(GATE);
            handle.release();
        });
        sample.reads(false).await;
        sample.answers(sample.requests.len() + 1).await;
        holder.join().unwrap();
    }
    sample.report(
        "io_ordinary_git_worker",
        "ordinary Git writer worker held 75ms per batch",
    );
}

fn validate_payload(method: &str, result: &Value) {
    match method {
        "task.doc" | "task.stage_doc" => assert!(!result["contents"].as_str().unwrap().is_empty()),
        "tasks.attachment" | "thread.attachment" => assert_eq!(
            crate::encoding::b64decode(result["content_b64"].as_str().unwrap())
                .unwrap()
                .len(),
            12
        ),
        "tasks.review.get" => assert_eq!(result["review"]["version"], 1),
        "tasks.review.diff" => assert_eq!(
            result["content_b64"],
            crate::encoding::b64encode(b"# project\n")
        ),
        "github.repos" => assert_eq!(result["repos"].as_array().unwrap().len(), 1),
        "archived.list" => assert!(!result["items"].as_array().unwrap().is_empty()),
        _ => assert!(result.is_object(), "{method}: {result}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn io_observations_are_independent_of_application_mutex() {
    let mut sample = IoSample::new();
    let (gate, acquired, mut release) = WorkGate::new();
    let state = Arc::clone(&sample.sample.state);
    release.holder = Some(std::thread::spawn(move || {
        let _guard = state.lock().unwrap();
        gate.wait();
    }));
    acquired.recv_timeout(PATIENCE).unwrap();
    sample.reads(false).await;
    let count = sample.requests.len();
    let completed = tokio::time::timeout(PATIENCE, sample.answers(count)).await;
    release.assert_held();
    release.finish();
    if completed.is_err() {
        let remaining = sample.pending.len();
        sample.answers(remaining).await;
    }
    completed.expect("all observation reads finish while the application mutex remains held");
}
