//! `build-bridge` — the device daemon entry point.
//!
//! `build-bridge serve` connects to the relay and serves the orchestrator-backed
//! application RPC over the E2EE channel. Configuration is by environment:
//!
//! Defaults are production: a fresh install needs NO environment — it pairs
//! against getbuild.ing and keeps state under `~/.build`. Dev stacks override.
//!
//! - `BRIDGE_RELAY_URL`   relay base URL (default `wss://relay.getbuild.ing`;
//!   TLS is rustls with bundled webpki roots)
//! - `BRIDGE_REPO`        optional default git repo to register as project one;
//!   unset means projects come from the UI (clone/add) + persisted config
//! - `BRIDGE_PROJECTS`    extra repos, comma-separated `path` or `path=branch`
//! - `BRIDGE_WORKTREES`   where task worktrees are created (default `~/.build/worktrees`)
//! - `BRIDGE_BASE_BRANCH` base branch (default `main`)
//! - `BRIDGE_PROJECTS_DIR` where cloned repos land (default `~/.build/projects`)
//! - `BRIDGE_CONFIG`      projects/settings persistence (default `~/.build/config.json`)
//! - `BRIDGE_TASKS_DIR`   state dir: `build.db` plus canonical plan docs (default `~/.build/tasks`)
//! - `BRIDGE_DEVICE_ID`   device id presented to the relay (default `bridge-dev`)
//! - `BRIDGE_QA_AGENT`    `1` to run the deterministic scripted agent (no LLM)
//! - `BRIDGE_IDLE_SECONDS` PTY-quiet threshold before a working task without a
//!   `done` is demoted to `idle_unreported` (default 300)
//! - `BRIDGE_IDENTITY_FILE` durable identity path (default `~/.build/identity.json`)
//! - `BRIDGE_API_URL`     the api (skriftapp) base URL for pairing and for the
//!   signed, content-free web-push notifies fired when a task needs the human
//!   (default `https://getbuild.ing`)
//! - `BRIDGE_WEB_URL`     the web app base URL printed in the approve link (default = api url)
//! - `BRIDGE_DEVICE_NAME` device name shown during pairing (default: hostname)
//! - `BRIDGE_PAIRING_CODE` dev/compose only: pair with this fixed code instead of
//!   a random one, so a scripted approver can complete the flow
//!
//! `build-bridge install-service` (macOS) installs a launchd LaunchAgent that
//! keeps `serve` running across crashes and logins. It is gated on pairing:
//! the api must confirm this device is approved and owned by an account before
//! anything is written. `uninstall-service` removes it.

use std::sync::Arc;
use std::time::Duration;

use build_bridge::app::AppState;
use build_bridge::backoff::Backoff;
use build_bridge::carrier::FrameIntake;
use build_bridge::config::BridgeConfig;
use build_bridge::harness::HarnessContext;
use build_bridge::notify::Notifier;
use build_bridge::relay::{self, DeviceIdentity};
use build_bridge::rtc::WebrtcPeerFactory;
use build_bridge::transport_ledger::{FanOutLedger, StderrLedger};
use build_bridge::transport_report::TransportReporter;
use build_bridge::{identity, pairing, service, transport};

#[tokio::main]
async fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("serve") | None => serve().await,
        Some("mcp") => mcp_stdio(),
        Some("provision") => provision(),
        Some("backup") => backup(),
        Some("install-service") => install_service().await,
        Some("uninstall-service") => uninstall_service(),
        Some("--version") | Some("-V") => {
            println!("build-bridge {}", env!("CARGO_PKG_VERSION"));
        }
        Some(other) => {
            eprintln!(
                "unknown command: {other}\nusage: build-bridge [serve|backup <path>|provision|install-service|uninstall-service]"
            );
            std::process::exit(2);
        }
    }
}

