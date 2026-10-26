use super::*;
use crate::compaction::{CompactionSource, ResolvedCompactionPolicy};
use crate::harness::compaction::CompactionLaunchIdentity;
use crate::harness::{harness_for, HarnessContext};
use crate::models::{AgentProvider, ModelChoice};
use crate::orchestrator::SpawnOptions;
use serde_json::Value;

fn round_trip(t: &Translation, model_window: u64, max_output: u64) -> u64 {
    let usable = t.window.min(model_window) - max_output.min(20_000);
    let ceiling = usable - 13_000;
    t.percentage.as_ref().map_or(ceiling, |pct| {
        ((usable as f64 * pct.parse::<f64>().unwrap() / 100.).floor() as u64).min(ceiling)
    })
}

#[test]
fn claude_compaction_translation_uses_the_real_output_reserve() {
    for (tokens, output, window) in [
        (150_000, 20_000, 183_000),
        (200_000, 64_000, 233_000),
        (150_000, 8192, 171_192),
    ] {
        let t = translate("2.1.284", tokens, 1_000_000, output).unwrap();
        assert_eq!(t.window, window);
        assert_eq!(t.percentage, None);
        assert_eq!(t.trigger, tokens);
        assert_eq!(round_trip(&t, 1_000_000, output), tokens);
    }
}

#[test]
fn claude_compaction_small_thresholds_round_trip_without_rounding_intent() {
    for output in [1, 8192, 20_000, 64_000] {
        for tokens in 1..=100_000 {
            let t = translate("2.1.284", tokens, 200_000, output).unwrap();
            assert!((100_000..=1_000_000).contains(&t.window));
            assert_eq!(round_trip(&t, 200_000, output), tokens, "{t:?}");
            assert_eq!(t.trigger, tokens);
        }
    }
}

#[test]
fn claude_compaction_bounds_and_versions_never_silently_clamp() {
    assert!(translate("2.1.284", 167_000, 200_000, 20_000).is_ok());
    for tokens in [0, 167_001, 200_000, u64::MAX] {
        assert!(translate("2.1.284", tokens, 200_000, 20_000).is_err());
    }
    assert!(translate("2.1.284", 967_000, 1_000_000, 20_000).is_ok());
    assert!(translate("2.1.284", 967_001, 1_000_000, 20_000).is_err());
    assert!(translate("2.1.284", 1, 0, 20_000).is_err());
    assert!(translate("2.1.284", 1, 200_000, 0).is_err());
    for version in ["", "2.1.283", "2.1.285", "999.0.0"] {
        assert!(translate(version, 150_000, 1_000_000, 20_000).is_err());
    }
}

pub(super) fn choice(provider: AgentProvider) -> ModelChoice {
    ModelChoice {
        provider,
        model: Some("claude-opus-5[1m]".into()),
        effort: None,
    }
}

#[test]
fn claude_compaction_native_settings_override_inherited_policy_preserve_hooks_and_resume() {
    let root = tempfile::tempdir().unwrap();
    let context = HarnessContext {
        channel: crate::config::Channel::Prod,
        bridge_exe: "/fake/bridge".into(),
        mcp_socket: root.path().join("socket"),
        state_root: root.path().join("state"),
    };
    for provider in [AgentProvider::Claude, AgentProvider::ClaudeAdk] {
        let harness = harness_for(provider);
        for resume in [None, Some("exact"), Some("continue")] {
            let choice = choice(provider);
            let policy = harness
                .prepare_compaction_policy(
                    ResolvedCompactionPolicy {
                        tokens: 150_000,
                        source: CompactionSource::Device,
                    },
                    choice.clone(),
                    Some("2.1.284".into()),
                    CompactionLaunchIdentity::new("thread"),
                )
                .unwrap();
            let options = SpawnOptions {
                compaction_policy: Some(policy),
                resume_session_id: resume.filter(|r| *r == "exact").map(str::to_owned),
                continue_session: resume.is_some(),
                ..SpawnOptions::default()
            };
            let spec = harness.spec(&choice, &options, &context).unwrap();
            let i = spec.args.iter().position(|a| a == "--settings").unwrap();
            let settings: Value = serde_json::from_str(&spec.args[i + 1]).unwrap();
            assert_eq!(settings["env"]["DISABLE_AUTO_COMPACT"], "0");
            assert_eq!(settings["env"]["DISABLE_COMPACT"], "0");
            assert_eq!(settings["env"]["CLAUDE_AUTOCOMPACT_PCT_OVERRIDE"], "");
            assert_eq!(settings["env"]["CLAUDE_CODE_AUTO_COMPACT_WINDOW"], "150000");
            assert_eq!(settings["autoCompactEnabled"], true);
            assert!(settings.get("precomputeCompactionEnabled").is_none());
            assert!(settings["hooks"]["PreCompact"].is_array());
            assert!(settings["env"]
                .get("CLAUDE_CODE_MAX_OUTPUT_TOKENS")
                .is_none());
            assert_eq!(
                spec.args.iter().any(|a| a == "--resume"),
                resume == Some("exact")
            );
            assert_eq!(
                spec.args.iter().any(|a| a == "--continue"),
                resume == Some("continue")
            );
        }
    }
    assert!(
        !context.state_root.exists(),
        "spec construction never writes settings or state"
    );
}

