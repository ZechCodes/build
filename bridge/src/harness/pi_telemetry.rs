//! Scoped Pi extension observations, separate from terminal painting.
use super::compaction::AppliedCompactionPolicy;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct PiTelemetryConfig {
    pub path: PathBuf,
    pub owner: String,
    pub policy: AppliedCompactionPolicy,
}
