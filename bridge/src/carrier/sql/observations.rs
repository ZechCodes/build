//! One bounded observation queue. Metadata preparation retains admission slots;
//! physical I/O admission releases them only after SQL resources are gone.
use super::*;

#[derive(Clone, Copy)]
enum Stage {
    Metadata,
    Io,
}

type Token = Arc<()>;

struct Reservation {
    token: Token,
    callers: Vec<SessionSender>,
    #[cfg(test)]
    observed: Option<Frame>,
}

impl Reservation {
    fn from_work(work: &Work, handler: &FrameHandler) -> Self {
        #[cfg(not(test))]
        let _ = handler;
        Self {
            token: Arc::new(()),
            callers: work
                .callers
                .iter()
                .map(|(_, sender)| sender.clone())
                .collect(),
            #[cfg(test)]
            observed: handler.clock.request_frame(&work.job.frame),
        }
    }
}

enum Entry {
    Pending(Box<Job>),
    Preparing(Reservation),
    Ready(Reservation, Execution),
}

struct IoWork {
    execution: Execution,
    #[cfg(test)]
    observed: Option<Frame>,
}

impl Entry {
    fn eligible(&self, stage: Stage) -> bool {
        match self {
            Self::Pending(job) => {
                crate::api::effects::observation_needs_sql(&job.frame.payload)
                    == matches!(stage, Stage::Metadata)
            }
            Self::Preparing(_) => false,
            Self::Ready(_, _) => matches!(stage, Stage::Io),
        }
    }

    fn weight(&self) -> usize {
        match self {
            Self::Pending(_) => 1,
            Self::Preparing(reservation) | Self::Ready(reservation, _) => reservation.callers.len(),
        }
    }

    fn live(&mut self) -> bool {
        match self {
            Self::Pending(job) => job.sender.read_is_live(),
            Self::Preparing(reservation) | Self::Ready(reservation, _) => {
                reservation.callers.retain(SessionSender::read_is_live);
                !reservation.callers.is_empty()
            }
        }
    }

    fn token_matches(&self, token: &Token) -> bool {
        match self {
            Self::Pending(_) => false,
            Self::Preparing(reservation) | Self::Ready(reservation, _) => {
                Arc::ptr_eq(&reservation.token, token)
            }
        }
    }
}

#[derive(Default)]
struct Waiting {
    openings: HashMap<u64, VecDeque<Entry>>,
    metadata: VecDeque<u64>,
    io: VecDeque<u64>,
    closed: bool,
}

impl Waiting {
    fn ring(&self, stage: Stage) -> &VecDeque<u64> {
        match stage {
            Stage::Metadata => &self.metadata,
            Stage::Io => &self.io,
        }
    }

    fn ring_mut(&mut self, stage: Stage) -> &mut VecDeque<u64> {
        match stage {
            Stage::Metadata => &mut self.metadata,
            Stage::Io => &mut self.io,
        }
    }

    fn refresh(&mut self, opening: u64) {
        let metadata = self
            .openings
            .get(&opening)
            .is_some_and(|entries| entries.iter().any(|entry| entry.eligible(Stage::Metadata)));
        let io = self
            .openings
            .get(&opening)
            .is_some_and(|entries| entries.iter().any(|entry| entry.eligible(Stage::Io)));
        self.refresh_ring(opening, Stage::Metadata, metadata);
        self.refresh_ring(opening, Stage::Io, io);
    }

    fn refresh_ring(&mut self, opening: u64, stage: Stage, eligible: bool) {
        let ring = self.ring_mut(stage);
        if !eligible {
            ring.retain(|id| *id != opening);
        } else if !ring.contains(&opening) {
            ring.push_back(opening);
        }
    }

    fn prune(&mut self) {
        self.openings.retain(|_, entries| {
            entries.retain_mut(Entry::live);
            !entries.is_empty()
        });
        self.metadata.retain(|id| {
            self.openings
                .get(id)
                .is_some_and(|entries| entries.iter().any(|entry| entry.eligible(Stage::Metadata)))
        });
        self.io.retain(|id| {
            self.openings
                .get(id)
                .is_some_and(|entries| entries.iter().any(|entry| entry.eligible(Stage::Io)))
        });
    }

