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
        }
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
