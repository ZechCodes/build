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

fn fixture_version(
    root: &Path,
    backend: Arc<FakeBackend>,
    development_build: bool,
    version: &str,
) -> UpdateService {
    UpdateService::new(
        UpdateConfig {
            status_path: root.join("tasks/bridge-update.json"),
            result_path: root.join("update/result.json"),
            running_version: version.into(),
            platform: "linux-x86_64".into(),
            development_build,
            check_interval: Duration::from_secs(24 * 60 * 60),
        },
        backend,
    )
    .unwrap()
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
    let accepted = service.request_install(InstallWhen::Now, true).unwrap();
    assert_eq!(accepted.state, UpdateState::Installing);
    assert!(!attempt_id(dir.path()).is_empty());
    assert!(matches!(
        service.request_install(InstallWhen::Now, true),
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
            .request_install(InstallWhen::Idle, false)
            .unwrap()
            .state,
        UpdateState::ScheduledWhenIdle
    );
    entered.notified().await;
    busy.store(true, Ordering::SeqCst);
    // A repeated click observes the durable queued state and cannot start a
    // second download while the first remains in progress.
    assert!(matches!(
        service.request_install(InstallWhen::Idle, true),
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
async fn invalid_release_is_throttled_and_keeps_previous_error() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("not-a-version"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend.clone(), false);
    assert!(service.tick_at(at(1), false).await.is_err());
    assert_eq!(service.status().state, UpdateState::Failed);
    *backend.latest.lock().unwrap() = Ok(release("1.1.0"));
    assert_eq!(
        service
            .tick_at(at(1) + time::Duration::hours(23), false)
            .await
            .unwrap()
            .state,
        UpdateState::Failed
    );
    let checked = service.tick_at(at(2), false).await.unwrap();
    assert_eq!(checked.state, UpdateState::Failed);
    assert!(checked.update_available);
    assert!(service.status().last_error.is_some());
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
async fn development_build_checks_but_does_not_install() {
    let dir = TempDir::new().unwrap();
    let backend = Arc::new(FakeBackend {
        latest: Mutex::new(Ok(release("1.1.0"))),
        ..Default::default()
    });
    let service = fixture(dir.path(), backend.clone(), true);
    let status = service.tick_at(at(1), false).await.unwrap();
    assert_eq!(status.latest_release, Some(release("1.1.0")));
    assert!(status.update_available);
    assert!(!status.can_install);
    assert!(service.check_at(at(1)).await.is_ok());
    assert!(service.install(InstallWhen::Now, false).await.is_err());
    assert!(backend.installs.lock().unwrap().is_empty());
}
