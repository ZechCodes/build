//! Scoped Pi extension observations, separate from terminal painting.
use super::compaction::AppliedCompactionPolicy;
use std::path::PathBuf;
use super::{AgentStatus, SessionStatusSnapshot};
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct PiTelemetryConfig {
    pub path: PathBuf,
    pub owner: String,
    pub policy: AppliedCompactionPolicy,
}

#[cfg(test)]
mod tests;

pub(crate) struct PiTelemetry {
    config: PiTelemetryConfig,
}

impl PiTelemetry {
    pub(crate) fn new(config: PiTelemetryConfig) -> Self { Self { config } }
    pub(crate) fn observe(&self, _event: &Value) {}
    pub(crate) fn snapshot(&self) -> SessionStatusSnapshot {
        SessionStatusSnapshot::new(AgentStatus::Starting)
    }
    pub(crate) fn policy(&self) -> &AppliedCompactionPolicy { &self.config.policy }
    pub(crate) fn sent(&self, _compaction: bool) -> Result<(), super::HarnessError> { Ok(()) }
    pub(crate) fn automatic_due(&self, _tokens: Option<u64>) -> bool { false }
}