    fn select(&mut self, stage: Stage) -> Option<(u64, usize)> {
        self.prune();
        let opening = self.ring_mut(stage).pop_front()?;
        let entries = self.openings.get(&opening)?;
        let index = entries.iter().position(|entry| entry.eligible(stage))?;
        Some((opening, index))
    }

    fn take_pending(&mut self, opening: u64, index: usize) -> Work {
        let entries = self.openings.get_mut(&opening).unwrap();
        let Entry::Pending(job) = entries.remove(index).unwrap() else {
            unreachable!()
        };
        let mut work = Work::new(*job);
        let mut remaining = VecDeque::new();
        for entry in entries.drain(..) {
            match entry {
                Entry::Pending(job) if work.matches(&job) => work.join(*job),
                entry => remaining.push_back(entry),
            }
        }
        *entries = remaining;
        work
    }

    fn finish_take(&mut self, opening: u64) {
        if self.openings.get(&opening).is_some_and(VecDeque::is_empty) {
            self.openings.remove(&opening);
        }
        self.refresh(opening);
    }

    fn reservation_index(&self, opening: u64, token: &Token) -> Option<usize> {
        self.openings
            .get(&opening)?
            .iter()
            .position(|entry| entry.token_matches(token))
    }
}

#[derive(Default)]
pub(super) struct ObservationQueue {
    waiting: Mutex<Waiting>,
    metadata: Notify,
    io: Notify,
    changed: Notify,
    // Only lifecycle/pending-metadata changes wake resource waiters. Pure I/O
    // admission or completion must not broadcast to eight SQL waiters.
    lifecycle: Notify,
}

