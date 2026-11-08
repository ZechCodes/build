//! Sibling fixtures share the production environment candidate builder.
use crate::app::AppState;

impl AppState {
    pub(in crate::app) fn add_environment_sibling_for_test(&mut self, project: &str) -> String {
        let original = self.projects.get(project).unwrap();
        let environment = original.environment_id.clone();
        let name = original.name.clone();
        let candidate = self.project_candidate_with_sources(
            original.home_path.clone(),
            original.base_branch.clone(),
            original.is_git,
            original.sources.clone(),
        );
        let key = format!("project:{}", uuid::Uuid::new_v4());
        let candidate = self
            .projects
            .add_project_to_environment(&environment, &name, &key, candidate)
            .unwrap();
        self.insert_project(candidate)
    }
}
