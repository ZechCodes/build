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

use tokio::io::AsyncBufReadExt;

use crate::mcp::{DoneOutputs, DonePhase, DoneReport, DoneStatus};
use crate::orchestrator::{ActiveTask, Agent, Orchestrator, OrchestratorError};
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

/// One registered project: a git repo, its base branch, and the orchestrator that
/// drives tasks on it. Each project gets its own worktrees subdir and orchestrator
/// so tasks on different repos never interact.
struct Project {
    id: String,
    name: String,
    repo_path: std::path::PathBuf,
    base_branch: String,
    orch: Orchestrator,
}

/// Build the harness adapter shared by every project's orchestrator: the
/// deterministic scripted agent for QA, or a one-shot `claude` headless run for
/// real work. The closure is shared (Arc) across projects via `Agent: Clone`.
fn build_agent(qa_agent: bool, mcp_socket: String) -> Agent {
    if qa_agent {
        // A warm no-op harness; the scripted agent does the file writing.
        Agent::Warm(HarnessSpec::new("sh").arg("-c").arg("sleep 86400"))
    } else {
        // Real agent: a one-shot `claude` headless run with the rendered prompt
        // baked in, the per-task `done` MCP server wired via .build/mcp.json, and
        // the daemon's control socket so its `done` reaches on_agent_done.
        Agent::OneShot(Arc::new(move |prompt: &str| {
            HarnessSpec::new("claude")
                .arg("-p")
                .arg(prompt)
                .arg("--mcp-config")
                .arg(".build/mcp.json")
                .arg("--strict-mcp-config")
                .arg("--dangerously-skip-permissions")
                .env("BRIDGE_MCP_SOCKET", &mcp_socket)
        }))
    }
}

/// Shared application state behind the relay handler.
pub struct AppState {
    /// Registered projects (repos) tasks can be dispatched to.
    projects: Vec<Project>,
    /// task id → the project it was dispatched to (routes approve/diff/merge/done).
    task_project: HashMap<String, String>,
    worktrees_root: std::path::PathBuf,
    /// Where cloned repos land and the directory browser starts; user-configurable.
    projects_dir: std::path::PathBuf,
    /// Where to persist the projects + settings, if persistence is enabled.
    config_path: Option<std::path::PathBuf>,
    agent: Agent,
    harness: String,
    tasks: HashMap<String, ActiveTask>,
    streams: HashMap<String, StreamState>,
    term: Option<TermSession>,
    next_id: u64,
    next_stream: u64,
    next_project: u64,
    /// When true, simulate the agent deterministically (local QA, no LLM).
    qa_agent: bool,
}

impl AppState {
    pub fn new(
        repo_path: impl Into<std::path::PathBuf>,
        worktrees_root: impl Into<std::path::PathBuf>,
        base_branch: impl Into<String>,
        qa_agent: bool,
        mcp_socket: impl Into<String>,
    ) -> Self {
        let harness = if qa_agent { "QA agent" } else { "Claude Code" }.to_string();
        let mut state = AppState {
            projects: Vec::new(),
            task_project: HashMap::new(),
            worktrees_root: worktrees_root.into(),
            projects_dir: default_projects_dir(),
            config_path: None,
            agent: build_agent(qa_agent, mcp_socket.into()),
            harness,
            tasks: HashMap::new(),
            streams: HashMap::new(),
            term: None,
            next_id: 1,
            next_stream: 1,
            next_project: 1,
            qa_agent,
        };
        state.add_project(repo_path.into(), base_branch.into());
        state
    }

