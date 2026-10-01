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
//!   signed web-push notifies fired when a task needs the human, their
//!   content sealed so the api sees only ids and ciphertext
//!   (default `https://getbuild.ing`)
//! - `BRIDGE_WEB_URL`     the web app base URL printed in the approve link (default = api url)
//! - `BRIDGE_DEVICE_NAME` device name shown during pairing (default: hostname)
//! - `BRIDGE_PAIRING_CODE` dev/compose only: pair with this fixed code instead of
//!   a random one, so a scripted approver can complete the flow
//! - `BRIDGE_ICE_POLICY` `all` (default) or `direct-only`: `direct-only` strips
//!   the browser's TURN servers and refuses relay candidates, for a bridge on
//!   the same LAN or Tailnet as the browser
//! - `BRIDGE_ICE_RELAY_MIN_WAIT_MS` how long a TURN pair waits before it may be
//!   accepted, so a slower direct pair can win (default 1500; `0` for no wait)
//! - `BRIDGE_ICE_INTERFACES` comma-separated interfaces to gather host
//!   candidates on, e.g. `tailscale0,eth0` (unset: every non-loopback one)
//!
//! `build-bridge pair` runs the pairing flow on its own — register, print the
//! pairing code, wait for the human to approve it in the web app, persist — then
//! asks the api which account owns the device and prints it. It is what an
//! installer runs before it installs the service, and it ends on the same gate
//! `install-service` starts with, so the two cannot disagree. A device seeded
//! with keys in the environment has no account to name: `pair` says so and
//! stops.
//!
//! `build-bridge install-service` installs the platform's own "keep this
//! running" unit: on macOS a launchd LaunchAgent, on Linux a systemd `--user`
//! unit. Either way it keeps `serve` running across crashes and logins, and it
//! is gated on pairing: the api must confirm this device is approved and owned
//! by an account before anything is written. `uninstall-service` removes it.

use std::sync::Arc;
use std::time::Duration;

use build_bridge::app::AppState;
use build_bridge::backoff::Backoff;
use build_bridge::carrier::FrameIntake;
use build_bridge::config::BridgeConfig;
use build_bridge::harness::HarnessContext;
use build_bridge::liveness::DedicatedRuntime;
use build_bridge::notify::Notifier;
use build_bridge::presence::PresenceReporter;
use build_bridge::priority::ChildPlacement;
use build_bridge::reachability::Reachability;
use build_bridge::relay::{self, DeviceIdentity};
use build_bridge::resume::{promote_live_roster, shut_down, LiveRoster};
use build_bridge::rtc::{IcePolicy, WebrtcPeerFactory};
use build_bridge::service::ServiceManager;
use build_bridge::transport_ledger::{FanOutLedger, StderrLedger, SummaryLedger};
use build_bridge::transport_report::TransportReporter;
use build_bridge::update::{UpdateConfig, UpdateService};
use build_bridge::{identity, pairing, service, transport};

#[tokio::main]
async fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("serve") | None => serve().await,
        Some("mcp") => mcp_stdio(),
        Some("provision") => provision(),
        Some("pair") => pair().await,
        Some("backup") => backup(),
        Some("install-service") => install_service().await,
        Some("uninstall-service") => uninstall_service(),
        Some("update-helper") => update_helper(),
        // One repository's Git state for the workspace reclaim service, read
        // in a process of its own so the service can kill it at its budget.
        Some("measure-git") => println!("{}", build_bridge::reclaim::git_reading_line()),
        Some("--version") | Some("-V") => {
            println!("build-bridge {}", env!("CARGO_PKG_VERSION"));
        }
        Some(other) => {
            eprintln!(
                "unknown command: {other}\nusage: build-bridge [serve|pair|backup <path>|provision|install-service|uninstall-service|update-helper <job-path>|--version]"
            );
            std::process::exit(2);
        }
    }
}

/// Run independently of the daemon while its executable is replaced and
/// restarted. The job file is written by the update backend before spawning.
fn update_helper() {
    let Some(job_path) = std::env::args().nth(2) else {
        eprintln!("usage: build-bridge update-helper <job-path>");
        std::process::exit(2);
    };
    if let Err(error) = build_bridge::update::installer::run_helper(std::path::Path::new(&job_path))
    {
        eprintln!("update helper: {error}");
        std::process::exit(1);
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
    let tasks_dir = env("BRIDGE_TASKS_DIR", &default_tasks_dir());
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
    place_children();
    // On the PATH just adopted, so the first catalog a client asks for is
    // what this machine's CLIs run. In the background; nothing waits on it.
    build_bridge::harness::installed::warm();
    // A power loss can remove the helper's transient service while leaving a
    // candidate that fails before full app construction. Relaunch recovery as
    // early as possible so that candidate cannot strand its prior binary.
    if let Err(error) = build_bridge::update::installer::ensure_recovery(&home_dir()) {
        eprintln!("bridge update recovery: {error}");
    }
    let runtime = match resolve_runtime_paths() {
        Ok(runtime) => runtime,
        Err(error) => exit_startup(error),
    };
    // How the identity came to be paired is `pair`'s business to report; serve
    // only needs the keys.
    let loaded = match load_device_identity(&runtime.config).await {
        Ok(loaded) => loaded,
        Err(error) => exit_startup(error),
    };
    let app = match construct_app(&runtime, &loaded.identity)
        .and_then(|app| configure_updates(app, &runtime))
    {
        Ok(app) => app,
        Err(error) => exit_startup(error),
    };
    run_daemon(runtime, loaded.identity, loaded.transport, app).await;
}

fn configure_updates(app: AppState, runtime: &RuntimePaths) -> Result<AppState, String> {
    let home = home_dir();
    let running_binary = std::env::current_exe()
        .map_err(|error| format!("cannot locate running bridge: {error}"))?;
    let managed_binary = build_bridge::update::provenance::managed_binary(&home, &running_binary);
    let platform_key = build_bridge::update::release::platform_key();
    let development_build = managed_binary.is_err() || platform_key.is_err();
    if let Err(reason) = &managed_binary {
        eprintln!("bridge updates: read only ({reason})");
    }
    if let Err(reason) = &platform_key {
        eprintln!("bridge updates: read only ({reason})");
    }
    let platform = platform_key
        .map(str::to_string)
        .unwrap_or_else(|_| format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH));
    let config = UpdateConfig {
        status_path: runtime.tasks_dir.join("bridge-update-status.json"),
        result_path: build_bridge::update::installer::result_path(&home),
        running_version: env!("CARGO_PKG_VERSION").to_string(),
        platform,
        development_build,
        check_interval: Duration::from_secs(24 * 60 * 60),
    };
    let backend = build_bridge::update::release::ProductionBackend::new(
        home,
        runtime.tasks_dir.clone(),
        managed_binary.unwrap_or(running_binary),
    );
    let service = UpdateService::new(config, Arc::new(backend))
        .map_err(|error| format!("cannot load bridge update state: {error}"))?;
    Ok(app.with_update_service(Arc::new(service)))
}

