//! Codex's total-token adapter shared by app-server and TUI.
//!
//! Translation revision 1 targets 0.160.0. A mathematical translation requires
//! an established effective raw model window and zero fallback buffer.
//! Exact threshold translation remains unqualified; provider native defaults
//! own automatic compaction without a Build between-turn fallback.
use crate::compaction::{ResolvedCompactionPolicy, MAX_SAFE_TOKENS};
use crate::harness::compaction::{
    fallback_policy, AppliedCompactionPolicy, CompactionEnforcement, CompactionLaunchIdentity,
    NativeCompactionSettings,
};
use crate::harness::{Harness, HarnessError};
use crate::models::{AgentProvider, ModelChoice};
use crate::pty::HarnessSpec;
use serde_json::{json, Value};

#[cfg(test)]
mod tests;

const VERSION: &str = "0.160.0";
const LIMIT_KEY: &str = "model_auto_compact_token_limit";
const SCOPE_KEY: &str = "model_auto_compact_token_limit_scope";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Translation {
    pub token_limit: u64,
    /// Established raw window for the effective model/configuration. Catalog
    /// defaults alone do not prove the window used by a live launch.
    pub model_window: u64,
    pub trigger: u64,
}

/// Revision 1: N = T, with explicit total-token scope. The caller must establish
/// the effective raw model window and zero fallback buffer independently.
/// Codex clamps N to floor(window * 9/10), distinct from its usable window.
/// The independently established usable-context cap must also be at least T,
/// and post-turn settings must not introduce an earlier trigger.
/// This arithmetic does not qualify a model.
/// Refuse an out-of-range request rather than silently clamping its intent.
pub fn translate(
    version: &str,
    tokens: u64,
    model_window: u64,
) -> Result<Translation, &'static str> {
    if version != VERSION {
        return Err("codex_version_unqualified");
    }
    if tokens == 0 || tokens > MAX_SAFE_TOKENS {
        return Err("codex_threshold_unrepresentable");
    }
    if model_window == 0 || model_window > (i64::MAX / 9) as u64 {
        return Err("codex_model_window_unqualified");
    }
    let ceiling = (u128::from(model_window) * 9 / 10) as u64;
    if tokens > ceiling {
        return Err("codex_threshold_exceeds_native_ceiling");
    }
    Ok(Translation {
        token_limit: tokens,
        model_window,
        trigger: tokens,
    })
}

pub(super) fn prepare<H: Harness + ?Sized>(
    harness: &H,
    requested: ResolvedCompactionPolicy,
    choice: ModelChoice,
    version: Option<String>,
    identity: CompactionLaunchIdentity,
) -> Result<AppliedCompactionPolicy, HarnessError> {
    let mut policy = fallback_policy(harness, requested, choice, version, identity)?;
    if policy.enforcement == CompactionEnforcement::Unsupported {
        return Ok(policy);
    }
    policy.enforcement = CompactionEnforcement::Native;
    let reason = if policy.capabilities.qualification.version.as_deref() != Some(VERSION) {
        "codex_version_unqualified"
    } else {
        "codex_model_window_buffer_and_profile_unqualified"
    };
    // No launch currently captures a qualified effective native ceiling. The
    // provider's mutable model catalog, selected-profile absence and config
    // precedence cannot be inferred from a version/model string. Re-evaluate on
    // each opening, never warm reuse. Do not read user config to guess support.
    policy.capabilities.qualification.evidence =
        "Codex total-token translation revision 1 (0.160.0); isolated compaction observed; effective model window, fallback buffer and selected-profile precedence unqualified".into();
    policy.capabilities.unsupported_reason = Some(reason.into());
    policy.reason = Some(reason.into());
    Ok(policy)
}

/// The exact two values shared by the child's argv and start/resume request.
/// Fallback does not disable or rewrite Codex's independent native defaults.
pub(super) fn thread_config(
    choice: &ModelChoice,
    policy: Option<&AppliedCompactionPolicy>,
) -> Result<Option<Value>, HarnessError> {
    let Some(policy) = policy else {
        return Ok(None);
    };
    if &policy.choice != choice
        || !matches!(
            choice.provider,
            AgentProvider::Codex | AgentProvider::CodexAppServer
        )
    {
        return Err(HarnessError::Setup(
            "Codex compaction policy belongs to another launch choice".into(),
        ));
    }
    if policy.enforcement != CompactionEnforcement::Native {
        return Ok(None);
    }
    // No translation means the provider owns its native defaults.
    if policy.native_settings.is_none() && policy.translated_trigger.is_none() {
        return Ok(None);
    }
    let translation = native_translation(policy)?;
    Ok(Some(json!({
        LIMIT_KEY: translation.token_limit,
        SCOPE_KEY: "total",
    })))
}

fn native_translation(policy: &AppliedCompactionPolicy) -> Result<&Translation, HarnessError> {
    let Some(NativeCompactionSettings::Codex(translation)) = &policy.native_settings else {
        return Err(HarnessError::Setup(
            "Codex native policy has no captured translation".into(),
        ));
    };
    let expected = translate(
        policy
            .capabilities
            .qualification
            .version
            .as_deref()
            .unwrap_or_default(),
        policy.requested.tokens,
        translation.model_window,
    )
    .map_err(|reason| HarnessError::Setup(reason.into()))?;
    if translation != &expected || policy.translated_trigger != Some(expected.trigger) {
        return Err(HarnessError::Setup(
            "Codex native translation does not match its policy".into(),
        ));
    }
    Ok(translation)
}

pub(super) fn apply(
    mut spec: HarnessSpec,
    choice: &ModelChoice,
    policy: Option<&AppliedCompactionPolicy>,
) -> Result<HarnessSpec, HarnessError> {
    let Some(config) = thread_config(choice, policy)? else {
        return Ok(spec);
    };
    spec.args = without_compaction_overrides(spec.args);
    for key in [LIMIT_KEY, SCOPE_KEY] {
        spec = spec.arg("--config").arg(format!("{key}={}", config[key]));
    }
    Ok(spec)
}

fn without_compaction_overrides(args: Vec<String>) -> Vec<String> {
    let mut retained = Vec::with_capacity(args.len());
    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        if matches!(arg.as_str(), "-c" | "--config")
            && args.peek().is_some_and(|value| compaction_key(value))
        {
            args.next();
            continue;
        }
        let inline = arg
            .strip_prefix("--config=")
            .or_else(|| arg.strip_prefix("-c="))
            .or_else(|| arg.strip_prefix("-c"));
        if !inline.is_some_and(compaction_key) {
            retained.push(arg);
        }
    }
    retained
}

fn compaction_key(value: &str) -> bool {
    let Some((key, _)) = value.split_once('=') else {
        return false;
    };
    let key = key.trim();
    let key = key
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| key.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(key);
    matches!(key, LIMIT_KEY | SCOPE_KEY)
}
