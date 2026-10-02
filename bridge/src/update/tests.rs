use super::*;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tempfile::TempDir;
use time::OffsetDateTime;
use tokio::sync::Notify;

struct FakeBackend {
    latest: Mutex<Result<Release, String>>,
    installs: Mutex<Vec<String>>,
    stage_busy: Mutex<Option<Arc<AtomicBool>>>,
    stages: Mutex<Vec<String>>,
    stage_gate: Mutex<Option<(Arc<Notify>, Arc<Notify>)>>,
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self {
            latest: Mutex::new(Err("no release fixture".into())),
            installs: Mutex::new(Vec::new()),
            stage_busy: Mutex::new(None),
            stages: Mutex::new(Vec::new()),
            stage_gate: Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl UpdateBackend for FakeBackend {
    async fn latest(&self) -> Result<Release, String> {
        self.latest.lock().unwrap().clone()
    }

    async fn install(&self, release: &Release, _attempt_id: &str) -> Result<(), String> {
        self.installs.lock().unwrap().push(release.version.clone());
        Ok(())
    }

    async fn stage(&self, release: &Release, _attempt_id: &str) -> Result<(), String> {
        self.stages.lock().unwrap().push(release.version.clone());
        if let Some(busy) = self.stage_busy.lock().unwrap().as_ref() {
            busy.store(true, Ordering::SeqCst);
        }
        let gate = self.stage_gate.lock().unwrap().clone();
        if let Some((entered, release)) = gate {
            entered.notify_one();
            release.notified().await;
        }
        Ok(())
    }
}

fn fixture(root: &Path, backend: Arc<FakeBackend>, development_build: bool) -> UpdateService {
    fixture_version(root, backend, development_build, "1.0.0")
}

/// A development build the bridge service runs, so a confirmed install may
/// replace it.
fn replaceable_fixture(root: &Path, backend: Arc<FakeBackend>) -> UpdateService {
    UpdateService::new(
        UpdateConfig {
            replaceable_development_build: true,
            ..config(root, true, "1.0.0")
        },
        backend,
    )
    .unwrap()
}

fn config(root: &Path, development_build: bool, version: &str) -> UpdateConfig {
    UpdateConfig {
        status_path: root.join("tasks/bridge-update.json"),
        result_path: root.join("update/result.json"),
        running_version: version.into(),
        platform: "linux-x86_64".into(),
        development_build,
        replaceable_development_build: false,
        running_from_cargo_target: false,
        check_interval: Duration::from_secs(24 * 60 * 60),
    }
}

fn fixture_version(
    root: &Path,
    backend: Arc<FakeBackend>,
    development_build: bool,
    version: &str,
) -> UpdateService {
    UpdateService::new(config(root, development_build, version), backend).unwrap()
}

fn attempt_id(root: &Path) -> String {
    let bytes = std::fs::read(root.join("tasks/bridge-update.json")).unwrap();
    serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["attempt_id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn at(day: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(day * 86_400).unwrap()
}

fn release(version: &str) -> Release {
    Release {
        version: version.into(),
        tag: format!("v{version}"),
        published_at: None,
    }
}

fn failed_result(root: &Path, id: String) {
    let result_path = root.join("update/result.json");
    std::fs::create_dir_all(result_path.parent().unwrap()).unwrap();
    std::fs::write(
        result_path,
        serde_json::to_vec(&HelperResult {
            attempt_id: id,
            success: false,
            version: "1.1.0".into(),
            error: Some("helper reported failure".into()),
            rollback_pending: false,
        })
        .unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn successful_idle_launcher_keeps_admission_closed_until_terminal_result() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend.clone(), false);
    service.check_at(at(1)).await.unwrap();
    service.install(InstallWhen::Idle, true).await.unwrap();
    service.tick_at(at(1), false).await.unwrap();
    assert!(service.admission().try_enter().is_none());

    drop(service);
    let service = fixture(dir.path(), backend, false);
    assert!(service.admission().try_enter().is_none());
    let gate = service.admission();
    let waiting = gate.enter_when_open();
    tokio::pin!(waiting);
    assert!(futures_util::poll!(waiting.as_mut()).is_pending());

    failed_result(dir.path(), attempt_id(dir.path()));
    service.tick_at(at(1), false).await.unwrap();
    let _lease = tokio::time::timeout(Duration::from_secs(1), waiting)
        .await
        .unwrap();
    assert!(service.admission().try_enter().is_some());
}

#[tokio::test]
async fn admitted_work_delays_final_idle_handoff_until_its_lease_finishes() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend.clone(), false);
    service.check_at(at(1)).await.unwrap();
    service.install(InstallWhen::Idle, true).await.unwrap();

    let lease = service.admission().try_enter().unwrap();
    assert_eq!(
        service.tick_at(at(1), false).await.unwrap().state,
        UpdateState::ScheduledWhenIdle
    );
    assert!(backend.installs.lock().unwrap().is_empty());

    drop(lease);
    assert_eq!(
        service.tick_at(at(1), false).await.unwrap().state,
        UpdateState::Installing
    );
}

struct UncertainLaunchBackend {
    active: Mutex<Option<String>>,
}

#[async_trait::async_trait]
impl UpdateBackend for UncertainLaunchBackend {
    async fn latest(&self) -> Result<Release, String> {
        Ok(release("1.1.0"))
    }

