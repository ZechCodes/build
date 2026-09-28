//! The notification keys a browser registers over E2EE (#200).
//!
//! `push.registerKey` is the only way a notification public key reaches the
//! bridge, and it arrives only over the E2EE session — never through the api.
//! That is the whole of what stops forged push content: the seal is not
//! sender-authenticated, so the key must stay secret. It is validated here,
//! at registration, so a key that could never be sealed to is refused where
//! the browser can hear about it.

use crate::app::AppState;
use crate::notify::seal::{is_subscription_id, parse_public_key};

/// Why a key was not registered or revoked. Each reads as a sentence, and
/// none repeats the key it refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushKeyRefusal {
    BadSubscription,
    BadKey,
    NoStore,
    NotSaved,
}

impl std::fmt::Display for PushKeyRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PushKeyRefusal::BadSubscription => {
                "Build cannot use that subscription id: it must be 43 base64url characters."
            }
            PushKeyRefusal::BadKey => {
                "Build cannot use that notification key: it must be an uncompressed P-256 public key."
            }
            PushKeyRefusal::NoStore => "Build cannot keep notification keys without its store.",
            PushKeyRefusal::NotSaved => "Build cannot save notification keys right now.",
        })
    }
}

impl AppState {
    /// Register (or replace) the notification key for one subscription.
    pub fn register_push_key(
        &mut self,
        subscription_id: &str,
        public_key: &str,
    ) -> Result<(), PushKeyRefusal> {
        let store = self.push_key_store(subscription_id)?;
        parse_public_key(public_key).map_err(|_| PushKeyRefusal::BadKey)?;
        store
            .upsert_push_key(subscription_id, public_key, unix_millis())
            .map_err(|_| PushKeyRefusal::NotSaved)
    }

    /// Forget one subscription's key. Forgetting one that is not here is no
    /// error: the browser's intent — no key — already holds.
    pub fn revoke_push_key(&mut self, subscription_id: &str) -> Result<(), PushKeyRefusal> {
        self.push_key_store(subscription_id)?
            .delete_push_key(subscription_id)
            .map_err(|_| PushKeyRefusal::NotSaved)
    }

    fn push_key_store(
        &self,
        subscription_id: &str,
    ) -> Result<&crate::store::Store, PushKeyRefusal> {
        if !is_subscription_id(subscription_id) {
            return Err(PushKeyRefusal::BadSubscription);
        }
        self.store.as_ref().ok_or(PushKeyRefusal::NoStore)
    }
}

fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or(0)
}
