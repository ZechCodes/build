//! The application layer: an orchestrator-backed RPC carried over the E2EE relay.
//!
//! This is what the browser actually talks to. A decrypted request frame carries
//! `{ "method": "task.dispatch", "id": "...", "params": {...} }`; the handler runs
//! the corresponding [`Orchestrator`] operation and returns
//! `{ "id": "...", "ok": true, "result": {...} }` (or `ok: false` + `error`).
//!
//! For local QA without a real LLM, a deterministic *scripted agent* stands in for
//! the harness: on dispatch/approval it writes the plan/code files an agent would
//! and reports `done`, so the whole lifecycle (plan → build → review → merge) runs
//! end to end. In production the harness is a real CLI in a PTY reporting `done`
//! over MCP; the orchestrator code path is identical.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use portable_pty::PtySize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;

use crate::mcp::{DoneOutputs, DonePhase, DoneReport, DoneStatus};
use crate::orchestrator::{ActiveTask, Orchestrator, OrchestratorError};
use crate::pty::{HarnessSpec, PtySession};
use crate::relay::{FrameHandler, SessionSender};
use crate::task::{TaskId, TaskKind, TaskState};
use crate::templates::{Templates, DEFAULT_PLAN_PATH};
use crate::transport::Frame;

/// A single event in a stream's authoritative log. `seq` is 1-based and dense.
#[derive(Debug, Clone)]
struct LogEvent {
    seq: u64,
    kind: String,
    data: Value,
}

/// The authoritative state of one agent output stream. Keyed by stream id (not by
/// transport session), so it survives client *and* bridge reconnects — the client
/// resumes by asking for events since the last seq it applied.
struct StreamState {
    count: u64,
    events: Vec<LogEvent>,
    complete: bool,
}

/// A live terminal session: a real PTY, an authoritative server-side screen model
/// (`vt100`), and the clients currently attached for live output. The screen model
/// is what makes reconnect a *snapshot* (current screen) rather than a byte replay.
struct TermSession {
    session: PtySession,
    parser: vt100::Parser,
    attached: Vec<SessionSender>,
    /// Output coalescing buffer: PTY bytes accumulate here and flush on a timer,
    /// so a repaint becomes one frame instead of ten.
    pending: Vec<u8>,
    /// Total output bytes processed — the live-tail cursor.
    total: u64,
    cols: u16,
    rows: u16,
}

/// Flush coalesced terminal output at ~100 fps.
const TERM_FLUSH_MS: u64 = 10;
/// If a single flush exceeds this, send the current screen snapshot instead of
/// the raw byte backlog — collapses a massive burst (scroll/flood) to one frame
/// and bounds per-frame size. The vt100 model makes this lossless for the screen.
const TERM_SNAPSHOT_THRESHOLD: usize = 128 * 1024;

impl TermSession {
    /// Spawn an interactive shell in a PTY, returning the session and a receiver
    /// for its output (subscribed immediately so no early bytes are missed).
    fn spawn(cols: u16, rows: u16) -> Result<(TermSession, broadcast::Receiver<Vec<u8>>), String> {
        let spec = HarnessSpec::new("bash")
            .arg("--norc")
            .arg("-i")
            .env("TERM", "xterm-256color")
            .env("PS1", "build$ ");
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let session = PtySession::spawn(&spec, None, size).map_err(|e| e.to_string())?;
        let rx = session.subscribe();
        let parser = vt100::Parser::new(rows, cols, 2000);
        Ok((
            TermSession {
                session,
                parser,
                attached: Vec::new(),
                pending: Vec::new(),
                total: 0,
                cols,
                rows,
            },
            rx,
        ))
    }

    /// The current screen serialized as escape sequences — write it to a fresh
    /// terminal and the screen is reproduced.
    fn snapshot(&self) -> String {
        b64encode(&self.parser.screen().contents_formatted())
    }
}

