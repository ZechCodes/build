//! Concrete factory preparation with explicit readiness denial, OFFSOURCE.
//! This is a partial pipeline, not a Ready implementation or qualification.
use super::backend::{ControlBackend, PipelineMode, PreparationFailure, PreparedBackend};
use super::trial_complete::{ArtifactSink, FixtureFactory, Provenance, TrialBackend, TrialName};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

pub(super) struct ConcreteFactory {
    provenance: Provenance,
    mode: PipelineMode,
    sink: Arc<dyn ArtifactSink>,
    // Cleanup failures remain owned; no automatic next original may start.
    retained: Mutex<Vec<PreparedBackend>>,
}
impl ConcreteFactory {
    pub fn qualification(provenance: Provenance, sink: Arc<dyn ArtifactSink>) -> Self {
        Self::new(provenance, PipelineMode::Qualification, sink)
    }
    pub fn pipeline_control(provenance: Provenance, sink: Arc<dyn ArtifactSink>) -> Self {
        Self::new(provenance, PipelineMode::TwoSecondControl, sink)
    }
    fn new(provenance: Provenance, mode: PipelineMode, sink: Arc<dyn ArtifactSink>) -> Self {
        Self {
            provenance,
            mode,
            sink,
            retained: Mutex::new(Vec::new()),
        }
    }
    pub fn mode(&self) -> PipelineMode {
        self.mode
    }
    pub fn artifacts(&self) -> &dyn ArtifactSink {
        &*self.sink
    }
    pub async fn prepare(&self, name: TrialName) -> Result<PreparedBackend, String> {
        if !self.retained.lock().unwrap().is_empty() {
            return Err(
                "previous concrete setup cleanup unfinished; ownership retained, no new attempt"
                    .into(),
            );
        }
        match PreparedBackend::prepare(name, self.mode, self.provenance.clone()).await {
            Ok(prepared) => Ok(prepared),
            Err(PreparationFailure { reason, retained }) => {
                self.cleanup(retained).await?;
                Err(reason)
            }
        }
    }
    pub async fn cleanup(&self, prepared: PreparedBackend) -> Result<(), String> {
        match prepared.shutdown().await {
            Ok(evidence)
                if evidence.producers_joined
                    && evidence.mcp_quiet
                    && evidence.client_watch_joined
                    && evidence.native_cleanup_completed =>
            {
                Ok(())
            }
            Ok(_) => Err("actual setup cleanup evidence incomplete".into()),
            Err(failure) => {
                let reason = failure.reason;
                self.retained.lock().unwrap().push(failure.retained);
                Err(reason)
            }
        }
    }
}
impl FixtureFactory for ConcreteFactory {
    fn create(
        &self,
        name: TrialName,
    ) -> Pin<Box<dyn Future<Output = Result<Arc<dyn TrialBackend>, String>> + Send + '_>> {
        Box::pin(async move {
            let prepared = self.prepare(name).await?;
            if self.mode == PipelineMode::TwoSecondControl {
                return match ControlBackend::new(prepared, Arc::clone(&self.sink)) {
                    Ok(backend) => Ok(Arc::new(backend) as Arc<dyn TrialBackend>),
                    Err(failure) => {
                        self.cleanup(failure.retained).await?;
                        Err(failure.reason)
                    }
                };
            }
            let reason = prepared.blocked_reason();
            self.cleanup(prepared)
                .await
                .map_err(|cleanup| format!("{reason}; cleanup: {cleanup}"))?;
            // There is no cast/default-success path around the real missing
            // receipt, accepted-version, lifecycle and actual form contracts.
            Err(reason)
        })
    }
}

impl Drop for ConcreteFactory {
    fn drop(&mut self) {
        // A failed bounded join must never delete owned temporary roots while
        // accepted worker/server resources might still use them. The final
        // error already names retained cleanup; supervisor owns process exit.
        let retained = std::mem::take(
            self.retained
                .get_mut()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        for setup in retained {
            std::mem::forget(setup);
        }
    }
}
