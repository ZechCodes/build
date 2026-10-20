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
        let live = self
            .plans
            .iter()
            .map(|(owner, plan)| (owner, &plan.agents))
            .chain(self.runs.iter().map(|(owner, run)| (owner, &run.agents)))
            .flat_map(|(owner, roster)| roster.agents().iter().map(move |agent| (owner, agent)))
            .filter_map(|(owner, agent)| {
                self.live_task_identity(&agent.id)
                    .map(|identity| (owner, identity))
            })
            .map(|(owner, identity)| {
                (
                    identity.agent_id,
                    LiveTaskLocation {
                        owner: owner.clone(),
                        workspace_id: identity.workspace_id,
                        workspace_name: identity.workspace_name,
                        available: identity.available,
                    },
                )
            })
            .collect();
        let mut workspaces = HashMap::new();
        for workspace in self.workspaces.list(None) {
            workspaces
                .entry(workspace.root.clone())
                .or_insert_with(|| workspace.id.clone());
            if let Ok(path) = std::fs::canonicalize(&workspace.root) {
                workspaces
                    .entry(path)
                    .or_insert_with(|| workspace.id.clone());
            }
        }
        // Retain aliases known at publication. SQL readers only compare these
        // immutable paths, even after the corresponding directory disappears.
        for run in self.runs.values() {
            if let Ok(path) = std::fs::canonicalize(&run.worktree.path) {
                if let Some(id) = workspaces.get(&path).cloned() {
                    workspaces.entry(run.worktree.path.clone()).or_insert(id);
                }
            }
        }
        TaskIdentityContext { live, workspaces }
    }
}

impl TaskIdentityContext {
    pub(super) fn retain_live_owners(
        &mut self,
        previous: &Self,
        app: &AppState,
        published: Option<(&str, bool)>,
    ) {
        self.live.retain(
            |id, live| match published.filter(|(owner, _)| *owner == live.owner) {
                Some((_, true)) => true,
                Some((_, false)) => match previous.live.get(id) {
                    Some(prior) => {
                        *live = prior.clone();
                        true
                    }
                    None => false,
                },
                None => previous.live.contains_key(id),
            },
        );
        for (id, live) in &previous.live {
            let present = app.plans.contains_key(&live.owner) || app.runs.contains_key(&live.owner);
            let publishing = published.is_some_and(|(owner, _)| owner == live.owner);
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

fn unknown_identity(id: &str) -> TaskAgentIdentity {
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
