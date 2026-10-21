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
    let mut value = json!({"task_id":opened["review"]["task_id"],"expected_version":opened["review"]["version"]});
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

fn published_statuses(app: &AppState, opened: &Value) -> (Value, Value) {
    let view = app.read_view.load();
    let workspace = opened["review"]["workspace_id"].as_str().unwrap();
    let workspace_status =
        view.get("workspace.get", workspace).unwrap()["active_review"]["status"].clone();
    let task_status = view.get("board.list", "").unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["task_id"] == opened["task"]["id"])
        .unwrap()["review_summary"]["status"]
        .clone();
    (workspace_status, task_status)
}

#[test]
fn pending_close_keeps_workspace_and_watched_task_publication_consistent() {
    let (_home, _repository, shared, opened) = fixture();
    let request = params("tasks.review.close", &opened);
    assert_eq!(
        published_statuses(&shared.lock().unwrap(), &opened),
        (json!("open"), json!("open"))
    );
    let job = defer(&shared, "tasks.review.close", &request).unwrap();
    // Admission cannot publish the completed watched task while its workspace
    // still has the accepted open summary and its SQL observation is pending.
    assert_eq!(
        published_statuses(&shared.lock().unwrap(), &opened),
        (json!("open"), json!("open"))
    );
    let done = job.run();
    assert_eq!(
        published_statuses(&shared.lock().unwrap(), &opened),
        (json!("open"), json!("open"))
    );
    shared
        .lock()
        .unwrap()
        .apply_deferred("tasks.review.close", &request, done)
        .unwrap();
    assert_eq!(
        published_statuses(&shared.lock().unwrap(), &opened),
        (json!("closed"), json!("closed"))
    );
}

#[test]
fn close_writer_rejects_review_version_changed_after_admission() {
    let (_home, _repository, shared, opened) = fixture();
    let request = params("tasks.review.close", &opened);
    let job = defer(&shared, "tasks.review.close", &request).unwrap();
    let task = opened["task"]["id"].as_str().unwrap();
    review_call(
        &mut shared.lock().unwrap(),
        "tasks.comment",
        json!({
            "task_id":task,"body":"New opinion before close commits",
            "opinion":{"snapshot_id":opened["review"]["snapshots"][0]["id"],"verdict":"approve"}
        }),
    );
    let done = job.run();
    let mut app = shared.lock().unwrap();
    let error = app
        .apply_deferred("tasks.review.close", &request, done)
        .unwrap_err();
    assert!(error.contains("stale_version"), "{error}");
    let review = app
        .store
        .as_ref()
        .unwrap()
        .load_review(task)
        .unwrap()
        .unwrap();
    assert_eq!(review.version, 2);
    assert!(review.completion.is_none());
    assert_eq!(
        published_statuses(&app, &opened),
        (json!("approved"), json!("approved"))
    );
}

fn task_actions(app: &mut AppState, owner: &str, agent: &str) -> Vec<Value> {
    let page = review_call(
        app,
        "thread.page",
        json!({"entity_id":owner,"agent_id":agent,"limit":50}),
    );
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["data"].get("task_action").cloned())
        .collect()
}

#[test]
fn close_projection_failure_still_delivers_captured_completion_once() {
    let (_home, _repository, shared, opened) = fixture();
    let request = params("tasks.review.close", &opened);
    let (owner, agent, store, job) = {
        let mut app = shared.lock().unwrap();
        let workspace = opened["review"]["workspace_id"].as_str().unwrap();
        let project = app.workspaces.get(workspace).unwrap().project_id.clone();
        let (owner, agent) = project_agent(&mut app, &project);
        app.review_close(
            serde_json::from_value(request.clone()).unwrap(),
            crate::tracker::Actor::Agent {
                agent_id: agent.clone(),
            },
        )
        .unwrap();
        (
            owner,
            agent,
            app.store.clone().unwrap(),
            app.take_deferred().unwrap(),
        )
    };
    store
        .writer_for_test()
        .execute_batch("ALTER TABLE review_sync_observations RENAME TO hidden_sync_observations")
        .unwrap();
    let done = job.run();
    store
        .writer_for_test()
        .execute_batch("ALTER TABLE hidden_sync_observations RENAME TO review_sync_observations")
        .unwrap();
    let mut app = shared.lock().unwrap();
    assert!(
        task_actions(&mut app, &owner, &agent).is_empty(),
        "completion side effects wait for apply"
    );
    let error = app
        .apply_deferred("tasks.review.close", &request, done)
        .unwrap_err();
    assert!(error.contains("review_sync_observations"), "{error}");
    let actions = task_actions(&mut app, &owner, &agent);
    assert_eq!(actions.len(), 1, "{actions:?}");
    assert_eq!(actions[0]["action"], "moved");
    assert_eq!(actions[0]["to"], "done");
    let review = store
        .load_review(opened["task"]["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        review.completion.unwrap().actor.agent_id(),
        Some(agent.as_str())
    );
    assert_eq!(
        published_statuses(&app, &opened),
        (json!("closed"), json!("closed"))
    );
}
