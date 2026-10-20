use super::*;
use crate::app::runtime::sessions::ProjectionRegistration;
use crate::workspace::Workspace;
use std::time::Instant;

/// Only structural inputs belong here. Message contents and active session
/// observations are refreshed through the owner closure instead.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Layout {
    owners: Vec<OwnerLayout>,
    projects: Vec<Value>,
    parked_projects: Vec<Value>,
    project_paths: Vec<(PathBuf, PathBuf)>,
    project_sources: Vec<Vec<crate::app::projects::ProjectSource>>,
    workspaces: Vec<Workspace>,
    paths: BTreeMap<PathBuf, PathBuf>,
    existing_paths: BTreeSet<PathBuf>,
    branches: BTreeMap<String, String>,
    reviews: BTreeMap<String, Value>,
    lifecycle: HashMap<String, crate::reclaim::LifecycleRecord>,
    registrations: Vec<ProjectionRegistration>,
    settings: Value,
    render_mode: (bool, String),
    external_versions: Vec<(Option<Instant>, bool)>,
    project_versions: Vec<(String, u64)>,
    workspace_compaction: Vec<Value>,
}

#[derive(Clone, PartialEq, Eq)]
struct OwnerLayout {
    id: String,
    project: String,
    retained_project_path: String,
    root: Option<PathBuf>,
    raw_root: Option<PathBuf>,
    docs_root: Option<PathBuf>,
    structure: Value,
    agents: Vec<AgentLayout>,
}

#[derive(Clone, PartialEq, Eq)]
struct AgentLayout {
    id: String,
    conversation: Option<String>,
    thread: String,
    generation: u64,
    watched: bool,
    choice_revision: u64,
}

impl Layout {
    pub fn capture(app: &AppState) -> Self {
        let workspaces = app
            .workspaces
            .list(None)
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        let workspace_compaction = workspaces
            .iter()
            .map(|workspace| {
                let scope = crate::compaction::CompactionScope::Workspace(workspace.id.clone());
                json!([
                    app.compaction_thresholds(&scope)
                        .unwrap_or_default()
                        .read_value(),
                    app.scope_compaction_observation(&scope)
                ])
            })
            .collect();
        Self {
            owners: owner_layouts(app),
            projects: project_layouts(app),
            parked_projects: app.projects.parked().to_vec(),
            project_paths: app
                .projects
                .iter()
                .map(|project| {
                    (
                        project.repo_path.clone(),
                        app.project_worktrees_root(&project.id),
                    )
                })
                .collect(),
            project_sources: app
                .projects
                .iter()
                .map(|project| project.sources.clone())
                .collect(),
            workspaces,
            paths: app.snapshot_observations.paths.clone(),
            existing_paths: app.snapshot_observations.existing_paths.clone(),
            branches: app.snapshot_observations.branches.clone(),
            reviews: app.snapshot_observations.workspace_reviews.clone(),
            lifecycle: app.workspace_lifecycle.clone(),
            registrations: app.session_registry.projection_layout(),
            settings: app.settings_get_observed(app.snapshot_account_availability()),
            render_mode: (app.qa_agent, app.harness.as_str().to_owned()),
            external_versions: app
                .projects
                .iter()
                .map(|project| {
                    let scan = app.board.diff().external_scan(&project.id);
                    (scan.settled_at, scan.has_readable_scan)
                })
                .collect(),
            project_versions: app
                .snapshot_observations
                .projects
                .lock()
                .unwrap()
                .iter()
                .map(|(id, observed)| (id.clone(), observed.version))
                .collect(),
            workspace_compaction,
        }
    }

    pub fn expand_owners(&self, app: &AppState, requested: &BTreeSet<String>) -> BTreeSet<String> {
        let mut owners = requested.clone();
        owners.extend(app.session_registry.projection_agent_owners());
        owners.extend(
            self.owners
                .iter()
                .filter(|owner| {
                    app.entity_agents_working(&owner.id)
                        || app.board.attention().clock(&owner.id).created_at.is_none()
                })
                .map(|owner| owner.id.clone()),
        );
        let indexes = OwnerIndexes::new(&self.owners);
        let mut pending = owners.iter().cloned().collect::<Vec<_>>();
        while let Some(id) = pending.pop() {
            for dependency in indexes.dependencies(&id) {
                if owners.insert(dependency.to_owned()) {
                    pending.push(dependency.to_owned());
                }
            }
        }
        owners
    }
}

