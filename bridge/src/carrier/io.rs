//! Demand-grown, dedicated I/O execution. Admission remains in the read lane;
//! its eight coordinators bound submitted work, and serial reads reuse a worker.

use std::io;
use std::sync::{mpsc, Arc, Mutex};
use tokio::sync::oneshot;

pub(super) type Execution = Box<dyn FnOnce() + Send>;

#[derive(Default)]
struct Counts {
    workers: usize,
    outstanding: usize,
    next_worker: usize,
}

struct Outstanding(Arc<Mutex<Counts>>);

impl Drop for Outstanding {
    fn drop(&mut self) {
        self.0.lock().unwrap().outstanding -= 1;
    }
}

struct Worker(Arc<Mutex<Counts>>);

impl Drop for Worker {
    fn drop(&mut self) {
        self.0.lock().unwrap().workers -= 1;
    }
}

struct Job {
    execution: Execution,
    // Release accounting before waking the coordinator, including send failure.
    outstanding: Outstanding,
    done: oneshot::Sender<()>,
}

pub(super) struct IoExecutors {
    send: mpsc::Sender<Job>,
    receive: Arc<Mutex<mpsc::Receiver<Job>>>,
    counts: Arc<Mutex<Counts>>,
    limit: usize,
    #[cfg(test)]
    starts: Arc<std::sync::atomic::AtomicUsize>,
}

impl IoExecutors {
    pub(super) fn new(
        limit: usize,
        #[cfg(test)] starts: Arc<std::sync::atomic::AtomicUsize>,
    ) -> Self {
        let (send, receive) = mpsc::channel();
        Self {
            send,
            receive: Arc::new(Mutex::new(receive)),
            counts: Arc::new(Mutex::new(Counts::default())),
            limit: limit.max(1),
            #[cfg(test)]
            starts,
        }
    }

    pub(super) fn submit(&self, execution: Execution) -> io::Result<oneshot::Receiver<()>> {
        let mut counts = self.counts.lock().unwrap();
        if counts.outstanding + 1 > counts.workers && counts.workers < self.limit {
            self.spawn(counts.next_worker)?;
            counts.next_worker += 1;
            counts.workers += 1;
        }
        counts.outstanding += 1;
        let outstanding = Outstanding(self.counts.clone());
        drop(counts);
        let (done, complete) = oneshot::channel();
        self.send
            .send(Job {
                execution,
                outstanding,
                done,
            })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "I/O executor stopped"))?;
        Ok(complete)
    }

    fn spawn(&self, number: usize) -> io::Result<()> {
        let receive = self.receive.clone();
        let counts = self.counts.clone();
        // Workers own no sender/executor handle. Dropping the executor closes
        // the receiver after active work, without a sender-retention cycle.
        std::thread::Builder::new()
            .name(format!("bridge-io-{number}"))
            .spawn(move || {
                let _worker = Worker(counts);
                loop {
                    // This statement drops the receiver mutex before execution.
                    let received = receive.lock().unwrap().recv();
                    let Ok(Job {
                        execution,
                        outstanding,
                        done,
                    }) = received
                    else {
                        return;
                    };
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(execution));
                    drop(outstanding);
                    let _ = done.send(());
                }
            })?;
        #[cfg(test)]
        self.starts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::off_lock::OffLockGate;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    const PATIENCE: Duration = Duration::from_secs(5);

    fn counts(pool: &IoExecutors) -> (usize, usize) {
        let counts = pool.counts.lock().unwrap();
        (counts.workers, counts.outstanding)
    }

    #[tokio::test]
    async fn panic_returns_outstanding_capacity_before_completion_and_reuses_worker() {
        let starts = Arc::new(AtomicUsize::new(0));
        let pool = IoExecutors::new(1, starts.clone());
        let complete = pool
            .submit(Box::new(|| panic!("injected I/O execution failure")))
            .unwrap();
        tokio::time::timeout(PATIENCE, complete)
            .await
            .expect("panicking work releases its coordinator")
            .unwrap();
        assert_eq!(counts(&pool), (1, 0));
        let ran = Arc::new(AtomicBool::new(false));
        let observed = ran.clone();
        let complete = pool
            .submit(Box::new(move || observed.store(true, Ordering::SeqCst)))
            .unwrap();
        tokio::time::timeout(PATIENCE, complete)
            .await
            .expect("the worker survives execution failure")
            .unwrap();
        assert!(ran.load(Ordering::SeqCst));
        assert_eq!(counts(&pool), (1, 0));
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn dropping_executor_does_not_retain_its_sender_or_wait_for_active_work() {
        let pool = Arc::new(IoExecutors::new(1, Arc::new(AtomicUsize::new(0))));
        let executor = Arc::downgrade(&pool);
        let receiver = Arc::downgrade(&pool.receive);
        let accounting = pool.counts.clone();
        let (gate, held) = OffLockGate::new();
        let complete = pool.submit(Box::new(move || gate.arrive())).unwrap();
        held.wait_for_arrival();
        assert_eq!(counts(&pool), (1, 1));
        drop(pool);
        assert!(
            executor.upgrade().is_none(),
            "workers retain no executor owner"
        );
        assert_eq!(accounting.lock().unwrap().outstanding, 1);
        held.release();
        tokio::time::timeout(PATIENCE, complete)
            .await
            .expect("active work finishes after its owner is dropped")
            .unwrap();
        tokio::time::timeout(PATIENCE, async {
            loop {
                let retired = accounting.lock().unwrap().workers == 0;
                if retired && receiver.upgrade().is_none() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("closing the executor closes and retires its worker receiver");
        assert_eq!(accounting.lock().unwrap().outstanding, 0);
    }
}
