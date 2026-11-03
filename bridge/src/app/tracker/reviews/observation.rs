//! Prepare workspace SQL once off lock, then publish its owned observation.
use crate::app::snapshot_projection::{WorkspaceReviewObservation, WorkspaceReviewReader};
use crate::app::AppState;
use std::cell::RefCell;

pub(super) struct ReviewInvalidation {
    workspace: String,
    parent_task_id: Option<String>,
    reader: WorkspaceReviewReader,
    observed: RefCell<Option<WorkspaceReviewObservation>>,
}

impl ReviewInvalidation {
    pub(super) fn new(
        app: &AppState,
        review: &crate::reviews::records::Review,
        task: &crate::tracker::Task,
    ) -> Self {
        Self {
            workspace: review.workspace_id.clone(),
            parent_task_id: task.links.parent_task_id.clone(),
            reader: app.workspace_review_reader(),
            observed: RefCell::default(),
        }
    }

    /// Called after the service has persisted, including partial failures.
    pub(super) fn prepare(&self) {
        *self.observed.borrow_mut() = self.load();
    }

    pub(super) fn load(&self) -> Option<WorkspaceReviewObservation> {
        self.reader.load(&self.workspace)
    }

    pub(super) fn workspace(&self) -> &str {
        &self.workspace
    }

    pub(super) fn invalidate_parent(&self, changes: &crate::changes::ChangeBus, project: &str) {
        if let Some(parent) = &self.parent_task_id {
            changes.note_tasks(project, std::slice::from_ref(parent));
        }
    }

    pub(super) fn install(&self, app: &mut AppState) {
        if let Some(observed) = self.observed.take() {
            app.install_workspace_review_observation(&self.workspace, observed);
        }
    }

    pub(super) fn invalidate(&self, app: &mut AppState, project: &str, task: &str) {
        app.publish_review_observation(project, task, &self.workspace, self.observed.take());
        self.invalidate_parent(&app.changes, project);
        super::lifecycle::invalidates(app, &self.workspace);
    }
}
