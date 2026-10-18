//! A transparent session decorator: one admitted policy for one opening.
use super::{AppliedCompactionPolicy, CompactionEnforcement};
use crate::harness::surfaces::AgentSurfaces;
use crate::harness::*;
use crate::models::ModelChoice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};

pub(crate) struct PolicySession {
    inner: Arc<dyn AgentSession>,
    policy: AppliedCompactionPolicy,
    applied: AtomicBool,
}

impl PolicySession {
    pub(crate) fn attach(
        inner: Arc<dyn AgentSession>,
        policy: AppliedCompactionPolicy,
    ) -> Arc<dyn AgentSession> {
        Arc::new(Self {
            inner,
            policy,
            applied: AtomicBool::new(false),
        })
    }
}

impl AgentSession for PolicySession {
    fn opening_succeeded(&self) -> bool {
        self.inner.opening_succeeded()
    }
    fn compaction_launch_identity(&self) -> Option<&super::CompactionLaunchIdentity> {
        Some(&self.policy.launch_identity)
    }
    fn compaction_policy(&self) -> Option<&AppliedCompactionPolicy> {
        // A hook-backed carrier can explicitly withdraw support after an
        // extension failure. Its runtime observation owns that decision.
        if let Some(policy) = self.inner.observed_compaction_policy() {
            return Some(policy);
        }
        if self.inner.opening_succeeded() {
            self.applied.store(true, Ordering::Relaxed);
        }
        self.applied.load(Ordering::Relaxed).then_some(&self.policy)
    }
    fn observed_compaction_policy(&self) -> Option<&AppliedCompactionPolicy> {
        self.inner.observed_compaction_policy()
    }
    fn automatic_compaction_due(&self, context_tokens: Option<u64>) -> bool {
        if self.inner.observed_compaction_policy().is_some() {
            return self.inner.automatic_compaction_due(context_tokens);
        }
        self.status() == AgentStatus::Waiting
            && self
                .compaction_policy()
                .is_some_and(|policy| policy.automatic_due(context_tokens))
    }
    fn turn_choice_support(&self, choice: &ModelChoice) -> TurnChoiceSupport {
        if self.policy.enforcement == CompactionEnforcement::Native && choice != &self.policy.choice
        {
            // A translated native trigger is qualified for its launch model.
            // Reopening lets the adapter qualify a different budget first.
            return TurnChoiceSupport::RestartRequired;
        }
        self.inner.turn_choice_support(choice)
    }
    fn send_turn(&self, turn: &Turn) -> Result<(), HarnessError> {
        let policy = self.compaction_policy().unwrap_or(&self.policy);
        if turn.text.split_whitespace().next() == Some(COMPACT_COMMAND)
            && !policy.capabilities.between_turn_compact
        {
            return Err(HarnessError::Unsupported(
                "this session has no qualified compaction command".into(),
            ));
        }
        self.inner.send_turn(turn)
    }
    fn accepts_turn_choice(&self) -> bool {
        self.inner.accepts_turn_choice()
    }
    fn status(&self) -> AgentStatus {
        self.inner.status()
    }
    fn provider_recovery_episode(&self) -> Option<String> {
        self.inner.provider_recovery_episode()
    }
    fn status_changed(&self) -> Option<watch::Receiver<SessionStatusSnapshot>> {
        self.inner.status_changed()
    }
    fn turn_receipt_support(&self) -> TurnReceiptSupport {
        self.inner.turn_receipt_support()
    }
    fn turn_receipts(&self) -> Option<watch::Receiver<TurnReceiptSnapshot>> {
        self.inner.turn_receipts()
    }
    fn quiet_for(&self) -> Duration {
        self.inner.quiet_for()
    }
    fn exited_within(&self, timeout: Duration) -> bool {
        self.inner.exited_within(timeout)
    }
    fn end(&self) {
        self.inner.end()
    }
    fn conversation_artifacts(&self) -> Vec<std::path::PathBuf> {
        self.inner.conversation_artifacts()
    }
    fn epitaph(&self) -> Option<String> {
        self.inner.epitaph()
    }
    fn start_refused(&self) -> Option<String> {
        self.inner.start_refused()
    }
    fn terminal(&self) -> Option<&dyn TerminalView> {
        self.inner.terminal()
    }
    fn activity(&self) -> Option<broadcast::Receiver<ActivityReport>> {
        self.inner.activity()
    }
    fn surfaces(&self) -> Option<AgentSurfaces> {
        self.inner.surfaces()
    }
    fn surfaces_changed(&self) -> Option<watch::Receiver<u64>> {
        self.inner.surfaces_changed()
    }
    fn can_interrupt(&self) -> bool {
        self.inner.can_interrupt()
    }
    fn interrupt(&self) -> Result<(), HarnessError> {
        self.inner.interrupt()
    }
    fn session_id(&self) -> Option<String> {
        self.inner.session_id()
    }
    fn active_model(&self) -> Option<String> {
        self.inner.active_model()
    }
    fn active_effort(&self) -> Option<String> {
        self.inner.active_effort()
    }
    fn active_choice(&self) -> (Option<String>, Option<String>) {
        self.inner.active_choice()
    }
    #[cfg(test)]
    fn backdate_last_output(&self, ago: Duration) {
        self.inner.backdate_last_output(ago)
    }
}
