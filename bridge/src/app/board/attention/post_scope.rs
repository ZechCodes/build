//! Coalesce only the two human attention stamps inside one RPC dispatch guard.
use super::AppState;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

impl AppState {
    pub(in crate::app) fn with_rpc_post_attention<R>(
        &mut self,
        method: &str,
        dispatch: impl FnOnce(&mut Self) -> R,
    ) -> R {
        if method != "thread.post" {
            return dispatch(self);
        }
        let previous = self.rpc_post_attention_owners.replace(Default::default());
        let result = catch_unwind(AssertUnwindSafe(|| dispatch(self)));
        let owners = self
            .rpc_post_attention_owners
            .take()
            .expect("the RPC attention scope is still installed");
        // Restore before any flush or resumed unwind. A nested dispatch joins
        // its parent; no scope can survive a handed-out deferred job.
        self.rpc_post_attention_owners = previous;
        if let Some(outer) = self.rpc_post_attention_owners.as_mut() {
            outer.extend(owners);
        } else if result.is_ok() {
            self.publish_post_attention_snapshot_projection(&owners);
        }
        match result {
            Ok(answer) => answer,
            // Stamps already retained their dirty owner/observation receipts.
            // Never publish potentially half-mutated canonical state here.
            Err(panic) => resume_unwind(panic),
        }
    }

    pub(in crate::app) fn persist_post_stamp_attention(&mut self, owner: &str) {
        let known_owner = self.plans.contains_key(owner) || self.runs.contains_key(owner);
        let Some(owners) = self
            .rpc_post_attention_owners
            .as_mut()
            .filter(|_| known_owner)
        else {
            self.persist_attention_for_owner(owner);
            return;
        };
        owners.insert(owner.to_owned());
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("finisher.attention");
        // Keep dependencies pending even if dispatch unwinds before its final
        // union flush. No dirty receipt is consumed by recording these keys.
        self.mark_post_attention_snapshot_owner_pending(owner);
        self.publish_read_metadata_only();
        self.changes
            .note_board_lists(crate::changes::BoardLists::default());
        self.watchers.board_moved(self.worktree_roots());
        self.save_attention_map();
    }
}