    async fn install(&self, release: &Release, attempt_id: &str) -> Result<(), String> {
        self.launch(release, attempt_id).await
    }

    async fn launch(&self, _release: &Release, attempt_id: &str) -> Result<(), String> {
        *self.active.lock().unwrap() = Some(attempt_id.to_string());
        Err("launcher exited after starting helper".into())
    }

    fn active_attempt(&self) -> Result<Option<String>, String> {
        Ok(self.active.lock().unwrap().clone())
    }
}

#[tokio::test]
async fn uncertain_idle_launcher_keeps_attempt_and_admission_until_recovery() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(UncertainLaunchBackend {
        active: Mutex::new(None),
    });
    let service = UpdateService::new(
        UpdateConfig {
            status_path: dir.path().join("tasks/bridge-update.json"),
            result_path: dir.path().join("update/result.json"),
            running_version: "1.0.0".into(),
            platform: "linux-x86_64".into(),
            development_build: false,
            replaceable_development_build: false,
            running_from_cargo_target: false,
            check_interval: Duration::from_secs(24 * 60 * 60),
        },
        backend.clone(),
    )
    .unwrap();
    service.check_at(at(1)).await.unwrap();
    service.install(InstallWhen::Idle, true).await.unwrap();
    assert!(service.tick_at(at(1), false).await.is_err());
    assert_eq!(service.status().state, UpdateState::Installing);
    assert_eq!(
        backend.active_attempt().unwrap(),
        Some(attempt_id(dir.path()))
    );
    assert!(service.admission().try_enter().is_none());

    failed_result(dir.path(), attempt_id(dir.path()));
    *backend.active.lock().unwrap() = None;
    service.tick_at(at(1), false).await.unwrap();
    assert_eq!(service.status().state, UpdateState::Failed);
    assert!(service.admission().try_enter().is_some());
}

#[tokio::test]
async fn check_persists_available_version_and_broadcasts() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend.clone(), false);
    let mut receiver = service.subscribe();
    let status = service.check_at(at(1)).await.unwrap();
    assert_eq!(status.state, UpdateState::Available);
    assert_eq!(
        status.latest_release.as_ref().map(|r| r.version.as_str()),
        Some("1.1.0")
    );
    assert!(status.can_install);
    receiver.changed().await.unwrap();
    assert_eq!(*receiver.borrow(), status);
    let restarted = fixture(dir.path(), backend, false);
    assert_eq!(restarted.status(), status);
}

#[tokio::test]
async fn scheduled_update_waits_for_agents_and_survives_restart() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend.clone(), false);
    service.check_at(at(1)).await.unwrap();
    let scheduled = service.install(InstallWhen::Idle, true).await.unwrap();
    assert_eq!(scheduled.state, UpdateState::ScheduledWhenIdle);
    assert!(backend.installs.lock().unwrap().is_empty());
    drop(service);
    let restarted = fixture(dir.path(), backend.clone(), false);
    assert_eq!(restarted.status().state, UpdateState::ScheduledWhenIdle);
    restarted.tick_at(at(1), false).await.unwrap();
    assert_eq!(*backend.installs.lock().unwrap(), vec!["1.1.0"]);
    assert_eq!(restarted.status().state, UpdateState::Installing);
}

