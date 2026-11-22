//! Mount the candidate with the real issuer verifier for local client tests.
use std::path::PathBuf;

use serde::Deserialize;

use crate::{
    app::AppState,
    harness::HarnessContext,
    http::{
        auth::HttpAuthVerifier,
        discovery::{CanonicalOrigin, DeviceProofSigner, EndpointRegistrar},
        runtime::mount,
        tls,
    },
    identity,
};

use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Qualification {
    hosted_origin: String,
    bridge_origin: String,
    run_dir: PathBuf,
}

impl Qualification {
    fn read() -> Self {
        let path = PathBuf::from(
            std::env::var_os("BUILD_HTTP_QUALIFICATION_FIXTURE")
                .expect("set BUILD_HTTP_QUALIFICATION_FIXTURE to the prepared runtime.json"),
        );
        let path = path.canonicalize().unwrap();
        let scratch = PathBuf::from(
            std::env::var_os("BUILD_HTTP_QUALIFICATION_SCRATCH")
                .expect("set BUILD_HTTP_QUALIFICATION_SCRATCH to the disposable fixture root"),
        )
        .canonicalize()
        .expect("BUILD_HTTP_QUALIFICATION_SCRATCH must be an existing directory");
        assert!(scratch.is_dir());
        let dir = path.parent().unwrap();
        assert!(dir.starts_with(&scratch) && dir != scratch);
        assert_eq!(path.file_name().unwrap(), "runtime.json");
        let fixture: Self = serde_json::from_slice(&std::fs::read(path.clone()).unwrap()).unwrap();
        assert_eq!(fixture.run_dir.canonicalize().unwrap(), dir);
        loopback_port(&fixture.hosted_origin, "localhost");
        loopback_port(&fixture.bridge_origin, "127.0.0.1");
        fixture
    }

    fn execution(&self) -> Arc<ExecutionService> {
        if std::env::var_os("BUILD_HTTP_QUALIFICATION_GRID").is_some() {
            return ExecutionService::new(super::flow::maximal_handler());
        }
        let home = self.run_dir.join("home");
        let harness = HarnessContext::resolved(home.join("unused.sock"), home.clone()).unwrap();
        let app = AppState::new_unrooted_configured(home.join("worktrees"), "main", true, harness)
            .with_projects_dir_default(home.join("projects"))
            .with_config(home.join("config.json"))
            .unwrap()
            .with_task_store(home.join("tasks"))
            .unwrap()
            .shared();
        let handler = AppState::handler(app);
        handler.start_read_workers().unwrap();
        ExecutionService::new(handler)
    }

    fn signer(&self) -> Arc<DeviceProofSigner> {
        let identity = identity::load(&self.run_dir.join("home/identity.json"))
            .unwrap()
            .unwrap();
        Arc::new(
            DeviceProofSigner::new(
                &identity,
                &self.hosted_origin,
                CanonicalOrigin::parse(&self.bridge_origin).unwrap(),
                uuid::Uuid::new_v4().to_string(),
            )
            .unwrap(),
        )
    }
}

fn loopback_port(origin: &str, host: &str) -> u16 {
    let url = reqwest::Url::parse(origin).unwrap();
    assert_eq!(url.host_str(), Some(host));
    assert_eq!(url.origin().ascii_serialization(), origin);
    assert_eq!(url.scheme(), "https");
    url.port().filter(|port| *port >= 1024).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "child listener for the disposable issuer and real client qualification"]
async fn serve_terminal_qualification_fixture() {
    let fixture = Qualification::read();
    let port = loopback_port(&fixture.bridge_origin, "127.0.0.1");
    let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    let tls = tls::load(
        &fixture.run_dir.join("cert.pem"),
        &fixture.run_dir.join("key.pem"),
    )
    .unwrap();
    let policy = HttpPolicy::new(
        &fixture.bridge_origin,
        std::slice::from_ref(&fixture.hosted_origin),
    )
    .unwrap();
    let signer = fixture.signer();
    let registrar = EndpointRegistrar::new(signer.clone()).unwrap();
    let disabled_epoch = fixture.run_dir.join("disabled-epoch");
    let rebind = std::env::var_os("BUILD_HTTP_QUALIFICATION_REBIND") == Some("1".into());
    let previous = if rebind && disabled_epoch.exists() {
        Some(std::fs::read_to_string(&disabled_epoch).unwrap())
    } else {
        None
    };
    registrar.register(previous.as_deref(), true).await.unwrap();
    let verifier = HttpAuthVerifier::new(signer.clone()).unwrap();
    let mut runtime = mount(fixture.execution(), verifier).with_discovery(signer.clone());
    if std::env::var_os("BUILD_HTTP_QUALIFICATION_TERMINAL").as_deref()
        == Some(std::ffi::OsStr::new("on"))
    {
        runtime.state.terminal_tickets = Some(Arc::default());
    }
    let (stop, stopped) = watch::channel(false);
    let mut term =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
    let mut serving = tokio::spawn(serve_mounted(listener, tls, policy, runtime, stopped));
    tokio::select! {
        result = &mut serving => result.unwrap().unwrap(),
        _ = term.recv() => {
            let _ = stop.send(true);
            serving.await.unwrap().unwrap();
        }
    }
    let disabled = fixture.signer();
    EndpointRegistrar::new(disabled.clone())
        .unwrap()
        .register(Some(signer.endpoint_epoch()), false)
        .await
        .unwrap();
    if rebind {
        std::fs::write(disabled_epoch, disabled.endpoint_epoch()).unwrap();
    }
}
