use super::{Project, ProjectSource};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::isolation::Isolation;

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
    entity_project: HashMap<String, String>,
    entity_project_path: HashMap<String, String>,
    next_project: u64,
}

impl ProjectRegistry {
    pub(in crate::app) fn new() -> Self {
        Self {
            projects: Vec::new(),
            entity_project: HashMap::new(),
            entity_project_path: HashMap::new(),
            next_project: 1,
        }
    }

    pub(in crate::app) fn remove(&mut self, project_id: &str) {
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

    pub(in crate::app) fn set_sources(
        &mut self,
        project_id: &str,
        sources: Vec<ProjectSource>,
    ) -> bool {
        let Some(project) = self
            .projects
            .iter_mut()
            .find(|project| project.id == project_id)
        else {
            return false;
        };
        project.sources = sources;
        true
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
        self.entity_project.insert(entity_id, project_id);
    }

    pub(in crate::app) fn unbind_entity(&mut self, entity_id: &str) {
        self.entity_project.remove(entity_id);
        self.entity_project_path.remove(entity_id);
    }

    /// Remove only the live route while retaining any stored recovery path.
    pub(in crate::app) fn unbind_live_entity(&mut self, entity_id: &str) {
        self.entity_project.remove(entity_id);
    }

    pub(in crate::app) fn retain_entity_path(&mut self, entity_id: String, stored_path: String) {
        self.entity_project_path.insert(entity_id, stored_path);
    }
}

impl Default for ProjectRegistry {
    fn default() -> Self {
        Self::new()
    }
}
