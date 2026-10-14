use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::super::protocol::PendingOperation;
use super::super::state::{SessionEffect, StateTransition};
use crate::harness::{AgentActivity, AgentStatus};
use crate::provider_recovery::RecoveryJournal;

/// The checkpoint is a barrier before an outgoing continuation or failure report.
/// New queued input does not spend or replace the active failure episode.
pub(in crate::harness::codex_app_server) fn persist_transition(
    journal: &mut RecoveryJournal,
    transition: &StateTransition,
    elapsed: Duration,
    session_source: &str,
) -> Result<(), String> {
    let state = &transition.state;
    if let Some(checkpoint) = state.recovery_checkpoint() {
        let remaining = checkpoint.retry_at.saturating_sub(elapsed);
        let deadline = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .saturating_add(remaining);
        journal.checkpoint(
            &checkpoint.source,
            checkpoint.operation_id,
            checkpoint.summary,
            deadline.as_millis().try_into().unwrap_or(u64::MAX),
        )?;
    }
    let mut failed = false;
    for effect in &transition.effects {
        if let SessionEffect::Report(report) = effect {
            if let AgentActivity::TurnFailed {
                summary,
                operation_id,
                ..
            } = &report.activity
            {
                let source = state
                    .failure_source()
                    .unwrap_or_else(|| session_source.to_string());
                journal.finish(&source, operation_id.clone(), summary.clone())?;
                failed = true;
            }
        }
    }
    let starts_turn = transition.effects.iter().any(|effect| {
        matches!(
            effect,
            SessionEffect::Request(
                PendingOperation::StartTurn { .. } | PendingOperation::CompactThread { .. }
            )
        )
    });
    if starts_turn {
        if state.automatic_retry_in_flight() {
            journal.retry_in_flight()?;
        } else {
            journal.deactivate()?;
        }
    } else if !failed && state.live_status() == Some(AgentStatus::Waiting) {
        journal.resolve()?;
    }
    Ok(())
}
