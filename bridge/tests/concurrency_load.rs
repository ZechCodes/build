//! The load test `Bridge Concurrency Spec.md` asks for, in its Verification
//! section: three PTY sessions streaming output while `board.list`,
//! `thread.post` and `workspace.create` are timed. p95 under 200 ms for the
//! reads, under 500 ms for the writes.
//!
//! It is the whole daemon: real frames through the real [`FrameHandler`] and
//! the real [`FrameClock`], real PTYs with real children painting into real
//! screens, real git under `workspace.create`, and a real delivery behind every
//! `thread.post`. Nothing is stubbed except the harness, which is the daemon's
//! own QA agent (`sh -c "printf …; cat >/dev/null"`) — no provider binary is
//! ever looked for, so the numbers are the bridge's own.
//!
//! Determinism is bought two ways. The flood is paced by the test rather than
//! by however fast a machine can run `yes`: each shell is a script that answers
//! one line of input with a fixed burst, so the bytes per second are the same
//! on a laptop and on a loaded CI box. And every measurement is a percentile
//! over a fixed number of calls, so one scheduling hiccup moves the number
//! instead of failing the run, and a slow machine takes longer to make its
//! calls rather than making too few of them to measure (#280).

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use build_bridge::app::AppState;
use build_bridge::carrier::{FrameHandler, SessionSender};
use build_bridge::transport::Frame;
use serde_json::{json, Value};

/// How many shells flood at once. The spec's number, and the shape of the
/// wedge it was written about: three agents painting TUIs at the same time.
const FLOODING_SHELLS: usize = 3;

/// How many times each measured method is called. At the pacing below that is
/// about six seconds of calls on an idle machine: enough reads for a
/// percentile that means something, and as many writes as six seconds held.
const READ_CALLS: u64 = 240;
const POST_CALLS: u64 = 24;
const CREATE_CALLS: u64 = 12;

/// How often each flooding shell is given a line to answer. Its reply is
/// [`BURST_LINES`] lines, so this sets the bytes per second each screen parses.
const FLOOD_INTERVAL: Duration = Duration::from_millis(20);

/// How many lines a shell answers one line of input with. Eighty columns each,
/// so a shell at [`FLOOD_INTERVAL`] paints about 120 KB a second — a TUI
/// redrawing hard, three times over.
const BURST_LINES: usize = 32;

/// A board read every this often: the SPA's poll, several tabs over.
const READ_INTERVAL: Duration = Duration::from_millis(25);

/// A message to an agent every this often.
const POST_INTERVAL: Duration = Duration::from_millis(250);

/// A worktree cut every this often. Each one is a real `git worktree add`, and
/// the reply waits for it.
const CREATE_INTERVAL: Duration = Duration::from_millis(500);

/// The least a shell must have painted for the run to have measured anything.
/// A sixth of what the pacing above asks for, so a slow machine still passes
/// and a flood that silently stopped still fails.
const FLOOD_FLOOR_BYTES: u64 = 100_000;

/// What the spec allows a read to take.
const READ_CEILING: Duration = Duration::from_millis(200);

/// What the spec allows a write's reply to take.
const WRITE_CEILING: Duration = Duration::from_millis(500);

/// How long any one call may go unanswered before the daemon counts as
/// wedged. Sixty times the write ceiling: a loaded machine is slow, not this
/// slow.
const CALL_DEADLINE: Duration = Duration::from_secs(30);

/// How long the whole run may take, setup to teardown. The pacing above is
/// about six seconds of calls on an idle machine; this is thirty times that.
const RUN_DEADLINE: Duration = Duration::from_secs(180);

/// Out of the default run: its ceilings are the spec's latencies on a machine
/// with cores to spare. Four copies on two loaded cores take `workspace.create`
/// to a p95 of 850–940 ms against 500 with nothing wrong in the daemon (#280).
///
/// The runtime is the test's own rather than `#[tokio::test]`'s, so a failure
/// can shut it down with a timeout: a wedged daemon leaves blocking tasks that
/// never finish, and dropping the runtime would wait for them forever (#290).
#[test]
#[ignore = "timing: run with --ignored on a quiet machine"]
fn the_daemon_answers_reads_and_writes_while_three_ptys_flood() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("a runtime");
    let measured = std::panic::catch_unwind(|| {
        let _in_runtime = runtime.enter();
        flood_and_measure();
    });
    runtime.shutdown_timeout(Duration::from_secs(5));
    if let Err(failure) = measured {
        std::panic::resume_unwind(failure);
    }
}

