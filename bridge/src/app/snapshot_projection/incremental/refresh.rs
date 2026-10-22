use super::cache::{agent_states, captures, stat_versions, GlobalProjection};
use super::layout::Layout;
use super::rows::{self, Row};
use super::*;
use crate::app::DigestScope;
use crate::thread::ThreadDetail;

impl GlobalProjection {
    pub(super) fn refresh(
        &self,
        app: &AppState,
        requested: &BTreeSet<String>,
    ) -> Option<(Self, BTreeSet<String>)> {
        #[cfg(test)]
        let _layout_phase =
            crate::timing::phase_samples::phase("publication.incremental.layout_check");
        let projects = app.project_list_rows();
        let current = Arc::new(Layout::capture_with_projects(app, Some(&projects)));
        if self.layout.as_ref()? != &current {
            return None;
        }
        #[cfg(test)]
        drop(_layout_phase);
        #[cfg(test)]
        let _owners_phase =
            crate::timing::phase_samples::phase("publication.incremental.owner_inputs");
        let mut requested = requested.clone();
        requested.extend(self.active_owners.iter().cloned());
        let agent_states = agent_states(app);
        requested.extend(
            agent_states
                .iter()
                .filter(|(id, states)| self.agent_states.get(*id) != Some(*states))
                .map(|(id, _)| id.clone()),
        );
        let stat_versions = stat_versions(app);
        requested.extend(
            stat_versions
                .iter()
                .filter(|(id, token)| self.stat_versions.get(*id) != Some(*token))
                .map(|(id, _)| id.clone()),
        );
        let (owners, active_owners) = current.expand_owner_sets(app, &requested);
        #[cfg(test)]
        super::super::work_counts::owner_closure::outputs(&owners, &active_owners);
        #[cfg(test)]
        drop(_owners_phase);
        #[cfg(test)]
        let _clone_phase =
            crate::timing::phase_samples::phase("publication.incremental.cache_clone");
        let mut next = self.clone();
        next.layout = Some(current);
        next.structure = app.snapshot_projection_structure();
        next.active_owners = active_owners;
        next.stat_versions = Arc::new(stat_versions);
        next.agent_states = Arc::new(agent_states);
        #[cfg(test)]
        drop(_clone_phase);
        refresh_runs(app, &owners, &mut next.runs);
        refresh_winners(app, &owners, &mut next.winners);
        refresh_workspaces(app, &owners, &mut next.workspaces)?;
        refresh_projects(app, &owners, &mut next.projects, projects);
        next.tasks = rows::tasks(app, &self.tasks);
        next.captures = captures(app);
        #[cfg(test)]
        let _summaries_phase =
            crate::timing::phase_samples::phase("publication.incremental.summary_rows");
        next.summaries = app
            .cached_workspace_summaries_json()
            .into_iter()
            .map(|value| Row::object("workspace_id", value))
            .collect();
        #[cfg(test)]
        drop(_summaries_phase);
        #[cfg(test)]
        let _external_phase =
            crate::timing::phase_samples::phase("publication.incremental.external_rows");
        let checkouts = app.cached_external_worktrees_json();
        next.board_fields.insert(
            "external_worktrees".into(),
            SharedJson::value(Value::Array(checkouts.rows)),
        );
        next.board_fields.insert(
            "scanning".into(),
            SharedJson::value(Value::Bool(checkouts.scanning)),
        );
        #[cfg(test)]
        drop(_external_phase);
        #[cfg(test)]
        let _dynamic_phase =
            crate::timing::phase_samples::phase("publication.incremental.pending_usage");
        next.board_fields.insert(
            "pending".into(),
            SharedJson::value(Value::Array(app.pending_rows_json())),
        );
        next.board_fields.insert(
            "usage_limits".into(),
            SharedJson::value(app.usage_limits_json()),
        );
        #[cfg(test)]
        drop(_dynamic_phase);
        Some((next, owners))
    }
}

fn run_stat(app: &AppState, owner: &str) -> Value {
    app.board
        .diff()
        .run_stat(owner)
        .map(|stat| stat.value.clone())
        .unwrap_or(Value::Null)
}

fn refresh_runs(app: &AppState, owners: &BTreeSet<String>, rows: &mut [Row]) {
    #[cfg(test)]
    let _phase = crate::timing::phase_samples::phase("publication.incremental.run_rows");
    for row in rows.iter_mut().filter(|row| owners.contains(&row.id)) {
        let mut value = app.run_view(
            &row.id,
            &app.runs[&row.id],
            ThreadDetail::Digest,
            DigestScope::List,
        );
        value["stat"] = run_stat(app, &row.id);
        *row = Row::new("run_id", value);
    }
}

fn refresh_winners(app: &AppState, owners: &BTreeSet<String>, rows: &mut [Row]) {
    #[cfg(test)]
    let _phase = crate::timing::phase_samples::phase("publication.incremental.winner_rows");
    for row in rows.iter_mut().filter(|row| owners.contains(&row.id)) {
        #[cfg(test)]
        let _source = crate::app::snapshot_projection::work_counts::named_branch::source(
            &row.id,
            crate::app::snapshot_projection::work_counts::named_branch::Source::Winner,
        );
        let named = app.snapshot_named_branch_row(&row.id, &run_stat(app, &row.id));
        *row = Row::from_shared("run_id", named);
    }
}

fn refresh_workspaces(app: &AppState, owners: &BTreeSet<String>, rows: &mut [Row]) -> Option<()> {
    #[cfg(test)]
    let _phase = crate::timing::phase_samples::phase("publication.incremental.workspace_rows");
    for row in rows.iter_mut().filter(|row| {
        row.owner
            .as_ref()
            .is_some_and(|owner| owners.contains(owner))
    }) {
        let workspace = app.workspaces.get(&row.id)?;
        *row = Row::object(
            "workspace_id",
            app.workspace_list_snapshot_row(workspace).ok()?,
        );
    }
    Some(())
}

fn refresh_projects(
    app: &AppState,
    owners: &BTreeSet<String>,
    rows: &mut [Row],
    captured: Vec<crate::app::ProjectListRow>,
) {
    #[cfg(test)]
    let _phase = crate::timing::phase_samples::phase("publication.incremental.project_rows");
    #[cfg(test)]
    let _project_filter_phase =
        crate::timing::phase_samples::phase("publication.incremental.project_capture_filter");
    let projects = owners
        .iter()
        .filter_map(|owner| app.projects.project_id_of(owner))
        .chain(owners.iter().map(String::as_str))
        .collect::<BTreeSet<_>>();
    let observed = app.snapshot_observations.projects.lock().unwrap();
    let mut captured = captured
        .into_iter()
        .filter(|row| projects.contains(row.project_id.as_str()))
        .map(|row| (row.project_id.clone(), row))
        .collect::<HashMap<_, _>>();
    #[cfg(test)]
    drop(_project_filter_phase);
    for row in rows {
        let Some(captured) = captured.remove(&row.id) else {
            continue;
        };
        let observation = observed
            .get(&row.id)
            .expect("live project observation warmed");
        let value = captured.render_observed(
            observation.value["isolation_available"].clone(),
            Some(&observation.value),
        );
        *row = Row::new("project_id", value);
    }
}
