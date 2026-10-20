//! File-backed SQL reads through the real carrier intake and worker pool.
//! Each sample uses a bounded gate to make one source of delay observable.
//!
//! For a persistent recapture, from `bridge/` after committing the source:
//! `capture_dir=$(mktemp -d /tmp/build-684-baseline.XXXXXX)`
//! `BUILD_CONCURRENCY_BASELINE_DIR="$capture_dir" nice -n 10 cargo test --lib concurrency_timing -- --nocapture --test-threads=1`
//! The fresh directory receives `bridge-concurrency-baseline.jsonl` and
//! `bridge-concurrency-baseline.md`. Ordinary runs write only to fixture temps.

use super::*;
use crate::app::tests::tracker::{filed, tracked};
use crate::carrier::testing::{client_request, session_init, within_patience, ChannelWire};
use crate::carrier::{FrameHandler, FrameIntake, OutboundEnvelope, SessionSender};
use crate::timing::FrameClock;
use serde_json::{json, Value};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const SESSION: &str = "timing-sample";
const GATE: Duration = Duration::from_millis(75);
const PATIENCE: Duration = Duration::from_secs(5);
const GATE_PATIENCE: Duration = Duration::from_secs(10);
const SAMPLE_COUNT: usize = 8;
const SQL_WORKERS: usize = 8;
const READ_METHODS: [&str; 6] = [
    "tasks.list",
    "tasks.get",
    "tasks.waiting_on.blocks",
    "thread.page",
    "thread.activity",
    "thread.operation",
];
const JSONL_FILE: &str = "bridge-concurrency-baseline.jsonl";
const MARKDOWN_FILE: &str = "bridge-concurrency-baseline.md";
static EMIT_LOCK: Mutex<()> = Mutex::new(());

/// Holds a synthetic write or Git worker until the test releases it. The
/// longer worker deadline leaves time for the bounded read attempt to fail
/// and its release guard to cancel the gate without poisoning app state.
struct WorkGate {
    entered: mpsc::Sender<()>,
    released: Mutex<mpsc::Receiver<bool>>,
    held: Arc<AtomicBool>,
}

impl WorkGate {
    fn new() -> (Self, mpsc::Receiver<()>, GateRelease) {
        let (entered, running) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let held = Arc::new(AtomicBool::new(false));
        (
            Self {
                entered,
                released: Mutex::new(released),
                held: Arc::clone(&held),
            },
            running,
            GateRelease {
                release: Some(release),
                held,
                holder: None,
            },
        )
    }

    fn wait(&self) -> bool {
        self.held.store(true, Ordering::Release);
        let _ = self.entered.send(());
        let released = self.released.lock().unwrap().recv_timeout(GATE_PATIENCE);
        self.held.store(false, Ordering::Release);
        released.expect("the test releases or cancels the worker gate")
    }

    fn run_git(&self, repo: &Path) -> bool {
        if !self.wait() {
            return false;
        }
        git2::Repository::open(repo)
            .unwrap()
            .statuses(None)
            .expect("the temporary checkout can be scanned");
        true
    }
}

/// Cancels blocked work on every failure path. Cancellation skips the Git
/// scan, so a worker cannot access a fixture after the failing test drops it.
struct GateRelease {
    release: Option<mpsc::Sender<bool>>,
    held: Arc<AtomicBool>,
    holder: Option<std::thread::JoinHandle<()>>,
}

impl GateRelease {
    fn assert_held(&self) {
        assert!(
            self.held.load(Ordering::Acquire),
            "worker gate must remain held through all SQL answers"
        );
    }

    fn finish(mut self) {
        self.signal(true);
        if let Some(holder) = self.holder.take() {
            holder.join().expect("the synthetic write finished cleanly");
        }
    }

    fn signal(&mut self, continue_work: bool) {
        if let Some(release) = self.release.take() {
            let _ = release.send(continue_work);
        }
    }
}

impl Drop for GateRelease {
    fn drop(&mut self) {
        self.signal(false);
        if let Some(holder) = self.holder.take() {
            let _ = holder.join();
        }
    }
}

