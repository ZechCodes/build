//! Schedule enrollment and actual caller lifecycle observations.
use super::*;
use std::sync::atomic::Ordering;

impl ClientWatch {
    pub fn new(clock: Clock) -> Self {
        Self::with_maximum_originals(clock, DEFAULT_MAX_ORIGINALS)
    }

    pub fn with_maximum_originals(clock: Clock, maximum_originals: usize) -> Self {
        assert!(maximum_originals > 0, "declare a nonzero schedule bound");
        let epoch = clock();
        let (sender, receiver) = mpsc::channel();
        Self {
            inner: Arc::new(Inner {
                clock,
                epoch,
                maximum_originals,
                registry: Mutex::new(Registry {
                    rows: BTreeMap::new(),
                    receiver: Some(receiver),
                    latest_registration: epoch,
                    maximum_relative_offset: Duration::ZERO,
                }),
                sender,
                watchdog_thread: Mutex::new(None),
                relative_base: Arc::new(OnceLock::new()),
                ready: AtomicBool::new(false),
                unmatched_replies: Mutex::new(Vec::new()),
                decoded_observer: OnceLock::new(),
            }),
        }
    }

    /// Default cohort is a semantic write, preserving the original test API.
    pub fn register_at(&self, original_id: &str, intended_at: Instant) {
        self.register_cohort_at(original_id, intended_at, TrafficCohort::SemanticWrite);
    }

    /// Identity-only lookup; safe before relative T0, with no archive scan.
    pub fn contains(&self, original_id: &str) -> bool {
        self.inner
            .registry
            .lock()
            .unwrap()
            .rows
            .contains_key(original_id)
    }

    pub fn schedule_lookup(&self, original_id: &str) -> Option<ScheduledIdentity> {
        let registry = self.inner.registry.lock().unwrap();
        registry
            .rows
            .get(original_id)
            .map(|evidence| ScheduledIdentity {
                cohort: evidence.deadline.cohort,
                intended_at: evidence.deadline.schedule.intended_at(),
            })
    }

    pub fn register_cohort_at(
        &self,
        original_id: &str,
        intended_at: Instant,
        cohort: TrafficCohort,
    ) {
        self.register(original_id, Schedule::Absolute(intended_at), cohort, false);
    }

    /// Allocate all 149k identities and immutable offsets during setup. No
    /// absolute trial timestamp is chosen until watch() acknowledges indexing.
    pub fn register_relative_at(&self, original_id: &str, offset: Duration, cohort: TrafficCohort) {
        self.register(
            original_id,
            Schedule::Relative {
                offset,
                base: Arc::clone(&self.inner.relative_base),
            },
            cohort,
            false,
        );
    }

    /// O(1) release of the already indexed schedule. Call immediately with a
    /// fresh Instant::now(), then release callers. It never moves a deadline.
    pub fn start_at(&self, base: Instant) {
        assert!(
            self.inner.ready.load(Ordering::Acquire),
            "watchdog acknowledges setup before T0"
        );
        let registry = self.inner.registry.lock().unwrap();
        assert!(
            base >= registry.latest_registration,
            "T0 follows full schedule registration"
        );
        base.checked_add(registry.maximum_relative_offset)
            .and_then(|at| at.checked_add(LIMIT))
            .expect("entire relative schedule is representable");
        self.inner
            .relative_base
            .set(base)
            .expect("one immutable trial T0");
        drop(registry);
        self.unpark();
    }

