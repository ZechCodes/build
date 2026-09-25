//! The live roster: who a crash would have to bring back, on disk for as long
//! as the daemon runs.
//!
//! The app hands it each entity's agents as that entity's mutation settles
//! ([`LiveRoster::set_entity`]), with the app mutex held — so it does no
//! filesystem work there at all. It keeps the newest list per entity, and a
//! writer thread of its own puts the whole of it on disk with every lock
//! released. Per entity, because a transaction checks more than one entity
//! out at a time: an entity absent from the app's maps mid-transaction keeps
//! the list it last settled with, rather than vanishing from the file.
//!
//! A write that fails is retried, with a pause that doubles up to
//! [`RETRY_PAUSE_MAX`], until one lands; lists published in the meantime
//! collapse into the newest one.
//!
//! What the file cannot close is the interval between a change and its write
//! landing — one write and its syncs, longer while the disk refuses. A death
//! inside it resumes the list from before the change: an agent that had just
//! finished comes back and is told its turn may have been cut short (it
//! reads its conversation and stops), and one that had just started is not
//! brought back.

use super::disk::{self, Disk, RealDisk};
use super::{live_path, resume_is_wanted, ResumeRoster, ResumingAgent};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// The first pause after a failed write, doubled after every failure since.
pub const RETRY_PAUSE_FIRST: Duration = Duration::from_millis(50);

/// The longest pause between retries.
pub const RETRY_PAUSE_MAX: Duration = Duration::from_secs(5);

/// How the writer thread is started; a test hands in one that fails.
pub type SpawnWriter = fn(Box<dyn FnOnce() + Send>) -> std::io::Result<()>;

/// See the module docs.
#[derive(Clone)]
pub struct LiveRoster {
    shared: Arc<Shared>,
}

struct Shared {
    dir: PathBuf,
    version: String,
    disk: Arc<dyn Disk>,
    slot: Mutex<Slot>,
    changed: Condvar,
}

#[derive(Default)]
struct Slot {
    /// The newest list per entity; an entity with nobody to resume is absent.
    entities: BTreeMap<String, Vec<ResumingAgent>>,
    /// Bumped by every change to `entities`.
    generation: u64,
    /// The generation the file on disk holds.
    written: u64,
    /// Whether the writer is between taking a list and finishing with it.
    writing: bool,
    /// Whether a writer thread is running. Without one the lists are still
    /// kept, for the clean shutdown to write.
    writer_running: bool,
    /// Set by the clean shutdown; nothing published after it is kept.
    sealed: bool,
    /// Writes that have failed in a row.
    failures: u32,
}

impl Slot {
    fn unwritten(&self) -> bool {
        self.generation != self.written
    }

    fn agents(&self) -> Vec<ResumingAgent> {
        self.entities.values().flatten().cloned().collect()
    }
}

fn spawn_thread(work: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("live-roster".to_string())
        .spawn(work)
        .map(|_| ())
}

/// The pause after the `failures`th failed write in a row.
fn retry_pause(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    RETRY_PAUSE_FIRST
        .saturating_mul(1 << doublings)
        .min(RETRY_PAUSE_MAX)
}

/// Marks the writer gone however its thread ends — a panic included — so a
/// shutdown waiting for its write is never left waiting for a dead thread.
struct WriterAlive<'a>(&'a LiveRoster);

impl Drop for WriterAlive<'_> {
    fn drop(&mut self) {
        let mut slot = self.0.slot();
        slot.writing = false;
        slot.writer_running = false;
        self.0.shared.changed.notify_all();
    }
}

impl LiveRoster {
    /// Start the roster for `dir`. Call [`super::promote_live_roster`] first:
    /// the first list written replaces whatever the last run left.
    pub fn start(dir: &Path, version: &str) -> LiveRoster {
        Self::start_with(dir, version, Arc::new(RealDisk), spawn_thread)
    }

    pub(crate) fn start_with(
        dir: &Path,
        version: &str,
        disk: Arc<dyn Disk>,
        spawn: SpawnWriter,
    ) -> LiveRoster {
        let live = LiveRoster {
            shared: Arc::new(Shared {
                dir: dir.to_path_buf(),
                version: version.to_string(),
                disk,
                slot: Mutex::new(Slot {
                    writer_running: true,
                    ..Slot::default()
                }),
                changed: Condvar::new(),
            }),
        };
        let writer = live.clone();
        if let Err(error) = spawn(Box::new(move || writer.write_until_sealed())) {
            live.slot().writer_running = false;
            eprintln!(
                "resume: no live roster writer this run ({error}); a crash will resume nobody, \
                 a clean shutdown still records who was working"
            );
        }
        live
    }

    /// Say who on `entity_id` a crash would have to bring back. Cheap and
    /// safe under the app mutex: an unchanged list is dropped here, and a
    /// changed one is only handed to the writer. An empty list forgets the
    /// entity.
    pub fn set_entity(&self, entity_id: &str, agents: Vec<ResumingAgent>) {
        let mut slot = self.slot();
        if slot.sealed {
            return;
        }
        let changed = if agents.is_empty() {
            slot.entities.remove(entity_id).is_some()
        } else if slot.entities.get(entity_id) == Some(&agents) {
            false
        } else {
            slot.entities.insert(entity_id.to_string(), agents);
            true
        };
        if changed {
            slot.generation += 1;
            self.shared.changed.notify_all();
        }
    }