fn emit_baseline(
    output_dir: &Path,
    scenario: &str,
    gate: &str,
    dispatch_workers: usize,
    methods: &Value,
) -> Value {
    let record = json!({
        "kind": "bridge_concurrency_baseline",
        "scenario": scenario,
        "gate": gate,
        "crate_version": env!("CARGO_PKG_VERSION"),
        "git_revision": git_revision(Path::new(env!("CARGO_MANIFEST_DIR"))),
        "rust_toolchain": rust_toolchain(Path::new("rustc")),
        "dispatch_workers": dispatch_workers,
        "sql_workers": SQL_WORKERS,
        "methods": methods,
    });
    append_artifacts(output_dir, &record);
    eprintln!("{record}");
    record
}

fn git_revision(repository: &Path) -> String {
    git2::Repository::discover(repository)
        .and_then(|repo| Ok(repo.head()?.peel_to_commit()?.id().to_string()))
        .unwrap_or_else(|_| "unknown".to_string())
}

fn rust_toolchain(rustc: &Path) -> String {
    std::process::Command::new(rustc)
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|version| version.trim().to_string())
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn append_artifacts(output_dir: &Path, record: &Value) {
    let _guard = EMIT_LOCK.lock().unwrap();
    std::fs::create_dir_all(output_dir).unwrap();
    let mut jsonl = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(output_dir.join(JSONL_FILE))
        .unwrap();
    writeln!(jsonl, "{record}").unwrap();

    let mut markdown = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(output_dir.join(MARKDOWN_FILE))
        .unwrap();
    if markdown.metadata().unwrap().len() == 0 {
        writeln!(markdown, "# Bridge concurrency baseline\n").unwrap();
    }
    markdown
        .write_all(markdown_section(record).as_bytes())
        .unwrap();
}

fn markdown_section(record: &Value) -> String {
    let mut section = format!(
        "## {}\n\n- Git revision: `{}`\n- Rust toolchain: `{}`\n- Dispatch workers: {}\n- SQL workers: {}\n- Crate version: `{}`\n- Gate: {}\n\n| Method | Phase | Count | Sum ms | p50 ms | p95 ms | Max ms |\n| --- | --- | ---: | ---: | ---: | ---: | ---: |\n",
        record["scenario"].as_str().unwrap(),
        record["git_revision"].as_str().unwrap(),
        record["rust_toolchain"].as_str().unwrap(),
        record["dispatch_workers"],
        record["sql_workers"],
        record["crate_version"].as_str().unwrap(),
        record["gate"].as_str().unwrap(),
    );
    for (method, stats) in record["methods"].as_object().unwrap() {
        for phase in ["queued", "lock_wait", "held"] {
            let metric = &stats[phase];
            section.push_str(&format!(
                "| {method} | {phase} | {} | {} | {} | {} | {} |\n",
                metric["count"],
                metric["sum_ms"],
                metric["p50_ms"],
                metric["p95_ms"],
                metric["max_ms"],
            ));
        }
    }
    section.push('\n');
    section
}

fn baseline_output_dir(fixture: &Path) -> PathBuf {
    // A fresh caller directory persists a recapture; ordinary test artifacts
    // disappear with the file-backed fixture.
    std::env::var_os("BUILD_CONCURRENCY_BASELINE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| fixture.join("baseline"))
}

#[test]
fn baseline_metadata_without_a_repository_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(git_revision(dir.path()), "unknown");
}

#[test]
fn baseline_metadata_without_a_compiler_is_unknown() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(rust_toolchain(&dir.path().join("missing-rustc")), "unknown");
}

#[test]
fn baseline_metadata_with_a_failed_compiler_is_unknown() {
    // Cargo-built test binaries are safe to run; this unsupported argument
    // gives a deterministic command failure without replacing HOME or PATH.
    let test_binary = std::env::current_exe().unwrap();
    assert_eq!(rust_toolchain(&test_binary), "unknown");
}

#[test]
fn baseline_emitter_writes_self_describing_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    let metric = json!({ "count": 8, "sum_ms": 12.0, "p50_ms": 2.0,
        "p95_ms": 5.0, "max_ms": 6.0 });
    let methods = json!({ "tasks.list": { "served": 8, "queued": metric,
        "lock_wait": metric, "held": metric } });
    let record = emit_baseline(dir.path(), "isolated", "none", 2, &methods);
    let revision = git_revision(Path::new(env!("CARGO_MANIFEST_DIR")));
    let toolchain = rust_toolchain(Path::new("rustc"));
    assert_eq!(record["git_revision"], revision);
    assert_eq!(record["rust_toolchain"], toolchain);
    assert_eq!(record["dispatch_workers"], 2);
    assert_eq!(record["sql_workers"], 8);

    let jsonl =
        std::fs::read_to_string(dir.path().join("bridge-concurrency-baseline.jsonl")).unwrap();
    assert_eq!(jsonl.lines().count(), 1);
    assert_eq!(
        serde_json::from_str::<Value>(jsonl.lines().next().unwrap()).unwrap(),
        record
    );
    let markdown =
        std::fs::read_to_string(dir.path().join("bridge-concurrency-baseline.md")).unwrap();
    assert!(markdown.contains(&revision));
    assert!(markdown.contains(&toolchain));
    assert!(markdown.contains("Dispatch workers: 2"));
    assert!(markdown.contains("SQL workers: 8"));
    assert!(markdown.contains("| tasks.list | queued | 8 | 12.0 | 2.0 | 5.0 | 6.0 |"));
}

