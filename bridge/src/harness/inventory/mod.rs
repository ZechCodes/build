//! The harness inventory: whether each harness's CLI is installed, which
//! version is installed and which its sessions run, and how its credential
//! context is signed in (#434).
//!
//! A daemon service, not an RPC: one thread sweeps every
//! [`SWEEP_INTERVAL`] whether or not a client is connected, and
//! `harnesses.list` reads the snapshot it keeps without waiting on any CLI.
//! Its lock is its own, never `AppState`'s, and no probe runs under it.
//!
//! Each sweep looks for every harness's executable (a few `stat`s), takes the
//! installed version from the CLI readings (`harness::installed`, on their own
//! ten-minute schedule), and observes each credential context through its
//! passive, metadata-only adapter ([`adapters`]; no CLI is run) at most once
//! per [`AUTH_CHECK_INTERVAL`],
//! backing off after failures. An executable or credential file that
//! changes, or an explicit `harnesses.refresh`, checks again sooner, no more
//! often than every [`FORCED_CHECK_FLOOR`]. A failed check keeps the facts it
//! could not replace, marked stale; an older check never overwrites a newer
//! one. Each change raises the revision, is saved (nonsecret, see
//! [`persisted`]), and wakes the `harnesses.changed` push.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use tokio::sync::watch;

use crate::harness::harness_for;
use crate::harness::installed::executable::Executable;
use crate::harness::installed::Readings;
use crate::models::AgentProvider;

pub mod adapters;
pub mod environment;
pub mod model;
mod persisted;
pub mod running;

pub use adapters::{AuthAdapter, ObservationFailed, CLAUDE_AUTH, CODEX_AUTH, PI_AUTH};
pub use environment::DeviceEnvironment;
pub use model::*;
pub use running::{report_running_version, running_sessions, ReportedVersion, RunningSessions};

/// How often the inventory looks, with or without a client.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(15);

/// How often a credential context that answered is observed again.
pub const AUTH_CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// The longest a failing context waits between checks: its delay doubles
/// from [`AUTH_CHECK_INTERVAL`] up to here.
pub const MAX_FAILURE_BACKOFF: Duration = Duration::from_secs(10 * 60);

/// The soonest a changed file, executable or refresh checks a context again
/// after its last check: repeated presses coalesce rather than run a CLI
/// each.
pub const FORCED_CHECK_FLOOR: Duration = Duration::from_secs(5);

type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;
type WallClock = Arc<dyn Fn() -> SystemTime + Send + Sync>;
type EnvironmentSource = Arc<dyn Fn() -> DeviceEnvironment + Send + Sync>;

/// What a sweep reads, and where it writes. Replaced piecemeal in tests.
pub struct InventoryBuilder {
    environment: EnvironmentSource,
    readings: Arc<Readings>,
    running: Arc<RunningSessions>,
    persist: Option<PathBuf>,
    now: Clock,
    wall: WallClock,
    sweep_interval: Duration,
}

impl Default for InventoryBuilder {
    fn default() -> Self {
        InventoryBuilder {
            environment: Arc::new(DeviceEnvironment::of_this_process),
            readings: Arc::clone(crate::harness::installed::readings()),
            running: Arc::clone(running_sessions()),
            persist: None,
            now: Arc::new(Instant::now),
            wall: Arc::new(SystemTime::now),
            sweep_interval: SWEEP_INTERVAL,
        }
    }
}

impl InventoryBuilder {
    pub fn environment(
        mut self,
        source: impl Fn() -> DeviceEnvironment + Send + Sync + 'static,
    ) -> Self {
        self.environment = Arc::new(source);
        self
    }

    pub fn readings(mut self, readings: Arc<Readings>) -> Self {
        self.readings = readings;
        self
    }

    pub fn running(mut self, running: Arc<RunningSessions>) -> Self {
        self.running = running;
        self
    }

    /// Restore from, and save to, `path`.
    pub fn persist_to(mut self, path: PathBuf) -> Self {
        self.persist = Some(path);
        self
    }