/// Copy the state database to a file, consistently, while the daemon runs.
///
/// The state dir holds a live database and its write-ahead log, which a
/// file-at-a-time backup tool cannot copy coherently — the JSON records it
/// replaced could each be copied on their own, and this cannot. This is the
/// supported way to take one.
fn backup() {
    let Some(destination) = std::env::args().nth(2) else {
        eprintln!("usage: build-bridge backup <path>");
        std::process::exit(2);
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let tasks_dir = env("BRIDGE_TASKS_DIR", &format!("{home}/.build/tasks"));
    let store = match build_bridge::store::Store::new(&tasks_dir) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("cannot open the store at {tasks_dir}: {error}");
            std::process::exit(1);
        }
    };
    match store.backup_to(std::path::Path::new(&destination)) {
        Ok(()) => println!("wrote {destination}"),
        Err(error) => {
            eprintln!("backup failed: {error}");
            std::process::exit(1);
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
    // Before anything opens TLS: pairing (https), the relay socket (wss) and
    // the peer transport (DTLS) all run on the one provider this installs.
    relay::install_crypto_provider();
    adopt_login_path();
    let runtime = match resolve_runtime_paths() {
        Ok(runtime) => runtime,
        Err(error) => exit_startup(error),
    };
    let (identity, transport_keypair) = match load_device_identity(&runtime.config).await {
        Ok(loaded) => loaded,
        Err(error) => exit_startup(error),
    };
    let app = match construct_app(&runtime, &identity) {
        Ok(app) => app,
        Err(error) => exit_startup(error),
    };
    run_daemon(runtime, identity, transport_keypair, app).await;
}

fn adopt_login_path() {
    if std::env::var("BRIDGE_PATH_FROM_SHELL").as_deref() != Ok("0") {
        let shell = build_bridge::app::resolve_term_shell();
        match build_bridge::app::capture_login_path(&shell, Duration::from_secs(5)) {
            Some(path) => {
                std::env::set_var("PATH", &path);
                eprintln!("PATH adopted from login shell ({shell})");
            }
            None => eprintln!(
                "no usable PATH from login shell ({shell}); keeping inherited PATH {:?}",
                std::env::var("PATH").unwrap_or_default()
            ),
        }
    }
}

struct RuntimePaths {
    config: BridgeConfig,
    device_url: String,
    worktrees: String,
    tasks_dir: std::path::PathBuf,
    config_path: std::path::PathBuf,
    harness_context: HarnessContext,
    mcp_socket: String,
    qa_agent: bool,
}

fn resolve_runtime_paths() -> Result<RuntimePaths, String> {
    let config = bridge_config();
    let device_url = format!("{}/ws/device", config.relay_url.trim_end_matches('/'));
    let qa_agent = matches!(
        std::env::var("BRIDGE_QA_AGENT").as_deref(),
        Ok("1") | Ok("true")
    );
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let tasks_dir =
        std::path::PathBuf::from(env("BRIDGE_TASKS_DIR", &format!("{home}/.build/tasks")));
    let state_root = tasks_dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let harness_context = HarnessContext::resolved(config.mcp_socket.clone(), state_root.into())
        .map_err(|error| format!("cannot resolve harness runtime: {error}"))?;
    Ok(RuntimePaths {
        device_url,
        worktrees: config.worktrees.to_string_lossy().into_owned(),
        config_path: env("BRIDGE_CONFIG", &format!("{home}/.build/config.json")).into(),
        mcp_socket: config.mcp_socket.to_string_lossy().into_owned(),
        config,
        tasks_dir,
        harness_context,
        qa_agent,
    })
}

/// The device's identity and its transport keypair. A provisioned identity in
/// the environment (matches the relay DB seed) is a prod/seed override that is
/// treated as already approved and skips pairing. Otherwise the bridge
/// loads-or-generates a durable identity and pairs it to a user account:
/// register as pending, print the pairing code + fingerprint, wait for the
/// human to approve in the web app, then connect.
///
/// The transport keypair travels beside the identity, not inside it: its one
/// owner is the intake that opens session keys with it (`carrier.rs`).
async fn load_device_identity(
    config: &BridgeConfig,
) -> Result<(DeviceIdentity, transport::KeyPairB64), String> {
    match (
        std::env::var("BRIDGE_IDENTITY_PRIV"),
        std::env::var("BRIDGE_TRANSPORT_PRIV"),
        std::env::var("BRIDGE_TRANSPORT_PUB"),
    ) {
        (Ok(id_priv), Ok(tp_priv), Ok(tp_pub)) => Ok((
            DeviceIdentity {
                device_id: env("BRIDGE_DEVICE_ID", "bridge-dev"),
                identity_private_key_b64: id_priv,
            },
            transport::KeyPairB64 {
                public_key_b64: tp_pub,
                private_key_b64: tp_priv,
            },
        )),
        _ => {
            let identity_path = config.identity_file.clone();
            let stored = match identity::load(&identity_path) {
                Ok(Some(stored)) => stored,
                Ok(None) => {
                    let fresh = identity::generate(&env("BRIDGE_DEVICE_NAME", &hostname()));
                    identity::save(&identity_path, &fresh).map_err(|error| {
                        format!("could not persist identity to {identity_path:?}: {error}")
                    })?;
                    fresh
                }
                Err(error) => {
                    return Err(format!(
                        "could not load identity from {identity_path:?}: {error}"
                    ))
                }
            };
            let pairing_code_override = std::env::var("BRIDGE_PAIRING_CODE").ok();
            let client = reqwest::Client::new();
            let approved = pairing::ensure_paired(
                &client,
                &config.api_url,
                &config.web_url,
                &identity_path,
                stored,
                Duration::from_secs(2),
                pairing_code_override.as_deref(),
            )
            .await
            .map_err(|error| format!("pairing failed: {error}"))?;
            Ok((
                identity::to_device_identity(&approved),
                approved.transport.clone(),
            ))
        }
    }
}

fn construct_app(runtime: &RuntimePaths, identity: &DeviceIdentity) -> Result<AppState, String> {
    let config = &runtime.config;
    let app = match &config.repo {
        Some(repo) => AppState::new_configured(
            repo,
            &runtime.worktrees,
            &config.base_branch,
            runtime.qa_agent,
            runtime.harness_context.clone(),
        ),
        None => AppState::new_unrooted_configured(
            &runtime.worktrees,
            &config.base_branch,
            runtime.qa_agent,
            runtime.harness_context.clone(),
        ),
    }
    .with_notifier(Notifier::new(
        &config.api_url,
        &identity.device_id,
        &identity.identity_private_key_b64,
    ));
    let mut app = register_environment_projects(app, &config.base_branch);
    app = app
        .with_config(&runtime.config_path)
        .map_err(|error| format!("cannot load config: {error}"))?;
    if let Ok(dir) = std::env::var("BRIDGE_PROJECTS_DIR") {
        app.set_projects_dir(std::path::PathBuf::from(dir));
    }
    app.with_task_store(&runtime.tasks_dir)
        .map_err(|error| task_store_startup_error(&runtime.tasks_dir, &error))
}

fn register_environment_projects(mut app: AppState, base_branch: &str) -> AppState {
    for entry in std::env::var("BRIDGE_PROJECTS")
        .unwrap_or_default()
        .split(',')
    {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (path, branch) = entry.split_once('=').unwrap_or((entry, base_branch));
        let id = app.add_project(std::path::PathBuf::from(path), branch.to_string());
        println!("  + project {id}: {path} (base {branch})");
    }
    app
}

fn task_store_startup_error(tasks_dir: &std::path::Path, error: &str) -> String {
    format!(
        "cannot start because the task store did not open\n  {error}\n  state lives in {} \n  check that no second bridge is running and the directory is readable and writable",
        tasks_dir.display()
    )
}

async fn run_daemon(
    runtime: RuntimePaths,
    identity: DeviceIdentity,
    transport_keypair: transport::KeyPairB64,
    app: AppState,
) {
    let repo_display = runtime
        .config
        .repo
        .clone()
        .unwrap_or_else(|| "<none>".to_string());
    println!(
        "bridge serve → {}  (repo={} worktrees={} base={} qa_agent={} mcp_socket={})",
        runtime.device_url,
        repo_display,
        runtime.worktrees,
        runtime.config.base_branch,
        runtime.qa_agent,
        runtime.mcp_socket
    );
    let app = app.shared();
    AppState::spawn_done_socket(app.clone(), runtime.mcp_socket.clone());
    let idle_threshold = std::env::var("BRIDGE_IDLE_SECONDS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or(300);
    AppState::spawn_idle_monitor(
        app.clone(),
        Duration::from_secs(idle_threshold),
        Duration::from_secs(5),
    );
    AppState::spawn_terminal_reaper(app.clone(), Duration::from_secs(30));
    let handler = AppState::handler(app.clone());
    // Every session's transport events go two places: this daemon's stderr —
    // the record of truth on the device — and, best effort, the api, which
    // keeps one row per session for the admin's transport page. Both are
    // content-free: a session id, a word, a candidate type.
    let ledger = FanOutLedger::new(vec![
        Arc::new(StderrLedger),
        TransportReporter::start(
            &runtime.config.api_url,
            &identity.device_id,
            &identity.identity_private_key_b64,
        ),
    ]);
    // One intake for the life of the daemon: a session is minted once and
    // reachable from every carrier, so it outlives the relay socket it arrived
    // on. The peer's channels deliver through this same intake, so a session
    // reached over either wire is the one session.
    let intake = FrameIntake::with_ledger(handler, transport_keypair, ledger);
    // The peer transport a browser upgrades to. It is built last because it is
    // built from the intake, which runs the app's own handler.
    app.lock()
        .unwrap()
        .set_peer_factory(WebrtcPeerFactory::new(intake.clone()));

    // Reconnect with exponential backoff (2s → 30s cap) so a relay outage doesn't
    // become a tight reconnect loop hammering the server. A connection that lasted
    // long enough to be "clean" resets the delay, so a brief blip still recovers
    // fast. The policy lives in `Backoff` so it is unit-tested, not inline-and-hoped.
    let mut backoff = Backoff::new(Duration::from_secs(2), Duration::from_secs(30));
    loop {
        let connected_at = std::time::Instant::now();
        match relay::run(&runtime.device_url, &identity, intake.clone()).await {
            Ok(()) => eprintln!(
                "relay disconnected; reconnecting in {}s",
                backoff.current().as_secs()
            ),
            Err(e) => eprintln!(
                "relay error: {e}; reconnecting in {}s",
                backoff.current().as_secs()
            ),
        }
        backoff.note_session(connected_at.elapsed());
        tokio::time::sleep(backoff.current()).await;
        backoff.increase();
    }
}

fn exit_startup(error: String) -> ! {
    eprintln!("bridge: {error}");
    std::process::exit(1)
}

/// Install the launchd LaunchAgent (macOS). Refuses unless this device is
/// paired: the api must report it approved and owned by an account. The gate
/// runs BEFORE anything is written.
async fn install_service() {
    if !cfg!(target_os = "macos") {
        eprintln!("install-service writes a launchd LaunchAgent and is macOS-only");
        std::process::exit(2);
    }

    let cfg = bridge_config();
    let api_url = cfg.api_url.clone();
    let identity_path = cfg.identity_file.clone();

    // Gate: local identity + live api approval, never the local flag alone.
    let stored = match identity::load(&identity_path) {
        Ok(stored) => stored,
        Err(e) => {
            eprintln!("could not load identity from {identity_path:?}: {e}");
            std::process::exit(1);
        }
    };
    let api_status = match &stored {
        None => Err("no identity".to_string()),
        Some(stored) => {
            let client = reqwest::Client::new();
            pairing::fetch_status(&client, &api_url, &stored.device_id)
                .await
                .map_err(|e| e.to_string())
        }
    };
    let owner = match service::check_install_gate(
        stored.is_some(),
        api_status.as_ref().map_err(String::as_str),
    ) {
        Ok(owner) => owner,
        Err(gate) => {
            eprintln!("not installing: {gate}");
            std::process::exit(1);
        }
    };

    let home = std::path::PathBuf::from(std::env::var("HOME").expect("HOME is set"));
    let binary_path = std::env::current_exe().expect("current executable path");
    let log_dir = home.join(".build/log");
    std::fs::create_dir_all(&log_dir).expect("create log dir");

    // Carry every BRIDGE_* var set right now, and pin the two URLs and the
    // identity file to their resolved values so the daemon can't drift from
    // what the gate just verified.
    let mut daemon_env: Vec<(String, String)> = std::env::vars()
        .filter(|(key, _)| key.starts_with("BRIDGE_"))
        .collect();
    for (key, value) in [
        ("BRIDGE_API_URL", api_url.clone()),
        ("BRIDGE_RELAY_URL", cfg.relay_url.clone()),
        (
            "BRIDGE_IDENTITY_FILE",
            identity_path.to_string_lossy().into_owned(),
        ),
    ] {
        if !daemon_env.iter().any(|(k, _)| k == key) {
            daemon_env.push((key.to_string(), value));
        }
    }
    // launchd's bare PATH can't see `claude`; carry this shell's PATH.
    let mut daemon_env = service::with_install_path(daemon_env, || std::env::var("PATH").ok());
    daemon_env.sort();

    let config = service::ServiceConfig {
        binary_path,
        log_dir: log_dir.clone(),
        env: daemon_env,
    };
    let plist_file = service::plist_path(&home);
    std::fs::create_dir_all(plist_file.parent().expect("plist parent"))
        .expect("create LaunchAgents dir");
    std::fs::write(&plist_file, service::render_launchd_plist(&config)).expect("write plist");

    let uid = String::from_utf8(
        std::process::Command::new("id")
            .arg("-u")
            .output()
            .expect("id -u")
            .stdout,
    )
    .expect("uid utf8")
    .trim()
    .to_string();
    // Re-installs: boot the old instance out first; ignore "not loaded" errors.
    let _ = std::process::Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}/{}", service::SERVICE_LABEL)])
        .status();
    let bootstrap = std::process::Command::new("launchctl")
        .args([
            "bootstrap",
            &format!("gui/{uid}"),
            &plist_file.to_string_lossy(),
        ])
        .status()
        .expect("run launchctl bootstrap");
    if !bootstrap.success() {
        eprintln!("launchctl bootstrap failed (plist written to {plist_file:?})");
        std::process::exit(1);
    }
    println!(
        "installed {} for account owner {owner}\n  plist: {}\n  logs:  {}/bridge.log",
        service::SERVICE_LABEL,
        plist_file.display(),
        log_dir.display()
    );
}

/// Remove the LaunchAgent: stop the daemon and delete the plist.
fn uninstall_service() {
    if !cfg!(target_os = "macos") {
        eprintln!("uninstall-service is macOS-only");
        std::process::exit(2);
    }
    let home = std::path::PathBuf::from(std::env::var("HOME").expect("HOME is set"));
    let plist_file = service::plist_path(&home);
    let uid = String::from_utf8(
        std::process::Command::new("id")
            .arg("-u")
            .output()
            .expect("id -u")
            .stdout,
    )
    .expect("uid utf8")
    .trim()
    .to_string();
    let _ = std::process::Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}/{}", service::SERVICE_LABEL)])
        .status();
    match std::fs::remove_file(&plist_file) {
        Ok(()) => println!("removed {}", plist_file.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("nothing installed at {}", plist_file.display());
        }
        Err(e) => {
            eprintln!("could not remove {}: {e}", plist_file.display());
            std::process::exit(1);
        }
    }
}

