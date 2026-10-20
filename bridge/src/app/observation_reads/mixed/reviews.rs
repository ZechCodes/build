//! SQL creates a complete immutable review; Git only adds live destinations.
use crate::app::projects::ProjectSource;
use crate::app::AppState;
use crate::reviews::actions::ActionSource;
use crate::reviews::records::Review;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub(super) struct ReviewSources {
    projects: HashMap<String, Vec<ProjectSource>>,
    checkouts: HashMap<String, Vec<(String, PathBuf)>>,
    reservations: Vec<(PathBuf, Instant)>,
}

impl ReviewSources {
    pub(super) fn capture(app: &AppState) -> Self {
        let mut checkouts: HashMap<String, Vec<(String, PathBuf)>> = HashMap::new();
        for workspace in app.workspaces.list(None) {
            #[cfg(test)]
            crate::app::observation_reads::capture_metrics::record(
                "review_workspace",
                &workspace.id,
            );
            checkouts
                .entry(workspace.project_id.clone())
                .or_default()
                .extend(
                    workspace
                        .directories
                        .iter()
                        .map(|directory| (directory.source_id.clone(), directory.path.clone())),
                );
        }
        #[cfg(test)]
        crate::app::observation_reads::capture_metrics::record("reservation_scan", "");
        Self {
            projects: app
                .projects
                .iter()
                .map(|project| {
                    #[cfg(test)]
                    crate::app::observation_reads::capture_metrics::record(
                        "review_project",
                        &project.id,
                    );
                    (project.id.clone(), project.sources.clone())
                })
                .collect(),
            checkouts,
            reservations: app.read_reclaim_reservations(),
        }
    }

    pub(super) fn project(&self, id: &str) -> Self {
        Self {
            projects: self
                .projects
                .get(id)
                .map(|sources| HashMap::from([(id.into(), sources.clone())]))
                .unwrap_or_default(),
            checkouts: self
                .checkouts
                .get(id)
                .map(|paths| HashMap::from([(id.into(), paths.clone())]))
                .unwrap_or_default(),
            reservations: self.reservations.clone(),
        }
    }

    pub(super) fn destinations(&self, project: &str, review: Review) -> Review {
        let configured = self
            .projects
            .get(project)
            .map(Vec::as_slice)
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
        for (_, path) in self
            .checkouts
            .get(project)
            .into_iter()
            .flatten()
            .filter(|(id, _)| id == source)
        {
            self.check_path(path)?;
        }
        Ok(())
    }

    fn check_path(&self, path: &Path) -> Result<(), String> {
        let path = crate::worktree::canonical_planned_path(path);
        if self
            .reservations
            .iter()
            .any(|(root, expires)| *expires > Instant::now() && path.starts_with(root))
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