fn flood_and_measure() {
    let deadlines = Deadlines::starting_now(CALL_DEADLINE, RUN_DEADLINE);
    let dir = tempfile::tempdir().expect("a temp dir");
    let repo = init_repo(dir.path());
    std::env::set_var("BRIDGE_TERM_SHELL", flooding_shell(dir.path()));

    let context = build_bridge::harness::HarnessContext::resolved(
        dir.path().join("mcp.sock"),
        dir.path().to_path_buf(),
    )
    .expect("a private harness context");
    let state =
        AppState::new_configured(&repo, dir.path().join("worktrees"), "main", true, context)
            .with_task_store(dir.path().join("state"))
            .expect("a task store")
            .shared();
    let handler = AppState::handler(Arc::clone(&state));
    let client = Client::new(&handler, "s-setup", deadlines);

    let project_id = client.ok("project.list", json!({}))["projects"][0]["project_id"]
        .as_str()
        .expect("the repo is registered as a project")
        .to_string();
    // The id the checkout's path hashes to. Computed here rather than read
    // off the board: this test counts the board reads it makes, and adoption
    // resolves the id against a scan of its own anyway.
    let worktree_id = build_bridge::worktree::external_worktree_id(
        &build_bridge::worktree::canonical_root(&dir.path().join("loose")),
    );
    let run_id = client.ok(
        "run.adopt",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    )["run_id"]
        .as_str()
        .expect("adopting a checkout mints a run")
        .to_string();
    let flooding: Vec<String> = (0..FLOODING_SHELLS)
        .map(|_| {
            client.ok("term.create", json!({ "project_id": project_id }))["term_id"]
                .as_str()
                .expect("a shell answers with its tab id")
                .to_string()
        })
        .collect();

    let measured = Arc::new(AtomicBool::new(false));
    let feeding: Vec<Callers> = flooding
        .iter()
        .map(|term_id| {
            let term_id = term_id.clone();
            Callers::spawn(
                "term.input",
                FLOOD_INTERVAL,
                Until::Set(Arc::clone(&measured)),
                deadlines,
                daemon(&handler, SessionSender::detached("s-term.input")),
                move |_| json!({ "term_id": term_id, "data": base64_of(b"\n") }),
            )
        })
        .collect();
    let reading = Callers::spawn(
        "board.list",
        READ_INTERVAL,
        Until::Called(READ_CALLS),
        deadlines,
        daemon(&handler, SessionSender::detached("s-board.list")),
        |_| json!({}),
    );
    let posting = {
        let run_id = run_id.clone();
        Callers::spawn(
            "thread.post",
            POST_INTERVAL,
            Until::Called(POST_CALLS),
            deadlines,
            daemon(&handler, SessionSender::detached("s-thread.post")),
            move |n| json!({ "entity_id": run_id, "body": format!("load test message {n}") }),
        )
    };
    let creating = Callers::spawn(
        "workspace.create",
        CREATE_INTERVAL,
        Until::Called(CREATE_CALLS),
        deadlines,
        daemon(&handler, SessionSender::detached("s-workspace.create")),
        move |n| json!({ "project_id": project_id, "name": format!("load {n}"), "isolation": "worktree" }),
    );

    let reads = reading.join().unwrap_or_else(|wedged| panic!("{wedged}"));
    let posts = posting.join().unwrap_or_else(|wedged| panic!("{wedged}"));
    let creates = creating.join().unwrap_or_else(|wedged| panic!("{wedged}"));
    // The shells flood for as long as the measured calls take, however long
    // that is on this machine.
    measured.store(true, Ordering::SeqCst);
    for feeder in feeding {
        feeder.join().unwrap_or_else(|wedged| panic!("{wedged}"));
    }

    for term_id in &flooding {
        let attached = client.ok("term.attach", json!({ "term_id": term_id }));
        assert!(
            attached["cursor"].as_u64().unwrap_or(0) > FLOOD_FLOOR_BYTES,
            "{term_id} painted too little to have been flooding: {}",
            attached["cursor"]
        );
        client.ok("term.close", json!({ "term_id": term_id }));
    }

    let served = client.ok("bridge.stats", json!({}));
    for measured in [&reads, &posts, &creates] {
        assert_eq!(
            served["methods"][&measured.method]["served"].as_u64(),
            Some(measured.samples.len() as u64),
            "{} was not counted by the clock that timed it: {served:?}",
            measured.method
        );
    }

    reads.assert_p95_under(READ_CEILING);
    posts.assert_p95_under(WRITE_CEILING);
    creates.assert_p95_under(WRITE_CEILING);

    client.ok("run.abandon", json!({ "run_id": run_id }));
}

