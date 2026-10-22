//! Prepare workspace SQL once off lock, then publish its owned observation.
use crate::app::snapshot_projection::{WorkspaceReviewObservation, WorkspaceReviewReader};
use crate::app::AppState;
use std::cell::RefCell;

pub(super) struct ReviewInvalidation {
    workspace: String,
    reader: WorkspaceReviewReader,
    observed: RefCell<Option<WorkspaceReviewObservation>>,
}

impl ReviewInvalidation {
    pub(super) fn new(app: &AppState, workspace: &str) -> Self {
        Self {
            workspace: workspace.to_owned(),
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

    pub(super) fn install(&self, app: &mut AppState) {
        if let Some(observed) = self.observed.take() {
            app.install_workspace_review_observation(&self.workspace, observed);
        }
    }

    pub(super) fn invalidate(&self, app: &mut AppState, project: &str, task: &str) {
        app.publish_review_observation(project, task, &self.workspace, self.observed.take());
        super::lifecycle::invalidates(app, &self.workspace);
    }
}
