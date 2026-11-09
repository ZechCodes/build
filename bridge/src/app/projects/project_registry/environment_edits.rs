//! Environment edits preserve project identities and rebuild only derived sources.
use super::*;

impl ProjectRegistry {
    pub(in crate::app) fn environment_mut(&mut self, id: &str) -> Option<&mut Environment> {
        self.environments.iter_mut().find(|e| e.id == id)
    }
    pub(in crate::app) fn rename_project(&mut self, id: &str, name: String) -> Result<(), String> {
        self.observation_projects.mark(id);
        let project = self
            .projects
            .iter_mut()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("unknown project: {id}"))?;
        project.name = name;
        Ok(())
    }
    /// Includes unavailable and parked references, and the temporary live home fallback.
    pub(in crate::app) fn environment_reference_sources(
        &self,
        id: &str,
        except: Option<&str>,
    ) -> Vec<ProjectSource> {
        self.source_references()
            .into_iter()
            .filter(|r| r.environment_id == id)
            .filter(|r| !(r.project_id.is_none() && except == Some(r.source.id.as_str())))
            .map(|r| canonical_reference(r.source))
            .collect()
    }
    pub(in crate::app) fn registered_except_environment_source(
        &self,
        id: &str,
        source: &str,
    ) -> Vec<ProjectSource> {
        self.source_references()
            .into_iter()
            .filter(|r| {
                !(r.environment_id == id && r.project_id.is_none() && r.source.id == source)
            })
            .map(|r| canonical_reference(r.source))
            .collect()
    }
    pub(in crate::app) fn environment_sources_mut(
        &mut self,
    ) -> impl Iterator<Item = (&str, &mut ProjectSource)> {
        self.environments.iter_mut().flat_map(|e| {
            let id = e.id.as_str();
            e.sources.iter_mut().map(move |s| (id, s))
        })
    }
}

impl ProjectConfiguration {
    /// A deferred configuration never replaces conclusions a sync made meanwhile.
    pub(in crate::app) fn restore_runtime_into(&self, projects: &mut ProjectRegistry) {
        for (id, record) in &self.records {
            if let Some(project) = projects.projects.iter_mut().find(|p| p.id == *id) {
                copy_statuses(&record.sources, &mut project.sources);
                copy_statuses(&record.sources, &mut project.added_sources);
            }
        }
        for environment in &self.environments {
            if let Some(current) = projects.environment_mut(&environment.id) {
                copy_statuses(&environment.sources, &mut current.sources);
            }
        }
    }
}
fn copy_statuses(previous: &[ProjectSource], current: &mut [ProjectSource]) {
    for source in current {
        source.sync_status = previous
            .iter()
            .find(|old| {
                old.id == source.id
                    && old.path == source.path
                    && old.base_branch == source.base_branch
            })
            .and_then(|old| old.sync_status.clone());
    }
}
