use super::*;
use crate::app::tests::runtime::write_wait_qualification::client_watch::TrafficCohort;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc as sync;

mod native;
mod refusal;

struct Pause {
    entered: sync::Receiver<()>,
    release: Option<sync::Sender<()>>,
    timely: Arc<AtomicBool>,
}
impl Pause {
    fn new() -> (Self, impl FnOnce() + Send + 'static) {
        let (arrive, entered) = sync::channel();
        let (release, wait) = sync::channel();
        let timely = Arc::new(AtomicBool::new(true));
        let outcome = Arc::clone(&timely);
        (
            Self {
                entered,
                release: Some(release),
                timely,
            },
            move || {
                let _ = arrive.send(());
                if wait.recv_timeout(Duration::from_secs(2)).is_err() {
                    outcome.store(false, Ordering::Release);
                }
            },
        )
    }
    fn reached(&self) -> bool {
        self.entered.recv_timeout(Duration::from_secs(2)).is_ok()
    }
    fn release(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}
impl Drop for Pause {
    fn drop(&mut self) {
        self.release();
    }
}

async fn server_terminal(rpc: &RpcHarness, id: &str) -> bool {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        let terminal = rpc.registry.snapshot().iter().any(|row| {
            row.original_id == id
                && row.root_finished
                && row.completed_at.is_some()
                && row.outstanding.is_empty()
                && row.active.is_empty()
        });
        if terminal || Instant::now() >= until {
            return terminal;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}
async fn detach_when_quiet(
    rpc: &RpcHarness,
) -> Result<Option<Arc<FinalReconciliation>>, Vec<String>> {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        let result = rpc.detach_client_watch();
        if result.is_ok() || Instant::now() >= until {
            return result;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}
