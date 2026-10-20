//! SQL creates a complete immutable review; Git only adds live destinations.
use crate::app::projects::ProjectSource;
use crate::app::AppState;
use crate::reviews::actions::ActionSource;
use crate::reviews::records::Review;
use im::OrdMap;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, Default)]
pub(super) struct ReviewSources {
    projects: OrdMap<String, Arc<Vec<ProjectSource>>>,
    workspaces: OrdMap<String, Arc<crate::workspace::Workspace>>,
    checkouts: OrdMap<String, OrdMap<String, Arc<crate::workspace::Workspace>>>,
    reservations: OrdMap<String, Arc<(PathBuf, Instant)>>,
}

impl ReviewSources {
    pub(super) fn capture(app: &AppState) -> Self {
        let mut sources = Self::default();
        for project in app.projects.iter() {
            #[cfg(test)]
            crate::app::observation_reads::capture_metrics::record("review_project", &project.id);
            sources
                .projects
                .insert(project.id.clone(), Arc::new(project.sources.clone()));
        }
        for workspace in app.workspaces.list(None) {
            #[cfg(test)]
            crate::app::observation_reads::capture_metrics::record(
                "review_workspace",
                &workspace.id,
            );
            sources.workspace(&workspace.id, &Some(Arc::new(workspace.clone())));
        }
        #[cfg(test)]
        crate::app::observation_reads::capture_metrics::record("reservation_scan", "");
        for (id, row) in app.read_reclaim_reservation_rows() {
            sources.reservations.insert(id, Arc::new(row));
        }
        sources
    }

    pub(super) fn project(&self, _id: &str) -> Self {
        self.clone()
    }
    pub(super) fn update_project(&mut self, id: &str, row: &Option<Arc<Vec<ProjectSource>>>) {
        match row {
            Some(row) => {
                self.projects.insert(id.into(), row.clone());
            }
            None => {
                self.projects.remove(id);
            }
        }
    }
    pub(super) fn workspace(&mut self, id: &str, row: &Option<Arc<crate::workspace::Workspace>>) {
        if let Some(previous) = self.workspaces.get(id) {
            let project = previous.project_id.clone();
            let mut checkouts = self.checkouts.get(&project).cloned().unwrap_or_default();
            checkouts.remove(id);
            self.checkouts.insert(project, checkouts);
        }
        match row {
            Some(row) => {
                let mut checkouts = self
                    .checkouts
                    .get(&row.project_id)
                    .cloned()
                    .unwrap_or_default();
                checkouts.insert(id.into(), row.clone());
                self.checkouts.insert(row.project_id.clone(), checkouts);
                self.workspaces.insert(id.into(), row.clone());
            }
            None => {
                self.workspaces.remove(id);
            }
        }
    }
    pub(super) fn reservation(&mut self, id: &str, row: &Option<Arc<(PathBuf, Instant)>>) {
        match row {
            Some(row) => {
                self.reservations.insert(id.into(), row.clone());
            }
            None => {
                self.reservations.remove(id);
            }
        }
    }

    pub(super) fn destinations(&self, project: &str, review: Review) -> Review {
        let configured = self
            .projects
            .get(project)
            .map(|sources| sources.as_slice())
            .unwrap_or_default();
        let sources = review
            .snapshots
            .iter()
            .flat_map(|snapshot| snapshot.directories.iter())
            .map(|directory| {
                let source = configured
                    .iter()
                    .find(|source| source.id == directory.source_id);
                let source_path = source
                    .map(|source| source.path.clone())
                    .unwrap_or_else(|| directory.source_path.clone());
                let error = match source {
                    None => Some(format!(
                        "source {} is no longer configured",
                        directory.source_id
                    )),
                    Some(_) => self
                        .reserved(project, &directory.source_id, &directory.path, &source_path)
                        .err(),
                };
                ActionSource {
                    directory: directory.clone(),
                    source_path,
                    error,
                }
            })
            .collect::<Vec<_>>();
        crate::reviews::actions::with_destinations(review, &sources)
    }

    fn reserved(
        &self,
        project: &str,
        source: &str,
        directory: &Path,
        source_path: &Path,
    ) -> Result<(), String> {
        self.check_path(directory)?;
        self.check_path(source_path)?;
        if let Some(workspaces) = self.checkouts.get(project) {
            for workspace in workspaces.values() {
                for checkout in workspace
                    .directories
                    .iter()
                    .filter(|checkout| checkout.source_id == source)
                {
                    self.check_path(&checkout.path)?;
                }
            }
        }
        Ok(())
    }

    fn check_path(&self, path: &Path) -> Result<(), String> {
        let path = crate::worktree::canonical_planned_path(path);
        if self
            .reservations
            .values()
            .any(|row| row.1 > Instant::now() && path.starts_with(&row.0))
        {
            return Err(crate::reclaim::RESERVED.into());
        }
        Ok(())
    }
}

pub(super) fn result(
    review: Review,
    sync: Vec<crate::reviews::model::ReviewSyncObservation>,
    intents: Vec<crate::reviews::model::ReviewMergeIntent>,
) -> Value {
    let mut value = json!({"review":review});
    if !sync.is_empty() {
        value["sync"] = json!(sync);
    }
    if !intents.is_empty() {
        value["merge_intents"] = json!(intents);
    }
    value
}