/// Shared application state behind the relay handler.
pub struct AppState {
    orch: Orchestrator,
    base_branch: String,
    tasks: HashMap<String, ActiveTask>,
    streams: HashMap<String, StreamState>,
    term: Option<TermSession>,
    next_id: u64,
    next_stream: u64,
    /// When true, simulate the agent deterministically (local QA, no LLM).
    qa_agent: bool,
}

impl AppState {
    pub fn new(
        repo_path: impl Into<std::path::PathBuf>,
        worktrees_root: impl Into<std::path::PathBuf>,
        base_branch: impl Into<String>,
        qa_agent: bool,
    ) -> Self {
        // A warm no-op harness; in QA the scripted agent does the file writing.
        let harness = HarnessSpec::new("sh").arg("-c").arg("sleep 86400");
        let orch = Orchestrator::new(repo_path, worktrees_root, harness, Templates::default());
        AppState {
            orch,
            base_branch: base_branch.into(),
            tasks: HashMap::new(),
            streams: HashMap::new(),
            term: None,
            next_id: 1,
            next_stream: 1,
            qa_agent,
        }
    }

    /// Wrap this state in the relay's frame handler. `stream.start`/`term.attach`
    /// need the shared handle (they spawn background producers/pumps), so the
    /// handler dispatches through [`dispatch_frame`].
    pub fn into_handler(self) -> FrameHandler {
        let state = Arc::new(Mutex::new(self));
        Arc::new(move |sender, frame| dispatch_frame(&state, sender, frame))
    }

    /// Synchronous dispatch used by the unit tests (no background producer). The
    /// running handler goes through [`dispatch_frame`].
    #[cfg(test)]
    fn handle(&mut self, frame: Frame) -> Value {
        let id = frame.payload.get("id").cloned().unwrap_or(Value::Null);
        let method = frame
            .payload
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let params = frame
            .payload
            .get("params")
            .cloned()
            .unwrap_or_else(|| json!({}));

        match self.dispatch(&method, &params) {
            Ok(result) => json!({ "id": id, "ok": true, "result": result }),
            Err(message) => json!({ "id": id, "ok": false, "error": message }),
        }
    }

    fn dispatch(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        match method {
            "ping" => Ok(json!({ "pong": true })),
            "task.dispatch" => self.task_dispatch(params),
            "task.list" => Ok(self.task_list()),
            "task.get" => self.task_get(params),
            "task.plan" => self.task_plan(params),
            "task.diff" => self.task_diff(params),
            "task.approve_plan" => self.task_approve_plan(params),
            "task.approve_merge" => self.task_approve_merge(params),
            "task.abandon" => self.task_abandon(params),
            "stream.events" => self.stream_events(params),
            "stream.state" => self.stream_state(params),
            "term.input" => self.term_input(params),
            "term.resize" => self.term_resize(params),
            other => Err(format!("unknown method: {other}")),
        }
    }

    /// Write client keystrokes (base64) to the terminal's PTY.
    fn term_input(&mut self, params: &Value) -> Result<Value, String> {
        let data = b64decode(&require_str(params, "data")?)?;
        let term = self.term.as_ref().ok_or("no terminal session")?;
        term.session.write_input(&data).map_err(|e| e.to_string())?;
        Ok(json!({ "ok": true }))
    }

    /// Resize the terminal's PTY and screen model.
    fn term_resize(&mut self, params: &Value) -> Result<Value, String> {
        let cols = params.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16;
        let rows = params.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16;
        let term = self.term.as_mut().ok_or("no terminal session")?;
        term.session
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;
        term.parser.set_size(rows, cols);
        term.cols = cols;
        term.rows = rows;
        Ok(json!({ "ok": true }))
    }

