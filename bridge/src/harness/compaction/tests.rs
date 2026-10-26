use super::*;
use crate::compaction::CompactionSource;
use crate::harness::harness_for;
use crate::models::AgentProvider;

fn applied(provider: AgentProvider, tokens: u64, version: Option<&str>) -> AppliedCompactionPolicy {
    harness_for(provider)
        .prepare_compaction_policy(
            ResolvedCompactionPolicy {
                tokens,
                source: CompactionSource::Device,
            },
            ModelChoice {
                provider,
                ..ModelChoice::default()
            },
            version.map(str::to_owned),
            CompactionLaunchIdentity::new("generation"),
        )
        .unwrap()
}

#[test]
fn all_wrappers_delegate_automatic_compaction_to_native_or_report_unsupported() {
    for provider in AgentProvider::ALL {
        for version in [None, Some("0.0.1"), Some("999.0.0"), Some("unreadable")] {
            let policy = applied(provider, 150_000, version);
            if !matches!(provider, AgentProvider::Claude | AgentProvider::ClaudeAdk) {
                assert!(!policy.capabilities.native_threshold);
            }
            assert!(!policy.automatic_due(Some(u64::MAX)));
            assert_eq!(
                policy.capabilities.qualification.version.as_deref(),
                version
            );
            assert!(!policy.capabilities.qualification.evidence.is_empty());
            assert!(policy.reason.is_some());
            assert_eq!(policy.translated_trigger, None);
            let supported =
                harness_for(provider).compacts_on_command() && provider != AgentProvider::Pi;
            assert_eq!(policy.capabilities.between_turn_compact, supported);
            assert_eq!(
                policy.enforcement,
                if supported {
                    CompactionEnforcement::Native
                } else {
                    CompactionEnforcement::Unsupported
                }
            );
        }
    }
}

#[test]
fn captured_fallback_is_inclusive_and_unknown_or_native_never_schedules_it() {
    let mut policy = applied(AgentProvider::CodexAppServer, 150_000, None);
    assert!(!policy.automatic_due(Some(u64::MAX)));
    policy.enforcement = CompactionEnforcement::BetweenTurn;
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
fn zero_is_refused_and_unsafe_claude_window_keeps_exact_intent_using_native_auto() {
    assert!(harness_for(AgentProvider::Claude)
        .prepare_compaction_policy(
            ResolvedCompactionPolicy {
                tokens: 0,
                source: CompactionSource::Device
            },
            ModelChoice::default(),
            None,
            CompactionLaunchIdentity::new("generation"),
        )
        .is_err());
    let policy = applied(AgentProvider::Claude, u64::MAX, None);
    assert_eq!(policy.requested.tokens, u64::MAX);
    assert_eq!(policy.enforcement, CompactionEnforcement::Native);
    assert_eq!(
        policy.reason.as_deref(),
        Some("claude_requested_window_unrepresentable_using_auto")
    );
    assert!(!policy.automatic_due(Some(u64::MAX)));
}

struct OpeningSession(std::sync::Arc<std::sync::Mutex<crate::harness::AgentStatus>>);
impl crate::harness::AgentSession for OpeningSession {
    fn send_turn(&self, _: &crate::harness::Turn) -> Result<(), HarnessError> {
        Ok(())
    }
    fn status(&self) -> crate::harness::AgentStatus {
        *self.0.lock().unwrap()
    }
    fn quiet_for(&self) -> std::time::Duration {
        std::time::Duration::ZERO
    }
    fn exited_within(&self, _: std::time::Duration) -> bool {
        false
    }
    fn end(&self) {
        *self.0.lock().unwrap() = crate::harness::AgentStatus::Ended { code: None };
    }
    fn backdate_last_output(&self, _: std::time::Duration) {}
    fn turn_choice_support(&self, choice: &ModelChoice) -> crate::harness::TurnChoiceSupport {
        if choice.model.as_deref() == Some("carrier-unsupported-model") {
            crate::harness::TurnChoiceSupport::RestartRequired
        } else {
            crate::harness::TurnChoiceSupport::Native
        }
    }
}

#[test]
fn an_async_opening_installs_no_policy_until_the_protocol_accepts_it() {
    use crate::harness::AgentStatus;
    use std::sync::{Arc, Mutex};
    let status = Arc::new(Mutex::new(AgentStatus::Starting));
    let policy = applied(AgentProvider::ClaudeAdk, 150_000, None);
    let session = PolicySession::attach(
        Arc::new(OpeningSession(Arc::clone(&status))),
        policy.clone(),
    );
    assert_eq!(session.compaction_policy(), None);
    *status.lock().unwrap() = AgentStatus::Ended { code: Some(1) };
    assert_eq!(
        session.compaction_policy(),
        None,
        "failed protocol startup never applies policy"
    );

    let status = Arc::new(Mutex::new(AgentStatus::Starting));
    let session = PolicySession::attach(
        Arc::new(OpeningSession(Arc::clone(&status))),
        policy.clone(),
    );
    *status.lock().unwrap() = AgentStatus::Working;
    assert_eq!(session.compaction_policy(), Some(&policy));
    assert!(
        !session.automatic_compaction_due(Some(200_000)),
        "never mid-turn"
    );
    *status.lock().unwrap() = AgentStatus::Waiting;
    assert!(!session.automatic_compaction_due(Some(200_000)));
}

#[test]
fn native_model_change_delegation_preserves_carrier_support_and_exact_trigger_requalification() {
    use crate::harness::{AgentStatus, TurnChoiceSupport};
    use std::sync::{Arc, Mutex};
    let mut policy = applied(AgentProvider::CodexAppServer, 150_000, None);
    policy.enforcement = CompactionEnforcement::Native;
    let changed = ModelChoice {
        model: Some("carrier-supported-model".into()),
        ..policy.choice.clone()
    };
    let unsupported = ModelChoice {
        model: Some("carrier-unsupported-model".into()),
        ..policy.choice.clone()
    };
    for translated_trigger in [None, Some(150_000)] {
        policy.translated_trigger = translated_trigger;
        let inner = Arc::new(OpeningSession(Arc::new(Mutex::new(AgentStatus::Waiting))));
        let session = PolicySession::attach(inner, policy.clone());
        assert_eq!(session.compaction_policy(), Some(&policy));
        assert_eq!(
            session.turn_choice_support(&policy.choice),
            TurnChoiceSupport::Native
        );
        assert_eq!(
            session.turn_choice_support(&changed),
            if translated_trigger.is_some() {
                TurnChoiceSupport::RestartRequired
            } else {
                TurnChoiceSupport::Native
            },
            "only an exact translated trigger requires model requalification"
        );
        assert_eq!(
            session.turn_choice_support(&unsupported),
            TurnChoiceSupport::RestartRequired,
            "delegated native compaction must preserve the carrier's model restriction"
        );
        assert_eq!(
            session.compaction_policy(),
            Some(&policy),
            "a choice probe never replaces the admitted policy"
        );
    }
}

#[test]
fn pi_unsupported_policy_keeps_messages_but_refuses_unobservable_manual_compaction() {
    use crate::harness::{AgentStatus, Turn};
    use std::sync::{Arc, Mutex};
    let policy = applied(AgentProvider::Pi, 150_000, None);
    let session = PolicySession::attach(
        Arc::new(OpeningSession(Arc::new(Mutex::new(AgentStatus::Waiting)))),
        policy,
    );
    assert!(session.send_turn(&Turn::new("ordinary input")).is_ok());
    assert!(session
        .send_turn(&Turn::new("/compact keep the task"))
        .is_err());
}
