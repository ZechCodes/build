//! Claude Code's explicit-window adapter shared by ADK and TUI.
//!
//! Build selects Claude's native context window; Claude owns when to compact.
//! Its trigger is approximate because effective output reserves, managed policy,
//! model remapping and native fallback remain provider-owned.
use crate::compaction::ResolvedCompactionPolicy;
use crate::harness::compaction::{
    fallback_policy, AppliedCompactionPolicy, CompactionEnforcement, CompactionLaunchIdentity,
    NativeCompactionSettings,
};
use crate::harness::{Harness, HarnessError};
use crate::models::ModelChoice;
use crate::pty::HarnessSpec;
use serde_json::{json, Value};

#[cfg(test)]
mod children;
mod managed;
#[cfg(test)]
mod tests;

const VERSION: &str = "2.1.284";
const COMPACTION_ENV: [&str; 4] = [
    "CLAUDE_CODE_AUTO_COMPACT_WINDOW",
    "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE",
    "DISABLE_AUTO_COMPACT",
    "DISABLE_COMPACT",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Translation {
    pub window: u64,
    pub percentage: Option<String>,
    pub trigger: u64,
}

/// Translate only *established* model/output budgets. This mathematical result
/// alone is not evidence that a live process uses those budgets.
pub fn translate(
    version: &str,
    tokens: u64,
    model_window: u64,
    max_output: u64,
) -> Result<Translation, &'static str> {
    if version != VERSION {
        return Err("claude_version_unqualified");
    }
    if tokens == 0 || model_window == 0 || max_output == 0 {
        return Err("claude_budget_or_threshold_unknown");
    }
    let reserve = max_output.min(20_000);
    let available = model_window
        .min(1_000_000)
        .checked_sub(reserve + 13_000)
        .ok_or("claude_threshold_exceeds_native_window")?;
    if tokens > available {
        return Err("claude_threshold_exceeds_native_window");
    }
    let window = (tokens + reserve + 13_000).max(100_000);
    let usable = window.min(model_window) - reserve;
    let percentage = if usable - 13_000 == tokens {
        None
    } else {
        // Choose the middle of the floor interval, then round-trip the actual
        // decimal sent to JS. Never round the requested token count itself.
        let pct = format!("{:.12}", (tokens as f64 + 0.5) * 100.0 / usable as f64);
        let parsed = pct
            .parse::<f64>()
            .map_err(|_| "claude_percentage_unrepresentable")?;
        let trigger = ((usable as f64 * parsed / 100.0).floor() as u64).min(usable - 13_000);
        if trigger != tokens || parsed <= 0.0 || parsed > 100.0 {
            return Err("claude_percentage_unrepresentable");
        }
        Some(pct)
    };
    Ok(Translation {
        window,
        percentage,
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
    let window = (100_000..=1_000_000)
        .contains(&policy.requested.tokens)
        .then_some(policy.requested.tokens);
    policy.enforcement = CompactionEnforcement::Native;
    policy.native_settings = Some(NativeCompactionSettings::ClaudeWindow(window));
    policy.capabilities.native_threshold = true;
    policy.capabilities.native_unit = Some("context_window_tokens".into());
    policy.capabilities.native_range = Some(100_000..=1_000_000);
    policy.capabilities.qualification.evidence =
        "Claude native auto-compaction requested; trigger approximately window minus min(maxOutput, 20000) minus 13000; effective model/output and managed policy remain provider-owned".into();
    let reason = managed::launch_conflict()
        .or_else(|| {
            window
                .is_none()
                .then(|| "claude_requested_window_unrepresentable_using_auto".into())
        })
        .or_else(|| Some("claude_native_trigger_approximate_no_live_readback".into()));
    policy.capabilities.unsupported_reason = reason.clone();
    policy.reason = reason;
    Ok(policy)
}

/// Merge only compaction controls into the existing hook settings. Settings.env
/// is required: Claude applies it *after* the process's inherited environment.
pub(super) fn apply(
    mut spec: HarnessSpec,
    choice: &ModelChoice,
    policy: Option<&AppliedCompactionPolicy>,
) -> Result<HarnessSpec, HarnessError> {
    if policy.is_some_and(|policy| &policy.choice != choice) {
        return Err(HarnessError::Setup(
            "Claude compaction policy belongs to another launch choice".into(),
        ));
    }
    let (window, percentage) = native_window(policy)?;
    let values = [
        window.map(|window| window.to_string()).unwrap_or_default(),
        percentage.unwrap_or_default(),
        "0".into(),
        "0".into(),
    ];
    let index = spec
        .args
        .iter()
        .position(|arg| arg == "--settings")
        .ok_or_else(|| HarnessError::Setup("Claude hook settings are missing".into()))?
        + 1;
    let settings = spec
        .args
        .get(index)
        .ok_or_else(|| HarnessError::Setup("Claude hook settings are missing".into()))?;
    let mut settings: Value = serde_json::from_str(settings)
        .map_err(|_| HarnessError::Setup("Claude hook settings are invalid".into()))?;
    merge_settings(&mut settings, window, &values)?;
    spec.args[index] = settings.to_string();
    spec = spec.unset_all(COMPACTION_ENV);
    for (key, value) in COMPACTION_ENV.into_iter().zip(values) {
        spec = spec.env(key, value);
    }
    Ok(spec.arg("--autocompact").arg(
        window
            .map(|window| window.to_string())
            .unwrap_or_else(|| "auto".into()),
    ))
}

fn native_window(
    policy: Option<&AppliedCompactionPolicy>,
) -> Result<(Option<u64>, Option<String>), HarnessError> {
    let Some(policy) = policy else {
        return Ok((None, None));
    };
    if let Some(NativeCompactionSettings::ClaudeWindow(window)) = policy.native_settings {
        let expected = (100_000..=1_000_000)
            .contains(&policy.requested.tokens)
            .then_some(policy.requested.tokens);
        if policy.enforcement != CompactionEnforcement::Native || window != expected {
            return Err(HarnessError::Setup(
                "Claude native window does not match its policy".into(),
            ));
        }
        return Ok((window, None));
    }
    let translation = native_translation(policy)?;
    Ok((
        translation.map(|t| t.window),
        translation.and_then(|t| t.percentage.clone()),
    ))
}

fn native_translation(
    policy: &AppliedCompactionPolicy,
) -> Result<Option<&Translation>, HarnessError> {
    if policy.enforcement != CompactionEnforcement::Native {
        return Ok(None);
    }
    let Some(NativeCompactionSettings::Claude(translation)) = &policy.native_settings else {
        return Err(HarnessError::Setup(
            "Claude native policy has no captured translation".into(),
        ));
    };
    if policy.capabilities.qualification.version.as_deref() != Some(VERSION)
        || policy.translated_trigger != Some(translation.trigger)
        || translation.trigger != policy.requested.tokens
        || !(100_000..=1_000_000).contains(&translation.window)
    {
        return Err(HarnessError::Setup(
            "Claude native translation does not match its policy".into(),
        ));
    }
    Ok(Some(translation))
}

fn merge_settings(
    settings: &mut Value,
    window: Option<u64>,
    values: &[String; 4],
) -> Result<(), HarnessError> {
    let object = settings
        .as_object_mut()
        .ok_or_else(|| HarnessError::Setup("Claude hook settings must be an object".into()))?;
    object.insert("autoCompactEnabled".into(), json!(true));
    object.insert(
        "autoCompactWindow".into(),
        window.map_or(json!("auto"), |window| json!(window)),
    );
    let env = object
        .entry("env")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| HarnessError::Setup("Claude settings env must be an object".into()))?;
    for (key, value) in COMPACTION_ENV.iter().zip(values) {
        env.insert((*key).into(), json!(value));
    }
    Ok(())
}
