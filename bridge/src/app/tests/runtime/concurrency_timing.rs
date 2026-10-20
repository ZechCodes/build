//! File-backed task reads through the real carrier intake and worker pool.
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
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const SESSION: &str = "timing-sample";
const GATE: Duration = Duration::from_millis(75);
const PATIENCE: Duration = Duration::from_secs(5);
const SAMPLE_COUNT: usize = 8;
const JSONL_FILE: &str = "bridge-concurrency-baseline.jsonl";
const MARKDOWN_FILE: &str = "bridge-concurrency-baseline.md";
static EMIT_LOCK: Mutex<()> = Mutex::new(());

/// Occupies a dispatch worker before it takes the app lock, then scans the
/// temporary repository. The bounded receive lets a failed test unwind.
struct GitGate {
    entered: mpsc::Sender<()>,
    released: Mutex<mpsc::Receiver<()>>,
}

impl GitGate {
    fn new() -> (Self, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered, running) = mpsc::channel();
        let (release, released) = mpsc::channel();
        (
            Self {
                entered,
                released: Mutex::new(released),
            },
            running,
            release,
        )
    }

    fn run(&self, repo: &Path) {
        self.entered.send(()).unwrap();
        self.released
            .lock()
            .unwrap()
            .recv_timeout(PATIENCE)
            .expect("the test releases the off-lock Git worker");
        git2::Repository::open(repo)
            .unwrap()
            .statuses(None)
            .expect("the temporary checkout can be scanned");
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
        "git_revision": git_revision(),
        "rust_toolchain": rust_toolchain(),
        "dispatch_workers": dispatch_workers,
        "methods": methods,
    });
    append_artifacts(output_dir, &record);
    eprintln!("{record}");
    record
}

fn git_revision() -> String {
    git2::Repository::discover(env!("CARGO_MANIFEST_DIR"))
        .unwrap()
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string()
}

