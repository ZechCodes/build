use super::{next_source_number, Project, ProjectSource};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::isolation::Isolation;

/// Only configuration-owned project fields; runtime orchestrators stay installed.
pub(super) struct ProjectConfiguration {
    base_branch: String,
    is_git: bool,
    sources: Vec<ProjectSource>,
    next_source: u64,
    isolation: Option<Isolation>,
}

impl ProjectConfiguration {
    pub(super) fn capture(project: &Project) -> Self {
        Self {
            base_branch: project.base_branch.clone(),
            is_git: project.is_git,
            sources: project.sources.clone(),
            next_source: project.next_source,
            isolation: project.isolation,
        }
    }

    pub(super) fn swap(&mut self, projects: &mut ProjectRegistry, id: &str) {
        projects.observation_projects.mark(id);
        let project = projects
            .projects
            .iter_mut()
            .find(|project| project.id == id)
            .expect("staged project remains registered under the app lock");
        std::mem::swap(&mut self.base_branch, &mut project.base_branch);
        std::mem::swap(&mut self.is_git, &mut project.is_git);
        std::mem::swap(&mut self.sources, &mut project.sources);
        std::mem::swap(&mut self.next_source, &mut project.next_source);
        std::mem::swap(&mut self.isolation, &mut project.isolation);
    }
}

/// The stable id assigned to one registered project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProjectId(String);

impl ProjectId {
    #[cfg(test)]
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }

    pub(super) fn into_string(self) -> String {
        self.0
    }
}

/// A fully built project that has not yet become visible to the application.
///
/// RPC registration holds this value while it serializes and durably writes
/// the prospective configuration. Dropping it publishes nothing and consumes
/// no id.
pub(super) struct ProjectCandidate {
    project: Project,
}

impl ProjectCandidate {
    pub(super) fn project(&self) -> &Project {
        &self.project
    }
}

/// Registered projects and the entity bindings that point at them.
///
/// The vector preserves project-list/config iteration order. Both maps remain
/// private so a live binding and its retained recovery path can only be
/// changed through the operations that preserve their existing relationship.
pub(in crate::app) struct ProjectRegistry {
    projects: Vec<Project>,
    observation_projects: crate::observation_changes::ChangeKeys,
    observation_bindings: crate::observation_changes::ChangeKeys,
    entity_project: HashMap<String, String>,
    entity_project_path: HashMap<String, String>,
    next_project: u64,
    /// The configuration of each project whose folder was away when it was
    /// loaded, kept verbatim so every save writes it back until it returns.
    parked: Vec<serde_json::Value>,
}

impl ProjectRegistry {
    pub(in crate::app) fn observation_project_changes(
        &self,
    ) -> &crate::observation_changes::ChangeKeys {
        &self.observation_projects
    }
    pub(in crate::app) fn observation_binding_changes(
        &self,
    ) -> &crate::observation_changes::ChangeKeys {
        &self.observation_bindings
    }
    pub(in crate::app) fn new() -> Self {
        Self {
            projects: Vec::new(),
            observation_projects: Default::default(),
            observation_bindings: Default::default(),
            entity_project: HashMap::new(),
            entity_project_path: HashMap::new(),
            next_project: 1,
            parked: Vec::new(),
        }
    }

    /// Keep one project's configuration while its folder is away.
    pub(in crate::app) fn park(&mut self, entry: serde_json::Value) {
        self.parked.push(entry);
    }

    /// The configuration of every project whose folder is away.
    pub(in crate::app) fn parked(&self) -> &[serde_json::Value] {
        &self.parked
    }

    /// Take the parked configurations `returned` says are back.
    pub(in crate::app) fn take_returned(
        &mut self,
        returned: impl Fn(&serde_json::Value) -> bool,
    ) -> Vec<serde_json::Value> {
        let (back, away) = std::mem::take(&mut self.parked)
            .into_iter()
            .partition(|entry| returned(entry));
        self.parked = away;
        back
    }

    /// Bind every entity a boot could bind to no project, because the
    /// folder its record names was away, to `project_id` standing at `repo`.
    pub(in crate::app) fn bind_retained_entities(&mut self, project_id: &str, repo: &Path) {
        let unbound: Vec<String> = self
            .entity_project_path
            .iter()
            .filter(|(entity_id, _)| !self.entity_project.contains_key(*entity_id))
            .filter(|(_, path)| std::fs::canonicalize(path).is_ok_and(|path| path == repo))
            .map(|(entity_id, _)| entity_id.clone())
            .collect();
        for entity_id in unbound {
            self.observation_bindings.mark(&entity_id);
            self.entity_project
                .insert(entity_id, project_id.to_string());
        }
    }

