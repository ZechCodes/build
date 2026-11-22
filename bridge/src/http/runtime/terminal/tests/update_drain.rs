use crate::{
    app::AppState,
    harness::HarnessContext,
    update::{InstallWhen, Release, UpdateBackend, UpdateConfig, UpdateService, UpdateState},
};

use super::*;

#[tokio::test]
async fn mounted_tls_update_handoff_blocks_terminal_effects_and_joins_the_socket() {
    let state = tempfile::tempdir().unwrap();
    let updates = fixture_updater(state.path());
    let app = AppState::new_unrooted_configured(
        state.path().join("worktrees"),
        "main",
        true,
        HarnessContext::resolved(state.path().join("mcp.sock"), state.path().to_owned()).unwrap(),
    )
    .with_update_service(updates.clone())
    .shared();
    let server = Server::start(AppState::handler(app)).await;
    let opening = server.opening().await;
    let mut socket = server.socket(&server.ticket(&opening).await).await;
    assert_eq!(
        call(&mut socket, "before", "ping", json!({})).await["ok"],
        true
    );

    // Use the real admission gate and a backend that never installs anything.
    updates.check().await.unwrap();
    updates.install(InstallWhen::Idle, true).await.unwrap();
    let status = updates.tick(false).await.unwrap();
    assert_eq!(status.state, UpdateState::Installing);
    assert!(updates.admission().try_enter().is_none());
    let blocked = call(
        &mut socket,
        "effect",
        "term.create",
        json!({"project_id":"qualification-project"}),
    )
    .await;
    assert_eq!(blocked["error_code"], "busy", "{blocked:?}");

    timeout(crate::http::limits::SOCKET_TEARDOWN_TIMEOUT, server.stop())
        .await
        .expect("the listener joins the active WSS connection inside its drain allowance");
    assert_closed(&mut socket, None).await;
}

struct FixtureBackend;

#[async_trait::async_trait]
impl UpdateBackend for FixtureBackend {
    async fn latest(&self) -> Result<Release, String> {
        Ok(Release {
            version: "0.3.0".into(),
            tag: "bridge-v0.3.0".into(),
            published_at: None,
        })
    }

    async fn install(&self, _release: &Release, _attempt: &str) -> Result<(), String> {
        Ok(())
    }
}

fn fixture_updater(root: &std::path::Path) -> Arc<UpdateService> {
    Arc::new(
        UpdateService::new(
            UpdateConfig {
                status_path: root.join("update-status.json"),
                result_path: root.join("update-result.json"),
                running_version: "0.2.0".into(),
                platform: "linux-x86_64".into(),
                development_build: false,
                replaceable_development_build: false,
                running_from_cargo_target: false,
                check_interval: Duration::from_secs(24 * 60 * 60),
            },
            Arc::new(FixtureBackend),
        )
        .unwrap(),
    )
}
