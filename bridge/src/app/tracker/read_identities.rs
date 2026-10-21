//! Live availability is an immutable observation; durable names come from the
//! same SQL snapshot as the task, so a failed roster save cannot leak into it.
use super::identities::{mentioned, plan_identities, retain_known_fields};
use super::read_source::TaskReadSource;
use crate::app::AppState;
use crate::store::PersistedRun;
use crate::tracker::{Task, TaskAgentIdentity, TimelineEntry};
use std::cell::OnceCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Default)]
pub(super) struct TaskIdentityContext {
    live: HashMap<String, LiveTaskLocation>,
    workspaces: HashMap<PathBuf, String>,
}

#[derive(Clone)]
struct LiveTaskLocation {
    owner: String,
    workspace_id: Option<String>,
    workspace_name: Option<String>,
    available: bool,
}

#[derive(Default)]
pub(super) struct ReadRosters {
    known: OnceCell<HashMap<String, TaskAgentIdentity>>,
}

impl AppState {
    pub(super) fn task_identity_context(&self) -> TaskIdentityContext {
        let _ownership = self.begin_snapshot_ownership();
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("read_context.task_identity");
        let context = self
            .snapshot_ownership_context()
            .expect("ownership guard captures a context");
        let live = self
            .plans
            .iter()
            .map(|(owner, plan)| (owner, &plan.agents))
            .chain(self.runs.iter().map(|(owner, run)| (owner, &run.agents)))
            .flat_map(|(owner, roster)| roster.agents().iter().map(move |agent| (owner, agent)))
            .filter_map(|(owner, agent)| {
                self.task_live_location(&context, owner, &agent.id)
                    .map(|location| (agent.id.clone(), location))
            })
            .collect();
        let workspaces = context.task_paths().clone();
        TaskIdentityContext { live, workspaces }
    }

    fn task_live_location(
        &self,
        context: &crate::app::workspaces::ProjectionOwnership,
        owner: &str,
        agent: &str,
    ) -> Option<LiveTaskLocation> {
        let project_agent = crate::agent::is_project_agent(agent);
        let workspace = if project_agent {
            None
        } else {
            self.runs.get(owner)?;
            self.projects.project_id_of(owner)?;
            context
                .task_workspace(owner)
                .and_then(|id| self.workspaces.get(id))
        };
        Some(LiveTaskLocation {
            owner: owner.into(),
            workspace_id: workspace.map(|workspace| workspace.id.clone()),
            workspace_name: workspace.map(|workspace| workspace.name.clone()),
            available: !self.session_registry.agent_binding_retired(agent)
                && (project_agent
                    || workspace.is_some_and(|workspace| {
                        workspace.status == crate::workspace::WorkspaceStatus::Ready
                    })),
        })
    }
}

impl TaskIdentityContext {
    pub(super) fn retain_live_owners(
        &mut self,
        previous: &Self,
        app: &AppState,
        published: Option<(&str, bool)>,
    ) {
        self.retain_with(previous, app, |owner| {
            published
                .filter(|(saved, _)| *saved == owner)
                .map(|(_, saved)| saved)
        });
    }

    pub(super) fn retain_saved_owners(
        &mut self,
        previous: &Self,
        app: &AppState,
        owners: &std::collections::HashSet<String>,
    ) {
        self.retain_with(previous, app, |owner| {
            owners.contains(owner).then_some(true)
        });
    }

    fn retain_with(
        &mut self,
        previous: &Self,
        app: &AppState,
        published: impl Fn(&str) -> Option<bool>,
    ) {
        self.live.retain(|id, live| match published(&live.owner) {
            Some(true) => true,
            Some(false) => match previous.live.get(id) {
                Some(prior) => {
                    // A roster rollback preserves committed association,
                    // but cannot undo actual runtime retirement or start.
                    let available = live.available;
                    *live = prior.clone();
                    live.available = available;
                    true
                }
                None => false,
            },
            None => previous.live.contains_key(id),
        });
        for (id, live) in &previous.live {
            let present = app.plans.contains_key(&live.owner) || app.runs.contains_key(&live.owner);
            let publishing = published(&live.owner).is_some();
            if !present && !publishing {
                self.live.entry(id.clone()).or_insert_with(|| live.clone());
            }
        }
    }

    pub(super) fn forget_live_owner(&mut self, owner: &str) {
        self.live.retain(|_, live| live.owner != owner);
    }

    pub(super) fn read_task(
        &self,
        source: &impl TaskReadSource,
        mut task: Task,
        timeline: &[TimelineEntry],
        rosters: &ReadRosters,
    ) -> Task {
        for id in mentioned(&task, timeline) {
            let known = rosters.known.get_or_init(|| self.stored_identities(source));
            if let Some(durable) = known.get(&id) {
                let identity = self.identity(&id, durable);
                task.identities
                    .entry(id)
                    .and_modify(|prior| {
                        *prior = retain_known_fields(prior, identity.clone());
                    })
                    .or_insert(identity);
            } else {
                task.identities
                    .entry(id.clone())
                    .or_insert_with(|| unknown_identity(&id));
            }
        }
        for identity in task.identities.values_mut() {
            identity.available = self
                .live
                .get(&identity.agent_id)
                .is_some_and(|live| live.available);
        }
        task
    }

    fn identity(&self, id: &str, durable: &TaskAgentIdentity) -> TaskAgentIdentity {
        let mut identity = durable.clone();
        if let Some(live) = self.live.get(id) {
            // Only association and availability are observations. Durable
            // names/provider/purpose/ordinal never come from a failed candidate.
            identity.workspace_id = live.workspace_id.clone().or(identity.workspace_id);
            identity.workspace_name = live.workspace_name.clone().or(identity.workspace_name);
            identity.available = live.available;
        }
        if identity.project_role.is_some() {
            identity.workspace_id = None;
            identity.workspace_name = None;
        }
        identity
    }

    fn stored_identities(
        &self,
        source: &impl TaskReadSource,
    ) -> HashMap<String, TaskAgentIdentity> {
        let Ok(runs) = source.load_all_run_rosters() else {
            return HashMap::new();
        };
        let plans = source.load_all_plan_rosters().unwrap_or_default();
        let mut known = HashMap::new();
        for identity in self.run_identities(&runs).chain(plan_identities(&plans)) {
            known.entry(identity.agent_id.clone()).or_insert(identity);
        }
        known
    }

    fn run_identities<'a>(
        &'a self,
        runs: &'a [PersistedRun],
    ) -> impl Iterator<Item = TaskAgentIdentity> + 'a {
        runs.iter().flat_map(|run| {
            let workspace_id = self.workspaces.get(Path::new(&run.worktree_path)).cloned();
            run.members()
                .into_iter()
                .map(move |member| TaskAgentIdentity {
                    agent_id: member.id,
                    name: member.name,
                    project_role: member.project_role,
                    ordinal: Some(member.ordinal),
                    workspace_id: workspace_id
                        .clone()
                        .filter(|_| member.project_role.is_none()),
                    workspace_name: member
                        .project_role
                        .is_none()
                        .then(|| run.worktree_name.clone()),
                    provider: Some(member.provider.wire_id().to_string()),
                    available: false,
                })
        })
    }
}

pub(super) fn unknown_identity(id: &str) -> TaskAgentIdentity {
    TaskAgentIdentity {
        agent_id: id.to_string(),
        name: None,
        project_role: None,
        ordinal: None,
        workspace_id: None,
        workspace_name: None,
        provider: None,
        available: false,
    }
}

#[cfg(test)]
mod tests;
