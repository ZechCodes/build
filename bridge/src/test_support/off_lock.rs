use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

#[derive(Clone)]
pub struct OffLockGate {
    arrived: mpsc::Sender<()>,
    permits: Arc<Mutex<mpsc::Receiver<()>>>,
}

pub struct OffLockGateHandle {
    arrivals: mpsc::Receiver<()>,
    permits: mpsc::Sender<()>,
}

impl OffLockGate {
    pub(crate) fn new() -> (Self, OffLockGateHandle) {
        let (arrived, arrivals) = mpsc::channel();
        let (permits, waiting) = mpsc::channel();
        (
            Self {
                arrived,
                permits: Arc::new(Mutex::new(waiting)),
            },
            OffLockGateHandle { arrivals, permits },
        )
    }

    pub fn arrive(&self) {
        let _ = self.arrived.send(());
        let _ = self.permits.lock().unwrap().recv();
    }
}

impl OffLockGateHandle {
    pub(crate) fn wait_for_arrival(&self) {
        self.arrivals
            .recv_timeout(Duration::from_secs(30))
            .expect("the deferred work reached its lock-free phase");
    }

    pub(crate) fn has_pending_arrival(&self) -> bool {
        self.arrivals.try_recv().is_ok()
    }

    pub(crate) fn release(&self) {
        self.permits.send(()).expect("the gate is still open");
    }
}

// Path-scoped, one-shot pauses inside origin observation. Holding an opened
// repository here exercises the production repository-access call stack.
struct OriginPause {
    path: std::path::PathBuf,
    skip: usize,
    gate: OffLockGate,
}

static ORIGIN_PAUSES: std::sync::OnceLock<Mutex<Vec<OriginPause>>> = std::sync::OnceLock::new();

pub(crate) struct OriginPauseGuard(std::path::PathBuf);
impl Drop for OriginPauseGuard {
    fn drop(&mut self) {
        ORIGIN_PAUSES
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .retain(|p| p.path != self.0);
    }
}

pub(crate) fn pause_origin_read(
    path: &std::path::Path,
    skip: usize,
) -> (OriginPauseGuard, OffLockGateHandle) {
    let (gate, handle) = OffLockGate::new();
    ORIGIN_PAUSES
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .push(OriginPause {
            path: path.to_owned(),
            skip,
            gate,
        });
    (OriginPauseGuard(path.to_owned()), handle)
}

pub(crate) fn inside_origin_read(path: &std::path::Path) {
    let Some(pauses) = ORIGIN_PAUSES.get() else {
        return;
    };
    let gate = {
        let mut pauses = pauses.lock().unwrap();
        let Some(index) = pauses.iter().position(|p| p.path == path) else {
            return;
        };
        if pauses[index].skip > 0 {
            pauses[index].skip -= 1;
            None
        } else {
            Some(pauses.remove(index).gate)
        }
    };
    if let Some(gate) = gate {
        gate.arrive();
    }
}