#[tokio::test]
async fn queued_update_rechecks_agent_activity_after_staging() {
    let dir = TempDir::new().unwrap();
    let busy = Arc::new(AtomicBool::new(false));
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        stage_busy: Mutex::new(Some(busy.clone())),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend.clone(), false);
    let activity = busy.clone();
    assert!(service
        .set_working_agents_probe(Arc::new(move || activity.load(Ordering::SeqCst)))
        .is_ok());
    service.check_at(at(1)).await.unwrap();
    service.install(InstallWhen::Idle, true).await.unwrap();
    assert_eq!(
        service.tick_at(at(1), false).await.unwrap().state,
        UpdateState::ScheduledWhenIdle
    );
    assert_eq!(backend.stages.lock().unwrap().len(), 1);
    assert!(backend.installs.lock().unwrap().is_empty());
    busy.store(false, Ordering::SeqCst);
    drop(service);
    let restarted = fixture(dir.path(), backend.clone(), false);
    assert_eq!(
        restarted.tick_at(at(1), false).await.unwrap().state,
        UpdateState::Installing
    );
    assert_eq!(backend.stages.lock().unwrap().len(), 1);
    assert_eq!(backend.installs.lock().unwrap().as_slice(), &["1.1.0"]);
}

#[tokio::test]
async fn immediate_request_is_durable_before_it_returns() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = Arc::new(fixture(dir.path(), backend, false));
    service.check_at(at(1)).await.unwrap();
    let accepted = service
        .request_install(InstallWhen::Now, true, false)
        .unwrap();
    assert_eq!(accepted.state, UpdateState::Installing);
    assert!(!attempt_id(dir.path()).is_empty());
    assert!(matches!(
        service.request_install(InstallWhen::Now, true, false),
        Err(UpdateError::AlreadyInstalling)
    ));
    assert!(matches!(
        service.request_check(),
        Err(UpdateError::AlreadyInstalling)
    ));
}

#[tokio::test]
async fn queued_request_keeps_single_stage_and_waits_if_agents_start_during_download() {
    let dir = TempDir::new().unwrap();
    let busy = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(Notify::new());
    let release_stage = Arc::new(Notify::new());
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        stage_gate: Mutex::new(Some((entered.clone(), release_stage.clone()))),
        ..Default::default()
    });
    let service = Arc::new(fixture(dir.path(), backend.clone(), false));
    let activity = busy.clone();
    assert!(service
        .set_working_agents_probe(Arc::new(move || activity.load(Ordering::SeqCst)))
        .is_ok());
    service.check_at(at(1)).await.unwrap();
    assert_eq!(
        service
            .request_install(InstallWhen::Idle, false, false)
            .unwrap()
            .state,
        UpdateState::ScheduledWhenIdle
    );
    entered.notified().await;
    busy.store(true, Ordering::SeqCst);
    // A repeated click observes the durable queued state and cannot start a
    // second download while the first remains in progress.
    assert!(matches!(
        service.request_install(InstallWhen::Idle, true, false),
        Err(UpdateError::Busy) | Ok(_)
    ));
    assert_eq!(backend.stages.lock().unwrap().len(), 1);
    release_stage.notify_one();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let bytes = std::fs::read(dir.path().join("tasks/bridge-update.json")).unwrap();
            let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            if saved["staged"] == true {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(service.status().state, UpdateState::ScheduledWhenIdle);
    assert!(backend.installs.lock().unwrap().is_empty());
    busy.store(false, Ordering::SeqCst);
    assert_eq!(
        service.tick_at(at(1), false).await.unwrap().state,
        UpdateState::Installing
    );
    assert_eq!(backend.stages.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn queued_release_change_discards_old_staged_attempt_and_daily_check_continues() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend.clone(), false);
    let busy = Arc::new(AtomicBool::new(true));
    let activity = busy.clone();
    assert!(service
        .set_working_agents_probe(Arc::new(move || activity.load(Ordering::SeqCst)))
        .is_ok());
    service.check_at(at(1)).await.unwrap();
    service.install(InstallWhen::Idle, true).await.unwrap();
    service.tick_at(at(1), true).await.unwrap();
    let first_attempt = attempt_id(dir.path());
    assert_eq!(backend.stages.lock().unwrap().as_slice(), &["1.1.0"]);

    *backend.latest.lock().unwrap() = Ok(release("1.2.0"));
    let scheduled = service.tick_at(at(2), true).await.unwrap();
    assert_eq!(scheduled.state, UpdateState::ScheduledWhenIdle);
    assert_eq!(scheduled.latest_release.as_ref().unwrap().version, "1.2.0");
    assert_ne!(attempt_id(dir.path()), first_attempt);
    assert_eq!(
        backend.stages.lock().unwrap().as_slice(),
        &["1.1.0", "1.2.0"]
    );
    busy.store(false, Ordering::SeqCst);
    service.tick_at(at(2), false).await.unwrap();
    assert_eq!(backend.installs.lock().unwrap().as_slice(), &["1.2.0"]);
}