    pub(in crate::app) fn remove(&mut self, project_id: &str) {
        self.observation_projects.mark(project_id);
        self.projects.retain(|project| project.id != project_id);
        let entities: Vec<_> = self
            .entity_project
            .iter()
            .filter(|(_, id)| id.as_str() == project_id)
            .map(|(entity, _)| entity.clone())
            .collect();
        for entity in entities {
            self.unbind_entity(&entity);
        }
    }

    pub(in crate::app) fn iter(&self) -> impl Iterator<Item = &Project> {
        self.projects.iter()
    }

    /// One source of one project, to write what its sync concluded.
    pub(in crate::app) fn source_mut(
        &mut self,
        project_id: &str,
        source_id: &str,
    ) -> Option<&mut ProjectSource> {
        self.observation_projects.mark(project_id);
        self.projects
            .iter_mut()
            .find(|project| project.id == project_id)?
            .sources
            .iter_mut()
            .find(|source| source.id == source_id)
    }

    /// Every source of every project, with its project's id.
    pub(in crate::app) fn sources_mut(
        &mut self,
    ) -> impl Iterator<Item = (&str, &mut ProjectSource)> {
        let changes = &self.observation_projects;
        self.projects.iter_mut().flat_map(move |project| {
            changes.mark(&project.id);
            let project_id = project.id.as_str();
            project
                .sources
                .iter_mut()
                .map(move |source| (project_id, source))
        })
    }

    pub(in crate::app) fn get(&self, project_id: &str) -> Option<&Project> {
        self.projects
            .iter()
            .find(|project| project.id == project_id)
    }

    /// Update only the registered project's configuration-owned isolation.
    pub(in crate::app) fn set_isolation(
        &mut self,
        project_id: &str,
        isolation: Option<Isolation>,
    ) -> bool {
        self.observation_projects.mark(project_id);
        let Some(project) = self
            .projects
            .iter_mut()
            .find(|project| project.id == project_id)
        else {
            return false;
        };
        project.isolation = isolation;
        true
    }

    pub(in crate::app) fn mark_git(&mut self, project_id: &str) -> bool {
        self.observation_projects.mark(project_id);
        let Some(project) = self
            .projects
            .iter_mut()
            .find(|project| project.id == project_id)
        else {
            return false;
        };
        project.is_git = true;
        if let Some(source) = project.sources.first_mut() {
            source.is_git = true;
        }
        true
    }

    pub(in crate::app) fn mark_source_git(
        &mut self,
        project_id: &str,
        source_id: &str,
        expected_path: &Path,
        branch: &str,
    ) -> Result<bool, String> {
        self.observation_projects.mark(project_id);
        let project = self
            .projects
            .iter_mut()
            .find(|project| project.id == project_id)
            .ok_or_else(|| format!("unknown project: {project_id}"))?;
        let source = project
            .sources
            .iter_mut()
            .find(|source| source.id == source_id)
            .ok_or_else(|| format!("unknown source_id {source_id} in project {project_id}"))?;
        if source.path != expected_path {
            return Err(format!(
                "source {source_id} no longer matches the workspace record"
            ));
        }
        source.is_git = true;
        source.base_branch = branch.to_string();
        let primary = project.repo_path == source.path;
        if primary {
            project.is_git = true;
            project.base_branch = branch.to_string();
        }
        Ok(primary)
    }

    /// Write an edited source's record, when the source is still where the
    /// edit found it: a source removed or moved while its git ran is not
    /// resurrected. The first source's base is the project's.
    pub(in crate::app) fn update_source(
        &mut self,
        project_id: &str,
        source_id: &str,
        expected_path: &Path,
        record: super::SourceRecord,
    ) -> Result<(), String> {
        self.observation_projects.mark(project_id);
        let project = self
            .projects
            .iter_mut()
            .find(|project| project.id == project_id)
            .ok_or_else(|| format!("unknown project: {project_id}"))?;
        let source = project
            .sources
            .iter_mut()
            .find(|source| source.id == source_id && source.path == expected_path)
            .ok_or_else(|| format!("source {source_id} changed while it was being edited"))?;
        if let Some(name) = record.name {
            source.name = name;
        }
        source.path = record.path;
        source.is_git = record.is_git;
        source.base_branch = record.base_branch;
        if let Some(sync_base) = record.sync_base {
            source.sync_base = Some(sync_base);
        }
        if project.repo_path == source.path {
            project.base_branch = source.base_branch.clone();
        }
        Ok(())
    }

    pub(in crate::app) fn set_sources(
        &mut self,
        project_id: &str,
        sources: Vec<ProjectSource>,
    ) -> bool {
        self.observation_projects.mark(project_id);
        let Some(project) = self
            .projects
            .iter_mut()
            .find(|project| project.id == project_id)
        else {
            return false;
        };
        project.next_source = project.next_source.max(next_source_number(&sources));
        project.sources = sources;
        true
    }

