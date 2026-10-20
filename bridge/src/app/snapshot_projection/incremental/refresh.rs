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
        let current = Arc::new(Layout::capture(app));
        if self.layout.as_ref()? != &current {
            return None;
        }
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
        let owners = current.expand_owners(app, &requested);
        let active_owners = current.expand_owners(app, &BTreeSet::new());
        let mut next = self.clone();
        next.layout = Some(current);
        next.active_owners = active_owners;
        next.stat_versions = Arc::new(stat_versions);
        next.agent_states = Arc::new(agent_states);
        refresh_runs(app, &owners, &mut next.runs);
        refresh_winners(app, &owners, &mut next.winners);
        refresh_workspaces(app, &owners, &mut next.workspaces)?;
        refresh_projects(app, &owners, &mut next.projects);
        next.tasks = rows::tasks(app, &self.tasks);
        next.captures = captures(app);
        next.summaries = app
            .cached_workspace_summaries_json()
            .into_iter()
            .map(|value| Row::object("workspace_id", value))
            .collect();
        let checkouts = app.cached_external_worktrees_json();
        next.board_fields.insert(
            "external_worktrees".into(),
            SharedJson::value(Value::Array(checkouts.rows)),
        );
        next.board_fields.insert(
            "scanning".into(),
            SharedJson::value(Value::Bool(checkouts.scanning)),
        );
        next.board_fields.insert(
            "pending".into(),
            SharedJson::value(Value::Array(app.pending_rows_json())),
        );
        next.board_fields.insert(
            "usage_limits".into(),
            SharedJson::value(app.usage_limits_json()),
        );
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
    for row in rows.iter_mut().filter(|row| owners.contains(&row.id)) {
        let candidate = app.branch_candidate_from_run(&row.id, &run_stat(app, &row.id));
        *row = Row::new("run_id", crate::branch::named_project_row(candidate.row));
    }
}

fn refresh_workspaces(app: &AppState, owners: &BTreeSet<String>, rows: &mut [Row]) -> Option<()> {
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

fn refresh_projects(app: &AppState, owners: &BTreeSet<String>, rows: &mut [Row]) {
    let projects = owners
        .iter()
        .filter_map(|owner| app.projects.project_id_of(owner))
        .chain(owners.iter().map(String::as_str))
        .collect::<BTreeSet<_>>();
    let observed = app.snapshot_observations.projects.lock().unwrap();
    let mut captured = app
        .project_list_rows()
        .into_iter()
        .filter(|row| projects.contains(row.project_id.as_str()))
        .map(|row| (row.project_id.clone(), row))
        .collect::<HashMap<_, _>>();
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