    /// Return a bounded batch of events with `seq > since` for resume. The batch
    /// is capped (`limit`, default 64) so a client far behind catches up in
    /// bounded chunks rather than one giant replay — proper reconnect load.
    fn stream_events(&mut self, params: &Value) -> Result<Value, String> {
        let stream_id = require_str(params, "stream_id")?;
        let since = params.get("since").and_then(Value::as_u64).unwrap_or(0);
        let limit = params
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(64)
            .clamp(1, 256) as usize;

        let stream = self.streams.get(&stream_id).ok_or("unknown stream_id")?;
        let head = stream.events.last().map(|e| e.seq).unwrap_or(0);

        let batch: Vec<&LogEvent> = stream
            .events
            .iter()
            .filter(|e| e.seq > since)
            .take(limit)
            .collect();
        let next = batch.last().map(|e| e.seq).unwrap_or(since);
        let events: Vec<Value> = batch
            .iter()
            .map(|e| json!({ "seq": e.seq, "kind": e.kind, "data": e.data }))
            .collect();

        Ok(json!({
            "events": events,
            "next": next,
            "head": head,
            "complete": stream.complete,
        }))
    }

    /// The authoritative summary of a stream: head seq, completion, and a checksum
    /// over the full concatenated output the client can compare against.
    fn stream_state(&mut self, params: &Value) -> Result<Value, String> {
        let stream_id = require_str(params, "stream_id")?;
        let stream = self.streams.get(&stream_id).ok_or("unknown stream_id")?;
        let output = concat_output(&stream.events);
        Ok(json!({
            "stream_id": stream_id,
            "count": stream.count,
            "head": stream.events.last().map(|e| e.seq).unwrap_or(0),
            "complete": stream.complete,
            "output_len": output.len(),
            "checksum": sha256_hex(output.as_bytes()),
        }))
    }

    fn task_dispatch(&mut self, params: &Value) -> Result<Value, String> {
        let goal = require_str(params, "goal")?;
        let kind = match params.get("kind").and_then(Value::as_str) {
            Some("quick") => TaskKind::Quick,
            _ => TaskKind::Standard,
        };
        let task_id = format!("task-{}", self.next_id);
        self.next_id += 1;

        let mut active = self
            .orch
            .dispatch(TaskId::new(&task_id), goal, kind, &self.base_branch)
            .map_err(err)?;

        // Scripted agent: produce the plan (standard) or the code (quick) and
        // report done, exactly as a real harness would over MCP.
        if self.qa_agent {
            match active.task.state {
                TaskState::Planning => self.simulate_plan(&mut active)?,
                TaskState::Building => self.simulate_build(&mut active)?,
                _ => {}
            }
        }

        let view = self.task_view(&task_id, &active);
        self.tasks.insert(task_id.clone(), active);
        Ok(view)
    }

    fn task_approve_plan(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let mut active = self.take(&task_id)?;
        let outcome = (|| -> Result<(), String> {
            self.orch.approve_plan(&mut active).map_err(err)?;
            if self.qa_agent {
                self.simulate_build(&mut active)?;
            }
            Ok(())
        })();
        let view = self.task_view(&task_id, &active);
        self.tasks.insert(task_id, active);
        outcome?;
        Ok(view)
    }

    fn task_approve_merge(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let mut active = self.take(&task_id)?;
        let result = self.orch.approve_merge(&mut active).map_err(err);
        let view = self.task_view(&task_id, &active);
        self.tasks.insert(task_id, active);
        result?;
        Ok(view)
    }

    fn task_abandon(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let mut active = self.take(&task_id)?;
        let result = self.orch.abandon(&mut active).map_err(err);
        let view = self.task_view(&task_id, &active);
        self.tasks.insert(task_id, active);
        result?;
        Ok(view)
    }

    fn task_diff(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let active = self.tasks.get(&task_id).ok_or("unknown task_id")?;
        let diff = self.orch.diff(active).map_err(err)?;
        let files: Vec<Value> = diff
            .files()
            .iter()
            .map(|f| json!({ "path": f.path, "status": format!("{:?}", f.status) }))
            .collect();
        let stat = diff.stat();
        Ok(json!({
            "stat": {
                "files_changed": stat.files_changed,
                "insertions": stat.insertions,
                "deletions": stat.deletions,
            },
            "files": files,
            "patch": diff.patch(),
        }))
    }

