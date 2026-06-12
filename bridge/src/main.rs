//! `build-bridge` — the device daemon entry point.
//!
//! `build-bridge serve` connects to the relay and serves the orchestrator-backed
//! application RPC over the E2EE channel. Configuration is by environment:
//!
//! - `BRIDGE_RELAY_URL`   relay base URL (default `ws://127.0.0.1:8799`)
//! - `BRIDGE_REPO`        the git repo tasks operate on (default `/repo`)
//! - `BRIDGE_WORKTREES`   where task worktrees are created (default `/worktrees`)
//! - `BRIDGE_BASE_BRANCH` base branch (default `main`)
//! - `BRIDGE_DEVICE_ID`   device id presented to the relay (default `bridge-dev`)
//! - `BRIDGE_QA_AGENT`    `1` to run the deterministic scripted agent (no LLM)

use std::time::Duration;

use build_bridge::app::AppState;
use build_bridge::relay::{self, DeviceIdentity};
use build_bridge::transport;

#[tokio::main]
async fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("serve") | None => serve().await,
        Some("--version") | Some("-V") => {
            println!("build-bridge {}", env!("CARGO_PKG_VERSION"));
        }
        Some(other) => {
            eprintln!("unknown command: {other}\nusage: build-bridge [serve]");
            std::process::exit(2);
        }
    }
}

async fn serve() {
    let relay_url = env("BRIDGE_RELAY_URL", "ws://127.0.0.1:8799");
    let repo = env("BRIDGE_REPO", "/repo");
    let worktrees = env("BRIDGE_WORKTREES", "/worktrees");
    let base_branch = env("BRIDGE_BASE_BRANCH", "main");
    let qa_agent = matches!(
        std::env::var("BRIDGE_QA_AGENT").as_deref(),
        Ok("1") | Ok("true")
    );
    let device_url = format!("{}/ws/device", relay_url.trim_end_matches('/'));

    // An ephemeral identity is fine for local dev; production loads a persisted one.
    let identity = DeviceIdentity {
        device_id: env("BRIDGE_DEVICE_ID", "bridge-dev"),
        identity_private_key_b64: transport::generate_identity_keypair().private_key_b64,
        transport: transport::generate_transport_keypair(),
    };

    println!(
        "bridge serve → {device_url}  (repo={repo} worktrees={worktrees} base={base_branch} qa_agent={qa_agent})"
    );

    // Build the handler once so task state survives reconnects.
    let handler = AppState::new(&repo, &worktrees, &base_branch, qa_agent).into_handler();

    loop {
        match relay::run(&device_url, &identity, handler.clone()).await {
            Ok(()) => eprintln!("relay disconnected; reconnecting in 2s"),
            Err(e) => eprintln!("relay error: {e}; reconnecting in 2s"),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
