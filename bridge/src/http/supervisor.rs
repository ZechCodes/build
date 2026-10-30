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
        // Reserve the configured address before publishing readiness. Merely
        // binding does not accept traffic; if hosted registration fails the
        // socket is dropped, and a later bind failure can never leave a
        // freshly advertised endpoint behind.
        let listener = tokio::net::TcpListener::bind(settings.bind)
            .await
            .map_err(|error| format!("cannot bind HTTP listener: {error}"))?;
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
        let registrar = EndpointRegistrar::new(signer)
            .map_err(|error| format!("cannot construct endpoint registrar: {error}"))?;
        register_generation(&registrar, &record, true)
            .await
            .map_err(|error| format!("HTTP endpoint registration failed: {error}"))?;
        acknowledge_record(&state_path, record).await?;

        let runtime = runtime::mount(execution, verifier);
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
pub async fn reconcile_disabled(identity: &StoredIdentity, state_path: PathBuf) -> Result<(), String> {
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
    register_generation(&registrar, &record, false)
        .await
        .map_err(|error| format!("HTTP endpoint deactivation remains pending: {error}"))?;
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
            register_generation(&registrar, &record, false)
                .await
                .map_err(|error| error.to_string())?;
            acknowledge_record(&self.state_path, record).await
        }
        .await;
        if let Err(error) = result {
            eprintln!("HTTP endpoint deactivation remains pending: {error}");
        }
    }
}

/// Register from the last locally acknowledged predecessor. If the hosted
/// commit succeeded but its response or the subsequent local acknowledge was
/// lost, that predecessor is stale; a second proof naming the desired epoch as
/// its own predecessor is an idempotent confirmation of exactly that hosted
/// generation. It cannot advance a host that still has the old predecessor.
async fn register_generation(
    registrar: &EndpointRegistrar,
    record: &EndpointRecord,
    enabled: bool,
) -> Result<(), ProofError> {
    reconcile_registration(record, |previous| async move {
        registrar
            .register(previous.as_deref(), enabled)
            .await
            .map(|_| ())
    })
    .await
}

async fn reconcile_registration<F, Fut>(
    record: &EndpointRecord,
    mut register: F,
) -> Result<(), ProofError>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: std::future::Future<Output = Result<(), ProofError>>,
{
    match register(record.registered_epoch.clone()).await {
        Ok(()) => Ok(()),
        Err(original @ (ProofError::Unavailable | ProofError::Conflict)) => {
            register(Some(record.endpoint_epoch.clone()))
                .await
                .map_err(|_| original)
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
        }
    }

    async fn reconcile_script(
        outcomes: Vec<Result<(), ProofError>>,
    ) -> (Result<(), ProofError>, Vec<Option<String>>) {
        let outcomes = std::sync::Mutex::new(VecDeque::from(outcomes));
        let attempts = std::sync::Mutex::new(Vec::new());
        let result = reconcile_registration(&record(), |previous| {
            attempts.lock().unwrap().push(previous);
            std::future::ready(outcomes.lock().unwrap().pop_front().unwrap())
        })
        .await;
        (result, attempts.into_inner().unwrap())
    }

    #[tokio::test]
    async fn lost_success_response_is_confirmed_with_the_desired_epoch() {
        let (result, attempts) = reconcile_script(vec![Err(ProofError::Unavailable), Ok(())]).await;
        assert_eq!(result, Ok(()));
        assert_eq!(
            attempts,
            vec![Some("previous".into()), Some("desired".into())]
        );
    }

    #[tokio::test]
    async fn crash_after_hosted_commit_is_confirmed_after_old_predecessor_conflict() {
        let (result, attempts) = reconcile_script(vec![Err(ProofError::Conflict), Ok(())]).await;
        assert_eq!(result, Ok(()));
        assert_eq!(
            attempts,
            vec![Some("previous".into()), Some("desired".into())]
        );
    }

    #[tokio::test]
    async fn unconfirmed_registration_preserves_the_original_failure() {
        let (result, attempts) = reconcile_script(vec![
            Err(ProofError::Unavailable),
            Err(ProofError::Refused),
        ])
        .await;
        assert_eq!(result, Err(ProofError::Unavailable));
        assert_eq!(
            attempts,
            vec![Some("previous".into()), Some("desired".into())]
        );
    }

    #[tokio::test]
    async fn authorization_refusal_is_never_retried_as_reconciliation() {
        let (result, attempts) = reconcile_script(vec![Err(ProofError::Refused)]).await;
        assert_eq!(result, Err(ProofError::Refused));
        assert_eq!(attempts, vec![Some("previous".into())]);
    }

    #[tokio::test]
    async fn newer_rotation_or_disable_cannot_be_overwritten_by_stale_confirmation() {
        let (result, attempts) = reconcile_script(vec![
            Err(ProofError::Conflict),
            Err(ProofError::Conflict),
        ])
        .await;
        assert_eq!(result, Err(ProofError::Conflict));
        assert_eq!(
            attempts,
            vec![Some("previous".into()), Some("desired".into())]
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
        let reconciled = reconcile_registration(&disabled, |previous| {
            let mut current = hosted.lock().unwrap();
            if previous.as_ref() == current.as_ref() {
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
        let refused = reconcile_registration(&disabled, |previous| {
            let current = hosted.lock().unwrap();
            std::future::ready(if previous.as_ref() == current.as_ref() {
                Ok(())
            } else {
                Err(ProofError::Conflict)
            })
        })
        .await;
        assert_eq!(refused, Err(ProofError::Conflict));
        assert_eq!(*hosted.lock().unwrap(), Some(external));
    }
}
