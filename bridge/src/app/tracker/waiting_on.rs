//! Client-independent dependency reconciliation. Slow SQLite work stays off
//! tokio workers; each app-lock section processes at most one fan-out page.
use crate::app::{off_the_workers, AppState};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

impl AppState {
    pub(in crate::app) fn spawn_waiting_on_service(state: &Arc<Mutex<Self>>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let Some(wakeup) = state
            .lock()
            .unwrap()
            .store
            .as_ref()
            .map(|s| Arc::clone(&s.waiting_on_wakeup))
        else {
            return;
        };
        let weak = Arc::downgrade(state);
        runtime.spawn(async move {
            loop {
                let work = weak.clone();
                match off_the_workers(move || reconcile_page(&work)).await {
                    None => return,
                    Some(Ok(true)) => {
                        tokio::task::yield_now().await;
                        continue;
                    }
                    Some(Ok(false)) => {}
                    Some(Err(error)) => eprintln!("reconcile task dependencies: {error}"),
                }
                tokio::select! {
                    _ = wakeup.notified() => {},
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {},
                }
            }
        });
    }
}

fn reconcile_page(weak: &Weak<Mutex<AppState>>) -> Option<Result<bool, String>> {
    let state = weak.upgrade()?;
    let app = state.lock().unwrap();
    Some((|| {
        let store = app.tracker_store()?;
        let page = store.reconcile_waiting_on(100).map_err(|e| e.to_string())?;
        let changes = store
            .take_waiting_on_changes(100)
            .map_err(|e| e.to_string())?;
        for (path, task_id) in &changes {
            if let Some(project) = app
                .projects
                .iter()
                .find(|p| p.repo_path.to_str() == Some(path))
            {
                app.note_tasks_changed(&project.id, task_id);
            }
        }
        Ok(page.pending || changes.len() == 100)
    })())
}