#[tokio::test]
async fn helper_result_completes_install_and_clears_sticky_failure() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend, false);
    service.check_at(at(1)).await.unwrap();
    service.install(InstallWhen::Now, true).await.unwrap();
    let first_attempt = attempt_id(dir.path());
    let result_path = dir.path().join("update/result.json");
    std::fs::create_dir_all(result_path.parent().unwrap()).unwrap();
    std::fs::write(
        &result_path,
        serde_json::to_vec(&HelperResult {
            attempt_id: first_attempt,
            success: false,
            version: "1.1.0".into(),
            error: Some("health check failed; rolled back".into()),
            rollback_pending: false,
        })
        .unwrap(),
    )
    .unwrap();
    let failed = service.tick_at(at(1), false).await.unwrap();
    assert_eq!(failed.state, UpdateState::Failed);
    assert_eq!(
        failed.last_error.as_deref(),
        Some("health check failed; rolled back")
    );
    service.check_at(at(2)).await.unwrap();
    assert!(service.status().last_error.is_some());
    assert_eq!(service.status().state, UpdateState::Failed);
    service.install(InstallWhen::Now, false).await.unwrap();
    let second_attempt = attempt_id(dir.path());
    std::fs::write(
        &result_path,
        serde_json::to_vec(&HelperResult {
            attempt_id: second_attempt,
            success: true,
            version: "1.1.0".into(),
            error: None,
            rollback_pending: false,
        })
        .unwrap(),
    )
    .unwrap();
    // A success result cannot impersonate a new executable. The old process
    // leaves it for the newly started bridge to consume.
    assert_eq!(
        service.tick_at(at(2), false).await.unwrap().state,
        UpdateState::Installing
    );
    drop(service);
    let backend = Arc::new(FakeBackend::default());
    let restarted = fixture_version(dir.path(), backend, false, "1.1.0");
    let completed = restarted.tick_at(at(2), false).await.unwrap();
    assert_eq!(completed.running_version, "1.1.0");
    assert_eq!(completed.last_error, None);
    assert_eq!(completed.state, UpdateState::Idle);
}

#[tokio::test]
async fn rollback_pending_blocks_new_install_until_recovery_reports_terminal_result() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend, false);
    service.check_at(at(1)).await.unwrap();
    service.install(InstallWhen::Now, false).await.unwrap();
    let attempt = attempt_id(dir.path());
    let result_path = dir.path().join("update/result.json");
    std::fs::create_dir_all(result_path.parent().unwrap()).unwrap();
    std::fs::write(
        &result_path,
        serde_json::to_vec(&HelperResult {
            attempt_id: attempt.clone(),
            success: false,
            version: "1.1.0".into(),
            error: Some("rollback failed".into()),
            rollback_pending: true,
        })
        .unwrap(),
    )
    .unwrap();
    let pending = service.tick_at(at(1), false).await.unwrap();
    assert_eq!(pending.state, UpdateState::Failed);
    assert!(!pending.can_install);
    assert!(result_path.exists());
    assert!(matches!(
        service.install(InstallWhen::Now, false).await,
        Err(UpdateError::Busy)
    ));

    std::fs::write(
        &result_path,
        serde_json::to_vec(&HelperResult {
            attempt_id: attempt,
            success: false,
            version: "1.1.0".into(),
            error: Some("restored previous bridge".into()),
            rollback_pending: false,
        })
        .unwrap(),
    )
    .unwrap();
    let recovered = service.tick_at(at(1), false).await.unwrap();
    assert_eq!(recovered.state, UpdateState::Failed);
    assert!(recovered.can_install);
    assert!(!result_path.exists());
}

