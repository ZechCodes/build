//! Persist the first-turn naming claim on the workspace, not its agents.
use super::*;

impl WorkspaceRegistry {
    pub(crate) fn claim_name_request(&mut self, id: &str) -> Result<bool, String> {
        let Some(mut workspace) = self.get(id).cloned() else {
            return Ok(false);
        };
        if !workspace.waiting_for_name || workspace.name_asked {
            return Ok(false);
        }
        workspace.name_asked = true;
        persist(&workspace)?;
        self.install_workspace(workspace);
        Ok(true)
    }

    pub(crate) fn release_name_request(&mut self, id: &str) -> Result<(), String> {
        let Some(mut workspace) = self.get(id).cloned() else {
            return Ok(());
        };
        if !workspace.waiting_for_name {
            return Ok(());
        }
        workspace.name_asked = false;
        persist(&workspace)?;
        self.install_workspace(workspace);
        Ok(())
    }
}
