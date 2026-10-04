//! Real Chromium + TURN + stock inbound firewall regression for task #372.
//!
//! Run explicitly on Linux with `cargo test --test rtc_lan_upgrade -- --ignored`.
//! All processes, addresses and firewall rules live in disposable user/network
//! namespaces. The fixture never starts the daemon or reads its identity.

use build_bridge::app::AppState;
use build_bridge::carrier::FrameIntake;
use build_bridge::harness::HarnessContext;
use build_bridge::rtc::{IcePolicy, WebrtcPeerFactory};
use build_bridge::transport;
use common::{connected_device, device_identity};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

mod common;

#[test]
#[ignore = "needs Linux user/network namespaces, nft, ip and Chromium"]
fn late_mdns_host_is_checked_before_the_spa_restarts_ice() {
    if std::env::var_os("BUILD_RTC_LAN_CHILD").is_some() {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(serve_fixture());
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let status = std::process::Command::new("python3")
        .arg(root.join("web/rtc-lan-upgrade/run.py"))
        .arg(std::env::current_exe().unwrap())
        .arg(directory.path())
        .status()
        .expect("run the isolated LAN fixture");
    assert!(
        status.success(),
        "the Chromium LAN regression failed: {status}"
    );
}

/// The device's actual relay client and FrameIntake. The extra WebSocket is
/// just the mock relay's browser side; encrypted frames pass through unchanged.
async fn serve_fixture() {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .ok();
    let state_dir = tempfile::tempdir().unwrap();
    let app = AppState::new_unrooted_configured(
        state_dir.path().join("worktrees"),
        "main",
        false,
        HarnessContext::resolved(
            state_dir.path().join("mcp.sock"),
            state_dir.path().to_path_buf(),
        )
        .unwrap(),
    )
    .with_task_store(state_dir.path().join("tasks"))
    .unwrap()
    .shared();
    let repo_path = fixture_repository(state_dir.path());
    app.lock()
        .unwrap()
        .add_project(repo_path, "main".to_string());
    let intake = FrameIntake::new(
        AppState::handler(app.clone()),
        transport::generate_transport_keypair(),
    );
    app.lock()
        .unwrap()
        .set_peer_factory(WebrtcPeerFactory::with_pending_direct_pair_checks(
            intake.clone(),
            IcePolicy::default(),
            std::env::var_os("BUILD_RTC_LAN_BASELINE").is_none(),
        ));
    let device = connected_device(intake, &device_identity()).await;
    let listener = TcpListener::bind("10.72.0.1:9000").await.unwrap();
    std::fs::write(std::env::var("BUILD_RTC_LAN_READY").unwrap(), "ready").unwrap();
    let (socket, _) = listener.accept().await.unwrap();
    let websocket = tokio_tungstenite::accept_async(socket).await.unwrap();
    let (mut output, mut input) = websocket.split();
    output
        .send(Message::Text(
            json!({ "type": "fixture", "transport_public_key": device.transport_public_key })
                .to_string(),
        ))
        .await
        .unwrap();
    let mut from_device = device.from_device;
    loop {
        tokio::select! {
            message = input.next() => {
                let Some(Ok(Message::Text(text))) = message else { break; };
                device.to_device.send(serde_json::from_str(&text).unwrap()).await.unwrap();
            },
            message = from_device.recv() => {
                let Some(message) = message else { break; };
                output.send(Message::Text(message.to_string())).await.unwrap();
            }
        }
    }
    device.bridge.abort();
    device.relay_socket.abort();
}

/// libgit2 fixture setup with an explicit local identity, unsigned product
/// commits, and a cleared process environment supplied by the namespace runner.
fn fixture_repository(parent: &std::path::Path) -> std::path::PathBuf {
    let path = parent.join("fixture-project");
    let repository = git2::Repository::init_opts(
        &path,
        git2::RepositoryInitOptions::new().initial_head("main"),
    )
    .unwrap();
    let mut config = repository.config().unwrap();
    config.set_str("user.name", "LAN fixture").unwrap();
    config.set_str("user.email", "fixture@localhost").unwrap();
    config.set_bool("commit.gpgsign", false).unwrap();
    let tree_id = repository.index().unwrap().write_tree().unwrap();
    let signature = git2::Signature::now("LAN fixture", "fixture@localhost").unwrap();
    repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            "fixture",
            &repository.find_tree(tree_id).unwrap(),
            &[],
        )
        .unwrap();
    path
}
