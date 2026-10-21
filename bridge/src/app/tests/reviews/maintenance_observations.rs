//! Maintenance must not query review SQL while holding application state.
use super::*;
use std::sync::mpsc;
use std::time::Duration;

fn fixture() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    Arc<Mutex<AppState>>,
    String,
) {
    let home = tempfile::tempdir().unwrap();
    let (repository, mut app, project) = tracked(home.path());
    let workspace = workspace(&mut app, &project, "maintenance observations");
    wire_actions::open(&mut app, &workspace);
    app.frame_clock = FrameClock::reporting_to(Arc::new(|_| {}));
    app.initialize_snapshot_projection();
    (home, repository, Arc::new(Mutex::new(app)), workspace)
}

fn refresh(app: &mut AppState, workspace: &str, all: bool) {
    if all {
        // The writer-held check exercises physical refresh without loading review SQL.
        let _ = app.refresh_snapshot_observations();
    } else {
        app.refresh_workspace_snapshot_observation(workspace);
    }
}

fn refresh_with_writer_held(all: bool) {
    let (_home, _repository, shared, workspace) = fixture();
    let (store, clock) = {
        let app = shared.lock().unwrap();
        (app.store.clone().unwrap(), app.frame_clock.clone())
    };
    let samples = clock.capture_phases();
    let writer = store.writer_for_test();
    let (started, entered) = mpsc::channel();
    let (finished, complete) = mpsc::channel();
    let state = shared.clone();
    let worker = std::thread::spawn(move || {
        let timer = clock.frame("other");
        timer.sample_tool("maintenance-review-refresh");
        let mut app = timer.lock(&state);
        started.send(()).unwrap();
        refresh(&mut app, &workspace, all);
        drop(app);
        finished.send(()).unwrap();
    });
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let completed = complete.recv_timeout(Duration::from_secs(2)).is_ok();
    let mutex_available = shared.try_lock().is_ok();
    drop(writer);
    worker.join().unwrap();
    println!("TASK735_REFRESH_HOLD all={all} {}", samples.snapshot());
    assert!(
        completed && mutex_available,
        "physical snapshot refresh waited for review SQL with the app mutex held (all={all})"
    );
}

#[test]
fn maintenance_physical_refresh_releases_app_mutex_while_review_writer_is_held() {
    refresh_with_writer_held(true);
}

#[test]
fn workspace_physical_refresh_releases_app_mutex_while_review_writer_is_held() {
    refresh_with_writer_held(false);
}

#[test]
fn maintenance_review_refresh_native_hold_samples() {
    let (_home, _repository, shared, workspace) = fixture();
    let clock = shared.lock().unwrap().frame_clock.clone();
    let samples = clock.capture_phases();
    for _ in 0..8 {
        let timer = clock.frame("other");
        timer.sample_tool("maintenance-turn");
        // These are the complete capture/apply sections of the maintenance loop.
        let (refresh, reviews) = {
            let mut app = timer.lock(&shared);
            app.revive_parked_projects();
            app.refresh_workspace_snapshot_observations();
            app.board_list();
            let reviews = app.refresh_snapshot_observations();
            (app.capture_project_snapshot_refresh(), reviews)
        };
        let rendered = refresh.render();
        let reviews = reviews.load();
        let mut app = timer.lock(&shared);
        app.install_workspace_review_refresh(reviews);
        app.install_project_snapshot_refresh(rendered);
        app.publish_snapshot_projection();
    }
    for _ in 0..8 {
        let timer = clock.frame("other");
        timer.sample_tool("maintenance-workspace");
        refresh(&mut timer.lock(&shared), &workspace, false);
    }
    let captured = samples.snapshot();
    let turns = &captured["other:maintenance-turn"];
    assert_eq!(turns["frame.held"]["count"], 8);
    assert_eq!(turns["entered.lock"]["count"], 16);
    assert_eq!(
        captured["other:maintenance-workspace"]["frame.held"]["count"],
        8
    );
    println!("TASK735_REFRESH_NATIVE_HOLDS {captured}");
}

#[tokio::test(start_paused = true)]
async fn periodic_maintenance_loads_review_sql_while_app_mutex_is_held_elsewhere() {
    let (_home, _repository, shared, _workspace) = fixture();
    let (before, waiting) = OffLockGate::new();
    let (after, loaded) = OffLockGate::new();
    let workspaces = {
        let mut app = shared.lock().unwrap();
        app.off_lock_review_observation_gate = Some(before);
        app.off_lock_review_observation_loaded = Some(after);
        app.workspaces.list(None).len()
    };
    AppState::spawn_snapshot_maintenance(&shared);
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(10)).await;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !waiting.has_pending_arrival() {
        assert!(
            std::time::Instant::now() < deadline,
            "periodic maintenance did not reach the off-lock review reader"
        );
        tokio::task::yield_now().await;
        std::thread::sleep(Duration::from_millis(1));
    }
    let held = shared.try_lock();
    let mutex_available = held.is_ok();
    waiting.release();
    loaded.wait_for_arrival();
    loaded.release();
    for _ in 1..workspaces {
        waiting.wait_for_arrival();
        waiting.release();
        loaded.wait_for_arrival();
        loaded.release();
    }
    drop(held);
    assert!(
        mutex_available,
        "maintenance held the app mutex during review SQL"
    );
}

