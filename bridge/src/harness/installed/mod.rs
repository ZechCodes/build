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
//! older than [`READING_TTL`], sooner after a failed ask or an executable
//! change. A periodic refresh keeps cached clients current, and a session
//! that reports its own version ([`observe_version`]) asks again at once when
//! it differs. Every change is
//! counted on [`Readings::changes`], which the `models.changed` push follows.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use semver::Version;
use tokio::sync::watch;

use crate::harness::{harness_for, Harness};
use crate::models::{AgentProvider, ModelChoice};

mod executable;
pub(crate) mod offer;
pub(crate) mod probe;

use executable::Executable;

pub use offer::{ModelOffer, OfferedModel, UnavailableModel};
pub use probe::{CliProbe, CODEX_MODEL_LIST, NO_PROBE, VERSION_FLAG};

/// How long an answer stands before the next ask for it asks the CLI again.
pub const READING_TTL: Duration = Duration::from_secs(10 * 60);

/// Initial retry delay after a failed or incomplete answer, doubled after
/// each further failure up to the TTL. Also the periodic refresh interval.
const RETRY_BACKOFF: Duration = Duration::from_secs(15);

/// How old an answer may be and still refuse a session on its own word. An
/// older one is asked again first, so a CLI updated a minute ago is not
/// refused for the version it replaced.
pub const REFUSAL_FRESHNESS: Duration = Duration::from_secs(30);

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
    reading_usable: bool,
    /// When the latest attempt finished, even if its answer was not kept.
    read_at: Option<Instant>,
    asking: Option<u64>,
    generation: u64,
    retry: bool,
    retry_delay: Duration,
    /// The executable that gave the held reading.
    executable: Option<Executable>,
    /// The latest attempt's executable, so a failed replacement does not
    /// bypass its retry delay on every catalog read or refresh.
    attempted_executable: Option<Executable>,
    /// A repeated session hint must not bypass a failed probe's backoff just
    /// because the retained usable reading still has an older version.
    observed_version: Option<Version>,
    probe: Option<&'static dyn CliProbe>,
}

impl Entry {
    fn record_attempt(
        &mut self,
        reading: Arc<CliReading>,
        retry: bool,
        executable: Option<Executable>,
        now: Instant,
        ttl: Duration,
    ) -> bool {
        self.retry_delay = if retry {
            let delay = if self.retry && self.attempted_executable == executable {
                self.retry_delay.saturating_mul(2)
            } else {
                RETRY_BACKOFF
            };
            delay.min(ttl)
        } else {
            Duration::ZERO
        };
        self.retry = retry;
        self.read_at = Some(now);
        self.attempted_executable = executable.clone();
        if retry && self.reading_usable {
            return false;
        }
        let changed = self.reading.as_deref() != Some(reading.as_ref());
        self.reading = Some(reading);
        self.reading_usable = !retry;
        self.executable = executable;
        changed
    }
}

/// Dropping a scheduled job (including failed thread creation) or unwinding
/// out of a probe must release the claim and permit a later retry.
struct PendingAsk {
    readings: Arc<Readings>,
    binary: &'static str,
    executable: Option<Executable>,
    generation: u64,
    completed: bool,
}

impl PendingAsk {
    fn run(&mut self, probe: &'static dyn CliProbe) {
        self.readings.read_and_record(
            self.binary,
            probe,
            self.executable.clone(),
            self.generation,
            true,
        );
        self.completed = true;
    }
}

impl Drop for PendingAsk {
    fn drop(&mut self) {
        if !self.completed {
            let recorded = self.readings.record(
                self.binary,
                Arc::new(CliReading::default()),
                true,
                self.executable.clone(),
                self.generation,
                true,
            );
            if recorded {
                eprintln!(
                    "cli probe: {}: probe did not finish; will retry",
                    self.binary
                );
            }
        }
    }
}

/// Every CLI's last usable answer, or its latest answer until one succeeds.
pub struct Readings {
    entries: Mutex<HashMap<&'static str, Entry>>,
    ttl: Duration,
    schedule: Option<Scheduler>,
    now: Clock,
    changed: watch::Sender<u64>,
    /// Asked in place of every CLI's own probe, in a test.
    #[cfg(test)]
    stand_in: Option<&'static dyn CliProbe>,
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

    /// Readings that ask on the calling thread, and ask `probe` whichever CLI
    /// is meant: an app test's stand-in for every installed CLI.
    #[cfg(test)]
    pub(crate) fn answering_inline(probe: &'static dyn CliProbe) -> Arc<Self> {
        let mut readings = Self::with(
            Some(Arc::new(|ask: Box<dyn FnOnce() + Send>| ask())),
            Arc::new(Instant::now),
            READING_TTL,
        );
        Arc::get_mut(&mut readings).expect("just made").stand_in = Some(probe);
        readings
    }

