use super::*;
use crate::update::{
    InstallWhen, Release, UpdateBackend, UpdateConfig, UpdateService, UpdateState,
};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Default)]
pub(super) struct Backend {
    launches: AtomicUsize,
}

#[async_trait::async_trait]
impl UpdateBackend for Backend {
    async fn latest(&self) -> Result<Release, String> {
        Ok(Release {
            version: "1.1.0".into(),
            tag: "v1.1.0".into(),
            published_at: None,
        })
    }

    async fn install(&self, _release: &Release, _attempt_id: &str) -> Result<(), String> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

pub(super) fn update_service(root: &Path, backend: Arc<Backend>) -> Arc<UpdateService> {
    Arc::new(
        UpdateService::new(
            UpdateConfig {
                status_path: root.join("test-update/status.json"),
                result_path: root.join("test-update/result.json"),
                running_version: "1.0.0".into(),
                platform: "linux-x86_64".into(),
                development_build: false,
                replaceable_development_build: false,
                running_from_cargo_target: false,
                check_interval: Duration::from_secs(24 * 60 * 60),
            },
            backend,
        )
        .unwrap(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_mcp_read_keeps_update_admission_until_blocking_query_finishes() {
    let fixture = Fixture::new();
    let backend = Arc::new(Backend::default());
    let updates = update_service(fixture._root.path(), Arc::clone(&backend));
    fixture.state.lock().unwrap().updates = Some(Arc::clone(&updates));
    updates.check().await.unwrap();
    updates.install(InstallWhen::Idle, true).await.unwrap();
    let (gate, held) = OffLockGate::new();
    fixture.state.lock().unwrap().off_lock_gate = Some(gate);
    let (mut client, daemon) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(handle_done_stream(
        Arc::clone(&fixture.state),
        daemon,
        Arc::clone(&fixture.plane),
    ));
    client
        .write_all(format!("{}\n", fixture.frame(fixture.get())).as_bytes())
        .await
        .unwrap();
    tokio::time::timeout(WATCHDOG, async {
        while !held.has_pending_arrival() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("read never reached the blocking query");
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    drop(client);
    assert_eq!(
        updates.tick(false).await.unwrap().state,
        UpdateState::ScheduledWhenIdle,
        "cancelling the socket must not release a still-running query's lease"
    );
    assert_eq!(backend.launches.load(Ordering::SeqCst), 0);
    held.release();
    tokio::time::timeout(WATCHDOG, async {
        loop {
            if updates.tick(false).await.unwrap().state == UpdateState::Installing {
                break;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("query completion did not release its admission lease");
    assert_eq!(backend.launches.load(Ordering::SeqCst), 1);
}
