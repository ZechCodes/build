//! Device-signed claim and lifecycle for a Build-managed HTTPS endpoint.

mod claim;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod connector;
#[cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod session;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time::Instant;

use self::claim::{Assignment, ClaimClient, ClaimFailure, Reply, Report, SetupReason};
use super::discovery::CanonicalOrigin;
use super::endpoint_state::EndpointRecord;
use super::settings::OriginMode;
use super::supervisor;
use crate::backoff::Backoff;
use crate::execution::ExecutionService;
use crate::identity::StoredIdentity;
use crate::reachability::Reachability;

const CAPABLE: bool = cfg!(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
));
const RECHECK: Duration = Duration::from_secs(60);
const RETRY_MIN: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeploymentMode {
    Checking,
    WebRtc,
    PersonalReady,
    HostedPending,
    HostedReady,
}

impl DeploymentMode {
    fn label(self) -> &'static str {
        match self {
            Self::Checking => "checking",
            Self::WebRtc => "webrtc",
            Self::PersonalReady => "personal-ready",
            Self::HostedPending => "hosted-pending",
            Self::HostedReady => "hosted-ready",
        }
    }
}

#[derive(Clone)]
pub struct LocalTransport {
    rtc_allowed: Arc<watch::Sender<bool>>,
    revision: Arc<watch::Sender<Option<u64>>>,
    mode: Arc<watch::Sender<DeploymentMode>>,
    invocation: Option<String>,
}

impl LocalTransport {
    pub fn new() -> Self {
        Self {
            rtc_allowed: Arc::new(watch::channel(true).0),
            revision: Arc::new(watch::channel(None).0),
            mode: Arc::new(watch::channel(DeploymentMode::Checking).0),
            invocation: std::env::var("INVOCATION_ID")
                .ok()
                .filter(|value| valid_invocation(value)),
        }
    }

    pub(crate) fn hosted_endpoint_acknowledged(&self) {
        self.rtc_allowed.send_replace(false);
    }

    pub fn uses_hosted_heartbeat(&self) -> bool {
        !*self.rtc_allowed.borrow()
    }

    pub fn rtc_permission(&self) -> watch::Receiver<bool> {
        self.rtc_allowed.subscribe()
    }

    pub fn entitlement_revision(&self, revision: u64) {
        self.revision.send_if_modified(|last| {
            if *last == Some(revision) {
                return false;
            }
            *last = Some(revision);
            true
        });
    }

    pub fn log_initial_check(&self) {
        self.log_mode(DeploymentMode::Checking);
    }

    fn set_mode(&self, mode: DeploymentMode) {
        if self.mode.send_if_modified(|last| {
            if *last == mode {
                return false;
            }
            *last = mode;
            true
        }) {
            self.log_mode(mode);
        }
    }

    fn log_mode(&self, mode: DeploymentMode) {
        if let Some(invocation) = self.invocation.as_deref() {
            eprintln!(
                "bridge transport: {} {invocation} {}",
                std::process::id(),
                mode.label()
            );
        }
    }

    pub fn restore_hosted_history(
        &self,
        state_path: &Path,
        origin_mode: OriginMode,
        identity: &StoredIdentity,
        issuer: &str,
    ) {
        if origin_mode != OriginMode::Hosted || !CAPABLE {
            return;
        }
        let issuer = CanonicalOrigin::parse(issuer).ok();
        let previous = EndpointRecord::current(state_path).ok().flatten();
        if previous.is_some_and(|record| {
            record.hosted_ready_once
                && record.policy.device_id == identity.device_id
                && issuer
                    .as_ref()
                    .is_some_and(|issuer| issuer.as_str() == record.policy.issuer)
        }) {
            self.hosted_endpoint_acknowledged();
        }
    }
}

impl Default for LocalTransport {
    fn default() -> Self {
        Self::new()
    }
}

