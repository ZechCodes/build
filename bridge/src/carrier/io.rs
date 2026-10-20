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
