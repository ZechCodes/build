//! Share ownership resolution across every chunk prepared for one publication.
use super::{AppState, Arc, BTreeSet, Mutex, ProjectionOwnership};

pub(in crate::app) struct ProjectionOwnershipGuard<'a> {
    slot: &'a Mutex<Option<Arc<ProjectionOwnership>>>,
    owns_context: bool,
}

impl Drop for ProjectionOwnershipGuard<'_> {
    fn drop(&mut self) {
        if self.owns_context {
            *crate::timing::resource_lock!(self.slot).unwrap() = None;
        }
    }
}

impl AppState {
    /// Candidate preparation runs under the app lock. Nested preparation
    /// reuses its context; the outer guard removes it before persistence or
    /// canonical fields can change, including a failed candidate's rollback.
    pub(in crate::app) fn begin_snapshot_ownership(&self) -> ProjectionOwnershipGuard<'_> {
        let slot = &self.snapshot_observations.ownership;
        let owns_context = crate::timing::resource_lock!(slot).unwrap().is_none();
        if owns_context {
            let context = Arc::new(self.capture_snapshot_ownership());
            *crate::timing::resource_lock!(slot).unwrap() = Some(context);
        }
        ProjectionOwnershipGuard { slot, owns_context }
    }

    pub(in crate::app) fn snapshot_ownership_context(&self) -> Option<Arc<ProjectionOwnership>> {
        crate::timing::resource_lock!(self.snapshot_observations.ownership)
            .unwrap()
            .clone()
    }

    /// Pure structure is retained only by the delta's accepted global cache.
    pub(in crate::app) fn snapshot_projection_structure(
        &self,
    ) -> Option<Arc<crate::app::workspaces::ProjectionStructure>> {
        self.snapshot_projection_initialized()
            .then(|| self.snapshot_ownership_context())
            .flatten()
            .map(|context| context.structure())
    }

    pub(in crate::app) fn snapshot_ownership_paths(
        &self,
    ) -> &std::collections::BTreeMap<std::path::PathBuf, std::path::PathBuf> {
        &self.snapshot_observations.paths
    }

    /// A canonical conversation can appear on another owner's roster. Include
    /// aliases of both the committed and candidate bindings when that owner
    /// changes, without rebuilding unrelated owners' agent chunks.
    pub(super) fn snapshot_affected_owner_ids(&self, owner: &str) -> BTreeSet<String> {
        let mut conversations = BTreeSet::new();
        if let Ok(roster) = self.entity_agents(owner) {
            for agent in roster.iter() {
                conversations.insert(agent.id.clone());
                conversations.insert(agent.conversation_id().to_string());
            }
        }
        let previous = self.read_view.load();
        let previous_row = previous.get("agent.list", owner);
        let previous_agents = previous_row
            .as_deref()
            .and_then(|row| row["agents"].as_array());
        for agent in previous_agents.into_iter().flatten() {
            for field in ["id", "conversation_id"] {
                if let Some(id) = agent[field].as_str() {
                    conversations.insert(id.to_string());
                }
            }
        }
        let mut affected = self
            .snapshot_owner_ids()
            .into_iter()
            .filter(|candidate| {
                self.entity_agents(candidate).is_ok_and(|roster| {
                    roster
                        .iter()
                        .any(|agent| conversations.contains(agent.conversation_id()))
                })
            })
            .collect::<BTreeSet<_>>();
        affected.insert(owner.to_string());
        affected
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