#[tokio::test]
async fn wrong_attempt_result_cannot_finish_same_version_retry() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend, false);
    service.check_at(at(1)).await.unwrap();
    service.install(InstallWhen::Now, false).await.unwrap();
    let result_path = dir.path().join("update/result.json");
    std::fs::create_dir_all(result_path.parent().unwrap()).unwrap();
    std::fs::write(
        &result_path,
        serde_json::to_vec(&HelperResult {
            attempt_id: "earlier-attempt".into(),
            success: false,
            version: "1.1.0".into(),
            error: Some("old failure".into()),
            rollback_pending: false,
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        service.tick_at(at(1), false).await.unwrap().state,
        UpdateState::Installing
    );
}

#[tokio::test]
async fn abandoned_install_recovers_after_stabilization_window() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend.clone(), false);
    service.check_at(at(1)).await.unwrap();
    service.install(InstallWhen::Now, false).await.unwrap();
    drop(service);

    // Simulate a machine restart with a helper that never came back. The
    // persisted attempt is older than the helper stabilization grace period.
    let status_path = dir.path().join("tasks/bridge-update.json");
    let mut saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&status_path).unwrap()).unwrap();
    // The service parser expects RFC3339, including the UTC offset.
    saved["started_at"] = serde_json::Value::String("1970-01-02T00:00:00Z".into());
    std::fs::write(&status_path, serde_json::to_vec(&saved).unwrap()).unwrap();
    let restarted = fixture(dir.path(), backend, false);
    let status = restarted.tick_at(at(2), false).await.unwrap();
    assert_eq!(status.state, UpdateState::Failed);
    assert!(status.last_error.unwrap().contains("helper stopped"));
}

#[tokio::test]
async fn invalid_release_is_throttled_and_cleared_by_the_next_check() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("not-a-version"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend.clone(), false);
    assert!(service.tick_at(at(1), false).await.is_err());
    // A failed check is not a failed update.
    assert_eq!(service.status().state, UpdateState::Idle);
    assert!(service.status().last_error.is_some());
    *backend.latest.lock().unwrap() = Ok(release("1.1.0"));
    let throttled = service
        .tick_at(at(1) + time::Duration::hours(23), false)
        .await
        .unwrap();
    assert_eq!(throttled.state, UpdateState::Idle);
    assert!(throttled.last_error.is_some());
    let checked = service.tick_at(at(2), false).await.unwrap();
    assert_eq!(checked.state, UpdateState::Available);
    assert!(checked.update_available);
    assert_eq!(checked.last_error, None);
}

#[tokio::test]
async fn check_error_clears_on_the_next_successful_check_across_restart() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Err("HTTP status client error (404 Not Found)".into())),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend.clone(), false);
    assert!(service.check_at(at(1)).await.is_err());
    let failed = service.status();
    assert_eq!(failed.state, UpdateState::Idle);
    assert_eq!(
        failed.last_error.as_deref(),
        Some("HTTP status client error (404 Not Found)")
    );
    drop(service);

    *backend.latest.lock().unwrap() = Ok(release("1.1.0"));
    let restarted = fixture(dir.path(), backend, false);
    let checked = restarted.check_at(at(2)).await.unwrap();
    assert_eq!(checked.state, UpdateState::Available);
    assert_eq!(checked.last_error, None);
    assert!(checked.can_install);
}

#[tokio::test]
async fn helper_failure_outlives_checks_until_an_install_succeeds() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend.clone(), false);
    service.check_at(at(1)).await.unwrap();
    service.install(InstallWhen::Now, false).await.unwrap();
    failed_result(dir.path(), attempt_id(dir.path()));
    assert_eq!(
        service.tick_at(at(1), false).await.unwrap().state,
        UpdateState::Failed
    );

    // A failed check does not replace the install's cause.
    *backend.latest.lock().unwrap() = Err("network down".into());
    assert!(service.check_at(at(2)).await.is_err());
    assert_eq!(service.status().state, UpdateState::Failed);
    assert_eq!(
        service.status().last_error.as_deref(),
        Some("helper reported failure")
    );

    // Nor does a successful one erase it, before or after a restart.
    *backend.latest.lock().unwrap() = Ok(release("1.1.0"));
    let checked = service.check_at(at(3)).await.unwrap();
    assert_eq!(checked.state, UpdateState::Failed);
    assert_eq!(
        checked.last_error.as_deref(),
        Some("helper reported failure")
    );
    drop(service);
    let restarted = fixture(dir.path(), backend, false);
    let checked = restarted.check_at(at(4)).await.unwrap();
    assert_eq!(checked.state, UpdateState::Failed);
    assert_eq!(
        checked.last_error.as_deref(),
        Some("helper reported failure")
    );
}

