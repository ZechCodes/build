//! A captured device-vault fence for workspace I/O outside the application lock.
use crate::app::AppState;
use crate::workspace::{Workspace, WorkspaceRegistry, WorkspaceSource};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(super) struct SecretSourcePolicy {
    state_root: Option<PathBuf>,
}

impl AppState {
    pub(super) fn workspace_secret_source_policy(&self) -> SecretSourcePolicy {
        SecretSourcePolicy {
            state_root: self.secret_state_root.clone(),
        }
    }
}

impl SecretSourcePolicy {
    pub(super) fn path(&self, path: &Path) -> Result<(), String> {
        if let Some(root) = &self.state_root {
            crate::config::vault::validate_tree("workspace source or destination", path, root)?;
        }
        Ok(())
    }

    pub(super) fn sources(&self, sources: &[WorkspaceSource]) -> Result<(), String> {
        for source in sources {
            self.path(&source.path)?;
        }
        Ok(())
    }

    pub(super) fn workspace(&self, workspace: &Workspace) -> Result<(), String> {
        self.path(&workspace.root)?;
        self.sources(&WorkspaceRegistry::retry_sources(workspace))
    }

    pub(super) fn copying(
        &self,
        source: &WorkspaceSource,
        destination: &Path,
    ) -> Result<(), String> {
        self.path(&source.path)?;
        self.path(destination)
    }
}
