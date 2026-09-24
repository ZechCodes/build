//! A real bridge behind real sessions, driven one JSON line at a time.
//!
//! For a client test (the SPA's sync layer under vitest) that has to run
//! against the bridge itself rather than a mock of it: the real `AppState`
//! handler over a temp repo, with the daemon's QA agent as the harness, behind
//! the real `FrameIntake`. Every session rides a `ChannelWire` of its own — the
//! DataChannel carrier a browser's session rides — so a reply, a receipt and a
//! `changes` push reach the test exactly as they reach a browser: encrypted to
//! that session's key, and decrypted here.
//!
//! Run from `bridge/`: `cargo run -q --example session_bridge`.
//!
//! **In** (stdin, one object per line):
//!
//! - `{"op":"open","session":"s1"}` — mint a session key, wrap it to the
//!   device, open the session on a wire of its own.
//! - `{"op":"call","session":"s1","id":7,"method":"board.list","params":{}}` —
//!   one encrypted request frame on that session's wire.
//! - `{"op":"close","session":"s1"}` — the wire drops, the way a browser's
//!   connection does: the session ends and its subscriptions die with it.
//! - `{"op":"direct","id":7,"method":"thread.post","params":{}}` — the same
//!   handler, called on a detached sender that belongs to no carrier session:
//!   setup and mutations that must not come from a connected client.
//!
//! **Out** (stdout, one object per line):
//!
//! - `{"ready":true,"project_id":…,"run_id":…,"agent_id":…,"conversation_id":…}`
//!   once, when the run is adopted and its agent is live and working (setup
//!   posted one message, operation `setup-start`, and waited for it to be
//!   `sent`). `run_id` is the `entity_id` every thread verb takes.
//! - `{"session":"s1","opened":true}` when the device's `session_accept`
//!   verified under the session key.
//! - `{"session":"s1","frame":{…}}` for every frame the device sent that
//!   session: the admission receipt (`{"id":7,"accepted":true}`), the reply
//!   (`{"id":7,"ok":…}`), and every push (`frame.type == "changes"`).
//! - `{"session":"s1","closed":true}` once the bridge has handled the session's
//!   end — its subscriptions are gone by then.
//! - `{"direct":true,"id":7,"reply":{…}}` for a direct call.
//! - `{"error":"…"}` for a line the harness could not act on.
//!
//! **A loaded message that changes while nobody is connected.** Everything
//! below is the bridge's own RPCs and delivery queue; nothing is poked.
//!
//! 1. `direct agent.choose {entity_id, agent_id, conversation_id, model:
//!    "claude-opus-5", effort: "high"}` — the running session was started on
//!    another model.
//! 2. `direct thread.post {entity_id, agent_id, body, operation_id: "op-b"}` —
//!    B lands `delivery_status: "queued"`, and stays queued: its turn needs a
//!    fresh session and the agent is working, so the delivery preflight defers
//!    it (`app/runtime/delivery/preflight.rs`).
//! 3. A session loads the conversation, then `close`s.
//! 4. `direct thread.post {…, operation_id: "op-c", interrupt: true}` — C
//!    replaces the session, and the requeued B is delivered: B turns `"sent"`
//!    in place (same `sequence`, higher `updated_sequence`) and C and the
//!    session events land after it, so B is below the new tail.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use build_bridge::app::AppState;
use build_bridge::carrier::testing::{self, ChannelWire};
use build_bridge::carrier::{FrameHandler, FrameIntake, OutboundEnvelope, SessionSender};
use build_bridge::timing::FrameClock;
use build_bridge::transport::{self, Frame, CLOSE_FRAME_TYPE, DATA_FRAME_TYPE};
use serde_json::{json, Value};
use tokio::sync::mpsc;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut bridge = Bridge::start(dir.path());
    let ready = bridge.setup().await;
    emit(ready);
    bridge.serve(stdin_lines()).await;
    drop(bridge);
    drop(dir);
    std::process::exit(0);
}

