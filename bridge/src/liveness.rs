//! The runtime the daemon's liveness traffic runs on.
//!
//! The daemon's main runtime is where the app's work happens, and the app's
//! work waits on one mutex. That is by design for the handlers, which run on
//! the blocking pool; it is fatal for anything that must answer within seconds
//! and runs on a worker thread instead. On 2026-09-24 the per-agent pumps, the
//! MCP control socket and the idle monitor — thirty-odd tasks, each taking the
//! app lock on a worker — parked every worker behind a 44-second `tasks.list`,
//! and with the workers went the I/O driver: no ICE consent answered, no relay
//! pong sent, no data-channel frame read, no presence beat posted. The phone
//! saw ICE go `disconnected`, its 3-second ping went unanswered, and it severed
//! the session, thirty times in thirty minutes (task #128).
//!
//! So the traffic whose whole meaning is "still here" runs on a runtime of its
//! own, whose threads take no lock the app holds: the relay socket and its
//! heartbeat, the presence beat to the api, and every peer connection's
//! channels. The peer connections' drivers — UDP, ICE, DTLS, SCTP — run on
//! the webrtc crate's own reactor pool, apart even from this runtime. A
//! handler that must wait for a peer's answer drives that future on this
//! runtime from the blocking thread it holds ([`crate::rtc::SessionPeers`]).
//!
//! Nothing here may take the app mutex. A frame that needs it is handed to the
//! dispatcher, whose workers live on the main runtime and whose queues are the
//! only thing a task here ever waits on, and waiting on a queue suspends the
//! task, never the thread.
//!
//! The same shape carries one more runtime, `bridge-push`: the work that
//! serializes and encrypts what the bridge pushes to its clients — the change
//! bus's flusher, a frame per subscriber per window, and the terminals' byte
//! pumps, a vt100 parse per chunk and a frame per attached client every 10 ms
//! (task #131). That is CPU, not waiting, and on the main runtime's workers
//! it stood in line with the dispatcher that answers the clients' requests.
//! It takes no lock the app holds on its own threads; a pump's death rites,
//! which do, go to the blocking pool.

use std::sync::{Arc, Mutex};

use tokio::runtime::{Handle, Runtime};

/// How many threads each runtime here has. Two: the liveness work is a socket,
/// a beat and a few channels' worth of encryption, the push work a few
/// terminals' paint and a flush, and the second thread is so one long chunk of
/// either never holds up the rest.
const WORKER_THREADS: usize = 2;

/// A runtime of its own, alive for as long as the daemon is.
///
/// It is stopped, never merely dropped: the daemon's shutdown runs inside a
/// task of the main runtime, and a tokio runtime dropped from inside another
/// runtime's task panics ("Cannot drop a runtime in a context where blocking
/// is not allowed"), which turned every `systemctl stop` of the bridge into
/// an exit 101. [`stop`](Self::stop) shuts the runtime down in the background,
/// which blocks nothing and is allowed anywhere; the drop does the same for
/// whoever forgets.
pub struct DedicatedRuntime {
    runtime: Mutex<Option<Runtime>>,
    handle: Handle,
}

impl DedicatedRuntime {
    /// The relay socket, the presence beat and the peers' channels.
    pub fn liveness() -> Result<Arc<DedicatedRuntime>, String> {
        DedicatedRuntime::start("bridge-live")
    }

    /// What the bridge pushes to its clients: the change bus's flusher and
    /// the terminals' byte pumps.
    pub fn push() -> Result<Arc<DedicatedRuntime>, String> {
        DedicatedRuntime::start("bridge-push")
    }

    /// `name` is under 15 bytes, so it survives Linux's `comm` truncation and
    /// reads as itself in `top -H`.
    fn start(name: &'static str) -> Result<Arc<DedicatedRuntime>, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(WORKER_THREADS)
            .thread_name(name)
            .enable_all()
            .build()
            .map_err(|error| format!("cannot start the {name} runtime: {error}"))?;
        let handle = runtime.handle().clone();
        Ok(Arc::new(DedicatedRuntime {
            runtime: Mutex::new(Some(runtime)),
            handle,
        }))
    }

    /// A handle onto this runtime, for spawning onto it and for driving a
    /// future on it from a thread that holds no runtime of its own.
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }

    /// Run `future` on this runtime.
    pub fn spawn<F>(&self, future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.handle.spawn(future)
    }

    /// End the runtime: every task on it is cancelled at its next yield and
    /// its threads go. Returns at once, waiting for none of that, so it may
    /// be called from inside another runtime's task — which is where the
    /// daemon's shutdown runs.
    pub fn stop(&self) {
        if let Some(runtime) = self.runtime.lock().unwrap().take() {
            runtime.shutdown_background();
        }
    }
}

impl Drop for DedicatedRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// The whole point, in one test: a main runtime whose only worker is
    /// parked on a std mutex still has a liveness runtime that ticks.
    #[test]
    fn a_parked_main_runtime_does_not_stop_the_liveness_runtime() {
        let main = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let liveness = DedicatedRuntime::liveness().unwrap();

        // The app lock, held by "a handler" for the length of the test.
        let app_lock = Arc::new(Mutex::new(()));
        let held = app_lock.lock().unwrap();
        // "A pump": takes the app lock on the main runtime's one worker.
        let parked = Arc::clone(&app_lock);
        main.spawn(async move {
            let _guard = parked.lock().unwrap();
        });
        // Let the worker reach the lock before the timer is measured.
        std::thread::sleep(Duration::from_millis(50));

        // A timer on the main runtime cannot fire: nobody drives it.
        let main_timer = main.spawn(async { tokio::time::sleep(Duration::from_millis(10)).await });
        // The same timer on the liveness runtime fires on time.
        let started = Instant::now();
        liveness
            .handle()
            .block_on(async { tokio::time::sleep(Duration::from_millis(10)).await });
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "the liveness runtime ticked in {:?} with the main runtime parked",
            started.elapsed()
        );
        assert!(
            !main_timer.is_finished(),
            "the main runtime's timer had nobody to drive it"
        );
        drop(held);
    }

    /// The shutdown, in one test: stopping the liveness runtime from inside a
    /// task of the main runtime — where the daemon's shutdown runs — neither
    /// panics nor blocks, and the tasks it carried are gone.
    #[test]
    fn stopping_from_inside_another_runtime_ends_its_tasks_without_a_panic() {
        let main = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        main.block_on(async {
            let liveness = DedicatedRuntime::liveness().unwrap();
            let ticking = liveness.spawn(async {
                loop {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            });
            let started = Instant::now();
            liveness.stop();
            drop(liveness);
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "the stop waited {:?}",
                started.elapsed()
            );
            let ended = tokio::time::timeout(Duration::from_secs(5), ticking)
                .await
                .expect("the task ended with its runtime");
            assert!(ended.is_err(), "the task was cancelled, not finished");
        });
    }
}