    fn task_plan(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let active = self.tasks.get(&task_id).ok_or("unknown task_id")?;
        let path = active.worktree.path.join(&active.plan_path);
        let contents =
            std::fs::read_to_string(&path).map_err(|e| format!("plan not available: {e}"))?;
        Ok(json!({ "plan_path": active.plan_path, "contents": contents }))
    }

    fn task_get(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let active = self.tasks.get(&task_id).ok_or("unknown task_id")?;
        Ok(self.task_view(&task_id, active))
    }

    fn task_list(&self) -> Value {
        let tasks: Vec<Value> = self
            .tasks
            .iter()
            .map(|(id, active)| {
                json!({
                    "task_id": id,
                    "goal": active.task.goal,
                    "state": state_str(&active.task.state),
                    "needs_attention": active.task.state.needs_attention(),
                })
            })
            .collect();
        json!({ "tasks": tasks })
    }

    // --- the scripted QA agent ------------------------------------------------

    fn simulate_plan(&mut self, active: &mut ActiveTask) -> Result<(), String> {
        let plan = format!(
            "# Plan: {goal}\n\n1. Implement the goal.\n2. Add a result file.\n",
            goal = active.task.goal
        );
        write_in_worktree(active, DEFAULT_PLAN_PATH, &plan)?;
        self.orch
            .on_done(
                active,
                DoneReport {
                    phase: DonePhase::Plan,
                    status: DoneStatus::Completed,
                    summary: format!("Planned: {}", active.task.goal),
                    outputs: DoneOutputs {
                        plan_path: Some(DEFAULT_PLAN_PATH.to_string()),
                    },
                },
            )
            .map_err(err)
    }

    fn simulate_build(&mut self, active: &mut ActiveTask) -> Result<(), String> {
        let content = format!("Implemented: {}\n", active.task.goal);
        write_in_worktree(active, "result.txt", &content)?;
        self.orch
            .on_done(
                active,
                DoneReport {
                    phase: DonePhase::Build,
                    status: DoneStatus::Completed,
                    summary: format!("Built: {}", active.task.goal),
                    outputs: DoneOutputs::default(),
                },
            )
            .map_err(err)
    }

    // --- helpers --------------------------------------------------------------

    fn task_view(&self, task_id: &str, active: &ActiveTask) -> Value {
        json!({
            "task_id": task_id,
            "goal": active.task.goal,
            "state": state_str(&active.task.state),
            "needs_attention": active.task.state.needs_attention(),
            "branch": active.worktree.branch,
            "summary": active.last_summary,
        })
    }

    fn take(&mut self, task_id: &str) -> Result<ActiveTask, String> {
        self.tasks.remove(task_id).ok_or("unknown task_id".into())
    }
}

fn write_in_worktree(active: &ActiveTask, rel: &str, contents: &str) -> Result<(), String> {
    let path = active.worktree.path.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, contents).map_err(|e| e.to_string())
}

fn require_str(params: &Value, key: &str) -> Result<String, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing required param: {key}"))
}

fn err(e: OrchestratorError) -> String {
    e.to_string()
}

/// Dispatch one decrypted request frame. `stream.start` and `term.attach` are
/// handled here because they need the shared `Arc` (background producer/pump) and
/// the `SessionSender` (to push live output to this client); everything else runs
/// under a short-held lock.
fn dispatch_frame(state: &Arc<Mutex<AppState>>, sender: SessionSender, frame: Frame) -> Value {
    let id = frame.payload.get("id").cloned().unwrap_or(Value::Null);
    let method = frame
        .payload
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let params = frame
        .payload
        .get("params")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let result = match method.as_str() {
        "stream.start" => stream_start(state, &params),
        "term.attach" => term_attach(state, &sender, &params),
        _ => state.lock().unwrap().dispatch(&method, &params),
    };
    match result {
        Ok(result) => json!({ "id": id, "ok": true, "result": result }),
        Err(message) => json!({ "id": id, "ok": false, "error": message }),
    }
}

