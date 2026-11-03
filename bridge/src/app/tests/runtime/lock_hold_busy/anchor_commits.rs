//! A deterministic gate at the actual Git graph check, independent of the
//! deferred-work gate, catches accidental validation under the app mutex.
use super::*;
use crate::app::tests::project_agent::project_agent;
use crate::app::tests::workflow::lifecycle_offlock::frame_on_a_thread;
use crate::reviews::model::ReviewSnapshot;

const TOKEN: &str = "anchor-commit-off-lock-fixture";

struct Fixture {
    _root: tempfile::TempDir,
    _repo: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    project: String,
    agent: String,
    task: String,
    anchor: Value,
    git_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let (repo_home, mut app, project) = tracked(root.path());
        let repo_path = app.project_at(0).repo_path.clone();
        let repo = git2::Repository::open(&repo_path).unwrap();
        let base = repo.head().unwrap().target().unwrap();
        std::fs::write(repo_path.join("anchor.txt"), "saved line\n").unwrap();
        git_in(&repo_path, &["add", "anchor.txt"]);
        git_in(&repo_path, &["commit", "-m", "anchored change"]);
        let head = repo.head().unwrap().target().unwrap();
        let git_dir = repo.commondir().to_path_buf();
        let (_, agent) = project_agent(&mut app, &project);
        let task = filed(&mut app, &project, "off-lock anchor validation")["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let snapshot: ReviewSnapshot = serde_json::from_value(json!({
            "id":"anchor-snapshot", "number":0, "created_at":crate::store::now_rfc3339(),
            "author":{"kind":"user"}, "directories":[{
                "id":"anchor-directory", "source_id":"source", "name":"repo", "path":repo_path,
                "source_path":repo_path, "is_git":true, "status":"git", "common_git_dir":git_dir,
                "base":{"kind":"configured", "name":"main", "oid":base.to_string()}, "head":head.to_string()
            }]
        })).unwrap();
        app.tracker_store()
            .unwrap()
            .save_review_snapshot(&task, "anchor-workspace", 0, snapshot)
            .unwrap();
        let unwatched = app.handle(req("tasks.unwatch", json!({"task_id":task})));
        assert_eq!(unwatched["ok"], true, "{unwatched}");
        app.delivery_queue.clear_queued();
        app.test_install_mcp_token(agent.clone(), TOKEN.into());
        // Initialize the real writer/read projections without background
        // maintenance competing for the mutex being observed.
        app.initialize_read_context();
        app.initialize_snapshot_projection();
        let state = Arc::new(Mutex::new(app));
        state.lock().unwrap().self_handle = Some(Arc::downgrade(&state));
        Self {
            _root: root,
            _repo: repo_home,
            state,
            project,
            agent,
            task,
            anchor: json!({"snapshot_id":"anchor-snapshot", "directory_id":"anchor-directory",
                "path":"anchor.txt", "side":"new", "line":1, "commit":head.to_string()}),
            git_dir,
        }
    }

    fn hold_validation(&self) -> crate::test_support::off_lock::OffLockGateHandle {
        let (gate, held) = OffLockGate::new();
        crate::reviews::read::install_commit_validation_gate(self.git_dir.clone(), gate);
        held
    }

    fn unrelated_write(&self) -> Value {
        frame_on_a_thread(
            &self.state,
            "anchor-unrelated",
            "tasks.create",
            json!({"project_id":self.project,"title":"unrelated write during graph validation"}),
        )
        .recv_timeout(Duration::from_secs(5))
        .expect("an unrelated writer completes while the commit graph check is held")
    }
}

