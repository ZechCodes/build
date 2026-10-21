//! Independent std-thread observation over immutable schedules and atomics.
use super::*;
use std::sync::atomic::Ordering;

const POLL: Duration = Duration::from_millis(10);
const MAILBOX_BATCH: usize = 256;

impl ClientWatch {
    /// One independent std thread; call and retain its guard before starting
    /// runtime callers. Initial schedule indexing is acknowledged synchronously.
    /// New registrations are supported but normal qualification freezes first.
    pub fn watch(&self) -> ClientWatchGuard {
        let (initial, receiver) = {
            let mut registry = self.inner.registry.lock().unwrap();
            let receiver = registry.receiver.take().expect("one watchdog per trial");
            let initial: Vec<_> = registry
                .rows
                .values()
                .map(|row| Arc::clone(&row.deadline))
                .collect();
            (initial, receiver)
        };
        let state = Arc::new(WatchState {
            stop: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            polls: AtomicU64::new(0),
            inspected_deadlines: AtomicU64::new(0),
            indexed: AtomicU64::new(0),
        });
        let (breaches_tx, breaches_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
        let observer = WatchLoop {
            index: DeadlineIndex::default(),
            receiver,
            state: Arc::clone(&state),
            clock: Arc::clone(&self.inner.clock),
            relative_base: Arc::clone(&self.inner.relative_base),
            epoch: self.inner.epoch,
            breaches: breaches_tx,
        };
        let handle = thread::Builder::new()
            .name("qualification-client-watch".into())
            .spawn(move || observer.run(initial, ready_tx))
            .expect("spawn independent client watchdog");
        ready_rx
            .recv()
            .expect("watchdog indexed schedule before caller wake");
        *self.inner.watchdog_thread.lock().unwrap() = Some(handle.thread().clone());
        self.inner.ready.store(true, Ordering::Release);
        ClientWatchGuard {
            state,
            handle: Some(handle),
            breaches: breaches_rx,
        }
    }
}

/// Owns only immutable schedule/atomic rows, its mailbox and deadline index.
struct WatchLoop {
    index: DeadlineIndex,
    receiver: mpsc::Receiver<Event>,
    state: Arc<WatchState>,
    clock: Clock,
    relative_base: Arc<OnceLock<Instant>>,
    epoch: Instant,
    breaches: mpsc::Sender<Breach>,
}

impl WatchLoop {
    fn run(mut self, initial: Vec<Arc<Deadline>>, ready: mpsc::SyncSender<()>) {
        for row in initial {
            self.ingest(row);
        }
        self.publish_index_count();
        ready.send(()).expect("watchdog startup receiver alive");
        loop {
            let consumed = self.drain_mailbox();
            self.observe_expired();
            self.publish_index_count();
            self.state.polls.fetch_add(1, Ordering::Release);
            // Callers/producers are joined before stop. Drain finite queued
            // events before exit, retaining the original full-batch behavior.
            if self.state.stop.load(Ordering::Acquire) && consumed < MAILBOX_BATCH {
                break;
            }
            if consumed == MAILBOX_BATCH {
                continue;
            }
            thread::park_timeout(POLL);
        }
    }

    fn drain_mailbox(&mut self) -> usize {
        // Bounded mailbox work prevents continuous producers from starving
        // deadline checks. No archive/future scan occurs.
        let mut consumed = 0;
        while consumed < MAILBOX_BATCH {
            match self.receiver.try_recv() {
                Ok(event) => self.observe_event(event),
                Err(mpsc::TryRecvError::Empty | mpsc::TryRecvError::Disconnected) => break,
            }
            consumed += 1;
        }
        consumed
    }

    fn observe_event(&mut self, event: Event) {
        match event {
            Event::Registered(row) => self.ingest(row),
            Event::Settled(row) => {
                self.index.remove(&row);
                publish_breach(
                    &row,
                    self.epoch,
                    (self.clock)(),
                    &self.state,
                    &self.breaches,
                );
            }
        }
    }

    fn ingest(&mut self, row: Arc<Deadline>) {
        ingest_row(
            &mut self.index,
            row,
            self.epoch,
            (self.clock)(),
            &self.state,
            &self.breaches,
        );
    }

    fn observe_expired(&mut self) {
        let now = (self.clock)();
        while let Some(row) = self
            .index
            .pop_expired(now, self.relative_base.get().copied())
        {
            self.state
                .inspected_deadlines
                .fetch_add(1, Ordering::Relaxed);
            publish_breach(&row, self.epoch, now, &self.state, &self.breaches);
        }
    }

    fn publish_index_count(&self) {
        self.state
            .indexed
            .store(self.index.len() as u64, Ordering::Release);
    }
}

#[derive(Default)]
struct DeadlineIndex {
    absolute: BTreeMap<(Instant, String), Arc<Deadline>>,
    relative: BTreeMap<(Duration, String), Arc<Deadline>>,
}

impl DeadlineIndex {
    fn len(&self) -> usize {
        self.absolute.len() + self.relative.len()
    }

