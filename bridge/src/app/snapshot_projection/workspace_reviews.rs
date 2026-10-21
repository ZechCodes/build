//! Review observations can load outside the app lock and settle in any order.
use super::*;
use crate::app::read_view::ReadViewSlot;
use crate::store::Store;

pub(in crate::app) struct WorkspaceReviewObservation {
    version: u64,
    pub(super) value: Value,
}

impl WorkspaceReviewObservation {
    /// Allocate the observation generation after the service's durable write,
    /// before querying SQL. Later producers fence this result at installation.
    pub(in crate::app) fn load(
        read_view: &ReadViewSlot,
        store: Option<&Store>,
        workspace: &str,
    ) -> Result<Self, String> {
        let version = read_view.next_version();
        let mut value = json!({});
        if let Some(summary) = store
            .map(|store| store.load_workspace_review_summary(workspace))
            .transpose()
            .map_err(|error| error.to_string())?
            .flatten()
        {
            value["active_review"] = json!(summary);
        }
        Ok(Self { version, value })
    }
}

impl AppState {
    pub(in crate::app) fn install_workspace_review_observation(
        &mut self,
        id: &str,
        observed: WorkspaceReviewObservation,
    ) {
        if self.workspaces.get(id).is_none() {
            return;
        }
        let observations = &mut self.snapshot_observations.workspace_reviews;
        if observations
            .get(id)
            .is_none_or(|current| current.version < observed.version)
        {
            observations.insert(id.to_owned(), observed);
        }
    }

    pub(in crate::app) fn refresh_workspace_review_observation(&mut self, id: &str) {
        match WorkspaceReviewObservation::load(&self.read_view, self.store.as_ref(), id) {
            Ok(observed) => self.install_workspace_review_observation(id, observed),
            Err(error) => eprintln!("workspace review observation {id}: {error}"),
        }
    }

    pub(in crate::app) fn snapshot_workspace_review(&self, id: &str, row: &mut Value) {
        if let Some(summary) = self
            .snapshot_observations
            .workspace_reviews
            .get(id)
            .and_then(|observed| observed.value.get("active_review"))
        {
            row["active_review"] = summary.clone();
        }
    }
}
