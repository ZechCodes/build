//! Sibling fixtures share the production environment candidate builder.

use crate::app::AppState;

pub(in crate::app) fn register_sibling(app: &mut AppState, existing: &str) -> String {
    let project = app.project(existing).expect("existing sibling project");
    let environment = project.environment_id.clone();
    let name = project.name.clone();
    let candidate = app.project_candidate_with_sources(
        project.home_path.clone(),
        project.base_branch.clone(),
        project.is_git,
        project.sources.clone(),
    );
    let key = format!("project:{}", uuid::Uuid::new_v4());
    let candidate = app
        .projects
        .add_project_to_environment(&environment, &name, &key, candidate)
        .unwrap();
    app.insert_project(candidate)
}
