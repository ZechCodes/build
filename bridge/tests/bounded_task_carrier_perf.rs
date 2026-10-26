//! Serial production-profile task-read gate through encrypted carrier admission.
#![cfg(feature = "testing")]
use build_bridge::app::AppState;
use build_bridge::carrier::testing::{client_request, session_init, ChannelWire};
use build_bridge::carrier::{FrameIntake, SessionSender};
use build_bridge::harness::HarnessContext;
use build_bridge::tracker::{Actor, Task};
use build_bridge::transport;
use rusqlite::{params, Connection};
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
fn seed(root: &Path, project: &str, count: usize) {
    let mut connection = Connection::open(root.join("store/build.db")).unwrap();
    let tx = connection.transaction().unwrap();
    let mut insert=tx.prepare("INSERT INTO tracker_tasks(id,project_key,number,state,status,created_at,updated_at,record) VALUES(?1,?2,?3,'open','backlog',?4,?4,?5)").unwrap();
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
        insert
            .execute(params![
                task.id,
                project,
                number as i64,
                task.created_at,
                serde_json::to_string(&task).unwrap()
            ])
            .unwrap();
    }
    drop(insert);
    tx.commit().unwrap();
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
        let started = Instant::now();
        seed(root.path(), repo.to_str().unwrap(), count);
        println!(
            "carrier_setup tasks={count} seed_us={}",
            started.elapsed().as_micros()
        );
        let store = app.attached_store_for_test().unwrap();
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
    async fn samples(
        &mut self,
        scenario: usize,
        method: &str,
        params: Value,
    ) -> Result<(Vec<Duration>, usize, usize), String> {
        let mut times = Vec::new();
        let mut bytes = 0;
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
            bytes = serde_json::to_vec(&response["result"]).unwrap().len();
            rows = response["result"]["tasks"].as_array().map_or(0, Vec::len);
        }
        Ok((times, bytes, rows))
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bounded_reads_carrier_max_and_p99_with_both_locks_held() {
    for count in [1, 100, 5000] {
        let mut fixture = Fixture::new(count);
        let (release, holder) = fixture.hold_locks();
        let mut results = Vec::new();
        for (scenario, (method, params)) in [
            ("tasks.list", json!({"summary":true})),
            ("tasks.list", json!({"summary":true,"limit":500})),
            ("tasks.list", json!({"summary":true,"labels":["absent"]})),
            ("tasks.filterChoices", json!({"dimension":"label"})),
        ]
        .into_iter()
        .enumerate()
        {
            results.push((
                scenario,
                method,
                fixture.samples(scenario, method, params).await,
            ));
        }
        release.send(()).unwrap();
        holder.join().unwrap();
        for (scenario, method, result) in results {
            let (mut times, bytes, rows) = result.unwrap();
            times.sort();
            let p99 = times[(times.len() * 99).div_ceil(100) - 1];
            let max = *times.last().unwrap();
            println!("carrier_read profile={} tasks={count} scenario={scenario} method={method} samples={} rows={rows} bytes={bytes} p99_us={} max_us={}",if cfg!(debug_assertions){"debug"}else{"release"},times.len(),p99.as_micros(),max.as_micros());
            assert!(bytes <= 1_048_576);
            assert!(
                max <= CEILING,
                "{method} tasks={count} scenario={scenario}: max={max:?}, p99={p99:?}"
            );
        }
    }
}
