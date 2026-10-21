//! Every lifecycle producer loads SQL before acquiring its publication lock.
use super::*;
use crate::app::runtime::deferred::DeferredJob;

fn fixture() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    Arc<Mutex<AppState>>,
    Value,
) {
    let home = tempfile::tempdir().unwrap();
    let (repository, mut app, project) = tracked(home.path());
    let workspace = workspace(&mut app, &project, "lifecycle observations");
    let opened = wire_actions::open(&mut app, &workspace);
    let shared = app.shared();
    (home, repository, shared, opened)
}

fn params(method: &str, opened: &Value) -> Value {
    let binding = &opened["review"]["bindings"][0];
    let mut value =
        json!({"task_id":opened["review"]["task_id"],"expected_version":opened["review"]["version"]});
    match method {
        "tasks.review.snapshot" => value["workspace_id"] = opened["review"]["workspace_id"].clone(),
        "tasks.review.close" => value["description"] = json!("Closed after review"),
        "tasks.review.push" => {
            value["sources"] = json!([{"directory_id":binding["directory_id"],"expected_head":binding["last_received_head"],"expected_received_head":binding["last_received_head"]}])
        }
        "tasks.review.update" => {
            value["bases"] = json!([{"directory_id":binding["directory_id"],"branch":"main"}])
        }
        "tasks.review.merge" => return wire_actions::merge_params(opened),
        _ => (),
    }
    value
}

fn defer(shared: &Arc<Mutex<AppState>>, method: &str, params: &Value) -> Option<DeferredJob> {
    let (answer, job) = shared.lock().unwrap().dispatch_deferring(method, params);
    answer.unwrap();
    job
}

fn gates(shared: &Arc<Mutex<AppState>>) -> (OffLockGateHandle, OffLockGateHandle) {
    let (before, waiting) = OffLockGate::new();
    let (after, loaded) = OffLockGate::new();
    let mut app = shared.lock().unwrap();
    app.off_lock_review_observation_gate = Some(before);
    app.off_lock_review_observation_loaded = Some(after);
    (waiting, loaded)
}

fn finish_while_held(
    shared: &Arc<Mutex<AppState>>,
    waiting: OffLockGateHandle,
    loaded: OffLockGateHandle,
) {
    waiting.wait_for_arrival();
    let held = shared.try_lock();
    if held.is_err() {
        waiting.release();
        loaded.wait_for_arrival();
        loaded.release();
        panic!("observation SQL began while its producer held the app mutex");
    }
    let store = held.as_ref().unwrap().store.clone().unwrap();
    let writer = store.writer_for_test();
    waiting.release();
    drop(writer);
    loaded.wait_for_arrival();
    loaded.release();
    drop(held);
}

fn deferred_observation(method: &str) {
    let (_home, _repository, shared, mut opened) = fixture();
    if method == "tasks.review.reopen" {
        let closed = params("tasks.review.close", &opened);
        let mut app = shared.lock().unwrap();
        let closed = review_call(&mut app, "tasks.review.close", closed);
        opened["review"] = closed["review"].clone();
    }
    let request = params(method, &opened);
    if method == "tasks.review.merge" {
        shared.lock().unwrap().self_handle = None;
    }
    let (waiting, loaded) = gates(&shared);
    let state = shared.clone();
    let request_copy = request.clone();
    let method = method.to_owned();
    let worker = std::thread::spawn(move || {
        let job = defer(&state, &method, &request_copy);
        if let Some(job) = job {
            let done = job.run();
            state
                .lock()
                .unwrap()
                .apply_deferred(&method, &request_copy, done)
                .unwrap()
        } else {
            Value::Null
        }
    });
    finish_while_held(&shared, waiting, loaded);
    let answer = worker.join().unwrap();
    let app = shared.lock().unwrap();
    let workspace = opened["review"]["workspace_id"].as_str().unwrap();
    let view = app.read_view.load();
    assert_eq!(
        view.get("workspace.get", workspace).unwrap()["active_review"]["version"],
        answer["review"]["version"]
    );
}