/// Decide where this daemon's children run (`crate::priority`): in transient
/// scopes of their own where the user's systemd answers, niced everywhere. Said
/// once at start, because it is the fact that explains every later `top`.
fn place_children() {
    let (placement, reason) = ChildPlacement::resolve(|key| std::env::var(key).ok());
    let scoped = placement.is_scoped();
    placement.install();
    if !scoped {
        eprintln!(
            "children: no scope ({reason}); agents nice +{}, terminals nice +{}, all at their gate",
            build_bridge::priority::CHILD_NICE,
            build_bridge::priority::TERMINAL_NICE_WITHOUT_SCOPE
        );
        return;
    }
    match build_bridge::priority::apply_slice_properties_for_this_boot(|key| {
        std::env::var(key).ok()
    }) {
        Ok(applied) => eprintln!(
            "children: {reason}, nice {}; {applied} (this boot)",
            build_bridge::priority::CHILD_NICE
        ),
        Err(error) => eprintln!(
            "children: {reason}, nice {}; slice properties not applied: {error}",
            build_bridge::priority::CHILD_NICE
        ),
    }
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
    /// How this bridge's ICE agent reaches a browser (`BRIDGE_ICE_*`).
    /// Resolved at startup so a mistyped value stops the daemon here rather
    /// than at the first offer.
    ice_policy: IcePolicy,
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
    // Placeholders and completed checkouts must hash the same canonical root.
    std::fs::create_dir_all(&config.worktrees).map_err(|error| {
        format!(
            "cannot create worktrees root {}: {error}",
            config.worktrees.display()
        )
    })?;
    let worktrees = std::fs::canonicalize(&config.worktrees)
        .map_err(|error| {
            format!(
                "cannot resolve worktrees root {}: {error}",
                config.worktrees.display()
            )
        })?
        .to_string_lossy()
        .into_owned();
    let device_url = format!("{}/ws/device", config.relay_url.trim_end_matches('/'));
    let qa_agent = matches!(
        std::env::var("BRIDGE_QA_AGENT").as_deref(),
        Ok("1") | Ok("true")
    );
    let home = home_dir();
    let tasks_dir = std::path::PathBuf::from(env("BRIDGE_TASKS_DIR", &default_tasks_dir()));
    let state_root = tasks_dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let harness_context = HarnessContext::resolved(config.mcp_socket.clone(), state_root.into())
        .map_err(|error| format!("cannot resolve harness runtime: {error}"))?;
    let ice_policy = IcePolicy::resolve(|key| std::env::var(key).ok())?;
    Ok(RuntimePaths {
        ice_policy,
        device_url,
        worktrees,
        config_path: env(
            "BRIDGE_CONFIG",
            &format!("{}/.build/config.json", home.display()),
        )
        .into(),
        mcp_socket: config.mcp_socket.to_string_lossy().into_owned(),
        config,
        tasks_dir,
        harness_context,
        qa_agent,
    })
}

/// What `load_device_identity` had to do to produce the identity it returns.
/// It is the one place that reads the stored identity, so it is the one place
/// that knows; callers report it instead of loading the file again to guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PairingOutcome {
    /// The keys came from the environment (a provisioned/seeded device). Nothing
    /// was paired: that identity is treated as approved by construction.
    Provisioned,
    /// The stored identity was already approved, so `ensure_paired` returned it
    /// untouched.
    AlreadyApproved,
    /// A human approved this device during this run.
    JustApproved,
}

impl PairingOutcome {
    /// The outcome for a stored identity, read from the same `approved` flag
    /// `pairing::ensure_paired` decides on.
    fn for_stored(approved: bool) -> Self {
        if approved {
            Self::AlreadyApproved
        } else {
            Self::JustApproved
        }
    }

    /// The line `pair` ends on once the gate names the owning account.
    fn paired_to(self, owner: &str) -> String {
        match self {
            Self::AlreadyApproved => format!("already paired to account {owner}"),
            Self::Provisioned | Self::JustApproved => format!("paired to account {owner}"),
        }
    }
}