    fn register(
        &self,
        original_id: &str,
        schedule: Schedule,
        cohort: TrafficCohort,
        implicit_schedule: bool,
    ) {
        assert!(!original_id.is_empty(), "original identity is nonempty");
        let registered_at = (self.inner.clock)();
        let deadline = Arc::new(Deadline {
            original_id: original_id.into(),
            cohort,
            schedule: schedule.clone(),
            settled_ns: AtomicU64::new(0),
            breached: AtomicBool::new(false),
            notified: AtomicBool::new(false),
        });
        let evidence = Arc::new(Evidence {
            row: Mutex::new(Lifecycle {
                registered_at,
                implicit_schedule,
                offered_at: None,
                sent_at: None,
                server_registered_at: None,
                admitted_at: None,
                final_at: None,
                final_outcome: None,
                cancelled_at: None,
                finished_at: None,
            }),
            deadline: Arc::clone(&deadline),
        });
        let mut registry = self.inner.registry.lock().unwrap();
        assert!(
            !registry.rows.contains_key(original_id),
            "original schedule identity is unique"
        );
        assert!(
            registry.rows.len() < self.inner.maximum_originals,
            "declared schedule bound exceeded"
        );
        if let Schedule::Relative { offset, base } = &schedule {
            assert!(
                base.get().is_none(),
                "relative schedule is frozen before trial T0"
            );
            assert!(
                registry.receiver.is_some(),
                "register entire relative schedule before watch setup barrier"
            );
            registry.maximum_relative_offset = registry.maximum_relative_offset.max(*offset);
        }
        registry.latest_registration = registry.latest_registration.max(registered_at);
        registry.rows.insert(original_id.into(), evidence);
        // Send while serializing registrations so startup sees either an initial
        // immutable row or its queued registration. It cannot miss both.
        if registry.receiver.is_none() {
            self.inner
                .sender
                .send(Event::Registered(deadline))
                .expect("watchdog mailbox alive");
        }
        drop(registry);
        self.unpark();
    }

    fn evidence(&self, original_id: &str) -> Arc<Evidence> {
        Arc::clone(
            self.inner
                .registry
                .lock()
                .unwrap()
                .rows
                .get(original_id)
                .expect("original must already be scheduled"),
        )
    }

    pub fn offer(&self, original_id: &str) {
        // Retain the three old offer-only controls. Such rows are explicitly
        // rejected by final transport validation in normal trials.
        let exists = self
            .inner
            .registry
            .lock()
            .unwrap()
            .rows
            .contains_key(original_id);
        if !exists {
            self.register(
                original_id,
                Schedule::Absolute((self.inner.clock)()),
                TrafficCohort::SemanticWrite,
                true,
            );
        }
        let evidence = self.evidence(original_id);
        assert!(
            evidence.deadline.schedule.intended_at().is_some(),
            "T0 published before caller offer"
        );
        let mut row = evidence.row.lock().unwrap();
        assert!(row.finished_at.is_none(), "cannot offer a settled original");
        assert!(row.offered_at.is_none(), "original is offered once");
        row.offered_at = Some((self.inner.clock)());
    }

    pub fn sent(&self, original_id: &str) {
        let evidence = self.evidence(original_id);
        let mut row = evidence.row.lock().unwrap();
        assert!(row.offered_at.is_some(), "send follows actual offer");
        assert!(row.finished_at.is_none(), "cannot send a settled original");
        assert!(row.sent_at.is_none(), "original is sent once");
        row.sent_at = Some((self.inner.clock)());
    }

    /// Call only from an actual server-origin receipt correlated with this ID.
    pub fn server_registered(&self, original_id: &str) {
        self.server_registered_at(original_id, (self.inner.clock)());
    }

    /// Archive the actual registry boundary timestamp, not the later time at
    /// which the caller collected its correlated receipt.
    pub fn server_registered_at(&self, original_id: &str, observed_actual: Instant) {
        let evidence = self.evidence(original_id);
        let mut row = evidence.row.lock().unwrap();
        let sent_at = row.sent_at.expect("server receipt follows actual send");
        assert!(
            observed_actual >= sent_at,
            "server origin cannot precede actual send"
        );
        assert!(
            observed_actual <= (self.inner.clock)(),
            "actual server origin cannot be in the future"
        );
        if let Some(final_at) = row.final_at {
            assert!(
                observed_actual <= final_at,
                "server origin precedes final reply even when archived later"
            );
        }
        assert!(
            row.server_registered_at.is_none(),
            "one server origin per original"
        );
        row.server_registered_at = Some(observed_actual);
    }

    /// Separate actual admission evidence; a socket send is not admission.
    pub fn admitted(&self, original_id: &str) {
        self.admitted_at(original_id, (self.inner.clock)());
    }