fn summary(shared: &Arc<Mutex<AppState>>, workspace: &str) -> Value {
    shared
        .lock()
        .unwrap()
        .read_view
        .load()
        .get("workspace.get", workspace)
        .unwrap()["active_review"]
        .clone()
}

fn call(handler: &FrameHandler, method: &str, params: Value) -> Value {
    let answer = handler.call(
        SessionSender::detached("maintenance-review"),
        req(method, params),
    );
    assert_eq!(answer["ok"], true, "{answer}");
    answer["result"].clone()
}

#[test]
fn delayed_maintenance_batch_preserves_newer_close_and_replacement_review() {
    for replace in [false, true] {
        let (_home, _repository, shared, workspace) = fixture();
        let before = summary(&shared, &workspace);
        let refresh = shared.lock().unwrap().refresh_snapshot_observations();
        let delayed = refresh.load();
        let handler = AppState::handler(shared.clone());
        call(
            &handler,
            "tasks.review.close",
            json!({
                "task_id":before["task_id"], "expected_version":before["version"],
                "description":"Newer lifecycle observation",
            }),
        );
        if replace {
            call(
                &handler,
                "tasks.review.open",
                json!({
                    "workspace_id":workspace, "request_id":"replacement-after-maintenance",
                    "title":"Replacement review", "description":"New task generation",
                }),
            );
        }
        let expected = summary(&shared, &workspace);
        assert_eq!(expected["status"], if replace { "open" } else { "closed" });
        {
            let mut app = shared.lock().unwrap();
            app.install_workspace_review_refresh(delayed);
            app.publish_snapshot_projection();
        }
        assert_eq!(summary(&shared, &workspace), expected);
    }
}

#[test]
fn delayed_maintenance_batch_does_not_restore_a_deleted_workspace_review() {
    let (_home, _repository, shared, workspace) = fixture();
    let refresh = shared.lock().unwrap().refresh_snapshot_observations();
    let delayed = refresh.load();
    let mut app = shared.lock().unwrap();
    app.workspaces.forget(&workspace);
    // Only prune the removed workspace before attempting the delayed review installation.
    let _ = app.refresh_snapshot_observations();
    app.install_workspace_review_refresh(delayed);
    app.publish_snapshot_projection();
    let mut row = json!({});
    app.snapshot_workspace_review(&workspace, &mut row);
    assert!(row.get("active_review").is_none(), "{row}");
    assert!(app
        .read_view
        .load()
        .get("workspace.get", &workspace)
        .is_none());
}

fn refresh_reviews(shared: &Arc<Mutex<AppState>>) {
    let refresh = shared.lock().unwrap().refresh_snapshot_observations();
    let reviews = refresh.load();
    let mut app = shared.lock().unwrap();
    app.install_workspace_review_refresh(reviews);
    app.publish_snapshot_projection();
}

#[test]
fn maintenance_failure_preserves_accepted_summary_and_next_turn_repairs_it() {
    let (_home, _repository, shared, workspace) = fixture();
    let before = summary(&shared, &workspace);
    let store = shared.lock().unwrap().store.clone().unwrap();
    let task = before["task_id"].as_str().unwrap();
    let record: String = {
        let writer = store.writer_for_test();
        let record = writer
            .query_row(
                "SELECT record FROM reviews WHERE task_id = ?1",
                [task],
                |row| row.get(0),
            )
            .unwrap();
        writer
            .execute(
                "UPDATE reviews SET record = '{}' WHERE task_id = ?1",
                [task],
            )
            .unwrap();
        record
    };
    refresh_reviews(&shared);
    assert_eq!(summary(&shared, &workspace), before);
    let mut repaired: Value = serde_json::from_str(&record).unwrap();
    repaired["version"] = json!(before["version"].as_u64().unwrap() + 1);
    store
        .writer_for_test()
        .execute(
            "UPDATE reviews SET version = ?1, record = ?2 WHERE task_id = ?3",
            rusqlite::params![
                repaired["version"].as_i64().unwrap(),
                repaired.to_string(),
                task
            ],
        )
        .unwrap();
    refresh_reviews(&shared);
    let after = summary(&shared, &workspace);
    assert_eq!(after["task_id"], before["task_id"]);
    assert_eq!(after["version"], repaired["version"]);
}