fn valid_invocation(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Don't start a new RTC dial, and drop in-flight signaling once hosted takes
/// over. The already-established peer channels own their own lifetimes.
pub async fn allowed_rtc_dial<T>(
    gate: &mut watch::Receiver<bool>,
    dial: impl Future<Output = T>,
) -> Option<T> {
    if gate.wait_for(|allowed| *allowed).await.is_err() {
        return std::future::pending().await;
    }
    tokio::select! {
        biased;
        _ = gate.wait_for(|allowed| !*allowed) => None,
        result = dial => Some(result),
    }
}

#[derive(Clone)]
pub struct Services {
    pub identity: StoredIdentity,
    pub issuer: String,
    pub web_origin: String,
    pub state_path: PathBuf,
    pub state_root: PathBuf,
    pub execution: Arc<ExecutionService>,
    pub transport: LocalTransport,
    pub hosted_reachability: Reachability,
    pub executable: Option<PathBuf>,
}

impl Services {
    pub async fn run(
        self,
        mode: OriginMode,
        personal_ready: watch::Receiver<bool>,
        shutdown: watch::Receiver<bool>,
    ) -> Result<(), String> {
        self.clean_crashed_runtime();
        let client =
            ClaimClient::new(&self.identity, &self.issuer).map_err(|error| error.to_string())?;
        let report = Report {
            mode,
            hosted_capable: CAPABLE,
        };
        self.maintain(client, report, personal_ready, shutdown)
            .await
    }

    fn clean_crashed_runtime(&self) {
        #[cfg(all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ))]
        {
            let path = self.state_root.join("hosted-runtime");
            if std::fs::symlink_metadata(&path).is_ok()
                && connector::PrivateRuntime::open(path).is_err()
            {
                crate::logline::say("hosted HTTPS runtime cleanup refused");
            }
        }
    }

    async fn maintain(
        self,
        client: ClaimClient,
        report: Report,
        mut personal_ready: watch::Receiver<bool>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), String> {
        let mut revision = self.transport.revision.subscribe();
        let mut assignments = Assignments::default();
        let mut schedule = ClaimSchedule::new();
        let mut retire_personal = self.has_retired_personal_origin(report.mode);
        loop {
            revision.borrow_and_update();
            let Some(result) =
                supervisor::prepare_or_shutdown(client.claim(report), &mut shutdown).await
            else {
                break;
            };

            let delay = schedule.delay(&result);
            if matches!(&result, Ok(Reply::Assigned(_))) {
                // The new hosted registration supersedes the personal epoch.
                retire_personal = false;
            }
            self.apply(result, &mut assignments, &personal_ready).await;
            if retire_personal {
                retire_personal = !self.retire_personal_origin(&mut shutdown).await;
            }
            let due = Instant::now() + delay;
            let wait = wait_for_claim(due, &mut revision, &mut personal_ready, &self.transport);
            if supervisor::prepare_or_shutdown(wait, &mut shutdown)
                .await
                .is_none()
            {
                break;
            }
        }
        assignments.stop().await;
        Ok(())
    }

    fn has_retired_personal_origin(&self, mode: OriginMode) -> bool {
        mode == OriginMode::Hosted
            && EndpointRecord::current(&self.state_path)
                .ok()
                .flatten()
                .is_some_and(|record| {
                    record.policy.device_id == self.identity.device_id
                        && record.policy.assignment_generation.is_none()
                        && (record.enabled
                            || record.registered_epoch.as_deref() != Some(&record.endpoint_epoch))
                })
    }

    async fn retire_personal_origin(&self, shutdown: &mut watch::Receiver<bool>) -> bool {
        let registration = supervisor::reconcile_disabled(&self.identity, self.state_path.clone());
        let attempt = tokio::time::timeout(Duration::from_secs(5), registration);
        matches!(
            supervisor::prepare_or_shutdown(attempt, shutdown).await,
            Some(Ok(Ok(())))
        )
    }

    async fn apply(
        &self,
        result: Result<Reply, ClaimFailure>,
        assignments: &mut Assignments,
        personal_ready: &watch::Receiver<bool>,
    ) {
        match result {
            Ok(Reply::Assigned(assignment)) => {
                assignments.select(self, assignment).await;
            }
            Err(ClaimFailure::LegacyIssuer) if self.transport.uses_hosted_heartbeat() => {}
            Ok(Reply::Off) | Err(ClaimFailure::LegacyIssuer) => {
                assignments.stop().await;
                self.transport.rtc_allowed.send_replace(true);
                self.transport.set_mode(DeploymentMode::WebRtc);
            }
            Ok(Reply::Personal) => {
                self.transport.set_mode(if *personal_ready.borrow() {
                    DeploymentMode::PersonalReady
                } else {
                    DeploymentMode::Checking
                });
            }
            Ok(Reply::Setup { reason }) => {
                assignments.stop().await;
                self.transport.set_mode(DeploymentMode::HostedPending);
                if matches!(reason, SetupReason::UpdateRequired) {
                    crate::logline::say("hosted HTTPS requires a supported bridge platform");
                }
            }
            Err(ClaimFailure::Refused) => {
                assignments.stop().await;
                self.transport.set_mode(DeploymentMode::Checking);
                crate::logline::say("hosted HTTPS claim refused");
            }
            Err(ClaimFailure::Unavailable) => {}
        }
    }
}

async fn wait_for_claim(
    due: Instant,
    revision: &mut watch::Receiver<Option<u64>>,
    personal_ready: &mut watch::Receiver<bool>,
    transport: &LocalTransport,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(due) => return,
            _ = revision.changed() => return,
            result = personal_ready.changed(), if personal_ready.has_changed().is_ok() => {
                let mode = *transport.mode.borrow();
                if result.is_ok() && !*personal_ready.borrow_and_update() && mode == DeploymentMode::PersonalReady {
                    transport.set_mode(DeploymentMode::Checking);
                }
                if result.is_ok() && *personal_ready.borrow() && mode == DeploymentMode::Checking {
                    // Check issuer mode before deciding whether personal registration is deploy evidence.
                    return;
                }
            }
        }
    }
}

