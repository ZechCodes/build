//! Client-independent assignment recovery. Resource work never holds the app lock.
use super::deferred_delivery::TaskDeliveryWork;
use crate::app::{off_the_workers, AppState, DeliveryRunner};
use crate::timing::{FrameClock, FrameTimer};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

struct TaskDeliveryService {
    state: Weak<Mutex<AppState>>,
    clock: Arc<FrameClock>,
    wakeup: Option<Arc<tokio::sync::Notify>>,
    #[cfg(test)]
    origins: Option<crate::store::delivery_origins::DeliveryOrigins>,
}

impl TaskDeliveryService {
    fn new(state: &Arc<Mutex<AppState>>) -> Self {
        let app = state.lock().unwrap();
        Self {
            state: Arc::downgrade(state),
            clock: Arc::clone(&app.frame_clock),
            wakeup: app
                .store
                .as_ref()
                .map(|store| Arc::clone(&store.waiting_on_wakeup)),
            #[cfg(test)]
            origins: app
                .store
                .as_ref()
                .map(|store| store.delivery_origins.clone()),
        }
    }

    fn step(&self) -> Option<Result<bool, String>> {
        let state = self.state.upgrade()?;
        #[cfg(test)]
        let background = self.clock.background_write("tasks.delivery_service");
        #[cfg(test)]
        let scan = self.origins.as_ref().map(|origins| origins.scan());
        #[cfg(test)]
        let scanning = crate::store::delivery_origins::Run::joined(
            scan.as_ref()
                .and_then(|scan| scan.context())
                .into_iter()
                .chain(background.as_ref().map(|write| write.context())),
        );
        let mut timer = self.clock.frame("tasks.delivery_service");
        #[cfg(test)]
        timer.set_write_context(scanning.context());
        let result = self.deliver_step(
            &state,
            &mut timer,
            #[cfg(test)]
            background.as_ref().map(|write| write.context()),
            #[cfg(test)]
            (scan, scanning),
        );
        #[cfg(test)]
        {
            if let Some(background) = background {
                background.complete();
            }
        }
        Some(result)
    }

    fn deliver_step(
        &self,
        state: &Arc<Mutex<AppState>>,
        timer: &mut FrameTimer,
        #[cfg(test)] background: Option<crate::timing::write_wait::WriteContext>,
        #[cfg(test)] scan_leases: (
            Option<crate::store::delivery_origins::Run>,
            crate::store::delivery_origins::Run,
        ),
    ) -> Result<bool, String> {
        let prepared: Result<(Option<TaskDeliveryWork>, bool), String> = (|| {
            #[cfg(test)]
            FirstLockPulse::arrived();
            #[cfg(test)]
            let mut app = timer.lock(state);
            #[cfg(not(test))]
            let mut app = state.lock().unwrap();
            let notices = {
                // The shared first acquisition is owed to ready assignments;
                // unrelated persisted notices belong to this background pass.
                #[cfg(test)]
                let _notice = background.as_ref().map(|context| context.bind_sync());
                app.pump_task_delivery_notices()?
            };
            #[cfg(test)]
            let work = app.prepare_task_delivery_with_context(background.clone())?;
            #[cfg(not(test))]
            let work = app.prepare_task_delivery()?;
            Ok((work, notices))
        })();
        #[cfg(test)]
        let running = prepared
            .as_ref()
            .ok()
            .and_then(|(work, _)| work.as_ref())
            .and_then(|work| work.attempt.as_ref())
            .map(|attempt| attempt.start());
        #[cfg(test)]
        let selected = crate::store::delivery_origins::Run::joined(
            running
                .as_ref()
                .and_then(|run| run.context())
                .into_iter()
                .chain(background),
        );
        #[cfg(test)]
        timer.set_write_context(selected.context());
        #[cfg(test)]
        {
            scan_leases.1.complete();
            if let Some(scan) = scan_leases.0 {
                scan.complete();
            }
        }
        let result = prepared.and_then(|(work, notices)| {
            let Some(work) = work else {
                return Ok(notices);
            };
            #[cfg(test)]
            let _binding = timer.write_context().map(|context| context.bind_sync());
            let result = work.git.as_ref().map(crate::app::DeferredGit::run);
            #[cfg(test)]
            let mut app = timer.lock(state);
            #[cfg(not(test))]
            let mut app = state.lock().unwrap();
            app.finish_prepared_task_delivery(work, result)?;
            Ok(true)
        });
        DeliveryRunner::drain(state, timer);
        result
    }
}

impl AppState {
    /// Capture the real service handle before the test holds the app mutex.
    #[cfg(test)]
    pub(in crate::app) fn task_delivery_step_for_test(
        state: &Arc<Mutex<Self>>,
        pulse: std::sync::mpsc::Sender<()>,
    ) -> impl FnOnce() -> Option<Result<bool, String>> + Send + 'static {
        let service = TaskDeliveryService::new(state);
        move || {
            let _pulse = FirstLockPulse::install(pulse);
            service.step()
        }
    }

    pub(in crate::app) fn spawn_task_delivery_service(state: &Arc<Mutex<Self>>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let service = Arc::new(TaskDeliveryService::new(state));
        let Some(wakeup) = service.wakeup.clone() else {
            return;
        };
        runtime.spawn(async move {
            loop {
                let current = Arc::clone(&service);
                match off_the_workers(move || current.step()).await {
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
