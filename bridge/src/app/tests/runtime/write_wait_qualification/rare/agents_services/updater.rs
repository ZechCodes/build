use super::*;
use crate::update::{
    HelperResult, Release, UpdateBackend, UpdateConfig, UpdateService, UpdateState,
};
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

struct FiniteBackend {
    result_path: PathBuf,
    checks: AtomicUsize,
    installed: Mutex<Vec<(String, String)>>,
    check_entered: Notify,
    check_release: Arc<Notify>,
    install_entered: Notify,
    install_release: Arc<Notify>,
}

struct ReleaseBackendGate(Arc<Notify>);

impl Drop for ReleaseBackendGate {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

#[async_trait]
impl UpdateBackend for FiniteBackend {
    async fn latest(&self) -> Result<Release, String> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        self.check_entered.notify_one();
        self.check_release.notified().await;
        Ok(Release {
            version: "0.3.0".into(),
            tag: "bridge-v0.3.0".into(),
            published_at: None,
        })
    }

    async fn install(&self, release: &Release, attempt: &str) -> Result<(), String> {
        self.install_entered.notify_one();
        self.install_release.notified().await;
        std::fs::create_dir_all(self.result_path.parent().unwrap())
            .map_err(|error| error.to_string())?;
        let result = HelperResult {
            attempt_id: attempt.to_owned(),
            success: true,
            version: release.version.clone(),
            error: None,
            rollback_pending: false,
        };
        std::fs::write(&self.result_path, serde_json::to_vec(&result).unwrap())
            .map_err(|error| error.to_string())?;
        self.installed
            .lock()
            .unwrap()
            .push((attempt.to_owned(), release.version.clone()));
        Ok(())
    }
}

fn config(root: &Path, version: &str) -> UpdateConfig {
    UpdateConfig {
        status_path: root.join("store/bridge-update-status.json"),
        result_path: root.join("updates/result.json"),
        running_version: version.into(),
        platform: "linux-x86_64".into(),
        development_build: false,
        replaceable_development_build: false,
        running_from_cargo_target: false,
        check_interval: Duration::from_secs(24 * 60 * 60),
    }
}

async fn await_status(service: &UpdateService, expected: UpdateState) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while service.status().state != expected {
        assert!(
            Instant::now() < deadline,
            "update did not reach {expected:?}: {:?}",
            service.status()
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

pub(super) async fn execute() -> Vec<VerifiedStep> {
    let (directory, repository) = init_repo();
    let backend = Arc::new(FiniteBackend {
        result_path: directory.path().join("updates/result.json"),
        checks: AtomicUsize::new(0),
        installed: Mutex::new(Vec::new()),
        check_entered: Notify::new(),
        check_release: Arc::new(Notify::new()),
        install_entered: Notify::new(),
        install_release: Arc::new(Notify::new()),
    });
    let service =
        Arc::new(UpdateService::new(config(directory.path(), "0.2.0"), backend.clone()).unwrap());
    let app = qa_state(&repository, directory.path()).with_update_service(Arc::clone(&service));
    let mut fixture = RareFixture::new("updater", app);
    assert_eq!(service.status().state, UpdateState::Idle);
    let release_check = ReleaseBackendGate(Arc::clone(&backend.check_release));
    let checked = fixture.call("bridge.check_update", json!({})).await;
    tokio::time::timeout(Duration::from_secs(2), backend.check_entered.notified()).await.unwrap();
    let check_retained_origin = fixture.origin_active(&checked).await;
    drop(release_check);
    await_status(&service, UpdateState::Available).await;
    assert_eq!(backend.checks.load(Ordering::SeqCst), 1);
    let status = fixture.call("bridge.update_status", json!({})).await;
    assert_eq!(status.reply["result"]["latest_release"]["version"], "0.3.0");
    let disk: Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("store/bridge-update-status.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(disk["state"], "available");

    let release_install = ReleaseBackendGate(Arc::clone(&backend.install_release));
    let installed = fixture
        .call("bridge.install_update", json!({"when":"now"}))
        .await;
    assert_eq!(installed.reply["result"]["state"], "installing");
    tokio::time::timeout(Duration::from_secs(2), backend.install_entered.notified()).await.unwrap();
    let install_retained_origin = fixture.origin_active(&installed).await;
    drop(release_install);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if backend.installed.lock().unwrap().len() == 1 {
            break;
        }
        assert!(Instant::now() < deadline, "finite backend was not called");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let (attempt, target) = backend.installed.lock().unwrap()[0].clone();
    let result: HelperResult =
        serde_json::from_slice(&std::fs::read(&backend.result_path).unwrap()).unwrap();
    assert_eq!(result.attempt_id, attempt);
    assert_eq!(result.version, target);
    assert_eq!(service.status().state, UpdateState::Installing);
    // A successful helper result cannot impersonate a new executable. The
    // normal restarted service, configured as the target version, consumes it.
    let restarted =
        Arc::new(UpdateService::new(config(directory.path(), "0.3.0"), backend.clone()).unwrap());
    let completed = restarted.tick(false).await.unwrap();
    assert_eq!(completed.running_version, "0.3.0");
    assert_eq!(completed.state, UpdateState::Idle);
    assert_eq!(completed.last_error, None);
    assert!(!backend.result_path.exists());
    assert!(restarted.admission().try_enter().is_some());
    assert!(check_retained_origin, "check acknowledgement lost its required backend origin");
    assert!(install_retained_origin, "install acknowledgement lost its required backend origin");
    fixture.verify(checked, json!({"before":"idle","after":status.reply["result"],"checks":1})).await;
    fixture.verify(installed, json!({"attempt":attempt,"helper_target":target,"restarted_running_version":completed.running_version,"settled_state":"idle","result_consumed":true})).await;
    fixture.finish().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_update_check_and_install_retain_required_service_origins() {
    let steps = execute().await;
    assert_eq!(steps.len(), 2);
}
