//! The load test `Bridge Concurrency Spec.md` asks for, in its Verification
//! section: three PTY sessions streaming output while `board.list`,
//! `thread.post` and `worktree.create` are timed. p95 under 200 ms for the
//! reads, under 500 ms for the writes.
//!
//! It is the whole daemon: real frames through the real [`FrameHandler`] and
//! the real [`FrameClock`], real PTYs with real children painting into real
//! screens, real git under `worktree.create`, and a real delivery behind every
//! `thread.post`. Nothing is stubbed except the harness, which is the daemon's
//! own QA agent (`sh -c "printf …; cat >/dev/null"`) — no provider binary is
//! ever looked for, so the numbers are the bridge's own.
//!
//! Determinism is bought two ways. The flood is paced by the test rather than
//! by however fast a machine can run `yes`: each shell is a script that answers
//! one line of input with a fixed burst, so the bytes per second are the same
//! on a laptop and on a loaded CI box. And every measurement is a percentile
//! over a few hundred samples inside a bounded window, so one scheduling
//! hiccup moves the number instead of failing the run.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
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

/// How long the measured frames run for. Long enough for a few hundred reads
/// to give a percentile that means something, short enough that the whole test
/// is a few seconds of CI.
const MEASURED_FOR: Duration = Duration::from_secs(6);

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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_daemon_answers_reads_and_writes_while_three_ptys_flood() {
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
    let client = Client::new(&handler, "s-setup");

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

    let until = Instant::now() + MEASURED_FOR;
    let feeding: Vec<Callers> = flooding
        .iter()
        .map(|term_id| {
            let term_id = term_id.clone();
            Callers::spawn(
                &handler,
                "term.input",
                FLOOD_INTERVAL,
                until,
                move |_| json!({ "term_id": term_id, "data": base64_of(b"\n") }),
            )
        })
        .collect();
    let reading = Callers::spawn(&handler, "board.list", READ_INTERVAL, until, |_| json!({}));
    let posting = {
        let run_id = run_id.clone();
        Callers::spawn(
            &handler,
            "thread.post",
            POST_INTERVAL,
            until,
            move |n| json!({ "entity_id": run_id, "body": format!("load test message {n}") }),
        )
    };
    let creating = Callers::spawn(
        &handler,
        "worktree.create",
        CREATE_INTERVAL,
        until,
        move |n| json!({ "project_id": project_id, "name": format!("load {n}") }),
    );

    let reads = reading.join();
    let posts = posting.join();
    let creates = creating.join();
    for feeder in feeding {
        feeder.join();
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

/// One browser session, calling frames on the test's own thread.
struct Client {
    handler: FrameHandler,
    sender: SessionSender,
}

impl Client {
    fn new(handler: &FrameHandler, session_id: &str) -> Client {
        Client {
            handler: handler.clone(),
            sender: SessionSender::detached(session_id),
        }
    }

    /// Call `method` and hand back its result, failing the test on a refusal —
    /// nothing in the setup or the teardown has a failure the load numbers
    /// would still mean something without.
    fn ok(&self, method: &str, params: Value) -> Value {
        let answered = self
            .handler
            .call(self.sender.clone(), request(method, params));
        assert_eq!(answered["ok"], true, "{method}: {answered:?}");
        answered["result"].clone()
    }
}

/// One caller repeating one method until a deadline, and how long each of its
/// replies took.
///
/// It runs on a thread of its own with the test's runtime entered, because
/// that is what a relay dispatch worker is: a blocking thread inside the
/// runtime, calling the handler and waiting for the answer.
struct Callers {
    thread: std::thread::JoinHandle<Latencies>,
}

impl Callers {
    fn spawn(
        handler: &FrameHandler,
        method: &'static str,
        every: Duration,
        until: Instant,
        mut params: impl FnMut(u64) -> Value + Send + 'static,
    ) -> Callers {
        let handler = handler.clone();
        let runtime = tokio::runtime::Handle::current();
        let session_id = format!("s-{method}");
        Callers {
            thread: std::thread::spawn(move || {
                let _in_runtime = runtime.enter();
                let sender = SessionSender::detached(session_id);
                let mut samples = Vec::new();
                let mut n = 0;
                while Instant::now() < until {
                    let frame = request(method, params(n));
                    let asked_at = Instant::now();
                    let answered = handler.call(sender.clone(), frame);
                    samples.push(asked_at.elapsed());
                    assert_eq!(answered["ok"], true, "{method}: {answered:?}");
                    n += 1;
                    std::thread::sleep(every);
                }
                Latencies {
                    method: method.to_string(),
                    samples,
                }
            }),
        }
    }

    fn join(self) -> Latencies {
        self.thread.join().expect("a caller ran to its deadline")
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
        assert!(
            self.samples.len() >= 8,
            "{} was called {} times, too few for a percentile",
            self.method,
            self.samples.len()
        );
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
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.email", "t@build.ing"]);
    git(&["config", "user.name", "T"]);
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
