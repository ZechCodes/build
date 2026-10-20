//! Accepted-root fragments. Candidate preparation never mutates the old cache.
use super::*;
use crate::app::read_view::SharedJson;

#[derive(Clone, Default)]
pub(in crate::app) struct GlobalProjection {
    fragments: BTreeMap<String, SharedJson>,
}

impl GlobalProjection {
    pub(in crate::app) fn patch_shared(
        &self,
        patch: &mut impl FnMut(&SharedJson) -> SharedJson,
    ) -> Self {
        Self {
            fragments: self.fragments.iter().map(|(key, row)| (key.clone(), patch(row))).collect(),
        }
    }
}

impl AppState {
    pub(in crate::app) fn prepare_incremental_owner_snapshot(
        &self,
        _delta: SnapshotDelta,
        _previous: &ReadView,
        _owners: &BTreeSet<String>,
    ) -> Option<SnapshotDelta> { None }

    pub(in crate::app) fn prepare_incremental_task_snapshot(
        &self,
        _delta: SnapshotDelta,
        _previous: &ReadView,
    ) -> Option<SnapshotDelta> { None }

    pub(in crate::app) fn prepare_full_global_snapshot(
        &self,
        mut delta: SnapshotDelta,
        previous: &ReadView,
    ) -> SnapshotDelta {
        let board = self.board_snapshot();
        let projects = self.snapshot_project_list();
        let workspaces = self.workspace_list_snapshot(&json!({})).unwrap_or_else(|error| {
            eprintln!("workspace snapshot: {error}");
            previous.get_owned("workspace.list", "").expect("workspace observations warmed before publication")
        });
        let mut lists = json!({"projects": projects["projects"], "usage_limits": board["usage_limits"]});
        lists["workspaces"] = snapshot_push_workspace_rows(&workspaces["workspaces"], &board["workspace_summaries"]);
        delta = put_changed(delta, previous, "board.list", "", board);
        delta = put_changed(delta, previous, "project.list", "", projects);
        delta = self.prepare_workspace_lists(delta, previous, &workspaces);
        delta = put_changed(delta, previous, "workspace.list", "", workspaces);
        delta = put_changed(delta, previous, "settings.get", "", self.settings_get_observed(self.snapshot_account_availability()));
        delta = put_changed(delta, previous, "models.list", "", self.models_list_snapshot());
        delta.set_facts(Arc::new(self.push_facts_snapshot(lists)))
    }
}
