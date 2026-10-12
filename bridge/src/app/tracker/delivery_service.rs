//! Client-independent assignment recovery. Resource work never holds the app lock.
use super::deferred_delivery::TaskDeliveryWork;
use crate::app::{off_the_workers, AppState, DeliveryRunner};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

impl AppState {
    pub(in crate::app) fn spawn_task_delivery_service(state: &Arc<Mutex<Self>>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let Some(wakeup) = state
            .lock()
            .unwrap()
            .store
            .as_ref()
            .map(|store| Arc::clone(&store.waiting_on_wakeup))
        else {
            return;
        };
        let weak = Arc::downgrade(state);
        runtime.spawn(async move {
            loop {
                let current = weak.clone();
                match off_the_workers(move || delivery_step(&current)).await {
                    None => return,
                    Some(Ok(true)) => {
                        tokio::task::yield_now().await;
                        continue;
                    }
                    Some(Ok(false)) => {}
                    Some(Err(error)) => eprintln!("deliver saved task assignment: {error}"),
                }
                tokio::select! {
                    _ = wakeup.notified() => {},
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {},
                }
            }
        });
    }
}

fn delivery_step(weak: &Weak<Mutex<AppState>>) -> Option<Result<bool, String>> {
    let state = weak.upgrade()?;
    let result = (|| {
        let (work, notices) = {
            let mut app = state.lock().unwrap();
            let notices = app.pump_task_delivery_notices()?;
            (app.prepare_task_delivery()?, notices)
        };
        let Some(TaskDeliveryWork { intent, git }) = work else {
            return Ok(notices);
        };
        let result = git.as_ref().map(crate::app::DeferredGit::run);
        state
            .lock()
            .unwrap()
            .finish_task_delivery(intent, git.as_ref(), result)?;
        Ok(true)
    })();
    let clock = Arc::clone(&state.lock().unwrap().frame_clock);
    DeliveryRunner::drain(&state, &clock.frame("tasks.delivery_service"));
    Some(result)
}
