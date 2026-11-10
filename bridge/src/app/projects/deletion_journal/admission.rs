//! Bounded in-memory capture; no filesystem or store access at admission.
use super::*;

pub(super) struct CapturedDeletion {
    pub(super) plan: ProjectDeletionPlan,
    pub(super) sources: Vec<PathBuf>,
    pub(super) workspaces: Vec<crate::workspace::Workspace>,
    pub(super) candidates: Vec<(PathBuf, PathBuf, PathBuf)>,
}
impl AppState {
    pub(in crate::app::projects) fn prepare_recorded_project_delete(
        &mut self,
        id: &str,
        delete_tasks: bool,
    ) -> Result<Value, String> {
        let captured = self.capture_deletion(id, delete_tasks)?;
        let operation = self.claim_deletion(&captured.plan)?;
        let job = super::preparation::PrepareDeletion::new(self, captured, operation)?;
        self.defer_project_deletion(job);
        Ok(json!({"project_id":id,"deleted":true}))
    }
    fn capture_deletion(&self, id: &str, delete_tasks: bool) -> Result<CapturedDeletion, String> {
        let project = self.projects.get(id).ok_or("unknown project")?;
        let workspaces: Vec<_> = self
            .project_deletion_workspaces(id)?
            .into_iter()
            .cloned()
            .collect();
        validate_workspaces(&workspaces, delete_tasks)?;
        let run_ids: Vec<_> = self
            .runs
            .keys()
            .filter(|run| self.projects.project_id_of(run) == Some(id))
            .cloned()
            .collect();
        let refs = workspaces.iter().collect::<Vec<_>>();
        let roots = self.owned_project_roots(id, &run_ids, &refs);
        let session_paths: Vec<_> = workspaces
            .iter()
            .map(|w| w.root.clone())
            .chain(run_ids.iter().map(|id| self.runs[id].worktree.path.clone()))
            .collect();
        if session_paths
            .iter()
            .any(|root| self.session_registry.pending_shell_within(root))
        {
            return Err(crate::reclaim::BUSY.into());
        }
        if session_paths
            .iter()
            .any(|root| self.delivery_queue.has_in_flight_at_root(root))
        {
            return Err("Stop running agents before deleting the project".into());
        }
        let config = self.config_value(&self.projects_dir, self.default_harness, self.isolation);
        let plan = ProjectDeletionPlan {
            project_id: id.into(),
            project_key: project.project_key.clone(),
            project_config: config["projects"]
                .as_array()
                .expect("project array")
                .iter()
                .find(|p| p["id"] == id)
                .cloned()
                .ok_or("project config missing")?,
            environment_config: config["environments"]
                .as_array()
                .expect("environment array")
                .iter()
                .find(|e| e["id"] == project.environment_id)
                .cloned()
                .ok_or("project environment missing")?,
            delete_tasks,
            roots,
            checkouts: vec![],
            rift_root: self.project_worktrees_root(id),
            run_ids,
            plan_ids: self
                .plans
                .keys()
                .filter(|owner| self.projects.project_id_of(owner) == Some(id))
                .cloned()
                .collect(),
            capture_ids: self
                .captures
                .values()
                .filter(|c| c.routing.as_ref().is_some_and(|r| r.project_id == id))
                .map(|c| c.id.clone())
                .collect(),
            workspace_ids: workspaces.iter().map(|w| w.id.clone()).collect(),
            inventory: Default::default(),
        };
        let candidates = self.checkout_candidates(&plan, &workspaces);
        Ok(CapturedDeletion {
            plan,
            workspaces,
            sources: self.deletion_sources(),
            candidates,
        })
    }
    fn checkout_candidates(
        &self,
        plan: &ProjectDeletionPlan,
        workspaces: &[crate::workspace::Workspace],
    ) -> Vec<(PathBuf, PathBuf, PathBuf)> {
        let mut candidates: Vec<_> = workspaces
            .iter()
            .filter(|w| w.managed)
            .flat_map(|w| {
                w.directories.iter().filter(|d| d.is_git).map(|d| {
                    (
                        d.source_path.clone(),
                        d.path.clone(),
                        self.project_worktrees_root(&w.project_id),
                    )
                })
            })
            .collect();
        candidates.extend(plan.run_ids.iter().filter_map(|id| {
            let path = &self.runs[id].worktree.path;
            plan.roots.contains(path).then(|| {
                (
                    self.projects
                        .get(&plan.project_id)
                        .expect("project")
                        .repo_path
                        .clone(),
                    path.clone(),
                    plan.rift_root.clone(),
                )
            })
        }));
        candidates
    }
}
fn validate_workspaces(
    workspaces: &[crate::workspace::Workspace],
    delete_tasks: bool,
) -> Result<(), String> {
    use crate::workspace::WorkspaceStatus;
    if delete_tasks
        && workspaces
            .iter()
            .any(|w| w.status != WorkspaceStatus::Finished)
    {
        return Err("Finish its workspaces before deleting tasks".into());
    }
    if workspaces.iter().any(|w| w.locked) {
        return Err(crate::workspace::LOCKED_REFUSAL.into());
    }
    if workspaces
        .iter()
        .any(|w| w.status == WorkspaceStatus::Provisioning)
    {
        return Err("Wait for workspace provisioning to finish before deleting the project".into());
    }
    Ok(())
}
