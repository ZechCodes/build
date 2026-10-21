use super::layout::Layout;
use super::rows::{self, Row, TaskRow};
use super::*;

#[derive(Clone, Default)]
pub(in crate::app) struct GlobalProjection {
    pub(super) layout: Option<Arc<Layout>>,
    pub(super) structure: Option<Arc<crate::app::workspaces::ProjectionStructure>>,
    pub(super) board_fields: BTreeMap<String, SharedJson>,
    pub(super) runs: Vec<Row>,
    pub(super) winners: Vec<Row>,
    pub(super) captures: Vec<Row>,
    pub(super) projects: Vec<Row>,
    pub(super) workspaces: Vec<Row>,
    pub(super) summaries: Vec<Row>,
    pub(super) tasks: Vec<TaskRow>,
    #[cfg(test)]
    pub(in crate::app) task_input_revision: Option<u64>,
    pub(super) active_owners: BTreeSet<String>,
    pub(super) stat_versions: Arc<BTreeMap<String, Option<std::time::Instant>>>,
    pub(super) agent_states: Arc<BTreeMap<String, Vec<crate::agent::AgentLifecycle>>>,
}

impl GlobalProjection {
    #[cfg(test)]
    pub(in crate::app) fn active_owners_for_test(&self) -> &BTreeSet<String> {
        &self.active_owners
    }

    #[cfg(test)]
    pub(in crate::app) fn named_winner_for_test(&self, owner: &str) -> Option<SharedJson> {
        self.winners
            .iter()
            .find(|row| row.id == owner)
            .map(|row| row.value.clone())
    }

    pub(in crate::app) fn ownership_structure(
        &self,
    ) -> Option<&Arc<crate::app::workspaces::ProjectionStructure>> {
        self.structure.as_ref()
    }

    /// Exactly the owner already captured with the accepted row. A present
    /// row with no owner is a valid selected-none, not an unknown workspace.
    pub(in crate::app) fn mcp_workspace_selection(&self, id: &str) -> Option<(&str, Option<&str>)> {
        self.workspaces
            .iter()
            .find(|row| row.id == id)
            .map(|row| (row.project.as_str(), row.owner.as_deref()))
    }
    pub(in crate::app) fn mcp_project_selection(&self, id: &str) -> Option<Option<&str>> {
        self.projects
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.owner.as_deref())
    }

    pub(in crate::app) fn patch_shared(
        &self,
        patch: &mut impl FnMut(&SharedJson) -> SharedJson,
    ) -> Self {
        Self {
            layout: self.layout.clone(),
            structure: self.structure.clone(),
            board_fields: self
                .board_fields
                .iter()
                .map(|(key, value)| (key.clone(), patch(value)))
                .collect(),
            runs: self.runs.iter().map(|row| row.patch(patch)).collect(),
            winners: self.winners.iter().map(|row| row.patch(patch)).collect(),
            captures: self.captures.iter().map(|row| row.patch(patch)).collect(),
            projects: self.projects.iter().map(|row| row.patch(patch)).collect(),
            workspaces: self.workspaces.iter().map(|row| row.patch(patch)).collect(),
            summaries: self.summaries.iter().map(|row| row.patch(patch)).collect(),
            tasks: self
                .tasks
                .iter()
                .map(|row| TaskRow {
                    value: patch(&row.value),
                    ..row.clone()
                })
                .collect(),
            #[cfg(test)]
            task_input_revision: self.task_input_revision,
            active_owners: self.active_owners.clone(),
            stat_versions: self.stat_versions.clone(),
            agent_states: self.agent_states.clone(),
        }
    }

    pub(super) fn seed(
        app: &AppState,
        board: Value,
        projects: Value,
        workspaces: Value,
        reusable: bool,
    ) -> Self {
        let Value::Object(mut board) = board else {
            unreachable!("board snapshot object")
        };
        let runs = rows::take_rows(&mut board, "runs", "run_id", false);
        let (mut winners, captures) = split_items(board.remove("items"));
        let summaries = rows::take_rows(&mut board, "workspace_summaries", "workspace_id", true);
        let winners = app
            .runs
            .keys()
            .filter_map(|id| winners.remove(id))
            .collect();
        let layout = reusable.then(|| Arc::new(Layout::capture(app)));
        let active_owners = layout
            .as_ref()
            .map(|layout| layout.expand_owners(app, &BTreeSet::new()))
            .unwrap_or_default();
        let tasks = rows::tasks(app, &[]);
        Self {
            structure: reusable
                .then(|| app.snapshot_projection_structure())
                .flatten(),
            layout,
            runs,
            winners,
            captures,
            summaries,
            active_owners,
            stat_versions: Arc::new(stat_versions(app)),
            agent_states: Arc::new(agent_states(app)),
            tasks: tasks.rows,
            #[cfg(test)]
            task_input_revision: Some(tasks.revision),
            projects: wrapped_rows(projects, "projects", "project_id", false),
            workspaces: wrapped_rows(workspaces, "workspaces", "workspace_id", true),
            board_fields: board
                .into_iter()
                .map(|(key, value)| (key, SharedJson::value(value)))
                .collect(),
        }
    }

    pub(super) fn board(&self) -> SharedJson {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.incremental.board_json");
        let mut fields = self.board_fields.clone();
        fields.insert("runs".into(), rows::array(&self.runs));
        fields.insert("workspace_summaries".into(), rows::array(&self.summaries));
        #[cfg(test)]
        let _item_order_phase =
            crate::timing::phase_samples::phase("publication.incremental.item_order");
        let mut items = self
            .winners
            .iter()
            .chain(&self.captures)
            .map(|row| (row.anchor.as_deref(), row.value.clone()))
            .chain(
                self.tasks
                    .iter()
                    .map(|row| (row.source["anchor"].as_str(), row.value.clone())),
            )
            .collect::<Vec<_>>();
        items.sort_by(|(left, _), (right, _)| crate::branch::compare_anchors(*left, *right));
        #[cfg(test)]
        drop(_item_order_phase);
        fields.insert(
            "items".into(),
            SharedJson::array(items.into_iter().map(|(_, value)| value)),
        );
        SharedJson::object(fields)
    }

    pub(super) fn lists(&self) -> SharedJson {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.incremental.list_json");
        SharedJson::object([
            ("projects".into(), rows::array(&self.projects)),
            (
                "workspaces".into(),
                rows::push_workspaces(&self.workspaces, &self.summaries),
            ),
            (
                "usage_limits".into(),
                self.board_fields["usage_limits"].clone(),
            ),
        ])
    }

    pub(super) fn publish(
        &self,
        app: &AppState,
        mut delta: SnapshotDelta,
        previous: &ReadView,
    ) -> SnapshotDelta {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("publication.incremental.publish");
        delta = rows::put(delta, previous, "board.list", "", self.board());
        delta = rows::put(
            delta,
            previous,
            "project.list",
            "",
            rows::wrapped("projects", &self.projects),
        );
        delta = rows::put(
            delta,
            previous,
            "workspace.list",
            "",
            rows::wrapped("workspaces", &self.workspaces),
        );
        #[cfg(test)]
        let _project_lists_phase =
            crate::timing::phase_samples::phase("publication.incremental.project_lists");
        let projects = app
            .projects
            .iter()
            .map(|project| project.id.as_str())
            .collect::<BTreeSet<_>>();
        for key in previous
            .keys()
            .filter(|key| key.method == "workspace.list" && !key.owner.is_empty())
        {
            if !projects.contains(key.owner.as_str()) {
                delta = delta.remove(key.method, key.owner.clone());
            }
        }
        for project in projects {
            let rows = self
                .workspaces
                .iter()
                .filter(|row| row.project == project)
                .map(|row| row.value.clone());
            delta = rows::put(
                delta,
                previous,
                "workspace.list",
                project,
                SharedJson::object([("workspaces".into(), SharedJson::array(rows))]),
            );
        }
        #[cfg(test)]
        drop(_project_lists_phase);
        delta = put_changed(
            delta,
            previous,
            "settings.get",
            "",
            app.settings_get_observed(app.snapshot_account_availability()),
        );
        let delta = put_changed(
            delta,
            previous,
            "models.list",
            "",
            app.models_list_snapshot(),
        );
        #[cfg(test)]
        let delta = delta.set_task_publication_coverage(self.task_input_revision);
        delta
    }
}