#[tokio::test]
async fn development_build_drops_a_stale_check_error_saved_before_error_kinds() {
    let dir = TempDir::new().unwrap();
    let status_path = dir.path().join("tasks/bridge-update.json");
    std::fs::create_dir_all(status_path.parent().unwrap()).unwrap();
    // The shape an older bridge saved: a check error marked the state failed
    // and nothing said what kind of error it was.
    std::fs::write(
        &status_path,
        serde_json::to_vec(&serde_json::json!({
            "running_version": "1.0.0",
            "platform": "linux-x86_64",
            "development_build": true,
            "latest_release": {"version": "1.1.0", "tag": "v1.1.0", "published_at": null},
            "last_checked_at": "1970-01-02T00:00:00Z",
            "state": "failed",
            "last_error": "HTTP status client error (404 Not Found)",
            "update_available": true,
            "can_install": false
        }))
        .unwrap(),
    )
    .unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend, true);
    // A development build never installed, so the saved error was a check's,
    // and it never checks on its own, so the old result goes at startup.
    let loaded = service.status();
    assert_eq!(loaded.state, UpdateState::Idle);
    assert_eq!(loaded.last_error, None);
    assert!(!loaded.update_available);
    let checked = service.check_at(at(2)).await.unwrap();
    assert_eq!(checked.state, UpdateState::Available);
    assert_eq!(checked.last_error, None);
    assert!(!checked.can_install);
}

#[tokio::test]
async fn daily_check_is_due_only_after_interval() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.0.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend.clone(), false);
    service.tick_at(at(1), false).await.unwrap();
    *backend.latest.lock().unwrap() = Ok(release("1.1.0"));
    assert_eq!(
        service
            .tick_at(at(1) + time::Duration::hours(23), false)
            .await
            .unwrap()
            .state,
        UpdateState::Idle
    );
    assert_eq!(
        service.tick_at(at(2), false).await.unwrap().state,
        UpdateState::Available
    );
}

#[tokio::test]
async fn development_build_never_checks_on_its_own() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend.clone(), true);
    // Startup and every later tick: nothing is fetched or recorded.
    let status = service.tick_at(at(1), false).await.unwrap();
    assert_eq!(status.last_checked_at, None);
    assert_eq!(status.latest_release, None);
    let status = service.tick_at(at(30), false).await.unwrap();
    assert_eq!(status.last_checked_at, None);
    assert!(!status.update_available);
}

#[tokio::test]
async fn development_build_checks_when_asked_but_does_not_install() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend.clone(), true);
    let status = service.check_at(at(1)).await.unwrap();
    assert_eq!(status.latest_release, Some(release("1.1.0")));
    assert!(status.update_available);
    assert!(!status.can_install);
    // The service does not run this binary: no confirmation makes it replaceable.
    assert!(!status.can_replace_development_build);
    assert!(service.install(InstallWhen::Now, false).await.is_err());
    assert!(matches!(
        service
            .install_confirmed(InstallWhen::Now, false, true)
            .await,
        Err(UpdateError::DevelopmentBuild)
    ));
    assert!(backend.installs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn replaceable_development_build_installs_only_when_the_replacement_is_confirmed() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = replaceable_fixture(dir.path(), backend.clone());
    let status = service.check_at(at(1)).await.unwrap();
    assert!(!status.can_install);
    assert!(status.can_replace_development_build);
    assert!(matches!(
        service.install(InstallWhen::Now, false).await,
        Err(UpdateError::ReplacementNotConfirmed)
    ));
    assert!(backend.installs.lock().unwrap().is_empty());
    let installing = service
        .install_confirmed(InstallWhen::Now, false, true)
        .await
        .unwrap();
    assert_eq!(installing.state, UpdateState::Installing);
    assert!(!service.status().can_replace_development_build);
    assert_eq!(*backend.installs.lock().unwrap(), ["1.1.0"]);
}