/// What `load_device_identity` produced: the keys the daemon runs on, and how
/// they came to be paired. `outcome` is decided from the single load the
/// function already performs, so `pair` can report it without re-reading the
/// identity file behind that function's back.
struct LoadedIdentity {
    identity: DeviceIdentity,
    transport: transport::KeyPairB64,
    outcome: PairingOutcome,
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
async fn load_device_identity(config: &BridgeConfig) -> Result<LoadedIdentity, String> {
    match provisioned_identity() {
        Some(provisioned) => Ok(provisioned),
        None => load_stored_identity(config).await,
    }
}

/// The file-backed identity, minted on first use, paired before it returns.
async fn load_stored_identity(config: &BridgeConfig) -> Result<LoadedIdentity, String> {
    let identity_path = &config.identity_file;
    let stored = identity::load_or_generate(identity_path, &identity::default_device_name())
        .map_err(|error| {
            format!("could not load or create identity at {identity_path:?}: {error}")
        })?;
    let outcome = PairingOutcome::for_stored(stored.approved);
    let pairing_code_override = std::env::var("BRIDGE_PAIRING_CODE").ok();
    let client = reqwest::Client::new();
    let approved = pairing::ensure_paired(
        &client,
        &config.api_url,
        &config.web_url,
        identity_path,
        stored,
        Duration::from_secs(2),
        pairing_code_override.as_deref(),
    )
    .await
    .map_err(|error| format!("pairing failed: {error}"))?;
    Ok(LoadedIdentity {
        identity: identity::to_device_identity(&approved),
        transport: approved.transport.clone(),
        outcome,
    })
}

/// The identity seeded into the environment, if all three keys are there.
fn provisioned_identity() -> Option<LoadedIdentity> {
    let id_priv = std::env::var("BRIDGE_IDENTITY_PRIV").ok()?;
    let tp_priv = std::env::var("BRIDGE_TRANSPORT_PRIV").ok()?;
    let tp_pub = std::env::var("BRIDGE_TRANSPORT_PUB").ok()?;
    Some(LoadedIdentity {
        identity: DeviceIdentity {
            device_id: env("BRIDGE_DEVICE_ID", "bridge-dev"),
            identity_private_key_b64: id_priv,
        },
        transport: transport::KeyPairB64 {
            public_key_b64: tp_pub,
            private_key_b64: tp_priv,
        },
        outcome: PairingOutcome::Provisioned,
    })
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
    let app = register_environment_projects(app, &config.base_branch);
    let app = match std::env::var_os("BRIDGE_PROJECTS_DIR") {
        Some(dir) => app.with_projects_dir_default(std::path::PathBuf::from(dir)),
        None => app,
    }
    .with_config(&runtime.config_path)
    .map_err(|error| format!("cannot load config: {error}"))?;
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
    // A live roster the last run left and no clean shutdown replaced is the
    // roster this boot resumes from; it is renamed into place before the new
    // run's live roster can overwrite it.
    let tasks_dir = &runtime.tasks_dir;
    if promote_live_roster(tasks_dir) {
        eprintln!(
            "resume: the last run ended without a clean shutdown; resuming from its live roster"
        );
    }
    let live_roster = LiveRoster::start(tasks_dir, env!("CARGO_PKG_VERSION"));
    // What the bridge pushes to its clients — the change bus's flush, every
    // terminal's paint — is serialized and encrypted per client, and runs on a
    // runtime of its own, apart from the one that answers their requests
    // (task #131).
    let push = match DedicatedRuntime::push() {
        Ok(push) => push,
        Err(error) => exit_startup(error),
    };
    let app = app
        .with_live_roster(live_roster.clone())
        .with_push_runtime(push.handle())
        .shared();
    let handler = AppState::handler(app.clone());
    // Every session's transport events go two places: this daemon's stderr —
    // the record of truth on the device — and, best effort, the api, which
    // keeps one row per session for the admin's transport page. Both are
    // content-free: a session id, a word, a candidate type.
    let ledger = FanOutLedger::new(vec![
        Arc::new(StderrLedger),
        SummaryLedger::new(),
        TransportReporter::start(
            &runtime.config.api_url,
            &identity.device_id,
            &identity.identity_private_key_b64,
        ),
    ]);
    // Presence is the api's (`planning/v2/Strict P2P Transport Spec.md` rule 6):
    // this daemon says so itself, signed, every 30 s, and the relay reports
    // nothing about any device. What it says is that this device can be
    // REACHED, which is the question `online` is read as answering: the beat
    // goes out only while the relay loop below holds an authenticated socket,
    // because that socket is the only way in. A bridge that is running, and can
    // still reach the api, and has no relay socket is a bridge no browser can
    // open a session to — and saying "online" for it is what had this account
    // dialling two machines that were never going to answer.
    let reachable = Reachability::unreachable();
    spawn_update_heartbeat(home_dir(), app.clone());
    // The service manager's stderr file, rotated rather than left to grow.
    build_bridge::logfile::spawn_rotation();
    // The runtime the relay socket, the presence beat and every peer's
    // channels run on: threads that never take the app lock, so a handler
    // holding it for a minute slows answers and severs nothing (task #128).
    let liveness = match DedicatedRuntime::liveness() {
        Ok(liveness) => liveness,
        Err(error) => exit_startup(error),
    };
    let _presence_beats = {
        let _on_liveness = liveness.handle().enter();
        PresenceReporter::start(&runtime.config.api_url, &identity, &reachable)
    };
    // One intake for the life of the daemon: a session is minted once and
    // reachable from every carrier, so it outlives the relay socket it arrived
    // on. The peer's channels deliver through this same intake, so a session
    // reached over either wire is the one session.
    let intake = FrameIntake::with_ledger(handler, transport_keypair, ledger);
    // The peer transport a browser upgrades to. It is built last because it is
    // built from the intake, which runs the app's own handler.
    app.lock().unwrap().set_peer_factory_on(
        WebrtcPeerFactory::new(intake.clone(), runtime.ice_policy.clone()),
        Some(liveness.handle()),
    );

    // While a candidate binary is on probation, its task-store copy may be
    // restored. Keep it off the relay until the helper commits: no browser or
    // agent can create work in state that might be discarded. The local health
    // beat still proves the daemon initialized and its app lock is responsive.
    wait_for_update_probation(&home_dir()).await;
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
    AppState::spawn_update_checks(app.clone(), Duration::from_secs(5));
    // Measures every workspace and tells the project agent about quiet ones
    // (#135). It never removes a workspace; `workspace.reclaim` does. It drops
    // build output only when BRIDGE_WORKSPACE_PRUNE is on.
    let reclaim_stop = AppState::spawn_workspace_reclaim(
        app.clone(),
        build_bridge::reclaim::ReclaimPolicy::from_env(),
    )
    .await;
    // Keeps each source's base branch in step with its remote, fast-forward
    // only, for the sources that have it on (#267).
    let source_sync_stop =
        AppState::spawn_source_sync(app.clone(), build_bridge::app::SourceSyncPolicy::default())
            .await;

    // Walks a workspace's size on disk when the Workspaces tab asks (#273).
    AppState::spawn_workspace_sizes(app.clone());

    // Bring back whoever the last shutdown was holding. It waits for an
    // authenticated relay socket rather than firing here, because a resumed
    // agent starts talking immediately and the human has to be able to SEE it:
    // a bridge no browser can reach is one whose agents work in the dark.
    spawn_resume_after_restart(app.clone(), runtime.tasks_dir.clone(), reachable.clone());

    // The relay socket, redialled for as long as the daemon runs, on the
    // liveness runtime. The main thread waits for the signal that ends the
    // daemon; the socket task is aborted then, which is the socket generation
    // ending the way every other end does (`relay::RelayConnection`'s drop).
    let mut relay_socket = liveness.spawn(relay_forever(
        runtime.device_url.clone(),
        identity.clone(),
        intake.clone(),
        reachable.clone(),
    ));
    let mut going_down = shutdown_signals();
    tokio::select! {
        // Biased so a SIGTERM that lands while the relay task is also ready
        // is still the branch taken: systemd is about to send SIGKILL, and
        // one more reconnect is worth nothing next to the roster.
        biased;
        () = going_down.recv() => {}
        _ = &mut relay_socket => {}
    }
    // The one exit the daemon has, whichever way the loop ended: record who was
    // working before the harnesses go with the process. Rolling the binary
    // kills every session on the device at once, and nothing but this says so.
    // Cancellation is a held atomic handle: shutdown never waits on the app mutex.
    reclaim_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    source_sync_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    // It writes from the live roster's newest lists and never waits on the app
    // mutex, so a handler holding that cannot keep it past `TimeoutStopSec`.
    shut_down(&live_roster, || {
        // Then the children: in scopes of their own they are no longer in
        // this unit's cgroup for systemd to end, so they are ended here (and
        // by the `BindsTo=` each scope carries, should this not run).
        if let Some(stopped) = build_bridge::priority::stop_children() {
            eprintln!("{stopped}");
        }
        // Then the transport: the socket task first, so the relay sees a
        // close rather than a silence, then the runtime it ran on — stopped,
        // not dropped, because this is a task of the main runtime
        // (`liveness.rs`).
        relay_socket.abort();
        liveness.stop();
        push.stop();
    });
}

/// Hold a relay socket open, and redial whenever it ends.
///
/// Reconnect with exponential backoff (2s → 30s cap) so a relay outage doesn't
/// become a tight reconnect loop hammering the server. A connection that lasted
/// long enough to be "clean" resets the delay, so a brief blip still recovers
/// fast. The policy lives in `Backoff` so it is unit-tested, not inline-and-hoped.
async fn relay_forever(
    device_url: String,
    identity: DeviceIdentity,
    intake: Arc<FrameIntake>,
    reachable: Reachability,
) {
    let mut backoff = Backoff::new(Duration::from_secs(2), Duration::from_secs(30));
    loop {
        let connected_at = std::time::Instant::now();
        let outcome = relay::run(&device_url, &identity, intake.clone(), &reachable).await;
        backoff.note_session(connected_at.elapsed());
        let wait = relay::redial_wait(&outcome, &mut backoff);
        relay::say_redial(&outcome, wait);
        tokio::time::sleep(wait).await;
    }
}

/// A receiver that fires once the operating system asks this daemon to stop.
///
/// SIGTERM is what `systemctl stop` sends and so what a roll sends; SIGINT is
/// what a terminal sends, and a developer's Ctrl-C should record a roster for
/// the same reason. A platform whose handlers cannot be installed gets a
/// receiver that never fires — the daemon behaves exactly as it did before any
/// of this, rather than refusing to start over a signal.
fn shutdown_signals() -> ShutdownSignals {
    use tokio::signal::unix::{signal, SignalKind};
    ShutdownSignals {
        term: signal(SignalKind::terminate()).ok(),
        interrupt: signal(SignalKind::interrupt()).ok(),
    }
}

struct ShutdownSignals {
    term: Option<tokio::signal::unix::Signal>,
    interrupt: Option<tokio::signal::unix::Signal>,
}

impl ShutdownSignals {
    /// Resolve on the first of either signal. Cancel-safe, because it is
    /// polled inside a `select!` that loses the race every time the relay wins
    /// one — `Signal::recv` is itself cancel-safe, and a pending signal is
    /// still pending on the next poll.
    async fn recv(&mut self) {
        match (&mut self.term, &mut self.interrupt) {
            (Some(term), Some(interrupt)) => {
                tokio::select! {
                    _ = term.recv() => {}
                    _ = interrupt.recv() => {}
                }
            }
            (Some(one), None) | (None, Some(one)) => {
                one.recv().await;
            }
            (None, None) => std::future::pending().await,
        }
    }
}

/// Resume once, as soon as a browser could see it happen.
///
/// The store is already open — `construct_app` would have refused to start
/// otherwise — so the only thing left to wait for is the relay, and the wait is
/// bounded: a device that cannot reach its relay tonight still has agents that
/// were cut off, and leaving them down until it can would be the outage this
/// exists to end. It polls rather than takes a callback because
/// `Reachability` is the one thing both the relay loop and the presence beats
/// already agree on.
fn spawn_resume_after_restart(
    app: std::sync::Arc<std::sync::Mutex<AppState>>,
    tasks_dir: std::path::PathBuf,
    reachable: Reachability,
) {
    const POLL: Duration = Duration::from_millis(250);
    const WAIT_FOR_RELAY: Duration = Duration::from_secs(60);
    tokio::spawn(async move {
        let deadline = std::time::Instant::now() + WAIT_FOR_RELAY;
        while !reachable.is_reachable() && std::time::Instant::now() < deadline {
            tokio::time::sleep(POLL).await;
        }
        if !reachable.is_reachable() {
            eprintln!(
                "resume: no relay socket after {}s; resuming anyway",
                WAIT_FOR_RELAY.as_secs()
            );
        }
        // Read on the blocking pool: no runtime worker waits on the app mutex.
        let reading = app.clone();
        let admission = tokio::task::spawn_blocking(move || {
            reading
                .lock()
                .unwrap()
                .update_service()
                .map(|service| service.admission())
        })
        .await
        .expect("reading the update service panicked");
        // An unfinished helper may keep the gate closed beyond the relay
        // wait. Hold admission across the roster read so an idle handoff
        // cannot begin between the wake and the resumed turns.
        let lease = match admission {
            Some(gate) => Some(gate.enter_when_open().await),
            None => None,
        };
        if let Err(joined) = tokio::task::spawn_blocking(move || {
            let _lease = lease;
            AppState::resume_after_restart(&app, &tasks_dir, env!("CARGO_PKG_VERSION"));
        })
        .await
        {
            eprintln!("resume: startup task failed: {joined}");
        }
    });
}

async fn wait_for_update_probation(home: &std::path::Path) {
    while build_bridge::update::installer::probation_active(home) {
        if let Err(error) = build_bridge::update::installer::ensure_recovery(home) {
            eprintln!("bridge update recovery: {error}");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn spawn_update_heartbeat(
    home: std::path::PathBuf,
    app: std::sync::Arc<std::sync::Mutex<AppState>>,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            interval.tick().await;
            // The helper requires a fresh beat from a fully initialized
            // daemon whose app state remains responsive. The relay is held
            // closed until this probation ends, so it is not a health input.
            if app.try_lock().is_ok() {
                if let Err(error) =
                    build_bridge::update::installer::heartbeat(&home, env!("CARGO_PKG_VERSION"))
                {
                    eprintln!("bridge update heartbeat: {error}");
                }
            }
        }
    });
}

fn exit_startup(error: String) -> ! {
    eprintln!("bridge: {error}");
    std::process::exit(1)
}

/// Pair this device to an account and stop. Registers the identity, prints the
/// pairing code + fingerprint + approve link, and waits for a human to approve
/// it in the web app; an identity the api still approves skips all of that.
/// This is the pairing half of a first install, on its own, so an installer can
/// run it and then `install-service`.
///
/// An identity stored as approved that the api no longer approves (revoked in
/// Settings → Devices, or unknown to it) is retired first, so this run pairs a
/// new identity instead of dead-ending on a code that does not exist (#317).
///
/// It ends on the same gate `install-service` runs — one status GET, even for an
/// identity that was already approved — so the account it names is the account
/// the api will name, and an installer that gets past `pair` cannot then be
/// refused by `install-service`. A device whose keys came from the environment
/// is the exception: it is approved by construction and owned by no account, so
/// there is nothing to pair and nothing to ask.
async fn pair() {
    // Pairing talks https before anything else does; the provider must be in
    // place first.
    relay::install_crypto_provider();
    match pair_device(&bridge_config(), provisioned_identity()).await {
        Ok(line) => println!("    {line}"),
        Err(reason) => {
            eprintln!("{}", pairing::wrapped(&format!("not paired: {reason}"), "    "));
            std::process::exit(1);
        }
    }
}

/// `pair`, in order, returning the line it ends on or why it refused: a
/// provisioned identity returns before anything is asked; a stored approval
/// the api dropped is retired before the identity is loaded, so the load
/// mints a new device.
async fn pair_device(
    cfg: &BridgeConfig,
    provisioned: Option<LoadedIdentity>,
) -> Result<String, String> {
    if let Some(provisioned) = provisioned {
        return Ok(format!(
            "provisioned device {} — nothing to pair",
            provisioned.identity.device_id
        ));
    }
    let retired = pairing::retire_lapsed_approval(
        &pairing::status_client(),
        &cfg.api_url,
        &cfg.identity_file,
    )
    .await
    .map_err(|error| service::InstallGateError::from(error).to_string())?;
    if let Some(retired) = retired {
        eprintln!("{}", retired.notice(&cfg.home));
    }
    let loaded = load_stored_identity(cfg).await?;
    let owner = approved_owner(cfg).await?;
    Ok(loaded.outcome.paired_to(&owner))
}

/// Install the platform's service unit — a launchd LaunchAgent on macOS, a
/// systemd `--user` unit on Linux. Refuses unless this device is paired: the
/// api must report it approved and owned by an account. The gate runs BEFORE
/// anything is written.
async fn install_service() {
    let cfg = bridge_config();
    let owner = match approved_owner(&cfg).await {
        Ok(owner) => owner,
        Err(reason) => {
            eprintln!("not installing: {reason}");
            std::process::exit(1);
        }
    };
    let manager = manager_or_exit();

    let home = home_dir();
    let log_dir = home.join(".build/log");
    std::fs::create_dir_all(&log_dir).expect("create log dir");
    let config = service::ServiceConfig {
        binary_path: std::env::current_exe().expect("current executable path"),
        log_dir: log_dir.clone(),
        env: daemon_environment(&cfg),
    };

    match service::install(&*manager, &service_context(home), &config, &mut run_shell) {
        Ok(unit_path) => {
            println!(
                "installed {} for account owner {owner}\n  unit: {}\n  logs: {}/bridge.log",
                manager.name(),
                unit_path.display(),
                log_dir.display()
            );
            if let Some(hint) = manager.after_install_hint() {
                println!("  {hint}");
            }
        }
        Err(error) => {
            eprintln!("install failed: {error}");
            std::process::exit(1);
        }
    }
}

/// Remove the service unit: stop the daemon, then delete the file.
fn uninstall_service() {
    let manager = manager_or_exit();
    match service::uninstall(&*manager, &service_context(home_dir()), &mut run_shell) {
        Ok(service::Uninstalled::Removed(path)) => println!("removed {}", path.display()),
        Ok(service::Uninstalled::NothingInstalled(path)) => {
            println!("nothing installed at {}", path.display());
        }
        Err(error) => {
            eprintln!("could not remove the {}: {error}", manager.name());
            std::process::exit(1);
        }
    }
    if let Some(note) = kept_identity_note(&bridge_config().identity_file) {
        println!("{note}");
    }
}

/// What uninstalling says about the device identity, which it keeps: the
/// account still lists this device, so deleting the keys would strand that
/// entry, and a later `pair` either finds it still approved or retires it and
/// pairs anew. `None` when there is no identity to speak of.
fn kept_identity_note(identity_file: &std::path::Path) -> Option<String> {
    identity_file.exists().then(|| {
        format!(
            "kept this device's identity at {} — `build-bridge pair` reuses it while your \
             account still lists this device, and pairs a new one if it was revoked; \
             revoke it in Settings → Devices to retire it",
            identity_file.display()
        )
    })
}

/// The install gate: a local identity AND a live api confirmation that this
/// device is approved and owned by an account, never the local flag alone.
/// `Err` is the reason, in the operator's words and with no prefix — the caller
/// says what it is refusing to do, because `pair` and `install-service` refuse
/// different things on the same answer.
async fn approved_owner(cfg: &BridgeConfig) -> Result<String, String> {
    let identity_path = &cfg.identity_file;
    let stored = identity::load(identity_path)
        .map_err(|error| format!("could not load identity from {identity_path:?}: {error}"))?;
    let api_status = match &stored {
        None => Err("no identity".to_string()),
        Some(stored) => {
            let client = pairing::status_client();
            pairing::fetch_status(&client, &cfg.api_url, &stored.device_id)
                .await
                .map_err(|error| error.to_string())
        }
    };
    service::check_install_gate(stored.as_ref(), api_status.as_ref().map_err(String::as_str))
        .map_err(|gate| gate.to_string())
}

/// The one platform decision in the crate, and the one place it is refused.
fn manager_or_exit() -> Box<dyn ServiceManager> {
    match service::manager_for(std::env::consts::OS) {
        Some(manager) => manager,
        None => {
            eprintln!("install-service supports macOS (launchd) and Linux (systemd --user)");
            std::process::exit(2);
        }
    }
}

/// What the daemon starts with: every BRIDGE_* var set right now, minus the
/// device's key material, plus the two URLs and the identity file pinned to
/// their resolved values so the daemon can't drift from what the gate just
/// verified, plus the installing shell's PATH. A validated release-repository
/// override follows the service so daily checks use the same source.
fn daemon_environment(cfg: &BridgeConfig) -> Vec<(String, String)> {
    let inherited = std::env::vars().filter(|(key, value)| {
        if key.starts_with("BRIDGE_") {
            return true;
        }
        if matches!(key.as_str(), "BUILD_RELEASES_REPO" | "RELEASES_REPO") {
            if valid_release_repo(value) {
                return true;
            }
            eprintln!("ignoring invalid {key} for installed bridge service");
        }
        false
    });
    service_environment(inherited.collect(), cfg, || std::env::var("PATH").ok())
}

fn valid_release_repo(value: &str) -> bool {
    let Some((owner, repo)) = value.split_once('/') else {
        return false;
    };
    [owner, repo].into_iter().all(|part| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    })
}

