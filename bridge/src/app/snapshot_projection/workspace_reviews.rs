//! Review observations can load outside the app lock and settle in any order.
use super::plan_docs::PlanDocObservation;
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

    #[cfg(test)]
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

/// Maintenance captures workspace membership with its physical observations.
/// SQL runs in the existing unlocked phase, before the publication guard.
#[must_use = "load observations off-lock, then install them before publication"]
pub(in crate::app) struct WorkspaceReviewRefresh {
    reader: WorkspaceReviewReader,
    workspaces: Vec<String>,
    plan_docs: Vec<String>,
}

pub(in crate::app) struct RenderedWorkspaceReviewRefresh {
    observations: Vec<(String, WorkspaceReviewObservation)>,
    plan_docs: Vec<(String, PlanDocObservation)>,
}

impl WorkspaceReviewRefresh {
    pub(super) fn new(
        reader: WorkspaceReviewReader,
        workspaces: Vec<String>,
        plan_docs: Vec<String>,
    ) -> Self {
        Self {
            reader,
            workspaces,
            plan_docs,
        }
    }

    pub(in crate::app) fn load(self) -> RenderedWorkspaceReviewRefresh {
        RenderedWorkspaceReviewRefresh {
            observations: self
                .workspaces
                .into_iter()
                .filter_map(|id| self.reader.load(&id).map(|observed| (id, observed)))
                .collect(),
            plan_docs: self
                .plan_docs
                .into_iter()
                .map(|owner| {
                    let observed = PlanDocObservation::load(
                        &self.reader.read_view,
                        self.reader.store.as_ref(),
                        &owner,
                    );
                    (owner, observed)
                })
                .collect(),
        }
    }
}

impl AppState {
    pub(in crate::app) fn install_workspace_review_refresh(
        &mut self,
        refresh: RenderedWorkspaceReviewRefresh,
    ) {
        for (id, observed) in refresh.observations {
            self.install_workspace_review_observation(&id, observed);
        }
        for (owner, observed) in refresh.plan_docs {
            self.install_plan_doc_observation(owner, observed);
        }
    }
}

/// Owned inputs captured at admission/startup; querying never needs AppState.
pub(in crate::app) struct WorkspaceReviewReader {
    read_view: ReadViewSlot,
    store: Option<Store>,
    #[cfg(test)]
    before: Option<crate::app::OffLockGate>,
    #[cfg(test)]
    loaded: Option<crate::app::OffLockGate>,
}

impl WorkspaceReviewReader {
    pub(in crate::app) fn load(&self, workspace: &str) -> Option<WorkspaceReviewObservation> {
        #[cfg(test)]
        if let Some(gate) = &self.before {
            gate.arrive();
        }
        let observed =
            WorkspaceReviewObservation::load(&self.read_view, self.store.as_ref(), workspace);
        #[cfg(test)]
        if let Some(gate) = &self.loaded {
            gate.arrive();
        }
        match observed {
            Ok(observed) => Some(observed),
            Err(error) => {
                eprintln!("workspace review observation {workspace}: {error}");
                None
            }
        }
    }
}

impl AppState {
    pub(in crate::app) fn workspace_review_reader(&self) -> WorkspaceReviewReader {
        WorkspaceReviewReader {
            read_view: self.read_view.clone(),
            store: self.store.clone(),
            #[cfg(test)]
            before: self.off_lock_review_observation_gate.clone(),
            #[cfg(test)]
            loaded: self.off_lock_review_observation_loaded.clone(),
        }
    }
}
