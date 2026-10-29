//! Owner proof fixtures for adapter integration tests, never a wire auth path.

use super::session::{Opening, SessionLease, VerifiedCaller};
use super::ExecutionService;

/// A verified owner opening with no carrier, key or HTTP credential fabrication.
/// Real HTTP authentication tests must use the verifier adapter instead.
pub fn owner_session(service: &ExecutionService) -> SessionLease {
    service
        .sessions()
        .open(VerifiedCaller::verified_relay(
            "test-device",
            Opening::new(),
        ))
        .expect("fresh owner fixture opens")
}