    pub fn clocks(
        mut self,
        now: impl Fn() -> Instant + Send + Sync + 'static,
        wall: impl Fn() -> SystemTime + Send + Sync + 'static,
    ) -> Self {
        self.now = Arc::new(now);
        self.wall = Arc::new(wall);
        self
    }

    pub fn sweep_every(mut self, interval: Duration) -> Self {
        self.sweep_interval = interval;
        self
    }

    pub fn build(self) -> Arc<Inventory> {
        let mut state = State::fresh();
        if let Some(saved) = self.persist.as_deref().and_then(persisted::load) {
            state.restore(saved);
        }
        let inventory = Arc::new(Inventory {
            changed: watch::channel(state.revision).0,
            state: Mutex::new(state),
            wake: Condvar::new(),
            sources: self,
        });
        let woken = Arc::downgrade(&inventory);
        inventory.sources.running.on_change(Box::new(move || {
            if let Some(inventory) = woken.upgrade() {
                inventory.nudge();
            }
        }));
        inventory
    }
}

pub struct Inventory {
    state: Mutex<State>,
    wake: Condvar,
    changed: watch::Sender<u64>,
    sources: InventoryBuilder,
}

struct State {
    revision: u64,
    refresh: RefreshProgress,
    harnesses: Vec<HarnessState>,
    contexts: Vec<ContextState>,
    /// Something asked for a sweep before the next interval.
    pending: bool,
    running_seen: u64,
}

struct HarnessState {
    provider: AgentProvider,
    context: usize,
    installation: Installation,
    executable: Option<Executable>,
    installed_version: Option<String>,
}

struct ContextState {
    adapter: &'static dyn AuthAdapter,
    harnesses: Vec<AgentProvider>,
    facts: AuthFacts,
    health: Health,
    checked_at_ms: Option<u64>,
    credential_generation: u64,
    fingerprint: Option<Vec<Fingerprint>>,
    /// Asked to check before its next due time.
    forced: bool,
    last_attempt: Option<Instant>,
    next_due: Option<Instant>,
    failures: u32,
    claimed: u64,
    recorded: u64,
}

/// What a watched file looked like: a change to any part is a change.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fingerprint(Option<(Option<SystemTime>, u64)>);

fn fingerprints(paths: &[PathBuf]) -> Vec<Fingerprint> {
    paths
        .iter()
        .map(|path| {
            Fingerprint(
                std::fs::metadata(path)
                    .ok()
                    .map(|metadata| (metadata.modified().ok(), metadata.len())),
            )
        })
        .collect()
}

fn unix_ms(at: SystemTime) -> u64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

fn backoff(failures: u32) -> Duration {
    AUTH_CHECK_INTERVAL
        .saturating_mul(1 << failures.saturating_sub(1).min(16))
        .min(MAX_FAILURE_BACKOFF)
}

impl State {
    /// Every harness, each linked to its credential context; nothing seen.
    fn fresh() -> State {
        let mut contexts: Vec<ContextState> = Vec::new();
        let harnesses = AgentProvider::ALL
            .into_iter()
            .map(|provider| {
                let adapter = harness_for(provider).auth();
                let context = contexts
                    .iter()
                    .position(|context| context.adapter.context() == adapter.context())
                    .unwrap_or_else(|| {
                        contexts.push(ContextState::new(adapter));
                        contexts.len() - 1
                    });
                contexts[context].harnesses.push(provider);
                HarnessState {
                    provider,
                    context,
                    installation: Installation {
                        state: InstallationState::Unknown,
                        health: Health::Unobserved,
                        checked_at_ms: None,
                    },
                    executable: None,
                    installed_version: None,
                }
            })
            .collect();
        State {
            revision: 0,
            refresh: RefreshProgress::default(),
            harnesses,
            contexts,
            pending: false,
            running_seen: 0,
        }
    }

