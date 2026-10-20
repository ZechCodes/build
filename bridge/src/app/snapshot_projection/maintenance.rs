//! Periodic producers replace maintenance formerly triggered by list reads.
use super::AppState;
use crate::app::off_the_workers;
use std::sync::{Arc, Mutex};
use std::time::Duration;

impl AppState {
    pub(in crate::app) fn spawn_snapshot_maintenance(state: &Arc<Mutex<Self>>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let state = Arc::downgrade(state);
        runtime.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            interval.tick().await;
            loop {
                interval.tick().await;
                let Some(state) = state.upgrade() else {
                    return;
                };
                let captured = off_the_workers(move || {
                    let mut app = state.lock().unwrap();
                    app.revive_parked_projects();
                    app.refresh_workspace_snapshot_observations();
                    app.board_list();
                    app.refresh_snapshot_observations();
                    (Arc::clone(&state), app.capture_project_snapshot_refresh())
                })
                .await;
                off_the_workers(move || {
                    let (state, refresh) = captured;
                    let rendered = refresh.render();
                    let app = state.lock().unwrap();
                    app.install_project_snapshot_refresh(rendered);
                    app.publish_snapshot_projection();
                })
                .await;
            }
        });
    }
}
