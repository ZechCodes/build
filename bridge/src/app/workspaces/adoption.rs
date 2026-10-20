use crate::app::AppState;
use crate::orchestrator::ActiveRun;

impl AppState {
    /// Capture the compatibility alias before the owner's candidate snapshot
    /// and SQL write. An existing workspace is never part of this rollback.
    pub(in crate::app) fn stage_adoption_workspace(
        &mut self,
        run_id: &str,
        active: &ActiveRun,
    ) -> Option<String> {
        if self.workspaces.get(run_id).is_some() {
            return None;
        }
        let project_id = self
            .projects
            .project_id_of(run_id)
            .unwrap_or_default()
            .to_string();
        let source_id = self
            .projects
            .get(&project_id)
            .and_then(|project| project.sources.first())
            .map(|source| source.id.clone())
            .unwrap_or_else(|| "root".into());
        self.workspaces.adopt_root(
            &project_id,
            run_id.to_string(),
            active.worktree.name.clone(),
            active.worktree.path.clone(),
            source_id,
            true,
        );
        self.refresh_workspace_snapshot_observation(run_id);
        Some(run_id.to_string())
    }

    pub(in crate::app) fn rollback_adoption_workspace(&mut self, staged: Option<String>) {
        if let Some(id) = staged {
            self.workspaces.remove_staged_adoption(&id);
            self.refresh_workspace_snapshot_observation(&id);
        }
    }
}
