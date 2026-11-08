//! Production construction and lifecycle for the optional HTTPS listener.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use super::auth::HttpAuthVerifier;
use super::discovery::{CanonicalOrigin, DeviceProofSigner, EndpointRegistrar, ProofError};
use super::endpoint_state::{EndpointPolicy, EndpointRecord};
use super::policy::HttpPolicy;
use super::runtime::{self, HttpRuntime};
use super::settings::HttpSettings;
use crate::execution::ExecutionService;
use crate::identity::StoredIdentity;

const STATE_RETRIES: usize = 3;
const REGISTRATION_RETRY_MIN: Duration = Duration::from_secs(2);
const REGISTRATION_RETRY_MAX: Duration = Duration::from_secs(300);

/// Prepare HTTP independently of daemon startup. Registration can remain
/// unavailable indefinitely without delaying relay, MCP or agent services.
/// Cancellation during preparation drops the reserved, non-serving socket;
/// once serving, shutdown goes through the runtime's authority fence.
pub async fn run(
    settings: Option<HttpSettings>,
    identity: StoredIdentity,
    issuer: String,
    default_allowed_origin: String,
    state_path: PathBuf,
    execution: Arc<ExecutionService>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), String> {
    let preparation = async {
        match settings {
            Some(settings) => PreparedHttp::prepare(
                settings,
                &identity,
                &issuer,
                &default_allowed_origin,
                state_path,
                execution,
            )
            .await
            .map(Some),
            None => reconcile_disabled(&identity, state_path)
                .await
                .map(|()| None),
        }
    };
    match prepare_or_shutdown(preparation, &mut shutdown).await {
        Some(Ok(Some(prepared))) => prepared.serve(shutdown).await,
        Some(Ok(None)) => {
            wait_for_shutdown(&mut shutdown).await;
            Ok(())
        }
        Some(Err(error)) => Err(error),
        None => Ok(()),
    }
}

async fn wait_for_shutdown(shutdown: &mut watch::Receiver<bool>) {
    while !*shutdown.borrow_and_update() {
        if shutdown.changed().await.is_err() {
            break;
        }
    }
}

async fn prepare_or_shutdown<T>(
    preparation: impl std::future::Future<Output = T>,
    shutdown: &mut watch::Receiver<bool>,
) -> Option<T> {
    tokio::select! {
        biased;
        () = wait_for_shutdown(shutdown) => None,
        result = preparation => Some(result),
    }
}

pub struct PreparedHttp {
    listener: tokio::net::TcpListener,
    tls: Arc<rustls::ServerConfig>,
    policy: HttpPolicy,
    runtime: HttpRuntime,
    disable: DisableContext,
}

struct DisableContext {
    identity: StoredIdentity,
    issuer: String,
    origin: CanonicalOrigin,
    state_path: PathBuf,
}

async fn prepare_listener<F, Fut>(
    bind: std::net::SocketAddr,
    register_and_acknowledge: F,
) -> Result<tokio::net::TcpListener, String>
where
    F: FnOnce(std::net::SocketAddr) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    // Reserve the address before any hosted publication, without listening:
    // TCP connections must be refused throughout registration retries.
    let socket = if bind.is_ipv4() {
        tokio::net::TcpSocket::new_v4()
    } else {
        tokio::net::TcpSocket::new_v6()
    }
    .map_err(|error| format!("cannot create HTTP socket: {error}"))?;
    // Match TcpListener::bind on Unix so a roll can rebind while accepted
    // connections from the previous listener are still in TIME_WAIT.
    #[cfg(unix)]
    socket
        .set_reuseaddr(true)
        .map_err(|error| format!("cannot configure HTTP socket: {error}"))?;
    socket
        .bind(bind)
        .map_err(|error| format!("cannot bind HTTP listener: {error}"))?;
    let address = socket
        .local_addr()
        .map_err(|error| format!("cannot inspect HTTP socket: {error}"))?;
    register_and_acknowledge(address).await?;
    // Start accepting connections only after hosted registration and its
    // durable local acknowledgment have both succeeded.
    socket
        .listen(1024)
        .map_err(|error| format!("cannot listen on HTTP socket: {error}"))
}

