//! Client-independent assignment recovery. Resource work never holds the app lock.
use crate::app::{off_the_workers, AppState, DeliveryRunner};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

impl AppState {
    /// Invoke the actual service step without starting its perpetual worker.
    #[cfg(test)]
    pub(in crate::app) fn task_delivery_step_for_test(
        state: &Arc<Mutex<Self>>,
        pulse: std::sync::mpsc::Sender<()>,
    ) -> Option<Result<bool, String>> {
        let _pulse = FirstLockPulse::install(pulse);
        delivery_step(&Arc::downgrade(state))
    }

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
            #[cfg(test)]
            FirstLockPulse::arrived();
            let mut app = state.lock().unwrap();
            let notices = app.pump_task_delivery_notices()?;
            (app.prepare_task_delivery()?, notices)
        };
        let Some(work) = work else {
            return Ok(notices);
        };
        let result = work.git.as_ref().map(crate::app::DeferredGit::run);
        state
            .lock()
            .unwrap()
            .finish_prepared_task_delivery(work, result)?;
        Ok(true)
    })();
    let clock = Arc::clone(&state.lock().unwrap().frame_clock);
    DeliveryRunner::drain(&state, &clock.frame("tasks.delivery_service"));
    Some(result)
}

// Observation only: this pulse neither creates attribution nor changes locking.
#[cfg(test)]
thread_local! {
    static FIRST_LOCK_PULSE: std::cell::RefCell<Option<std::sync::mpsc::Sender<()>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
struct FirstLockPulse;

#[cfg(test)]
impl FirstLockPulse {
    fn install(pulse: std::sync::mpsc::Sender<()>) -> Self {
        FIRST_LOCK_PULSE.with(|slot| {
            assert!(slot.borrow().is_none(), "service pulse cannot nest");
            *slot.borrow_mut() = Some(pulse);
        });
        Self
    }

    fn arrived() {
        FIRST_LOCK_PULSE.with(|slot| {
            if let Some(pulse) = slot.borrow_mut().take() {
                let _ = pulse.send(());
            }
        });
    }
}

#[cfg(test)]
impl Drop for FirstLockPulse {
    fn drop(&mut self) {
        FIRST_LOCK_PULSE.with(|slot| {
            slot.borrow_mut().take();
        });
    }
}