/// Attach this client to the terminal: create the session on first attach (and
/// start the output pump), register the caller's [`SessionSender`] for live
/// output, and return the current **screen snapshot** + cursor. Reconnect is just
/// another attach — a new session re-registers and gets a fresh snapshot.
fn term_attach(
    state: &Arc<Mutex<AppState>>,
    sender: &SessionSender,
    params: &Value,
) -> Result<Value, String> {
    let cols = params.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16;
    let rows = params.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16;

    let mut s = state.lock().unwrap();
    let mut new_output_rx = None;
    if s.term.is_none() {
        let (term, rx) = TermSession::spawn(cols, rows)?;
        s.term = Some(term);
        new_output_rx = Some(rx);
    }

    // Snapshot the screen and register this client atomically under the lock, so
    // the pump pushes only bytes *after* the cursor to the new sender — no gap, no
    // dupe across a reconnect.
    let term = s.term.as_mut().expect("term created above");
    // Drop any prior sender for this same session id (a reconnect on the same id).
    term.attached
        .retain(|snd| snd.session_id() != sender.session_id());
    term.attached.push(sender.clone());
    let response = json!({
        "snapshot": term.snapshot(),
        "cursor": term.total,
        "cols": term.cols,
        "rows": term.rows,
    });
    drop(s);

    // First attach starts the pump that feeds the screen model and fans output out
    // to every attached client.
    if let Some(rx) = new_output_rx {
        spawn_term_pump(Arc::clone(state), rx);
    }
    Ok(response)
}

/// Pump PTY output into the screen model, coalescing bytes and flushing one frame
/// per ~`TERM_FLUSH_MS` to every attached client. A huge burst collapses to a
/// screen snapshot so frame size/rate stay bounded and control frames (the
/// liveness ping) are never head-of-line-blocked behind megabytes of output.
fn spawn_term_pump(state: Arc<Mutex<AppState>>, mut rx: broadcast::Receiver<Vec<u8>>) {
    tokio::spawn(async move {
        let mut flush = tokio::time::interval(Duration::from_millis(TERM_FLUSH_MS));
        flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                recv = rx.recv() => match recv {
                    // Update the authoritative screen as bytes arrive; buffer raw
                    // bytes for the next flush.
                    Ok(chunk) => {
                        let mut s = state.lock().unwrap();
                        let Some(term) = s.term.as_mut() else { return; };
                        term.parser.process(&chunk);
                        term.total += chunk.len() as u64;
                        term.pending.extend_from_slice(&chunk);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return, // PTY closed
                },
                _ = flush.tick() => {
                    let mut s = state.lock().unwrap();
                    let Some(term) = s.term.as_mut() else { return; };
                    if term.pending.is_empty() { continue; }
                    let cursor = term.total;
                    let payload = if term.pending.len() > TERM_SNAPSHOT_THRESHOLD {
                        // Too much at once — skip the backlog, send the screen.
                        json!({ "type": "term.reset", "data": term.snapshot(), "cursor": cursor })
                    } else {
                        json!({ "type": "term.output", "data": b64encode(&term.pending), "cursor": cursor })
                    };
                    term.pending.clear();
                    term.attached.retain(|snd| snd.push(payload.clone()));
                }
            }
        }
    });
}

