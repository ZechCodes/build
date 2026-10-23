use async_trait::async_trait;
use build_bridge::config::Channel;
use build_bridge::update::{
    InstallWhen, Release, UpdateBackend, UpdateConfig, UpdateError, UpdateService, UpdateState,
};
use std::sync::Arc;
use std::time::Duration;

struct NoCalls;
#[async_trait]
impl UpdateBackend for NoCalls {
    async fn latest(&self) -> Result<Release, String> {
        panic!("dev called latest")
    }
    async fn install(&self, _: &Release, _: &str) -> Result<(), String> {
        panic!("dev called install")
    }
    async fn stage(&self, _: &Release, _: &str) -> Result<(), String> {
        panic!("dev called stage")
    }
    async fn launch(&self, _: &Release, _: &str) -> Result<(), String> {
        panic!("dev called launch")
    }
    fn active_attempt(&self) -> Result<Option<String>, String> {
        panic!("dev inspected helper")
    }
    fn maintain(&self) -> Result<(), String> {
        panic!("dev maintained helper")
    }
}

#[tokio::test]
async fn dev_refuses_every_update_request_and_ignores_persisted_schedule_without_io() {
    let home = tempfile::tempdir().unwrap();
    // A directory cannot be read as JSON or removed as a helper result. Dev
    // must not inspect either, even when told this is a replaceable release.
    let blocked = home.path().join("do-not-read");
    std::fs::create_dir(&blocked).unwrap();
    let config = UpdateConfig {
        status_path: blocked.clone(),
        result_path: blocked.clone(),
        running_version: "1.0.0".into(),
        platform: "linux-x86_64".into(),
        development_build: false,
        replaceable_development_build: true,
        running_from_cargo_target: true,
        check_interval: Duration::ZERO,
    };
    let service =
        Arc::new(UpdateService::new_for_channel(config, Arc::new(NoCalls), Channel::Dev).unwrap());
    assert!(matches!(
        service.check().await,
        Err(UpdateError::ChannelDisabled)
    ));
    assert!(matches!(
        service.request_check(),
        Err(UpdateError::ChannelDisabled)
    ));
    for when in [InstallWhen::Now, InstallWhen::Idle] {
        assert!(matches!(
            service.install(when, false).await,
            Err(UpdateError::ChannelDisabled)
        ));
        for confirmed in [false, true] {
            assert!(matches!(
                service.install_confirmed(when, false, confirmed).await,
                Err(UpdateError::ChannelDisabled)
            ));
            assert!(matches!(
                service.request_install(when, false, confirmed),
                Err(UpdateError::ChannelDisabled)
            ));
        }
    }
    assert_eq!(service.tick(false).await.unwrap().state, UpdateState::Idle);
    assert_eq!(service.tick(true).await.unwrap().state, UpdateState::Idle);
    assert!(service.status().development_build);
    assert!(!service.status().can_install);
    assert!(!service.status().can_replace_development_build);
    assert!(blocked.is_dir());
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1);
}
