//! Environment reads, creation and forward-looking reference edits.
use super::board::{ProjectRow, ProjectSourceParams, ProjectSourceRow};
use super::workspace::WorkspaceRow;
use super::{answer, Answer, Handler, NoParams, WireParams};
use crate::api::ApiError;
use crate::app::AppState;
use crate::{v1_method, v1_methods};
use serde::{Deserialize, Serialize};

pub fn methods() -> &'static [(&'static str, Handler)] {
    v1_methods![
        v1_method!("environment.delete", delete, DeleteParams, super::board::EmptyResult),
        v1_method!("environment.rename", rename, RenameParams, EnvironmentRow),
        v1_method!(
            "environment.add_source",
            add_source,
            AddSourceParams,
            EnvironmentRow
        ),
        v1_method!(
            "environment.remove_source",
            remove_source,
            RemoveSourceParams,
            EnvironmentRow
        ),
        v1_method!(
            "environment.update_source",
            update_source,
            UpdateSourceParams,
            SourceUpdated
        ),
        v1_method!(
            "environment.sync_source",
            sync_source,
            SyncSourceParams,
            super::board::SourceSyncRequested
        ),
        v1_method!(
            "environment.set_isolation",
            set_isolation,
            SetIsolationParams,
            EnvironmentRow
        ),
        v1_method!("environment.create", create, CreateParams, CreateResult),
        v1_method!(
            "environment.discard_unused",
            discard_unused,
            DiscardUnusedParams,
            super::board::EmptyResult
        ),
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
    pub added_sources: Option<Vec<ProjectSourceParams>>,
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

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FirstProjectParams {
    pub name: String,
    pub workspace: CreateWorkspaceParams,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_sources: Option<Vec<ProjectSourceParams>>,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct CreateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub sources: Vec<ProjectSourceParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub project: FirstProjectParams,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continues: Option<String>,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct CreateResult {
    pub environment: EnvironmentRow,
    pub project: ProjectRow,
    pub workspace: WorkspaceRow,
}
fn create(_: &mut AppState, _: CreateParams) -> Result<Answer<CreateResult>, ApiError> {
    Err(ApiError::invalid_params(
        "environment.create requires frame write.operation_id",
    ))
}
#[derive(Debug, Deserialize, Serialize)]
pub struct DiscardUnusedParams {
    pub environment_id: String,
    pub creation_operation_id: String,
}
fn discard_unused(
    app: &mut AppState,
    params: DiscardUnusedParams,
) -> Result<Answer<super::board::EmptyResult>, ApiError> {
    let value = app
        .environment_discard_unused(&params.wire())
        .map_err(super::board::discard_unused_error)?;
    answer(Ok(value))
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RenameParams {
    pub environment_id: String,
    pub name: String,
}
#[derive(Debug, Deserialize, Serialize)]
pub struct SourceUpdated {
    #[serde(flatten)]
    pub environment: EnvironmentRow,
    pub checkouts_updated: u64,
    pub checkouts_failed: Vec<super::board::CheckoutLeftBehind>,
}

fn rename(app: &mut AppState, params: RenameParams) -> Result<Answer<EnvironmentRow>, ApiError> {
    answer(app.environment_rename(&params.wire())).map_err(super::board::refine)
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AddSourceParams {
    pub environment_id: String,
    /// Absolute or `~`-relative. Mutually exclusive with `remote`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// A clone url. Mutually exclusive with `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// The directory's own last segment (or the url's repo name) when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The source's own checked-out branch when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
}

fn add_source(
    app: &mut AppState,
    params: AddSourceParams,
) -> Result<Answer<EnvironmentRow>, ApiError> {
    answer(app.environment_add_source(&params.wire())).map_err(super::board::refine)
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RemoveSourceParams {
    pub environment_id: String,
    pub source_id: String,
}

fn remove_source(
    app: &mut AppState,
    params: RemoveSourceParams,
) -> Result<Answer<EnvironmentRow>, ApiError> {
    answer(app.environment_remove_source(&params.wire())).map_err(super::board::refine)
}

#[derive(Debug, Deserialize, Serialize)]
pub struct UpdateSourceParams {
    pub environment_id: String,
    pub source_id: String,
    /// The label shown for the source. Its folder name inside workspaces
    /// (`mount`) does not change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Another folder for the source to stand on. Refused for the first
    /// source, which is the project's home.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// A branch the source's checkout has, locally or on `origin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_branch: Option<String>,
    /// The checkout's new `origin`; empty takes it off. Written to the
    /// source's checkout, and to each workspace checkout with a repository of
    /// its own that still named the old one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Keep the source's base branch in step with its remote, fast-forward
    /// only (since 3.3.0, `sources.syncBase`). Turning it on syncs at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_base: Option<bool>,
}

fn update_source(
    app: &mut AppState,
    params: UpdateSourceParams,
) -> Result<Answer<SourceUpdated>, ApiError> {
    answer(app.environment_update_source(&params.wire())).map_err(super::board::refine)
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SyncSourceParams {
    pub environment_id: String,
    pub source_id: String,
    /// Captured source location, announced by sources.syncBasePath.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
}

fn sync_source(
    app: &mut AppState,
    params: SyncSourceParams,
) -> Result<Answer<super::board::SourceSyncRequested>, ApiError> {
    answer(app.environment_sync_source(&params.wire())).map_err(super::board::refine)
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SetIsolationParams {
    pub environment_id: String,
    /// `worktree` or `cow`, and `null` to go back to inheriting the account's
    /// choice. Required — naming no isolation at all is a missing param while
    /// naming `null` is the clear, which is why it reads through
    /// [`required_nullable`] (serde would otherwise let a missing field pass
    /// as `None`) and is not skipped when it serialises.
    #[serde(deserialize_with = "super::board::required_nullable")]
    pub isolation: Option<String>,
}

fn set_isolation(
    app: &mut AppState,
    params: SetIsolationParams,
) -> Result<Answer<EnvironmentRow>, ApiError> {
    answer(app.environment_set_isolation(&params.wire())).map_err(super::board::refine)
}

/// Delete an empty environment registration, preserving its source folders.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteParams {
    pub environment_id: String,
}

fn delete(app: &mut AppState, params: DeleteParams) -> Result<Answer<super::board::EmptyResult>, ApiError> {
    answer(app.environment_delete(&params.environment_id)).map_err(super::board::refine)
}