    fn insert(&mut self, row: Arc<Deadline>) {
        match row.schedule.deadline_key() {
            DeadlineKey::Absolute(at) => {
                self.absolute.insert((at, row.original_id.clone()), row);
            }
            DeadlineKey::Relative(offset) => {
                self.relative.insert((offset, row.original_id.clone()), row);
            }
        }
    }

    fn remove(&mut self, row: &Deadline) {
        match row.schedule.deadline_key() {
            DeadlineKey::Absolute(at) => {
                self.absolute.remove(&(at, row.original_id.clone()));
            }
            DeadlineKey::Relative(offset) => {
                self.relative.remove(&(offset, row.original_id.clone()));
            }
        }
    }

    fn pop_expired(
        &mut self,
        now: Instant,
        relative_base: Option<Instant>,
    ) -> Option<Arc<Deadline>> {
        if self
            .absolute
            .first_key_value()
            .is_some_and(|((at, _), _)| *at < now)
        {
            return self.absolute.pop_first().map(|(_, row)| row);
        }
        if let Some(base) = relative_base {
            let elapsed = now.saturating_duration_since(base);
            if self
                .relative
                .first_key_value()
                .is_some_and(|((offset, _), _)| *offset < elapsed)
            {
                return self.relative.pop_first().map(|(_, row)| row);
            }
        }
        None
    }
}

fn ingest_row(
    index: &mut DeadlineIndex,
    row: Arc<Deadline>,
    epoch: Instant,
    now: Instant,
    state: &WatchState,
    breaches: &mpsc::Sender<Breach>,
) {
    if row.settled_ns.load(Ordering::Acquire) == 0 {
        index.insert(row);
    } else {
        publish_breach(&row, epoch, now, state, breaches);
    }
}

fn publish_breach(
    row: &Deadline,
    epoch: Instant,
    now: Instant,
    state: &WatchState,
    breaches: &mpsc::Sender<Breach>,
) {
    let settled = row.settled_ns.load(Ordering::Acquire);
    let Some(intended_at) = row.schedule.intended_at() else {
        return; // Relative schedule is fully indexed but T0 is not released.
    };
    let observed_end = if settled == 0 {
        now
    } else {
        epoch
            .checked_add(Duration::from_nanos(settled - 1))
            .expect("stored terminal instant")
    };
    if row.breached.load(Ordering::Acquire)
        || observed_end.saturating_duration_since(intended_at) > LIMIT
    {
        row.breached.store(true, Ordering::Release);
        state.failed.store(true, Ordering::Release);
        // Events are retained for evidence. Per-ID breached is sticky even if
        // the caller is cancelled or completes after this observation.
        if !row.notified.swap(true, Ordering::AcqRel) {
            let _ = breaches.send(Breach {
                original_id: row.original_id.clone(),
                cohort: row.cohort,
                intended_at,
                observed_at: now,
                settled_at: (settled != 0).then_some(observed_end),
            });
        }
    }
}

impl ClientWatchGuard {
    pub fn observer(&self) -> Arc<ClientWatchObservation> {
        Arc::new(ClientWatchObservation {
            state: Arc::clone(&self.state),
        })
    }

    pub fn failed(&self) -> bool {
        self.state.failed.load(Ordering::Acquire)
            || self.handle.as_ref().is_some_and(JoinHandle::is_finished)
    }

    pub fn polls(&self) -> u64 {
        self.state.polls.load(Ordering::Acquire)
    }

    /// Retain all per-ID rows separately via ClientWatch::snapshot(). Join all
    /// callers first, finish this guard second, archive snapshot last. An early
    /// failure stops new offers; it does not convert accepted writes to success.
    pub fn finish(mut self) -> WatchReport {
        let panicked = self.stop_and_join();
        WatchReport {
            failed: panicked || self.state.failed.load(Ordering::Acquire),
            panicked,
            polls: self.state.polls.load(Ordering::Acquire),
            inspected_deadlines: self.state.inspected_deadlines.load(Ordering::Acquire),
            still_indexed: self.state.indexed.load(Ordering::Acquire),
            breaches: self.breaches.try_iter().collect(),
        }
    }

    fn stop_and_join(&mut self) -> bool {
        self.state.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            handle.thread().unpark();
            handle.join().is_err()
        } else {
            false
        }
    }
}

impl Drop for ClientWatchGuard {
    fn drop(&mut self) {
        // Normal qualification must call finish() and assert its report. Drop
        // is only panic/cleanup containment; it makes no qualification claim.
        self.stop_and_join();
    }
}

impl ClientWatchObservation {
    pub fn failed(&self) -> bool {
        self.state.failed.load(Ordering::Acquire)
    }

    pub fn polls(&self) -> u64 {
        self.state.polls.load(Ordering::Acquire)
    }

    /// Includes future scheduled originals; never use this as receiver drain.
    pub fn indexed(&self) -> u64 {
        self.state.indexed.load(Ordering::Acquire)
    }
}