    /// What the last daemon saw, every fact of it stale until seen again.
    fn restore(&mut self, saved: persisted::Persisted) {
        self.revision = saved.revision + 1;
        for harness in &mut self.harnesses {
            if let Some(row) = saved
                .harnesses
                .iter()
                .find(|row| row.id == harness.provider)
            {
                harness.installation = Installation {
                    state: row.installation,
                    health: Health::Stale,
                    checked_at_ms: row.checked_at_ms,
                };
                harness.installed_version = row.installed_version.clone();
            }
        }
        for context in &mut self.contexts {
            if let Some(row) = saved
                .contexts
                .iter()
                .find(|row| row.id == context.adapter.context())
            {
                context.facts = row.facts.clone();
                context.health = Health::Stale;
                context.checked_at_ms = row.checked_at_ms;
                context.credential_generation = row.credential_generation;
            }
        }
    }

    fn persisted(&self) -> persisted::Persisted {
        persisted::Persisted {
            revision: self.revision,
            harnesses: self
                .harnesses
                .iter()
                .map(|harness| persisted::PersistedHarness {
                    id: harness.provider,
                    installation: harness.installation.state,
                    installed_version: harness.installed_version.clone(),
                    checked_at_ms: harness.installation.checked_at_ms,
                })
                .collect(),
            contexts: self
                .contexts
                .iter()
                .map(|context| persisted::PersistedContext {
                    id: context.adapter.context().to_string(),
                    facts: context.facts.clone(),
                    checked_at_ms: context.checked_at_ms,
                    credential_generation: context.credential_generation,
                })
                .collect(),
        }
    }
}

impl ContextState {
    fn new(adapter: &'static dyn AuthAdapter) -> ContextState {
        ContextState {
            adapter,
            harnesses: Vec::new(),
            facts: AuthFacts::unobserved(),
            health: Health::Unobserved,
            checked_at_ms: None,
            credential_generation: 0,
            fingerprint: None,
            forced: false,
            last_attempt: None,
            next_due: None,
            failures: 0,
            claimed: 0,
            recorded: 0,
        }
    }

    /// The generation of a check to run now, if one is due and none is
    /// running: on schedule, or forced (by a refresh, an executable or a file
    /// that changed) once the floor since the last check has passed.
    fn claim(&mut self, fingerprint: &[Fingerprint], now: Instant) -> Option<u64> {
        let files_moved = self.fingerprint.as_deref() != Some(fingerprint);
        let due = self.next_due.is_none_or(|due| now >= due);
        let floor_passed = self
            .last_attempt
            .is_none_or(|at| now.duration_since(at) >= FORCED_CHECK_FLOOR);
        let wanted = due || ((self.forced || files_moved) && floor_passed);
        if !wanted || self.claimed > self.recorded {
            return None;
        }
        self.forced = false;
        self.last_attempt = Some(now);
        self.claimed += 1;
        Some(self.claimed)
    }

    /// Record a check's outcome unless a newer one is already in. Whether
    /// anything a client sees changed; a check time moving alone is not news.
    fn record(
        &mut self,
        generation: u64,
        outcome: Result<AuthFacts, ObservationFailed>,
        fingerprint: Vec<Fingerprint>,
        now: Instant,
        wall: SystemTime,
    ) -> bool {
        if generation <= self.recorded {
            return false;
        }
        self.recorded = generation;
        let before = (self.facts.clone(), self.health, self.credential_generation);
        match outcome {
            Ok(facts) => self.observed(facts, &fingerprint, now, wall),
            Err(failure) => self.failed(failure, now),
        }
        self.fingerprint = Some(fingerprint);
        before != (self.facts.clone(), self.health, self.credential_generation)
    }

    fn observed(
        &mut self,
        facts: AuthFacts,
        fingerprint: &[Fingerprint],
        now: Instant,
        wall: SystemTime,
    ) {
        let files_moved = self
            .fingerprint
            .as_deref()
            .is_some_and(|seen| seen != fingerprint);
        if facts != self.facts || files_moved {
            self.credential_generation += 1;
        }
        self.facts = facts;
        self.health = Health::Fresh;
        self.checked_at_ms = Some(unix_ms(wall));
        self.failures = 0;
        self.next_due = Some(now + AUTH_CHECK_INTERVAL);
    }