impl PreparedHttp {
    /// Complete persisted intent and hosted registration before binding is
    /// made reachable. Disabled settings never call this constructor.
    pub async fn prepare(
        settings: HttpSettings,
        identity: &StoredIdentity,
        issuer: &str,
        default_allowed_origin: &str,
        state_path: PathBuf,
        execution: Arc<ExecutionService>,
    ) -> Result<Self, String> {
        let origin = CanonicalOrigin::parse(&settings.origin)
            .map_err(|error| format!("invalid HTTP origin: {error}"))?;
        let issuer = CanonicalOrigin::parse(issuer)
            .map_err(|error| format!("invalid hosted issuer: {error}"))?;
        let default_allowed_origin = CanonicalOrigin::parse(default_allowed_origin)
            .map_err(|error| format!("invalid hosted application origin: {error}"))?;
        let allowed_origins = settings
            .allowed_origins
            .clone()
            .unwrap_or_else(|| vec![default_allowed_origin.as_str().to_owned()]);
        // Validate every local policy input before persisting or publishing a
        // new endpoint generation.
        let policy = HttpPolicy::new(origin.as_str(), &allowed_origins)?;
        let loaded_tls = super::tls::load_identity(&settings.tls_cert, &settings.tls_key)?;
        let tls = loaded_tls.config;
        let certificate_fingerprint = loaded_tls.certificate_fingerprint;
        let desired = EndpointPolicy {
            device_id: identity.device_id.clone(),
            issuer: issuer.as_str().to_owned(),
            origin: origin.as_str().to_owned(),
            allowed_origins: allowed_origins.clone(),
            certificate_fingerprint,
        };
        let record = prepare_record(&state_path, Some(desired))
            .await?
            .ok_or("HTTP endpoint intent was not persisted")?;
        if !record.enabled {
            return Err("HTTP endpoint intent is disabled".into());
        }
        let signer = Arc::new(
            DeviceProofSigner::new(
                identity,
                issuer.as_str(),
                origin.clone(),
                record.endpoint_epoch.clone(),
            )
            .map_err(|error| format!("cannot construct HTTP signer: {error}"))?,
        );
        let verifier = HttpAuthVerifier::new(signer.clone())
            .map_err(|error| format!("cannot construct HTTP verifier: {error}"))?;
        let registrar = EndpointRegistrar::new(signer.clone())
            .map_err(|error| format!("cannot construct endpoint registrar: {error}"))?;
        let listener = prepare_listener(settings.bind, |_| async {
            register_generation(&registrar, &record, true).await;
            acknowledge_record(&state_path, record).await
        })
        .await?;

        let runtime = runtime::mount(execution, verifier).with_discovery(signer);
        Ok(Self {
            listener,
            tls,
            policy,
            runtime,
            disable: DisableContext {
                identity: identity.clone(),
                issuer: issuer.as_str().to_owned(),
                origin,
                state_path,
            },
        })
    }

    pub async fn serve(self, shutdown: watch::Receiver<bool>) -> Result<(), String> {
        let Self {
            listener,
            tls,
            policy,
            runtime,
            disable,
        } = self;
        let result = runtime::serve_mounted(listener, tls, policy, runtime, shutdown)
            .await
            .map_err(|error| format!("HTTP listener failed: {error}"));
        // Local authority has already been fenced and the listener retired.
        // Hosted deactivation is bounded and can never postpone that fence.
        if let Err(error) = tokio::time::timeout(Duration::from_secs(5), disable.register()).await {
            eprintln!("HTTP endpoint deactivation timed out: {error}");
        }
        result
    }
}

/// Persist a disabled generation on startup even when HTTP settings resolve to
/// `None`, then best-effort reconcile a previously advertised endpoint. Local
/// authority is already absent in this path, so hosted unavailability is
/// reported without reviving or delaying a listener.
pub async fn reconcile_disabled(
    identity: &StoredIdentity,
    state_path: PathBuf,
) -> Result<(), String> {
    let Some(record) = prepare_record(&state_path, None).await? else {
        return Ok(());
    };
    let origin = CanonicalOrigin::parse(&record.policy.origin)
        .map_err(|error| format!("invalid persisted HTTP origin: {error}"))?;
    let signer = Arc::new(
        DeviceProofSigner::new(
            identity,
            &record.policy.issuer,
            origin,
            record.endpoint_epoch.clone(),
        )
        .map_err(|error| format!("cannot construct HTTP disable signer: {error}"))?,
    );
    let registrar = EndpointRegistrar::new(signer)
        .map_err(|error| format!("cannot construct endpoint registrar: {error}"))?;
    register_generation(&registrar, &record, false).await;
    acknowledge_record(&state_path, record).await
}

