//! Versioned sparse input deltas, published at the existing commit boundary.
use super::*;
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::app) enum InputKey {
    Owner(String),
    Project(String),
    Workspace(String),
    Reservation(String),
    Archives,
    Topology,
    ProjectOrder,
    Gate,
    Terminals,
}

#[derive(Default)]
pub(in crate::app) struct ObservationPatch {
    scopes: scopes::ScopePatch,
    mixed: mixed::MixedPatch,
    keys: BTreeSet<InputKey>,
    deleting_project: Option<bool>,
}

impl ObservationPatch {
    pub(in crate::app) fn owner(
        &mut self,
        app: &AppState,
        id: &str,
        previous: &ObservationContext,
    ) {
        if self.keys.contains(&InputKey::Owner(id.into())) {
            return;
        }
        self.keys.insert(InputKey::Owner(id.into()));
        self.scopes.owner(app, id);
        self.mixed.owner(app, id);
        for dependent in previous.scopes.ownership_dependents(app, id) {
            self.owner(app, &dependent, previous);
        }
        let present = app.plans.contains_key(id) || app.runs.contains_key(id);
        if previous.mixed.contains_owner(id) != present
            || previous.scopes.contains_run(id) != app.runs.contains_key(id)
        {
            self.keys.insert(InputKey::Topology);
            self.scopes.topology(app);
            self.mixed.topology(app);
            // HashMap membership changes can reorder legacy fallback winners
            // at any shared root. This thin topology pass never runs on status.
            for workspace in app.workspaces.list(None) {
                let before = previous.scopes.workspace_dependents(&workspace.id);
                let after = app.workspace_snapshot_conversation_owner(workspace);
                if before.as_slice() != after.as_slice() {
                    for owner in before.into_iter().chain(after) {
                        self.owner(app, &owner, previous);
                    }
                }
            }
        }
    }
    fn project(&mut self, app: &AppState, id: &str, previous: &ObservationContext) {
        self.keys.insert(InputKey::Project(id.into()));
        self.scopes.project(app, id);
        self.mixed.project(app, id);
        if previous.scopes.contains_project(id) != app.projects.get(id).is_some() {
            self.keys.insert(InputKey::ProjectOrder);
            self.mixed.project_topology(app);
        }
    }
    fn workspace(&mut self, app: &AppState, id: &str, previous: &ObservationContext) {
        self.keys.insert(InputKey::Workspace(id.into()));
        self.scopes.workspace(app, id);
        self.mixed.workspace(app, id);
        let mut dependents = previous.scopes.workspace_dependents(id);
        if let Some(owner) = app
            .workspaces
            .get(id)
            .and_then(|workspace| app.workspace_snapshot_conversation_owner(workspace))
        {
            dependents.push(owner);
        }
        for owner in dependents {
            self.owner(app, &owner, previous);
        }
    }
    pub(in crate::app) fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
    pub(in crate::app) fn accepts(&self, context: &ObservationContext, version: u64) -> bool {
        self.keys.iter().all(|key| {
            context
                .versions
                .get(key)
                .copied()
                .unwrap_or(context.base_version)
                .max(context.base_version)
                < version
        })
    }
    pub(in crate::app) fn apply(
        &self,
        context: &ObservationContext,
        version: u64,
    ) -> ObservationContext {
        let mut next = context.clone();
        next.scopes.apply(&self.scopes);
        next.mixed.apply(&self.mixed);
        if let Some(deleting) = self.deleting_project {
            next.deleting_project = deleting;
        }
        for key in &self.keys {
            next.versions.insert(key.clone(), version);
        }
        next
    }
}

#[derive(Default)]
pub(in crate::app) struct ObservationReceipts {
    owners: Vec<(String, u64)>,
    projects: Vec<(String, u64)>,
    bindings: Vec<(String, u64)>,
    workspaces: Vec<(String, u64)>,
    external: Vec<(String, u64)>,
    archives: Vec<(String, u64)>,
    reservations: Vec<(String, u64)>,
}

