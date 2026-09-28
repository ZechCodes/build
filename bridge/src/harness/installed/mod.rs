//! What the agent CLIs installed on this machine say they can run.
//!
//! A harness's catalog is what Build knows about a provider's models; whether
//! the CLI on this machine can run each of them is a separate fact, and a
//! changing one — mise and the CLIs' own updaters replace the binary behind a
//! `PATH` entry that never changes itself (#203). So each CLI is asked (see
//! [`probe`]), the answer is kept here as a [`CliReading`], and what a harness
//! offers is its catalog seen through that reading (see [`offer`]).
//!
//! Nothing that answers a client waits on a CLI: [`Readings::reading`] answers
//! from what it holds and asks again in the background once the answer is
//! older than [`READING_TTL`], and a session that reports its own version
//! ([`observe_version`]) asks again at once when it differs. Every change is
//! counted on [`Readings::changes`], which the `models.changed` push follows.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use semver::Version;
use tokio::sync::watch;

use crate::harness::harness_for;
use crate::models::AgentProvider;

pub(crate) mod offer;
pub(crate) mod probe;

pub use offer::{ModelOffer, OfferedModel, UnavailableModel};
pub use probe::{CliProbe, CODEX_MODEL_LIST, NO_PROBE, VERSION_FLAG};

/// How long an answer stands before the next ask for it asks the CLI again.
pub const READING_TTL: Duration = Duration::from_secs(10 * 60);

/// What one installed CLI said about itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CliReading {
    /// Its version, where it said one Build could read.
    pub version: Option<Version>,
    /// The models it lists as its own, where it can list them. Every one,
    /// hidden included: what a picker offers and what a session may be started
    /// on are two different questions.
    pub listed: Option<Vec<ListedModel>>,
}

/// One model a CLI lists as its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedModel {
    pub id: String,
    pub label: String,
    /// Kept out of the CLI's own picker, and so out of Build's.
    pub hidden: bool,
    pub efforts: Vec<String>,
}

/// Where a background ask runs.
type Scheduler = Arc<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;
type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

#[derive(Default)]
struct Entry {
    reading: Option<Arc<CliReading>>,
    read_at: Option<Instant>,
    asking: bool,
}

/// Every CLI's latest answer, by the binary that gave it.
pub struct Readings {
    entries: Mutex<HashMap<&'static str, Entry>>,
    ttl: Duration,
    schedule: Option<Scheduler>,
    now: Clock,
    changed: watch::Sender<u64>,
}

impl Readings {
    /// Readings that ask on a thread of their own.
    fn background() -> Arc<Self> {
        Self::with(
            Some(Arc::new(|ask: Box<dyn FnOnce() + Send>| {
                let spawned = std::thread::Builder::new()
                    .name("cli-probe".to_string())
                    .spawn(ask);
                if let Err(error) = spawned {
                    eprintln!("cli probe: cannot start a probe thread: {error}");
                }
            })),
            Arc::new(Instant::now),
            READING_TTL,
        )
    }

    /// Readings that never ask, so every catalog is served whole. What the
    /// unit tests' process-wide instance is: a test that means to probe builds
    /// its own, and no other test starts a real CLI by listing a catalog.
    #[cfg(test)]
    fn inert() -> Arc<Self> {
        Self::with(None, Arc::new(Instant::now), READING_TTL)
    }

    fn with(schedule: Option<Scheduler>, now: Clock, ttl: Duration) -> Arc<Self> {
        Arc::new(Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
            schedule,
            now,
            changed: watch::channel(0).0,
        })
    }

    /// What `binary` last said, asking it again in the background when that is
    /// older than the TTL or has never been asked. `None` until the first
    /// answer lands.
    pub fn reading(
        self: &Arc<Self>,
        binary: &'static str,
        probe: &'static dyn CliProbe,
    ) -> Option<Arc<CliReading>> {
        let stale = {
            let mut entries = self.entries.lock().unwrap();
            let entry = entries.entry(binary).or_default();
            let expired = entry
                .read_at
                .is_none_or(|read_at| (self.now)().duration_since(read_at) >= self.ttl);
            self.claim_ask(entry, expired)
        };
        if stale {
            self.ask(binary, probe);
        }
        self.held(binary)
    }

    /// A session of `binary` says it runs `version`: when that is not what the
    /// reading holds, the CLI changed under it and is asked again now.
    pub fn observe_version(
        self: &Arc<Self>,
        binary: &'static str,
        probe: &'static dyn CliProbe,
        version: &Version,
    ) {
        let differs = {
            let mut entries = self.entries.lock().unwrap();
            let entry = entries.entry(binary).or_default();
            let held = entry
                .reading
                .as_ref()
                .and_then(|reading| reading.version.as_ref());
            self.claim_ask(entry, held != Some(version))
        };
        if differs {
            self.ask(binary, probe);
        }
    }

    /// Counts every reading that changed what it said. A receiver wakes once
    /// per change, whoever it is.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    fn held(&self, binary: &'static str) -> Option<Arc<CliReading>> {
        let entries = self.entries.lock().unwrap();
        entries.get(binary).and_then(|entry| entry.reading.clone())
    }

    /// Whether this caller is the one to ask: the entry wants asking, no ask
    /// is already under way, and these readings ask at all.
    fn claim_ask(&self, entry: &mut Entry, wanted: bool) -> bool {
        let claimed = wanted && !entry.asking && self.schedule.is_some();
        if claimed {
            entry.asking = true;
        }
        claimed
    }

    fn ask(self: &Arc<Self>, binary: &'static str, probe: &'static dyn CliProbe) {
        let Some(schedule) = &self.schedule else {
            return;
        };
        let readings = Arc::clone(self);
        schedule(Box::new(move || {
            let reading = probe.read(binary);
            readings.record(binary, reading);
        }));
    }

    fn record(&self, binary: &'static str, reading: CliReading) {
        let changed = {
            let mut entries = self.entries.lock().unwrap();
            let entry = entries.entry(binary).or_default();
            let changed = entry.reading.as_deref() != Some(&reading);
            entry.reading = Some(Arc::new(reading));
            entry.read_at = Some((self.now)());
            entry.asking = false;
            changed
        };
        if changed {
            self.changed.send_modify(|count| *count += 1);
        }
    }
}

/// The readings this process serves catalogs from.
pub fn readings() -> &'static Arc<Readings> {
    static READINGS: OnceLock<Arc<Readings>> = OnceLock::new();
    READINGS.get_or_init(|| {
        #[cfg(test)]
        {
            Readings::inert()
        }
        #[cfg(not(test))]
        {
            Readings::background()
        }
    })
}

/// What `provider` offers on this machine: its catalog, seen through what its
/// CLI last said.
pub fn model_offer(provider: AgentProvider) -> ModelOffer {
    let harness = harness_for(provider);
    let reading = readings().reading(harness.binary(), harness.cli_probe());
    harness.offer(reading.as_deref())
}

/// A running session of `provider` reported the version it is.
pub fn observe_version(provider: AgentProvider, reported: &str) {
    let Some(version) = probe::version_in(reported) else {
        return;
    };
    let harness = harness_for(provider);
    readings().observe_version(harness.binary(), harness.cli_probe(), &version);
}

/// Ask every harness's CLI once, so the first catalog a client reads is
/// already the installed one.
pub fn warm() {
    for provider in AgentProvider::ALL {
        let harness = harness_for(provider);
        readings().reading(harness.binary(), harness.cli_probe());
    }
}

#[cfg(test)]
mod tests;
