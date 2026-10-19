//! Pi's version-qualified between-turn contract. Native threshold injection
//! remains unqualified; the extension cancels only native threshold attempts
//! while this fallback owns the policy, preserving overflow recovery.
use std::path::Path;

use crate::compaction::{ResolvedCompactionPolicy, MAX_SAFE_TOKENS};
use crate::harness::compaction::{
    fallback_policy, AppliedCompactionPolicy, CompactionEnforcement, CompactionLaunchIdentity,
};
use crate::harness::pi_telemetry::PiTelemetryConfig;
use crate::harness::{Harness, HarnessError};
use crate::models::{AgentProvider, ModelChoice};
use crate::orchestrator::SpawnOptions;
use crate::pty::HarnessSpec;
use serde_json::json;

#[cfg(test)]
mod tests;

const VERSION: &str = "0.86.1";
const POLICY_ENV: &str = "BUILD_PI_COMPACTION_POLICY";
const TELEMETRY_ENV: &str = "BUILD_PI_TELEMETRY_LOG";

pub(super) fn prepare<H: Harness + ?Sized>(
    harness: &H,
    requested: ResolvedCompactionPolicy,
    choice: ModelChoice,
    version: Option<String>,
    identity: CompactionLaunchIdentity,
) -> Result<AppliedCompactionPolicy, HarnessError> {
    let mut policy = fallback_policy(harness, requested, choice, version, identity)?;
    let qualified = policy.capabilities.qualification.version.as_deref() == Some(VERSION);
    policy.capabilities.between_turn_compact = qualified;
    policy.capabilities.qualification.evidence =
        "Pi 0.86.1 extension hooks and agent_settled contract; native per-launch threshold injection unqualified".into();
    if policy.enforcement == CompactionEnforcement::Unsupported {
        policy.capabilities.unsupported_reason = policy.reason.clone();
        return Ok(policy);
    }
    let reason = if qualified {
        "pi_native_threshold_injection_unqualified"
    } else {
        policy.enforcement = CompactionEnforcement::Unsupported;
        "pi_version_unqualified"
    };
    policy.reason = Some(reason.into());
    policy.capabilities.unsupported_reason = Some(reason.into());
    Ok(policy)
}

/// Check the captured launch before materializing any private extension or
/// session files. A policy cannot be repurposed for a different model/provider.
pub(super) fn validate(choice: &ModelChoice, options: &SpawnOptions) -> Result<(), HarnessError> {
    if choice.provider != AgentProvider::Pi || !super::is_a_filename(&options.owner_id) {
        return Err(HarnessError::Setup(
            "invalid Pi launch choice or owner".into(),
        ));
    }
    let Some(policy) = &options.compaction_policy else {
        return Ok(());
    };
    if &policy.choice != choice
        || !super::is_a_filename(&policy.launch_identity.id)
        || !valid_thread_id(&policy.launch_identity.thread_id)
    {
        return Err(HarnessError::Setup(
            "Pi compaction policy belongs to another launch choice or has invalid identity".into(),
        ));
    }
    let expected = prepare(
        &super::pi::PiHarness,
        policy.requested.clone(),
        choice.clone(),
        policy.capabilities.qualification.version.clone(),
        policy.launch_identity.clone(),
    )?;
    if policy != &expected {
        return Err(HarnessError::Setup(
            "Pi compaction policy does not match its captured qualification".into(),
        ));
    }
    Ok(())
}

// Canonical threads use `thread:<owner>` and may append a reset generation.
// This identity is an opaque telemetry value, never a path component.
fn valid_thread_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 1024 && !id.chars().any(char::is_control)
}

/// Child-only policy transport. Every opening gets an independent sidecar;
/// neither user settings nor the checkout supply mutable compaction policy.
pub(super) fn apply(spec: HarnessSpec, options: &SpawnOptions, session_dir: &Path) -> HarnessSpec {
    let mut spec = spec.unset_all([POLICY_ENV, TELEMETRY_ENV]);
    let Some(policy) = &options.compaction_policy else {
        return spec;
    };
    // Unknown hooks cannot supply authoritative idle boundaries. Keep their
    // existing terminal lifecycle while the outer session reports unsupported.
    if policy.capabilities.qualification.version.as_deref() != Some(VERSION) {
        return spec;
    }
    let mut encoded = json!({
        "tokens": (policy.requested.tokens <= MAX_SAFE_TOKENS).then_some(policy.requested.tokens),
        "owner": options.owner_id,
        "thread_id": policy.launch_identity.thread_id,
        "launch_id": policy.launch_identity.id,
        "enabled": policy.enforcement == CompactionEnforcement::BetweenTurn,
    });
    if policy.requested.tokens > MAX_SAFE_TOKENS {
        encoded["tokens_decimal"] = json!(policy.requested.tokens.to_string());
    }
    let path = session_dir.join(format!("telemetry-{}.jsonl", uuid::Uuid::new_v4()));
    spec = spec
        .env(POLICY_ENV, encoded.to_string())
        .env(TELEMETRY_ENV, path.to_string_lossy().into_owned());
    spec.pi_telemetry = Some(Box::new(PiTelemetryConfig {
        path,
        owner: options.owner_id.clone(),
        policy: policy.clone(),
    }));
    spec
}
