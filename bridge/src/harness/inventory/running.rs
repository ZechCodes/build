//! The CLI versions live sessions say they run.
//!
//! A session that reports its version (the ADK init line, the codex app
//! server's `initialize`) holds a [`ReportedVersion`] for as long as it runs;
//! the last clone dropped takes the report away. So the inventory lists what
//! is running now, beside but never instead of what is installed.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use super::model::RunningVersion as RunningVersionRow;
use crate::models::AgentProvider;

type Listener = Box<dyn Fn() + Send + Sync>;

#[derive(Default)]
pub struct RunningSessions {
    sessions: Mutex<BTreeMap<u64, (AgentProvider, String)>>,
    next: AtomicU64,
    changes: AtomicU64,
    listeners: Mutex<Vec<Listener>>,
}

/// One live session's report. Cloned with the session state that holds it;
/// dropped with the last clone.
#[derive(Clone)]
pub struct ReportedVersion(#[allow(dead_code)] Arc<Registration>);

impl std::fmt::Debug for ReportedVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ReportedVersion")
    }
}

struct Registration {
    registry: Weak<RunningSessions>,
    id: u64,
}

impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            registry.sessions.lock().unwrap().remove(&self.id);
            registry.changed();
        }
    }
}

impl RunningSessions {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A session of `provider` says it runs `version`, until the answer is
    /// dropped.
    pub fn report(self: &Arc<Self>, provider: AgentProvider, version: String) -> ReportedVersion {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.sessions
            .lock()
            .unwrap()
            .insert(id, (provider, version));
        self.changed();
        ReportedVersion(Arc::new(Registration {
            registry: Arc::downgrade(self),
            id,
        }))
    }

    /// Each version `provider`'s live sessions reported, with how many did.
    pub fn versions_of(&self, provider: AgentProvider) -> Vec<RunningVersionRow> {
        let mut counts: BTreeMap<String, u32> = BTreeMap::new();
        for (session_provider, version) in self.sessions.lock().unwrap().values() {
            if *session_provider == provider {
                *counts.entry(version.clone()).or_default() += 1;
            }
        }
        counts
            .into_iter()
            .map(|(version, sessions)| RunningVersionRow { version, sessions })
            .collect()
    }

    /// How many reports have come and gone.
    pub(super) fn change_count(&self) -> u64 {
        self.changes.load(Ordering::Acquire)
    }

    pub(super) fn on_change(&self, listener: Listener) {
        self.listeners.lock().unwrap().push(listener);
    }

    fn changed(&self) {
        self.changes.fetch_add(1, Ordering::AcqRel);
        for listener in self.listeners.lock().unwrap().iter() {
            listener();
        }
    }
}

/// The sessions this process runs.
pub fn running_sessions() -> &'static Arc<RunningSessions> {
    static RUNNING: OnceLock<Arc<RunningSessions>> = OnceLock::new();
    RUNNING.get_or_init(RunningSessions::new)
}

/// A live session of `provider` reported `reported` as its version: held for
/// the inventory while the session runs, and passed on to the installed-CLI
/// readings, which ask the CLI again when it differs from theirs.
pub fn report_running_version(provider: AgentProvider, reported: &str) -> Option<ReportedVersion> {
    let version = crate::harness::installed::probe::version_in(reported)?;
    crate::harness::installed::observe_version(provider, reported);
    Some(running_sessions().report(provider, version.to_string()))
}
