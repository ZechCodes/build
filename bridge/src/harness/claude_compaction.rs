//! Claude Code's explicit-window adapter shared by ADK and TUI.
//!
//! Formula revision 1 is inspected against 2.1.284. Production native promotion
//! remains closed: launch version/model do not bound effective output settings,
//! managed policy refresh or native in-turn model fallback. See the qualification
//! record in planning/v2/Claude Compaction Qualification.md.
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
    if policy.enforcement == CompactionEnforcement::Unsupported {
        return Ok(policy);
    }
    let reason = managed::launch_conflict().unwrap_or_else(|| unqualified_reason(&policy).into());
    policy.capabilities.qualification.evidence =
        "Claude formula revision 1 (2.1.284); isolated resumed compaction observed; effective launch budgets and model fallback unverified".into();
    policy.capabilities.unsupported_reason = Some(reason.clone());
    policy.reason = Some(reason);
    Ok(policy)
}

fn unqualified_reason(policy: &AppliedCompactionPolicy) -> &'static str {
    if policy.capabilities.qualification.version.as_deref() != Some(VERSION) {
        return "claude_version_unqualified";
    }
    // Even an explicit model can be remapped by managed restrictions or native
    // fallback. Do not infer a reserve from the catalog's advertised context.
    if policy.choice.model.is_none() {
        return "claude_model_and_output_budgets_unverified";
    }
    "claude_effective_output_budget_and_model_fallback_unverified"
}

/// Merge only compaction controls into the existing hook settings. Settings.env
/// is required: Claude applies it *after* the process's inherited environment.
pub(super) fn apply(
    mut spec: HarnessSpec,
    choice: &ModelChoice,
    policy: Option<&AppliedCompactionPolicy>,
) -> Result<HarnessSpec, HarnessError> {
    let Some(policy) = policy else {
        return Ok(spec);
    };
    if &policy.choice != choice {
        return Err(HarnessError::Setup(
            "Claude compaction policy belongs to another launch choice".into(),
        ));
    }
    let translation = native_translation(policy)?;
    let values = [
        translation
            .map(|t| t.window.to_string())
            .unwrap_or_default(),
        translation
            .and_then(|t| t.percentage.clone())
            .unwrap_or_default(),
        if translation.is_some() { "0" } else { "1" }.into(),
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
    merge_settings(&mut settings, translation, &values)?;
    spec.args[index] = settings.to_string();
    spec = spec.unset_all(COMPACTION_ENV);
    for (key, value) in COMPACTION_ENV.into_iter().zip(values) {
        spec = spec.env(key, value);
    }
    if let Some(translation) = translation {
        spec = spec
            .arg("--autocompact")
            .arg(translation.window.to_string());
    }
    Ok(spec)
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
    translation: Option<&Translation>,
    values: &[String; 4],
) -> Result<(), HarnessError> {
    let object = settings
        .as_object_mut()
        .ok_or_else(|| HarnessError::Setup("Claude hook settings must be an object".into()))?;
    object.insert("autoCompactEnabled".into(), json!(translation.is_some()));
    object.insert(
        "autoCompactWindow".into(),
        translation.map_or(json!("auto"), |t| json!(t.window)),
    );
    object.insert("precomputeCompactionEnabled".into(), json!(false));
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
