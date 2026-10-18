use super::*;
use crate::compaction::CompactionSource;
use crate::harness::harness_for;
use crate::models::AgentProvider;

fn applied(provider: AgentProvider, tokens: u64, version: Option<&str>) -> AppliedCompactionPolicy {
    harness_for(provider).prepare_compaction_policy(
        ResolvedCompactionPolicy { tokens, source: CompactionSource::Device },
        ModelChoice { provider, ..ModelChoice::default() },
        version.map(str::to_owned),
        CompactionLaunchIdentity::new("generation"),
    ).unwrap()
}

#[test]
fn all_unqualified_versions_and_values_declare_fallback_or_unsupported() {
    for provider in AgentProvider::ALL {
        for version in [None, Some("0.0.1"), Some("999.0.0"), Some("unreadable")] {
            let policy = applied(provider, 150_000, version);
            assert!(!policy.capabilities.native_threshold);
            assert_eq!(policy.capabilities.qualification.version.as_deref(), version);
            assert!(!policy.capabilities.qualification.evidence.is_empty());
            assert!(policy.reason.is_some());
            assert_eq!(policy.translated_trigger, None);
            let supported = harness_for(provider).compacts_on_command();
            assert_eq!(policy.capabilities.between_turn_compact, supported);
            assert_eq!(policy.enforcement, if supported { CompactionEnforcement::BetweenTurn } else { CompactionEnforcement::Unsupported });
        }
    }
}

#[test]
fn captured_fallback_is_inclusive_and_unknown_or_native_never_schedules_it() {
    let mut policy = applied(AgentProvider::CodexAppServer, 150_000, None);
    assert!(!policy.automatic_due(None));
    assert!(!policy.automatic_due(Some(149_999)));
    assert!(policy.automatic_due(Some(150_000)));
    assert!(policy.automatic_due(Some(150_001)));
    policy.enforcement = CompactionEnforcement::Native;
    assert!(!policy.automatic_due(Some(u64::MAX)));
    policy.enforcement = CompactionEnforcement::Unsupported;
    assert!(!policy.automatic_due(Some(u64::MAX)));
}

#[test]
fn zero_is_refused_and_unsafe_legacy_values_remain_exact_but_unsupported() {
    assert!(harness_for(AgentProvider::Claude).prepare_compaction_policy(
        ResolvedCompactionPolicy { tokens: 0, source: CompactionSource::Device },
        ModelChoice::default(), None, CompactionLaunchIdentity::new("generation"),
    ).is_err());
    let policy = applied(AgentProvider::Claude, u64::MAX, None);
    assert_eq!(policy.requested.tokens, u64::MAX);
    assert_eq!(policy.enforcement, CompactionEnforcement::Unsupported);
    assert_eq!(policy.reason.as_deref(), Some("unsupported_representation"));
    assert!(!policy.automatic_due(Some(u64::MAX)));
}