    fn failed(&mut self, failure: ObservationFailed, now: Instant) {
        if self.health == Health::Fresh {
            eprintln!(
                "harness inventory: {}: {}; keeping what it said before",
                self.adapter.context(),
                failure.0
            );
            self.health = Health::Stale;
        }
        self.failures += 1;
        self.next_due = Some(now + backoff(self.failures));
    }

    fn row(&self) -> AuthContextRow {
        AuthContextRow {
            id: self.adapter.context().to_string(),
            harnesses: self.harnesses.clone(),
            facts: self.facts.clone(),
            health: self.health,
            checked_at_ms: self.checked_at_ms,
            credential_generation: self.credential_generation,
            supported_login_methods: Vec::new(),
        }
    }
}

impl Inventory {
    pub fn builder() -> InventoryBuilder {
        InventoryBuilder::default()
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// Everything it holds now. Never waits on a CLI.
    pub fn snapshot(&self) -> HarnessesSnapshot {
        let state = self.lock();
        HarnessesSnapshot {
            revision: state.revision,
            refresh: state.refresh,
            harnesses: state
                .harnesses
                .iter()
                .map(|harness| self.harness_row(&state, harness))
                .collect(),
            auth_contexts: state.contexts.iter().map(ContextState::row).collect(),
        }
    }

    fn harness_row(&self, state: &State, harness: &HarnessState) -> HarnessRow {
        let implementation = harness_for(harness.provider);
        let reported = implementation.reports_running_version();
        HarnessRow {
            id: harness.provider,
            label: implementation.label().to_string(),
            cli_name: implementation.cli_name().to_string(),
            auth_context: state.contexts[harness.context]
                .adapter
                .context()
                .to_string(),
            installation: harness.installation.clone(),
            version: VersionFacts {
                installed: harness.installed_version.clone(),
                running: RunningVersions {
                    reported,
                    versions: if reported {
                        self.sources.running.versions_of(harness.provider)
                    } else {
                        Vec::new()
                    },
                },
            },
        }
    }

    /// Ask for every context to be observed again soon, and answer at once
    /// with the request to watch for. Never signs in or refreshes a token.
    pub fn request_refresh(&self) -> RefreshReceipt {
        let mut state = self.lock();
        state.refresh.requested += 1;
        for context in &mut state.contexts {
            context.forced = true;
        }
        state.pending = true;
        self.wake.notify_all();
        RefreshReceipt {
            request: state.refresh.requested,
            revision: state.revision,
        }
    }

    /// Something worth a look happened (a session reported its version):
    /// sweep now rather than at the next interval.
    pub fn nudge(&self) {
        self.lock().pending = true;
        self.wake.notify_all();
    }

    /// How many checks of context `id` have run.
    #[cfg(test)]
    fn checks_of(&self, id: &str) -> u64 {
        let state = self.lock();
        state
            .contexts
            .iter()
            .find(|context| context.adapter.context() == id)
            .map_or(0, |context| context.claimed)
    }

    /// Wakes with the revision each time it rises.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// One sweep: installations and versions, then every context that is
    /// due. Probes run with the lock released.
    pub fn sweep(&self) {
        let environment = (self.sources.environment)();
        let refresh_target = {
            let mut state = self.lock();
            state.pending = false;
            state.refresh.requested
        };
        let mut changed = self.observe_installations(&environment);
        let contexts = self.lock().contexts.len();
        for context in 0..contexts {
            changed |= self.observe_context(context, &environment);
        }
        self.finish(refresh_target, changed);
    }

    fn observe_installations(&self, environment: &DeviceEnvironment) -> bool {
        let wall = unix_ms((self.sources.wall)());
        let mut changed = false;
        for index in 0..AgentProvider::ALL.len() {
            let provider = self.lock().harnesses[index].provider;
            let harness = harness_for(provider);
            let executable = environment.executable(harness.binary());
            let version = self
                .sources
                .readings
                .reading(harness.binary(), harness.cli_probe())
                .and_then(|reading| reading.version.as_ref().map(ToString::to_string));
            let mut state = self.lock();
            changed |= state.see_installation(index, executable, version, wall);
        }
        changed
    }

    fn observe_context(&self, index: usize, environment: &DeviceEnvironment) -> bool {
        let now = (self.sources.now)();
        let adapter = self.lock().contexts[index].adapter;
        let fingerprint = fingerprints(&adapter.watched(environment));
        let Some(generation) = self.lock().contexts[index].claim(&fingerprint, now) else {
            return false;
        };
        let outcome = adapter.observe(environment, (self.sources.wall)());
        let wall = (self.sources.wall)();
        self.lock().contexts[index].record(generation, outcome, fingerprint, now, wall)
    }

    fn finish(&self, refresh_target: u64, mut changed: bool) {
        let mut state = self.lock();
        let running = self.sources.running.change_count();
        if running != state.running_seen {
            state.running_seen = running;
            changed = true;
        }
        if !state.contexts.iter().any(|context| context.forced)
            && state.refresh.completed < refresh_target
        {
            state.refresh.completed = refresh_target;
            changed = true;
        }
        if !changed {
            return;
        }
        state.revision += 1;
        if let Some(path) = &self.sources.persist {
            persisted::save(path, &state.persisted());
        }
        self.changed.send_replace(state.revision);
    }

    /// Until there is something to do: a nudge or refresh, a forced check
    /// whose floor has passed, or the next interval.
    fn wait_for_work(&self) {
        let state = self.lock();
        let now = (self.sources.now)();
        let timeout = state
            .contexts
            .iter()
            .filter(|context| context.forced)
            .filter_map(|context| context.last_attempt)
            .map(|at| (at + FORCED_CHECK_FLOOR).saturating_duration_since(now))
            .fold(self.sources.sweep_interval, Duration::min);
        let _unused = self
            .wake
            .wait_timeout_while(state, timeout, |state| !state.pending)
            .unwrap();
    }

    /// Sweep on a thread of its own from now on, whether or not anyone asks.
    pub fn start(self: &Arc<Self>) {
        let inventory = Arc::downgrade(self);
        let spawned = std::thread::Builder::new()
            .name("harness-inventory".into())
            .spawn(move || {
                while let Some(inventory) = inventory.upgrade() {
                    inventory.sweep();
                    inventory.wait_for_work();
                }
            });
        if let Err(error) = spawned {
            eprintln!("harness inventory: cannot start its sweep: {error}");
        }
    }
}

impl State {
    fn see_installation(
        &mut self,
        index: usize,
        executable: Option<Executable>,
        version: Option<String>,
        wall: u64,
    ) -> bool {
        let installed = if executable.is_some() {
            InstallationState::Installed
        } else {
            InstallationState::NotInstalled
        };
        let context = self.harnesses[index].context;
        let harness = &mut self.harnesses[index];
        let before = (
            harness.installation.state,
            harness.installation.health,
            harness.installed_version.clone(),
        );
        if harness.executable != executable {
            // A new install, a removal or a retargeted link: its sign-in may
            // be another one.
            self.contexts[context].forced = true;
        }
        harness.executable = executable;
        harness.installed_version = version.filter(|_| installed == InstallationState::Installed);
        harness.installation = Installation {
            state: installed,
            health: Health::Fresh,
            checked_at_ms: Some(wall),
        };
        before != (installed, Health::Fresh, harness.installed_version.clone())
    }
}

static SHARED: OnceLock<Arc<Inventory>> = OnceLock::new();

/// The inventory this process serves `harnesses.list` from. Unstarted until
/// [`start`]; in unit tests never started at all, so no test runs a real CLI
/// by listing.
pub fn shared() -> &'static Arc<Inventory> {
    SHARED.get_or_init(|| Inventory::builder().build())
}

/// Start the daemon's inventory, restored from and saved to `persist`. Call
/// before anything reads [`shared`], or the restore is skipped.
pub fn start(persist: PathBuf) {
    let inventory = SHARED.get_or_init(|| Inventory::builder().persist_to(persist).build());
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| inventory.start());
}

#[cfg(test)]
mod tests;
