use crate::api::ApiError;
use crate::app::AppState;
use crate::store::ReadTransaction;
use serde_json::Value;

pub(in crate::app) struct MixedReadContext;
pub(in crate::app) struct MixedJob;
pub(in crate::app) struct MixedFence;

impl MixedReadContext {
    pub fn capture(_: &AppState) -> Self { Self }
    pub fn prepare(&self, method: &str, _: &Value, _: Option<&ReadTransaction<'_>>) -> Result<MixedJob, ApiError> {
        Err(ApiError::unknown_method(method))
    }
    pub fn validate(&self, _: &MixedFence) -> Result<(), ApiError> { Ok(()) }
}

impl MixedJob {
    pub fn fence(&self) -> MixedFence { MixedFence }
    pub fn run(self) -> Result<Value, ApiError> { unreachable!() }
}
