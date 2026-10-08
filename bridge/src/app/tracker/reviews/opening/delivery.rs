//! Post-publication reviewer delivery reuses the ordinary task dispatcher.
use super::{validate_live_workspace, validate_recorded_workspace, LiveOpeningHooks, OpeningJob};
use crate::app::AppState;
use crate::reviews::opening::OpeningHooks;
use crate::tracker::{Assignee, Task};
use crate::workspace::Workspace;
use std::cell::RefCell;
use std::sync::Weak;

impl OpeningHooks for LiveOpeningHooks {
    fn check_workspace(&self, workspace: &Workspace) -> Result<(), String> {
        if let Some(shared) = &self.shared {
            let app = shared
                .upgrade()
                .ok_or("unavailable: bridge state disappeared")?;
            return validate_live_workspace(&app.lock().unwrap(), workspace);
        }
        validate_recorded_workspace(&self.registry_root, workspace)
    }
    fn workspace_changed(&self, _: &Workspace) -> Result<(), String> {
        if let Some(shared) = self.shared.as_ref().and_then(Weak::upgrade) {
            shared.lock().unwrap().workspaces.reload()?;
        }
        Ok(())
    }
    fn dispatch_reviewer(&self, task: &Task, operation_id: &str) -> Result<(), String> {
        if matches!(task.assignee, None | Some(Assignee::User)) {
            return Ok(());
        }
        let shared = self
            .shared
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or("unavailable: reviewer delivery awaits owning app state")?;
        let mut app = shared.lock().unwrap();
        dispatch(
            &mut app,
            &self.store,
            &self.project_id,
            &self.project_path,
            &self.request_id,
            operation_id,
        )
    }
}

pub(super) struct SettlementHooks<'a> {
    pub(super) app: RefCell<&'a mut AppState>,
    pub(super) job: &'a OpeningJob,
}

impl OpeningHooks for SettlementHooks<'_> {
    fn check_workspace(&self, _: &Workspace) -> Result<(), String> {
        Ok(())
    }
    fn workspace_changed(&self, _: &Workspace) -> Result<(), String> {
        Ok(())
    }
    fn dispatch_reviewer(&self, _: &Task, operation_id: &str) -> Result<(), String> {
        dispatch(
            &mut self.app.borrow_mut(),
            &self.job.store,
            &self.job.project_id,
            &self.job.request.project_path,
            &self.job.request.request_id,
            operation_id,
        )
    }
}

fn dispatch(
    app: &mut AppState,
    store: &crate::store::Store,
    project_id: &str,
    project_path: &str,
    request_id: &str,
    operation_id: &str,
) -> Result<(), String> {
    let opening = store
        .load_review_opening(project_path, request_id)
        .map_err(|error| error.to_string())?
        .ok_or("unavailable: review opening is unavailable")?;
    let review = store
        .load_review(&opening.task.id)
        .map_err(|error| error.to_string())?
        .ok_or("unavailable: published review is unavailable")?;
    let snapshot = review
        .snapshots
        .first()
        .ok_or("unavailable: published snapshot is unavailable")?;
    let note = format!("Review the published PR snapshot {}. Read the task with get_task and the pinned snapshot with get_review/read_review before acting. Later pushes create separate snapshots.", snapshot.id);
    app.dispatch_review_reviewer(
        project_id,
        &opening.task,
        &note,
        operation_id,
        &opening.request.creator,
    )
}
