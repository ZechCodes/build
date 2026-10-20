//! File-backed task reads through the real carrier intake and worker pool.
//! Each sample uses a bounded gate to make one source of delay observable.

use super::*;
use crate::app::tests::tracker::{filed, tracked};
use crate::carrier::testing::{client_request, session_init, within_patience, ChannelWire};
use crate::carrier::{FrameHandler, FrameIntake, OutboundEnvelope, SessionSender};
use crate::timing::FrameClock;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const SESSION: &str = "timing-sample";
const GATE: Duration = Duration::from_millis(75);
const PATIENCE: Duration = Duration::from_secs(5);
const SAMPLE_COUNT: usize = 8;

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
        let sample = json!({ "kind": "bridge_concurrency_step1_baseline", "scenario": scenario,
            "gate": gate, "crate_version": env!("CARGO_PKG_VERSION"), "methods": methods });
        eprintln!("{sample}");
        sample
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
