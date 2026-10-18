//! A launch-qualified policy, owned by the session that actually opened.
//! Settings edits cannot replace this snapshot. Native adapters may qualify
//! particular versions, values and model budgets; none are qualified yet.

use super::{Harness, HarnessError};
use crate::compaction::{ResolvedCompactionPolicy, MAX_SAFE_TOKENS};
use crate::models::ModelChoice;

mod session;
pub(crate) use session::PolicySession;
#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactionQualification {
    /// Last successful launch-time CLI observation, not a live readback.
    pub version: Option<String>,
    pub evidence: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactionCapabilities {
    pub native_threshold: bool,
    pub between_turn_compact: bool,
    pub native_unit: Option<String>,
    pub native_range: Option<std::ops::RangeInclusive<u64>>,
    pub qualification: CompactionQualification,
    pub unsupported_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactionEnforcement {
    Native,
    BetweenTurn,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactionLaunchIdentity {
    pub id: String,
    pub thread_id: String,
}

impl CompactionLaunchIdentity {
    pub fn new(thread_id: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            thread_id: thread_id.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeCompactionSettings {
    Claude(super::claude_compaction::Translation),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedCompactionPolicy {
    pub requested: ResolvedCompactionPolicy,
    /// A qualified native trigger, when the adapter can establish one.
    pub translated_trigger: Option<u64>,
    /// Captured adapter settings; never re-resolved during spec construction.
    pub native_settings: Option<NativeCompactionSettings>,
    pub enforcement: CompactionEnforcement,
    pub reason: Option<String>,
    pub capabilities: CompactionCapabilities,
    pub choice: ModelChoice,
    pub launch_identity: CompactionLaunchIdentity,
}

impl AppliedCompactionPolicy {
    pub fn automatic_due(&self, context_tokens: Option<u64>) -> bool {
        self.enforcement == CompactionEnforcement::BetweenTurn
            && context_tokens.is_some_and(|tokens| tokens >= self.requested.tokens)
    }
}

pub(super) fn fallback_policy<H: Harness + ?Sized>(
    harness: &H,
    requested: ResolvedCompactionPolicy,
    choice: ModelChoice,
    version: Option<String>,
    launch_identity: CompactionLaunchIdentity,
) -> Result<AppliedCompactionPolicy, HarnessError> {
    if requested.tokens == 0 {
        return Err(HarnessError::Setup(
            "compaction threshold must be positive".into(),
        ));
    }
    let capabilities = CompactionCapabilities {
        native_threshold: false,
        between_turn_compact: harness.compacts_on_command(),
        native_unit: None,
        native_range: None,
        qualification: CompactionQualification {
            version,
            evidence: "Build wrapper command contract; native threshold adapter is not qualified"
                .into(),
        },
        unsupported_reason: Some("native_threshold_unqualified".into()),
    };
    let (enforcement, reason) = if requested.tokens > MAX_SAFE_TOKENS {
        (
            CompactionEnforcement::Unsupported,
            "unsupported_representation",
        )
    } else if capabilities.between_turn_compact {
        (
            CompactionEnforcement::BetweenTurn,
            "native_threshold_unqualified",
        )
    } else {
        (
            CompactionEnforcement::Unsupported,
            "between_turn_compact_unavailable",
        )
    };
    Ok(AppliedCompactionPolicy {
        requested,
        translated_trigger: None,
        native_settings: None,
        enforcement,
        reason: Some(reason.into()),
        capabilities,
        choice,
        launch_identity,
    })
}
