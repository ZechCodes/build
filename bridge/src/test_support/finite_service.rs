//! Opt-in finite service controls for isolated qualification fixtures.
//!
//! Test-first scaffold. The next stage adds root registration, original-ID
//! lookup, actual service accounting, and calls at the real execution sites.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ServiceKind {
    Filesystem,
    Git,
}

impl ServiceKind {
    pub(crate) fn ceiling(self) -> Duration {
        Duration::from_millis(match self {
            Self::Filesystem => 250,
            Self::Git => 500,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PlannedService {
    pub original_id: String,
    pub kind: ServiceKind,
    pub target: Duration,
}

#[derive(Clone, Debug)]
pub(crate) struct ServiceSample {
    pub original_id: String,
    pub kind: ServiceKind,
    pub target: Duration,
    pub actual: Duration,
    pub succeeded: bool,
    pub abandoned: bool,
}

impl ServiceSample {
    pub(crate) fn exceeded_ceiling(&self) -> bool {
        self.actual > self.kind.ceiling()
    }
}

pub(crate) type EntryObserver = Arc<dyn Fn(&str, ServiceKind) + Send + Sync>;

/// Owned fixture lifetime. The eventual Drop removes only this registration.
pub(crate) struct FixtureServices;

impl FixtureServices {
    pub(crate) fn install(
        _roots: Vec<PathBuf>,
        _plans: Vec<PlannedService>,
        _observer: Option<EntryObserver>,
    ) -> Self {
        Self
    }

    pub(crate) fn samples(&self) -> Vec<ServiceSample> {
        Vec::new()
    }
}

/// Keeps the actual Result intact. Unregistered roots always take this path.
pub(crate) fn run<T, E>(
    _root: &Path,
    _kind: ServiceKind,
    work: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    work()
}
