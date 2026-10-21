//! Control cleanup uses real all-tab native retirements; no new RPC cleanup IDs.
use super::*;
use futures_util::FutureExt;
use std::time::Duration;

struct CloseJob {
    resources: Arc<RwLock<Option<PreparedBackend>>>,
    closed: Arc<AtomicBool>,
    errors: Arc<Mutex<Vec<String>>>,
    archive: Arc<Mutex<receipt_archive::Pump>>,
    observation: Option<Observation>,
    sink: Arc<dyn ArtifactSink>,
}
impl ControlBackend {
    pub(super) fn start_close(&self) {
        if self.close_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let job = CloseJob {
            resources: Arc::clone(&self.resources),
            closed: Arc::clone(&self.closed),
            errors: Arc::clone(&self.close_error),
            archive: Arc::clone(&self.archive),
            observation: self.observation.lock().unwrap().clone(),
            sink: Arc::clone(&self.sink),
        };
        *self.cleanup_task.lock().unwrap() = Some(tokio::spawn(job.run()));
    }
}
impl CloseJob {
    fn error(&self, error: impl Into<String>) {
        self.errors.lock().unwrap().push(error.into());
    }
    async fn run(self) {
        let mut prepared = self.resources.write().await.take();
        if prepared.is_none() {
            self.error("control cleanup resources already taken");
            return;
        }
        // The owner stays outside the caught future: a panic cannot drop the
        // private root or detach accepted work by unwinding the cleanup task.
        let result = std::panic::AssertUnwindSafe(self.run_owned(&mut prepared))
            .catch_unwind()
            .await;
        if result.is_err() {
            self.error("actual cleanup panicked; resources retained, no closed claim");
        }
        if let Some(prepared) = prepared {
            *self.resources.write().await = Some(prepared);
        }
    }
    async fn run_owned(&self, owned: &mut Option<PreparedBackend>) {
        let prepared = owned.as_mut().expect("retained cleanup owner");
        if !self.join_workers(prepared).await {
            return;
        }
        self.final_archive(prepared);
        let owner = release_app_owners(prepared);
        let native_joined = self.close_native(prepared).await;
        if let Some(owner) = owner {
            if !self.finish_watch(&owner).await {
                prepared.lifecycle = Some(owner);
                return;
            }
        }
        if native_joined {
            // Physical cleanup status is separate from explicit proof RED.
            owned.take();
            self.closed.store(true, Ordering::Release);
        }
    }
    async fn join_workers(&self, prepared: &PreparedBackend) -> bool {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while prepared
            .producers
            .as_ref()
            .is_some_and(|producers| !producers.handles.iter().all(|producer| producer.joined()))
        {
            if tokio::time::Instant::now() >= deadline {
                self.error("actual producer join incomplete; ownership retained");
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if let Some(mcp) = &prepared.mcp {
            let report = mcp.shutdown(deadline).await;
            if !report.is_clean() {
                self.error(format!(
                    "actual MCP cleanup report: unfinished={:?}, errors={:?}",
                    report.unfinished, report.evidence.errors
                ));
            }
            if !report.quiet || !report.unfinished.is_empty() {
                return false;
            }
        }
        true
    }
    fn final_archive(&self, prepared: &PreparedBackend) {
        let Some(observe) = &self.observation else {
            self.error("actual control observation missing at cleanup");
            return;
        };
        if let Err(error) = prepared.frames.drain_into(observe) {
            self.error(error);
        }
        if let Err(error) = self
            .archive
            .lock()
            .unwrap()
            .finish(prepared, observe, &*self.sink)
        {
            self.error(error);
        }
    }
    async fn close_native(&self, prepared: &mut PreparedBackend) -> bool {
        let joined = if let Some(fixture) = prepared.fixture.take() {
            std::panic::AssertUnwindSafe(fixture.shutdown())
                .catch_unwind()
                .await
                .is_ok()
        } else {
            false
        };
        if !joined {
            self.error("actual native control cleanup failed; native private-root retention evidence required");
        }
        joined
    }
    async fn finish_watch(&self, owner: &Arc<watch_owner::WatchOwner>) -> bool {
        owner.request_finish();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !owner.is_finished() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let report = owner.report();
        if report
            .as_ref()
            .is_none_or(|report| report.failed || report.panicked)
        {
            self.error(format!("actual client watchdog report: {report:?}"));
        }
        owner.is_finished() && report.is_some()
    }
}
fn release_app_owners(prepared: &mut PreparedBackend) -> Option<Arc<watch_owner::WatchOwner>> {
    let owner = prepared.lifecycle.take();
    // McpHarness Inner owns AppState; immutable Probe.mcp does not. Joined
    // producer bodies already released captured State. Actual fixture tabs
    // own the second PTY after this external child Arc drops, then real native
    // Retirement receipts close/join it inside fixture.shutdown().
    prepared.probe.take();
    prepared.mcp.take();
    prepared.producers.take();
    prepared.services.take();
    prepared.reads.take();
    prepared.terminal.take();
    owner
}
