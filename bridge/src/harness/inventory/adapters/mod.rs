//! Passive authentication adapters, one per credential context.
//!
//! Passive means: no sign-in, no token refresh, no inference, no helper
//! command, and no CLI run at all. None of the three CLIs has a status path
//! that can be shown passive through its startup (#466: Claude Code 2.1.284's
//! `auth status` runs the root pre-action hook, which can refresh OAuth and
//! run policy helpers), so each adapter reads only saved metadata: which
//! credentials a file holds and when they expire, never their values, and
//! which variables are set, by name.
//!
//! A metadata file that is missing says "nothing saved here". One that is
//! there but cannot be read — not a regular file, too large, unreadable,
//! malformed — fails the observation, so what was seen before stands, stale.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer};

use super::environment::DeviceEnvironment;
use super::model::{AuthFacts, AuthMethod, AuthStatus};

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

    /// What is saved for this context now.
    fn observe(
        &self,
        environment: &DeviceEnvironment,
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
/// files are a few KiB; a larger one fails the observation unread.
const MAX_METADATA_BYTES: u64 = 1024 * 1024;

/// The bytes of `path`, `None` where nothing is there, or why it cannot be
/// read. Opened without blocking and without following a FIFO's wait: a
/// non-regular file (a FIFO, a device, a directory) is refused by the
/// descriptor's own type, and the read is bounded.
pub(super) fn read_metadata(path: &Path) -> Result<Option<Vec<u8>>, ObservationFailed> {
    use std::io::Read;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ObservationFailed("a metadata file could not be opened")),
    };
    let metadata = file
        .metadata()
        .map_err(|_| ObservationFailed("a metadata file could not be examined"))?;
    if !metadata.is_file() {
        return Err(ObservationFailed("a metadata file is not a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_METADATA_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ObservationFailed("a metadata file could not be read"))?;
    if bytes.len() as u64 > MAX_METADATA_BYTES {
        return Err(ObservationFailed("a metadata file is too large"));
    }
    Ok(Some(bytes))
}

/// `path` parsed as `T`: `None` where it is missing, a failure where it is
/// there and is not `T`.
pub(super) fn read_json<T: serde::de::DeserializeOwned>(
    path: &Path,
) -> Result<Option<T>, ObservationFailed> {
    let Some(bytes) = read_metadata(path)? else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| ObservationFailed("a metadata file is malformed"))
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