/// The rule `daemon_environment` applies to the variables it collected, apart
/// from the process that supplies them. Sorted, so re-installing the same setup
/// writes the same unit.
fn service_environment(
    env: Vec<(String, String)>,
    cfg: &BridgeConfig,
    path: impl FnOnce() -> Option<String>,
) -> Vec<(String, String)> {
    let mut env = service::without_device_keys(env);
    for (key, value) in [
        ("BRIDGE_API_URL", cfg.api_url.clone()),
        ("BRIDGE_RELAY_URL", cfg.relay_url.clone()),
        (
            "BRIDGE_IDENTITY_FILE",
            cfg.identity_file.to_string_lossy().into_owned(),
        ),
    ] {
        if !env.iter().any(|(existing, _)| existing == key) {
            env.push((key.to_string(), value));
        }
    }
    let mut env = service::with_install_path(env, path);
    env.sort();
    env
}

fn service_context(home: std::path::PathBuf) -> service::ServiceContext {
    service::ServiceContext {
        home,
        uid: current_uid(),
    }
}

/// The installing user's numeric id — launchd addresses their gui domain by it.
fn current_uid() -> String {
    String::from_utf8(
        std::process::Command::new("id")
            .arg("-u")
            .output()
            .expect("id -u")
            .stdout,
    )
    .expect("uid utf8")
    .trim()
    .to_string()
}

