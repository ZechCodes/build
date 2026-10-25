use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

/// Dropping the handle stops new work; a running operation settles its journal.
pub struct MaintenanceHandle {
    stopped: Arc<AtomicBool>,
}
impl Drop for MaintenanceHandle {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

pub(crate) enum Registration {
    Ready(Store, PathBuf),
    Busy,
    Unavailable,
}

struct Service {
    stop: Arc<AtomicBool>,
    capture: Arc<dyn Fn() -> Registration + Send + Sync>,
    alive: Arc<dyn Fn() -> bool + Send + Sync>,
}

pub(crate) fn spawn(
    capture: Arc<dyn Fn() -> Registration + Send + Sync>,
    alive: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<MaintenanceHandle, String> {
    let stopped = Arc::new(AtomicBool::new(false));
    let service = Service {
        stop: stopped.clone(),
        capture,
        alive,
    };
    std::thread::Builder::new()
        .name("review-receiver-maintenance".into())
        .spawn(move || service.run())
        .map_err(|error| error.to_string())?;
    Ok(MaintenanceHandle { stopped })
}

impl Service {
    fn running(&self) -> bool {
        !self.stop.load(Ordering::Acquire) && (self.alive)()
    }

    fn register(&self) -> Option<Worker> {
        let mut last_notice = Instant::now();
        while self.running() {
            match (self.capture)() {
                Registration::Ready(store, root) => return Some(Worker::new(store, root)),
                Registration::Unavailable => return None,
                Registration::Busy => {
                    if last_notice.elapsed() >= Duration::from_secs(5) {
                        eprintln!("review receivers: startup capture busy; retrying");
                        last_notice = Instant::now();
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        None
    }

    fn run(self) {
        // Never return this niced thread to an interactive worker pool. Git
        // children inherit its priority; deletion/object work is niced too.
        crate::priority::lower_this_thread(crate::priority::CHILD_NICE);
        let Some(mut worker) = self.register() else {
            return;
        };
        while self.running() {
            match worker.turn() {
                Ok(report) => report_freed(&report),
                Err(error) => eprintln!("review receivers: maintenance will retry: {error}"),
            }
            // No app mutex, runtime worker, RPC callback or blocking admission.
            for _ in 0..30 {
                if !self.running() {
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

fn report_freed(report: &Report) {
    if report.freed_bytes > 0 || report.removed > 0 || report.collected > 0 {
        eprintln!("review receivers: freed at least {} allocated bytes; removed {} repositories; collected {}", report.freed_bytes, report.removed, report.collected);
    }
}
