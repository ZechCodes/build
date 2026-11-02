use super::AuthError;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::time::Instant;

/// Immutable session identity. Rotation changes token ID and expiry, not this.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpGrantBinding {
    pub(super) issuer: String,
    pub(super) account_id: String,
    pub(super) device_id: String,
    pub(super) origin: String,
    pub(super) endpoint_epoch: String,
    pub(super) authorization_epoch: u64,
    pub(super) family_id: String,
    pub(super) scopes: Vec<String>,
}

impl HttpGrantBinding {
    pub fn issuer(&self) -> &str {
        &self.issuer
    }
    pub fn account_id(&self) -> &str {
        &self.account_id
    }
    pub fn device_id(&self) -> &str {
        &self.device_id
    }
    pub fn origin(&self) -> &str {
        &self.origin
    }
    pub fn endpoint_epoch(&self) -> &str {
        &self.endpoint_epoch
    }
    pub fn authorization_epoch(&self) -> u64 {
        self.authorization_epoch
    }
    pub fn family_id(&self) -> &str {
        &self.family_id
    }
    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }
}

/// Opaque, privately minted authority. Clones share an irreversible close fence.
#[derive(Clone)]
pub struct VerifiedHttpGrant {
    pub(super) binding: HttpGrantBinding,
    pub(super) lease: Arc<Lease>,
}

pub(super) struct Lease {
    pub(super) evidence: Mutex<Arc<Evidence>>,
    pub(super) closed: AtomicBool,
}

pub(super) struct Evidence {
    pub(super) binding: HttpGrantBinding,
    pub(super) deadline: Instant,
    pub(super) refresh_at: Instant,
    pub(super) token_state: Arc<TokenState>,
}

pub(super) struct TokenState {
    pub(super) verifier_closed: Arc<AtomicBool>,
    pub(super) revoked: AtomicBool,
}

impl VerifiedHttpGrant {
    pub(super) fn mint(evidence: Arc<Evidence>) -> Result<Self, AuthError> {
        let grant = Self {
            binding: evidence.binding.clone(),
            lease: Arc::new(Lease {
                evidence: Mutex::new(evidence),
                closed: AtomicBool::new(false),
            }),
        };
        grant.check()?;
        Ok(grant)
    }

    pub fn binding(&self) -> &HttpGrantBinding {
        &self.binding
    }
    pub fn principal(&self) -> &'static str {
        "device_user"
    }
    pub fn account_id(&self) -> &str {
        self.binding.account_id()
    }

    /// Check at admission and immediately before handing each output to a socket.
    pub fn check(&self) -> Result<(), AuthError> {
        let evidence = self
            .lease
            .evidence
            .lock()
            .map_err(|_| AuthError::Unavailable)?;
        self.check_evidence(&evidence)
    }

    pub(super) fn check_evidence(&self, evidence: &Evidence) -> Result<(), AuthError> {
        if evidence.token_state.verifier_closed.load(Ordering::Acquire) {
            self.close();
            return Err(AuthError::Closed);
        }
        if self.lease.closed.load(Ordering::Acquire) {
            return Err(AuthError::Closed);
        }
        if evidence.token_state.revoked.load(Ordering::Acquire) {
            self.close();
            return Err(AuthError::InvalidCredential);
        }
        if Instant::now() >= evidence.deadline {
            self.close();
            return Err(AuthError::Expired);
        }
        Ok(())
    }

    pub fn ensure_scope(&self, required: &str) -> Result<(), AuthError> {
        self.check()?;
        if self.binding.scopes.iter().any(|scope| scope == required) {
            Ok(())
        } else {
            Err(AuthError::ForbiddenScope)
        }
    }

    /// Callers arm an independent timer at this deadline; refresh never revives
    /// a closed opening. The timer must re-read the deadline after renewal.
    pub fn deadline(&self) -> Instant {
        self.lease
            .evidence
            .lock()
            .map(|e| e.deadline)
            .unwrap_or_else(|_| Instant::now())
    }

    pub fn refresh_at(&self) -> Instant {
        self.lease
            .evidence
            .lock()
            .map(|e| e.refresh_at)
            .unwrap_or_else(|_| Instant::now())
    }

    pub fn close(&self) {
        self.lease.closed.store(true, Ordering::Release);
    }
}
