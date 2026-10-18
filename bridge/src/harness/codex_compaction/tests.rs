use super::*;
use crate::compaction::CompactionSource;
use crate::harness::compaction::{CompactionEnforcement, NativeCompactionSettings};
use crate::harness::codex::CodexHarness;
use crate::models::AgentProvider;
use serde_json::json;

fn choice() -> ModelChoice {
    ModelChoice { provider: AgentProvider::Codex, model: Some("gpt-5.4".into()), effort: None }
}

fn policy() -> AppliedCompactionPolicy {
    let mut policy = prepare(&CodexHarness, ResolvedCompactionPolicy { tokens: 150_000, source: CompactionSource::Device }, choice(), Some("0.160.0".into()), CompactionLaunchIdentity::new("test-thread")).unwrap();
    policy.enforcement = CompactionEnforcement::Native;
    policy.translated_trigger = Some(150_000);
    policy.native_settings = Some(NativeCompactionSettings::Codex(Translation { token_limit: 150_000, model_window: 1_000_000, trigger: 150_000 }));
    policy
}

#[test]
fn codex_compaction_captures_total_scope_and_only_the_two_compaction_keys() {
    assert_eq!(thread_config(&choice(), Some(&policy())).unwrap(), Some(json!({
        "model_auto_compact_token_limit": 150_000,
        "model_auto_compact_token_limit_scope": "total"
    })));
}

#[test]
fn codex_compaction_rejects_another_choice_and_corrupted_native_translation() {
    let mut p = policy();
    let mut other = choice();
    other.model = Some("unknown".into());
    assert!(thread_config(&other, Some(&p)).is_err());
    p.translated_trigger = Some(99);
    assert!(thread_config(&choice(), Some(&p)).is_err());
    p = policy(); p.native_settings = None;
    assert!(thread_config(&choice(), Some(&p)).is_err());
    p = policy(); p.capabilities.qualification.version = Some("0.161.0".into());
    assert!(thread_config(&choice(), Some(&p)).is_err());
}

#[test]
fn codex_compaction_normalizes_only_child_compaction_overrides() {
    let original = HarnessSpec::new("codex")
        .arg("-c").arg("model_auto_compact_token_limit=3")
        .arg("--config=model_auto_compact_token_limit_scope=\"body_after_prefix\"")
        .arg("--config").arg("model_context_window=123456")
        .arg("--config").arg("unrelated=42");
    let updated = apply(original, &choice(), Some(&policy())).unwrap();
    assert_eq!(updated.args, ["--config", "model_context_window=123456", "--config", "unrelated=42", "--config", "model_auto_compact_token_limit=150000", "--config", "model_auto_compact_token_limit_scope=\"total\""]);
}

#[test]
fn codex_compaction_fallback_does_not_claim_to_disable_native_defaults() {
    let p = prepare(&CodexHarness, ResolvedCompactionPolicy { tokens: 150_000, source: CompactionSource::Device }, choice(), Some("0.160.0".into()), CompactionLaunchIdentity::new("test-thread")).unwrap();
    assert_eq!(p.enforcement, CompactionEnforcement::BetweenTurn);
    assert!(!p.capabilities.native_threshold);
    assert!(p.reason.is_some());
    assert_eq!(thread_config(&choice(), Some(&p)).unwrap(), None);
    let spec = HarnessSpec::new("codex").arg("--config").arg("model_auto_compact_token_limit=42");
    let expected = spec.args.clone();
    assert_eq!(apply(spec, &choice(), Some(&p)).unwrap().args, expected);
}

#[test]
fn codex_compaction_translation_preserves_total_tokens_within_established_model_ceiling() {
    for window in [2, 200_000, 500_000, 1_000_000] {
        let ceiling = window * 9 / 10;
        for tokens in [1, ceiling / 2 + 1, ceiling] {
            let translated = translate("0.160.0", tokens, window).unwrap();
            assert_eq!(translated.token_limit, tokens);
            assert_eq!(translated.trigger, tokens);
            assert_eq!(translated.model_window, window);
        }
        assert!(translate("0.160.0", ceiling + 1, window).is_err());
    }
}

#[test]
fn codex_compaction_translation_rejects_unknown_or_unrepresentable_inputs() {
    for (tokens, ceiling) in [(0, 1_000_000), (1, 0), (u64::MAX, u64::MAX), (1, u64::MAX), (crate::compaction::MAX_SAFE_TOKENS + 1, i64::MAX as u64)] {
        assert!(translate("0.160.0", tokens, ceiling).is_err());
    }
    for version in ["", "0.159.0", "0.160.1", "0.161.0", "codex-cli 0.160.0"] {
        assert!(translate(version, 150_000, 950_000).is_err());
    }
}

#[test]
fn codex_compaction_unqualified_models_and_versions_keep_positive_fallback() {
    for version in [None, Some("0.159.0"), Some("0.160.0"), Some("0.161.0")] {
        for model in [None, Some("gpt-6.1-sol"), Some("gpt-5.4"), Some("unknown-model")] {
            let mut selection = choice(); selection.model = model.map(str::to_owned);
            let p = prepare(&CodexHarness, ResolvedCompactionPolicy { tokens: 200_000, source: CompactionSource::Conversation("agent".into()) }, selection.clone(), version.map(str::to_owned), CompactionLaunchIdentity::new("thread")).unwrap();
            assert_eq!(p.choice, selection);
            assert_eq!(p.enforcement, CompactionEnforcement::BetweenTurn);
            assert!(!p.capabilities.native_threshold);
            assert!(p.native_settings.is_none());
            assert!(p.translated_trigger.is_none());
            assert!(p.reason.as_deref().unwrap().starts_with("codex_"));
            assert_eq!(p.reason, p.capabilities.unsupported_reason);
        }
    }
}
