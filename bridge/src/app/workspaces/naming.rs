//! An agent may supply the missing label of its own workspace once.
use super::*;

impl AppState {
    pub(in crate::app) fn workspace_metadata_busy(&self, id: &str) -> bool {
        self.deferred_work.is_some()
            || self.active_deferred_filesystem_jobs > 0
            || self.workspace_reserved(id)
            || self.workspace_has_pending_shell(id)
    }

    pub(in crate::app) fn claim_workspace_name_request(
        &mut self,
        owner: &str,
        agent_id: &str,
    ) -> Option<String> {
        if self.is_project_coordinator(owner, agent_id) {
            return None;
        }
        let workspace = self.workspace_for_name_request(owner)?;
        let id = workspace.id.clone();
        self.workspaces.claim_name_request(&id).then_some(id)
    }

    /// Managed workspace roots are observed when provisioned and at startup.
    /// Naming delivery consumes those retained identities without probing paths.
    fn workspace_for_name_request(&self, owner: &str) -> Option<&Workspace> {
        let active = self.runs.get(owner)?;
        if self.project_pin(owner).is_some() {
            return None;
        }
        let project = self.projects.project_id_of(owner)?;
        let root = self.cached_snapshot_root(&active.worktree.path);
        self.workspaces
            .list(Some(project))
            .into_iter()
            .find(|workspace| {
                workspace.waiting_for_name
                    && (workspace.id == owner
                        || (!active.run.state.is_terminal()
                            && self.cached_snapshot_root(&workspace.root) == root
                            && self.workspace_name_request_owner(workspace) == Some(owner)))
            })
    }

    fn workspace_name_request_owner(&self, workspace: &Workspace) -> Option<&str> {
        if self.runs.contains_key(&workspace.id) && self.project_pin(&workspace.id).is_none() {
            return self
                .runs
                .get_key_value(&workspace.id)
                .map(|(id, _)| id.as_str());
        }
        let root = self.cached_snapshot_root(&workspace.root);
        self.runs
            .iter()
            .find(|(owner, active)| {
                !active.run.state.is_terminal()
                    && self.project_pin(owner).is_none()
                    && self.projects.project_id_of(owner) == Some(workspace.project_id.as_str())
                    && self.cached_snapshot_root(&active.worktree.path) == root
            })
            .map(|(owner, _)| owner.as_str())
    }

    pub(in crate::app) fn set_workspace_name(
        &mut self,
        owner: &str,
        name: &str,
    ) -> Result<Value, String> {
        let name = crate::mcp::normalized_workspace_name(name)?;
        let workspace = self
            .workspace_for_name_request(owner)
            .or_else(|| self.workspace_for_conversation(owner, None))
            .ok_or("set_workspace_name: you are not in a workspace")?;
        if !workspace.waiting_for_name {
            return Err(
                "set_workspace_name: workspace already named; keep the person's name".into(),
            );
        }
        self.workspace_rename(&json!({"workspace_id":workspace.id,"name":name}))
    }
}
