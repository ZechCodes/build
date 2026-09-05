//! Ending a session, off the app mutex.
//!
//! [`AgentSession::end`] is kill THEN reap, and SIGKILL does not land on a
//! child wedged in uninterruptible I/O until that I/O returns. It is therefore
//! an unbounded process wait, and every verb that closes a tab used to make it
//! with the app mutex in hand: one stuck harness stopped the daemon.
//!
//! A [`Retirement`] is that wait, moved to a thread of its own. One thread per
//! kill and never a queue — a child that parks its reaper parks nobody else's
//! — and no lock of any kind is held while it runs.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use crate::harness::AgentSession;

/// A session being killed and reaped somewhere else.
///
/// The receipt is what a caller waits on when it has to — a checkout cannot be
/// removed out from under a process still writing into it — and what every
/// other caller drops, because the tab left the registry when the retirement
/// began and that is what makes the agent unaddressable.
#[derive(Clone)]
pub struct Retirement {
    reaped: Arc<(Mutex<bool>, Condvar)>,
}

impl Retirement {
    /// Kill and reap `session` on a thread of its own. Returns before either
    /// has happened.
    pub fn begin(session: Arc<dyn AgentSession>) -> Retirement {
        let reaped = Arc::new((Mutex::new(false), Condvar::new()));
        let signal = Arc::clone(&reaped);
        // A std thread and not a spawned task: the synchronous unit tests run
        // with no runtime under them, and a tab still has to end there.
        std::thread::spawn(move || {
            session.end();
            let (done, waiting) = &*signal;
            *done.lock().unwrap() = true;
            waiting.notify_all();
        });
        Retirement { reaped }
    }

    /// Whether the process is reaped, waiting up to `timeout` for it.
    ///
    /// Callable only with the app mutex released: it is the wait the daemon's
    /// one rule forbids, made explicit so the callers that genuinely need it
    /// are the ones that say so.
    pub fn wait(&self, timeout: Duration) -> bool {
        let (done, waiting) = &*self.reaped;
        let (done, _) = waiting
            .wait_timeout_while(done.lock().unwrap(), timeout, |done| !*done)
            .expect("a retirement's thread never panics while holding this");
        *done
    }

    /// Whether every one of `writers` is reaped, waiting up to `timeout` for
    /// each in turn. The wait a directory removal makes before it walks: a
    /// child still creating files under the walk fails it.
    pub fn wait_all(writers: &[Retirement], timeout: Duration) -> bool {
        writers.iter().fold(true, |every_writer_reaped, writer| {
            let reaped = writer.wait(timeout);
            every_writer_reaped && reaped
        })
    }
}

static RETIRING_DIRS: AtomicU64 = AtomicU64::new(0);

/// The suffix a directory waits under between leaving its place and being
/// removed. Tests read it to find the directory a retirement is still holding.
pub const RETIRING_DIR_MARK: &str = ".retiring-";

/// Take `dir` out of its place now and remove it once the sessions writing
/// into it are reaped.
///
/// The rename is one bounded syscall, so it is made here, under whatever lock
/// the caller holds: the place is free for a successor at once, and a writer
/// still alive keeps its files under the new name. The removal is a filesystem
/// walk — one a child still creating files under it fails — and the reap it
/// waits for is unbounded, so both run on a thread that holds nothing. A
/// writer still not reaped after `timeout` is logged under `subject` and the
/// removal goes ahead anyway; a directory already gone is nothing to remove.
pub fn remove_dir_once_reaped(
    writers: Vec<Retirement>,
    dir: PathBuf,
    timeout: Duration,
    subject: String,
) {
    let retiring = retiring_name(&dir);
    let doomed = match std::fs::rename(&dir, &retiring) {
        Ok(()) => retiring,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            eprintln!(
                "{subject}: could not move {} aside: {error}; removing it in place",
                dir.display()
            );
            dir
        }
    };
    std::thread::spawn(move || {
        if !Retirement::wait_all(&writers, timeout) {
            eprintln!(
                "{subject}: a session did not die within {timeout:?}; removing {} anyway",
                doomed.display()
            );
        }
        if let Err(error) = std::fs::remove_dir_all(&doomed) {
            if error.kind() != std::io::ErrorKind::NotFound {
                eprintln!("{subject}: could not remove {}: {error}", doomed.display());
            }
        }
    });
}

/// A sibling name nothing else will claim: the process id keeps a leftover
/// from an earlier daemon out of the way, the counter keeps this daemon's own
/// retirements apart.
fn retiring_name(dir: &std::path::Path) -> PathBuf {
    let name = dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ordinal = RETIRING_DIRS.fetch_add(1, Ordering::Relaxed);
    dir.with_file_name(format!(
        "{name}{RETIRING_DIR_MARK}{}-{ordinal}",
        std::process::id()
    ))
}
