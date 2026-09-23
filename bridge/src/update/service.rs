use super::{HelperResult, InstallWhen, Release, UpdateState, UpdateStatus};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use tokio::sync::{watch, Mutex};

#[async_trait]
pub trait UpdateBackend: Send + Sync {
    /// Fetch release metadata. The install backend verifies the downloaded
    /// asset and signature before it launches the helper.
    async fn latest(&self) -> Result<Release, String>;

    /// Stage and launch the independent helper; returns once the helper owns
    /// the update. It does not wait for bridge termination or installation.
    async fn install(&self, release: &Release, attempt_id: &str) -> Result<(), String>;

    async fn stage(&self, _release: &Release, _attempt_id: &str) -> Result<(), String> {
        Ok(())
    }

    async fn launch(&self, release: &Release, attempt_id: &str) -> Result<(), String> {
        self.install(release, attempt_id).await
    }

    /// Report a live helper attempt, not just a stale marker left on disk.
    fn active_attempt(&self) -> Result<Option<String>, String> {
        Ok(None)
    }
}

const HELPER_STABILIZATION: Duration = Duration::from_secs(10 * 60);
pub type BusyProbe = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct AttemptMetadata {
    #[serde(default)]
    attempt_id: Option<String>,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    staged: bool,
    #[serde(default)]
    target_version: Option<String>,
    #[serde(default)]
    rollback_pending: bool,
}

#[derive(Serialize, Deserialize)]
struct PersistedUpdate {
    #[serde(flatten)]
    status: UpdateStatus,
    #[serde(flatten)]
    attempt: AttemptMetadata,
}

#[derive(Debug, Clone)]
pub struct UpdateConfig {
    pub status_path: PathBuf,
    pub result_path: PathBuf,
    pub running_version: String,
    pub platform: String,
    pub development_build: bool,
    pub check_interval: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("updates are unavailable in a development build")]
    DevelopmentBuild,
    #[error("no newer release is available")]
    NoUpdateAvailable,
    #[error("an update is already installing")]
    AlreadyInstalling,
    #[error("an update check is already running")]
    AlreadyChecking,
    #[error("update state is busy")]
    Busy,
    #[error("update backend: {0}")]
    Backend(String),
    #[error("update state at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid update state at {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid release version: {0}")]
    InvalidVersion(String),
}

/// Serializes update transitions, persists each one before publishing it, and
/// watches for the helper's final outcome. The process that owns this service
/// can be killed after `install`: its state is restored from disk on boot.
pub struct UpdateService {
    config: UpdateConfig,
    backend: Arc<dyn UpdateBackend>,
    transition: Mutex<AttemptMetadata>,
    check_in_flight: AtomicBool,
    staging: AtomicBool,
    working_agents_probe: OnceLock<BusyProbe>,
    status_tx: watch::Sender<UpdateStatus>,
}

impl UpdateService {
    pub fn new(config: UpdateConfig, backend: Arc<dyn UpdateBackend>) -> Result<Self, UpdateError> {
        let (mut status, attempt) = match std::fs::read(&config.status_path) {
            Ok(bytes) => serde_json::from_slice::<PersistedUpdate>(&bytes)
                .map_err(|source| UpdateError::Json {
                    path: config.status_path.clone(),
                    source,
                })
                .map(|persisted| (persisted.status, persisted.attempt))?,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => (
                UpdateStatus::new(
                    config.running_version.clone(),
                    config.platform.clone(),
                    config.development_build,
                ),
                AttemptMetadata::default(),
            ),
            Err(source) => {
                return Err(UpdateError::Io {
                    path: config.status_path.clone(),
                    source,
                })
            }
        };
        // The executable is the source of truth after a restart or rollback.
        status.running_version = config.running_version.clone();
        status.platform = config.platform.clone();
        status.development_build = config.development_build;
        status.refresh_computed();
        status.can_install &= !attempt.rollback_pending
            && backend
                .active_attempt()
                .map(|active| active.is_none())
                .unwrap_or(false);
        let (status_tx, _) = watch::channel(status);
        Ok(Self {
            config,
            backend,
            transition: Mutex::new(attempt),
            check_in_flight: AtomicBool::new(false),
            staging: AtomicBool::new(false),
            working_agents_probe: OnceLock::new(),
            status_tx,
        })
    }

