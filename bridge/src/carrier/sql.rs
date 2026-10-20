//! Bounded read admission shared by the SQL and observation lanes. Each lane
//! owns a fair per-opening queue; fixed SQL workers prepare owned metadata and
//! independent I/O workers execute external observations.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use tokio::sync::{oneshot, Notify, OwnedSemaphorePermit, Semaphore};

use super::io::{Execution, IoExecutors};
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

/// Both ordinary SQL reads and observation metadata preparation share these
/// fixed SQL workers. External filesystem/Git work never enters this executor.
pub(super) struct ReadExecutors {
    send: std::sync::mpsc::Sender<Execution>,
    slots: Arc<Semaphore>,
}

impl ReadExecutors {
    fn new(#[cfg(test)] starts: Arc<std::sync::atomic::AtomicUsize>) -> Self {
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
            #[cfg(test)]
            starts.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
    ready: Notify,
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
        match lane {
            ReadLane::Sql => {
                handler.read_executors.get_or_init(|| {
                    ReadExecutors::new(
                        #[cfg(test)]
                        handler.sql_thread_starts.clone(),
                    )
                });
                for _ in 0..SQL_WORKERS {
                    tokio::spawn(coordinate_sql(queue.clone(), handler.clone()));
                }
            }
            ReadLane::Observation => {
                let executor = Arc::new(IoExecutors::new(
                    IO_WORKERS,
                    #[cfg(test)]
                    handler.io_thread_starts.clone(),
                ));
                for _ in 0..IO_WORKERS {
                    tokio::spawn(coordinate_io(
                        queue.clone(),
                        handler.clone(),
                        executor.clone(),
                    ));
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
        #[cfg(test)]
        let observed = self.handler.clock.request_frame(&frame);
        let mut job = Some(Job {
            sender,
            frame,
            queued: self.handler.clock.queued_since(priority, arrived),
        });
        loop {
            let changed = self.queue.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            #[cfg(test)]
            let mut admitted_at = None;
            if self.queue.admit(
                &mut job,
                #[cfg(test)]
                &mut admitted_at,
            ) {
                #[cfg(test)]
                if let Some(admitted_at) = admitted_at {
                    self.handler
                        .clock
                        .mark_observed(&observed, "admitted", admitted_at);
                }
                self.queue.notify_admitted();
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
        self.queue.ready.notify_waiters();
        self.queue.changed.notify_waiters();
    }
}

impl Queue {
    fn notify_admitted(&self) {
        self.ready.notify_one();
    }

    /// A full queue keeps exactly the carrier's current intake future parked.
    /// Nothing has been moved into a worker or another internal queue yet.
    fn admit(
        &self,
        incoming: &mut Option<Job>,
        #[cfg(test)] admitted_at: &mut Option<std::time::Instant>,
    ) -> bool {
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
        // Save the diagnostic timestamp before another coordinator can pop.
        // Emit its observer callback after releasing this queue mutex.
        #[cfg(test)]
        {
            *admitted_at = Some(std::time::Instant::now());
        }
        true
    }

    async fn work_available(&self) -> bool {
        loop {
            let changed = self.ready.notified();
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
        let (work, more_ready) = {
            let mut waiting = self.waiting.lock().unwrap();
            let work = waiting.take();
            (work, !waiting.ready.is_empty())
        };
        // Pass a single wakeup along the backlog. Notify's stored permit may
        // coalesce admissions while all workers are busy; the baton ensures
        // the available workers still fan out without waking every idle one.
        if more_ready {
            self.ready.notify_one();
        }
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
        #[cfg(test)]
        let worker_started = std::time::Instant::now();
        if handler.observations.is_none() {
            let handler = handler.clone();
            return Box::new(move || self.run_on(&handler, connection, ReadLane::Observation));
        }
        self.callers.retain(|(_, caller)| caller.read_is_live());
        let Some((_, sender)) = self.callers.last() else {
            return Box::new(|| {});
        };
        self.job.sender = sender.clone();
        #[cfg(test)]
        let observed = handler.clock.request_frame(&self.job.frame);
        #[cfg(test)]
        let clock = handler.clock.clone();
        #[cfg(test)]
        clock.mark_observed(&observed, "worker_start", worker_started);
        let timer = self
            .job
            .queued
            .start(self.job.frame.payload["method"].as_str().unwrap_or("other"));
        #[cfg(test)]
        let needs_metadata = crate::api::effects::observation_needs_sql(&self.job.frame.payload);
        #[cfg(test)]
        if needs_metadata {
            handler
                .clock
                .mark_observed(&observed, "metadata_start", std::time::Instant::now());
        }
        let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            handler.prepare_observation(self.job.sender, self.job.frame, connection)
        }));
        #[cfg(test)]
        if needs_metadata {
            handler
                .clock
                .mark_observed(&observed, "metadata_end", std::time::Instant::now());
        }
        Box::new(move || {
            #[cfg(test)]
            clock.mark_observed(
                &observed,
                "external_worker_start",
                std::time::Instant::now(),
            );
            if !self.callers.iter().any(|(_, caller)| caller.read_is_live()) {
                return;
            }
            let answer = prepared
                .ok()
                .and_then(|job| std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).ok())
                .unwrap_or_else(|| json!({"ok":false,"error":"handler failed"}));
            // Match the ordinary handler boundary. Reply cloning, encryption
            // and carrier enqueue are measured separately by the diagnostic
            // hooks and remain included in client-observed acceptance timing.
            drop(timer);
            #[cfg(test)]
            clock.mark_observed(&observed, "handler_complete", std::time::Instant::now());
            for (id, sender) in self.callers {
                if sender.read_is_live() {
                    let mut answer = answer.clone();
                    answer["id"] = id;
                    #[cfg(test)]
                    clock.mark_observed(&observed, "reply_start", std::time::Instant::now());
                    #[cfg(test)]
                    sender.push_observed(answer, &clock, &observed, true);
                    #[cfg(not(test))]
                    sender.push_read_reply(answer);
                    #[cfg(test)]
                    clock.mark_observed(&observed, "reply_enqueued", std::time::Instant::now());
                }
            }
        })
    }
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

async fn coordinate_io(queue: Arc<Queue>, handler: FrameHandler, executor: Arc<IoExecutors>) {
    while queue.work_available().await {
        #[cfg(test)]
        let queue_take_started = std::time::Instant::now();
        let Some(work) = queue.take() else { continue };
        #[cfg(test)]
        let observed = handler.clock.request_frame(&work.job.frame);
        #[cfg(test)]
        {
            handler
                .clock
                .mark_observed(&observed, "queue_take_start", queue_take_started);
            handler
                .clock
                .mark_observed(&observed, "lane_pop", std::time::Instant::now());
        }
        let Some(execution) = prepare_io(&queue, &handler, work).await else {
            continue;
        };
        #[cfg(test)]
        handler
            .clock
            .mark_observed(&observed, "io_submitted", std::time::Instant::now());
        let Ok(complete) = executor.submit(execution) else {
            return;
        };
        let _ = complete.await;
    }
}

async fn prepare_io(queue: &Queue, handler: &FrameHandler, work: Work) -> Option<Execution> {
    if !crate::api::effects::observation_needs_sql(&work.job.frame.payload) {
        let handler = handler.clone();
        return Some(Box::new(move || work.prepare_observation(&handler, None)()));
    }
    #[cfg(test)]
    handler.clock.mark_request(
        &work.job.frame,
        "sql_executor_init_start",
        std::time::Instant::now(),
    );
    let executors = handler.read_executors.get_or_init(|| {
        ReadExecutors::new(
            #[cfg(test)]
            handler.sql_thread_starts.clone(),
        )
    });
    #[cfg(test)]
    handler.clock.mark_request(
        &work.job.frame,
        "sql_executor_init_end",
        std::time::Instant::now(),
    );
    #[cfg(test)]
    handler.clock.mark_request(
        &work.job.frame,
        "sql_permit_start",
        std::time::Instant::now(),
    );
    let permit = while_live(queue, Some(&work), executors.acquire()).await?;
    #[cfg(test)]
    handler
        .clock
        .mark_request(&work.job.frame, "sql_permit_end", std::time::Instant::now());
    let pool = handler
        .observations
        .as_ref()
        .and_then(|observations| observations.pool.as_ref());
    #[cfg(test)]
    handler.clock.mark_request(
        &work.job.frame,
        "sql_borrow_start",
        std::time::Instant::now(),
    );
    let connection = match pool {
        Some(pool) => Some(while_live(queue, Some(&work), pool.acquire()).await?),
        None => None,
    };
    #[cfg(test)]
    {
        handler
            .clock
            .mark_request(&work.job.frame, "sql_borrow_end", std::time::Instant::now());
        handler
            .clock
            .mark_request(&work.job.frame, "sql_submitted", std::time::Instant::now());
    }
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
                let executors = handler.read_executors.get_or_init(|| {
                    ReadExecutors::new(
                        #[cfg(test)]
                        handler.sql_thread_starts.clone(),
                    )
                });
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

#[cfg(test)]
mod queue_wakeup_tests {
    use super::*;
    use std::future::Future;
    use std::task::Poll;

    #[tokio::test]
    async fn one_queued_read_wakes_one_idle_execution_coordinator() {
        let queue = Queue::default();
        let mut waiters = (0..IO_WORKERS)
            .map(|_| Box::pin(queue.work_available()))
            .collect::<Vec<_>>();
        for waiter in &mut waiters {
            let pending =
                std::future::poll_fn(|cx| Poll::Ready(waiter.as_mut().poll(cx).is_pending())).await;
            assert!(pending, "every coordinator is idle before admission");
        }
        let frame: Frame = serde_json::from_value(json!({"session_id":"s","message_id":"m","frame_type":"data","sender":"client","created_at":"","payload":{"id":1,"method":"fs.list"}})).unwrap();
        let mut job = Some(Job {
            sender: SessionSender::detached("s"),
            frame,
            queued: crate::timing::FrameClock::new().queued(),
        });
        let mut admitted_at = None;
        assert!(queue.admit(&mut job, &mut admitted_at));
        queue.notify_admitted();
        let mut awakened = 0;
        for waiter in &mut waiters {
            awakened += usize::from(
                std::future::poll_fn(|cx| Poll::Ready(waiter.as_mut().poll(cx).is_ready())).await,
            );
        }
        assert_eq!(
            awakened, 1,
            "one queued read must not wake all idle execution coordinators"
        );
    }
}
