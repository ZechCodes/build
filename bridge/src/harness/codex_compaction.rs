//! Codex total-token compaction adapter, shared by both carriers.
use crate::compaction::ResolvedCompactionPolicy;
use crate::harness::compaction::{fallback_policy, AppliedCompactionPolicy, CompactionLaunchIdentity};
use crate::harness::{Harness, HarnessError};
use crate::models::ModelChoice;
use crate::pty::HarnessSpec;
use serde_json::Value;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Translation {
    pub token_limit: u64,
    pub model_window: u64,
    pub trigger: u64,
}

pub fn translate(_version: &str, _tokens: u64, _model_window: u64) -> Result<Translation, &'static str> {
    Err("codex_native_unqualified")
}

pub(super) fn prepare<H: Harness + ?Sized>(
    harness: &H, requested: ResolvedCompactionPolicy, choice: ModelChoice,
    version: Option<String>, identity: CompactionLaunchIdentity,
) -> Result<AppliedCompactionPolicy, HarnessError> {
    fallback_policy(harness, requested, choice, version, identity)
}

pub(super) fn apply(spec: HarnessSpec, _choice: &ModelChoice, _policy: Option<&AppliedCompactionPolicy>) -> Result<HarnessSpec, HarnessError> {
    Ok(spec)
}

pub(super) fn thread_config(_choice: &ModelChoice, _policy: Option<&AppliedCompactionPolicy>) -> Result<Option<Value>, HarnessError> {
    Ok(None)
}