struct ClaimSchedule(Backoff);

impl ClaimSchedule {
    fn new() -> Self {
        Self(Backoff::new(RETRY_MIN, RETRY_MAX))
    }

    fn delay(&mut self, result: &Result<Reply, ClaimFailure>) -> Duration {
        match result {
            Ok(Reply::Setup {
                reason: SetupReason::UpdateRequired,
            }) => RECHECK,
            Ok(Reply::Setup { .. }) | Err(ClaimFailure::Unavailable | ClaimFailure::Refused) => {
                let delay = self.0.current();
                self.0.increase();
                delay
            }
            _ => {
                self.0.reset();
                RECHECK
            }
        }
    }
}

#[derive(Default)]
struct Assignments {
    #[cfg(all(
        target_os = "linux",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    running: Option<session::ActiveAssignment>,
}

impl Assignments {
    async fn select(&mut self, service: &Services, next: Assignment) {
        #[cfg(all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ))]
        {
            if self
                .running
                .as_ref()
                .is_some_and(|running| running.matches(&next))
            {
                return;
            }
            self.stop().await;
            service.transport.set_mode(DeploymentMode::HostedPending);
            self.running = Some(session::ActiveAssignment::start(service, next));
        }
        #[cfg(not(all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )))]
        let _ = (service, next);
    }

