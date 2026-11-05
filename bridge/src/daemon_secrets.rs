//! Daemon-only secret upload and worker shutdown composition.
use build_bridge::secrets::SweeperHandle;

pub(super) async fn stop_sweeper(sweeper: Option<SweeperHandle>) {
    if let Some(sweeper) = sweeper {
        sweeper.stop().await;
    }
}

/// The only application upload handle that retains the transport key. General
/// snapshots and agent launches receive neither the key nor an opening method.
pub(super) fn with_upload_opener(
    app: build_bridge::app::AppState,
    private_key_b64: &str,
) -> build_bridge::app::AppState {
    match build_bridge::secrets::upload::SealedUploadOpener::new(private_key_b64) {
        Ok(opener) => app.with_secret_upload_opener(std::sync::Arc::new(opener)),
        Err(_) => {
            eprintln!("secrets: upload opener unavailable");
            app
        }
    }
}
