use super::*;
use crate::execution::FrameHandler;
use crate::http::discovery::unix_now;
use crate::reachability::Reachability;
use crate::timing::FrameClock;
use axum::{extract::State, routing::post, Json, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use tokio::sync::{mpsc, oneshot};

const ISSUER: &str = "https://issuer.example";
const HOSTNAME: &str = "b-4071dc1c1a3d412b8a47cf8bfaf0bf51.hosted.example";
const GENERATION: u64 = 2251799813685265;
const DEADLINE: Duration = Duration::from_secs(4);

pub(super) struct SessionFixture {
    pub issuer: String,
    pub executable: PathBuf,
}

struct PendingRegistration {
    claims: Value,
    answer: oneshot::Sender<Value>,
}

impl PendingRegistration {
    fn acknowledge(self) {
        let claims = self.claims;
        self.answer
            .send(json!({
                "origin": claims["origin"],
                "endpoint_epoch": claims["endpoint_epoch"],
                "enabled": claims["enabled"],
            }))
            .unwrap();
    }
}

struct Issuer {
    base: String,
    requests: mpsc::Receiver<PendingRegistration>,
    task: JoinHandle<()>,
}

impl Issuer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (send, requests) = mpsc::channel(8);
        let app = Router::new()
            .route(
                "/api/devices/{device}/http-endpoint/challenge",
                post(challenge),
            )
            .route("/api/devices/{device}/http-endpoint", post(register))
            .with_state(send);
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            base,
            requests,
            task,
        }
    }

    async fn next(&mut self, enabled: bool) -> PendingRegistration {
        let request = tokio::time::timeout(DEADLINE, self.requests.recv())
            .await
            .expect("session submitted registration")
            .unwrap();
        assert_eq!(request.claims["enabled"], enabled);
        assert_eq!(request.claims["assignment_generation"], GENERATION);
        request
    }
}

impl Drop for Issuer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn challenge() -> Json<Value> {
    Json(json!({
        "challenge_id": "test-challenge",
        "nonce": "test-nonce",
        "expires_at": unix_now().unwrap() + 60,
    }))
}

async fn register(
    State(requests): State<mpsc::Sender<PendingRegistration>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let proof = body["proof"].as_str().unwrap();
    let payload = URL_SAFE_NO_PAD
        .decode(proof.split('.').nth(1).unwrap())
        .unwrap();
    let claims = serde_json::from_slice(&payload).unwrap();
    let (answer, pending) = oneshot::channel();
    requests
        .send(PendingRegistration { claims, answer })
        .await
        .unwrap();
    Json(pending.await.unwrap_or(Value::Null))
}

struct TestSession {
    services: Services,
    directory: tempfile::TempDir,
    executable: PathBuf,
}

impl TestSession {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("connector");
        fs::write(
            &executable,
            r#"#!/bin/sh
printf '%s\n' started > started
printf '%s\n' '{"message":"Registered tunnel connection"}' >&2
while [ ! -f config-push ]; do /bin/sleep 0.02; done
printf '%s\n' '{"message":"Updated to new configuration"}' >&2
exec /bin/sleep 3600
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let mut identity = crate::identity::generate("session test");
        identity.approved = true;
        identity.approved_by = Some(ISSUER.into());
        let services = Services {
            identity,
            issuer: ISSUER.into(),
            web_origin: ISSUER.into(),
            state_path: directory.path().join("endpoint.json"),
            state_root: directory.path().into(),
            execution: crate::execution::ExecutionService::new(FrameHandler::new(
                FrameClock::new(),
                |_, _, _| Value::Null,
            )),
            transport: Default::default(),
            hosted_reachability: Reachability::unreachable(),
            executable: None,
        };
        Self {
            services,
            directory,
            executable,
        }
    }

    fn runtime(&self) -> PathBuf {
        self.directory.path().join("hosted-runtime")
    }

    fn start(&self, issuer: &Issuer) -> RunningSession {
        let mut session = Session::new(&self.services, Arc::new(assignment()));
        session.fixture = Some(SessionFixture {
            issuer: issuer.base.clone(),
            executable: self.executable.clone(),
        });
        let (shutdown, stop) = watch::channel(false);
        RunningSession {
            shutdown,
            task: tokio::spawn(session.run(stop)),
        }
    }

    fn read_record(&self) -> EndpointRecord {
        EndpointRecord::current(&self.services.state_path)
            .unwrap()
            .unwrap()
    }
}