#[tokio::test]
async fn a_confirmed_idle_replacement_runs_without_an_automatic_check() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = replaceable_fixture(dir.path(), backend.clone());
    let checked = service.check_at(at(1)).await.unwrap();
    let queued = service
        .install_confirmed(InstallWhen::Idle, true, true)
        .await
        .unwrap();
    assert_eq!(queued.state, UpdateState::ScheduledWhenIdle);
    assert!(backend.installs.lock().unwrap().is_empty());
    *backend.latest.lock().unwrap() = Ok(release("1.2.0"));
    let launched = service.tick_at(at(5), false).await.unwrap();
    assert_eq!(launched.state, UpdateState::Installing);
    assert_eq!(launched.last_checked_at, checked.last_checked_at);
    assert_eq!(*backend.stages.lock().unwrap(), ["1.1.0"]);
}

#[tokio::test]
async fn development_build_status_never_offers_a_release_install() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend, false);
    assert!(
        !service
            .check_at(at(1))
            .await
            .unwrap()
            .can_replace_development_build
    );
}

/// Save the status shape a bridge from before error kinds wrote, with an
/// error and whatever attempt fields `extra` adds.
fn write_legacy_status(root: &Path, development_build: bool, extra: serde_json::Value) {
    let status_path = root.join("tasks/bridge-update.json");
    std::fs::create_dir_all(status_path.parent().unwrap()).unwrap();
    let mut saved = serde_json::json!({
        "running_version": "1.0.0",
        "platform": "linux-x86_64",
        "development_build": development_build,
        "latest_release": {"version": "1.1.0", "tag": "v1.1.0", "published_at": null},
        "last_checked_at": "1970-01-02T00:00:00Z",
        "state": "failed",
        "last_error": "HTTP status client error (404 Not Found)",
        "update_available": true,
        "can_install": !development_build
    });
    saved
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    std::fs::write(&status_path, serde_json::to_vec(&saved).unwrap()).unwrap();
}

fn saved_status(root: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(root.join("tasks/bridge-update.json")).unwrap()).unwrap()
}

#[tokio::test]
async fn release_build_reads_a_legacy_error_with_no_install_evidence_as_a_check_error() {
    let dir = TempDir::new().unwrap();
    write_legacy_status(dir.path(), false, serde_json::json!({}));
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend, false);
    let loaded = service.status();
    assert_eq!(loaded.state, UpdateState::Available);
    assert!(loaded.can_install);
    let checked = service.check_at(at(2)).await.unwrap();
    assert_eq!(checked.state, UpdateState::Available);
    assert_eq!(checked.last_error, None);
}

#[tokio::test]
async fn release_build_keeps_a_legacy_error_with_a_saved_attempt_as_an_install_error() {
    let dir = TempDir::new().unwrap();
    write_legacy_status(
        dir.path(),
        false,
        serde_json::json!({"rollback_pending": true, "attempt_id": "attempt-1"}),
    );
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend, false);
    assert_eq!(service.status().state, UpdateState::Failed);
    assert_eq!(
        service.status().last_error.as_deref(),
        Some("HTTP status client error (404 Not Found)")
    );
}

#[tokio::test]
async fn release_build_keeps_a_legacy_error_with_a_helper_result_as_an_install_error() {
    let dir = TempDir::new().unwrap();
    write_legacy_status(dir.path(), false, serde_json::json!({}));
    failed_result(dir.path(), "attempt-1".into());
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend, false);
    assert_eq!(service.status().state, UpdateState::Failed);
    let checked = service.check_at(at(2)).await.unwrap();
    assert_eq!(checked.state, UpdateState::Failed);
    assert_eq!(
        checked.last_error.as_deref(),
        Some("HTTP status client error (404 Not Found)")
    );
}

