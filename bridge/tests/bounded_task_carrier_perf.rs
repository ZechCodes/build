//! Serial production-profile task-read gate through encrypted carrier admission.
#![cfg(feature = "testing")]
mod project_bridge;
use build_bridge::app::AppState;
use build_bridge::carrier::testing::{client_request, session_init, ChannelWire};
use build_bridge::carrier::{FrameIntake, SessionSender};
use build_bridge::harness::HarnessContext;
use build_bridge::tracker::{Actor, Task};
use build_bridge::transport;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
const SAMPLES: usize = 100;
const CEILING: Duration = Duration::from_millis(100);

fn repository(root: &Path) -> std::path::PathBuf {
    let path = root.join("repo");
    std::fs::create_dir(&path).unwrap();
    let mut options = git2::RepositoryInitOptions::new();
    options.initial_head("main");
    let repo = git2::Repository::init_opts(&path, &options).unwrap();
    std::fs::write(path.join("README.md"), "fixture\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("README.md")).unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let signature = git2::Signature::now("Fixture", "fixture@example.invalid").unwrap();
    repo.commit(Some("HEAD"), &signature, &signature, "fixture", &tree, &[])
        .unwrap();
    std::fs::canonicalize(path).unwrap()
}
fn seed(store: &build_bridge::store::Store, project: &str, count: usize) {
    for number in 1..=count {
        let mut task = Task::drafted(
            project,
            &format!("Task {number}"),
            Actor::User,
            "2026-10-01T00:00:00Z",
        );
        task.number = number as u64;
        task.labels = vec!["duplicate".into(), format!("label-{:03}", number % 130)];
        task.body = "body omitted from every summary".repeat(200);
        let saved = store.create_tracker_task(task, &[]).unwrap();
        assert_eq!(saved.number, number as u64);
    }
}

fn assert_materialized_fixture(store: &build_bridge::store::Store, project: &str, count: usize) {
    let mut below = None;
    let mut remaining = count as u64;
    loop {
        let rows = store
            .list_task_summary_candidates(project, below, 2001)
            .unwrap();
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            assert_eq!(
                row.row["number"], remaining,
                "carrier fixture must contain every materialized task in descending order"
            );
            remaining -= 1;
        }
        below = rows.last().unwrap().row["number"].as_u64();
    }
    assert_eq!(
        remaining, 0,
        "carrier fixture must materialize all {count} canonical tasks"
    );
    let choices = store
        .list_task_filter_choices(project, "label", None, 500)
        .unwrap();
    assert_eq!(
        choices.len(),
        count.min(130) + 1,
        "carrier fixture distinct labels must include the shared duplicate membership"
    );
}

fn validate_sample(count: usize, scenario: usize, result: &Value) -> Result<usize, String> {
    if scenario == 3 {
        let choices = result["choices"]
            .as_array()
            .ok_or("filter choices result has no choice array")?;
        let expected = std::iter::once("duplicate".to_string())
            .chain((1..=count.min(130)).map(|number| format!("label-{:03}", number % 130)))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .take(100)
            .map(Value::String)
            .collect::<Vec<_>>();
        if choices != &expected {
            return Err(format!("filter choices cardinality/content mismatch: expected {} choices, got {}: {choices:?}",expected.len(),choices.len()));
        }
        return Ok(choices.len());
    }
    let tasks = result["tasks"]
        .as_array()
        .ok_or("summary result has no task array")?;
    let expected = match scenario {
        0 => count.min(100),
        1 => count.min(500),
        2 => 0,
        _ => return Err(format!("unknown carrier scenario {scenario}")),
    };
    if tasks.len() != expected {
        return Err(format!(
            "summary cardinality mismatch: expected {expected} tasks, got {}",
            tasks.len()
        ));
    }
    for (offset, task) in tasks.iter().enumerate() {
        if task["number"].as_u64() != Some((count - offset) as u64) {
            return Err(format!(
                "summary order mismatch at row {offset}: {}",
                task["number"]
            ));
        }
    }
    Ok(tasks.len())
}

