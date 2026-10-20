//! Bounded read admission shared by the SQL and observation lanes. Each lane
//! owns a fair per-opening queue; fixed SQL workers prepare owned metadata and
//! independent I/O workers execute external observations.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::sync::{oneshot, Notify, OwnedSemaphorePermit, Semaphore};

use super::{FrameHandler, SessionSender};
use crate::store::{ReadConnection, ReadPool};
use crate::timing::{Priority, QueuedFrame, WaitTimer};
use crate::transport::Frame;

pub(super) const SQL_WORKERS: usize = 8;
pub(super) const SQL_BACKLOG: usize = 16;
pub(super) const IO_WORKERS: usize = 8;

#[derive(Clone, Copy)]
enum ReadLane {
    Sql,
    Observation,
}

type Execution = Box<dyn FnOnce() + Send>;

/// Both ordinary SQL reads and observation metadata preparation share these
/// fixed SQL workers. External filesystem/Git work never enters this executor.
pub(super) struct ReadExecutors {
    send: std::sync::mpsc::Sender<Execution>,
    slots: Arc<Semaphore>,
}

impl ReadExecutors {
    fn new() -> Self {
        let (send, receive) = std::sync::mpsc::channel::<Execution>();
        let receive = Arc::new(Mutex::new(receive));
        for number in 0..SQL_WORKERS {
            let receive = receive.clone();
            std::thread::Builder::new()
                .name(format!("bridge-sql-{number}"))
                .spawn(move || loop {
                    let execution = receive.lock().unwrap().recv();
                    match execution {
                        Ok(execution) => execution(),
                        Err(_) => return,
                    }
                })
                .expect("start SQL read worker");
        }
        Self {
            send,
            slots: Arc::new(Semaphore::new(SQL_WORKERS)),
        }
    }

    async fn acquire(&self) -> OwnedSemaphorePermit {
        self.slots
            .clone()
            .acquire_owned()
            .await
            .expect("read executor stays open")
    }

    fn submit<T: Send + 'static>(
        &self,
        permit: OwnedSemaphorePermit,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> oneshot::Receiver<T> {
        let (done, complete) = oneshot::channel();
        self.send
            .send(Box::new(move || {
                let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
                drop(permit);
                if let Ok(answer) = answer {
                    let _ = done.send(answer);
                }
            }))
            .expect("SQL executors stay open");
        complete
    }
}

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

pub(super) struct ReadDispatcher {
    queue: Arc<Queue>,
    handler: FrameHandler,
}

impl ReadDispatcher {
    pub(super) fn new(handler: FrameHandler) -> Self {
        Self::for_lane(handler, ReadLane::Sql)
    }

    pub(super) fn new_observations(handler: FrameHandler) -> Self {
        Self::for_lane(handler, ReadLane::Observation)
    }