impl DisableContext {
    async fn register(self) {
        let record = match prepare_record(&self.state_path, None).await {
            Ok(Some(record)) => record,
            Ok(None) => return,
            Err(error) => {
                eprintln!("HTTP endpoint disable persistence failed: {error}");
                return;
            }
        };
        let signer = match DeviceProofSigner::new(
            &self.identity,
            &self.issuer,
            self.origin,
            record.endpoint_epoch.clone(),
        ) {
            Ok(signer) => Arc::new(signer),
            Err(error) => {
                eprintln!("HTTP endpoint disable signer failed: {error}");
                return;
            }
        };
        let result = async {
            let registrar = EndpointRegistrar::new(signer).map_err(|error| error.to_string())?;
            register_generation(&registrar, &record, false).await;
            acknowledge_record(&self.state_path, record).await
        }
        .await;
        if let Err(error) = result {
            eprintln!("HTTP endpoint deactivation remains pending: {error}");
        }
    }
}

/// Atomically name every bounded generation this exact device/issuer may have
/// committed, including the desired generation for response-loss recovery.
/// The hosted CAS refuses any unknown external generation. Backoff retries
/// reuse this prepared record and candidate set, including after Conflict.
async fn register_generation(
    registrar: &EndpointRegistrar,
    record: &EndpointRecord,
    enabled: bool,
) {
    let phase = if enabled {
        "registration"
    } else {
        "deactivation"
    };
    retry_registration(record, phase, |predecessors| async move {
        registrar
            .register_candidates(&predecessors, enabled)
            .await
            .map(|_| ())
    })
    .await;
}

async fn retry_registration<F, Fut>(record: &EndpointRecord, phase: &str, register: F)
where
    F: Fn(Vec<Option<String>>) -> Fut,
    Fut: std::future::Future<Output = Result<(), ProofError>>,
{
    let mut backoff = crate::backoff::Backoff::new(REGISTRATION_RETRY_MIN, REGISTRATION_RETRY_MAX);
    loop {
        match reconcile_registration(record, &register).await {
            Ok(()) => return,
            Err(error) => eprintln!("{}", retry_message(phase, error, backoff.current())),
        }
        tokio::time::sleep(backoff.current()).await;
        backoff.increase();
    }
}

fn retry_message(phase: &str, error: ProofError, delay: Duration) -> String {
    let hint = if error == ProofError::Conflict {
        "; persistent conflicts require out-of-band recovery of local endpoint state or the hosted registration"
    } else {
        ""
    };
    format!(
        "HTTP endpoint {phase} remains pending: {error}; retrying in {}s{hint}",
        delay.as_secs(),
    )
}

async fn reconcile_registration<F, Fut>(
    record: &EndpointRecord,
    mut register: F,
) -> Result<(), ProofError>
where
    F: FnMut(Vec<Option<String>>) -> Fut,
    Fut: std::future::Future<Output = Result<(), ProofError>>,
{
    let predecessors = record.registration_predecessors();
    match register(predecessors.clone()).await {
        Ok(()) => Ok(()),
        Err(original @ ProofError::Unavailable) => {
            register(predecessors).await.map_err(|_| original)
        }
        Err(error) => Err(error),
    }
}

async fn prepare_record(
    path: &Path,
    desired: Option<EndpointPolicy>,
) -> Result<Option<EndpointRecord>, String> {
    for attempt in 0..STATE_RETRIES {
        let path = path.to_owned();
        let desired = desired.clone();
        let result = tokio::task::spawn_blocking(move || EndpointRecord::prepare(&path, desired))
            .await
            .map_err(|_| "HTTP endpoint state worker failed".to_string())?;
        match result {
            Err(error) if error == "HTTP endpoint state is busy" && attempt + 1 < STATE_RETRIES => {
                tokio::time::sleep(Duration::from_millis(25 * (attempt as u64 + 1))).await;
            }
            result => return result,
        }
    }
    unreachable!()
}