    /// Hold this project's source count past `next`, for one restored from a
    /// config that recorded where its minting had got to.
    pub(in crate::app) fn reserve_source_ids_through(&mut self, project_id: &str, next: u64) {
        if let Some(project) = self
            .projects
            .iter_mut()
            .find(|project| project.id == project_id)
        {
            project.next_source = project.next_source.max(next);
        }
    }

    pub(in crate::app) fn find_by_canonical_path(&self, path: &Path) -> Option<&Project> {
        self.projects
            .iter()
            .find(|project| project.repo_path == path)
    }

    pub(in crate::app) fn project_id_of(&self, entity_id: &str) -> Option<&str> {
        self.entity_project.get(entity_id).map(String::as_str)
    }

    /// Prefer the registered project's current display path, then the exact
    /// recovery text retained from storage, then the historical empty value.
    pub(in crate::app) fn project_path_for(&self, entity_id: &str) -> String {
        self.project_id_of(entity_id)
            .and_then(|project_id| self.get(project_id))
            .map(|project| project.repo_path.display().to_string())
            .or_else(|| self.entity_project_path.get(entity_id).cloned())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(in crate::app) fn ids(&self) -> impl Iterator<Item = &str> {
        self.projects.iter().map(|project| project.id.as_str())
    }

    #[cfg(test)]
    pub(in crate::app) fn at(&self, index: usize) -> &Project {
        &self.projects[index]
    }

    #[cfg(test)]
    pub(in crate::app) fn at_mut(&mut self, index: usize) -> &mut Project {
        self.observation_projects.mark(&self.projects[index].id);
        &mut self.projects[index]
    }

    #[cfg(test)]
    pub(in crate::app) fn len(&self) -> usize {
        self.projects.len()
    }

    #[cfg(test)]
    pub(in crate::app) fn is_empty(&self) -> bool {
        self.projects.is_empty()
    }

    #[cfg(test)]
    pub(in crate::app) fn clear(&mut self) {
        for project in &self.projects {
            self.observation_projects.mark(&project.id);
        }
        self.projects.clear();
    }

    pub(in crate::app) fn next_id(&self) -> u64 {
        self.next_project
    }

    pub(super) fn reserve_ids_through(&mut self, next: u64) {
        self.next_project = self.next_project.max(next);
    }

    pub(super) fn candidate_with_id(
        &self,
        number: u64,
        repo_path: PathBuf,
        base_branch: String,
        build: impl FnOnce(ProjectId, &Path, &str) -> Project,
    ) -> ProjectCandidate {
        let project = build(
            ProjectId(format!("proj-{number}")),
            &repo_path,
            &base_branch,
        );
        ProjectCandidate { project }
    }

    /// Build a project under the next id without changing registry state.
    #[cfg(test)]
    pub(super) fn candidate(
        &self,
        repo_path: PathBuf,
        base_branch: String,
        build: impl FnOnce(ProjectId, &Path, &str) -> Project,
    ) -> ProjectCandidate {
        self.candidate_with_id(self.next_project, repo_path, base_branch, build)
    }

    /// Publish only after the caller's durability step has succeeded.
    pub(super) fn publish(&mut self, candidate: ProjectCandidate) -> ProjectId {
        self.observation_projects.mark(&candidate.project.id);
        let id = ProjectId(candidate.project.id.clone());
        let number = candidate
            .project
            .id
            .strip_prefix("proj-")
            .and_then(|id| id.parse::<u64>().ok())
            .expect("project IDs are allocated numerically");
        self.projects.push(candidate.project);
        self.reserve_ids_through(number + 1);
        id
    }

    pub(in crate::app) fn bind_entity(&mut self, entity_id: String, project_id: String) {
        self.observation_bindings.mark(&entity_id);
        self.entity_project.insert(entity_id, project_id);
    }

    pub(in crate::app) fn unbind_entity(&mut self, entity_id: &str) {
        self.observation_bindings.mark(entity_id);
        self.entity_project.remove(entity_id);
        self.entity_project_path.remove(entity_id);
    }

    /// Remove only the live route while retaining any stored recovery path.
    pub(in crate::app) fn unbind_live_entity(&mut self, entity_id: &str) {
        self.observation_bindings.mark(entity_id);
        self.entity_project.remove(entity_id);
    }

    pub(in crate::app) fn retain_entity_path(&mut self, entity_id: String, stored_path: String) {
        self.observation_bindings.mark(&entity_id);
        self.entity_project_path.insert(entity_id, stored_path);
    }
}

impl Default for ProjectRegistry {
    fn default() -> Self {
        Self::new()
    }
}