/// The per-call deadline, against a daemon that answers a few calls and then
/// never again: the caller gives up with the count it reached instead of
/// waiting forever (#290).
#[test]
fn a_caller_fails_with_its_count_when_a_call_outlives_its_deadline() {
    assert_the_wedge_fails_fast(Deadlines::starting_now(
        Duration::from_millis(200),
        Duration::from_secs(60),
    ));
}

/// The run's deadline on its own: a call that may wait a minute still fails
/// once the run is out of time.
#[test]
fn a_caller_fails_with_its_count_when_the_run_outlives_its_deadline() {
    assert_the_wedge_fails_fast(Deadlines::starting_now(
        Duration::from_secs(60),
        Duration::from_millis(200),
    ));
}

/// One of `deadlines` is short and the other a minute: the short one, and
/// only it, must end the wait, with how far the caller got.
fn assert_the_wedge_fails_fast(deadlines: Deadlines) {
    let caller = wedging_caller(deadlines);
    let waiting = Instant::now();

    let wedged = caller
        .join()
        .err()
        .expect("a wedged daemon fails the caller");

    assert!(
        waiting.elapsed() < Duration::from_secs(10),
        "the short deadline ended the wait, not the long one: {wedged}"
    );
    assert!(
        wedged.starts_with("workspace.create: 3 of 12 calls answered"),
        "the failure names the method and how far it got: {wedged}"
    );
}

/// A `workspace.create` caller against a daemon stub that answers three calls
/// and then never answers again.
fn wedging_caller(deadlines: Deadlines) -> Callers {
    let mut answered = 0;
    let wedging = move |_frame: Frame| {
        if answered == 3 {
            loop {
                std::thread::park();
            }
        }
        answered += 1;
        json!({ "ok": true })
    };
    Callers::spawn(
        "workspace.create",
        Duration::ZERO,
        Until::Called(CREATE_CALLS),
        deadlines,
        wedging,
        |_| json!({}),
    )
}

/// One browser session, calling frames one at a time for the test's setup
/// and teardown.
struct Client {
    handler: FrameHandler,
    sender: SessionSender,
    deadlines: Deadlines,
}

impl Client {
    fn new(handler: &FrameHandler, session_id: &str, deadlines: Deadlines) -> Client {
        Client {
            handler: handler.clone(),
            sender: SessionSender::detached(session_id),
            deadlines,
        }
    }

    /// Call `method` and hand back its result, failing the test on a refusal
    /// or on no answer by the deadline — nothing in the setup or the teardown
    /// has a failure the load numbers would still mean something without.
    fn ok(&self, method: &str, params: Value) -> Value {
        let mut answer = daemon(&self.handler, self.sender.clone());
        let frame = request(method, params);
        let (reply, answered) = mpsc::channel();
        std::thread::spawn(move || reply.send(answer(frame)));
        let answered = answered
            .recv_timeout(self.deadlines.left_for_a_call_asked_at(Instant::now()))
            .unwrap_or_else(|_| panic!("{method}: {}", self.deadlines.missed()));
        assert_eq!(answered["ok"], true, "{method}: {answered:?}");
        answered["result"].clone()
    }
}