    /// Enable persistence at `path`: load any saved projects + projects-dir from it
    /// (skipping repos that no longer exist), and remember it for future writes.
    pub fn with_config(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        let path = path.into();
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<Value>(&text) {
                if let Some(dir) = cfg.get("projects_dir").and_then(Value::as_str) {
                    self.projects_dir = expand_tilde(dir);
                }
                for p in cfg
                    .get("projects")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(repo) = p.get("path").and_then(Value::as_str) {
                        let base = p
                            .get("base_branch")
                            .and_then(Value::as_str)
                            .unwrap_or("main")
                            .to_string();
                        let repo = std::path::PathBuf::from(repo);
                        if repo.exists() {
                            self.add_project(repo, base);
                        }
                    }
                }
            }
        }
        self.config_path = Some(path);
        self
    }

    /// Override where cloned repos land and the browser starts (e.g. from an env).
    pub fn set_projects_dir(&mut self, dir: std::path::PathBuf) {
        self.projects_dir = dir;
    }

    /// Persist projects + projects-dir to the config file, if one is configured.
    fn persist(&self) {
        let Some(path) = &self.config_path else {
            return;
        };
        let cfg = json!({
            "projects_dir": self.projects_dir.display().to_string(),
            "projects": self.projects.iter().map(|p| json!({
                "path": p.repo_path.display().to_string(),
                "base_branch": p.base_branch,
            })).collect::<Vec<_>>(),
        });
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, serde_json::to_string_pretty(&cfg).unwrap_or_default());
    }

    /// Register a project (repo + base branch) and return its id. Idempotent: a
    /// repo already registered (by canonical path) returns its existing id. Each
    /// project gets an isolated worktrees subdir keyed by id.
    pub fn add_project(&mut self, repo_path: std::path::PathBuf, base_branch: String) -> String {
        let repo_path = std::fs::canonicalize(&repo_path).unwrap_or(repo_path);
        if let Some(existing) = self.projects.iter().find(|p| p.repo_path == repo_path) {
            return existing.id.clone();
        }
        let id = format!("proj-{}", self.next_project);
        self.next_project += 1;
        let name = repo_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("project")
            .to_string();
        let worktrees = self.worktrees_root.join(&id);
        let orch = Orchestrator::new(
            repo_path.clone(),
            worktrees,
            self.agent.clone(),
            Templates::default(),
        );
        self.projects.push(Project {
            id: id.clone(),
            name,
            repo_path,
            base_branch,
            orch,
        });
        id
    }

    /// Share this state so the relay handler and the done-socket listener both
    /// drive the same tasks.
    pub fn shared(self) -> Arc<Mutex<AppState>> {
        Arc::new(Mutex::new(self))
    }

    /// The relay's frame handler over a shared state. `stream.start`/`term.attach`
    /// need the shared handle (background producers/pumps), so it dispatches
    /// through [`dispatch_frame`].
    pub fn handler(state: Arc<Mutex<AppState>>) -> FrameHandler {
        Arc::new(move |sender, frame| dispatch_frame(&state, sender, frame))
    }

    /// Convenience for tests: own the state and build a handler in one step.
    pub fn into_handler(self) -> FrameHandler {
        Self::handler(self.shared())
    }

    /// Listen on the daemon control socket for `done` reports forwarded by the
    /// per-task `build-bridge mcp` servers, and route each to its task's `on_done`.
    pub fn spawn_done_socket(state: Arc<Mutex<AppState>>, path: String) {
        tokio::spawn(async move {
            if let Some(parent) = std::path::Path::new(&path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::remove_file(&path);
            let listener = match tokio::net::UnixListener::bind(&path) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("done socket: bind {path} failed: {e}");
                    return;
                }
            };
            eprintln!("done socket: listening on {path}");
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    let mut lines = tokio::io::BufReader::new(stream).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let Ok(v) = serde_json::from_str::<Value>(&line) else {
                            continue;
                        };
                        let task_id = v.get("task_id").and_then(Value::as_str).unwrap_or("");
                        if let Ok(report) = serde_json::from_value::<DoneReport>(
                            v.get("report").cloned().unwrap_or(Value::Null),
                        ) {
                            state.lock().unwrap().on_agent_done(task_id, report);
                        }
                    }
                });
            }
        });
    }

    /// Route an agent's `done` to its task's lifecycle transition, on that task's
    /// project orchestrator.
    fn on_agent_done(&mut self, task_id: &str, report: DoneReport) {
        let Some(mut active) = self.tasks.remove(task_id) else {
            eprintln!("on_agent_done: unknown task {task_id}");
            return;
        };
        let outcome = match self.project_of(task_id) {
            Ok(pid) => match self.orch_for(&pid) {
                Ok(orch) => orch.on_done(&mut active, report).map_err(err),
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        };
        if let Err(e) = outcome {
            eprintln!("on_agent_done {task_id}: {e}");
        }
        self.tasks.insert(task_id.to_string(), active);
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
            "fs.list" => self.fs_list(params),
            "settings.get" => Ok(self.settings_get()),
            "settings.set" => self.settings_set(params),
            "project.list" => Ok(self.project_list()),
            "project.add" => self.project_add(params),
            "project.create" => self.project_create(params),
            "project.clone" => self.project_clone(params),
            "project.set_remote" => self.project_set_remote(params),
            "task.dispatch" => self.task_dispatch(params),
            "task.list" => Ok(self.task_list()),
            "task.get" => self.task_get(params),
            "task.plan" => self.task_plan(params),
            "task.diff" => self.task_diff(params),
            "task.approve_plan" => self.task_approve_plan(params),
            "task.send_notes" => self.task_send_notes(params),
            "task.request_changes" => self.task_request_changes(params),
            "task.approve_merge" => self.task_approve_merge(params),
            "task.git_action" => self.task_git_action(params),
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

    /// All registered projects, for the New-task picker and Settings.
    fn project_list(&self) -> Value {
        let projects: Vec<Value> = self.projects.iter().map(project_json).collect();
        json!({ "projects": projects })
    }

    /// Register a project from a host path. Validates it is a git repo with the
    /// requested base branch before adding, so a bad path fails loudly here rather
    /// than at first dispatch.
    fn project_add(&mut self, params: &Value) -> Result<Value, String> {
        let path = require_str(params, "path")?;
        let repo_path = expand_tilde(&path);
        let repo =
            git2::Repository::open(&repo_path).map_err(|e| format!("not a git repository: {e}"))?;
        // Use the requested branch, or fall back to the repo's checked-out default —
        // so browsing to a repo and adding it "just works" without naming a branch.
        let base_branch = params
            .get("base_branch")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| git_default_branch(&repo_path))
            .unwrap_or_else(|| "main".to_string());
        repo.revparse_single(&base_branch)
            .map_err(|_| format!("base branch '{base_branch}' not found in repo"))?;
        let id = self.add_project(repo_path, base_branch);
        self.persist();
        let project = self
            .projects
            .iter()
            .find(|p| p.id == id)
            .expect("just added");
        Ok(project_json(project))
    }

    /// Browse host directories so the user can pick a repo without typing a path.
    /// Returns the canonical path, its parent (for "up"), whether it is itself a git
    /// repo, and its subdirectories (each flagged if it is a git repo).
    fn fs_list(&self, params: &Value) -> Result<Value, String> {
        let path = match params
            .get("path")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            Some(p) => expand_tilde(p),
            None => expand_tilde("~"),
        };
        let path = std::fs::canonicalize(&path)
            .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        let reader =
            std::fs::read_dir(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let mut dirs: Vec<(String, std::path::PathBuf, bool)> = reader
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| {
                let p = e.path();
                let is_git = p.join(".git").exists();
                (e.file_name().to_string_lossy().into_owned(), p, is_git)
            })
            .collect();
        dirs.sort_by_key(|a| a.0.to_lowercase());
        let entries: Vec<Value> = dirs
            .into_iter()
            .map(|(name, p, is_git)| {
                let is_hidden = name.starts_with('.');
                json!({ "name": name, "path": p.display().to_string(), "is_git": is_git, "is_hidden": is_hidden })
            })
            .collect();
        Ok(json!({
            "path": path.display().to_string(),
            "parent": path.parent().map(|p| p.display().to_string()),
            "is_git": path.join(".git").exists(),
            "entries": entries,
        }))
    }

    fn settings_get(&self) -> Value {
        json!({ "projects_dir": self.projects_dir.display().to_string() })
    }

    /// Choose where cloned repos land (and the browser's default start), creating
    /// the folder and persisting the choice.
    fn settings_set(&mut self, params: &Value) -> Result<Value, String> {
        let dir = expand_tilde(&require_str(params, "projects_dir")?);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        self.projects_dir = std::fs::canonicalize(&dir).unwrap_or(dir);
        self.persist();
        Ok(self.settings_get())
    }

    /// Clone a remote into the projects folder and register it as a project. The
    /// base branch defaults to the clone's checked-out branch.
    fn project_clone(&mut self, params: &Value) -> Result<Value, String> {
        let url = require_str(params, "url")?;
        let name = match params
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(n) => n.to_string(),
            None => repo_name_from_url(&url),
        };
        if name.is_empty() || name.contains('/') || name.contains("..") {
            return Err(format!("invalid project name: {name:?}"));
        }
        std::fs::create_dir_all(&self.projects_dir)
            .map_err(|e| format!("cannot create projects folder: {e}"))?;
        let dest = self.projects_dir.join(&name);
        if dest.exists() {
            // Already in the projects folder — register the existing checkout instead
            // of cloning again, as long as it's the same repo (matching remote).
            if !dest.join(".git").exists() {
                return Err(format!(
                    "'{name}' already exists in the projects folder and is not a git repo"
                ));
            }
            if let Some(origin) = git_remote_origin(&dest) {
                if !remotes_match(&origin, &url) {
                    return Err(format!(
                        "'{name}' already exists with a different remote ({origin})"
                    ));
                }
            }
            return self.register_clone(params, dest);
        }
        let out = std::process::Command::new("git")
            .arg("clone")
            .arg(&url)
            .arg(&dest)
            .output()
            .map_err(|e| format!("could not run git: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "git clone failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        self.register_clone(params, dest)
    }

    /// Register a freshly cloned (or already-present) checkout as a project,
    /// defaulting the base branch to its checked-out branch.
    fn register_clone(
        &mut self,
        params: &Value,
        dest: std::path::PathBuf,
    ) -> Result<Value, String> {
        let base = params
            .get("base_branch")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| git_default_branch(&dest))
            .unwrap_or_else(|| "main".to_string());
        let id = self.add_project(dest, base);
        self.persist();
        let project = self
            .projects
            .iter()
            .find(|p| p.id == id)
            .expect("just added");
        Ok(project_json(project))
    }

    /// Create a brand-new git repo (with an initial commit so its base branch
    /// resolves and tasks can dispatch) inside `parent` — a browsed-to directory,
    /// or the projects folder by default — and register it. An optional `remote`
    /// is wired as `origin` at creation.
    fn project_create(&mut self, params: &Value) -> Result<Value, String> {
        let name = require_str(params, "name")?;
        let name = name.trim();
        if name.is_empty() || name.contains('/') || name.contains("..") {
            return Err(format!("invalid project name: {name:?}"));
        }
        let base_branch = params
            .get("base_branch")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("main")
            .to_string();
        let parent = match params
            .get("parent")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(p) => expand_tilde(p),
            None => self.projects_dir.clone(),
        };
        std::fs::create_dir_all(&parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        let dest = parent.join(name);
        if dest.exists() {
            return Err(format!("'{name}' already exists in {}", parent.display()));
        }
        std::fs::create_dir_all(&dest).map_err(|e| format!("cannot create {name}: {e}"))?;
        git_in(&dest, &["init", "-b", &base_branch])?;
        std::fs::write(dest.join("README.md"), format!("# {name}\n"))
            .map_err(|e| format!("cannot write README: {e}"))?;
        git_in(&dest, &["add", "."])?;
        // Commit with an explicit identity so it never depends on host git config.
        git_in(
            &dest,
            &[
                "-c",
                "user.email=build@build.ing",
                "-c",
                "user.name=Build",
                "commit",
                "-m",
                "Initial commit",
            ],
        )?;
        if let Some(remote) = params
            .get("remote")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            git_in(&dest, &["remote", "add", "origin", remote])?;
        }
        let id = self.add_project(dest, base_branch);
        self.persist();
        let project = self
            .projects
            .iter()
            .find(|p| p.id == id)
            .expect("just added");
        Ok(project_json(project))
    }

    /// Set (or clear, with an empty url) a project's `origin` remote.
    fn project_set_remote(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let url = require_str(params, "url")?;
        let url = url.trim();
        let repo_path = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.repo_path.clone())
            .ok_or_else(|| format!("unknown project: {project_id}"))?;
        if url.is_empty() {
            // Clearing: removing a missing origin is not an error.
            let _ = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo_path)
                .args(["remote", "remove", "origin"])
                .output();
        } else if git_remote_origin(&repo_path).is_some() {
            git_in(&repo_path, &["remote", "set-url", "origin", url])?;
        } else {
            git_in(&repo_path, &["remote", "add", "origin", url])?;
        }
        self.persist();
        let project = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .expect("exists");
        Ok(project_json(project))
    }

    fn task_dispatch(&mut self, params: &Value) -> Result<Value, String> {
        let goal = require_str(params, "goal")?;
        let kind = match params.get("kind").and_then(Value::as_str) {
            Some("quick") => TaskKind::Quick,
            _ => TaskKind::Standard,
        };
        // Default to the first project when the client doesn't choose one.
        let project_id = match params.get("project_id").and_then(Value::as_str) {
            Some(p) => p.to_string(),
            None => self
                .projects
                .first()
                .map(|p| p.id.clone())
                .ok_or("no projects configured")?,
        };
        let base = self.base_for(&project_id)?;
        let task_id = format!("task-{}", self.next_id);
        self.next_id += 1;

        let mut active = self
            .orch_for(&project_id)?
            .dispatch(TaskId::new(&task_id), goal, kind, &base)
            .map_err(err)?;
        self.task_project
            .insert(task_id.clone(), project_id.clone());

        // Scripted agent: produce the plan (standard) or the code (quick) and
        // report done, exactly as a real harness would over MCP.
        if self.qa_agent {
            match active.task.state {
                TaskState::Planning => self.simulate_plan(&project_id, &mut active)?,
                TaskState::Building => self.simulate_build(&project_id, &mut active)?,
                _ => {}
            }
        }

        let view = self.task_view(&task_id, &active);
        self.tasks.insert(task_id.clone(), active);
        Ok(view)
    }

    /// The orchestrator for a project id.
    fn orch_for(&self, project_id: &str) -> Result<&Orchestrator, String> {
        self.projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| &p.orch)
            .ok_or_else(|| format!("unknown project: {project_id}"))
    }

    /// The base branch configured for a project id.
    fn base_for(&self, project_id: &str) -> Result<String, String> {
        self.projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.base_branch.clone())
            .ok_or_else(|| format!("unknown project: {project_id}"))
    }

    /// The project a task was dispatched to.
    fn project_of(&self, task_id: &str) -> Result<String, String> {
        self.task_project
            .get(task_id)
            .cloned()
            .ok_or_else(|| "unknown task_id".to_string())
    }

    fn task_approve_plan(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let project_id = self.project_of(&task_id)?;
        let mut active = self.take(&task_id)?;
        let outcome = (|| -> Result<(), String> {
            self.orch_for(&project_id)?
                .approve_plan(&mut active)
                .map_err(err)?;
            if self.qa_agent {
                self.simulate_build(&project_id, &mut active)?;
            }
            Ok(())
        })();
        let view = self.task_view(&task_id, &active);
        self.tasks.insert(task_id, active);
        outcome?;
        Ok(view)
    }

    /// Send the reviewer's plan comments back to the agent to revise the plan
    /// (plan_review → planning → … → plan_review).
    fn task_send_notes(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let comments = require_str(params, "comments")?;
        let project_id = self.project_of(&task_id)?;
        let mut active = self.take(&task_id)?;
        let outcome = (|| -> Result<(), String> {
            self.orch_for(&project_id)?
                .send_notes(&mut active, &comments)
                .map_err(err)?;
            if self.qa_agent {
                self.simulate_plan(&project_id, &mut active)?;
            }
            Ok(())
        })();
        let view = self.task_view(&task_id, &active);
        self.tasks.insert(task_id, active);
        outcome?;
        Ok(view)
    }

    /// Send the reviewer's diff comments to the coding agent to make changes —
    /// works from `review` and from `building` (redirecting a running agent).
    fn task_request_changes(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let comments = require_str(params, "comments")?;
        let project_id = self.project_of(&task_id)?;
        let mut active = self.take(&task_id)?;
        let outcome = (|| -> Result<(), String> {
            self.orch_for(&project_id)?
                .request_changes(&mut active, &comments)
                .map_err(err)?;
            if self.qa_agent {
                self.simulate_build(&project_id, &mut active)?;
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
        let project_id = self.project_of(&task_id)?;
        let mut active = self.take(&task_id)?;
        let result = self
            .orch_for(&project_id)?
            .approve_merge(&mut active)
            .map_err(err);
        let view = self.task_view(&task_id, &active);
        self.tasks.insert(task_id, active);
        result?;
        Ok(view)
    }

    /// Finish-the-worktree git actions from the diff review: `commit` and `push`
    /// keep the worktree (no lifecycle change); `merge` and `merge_push` merge into
    /// the base and end the task. Every action commits outstanding work first.
    fn task_git_action(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let action = require_str(params, "action")?;
        let project_id = self.project_of(&task_id)?;
        let mut active = self.take(&task_id)?;
        let result = {
            let orch = self.orch_for(&project_id)?;
            match action.as_str() {
                "commit" => orch.commit(&active).map_err(err),
                "push" => orch.push(&active).map_err(err),
                "merge" => orch.approve_merge(&mut active).map_err(err),
                "merge_push" => orch.merge_and_push(&mut active).map_err(err),
                other => Err(format!("unknown git action: {other}")),
            }
        };
        let view = self.task_view(&task_id, &active);
        self.tasks.insert(task_id, active);
        result?;
        Ok(view)
    }

    fn task_abandon(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let project_id = self.project_of(&task_id)?;
        let mut active = self.take(&task_id)?;
        let result = self
            .orch_for(&project_id)?
            .abandon(&mut active)
            .map_err(err);
        let view = self.task_view(&task_id, &active);
        self.tasks.insert(task_id, active);
        result?;
        Ok(view)
    }

    fn task_diff(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let project_id = self.project_of(&task_id)?;
        let active = self.tasks.get(&task_id).ok_or("unknown task_id")?;
        let diff = self.orch_for(&project_id)?.diff(active).map_err(err)?;
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
            .map(|(id, active)| self.task_view(id, active))
            .collect();
        json!({ "tasks": tasks })
    }

    // --- the scripted QA agent ------------------------------------------------

    fn simulate_plan(&self, project_id: &str, active: &mut ActiveTask) -> Result<(), String> {
        let plan = format!(
            "# Plan: {goal}\n\n1. Implement the goal.\n2. Add a result file.\n",
            goal = active.task.goal
        );
        write_in_worktree(active, DEFAULT_PLAN_PATH, &plan)?;
        self.orch_for(project_id)?
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

    fn simulate_build(&self, project_id: &str, active: &mut ActiveTask) -> Result<(), String> {
        let content = format!("Implemented: {}\n", active.task.goal);
        write_in_worktree(active, "result.txt", &content)?;
        self.orch_for(project_id)?
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
        let project_id = self.task_project.get(task_id).cloned().unwrap_or_default();
        let project = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        json!({
            "task_id": task_id,
            "goal": active.task.goal,
            "state": state_str(&active.task.state),
            "needs_attention": active.task.state.needs_attention(),
            "branch": active.worktree.branch,
            "base_branch": active.worktree.base_branch,
            "summary": active.last_summary,
            "project": project,
            "project_id": project_id,
            "harness": self.harness,
        })
    }

    fn take(&mut self, task_id: &str) -> Result<ActiveTask, String> {
        self.tasks.remove(task_id).ok_or("unknown task_id".into())
    }
}

fn project_json(p: &Project) -> Value {
    json!({
        "project_id": p.id,
        "name": p.name,
        "path": p.repo_path.display().to_string(),
        "base_branch": p.base_branch,
        "remote": git_remote_origin(&p.repo_path),
    })
}

/// Run a git subcommand in `dir`, mapping a non-zero exit to a readable error.
fn git_in(dir: &std::path::Path, args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

/// Expand a leading `~` / `~/` to the user's home directory; otherwise return the
/// path unchanged. Lets path fields accept `~/code/foo`.
fn expand_tilde(path: &str) -> std::path::PathBuf {
    if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return std::path::PathBuf::from(home);
        }
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return std::path::Path::new(&home).join(rest);
        }
    }
    std::path::PathBuf::from(path)
}

/// The default folder cloned repos land in, `~/.build/projects`.
fn default_projects_dir() -> std::path::PathBuf {
    expand_tilde("~/.build/projects")
}

/// Derive a project folder name from a clone URL: the last path segment with a
/// trailing `.git` stripped (`git@host:org/repo.git` → `repo`).
fn repo_name_from_url(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    let last = trimmed.rsplit(['/', ':']).next().unwrap_or("repo");
    last.strip_suffix(".git").unwrap_or(last).to_string()
}

/// The `origin` remote URL of a repo, if it has one.
fn git_remote_origin(dir: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!url.is_empty()).then_some(url)
}

