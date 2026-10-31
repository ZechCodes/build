//! Account-bound HTTP authority, minted exclusively from the configured hosted API.
//!
//! Bearers live only in an outstanding request. The bounded cache holds their
//! SHA-256 digests and verified evidence; domain handlers receive neither.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{watch, Semaphore};
use tokio::time::Instant;

use super::discovery::DeviceProofSigner;

#[path = "auth/cache.rs"]
mod cache;
#[path = "auth/client.rs"]
mod client;
#[path = "auth/grant.rs"]
mod grant;

#[cfg(test)]
use client::hex_digest;
use client::{token_digest, validate_destination};
use grant::{Evidence, TokenState};
pub use grant::{HttpGrantBinding, VerifiedHttpGrant};

const CACHE_LIMIT: usize = 1024;
const NETWORK_LIMIT: usize = 8;
const WAITER_LIMIT: usize = 64;
const NETWORK_DEADLINE: Duration = Duration::from_secs(2);
const FRESHNESS: Duration = Duration::from_secs(30);
const REFRESH_AFTER: Duration = Duration::from_secs(20);
const RESPONSE_LIMIT: usize = 16 * 1024;
const TOKEN_LIMIT: usize = 2048;

/// How long hosted evidence stays fresh. Production always uses
/// `FRESHNESS`/`REFRESH_AFTER`; only `#[cfg(test)]` code can shorten them.
#[derive(Clone, Copy)]
struct Freshness {
    window: Duration,
    refresh_after: Duration,
}

impl Freshness {
    const PRODUCTION: Self = Self {
        window: FRESHNESS,
        refresh_after: REFRESH_AFTER,
    };
}

type TokenDigest = [u8; 32];
type Verification = Result<Arc<Evidence>, AuthError>;
type Pending = watch::Receiver<Option<Verification>>;

/// Deliberately contains no upstream error, URL, response body or credential.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthError {
    InvalidCredential,
    Expired,
    Closed,
    BindingMismatch,
    ForbiddenScope,
    Unavailable,
    Overloaded,
    InvalidConfiguration,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidCredential => "invalid HTTP credential",
            Self::Expired => "HTTP authorization expired",
            Self::Closed => "HTTP authorization closed",
            Self::BindingMismatch => "HTTP authorization binding changed",
            Self::ForbiddenScope => "HTTP scope denied",
            Self::Unavailable => "hosted authorization unavailable",
            Self::Overloaded => "HTTP authorization capacity exceeded",
            Self::InvalidConfiguration => "invalid hosted authorization configuration",
        })
    }
}

impl std::error::Error for AuthError {}

// Kept private: only the configured paired-device signer can authenticate the
// production verifier. Tests substitute a signer without production bypasses.
trait AssertionSigner: Send + Sync {
    fn issuer(&self) -> &str;
    fn device_id(&self) -> &str;
    fn origin(&self) -> &str;
    fn endpoint_epoch(&self) -> &str;
    fn introspection_url(&self) -> String;
    fn assertion(&self, token_hash: &str, now: u64) -> Result<String, AuthError>;
}

impl AssertionSigner for DeviceProofSigner {
    fn issuer(&self) -> &str {
        self.issuer()
    }
    fn device_id(&self) -> &str {
        self.device_id()
    }
    fn origin(&self) -> &str {
        self.origin()
    }
    fn endpoint_epoch(&self) -> &str {
        self.endpoint_epoch()
    }
    fn introspection_url(&self) -> String {
        self.introspection_url()
    }
    fn assertion(&self, hash: &str, now: u64) -> Result<String, AuthError> {
        self.introspection_assertion(hash, now)
            .map_err(|_| AuthError::Unavailable)
    }
}

#[derive(Clone)]
pub struct HttpAuthVerifier {
    inner: Arc<VerifierInner>,
}

struct VerifierInner {
    closed: Arc<AtomicBool>,
    signer: Arc<dyn AssertionSigner>,
    client: reqwest::Client,
    cache: Mutex<HashMap<TokenDigest, CacheEntry>>,
    network: Arc<Semaphore>,
    waiters: Arc<Semaphore>,
    jobs: Arc<Semaphore>,
    freshness: Freshness,
}

struct CacheEntry {
    state: Arc<TokenState>,
    evidence: Option<Arc<Evidence>>,
    pending: Option<Pending>,
}

enum Prepared {
    Ready(Arc<Evidence>),
    Pending(Pending, tokio::sync::OwnedSemaphorePermit),
}

impl HttpAuthVerifier {
    /// The destination comes exclusively from the approved device configuration.
    pub fn new(signer: Arc<DeviceProofSigner>) -> Result<Self, AuthError> {
        validate_destination(signer.as_ref())?;
        let client = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .referer(false)
            .connect_timeout(NETWORK_DEADLINE)
            .timeout(NETWORK_DEADLINE)
            .build()
            .map_err(|_| AuthError::InvalidConfiguration)?;
        Ok(Self::configured(signer, client))
    }

    fn configured(signer: Arc<dyn AssertionSigner>, client: reqwest::Client) -> Self {
        Self {
            inner: Arc::new(VerifierInner {
                closed: Arc::new(AtomicBool::new(false)),
                signer,
                client,
                cache: Mutex::new(HashMap::new()),
                network: Arc::new(Semaphore::new(NETWORK_LIMIT)),
                waiters: Arc::new(Semaphore::new(WAITER_LIMIT)),
                jobs: Arc::new(Semaphore::new(WAITER_LIMIT)),
                freshness: Freshness::PRODUCTION,
            }),
        }
    }

    /// Immediately fence every grant and in-flight lookup when HTTP is disabled
    /// or its configured endpoint is replaced. This verifier cannot be reopened.
    pub fn close(&self) {
        self.inner.closed.store(true, Ordering::Release);
    }

    pub async fn verify(&self, token: &str) -> Result<VerifiedHttpGrant, AuthError> {
        VerifiedHttpGrant::mint(self.evidence(token, false).await?)
    }

    /// Verifies afresh with hosted, never from cached evidence: how a live
    /// opening renews. A hosted refusal revokes every grant from this token.
    pub async fn revalidate(&self, token: &str) -> Result<VerifiedHttpGrant, AuthError> {
        VerifiedHttpGrant::mint(self.evidence(token, true).await?)
    }

    /// Proactively call at `refresh_at()`, including with a rotated family token.
    /// Failure leaves only the original, unchanged hard deadline in force.
    pub async fn refresh(
        &self,
        grant: &VerifiedHttpGrant,
        token: &str,
    ) -> Result<VerifiedHttpGrant, AuthError> {
        grant.check()?;
        {
            let previous = grant
                .lease
                .evidence
                .lock()
                .map_err(|_| AuthError::Unavailable)?;
            if !Arc::ptr_eq(&previous.token_state.verifier_closed, &self.inner.closed) {
                return Err(AuthError::BindingMismatch);
            }
        }
        let evidence = self.evidence(token, true).await?;
        if evidence.binding != grant.binding {
            grant.close();
            return Err(AuthError::BindingMismatch);
        }
        let mut previous = grant
            .lease
            .evidence
            .lock()
            .map_err(|_| AuthError::Unavailable)?;
        grant.check_evidence(&previous)?;
        if evidence.token_state.revoked.load(Ordering::Acquire)
            || Instant::now() >= evidence.deadline
        {
            return Err(AuthError::Expired);
        }
        *previous = evidence;
        Ok(grant.clone())
    }
}

#[cfg(test)]
#[path = "auth/tests.rs"]
mod tests;
