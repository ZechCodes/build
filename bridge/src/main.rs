//! `build-bridge` — the device daemon entry point.
//!
//! `build-bridge serve` connects to the relay and serves the orchestrator-backed
//! application RPC over the E2EE channel. Configuration is by environment:
//!
//! - `BRIDGE_RELAY_URL`   relay base URL (default `ws://127.0.0.1:8799`; production
//!   `wss://relay.getbuild.ing` — TLS is rustls with bundled webpki roots)
//! - `BRIDGE_REPO`        the default git repo tasks operate on (default `/repo`)
//! - `BRIDGE_PROJECTS`    extra repos, comma-separated `path` or `path=branch`
//! - `BRIDGE_WORKTREES`   where task worktrees are created (default `/worktrees`)
//! - `BRIDGE_BASE_BRANCH` base branch (default `main`)
//! - `BRIDGE_PROJECTS_DIR` where cloned repos land (default `~/.build/projects`)
//! - `BRIDGE_CONFIG`      projects/settings persistence (default `~/.build/config.json`)
//! - `BRIDGE_DEVICE_ID`   device id presented to the relay (default `bridge-dev`)
//! - `BRIDGE_QA_AGENT`    `1` to run the deterministic scripted agent (no LLM)
//! - `BRIDGE_IDENTITY_FILE` durable identity path (default `~/.build/identity.json`)
//! - `BRIDGE_API_URL`     the api (skriftapp) base URL for pairing (default
//!   `http://127.0.0.1:8080`; production `https://getbuild.ing`)
//! - `BRIDGE_WEB_URL`     the web app base URL printed in the approve link (default = api url)
//! - `BRIDGE_DEVICE_NAME` device name shown during pairing (default: hostname)

use std::time::Duration;

use build_bridge::app::AppState;
use build_bridge::relay::{self, DeviceIdentity};
use build_bridge::{identity, pairing, transport};

#[tokio::main]
async fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("serve") | None => serve().await,
        Some("mcp") => mcp_stdio(),
        Some("provision") => provision(),
        Some("--version") | Some("-V") => {
            println!("build-bridge {}", env!("CARGO_PKG_VERSION"));
        }
        Some(other) => {
            eprintln!("unknown command: {other}\nusage: build-bridge [serve|provision]");
            std::process::exit(2);
        }
    }
}