/// Whether two clone URLs point at the same repo, ignoring a trailing `/` or
/// `.git`. A loose check — enough to catch "already cloned" without surprises.
fn remotes_match(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.trim()
            .trim_end_matches('/')
            .trim_end_matches(".git")
            .to_string()
    };
    norm(a) == norm(b)
}

/// The checked-out branch name of a freshly cloned repo (its default branch).
fn git_default_branch(dir: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!branch.is_empty() && branch != "HEAD").then_some(branch)
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

    // Match the PTY + screen model to this client's viewport, or TUIs (which draw
    // to the reported size) render to the wrong width and garble.
    if term.cols != cols || term.rows != rows {
        let _ = term.session.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        term.parser.set_size(rows, cols);
        term.cols = cols;
        term.rows = rows;
    }
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
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );

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
    fn git_action_commit_keeps_task_merge_finishes_it() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let res = state.handle(req(
            "task.dispatch",
            json!({ "goal": "do work", "kind": "quick" }),
        ));
        let task_id = res["result"]["task_id"].as_str().unwrap().to_string();
        assert_eq!(res["result"]["state"], "review");

        // Commit keeps the task in review (no merge).
        let c = state.handle(req(
            "task.git_action",
            json!({ "task_id": task_id, "action": "commit" }),
        ));
        assert_eq!(c["ok"], true, "{c:?}");
        assert_eq!(c["result"]["state"], "review");
        assert!(
            !repo.join("result.txt").exists(),
            "not merged into base yet"
        );

        // An unknown action is a clean error and leaves the task untouched.
        let bad = state.handle(req(
            "task.git_action",
            json!({ "task_id": task_id, "action": "rebase" }),
        ));
        assert_eq!(bad["ok"], false);
        assert!(bad["error"]
            .as_str()
            .unwrap()
            .contains("unknown git action"));

        // Merge finishes it: base branch carries the work.
        let m = state.handle(req(
            "task.git_action",
            json!({ "task_id": task_id, "action": "merge" }),
        ));
        assert_eq!(m["result"]["state"], "merged");
        assert!(repo.join("result.txt").exists());
    }

    #[test]
    fn send_notes_reruns_planning() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo,
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let res = state.handle(req("task.dispatch", json!({ "goal": "add a greeting" })));
        let task_id = res["result"]["task_id"].as_str().unwrap().to_string();
        assert_eq!(res["result"]["state"], "plan_review");

        // Requesting updates re-runs planning (QA simulates it) → back to plan_review.
        let upd = state.handle(req(
            "task.send_notes",
            json!({ "task_id": task_id, "comments": "On \"step 1\": please add error handling." }),
        ));
        assert_eq!(upd["ok"], true, "{upd:?}");
        assert_eq!(upd["result"]["state"], "plan_review");
        // The plan is still readable afterwards.
        let plan = state.handle(req("task.plan", json!({ "task_id": task_id })));
        assert!(plan["result"]["contents"]
            .as_str()
            .unwrap()
            .contains("greeting"));

        // Missing comments is a clean error.
        let bad = state.handle(req("task.send_notes", json!({ "task_id": task_id })));
        assert_eq!(bad["ok"], false);
        assert!(bad["error"].as_str().unwrap().contains("comments"));
    }

    #[test]
    fn request_changes_reruns_building_from_review_and_while_building() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo,
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        // Quick task → review (QA simulates the build).
        let res = state.handle(req(
            "task.dispatch",
            json!({ "goal": "do work", "kind": "quick" }),
        ));
        let task_id = res["result"]["task_id"].as_str().unwrap().to_string();
        assert_eq!(res["result"]["state"], "review");

        // Request changes from review → re-built (QA simulates) → back to review.
        let rc = state.handle(req(
            "task.request_changes",
            json!({ "task_id": task_id, "comments": "On result.txt line 1: rename the symbol." }),
        ));
        assert_eq!(rc["ok"], true, "{rc:?}");
        assert_eq!(rc["result"]["state"], "review");

        // Missing comments is a clean error.
        let bad = state.handle(req("task.request_changes", json!({ "task_id": task_id })));
        assert_eq!(bad["ok"], false);
        assert!(bad["error"].as_str().unwrap().contains("comments"));
    }

    #[test]
    fn dispatch_routes_to_the_selected_project() {
        let (dir_a, repo_a) = init_repo();
        let (_dir_b, repo_b) = init_repo();
        let mut state = AppState::new(
            repo_a.clone(),
            dir_a.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let proj_b = state.add_project(repo_b.clone(), "main".into());

        // Both projects are listed.
        let list = state.handle(req("project.list", json!({})));
        assert_eq!(list["result"]["projects"].as_array().unwrap().len(), 2);

        // Dispatch explicitly to project B → on merge the file lands in repo B only.
        let res = state.handle(req(
            "task.dispatch",
            json!({ "goal": "greet b", "project_id": proj_b }),
        ));
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["project_id"], proj_b);
        let task_id = res["result"]["task_id"].as_str().unwrap().to_string();
        state.handle(req("task.approve_plan", json!({ "task_id": task_id })));
        let merged = state.handle(req("task.approve_merge", json!({ "task_id": task_id })));
        assert_eq!(merged["result"]["state"], "merged");
        assert!(repo_b.join("result.txt").exists(), "merged into project B");
        assert!(!repo_a.join("result.txt").exists(), "project A untouched");
    }

    #[test]
    fn project_add_validates_and_dedupes() {
        let (dir_a, repo_a) = init_repo();
        let mut state = AppState::new(
            repo_a.clone(),
            dir_a.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        // A non-repo path is rejected.
        let bad = state.handle(req(
            "project.add",
            json!({ "path": "/definitely/not/a/repo" }),
        ));
        assert_eq!(bad["ok"], false);
        // A real repo is added and listed.
        let (_dir_c, repo_c) = init_repo();
        let ok = state.handle(req(
            "project.add",
            json!({ "path": repo_c.to_str().unwrap() }),
        ));
        assert_eq!(ok["ok"], true, "{ok:?}");
        assert_eq!(
            state.handle(req("project.list", json!({})))["result"]["projects"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        // Adding the same repo again is idempotent (deduped by canonical path).
        state.handle(req(
            "project.add",
            json!({ "path": repo_c.to_str().unwrap() }),
        ));
        assert_eq!(
            state.handle(req("project.list", json!({})))["result"]["projects"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn fs_list_browses_dirs_and_flags_git_repos() {
        let (dir_a, repo_a) = init_repo();
        let mut state = AppState::new(
            repo_a,
            dir_a.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        // A browse root with a plain folder, a git repo, and a hidden folder.
        let root = dir_a.path().join("browse");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("plain")).unwrap();
        std::fs::create_dir(root.join(".hidden")).unwrap();
        let repo_dir = root.join("myrepo");
        std::fs::create_dir(&repo_dir).unwrap();
        std::fs::create_dir(repo_dir.join(".git")).unwrap();

        let res = state.handle(req("fs.list", json!({ "path": root.to_str().unwrap() })));
        assert_eq!(res["ok"], true, "{res:?}");
        assert!(res["result"]["parent"].is_string());
        let entries = res["result"]["entries"].as_array().unwrap();
        assert_eq!(
            entries.iter().find(|e| e["name"] == "myrepo").unwrap()["is_git"],
            true
        );
        assert_eq!(
            entries.iter().find(|e| e["name"] == "plain").unwrap()["is_git"],
            false
        );
        // Hidden dirs are still returned, flagged so the client can toggle them.
        let hidden = entries.iter().find(|e| e["name"] == ".hidden").unwrap();
        assert_eq!(hidden["is_hidden"], true);
        assert_eq!(
            entries.iter().find(|e| e["name"] == "plain").unwrap()["is_hidden"],
            false
        );
    }

    #[test]
    fn settings_set_then_clone_registers_project() {
        let (dir_src, repo_src) = init_repo();
        let (dir_a, repo_a) = init_repo();
        let mut state = AppState::new(
            repo_a,
            dir_a.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        // Point the projects folder at a temp location.
        let projects_dir = dir_src.path().join("projects");
        let set = state.handle(req(
            "settings.set",
            json!({ "projects_dir": projects_dir.to_str().unwrap() }),
        ));
        assert_eq!(set["ok"], true, "{set:?}");
        assert!(
            state.handle(req("settings.get", json!({})))["result"]["projects_dir"]
                .as_str()
                .unwrap()
                .contains("projects")
        );

        // Clone the source repo into the projects folder and register it.
        let cloned = state.handle(req(
            "project.clone",
            json!({ "url": repo_src.to_str().unwrap() }),
        ));
        assert_eq!(cloned["ok"], true, "{cloned:?}");
        let clone_path = cloned["result"]["path"].as_str().unwrap().to_string();
        assert!(std::path::Path::new(&clone_path).join("README.md").exists());
        assert_eq!(
            state.handle(req("project.list", json!({})))["result"]["projects"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        // Cloning the same repo again registers the existing checkout — no error, no
        // duplicate, same path (not a second clone).
        let again = state.handle(req(
            "project.clone",
            json!({ "url": repo_src.to_str().unwrap() }),
        ));
        assert_eq!(again["ok"], true, "{again:?}");
        assert_eq!(again["result"]["path"].as_str().unwrap(), clone_path);
        assert_eq!(
            state.handle(req("project.list", json!({})))["result"]["projects"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn project_create_inits_a_repo_and_set_remote_sets_origin() {
        let (dir_a, repo_a) = init_repo();
        let mut state = AppState::new(
            repo_a,
            dir_a.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let projects_dir = dir_a.path().join("projects");
        state.handle(req(
            "settings.set",
            json!({ "projects_dir": projects_dir.to_str().unwrap() }),
        ));

        // Create a repo in a browsed-to location with no remote yet: it has an
        // initial commit (base branch resolves) and is registered at that path.
        let where_to = dir_a.path().join("code");
        let created = state.handle(req(
            "project.create",
            json!({ "name": "fresh", "parent": where_to.to_str().unwrap() }),
        ));
        assert_eq!(created["ok"], true, "{created:?}");
        let project_id = created["result"]["project_id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(created["result"]["remote"].is_null());
        let repo = where_to.join("fresh");
        assert!(
            created["result"]["path"]
                .as_str()
                .unwrap()
                .ends_with("code/fresh"),
            "{created:?}"
        );
        assert!(
            repo.join(".git").exists(),
            "git repo created at chosen location"
        );
        // The base branch resolves (there is a commit), so tasks can dispatch.
        let r = git2::Repository::open(&repo).unwrap();
        assert!(
            r.revparse_single("main").is_ok(),
            "base branch has a commit"
        );

        // Set the origin remote, then read it back from project.list.
        let set = state.handle(req(
            "project.set_remote",
            json!({ "project_id": project_id, "url": "git@github.com:me/fresh.git" }),
        ));
        assert_eq!(set["ok"], true, "{set:?}");
        assert_eq!(set["result"]["remote"], "git@github.com:me/fresh.git");
        let listed = state.handle(req("project.list", json!({})))["result"]["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["project_id"] == project_id.as_str())
            .unwrap()
            .clone();
        assert_eq!(listed["remote"], "git@github.com:me/fresh.git");

        // A remote can also be assigned at creation time (default folder).
        let created2 = state.handle(req(
            "project.create",
            json!({ "name": "withremote", "remote": "https://github.com/me/withremote.git" }),
        ));
        assert_eq!(created2["ok"], true, "{created2:?}");
        assert_eq!(
            created2["result"]["remote"],
            "https://github.com/me/withremote.git"
        );
        assert!(
            created2["result"]["path"]
                .as_str()
                .unwrap()
                .ends_with("projects/withremote"),
            "{created2:?}"
        );
    }

    #[test]
    fn config_persists_projects_and_dir_across_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let (_dir_a, repo_a) = init_repo();
        let (_dir_b, repo_b) = init_repo();
        let cfg = tmp.path().join("config.json");
        {
            let mut state = AppState::new(
                repo_a.clone(),
                tmp.path().join("wt"),
                "main",
                true,
                "/tmp/test-mcp.sock",
            )
            .with_config(&cfg);
            state.handle(req(
                "project.add",
                json!({ "path": repo_b.to_str().unwrap() }),
            ));
            state.handle(req(
                "settings.set",
                json!({ "projects_dir": tmp.path().join("myprojects").to_str().unwrap() }),
            ));
        }
        // A fresh instance restores the added project and the chosen dir from disk.
        let mut reloaded = AppState::new(
            repo_a,
            tmp.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_config(&cfg);
        assert_eq!(
            reloaded.handle(req("project.list", json!({})))["result"]["projects"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(
            reloaded.handle(req("settings.get", json!({})))["result"]["projects_dir"]
                .as_str()
                .unwrap()
                .contains("myprojects")
        );
    }

    #[test]
    fn unknown_method_and_missing_params_error_cleanly() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo,
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        assert_eq!(state.handle(req("nope", json!({})))["ok"], false);
        let missing = state.handle(req("task.dispatch", json!({})));
        assert_eq!(missing["ok"], false);
        assert!(missing["error"].as_str().unwrap().contains("goal"));
    }

    #[tokio::test]
    async fn stream_resume_reconstructs_full_output() {
        let (dir, repo) = init_repo();
        let handler = AppState::new(
            repo,
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .into_handler();
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
        let handler = AppState::new(
            repo,
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .into_handler();
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
