//! Sibling fixtures without exposing the future project-creation RPC.

use crate::app::AppState;

pub(in crate::app) fn register_sibling(app: &mut AppState, existing: &str) -> String {
    let project = app.project(existing).expect("existing sibling project");
    let environment = project.environment_id.clone();
    let mut candidate = app.project_candidate_with_sources(
        project.home_path.clone(),
        project.base_branch.clone(),
        project.is_git,
        project.sources.clone(),
    );
    candidate.set_test_identity(environment, format!("project:{}", uuid::Uuid::new_v4()));
    app.insert_project(candidate)
}