pub(super) fn native_policy(
    provider: AgentProvider,
    translation: Translation,
) -> AppliedCompactionPolicy {
    let mut policy = fallback_policy(
        harness_for(provider),
        ResolvedCompactionPolicy {
            tokens: translation.trigger,
            source: CompactionSource::Device,
        },
        choice(provider),
        Some("2.1.284".into()),
        CompactionLaunchIdentity::new("fixture"),
    )
    .unwrap();
    policy.enforcement = CompactionEnforcement::Native;
    policy.translated_trigger = Some(translation.trigger);
    policy.native_settings = Some(NativeCompactionSettings::Claude(translation));
    policy.capabilities.native_threshold = true;
    policy
}

#[test]
fn claude_compaction_merge_preserves_unrelated_settings_and_hooks() {
    let original = json!({"hooks":{"PreCompact":[{"sentinel":true}]},"env":{"OTHER":"keep","CLAUDE_CODE_MAX_OUTPUT_TOKENS":"8192","DISABLE_COMPACT":"1"},"otherSetting":42,"precomputeCompactionEnabled":true});
    let mut settings = original.clone();
    let values = ["183000".into(), "".into(), "0".into(), "0".into()];
    merge_settings(&mut settings, Some(183_000), &values).unwrap();
    assert_eq!(settings["hooks"], original["hooks"]);
    assert_eq!(settings["env"]["OTHER"], "keep");
    assert_eq!(settings["env"]["CLAUDE_CODE_MAX_OUTPUT_TOKENS"], "8192");
    assert_eq!(settings["otherSetting"], 42);
    assert_eq!(settings["precomputeCompactionEnabled"], true);
    assert_eq!(settings["env"]["DISABLE_COMPACT"], "0");
}

#[test]
fn claude_compaction_production_uses_native_window_without_exact_trigger_claim() {
    for provider in [AgentProvider::Claude, AgentProvider::ClaudeAdk] {
        for version in [None, Some("2.1.284"), Some("2.1.285")] {
            for tokens in [
                1,
                99_999,
                100_000,
                150_000,
                200_000,
                1_000_000,
                1_000_001,
                u64::MAX,
            ] {
                let policy = harness_for(provider)
                    .prepare_compaction_policy(
                        ResolvedCompactionPolicy {
                            tokens,
                            source: CompactionSource::Device,
                        },
                        choice(provider),
                        version.map(str::to_owned),
                        CompactionLaunchIdentity::new("native-window"),
                    )
                    .unwrap();
                assert_eq!(policy.requested.tokens, tokens);
                assert_eq!(policy.enforcement, CompactionEnforcement::Native);
                assert!(policy.capabilities.native_threshold);
                assert!(
                    policy.translated_trigger.is_none(),
                    "effective trigger is not exact"
                );
                let window = (100_000..=1_000_000).contains(&tokens).then_some(tokens);
                assert_eq!(
                    policy.native_settings,
                    Some(NativeCompactionSettings::ClaudeWindow(window))
                );
                assert!(
                    !policy.automatic_due(Some(u64::MAX)),
                    "Build must never double-compact Claude"
                );
            }
        }
    }
}

#[test]
fn claude_compaction_refuses_mismatched_native_policy_or_choice() {
    let translation = translate("2.1.284", 150_000, 1_000_000, 20_000).unwrap();
    let mut policy = native_policy(AgentProvider::Claude, translation);
    policy.translated_trigger = Some(200_000);
    assert!(native_translation(&policy).is_err());
    policy.translated_trigger = Some(150_000);
    policy.capabilities.qualification.version = Some("2.1.285".into());
    assert!(native_translation(&policy).is_err());
    assert!(apply(
        HarnessSpec::new("claude"),
        &choice(AgentProvider::ClaudeAdk),
        Some(&policy)
    )
    .is_err());
    policy.native_settings = None;
    assert!(native_translation(&policy).is_err());
}

#[test]
fn managed_disable_selects_fallback_but_window_override_keeps_native() {
    for (disabled, command_disabled, expected) in [
        (false, false, CompactionEnforcement::Native),
        (true, false, CompactionEnforcement::BetweenTurn),
        (true, true, CompactionEnforcement::BetweenTurn),
    ] {
        let choice = choice(AgentProvider::ClaudeAdk);
        let harness = harness_for(choice.provider);
        let policy = prepare_observed(
            harness,
            ResolvedCompactionPolicy {
                tokens: 150_000,
                source: CompactionSource::Device,
            },
            choice,
            Some("2.1.284".into()),
            CompactionLaunchIdentity::new("thread"),
            managed::Observation {
                reason: Some("managed policy present: test key".into()),
                disables_auto: disabled,
                disables_command: command_disabled,
            },
        )
        .unwrap();
        assert_eq!(policy.enforcement, expected);
        assert_eq!(policy.capabilities.between_turn_compact, !command_disabled);
        assert_eq!(policy.automatic_due(Some(150_000)), disabled);
        assert_eq!(policy.requested.tokens, 150_000);
        assert!(policy.reason.as_deref().unwrap().contains("managed policy"));
    }
}
