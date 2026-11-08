//! Environment reads and journaled creation in an existing environment.
use super::board::{ProjectRow, ProjectSourceRow};
use super::workspace::WorkspaceRow;
use super::{answer, Answer, Handler, NoParams, WireParams};
use crate::api::ApiError;
use crate::app::AppState;
use crate::{v1_method, v1_methods};
use serde::{Deserialize, Serialize};

pub fn methods() -> &'static [(&'static str, Handler)] {
    v1_methods![
        v1_method!(
            "environment.list",
            environment_list,
            NoParams,
            EnvironmentListResult
        ),
        v1_method!(
            "environment.create_project",
            create_project,
            CreateProjectParams,
            CreateProjectResult
        )
    ]
}

#[derive(Debug, Deserialize, Serialize)]
pub struct EnvironmentRow {
    pub environment_id: String,
    pub name: String,
    pub home_path: String,
    pub is_git: bool,
    pub base_branch: String,
    pub sources: Vec<ProjectSourceRow>,
    pub isolation: Option<String>,
    pub isolation_effective: String,
    pub project_ids: Vec<String>,
    pub device: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct EnvironmentListResult {
    pub environments: Vec<EnvironmentRow>,
}

fn environment_list(
    app: &mut AppState,
    _: NoParams,
) -> Result<Answer<EnvironmentListResult>, ApiError> {
    answer(Ok(app.environment_list()))
}

#[derive(Debug, Deserialize, Serialize)]
pub struct CreateProjectParams {
    pub environment_id: String,
    pub name: String,
    pub workspace: CreateWorkspaceParams,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continues: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CreateWorkspaceParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct CreateProjectResult {
    pub project: ProjectRow,
    pub workspace: WorkspaceRow,
}
fn create_project(
    app: &mut AppState,
    params: CreateProjectParams,
) -> Result<Answer<CreateProjectResult>, ApiError> {
    let value = app
        .environment_create_project(&params.wire())
        .map_err(ApiError::invalid_params)?;
    answer(Ok(value))
}
