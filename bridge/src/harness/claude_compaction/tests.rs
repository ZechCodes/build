use super::*;
use crate::compaction::{CompactionSource, ResolvedCompactionPolicy};
use crate::harness::{harness_for, HarnessContext};
use crate::harness::compaction::CompactionLaunchIdentity;
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
    for (tokens, output, window) in [(150_000, 20_000, 183_000), (200_000, 64_000, 233_000), (150_000, 8192, 171_192)] {
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
    ModelChoice { provider, model: Some("claude-haiku-4-5".into()), effort: None }
}

#[test]
fn claude_compaction_fallback_settings_override_inherited_policy_preserve_hooks_and_resume() {
    let root = tempfile::tempdir().unwrap();
    let context = HarnessContext { bridge_exe: "/fake/bridge".into(), mcp_socket: root.path().join("socket"), state_root: root.path().join("state") };
    for provider in [AgentProvider::Claude, AgentProvider::ClaudeAdk] {
        let harness = harness_for(provider);
        for resume in [None, Some("exact"), Some("continue")] {
            let choice = choice(provider);
            let policy = harness.prepare_compaction_policy(ResolvedCompactionPolicy { tokens: 150_000, source: CompactionSource::Device }, choice.clone(), Some("2.1.284".into()), CompactionLaunchIdentity::new("thread")).unwrap();
            let options = SpawnOptions { compaction_policy: Some(policy), resume_session_id: resume.filter(|r| *r == "exact").map(str::to_owned), continue_session: resume.is_some(), ..SpawnOptions::default() };
            let spec = harness.spec(&choice, &options, &context).unwrap();
            let i = spec.args.iter().position(|a| a == "--settings").unwrap();
            let settings: Value = serde_json::from_str(&spec.args[i+1]).unwrap();
            assert_eq!(settings["env"]["DISABLE_AUTO_COMPACT"], "1");
            assert_eq!(settings["env"]["DISABLE_COMPACT"], "0");
            assert_eq!(settings["env"]["CLAUDE_AUTOCOMPACT_PCT_OVERRIDE"], "");
            assert_eq!(settings["env"]["CLAUDE_CODE_AUTO_COMPACT_WINDOW"], "");
            assert_eq!(settings["autoCompactEnabled"], false);
            assert!(settings["hooks"]["PreCompact"].is_array());
            assert!(settings["env"].get("CLAUDE_CODE_MAX_OUTPUT_TOKENS").is_none());
            assert_eq!(spec.args.iter().any(|a| a == "--resume"), resume == Some("exact"));
            assert_eq!(spec.args.iter().any(|a| a == "--continue"), resume == Some("continue"));
        }
    }
    assert!(!context.state_root.exists(), "spec construction never writes settings or state");
}
