mod environment_edits;

use super::{next_source_number, Project, ProjectSource};
use crate::app::environments::Environment;
#[cfg(test)]
mod test_support;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::isolation::Isolation;

#[derive(Default)]
struct SourceReservations {
    used_ids: HashSet<String>,
    next_source: u64,
}

/// A stored reference's owner, retained while checking shared directories.
struct SourceReference<'a> {
    environment_id: &'a str,
    project_id: Option<&'a str>,
    source: ProjectSource,
}

impl SourceReference<'_> {
    fn belongs_to_source(
        &self,
        project_id: &str,
        source_id: &str,
        inherited_environment: Option<&str>,
    ) -> bool {
        if self.source.id != source_id {
            return false;
        }
        match inherited_environment {
            Some(id) => self.project_id.is_none() && self.environment_id == id,
            None => self.project_id == Some(project_id),
        }
    }
}

fn canonical_reference(mut source: ProjectSource) -> ProjectSource {
    source.path = std::fs::canonicalize(&source.path)
        .unwrap_or_else(|_| crate::worktree::canonical_planned_path(&source.path));
    source
}

impl SourceReservations {
    fn allocate_id(&mut self) -> String {
        let number = self.next_source.max(1);
        let next = number.checked_add(1).expect("source id counter exhausted");
        let id = format!("source-{number}");
        self.reserve(next, std::iter::once(id.as_str()));
        id
    }

    fn reserve<'a>(&mut self, counter: u64, ids: impl IntoIterator<Item = &'a str>) {
        self.next_source = self.next_source.max(counter).max(1);
        for id in ids {
            self.used_ids.insert(id.to_string());
            if let Some(number) = id
                .strip_prefix("source-")
                .and_then(|number| number.parse::<u64>().ok())
            {
                let next = number.checked_add(1).expect("source id counter exhausted");
                self.next_source = self.next_source.max(next);
            }
        }
    }
}

/// Only configuration-owned project fields; runtime orchestrators stay installed.
pub(in crate::app) struct ProjectConfiguration {
    records: Vec<(String, ProjectConfigurationRecord)>,
    environments: Vec<Environment>,
}

impl ProjectConfiguration {
    pub(super) fn capture(projects: &ProjectRegistry, project_id: &str) -> Self {
        let environment_id = &projects
            .get(project_id)
            .expect("resolved project")
            .environment_id;
        Self::capture_environment(projects, environment_id)
    }

    pub(in crate::app) fn capture_environment(
        projects: &ProjectRegistry,
        environment_id: &str,
    ) -> Self {
        Self {
            records: projects
                .projects
                .iter()
                .filter(|project| project.environment_id == environment_id)
                .map(|p| (p.id.clone(), ProjectConfigurationRecord::capture(p)))
                .collect(),
            environments: projects
                .environments
                .iter()
                .filter(|e| e.id == environment_id)
                .cloned()
                .collect(),
        }
    }

    pub(in crate::app) fn swap(&mut self, projects: &mut ProjectRegistry, _id: &str) {
        for environment in &mut self.environments {
            let current = projects
                .environment_mut(&environment.id)
                .expect("staged environment remains registered");
            std::mem::swap(environment, current);
        }
        for (id, record) in &mut self.records {
            record.swap(projects, id);
        }
    }
}

struct ProjectConfigurationRecord {
    name: String,
    base_branch: String,
    is_git: bool,
    sources: Vec<ProjectSource>,
    added_sources: Vec<ProjectSource>,
    next_source: u64,
    isolation: Option<Isolation>,
}

