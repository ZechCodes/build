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
        let (report, before) = match self.audit_before_native_cleanup().await {
            Ok(audit) => audit,
            Err(reason) => {
                return Err(RetainedSetup {
                    reason,
                    retained: self,
                });
            }
        };
        self.probe.take();
        self.mcp.take();
        // Retain actual report and final lifecycle rows if cleanup returns an
        // error. A failed trial must never be reset to permit native cleanup.
        self.producers.take();
        self.services.take();
        self.reads.take();
        let mut cleanup_error = if report.as_ref().is_some_and(|report| report.failed)
            || before
                .as_ref()
                .is_some_and(|audit| !audit.violations.is_empty())
        {
            Some("actual joined client watch remains failed; native cleanup attempted, raw report retained".to_owned())
        } else {
            None
        };
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
        if before.is_some()
            && self
                .watch
                .as_ref()
                .is_some_and(|watch| !watch.reconcile_final().violations.is_empty())
        {
            cleanup_error = Some("actual final client reconciliation remains failed after native cleanup; raw watch retained".into());
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
    /// Both native close paths share the actual join and quiet-detach boundary.
    /// Failed terminal rows stay failed; they do not represent active receivers.
    pub(super) async fn audit_before_native_cleanup(
        &self,
    ) -> Result<
        (
            Option<Arc<super::super::client_watch::WatchReport>>,
            Option<Arc<super::super::client_watch::FinalReconciliation>>,
        ),
        String,
    > {
        let report = if let Some(lifecycle) = &self.lifecycle {
            lifecycle.request_finish();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            while !lifecycle.is_finished() {
                if tokio::time::Instant::now() >= deadline {
                    return Err("actual client watchdog has not joined; ownership retained".into());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let report = lifecycle
                .report()
                .ok_or("actual joined client watchdog lacks a report; ownership retained")?;
            if report.panicked {
                return Err("actual joined client watchdog panicked; ownership retained".into());
            }
            Some(report)
        } else {
            None
        };
        let fixture = self
            .fixture
            .as_ref()
            .ok_or("owned fixture missing before quiet detach")?;
        let audit = fixture.rpc.detach_client_watch().map_err(|errors| {
            format!("actual quiet trial detach refused; ownership retained: {errors:?}")
        })?;
        Ok((report, audit))
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