struct Fixture {
    root: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    store: build_bridge::store::Store,
    intake: Arc<FrameIntake>,
    wire: ChannelWire,
    key: String,
    project: String,
    count: usize,
}
impl Fixture {
    fn new(count: usize) -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = repository(root.path());
        let context =
            HarnessContext::resolved(root.path().join("mcp.sock"), root.path().to_path_buf())
                .unwrap();
        let mut app =
            AppState::new_unrooted_configured(root.path().join("worktrees"), "main", true, context)
                .with_config(root.path().join("config.json"))
                .unwrap()
                .with_task_store(root.path().join("store"))
                .unwrap();
        let project = app.add_project(repo.clone(), "main".into());
        let store = app.attached_store_for_test().unwrap();
        let started = Instant::now();
        seed(&store, repo.to_str().unwrap(), count);
        println!(
            "carrier_setup tasks={count} seed_us={}",
            started.elapsed().as_micros()
        );
        let started = Instant::now();
        assert_materialized_fixture(&store, repo.to_str().unwrap(), count);
        println!(
            "carrier_setup tasks={count} fixture_validation_us={}",
            started.elapsed().as_micros()
        );
        let started = Instant::now();
        let state = app.shared();
        println!(
            "carrier_setup tasks={count} shared_us={}",
            started.elapsed().as_micros()
        );
        let started = Instant::now();
        let transport = transport::generate_transport_keypair();
        let intake = FrameIntake::new(AppState::handler(Arc::clone(&state)), transport.clone());
        println!(
            "carrier_setup tasks={count} intake_us={}",
            started.elapsed().as_micros()
        );
        let mut wire = ChannelWire::open();
        let key = transport::generate_session_key();
        wire.open_session(
            &intake,
            "bounded-perf",
            &session_init("bounded-perf", &transport.public_key_b64, &key),
        )
        .unwrap();
        wire.outbound.try_recv().unwrap();
        Self {
            root,
            state,
            store,
            intake,
            wire,
            key,
            project,
            count,
        }
    }
    fn hold_locks(&self) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
        let state = Arc::clone(&self.state);
        let store = self.store.clone();
        let database = self.root.path().join("store/build.db");
        let (ready, waiting) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _app = state.lock().unwrap();
            let _writer = store.writer_for_test();
            let writer = Connection::open(database).unwrap();
            writer.execute_batch("BEGIN IMMEDIATE").unwrap();
            ready.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(120)).unwrap();
            writer.execute_batch("ROLLBACK").unwrap();
        });
        waiting.recv_timeout(Duration::from_secs(5)).unwrap();
        (release, holder)
    }
    async fn read(&mut self, id: usize, method: &str, mut params: Value) -> Value {
        params["project_id"] = json!(self.project);
        self.wire
            .accept(
                &self.intake,
                client_request(
                    &self.key,
                    "bounded-perf",
                    "data",
                    json!({"id":id,"method":method,"params":params}),
                ),
            )
            .await
            .unwrap();
        loop {
            let envelope = self.wire.outbound.recv().await.unwrap();
            let response = SessionSender::decrypt_push(&self.key, &envelope);
            if response.get("accepted").is_none() && response.get("id") == Some(&json!(id)) {
                return response;
            }
        }
    }
    async fn samples(
        &mut self,
        scenario: usize,
        method: &str,
        params: Value,
    ) -> Result<(Vec<Duration>, usize, usize), String> {
        let mut times = Vec::new();
        let mut max_bytes = 0;
        let mut rows = 0;
        for sample in 0..SAMPLES {
            let started = Instant::now();
            let response = tokio::time::timeout(
                Duration::from_secs(5),
                self.read(scenario * SAMPLES + sample, method, params.clone()),
            )
            .await
            .map_err(|error| error.to_string())?;
            times.push(started.elapsed());
            if response["ok"] != true {
                return Err(response.to_string());
            }
            max_bytes = max_bytes.max(serde_json::to_vec(&response["result"]).unwrap().len());
            rows = validate_sample(self.count, scenario, &response["result"])?;
        }
        Ok((times, max_bytes, rows))
    }
}
struct ScenarioResult {
    tasks: usize,
    scenario: usize,
    method: &'static str,
    result: Result<(Vec<Duration>, usize, usize), String>,
}