#[test]
fn refresh_observation_sql_finishes_while_app_mutex_is_held() {
    deferred_observation("tasks.review.refresh");
}
#[test]
fn reopen_observation_sql_finishes_while_app_mutex_is_held() {
    deferred_observation("tasks.review.reopen");
}
#[test]
fn pr_snapshot_observation_sql_finishes_while_app_mutex_is_held() {
    deferred_observation("tasks.review.snapshot");
}
#[test]
fn close_observation_sql_finishes_while_app_mutex_is_held() {
    deferred_observation("tasks.review.close");
}
#[test]
fn push_observation_sql_finishes_while_app_mutex_is_held() {
    deferred_observation("tasks.review.push");
}
#[test]
fn update_observation_sql_finishes_while_app_mutex_is_held() {
    deferred_observation("tasks.review.update");
}
#[test]
fn merge_final_observation_sql_finishes_while_app_mutex_is_held() {
    deferred_observation("tasks.review.merge");
}

#[test]
fn merge_progress_observation_sql_finishes_while_app_mutex_is_held() {
    let (_home, _repository, shared, opened) = fixture();
    let request = params("tasks.review.merge", &opened);
    let (waiting, loaded) = gates(&shared);
    let job = defer(&shared, "tasks.review.merge", &request).unwrap();
    let worker = std::thread::spawn(move || job.run());
    finish_while_held(&shared, waiting, loaded);
    shared.lock().unwrap().off_lock_review_observation_gate = None;
    shared.lock().unwrap().off_lock_review_observation_loaded = None;
    let done = worker.join().unwrap();
    shared
        .lock()
        .unwrap()
        .apply_deferred("tasks.review.merge", &request, done)
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_observation_sql_finishes_while_app_mutex_is_held() {
    let (_home, _repository, shared, opened) = fixture();
    let working = std::path::Path::new(
        opened["review"]["bindings"][0]["working_repository"]
            .as_str()
            .unwrap(),
    );
    std::fs::write(working.join("sync-change.txt"), "new revision\n").unwrap();
    git_in(working, &["add", "sync-change.txt"]);
    git_in(working, &["commit", "-m", "background observation"]);
    git_in(working, &["push"]);
    let (waiting, loaded) = gates(&shared);
    let service = AppState::spawn_review_sync(shared.clone()).await.unwrap();
    finish_while_held(&shared, waiting, loaded);
    shared.lock().unwrap().off_lock_review_observation_gate = None;
    shared.lock().unwrap().off_lock_review_observation_loaded = None;
    drop(service);
}

#[test]
fn delayed_lifecycle_observations_preserve_newer_close_and_replacement() {
    for method in [
        "tasks.review.refresh",
        "tasks.review.snapshot",
        "tasks.review.push",
        "tasks.review.update",
        "tasks.review.close",
        "tasks.review.reopen",
        "tasks.review.merge",
    ] {
        let (_home, _repository, shared, mut opened) = fixture();
        if method == "tasks.review.reopen" {
            let closed = params("tasks.review.close", &opened);
            let mut app = shared.lock().unwrap();
            opened["review"] =
                review_call(&mut app, "tasks.review.close", closed)["review"].clone();
        }
        let request = params(method, &opened);
        let done = defer(&shared, method, &request)
            .expect("lifecycle observation must be deferred")
            .run();
        let workspace = opened["review"]["workspace_id"].as_str().unwrap();
        let handler = AppState::handler(shared.clone());
        let current = handler.call(
            SessionSender::detached("stale-review"),
            req("tasks.review.get", json!({"task_id":opened["task"]["id"]})),
        );
        assert_eq!(current["ok"], true, "{current}");
        if method != "tasks.review.close" && method != "tasks.review.merge" {
            let close = handler.call(
                SessionSender::detached("stale-review"),
                req(
                    "tasks.review.close",
                    params("tasks.review.close", &current["result"]),
                ),
            );
            assert_eq!(close["ok"], true, "{close}");
        }
        // A new task starts again at review version one. Only the producer
        // generation, rather than task identity/version order, fences this.
        let replacement = handler.call(SessionSender::detached("stale-review"), req("tasks.review.open", json!({"workspace_id":workspace,"request_id":"new-generation","title":"Replacement review","description":"New publication"})));
        assert_eq!(replacement["ok"], true, "{replacement}");
        let before = shared
            .lock()
            .unwrap()
            .read_view
            .load()
            .get("workspace.get", workspace)
            .unwrap()["active_review"]
            .clone();
        shared
            .lock()
            .unwrap()
            .apply_deferred(method, &request, done)
            .unwrap();
        let after = shared
            .lock()
            .unwrap()
            .read_view
            .load()
            .get("workspace.get", workspace)
            .unwrap()["active_review"]
            .clone();
        assert_eq!(after, before, "delayed {method} overwrote replacement");
    }
}
