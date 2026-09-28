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

fn choice(provider: AgentProvider, model: Option<&str>) -> ModelChoice {
    ModelChoice {
        provider,
        model: model.map(str::to_string),
        effort: None,
    }
}

fn claude_at(raw: &str) -> CliReading {
    CliReading {
        version: Some(version(raw)),
        listed: None,
    }
}

#[test]
fn a_model_newer_than_the_installed_claude_code_is_refused_on_both_carriers() {
    for provider in [AgentProvider::Claude, AgentProvider::ClaudeAdk] {
        let harness = harness_for(provider);
        let refused = refusal(
            harness,
            &choice(provider, Some("claude-sonnet-5-5")),
            Some(&claude_at("2.1.280")),
        );

        assert_eq!(
            refused.as_deref(),
            Some("Build cannot start Claude Sonnet 5.5 here: Claude Code 2.1.280 is installed, and Claude Sonnet 5.5 needs 2.1.284 or newer. An older Claude Code refuses it or runs it with too small a context window. Update Claude Code, or choose another model."),
            "{provider:?}"
        );
    }
}

#[test]
fn what_the_installed_cli_runs_is_started() {
    let harness = harness_for(AgentProvider::ClaudeAdk);
    let reading = claude_at("2.1.280");

    for model in [Some("claude-opus-5-5"), Some("claude-haiku-4-5-20251001"), None] {
        assert_eq!(
            refusal(harness, &choice(AgentProvider::ClaudeAdk, model), Some(&reading)),
            None,
            "{model:?}"
        );
    }
    assert_eq!(
        refusal(
            harness,
            &choice(AgentProvider::ClaudeAdk, Some("claude-sonnet-5-5")),
            None
        ),
        None,
        "an unread CLI refuses nothing"
    );
}

#[test]
fn a_model_codex_does_not_list_is_refused() {
    let harness = harness_for(AgentProvider::CodexAppServer);
    let reading = CliReading {
        version: Some(version("0.155.1")),
        listed: Some(vec![ListedModel {
            id: "gpt-6-sol".into(),
            label: "GPT-6-Sol".into(),
            hidden: false,
            efforts: vec!["high".into()],
        }]),
    };

    assert!(refusal(
        harness,
        &choice(AgentProvider::CodexAppServer, Some("gpt-5.2")),
        Some(&reading)
    )
    .is_some());
    assert!(refusal(
        harness,
        &choice(AgentProvider::Codex, Some("gpt-6-sol")),
        Some(&reading)
    )
    .is_none());
}

/// The process-wide readings of a test build never ask, so every spawn a test
/// makes is let through.
#[test]
fn the_unit_test_readings_refuse_nothing() {
    assert!(refuse_unrunnable(&choice(AgentProvider::ClaudeAdk, Some("claude-sonnet-5-5"))).is_ok());
}