fn report_results(results: Vec<ScenarioResult>, emit: impl FnMut(String)) {
    report_collected_results(results, None, emit);
}

fn report_latency_results(results: Vec<ScenarioResult>, emit: impl FnMut(String)) {
    report_collected_results(results, Some(CEILING), emit);
}

fn report_collected_results(
    results: Vec<ScenarioResult>,
    latency_ceiling: Option<Duration>,
    mut emit: impl FnMut(String),
) {
    let mut failures = Vec::new();
    for ScenarioResult {
        tasks,
        scenario,
        method,
        result,
    } in results
    {
        let (mut times, bytes, rows) = match result {
            Ok(samples) => samples,
            Err(error) => {
                emit(format!("carrier_read tasks={tasks} scenario={scenario} method={method} error={error:?}"));
                failures.push(format!(
                    "{method} tasks={tasks} scenario={scenario}: {error}"
                ));
                continue;
            }
        };
        times.sort();
        let p99 = times[(times.len() * 99).div_ceil(100) - 1];
        let max = *times.last().unwrap();
        emit(format!("carrier_read profile={} tasks={tasks} scenario={scenario} method={method} samples={} rows={rows} bytes={bytes} p99_us={} max_us={}",if cfg!(debug_assertions){"debug"}else{"release"},times.len(),p99.as_micros(),max.as_micros()));
        if bytes > 1_048_576 {
            failures.push(format!(
                "{method} tasks={tasks} scenario={scenario}: bytes={bytes} exceeds 1048576"
            ));
        }
        if latency_ceiling.is_some_and(|ceiling| max > ceiling) {
            failures.push(format!(
                "{method} tasks={tasks} scenario={scenario}: max={max:?}, p99={p99:?}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn correctness_report_accepts_slow_valid_results_for_all_scenarios() {
    let mut results = Vec::new();
    for tasks in [1, 100, 5000] {
        for scenario in 0..4 {
            results.push(ScenarioResult {
                tasks,
                scenario,
                method: "tasks.list",
                result: Ok((
                    vec![CEILING + Duration::from_millis(1); SAMPLES],
                    1_048_576,
                    1,
                )),
            });
        }
    }
    let mut reports = Vec::new();
    report_results(results, |line| reports.push(line));
    assert_eq!(
        reports.len(),
        12,
        "default correctness must report all 1,200 requests"
    );
    assert!(reports.iter().all(|line| line.contains("max_us=101000")));
}

#[test]
fn correctness_report_rejects_oversized_results_even_when_slow() {
    let mut reports = Vec::new();
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        report_results(
            vec![ScenarioResult {
                tasks: 1,
                scenario: 0,
                method: "tasks.list",
                result: Ok((
                    vec![CEILING + Duration::from_millis(1); SAMPLES],
                    1_048_577,
                    1,
                )),
            }],
            |line| reports.push(line),
        );
    }));
    assert!(
        failure.is_err(),
        "byte failures must remain nonzero in default correctness"
    );
    assert_eq!(reports.len(), 1);
    let message = failure.unwrap_err().downcast::<String>().unwrap();
    assert!(message.contains("bytes=1048577 exceeds 1048576"));
    assert!(
        !message.contains("max="),
        "latency is observed by default, not rejected"
    );
}

#[test]
fn reports_all_scenarios_before_rejecting_latency_and_byte_failures() {
    let mut results = Vec::new();
    for tasks in [1, 100, 5000] {
        for scenario in 0..4 {
            let time = if tasks == 1 && scenario == 0 {
                CEILING + Duration::from_nanos(1)
            } else {
                CEILING
            };
            let bytes = if tasks == 100 && scenario == 1 {
                1_048_577
            } else {
                1_048_576
            };
            results.push(ScenarioResult {
                tasks,
                scenario,
                method: "tasks.list",
                result: Ok((vec![time; SAMPLES], bytes, 1)),
            });
        }
    }
    let mut reports = Vec::new();
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        report_latency_results(results, |line| reports.push(line));
    }));
    assert!(
        failure.is_err(),
        "hard latency/byte failures must remain nonzero"
    );
    assert_eq!(
        reports.len(),
        12,
        "all 1,200 collected requests must be reported before failing"
    );
    assert!(reports.last().unwrap().contains("tasks=5000 scenario=3"));
    let error = failure.unwrap_err();
    let message = error.downcast_ref::<String>().unwrap();
    assert!(message.contains("max=100.000001ms"));
    assert!(message.contains("bytes=1048577"));
}

