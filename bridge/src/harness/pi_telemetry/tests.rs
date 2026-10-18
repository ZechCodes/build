use super::*;
use crate::compaction::{CompactionSource, ResolvedCompactionPolicy};
use crate::harness::compaction::{CompactionEnforcement, CompactionLaunchIdentity};
use crate::harness::{Harness, pi::PiHarness};
use crate::models::{AgentProvider, ModelChoice};
use serde_json::json;

fn telemetry() -> PiTelemetry {
    let mut policy = PiHarness.prepare_compaction_policy(
        ResolvedCompactionPolicy { tokens: 100, source: CompactionSource::Device },
        ModelChoice { provider: AgentProvider::Pi, ..ModelChoice::default() },
        Some("0.86.1".into()),
        CompactionLaunchIdentity { id: "launch-one".into(), thread_id: "thread-one".into() },
    ).unwrap();
    policy.enforcement = CompactionEnforcement::BetweenTurn;
    PiTelemetry::new(PiTelemetryConfig { path: "/unused".into(), owner: "agent-one".into(), policy })
}

fn event(sequence: u64, kind: &str) -> Value {
    json!({"sequence": sequence, "launch_id":"launch-one", "thread_id":"thread-one", "owner":"agent-one", "type":kind})
}

fn context(sequence: u64, tokens: Option<u64>) -> Value {
    let mut value = event(sequence, "context");
    value["tokens"] = json!(tokens);
    value
}

#[test]
fn pi_long_turn_crosses_threshold_but_only_settled_allows_fallback() {
    let telemetry = telemetry();
    telemetry.observe(&event(1, "ready"));
    telemetry.sent(false).unwrap();
    telemetry.observe(&event(2, "working"));
    telemetry.observe(&context(3, Some(101)));
    for (sequence, kind) in [(4, "turn_end"), (5, "agent_end")] {
        telemetry.observe(&event(sequence, kind));
        assert_eq!(telemetry.snapshot().status, AgentStatus::Working);
        assert!(!telemetry.automatic_due(Some(101)));
        assert!(telemetry.sent(true).is_err(), "manual compact must not abort active tools");
    }
    telemetry.observe(&event(6, "settled"));
    assert_eq!(telemetry.snapshot().status, AgentStatus::Waiting);
    assert!(telemetry.automatic_due(Some(101)));
    telemetry.sent(true).unwrap();
    assert!(!telemetry.automatic_due(Some(101)), "one request owns the idle boundary");
}

#[test]
fn pi_null_reading_invalidates_old_context_and_identical_fresh_usage_is_new() {
    let telemetry = telemetry();
    telemetry.observe(&event(1, "ready"));
    telemetry.observe(&context(2, Some(101)));
    telemetry.observe(&event(3, "settled"));
    assert!(telemetry.automatic_due(Some(101)));
    telemetry.observe(&context(4, None));
    assert_eq!(telemetry.snapshot().context, None);
    assert!(!telemetry.automatic_due(Some(101)));
    telemetry.observe(&context(5, Some(101)));
    assert_eq!(telemetry.snapshot().context.unwrap().context_tokens, 101);
    assert!(telemetry.automatic_due(Some(101)));
}

#[test]
fn pi_extension_readiness_and_failure_are_explicit_unsupported_states() {
    let telemetry = telemetry();
    assert_eq!(telemetry.policy().enforcement, CompactionEnforcement::Unsupported);
    assert_eq!(telemetry.policy().reason.as_deref(), Some("pi_extension_unconfirmed"));
    telemetry.observe(&event(1, "ready"));
    assert_eq!(telemetry.policy().enforcement, CompactionEnforcement::BetweenTurn);
    telemetry.observe(&event(2, "unsupported"));
    assert_eq!(telemetry.policy().enforcement, CompactionEnforcement::Unsupported);
    assert_eq!(telemetry.policy().reason.as_deref(), Some("pi_extension_unavailable"));
    telemetry.observe(&event(3, "ready"));
    assert_eq!(telemetry.policy().enforcement, CompactionEnforcement::Unsupported);
}

#[test]
fn pi_foreign_and_replayed_observations_never_unlock_another_turn() {
    let telemetry = telemetry();
    telemetry.observe(&event(1, "ready"));
    telemetry.observe(&event(2, "settled"));
    telemetry.sent(false).unwrap();
    telemetry.observe(&event(2, "settled"));
    for field in ["owner", "thread_id", "launch_id"] {
        let mut foreign = event(3, "settled");
        foreign[field] = json!("foreign");
        telemetry.observe(&foreign);
    }
    assert_eq!(telemetry.snapshot().status, AgentStatus::Working);
    telemetry.observe(&event(3, "settled"));
    assert_eq!(telemetry.snapshot().status, AgentStatus::Waiting);
}
