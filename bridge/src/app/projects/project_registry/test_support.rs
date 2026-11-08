//! Sibling fixtures bypass creation RPCs until the creation slice lands.
use crate::app::AppState;

impl AppState {
    pub(in crate::app) fn add_environment_sibling_for_test(&mut self, project: &str) -> String {
        let original = self.projects.get(project).unwrap();
        let environment = original.environment_id.clone();
        let mut candidate = self.project_candidate_with_sources(
            original.home_path.clone(), original.base_branch.clone(), original.is_git,
            original.sources.clone(),
        );
        candidate.project.environment_id = environment;
        candidate.project.project_key = format!("project:test-sibling-{}", self.projects.next_id());
        self.insert_project(candidate)
    }
}
