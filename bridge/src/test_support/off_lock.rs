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