/// The one place the bridge spawns a service-manager command. `Ok(false)` is
/// "it ran and said no"; `Err` is "it could not be run at all".
fn run_shell(command: &service::ShellCommand) -> std::io::Result<bool> {
    std::process::Command::new(command.program)
        .args(&command.args)
        .status()
        .map(|status| status.success())
}

fn home_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").expect("HOME is set"))
}

/// Where the state dir lives unless `BRIDGE_TASKS_DIR` says otherwise. `serve`
/// and `backup` both resolve it and must agree, or a backup copies a database
/// the daemon never wrote.
fn default_tasks_dir() -> String {
    format!("{}/.build/tasks", home_dir().display())
}

/// Resolve the runtime config from BRIDGE_* env against $HOME.
fn bridge_config() -> build_bridge::config::BridgeConfig {
    build_bridge::config::resolve(|key| std::env::var(key).ok(), &home_dir())
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
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
    // and so is a project agent's, so which tools a harness is offered can never
    // disagree with which kind of session it is.
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

#[cfg(test)]
mod tests {
    use super::{
        kept_identity_note, pair_device, service_environment, valid_release_repo, LoadedIdentity,
        PairingOutcome,
    };
    use build_bridge::{config, identity, relay::DeviceIdentity, transport};
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn release_repo_override_is_one_safe_owner_and_repo() {
        assert!(valid_release_repo("ZechCodes/build-releases"));
        assert!(!valid_release_repo("ZechCodes/build-releases/other"));
        assert!(!valid_release_repo("../build-releases"));
        assert!(!valid_release_repo("ZechCodes/repo\nExecStart=/other"));
    }

    /// A config as `install-service` resolves one, from no environment at all.
    fn config() -> build_bridge::config::BridgeConfig {
        build_bridge::config::resolve(|_| None, std::path::Path::new("/home/dev"))
    }

    /// The unit file is a copy of this environment that outlives the installing
    /// shell, so a seeded shell's key material must not travel into it.
    #[test]
    fn the_installed_daemon_inherits_no_device_key_material() {
        let env = service_environment(
            vec![
                ("BRIDGE_DEVICE_ID".into(), "device-7".into()),
                ("BRIDGE_IDENTITY_PRIV".into(), "ed25519-private".into()),
                ("BRIDGE_TRANSPORT_PRIV".into(), "x25519-private".into()),
                ("BRIDGE_TRANSPORT_PUB".into(), "x25519-public".into()),
            ],
            &config(),
            || None,
        );

        assert_eq!(
            env.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(),
            vec![
                "BRIDGE_API_URL",
                "BRIDGE_DEVICE_ID",
                "BRIDGE_IDENTITY_FILE",
                "BRIDGE_RELAY_URL",
            ]
        );
    }

    /// The daemon is pinned to what the gate just verified, so it cannot drift
    /// to another api, relay or identity file after the install.
    #[test]
    fn the_installed_daemon_is_pinned_to_the_resolved_config() {
        let cfg = config();
        let env = service_environment(vec![], &cfg, || None);

        assert_eq!(
            env,
            vec![
                ("BRIDGE_API_URL".to_string(), cfg.api_url.clone()),
                (
                    "BRIDGE_IDENTITY_FILE".to_string(),
                    cfg.identity_file.to_string_lossy().into_owned()
                ),
                ("BRIDGE_RELAY_URL".to_string(), cfg.relay_url.clone()),
            ]
        );
    }

    /// `service::manager_for` is the crate's one platform branch: it turns
    /// `std::env::consts::OS` into a `ServiceManager`, and everything above it
    /// is platform-blind. A compile-time platform check in this file would be a
    /// second dispatch, and the next platform would become a two-place change.
    #[test]
    fn main_dispatches_on_the_platform_in_exactly_one_place() {
        let compile_time_platform_check = concat!("target", "_os");
        let source = include_str!("main.rs");
        assert!(
            !source.contains(compile_time_platform_check),
            "main.rs must leave the platform to service::manager_for"
        );
        assert!(
            source.contains("service::manager_for(std::env::consts::OS)"),
            "and it must ask service::manager_for, so the two stay one decision"
        );
    }

    /// `$HOME` is one fact with one fallback rule, so it is read in one place:
    /// every caller that wants the home directory asks `home_dir()`. A second
    /// read is a second rule, and the two that used to exist disagreed about a
    /// missing HOME — an empty string in `backup`, `.` in `resolve_runtime_paths`
    /// — each quietly putting the bridge's state somewhere nobody asked for.
    #[test]
    fn home_is_read_in_exactly_one_place() {
        // Split so this assertion is not itself an occurrence.
        let read_home = concat!("std::env::var(\"HOME", "\")");
        let source = include_str!("main.rs");
        assert_eq!(
            source.matches(read_home).count(),
            1,
            "resolve HOME through home_dir(), not a second copy of the expression"
        );
    }

    #[test]
    fn a_stored_identity_already_approved_needed_no_pairing() {
        assert_eq!(
            PairingOutcome::for_stored(true),
            PairingOutcome::AlreadyApproved
        );
    }

    #[test]
    fn a_stored_identity_awaiting_approval_was_paired_by_this_run() {
        assert_eq!(
            PairingOutcome::for_stored(false),
            PairingOutcome::JustApproved
        );
    }

    /// `pair` ends on one line naming the account, and says "already" only
    /// for a pairing it found rather than made; it never says "already
    /// paired" before a gate that may still refuse (#317).
    #[test]
    fn pair_ends_on_one_line_naming_the_account() {
        assert_eq!(
            PairingOutcome::AlreadyApproved.paired_to("user-1"),
            "already paired to account user-1"
        );
        assert_eq!(
            PairingOutcome::JustApproved.paired_to("user-1"),
            "paired to account user-1"
        );
    }

    /// Uninstalling the service keeps the identity, and says where and what
    /// pairing will do with it; with no identity there is nothing to say.
    #[test]
    fn uninstall_says_it_kept_the_identity_and_what_pairing_does_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identity.json");
        assert_eq!(kept_identity_note(&path), None);
        std::fs::write(&path, "{}").unwrap();
        let note = kept_identity_note(&path).expect("an identity to speak of");
        assert!(note.contains(&path.display().to_string()), "{note}");
        assert!(note.contains("`build-bridge pair`"), "{note}");
        assert!(note.contains("revoked"), "{note}");
    }

    fn config_for(api_url: &str, home: &std::path::Path) -> config::BridgeConfig {
        let identity_file = home.join("identity.json");
        config::resolve(
            |key| match key {
                "BRIDGE_API_URL" => Some(api_url.to_string()),
                "BRIDGE_IDENTITY_FILE" => Some(identity_file.display().to_string()),
                _ => None,
            },
            home,
        )
    }

    fn stored_approved(path: &std::path::Path) -> identity::StoredIdentity {
        let mut stored = identity::generate("my-box");
        stored.approved = true;
        identity::save(path, &stored).unwrap();
        stored
    }

    /// The order `pair` depends on (#317): the revoked approval is retired
    /// before the identity is loaded, so the load mints a new device, which
    /// pairs and ends on the account line.
    #[tokio::test]
    async fn pair_retires_a_revoked_identity_before_loading_and_pairs_a_new_one() {
        let server = MockServer::start().await;
        let home = tempfile::tempdir().unwrap();
        let cfg = config_for(&server.uri(), home.path());
        let old = stored_approved(&cfg.identity_file);
        Mock::given(method("GET"))
            .and(path(format!("/api/devices/{}/status", old.device_id)))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"approved": false, "owner_user_id": null, "state": "revoked"}),
            ))
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/devices/register"))
            .respond_with(ResponseTemplate::new(201))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/api/devices/.+/status$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"approved": true, "owner_user_id": "u1", "state": "approved"}),
            ))
            .with_priority(2)
            .mount(&server)
            .await;

        let line = pair_device(&cfg, None).await.expect("pairs a new device");

        assert_eq!(line, "paired to account u1");
        let new = identity::load(&cfg.identity_file).unwrap().unwrap();
        assert_ne!(new.device_id, old.device_id);
        assert!(new.approved);
        let kept = home
            .path()
            .join(format!("identity.json.retired-{}", old.device_id));
        assert_eq!(identity::load(&kept).unwrap(), Some(old));
    }

    /// A provisioned device returns before anything is asked or retired.
    #[tokio::test]
    async fn pair_with_a_provisioned_identity_asks_nothing_and_touches_no_file() {
        let server = MockServer::start().await;
        let home = tempfile::tempdir().unwrap();
        let cfg = config_for(&server.uri(), home.path());
        let stored = stored_approved(&cfg.identity_file);
        let provisioned = LoadedIdentity {
            identity: DeviceIdentity {
                device_id: "bridge-dev".into(),
                identity_private_key_b64: stored.identity_private_key_b64.clone(),
            },
            transport: transport::generate_transport_keypair(),
            outcome: PairingOutcome::Provisioned,
        };

        let line = pair_device(&cfg, Some(provisioned)).await.unwrap();

        assert_eq!(line, "provisioned device bridge-dev — nothing to pair");
        assert_eq!(server.received_requests().await.unwrap().len(), 0);
        assert_eq!(identity::load(&cfg.identity_file).unwrap(), Some(stored));
    }
}
