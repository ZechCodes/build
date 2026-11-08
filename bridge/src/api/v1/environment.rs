//! Environment read wire, staged until the M1 capability is advertised.
use super::board::ProjectSourceRow;
use super::{answer, Answer, Handler, NoParams};
use crate::api::ApiError;
use crate::app::AppState;
use crate::{v1_method, v1_methods};
use serde::{Deserialize, Serialize};

pub fn methods() -> &'static [(&'static str, Handler)] {
    v1_methods![v1_method!(
        "environment.list",
        environment_list,
        NoParams,
        EnvironmentListResult
    )]
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