/// Generate a device identity bundle as JSON: a UUID device id, the Ed25519
/// identity key, and the X25519 transport key. The deploy scripts feed the
/// padded public key into the relay DB (its base64 decoder requires padding) and
/// the private keys into the bridge.
fn provision() {
    use base64::Engine;
    let pad = |unpadded: &str| {
        let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(unpadded)
            .expect("valid base64");
        base64::engine::general_purpose::STANDARD.encode(bytes)
    };
    let device_id = uuid::Uuid::new_v4().to_string();
    let identity = transport::generate_identity_keypair();
    let tp = transport::generate_transport_keypair();
    let bundle = serde_json::json!({
        "device_id": device_id,
        "ed25519_priv_b64": identity.private_key_b64,
        "ed25519_pub_b64_padded": pad(&identity.public_key_b64),
        "x25519_priv_b64": tp.private_key_b64,
        "x25519_pub_b64": tp.public_key_b64,
        "x25519_pub_b64_padded": pad(&tp.public_key_b64),
    });
    println!("{}", serde_json::to_string_pretty(&bundle).unwrap());
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

    // Identity. A provisioned identity in the environment (matches the relay DB seed)
    // is a prod/seed override that is treated as already approved and skips pairing.
    // Otherwise the bridge loads-or-generates a durable identity and pairs it to a user
    // account: register as pending, print the pairing code + fingerprint, wait for the
    // human to approve in the web app, then connect.
    let identity = match (
        std::env::var("BRIDGE_IDENTITY_PRIV"),
        std::env::var("BRIDGE_TRANSPORT_PRIV"),
        std::env::var("BRIDGE_TRANSPORT_PUB"),
    ) {
        (Ok(id_priv), Ok(tp_priv), Ok(tp_pub)) => DeviceIdentity {
            device_id: env("BRIDGE_DEVICE_ID", "bridge-dev"),
            identity_private_key_b64: id_priv,
            transport: transport::KeyPairB64 {
                public_key_b64: tp_pub,
                private_key_b64: tp_priv,
            },
        },
        _ => {
            let identity_path =
                expand_tilde(&env("BRIDGE_IDENTITY_FILE", "~/.build/identity.json"));
            let stored = match identity::load(&identity_path) {
                Ok(Some(stored)) => stored,
                Ok(None) => {
                    let fresh = identity::generate(&env("BRIDGE_DEVICE_NAME", &hostname()));
                    if let Err(e) = identity::save(&identity_path, &fresh) {
                        eprintln!("could not persist identity to {identity_path:?}: {e}");
                        std::process::exit(1);
                    }
                    fresh
                }
                Err(e) => {
                    eprintln!("could not load identity from {identity_path:?}: {e}");
                    std::process::exit(1);
                }
            };
            let api_url = env("BRIDGE_API_URL", "http://127.0.0.1:8080");
            let web_url = env("BRIDGE_WEB_URL", &api_url);
            let client = reqwest::Client::new();
            let approved = match pairing::ensure_paired(
                &client,
                &api_url,
                &web_url,
                &identity_path,
                stored,
                Duration::from_secs(2),
            )
            .await
            {
                Ok(approved) => approved,
                Err(e) => {
                    eprintln!("pairing failed: {e}");
                    std::process::exit(1);
                }
            };
            identity::to_device_identity(&approved)
        }
    };

    // The control socket real agents forward `done` to (and the daemon listens on).
    let mcp_socket = env(
        "BRIDGE_MCP_SOCKET",
        &format!("{}/build-bridge-mcp.sock", worktrees.trim_end_matches('/')),
    );

    println!(
        "bridge serve → {device_url}  (repo={repo} worktrees={worktrees} base={base_branch} qa_agent={qa_agent} mcp_socket={mcp_socket})"
    );

    // Shared state: the relay handler and the done-socket listener drive the same
    // tasks; state survives reconnects. The default repo is project one; any extra
    // repos in BRIDGE_PROJECTS are registered alongside it.
    let mut app = AppState::new(&repo, &worktrees, &base_branch, qa_agent, &mcp_socket);
    for entry in std::env::var("BRIDGE_PROJECTS")
        .unwrap_or_default()
        .split(',')
    {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (path, branch) = entry
            .split_once('=')
            .unwrap_or((entry, base_branch.as_str()));
        let id = app.add_project(std::path::PathBuf::from(path), branch.to_string());
        println!("  + project {id}: {path} (base {branch})");
    }

    // Persist projects + projects-dir so UI-added/cloned repos survive restarts,
    // and restore them on boot. An env override for the projects folder wins.
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let config_path = env("BRIDGE_CONFIG", &format!("{home}/.build/config.json"));
    let mut app = app.with_config(&config_path);
    if let Ok(dir) = std::env::var("BRIDGE_PROJECTS_DIR") {
        app.set_projects_dir(std::path::PathBuf::from(dir));
    }
    let app = app.shared();
    AppState::spawn_done_socket(app.clone(), mcp_socket.clone());
    let handler = AppState::handler(app);

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

/// Expand a leading `~/` against `$HOME`. (The bridge binary is a separate crate from
/// the lib, so it can't use the lib's `pub(crate)` helper.)
fn expand_tilde(path: &str) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return std::path::Path::new(&home).join(rest);
        }
    }
    std::path::PathBuf::from(path)
}

/// A human-recognizable default device name. Falls back to `bridge` when the host
/// name can't be determined.
fn hostname() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "bridge".to_string())
}

/// `build-bridge mcp --task <id>` — the per-session MCP server the harness spawns
/// (via the worktree's `.build/mcp.json`). It serves the single `done` tool over
/// stdio and forwards each report to the running daemon's control socket
/// (`BRIDGE_MCP_SOCKET`) as `{"task_id","report"}` lines, so a real agent's `done`
/// reaches `orchestrator.on_done`. Without the socket it just logs (for testing).
fn mcp_stdio() {
    use std::io::Write;

    let args: Vec<String> = std::env::args().collect();
    let task_id = args
        .iter()
        .position(|a| a == "--task")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());
    let socket = std::env::var("BRIDGE_MCP_SOCKET").ok();

    let server = build_bridge::mcp::DoneServer::new(&task_id);
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    let _ = server.run_stdio(stdin, stdout, |report| {
        let line = serde_json::json!({ "task_id": task_id, "report": report }).to_string();
        match &socket {
            Some(path) => {
                if let Ok(mut stream) = std::os::unix::net::UnixStream::connect(path) {
                    let _ = writeln!(stream, "{line}");
                } else {
                    eprintln!("[mcp] could not reach daemon socket {path}");
                }
            }
            None => eprintln!("[mcp] done: {line}"),
        }
    });
}