struct OwnerIndexes<'a> {
    owners: HashMap<&'a str, &'a OwnerLayout>,
    conversations: HashMap<&'a str, Vec<&'a str>>,
    roots: HashMap<(&'a str, &'a std::path::Path), Vec<&'a str>>,
}

impl<'a> OwnerIndexes<'a> {
    fn new(owners: &'a [OwnerLayout]) -> Self {
        let mut indexes = Self {
            owners: HashMap::new(),
            conversations: HashMap::new(),
            roots: HashMap::new(),
        };
        for owner in owners {
            indexes.owners.insert(&owner.id, owner);
            for agent in &owner.agents {
                indexes
                    .conversations
                    .entry(agent.conversation.as_deref().unwrap_or(&agent.id))
                    .or_default()
                    .push(&owner.id);
            }
            if let Some(root) = &owner.root {
                indexes
                    .roots
                    .entry((&owner.project, root.as_path()))
                    .or_default()
                    .push(&owner.id);
            }
        }
        indexes
    }

    fn dependencies(&self, id: &str) -> Vec<&'a str> {
        let Some(owner) = self.owners.get(id) else {
            return Vec::new();
        };
        let aliases = owner.agents.iter().flat_map(|agent| {
            [
                agent.id.as_str(),
                agent.conversation.as_deref().unwrap_or(&agent.id),
            ]
            .into_iter()
            .flat_map(|id| self.conversations.get(id).into_iter().flatten().copied())
        });
        let roots = owner
            .root
            .as_ref()
            .and_then(|root| self.roots.get(&(owner.project.as_str(), root.as_path())))
            .into_iter()
            .flatten()
            .copied();
        aliases.chain(roots).collect()
    }
}

fn agent_layouts(
    app: &AppState,
    owner: &str,
    roster: &crate::agent::AgentRoster,
) -> Vec<AgentLayout> {
    roster
        .iter()
        .map(|agent| {
            let thread = app
                .agent_conversation(owner, Some(&agent.id))
                .unwrap_or(&agent.thread);
            AgentLayout {
                id: agent.id.clone(),
                conversation: agent.conversation_id.clone(),
                thread: thread.id.clone(),
                generation: thread.generation_revision,
                watched: agent.watched,
                choice_revision: agent.choice_revision,
            }
        })
        .collect()
}

fn owner_layouts(app: &AppState) -> Vec<OwnerLayout> {
    let runs = app.runs.iter().map(|(id, active)| OwnerLayout {
        id: id.clone(),
        project: app
            .projects
            .project_id_of(id)
            .unwrap_or_default()
            .to_owned(),
        retained_project_path: app.projects.project_path_for(id),
        root: Some(app.snapshot_root(&active.worktree.path)),
        raw_root: Some(active.worktree.path.clone()),
        docs_root: None,
        structure: json!([
            app.projects.project_id_of(id),
            app.snapshot_run_branch(id, &active.worktree),
            active.worktree.base_branch,
            active.base_sha,
            active.run.state,
            active.run.plan_id,
            active.adopted,
            active.pinned_agent_id
        ]),
        agents: agent_layouts(app, id, &active.agents),
    });
    let plans = app.plans.iter().map(|(id, active)| OwnerLayout {
        id: id.clone(),
        project: app
            .projects
            .project_id_of(id)
            .unwrap_or_default()
            .to_owned(),
        retained_project_path: app.projects.project_path_for(id),
        root: active
            .workspace
            .as_ref()
            .map(|workspace| app.snapshot_root(&workspace.checkout)),
        raw_root: active
            .workspace
            .as_ref()
            .map(|workspace| workspace.checkout.clone()),
        docs_root: active
            .workspace
            .as_ref()
            .map(|workspace| workspace.docs_dir.clone()),
        structure: json!([
            app.projects.project_id_of(id),
            active.plan.state,
            active.plan.archived_at,
            active.base_branch
        ]),
        agents: agent_layouts(app, id, &active.agents),
    });
    runs.chain(plans).collect()
}

fn project_layouts(app: &AppState) -> Vec<Value> {
    app.project_list_rows().into_iter().map(|row| json!({
        "id": row.project_id, "name": row.name, "base": row.base_branch, "git": row.is_git,
        "sources": row.sources.iter().map(|source| source.wire_observed(Value::Null)).collect::<Vec<_>>(),
        "isolation": row.isolation, "default": row.isolation_default,
        "compaction": row.compaction, "thresholds": row.compaction_thresholds, "owner": row.conversation,
    })).collect()
}
