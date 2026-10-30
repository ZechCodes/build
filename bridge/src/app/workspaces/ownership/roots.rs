//! Owner-to-workspace lookup for one eager checkout-roots snapshot.
//! Preserve an existing outer observation; otherwise capture only root inputs.
use super::{inputs::Inputs, AppState, ProjectionOwnership};
use std::collections::BTreeMap;
use std::sync::Arc;

pub(in crate::app) enum RootsOwnership {
    Existing(Arc<ProjectionOwnership>),
    Local(BTreeMap<String, String>),
}

impl RootsOwnership {
    pub(in crate::app) fn capture(app: &AppState) -> Self {
        if let Some(context) = app.snapshot_ownership_context() {
            return Self::Existing(context);
        }
        let inputs = Inputs::capture(app);
        let mut workspaces = BTreeMap::new();
        // Inputs preserves registry name/ID order, exact run traversal and
        // existing direct/first-live/project/terminal ownership precedence.
        for (index, workspace) in inputs.workspaces.iter().enumerate() {
            if let Some(owner) = inputs.owner(index) {
                workspaces
                    .entry(owner.to_owned())
                    .or_insert_with(|| workspace.id.clone());
            }
        }
        Self::Local(workspaces)
    }

    pub(in crate::app) fn workspace_for_owner(&self, owner: &str) -> Option<&str> {
        match self {
            Self::Existing(context) => context.workspace_for_owner(owner, None),
            Self::Local(workspaces) => workspaces.get(owner).map(String::as_str),
        }
    }
}