#[tokio::test]
async fn development_build_forgets_an_earlier_run_s_check_at_startup() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = replaceable_fixture(dir.path(), backend.clone());
    let checked = service.check_at(at(1)).await.unwrap();
    assert!(checked.update_available);
    assert!(checked.can_replace_development_build);
    drop(service);

    // Nothing checks again on its own, so the old result is not shown as
    // news and an older app has nothing to badge.
    let restarted = replaceable_fixture(dir.path(), backend.clone());
    let status = restarted.status();
    assert_eq!(status.latest_release, None);
    assert_eq!(status.last_checked_at, None);
    assert!(!status.update_available);
    assert!(!status.can_replace_development_build);
    assert_eq!(status.state, UpdateState::Idle);

    *backend.latest.lock().unwrap() = Err("network down".into());
    assert!(restarted.check_at(at(2)).await.is_err());
    assert!(restarted.status().last_error.is_some());
    drop(restarted);
    let restarted = replaceable_fixture(dir.path(), backend);
    assert_eq!(restarted.status().last_error, None);
}

#[tokio::test]
async fn a_release_build_s_schedule_does_not_replace_a_development_build_after_restart() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend.clone(), false);
    service.check_at(at(1)).await.unwrap();
    let queued = service.install(InstallWhen::Idle, true).await.unwrap();
    assert_eq!(queued.state, UpdateState::ScheduledWhenIdle);
    drop(service);

    // A source build copied over the binary: the service runs a development
    // build nobody agreed to replace.
    let restarted = replaceable_fixture(dir.path(), backend.clone());
    assert_ne!(restarted.status().state, UpdateState::ScheduledWhenIdle);
    let ticked = restarted.tick_at(at(1), false).await.unwrap();
    assert_ne!(ticked.state, UpdateState::Installing);
    assert_eq!(ticked.last_error, None);
    assert!(backend.stages.lock().unwrap().is_empty());
    assert!(backend.installs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_development_build_that_cannot_be_replaced_drops_a_saved_schedule() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend.clone(), false);
    service.check_at(at(1)).await.unwrap();
    service.install(InstallWhen::Idle, true).await.unwrap();
    drop(service);

    let restarted = fixture(dir.path(), backend.clone(), true);
    assert_eq!(restarted.status().state, UpdateState::Idle);
    let ticked = restarted.tick_at(at(1), false).await.unwrap();
    assert_eq!(ticked.state, UpdateState::Idle);
    assert_eq!(ticked.last_error, None);
    assert!(backend.stages.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_confirmed_replacement_schedule_is_saved_and_survives_restart() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = replaceable_fixture(dir.path(), backend.clone());
    service.check_at(at(1)).await.unwrap();
    service
        .install_confirmed(InstallWhen::Idle, true, true)
        .await
        .unwrap();
    assert_eq!(saved_status(dir.path())["replaces_development_build"], true);
    drop(service);

    let restarted = replaceable_fixture(dir.path(), backend.clone());
    assert_eq!(restarted.status().state, UpdateState::ScheduledWhenIdle);
    let launched = restarted.tick_at(at(2), false).await.unwrap();
    assert_eq!(launched.state, UpdateState::Installing);
    assert_eq!(*backend.stages.lock().unwrap(), ["1.1.0"]);
}

#[tokio::test]
async fn a_newer_release_needs_its_own_confirmation_to_replace_a_development_build() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = replaceable_fixture(dir.path(), backend.clone());
    service.check_at(at(1)).await.unwrap();
    service
        .install_confirmed(InstallWhen::Idle, true, true)
        .await
        .unwrap();
    // The confirmation named 1.1.0.
    *backend.latest.lock().unwrap() = Ok(release("1.2.0"));
    let checked = service.check_at(at(2)).await.unwrap();
    assert_eq!(checked.state, UpdateState::Available);
    let ticked = service.tick_at(at(3), false).await.unwrap();
    assert_eq!(ticked.state, UpdateState::Available);
    assert!(backend.stages.lock().unwrap().is_empty());
}

#[tokio::test]
async fn status_says_when_a_development_build_runs_from_a_cargo_target_directory() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend::default());
    let development = UpdateService::new(
        UpdateConfig {
            running_from_cargo_target: true,
            ..config(dir.path(), true, "1.0.0")
        },
        backend.clone(),
    )
    .unwrap();
    assert!(development.status().running_from_cargo_target);
    let other = TempDir::new().unwrap();
    let release_build = UpdateService::new(
        UpdateConfig {
            running_from_cargo_target: true,
            ..config(other.path(), false, "1.0.0")
        },
        backend,
    )
    .unwrap();
    assert!(!release_build.status().running_from_cargo_target);
}
