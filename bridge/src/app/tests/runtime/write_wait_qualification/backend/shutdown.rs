//! Real blocked-setup cleanup, with ownership retained on bounded join failure.
use super::super::trial_complete::ProducerHandle;
use super::*;
use futures_util::FutureExt;
use std::time::Duration;
impl PreparedBackend {
    pub async fn shutdown(mut self) -> Result<CleanupEvidence, RetainedSetup> {
        if let Some(producers) = &self.producers {
            for producer in &producers.handles {
                producer.stop_offering();
            }
        }
        if let Some(mcp) = &self.mcp {
            mcp.stop_offers();
        }
        if let Err(reason) = self.join_setup_workers().await {
            return Err(RetainedSetup {
                reason,
                retained: self,
            });
        }
        if let Some(lifecycle) = &self.lifecycle {
            lifecycle.request_finish();
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while self
            .lifecycle
            .as_ref()
            .is_some_and(|lifecycle| !lifecycle.is_finished())
        {
            if tokio::time::Instant::now() >= deadline {
                return Err(RetainedSetup {
                    reason: "actual client watchdog has not joined; setup retained".into(),
                    retained: self,
                });
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if self.lifecycle.as_ref().is_some_and(|lifecycle| {
            lifecycle
                .report()
                .is_none_or(|report| report.failed || report.panicked)
        }) {
            return Err(RetainedSetup {
                reason:
                    "actual joined client watchdog lacks a clean report; setup evidence retained"
                        .into(),
                retained: self,
            });
        }
        // The blocked path NEVER attached ClientWatch to RPC, so native cleanup
        // IDs are legal here without inventing a future quiet-detach method.
        self.probe.take();
        self.mcp.take();
        self.lifecycle.take();
        self.watch.take();
        self.producers.take();
        self.services.take();
        self.reads.take();
        let mut cleanup_error = None;
        if let Some(terminal) = self.terminal.take() {
            let cleanup = terminal.shutdown(&self.fixture.as_ref().expect("owned fixture").rpc);
            if std::panic::AssertUnwindSafe(cleanup)
                .catch_unwind()
                .await
                .is_err()
            {
                cleanup_error = Some("actual exempt terminal cleanup failed".to_owned());
            }
        }
        if let Some(fixture) = self.fixture.take() {
            if std::panic::AssertUnwindSafe(fixture.shutdown())
                .catch_unwind()
                .await
                .is_err()
            {
                cleanup_error = Some(
                    "native owned fixture cleanup failed; native retention evidence required"
                        .into(),
                );
            }
        }
        if let Some(reason) = cleanup_error {
            return Err(RetainedSetup {
                reason,
                retained: self,
            });
        }
        Ok(CleanupEvidence {
            producers_joined: true,
            mcp_quiet: true,
            client_watch_joined: true,
            native_cleanup_completed: true,
        })
    }
    pub(super) async fn join_setup_workers(&self) -> Result<(), String> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let joined = self
                .producers
                .as_ref()
                .is_none_or(|producers| producers.handles.iter().all(|producer| producer.joined()));
            if joined {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(
                    "actual setup producer thread has not joined; body and fixture retained".into(),
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if let Some(mcp) = &self.mcp {
            let report = mcp.shutdown(deadline).await;
            if !report.is_clean() {
                return Err(
                    "actual MCP setup cleanup not clean; adapter/server ownership retained".into(),
                );
            }
        }
        Ok(())
    }
}
