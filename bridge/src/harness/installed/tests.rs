use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

use super::*;

/// A CLI that answers with whatever version the test last set, and counts how
/// often it was asked.
struct ScriptedCli {
    version: Mutex<Option<Version>>,
    asked: AtomicUsize,
}

impl ScriptedCli {
    fn leaked(version: &str) -> &'static Self {
        Box::leak(Box::new(Self {
            version: Mutex::new(Version::parse(version).ok()),
            asked: AtomicUsize::new(0),
        }))
    }

    fn set(&self, version: &str) {
        *self.version.lock().unwrap() = Version::parse(version).ok();
    }

    fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}

impl CliProbe for ScriptedCli {
    fn read(&self, _binary: &str) -> CliReading {
        self.asked.fetch_add(1, Ordering::SeqCst);
        CliReading {
            version: self.version.lock().unwrap().clone(),
            listed: None,
        }
    }
}

/// A clock the test moves by hand, in seconds past a fixed start.
struct HandClock {
    start: Instant,
    offset_secs: AtomicU64,
}

impl HandClock {
    fn leaked() -> &'static Self {
        Box::leak(Box::new(Self {
            start: Instant::now(),
            offset_secs: AtomicU64::new(0),
        }))
    }

    fn advance(&self, by: Duration) {
        self.offset_secs.fetch_add(by.as_secs(), Ordering::SeqCst);
    }

    fn now(&self) -> Instant {
        self.start + Duration::from_secs(self.offset_secs.load(Ordering::SeqCst))
    }
}

/// Readings that ask on the calling thread, so an ask has landed by the time
/// the call that started it returns.
fn inline(clock: &'static HandClock) -> Arc<Readings> {
    Readings::with(
        Some(Arc::new(|ask: Box<dyn FnOnce() + Send>| ask())),
        Arc::new(move || clock.now()),
        READING_TTL,
    )
}

fn version(raw: &str) -> Version {
    Version::parse(raw).unwrap()
}

#[test]
fn a_first_ask_reads_the_cli() {
    let cli = ScriptedCli::leaked("2.1.280");
    let readings = inline(HandClock::leaked());

    let reading = readings.reading("claude", cli).expect("an inline ask has landed");

    assert_eq!(reading.version, Some(version("2.1.280")));
    assert_eq!(cli.asked(), 1);
}

#[test]
fn an_answer_stands_until_the_ttl_runs_out() {
    let cli = ScriptedCli::leaked("2.1.280");
    let clock = HandClock::leaked();
    let readings = inline(clock);
    readings.reading("claude", cli);

    cli.set("2.1.284");
    clock.advance(READING_TTL - Duration::from_secs(1));
    let within = readings.reading("claude", cli).unwrap();
    clock.advance(Duration::from_secs(1));
    let after = readings.reading("claude", cli).unwrap();

    assert_eq!(within.version, Some(version("2.1.280")));
    assert_eq!(after.version, Some(version("2.1.284")));
    assert_eq!(cli.asked(), 2);
}

#[test]
fn a_session_reporting_another_version_asks_again_at_once() {
    let cli = ScriptedCli::leaked("2.1.280");
    let readings = inline(HandClock::leaked());
    readings.reading("claude", cli);

    cli.set("2.1.284");
    readings.observe_version("claude", cli, &version("2.1.284"));

    assert_eq!(
        readings.reading("claude", cli).unwrap().version,
        Some(version("2.1.284"))
    );
    assert_eq!(cli.asked(), 2);
}

#[test]
fn a_session_reporting_the_held_version_asks_nothing() {
    let cli = ScriptedCli::leaked("2.1.280");
    let readings = inline(HandClock::leaked());
    readings.reading("claude", cli);

    readings.observe_version("claude", cli, &version("2.1.280"));

    assert_eq!(cli.asked(), 1);
}

#[test]
fn only_a_changed_answer_counts_as_a_change() {
    let cli = ScriptedCli::leaked("2.1.280");
    let clock = HandClock::leaked();
    let readings = inline(clock);
    let changes = readings.changes();

    readings.reading("claude", cli);
    assert_eq!(*changes.borrow(), 1, "the first answer is news");
    clock.advance(READING_TTL);
    readings.reading("claude", cli);
    assert_eq!(*changes.borrow(), 1, "the same answer again is not");
    cli.set("2.1.284");
    clock.advance(READING_TTL);
    readings.reading("claude", cli);
    assert_eq!(*changes.borrow(), 2);
}

#[test]
fn one_ask_at_a_time_per_cli() {
    let cli = ScriptedCli::leaked("2.1.280");
    let queued: &'static Mutex<Vec<Box<dyn FnOnce() + Send>>> =
        Box::leak(Box::new(Mutex::new(Vec::new())));
    let readings = Readings::with(
        Some(Arc::new(|ask| queued.lock().unwrap().push(ask))),
        Arc::new(Instant::now),
        READING_TTL,
    );

    assert!(readings.reading("claude", cli).is_none(), "nothing has landed");
    readings.reading("claude", cli);
    readings.observe_version("claude", cli, &version("2.1.284"));

    assert_eq!(queued.lock().unwrap().len(), 1);
    for ask in queued.lock().unwrap().drain(..) {
        ask();
    }
    assert_eq!(cli.asked(), 1);
    assert!(readings.reading("claude", cli).is_some());
}

#[test]
fn readings_that_never_ask_hold_nothing() {
    let cli = ScriptedCli::leaked("2.1.280");
    let readings = Readings::inert();

    assert!(readings.reading("claude", cli).is_none());
    readings.observe_version("claude", cli, &version("2.1.284"));
    assert_eq!(cli.asked(), 0);
}

#[tokio::test]
async fn a_background_ask_lands_and_is_announced() {
    let cli = ScriptedCli::leaked("2.1.280");
    let readings = Readings::background();
    let mut changes = readings.changes();

    assert!(readings.reading("claude", cli).is_none());
    tokio::time::timeout(Duration::from_secs(5), changes.changed())
        .await
        .expect("the probe thread answers")
        .unwrap();

    assert_eq!(
        readings.reading("claude", cli).unwrap().version,
        Some(version("2.1.280"))
    );
}