    /// The entity is gone: nobody on it will be resumed.
    pub fn forget_entity(&self, entity_id: &str) {
        self.set_entity(entity_id, Vec::new());
    }

    fn slot(&self) -> MutexGuard<'_, Slot> {
        self.shared
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn wait<'a>(&self, slot: MutexGuard<'a, Slot>) -> MutexGuard<'a, Slot> {
        self.shared
            .changed
            .wait(slot)
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Wait out `pause`, or until the roster is sealed.
    fn pause<'a>(&self, mut slot: MutexGuard<'a, Slot>, pause: Duration) -> MutexGuard<'a, Slot> {
        let deadline = Instant::now() + pause;
        while !slot.sealed {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            slot = self
                .shared
                .changed
                .wait_timeout(slot, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        slot
    }

    fn write_until_sealed(&self) {
        let _alive = WriterAlive(self);
        let mut slot = self.slot();
        while !slot.sealed {
            if !slot.unwritten() {
                slot = self.wait(slot);
                continue;
            }
            let generation = slot.generation;
            let agents = slot.agents();
            slot.writing = true;
            drop(slot);
            let written = self.write(agents);
            slot = self.slot();
            slot.writing = false;
            self.shared.changed.notify_all();
            match written {
                Ok(()) => {
                    slot.written = generation;
                    slot.failures = 0;
                }
                Err(error) => {
                    slot.failures += 1;
                    if slot.failures.is_power_of_two() {
                        eprintln!(
                            "resume: could not write the live roster ({error}); retrying \
                             (failure {})",
                            slot.failures
                        );
                    }
                    let pause = retry_pause(slot.failures);
                    slot = self.pause(slot, pause);
                }
            }
        }
    }

    fn write(&self, agents: Vec<ResumingAgent>) -> Result<(), String> {
        let disk = &*self.shared.disk;
        let path = live_path(&self.shared.dir);
        if agents.is_empty() {
            return disk::remove(disk, &path);
        }
        let roster = ResumeRoster {
            recorded_at: crate::store::now_rfc3339(),
            version: self.shared.version.clone(),
            agents,
        };
        disk::replace(disk, &path, &roster.body()?)
    }

    /// The clean shutdown: seal the roster, write the roster proper from the
    /// newest lists, and only then remove the live file.
    ///
    /// The lists are the ones the app last settled, so this never waits on
    /// the app mutex — a handler holding it cannot hold the shutdown past
    /// systemd's patience. A write already in flight is waited for, so it
    /// cannot land after the removal; a writer that died mid-write has already
    /// said so. `wanted` is the opt-out: `false` records nobody and leaves
    /// neither file behind.
    ///
    /// Returns how many agents were recorded. On an error the live roster is
    /// left where it is, so the next boot still resumes from it.
    pub fn finish(&self, wanted: bool) -> Result<usize, String> {
        let agents = {
            let mut slot = self.slot();
            slot.sealed = true;
            self.shared.changed.notify_all();
            while slot.writing {
                slot = self.wait(slot);
            }
            slot.agents()
        };
        let disk = &*self.shared.disk;
        let dir = &self.shared.dir;
        if !wanted {
            ResumeRoster::forget_on(disk, dir)?;
            return Ok(0);
        }
        let count = agents.len();
        ResumeRoster {
            recorded_at: crate::store::now_rfc3339(),
            version: self.shared.version.clone(),
            agents,
        }
        .save_on(disk, dir)?;
        disk::remove(disk, &live_path(dir))?;
        Ok(count)
    }

    /// The whole of the shutdown half, as the daemon calls it: honour the
    /// opt-out, [`finish`](Self::finish), and say what happened on stderr.
    ///
    /// Silent about its own failure beyond that line: a roster that could not
    /// be written costs the next boot its resume, and must not cost this
    /// shutdown its exit — the harnesses are already dying and the store is
    /// already durable.
    pub fn record_at_shutdown(&self) {
        let wanted = resume_is_wanted(&self.shared.dir, |key| std::env::var(key).ok());
        match self.finish(wanted) {
            Ok(_) if !wanted => eprintln!("resume: opted out of this roll; recording nobody"),
            Ok(0) => eprintln!("resume: no agent was working; recorded nobody"),
            Ok(count) => eprintln!("resume: recorded {count} agent(s) to bring back"),
            Err(error) => eprintln!(
                "resume: could not record the roster: {error}; the live roster stays for the \
                 next boot"
            ),
        }
    }

    /// Block until everything published so far is on disk, or can never be
    /// (no writer). For tests, which read the file the writer thread writes.
    #[cfg(test)]
    pub fn settle(&self) {
        let mut slot = self.slot();
        while slot.writing || (slot.unwritten() && slot.writer_running && !slot.sealed) {
            slot = self.wait(slot);
        }
    }
}

/// The daemon's way down, in the order that matters: the roster is recorded
/// and durable before anything is torn down — the children, whose deaths
/// would otherwise read as turns ending, and the transport. `main` calls
/// this and nothing else to go down, so the order is tested where it lives.
pub fn shut_down(live: &LiveRoster, teardown: impl FnOnce()) {
    eprintln!("bridge: shutting down");
    live.record_at_shutdown();
    teardown();
}

#[cfg(test)]
mod tests;
