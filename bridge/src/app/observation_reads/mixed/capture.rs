//! Keyed committed inputs; derived joins stay in the requested reader.
use super::*;
use crate::app::board::archive::ArchivedEntity;
use crate::app::conversations::sql_read::AttachmentOwner;
use crate::app::projects::ProjectSource;
use crate::workspace::Workspace;
use std::collections::BTreeMap;
use std::time::Instant;

struct Owner {
    document: Option<Arc<documents::Document>>,
    thread: Option<Arc<AttachmentOwner>>,
    home: Option<Arc<homes::HomeInput>>,
    archive: Option<Arc<ArchivedEntity>>,
}

struct Project {
    path: Option<String>,
    sources: Option<Arc<Vec<ProjectSource>>>,
    archive: Option<Arc<(String, String)>>,
}

#[derive(Default)]
pub(in crate::app::observation_reads) struct MixedPatch {
    owners: BTreeMap<String, Owner>,
    projects: BTreeMap<String, Project>,
    workspaces: BTreeMap<String, Option<Arc<Workspace>>>,
    reservations: BTreeMap<String, Option<Arc<(PathBuf, Instant)>>>,
    records: Option<Arc<Vec<crate::store::PersistedArchivedWorktree>>>,
    owner_order: Option<Arc<Vec<String>>>,
    run_order: Option<Vec<String>>,
    project_order: Option<Arc<Vec<String>>>,
}

impl MixedPatch {
    pub fn owner(&mut self, app: &AppState, id: &str) {
        self.owners.insert(
            id.into(),
            Owner {
                document: documents::Documents::owner(app, id),
                thread: AttachmentReadContext::owner(app, id),
                home: homes::HomeInput::capture(app, id),
                archive: app.archive_entity(id),
            },
        );
    }
    pub fn project(&mut self, app: &AppState, id: &str) {
        #[cfg(test)]
        super::super::capture_metrics::record("review_project", id);
        let row = app.projects.get(id);
        self.projects.insert(
            id.into(),
            Project {
                path: row.map(|row| row.repo_path.display().to_string()),
                sources: row.map(|row| Arc::new(row.sources.clone())),
                archive: row
                    .map(|row| Arc::new((row.name.clone(), row.repo_path.display().to_string()))),
            },
        );
    }
    pub fn workspace(&mut self, app: &AppState, id: &str) {
        #[cfg(test)]
        super::super::capture_metrics::record("review_workspace", id);
        self.workspaces
            .insert(id.into(), app.workspaces.get(id).cloned().map(Arc::new));
    }
    pub fn reservation(&mut self, app: &AppState, id: &str) {
        self.reservations
            .insert(id.into(), app.read_reclaim_reservation(id).map(Arc::new));
    }
    pub fn archives(&mut self, app: &AppState) {
        #[cfg(test)]
        super::super::capture_metrics::record("archive_scan", "");
        self.records = Some(Arc::new(
            app.board
                .archived_values()
                .filter(|record| record.status == crate::store::WorktreeFinishStatus::Archived)
                .cloned()
                .collect(),
        ));
    }
    pub fn topology(&mut self, app: &AppState) {
        self.owner_order = Some(Arc::new(
            app.plans
                .keys()
                .chain(app.runs.keys())
                .map(|id| {
                    #[cfg(test)]
                    super::super::capture_metrics::record("mixed_topology", id);
                    id.clone()
                })
                .collect(),
        ));
        self.run_order = Some(app.runs.keys().cloned().collect());
    }
    pub fn project_topology(&mut self, app: &AppState) {
        self.project_order = Some(Arc::new(
            app.projects
                .iter()
                .map(|project| project.id.clone())
                .collect(),
        ));
    }
}

impl MixedReadContext {
    pub(in crate::app::observation_reads) fn contains_owner(&self, id: &str) -> bool {
        self.homes.contains(id)
    }
    pub(in crate::app::observation_reads) fn apply(&mut self, patch: &MixedPatch) {
        let archive = Arc::make_mut(&mut self.archive);
        for (id, row) in &patch.owners {
            self.documents.apply(id, &row.document);
            self.threads.apply_owner(id, &row.thread);
            self.homes.owner(id, &row.home);
            archive.apply_owner(id, &row.archive);
        }
        for (id, row) in &patch.projects {
            self.tasks.update_scope_project(id, row.path.clone());
            self.sources.update_project(id, &row.sources);
            archive.project(id, &row.archive);
        }
        for (id, row) in &patch.workspaces {
            self.sources.workspace(id, row);
            archive.workspace(id, row);
        }
        for (id, row) in &patch.reservations {
            self.sources.reservation(id, row);
        }
        if let Some(records) = &patch.records {
            archive.records(records.clone());
        }
        if let Some(order) = &patch.owner_order {
            archive.owner_order(order.clone());
        }
        if let Some(order) = &patch.run_order {
            self.homes.topology(order.clone());
        }
        if let Some(order) = &patch.project_order {
            archive.project_order(order.clone());
        }
    }
}