#[test]
fn rpc_anchor_commit_validation_leaves_app_mutex_free() {
    let fixture = Fixture::new();
    let held = fixture.hold_validation();
    let response = frame_on_a_thread(
        &fixture.state,
        "anchor-rpc",
        "tasks.comment",
        json!({"task_id":fixture.task,"body":"saved commit line","anchor":fixture.anchor}),
    );
    held.wait_for_arrival();
    let unlocked = fixture.state.try_lock().is_ok();
    let unrelated = unlocked.then(|| fixture.unrelated_write());
    let edited = unlocked.then(|| {
        frame_on_a_thread(
            &fixture.state,
            "anchor-concurrent-edit",
            "tasks.update",
            json!({"task_id":fixture.task,"body":"concurrent task body",
                "title":"edited while validation waits"}),
        )
        .recv_timeout(Duration::from_secs(5))
        .expect("editing the anchored task completes while Git validation is held")
    });
    held.release();
    let response = response.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(response["ok"], true, "{response}");
    assert!(
        unlocked,
        "tasks.comment held the app mutex during commit graph validation"
    );
    assert_eq!(unrelated.unwrap()["ok"], true);
    assert_eq!(edited.unwrap()["ok"], true);
    assert_eq!(response["result"]["task"]["body"], "concurrent task body");
    assert_eq!(
        response["result"]["task"]["title"],
        "edited while validation waits"
    );
    let app = fixture.state.lock().unwrap();
    let stored = app
        .tracker_store()
        .unwrap()
        .load_tracker_task(&fixture.task)
        .unwrap()
        .unwrap();
    assert_eq!(stored.body, "concurrent task body");
    assert_eq!(stored.title, "edited while validation waits");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcp_anchor_commit_validation_leaves_app_mutex_free() {
    let fixture = Fixture::new();
    let held = fixture.hold_validation();
    let plane = ControlPlane::new(&fixture.state.lock().unwrap());
    let (client, daemon) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(handle_done_stream(
        Arc::clone(&fixture.state),
        daemon,
        plane,
    ));
    let mut client = tokio::io::BufReader::new(client);
    let action = crate::mcp::DoneServer::for_owner(&fixture.agent)
        .handle_message(
            &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"comment_task",
            "arguments":{"task_id":fixture.task,"body":"saved commit line","anchor":fixture.anchor,
                "track":true,"mention_user":true}}})
            .to_string(),
        )
        .action
        .unwrap();
    let frame = json!({"task_id":fixture.agent,"session_token":TOKEN,"request":action});
    client
        .get_mut()
        .write_all(format!("{frame}\n").as_bytes())
        .await
        .unwrap();
    held.wait_for_arrival();
    let unlocked = fixture.state.try_lock().is_ok();
    let unrelated = unlocked.then(|| fixture.unrelated_write());
    held.release();
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(30), client.read_line(&mut response))
        .await
        .unwrap()
        .unwrap();
    drop(client);
    server.await.unwrap();
    let response: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["ok"], true, "{response}");
    assert!(
        unlocked,
        "comment_task held the app mutex during commit graph validation"
    );
    assert_eq!(unrelated.unwrap()["ok"], true);
    assert_eq!(response["result"]["comment"]["mentions_user"], true);
    assert_eq!(response["result"]["task"]["watched"], true);
    let app = fixture.state.lock().unwrap();
    let stored = app
        .tracker_store()
        .unwrap()
        .load_tracker_task(&fixture.task)
        .unwrap()
        .unwrap();
    assert!(stored.is_tracked_by(&fixture.agent));
    assert!(stored.watched);
}

#[test]
fn anchor_commit_opinion_preserves_original_review_version_across_git_validation() {
    let fixture = Fixture::new();
    let held = fixture.hold_validation();
    let response = frame_on_a_thread(
        &fixture.state,
        "anchor-opinion-race",
        "tasks.comment",
        json!({"task_id":fixture.task,"body":"opinion captured before publication",
            "anchor":fixture.anchor,"opinion":{"snapshot_id":"anchor-snapshot","verdict":"approve"}}),
    );
    held.wait_for_arrival();
    let advanced = match fixture.state.try_lock() {
        Ok(app) => Some((|| -> Result<u64, String> {
            let store = app.tracker_store()?;
            let review = store
                .load_review(&fixture.task)
                .map_err(|error| error.to_string())?
                .ok_or("review must exist")?;
            let mut snapshot = review.snapshots[0].clone();
            snapshot.id = "anchor-snapshot-published-during-validation".into();
            let (saved, _) = store
                .save_review_snapshot(&fixture.task, "anchor-workspace", 1, snapshot)
                .map_err(|error| error.to_string())?;
            Ok(saved.version)
        })()),
        Err(_) => None,
    };
    // Release before any assertion so a regressed locked writer still finishes.
    held.release();
    let response = response.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(
        advanced
            .expect("opinion validation held the app mutex")
            .unwrap(),
        2
    );
    assert_eq!(response["ok"], false, "{response}");
    assert_eq!(response["error_code"], "stale_version", "{response}");
    let app = fixture.state.lock().unwrap();
    let store = app.tracker_store().unwrap();
    let timeline = store.load_tracker_timeline(&fixture.task).unwrap();
    assert!(
        !timeline
            .iter()
            .any(|entry| matches!(entry, crate::tracker::TimelineEntry::Comment(_))),
        "a refused opinion must not publish a comment"
    );
    let review = store.load_review(&fixture.task).unwrap().unwrap();
    assert_eq!(review.version, 2);
    assert_eq!(review.snapshots.len(), 2);
    assert!(
        !store
            .load_tracker_task(&fixture.task)
            .unwrap()
            .unwrap()
            .watched,
        "a refused comment must not start watching the task"
    );
}
