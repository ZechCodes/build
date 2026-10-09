//! Passive authentication adapters, one per credential context.
//!
//! Passive means: no sign-in, no token refresh, no inference, and no helper
//! command a CLI is configured to run. Each adapter reads saved metadata —
//! which credentials a file holds and when they expire, never their values —
//! and, where the CLI's own status command can be shown not to refresh or
//! run helpers, that command's allowlisted answer.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer};

use super::environment::DeviceEnvironment;
use super::model::{AuthFacts, AuthMethod, AuthStatus};
use crate::harness::installed::executable::Executable;

mod claude;
mod codex;
mod pi;

pub use claude::CLAUDE_AUTH;
pub use codex::CODEX_AUTH;
pub use pi::PI_AUTH;

/// Why an observation found nothing it could stand behind. Its prior facts
/// stand, marked stale. Never carries what the CLI said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservationFailed(pub &'static str);

/// How one credential context is observed.
pub trait AuthAdapter: Send + Sync {
    /// The context's id on the wire.
    fn context(&self) -> &'static str;

    /// The files whose change means the context should be looked at again.
    fn watched(&self, environment: &DeviceEnvironment) -> Vec<PathBuf>;

    /// What is saved for this context now. `executable` is the CLI a spawn
    /// would run, where one is installed.
    fn observe(
        &self,
        environment: &DeviceEnvironment,
        executable: Option<&Executable>,
        now: SystemTime,
    ) -> Result<AuthFacts, ObservationFailed>;
}

/// Whether a JSON value is there and not `null`, read without keeping it: a
/// credential's value is skipped by the parser, never held.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Present(pub bool);

impl<'de> Deserialize<'de> for Present {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Present(
            Option::<IgnoredAny>::deserialize(deserializer)?.is_some(),
        ))
    }
}

/// The most of a metadata file any adapter reads. Credentials and settings
/// files are a few KiB; a larger one is not read at all.
const MAX_METADATA_BYTES: u64 = 1024 * 1024;

/// `path` parsed as `T`, or `None` where it is absent, oversized or not `T`.
pub(super) fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_METADATA_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Whether an OAuth credential that expires at `expires_at_ms` is still
/// usable, will be refreshed on next use, or is gone.
pub(super) fn oauth_status(
    expires_at_ms: Option<u64>,
    refreshable: bool,
    now: SystemTime,
) -> AuthStatus {
    let now_ms = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64);
    match expires_at_ms {
        Some(expiry) if expiry <= now_ms && refreshable => AuthStatus::RefreshPending,
        Some(expiry) if expiry <= now_ms => AuthStatus::Expired,
        _ => AuthStatus::SignedIn,
    }
}

/// One method and status summarising several sources: the one method they
/// share or `mixed`, and the best status any of them has.
pub(super) fn summarise(parts: &[(AuthMethod, AuthStatus)]) -> (AuthMethod, AuthStatus) {
    let Some((first, _)) = parts.first() else {
        return (AuthMethod::None, AuthStatus::NotSignedIn);
    };
    let method = if parts.iter().all(|(method, _)| method == first) {
        *first
    } else {
        AuthMethod::Mixed
    };
    let status = [
        AuthStatus::SignedIn,
        AuthStatus::RefreshPending,
        AuthStatus::Unknown,
        AuthStatus::Expired,
        AuthStatus::Invalid,
    ]
    .into_iter()
    .find(|wanted| parts.iter().any(|(_, status)| status == wanted))
    .unwrap_or(AuthStatus::NotSignedIn);
    (method, status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_skips_the_value() {
        #[derive(Deserialize)]
        struct Holder {
            #[serde(default)]
            secret: Present,
            #[serde(default)]
            empty: Present,
            #[serde(default)]
            absent: Present,
        }
        let held: Holder =
            serde_json::from_str(r#"{"secret": {"nested": "sk-ant-x"}, "empty": null}"#).unwrap();
        assert_eq!(
            (held.secret, held.empty, held.absent),
            (Present(true), Present(false), Present(false))
        );
    }

    #[test]
    fn expiry_with_a_refresh_credential_is_pending_not_expired() {
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(10_000);
        assert_eq!(oauth_status(Some(20_000), true, now), AuthStatus::SignedIn);
        assert_eq!(
            oauth_status(Some(5_000), true, now),
            AuthStatus::RefreshPending
        );
        assert_eq!(oauth_status(Some(5_000), false, now), AuthStatus::Expired);
        assert_eq!(oauth_status(None, false, now), AuthStatus::SignedIn);
    }

    #[test]
    fn several_sources_summarise_to_one_method_or_mixed() {
        use AuthMethod::{ApiKey, Mixed, Oauth};
        use AuthStatus::{Expired, NotSignedIn, SignedIn, Unknown};
        assert_eq!(summarise(&[]), (AuthMethod::None, NotSignedIn));
        assert_eq!(
            summarise(&[(ApiKey, SignedIn), (ApiKey, Unknown)]),
            (ApiKey, SignedIn)
        );
        assert_eq!(
            summarise(&[(Oauth, Expired), (ApiKey, Unknown)]),
            (Mixed, Unknown)
        );
    }
}
