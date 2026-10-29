//! Serial production-profile task-read gate through encrypted carrier admission.
#![cfg(feature = "testing")]
mod project_bridge;
use build_bridge::app::AppState;
use build_bridge::carrier::testing::{client_request, session_init, ChannelWire};
use build_bridge::carrier::{FrameIntake, SessionSender};
use build_bridge::harness::HarnessContext;
use build_bridge::tracker::{Actor, Assignee, Task};
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
        task.body = "excluded body".repeat(1000);
        task.assignee = Some(Assignee::Agent {
            agent_id: format!("agent-{}", (number - 1) % 100),
        });
        assert_eq!(
            store.create_tracker_task(task, &[]).unwrap().number,
            number as u64
        );
    }
}
struct Fixture {
    root: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    store: build_bridge::store::Store,
    intake: Arc<FrameIntake>,
    wire: ChannelWire,
    key: String,
    project: String,
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
        assert_eq!(
            store.task_counts(repo.to_str().unwrap(), &[]).unwrap()[0].unfinished_open_count,
            count as u64
        );
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
}

fn validate(count: usize, method: &str, response: &Value) {
    assert_eq!(response["ok"], true, "{response}");
    let result = &response["result"];
    assert!(serde_json::to_vec(result).unwrap().len() <= 65_536);
    assert!(result["basis"].is_string());
    if method == "tasks.counts" {
        assert_eq!(result["unfinished_open_count"], count);
        assert_eq!(result["agents"].as_array().unwrap().len(), 100);
        let actual: std::collections::BTreeSet<_> = result["agents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|bucket| bucket["agent_id"].as_str().unwrap().to_string())
            .collect();
        let expected = (0..100).map(|n| format!("agent-{n}")).collect();
        assert_eq!(actual, expected);
        for bucket in result["agents"].as_array().unwrap() {
            let index: usize = bucket["agent_id"]
                .as_str()
                .unwrap()
                .strip_prefix("agent-")
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(bucket["unfinished_open_count"], (count + 99 - index) / 100);
        }
    } else {
        assert_eq!(result["task"]["number"], count);
        assert_eq!(result["task"]["title"], format!("Task {count}"));
        assert!(result["task"].get("body").is_none());
        let assignee = format!("agent-{}", (count - 1) % 100);
        assert_eq!(
            result["task"]["assignee"],
            json!({"kind":"agent","agent_id":assignee})
        );
        assert!(result["identities"][&assignee].is_object());
        assert_eq!(result["identities"].as_object().unwrap().len(), 1);
    }
}

async fn run(timing: bool) {
    assert!(
        !timing || !cfg!(debug_assertions),
        "timing gate requires --release"
    );
    let tools = tempfile::tempdir().unwrap();
    std::env::set_var("PATH", project_bridge::private_path(tools.path()));
    let agents: Vec<_> = (0..100).map(|n| format!("agent-{n}")).collect();
    let mut failures = Vec::new();
    for count in [1, 100, 5000] {
        let mut fixture = Fixture::new(count);
        let (release, holder) = fixture.hold_locks();
        for (method, params) in [
            ("tasks.counts", json!({"agent_ids":agents})),
            ("tasks.lookup", json!({"number":count})),
        ] {
            let mut times = Vec::new();
            let mut bytes = 0;
            for sample in 0..SAMPLES {
                let start = Instant::now();
                let response = tokio::time::timeout(
                    Duration::from_secs(5),
                    fixture.read(sample, method, params.clone()),
                )
                .await
                .unwrap();
                times.push(start.elapsed());
                validate(count, method, &response);
                bytes = bytes.max(serde_json::to_vec(&response["result"]).unwrap().len());
            }
            times.sort();
            let max = *times.last().unwrap();
            println!("query_carrier profile={} tasks={count} method={method} samples={SAMPLES} bytes={bytes} p99_us={} max_us={}",if cfg!(debug_assertions){"debug"}else{"release"},times[98].as_micros(),max.as_micros());
            if timing && max > CEILING {
                failures.push(format!("{method} tasks={count} max={max:?}"));
            }
        }
        release.send(()).unwrap();
        holder.join().unwrap();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aggregate_lookup_carrier_contents_and_bytes_with_both_locks_held() {
    run(false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "timing: isolated release profile only"]
async fn aggregate_lookup_carrier_release_latency_gate() {
    run(true).await;
}

fn memory() -> String {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .filter(|line| line.starts_with("VmRSS:") || line.starts_with("VmHWM:"))
        .collect::<Vec<_>>()
        .join(" ")
}
fn storage(root: &Path) -> u64 {
    ["build.db", "build.db-wal", "build.db-shm"]
        .iter()
        .filter_map(|name| std::fs::metadata(root.join(name)).ok())
        .map(|metadata| metadata.len())
        .sum()
}
fn thread_cpu_us() -> u64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // A valid stack pointer; this reads only the current thread's CPU clock.
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) },
        0
    );
    time.tv_sec as u64 * 1_000_000 + time.tv_nsec as u64 / 1_000
}
fn observe<T>(label: &str, action: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let cpu = thread_cpu_us();
    let value = action();
    println!(
        "query_cost operation={label} wall_us={} thread_cpu_us={} memory={}",
        start.elapsed().as_micros(),
        thread_cpu_us() - cpu,
        memory()
    );
    value
}

