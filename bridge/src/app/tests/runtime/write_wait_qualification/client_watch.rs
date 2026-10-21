//! Caller deadlines are independent of server registration and completion.
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

#[derive(Clone, Debug)]
struct ClientRow {
    original_id: String,
    intended_at: Instant,
    offered_at: Option<Instant>,
    sent_at: Option<Instant>,
    finished_at: Option<Instant>,
    cancelled: bool,
    breached: bool,
}

struct ClientWatch {
    clock: Clock,
    rows: BTreeMap<String, ClientRow>,
}

impl ClientWatch {
    fn new(clock: Clock) -> Self {
        Self {
            clock,
            rows: BTreeMap::new(),
        }
    }

    fn register_at(&mut self, original_id: &str, intended: Instant) {
        assert!(
            !self.rows.contains_key(original_id),
            "original schedule identity is unique"
        );
        // Intended schedule time survives scheduler, socket and credit delay.
        let at = intended;
        self.rows.insert(
            original_id.into(),
            ClientRow {
                original_id: original_id.into(),
                intended_at: at,
                offered_at: None,
                sent_at: None,
                finished_at: None,
                cancelled: false,
                breached: false,
            },
        );
    }

    fn offer(&mut self, original_id: &str) {
        if !self.rows.contains_key(original_id) {
            self.register_at(original_id, (self.clock)());
        }
        let row = self.rows.get_mut(original_id).unwrap();
        assert!(row.offered_at.is_none(), "original is offered once");
        row.offered_at = Some((self.clock)());
    }

    fn sent(&mut self, original_id: &str) {
        self.rows.get_mut(original_id).unwrap().sent_at = Some((self.clock)());
    }

    fn complete(&mut self, original_id: &str) {
        let now = (self.clock)();
        let row = self.rows.get_mut(original_id).unwrap();
        assert!(row.finished_at.is_none(), "original settles once");
        row.finished_at = Some(now);
        row.breached |= now.saturating_duration_since(row.intended_at) > Duration::from_secs(5);
    }

    fn cancel(&mut self, original_id: &str) {
        self.complete(original_id);
        self.rows.get_mut(original_id).unwrap().cancelled = true;
    }

    fn watch(&self) -> ClientWatchGuard {
        // TEST-FIRST seam: the independently running deadline observer is not
        // installed yet. A snapshot caller is not an independent watchdog.
        ClientWatchGuard
    }

    fn snapshot(&self) -> Vec<ClientRow> {
        let now = (self.clock)();
        self.rows
            .values()
            .cloned()
            .map(|mut row| {
                row.breached |= row
                    .finished_at
                    .unwrap_or(now)
                    .saturating_duration_since(row.intended_at)
                    > Duration::from_secs(5);
                row
            })
            .collect()
    }
}

fn fixture() -> (ClientWatch, Arc<std::sync::Mutex<Instant>>) {
    let at = Arc::new(std::sync::Mutex::new(Instant::now()));
    let clock = Arc::clone(&at);
    (
        ClientWatch::new(Arc::new(move || *clock.lock().unwrap())),
        at,
    )
}

#[test]
fn unfinished_client_before_server_registration_breaches_original_deadline() {
    let (mut watch, clock) = fixture();
    watch.offer("qual/small/trial1/write/00000");
    *clock.lock().unwrap() += Duration::from_secs(5);
    assert!(!watch.snapshot()[0].breached, "five seconds is inclusive");
    *clock.lock().unwrap() += Duration::from_nanos(1);
    let rows = watch.snapshot();
    assert!(
        rows[0].sent_at.is_none(),
        "server registration need not exist"
    );
    assert!(
        rows[0].breached,
        "unfinished caller cannot disappear from completed histograms"
    );
}

#[test]
fn late_completion_without_a_prior_poll_retains_breach_and_original_times() {
    let (mut watch, clock) = fixture();
    let original = "qual/small/trial1/write/00001";
    watch.offer(original);
    let offered = *clock.lock().unwrap();
    *clock.lock().unwrap() += Duration::from_secs(1);
    watch.sent(original);
    let sent = *clock.lock().unwrap();
    *clock.lock().unwrap() += Duration::from_secs(5);
    watch.complete(original);
    let row = watch
        .snapshot()
        .pop()
        .expect("completed callers remain archived");
    assert_eq!(row.original_id, original);
    assert_eq!(row.offered_at, Some(offered));
    assert_eq!(row.sent_at, Some(sent));
    assert!(row.finished_at.is_some());
    assert!(
        row.breached,
        "late completion is a failure even without earlier polling"
    );
}

#[test]
fn cancellation_and_cleanup_cannot_erase_an_already_overdue_caller() {
    let (mut watch, clock) = fixture();
    let original = "qual/large/trial3/write/00002";
    watch.offer(original);
    watch.sent(original);
    *clock.lock().unwrap() += Duration::from_secs(6);
    watch.cancel(original);
    let row = watch
        .snapshot()
        .pop()
        .expect("cancelled caller remains evidence");
    assert!(row.cancelled);
    assert!(row.breached);
}

#[test]
fn intended_due_time_detects_late_scheduler_before_actual_offer_or_server_registration() {
    let (mut watch, clock) = fixture();
    let base = *clock.lock().unwrap();
    let original = "qual/small/trial1/write/00040";
    watch.register_at(original, base + Duration::from_secs(10));
    *clock.lock().unwrap() = base + Duration::from_secs(6);
    assert!(
        !watch.snapshot()[0].breached,
        "future due time has zero age"
    );
    *clock.lock().unwrap() = base + Duration::from_secs(15);
    assert!(
        !watch.snapshot()[0].breached,
        "exact five seconds is inclusive"
    );
    *clock.lock().unwrap() += Duration::from_nanos(1);
    assert!(
        watch.snapshot()[0].breached,
        "no offer or server origin is needed"
    );
    watch.offer(original);
    watch.sent(original);
    let row = &watch.snapshot()[0];
    assert_eq!(row.intended_at, base + Duration::from_secs(10));
    assert!(row.offered_at.unwrap() > row.intended_at);
    assert!(
        row.breached,
        "late offer cannot reset the original deadline"
    );
}

struct ClientWatchGuard;
impl ClientWatchGuard {
    fn failed(&self) -> bool {
        false
    }
    fn finish(self) -> bool {
        self.failed()
    }
}

#[test]
fn independent_client_watch_observes_unregistered_pending_without_snapshot_or_runtime_poll() {
    let (mut watch, clock) = fixture();
    let original = "qual/small/trial1/write/independent";
    let due = *clock.lock().unwrap();
    watch.register_at(original, due);
    watch.offer(original);
    let guard = watch.watch();
    // Only the fake clock changes. Neither a collector/app lock nor a caller
    // snapshot or Tokio task drives the watchdog's observation.
    *clock.lock().unwrap() += Duration::from_secs(5) + Duration::from_nanos(1);
    let deadline = Instant::now() + Duration::from_millis(250);
    while !guard.failed() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    let observed = guard.failed();
    let joined = guard.finish();
    assert!(
        observed && joined,
        "independent watchdog must retain unfinished client breach before server registration"
    );
}
