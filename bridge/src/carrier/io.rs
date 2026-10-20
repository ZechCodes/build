//! Demand-grown dedicated execution, with separate instances for SQL and I/O.
//! Read admission and shared SQL permits bound submitted work.

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
    done: Option<oneshot::Sender<()>>,
}

pub(super) struct DedicatedExecutors {
    send: mpsc::Sender<Job>,
    receive: Arc<Mutex<mpsc::Receiver<Job>>>,
    counts: Arc<Mutex<Counts>>,
    limit: usize,
    name: &'static str,
    #[cfg(test)]
    starts: Arc<std::sync::atomic::AtomicUsize>,
}

impl DedicatedExecutors {
    pub(super) fn new(
        name: &'static str,
        limit: usize,
        #[cfg(test)] starts: Arc<std::sync::atomic::AtomicUsize>,
    ) -> Self {
        let (send, receive) = mpsc::channel();
        Self {
            send,
            receive: Arc::new(Mutex::new(receive)),
            counts: Arc::new(Mutex::new(Counts::default())),
            limit: limit.max(1),
            name,
            #[cfg(test)]
            starts,
        }
    }

    pub(super) fn ensure_workers(&self, workers: usize) -> io::Result<()> {
        let mut counts = self.counts.lock().unwrap();
        while counts.workers < workers.min(self.limit) {
            self.grow(&mut counts)?;
        }
        Ok(())
    }

    pub(super) fn submit(&self, execution: Execution) -> io::Result<oneshot::Receiver<()>> {
        let (done, complete) = oneshot::channel();
        self.enqueue(execution, Some(done))?;
        Ok(complete)
    }

    /// SQL already owns its typed completion channel; do not allocate another.
    pub(super) fn submit_detached(&self, execution: Execution) -> io::Result<()> {
        self.enqueue(execution, None)
    }

    fn enqueue(&self, execution: Execution, done: Option<oneshot::Sender<()>>) -> io::Result<()> {
        let mut counts = self.counts.lock().unwrap();
        if counts.outstanding + 1 > counts.workers && counts.workers < self.limit {
            self.grow(&mut counts)?;
        }
        counts.outstanding += 1;
        let outstanding = Outstanding(self.counts.clone());
        drop(counts);
        self.send
            .send(Job {
                execution,
                outstanding,
                done,
            })
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "dedicated executor stopped"))
    }

    fn grow(&self, counts: &mut Counts) -> io::Result<()> {
        self.spawn(counts.next_worker)?;
        counts.next_worker += 1;
        counts.workers += 1;
        Ok(())
    }

    fn spawn(&self, number: usize) -> io::Result<()> {
        let receive = self.receive.clone();
        let counts = self.counts.clone();
        // Workers own no sender/executor handle. Dropping the executor closes
        // the receiver after active work, without a sender-retention cycle.
        std::thread::Builder::new()
            .name(format!("{}-{number}", self.name))
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
                    if let Some(done) = done {
                        let _ = done.send(());
                    }
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

    fn counts(pool: &DedicatedExecutors) -> (usize, usize) {
        let counts = pool.counts.lock().unwrap();
        (counts.workers, counts.outstanding)
    }

    #[tokio::test]
    async fn serial_submissions_reuse_one_worker_after_actual_execution_completion() {
        let starts = Arc::new(AtomicUsize::new(0));
        let pool = DedicatedExecutors::new("bridge-io", 8, starts.clone());
        for _ in 0..22 {
            let complete = pool.submit(Box::new(|| {})).unwrap();
            tokio::time::timeout(PATIENCE, complete)
                .await
                .expect("serial work finishes before the next submission")
                .unwrap();
            assert_eq!(counts(&pool), (1, 0));
            assert_eq!(starts.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn panic_returns_outstanding_capacity_before_completion_and_reuses_worker() {
        let starts = Arc::new(AtomicUsize::new(0));
        let pool = DedicatedExecutors::new("bridge-io", 1, starts.clone());
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
    async fn detached_work_survives_panic_without_an_extra_completion_channel() {
        let starts = Arc::new(AtomicUsize::new(0));
        let pool = DedicatedExecutors::new("bridge-sql", 1, starts.clone());
        pool.submit_detached(Box::new(|| panic!("injected detached SQL failure")))
            .unwrap();
        let complete = pool
            .submit(Box::new(|| {
                assert_eq!(std::thread::current().name(), Some("bridge-sql-0"));
            }))
            .unwrap();
        tokio::time::timeout(PATIENCE, complete)
            .await
            .expect("the named worker survives detached work failure")
            .unwrap();
        assert_eq!(counts(&pool), (1, 0));
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn ensuring_workers_is_bounded_idempotent_and_does_not_submit_work() {
        let starts = Arc::new(AtomicUsize::new(0));
        let pool = DedicatedExecutors::new("bridge-sql", 8, starts.clone());
        assert_eq!(counts(&pool), (0, 0));
        pool.ensure_workers(1).unwrap();
        assert_eq!(counts(&pool), (1, 0));
        pool.ensure_workers(8).unwrap();
        pool.ensure_workers(9).unwrap();
        assert_eq!(counts(&pool), (8, 0));
        assert_eq!(starts.load(Ordering::SeqCst), 8);
        let complete = pool.submit(Box::new(|| {})).unwrap();
        tokio::time::timeout(PATIENCE, complete)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(counts(&pool), (8, 0));
        assert_eq!(starts.load(Ordering::SeqCst), 8);
    }

    #[tokio::test]
    async fn dropping_executor_does_not_retain_its_sender_or_wait_for_active_work() {
        let pool = Arc::new(DedicatedExecutors::new(
            "bridge-io",
            1,
            Arc::new(AtomicUsize::new(0)),
        ));
        let executor = Arc::downgrade(&pool);
        let receiver = Arc::downgrade(&pool.receive);
        let accounting = pool.counts.clone();
        let (gate, held) = OffLockGate::new();
        let complete = pool.submit(Box::new(move || gate.arrive())).unwrap();
        held.wait_for_arrival();
        assert_eq!(counts(&pool), (1, 1));
        let (dropped, owner_released) = oneshot::channel();
        std::thread::spawn(move || {
            drop(pool);
            let _ = dropped.send(());
        });
        let owner_released = tokio::time::timeout(PATIENCE, owner_released).await;
        // Release active work before asserting, even if a future Drop starts
        // waiting for workers. A failure must not leave the fixture blocked.
        held.release();
        assert!(
            matches!(owner_released, Ok(Ok(()))),
            "dropping the owner must not wait for active work"
        );
        assert!(
            executor.upgrade().is_none(),
            "workers retain no executor owner"
        );
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