struct RunningSession {
    shutdown: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl RunningSession {
    async fn join(&mut self) {
        tokio::time::timeout(DEADLINE, &mut self.task)
            .await
            .unwrap()
            .unwrap();
    }
}

impl Drop for RunningSession {
    fn drop(&mut self) {
        self.shutdown.send_replace(true);
        self.task.abort();
    }
}

fn assignment() -> Assignment {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../../planning/v2/fixtures/hosted-tunnel-claim.json"
    ))
    .unwrap();
    let mut reply = fixture["cases"][2]["response"]["body"].clone();
    reply.as_object_mut().unwrap().remove("status");
    serde_json::from_value(reply).unwrap()
}

fn seed_enabled_endpoint(test: &TestSession) -> String {
    let policy = EndpointPolicy {
        device_id: test.services.identity.device_id.clone(),
        issuer: ISSUER.into(),
        origin: format!("https://{HOSTNAME}"),
        allowed_origins: vec![ISSUER.into()],
        certificate_fingerprint: "fixture-leaf".into(),
        assignment_generation: Some(GENERATION),
    };
    let endpoint = EndpointRecord::prepare(&test.services.state_path, Some(policy))
        .unwrap()
        .unwrap();
    endpoint.acknowledge(&test.services.state_path).unwrap();
    endpoint.endpoint_epoch
}

#[tokio::test]
async fn hosted_session_restart_withdraws_quarantined_generation_without_starting_child() {
    let test = TestSession::new();
    let previous = seed_enabled_endpoint(&test);
    let runtime = test.runtime();
    let private = PrivateRuntime::open(runtime.clone()).unwrap();
    private.quarantine(GENERATION).unwrap();
    drop(private);

    let mut issuer = Issuer::start().await;
    let mut session = test.start(&issuer);
    let withdrawal = issuer.next(false).await;
    assert_named_predecessor(&withdrawal, &previous);
    assert_eq!(
        test.read_record().policy.certificate_fingerprint,
        "fixture-leaf",
        "restart must withdraw the existing endpoint without preparing a new origin"
    );
    withdrawal.acknowledge();
    session.join().await;
    let record = test.read_record();
    assert!(!record.enabled);
    assert_eq!(
        record.registered_epoch.as_deref(),
        Some(record.endpoint_epoch.as_str())
    );
    assert!(
        !runtime.join("started").exists(),
        "a quarantined generation must never start"
    );
}

fn assert_named_predecessor(request: &PendingRegistration, previous: &str) {
    assert!(request.claims["previous_endpoint_epochs"]
        .as_array()
        .unwrap()
        .contains(&json!(previous)));
}

#[tokio::test]
async fn hosted_session_keeps_rtc_until_enabled_registration_acknowledgement() {
    let test = TestSession::new();
    let mut issuer = Issuer::start().await;
    let mut session = test.start(&issuer);
    let mut rtc = test.services.transport.rtc_permission();
    let disabled = issuer.next(false).await;
    let previous = disabled.claims["endpoint_epoch"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        *rtc.borrow(),
        "disabled publication must leave the bootstrap relay open"
    );
    disabled.acknowledge();

    let enabled = issuer.next(true).await;
    assert_named_predecessor(&enabled, &previous);
    assert!(
        *rtc.borrow(),
        "RTC must stay open while the enabled acknowledgement is pending"
    );
    assert!(!test.read_record().hosted_ready_once);
    enabled.acknowledge();
    tokio::time::timeout(DEADLINE, rtc.wait_for(|allowed| !*allowed))
        .await
        .expect("enabled acknowledgement closes RTC")
        .unwrap();
    let acknowledged = test.read_record();
    assert!(acknowledged.hosted_ready_once);
    assert_eq!(
        acknowledged.registered_epoch.as_deref(),
        Some(acknowledged.endpoint_epoch.as_str())
    );

    session.shutdown.send_replace(true);
    issuer.next(false).await.acknowledge();
    session.join().await;
}

#[tokio::test]
async fn hosted_session_persists_fatal_quarantine_before_withdrawal() {
    let test = TestSession::new();
    let mut issuer = Issuer::start().await;
    let mut session = test.start(&issuer);
    issuer.next(false).await.acknowledge();
    issuer.next(true).await.acknowledge();
    let mut rtc = test.services.transport.rtc_permission();
    tokio::time::timeout(DEADLINE, rtc.wait_for(|allowed| !*allowed))
        .await
        .unwrap()
        .unwrap();

    fs::write(test.runtime().join("config-push"), b"").unwrap();
    let withdrawal = issuer.next(false).await;
    assert!(
        test.runtime()
            .join(format!("rejected-{GENERATION}"))
            .is_file(),
        "the marker must be persisted before the first withdrawal request"
    );
    withdrawal.acknowledge();
    // The fatal path also reconciles the same disabled generation.
    issuer.next(false).await.acknowledge();
    session.join().await;
}
