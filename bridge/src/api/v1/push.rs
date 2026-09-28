//! Notification keys for sealed push content (#200, wire 2.1.0). The browser
//! registers the key its push content is sealed to, and revokes it when push
//! is turned off; both only over the E2EE session.

use crate::api::ApiError;
use crate::app::{AppState, PushKeyRefusal};
use crate::{v1_method, v1_methods};
use serde::{Deserialize, Serialize};

use super::Handler;

pub fn methods() -> &'static [(&'static str, Handler)] {
    v1_methods![
        v1_method!(
            "push.registerKey",
            register_key,
            RegisterKeyParams,
            Acknowledged
        ),
        v1_method!("push.revokeKey", revoke_key, RevokeKeyParams, Acknowledged),
    ]
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RegisterKeyParams {
    /// `b64u(SHA-256(push endpoint))`, 43 characters.
    pub subscription_id: String,
    /// `b64u(uncompressed P-256 point)`, 65 bytes.
    pub public_key: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct RevokeKeyParams {
    pub subscription_id: String,
}

/// `{}`: the verb did what it says.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Acknowledged {}

fn register_key(app: &mut AppState, params: RegisterKeyParams) -> Result<Acknowledged, ApiError> {
    app.register_push_key(&params.subscription_id, &params.public_key)
        .map(|()| Acknowledged {})
        .map_err(refusal)
}

fn revoke_key(app: &mut AppState, params: RevokeKeyParams) -> Result<Acknowledged, ApiError> {
    app.revoke_push_key(&params.subscription_id)
        .map(|()| Acknowledged {})
        .map_err(refusal)
}

fn refusal(refused: PushKeyRefusal) -> ApiError {
    match refused {
        PushKeyRefusal::BadSubscription | PushKeyRefusal::BadKey => {
            ApiError::invalid_params(refused.to_string())
        }
        PushKeyRefusal::NoStore => ApiError::unavailable(refused.to_string()),
        PushKeyRefusal::NotSaved => ApiError::internal(refused.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_shapes_match_the_typed_verbs() {
        for (method, _) in methods() {
            super::super::testing::fixture_round_trips(methods(), method);
        }
    }
}
