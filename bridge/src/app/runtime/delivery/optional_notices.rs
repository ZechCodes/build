//! No notice is excluded without a typed rule, enrollment, and publication proof.
use crate::app::{AppState, PendingAgentTurn};
use crate::timing::optional_notices::OptionalNoticeKind;
use crate::timing::write_wait::current_context;
use std::collections::BTreeSet;

impl AppState {
    pub(in crate::app) fn qualify_optional_notice(
        &self,
        kind: Option<OptionalNoticeKind>,
        turn: &mut PendingAgentTurn,
    ) {
        let Some(kind) = kind else { return };
        let Some(policy) = self.frame_clock.optional_notice_policy() else {
            return;
        };
        let Some(context) = current_context() else {
            return;
        };
        if policy.offer(&context, kind) && self.origin_publication_complete(&context, &turn.owner) {
            // Earlier publication work stays on the initiating write. Only
            // this new notice child gets its explicitly separate cohort.
            assert!(turn.logical_obligations.is_empty());
            turn.logical_obligations.push(policy.begin(&context, kind));
        }
    }
    pub(in crate::app) fn cancel_optional_notices_for_cleanup(
        &mut self,
    ) -> Result<usize, &'static str> {
        let Some(policy) = self.frame_clock.optional_notice_policy() else {
            return Ok(0);
        };
        let candidates = policy.cleanup_candidates()?;
        let mut removed = BTreeSet::new();
        self.delivery_queue.retain_queued(|turn| {
            let Some(ids) = policy.removable_origins(&turn.logical_contexts(), &candidates) else {
                return true;
            };
            removed.extend(ids);
            false
        });
        // retain_queued closed the actual queue intervals and cancelled only
        // those owned child tokens. In-flight work still owns its obligations.
        Ok(policy.record_cleanup(removed))
    }
}