/// One line to stdout. Rust's stdout is line-buffered, but a pipe reader
/// waiting on a line must not wait on a buffer, so every line is flushed.
fn emit(line: Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// The bridge and the sessions the test has open on it.
struct Bridge {
    dir: PathBuf,
    handler: FrameHandler,
    intake: Arc<FrameIntake>,
    sessions: HashMap<String, Session>,
}

/// One client session: its key and the wire only it rides.
struct Session {
    key: String,
    wire: ChannelWire,
}

impl Bridge {
    fn start(dir: &Path) -> Bridge {
        let repo = init_repo(dir);
        let context = build_bridge::harness::HarnessContext::resolved(
            dir.join("mcp.sock"),
            dir.to_path_buf(),
        )
        .expect("a private harness context");
        let state = AppState::new_configured(&repo, dir.join("worktrees"), "main", true, context)
            .with_task_store(dir.join("state"))
            .expect("a task store")
            .shared();
        let handler = announcing_closes(AppState::handler(Arc::clone(&state)));
        let intake = FrameIntake::new(handler.clone(), transport::generate_transport_keypair());
        Bridge {
            dir: dir.to_path_buf(),
            handler,
            intake,
            sessions: HashMap::new(),
        }
    }

    /// Adopt the loose checkout as a run and put its agent to work: one
    /// message, delivered, which spawns the QA agent and leaves it working —
    /// the state a later message waits behind (see the module docs). Then say
    /// what the test needs to address the conversation.
    async fn setup(&self) -> Value {
        let project_id = self.direct_ok("project.list", json!({})).await["projects"][0]
            ["project_id"]
            .as_str()
            .expect("the repo is registered as a project")
            .to_string();
        let worktree_id = build_bridge::worktree::external_worktree_id(
            &build_bridge::worktree::canonical_root(&self.dir.join("loose")),
        );
        let adopted = self
            .direct_ok(
                "run.adopt",
                json!({ "project_id": project_id, "worktree_id": worktree_id }),
            )
            .await;
        let run_id = adopted["run_id"]
            .as_str()
            .expect("adopting a checkout mints a run")
            .to_string();
        let posted = self
            .direct_ok(
                "thread.post",
                json!({
                    "entity_id": run_id,
                    "operation_id": SETUP_OPERATION,
                    "body": "Start working.",
                }),
            )
            .await;
        let agent = posted["agents"][0].clone();
        self.until_sent(&run_id, SETUP_OPERATION).await;
        json!({
            "ready": true,
            "project_id": project_id,
            "run_id": run_id,
            "agent_id": agent["id"],
            "conversation_id": agent["conversation_id"],
        })
    }

    /// Wait for the agent to have been handed the message `operation_id`
    /// posted — by then it is live, and marked working.
    async fn until_sent(&self, run_id: &str, operation_id: &str) {
        let deadline = tokio::time::Instant::now() + testing::PATIENCE;
        while tokio::time::Instant::now() < deadline {
            let page = self
                .direct_ok("thread.page", json!({ "entity_id": run_id, "limit": 20 }))
                .await;
            let sent = page["items"].as_array().into_iter().flatten().any(|item| {
                item["data"]["operation_id"] == operation_id
                    && item["data"]["delivery_status"] == "sent"
            });
            if sent {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("the QA agent was never handed {operation_id}");
    }

    /// The handler, called the way a dispatch worker calls it: on a blocking
    /// thread inside the runtime, for a sender no carrier session owns.
    async fn direct(&self, method: &str, params: Value) -> Value {
        let handler = self.handler.clone();
        let frame = request(method, params);
        tokio::task::spawn_blocking(move || {
            handler.call(SessionSender::detached(DIRECT_SESSION), frame)
        })
        .await
        .expect("the handler answered")
    }

    async fn direct_ok(&self, method: &str, params: Value) -> Value {
        let answered = self.direct(method, params).await;
        assert_eq!(answered["ok"], true, "{method}: {answered}");
        answered["result"].clone()
    }

    /// Act on each line in turn until stdin closes.
    async fn serve(&mut self, mut lines: mpsc::UnboundedReceiver<String>) {
        while let Some(line) = lines.recv().await {
            if line.trim().is_empty() {
                continue;
            }
            if let Err(error) = self.act_on(&line).await {
                emit(json!({ "error": error, "line": line }));
            }
        }
    }

    async fn act_on(&mut self, line: &str) -> Result<(), String> {
        let op: Value = serde_json::from_str(line).map_err(|error| error.to_string())?;
        let session = op["session"].as_str().unwrap_or_default().to_string();
        match op["op"].as_str().unwrap_or_default() {
            "open" => self.open(&session),
            "call" => self.call(&session, &op).await,
            "close" => self.close(&session),
            "direct" => {
                let method = op["method"].as_str().unwrap_or_default();
                let reply = self.direct(method, op["params"].clone()).await;
                emit(json!({ "direct": true, "id": op["id"], "reply": reply }));
                Ok(())
            }
            other => Err(format!("unknown op {other:?}")),
        }
    }

    fn open(&mut self, session_id: &str) -> Result<(), String> {
        if session_id.is_empty() || self.sessions.contains_key(session_id) {
            return Err(format!("session {session_id:?} is empty or already open"));
        }
        let key = transport::generate_session_key();
        let mut wire = ChannelWire::open();
        let (_, idle) = mpsc::unbounded_channel();
        let outbound = std::mem::replace(&mut wire.outbound, idle);
        tokio::spawn(relay_to_stdout(
            session_id.to_string(),
            key.clone(),
            outbound,
        ));
        let init = testing::session_init(session_id, self.intake.transport_public_key(), &key);
        wire.open_session(&self.intake, session_id, &init)
            .map_err(|error| error.to_string())?;
        self.sessions
            .insert(session_id.to_string(), Session { key, wire });
        Ok(())
    }

    async fn call(&self, session_id: &str, op: &Value) -> Result<(), String> {
        let session = self
            .sessions
            .get(session_id)
            .ok_or_else(|| format!("session {session_id:?} is not open"))?;
        let payload = json!({
            "id": op["id"],
            "method": op["method"],
            "params": op.get("params").cloned().unwrap_or_else(|| json!({})),
        });
        let envelope = testing::client_request(&session.key, session_id, DATA_FRAME_TYPE, payload);
        session
            .wire
            .accept(&self.intake, envelope)
            .await
            .map_err(|error| error.to_string())
    }

    fn close(&mut self, session_id: &str) -> Result<(), String> {
        let session = self
            .sessions
            .remove(session_id)
            .ok_or_else(|| format!("session {session_id:?} is not open"))?;
        session.wire.close(&self.intake);
        Ok(())
    }
}

/// The app's own handler, saying on stdout when it has handled a session's
/// end — the moment that session's subscriptions are gone. The frame goes to
/// the app unchanged; only the line after it is added.
fn announcing_closes(app: FrameHandler) -> FrameHandler {
    FrameHandler::new(FrameClock::new(), move |sender, frame, _timer| {
        let closing = frame.frame_type == CLOSE_FRAME_TYPE;
        let session_id = sender.session_id().to_string();
        let answered = app.call(sender, frame);
        if closing {
            emit(json!({ "session": session_id, "closed": true }));
        }
        answered
    })
}

/// Everything the device sends one session, decrypted under its key.
async fn relay_to_stdout(
    session_id: String,
    key: String,
    mut outbound: mpsc::UnboundedReceiver<OutboundEnvelope>,
) {
    while let Some(sent) = outbound.recv().await {
        emit(match sent {
            OutboundEnvelope::SessionAccept { envelope, .. } => {
                match transport::verify_session_accept(&key, &envelope, &session_id) {
                    Ok(()) => json!({ "session": session_id, "opened": true }),
                    Err(error) => json!({ "session": session_id, "error": error.to_string() }),
                }
            }
            OutboundEnvelope::Frame(envelope) => match transport::decrypt_envelope(&key, &envelope)
            {
                Ok(frame) => json!({ "session": session_id, "frame": frame.payload }),
                Err(error) => json!({ "session": session_id, "error": error.to_string() }),
            },
        });
    }
}

/// The operation the setup message is posted under.
const SETUP_OPERATION: &str = "setup-start";

/// The session a direct call is made for: one no carrier has open, so
/// nothing the handler pushes to it goes anywhere.
const DIRECT_SESSION: &str = "harness-direct";

/// stdin, line by line, off a thread of its own: a blocking read has no
/// business on a runtime worker.
fn stdin_lines() -> mpsc::UnboundedReceiver<String> {
    let (lines, read) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            let Ok(line) = line else { return };
            if lines.send(line).is_err() {
                return;
            }
        }
    });
    read
}

fn request(method: &str, params: Value) -> Frame {
    Frame {
        session_id: DIRECT_SESSION.into(),
        message_id: "m".into(),
        frame_type: DATA_FRAME_TYPE.into(),
        sender: transport::SENDER_CLIENT.into(),
        created_at: "t".into(),
        payload: json!({ "method": method, "id": "direct", "params": params }),
    }
}

/// A repository with one commit, and a checkout beside it for the run: work
/// happens in a checkout beside the repository, never in it.
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
    std::fs::write(repo.join("README.md"), "# session bridge\n").expect("a readme");
    git(&["add", "."]);
    git(&["commit", "-m", "initial"]);
    git(&[
        "worktree",
        "add",
        "-b",
        "loose",
        parent.join("loose").to_str().expect("a utf-8 path"),
    ]);
    repo
}
