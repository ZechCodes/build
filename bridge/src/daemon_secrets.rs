//! Daemon-only secret worker shutdown composition.
use build_bridge::secrets::SweeperHandle;

pub(super) async fn stop_sweeper(sweeper: Option<SweeperHandle>) {
    if let Some(sweeper) = sweeper {
        sweeper.stop().await;
    }
}
