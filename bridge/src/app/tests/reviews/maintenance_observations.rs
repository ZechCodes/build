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
        app.refresh_snapshot_observations();
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
        let timer = clock.frame("maintenance-review-refresh");
        let mut app = timer.lock(&state);
        started.send(()).unwrap();
        refresh(&mut app, &workspace, all);
        drop(app);
        finished.send(()).unwrap();
    });
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let completed = complete.recv_timeout(Duration::from_millis(200)).is_ok();
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
        let timer = clock.frame("maintenance-turn");
        // These are the complete capture/apply sections of the maintenance loop.
        let refresh = {
            let mut app = timer.lock(&shared);
            app.revive_parked_projects();
            app.refresh_workspace_snapshot_observations();
            app.board_list();
            app.refresh_snapshot_observations();
            app.capture_project_snapshot_refresh()
        };
        let rendered = refresh.render();
        let app = timer.lock(&shared);
        app.install_project_snapshot_refresh(rendered);
        app.publish_snapshot_projection();
    }
    for _ in 0..8 {
        let timer = clock.frame("maintenance-workspace");
        refresh(&mut timer.lock(&shared), &workspace, false);
    }
    println!("TASK735_REFRESH_NATIVE_HOLDS {}", samples.snapshot());
}