/// Start a deterministic agent output stream: register it, then spawn a background
/// producer that appends `count` ordered output events (one per `interval_ms`) and
/// a terminal `done` event into the authoritative log.
fn stream_start(state: &Arc<Mutex<AppState>>, params: &Value) -> Result<Value, String> {
    let count = params.get("count").and_then(Value::as_u64).unwrap_or(20);
    let interval_ms = params
        .get("interval_ms")
        .and_then(Value::as_u64)
        .unwrap_or(5)
        .clamp(0, 1000);

    let stream_id = {
        let mut s = state.lock().unwrap();
        let id = format!("stream-{}", s.next_stream);
        s.next_stream += 1;
        s.streams.insert(
            id.clone(),
            StreamState {
                count,
                events: Vec::new(),
                complete: false,
            },
        );
        id
    };

    let state = Arc::clone(state);
    let producer_id = stream_id.clone();
    tokio::spawn(async move {
        for i in 0..count {
            if interval_ms > 0 {
                tokio::time::sleep(Duration::from_millis(interval_ms)).await;
            }
            let mut s = state.lock().unwrap();
            let Some(stream) = s.streams.get_mut(&producer_id) else {
                return;
            };
            let seq = stream.events.len() as u64 + 1;
            stream.events.push(LogEvent {
                seq,
                kind: "output".into(),
                data: json!({ "index": i, "text": chunk_text(i) }),
            });
        }
        let mut s = state.lock().unwrap();
        if let Some(stream) = s.streams.get_mut(&producer_id) {
            let seq = stream.events.len() as u64 + 1;
            stream.events.push(LogEvent {
                seq,
                kind: "done".into(),
                data: json!({ "count": count }),
            });
            stream.complete = true;
        }
    });

    Ok(json!({ "stream_id": stream_id, "count": count }))
}

/// The deterministic text of output chunk `i`. Both the bridge and the client can
/// reproduce it independently, so the reconstructed output is verifiable.
fn chunk_text(i: u64) -> String {
    format!("chunk-{i:06}")
}

