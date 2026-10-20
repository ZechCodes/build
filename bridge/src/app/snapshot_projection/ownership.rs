//! Share ownership resolution across every chunk prepared for one publication.
use super::{AppState, Arc, Mutex, ProjectionOwnership};

pub(super) struct ProjectionOwnershipGuard<'a> {
    slot: &'a Mutex<Option<Arc<ProjectionOwnership>>>,
    owns_context: bool,
}

impl Drop for ProjectionOwnershipGuard<'_> {
    fn drop(&mut self) {
        if self.owns_context {
            *self.slot.lock().unwrap() = None;
        }
    }
}

impl AppState {
    /// Candidate preparation runs under the app lock. Nested preparation
    /// reuses its context; the outer guard removes it before persistence or
    /// canonical fields can change, including a failed candidate's rollback.
    pub(super) fn begin_snapshot_ownership(&self) -> ProjectionOwnershipGuard<'_> {
        let slot = &self.snapshot_observations.ownership;
        let owns_context = slot.lock().unwrap().is_none();
        if owns_context {
            let context = Arc::new(self.capture_snapshot_ownership());
            *slot.lock().unwrap() = Some(context);
        }
        ProjectionOwnershipGuard { slot, owns_context }
    }

    pub(in crate::app) fn snapshot_ownership_context(&self) -> Option<Arc<ProjectionOwnership>> {
        self.snapshot_observations.ownership.lock().unwrap().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_projection_reuses_ownership_until_the_outer_preparation_finishes() {
        let root = tempfile::tempdir().unwrap();
        let context = crate::harness::HarnessContext::resolved(
            root.path().join("mcp.sock"),
            root.path().to_path_buf(),
        )
        .unwrap();
        let app = AppState::new_unrooted_configured(root.path().join("wt"), "main", true, context);
        assert!(app.snapshot_ownership_context().is_none());
        {
            let _outer = app.begin_snapshot_ownership();
            let captured = app.snapshot_ownership_context().unwrap();
            {
                let _inner = app.begin_snapshot_ownership();
                assert!(Arc::ptr_eq(
                    &captured,
                    &app.snapshot_ownership_context().unwrap()
                ));
            }
            assert!(Arc::ptr_eq(
                &captured,
                &app.snapshot_ownership_context().unwrap()
            ));
        }
        assert!(app.snapshot_ownership_context().is_none());
    }
}