impl ProjectConfigurationRecord {
    fn capture(project: &Project) -> Self {
        Self {
            name: project.name.clone(),
            base_branch: project.base_branch.clone(),
            is_git: project.is_git,
            sources: project.sources.clone(),
            added_sources: project.added_sources.clone(),
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
        std::mem::swap(&mut self.name, &mut project.name);
        std::mem::swap(&mut self.base_branch, &mut project.base_branch);
        std::mem::swap(&mut self.is_git, &mut project.is_git);
        std::mem::swap(&mut self.sources, &mut project.sources);
        std::mem::swap(&mut self.added_sources, &mut project.added_sources);
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
    #[cfg(test)]
    pub(super) fn set_test_identity(&mut self, environment_id: String, key: String) {
        self.project.environment_id = environment_id;
        self.project.project_key = key;
    }

    pub(super) fn set_environment(&mut self, id: &str) {
        self.project.environment_id = id.to_string();
    }
    pub(super) fn add_creation_sources(&mut self, added: Vec<ProjectSource>) {
        self.project.next_source = self.project.next_source.max(next_source_number(&added));
        self.project.added_sources = added.clone();
        self.project.sources.extend(added);
    }
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
    environments: Vec<Environment>,
    next_environment: u64,
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
    /// Sources admitted anywhere on the device, including retained references
    /// that the live project projection cannot currently open.
    pub(in crate::app) fn registered_sources(&self) -> Vec<ProjectSource> {
        self.source_references()
            .into_iter()
            .map(|reference| canonical_reference(reference.source))
            .collect()
    }

    /// A repoint replaces one owning record. Equal references belonging to
    /// siblings still constrain containment at the old directory.
    pub(in crate::app) fn registered_sources_except(
        &self,
        project_id: &str,
        source_id: &str,
    ) -> Vec<ProjectSource> {
        let inherited_environment = self.inherited_source_environment(project_id, source_id);
        self.source_references()
            .into_iter()
            .filter(|reference| {
                !reference.belongs_to_source(project_id, source_id, inherited_environment)
            })
            .map(|reference| canonical_reference(reference.source))
            .collect()
    }

    /// An inherited edit changes every dependent project's effective sources.
    /// Each dependent's additions, including parked projects, must stay unique.
    /// A project-owned edit only needs its own effective source membership.
    pub(in crate::app) fn source_repoint_duplicate_project(
        &self,
        project_id: &str,
        source_id: &str,
        candidate: &Path,
    ) -> Option<String> {
        let project = self.get(project_id)?;
        let canonical = super::canonical_source_path(candidate).ok()?;
        let inherited_environment = self.inherited_source_environment(project_id, source_id);
        self.source_references()
            .into_iter()
            .filter(|reference| reference.environment_id == project.environment_id)
            .filter(|reference| {
                inherited_environment.is_some()
                    || reference.project_id.is_none()
                    || reference.project_id == Some(project_id)
            })
            .filter(|reference| {
                !reference.belongs_to_source(project_id, source_id, inherited_environment)
            })
            .find_map(|reference| {
                let path = super::canonical_source_path(&reference.source.path).ok()?;
                (path == canonical).then(|| reference.project_id.unwrap_or(project_id).to_owned())
            })
    }

    fn inherited_source_environment(&self, project_id: &str, source_id: &str) -> Option<&str> {
        let project = self.get(project_id)?;
        let environment = self.environment(&project.environment_id)?;
        let live = project
            .sources
            .iter()
            .find(|source| source.id == source_id)?;
        environment
            .sources
            .iter()
            .any(|source| source.id == source_id && source.path == live.path)
            .then_some(environment.id.as_str())
    }

    /// Directory equality identifies references, never project identity or a
    /// repository's common Git directory. Return each environment just once.
    #[cfg(test)]
    pub(in crate::app) fn environments_referencing(&self, dir: &Path) -> Vec<&Environment> {
        let Ok(canonical) = super::canonical_source_path(dir) else {
            return Vec::new();
        };
        let owners: HashSet<_> = self
            .source_references()
            .into_iter()
            .filter(|reference| {
                super::canonical_source_path(&reference.source.path)
                    .is_ok_and(|path| path == canonical)
            })
            .map(|reference| reference.environment_id)
            .collect();
        self.environments
            .iter()
            .filter(|environment| owners.contains(environment.id.as_str()))
            .collect()
    }

    fn source_references(&self) -> Vec<SourceReference<'_>> {
        let mut references = Vec::new();
        for environment in &self.environments {
            references.extend(
                environment
                    .sources
                    .iter()
                    .cloned()
                    .map(|source| SourceReference {
                        environment_id: &environment.id,
                        project_id: None,
                        source,
                    }),
            );
        }
        for project in &self.projects {
            let fallback = self
                .environment(&project.environment_id)
                .and_then(|environment| Self::unowned_home_source(project, environment));
            references.extend(project.added_sources.iter().cloned().chain(fallback).map(
                |source| SourceReference {
                    environment_id: &project.environment_id,
                    project_id: Some(&project.id),
                    source,
                },
            ));
        }
        for project in &self.parked {
            references.extend(self.parked_source_references(project));
        }
        references
    }

    fn parked_source_references<'a>(
        &'a self,
        project: &'a serde_json::Value,
    ) -> Vec<SourceReference<'a>> {
        let environment_id = project
            .get("environment_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        // Legacy projections can be stale after an environment edit. They
        // reserve paths only when no authoritative environment is available.
        let fields: &[&str] = if self.environment(environment_id).is_some() {
            &["added_sources"]
        } else {
            &["sources", "added_sources"]
        };
        let project_id = project
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        fields
            .iter()
            .flat_map(|field| {
                project
                    .get(*field)
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .enumerate()
            .filter_map(|(index, entry)| super::configured_source(index, entry))
            .map(|source| SourceReference {
                environment_id,
                project_id: Some(project_id),
                source,
            })
            .collect()
    }

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
            environments: Vec::new(),
            next_environment: 1,
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

    /// Rebind retained entities by exact key, including a returning parked project.
    pub(in crate::app) fn bind_retained_entities(&mut self, project_id: &str) {
        let Some(project) = self.get(project_id) else {
            return;
        };
        let key = &project.project_key;
        let unbound: Vec<String> = self
            .entity_project_path
            .iter()
            .filter(|(entity_id, _)| !self.entity_project.contains_key(*entity_id))
            .filter(|(_, retained)| *retained == key)
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

    /// One project projection, primarily for its runtime sync status.
    /// Configuration edits through this reference must be staged by
    /// commit_project_configuration, which reconciles the owning references.
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
        self.reconcile_project_sources(project_id);
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
        self.reconcile_project_sources(project_id);
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
        self.reconcile_project_sources(project_id);
        Ok(())
    }

    pub(in crate::app) fn set_sources(
        &mut self,
        project_id: &str,
        mut sources: Vec<ProjectSource>,
    ) -> bool {
        self.observation_projects.mark(project_id);
        let fallback_home = self.get(project_id).and_then(|project| {
            let environment = self.environment(&project.environment_id)?;
            Self::unowned_home_source(project, environment)
        });
        let promoted_home =
            self.prepare_home_promotion(project_id, &mut sources, fallback_home.as_ref());
        let Some(project) = self
            .projects
            .iter_mut()
            .find(|project| project.id == project_id)
        else {
            return false;
        };
        let retained: std::collections::HashSet<_> =
            sources.iter().map(|source| &source.id).collect();
        // Only a reference visible before this edit can have been unlinked.
        // Absence from the live projection alone does not remove stored refs.
        let removed: std::collections::HashSet<_> = project
            .sources
            .iter()
            .filter(|source| {
                !retained.contains(&source.id)
                    && !fallback_home
                        .as_ref()
                        .is_some_and(|home| home.id == source.id && home.path == source.path)
            })
            .map(|source| source.id.clone())
            .collect();
        project
            .added_sources
            .retain(|source| !removed.contains(&source.id));
        let environment_id = project.environment_id.clone();
        let reservation_floor = promoted_home.as_ref().map_or(0, |(_, floor)| *floor);
        project.next_source = project
            .next_source
            .max(next_source_number(&sources))
            .max(reservation_floor);
        project.sources = sources;
        if let Some(environment) = self
            .environments
            .iter_mut()
            .find(|environment| environment.id == environment_id)
        {
            environment
                .sources
                .retain(|source| !removed.contains(&source.id));
        }
        if let Some((home, _)) = promoted_home {
            self.promote_home_source(project_id, &home);
        }
        self.reconcile_project_sources(project_id);
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
        self.reconcile_project_sources(project_id);
    }

    pub(in crate::app) fn find_by_canonical_path(&self, path: &Path) -> Option<&Project> {
        self.projects
            .iter()
            .filter(|project| project.repo_path == path)
            .min_by_key(|project| project.project_key != path.to_string_lossy())
    }

    pub(in crate::app) fn project_id_of(&self, entity_id: &str) -> Option<&str> {
        self.entity_project.get(entity_id).map(String::as_str)
    }

    /// Prefer the registered project's current display path, then the exact
    /// recovery text retained from storage, then the historical empty value.
    pub(in crate::app) fn project_path_for(&self, entity_id: &str) -> String {
        self.project_id_of(entity_id)
            .and_then(|project_id| self.get(project_id))
            .map(|project| project.home_path.display().to_string())
            .or_else(|| self.entity_project_path.get(entity_id).cloned())
            .unwrap_or_default()
    }

    /// Identity for persistence and routing; the display path is separate.
    pub(in crate::app) fn project_key_for(&self, entity_id: &str) -> String {
        self.project_id_of(entity_id)
            .and_then(|project_id| self.get(project_id))
            .map(|project| project.project_key.clone())
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
        let mut project = build(
            ProjectId(format!("proj-{number}")),
            &repo_path,
            &base_branch,
        );
        project.environment_id = format!("env-{}", self.next_environment);
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

    /// Build a new line of work using the environment's references and home.
    pub(super) fn add_project_to_environment(
        &self,
        environment_id: &str,
        name: &str,
        key: &str,
        mut candidate: ProjectCandidate,
    ) -> Result<ProjectCandidate, String> {
        let environment = self
            .environment(environment_id)
            .ok_or_else(|| format!("unknown environment: {environment_id}"))?;
        let home = environment
            .sources
            .first()
            .ok_or("environment has no home source")?;
        if name.trim().is_empty() {
            return Err("project name cannot be empty".into());
        }
        if key.is_empty() {
            return Err("project key cannot be empty".into());
        }
        if self
            .projects
            .iter()
            .any(|project| project.id == candidate.project.id || project.project_key == key)
        {
            return Err("project identity already exists".into());
        }
        candidate.project.environment_id = environment_id.into();
        candidate.project.project_key = key.into();
        candidate.project.name = name.into();
        candidate.project.home_path = home.path.clone();
        candidate.project.repo_path = home.path.clone();
        candidate.project.sources = environment.sources.clone();
        candidate.project.next_source = environment.next_source;
        candidate.project.added_sources.clear();
        candidate.project.isolation = None;
        Ok(candidate)
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
        if self
            .environment(&candidate.project.environment_id)
            .is_none()
        {
            self.environments
                .push(Self::environment_for_project(&candidate.project));
            self.next_environment += 1;
        }
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

    /// Retain the opaque key while its registered project is unavailable.
    pub(in crate::app) fn retain_entity_path(&mut self, entity_id: String, stored_key: String) {
        self.observation_bindings.mark(&entity_id);
        self.entity_project_path.insert(entity_id, stored_key);
    }
}

impl Default for ProjectRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectRegistry {
    pub(in crate::app) fn environment(&self, id: &str) -> Option<&Environment> {
        self.environments
            .iter()
            .find(|environment| environment.id == id)
    }

    pub(in crate::app) fn environments(&self) -> impl Iterator<Item = &Environment> {
        self.environments.iter()
    }

    pub(in crate::app) fn next_environment(&self) -> u64 {
        self.next_environment
    }

    pub(in crate::app) fn restore_environments(&mut self, config: &serde_json::Value) {
        if let Some(next) = config
            .get("next_environment")
            .and_then(serde_json::Value::as_u64)
        {
            self.next_environment = self.next_environment.max(next);
        }
        // Missing owners stay parked. Never allocate one of their retained IDs
        // to an unrelated environment and accidentally revive that project.
        for project in config
            .get("projects")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(number) = project
                .get("environment_id")
                .and_then(serde_json::Value::as_str)
                .and_then(|id| id.strip_prefix("env-"))
                .and_then(|number| number.parse::<u64>().ok())
            {
                self.next_environment = self.next_environment.max(number.saturating_add(1));
            }
        }
        for entry in config
            .get("environments")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(id) = entry.get("id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            if let Some(number) = id.strip_prefix("env-").and_then(|n| n.parse::<u64>().ok()) {
                self.next_environment = self.next_environment.max(number.saturating_add(1));
            }
            let sources = super::configured_sources(
                entry
                    .get("sources")
                    .and_then(serde_json::Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            );
            let next_source = entry
                .get("next_source")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(1)
                .max(next_source_number(&sources));
            let environment = Environment {
                id: id.to_string(),
                name: entry
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("environment")
                    .to_string(),
                sources,
                next_source,
                isolation: crate::app::config::configured_isolation(entry, "environment isolation"),
            };
            if let Some(existing) = self
                .environments
                .iter_mut()
                .find(|e| e.id == environment.id)
            {
                *existing = environment;
            } else {
                self.environments.push(environment);
            }
        }
    }

    pub(in crate::app) fn environment_for_project(project: &Project) -> Environment {
        Environment {
            id: project.environment_id.clone(),
            name: project.name.clone(),
            sources: project.sources.clone(),
            next_source: project.next_source,
            isolation: None,
        }
    }

    /// Apply a legacy edit to its owning references, then rebuild read projections.
    pub(super) fn reconcile_project_sources(&mut self, id: &str) {
        let Some(project) = self.get(id) else {
            return;
        };
        let environment_id = project.environment_id.clone();
        let fallback = self
            .environment(&environment_id)
            .and_then(|environment| Self::unowned_home_source(project, environment));
        let effective = project.sources.clone();
        let mut added = project.added_sources.clone();
        let next = project.next_source;
        let Some(environment) = self
            .environments
            .iter_mut()
            .find(|e| e.id == environment_id)
        else {
            return;
        };
        for source in effective.iter().filter(|source| {
            !fallback
                .as_ref()
                .is_some_and(|home| home.id == source.id && home.path == source.path)
        }) {
            if let Some(inherited) = environment
                .sources
                .iter_mut()
                .find(|stored| stored.id == source.id)
            {
                // Runtime conclusions belong to the reference's actual owner.
                let status = inherited.sync_status.clone();
                *inherited = source.clone();
                inherited.sync_status = status;
            } else {
                Self::upsert_added_source(&mut added, source);
            }
        }
        environment.next_source = environment
            .next_source
            .max(next)
            .max(next_source_number(&effective));
        self.projects
            .iter_mut()
            .find(|p| p.id == id)
            .expect("resolved project")
            .added_sources = added;
        self.refresh_environment_projects(&environment_id);
    }

    fn upsert_added_source(stored: &mut Vec<ProjectSource>, source: &ProjectSource) {
        if let Some(existing) = stored.iter_mut().find(|stored| stored.id == source.id) {
            *existing = source.clone();
        } else {
            stored.push(source.clone());
        }
    }

    pub(in crate::app) fn refresh_environment_projects(&mut self, environment_id: &str) {
        let Some(environment) = self.environment(environment_id).cloned() else {
            return;
        };
        for project in self
            .projects
            .iter_mut()
            .filter(|p| p.environment_id == environment_id)
        {
            self.observation_projects.mark(&project.id);
            let fallback = Self::unowned_home_source(project, &environment);
            let statuses: HashMap<_, _> = project
                .sources
                .iter()
                .map(|s| (s.id.clone(), s.sync_status.clone()))
                .collect();
            project.sources = environment
                .sources
                .iter()
                .chain(&project.added_sources)
                .filter_map(|source| {
                    let path = super::canonical_source_path(&source.path).ok()?;
                    let mut live = source.clone();
                    live.path = path;
                    Some(live)
                })
                .collect();
            if project.sources.is_empty() {
                let home = fallback.or_else(|| Self::initial_home_source(project));
                project.sources.extend(home);
            }
            for source in &mut project.sources {
                // Runtime conclusions belong to this project's reference, not
                // the environment record copied from another project's edit.
                source.sync_status = statuses.get(&source.id).cloned().flatten();
            }
            project.next_source = environment.next_source;
        }
    }

    pub(super) fn restore_project_metadata(&mut self, id: &str, entry: &serde_json::Value) {
        let Some(environment_id) = entry
            .get("environment_id")
            .and_then(serde_json::Value::as_str)
        else {
            return;
        };
        let added_sources = super::configured_sources(
            entry
                .get("added_sources")
                .and_then(serde_json::Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default(),
        );
        if let Some(environment) = self
            .environments
            .iter_mut()
            .find(|e| e.id == environment_id)
        {
            environment.next_source = environment
                .next_source
                .max(next_source_number(&added_sources))
                .max(
                    entry
                        .get("next_source")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(1),
                );
            // A parked source may have returned since the environment was loaded.
            // Retain absent references, but observe newly available checkouts.
            for source in &mut environment.sources {
                if source.path.is_dir() {
                    source.path =
                        std::fs::canonicalize(&source.path).unwrap_or_else(|_| source.path.clone());
                    source.is_git = source.path.join(".git").exists();
                }
            }
        }
        let saved_home = self.saved_home_source(id, entry, &added_sources);
        let project = self
            .projects
            .iter_mut()
            .find(|p| p.id == id)
            .expect("registered project");
        project.environment_id = environment_id.to_string();
        let key = entry
            .get("project_key")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        // Registration resolved the returning home. Only a legacy path key
        // follows that resolution; an opaque sibling identity stays stable.
        project.project_key = if Some(key) == super::configured_primary_path(entry) {
            project.repo_path.display().to_string()
        } else {
            key.to_string()
        };
        project.home_path = project.repo_path.clone();
        if let Some(name) = entry.get("name").and_then(serde_json::Value::as_str) {
            project.name = name.to_string();
        }
        project.archived_at = entry.get("archived_at").and_then(serde_json::Value::as_u64);
        project.isolation = crate::app::config::configured_isolation(entry, "project isolation");
        project.added_sources = added_sources;
        if let Some(home) = saved_home {
            project.sources = vec![home];
        }
        self.refresh_environment_projects(environment_id);
    }
}

impl ProjectRegistry {
    fn source_is_owned(
        source: &ProjectSource,
        environment: &Environment,
        added: &[ProjectSource],
    ) -> bool {
        environment
            .sources
            .iter()
            .chain(added)
            .any(|stored| stored.id == source.id && stored.path == source.path)
    }

    fn unowned_home_source(project: &Project, environment: &Environment) -> Option<ProjectSource> {
        let home = super::canonical_source_path(&project.repo_path).ok()?;
        project
            .sources
            .iter()
            .find(|source| {
                source.id == "source-1"
                    && source.path == home
                    && !Self::source_is_owned(source, environment, &project.added_sources)
            })
            .cloned()
    }

    fn initial_home_source(project: &Project) -> Option<ProjectSource> {
        let path = super::canonical_source_path(&project.repo_path).ok()?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("project")
            .to_string();
        Some(ProjectSource::added(
            "source-1".into(),
            name.clone(),
            super::safe_mount_name(&name),
            path,
            project.is_git,
            project.base_branch.clone(),
        ))
    }

    fn saved_home_source(
        &self,
        id: &str,
        entry: &serde_json::Value,
        added: &[ProjectSource],
    ) -> Option<ProjectSource> {
        let project = self.get(id)?;
        let home = super::canonical_source_path(&project.repo_path).ok()?;
        let environment = self.environment(entry.get("environment_id")?.as_str()?)?;
        let sources = super::configured_sources(entry.get("sources")?.as_array()?);
        sources.into_iter().find(|source| {
            source.id == "source-1"
                && source.path == home
                && !Self::source_is_owned(source, environment, added)
        })
    }

    /// A membership edit adopts the fallback without replacing another reference.
    fn promote_home_source(&mut self, id: &str, home: &ProjectSource) {
        let project = self
            .projects
            .iter_mut()
            .find(|project| project.id == id)
            .expect("resolved project");
        Self::upsert_added_source(&mut project.added_sources, home);
    }

    fn prepare_home_promotion(
        &self,
        id: &str,
        incoming: &mut [ProjectSource],
        fallback: Option<&ProjectSource>,
    ) -> Option<(ProjectSource, u64)> {
        let fallback = fallback?;
        let index = incoming
            .iter()
            .position(|source| source.id == fallback.id && source.path == fallback.path)?;
        let project = self.get(id)?;
        let environment = self.environment(&project.environment_id)?;
        let mut reservations = self.source_reservations(environment, incoming, Some(fallback));
        if reservations.used_ids.contains(&incoming[index].id) {
            incoming[index].id = reservations.allocate_id();
        }
        // Keep every observed counter floor even when the existing home ID
        // needs no replacement. The caller stages this with the membership edit.
        Some((incoming[index].clone(), reservations.next_source))
    }

    /// IDs are provisional until the membership transaction publishes them.
    pub(super) fn allocate_source_ids(
        &self,
        project_id: Option<&str>,
        count: usize,
    ) -> Result<Vec<String>, String> {
        let mut reservations = match project_id {
            Some(id) => {
                let project = self
                    .get(id)
                    .ok_or_else(|| format!("unknown project: {id}"))?;
                let environment = self
                    .environment(&project.environment_id)
                    .ok_or_else(|| format!("unknown environment: {}", project.environment_id))?;
                self.source_reservations(environment, &[], None)
            }
            None => SourceReservations::default(),
        };
        Ok((0..count).map(|_| reservations.allocate_id()).collect())
    }

    /// Reserve both held references and retired IDs recorded by every line.
    fn source_reservations(
        &self,
        environment: &Environment,
        incoming: &[ProjectSource],
        fallback: Option<&ProjectSource>,
    ) -> SourceReservations {
        let mut reservations = SourceReservations::default();
        reservations.reserve(
            environment.next_source,
            environment.sources.iter().map(|source| source.id.as_str()),
        );
        for project in self
            .projects
            .iter()
            .filter(|project| project.environment_id == environment.id)
        {
            reservations.reserve(
                project.next_source,
                project
                    .sources
                    .iter()
                    .filter(|source| {
                        !fallback.is_some_and(|fallback| {
                            source.id == fallback.id && source.path == fallback.path
                        })
                    })
                    .map(|source| source.id.as_str()),
            );
            reservations.reserve(
                0,
                project
                    .added_sources
                    .iter()
                    .map(|source| source.id.as_str()),
            );
        }
        for project in self.parked.iter().filter(|project| {
            project
                .get("environment_id")
                .and_then(serde_json::Value::as_str)
                == Some(environment.id.as_str())
        }) {
            let counter = project
                .get("next_source")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(1);
            let ids = ["sources", "added_sources"]
                .into_iter()
                .flat_map(|field| {
                    project
                        .get(field)
                        .and_then(serde_json::Value::as_array)
                        .into_iter()
                        .flatten()
                })
                .filter_map(|source| source.get("id").and_then(serde_json::Value::as_str));
            reservations.reserve(counter, ids);
        }
        reservations.reserve(
            0,
            incoming
                .iter()
                .filter(|source| {
                    !fallback.is_some_and(|fallback| {
                        source.id == fallback.id && source.path == fallback.path
                    })
                })
                .map(|source| source.id.as_str()),
        );
        reservations
    }
}

impl ProjectRegistry {
    pub(in crate::app) fn publish_creation_environment(&mut self, environment: Environment) {
        assert!(self.environment(&environment.id).is_none());
        self.environments.push(environment);
    }
    pub(in crate::app) fn remove_unused_environment(&mut self, id: &str) {
        self.environments.retain(|e| e.id != id);
    }
    pub(in crate::app) fn allocate_environment_source_ids(
        &self,
        id: &str,
        count: usize,
    ) -> Result<Vec<String>, String> {
        let environment = self.environment(id).ok_or("unknown environment")?;
        let mut reservations = self.source_reservations(environment, &[], None);
        Ok((0..count).map(|_| reservations.allocate_id()).collect())
    }
}
