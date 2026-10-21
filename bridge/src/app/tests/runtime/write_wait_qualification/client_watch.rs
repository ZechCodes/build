//! Caller deadlines are independent of server registration and completion.
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

#[derive(Clone, Debug)]
struct ClientRow {
    original_id: String,
    offered_at: Instant,
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

    fn offer(&mut self, original_id: &str) {
        assert!(
            !self.rows.contains_key(original_id),
            "original is offered once"
        );
        self.rows.insert(
            original_id.into(),
            ClientRow {
                original_id: original_id.into(),
                offered_at: (self.clock)(),
                sent_at: None,
                finished_at: None,
                cancelled: false,
                breached: false,
            },
        );
    }

    fn sent(&mut self, original_id: &str) {
        self.rows.get_mut(original_id).unwrap().sent_at = Some((self.clock)());
    }

    // Initial scaffold mirrors completed-only accounting. These controls must
    // fail before active/archived deadline handling is implemented.
    fn complete(&mut self, original_id: &str) {
        self.rows.remove(original_id).unwrap();
    }

    fn cancel(&mut self, original_id: &str) {
        self.rows.remove(original_id).unwrap();
    }

    fn snapshot(&self) -> Vec<ClientRow> {
        self.rows.values().cloned().collect()
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
    assert_eq!(row.offered_at, offered);
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
