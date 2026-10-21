//! Isolated provider-opening seam for actual qualification RPC tests.
//!
//! Test-first scaffold: no opening hook is installed until the actual-RPC
//! regression has demonstrated its missing factory call.

use crate::harness::{HarnessError, OpenedSession, SessionOpenRequest};
use crate::models::AgentProvider;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) type SessionFactory = Arc<
    dyn Fn(AgentProvider, SessionOpenRequest) -> Result<OpenedSession, HarnessError> + Send + Sync,
>;

pub(crate) struct FixtureSessions;

impl FixtureSessions {
    pub(crate) fn install(_owned_root: PathBuf, _factory: SessionFactory) -> Self {
        Self
    }
}

pub(crate) fn open(
    _registry_root: &Path,
    provider: AgentProvider,
    request: SessionOpenRequest,
) -> Result<OpenedSession, HarnessError> {
    crate::harness::open_session(provider, request)
}
