//! SQL admission counts callers until a connection is in hand. One ready marker
//! per opening feeds a fixed set of coordinators and dedicated blocking threads.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::sync::{oneshot, Notify};

use super::{FrameHandler, SessionSender};
use crate::store::{ReadConnection, ReadPool};
use crate::timing::{Priority, QueuedFrame, WaitTimer};
use crate::transport::Frame;

pub(super) const SQL_WORKERS: usize = 8;
pub(super) const SQL_BACKLOG: usize = 16;

struct Job {
    sender: SessionSender,
    frame: Frame,
    queued: QueuedFrame,
}

#[derive(Default)]
struct Waiting {
    openings: HashMap<u64, VecDeque<Job>>,
    ready: VecDeque<u64>,
    closed: bool,
}

#[derive(Default)]
struct Queue {
    waiting: Mutex<Waiting>,
    changed: Notify,
}

pub(super) struct SqlDispatcher {
    queue: Arc<Queue>,
    handler: FrameHandler,
}

impl SqlDispatcher {
    pub(super) fn new(handler: FrameHandler) -> Self {
        let queue = Arc::new(Queue::default());
        for number in 0..SQL_WORKERS {
            let executor = spawn_executor(number, handler.clone());
            let pool = handler.sql.as_ref().and_then(|sql| sql.pool.clone());
            tokio::spawn(coordinate(queue.clone(), pool, executor));
        }
        Self { queue, handler }
    }

    pub(super) async fn dispatch(&self, sender: SessionSender, frame: Frame, arrived: WaitTimer) {
        let priority = match frame.payload["priority"].as_str() {
            Some("background") => Priority::Background,
            _ => Priority::Foreground,
        };
        let mut job = Some(Job {
            sender,
            frame,
            queued: self.handler.clock.queued_since(priority, arrived),
        });
        loop {
            let changed = self.queue.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.queue.admit(&mut job) {
                self.queue.changed.notify_waiters();
                return;
            }
            changed.await;
        }
    }

    pub(super) fn prune(&self) {
        self.queue.waiting.lock().unwrap().prune();
        self.queue.changed.notify_waiters();
    }

    #[cfg(test)]
    pub(super) fn depths(&self) -> (usize, usize) {
        let waiting = self.queue.waiting.lock().unwrap();
        (
            waiting.openings.values().map(VecDeque::len).sum(),
            waiting.ready.len(),
        )
    }
}

impl Drop for SqlDispatcher {
    fn drop(&mut self) {
        let mut waiting = self.queue.waiting.lock().unwrap();
        waiting.closed = true;
        waiting.openings.clear();
        waiting.ready.clear();
        self.queue.changed.notify_waiters();
    }
}

impl Queue {
    /// A full queue keeps exactly the carrier's current intake future parked.
    /// Nothing has been moved into a worker or another internal queue yet.
    fn admit(&self, incoming: &mut Option<Job>) -> bool {
        let mut waiting = self.waiting.lock().unwrap();
        let job = incoming.as_ref().unwrap();
        if waiting.closed || !job.sender.read_is_live() {
            return true;
        }
        let opening = job.sender.still_open.ordinal;
        let jobs = waiting.openings.entry(opening).or_default();
        if jobs.len() == SQL_BACKLOG {
            return false;
        }
        let was_empty = jobs.is_empty();
        jobs.push_back(incoming.take().unwrap());
        if was_empty {
            waiting.ready.push_back(opening);
        }
        true
    }

    async fn work_available(&self) -> bool {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let waiting = self.waiting.lock().unwrap();
                if waiting.closed {
                    return false;
                }
                if !waiting.ready.is_empty() {
                    return true;
                }
            }
            changed.await;
        }
    }

    fn take(&self) -> Option<Work> {
        let work = self.waiting.lock().unwrap().take();
        self.changed.notify_waiters();
        work
    }
}

