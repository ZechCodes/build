//! Native RPC samples include admission and deferred apply under the same frame.
//! Untimed merge progress locks and the background sync callback are outside
//! FrameClock's existing hold instrumentation; no synthetic timings replace them.
use super::*;

struct Fixture {
    _home: tempfile::TempDir,
    _repository: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    handler: FrameHandler,
    workspace: String,
}

impl Fixture {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let (repository, mut app, project) = tracked(home.path());
        let workspace = workspace(&mut app, &project, "observation holds");
        app.frame_clock = FrameClock::reporting_to(Arc::new(|_| {}));
        app.initialize_snapshot_projection();
        let state = Arc::new(Mutex::new(app));
        state.lock().unwrap().self_handle = Some(Arc::downgrade(&state));
        let handler = AppState::handler(Arc::clone(&state));
        Self {
            _home: home,
            _repository: repository,
            state,
            handler,
            workspace,
        }
    }

    fn call(&self, method: &str, params: Value) -> Value {
        let answer = self.handler.call(
            SessionSender::detached("review-observation-holds"),
            req(method, params),
        );
        assert_eq!(answer["ok"], true, "{answer}");
        answer["result"].clone()
    }

    fn measured(&self, method: &str, params: Value) -> Value {
        let samples = self.state.lock().unwrap().frame_clock.capture_phases();
        let result = self.call(method, params);
        assert_eq!(samples.active_frames(), 0, "RPC completion drops its frame");
        let phases = samples.snapshot();
        let measured = &phases[method];
        assert_eq!(measured["frame.held"]["count"], 1, "{phases}");
        let minimum_entries = if method == "tasks.review.close" { 1 } else { 2 };
        assert!(
            measured["entered.lock"]["count"].as_u64().unwrap() >= minimum_entries,
            "admission and deferred apply belong to the completed frame: {phases}"
        );
        println!(
            "TASK732_RPC_HOLD {}",
            json!({"method":method,"phases":measured})
        );
        result
    }

    fn assert_summary(&self, result: &Value, status: &str) {
        let detail = self.call("workspace.get", json!({"workspace_id":self.workspace}));
        let summary = &detail["active_review"];
        assert_eq!(summary["task_id"], result["review"]["task_id"], "{detail}");
        assert_eq!(summary["version"], result["review"]["version"], "{detail}");
        assert_eq!(summary["status"], status, "{detail}");
        assert_eq!(
            summary["latest_published_snapshot_id"],
            result["review"]["snapshots"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["id"],
            "{detail}"
        );
    }

    fn versioned(&self, method: &str, review: &Value) -> Value {
        self.measured(
            method,
            json!({
                "task_id":review["review"]["task_id"],
                "expected_version":review["review"]["version"],
            }),
        )
    }
}

fn commit(working: &Path, name: &str) -> String {
    std::fs::write(working.join(name), "review observation fixture\n").unwrap();
    git_in(working, &["add", name]);
    git_in(working, &["commit", "-m", name]);
    git2::Repository::open(working)
        .unwrap()
        .head()
        .unwrap()
        .target()
        .unwrap()
        .to_string()
}

#[test]
fn native_review_observation_rpc_hold_samples() {
    let fixture = Fixture::new();
    let opened = fixture.call(
        "tasks.review.open",
        json!({
            "workspace_id":fixture.workspace,"request_id":"observation-holds",
            "title":"Observe each lifecycle producer","description":"Native RPC fixture",
        }),
    );
    let binding = &opened["review"]["bindings"][0];
    let working = Path::new(binding["working_repository"].as_str().unwrap());
    let source = Path::new(binding["source_repository"].as_str().unwrap());
    git_in(source, &["branch", "release"]);
    let head = commit(working, "pushed.txt");
    let pushed = fixture.measured(
        "tasks.review.push",
        json!({
            "task_id":opened["task"]["id"],"expected_version":opened["review"]["version"],
            "sources":[{"directory_id":binding["directory_id"],"expected_head":head,
                "expected_received_head":binding["last_received_head"]}],
        }),
    );
    assert_eq!(pushed["sources"][0]["status"], "published", "{pushed}");
    fixture.assert_summary(&pushed, "open");

    let refreshed_head = commit(working, "refreshed.txt");
    git_in(working, &["push"]);
    let refreshed = fixture.versioned("tasks.review.refresh", &pushed);
    assert_eq!(
        refreshed["review"]["snapshots"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["directories"][0]["head"],
        refreshed_head
    );
    fixture.assert_summary(&refreshed, "open");

    let closed = fixture.measured(
        "tasks.review.close",
        json!({
            "task_id":opened["task"]["id"],"expected_version":refreshed["review"]["version"],
            "description":"Defer this review",
        }),
    );
    fixture.assert_summary(&closed, "closed");
    let reopened = fixture.versioned("tasks.review.reopen", &closed);
    fixture.assert_summary(&reopened, "open");

    let updated = fixture.measured(
        "tasks.review.update",
        json!({
            "task_id":opened["task"]["id"],"expected_version":reopened["review"]["version"],
            "bases":[{"directory_id":binding["directory_id"],"branch":"release"}],
        }),
    );
    assert_eq!(
        updated["review"]["bindings"][0]["base_branch_ref"],
        "refs/heads/release"
    );
    fixture.assert_summary(&updated, "open");

    let snapshot_head = commit(working, "snapshot.txt");
    git_in(working, &["push"]);
    let snapshot = fixture.measured(
        "tasks.review.snapshot",
        json!({
            "task_id":opened["task"]["id"],"workspace_id":fixture.workspace,
            "expected_version":updated["review"]["version"],
        }),
    );
    assert_eq!(
        snapshot["review"]["snapshots"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["directories"][0]["head"],
        snapshot_head
    );
    fixture.assert_summary(&snapshot, "open");

    let mut merge = snapshot.clone();
    merge["task"] = opened["task"].clone();
    let merged = fixture.measured(
        "tasks.review.merge",
        super::wire_actions::merge_params(&merge),
    );
    fixture.assert_summary(&merged, "merged");
    let repository = git2::Repository::open(source).unwrap();
    let target = repository.refname_to_id("refs/heads/release").unwrap();
    assert!(repository
        .graph_descendant_of(target, git2::Oid::from_str(&snapshot_head).unwrap())
        .unwrap());
}
