//! The runtime the daemon's liveness traffic runs on.
//!
//! The daemon's main runtime is where the app's work happens, and the app's
//! work waits on one mutex. That is by design for the handlers, which run on
//! the blocking pool; it is fatal for anything that must answer within seconds
//! and runs on a worker thread instead. On 2026-09-24 the per-agent pumps, the
//! MCP control socket and the idle monitor — thirty-odd tasks, each taking the
//! app lock on a worker — parked every worker behind a 44-second `issues.list`,
//! and with the workers went the I/O driver: no ICE consent answered, no relay
//! pong sent, no data-channel frame read, no presence beat posted. The phone
//! saw ICE go `disconnected`, its 3-second ping went unanswered, and it severed
//! the session, thirty times in thirty minutes (issue #128).
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

use std::sync::Arc;

use tokio::runtime::{Handle, Runtime};

/// How many threads the liveness runtime has. Two: the work is a socket, a
/// beat and a few channels' worth of encryption, and the second thread is so
/// one long chunk of that never holds up the rest.
const WORKER_THREADS: usize = 2;

/// The runtime, alive for as long as the daemon is.
pub struct LivenessRuntime {
    runtime: Runtime,
}

impl LivenessRuntime {
    pub fn start() -> Result<Arc<LivenessRuntime>, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(WORKER_THREADS)
            // Under 15 bytes, so it survives Linux's `comm` truncation and
            // reads as itself in `top -H`.
            .thread_name("bridge-live")
            .enable_all()
            .build()
            .map_err(|error| format!("cannot start the liveness runtime: {error}"))?;
        Ok(Arc::new(LivenessRuntime { runtime }))
    }

    /// A handle onto this runtime, for spawning onto it and for driving a
    /// future on it from a thread that holds no runtime of its own.
    pub fn handle(&self) -> Handle {
        self.runtime.handle().clone()
    }

    /// Run `future` on this runtime.
    pub fn spawn<F>(&self, future: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        self.runtime.spawn(future)
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
        let liveness = LivenessRuntime::start().unwrap();

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
}