/// How long a call may wait for its answer, and when the whole run must be
/// over. A wedged daemon then fails the test instead of hanging it (#290).
#[derive(Clone, Copy)]
struct Deadlines {
    per_call: Duration,
    run: Duration,
    run_ends: Instant,
}

impl Deadlines {
    fn starting_now(per_call: Duration, run: Duration) -> Deadlines {
        Deadlines {
            per_call,
            run,
            run_ends: Instant::now() + run,
        }
    }

    /// How much longer a call asked at `asked_at` may wait: its own deadline
    /// or the run's, whichever comes first.
    fn left_for_a_call_asked_at(&self, asked_at: Instant) -> Duration {
        (asked_at + self.per_call)
            .min(self.run_ends)
            .saturating_duration_since(Instant::now())
    }

    /// How much longer the run may wait for a caller between calls.
    fn left_for_the_run(&self) -> Duration {
        self.run_ends.saturating_duration_since(Instant::now())
    }

    fn missed(&self) -> String {
        format!(
            "no answer within the deadline ({:?} a call, {:?} the run)",
            self.per_call, self.run
        )
    }
}

/// When a caller stops: after a number of calls, or once a flag is set.
enum Until {
    Called(u64),
    Set(Arc<AtomicBool>),
}

impl Until {
    fn reached(&self, calls: u64) -> bool {
        match self {
            Until::Called(count) => calls >= *count,
            Until::Set(flag) => flag.load(Ordering::SeqCst),
        }
    }

    /// How far `calls` got, for a failure: "3 of 12", or just "3".
    fn progress(&self, calls: usize) -> String {
        match self {
            Until::Called(count) => format!("{calls} of {count}"),
            Until::Set(_) => calls.to_string(),
        }
    }
}

/// One caller repeating one method until it is done, and how long each of
/// its replies took. It runs on a thread of its own, as a relay dispatch
/// worker does, and tells the test each time it asks and each time it is
/// answered, so the test can hold every call to the deadlines.
struct Callers {
    method: &'static str,
    until: Arc<Until>,
    deadlines: Deadlines,
    progress: Receiver<Progress>,
    thread: std::thread::JoinHandle<()>,
}

/// What a caller thread reports as it goes.
enum Progress {
    Asked(Instant),
    Answered(Duration),
}

impl Callers {
    fn spawn(
        method: &'static str,
        every: Duration,
        until: Until,
        deadlines: Deadlines,
        mut answer: impl FnMut(Frame) -> Value + Send + 'static,
        mut params: impl FnMut(u64) -> Value + Send + 'static,
    ) -> Callers {
        let until = Arc::new(until);
        let (report, progress) = mpsc::channel();
        let calling = Arc::clone(&until);
        let thread = std::thread::spawn(move || {
            let mut n = 0;
            while !calling.reached(n) {
                let frame = request(method, params(n));
                let asked_at = Instant::now();
                let _ = report.send(Progress::Asked(asked_at));
                let answered = answer(frame);
                let _ = report.send(Progress::Answered(asked_at.elapsed()));
                assert_eq!(answered["ok"], true, "{method}: {answered:?}");
                n += 1;
                std::thread::sleep(every);
            }
        });
        Callers {
            method,
            until,
            deadlines,
            progress,
            thread,
        }
    }