/// The authoritative output: every `output` event's text, in seq order, joined by
/// newlines. The client reconstructs the same string and compares checksums.
fn concat_output(events: &[LogEvent]) -> String {
    events
        .iter()
        .filter(|e| e.kind == "output")
        .filter_map(|e| e.data.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn b64encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn b64decode(s: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| format!("invalid base64: {e}"))
}

/// Render a task state as a stable snake_case string for the wire.
pub fn state_str(state: &TaskState) -> String {
    match state {
        TaskState::Created => "created".into(),
        TaskState::Planning => "planning".into(),
        TaskState::PlanReview => "plan_review".into(),
        TaskState::Building => "building".into(),
        TaskState::Review => "review".into(),
        TaskState::Blocked(_) => "blocked".into(),
        TaskState::Failed(_) => "failed".into(),
        TaskState::IdleUnreported(_) => "idle_unreported".into(),
        TaskState::Merged => "merged".into(),
        TaskState::Abandoned => "abandoned".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    fn init_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            assert!(Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap()
                .success());
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "t@build.ing"]);
        git(&["config", "user.name", "T"]);
        std::fs::write(repo.join("README.md"), "# project\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "initial"]);
        (dir, repo)
    }

    fn req(method: &str, params: Value) -> Frame {
        Frame {
            session_id: "s".into(),
            message_id: "m".into(),
            frame_type: "data".into(),
            sender: "client".into(),
            created_at: "t".into(),
            payload: json!({ "method": method, "id": "1", "params": params }),
        }
    }

    #[test]
    fn full_lifecycle_over_the_app_rpc() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(repo.clone(), dir.path().join("wt"), "main", true);

        // Dispatch a standard task → scripted plan → plan_review.
        let res = state.handle(req("task.dispatch", json!({ "goal": "add a greeting" })));
        assert_eq!(res["ok"], true);
        let task_id = res["result"]["task_id"].as_str().unwrap().to_string();
        assert_eq!(res["result"]["state"], "plan_review");

        // The plan is readable.
        let plan = state.handle(req("task.plan", json!({ "task_id": task_id })));
        assert!(plan["result"]["contents"]
            .as_str()
            .unwrap()
            .contains("add a greeting"));

        // Approve → scripted build → review, with a diff.
        let approved = state.handle(req("task.approve_plan", json!({ "task_id": task_id })));
        assert_eq!(approved["result"]["state"], "review");
        let diff = state.handle(req("task.diff", json!({ "task_id": task_id })));
        assert!(diff["result"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == "result.txt"));

        // Approve & merge → merged, and the base branch has the file.
        let merged = state.handle(req("task.approve_merge", json!({ "task_id": task_id })));
        assert_eq!(merged["result"]["state"], "merged");
        assert!(repo.join("result.txt").exists());
    }

    #[test]
    fn unknown_method_and_missing_params_error_cleanly() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(repo, dir.path().join("wt"), "main", true);
        assert_eq!(state.handle(req("nope", json!({})))["ok"], false);
        let missing = state.handle(req("task.dispatch", json!({})));
        assert_eq!(missing["ok"], false);
        assert!(missing["error"].as_str().unwrap().contains("goal"));
    }

    #[tokio::test]
    async fn stream_resume_reconstructs_full_output() {
        let (dir, repo) = init_repo();
        let handler = AppState::new(repo, dir.path().join("wt"), "main", true).into_handler();
        let call = |method: &str, params: Value| {
            handler(SessionSender::detached("s"), req(method, params))
        };

        let started = call("stream.start", json!({ "count": 50, "interval_ms": 0 }));
        let stream_id = started["result"]["stream_id"].as_str().unwrap().to_string();

        // Wait for the producer to finish.
        loop {
            let st = call("stream.state", json!({ "stream_id": stream_id }));
            if st["result"]["complete"] == true {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // Resume in bounded batches from seq 0 — exactly how a reconnecting client
        // catches up. Collect the output text in order.
        let mut since = 0u64;
        let mut texts = Vec::new();
        loop {
            let ev = call(
                "stream.events",
                json!({ "stream_id": stream_id, "since": since, "limit": 7 }),
            );
            for e in ev["result"]["events"].as_array().unwrap() {
                if e["kind"] == "output" {
                    texts.push(e["data"]["text"].as_str().unwrap().to_string());
                }
                // seqs are contiguous and strictly increasing.
            }
            let next = ev["result"]["next"].as_u64().unwrap();
            let head = ev["result"]["head"].as_u64().unwrap();
            let complete = ev["result"]["complete"].as_bool().unwrap();
            since = next;
            if complete && since >= head {
                break;
            }
        }

        // All 50 deterministic chunks, in order.
        assert_eq!(texts.len(), 50);
        assert_eq!(texts[0], "chunk-000000");
        assert_eq!(texts[49], "chunk-000049");

        // The client's reconstruction matches the bridge's authoritative checksum.
        let reconstructed = texts.join("\n");
        let st = call("stream.state", json!({ "stream_id": stream_id }));
        assert_eq!(
            st["result"]["checksum"].as_str().unwrap(),
            sha256_hex(reconstructed.as_bytes())
        );
    }

    #[tokio::test]
    async fn terminal_snapshot_reflects_input_across_reattach() {
        let (dir, repo) = init_repo();
        let handler = AppState::new(repo, dir.path().join("wt"), "main", true).into_handler();
        let call = |sid: &str, method: &str, params: Value| {
            handler(SessionSender::detached(sid), req(method, params))
        };

        // First attach: spawns the shell + the output pump.
        let a = call("s1", "term.attach", json!({ "cols": 80, "rows": 24 }));
        assert_eq!(a["ok"], true);
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send a command (the PTY echoes it and runs it).
        let input = b64encode(b"echo build-terminal-ok\n");
        call("s1", "term.input", json!({ "data": input }));
        tokio::time::sleep(Duration::from_millis(700)).await;

        // Reconnect = a fresh attach. The screen snapshot (vt100 model) must reflect
        // the prior output — that's snapshot-based resync, not byte replay.
        let b = call("s2", "term.attach", json!({ "cols": 80, "rows": 24 }));
        let snap =
            String::from_utf8_lossy(&b64decode(b["result"]["snapshot"].as_str().unwrap()).unwrap())
                .into_owned();
        assert!(
            snap.contains("build-terminal-ok"),
            "reattach snapshot should reflect prior output; got: {snap:?}"
        );
        assert!(b["result"]["cursor"].as_u64().unwrap() > 0);
    }
}
