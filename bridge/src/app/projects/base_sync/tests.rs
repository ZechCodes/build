//! What a timeout does to the timer (#268): nothing that needs a person. The
//! service retries it on the next pass and backs off if it keeps happening,
//! and a cut that ran out of time leaves the source's status as it was.

use super::*;
use crate::git_fixture::{git_command, git_in, init_repo_named};

const MINUTE_MS: i64 = 60_000;
const START_MS: i64 = 1_000_000_000;

struct Fixture {
    _dir: tempfile::TempDir,
    state: AppState,
    project_id: String,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let repo = init_repo_named(dir.path(), "code");
    let mut state = AppState::new_unrooted(
        dir.path().join("worktrees"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.add_project(repo, "main".into());
    Fixture {
        _dir: dir,
        state,
        project_id,
    }
}

fn timed_out() -> SyncReport {
    SyncReport {
        outcome: SyncOutcome::Failed(Failure {
            reason: "origin did not answer within 30 s.".into(),
            needs_you: false,
            timed_out: true,
        }),
        ahead: 0,
        behind: 0,
        fetched: false,
    }
}

fn up_to_date() -> SyncReport {
    SyncReport {
        outcome: SyncOutcome::UpToDate,
        ahead: 0,
        behind: 0,
        fetched: true,
    }
}

impl Fixture {
    fn source(&self) -> &crate::app::projects::ProjectSource {
        &self.state.projects.get(&self.project_id).unwrap().sources[0]
    }

    fn status(&self) -> Option<SyncStatus> {
        self.source().sync_status.clone()
    }

    fn subject(&self, origin: SyncOrigin) -> SyncSubject {
        subject(
            &self.project_id,
            self.source(),
            Fetch::Within(SERVICE_FETCH_DEADLINE),
            origin,
        )
    }

    fn settle(&mut self, origin: SyncOrigin, report: SyncReport, now_ms: i64) {
        let subject = self.subject(origin);
        self.state
            .settle_source_syncs(vec![Synced { subject, report }], now_ms);
    }

    fn timer_takes_it(&mut self, now_ms: i64) -> bool {
        !self
            .state
            .source_sync_subjects(SyncPass::Due, now_ms)
            .is_empty()
    }
}

#[test]
fn a_service_fetch_that_timed_out_is_retried_on_the_next_pass() {
    let mut fixture = fixture();

    fixture.settle(SyncOrigin::Service, timed_out(), START_MS);

    let status = fixture.status().unwrap();
    assert_eq!(status.state, SyncState::Failed);
    assert!(!status.needs_you, "{status:?}");
    assert!(fixture.timer_takes_it(START_MS + 5 * MINUTE_MS));
}

#[test]
fn a_fetch_that_keeps_timing_out_is_tried_less_often() {
    let mut fixture = fixture();
    fixture.settle(SyncOrigin::Service, timed_out(), START_MS);
    let second = START_MS + 5 * MINUTE_MS;
    fixture.settle(SyncOrigin::Service, timed_out(), second);

    assert!(!fixture.timer_takes_it(second + 5 * MINUTE_MS));
    assert!(fixture.timer_takes_it(second + 10 * MINUTE_MS));

    let third = second + 10 * MINUTE_MS;
    fixture.settle(SyncOrigin::Service, timed_out(), third);
    assert!(!fixture.timer_takes_it(third + 10 * MINUTE_MS));
    assert!(fixture.timer_takes_it(third + 20 * MINUTE_MS));

    let mut last = third;
    for _ in 0..6 {
        last += 60 * MINUTE_MS;
        fixture.settle(SyncOrigin::Service, timed_out(), last);
    }
    assert!(
        fixture.timer_takes_it(last + 60 * MINUTE_MS),
        "the wait is capped at an hour"
    );
}

#[test]
fn a_sync_that_answers_ends_the_backoff() {
    let mut fixture = fixture();
    fixture.settle(SyncOrigin::Service, timed_out(), START_MS);
    fixture.settle(SyncOrigin::Service, timed_out(), START_MS + 5 * MINUTE_MS);

    fixture.settle(SyncOrigin::Cut, up_to_date(), START_MS + 6 * MINUTE_MS);

    assert!(fixture.timer_takes_it(START_MS + 11 * MINUTE_MS));
}

#[test]
fn sync_now_is_not_held_back_by_the_backoff() {
    let mut fixture = fixture();
    fixture.settle(SyncOrigin::Service, timed_out(), START_MS);
    fixture.settle(SyncOrigin::Service, timed_out(), START_MS + 5 * MINUTE_MS);
    let project_id = fixture.project_id.clone();
    let source_id = fixture.source().id.clone();
    fixture.state.request_source_sync(project_id, source_id);

    let asked = fixture
        .state
        .source_sync_subjects(SyncPass::Requested, START_MS + 6 * MINUTE_MS);

    assert_eq!(asked.len(), 1);
}

/// A cut gives its fetch ten seconds, a third of the service's. Running out
/// of them says nothing about the remote the timer should act on.
#[test]
fn a_cut_that_ran_out_of_time_leaves_the_status_as_it_was() {
    let mut fixture = fixture();
    fixture.settle(SyncOrigin::Service, up_to_date(), START_MS);
    let before = fixture.status();

    fixture.settle(SyncOrigin::Cut, timed_out(), START_MS + MINUTE_MS);

    assert_eq!(fixture.status(), before);
    assert!(fixture.timer_takes_it(START_MS + 5 * MINUTE_MS));
}

#[test]
fn a_remote_that_wanted_a_person_holds_the_timer() {
    let mut fixture = fixture();
    let wanted = SyncReport {
        outcome: SyncOutcome::Failed(Failure {
            reason: "Confirm user presence for key".into(),
            needs_you: true,
            timed_out: true,
        }),
        ..timed_out()
    };

    fixture.settle(SyncOrigin::Service, wanted, START_MS);

    assert!(fixture.status().unwrap().needs_you);
    assert!(!fixture.timer_takes_it(START_MS + 24 * 60 * MINUTE_MS));
}

/// A source cloned from `upstream`, whose next upstream commit adds a file
/// behind a smudge filter that sleeps `seconds`: the base's own checkout is
/// slow to fast-forward (git-lfs downloading a large file).
fn slow_checkout_fixture(seconds: u32) -> (Fixture, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let upstream = init_repo_named(dir.path(), "upstream");
    let base = dir.path().join("base");
    git_in(
        dir.path(),
        &["clone", "-q", upstream.to_str().unwrap(), "base"],
    );
    let smudge = format!("sleep {seconds}; cat");
    git_in(&base, &["config", "filter.slow.smudge", &smudge]);
    git_in(&base, &["config", "filter.slow.clean", "cat"]);
    std::fs::write(upstream.join(".gitattributes"), "*.bin filter=slow\n").unwrap();
    std::fs::write(upstream.join("a.bin"), "payload\n").unwrap();
    git_in(&upstream, &["add", "."]);
    git_in(&upstream, &["commit", "-q", "-m", "slow"]);
    let mut state = AppState::new_unrooted(
        dir.path().join("worktrees"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    let project_id = state.add_project(base.clone(), "main".into());
    (
        Fixture {
            _dir: dir,
            state,
            project_id,
        },
        base,
    )
}

fn head_of(repo: &std::path::Path, name: &str) -> String {
    let output = git_command(repo, &["rev-parse", name]).output().unwrap();
    assert!(output.status.success(), "rev-parse {name}");
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A cut waits on the network for ten seconds and no longer, and never on
/// the base's own checkout: a slow one is left to the service, which the cut
/// asks to run, and the cut goes ahead from the base as it stands with a
/// warning saying so (review #268).
#[test]
fn a_cut_leaves_a_slow_checkout_to_the_service() {
    let (fixture, base) = slow_checkout_fixture(40);
    let project_id = fixture.project_id.clone();
    let before = head_of(&base, "main");
    let state = Arc::new(Mutex::new(fixture.state));

    let started = std::time::Instant::now();
    let subjects = state.lock().unwrap().cut_sync_subjects(&project_id);
    let synced = sync_before_cut(&subjects);
    let took = started.elapsed();

    assert!(took < CUT_FETCH_DEADLINE, "the cut waited {took:?}");
    assert_eq!(head_of(&base, "main"), before);
    let warnings = cut_warnings(&synced);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("1 commit behind"), "{warnings:?}");
    state.lock().unwrap().settle_source_syncs(synced, START_MS);

    AppState::sync_sources(&state, SyncPass::Requested, START_MS + MINUTE_MS);

    assert_eq!(head_of(&base, "main"), head_of(&base, "origin/main"));
    assert_ne!(head_of(&base, "main"), before);
    assert_eq!(
        std::fs::read_to_string(base.join("a.bin")).unwrap(),
        "payload\n"
    );
    let app = state.lock().unwrap();
    let status = app.projects.get(&project_id).unwrap().sources[0]
        .sync_status
        .clone()
        .unwrap();
    assert_eq!(status.state, SyncState::Synced, "{status:?}");
}
