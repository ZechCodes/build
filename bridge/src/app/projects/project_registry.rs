use super::Project;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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

    pub(in crate::app) fn iter(&self) -> impl Iterator<Item = &Project> {
        self.projects.iter()
    }

    pub(in crate::app) fn get(&self, project_id: &str) -> Option<&Project> {
        self.projects
            .iter()
            .find(|project| project.id == project_id)
    }

    /// Temporary mutation seam for board caches and configured isolation.
    /// Stage 9 moves the caches to BoardIndex and leaves a narrow isolation
    /// update rather than retaining general mutable access.
    pub(in crate::app) fn get_mut(&mut self, project_id: &str) -> Option<&mut Project> {
        self.projects
            .iter_mut()
            .find(|project| project.id == project_id)
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

    #[cfg(test)]
    pub(in crate::app) fn next_id(&self) -> u64 {
        self.next_project
    }

    /// Build a project under the next id without changing registry state.
    pub(super) fn candidate(
        &self,
        repo_path: PathBuf,
        base_branch: String,
        build: impl FnOnce(ProjectId, &Path, &str) -> Project,
    ) -> ProjectCandidate {
        let id = ProjectId(format!("proj-{}", self.next_project));
        let project = build(id, &repo_path, &base_branch);
        ProjectCandidate { project }
    }

    /// Publish only after the caller's durability step has succeeded.
    pub(super) fn publish(&mut self, candidate: ProjectCandidate) -> ProjectId {
        let id = ProjectId(candidate.project.id.clone());
        self.projects.push(candidate.project);
        self.next_project += 1;
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