    fn with(schedule: Option<Scheduler>, now: Clock, ttl: Duration) -> Arc<Self> {
        Arc::new(Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
            schedule,
            now,
            changed: watch::channel(0).0,
            #[cfg(test)]
            stand_in: None,
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
        self.held_with_age(binary, probe).0
    }

    /// [`Self::reading`], with how old it is.
    fn held_with_age(
        self: &Arc<Self>,
        binary: &'static str,
        probe: &'static dyn CliProbe,
    ) -> (Option<Arc<CliReading>>, Option<Duration>) {
        let executable = executable::identify(binary);
        let stale = {
            let mut entries = self.entries.lock().unwrap();
            let entry = entries.entry(binary).or_default();
            entry.probe = Some(probe);
            let lifetime = if entry.retry {
                entry.retry_delay
            } else {
                self.ttl
            };
            let expired = entry
                .read_at
                .is_none_or(|read_at| (self.now)().duration_since(read_at) >= lifetime);
            self.claim_ask(entry, expired || entry.attempted_executable != executable)
        };
        if let Some(generation) = stale {
            self.ask(binary, probe, executable.clone(), generation);
        }
        let entries = self.entries.lock().unwrap();
        let Some(entry) = entries.get(binary) else {
            return (None, None);
        };
        let age = entry
            .read_at
            .filter(|_| !entry.retry && entry.executable == executable)
            .map(|read_at| (self.now)().duration_since(read_at));
        (entry.reading.clone(), age)
    }

    /// What `binary` says, asked again here and now unless its answer is
    /// younger than `fresh`: the caller waits on the CLI, for at most the
    /// probe's deadline. Only for a caller off every RPC and the event loop —
    /// the spawn gate, which is about to start the CLI anyway. Readings that
    /// never ask answer what they hold.
    pub fn fresh_reading(
        self: &Arc<Self>,
        binary: &'static str,
        probe: &'static dyn CliProbe,
        fresh: Duration,
    ) -> Option<Arc<CliReading>> {
        let executable = executable::identify(binary);
        let generation = {
            let mut entries = self.entries.lock().unwrap();
            let entry = entries.entry(binary).or_default();
            let young = entry
                .read_at
                .filter(|_| !entry.retry && entry.executable == executable)
                .is_some_and(|read_at| (self.now)().duration_since(read_at) < fresh);
            if young || self.schedule.is_none() {
                return entry.reading.clone();
            }
            entry.generation += 1;
            entry.generation
        };
        // The spawn verdict uses this probe's own answer, even if another
        // caller updates the cache before this call returns.
        Some(self.read_and_record(binary, probe, executable, generation, false))
    }

    /// A session of `binary` says it runs `version`: a new hint differing from
    /// the held reading asks again now; repeats honor a failed ask's backoff.
    pub fn observe_version(
        self: &Arc<Self>,
        binary: &'static str,
        probe: &'static dyn CliProbe,
        version: &Version,
    ) {
        let differs = {
            let mut entries = self.entries.lock().unwrap();
            let entry = entries.entry(binary).or_default();
            entry.probe = Some(probe);
            let new_hint = entry.observed_version.as_ref() != Some(version);
            entry.observed_version = Some(version.clone());
            let retry_due = entry
                .read_at
                .is_none_or(|read_at| (self.now)().duration_since(read_at) >= entry.retry_delay);
            let held = entry
                .reading
                .as_ref()
                .and_then(|reading| reading.version.as_ref());
            let wanted = held != Some(version) && (new_hint || !entry.retry || retry_due);
            self.claim_ask(entry, wanted)
        };
        if let Some(generation) = differs {
            self.ask(binary, probe, executable::identify(binary), generation);
        }
    }

    /// Counts every reading that changed what it said. A receiver wakes once
    /// per change, whoever it is.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    #[cfg(test)]
    fn held(&self, binary: &'static str) -> Option<Arc<CliReading>> {
        let entries = self.entries.lock().unwrap();
        entries.get(binary).and_then(|entry| entry.reading.clone())
    }

    /// Whether this caller is the one to ask: the entry wants asking, no ask
    /// is already under way, and these readings ask at all.
    fn claim_ask(&self, entry: &mut Entry, wanted: bool) -> Option<u64> {
        if wanted && entry.asking.is_none() && self.schedule.is_some() {
            entry.generation += 1;
            entry.asking = Some(entry.generation);
            return entry.asking;
        }
        None
    }

    fn ask(
        self: &Arc<Self>,
        binary: &'static str,
        probe: &'static dyn CliProbe,
        executable: Option<Executable>,
        generation: u64,
    ) {
        let Some(schedule) = &self.schedule else {
            return;
        };
        let mut pending = PendingAsk {
            readings: Arc::clone(self),
            binary,
            executable,
            generation,
            completed: false,
        };
        schedule(Box::new(move || pending.run(probe)));
    }

    fn read_and_record(
        &self,
        binary: &'static str,
        probe: &'static dyn CliProbe,
        executable: Option<Executable>,
        generation: u64,
        ends_ask: bool,
    ) -> Arc<CliReading> {
        #[cfg(test)]
        let reader = self.stand_in.unwrap_or(probe);
        #[cfg(not(test))]
        let reader = probe;
        self.entries
            .lock()
            .unwrap()
            .entry(binary)
            .or_default()
            .probe = Some(probe);
        let reading = Arc::new(reader.read(binary));
        let retry = probe.needs_retry(&reading);
        self.record(
            binary,
            Arc::clone(&reading),
            retry,
            executable,
            generation,
            ends_ask,
        );
        reading
    }

    /// Revisit previously requested CLIs even when every client uses its
    /// cached catalog. The normal claim still permits only one ask per CLI.
    fn refresh(self: &Arc<Self>) {
        let probes: Vec<_> = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(binary, entry)| entry.probe.map(|probe| (*binary, probe)))
            .collect();
        for (binary, probe) in probes {
            self.reading(binary, probe);
        }
    }