/// Resolve the runtime config from BRIDGE_* env against $HOME.
fn bridge_config() -> build_bridge::config::BridgeConfig {
    let home = std::path::PathBuf::from(std::env::var("HOME").expect("HOME is set"));
    build_bridge::config::resolve(|key| std::env::var(key).ok(), &home)
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
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
/// (via the worktree's `.build/mcp.json`). It serves scoped conversation tools over
/// stdio and forwards each report to the running daemon's control socket
/// (`BRIDGE_MCP_SOCKET`) as `{"task_id","report"}` lines. The `task_id` field is
/// the opaque owner id (a plan id or a run id) the `--task` flag was launched
/// with; the daemon routes it by owner lookup to `on_plan_done` / `on_run_done`.
/// The wire key stays `task_id` for cross-version compatibility. Without the
/// socket it just logs (for testing).
fn mcp_stdio() {
    use std::io::{BufRead, Write};

    let args: Vec<String> = std::env::args().collect();
    let owner_id = args
        .iter()
        .position(|a| a == "--task")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());
    let socket = std::env::var("BRIDGE_MCP_SOCKET").ok();
    let session_token = std::env::var("BRIDGE_MCP_TOKEN").ok();

    // The owner id decides the tool surface: a router session's id is prefixed,
    // so which tools a harness is offered can never disagree with which kind of
    // session it is.
    let server = build_bridge::mcp::DoneServer::for_owner(&owner_id);
    let stdin = std::io::stdin().lock();
    let stdout = std::io::stdout().lock();
    let _ = server.run_stdio(
        stdin,
        stdout,
        |report| {
            let line = serde_json::json!({
                "task_id": owner_id,
                "session_token": session_token,
                "report": report
            })
            .to_string();
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
        },
        |action| {
            let Some(path) = &socket else {
                return Err("Build daemon socket is not configured".to_string());
            };
            let mut stream = std::os::unix::net::UnixStream::connect(path)
                .map_err(|error| format!("could not reach Build daemon: {error}"))?;
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .map_err(|error| format!("could not set daemon timeout: {error}"))?;
            let line = serde_json::json!({
                "task_id": owner_id,
                "session_token": session_token,
                "request": action
            })
            .to_string();
            writeln!(stream, "{line}")
                .map_err(|error| format!("could not send request: {error}"))?;
            let mut response = String::new();
            std::io::BufReader::new(stream)
                .read_line(&mut response)
                .map_err(|error| format!("could not read response: {error}"))?;
            let value: serde_json::Value = serde_json::from_str(&response)
                .map_err(|error| format!("invalid daemon response: {error}"))?;
            if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
                Ok(value
                    .get("result")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null))
            } else {
                Err(value
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Build daemon rejected the request")
                    .to_string())
            }
        },
    );
}