impl ObservationQueue {
    pub(super) fn start(handler: FrameHandler) -> Arc<Self> {
        let queue = Arc::new(Self::default());
        let executor = Arc::new(DedicatedExecutors::new(
            "bridge-io",
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
        for _ in 0..SQL_WORKERS {
            tokio::spawn(coordinate_metadata(queue.clone(), handler.clone()));
        }
        queue
    }

    pub(super) async fn dispatch(
        &self,
        handler: &FrameHandler,
        sender: SessionSender,
        frame: Frame,
        arrived: WaitTimer,
    ) {
        let priority = if frame.payload["priority"] == "background" {
            Priority::Background
        } else {
            Priority::Foreground
        };
        let stage = if crate::api::effects::observation_needs_sql(&frame.payload) {
            Stage::Metadata
        } else {
            Stage::Io
        };
        #[cfg(test)]
        let observed = handler.clock.request_frame(&frame);
        let mut job = Some(Job {
            sender,
            frame,
            queued: handler.clock.queued_since(priority, arrived),
        });
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            #[cfg(test)]
            let mut admitted = None;
            if self.admit(
                &mut job,
                #[cfg(test)]
                &mut admitted,
            ) {
                #[cfg(test)]
                if let Some(at) = admitted {
                    handler.clock.mark_observed(&observed, "admitted", at);
                }
                self.notify(stage).notify_one();
                return;
            }
            changed.await;
        }
    }

    fn admit(
        &self,
        job: &mut Option<Job>,
        #[cfg(test)] admitted: &mut Option<std::time::Instant>,
    ) -> bool {
        let mut waiting = self.waiting.lock().unwrap();
        let incoming = job.as_ref().unwrap();
        if waiting.closed || !incoming.sender.read_is_live() {
            return true;
        }
        let opening = incoming.sender.still_open.ordinal;
        let entries = waiting.openings.entry(opening).or_default();
        if entries.iter().map(Entry::weight).sum::<usize>() >= SQL_BACKLOG {
            return false;
        }
        entries.push_back(Entry::Pending(Box::new(job.take().unwrap())));
        waiting.refresh(opening);
        #[cfg(test)]
        {
            *admitted = Some(std::time::Instant::now());
        }
        true
    }

    fn notify(&self, stage: Stage) -> &Notify {
        match stage {
            Stage::Metadata => &self.metadata,
            Stage::Io => &self.io,
        }
    }

    async fn available(&self, stage: Stage) -> bool {
        loop {
            let ready = self.notify(stage).notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            {
                let mut waiting = self.waiting.lock().unwrap();
                waiting.prune();
                if waiting.closed {
                    return false;
                }
                if !waiting.ring(stage).is_empty() {
                    return true;
                }
            }
            ready.await;
        }
    }

    fn take_metadata(self: &Arc<Self>, handler: &FrameHandler) -> Option<(Work, Preparing)> {
        let (work, preparing, metadata_empty) = {
            let mut waiting = self.waiting.lock().unwrap();
            let (opening, index) = waiting.select(Stage::Metadata)?;
            let work = waiting.take_pending(opening, index);
            let reservation = Reservation::from_work(&work, handler);
            let token = reservation.token.clone();
            waiting
                .openings
                .get_mut(&opening)
                .unwrap()
                .insert(index, Entry::Preparing(reservation));
            waiting.refresh(opening);
            (
                work,
                Preparing {
                    queue: self.clone(),
                    opening,
                    token,
                    active: true,
                },
                waiting.metadata.is_empty(),
            )
        };
        self.metadata.notify_one();
        if metadata_empty {
            self.lifecycle.notify_waiters();
        }
        Some((work, preparing))
    }

    fn take_io(&self, handler: &FrameHandler) -> Option<Execution> {
        #[cfg(test)]
        let take_started = std::time::Instant::now();
        let io_work = {
            let mut waiting = self.waiting.lock().unwrap();
            let (opening, index) = waiting.select(Stage::Io)?;
            let pending = matches!(
                waiting.openings[&opening].get(index),
                Some(Entry::Pending(_))
            );
            let io_work = if pending {
                let work = waiting.take_pending(opening, index);
                #[cfg(test)]
                let observed = handler.clock.request_frame(&work.job.frame);
                let handler = handler.clone();
                let execution: Execution =
                    Box::new(move || work.prepare_observation(&handler, None)());
                IoWork {
                    execution,
                    #[cfg(test)]
                    observed,
                }
            } else {
                let Entry::Ready(reservation, execution) = waiting
                    .openings
                    .get_mut(&opening)
                    .unwrap()
                    .remove(index)
                    .unwrap()
                else {
                    unreachable!()
                };
                #[cfg(test)]
                let observed = reservation.observed;
                #[cfg(not(test))]
                let _ = reservation;
                IoWork {
                    execution,
                    #[cfg(test)]
                    observed,
                }
            };
            waiting.finish_take(opening);
            io_work
        };
        #[cfg(test)]
        {
            handler
                .clock
                .mark_observed(&io_work.observed, "queue_take_start", take_started);
            handler
                .clock
                .mark_observed(&io_work.observed, "lane_pop", std::time::Instant::now());
            handler.clock.mark_observed(
                &io_work.observed,
                "io_submitted",
                std::time::Instant::now(),
            );
        }
        self.io.notify_one();
        self.changed.notify_waiters();
        Some(io_work.execution)
    }

    fn complete(&self, opening: u64, token: &Token, execution: Execution) {
        let mut waiting = self.waiting.lock().unwrap();
        waiting.prune();
        if waiting.closed {
            return;
        }
        let Some(index) = waiting.reservation_index(opening, token) else {
            return;
        };
        let entries = waiting.openings.get_mut(&opening).unwrap();
        if !matches!(entries.get(index), Some(Entry::Preparing(_))) {
            return;
        }
        let Entry::Preparing(reservation) = entries.remove(index).unwrap() else {
            unreachable!()
        };
        entries.insert(index, Entry::Ready(reservation, execution));
        waiting.refresh(opening);
        drop(waiting);
        self.io.notify_one();
    }

    fn cancel(&self, opening: u64, token: &Token) {
        let mut waiting = self.waiting.lock().unwrap();
        if let Some(index) = waiting.reservation_index(opening, token) {
            waiting.openings.get_mut(&opening).unwrap().remove(index);
            waiting.finish_take(opening);
        }
        drop(waiting);
        self.changed.notify_waiters();
    }

    fn preparing_is_live(&self, opening: u64, token: &Token) -> bool {
        let waiting = self.waiting.lock().unwrap();
        !waiting.closed
            && waiting.openings.get(&opening).is_some_and(|entries| {
                entries.iter().any(|entry| {
                    entry.token_matches(token)
                        && match entry {
                            Entry::Preparing(reservation) => {
                                reservation.callers.iter().any(SessionSender::read_is_live)
                            }
                            _ => false,
                        }
                })
            })
    }

    pub(super) fn prune(&self) {
        self.waiting.lock().unwrap().prune();
        self.changed.notify_waiters();
        self.lifecycle.notify_waiters();
    }

    pub(super) fn close(&self) {
        let mut waiting = self.waiting.lock().unwrap();
        waiting.closed = true;
        waiting.openings.clear();
        waiting.metadata.clear();
        waiting.io.clear();
        drop(waiting);
        self.metadata.notify_waiters();
        self.io.notify_waiters();
        self.changed.notify_waiters();
        self.lifecycle.notify_waiters();
    }

    #[cfg(test)]
    pub(super) fn depths(&self) -> (usize, usize) {
        let waiting = self.waiting.lock().unwrap();
        (
            waiting.openings.values().flatten().map(Entry::weight).sum(),
            waiting.openings.len(),
        )
    }

    async fn while_pending<T>(&self, future: impl std::future::Future<Output = T>) -> Option<T> {
        tokio::pin!(future);
        loop {
            let changed = self.lifecycle.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut waiting = self.waiting.lock().unwrap();
                waiting.prune();
                if waiting.closed || waiting.metadata.is_empty() {
                    return None;
                }
            }
            tokio::select! { result = &mut future => return Some(result), _ = changed => {} }
        }
    }
}