pub(super) fn captures(app: &AppState) -> Vec<Row> {
    #[cfg(test)]
    let _phase = crate::timing::phase_samples::phase("publication.incremental.captures");
    crate::branch::fold_work_items(app.capture_candidates())
        .into_iter()
        .map(|value| Row::new("capture_id", value))
        .collect()
}

fn wrapped_rows(value: Value, field: &str, id: &str, structured: bool) -> Vec<Row> {
    let Value::Object(mut fields) = value else {
        return Vec::new();
    };
    rows::take_rows(&mut fields, field, id, structured)
}

pub(super) fn stat_versions(app: &AppState) -> BTreeMap<String, Option<std::time::Instant>> {
    app.runs
        .keys()
        .map(|id| {
            (
                id.clone(),
                app.board.diff().run_stat(id).map(|stat| stat.computed_at),
            )
        })
        .collect()
}

pub(super) fn agent_states(app: &AppState) -> BTreeMap<String, Vec<crate::agent::AgentLifecycle>> {
    app.plans
        .iter()
        .map(|(id, active)| (id, &active.agents))
        .chain(app.runs.iter().map(|(id, active)| (id, &active.agents)))
        .map(|(id, agents)| (id.clone(), agents.iter().map(|agent| agent.state).collect()))
        .collect()
}

fn split_items(items: Option<Value>) -> (HashMap<String, Row>, Vec<Row>) {
    let mut winners = HashMap::new();
    let mut captures = Vec::new();
    let values = match items {
        Some(Value::Array(values)) => values,
        _ => Vec::new(),
    };
    for value in values {
        if value["kind"] == "capture" {
            captures.push(Row::new("capture_id", value));
        } else if value["kind"] == "branch" {
            let row = Row::new("run_id", value);
            winners.insert(row.id.clone(), row);
        }
    }
    (winners, captures)
}
