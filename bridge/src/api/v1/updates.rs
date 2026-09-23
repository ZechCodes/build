//! Bridge release status and update commands. The service owns network and
//! helper work; these handlers only read or durably accept a command.

use super::{Handler, NoParams};
use crate::api::ApiError;
use crate::app::AppState;
use crate::update::{InstallWhen, UpdateError, UpdateService, UpdateStatus};
use crate::{v1_method, v1_methods};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub fn methods() -> &'static [(&'static str, Handler)] {
    v1_methods![
        v1_method!(
            "bridge.update_status",
            update_status,
            NoParams,
            UpdateStatus
        ),
        v1_method!("bridge.check_update", check_update, NoParams, UpdateStatus),
        v1_method!(
            "bridge.install_update",
            install_update,
            InstallParams,
            UpdateStatus
        ),
    ]
}

#[derive(Debug, Deserialize, Serialize)]
pub struct InstallParams {
    pub when: InstallWhen,
}

fn service(app: &AppState) -> Result<Arc<UpdateService>, ApiError> {
    app.update_service()
        .ok_or_else(|| ApiError::unavailable("bridge updates are unavailable"))
}

fn update_status(app: &mut AppState, _params: NoParams) -> Result<UpdateStatus, ApiError> {
    Ok(service(app)?.status())
}

fn check_update(app: &mut AppState, _params: NoParams) -> Result<UpdateStatus, ApiError> {
    service(app)?.request_check().map_err(update_error)
}

fn install_update(app: &mut AppState, params: InstallParams) -> Result<UpdateStatus, ApiError> {
    service(app)?
        .request_install(params.when, app.update_has_working_agents())
        .map_err(update_error)
}

fn update_error(error: UpdateError) -> ApiError {
    match error {
        UpdateError::DevelopmentBuild => ApiError::unavailable(error.to_string()),
        UpdateError::NoUpdateAvailable => ApiError::conflict(error.to_string(), None),
        UpdateError::AlreadyInstalling | UpdateError::AlreadyChecking | UpdateError::Busy => {
            ApiError::busy(error.to_string())
        }
        _ => ApiError::internal(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_shapes_match_typed_status() {
        for (method, _) in methods() {
            super::super::testing::fixture_round_trips(methods(), method);
        }
    }
}