#[test]
fn reports_protocol_failure_and_later_results_before_aggregate_rejection() {
    let results = vec![
        ScenarioResult {
            tasks: 1,
            scenario: 0,
            method: "tasks.list",
            result: Err("request refused".into()),
        },
        ScenarioResult {
            tasks: 1,
            scenario: 1,
            method: "tasks.list",
            result: Ok((
                vec![CEILING + Duration::from_millis(1); SAMPLES],
                1_048_576,
                1,
            )),
        },
    ];
    let mut reports = Vec::new();
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        report_results(results, |line| reports.push(line));
    }));
    assert!(failure.is_err());
    assert_eq!(reports.len(), 2);
    assert!(reports[0].contains("request refused"));
    assert!(reports[1].contains("scenario=1"));
    let message = failure.unwrap_err().downcast::<String>().unwrap();
    assert!(message.contains("request refused"));
    assert!(
        !message.contains("max="),
        "protocol refusal must be the only failure"
    );
}

#[test]
fn reports_and_accepts_exact_latency_and_byte_limits() {
    let mut reports = Vec::new();
    report_latency_results(
        vec![ScenarioResult {
            tasks: 1,
            scenario: 0,
            method: "tasks.list",
            result: Ok((vec![CEILING; SAMPLES], 1_048_576, 1)),
        }],
        |line| reports.push(line),
    );
    assert_eq!(reports.len(), 1);
}

#[tokio::test]
async fn rejects_early_oversized_sample_when_final_sample_is_small() {
    let tools = tempfile::tempdir().unwrap();
    std::env::set_var("PATH", project_bridge::private_path(tools.path()));
    let mut fixture = Fixture::new(1);
    let oversized = json!({"tasks":[{"number":1}],"padding":"x".repeat(1_048_576)});
    let expected_bytes = serde_json::to_vec(&oversized).unwrap().len();
    let handler = build_bridge::carrier::FrameHandler::new(
        build_bridge::timing::FrameClock::new(),
        move |_sender, frame, _timer| {
            let result = if frame.payload["id"] == 0 {
                oversized.clone()
            } else {
                json!({"tasks":[{"number":1}]})
            };
            json!({"id":frame.payload["id"],"ok":true,"result":result})
        },
    );
    let transport = transport::generate_transport_keypair();
    fixture.intake = FrameIntake::new(handler, transport.clone());
    fixture.wire = ChannelWire::open();
    fixture
        .wire
        .open_session(
            &fixture.intake,
            "bounded-perf",
            &session_init("bounded-perf", &transport.public_key_b64, &fixture.key),
        )
        .unwrap();
    fixture.wire.outbound.try_recv().unwrap();
    let samples = fixture
        .samples(0, "carrier.syntheticBytes", json!({}))
        .await
        .unwrap();
    assert_eq!(samples.0.len(), SAMPLES);
    assert_eq!(
        samples.1, expected_bytes,
        "retain the largest sample, not the last"
    );
    let mut reports = Vec::new();
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        report_results(
            vec![ScenarioResult {
                tasks: 1,
                scenario: 0,
                method: "carrier.syntheticBytes",
                result: Ok(samples),
            }],
            |line| reports.push(line),
        );
    }));
    assert!(
        failure.is_err(),
        "an early oversized result must fail the gate"
    );
    assert!(reports[0].contains(&format!("bytes={expected_bytes}")));
    let message = failure.unwrap_err().downcast::<String>().unwrap();
    assert!(message.contains(&format!("bytes={expected_bytes} exceeds 1048576")));
}