struct Preparing {
    queue: Arc<ObservationQueue>,
    opening: u64,
    token: Token,
    active: bool,
}

impl Preparing {
    async fn receive(&self, future: oneshot::Receiver<Execution>) -> Option<Execution> {
        tokio::pin!(future);
        loop {
            let changed = self.queue.lifecycle.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if !self.queue.preparing_is_live(self.opening, &self.token) {
                return None;
            }
            tokio::select! { result = &mut future => return result.ok(), _ = changed => {} }
        }
    }

    fn complete(mut self, execution: Execution) {
        self.queue.complete(self.opening, &self.token, execution);
        self.active = false;
    }
}

impl Drop for Preparing {
    fn drop(&mut self) {
        if self.active {
            self.queue.cancel(self.opening, &self.token);
        }
    }
}

async fn coordinate_metadata(queue: Arc<ObservationQueue>, handler: FrameHandler) {
    while queue.available(Stage::Metadata).await {
        let executors = ReadExecutors::ready_for_handler(&handler);
        let Some(permit) = queue.while_pending(executors.acquire()).await else {
            continue;
        };
        let connection = match handler
            .observations
            .as_ref()
            .and_then(|observations| observations.pool.as_ref())
        {
            Some(pool) => match queue.while_pending(pool.acquire()).await {
                Some(connection) => Some(connection),
                None => continue,
            },
            None => None,
        };
        let Some((work, preparing)) = queue.take_metadata(&handler) else {
            continue;
        };
        #[cfg(test)]
        handler
            .clock
            .mark_request(&work.job.frame, "sql_submitted", std::time::Instant::now());
        let handler = handler.clone();
        let complete = executors.submit(permit, move || {
            work.prepare_observation(&handler, connection)
        });
        if let Some(execution) = preparing.receive(complete).await {
            preparing.complete(execution);
        }
    }
}

async fn coordinate_io(
    queue: Arc<ObservationQueue>,
    handler: FrameHandler,
    executor: Arc<DedicatedExecutors>,
) {
    while queue.available(Stage::Io).await {
        let Some(execution) = queue.take_io(&handler) else {
            continue;
        };
        let Ok(complete) = executor.submit(execution) else {
            return;
        };
        let _ = complete.await;
    }
}

#[cfg(test)]
mod tests;
