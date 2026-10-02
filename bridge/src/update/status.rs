use serde::{Deserialize, Serialize};

/// Release metadata kept with a pending update so an idle install can resume
/// after the bridge restarts without relying on a fresh network request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub tag: String,
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateState {
    Idle,
    Available,
    ScheduledWhenIdle,
    Installing,
    Failed,
}

/// What `last_error` came from. A check error lasts until the next check
/// succeeds; an install error lasts until an install succeeds, and only it
/// makes the state `Failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    Check,
    Install,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallWhen {
    Now,
    Idle,
}

/// The stable status wire shape. This entire value is also flattened into the
/// `bridge.update_status` event by the API adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateStatus {
    pub running_version: String,
    pub platform: String,
    pub development_build: bool,
    pub latest_release: Option<Release>,
    pub last_checked_at: Option<String>,
    pub state: UpdateState,
    pub last_error: Option<String>,
    pub update_available: bool,
    pub can_install: bool,
    /// Saved beside the status (`PersistedUpdate`), not sent: on the wire a
    /// `Failed` state is what marks `last_error` as an install's.
    #[serde(skip)]
    pub(super) last_error_kind: Option<ErrorKind>,
}

impl UpdateStatus {
    pub(super) fn new(running_version: String, platform: String, development_build: bool) -> Self {
        Self {
            running_version,
            platform,
            development_build,
            latest_release: None,
            last_checked_at: None,
            state: UpdateState::Idle,
            last_error: None,
            update_available: false,
            can_install: false,
            last_error_kind: None,
        }
    }

    /// Record a failed check unless an install error is already showing.
    pub(super) fn set_check_error(&mut self, error: String) {
        if self.last_error_kind != Some(ErrorKind::Install) || self.last_error.is_none() {
            self.last_error = Some(error);
            self.last_error_kind = Some(ErrorKind::Check);
        }
    }

    /// Record a failed install; it replaces whatever error was showing.
    pub(super) fn set_install_error(&mut self, error: String) {
        self.last_error = Some(error);
        self.last_error_kind = Some(ErrorKind::Install);
    }

    pub(super) fn clear_error(&mut self) {
        self.last_error = None;
        self.last_error_kind = None;
    }

    pub(super) fn has_install_error(&self) -> bool {
        self.last_error.is_some() && self.last_error_kind == Some(ErrorKind::Install)
    }

    pub(super) fn refresh_computed(&mut self) {
        self.update_available = self.latest_release.as_ref().is_some_and(|release| {
            match (
                semver::Version::parse(&release.version),
                semver::Version::parse(&self.running_version),
            ) {
                (Ok(latest), Ok(running)) => latest > running,
                _ => false,
            }
        });
        self.can_install = !self.development_build
            && self.update_available
            && self.state != UpdateState::Installing;
    }
}

/// The independent updater helper writes this result atomically after it has
/// restarted and health-checked the bridge (or restored the prior binary).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperResult {
    pub attempt_id: String,
    pub success: bool,
    pub version: String,
    pub error: Option<String>,
    #[serde(default)]
    pub rollback_pending: bool,
}