#[test]
fn rejects_empty_or_out_of_order_carrier_results_before_timing_aggregation() {
    assert!(validate_sample(3, 0, &json!({"tasks":[]}))
        .unwrap_err()
        .contains("expected 3 tasks"));
    assert!(validate_sample(3, 3, &json!({"choices":[]}))
        .unwrap_err()
        .contains("expected 4 choices"));
    assert!(validate_sample(
        3,
        0,
        &json!({"tasks":[{"number":1},{"number":2},{"number":3}]})
    )
    .unwrap_err()
    .contains("order mismatch"));
    assert_eq!(validate_sample(3, 2, &json!({"tasks":[]})).unwrap(), 0);
    assert_eq!(
        validate_sample(
            3,
            3,
            &json!({"choices":["duplicate","label-001","label-002","label-003"]})
        )
        .unwrap(),
        4
    );
}

#[tokio::test]
async fn seeded_carrier_fixture_has_materialized_tasks_and_choices() {
    let tools = tempfile::tempdir().unwrap();
    std::env::set_var("PATH", project_bridge::private_path(tools.path()));
    let fixture = Fixture::new(3);
    let project = std::fs::canonicalize(fixture.root.path().join("repo")).unwrap();
    let rows = fixture
        .store
        .list_task_summary_candidates(project.to_str().unwrap(), None, 10)
        .unwrap();
    assert_eq!(
        rows.len(),
        3,
        "carrier fixture must populate materialized task cards"
    );
    assert_eq!(
        rows.iter()
            .map(|row| row.row["number"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![3, 2, 1]
    );
    let choices = fixture
        .store
        .list_task_filter_choices(project.to_str().unwrap(), "label", None, 500)
        .unwrap();
    assert_eq!(
        choices.len(),
        4,
        "shared duplicate label plus three distinct task labels"
    );
}

async fn collect_carrier_results() -> Vec<ScenarioResult> {
    let tools = tempfile::tempdir().unwrap();
    // This standalone test process must never discover installed provider CLIs.
    std::env::set_var("PATH", project_bridge::private_path(tools.path()));
    let mut results = Vec::new();
    for count in [1, 100, 5000] {
        let mut fixture = Fixture::new(count);
        let (release, holder) = fixture.hold_locks();
        for (scenario, (method, params)) in [
            ("tasks.list", json!({"summary":true})),
            ("tasks.list", json!({"summary":true,"limit":500})),
            ("tasks.list", json!({"summary":true,"labels":["absent"]})),
            ("tasks.filterChoices", json!({"dimension":"label"})),
        ]
        .into_iter()
        .enumerate()
        {
            results.push(ScenarioResult {
                tasks: count,
                scenario,
                method,
                result: fixture.samples(scenario, method, params).await,
            });
        }
        release.send(()).unwrap();
        holder.join().unwrap();
    }
    results
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bounded_reads_carrier_contents_and_bytes_with_both_locks_held() {
    report_results(collect_carrier_results().await, |line| println!("{line}"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "timing: release-profile isolated latency gate; see timing.yml"]
async fn bounded_reads_carrier_max_and_p99_with_both_locks_held() {
    report_latency_results(collect_carrier_results().await, |line| println!("{line}"));
}
