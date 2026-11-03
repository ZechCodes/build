//! Disposable task-977 HTTP runtime for the real-client acceptance harness.
//! Uses typed constructors; never resolves daemon configuration or BRIDGE_*.
use build_bridge::{
    app::AppState,
    execution::ExecutionService,
    harness::HarnessContext,
    http::{settings::HttpSettings, supervisor::PreparedHttp},
    identity,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    hosted_origin: String,
    bridge_origin: String,
    run_dir: PathBuf,
}

fn loopback_port(origin: &str) -> Result<u16, String> {
    let url = reqwest::Url::parse(origin).map_err(|error| error.to_string())?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("qualification origins must be loopback HTTPS origins".into());
    }
    url.port()
        .filter(|port| *port >= 1024)
        .ok_or_else(|| "qualification requires an unprivileged explicit port".into())
}

fn read_fixture(path: &Path) -> Result<(Fixture, PathBuf), String> {
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    let run_dir = path
        .parent()
        .ok_or("fixture has no run directory")?
        .to_path_buf();
    let scratch = Path::new("/home/zech/Projects/.agent-scratch/977");
    if !run_dir.starts_with(scratch)
        || run_dir == scratch
        || path.file_name().is_none_or(|name| name != "runtime.json")
    {
        return Err("fixture must be runtime.json in disposable task-977 scratch".into());
    }
    let fixture: Fixture =
        serde_json::from_slice(&std::fs::read(path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    if fixture
        .run_dir
        .canonicalize()
        .map_err(|error| error.to_string())?
        != run_dir
    {
        return Err("fixture run directory does not match its location".into());
    }
    loopback_port(&fixture.hosted_origin)?;
    loopback_port(&fixture.bridge_origin)?;
    Ok((fixture, run_dir))
}

async fn run(path: &Path) -> Result<(), String> {
    let (fixture, run_dir) = read_fixture(path)?;
    let home = run_dir.join("home");
    let context = HarnessContext::resolved(home.join("unused.sock"), home.clone())
        .map_err(|error| error.to_string())?;
    let app = AppState::new_unrooted_configured(home.join("worktrees"), "main", true, context)
        .with_projects_dir_default(home.join("projects"))
        .with_config(home.join("config.json"))
        .map_err(|error| error.to_string())?
        .with_task_store(home.join("tasks"))?
        .shared();
    let handler = AppState::handler(app);
    handler
        .start_read_workers()
        .map_err(|error| error.to_string())?;
    let execution = ExecutionService::new(handler);
    let stored = identity::load(&home.join("identity.json"))
        .map_err(|error| error.to_string())?
        .ok_or("fixture identity is absent")?;
    let port = loopback_port(&fixture.bridge_origin)?;
    let settings = HttpSettings {
        bind: ([127, 0, 0, 1], port).into(),
        origin: fixture.bridge_origin,
        tls_cert: run_dir.join("cert.pem"),
        tls_key: run_dir.join("key.pem"),
        allowed_origins: Some(vec![fixture.hosted_origin.clone()]),
    };
    let prepared = PreparedHttp::prepare(
        settings,
        &stored,
        &fixture.hosted_origin,
        &fixture.hosted_origin,
        home.join("http-endpoint.json"),
        execution,
    )
    .await?;
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let mut serving = tokio::spawn(prepared.serve(stopped));
    let mut terminated = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|error| error.to_string())?;
    tokio::select! {
        result = &mut serving => return result.map_err(|error| error.to_string())?,
        _ = terminated.recv() => {},
        _ = tokio::signal::ctrl_c() => {},
    }
    let _ = stop.send(true);
    serving.await.map_err(|error| error.to_string())?
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    let Some(path) = std::env::args_os().nth(1) else {
        eprintln!("qualification requires an explicit disposable runtime.json");
        std::process::exit(2);
    };
    if let Err(error) = run(Path::new(&path)).await {
        eprintln!("qualification fixture failed: {error}");
        std::process::exit(1);
    }
}
