//! No notice is excluded without a typed rule, enrollment, and publication proof.
use crate::app::AppState;

impl AppState {
    pub(in crate::app) fn cancel_optional_notices_for_cleanup(
        &mut self,
    ) -> Result<usize, &'static str> {
        Err("required logical work is still active")
    }
}