impl Waiting {
    fn prune(&mut self) {
        self.openings.retain(|_, jobs| {
            jobs.retain(|job| job.sender.read_is_live());
            !jobs.is_empty()
        });
        self.ready
            .retain(|opening| self.openings.contains_key(opening));
    }

    fn take(&mut self) -> Option<Work> {
        self.prune();
        let opening = self.ready.pop_front()?;
        let jobs = self.openings.get_mut(&opening)?;
        let mut work = Work::new(jobs.pop_front()?);
        // Fold only queued callers. Each counted against B until this pop, and
        // each retains its carrier fence independently of the chosen handler.
        let mut remaining = VecDeque::new();
        for job in jobs.drain(..) {
            if work.matches(&job) {
                work.join(job);
            } else {
                remaining.push_back(job);
            }
        }
        *jobs = remaining;
        if jobs.is_empty() {
            self.openings.remove(&opening);
        } else {
            self.ready.push_back(opening);
        }
        Some(work)
    }
}

struct Work {
    job: Job,
    callers: Vec<(Value, SessionSender)>,
}

impl Work {
    fn new(job: Job) -> Self {
        let caller = (job.frame.payload["id"].clone(), job.sender.clone());
        Self {
            job,
            callers: vec![caller],
        }
    }

    fn matches(&self, job: &Job) -> bool {
        self.job.frame.payload["method"] == job.frame.payload["method"]
            && self.job.frame.payload.get("params") == job.frame.payload.get("params")
    }

    fn join(&mut self, job: Job) {
        self.callers
            .push((job.frame.payload["id"].clone(), job.sender.clone()));
        self.job.queued.fold_in(job.queued);
        self.job.sender = job.sender;
        self.job.frame = job.frame;
    }

    fn run(mut self, handler: &FrameHandler, connection: Option<ReadConnection>) {
        self.callers.retain(|(_, caller)| caller.read_is_live());
        let Some((_, sender)) = self.callers.last() else {
            return;
        };
        self.job.sender = sender.clone();
        let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handler.run_sql(self.job.queued, self.job.sender, self.job.frame, connection)
        }))
        .unwrap_or_else(|_| json!({"ok":false,"error":"handler failed"}));
        for (id, sender) in self.callers {
            if sender.read_is_live() {
                let mut answer = answer.clone();
                answer["id"] = id;
                sender.push_read_reply(answer);
            }
        }
    }
}

type Execution = (Work, Option<ReadConnection>, oneshot::Sender<()>);

fn spawn_executor(number: usize, handler: FrameHandler) -> std::sync::mpsc::Sender<Execution> {
    let (send, receive) = std::sync::mpsc::channel::<Execution>();
    std::thread::Builder::new()
        .name(format!("bridge-sql-{number}"))
        .spawn(move || {
            while let Ok((work, connection, done)) = receive.recv() {
                work.run(&handler, connection);
                let _ = done.send(());
            }
        })
        .expect("start SQL read worker");
    send
}

async fn coordinate(
    queue: Arc<Queue>,
    pool: Option<ReadPool>,
    executor: std::sync::mpsc::Sender<Execution>,
) {
    while queue.work_available().await {
        let connection = match &pool {
            Some(pool) => match connection_while_waiting(&queue, pool).await {
                Some(connection) => Some(connection),
                None => continue,
            },
            None => None,
        };
        let Some(work) = queue.take() else { continue };
        let (done, complete) = oneshot::channel();
        if executor.send((work, connection, done)).is_err() {
            return;
        }
        let _ = complete.await;
    }
}

async fn connection_while_waiting(queue: &Queue, pool: &ReadPool) -> Option<ReadConnection> {
    let acquiring = pool.acquire();
    tokio::pin!(acquiring);
    loop {
        let changed = queue.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        {
            let waiting = queue.waiting.lock().unwrap();
            if waiting.closed || waiting.ready.is_empty() {
                return None;
            }
        }
        tokio::select! {
            connection = &mut acquiring => return Some(connection),
            _ = changed => {},
        }
    }
}
