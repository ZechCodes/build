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

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::mcp::{DoneOutputs, DonePhase, DoneReport, DoneStatus};
use crate::orchestrator::{ActiveTask, Orchestrator, OrchestratorError};
use crate::pty::HarnessSpec;
use crate::relay::FrameHandler;
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

/// Shared application state behind the relay handler.
pub struct AppState {
    orch: Orchestrator,
    base_branch: String,
    tasks: HashMap<String, ActiveTask>,
    streams: HashMap<String, StreamState>,
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
            next_id: 1,
            next_stream: 1,
            qa_agent,
        }
    }

    /// Wrap this state in the relay's frame handler. `stream.start` needs the
    /// shared handle (it spawns a background producer), so the handler dispatches
    /// through [`dispatch_frame`].
    pub fn into_handler(self) -> FrameHandler {
        let state = Arc::new(Mutex::new(self));
        Arc::new(move |_session_id, frame| dispatch_frame(&state, frame))
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
            other => Err(format!("unknown method: {other}")),
        }
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

/// Dispatch one decrypted request frame. `stream.start` is handled here because
/// it needs the shared `Arc` to hand to its background producer; everything else
/// runs under a short-held lock.
fn dispatch_frame(state: &Arc<Mutex<AppState>>, frame: Frame) -> Value {
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

    let result = if method == "stream.start" {
        stream_start(state, &params)
    } else {
        state.lock().unwrap().dispatch(&method, &params)
    };
    match result {
        Ok(result) => json!({ "id": id, "ok": true, "result": result }),
        Err(message) => json!({ "id": id, "ok": false, "error": message }),
    }
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
        let call = |method: &str, params: Value| handler("s".to_string(), req(method, params));

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
}