impl ObservationReceipts {
    fn capture(app: &AppState) -> Self {
        Self {
            owners: app.snapshot_observations.observation_owners.snapshot(),
            projects: app.projects.observation_project_changes().snapshot(),
            bindings: app.projects.observation_binding_changes().snapshot(),
            workspaces: app.workspaces.observation_changes().snapshot(),
            external: app.board.diff().observation_changes().snapshot(),
            archives: app.board.observation_changes().snapshot(),
            reservations: app
                .snapshot_observations
                .observation_reservations
                .snapshot(),
        }
    }
    pub(in crate::app) fn acknowledge(self, app: &AppState) {
        app.snapshot_observations
            .observation_owners
            .acknowledge(&self.owners);
        app.projects
            .observation_project_changes()
            .acknowledge(&self.projects);
        app.projects
            .observation_binding_changes()
            .acknowledge(&self.bindings);
        app.workspaces
            .observation_changes()
            .acknowledge(&self.workspaces);
        app.board
            .diff()
            .observation_changes()
            .acknowledge(&self.external);
        app.board.observation_changes().acknowledge(&self.archives);
        app.snapshot_observations
            .observation_reservations
            .acknowledge(&self.reservations);
    }
}

impl AppState {
    pub(in crate::app) fn prepare_observation_inputs(
        &self,
        delta: super::super::read_view::SnapshotDelta,
        previous: &super::super::read_view::ReadView,
    ) -> super::super::read_view::SnapshotDelta {
        let receipts = ObservationReceipts::capture(self);
        let Some(context) = previous.observations.as_ref() else {
            return delta
                .set_observations(Arc::new(ObservationContext::capture(self)))
                .set_observation_receipts(receipts);
        };
        let mut patch = ObservationPatch::default();
        if context.scopes.terminals_changed(self) {
            patch.keys.insert(InputKey::Terminals);
            patch.scopes.terminals(self);
        }
        let owners = receipts
            .owners
            .iter()
            .chain(&receipts.bindings)
            .map(|(id, _)| id)
            .collect::<BTreeSet<_>>();
        for id in owners {
            patch.owner(self, id, context);
        }
        let projects = receipts
            .projects
            .iter()
            .chain(&receipts.external)
            .map(|(id, _)| id)
            .collect::<BTreeSet<_>>();
        for id in projects {
            patch.project(self, id, context);
        }
        for (id, _) in &receipts.workspaces {
            patch.workspace(self, id, context);
        }
        for (id, _) in &receipts.reservations {
            patch.keys.insert(InputKey::Reservation(id.clone()));
            patch.mixed.reservation(self, id);
        }
        if !receipts.archives.is_empty() {
            patch.keys.insert(InputKey::Archives);
            patch.mixed.archives(self);
        }
        if context.deleting_project != self.project_deletion_in_progress {
            patch.keys.insert(InputKey::Gate);
            patch.deleting_project = Some(self.project_deletion_in_progress);
        }
        delta
            .patch_observations(patch)
            .set_observation_receipts(receipts)
    }

    /// Actual shell membership commits independently of borrowed plan/run
    /// candidates. Publish only this family, preserving all other barriers.
    pub(in crate::app) fn publish_terminal_observations(&self) {
        if !self.snapshot_projection_initialized() {
            return;
        }
        let view = self.read_view.load();
        let Some(context) = view.observations.as_ref() else {
            return;
        };
        if !context.scopes.terminals_changed(self) {
            return;
        }
        let mut patch = ObservationPatch::default();
        patch.keys.insert(InputKey::Terminals);
        patch.scopes.terminals(self);
        self.read_view.publish_delta(
            super::super::read_view::SnapshotDelta::new(self.read_view.next_version())
                .patch_observations(patch),
        );
    }

    pub(in crate::app) fn mark_observation_owner_changed(&self, id: &str) {
        self.snapshot_observations.observation_owners.mark(id);
    }
    pub(in crate::app) fn mark_observation_reservation_changed(&self, id: &str) {
        self.snapshot_observations.observation_reservations.mark(id);
    }
}