struct HistorySample {
    owner: String,
    agent: String,
    message_sequences: Vec<u64>,
    first_activity: u64,
    last_activity: u64,
}

impl HistorySample {
    fn new(app: &mut AppState, project_id: &str) -> Self {
        let (owner, agent) = crate::app::tests::project_agent::project_agent(app, project_id);
        let (message_sequences, first_activity, last_activity) = app
            .edit_agent_conversation(&owner, &agent, |thread, _| {
                let mut message_sequences = Vec::new();
                for i in 0..SAMPLE_COUNT {
                    thread.post_user(format!("timing message {i}"), None, "2026-10-19T12:00:00Z");
                    message_sequences.push(thread.last_sequence());
                }
                let first_activity = thread.last_sequence() + 1;
                for i in 0..64 {
                    thread.push_event(
                        crate::thread::ThreadEventKind::ToolUse,
                        Some(format!("timing activity {i}")),
                        None,
                        None,
                        "2026-10-19T12:01:00Z",
                    );
                }
                Ok((message_sequences, first_activity, thread.last_sequence()))
            })
            .unwrap();
        let history = Self {
            owner,
            agent,
            message_sequences,
            first_activity,
            last_activity,
        };
        history.seed_operations(app);
        history
    }

    fn seed_operations(&self, app: &AppState) {
        let agents: Vec<_> = app
            .entity_agents(&self.owner)
            .unwrap()
            .iter()
            .cloned()
            .collect();
        for i in 0..SAMPLE_COUNT {
            let receipt = crate::operation::OperationReceipt {
                operation_id: format!("timing-operation-{i}"),
                method: "thread.post".to_string(),
                entity_id: self.owner.clone(),
                agent_id: self.agent.clone(),
                conversation_id: self.agent.clone(),
                choice_revision: 0,
                posted_sequence: self.message_sequences[i],
                message_start_sequence: self.message_sequences[i],
                status: crate::operation::OperationStatus::Delivered,
                execution_error: None,
                request_hash: format!("timing-request-{i}"),
                delivery: None,
                requested_by: None,
            };
            app.store
                .as_ref()
                .unwrap()
                .accept_thread_post(&self.owner, &agents, &receipt)
                .unwrap();
        }
    }