    async fn stop(&mut self) {
        #[cfg(all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ))]
        if let Some(running) = self.running.take() {
            running.stop().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn hosted_history_restores_rtc_fence_only_for_the_same_acknowledged_pairing() {
        use super::super::endpoint_state::EndpointPolicy;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("endpoint.json");
        let identity = crate::identity::generate("fixture");
        let other_device = crate::identity::generate("different fixture");
        let issuer = "https://issuer.example";
        let policy = EndpointPolicy {
            device_id: identity.device_id.clone(),
            issuer: issuer.into(),
            origin: "https://bridge.example".into(),
            allowed_origins: vec![issuer.into()],
            certificate_fingerprint: "fixture-leaf".into(),
            assignment_generation: Some(5),
        };
        let pending = EndpointRecord::prepare(&path, Some(policy))
            .unwrap()
            .unwrap();
        let can_dial = |mode, device, issuer| {
            let restarted = LocalTransport::new();
            restarted.restore_hosted_history(&path, mode, device, issuer);
            let allowed = *restarted.rtc_permission().borrow();
            allowed
        };

        assert!(
            can_dial(OriginMode::Hosted, &identity, issuer),
            "unacknowledged endpoint"
        );
        pending.acknowledge(&path).unwrap();
        EndpointRecord::prepare(&path, None).unwrap();
        assert_eq!(
            can_dial(OriginMode::Hosted, &identity, issuer),
            !CAPABLE,
            "matching pairing after retirement"
        );
        assert!(
            can_dial(OriginMode::Hosted, &other_device, issuer),
            "different device"
        );
        assert!(
            can_dial(OriginMode::Hosted, &identity, "https://other.example"),
            "different issuer"
        );
        assert!(
            can_dial(OriginMode::Personal, &identity, issuer),
            "personal override"
        );
    }

    async fn post_revision(
        server: &wiremock::MockServer,
        transport: &LocalTransport,
        status: u16,
        revision: u64,
        size: usize,
    ) {
        use crate::presence::post_test_heartbeat;
        use wiremock::{
            matchers::{method, path},
            Mock, ResponseTemplate,
        };
        let mut body = format!(r#"{{"entitlement_revision":{revision}}}"#);
        body.extend(std::iter::repeat_n(' ', size - body.len()));
        server.reset().await;
        Mock::given(method("POST"))
            .and(path("/api/devices/heartbeat"))
            .respond_with(ResponseTemplate::new(status).set_body_raw(body, "application/json"))
            .expect(1)
            .mount(server)
            .await;
        let url = format!("{}/api/devices/heartbeat", server.uri());
        assert_eq!(post_test_heartbeat(&url, transport).await, status == 200);
        server.verify().await;
    }

    #[tokio::test]
    async fn hosted_heartbeat_revision_wakes_the_claim_before_its_poll() {
        let transport = LocalTransport::new();
        let mut revision = transport.revision.subscribe();
        let (_personal, mut readiness) = watch::channel(false);
        let server = wiremock::MockServer::start().await;

        post_revision(&server, &transport, 200, 6, 1024).await;
        tokio::time::timeout(
            Duration::from_secs(1),
            wait_for_claim(
                Instant::now() + RECHECK,
                &mut revision,
                &mut readiness,
                &transport,
            ),
        )
        .await
        .expect("a heartbeat revision wakes the claim before its scheduled poll");
        assert_eq!(*revision.borrow_and_update(), Some(6));
    }

    #[tokio::test]
    async fn hosted_heartbeat_revision_ignores_oversize_unchanged_and_refused_replies() {
        let transport = LocalTransport::new();
        let mut revision = transport.revision.subscribe();
        let server = wiremock::MockServer::start().await;
        post_revision(&server, &transport, 200, 6, 1024).await;
        assert_eq!(*revision.borrow_and_update(), Some(6));
        for (status, hint, size) in [(200, 6, 1024), (200, 7, 1025), (403, 7, 1024)] {
            post_revision(&server, &transport, status, hint, size).await;
            assert!(
                !revision.has_changed().unwrap(),
                "ignored heartbeat hint ({status}, {hint}, {size})"
            );
        }
        post_revision(&server, &transport, 200, 5, 1024).await;
        assert!(
            revision.has_changed().unwrap(),
            "a lower changed revision also wakes the claim"
        );
        assert_eq!(*revision.borrow_and_update(), Some(5));
    }

    #[tokio::test]
    async fn hosted_rtc_bootstraps_until_ready_and_stops_redialling() {
        let transport = LocalTransport::new();
        let calls = AtomicUsize::new(0);
        let mut gate = transport.rtc_permission();
        let dial = || async { calls.fetch_add(1, Ordering::Relaxed) };
        assert_eq!(allowed_rtc_dial(&mut gate, dial()).await, Some(0));
        transport.rtc_allowed.send_replace(false);
        assert!(tokio::time::timeout(
            Duration::from_millis(5),
            allowed_rtc_dial(&mut gate, dial())
        )
        .await
        .is_err());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        transport.rtc_allowed.send_replace(true);
        assert_eq!(allowed_rtc_dial(&mut gate, dial()).await, Some(1));
    }

    #[tokio::test]
    async fn hosted_connector_is_never_installed_for_free_or_personal_claims() {
        use crate::execution::FrameHandler;
        use crate::timing::FrameClock;
        let directory = tempfile::tempdir().unwrap();
        let execution = ExecutionService::new(FrameHandler::new(FrameClock::new(), |_, _, _| {
            serde_json::Value::Null
        }));
        let transport = LocalTransport::new();
        let service = Services {
            identity: crate::identity::generate("fixture"),
            issuer: "https://issuer.example".into(),
            web_origin: "https://issuer.example".into(),
            state_path: directory.path().join("endpoint.json"),
            state_root: directory.path().to_owned(),
            execution,
            transport: transport.clone(),
            hosted_reachability: Reachability::unreachable(),
            executable: Some(directory.path().join("must-not-be-opened")),
        };
        let mut assignments = Assignments::default();
        let ready = watch::channel(true).1;
        service
            .apply(Ok(Reply::Off), &mut assignments, &ready)
            .await;
        assert_eq!(*transport.mode.borrow(), DeploymentMode::WebRtc);
        assert!(*transport.rtc_permission().borrow());
        service
            .apply(Ok(Reply::Personal), &mut assignments, &ready)
            .await;
        assert_eq!(*transport.mode.borrow(), DeploymentMode::PersonalReady);
        assert!(*transport.rtc_permission().borrow());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn hosted_claim_pending_retries_with_bounded_backoff() {
        let mut schedule = ClaimSchedule::new();
        let pending = Ok(Reply::Setup {
            reason: SetupReason::Provisioning,
        });
        assert_eq!(schedule.delay(&pending), RETRY_MIN);
        assert_eq!(schedule.delay(&pending), RETRY_MIN * 2);
        for _ in 0..10 {
            assert!(schedule.delay(&pending) <= RETRY_MAX);
        }
        assert_eq!(schedule.delay(&Ok(Reply::Off)), RECHECK);
        assert_eq!(schedule.delay(&pending), RETRY_MIN);
    }
}