async fn acknowledge_record(path: &Path, record: EndpointRecord) -> Result<(), String> {
    for attempt in 0..STATE_RETRIES {
        let path = path.to_owned();
        let record = record.clone();
        let result = tokio::task::spawn_blocking(move || record.acknowledge(&path))
            .await
            .map_err(|_| "HTTP endpoint state worker failed".to_string())?;
        match result {
            Err(error) if error == "HTTP endpoint state is busy" && attempt + 1 < STATE_RETRIES => {
                tokio::time::sleep(Duration::from_millis(25 * (attempt as u64 + 1))).await;
            }
            result => return result,
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn record() -> EndpointRecord {
        EndpointRecord {
            policy: EndpointPolicy {
                device_id: "device".into(),
                issuer: "https://account.example".into(),
                origin: "https://device.example".into(),
                allowed_origins: vec!["https://account.example".into()],
                certificate_fingerprint: "certificate".into(),
            },
            endpoint_epoch: "desired".into(),
            enabled: true,
            registered_epoch: Some("previous".into()),
            unacknowledged_epochs: Vec::new(),
        }
    }

    async fn reconcile_script(
        outcomes: Vec<Result<(), ProofError>>,
    ) -> (Result<(), ProofError>, Vec<Vec<Option<String>>>) {
        let outcomes = std::sync::Mutex::new(VecDeque::from(outcomes));
        let attempts = std::sync::Mutex::new(Vec::new());
        let result = reconcile_registration(&record(), |predecessors| {
            attempts.lock().unwrap().push(predecessors);
            std::future::ready(outcomes.lock().unwrap().pop_front().unwrap())
        })
        .await;
        (result, attempts.into_inner().unwrap())
    }

    #[tokio::test]
    async fn reserved_port_refuses_connections_while_registration_retries() {
        for bind in ["127.0.0.1:0", "[::1]:0"] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("endpoint.json");
            let record = prepare_record(&path, Some(record().policy))
                .await
                .unwrap()
                .unwrap();
            let epoch = record.endpoint_epoch.clone();
            let listener = prepare_listener(bind.parse().unwrap(), |address| {
                let record = record.clone();
                let path = path.clone();
                async move {
                    let allow_registration = std::sync::atomic::AtomicBool::new(false);
                    let attempts = std::sync::atomic::AtomicUsize::new(0);
                    let registration = retry_registration(&record, "registration", |_| {
                        attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        std::future::ready(
                            if allow_registration.load(std::sync::atomic::Ordering::SeqCst) {
                                Ok(())
                            } else {
                                Err(ProofError::Conflict)
                            },
                        )
                    });
                    tokio::pin!(registration);
                    assert!(
                        tokio::time::timeout(Duration::from_millis(1), &mut registration)
                            .await
                            .is_err()
                    );
                    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
                    let connection = tokio::time::timeout(
                        Duration::from_secs(1),
                        tokio::net::TcpStream::connect(address),
                    )
                    .await
                    .expect("connection to the reserved port must fail promptly");
                    let error = connection
                        .expect_err("pending registration must not accept TCP connections");
                    assert_eq!(error.kind(), std::io::ErrorKind::ConnectionRefused);
                    allow_registration.store(true, std::sync::atomic::Ordering::SeqCst);
                    registration.await;
                    acknowledge_record(&path, record.clone()).await
                }
            })
            .await
            .unwrap();
            let acknowledged = prepare_record(&path, Some(record.policy.clone()))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(acknowledged.registered_epoch, Some(epoch));
            let client = tokio::time::timeout(
                Duration::from_secs(1),
                tokio::net::TcpStream::connect(listener.local_addr().unwrap()),
            )
            .await
            .expect("acknowledged endpoint starts listening")
            .unwrap();
            let (_accepted, _) = listener.accept().await.unwrap();
            drop(client);
        }
    }

    #[tokio::test]
    async fn bind_failure_prevents_registration_and_advertisement() {
        for bind in ["127.0.0.1:0", "[::1]:0"] {
            let occupied = tokio::net::TcpListener::bind(bind).await.unwrap();
            let advertised = std::sync::atomic::AtomicBool::new(false);
            let error = prepare_listener(occupied.local_addr().unwrap(), |_| async {
                advertised.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
            .await
            .unwrap_err();
            assert!(error.starts_with("cannot bind HTTP listener:"), "{error}");
            assert!(!advertised.load(std::sync::atomic::Ordering::SeqCst));
        }
    }

    #[tokio::test]
    async fn failed_acknowledgment_never_starts_listening() {
        for bind in ["127.0.0.1:0", "[::1]:0"] {
            let address = std::cell::Cell::new(None);
            let error = prepare_listener(bind.parse().unwrap(), |reserved| {
                address.set(Some(reserved));
                std::future::ready(Err("acknowledgment failed".into()))
            })
            .await
            .unwrap_err();
            assert_eq!(error, "acknowledgment failed");
            let connection = tokio::time::timeout(
                Duration::from_secs(1),
                tokio::net::TcpStream::connect(address.get().unwrap()),
            )
            .await
            .expect("failed acknowledgment releases the reserved port");
            assert_eq!(
                connection.unwrap_err().kind(),
                std::io::ErrorKind::ConnectionRefused
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn listener_can_restart_while_closed_connections_are_in_time_wait() {
        const MAX_ATTEMPTS: usize = 5;
        let addr_in_use = format!(
            "cannot bind HTTP listener: {}",
            std::io::Error::from_raw_os_error(libc::EADDRINUSE)
        );

        for bind in ["127.0.0.1:0", "[::1]:0"] {
            for attempt in 1..=MAX_ATTEMPTS {
                match restart_listener_after_time_wait(bind.parse().unwrap()).await {
                    Ok(()) => break,
                    // Another parallel test can claim the port after we drop
                    // the listener. Retry the entire FIN exchange on a fresh
                    // ephemeral port; broken SO_REUSEADDR fails every attempt.
                    Err(error) if error == addr_in_use && attempt < MAX_ATTEMPTS => continue,
                    Err(error) => panic!(
                        "HTTP must rebind over TIME_WAIT for {bind} (attempt {attempt}/{MAX_ATTEMPTS}): {error}"
                    ),
                }
            }
        }
    }

    #[cfg(unix)]
    async fn restart_listener_after_time_wait(bind: std::net::SocketAddr) -> Result<(), String> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = prepare_listener(bind, |_| async { Ok(()) }).await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        let (mut server, _) = listener.accept().await.unwrap();
        // The server actively closes first, so its port retains TIME_WAIT
        // after both halves finish their FIN exchange.
        server.shutdown().await.unwrap();
        let mut byte = [0];
        assert_eq!(client.read(&mut byte).await.unwrap(), 0);
        client.shutdown().await.unwrap();
        assert_eq!(server.read(&mut byte).await.unwrap(), 0);
        drop(server);
        drop(client);
        drop(listener);

        let restarted = prepare_listener(address, |_| async { Ok(()) }).await?;
        assert_eq!(restarted.local_addr().unwrap(), address);
        Ok(())
    }

    #[test]
    fn deactivation_retry_log_names_its_phase_without_claiming_services_are_running() {
        let message = retry_message(
            "deactivation",
            ProofError::Unavailable,
            Duration::from_secs(2),
        );
        assert!(
            message.starts_with("HTTP endpoint deactivation remains pending:"),
            "{message}"
        );
        assert!(!message.contains("HTTP is off"), "{message}");
        assert!(!message.contains("agents continue"), "{message}");
    }

    #[test]
    fn conflict_retry_log_requires_out_of_band_recovery() {
        for phase in ["registration", "deactivation"] {
            let message = retry_message(phase, ProofError::Conflict, Duration::from_secs(300));
            assert!(message.contains(phase), "{message}");
            assert!(message.contains("out-of-band"), "{message}");
        }
        let transient = retry_message(
            "registration",
            ProofError::Unavailable,
            Duration::from_secs(2),
        );
        assert!(!transient.contains("out-of-band"), "{transient}");
    }

    #[tokio::test(start_paused = true)]
    async fn persistent_conflict_retries_with_a_cap_without_rotating_the_prepared_generation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("endpoint.json");
        let acknowledged = EndpointRecord::prepare(&path, Some(record().policy))
            .unwrap()
            .unwrap();
        acknowledged.acknowledge(&path).unwrap();
        let mut policy = acknowledged.policy;
        policy.certificate_fingerprint = "new-certificate".into();
        let prepared = EndpointRecord::prepare(&path, Some(policy))
            .unwrap()
            .unwrap();
        let original_bytes = std::fs::read(&path).unwrap();
        let attempts = std::sync::Mutex::new(Vec::new());
        let started = tokio::time::Instant::now();

        retry_registration(&prepared, "registration", |candidates| {
            assert_eq!(std::fs::read(&path).unwrap(), original_bytes);
            assert_eq!(candidates, prepared.registration_predecessors());
            let mut attempts = attempts.lock().unwrap();
            attempts.push(started.elapsed().as_secs());
            std::future::ready(if attempts.len() <= 12 {
                Err(ProofError::Conflict)
            } else {
                Ok(())
            })
        })
        .await;

        assert_eq!(
            *attempts.lock().unwrap(),
            vec![0, 2, 6, 14, 30, 62, 126, 254, 510, 810, 1110, 1410, 1710]
        );
        assert_eq!(std::fs::read(&path).unwrap(), original_bytes);
        assert_eq!(prepared.unacknowledged_epochs, Vec::<String>::new());
    }

    #[tokio::test(start_paused = true)]
    async fn every_registration_failure_keeps_http_off_until_a_retry_succeeds() {
        let outcomes = std::sync::Mutex::new(VecDeque::from([
            Err(ProofError::Conflict),
            Err(ProofError::Refused),
            Err(ProofError::Unavailable),
            Err(ProofError::Unavailable),
            Ok(()),
        ]));
        let (stop, mut shutdown) = watch::channel(false);
        let ready = std::sync::atomic::AtomicBool::new(false);
        let prepare = async {
            retry_registration(&record(), "registration", |_| {
                std::future::ready(outcomes.lock().unwrap().pop_front().unwrap())
            })
            .await;
            ready.store(true, std::sync::atomic::Ordering::SeqCst);
        };
        tokio::pin!(prepare);
        assert!(tokio::time::timeout(Duration::from_secs(5), &mut prepare)
            .await
            .is_err());
        assert!(!ready.load(std::sync::atomic::Ordering::SeqCst));
        prepare.await;
        assert!(ready.load(std::sync::atomic::Ordering::SeqCst));
        assert!(outcomes.lock().unwrap().is_empty());
        stop.send(true).unwrap();
        wait_for_shutdown(&mut shutdown).await;
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_cancels_pending_registration_without_waiting_for_backoff_or_network() {
        for stall_network in [false, true] {
            let (stop, mut shutdown) = watch::channel(false);
            let record = record();
            let pending = retry_registration(&record, "registration", |_| async {
                if stall_network {
                    std::future::pending::<()>().await;
                }
                Err(ProofError::Conflict)
            });
            tokio::pin!(pending);
            assert!(tokio::time::timeout(Duration::from_secs(1), &mut pending)
                .await
                .is_err());
            stop.send(true).unwrap();
            assert!(prepare_or_shutdown(pending, &mut shutdown).await.is_none());
        }
    }

    #[tokio::test]
    async fn lost_success_response_is_confirmed_with_the_desired_epoch() {
        let (result, attempts) = reconcile_script(vec![Err(ProofError::Unavailable), Ok(())]).await;
        assert_eq!(result, Ok(()));
        assert_eq!(
            attempts,
            vec![
                vec![Some("previous".into()), Some("desired".into())],
                vec![Some("previous".into()), Some("desired".into())]
            ]
        );
    }

    #[tokio::test]
    async fn crash_after_hosted_commit_is_confirmed_by_the_candidate_set() {
        let (result, attempts) = reconcile_script(vec![Ok(())]).await;
        assert_eq!(result, Ok(()));
        assert_eq!(
            attempts,
            vec![vec![Some("previous".into()), Some("desired".into())]]
        );
    }

    #[tokio::test]
    async fn unconfirmed_registration_preserves_the_original_failure() {
        let (result, attempts) =
            reconcile_script(vec![Err(ProofError::Unavailable), Err(ProofError::Refused)]).await;
        assert_eq!(result, Err(ProofError::Unavailable));
        assert_eq!(
            attempts,
            vec![
                vec![Some("previous".into()), Some("desired".into())],
                vec![Some("previous".into()), Some("desired".into())]
            ]
        );
    }

    #[tokio::test]
    async fn authorization_refusal_is_never_retried_as_reconciliation() {
        let (result, attempts) = reconcile_script(vec![Err(ProofError::Refused)]).await;
        assert_eq!(result, Err(ProofError::Refused));
        assert_eq!(
            attempts,
            vec![vec![Some("previous".into()), Some("desired".into())]]
        );
    }

    #[tokio::test]
    async fn newer_rotation_or_disable_cannot_be_overwritten_by_stale_confirmation() {
        let (result, attempts) = reconcile_script(vec![Err(ProofError::Conflict)]).await;
        assert_eq!(result, Err(ProofError::Conflict));
        assert_eq!(
            attempts,
            vec![vec![Some("previous".into()), Some("desired".into())]]
        );
    }

    #[tokio::test]
    async fn changed_intent_reconciles_an_uncertain_prior_commit_without_overwriting_newer_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("endpoint.json");
        let acknowledged = EndpointRecord::prepare(&path, Some(record().policy))
            .unwrap()
            .unwrap();
        acknowledged.acknowledge(&path).unwrap();

        let mut rotated_policy = acknowledged.policy.clone();
        rotated_policy.certificate_fingerprint = "rotated-certificate".into();
        let uncertain = EndpointRecord::prepare(&path, Some(rotated_policy))
            .unwrap()
            .unwrap();
        let disabled = EndpointRecord::prepare(&path, None).unwrap().unwrap();

        let hosted = std::sync::Mutex::new(Some(uncertain.endpoint_epoch.clone()));
        let reconciled = reconcile_registration(&disabled, |predecessors| {
            let mut current = hosted.lock().unwrap();
            if predecessors.contains(&current.clone()) {
                *current = Some(disabled.endpoint_epoch.clone());
                std::future::ready(Ok(()))
            } else {
                std::future::ready(Err(ProofError::Conflict))
            }
        })
        .await;
        assert_eq!(reconciled, Ok(()));
        assert_eq!(
            *hosted.lock().unwrap(),
            Some(disabled.endpoint_epoch.clone())
        );

        let external = "external-newer-generation".to_string();
        *hosted.lock().unwrap() = Some(external.clone());
        let refused = reconcile_registration(&disabled, |predecessors| {
            let current = hosted.lock().unwrap();
            std::future::ready(if predecessors.contains(&current.clone()) {
                Ok(())
            } else {
                Err(ProofError::Conflict)
            })
        })
        .await;
        assert_eq!(refused, Err(ProofError::Conflict));
        assert_eq!(*hosted.lock().unwrap(), Some(external));
    }

    #[test]
    fn unresolved_generation_bound_fails_before_discarding_recovery_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("endpoint.json");
        let acknowledged = EndpointRecord::prepare(&path, Some(record().policy))
            .unwrap()
            .unwrap();
        acknowledged.acknowledge(&path).unwrap();

        let mut last = acknowledged;
        for generation in 0..7 {
            let mut policy = last.policy.clone();
            policy.certificate_fingerprint = format!("certificate-{generation}");
            last = EndpointRecord::prepare(&path, Some(policy))
                .unwrap()
                .unwrap();
        }
        assert_eq!(last.registration_predecessors().len(), 8);

        let mut rejected_policy = last.policy.clone();
        rejected_policy.certificate_fingerprint = "one-generation-too-many".into();
        assert_eq!(
            EndpointRecord::prepare(&path, Some(rejected_policy)).unwrap_err(),
            "HTTP endpoint has too many unresolved registration generations"
        );
        let preserved = EndpointRecord::prepare(&path, Some(last.policy.clone()))
            .unwrap()
            .unwrap();
        assert_eq!(preserved.endpoint_epoch, last.endpoint_epoch);
        assert_eq!(preserved.registration_predecessors().len(), 8);

        preserved.acknowledge(&path).unwrap();
        let acknowledged = EndpointRecord::prepare(&path, Some(last.policy))
            .unwrap()
            .unwrap();
        assert!(acknowledged.unacknowledged_epochs.is_empty());
        assert_eq!(
            acknowledged.registration_predecessors(),
            vec![Some(acknowledged.endpoint_epoch.clone())]
        );
    }

    #[test]
    fn predecessor_candidates_never_cross_device_or_issuer_authority() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("endpoint.json");
        let acknowledged = EndpointRecord::prepare(&path, Some(record().policy))
            .unwrap()
            .unwrap();
        acknowledged.acknowledge(&path).unwrap();
        let mut pending_policy = acknowledged.policy.clone();
        pending_policy.certificate_fingerprint = "pending".into();
        EndpointRecord::prepare(&path, Some(pending_policy))
            .unwrap()
            .unwrap();

        let mut other_issuer = acknowledged.policy;
        other_issuer.issuer = "https://other-account.example".into();
        let replacement = EndpointRecord::prepare(&path, Some(other_issuer))
            .unwrap()
            .unwrap();
        assert_eq!(
            replacement.registration_predecessors(),
            vec![None, Some(replacement.endpoint_epoch.clone())]
        );
    }
}