    fn requests(&self, i: usize) -> [(&'static str, Value); 3] {
        let mut owner = json!({ "entity_id": self.owner, "agent_id": self.agent });
        owner["limit"] = json!(i + 1);
        let page = owner.clone();
        owner["from_sequence"] = json!(self.first_activity);
        owner["through_sequence"] = json!(self.last_activity);
        let activity = owner;
        let operation = json!({ "entity_id": self.owner, "agent_id": self.agent,
            "operation_id": format!("timing-operation-{i}") });
        [
            ("thread.page", page),
            ("thread.activity", activity),
            ("thread.operation", operation),
        ]
    }
}

struct Sample {
    _state_root: tempfile::TempDir,
    _repo_home: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    clock: Arc<FrameClock>,
    intake: Arc<FrameIntake>,
    wire: ChannelWire,
    key: String,
    project_id: String,
    task_ids: Vec<String>,
    history: HistorySample,
    next_id: u64,
    dispatch_workers: usize,
}

impl Sample {
    fn new(workers: usize, git_gate: Option<WorkGate>) -> Self {
        let state_root = tempfile::tempdir().unwrap();
        let (repo_home, mut app, project_id) = tracked(state_root.path());
        let task_ids = (0..SAMPLE_COUNT)
            .map(|i| {
                filed(&mut app, &project_id, &format!("timing sample {i}"))["id"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        let history = HistorySample::new(&mut app, &project_id);
        let repo = app.project_at(0).repo_path.clone();
        let clock = FrameClock::new();
        app.frame_clock = Arc::clone(&clock);
        let state = app.shared();
        let (peers, reads, read_pool) = {
            let app = state.lock().unwrap();
            app.initialize_read_context();
            (
                app.peers_slot(),
                app.read_service(),
                app.store.as_ref().map(crate::store::Store::read_pool),
            )
        };
        let dispatched = Arc::clone(&state);
        let handler = FrameHandler::new(Arc::clone(&clock), move |sender, frame, timer| {
            if frame.payload["method"].as_str() == Some("git.status")
                && !git_gate
                    .as_ref()
                    .expect("only the Git sample sends git.status")
                    .run_git(&repo)
            {
                return json!({ "id": frame.payload["id"], "ok": false, "error": "timing fixture cancelled" });
            }
            crate::app::dispatch_frame(&dispatched, &peers, sender, frame, timer)
        })
        .with_sql_reads(read_pool, move |_sender, frame, _timer, connection| {
            reads.call(frame, connection)
        });
        let transport = crate::transport::generate_transport_keypair();
        let intake = FrameIntake::with_pool(handler, transport.clone(), 128, workers);
        let mut wire = ChannelWire::open();
        let key = crate::transport::generate_session_key();
        wire.open_session(
            &intake,
            SESSION,
            &session_init(SESSION, &transport.public_key_b64, &key),
        )
        .unwrap();
        assert!(matches!(
            wire.outbound.try_recv(),
            Ok(OutboundEnvelope::SessionAccept { .. })
        ));
        Self {
            _state_root: state_root,
            _repo_home: repo_home,
            state,
            clock,
            intake,
            wire,
            key,
            project_id,
            task_ids,
            history,
            next_id: 1,
            dispatch_workers: workers,
        }
    }

    async fn send(&mut self, method: &str, params: Value) {
        let payload = json!({ "id": self.next_id, "method": method, "params": params });
        self.next_id += 1;
        self.wire
            .accept(
                &self.intake,
                client_request(&self.key, SESSION, "data", payload),
            )
            .await
            .unwrap();
    }

    async fn reads(&mut self, i: usize, serial: bool) {
        let tasks = [
            (
                "tasks.list",
                json!({ "project_id": self.project_id, "limit": i + 1 }),
            ),
            ("tasks.get", json!({ "task_id": self.task_ids[i] })),
            (
                "tasks.waiting_on.blocks",
                json!({ "task_id": self.task_ids[i] }),
            ),
        ];
        for (method, params) in tasks.into_iter().chain(self.history.requests(i)) {
            self.send(method, params).await;
            if serial {
                self.answers(1).await;
            }
        }
    }

    async fn answers(&mut self, count: usize) {
        for _ in 0..count {
            loop {
                let answer = SessionSender::decrypt_push(
                    &self.key,
                    &within_patience(self.wire.outbound.recv()).await,
                );
                if answer.get("accepted").is_none() {
                    assert_eq!(answer["ok"], true, "{answer}");
                    break;
                }
            }
        }
    }

    async fn read_all_while_held(&mut self) {
        tokio::time::timeout(PATIENCE, async {
            for i in 0..SAMPLE_COUNT {
                self.reads(i, false).await;
            }
            tokio::time::sleep(GATE).await;
            self.answers(READ_METHODS.len() * SAMPLE_COUNT).await;
        })
        .await
        .expect("all SQL reads answer without releasing the held worker");
    }

    fn baseline(&self, scenario: &str, gate: &str) -> Value {
        let stats = self.clock.stats();
        let methods = &stats["methods"];
        for method in READ_METHODS {
            assert_eq!(
                methods[method]["served"].as_u64(),
                Some(SAMPLE_COUNT as u64),
                "{stats}"
            );
            for phase in ["queued", "lock_wait", "held"] {
                let metric = &methods[method][phase];
                assert_eq!(
                    metric["count"].as_u64(),
                    Some(SAMPLE_COUNT as u64),
                    "{method}.{phase}: {metric}"
                );
                for field in ["sum_ms", "p50_ms", "p95_ms", "max_ms"] {
                    assert!(
                        metric[field].as_f64().is_some(),
                        "missing {method}.{phase}.{field}: {metric}"
                    );
                }
            }
            assert_no_app_lock(method, &methods[method]);
        }
        emit_baseline(
            &baseline_output_dir(self._state_root.path()),
            scenario,
            gate,
            self.dispatch_workers,
            methods,
        )
    }
}

fn assert_no_app_lock(method: &str, stats: &Value) {
    for phase in ["lock_wait", "held"] {
        let metric = &stats[phase];
        // Counts include computations that never acquired the app lock.
        assert_eq!(metric["count"], SAMPLE_COUNT, "{method}.{phase}");
        for field in ["sum_ms", "p50_ms", "p95_ms", "max_ms"] {
            assert_eq!(metric[field], 0.0, "{method}.{phase}.{field}");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn isolated_task_reads_publish_a_file_backed_baseline() {
    let mut sample = Sample::new(2, None);
    for i in 0..SAMPLE_COUNT {
        sample.reads(i, true).await;
    }
    sample.baseline("isolated", "none");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_reads_measure_wait_behind_an_app_held_write() {
    let mut sample = Sample::new(2, None);
    let (gate, acquired, mut release) = WorkGate::new();
    let state = Arc::clone(&sample.state);
    let clock = Arc::clone(&sample.clock);
    release.holder = Some(std::thread::spawn(move || {
        let timer = clock.frame("tasks.update");
        let _guard = timer.lock(&state);
        gate.wait();
    }));
    acquired
        .recv_timeout(PATIENCE)
        .expect("the synthetic write holds the app mutex");
    sample.read_all_while_held().await;
    release.assert_held();
    release.finish();
    sample.baseline("app_held_write", "app mutex held for at least 75ms");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_reads_measure_queue_behind_off_lock_git_worker_work() {
    let (git_gate, entered, release) = WorkGate::new();
    let mut sample = Sample::new(1, Some(git_gate));
    sample
        .send("git.status", json!({ "project_id": sample.project_id }))
        .await;
    entered
        .recv_timeout(PATIENCE)
        .expect("the Git worker reached its gate");
    sample.read_all_while_held().await;
    release.assert_held();
    release.finish();
    sample.answers(1).await;
    sample.baseline("off_lock_git_worker", "Git worker held for at least 75ms");
}

/// Measures the complete production list query, including row projections
/// and per-task review, dependency and delivery reads, in one transaction.
/// Request validation and response validation follow the service boundaries
/// but stay outside the transaction timer; this is not whole-RPC latency.
#[tokio::test]
async fn unbounded_task_list_reports_full_query_transaction_duration() {
    let dir = tempfile::tempdir().unwrap();
    let (_repo_home, mut app, project) = tracked(dir.path());
    for number in 0..501 {
        filed(
            &mut app,
            &project,
            &format!("unbounded timing task {number}"),
        );
    }
    let context = app.task_read_context();
    let store = app.tracker_store().unwrap().clone();
    drop(app);
    let params =
        crate::api::v1::normalize_read_params("tasks.list", &json!({ "project_id": project }))
            .unwrap();
    assert!(
        params.get("limit").is_none(),
        "the wire request has no limit"
    );
    let writes_before = store.total_changes();
    let pool = store.read_pool();
    let checkout_started = std::time::Instant::now();
    let mut connection = pool.acquire().await;
    let checkout_us = checkout_started.elapsed().as_micros();
    let transaction_started = std::time::Instant::now();
    let (answer, fence) = connection
        .read(|read| {
            let fence = context.validate_scope(read, "tasks.list", &params)?;
            let answer = context.tasks_list(read, &params)?;
            Ok::<_, String>((answer, fence))
        })
        .unwrap()
        .unwrap();
    let transaction_us = transaction_started.elapsed().as_micros();
    context.validate_fence(&fence).unwrap();
    let answer = crate::api::v1::check_read_result("tasks.list", answer).unwrap();
    let count = answer["tasks"].as_array().unwrap().len();
    assert_eq!(
        count, 501,
        "omitting the wire limit preserves the whole list"
    );
    assert!(answer.get("next_cursor").is_none());
    assert_eq!(
        store.total_changes(),
        writes_before,
        "the query performs no writes"
    );
    let record = json!({
        "kind": "bridge_sql_read_sizing",
        "scope": "TaskReadContext::tasks_list transaction including scope validation",
        "git_revision": git_revision(Path::new(env!("CARGO_MANIFEST_DIR"))),
        "rust_toolchain": rust_toolchain(Path::new("rustc")),
        "method": "tasks.list",
        "wire_limit": null,
        "task_count": count,
        "checkout_us": checkout_us,
        "transaction_us": transaction_us,
    });
    eprintln!("{record}");
}