#[test]
fn query_projection_backfill_reopen_and_write_costs() {
    use build_bridge::store::Store;
    use build_bridge::tracker::TaskComment;
    let root = tempfile::tempdir().unwrap();
    let store = observe("empty_startup", || Store::new(root.path()).unwrap());
    observe("seed_100_tasks", || seed(&store, "/query-cost", 100));
    let mut task = store
        .load_tracker_task(
            store
                .task_summary_by_number("/query-cost", 1)
                .unwrap()
                .unwrap()
                .row["id"]
                .as_str()
                .unwrap(),
        )
        .unwrap()
        .unwrap();
    task.watched = true;
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    drop(store);
    let connection = Connection::open(root.path().join("build.db")).unwrap();
    // A changed installed definition deliberately exercises supported repair,
    // not an approximation of the base schema or a read-triggered rebuild.
    connection.execute("UPDATE meta SET value='performance-fixture-old-definition' WHERE key IN ('task_summary_v1_definition','task_summary_v1_ready')", []).unwrap();
    drop(connection);
    let store = observe("forced_definition_backfill_100_tasks", || {
        Store::new(root.path()).unwrap()
    });
    assert_eq!(
        store.task_counts("/query-cost", &[]).unwrap()[0].unfinished_open_count,
        100
    );
    println!(
        "query_cost phase=after_backfill storage_bytes={}",
        storage(root.path())
    );
    drop(store);
    for cycle in 0..3 {
        let store = observe("completed_reopen", || Store::new(root.path()).unwrap());
        for sample in 0..3 {
            println!(
                "query_cost cycle={cycle} sample={sample} first_after_reopen={}",
                sample == 0
            );
            task.title = format!("changed {cycle}/{sample}");
            observe("ordinary_metadata_write", || {
                store.save_tracker_task_activity(&task, &[], &[]).unwrap()
            });
            let id = format!("tc-{:026}", cycle * 3 + sample);
            let note: TaskComment = serde_json::from_value(json!({"id":id,"task_id":task.id,"author":{"kind":"agent","agent_id":"agent-cost"},"body":"new activity","created_at":"2026-10-01T01:00:00Z"})).unwrap();
            observe("history_append", || {
                store
                    .save_tracker_task_activity(&task, &[note], &[])
                    .unwrap()
            });
            task.read_through = Some(id);
            observe("read_mark_write", || {
                store.save_tracker_task_activity(&task, &[], &[]).unwrap()
            });
            task.links.workspace_ids = vec![format!("workspace-{cycle}-{sample}")];
            task.trackers = vec![format!("agent-{cycle}-{sample}")];
            observe("relation_write", || {
                store.save_tracker_task_activity(&task, &[], &[]).unwrap()
            });
            let loaded = store.load_tracker_task(&task.id).unwrap().unwrap();
            assert_eq!(loaded.title, task.title);
            assert_eq!(loaded.read_through, task.read_through);
            assert_eq!(loaded.links.workspace_ids, task.links.workspace_ids);
            assert_eq!(loaded.trackers, task.trackers);
        }
        println!(
            "query_cost cycle={cycle} storage_bytes={}",
            storage(root.path())
        );
    }
}