    pub fn status(&self) -> UpdateStatus {
        self.status_tx.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<UpdateStatus> {
        self.status_tx.subscribe()
    }

    /// Install a live activity probe once the app state is available. A probe
    /// that cannot read activity should conservatively return true.
    pub fn set_working_agents_probe(&self, probe: BusyProbe) -> Result<(), BusyProbe> {
        self.working_agents_probe.set(probe)
    }

    pub async fn check(&self) -> Result<UpdateStatus, UpdateError> {
        self.check_at(OffsetDateTime::now_utc()).await
    }

    /// Accept one manual check without making the request wait on the network.
    /// The watch subscription publishes its result when it completes.
    pub fn request_check(self: &Arc<Self>) -> Result<UpdateStatus, UpdateError> {
        if self.check_in_flight.swap(true, Ordering::AcqRel) {
            return Err(UpdateError::AlreadyChecking);
        }
        let guard = match self.transition.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                self.check_in_flight.store(false, Ordering::Release);
                return Err(UpdateError::Busy);
            }
        };
        let status = self.status();
        if status.state == UpdateState::Installing {
            self.check_in_flight.store(false, Ordering::Release);
            return Err(UpdateError::AlreadyInstalling);
        }
        if guard.rollback_pending {
            self.check_in_flight.store(false, Ordering::Release);
            return Err(UpdateError::Busy);
        }
        match self.backend.active_attempt().map_err(UpdateError::Backend) {
            Ok(None) => {}
            Ok(Some(_)) => {
                self.check_in_flight.store(false, Ordering::Release);
                return Err(UpdateError::Busy);
            }
            Err(error) => {
                self.check_in_flight.store(false, Ordering::Release);
                return Err(error);
            }
        }
        drop(guard);
        let service = Arc::clone(self);
        tokio::spawn(async move {
            let _ = service.check().await;
            service.check_in_flight.store(false, Ordering::Release);
        });
        Ok(status)
    }

    pub async fn check_at(&self, now: OffsetDateTime) -> Result<UpdateStatus, UpdateError> {
        let mut guard = self.transition.lock().await;
        self.check_locked(now, &mut guard).await
    }

    async fn check_locked(
        &self,
        now: OffsetDateTime,
        attempt: &mut AttemptMetadata,
    ) -> Result<UpdateStatus, UpdateError> {
        let mut status = self.status();
        // An in-flight helper owns the version transition; a network refresh
        // must not replace its saved target.
        if status.state == UpdateState::Installing {
            return Err(UpdateError::AlreadyInstalling);
        }
        if attempt.rollback_pending
            || self
                .backend
                .active_attempt()
                .map_err(UpdateError::Backend)?
                .is_some()
        {
            return Err(UpdateError::Busy);
        }
        status.last_checked_at = Some(now.format(&Rfc3339).expect("valid UTC timestamp"));
        match self.backend.latest().await {
            Ok(release) => {
                if let Err(error) = validate_release(&release) {
                    if status.last_error.is_none() {
                        status.last_error = Some(error.to_string());
                    }
                    if status.state != UpdateState::ScheduledWhenIdle {
                        status.state = UpdateState::Failed;
                    }
                    self.publish(status, attempt)?;
                    return Err(error);
                }
                if status.state == UpdateState::ScheduledWhenIdle
                    && status
                        .latest_release
                        .as_ref()
                        .map(|old| old.version.as_str())
                        != Some(release.version.as_str())
                {
                    // A staged binary belongs to one release. A newer result
                    // invalidates the old queued attempt before handoff.
                    *attempt = AttemptMetadata::default();
                }
                status.latest_release = Some(release);
                if status.state != UpdateState::ScheduledWhenIdle {
                    status.state = if status.last_error.is_some() {
                        UpdateState::Failed
                    } else if release_available(&status) {
                        UpdateState::Available
                    } else {
                        UpdateState::Idle
                    };
                } else if !release_available(&status) {
                    status.state = UpdateState::Idle;
                    *attempt = AttemptMetadata::default();
                }
                // A prior helper failure is sticky until a successful helper
                // result; a successful metadata check does not erase it.
                self.publish(status, attempt)
            }
            Err(error) => {
                // Preserve a prior helper rollback cause; a transient network
                // failure must not replace the actionable install error.
                if status.last_error.is_none() {
                    status.last_error = Some(error.clone());
                }
                if status.state != UpdateState::ScheduledWhenIdle {
                    status.state = UpdateState::Failed;
                }
                self.publish(status, attempt)?;
                Err(UpdateError::Backend(error))
            }
        }
    }

    pub async fn install(
        &self,
        when: InstallWhen,
        working_agents: bool,
    ) -> Result<UpdateStatus, UpdateError> {
        let mut guard = self.transition.lock().await;
        if self.config.development_build {
            return Err(UpdateError::DevelopmentBuild);
        }
        if self.check_in_flight.load(Ordering::Acquire) {
            return Err(UpdateError::Busy);
        }
        let mut status = self.status();
        if status.state == UpdateState::Installing {
            return Err(UpdateError::AlreadyInstalling);
        }
        if guard.rollback_pending
            || self
                .backend
                .active_attempt()
                .map_err(UpdateError::Backend)?
                .is_some()
        {
            return Err(UpdateError::Busy);
        }
        if !release_available(&status) {
            return Err(UpdateError::NoUpdateAvailable);
        }
        if when == InstallWhen::Idle {
            status.state = UpdateState::ScheduledWhenIdle;
            let status = self.publish(status, &guard)?;
            if working_agents {
                return Ok(status);
            }
            return self
                .stage_scheduled_locked(status, &mut guard, working_agents)
                .await;
        }
        self.launch_locked(status, &mut guard).await
    }

    /// Accept an install request as soon as its state is durable. Staging and
    /// signature verification continue in a task while the bridge remains up.
    pub fn request_install(
        self: &Arc<Self>,
        when: InstallWhen,
        working_agents: bool,
    ) -> Result<UpdateStatus, UpdateError> {
        let mut guard = self.transition.try_lock().map_err(|_| UpdateError::Busy)?;
        if self.config.development_build {
            return Err(UpdateError::DevelopmentBuild);
        }
        if self.check_in_flight.load(Ordering::Acquire) {
            return Err(UpdateError::Busy);
        }
        let mut status = self.status();
        if status.state == UpdateState::Installing {
            return Err(UpdateError::AlreadyInstalling);
        }
        if guard.rollback_pending
            || self
                .backend
                .active_attempt()
                .map_err(UpdateError::Backend)?
                .is_some()
        {
            return Err(UpdateError::Busy);
        }
        if !release_available(&status) {
            return Err(UpdateError::NoUpdateAvailable);
        }
        if when == InstallWhen::Idle {
            if status.state == UpdateState::ScheduledWhenIdle {
                return Ok(status);
            }
            status.state = UpdateState::ScheduledWhenIdle;
            let status = self.publish(status, &guard)?;
            drop(guard);
            let service = Arc::clone(self);
            tokio::spawn(async move {
                let _ = service.tick(working_agents).await;
            });
            return Ok(status);
        }
        let (status, release, attempt_id) = self.prepare_launch_locked(status, &mut guard)?;
        self.staging.store(true, Ordering::Release);
        drop(guard);
        let service = Arc::clone(self);
        tokio::spawn(async move {
            let outcome = service.backend.install(&release, &attempt_id).await;
            if let Err(error) = outcome {
                let mut guard = service.transition.lock().await;
                if guard.attempt_id.as_deref() == Some(attempt_id.as_str()) {
                    let mut status = service.status();
                    status.state = UpdateState::Failed;
                    status.last_error = Some(error);
                    *guard = AttemptMetadata::default();
                    let _ = service.publish(status, &guard);
                }
            }
            service.staging.store(false, Ordering::Release);
        });
        Ok(status)
    }

    async fn launch_locked(
        &self,
        status: UpdateStatus,
        attempt: &mut AttemptMetadata,
    ) -> Result<UpdateStatus, UpdateError> {
        let (mut status, release, attempt_id) = self.prepare_launch_locked(status, attempt)?;
        if let Err(error) = self.backend.install(&release, &attempt_id).await {
            status.state = UpdateState::Failed;
            status.last_error = Some(error.clone());
            *attempt = AttemptMetadata::default();
            self.publish(status, attempt)?;
            return Err(UpdateError::Backend(error));
        }
        Ok(status)
    }

    fn prepare_launch_locked(
        &self,
        mut status: UpdateStatus,
        attempt: &mut AttemptMetadata,
    ) -> Result<(UpdateStatus, Release, String), UpdateError> {
        let release = status
            .latest_release
            .clone()
            .ok_or(UpdateError::NoUpdateAvailable)?;
        // A leftover outcome from an earlier attempt must never complete the
        // new attempt. A failure here keeps the current bridge running.
        match std::fs::remove_file(&self.config.result_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(UpdateError::Io {
                    path: self.config.result_path.clone(),
                    source,
                })
            }
        }
        let attempt_id = uuid::Uuid::new_v4().to_string();
        *attempt = AttemptMetadata {
            attempt_id: Some(attempt_id.clone()),
            started_at: Some(
                OffsetDateTime::now_utc()
                    .format(&Rfc3339)
                    .expect("valid UTC timestamp"),
            ),
            staged: false,
            target_version: Some(release.version.clone()),
            rollback_pending: false,
        };
        status.state = UpdateState::Installing;
        self.publish(status.clone(), attempt)?;
        Ok((status, release, attempt_id))
    }

    async fn stage_scheduled_locked(
        &self,
        mut status: UpdateStatus,
        attempt: &mut AttemptMetadata,
        working_agents: bool,
    ) -> Result<UpdateStatus, UpdateError> {
        let release = status
            .latest_release
            .clone()
            .ok_or(UpdateError::NoUpdateAvailable)?;
        if attempt.attempt_id.is_some()
            && attempt.target_version.as_deref() != Some(release.version.as_str())
        {
            *attempt = AttemptMetadata::default();
        }
        let attempt_id = match &attempt.attempt_id {
            Some(id) => id.clone(),
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                attempt.attempt_id = Some(id.clone());
                attempt.target_version = Some(release.version.clone());
                self.publish(status.clone(), attempt)?;
                id
            }
        };
        if !attempt.staged {
            self.staging.store(true, Ordering::Release);
            let result = self.backend.stage(&release, &attempt_id).await;
            self.staging.store(false, Ordering::Release);
            if let Err(error) = result {
                status.state = UpdateState::Failed;
                status.last_error = Some(error.clone());
                *attempt = AttemptMetadata::default();
                self.publish(status, attempt)?;
                return Err(UpdateError::Backend(error));
            }
            attempt.staged = true;
            self.publish(status.clone(), attempt)?;
        }
        // Agent work can start during the download. Check a fresh snapshot at
        // the final handoff rather than trusting the earlier tick argument.
        let busy = self
            .working_agents_probe
            .get()
            .map_or(working_agents, |probe| probe());
        if busy {
            return Ok(status);
        }
        self.launch_staged_locked(status, release, attempt).await
    }

    async fn launch_staged_locked(
        &self,
        mut status: UpdateStatus,
        release: Release,
        attempt: &mut AttemptMetadata,
    ) -> Result<UpdateStatus, UpdateError> {
        let attempt_id = attempt
            .attempt_id
            .clone()
            .ok_or(UpdateError::NoUpdateAvailable)?;
        match std::fs::remove_file(&self.config.result_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(UpdateError::Io {
                    path: self.config.result_path.clone(),
                    source,
                })
            }
        }
        attempt.started_at = Some(
            OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .expect("valid UTC timestamp"),
        );
        status.state = UpdateState::Installing;
        self.publish(status.clone(), attempt)?;
        if let Err(error) = self.backend.launch(&release, &attempt_id).await {
            status.state = UpdateState::Failed;
            status.last_error = Some(error.clone());
            *attempt = AttemptMetadata::default();
            self.publish(status, attempt)?;
            return Err(UpdateError::Backend(error));
        }
        Ok(status)
    }

    /// Reconcile the independent helper, start an idle-scheduled update, and
    /// check for releases after the configured interval. Call this at startup
    /// and periodically from the bridge's runtime task.
    pub async fn tick(&self, working_agents: bool) -> Result<UpdateStatus, UpdateError> {
        self.tick_at(OffsetDateTime::now_utc(), working_agents)
            .await
    }

    pub async fn tick_at(
        &self,
        now: OffsetDateTime,
        working_agents: bool,
    ) -> Result<UpdateStatus, UpdateError> {
        let mut guard = self.transition.lock().await;
        let prior = self.status();
        let was_pending = guard.rollback_pending;
        self.reconcile_helper_locked(now, &mut guard)?;
        let mut status = self.status();
        let mut computed = status.clone();
        computed.refresh_computed();
        computed.can_install &= !guard.rollback_pending
            && self
                .backend
                .active_attempt()
                .map(|active| active.is_none())
                .unwrap_or(false);
        if computed.can_install != status.can_install {
            status = self.publish(status, &guard)?;
        }
        if (prior.state == UpdateState::Installing && status.state != UpdateState::Installing)
            || (was_pending && !guard.rollback_pending)
        {
            return Ok(status);
        }
        if guard.rollback_pending {
            return Ok(status);
        }
        if status.state == UpdateState::Installing {
            return Ok(status);
        }
        if status.state == UpdateState::ScheduledWhenIdle {
            if self.check_in_flight.load(Ordering::Acquire) {
                return Ok(status);
            }
            if check_due(&status, now, self.config.check_interval) {
                self.check_locked(now, &mut guard).await?;
                let status = self.status();
                if status.state != UpdateState::ScheduledWhenIdle {
                    return Ok(status);
                }
            }
            let status = self.status();
            if !release_available(&status) {
                let mut status = status;
                status.state = UpdateState::Idle;
                return self.publish(status, &guard);
            }
            return self
                .stage_scheduled_locked(status, &mut guard, working_agents)
                .await;
        }
        if self.check_in_flight.load(Ordering::Acquire) {
            return Ok(status);
        }
        if check_due(&status, now, self.config.check_interval) {
            return self.check_locked(now, &mut guard).await;
        }
        Ok(status)
    }

    fn reconcile_helper_locked(
        &self,
        now: OffsetDateTime,
        attempt: &mut AttemptMetadata,
    ) -> Result<(), UpdateError> {
        let mut status = self.status();
        if status.state != UpdateState::Installing && !attempt.rollback_pending {
            return Ok(());
        }
        let bytes = match std::fs::read(&self.config.result_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return self.recover_missing_helper(now, status, attempt);
            }
            Err(source) => {
                return Err(UpdateError::Io {
                    path: self.config.result_path.clone(),
                    source,
                })
            }
        };
        let result: HelperResult =
            serde_json::from_slice(&bytes).map_err(|source| UpdateError::Json {
                path: self.config.result_path.clone(),
                source,
            })?;
        // A helper from an older attempt may still finish after a new attempt
        // is requested. It cannot settle a different target version.
        if attempt.attempt_id.as_deref() != Some(result.attempt_id.as_str())
            || status.latest_release.as_ref().map(|r| r.version.as_str())
                != Some(result.version.as_str())
        {
            return Ok(());
        }
        if result.rollback_pending {
            status.state = UpdateState::Failed;
            status.last_error = Some(
                result
                    .error
                    .unwrap_or_else(|| "rollback failed; recovery pending".into()),
            );
            if attempt.rollback_pending && status == self.status() {
                return Ok(());
            }
            attempt.rollback_pending = true;
            self.publish(status, attempt)?;
            return Ok(());
        }
        if result.success && self.config.running_version == result.version {
            status.last_error = None;
            status.state = UpdateState::Idle;
        } else if result.success {
            // The result claims success, but this executable is still the old
            // version. Keep waiting for the new bridge to start and consume it.
            return self.recover_missing_helper(now, status, attempt);
        } else {
            status.last_error = Some(
                result
                    .error
                    .unwrap_or_else(|| "update failed; previous bridge restored".into()),
            );
            status.state = UpdateState::Failed;
        }
        *attempt = AttemptMetadata::default();
        self.publish(status, attempt)?;
        std::fs::remove_file(&self.config.result_path).map_err(|source| UpdateError::Io {
            path: self.config.result_path.clone(),
            source,
        })?;
        Ok(())
    }

    fn recover_missing_helper(
        &self,
        now: OffsetDateTime,
        mut status: UpdateStatus,
        attempt: &mut AttemptMetadata,
    ) -> Result<(), UpdateError> {
        if attempt.rollback_pending {
            return Ok(());
        }
        let elapsed = attempt
            .started_at
            .as_deref()
            .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok())
            .is_some_and(|started| {
                now - started
                    >= time::Duration::try_from(HELPER_STABILIZATION).expect("bounded duration")
            });
        if !elapsed {
            return Ok(());
        }
        if self.staging.load(Ordering::Acquire) {
            return Ok(());
        }
        if self
            .backend
            .active_attempt()
            .map_err(UpdateError::Backend)?
            .as_deref()
            == attempt.attempt_id.as_deref()
        {
            return Ok(());
        }
        status.state = UpdateState::Failed;
        status.last_error = Some("update helper stopped before reporting a result".into());
        *attempt = AttemptMetadata::default();
        self.publish(status, attempt)?;
        Ok(())
    }

    fn publish(
        &self,
        mut status: UpdateStatus,
        attempt: &AttemptMetadata,
    ) -> Result<UpdateStatus, UpdateError> {
        status.refresh_computed();
        status.can_install &= !attempt.rollback_pending
            && self
                .backend
                .active_attempt()
                .map(|active| active.is_none())
                .unwrap_or(false);
        save_status(&self.config.status_path, &status, attempt)?;
        self.status_tx.send_replace(status.clone());
        Ok(status)
    }
}