fn rust_toolchain() -> String {
    let output = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .expect("the toolchain used by Cargo is available");
    assert!(output.status.success(), "rustc --version failed");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
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
        "## {}\n\n- Git revision: `{}`\n- Rust toolchain: `{}`\n- Dispatch workers: {}\n- Crate version: `{}`\n- Gate: {}\n\n| Method | Phase | Count | Sum ms | p50 ms | p95 ms | Max ms |\n| --- | --- | ---: | ---: | ---: | ---: | ---: |\n",
        record["scenario"].as_str().unwrap(),
        record["git_revision"].as_str().unwrap(),
        record["rust_toolchain"].as_str().unwrap(),
        record["dispatch_workers"],
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
fn baseline_emitter_writes_self_describing_artifacts() {
    let dir = tempfile::tempdir().unwrap();
    let metric = json!({ "count": 8, "sum_ms": 12.0, "p50_ms": 2.0,
        "p95_ms": 5.0, "max_ms": 6.0 });
    let methods = json!({ "tasks.list": { "served": 8, "queued": metric,
        "lock_wait": metric, "held": metric } });
    let record = emit_baseline(dir.path(), "isolated", "none", 2, &methods);
    let revision = git2::Repository::discover(env!("CARGO_MANIFEST_DIR"))
        .unwrap()
        .head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string();
    let toolchain = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .unwrap();
    assert!(toolchain.status.success());
    let toolchain = String::from_utf8(toolchain.stdout).unwrap();
    let toolchain = toolchain.trim();
    assert_eq!(record["git_revision"], revision);
    assert_eq!(record["rust_toolchain"], toolchain);
    assert_eq!(record["dispatch_workers"], 2);

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
    assert!(markdown.contains(toolchain));
    assert!(markdown.contains("Dispatch workers: 2"));
    assert!(markdown.contains("| tasks.list | queued | 8 | 12.0 | 2.0 | 5.0 | 6.0 |"));
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
    next_id: u64,
    task_started: mpsc::Receiver<()>,
    dispatch_workers: usize,
}

impl Sample {
    fn new(workers: usize, git_gate: Option<GitGate>) -> Self {
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
        let repo = app.project_at(0).repo_path.clone();
        let clock = FrameClock::new();
        app.frame_clock = Arc::clone(&clock);
        let state = app.shared();
        let peers = state.lock().unwrap().peers_slot();
        let dispatched = Arc::clone(&state);
        let (task_start, task_started) = mpsc::channel();
        let handler = FrameHandler::new(Arc::clone(&clock), move |sender, frame, timer| {
            if frame.payload["method"].as_str() == Some("git.status") {
                git_gate
                    .as_ref()
                    .expect("only the Git sample sends git.status")
                    .run(&repo);
            }
            if matches!(
                frame.payload["method"].as_str(),
                Some("tasks.list" | "tasks.get")
            ) {
                let _ = task_start.send(());
            }
            crate::app::dispatch_frame(&dispatched, &peers, sender, frame, timer)
        });
        let transport = crate::transport::generate_transport_keypair();
        let intake = FrameIntake::with_pool(handler, transport.clone(), 32, workers);
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
            next_id: 1,
            task_started,
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

    async fn list(&mut self, i: usize) {
        self.send(
            "tasks.list",
            json!({ "project_id": self.project_id, "limit": i + 1 }),
        )
        .await;
    }

    async fn get(&mut self, i: usize) {
        self.send("tasks.get", json!({ "task_id": self.task_ids[i] }))
            .await;
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

    fn baseline(&self, scenario: &str, gate: &str) -> Value {
        let stats = self.clock.stats();
        let methods = &stats["methods"];
        for method in ["tasks.list", "tasks.get"] {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn isolated_task_reads_publish_a_file_backed_baseline() {
    let mut sample = Sample::new(2, None);
    for i in 0..SAMPLE_COUNT {
        sample.list(i).await;
        sample.answers(1).await;
        sample.get(i).await;
        sample.answers(1).await;
    }
    sample.baseline("isolated", "none");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_reads_measure_wait_behind_an_app_held_write() {
    let mut sample = Sample::new(2, None);
    let (held, acquired) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let state = Arc::clone(&sample.state);
    let clock = Arc::clone(&sample.clock);
    let holder = std::thread::spawn(move || {
        let timer = clock.frame("tasks.update");
        let _guard = timer.lock(&state);
        held.send(()).unwrap();
        released
            .recv_timeout(PATIENCE)
            .expect("the test releases the app mutex");
    });
    acquired
        .recv_timeout(PATIENCE)
        .expect("the synthetic write holds the app mutex");
    for i in 0..SAMPLE_COUNT {
        sample.list(i).await;
        sample.get(i).await;
    }
    for _ in 0..2 {
        sample
            .task_started
            .recv_timeout(PATIENCE)
            .expect("both workers began task reads while the write held the app mutex");
    }
    tokio::time::sleep(GATE).await;
    release.send(()).unwrap();
    holder.join().unwrap();
    sample.answers(2 * SAMPLE_COUNT).await;
    sample.baseline("app_held_write", "app mutex held for at least 75ms");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn task_reads_measure_queue_behind_off_lock_git_worker_work() {
    let (git_gate, entered, release) = GitGate::new();
    let mut sample = Sample::new(1, Some(git_gate));
    sample
        .send("git.status", json!({ "project_id": sample.project_id }))
        .await;
    entered
        .recv_timeout(PATIENCE)
        .expect("the Git worker reached its gate");
    for i in 0..SAMPLE_COUNT {
        sample.list(i).await;
        sample.get(i).await;
    }
    tokio::time::sleep(GATE).await;
    release.send(()).unwrap();
    sample.answers(2 * SAMPLE_COUNT + 1).await;
    sample.baseline("off_lock_git_worker", "Git worker held for at least 75ms");
}