    fn keep_fresh(self: &Arc<Self>) {
        let readings = Arc::downgrade(self);
        let spawned = std::thread::Builder::new()
            .name("cli-refresh".into())
            .spawn(move || loop {
                std::thread::sleep(RETRY_BACKOFF);
                let Some(readings) = readings.upgrade() else {
                    break;
                };
                readings.refresh();
            });
        if let Err(error) = spawned {
            eprintln!("cli probe: cannot start the refresh thread: {error}");
        }
    }

    /// Record the newest attempt, keeping a usable answer through failures.
    /// An older background job still releases its own claim, but cannot
    /// overwrite a newer spawn-gate read.
    fn record(
        &self,
        binary: &'static str,
        reading: Arc<CliReading>,
        retry: bool,
        executable: Option<Executable>,
        generation: u64,
        ends_ask: bool,
    ) -> bool {
        let changed = {
            let mut entries = self.entries.lock().unwrap();
            let entry = entries.entry(binary).or_default();
            if ends_ask && entry.asking == Some(generation) {
                entry.asking = None;
            }
            if entry.generation != generation {
                return false;
            }
            entry.record_attempt(reading, retry, executable, (self.now)(), self.ttl)
        };
        if changed {
            self.changed.send_modify(|count| *count += 1);
        }
        true
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
    model_offer_from(readings(), provider)
}

/// The same, seen through `readings`.
pub fn model_offer_from(readings: &Arc<Readings>, provider: AgentProvider) -> ModelOffer {
    let harness = harness_for(provider);
    let reading = readings.reading(harness.binary(), harness.cli_probe());
    harness.offer(reading.as_deref())
}

/// Refuse a session on a model the installed CLI cannot run correctly, before
/// it is started: an older Claude Code gets a 400 from the API on every turn,
/// or runs the model at a 200k window, and neither says so where the person
/// who chose the model will look (#203). A CLI Build knows nothing about is
/// never refused, and one whose answer is older than [`REFUSAL_FRESHNESS`] is
/// asked again before it is: this waits on the CLI, so it is for the spawn
/// path alone, which runs off every RPC.
pub fn refuse_unrunnable(readings: &Arc<Readings>, choice: &ModelChoice) -> Result<(), String> {
    let harness = harness_for(choice.provider);
    let held = readings.reading(harness.binary(), harness.cli_probe());
    if refusal(harness, choice, held.as_deref()).is_none() {
        return Ok(());
    }
    let fresh = readings.fresh_reading(harness.binary(), harness.cli_probe(), REFUSAL_FRESHNESS);
    refusal(harness, choice, fresh.as_deref()).map_or(Ok(()), Err)
}

/// Why `choice` would be refused on what `readings` hold now, without asking
/// any CLI: for an RPC, which cannot wait on one. Only an answer younger than
/// [`REFUSAL_FRESHNESS`] refuses; an older one leaves the verdict to the
/// spawn, which asks again first.
pub fn held_refusal(readings: &Arc<Readings>, choice: &ModelChoice) -> Option<String> {
    let harness = harness_for(choice.provider);
    let (reading, read_at) = readings.held_with_age(harness.binary(), harness.cli_probe());
    if read_at.is_none_or(|age| age >= REFUSAL_FRESHNESS) {
        return None;
    }
    refusal(harness, choice, reading.as_deref())
}

fn refusal(
    harness: &dyn Harness,
    choice: &ModelChoice,
    reading: Option<&CliReading>,
) -> Option<String> {
    let model = choice.model.as_deref()?;
    harness.offer(reading).refusal(model, harness.cli_name())
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
    static REFRESHING: OnceLock<()> = OnceLock::new();
    REFRESHING.get_or_init(|| readings().keep_fresh());
}

#[cfg(test)]
mod tests;