    /// Every reply's latency once the caller is done, or how far it got if a
    /// call outlived its deadline or the run outlived its own. A wedged call
    /// is left blocked on its thread; the test fails without it.
    fn join(self) -> Result<Latencies, String> {
        let mut samples = Vec::new();
        let mut waiting_since = None;
        loop {
            let left = waiting_since.map_or_else(
                || self.deadlines.left_for_the_run(),
                |asked_at| self.deadlines.left_for_a_call_asked_at(asked_at),
            );
            match self.progress.recv_timeout(left) {
                Ok(Progress::Asked(asked_at)) => waiting_since = Some(asked_at),
                Ok(Progress::Answered(took)) if took <= self.deadlines.per_call => {
                    waiting_since = None;
                    samples.push(took);
                }
                Ok(Progress::Answered(_)) | Err(RecvTimeoutError::Timeout) => {
                    return Err(self.wedged(samples.len()))
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        self.thread.join().expect("a caller made all its calls");
        Ok(Latencies {
            method: self.method.to_string(),
            samples,
        })
    }

    fn wedged(&self, answered: usize) -> String {
        format!(
            "{}: {} calls answered, then {}",
            self.method,
            self.until.progress(answered),
            self.deadlines.missed()
        )
    }
}

/// The daemon as one session sees it: hand it a frame, get its reply. Called
/// with the test's runtime entered, because that is what a relay dispatch
/// worker is: a blocking thread inside the runtime, calling the handler and
/// waiting for the answer.
fn daemon(
    handler: &FrameHandler,
    sender: SessionSender,
) -> impl FnMut(Frame) -> Value + Send + 'static {
    let handler = handler.clone();
    let runtime = tokio::runtime::Handle::current();
    move |frame| {
        let _in_runtime = runtime.enter();
        handler.call(sender.clone(), frame)
    }
}

/// What one method's callers measured.
struct Latencies {
    method: String,
    samples: Vec<Duration>,
}

impl Latencies {
    fn percentile(&self, of_a_hundred: usize) -> Duration {
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        let at = (sorted.len() * of_a_hundred / 100).min(sorted.len() - 1);
        sorted[at]
    }

    fn worst(&self) -> Duration {
        self.samples.iter().copied().max().unwrap_or_default()
    }

    fn assert_p95_under(&self, ceiling: Duration) {
        let p95 = self.percentile(95);
        eprintln!(
            "{}: p95 {:?}, p50 {:?}, worst {:?} ({} frames)",
            self.method,
            p95,
            self.percentile(50),
            self.worst(),
            self.samples.len()
        );
        assert!(
            p95 < ceiling,
            "{}: p95 {:?} over the spec's {:?} ({} frames, p50 {:?}, worst {:?})",
            self.method,
            p95,
            ceiling,
            self.samples.len(),
            self.percentile(50),
            self.worst(),
        );
    }
}

fn request(method: &str, params: Value) -> Frame {
    Frame {
        session_id: "s".into(),
        message_id: "m".into(),
        frame_type: "data".into(),
        sender: "client".into(),
        created_at: "t".into(),
        payload: json!({ "method": method, "id": "1", "params": params }),
    }
}

fn base64_of(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A "login shell" that answers every line it is given with a fixed burst of
/// output and never exits until its pty closes. The daemon spawns it exactly
/// as it spawns a human's shell, so the whole path — pty, pump, screen, the
/// clients attached to it — is the production one; only the rate is the
/// test's.
fn flooding_shell(dir: &Path) -> PathBuf {
    let script = dir.join("flooding-shell");
    let line = "x".repeat(79);
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             while IFS= read -r _line; do\n\
             \x20 n=0\n\
             \x20 while [ \"$n\" -lt {BURST_LINES} ]; do\n\
             \x20   printf '%s\\n' '{line}'\n\
             \x20   n=$((n + 1))\n\
             \x20 done\n\
             done\n"
        ),
    )
    .expect("the flooding shell is written");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("the flooding shell is executable");
    script
}

fn init_repo(parent: &Path) -> PathBuf {
    let repo = parent.join("repo");
    std::fs::create_dir(&repo).expect("a repo dir");
    let git = |args: &[&str]| {
        // Nothing from the machine's own git config: a `commit.gpgsign`
        // there would sign this commit with a real key.
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.email", "t@build.ing"]);
    git(&["config", "user.name", "T"]);
    // The product checkpoints in the linked worktree inherit this policy.
    git(&["config", "--local", "commit.gpgsign", "false"]);
    let mut readme = std::fs::File::create(repo.join("README.md")).expect("a readme");
    readme
        .write_all(b"# load\n")
        .expect("the readme is written");
    drop(readme);
    git(&["add", "."]);
    git(&["commit", "-m", "initial"]);
    // A checkout the human already had, for the run this load test drives.
    // Work happens in a checkout beside the repository, never in it.
    git(&[
        "worktree",
        "add",
        "-b",
        "loose",
        parent.join("loose").to_str().expect("a utf-8 path"),
    ]);
    repo
}