fn release_available(status: &UpdateStatus) -> bool {
    status.latest_release.as_ref().is_some_and(|release| {
        match (
            semver::Version::parse(&release.version),
            semver::Version::parse(&status.running_version),
        ) {
            (Ok(latest), Ok(running)) => latest > running,
            _ => false,
        }
    })
}

fn validate_release(release: &Release) -> Result<(), UpdateError> {
    semver::Version::parse(&release.version)
        .map_err(|_| UpdateError::InvalidVersion(release.version.clone()))?;
    if release.tag.is_empty() {
        return Err(UpdateError::InvalidVersion("empty release tag".into()));
    }
    Ok(())
}

fn check_due(status: &UpdateStatus, now: OffsetDateTime, interval: Duration) -> bool {
    let Some(last) = status
        .last_checked_at
        .as_deref()
        .and_then(|value| OffsetDateTime::parse(value, &Rfc3339).ok())
    else {
        return true;
    };
    now - last >= time::Duration::try_from(interval).unwrap_or(time::Duration::MAX)
}

fn save_status(
    path: &Path,
    status: &UpdateStatus,
    attempt: &AttemptMetadata,
) -> Result<(), UpdateError> {
    let parent = path.parent().ok_or_else(|| UpdateError::Io {
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "status path has no parent",
        ),
    })?;
    std::fs::create_dir_all(parent).map_err(|source| UpdateError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let json = serde_json::to_vec_pretty(&PersistedUpdate {
        status: status.clone(),
        attempt: attempt.clone(),
    })
    .map_err(|source| UpdateError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&json)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        std::fs::File::open(parent)?.sync_all()
    };
    if let Err(source) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(UpdateError::Io {
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(())
}