    fn for_lane(handler: FrameHandler, lane: ReadLane) -> Self {
        let queue = Arc::new(Queue::default());
        handler.read_executors.get_or_init(ReadExecutors::new);
        match lane {
            ReadLane::Sql => {
                for _ in 0..SQL_WORKERS {
                    tokio::spawn(coordinate_sql(queue.clone(), handler.clone()));
                }
            }
            ReadLane::Observation => {
                for number in 0..IO_WORKERS {
                    let executor = spawn_io_executor(number);
                    tokio::spawn(coordinate_io(queue.clone(), handler.clone(), executor));
                }
            }
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

impl Drop for ReadDispatcher {
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
    #[cfg(test)]
    measurement: super::sql_measurements::SqlSchedule,
}

impl Work {
    fn new(job: Job) -> Self {
        let caller = (job.frame.payload["id"].clone(), job.sender.clone());
        Self {
            job,
            callers: vec![caller],
            #[cfg(test)]
            measurement: super::sql_measurements::SqlSchedule::default(),
        }
    }

    #[cfg(test)]
    fn with_measurement(mut self, checkout: std::time::Duration) -> Self {
        self.measurement = super::sql_measurements::SqlSchedule::new(checkout);
        self
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

    fn run(self, handler: &FrameHandler, connection: Option<ReadConnection>) {
        self.run_on(handler, connection, ReadLane::Sql);
    }

    fn run_on(
        mut self,
        handler: &FrameHandler,
        connection: Option<ReadConnection>,
        lane: ReadLane,
    ) {
        self.callers.retain(|(_, caller)| caller.read_is_live());
        let Some((_, sender)) = self.callers.last() else {
            return;
        };
        self.job.sender = sender.clone();
        #[cfg(test)]
        if matches!(lane, ReadLane::Sql) {
            self.measurement.observe(handler, &self.job.frame);
        }
        let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match lane {
            ReadLane::Sql => {
                handler.run_sql(self.job.queued, self.job.sender, self.job.frame, connection)
            }
            ReadLane::Observation => handler.run(self.job.queued, self.job.sender, self.job.frame),
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

/// Prepare on the caller's executor and return only owned I/O work. The timer
/// spans both stages, while connection capacity is released before the job runs.
impl Work {
    fn prepare_observation(
        mut self,
        handler: &FrameHandler,
        connection: Option<ReadConnection>,
    ) -> Execution {
        if handler.observations.is_none() {
            let handler = handler.clone();
            return Box::new(move || self.run_on(&handler, connection, ReadLane::Observation));
        }
        self.callers.retain(|(_, caller)| caller.read_is_live());
        let Some((_, sender)) = self.callers.last() else {
            return Box::new(|| {});
        };
        self.job.sender = sender.clone();
        let timer = self
            .job
            .queued
            .start(self.job.frame.payload["method"].as_str().unwrap_or("other"));
        let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handler.prepare_observation(self.job.sender, self.job.frame, connection)
        }));
        Box::new(move || {
            let _timer = timer;
            if !self.callers.iter().any(|(_, caller)| caller.read_is_live()) {
                return;
            }
            let answer = prepared
                .ok()
                .and_then(|job| std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).ok())
                .unwrap_or_else(|| json!({"ok":false,"error":"handler failed"}));
            for (id, sender) in self.callers {
                if sender.read_is_live() {
                    let mut answer = answer.clone();
                    answer["id"] = id;
                    sender.push_read_reply(answer);
                }
            }
        })
    }
}

fn spawn_io_executor(number: usize) -> std::sync::mpsc::Sender<(Execution, oneshot::Sender<()>)> {
    let (send, receive) = std::sync::mpsc::channel::<(Execution, oneshot::Sender<()>)>();
    std::thread::Builder::new()
        .name(format!("bridge-io-{number}"))
        .spawn(move || {
            while let Ok((execution, done)) = receive.recv() {
                execution();
                let _ = done.send(());
            }
        })
        .expect("start observation read worker");
    send
}

async fn coordinate_sql(queue: Arc<Queue>, handler: FrameHandler) {
    let pool = handler.sql.as_ref().and_then(|sql| sql.pool.clone());
    let executors = handler
        .read_executors
        .get()
        .expect("initialized SQL executors");
    while queue.work_available().await {
        let Some(permit) = while_live(&queue, None, executors.acquire()).await else {
            continue;
        };
        #[cfg(test)]
        let checkout_started = std::time::Instant::now();
        let connection = match &pool {
            Some(pool) => match connection_while_waiting(&queue, pool).await {
                Some(connection) => Some(connection),
                None => continue,
            },
            None => None,
        };
        #[cfg(test)]
        let checkout = checkout_started.elapsed();
        let Some(work) = queue.take() else { continue };
        #[cfg(test)]
        let work = work.with_measurement(checkout);
        let handler = handler.clone();
        let _ = executors
            .submit(permit, move || work.run(&handler, connection))
            .await;
    }
}

async fn coordinate_io(
    queue: Arc<Queue>,
    handler: FrameHandler,
    executor: std::sync::mpsc::Sender<(Execution, oneshot::Sender<()>)>,
) {
    while queue.work_available().await {
        let Some(work) = queue.take() else { continue };
        let Some(execution) = prepare_io(&queue, &handler, work).await else {
            continue;
        };
        let (done, complete) = oneshot::channel();
        if executor.send((execution, done)).is_err() {
            return;
        }
        let _ = complete.await;
    }
}

async fn prepare_io(queue: &Queue, handler: &FrameHandler, work: Work) -> Option<Execution> {
    if !crate::api::effects::observation_needs_sql(&work.job.frame.payload) {
        let handler = handler.clone();
        return Some(Box::new(move || work.prepare_observation(&handler, None)()));
    }
    let executors = handler
        .read_executors
        .get()
        .expect("initialized SQL executors");
    let permit = while_live(queue, Some(&work), executors.acquire()).await?;
    let pool = handler
        .observations
        .as_ref()
        .and_then(|observations| observations.pool.as_ref());
    let connection = match pool {
        Some(pool) => Some(while_live(queue, Some(&work), pool.acquire()).await?),
        None => None,
    };
    let handler = handler.clone();
    executors
        .submit(permit, move || {
            work.prepare_observation(&handler, connection)
        })
        .await
        .ok()
}

/// Preparation waits retain an I/O slot only while at least one caller still
/// owns its carrier/opening. Closing a carrier wakes admission and checkout.
async fn while_live<T>(
    queue: &Queue,
    work: Option<&Work>,
    future: impl std::future::Future<Output = T>,
) -> Option<T> {
    tokio::pin!(future);
    loop {
        let changed = queue.changed.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        {
            let waiting = queue.waiting.lock().unwrap();
            let live = work.map_or_else(
                || !waiting.ready.is_empty(),
                |work| work.callers.iter().any(|(_, sender)| sender.read_is_live()),
            );
            if waiting.closed || !live {
                return None;
            }
        }
        tokio::select! { result = &mut future => return Some(result), _ = changed => {} }
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

/// Synchronous callers use their own I/O thread. Any SQL preparation still
/// enters the same fixed SQL executor; no runtime is nested on a Tokio worker.
pub(super) fn call_observation(
    handler: FrameHandler,
    sender: SessionSender,
    frame: Frame,
    arrived: WaitTimer,
) -> Value {
    std::thread::Builder::new()
        .name("bridge-io-direct".into())
        .spawn(move || {
            let _timer = handler
                .clock
                .queued_since(Priority::Foreground, arrived)
                .start(frame.payload["method"].as_str().unwrap_or("other"));
            let needs_sql = crate::api::effects::observation_needs_sql(&frame.payload);
            if !needs_sql {
                return handler.prepare_observation(sender, frame, None)();
            }
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("direct read checkout runtime");
            let prepared = runtime.block_on(async {
                let executors = handler.read_executors.get_or_init(ReadExecutors::new);
                let permit = executors.acquire().await;
                let connection = match handler
                    .observations
                    .as_ref()
                    .and_then(|observations| observations.pool.as_ref())
                {
                    Some(pool) => Some(pool.acquire().await),
                    None => None,
                };
                let prepare = handler.clone();
                executors
                    .submit(permit, move || {
                        prepare.prepare_observation(sender, frame, connection)
                    })
                    .await
            });
            prepared
                .map(|job| job())
                .unwrap_or_else(|_| json!({"ok":false,"error":"handler failed"}))
        })
        .expect("start direct observation read")
        .join()
        .unwrap_or_else(|_| json!({"ok":false,"error":"handler failed"}))
}
