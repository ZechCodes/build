//! One guard owns fresh render memos and may share validated pure structure.
use super::{AppState, Workspace};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

mod inputs;
mod structure;
#[cfg(test)]
pub(in crate::app) mod work_counts;
pub(in crate::app) use structure::ProjectionStructure;

pub(in crate::app) struct ProjectionOwnership {
    pub(in crate::app) rendering: super::projection_rendering::ProjectionRendering,
    structure: Arc<ProjectionStructure>,
}

impl ProjectionOwnership {
    pub(in crate::app) fn structure(&self) -> Arc<ProjectionStructure> {
        Arc::clone(&self.structure)
    }

    #[cfg(test)]
    pub(in crate::app) fn structural_identity_for_test(&self) -> usize {
        Arc::as_ptr(&self.structure) as usize
    }

    pub(in crate::app) fn is_project_owner(&self, owner: &str) -> bool {
        self.structure.is_project_owner(owner)
    }
    pub(in crate::app) fn workspace_owner(&self, workspace: &str) -> Option<Option<&String>> {
        self.structure.workspace_owner(workspace)
    }
    pub(in crate::app) fn workspace_for_owner(
        &self,
        owner: &str,
        project: Option<&str>,
    ) -> Option<&str> {
        self.structure.workspace_for_owner(owner, project)
    }
    pub(in crate::app) fn surviving_workspace(&self, owner: &str) -> Option<&str> {
        self.structure.surviving_workspace(owner)
    }
    pub(in crate::app) fn compaction_workspace(&self, owner: &str) -> Option<&str> {
        self.structure.compaction_workspace(owner)
    }
    pub(in crate::app) fn task_workspace(&self, owner: &str) -> Option<&str> {
        self.structure.task_workspace(owner)
    }
    pub(in crate::app) fn task_paths(&self) -> &HashMap<PathBuf, String> {
        self.structure.task_paths()
    }
}

impl AppState {
    pub(in crate::app) fn capture_snapshot_ownership(&self) -> ProjectionOwnership {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.ownership_capture");
        let accepted = self
            .read_view
            .load()
            .global
            .as_ref()
            .and_then(|global| global.ownership_structure())
            .cloned();
        let structure = accepted
            .filter(|structure| structure.matches(self))
            .unwrap_or_else(|| Arc::new(ProjectionStructure::capture(self)));
        let rendering =
            super::projection_rendering::ProjectionRendering::new(Arc::clone(&structure.indexes));
        ProjectionOwnership {
            rendering,
            structure,
        }
    }
}
