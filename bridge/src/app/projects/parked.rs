//! A project whose folder is away is parked, never forgotten.
//!
//! A boot that cannot find a configured project's folder does not load it,
//! because nothing in it can be read. It keeps the project's configuration
//! as it was written, saves it back on every save, and looks again whenever
//! something would need the project: a worker starting, the project list,
//! the project's own conversation, and the source-sync timer. The moment the
//! folder is back the project is loaded under its own id, and every owner a
//! boot could bind to no project — the project's own conversation among them,
//! with its pin and workers — is bound to it again.

use super::configured_primary_path;
use crate::app::AppState;

impl AppState {
    /// Load every parked project whose folder is back, and answer their ids.
    pub(in crate::app) fn revive_parked_projects(&mut self) -> Vec<String> {
        let returned = self.projects.take_returned(|entry| {
            configured_primary_path(entry).is_some_and(|path| std::path::Path::new(path).exists())
        });
        let revived: Vec<String> = returned
            .iter()
            .filter_map(|entry| self.restore_configured_project(entry))
            .collect();
        for project_id in &revived {
            let repo = self
                .projects
                .get(project_id)
                .map(|project| project.repo_path.clone());
            if let Some(repo) = repo {
                self.projects.bind_retained_entities(project_id, &repo);
            }
        }
        if !revived.is_empty() {
            self.note_board_lists_changed(crate::changes::BoardLists::PROJECTS);
        }
        revived
    }
}
