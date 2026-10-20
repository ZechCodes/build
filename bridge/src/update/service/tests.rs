use super::*;

struct Backend;

#[async_trait]
impl UpdateBackend for Backend {
    async fn latest(&self) -> Result<Release, String> {
        unreachable!("snapshot tests do not check releases")
    }

    async fn install(&self, _: &Release, _: &str) -> Result<(), String> {
        unreachable!("snapshot tests do not install releases")
    }
}

fn service(root: &Path) -> Arc<UpdateService> {
    Arc::new(
        UpdateService::new(
            UpdateConfig {
                status_path: root.join("status.json"),
                result_path: root.join("result.json"),
                running_version: "1.0.0".into(),
                platform: "linux-x86_64".into(),
                development_build: false,
                replaceable_development_build: false,
                running_from_cargo_target: false,
                check_interval: Duration::from_secs(86400),
            },
            Arc::new(Backend),
        )
        .unwrap(),
    )
}

#[test]
fn update_status_does_not_wait_for_subscription_lock() {
    let root = tempfile::tempdir().unwrap();
    let service = service(root.path());
    let expected = service.status();
    let (entered, acquired) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let publishing = Arc::clone(&service);
    let publisher = std::thread::spawn(move || {
        publishing.status_tx.send_modify(|_| {
            entered.send(()).unwrap();
            released.recv().unwrap();
        });
    });
    acquired.recv_timeout(Duration::from_secs(1)).unwrap();
    let (sent, received) = std::sync::mpsc::channel();
    let reading = Arc::clone(&service);
    let reader = std::thread::spawn(move || sent.send(reading.status()).unwrap());
    let observed = received.recv_timeout(Duration::from_secs(1));
    release.send(()).unwrap();
    publisher.join().unwrap();
    reader.join().unwrap();
    assert_eq!(
        observed.expect("status waited for subscription lock"),
        expected
    );
}

#[test]
fn failed_status_save_cannot_contaminate_later_publication() {
    let root = tempfile::tempdir().unwrap();
    let service = service(root.path());
    let updates = service.subscribe();
    let committed = service.status();
    let mut candidate = committed.clone();
    candidate.state = UpdateState::Available;
    candidate.latest_release = Some(Release {
        version: "2.0.0".into(),
        tag: "v2.0.0".into(),
        published_at: None,
    });
    std::fs::create_dir(&service.config.status_path).unwrap();
    assert!(service
        .publish(candidate, &AttemptMetadata::default())
        .is_err());
    assert_eq!(service.status(), committed);
    assert!(!updates.has_changed().unwrap());

    std::fs::remove_dir(&service.config.status_path).unwrap();
    let mut unrelated = committed;
    unrelated.last_checked_at = Some("2026-10-19T00:00:00Z".into());
    let published = service
        .publish(unrelated, &AttemptMetadata::default())
        .unwrap();
    assert_eq!(service.status(), published);
    assert!(published.latest_release.is_none());
    assert_eq!(*updates.borrow(), published);
}
