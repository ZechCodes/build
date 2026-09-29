use crate::app::AppState;
use crate::lifecycle::holders::ProjectCheckouts;

pub(in crate::app) mod dispatch;

impl AppState {
    /// The checkouts of a project the mutex can name without touching the
    /// disk: the external scan the board already holds, every live run's
    /// checkout, and the primary. Which branch each one holds is git's to
    /// answer, and [`ProjectCheckouts::holders`] asks it.
    ///
    /// Only owned inputs are captured here. Every caller asks `holders` in
    /// its off-lock phase, including one-shot creates and dispatches.
    pub(in crate::app) fn project_checkouts(
        &self,
        project_id: &str,
    ) -> Result<ProjectCheckouts, String> {
        Ok(ProjectCheckouts {
            project: self.orch_for(project_id)?.clone(),
            primary_repo_path: self.repo_path_for(project_id)?,
            base_branch: self.base_for(project_id)?,
            excluded: self.bound_worktree_paths(),
            run_checkouts: self
                .live_runs_of(project_id)
                .map(|(id, run)| (id.clone(), run.worktree.clone()))
                .collect(),
        })
    }

    /// Recheck the records the off-lock holder reading was based on.
    pub(in crate::app) fn validate_checkout_snapshot(
        &self,
        project_id: &str,
        snapshot: &ProjectCheckouts,
    ) -> Result<(), String> {
        let project = self.project_for(project_id)?;
        let runs = self.live_runs_of(project_id).collect::<Vec<_>>();
        let unchanged = project.repo_path == snapshot.primary_repo_path
            && project.base_branch == snapshot.base_branch
            && runs.len() == snapshot.run_checkouts.len()
            && snapshot.run_checkouts.iter().all(|(id, checkout)| {
                runs.iter()
                    .any(|(current_id, active)| *current_id == id && active.worktree == *checkout)
            });
        if unchanged {
            Ok(())
        } else {
            Err("the project's branch holders changed while Git ran; retry the action".to_string())
        }
    }

    /// The repository of the project an entity belongs to — where a
    /// project-wide artifact like the review rules lives, rather than in
    /// whichever worktree happened to notice it.
    pub(in crate::app) fn project_repository_of(
        &self,
        entity_id: &str,
    ) -> Result<std::path::PathBuf, String> {
        let project_id = self.project_of(entity_id)?;
        self.repo_path_for(&project_id)
    }
}
