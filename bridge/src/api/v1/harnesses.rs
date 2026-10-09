//! The harness inventory (#434): which harness CLIs are installed, which
//! versions are installed and running, and how each credential context is
//! signed in. The inventory service owns every observation; these handlers
//! read its snapshot or ask it to look again, and wait on no CLI.

use super::{Handler, NoParams};
use crate::api::ApiError;
use crate::app::AppState;
use crate::harness::inventory::{HarnessesSnapshot, RefreshReceipt};
use crate::{v1_method, v1_methods};

pub fn methods() -> &'static [(&'static str, Handler)] {
    v1_methods![
        v1_method!("harnesses.list", list, NoParams, HarnessesSnapshot),
        v1_method!("harnesses.refresh", refresh, NoParams, RefreshReceipt),
    ]
}

fn list(app: &mut AppState, _params: NoParams) -> Result<HarnessesSnapshot, ApiError> {
    Ok(app.harness_inventory().snapshot())
}

/// Schedule an observation of every context and answer with its receipt.
/// Never signs in, logs out or refreshes a token.
fn refresh(app: &mut AppState, _params: NoParams) -> Result<RefreshReceipt, ApiError> {
    Ok(app.harness_inventory().request_refresh())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_shapes_match_the_typed_inventory() {
        for (method, _) in methods() {
            super::super::testing::fixture_round_trips(methods(), method);
        }
    }
}
