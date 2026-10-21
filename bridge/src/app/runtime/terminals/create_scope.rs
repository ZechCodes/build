//! Capture registered identity while guarded; resolve only the requested
//! alias, its eligibility and its physical path outside the application lock.
use super::scope::{source_scope_requested, TermScope};
use super::{TerminalScope, MOVED_TERMINAL_SOURCE};
use crate::app::AppState;
use crate::changes::ProjectSourceScope;
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(super) enum Selector {
    Workspace(String),
    Legacy(TermScope),
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct CapturedScope {
    project_id: String,
    workspace_id: Option<String>,
    raw_root: PathBuf,
    source: Option<ProjectSourceScope>,
    kind: ProbeKind,
    denied_roots: Vec<PathBuf>,
    discovery: Option<Discovery>,
    adoption: Option<Adoption>,
}

pub(super) struct ResolvedScope {
    pub project_id: String,
    pub workspace_id: Option<String>,
    pub raw_root: PathBuf,
    pub terminal: TerminalScope,
    pub adoption: Option<crate::workspace::Workspace>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProbeKind {
    Primary,
    Workspace,
    Run,
    Source,
    Checkout,
}

#[derive(Clone, PartialEq, Eq)]
struct Discovery {
    requested_id: String,
    projects: Vec<DiscoveryProject>,
    held_paths: Vec<PathBuf>,
    state_root: PathBuf,
    registry_root: PathBuf,
}

#[derive(Clone, PartialEq, Eq)]
struct DiscoveryProject {
    id: String,
    repo: PathBuf,
    legacy_root: PathBuf,
    source_id: String,
}

#[derive(Clone, PartialEq, Eq)]
struct Adoption {
    registry_root: PathBuf,
    id: String,
    name: String,
    source_id: String,
    is_git: bool,
}

impl Adoption {
    fn prepare(&self, project: &str, root: &Path) -> crate::workspace::Workspace {
        crate::workspace::WorkspaceRegistry::prepare_root_adoption(
            &self.registry_root,
            project,
            self.id.clone(),
            self.name.clone(),
            root.to_path_buf(),
            self.source_id.clone(),
            self.is_git,
        )
    }
}

impl Selector {
    pub fn parse(params: &Value) -> Result<Self, String> {
        if !source_scope_requested(params) {
            if let Some(id) = params
                .get("workspace_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
            {
                return Ok(Self::Workspace(id.into()));
            }
        }
        Ok(Self::Legacy(TermScope::parse(params)?))
    }

    pub fn capture(&self, app: &AppState) -> Result<CapturedScope, String> {
        match self {
            Self::Workspace(id) => capture_workspace(app, id),
            Self::Legacy(TermScope::Run { run_id }) => capture_run(app, run_id),
            Self::Legacy(TermScope::Primary { project_id }) => {
                let project = app.projects.get(project_id).ok_or("unknown project_id")?;
                Ok(CapturedScope::new(
                    project_id,
                    &project.repo_path,
                    ProbeKind::Primary,
                ))
            }
            Self::Legacy(TermScope::ProjectSource(source)) => capture_source(app, source),
            Self::Legacy(TermScope::ExternalWorktree {
                project_id,
                worktree_id,
            }) => {
                let project = app.projects.get(project_id).ok_or("unknown project_id")?;
                let scan = app.board.diff().external_scan(project_id);
                if let Some(row) = scan.worktrees.iter().find(|row| row.id == *worktree_id) {
                    return Ok(CapturedScope::new(
                        project_id,
                        &row.path,
                        ProbeKind::Checkout,
                    ));
                }
                let mut captured =
                    CapturedScope::new(project_id, Path::new(""), ProbeKind::Checkout);
                captured.discovery = Some(Discovery::capture(app, worktree_id, Some(&project.id)));
                Ok(captured)
            }
        }
    }
}

impl CapturedScope {
    fn new(project: &str, root: &Path, kind: ProbeKind) -> Self {
        Self {
            project_id: project.into(),
            workspace_id: None,
            raw_root: root.into(),
            source: None,
            kind,
            denied_roots: Vec::new(),
            discovery: None,
            adoption: None,
        }
    }

    pub fn known_root(&self) -> Option<&Path> {
        self.discovery.is_none().then_some(&self.raw_root)
    }

    pub fn probe(&self) -> Result<ResolvedScope, String> {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("term.create.scope_probe");
        let (project_id, raw_root, adoption) = match &self.discovery {
            Some(discovery) => {
                let (project, name, path) = discovery.resolve()?;
                let adoption = Adoption {
                    registry_root: discovery.registry_root.clone(),
                    id: discovery.requested_id.clone(),
                    name,
                    source_id: project.source_id,
                    is_git: true,
                }
                .prepare(&project.id, &path);
                (project.id, path, Some(adoption))
            }
            None => (
                self.project_id.clone(),
                self.raw_root.clone(),
                self.adoption
                    .as_ref()
                    .map(|adoption| adoption.prepare(&self.project_id, &self.raw_root)),
            ),
        };
        if self
            .denied_roots
            .iter()
            .any(|root| crate::app::workspaces::same_path(root, &raw_root))
        {
            return Err(format!(
                "unknown workspace_id: {}",
                self.workspace_id.as_deref().unwrap_or_default()
            ));
        }
        let root = match self.kind {
            // Legacy primary selectors retain their raw key when the checkout
            // is unavailable, just as term.list and the terminal reaper do.
            ProbeKind::Primary => AppState::canonical_root(&raw_root),
            ProbeKind::Workspace => std::fs::canonicalize(&raw_root).map_err(|error| {
                format!("resolve workspace root {}: {error}", raw_root.display())
            })?,
            ProbeKind::Run => std::fs::canonicalize(&raw_root)
                .map_err(|_| "worktree no longer exists".to_string())?,
            ProbeKind::Source | ProbeKind::Checkout => std::fs::canonicalize(&raw_root)
                .map_err(|error| format!("cannot resolve scope root: {error}"))?,
        };
        Ok(ResolvedScope {
            project_id,
            workspace_id: self
                .workspace_id
                .clone()
                .or_else(|| adoption.as_ref().map(|row| row.id.clone())),
            raw_root,
            adoption,
            terminal: TerminalScope {
                root,
                source: self.source.clone(),
            },
        })
    }

    fn adoption_revalidates(&self, current: &Self) -> bool {
        match (&self.adoption, &current.adoption) {
            (Some(previous), Some(current)) => previous == current,
            // A previously registered workspace cannot disappear into a
            // legacy fallback while this request retains its old identity.
            (None, Some(_)) => false,
            // A concurrent actual adoption keeps its own newer registry row.
            _ => true,
        }
    }

    pub fn revalidates(&self, current: &Self, resolved: &ResolvedScope, app: &AppState) -> bool {
        match (&self.discovery, &current.discovery) {
            (Some(old), Some(current)) => {
                let selected = |discovery: &Discovery| {
                    discovery
                        .projects
                        .iter()
                        .find(|project| project.id == resolved.project_id)
                        .cloned()
                };
                selected(old).is_some()
                    && selected(old) == selected(current)
                    && old.state_root == current.state_root
                    && old.registry_root == current.registry_root
                    && !current
                        .held_paths
                        .iter()
                        .any(|path| app.cached_snapshot_root(path) == resolved.terminal.root)
            }
            (Some(_), None) => {
                current.project_id == resolved.project_id
                    && current.raw_root == resolved.raw_root
                    && current.source == self.source
            }
            (None, None) => {
                self.project_id == current.project_id
                    && self.workspace_id == current.workspace_id
                    && self.raw_root == current.raw_root
                    && self.source == current.source
                    && self.kind == current.kind
                    && self.adoption_revalidates(current)
                    && !current
                        .denied_roots
                        .iter()
                        .any(|path| app.cached_snapshot_root(path) == resolved.terminal.root)
            }
            (None, Some(_)) => false,
        }
    }
}

impl Discovery {
    fn capture(app: &AppState, id: &str, project_id: Option<&str>) -> Self {
        let projects = app
            .projects
            .iter()
            .filter(|project| {
                project_id.map_or(project.is_git && project.sources.len() == 1, |id| {
                    id == project.id
                })
            })
            .map(|project| DiscoveryProject {
                id: project.id.clone(),
                repo: project.repo_path.clone(),
                legacy_root: app.project_worktrees_root(&project.id),
                source_id: project
                    .sources
                    .first()
                    .map(|source| source.id.clone())
                    .unwrap_or_else(|| "root".into()),
            })
            .collect();
        let held_paths = app
            .workspaces
            .list(None)
            .into_iter()
            .flat_map(|workspace| {
                std::iter::once(workspace.root.clone()).chain(
                    workspace
                        .directories
                        .iter()
                        .map(|directory| directory.path.clone()),
                )
            })
            .chain(app.runs.values().map(|run| run.worktree.path.clone()))
            .collect();
        Self {
            requested_id: id.into(),
            projects,
            held_paths,
            state_root: app.state_root.clone(),
            registry_root: app.workspaces.root().to_path_buf(),
        }
    }

    fn resolve(&self) -> Result<(DiscoveryProject, String, PathBuf), String> {
        for project in &self.projects {
            let Some(checkouts) = crate::app::workspaces::external_checkouts(
                &project.repo,
                &project.legacy_root,
                &self.state_root,
            ) else {
                continue;
            };
            for (name, path) in checkouts {
                if crate::worktree::external_worktree_id(&path) == self.requested_id
                    && self.accepts(&path)
                {
                    return Ok((project.clone(), name, path));
                }
            }
        }
        Err(format!(
            "unknown workspace_id or worktree_id: {}",
            self.requested_id
        ))
    }

    fn accepts(&self, path: &Path) -> bool {
        path.is_dir()
            && !crate::workspace::is_managed_workspace_mount(path)
            && !self
                .held_paths
                .iter()
                .any(|held| crate::app::workspaces::same_path(held, path))
    }
}

fn capture_workspace(app: &AppState, id: &str) -> Result<CapturedScope, String> {
    let mut captured = if let Some(workspace) = app.workspaces.get(id) {
        CapturedScope::new(&workspace.project_id, &workspace.root, ProbeKind::Workspace)
    } else if let Some(project) = id
        .strip_prefix("legacy-")
        .and_then(|id| app.projects.get(id))
        .filter(|project| project.sources.len() == 1)
    {
        CapturedScope::new(&project.id, &project.repo_path, ProbeKind::Workspace)
    } else if let Some(run) = app.runs.get(id) {
        let project = app.projects.project_id_of(id).unwrap_or_default();
        let mut captured = CapturedScope::new(project, &run.worktree.path, ProbeKind::Workspace);
        if let Some(project) = app.projects.get(project) {
            captured
                .denied_roots
                .push(crate::app::projects::scratch_dir(
                    &app.state_root,
                    &project.repo_path,
                ));
        }
        if run.worktree.base_branch.is_empty() {
            captured.denied_roots.extend(
                app.workspaces
                    .list(None)
                    .into_iter()
                    .map(|workspace| workspace.root.clone()),
            );
        }
        captured
    } else if crate::worktree::is_checkout_id(id) {
        let mut captured = CapturedScope::new("", Path::new(""), ProbeKind::Workspace);
        captured.discovery = Some(Discovery::capture(app, id, None));
        captured
    } else {
        return Err(format!("unknown workspace_id: {id}"));
    };
    captured.workspace_id = Some(id.into());
    if app.workspaces.get(id).is_none() && captured.discovery.is_none() {
        let project = app.projects.get(&captured.project_id);
        let name = app
            .runs
            .get(id)
            .map(|run| run.worktree.name.clone())
            .or_else(|| project.map(|project| project.name.clone()))
            .unwrap_or_else(|| id.into());
        captured.adoption = Some(Adoption {
            registry_root: app.workspaces.root().to_path_buf(),
            id: id.into(),
            name,
            source_id: project
                .and_then(|project| project.sources.first())
                .map(|source| source.id.clone())
                .unwrap_or_else(|| "root".into()),
            is_git: project.is_some_and(|project| project.is_git),
        });
    }
    Ok(captured)
}

fn capture_run(app: &AppState, id: &str) -> Result<CapturedScope, String> {
    let run = app.runs.get(id).ok_or("unknown run_id")?;
    Ok(CapturedScope::new(
        app.projects.project_id_of(id).unwrap_or_default(),
        &run.worktree.path,
        ProbeKind::Run,
    ))
}

fn capture_source(app: &AppState, source: &ProjectSourceScope) -> Result<CapturedScope, String> {
    let project = app
        .projects
        .get(&source.project_id)
        .ok_or_else(|| format!("unknown project_id: {}", source.project_id))?;
    let row = project
        .sources
        .iter()
        .find(|row| row.id == source.source_id)
        .ok_or_else(|| {
            format!(
                "unknown source_id {} in project {}",
                source.source_id, source.project_id
            )
        })?;
    if row.path.display().to_string() != source.source_path {
        return Err(MOVED_TERMINAL_SOURCE.into());
    }
    let mut captured = CapturedScope::new(&project.id, &row.path, ProbeKind::Source);
    captured.source = Some(source.clone());
    Ok(captured)
}