    /// Archive the actual correlated admission timestamp from the registry.
    pub fn admitted_at(&self, original_id: &str, observed_actual: Instant) {
        let evidence = self.evidence(original_id);
        let mut row = evidence.row.lock().unwrap();
        let origin_at = row
            .server_registered_at
            .expect("admission has a real origin receipt");
        assert!(
            observed_actual >= origin_at,
            "actual admission cannot precede server origin"
        );
        assert!(
            observed_actual <= (self.inner.clock)(),
            "actual admission cannot be in the future"
        );
        if let Some(final_at) = row.final_at {
            assert!(
                observed_actual <= final_at,
                "actual admission precedes final reply even when archived later"
            );
        }
        assert!(row.admitted_at.is_none(), "original is admitted once");
        row.admitted_at = Some(observed_actual);
    }

    /// Attach once before dispatch. Notification is only the actual validated
    /// decoded reply boundary, never a transport error, cancellation or proof.
    pub fn observe_decoded_replies(&self, observer: DecodedObserver) -> Result<(), String> {
        self.inner
            .decoded_observer
            .set(observer)
            .map_err(|_| "decoded reply observer already installed".into())
    }

    pub fn decoded_final_at(&self, original_id: &str, outcome: FinalOutcome, actual_at: Instant) {
        assert!(matches!(
            outcome,
            FinalOutcome::Reply | FinalOutcome::ProtocolError
        ));
        self.complete_with_outcome(original_id, outcome);
        self.notify_decoded(original_id, actual_at);
    }

    fn notify_decoded(&self, original_id: &str, actual_at: Instant) {
        if let Some(observer) = self.inner.decoded_observer.get() {
            observer(original_id, actual_at);
        }
    }

    pub fn complete(&self, original_id: &str) {
        self.complete_with_outcome(original_id, FinalOutcome::Reply);
    }

    pub fn complete_with_outcome(&self, original_id: &str, outcome: FinalOutcome) {
        self.settle(original_id, Some(outcome));
    }

    pub fn cancel(&self, original_id: &str) {
        self.settle(original_id, None);
    }

    fn settle(&self, original_id: &str, outcome: Option<FinalOutcome>) {
        let evidence = self.evidence(original_id);
        let mut row = evidence.row.lock().unwrap();
        assert!(row.finished_at.is_none(), "original settles once");
        if outcome == Some(FinalOutcome::Reply) || outcome == Some(FinalOutcome::ProtocolError) {
            assert!(row.sent_at.is_some(), "a reply follows an actual send");
        }
        // Instrument immediately upon observing final encoded reply, failure,
        // or cancellation. Cancellation is failure evidence, never a success.
        let now = (self.inner.clock)();
        let encoded = encode_instant(self.inner.epoch, now);
        row.finished_at = Some(now);
        match outcome {
            Some(outcome) => {
                row.final_at = Some(now);
                row.final_outcome = Some(outcome);
            }
            None => {
                row.cancelled_at = Some(now);
            }
        }
        let intended_at = evidence
            .deadline
            .schedule
            .intended_at()
            .expect("T0 published before settlement");
        if now.saturating_duration_since(intended_at) > LIMIT {
            evidence.deadline.breached.store(true, Ordering::Release);
        }
        evidence
            .deadline
            .settled_ns
            .store(encoded, Ordering::Release);
        drop(row);
        if self.inner.registry.lock().unwrap().receiver.is_none() {
            self.inner
                .sender
                .send(Event::Settled(Arc::clone(&evidence.deadline)))
                .expect("watchdog mailbox alive");
        }
        self.unpark();
    }

    fn unpark(&self) {
        if let Some(thread) = self.inner.watchdog_thread.lock().unwrap().as_ref() {
            thread.unpark();
        }
    }
}

fn encode_instant(epoch: Instant, at: Instant) -> u64 {
    u64::try_from(at.saturating_duration_since(epoch).as_nanos())
        .expect("trial timestamp fits u64 nanoseconds")
        .checked_add(1)
        .expect("trial terminal timestamp fits sentinel encoding")
}
