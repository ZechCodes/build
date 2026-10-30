//! Production construction and lifecycle for the optional HTTPS listener.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use super::auth::HttpAuthVerifier;
use super::discovery::{CanonicalOrigin, DeviceProofSigner, EndpointRegistrar};
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
        let tls = super::tls::load(&settings.tls_cert, &settings.tls_key)?;
        let certificate_fingerprint = super::tls::certificate_fingerprint(&settings.tls_cert)?;
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
        registrar
            .register(record.registered_epoch.as_deref(), true)
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
    EndpointRegistrar::new(signer)
        .map_err(|error| format!("cannot construct endpoint registrar: {error}"))?
        .register(record.registered_epoch.as_deref(), false)
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
            EndpointRegistrar::new(signer)
                .map_err(|error| error.to_string())?
                .register(record.registered_epoch.as_deref(), false)
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
