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

use crate::mcp::{CommentResolution, DoneOutputs, DonePhase, DoneReport, DoneStatus};
use crate::models::{self, ModelChoice};
use crate::notify::{Notifier, NotifyThrottle};
use crate::orchestrator::{
    ActiveTask, Agent, Orchestrator, OrchestratorError, SpawnOptions, TranscriptProbe,
};
use crate::pty::{HarnessSpec, PtySession};
use crate::relay::{FrameHandler, SessionSender};
use crate::store::{now_rfc3339, PersistedTask, TaskStore};
use crate::task::{
    CommentAnchor, CommentState, Stage, StageComment, StageManifestEntry, StageState, Task,
    TaskEvent, TaskId, TaskKind, TaskState, ValidationReport,
};
use crate::templates::{Templates, STAGES_MANIFEST_PATH};
use crate::transport::Frame;
use crate::worktree::{discover_external_worktrees, ExternalWorktree, Worktree};

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
    /// Cached external-worktree scan, refreshed at most every
    /// `EXTERNAL_SCAN_INTERVAL` (or on demand via `force`).
    external_scan: Option<ExternalScanCache>,
}

/// External-worktree scans are refreshed at most this often per project; the
/// board polls task.list every ~1.6 s and must never trigger a full rescan per
/// poll.
const EXTERNAL_SCAN_INTERVAL: Duration = Duration::from_secs(10);

/// One project's cached external-worktree scan.
struct ExternalScanCache {
    scanned_at: std::time::Instant,
    worktrees: Vec<ExternalWorktree>,
}

/// Build the harness adapter shared by every project's orchestrator: the
/// deterministic scripted agent for QA, or a one-shot `claude` headless run for
/// real work. The closure is shared (Arc) across projects via `Agent: Clone`.
fn build_agent(qa_agent: bool, mcp_socket: String) -> Agent {
    if qa_agent {
        // A warm no-op harness that drains stdin like a real interactive CLI
        // (a non-reading child would let the PTY input queue fill and block
        // prompt writes); the scripted agent does the file writing.
        Agent::Warm(HarnessSpec::new("sh").arg("-c").arg("cat >/dev/null"))
    } else {
        // Real agent: a one-shot `claude` headless run with the rendered prompt
        // baked in, the per-task `done` MCP server wired via .build/mcp.json, and
        // the daemon's control socket so its `done` reaches on_agent_done.
        Agent::OneShot(Arc::new(
            move |prompt: &str, choice: &ModelChoice, options: &SpawnOptions| {
                let mut spec = HarnessSpec::new("claude")
                    .arg("-p")
                    .arg(prompt)
                    .arg("--mcp-config")
                    .arg(".build/mcp.json")
                    .arg("--strict-mcp-config")
                    .arg("--dangerously-skip-permissions");
                if options.continue_session {
                    spec = spec.arg("--continue");
                }
                for arg in choice.harness_args() {
                    spec = spec.arg(arg);
                }
                spec.env("BRIDGE_MCP_SOCKET", &mcp_socket)
            },
        ))
    }
}

/// The transcript directory name Claude Code uses for a cwd under
/// `~/.claude/projects/`: the absolute path with `/` and `.` replaced by `-`.
/// Heuristic by design — a false negative just means a fresh session.
pub(crate) fn encode_claude_project_dir(path: &std::path::Path) -> String {
    path.display()
        .to_string()
        .chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect()
}

/// True iff the encoded directory exists under `root` and holds at least one
/// `.jsonl` transcript.
pub(crate) fn claude_transcript_exists(root: &std::path::Path, cwd: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(root.join(encode_claude_project_dir(cwd))) else {
        return false;
    };
    entries
        .flatten()
        .any(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some("jsonl"))
}

/// The production transcript probe, rooted at `~/.claude/projects` (claude-
/// specific, like the harness argv in [`build_agent`]).
fn default_claude_transcript_probe() -> TranscriptProbe {
    Arc::new(|cwd: &std::path::Path| {
        let Ok(home) = std::env::var("HOME") else {
            return false;
        };
        claude_transcript_exists(&std::path::Path::new(&home).join(".claude/projects"), cwd)
    })
}

/// Shared application state behind the relay handler.
pub struct AppState {
    /// Registered projects (repos) tasks can be dispatched to.
    projects: Vec<Project>,
    /// task id → the project it was dispatched to (routes approve/diff/merge/done).
    task_project: HashMap<String, String>,
    /// task id → its project's repo path, retained even when the project is not
    /// registered (a parked repo-missing task has no `task_project` entry, yet its
    /// record must keep the real path so a restored repo can un-park it).
    task_project_path: HashMap<String, String>,
    worktrees_root: std::path::PathBuf,
    /// Where cloned repos land and the directory browser starts; user-configurable.
    projects_dir: std::path::PathBuf,
    /// Where to persist the projects + settings, if persistence is enabled.
    config_path: Option<std::path::PathBuf>,
    agent: Agent,
    harness: String,
    tasks: HashMap<String, ActiveTask>,
    /// Durable task records under the bridge state dir, if persistence is enabled.
    task_store: Option<TaskStore>,
    /// task id → its RFC 3339 creation time, carried across saves (and restarts).
    task_created_at: HashMap<String, String>,
    streams: HashMap<String, StreamState>,
    term: Option<TermSession>,
    next_id: u64,
    next_stream: u64,
    next_project: u64,
    /// When true, simulate the agent deterministically (local QA, no LLM).
    qa_agent: bool,
    /// Whether the harness has a prior conversation for a worktree cwd — drives
    /// `--continue` on an adopted task's first session. Never true in QA mode.
    transcript_probe: TranscriptProbe,
    /// Web-push notifier for attention transitions, if configured. Content-free
    /// by contract — it only ever says "a task needs you".
    notifier: Option<Notifier>,
    /// At most one push per task-state change.
    notify_throttle: NotifyThrottle,
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
        let transcript_probe: TranscriptProbe = if qa_agent {
            Arc::new(|_| false)
        } else {
            default_claude_transcript_probe()
        };
        let mut state = AppState {
            projects: Vec::new(),
            task_project: HashMap::new(),
            task_project_path: HashMap::new(),
            worktrees_root: worktrees_root.into(),
            projects_dir: default_projects_dir(),
            config_path: None,
            agent: build_agent(qa_agent, mcp_socket.into()),
            harness,
            tasks: HashMap::new(),
            task_store: None,
            task_created_at: HashMap::new(),
            streams: HashMap::new(),
            term: None,
            next_id: 1,
            next_stream: 1,
            next_project: 1,
            qa_agent,
            transcript_probe,
            notifier: None,
            notify_throttle: NotifyThrottle::default(),
        };
        state.add_project(repo_path.into(), base_branch.into());
        state
    }

    /// Like [`AppState::new`] but with no default project: projects arrive only
    /// through the UI (`project.add`/`project.clone`) and persisted config. This
    /// is the end-user path — a default like `/repo` would register a phantom
    /// project on machines where that path never existed.
    pub fn new_unrooted(
        worktrees_root: impl Into<std::path::PathBuf>,
        base_branch: impl Into<String>,
        qa_agent: bool,
        mcp_socket: impl Into<String>,
    ) -> Self {
        let mut state = Self::new(
            "/nonexistent",
            worktrees_root,
            base_branch,
            qa_agent,
            mcp_socket,
        );
        state.projects.clear();
        state.next_project = 1;
        state
    }

    /// Enable web-push attention notifications: every task-state change into a
    /// state that needs the human fires one signed, content-free notify at the api.
    pub fn with_notifier(mut self, notifier: Notifier) -> Self {
        self.notifier = Some(notifier);
        self
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

    /// Enable durable task persistence at `dir` and recover every stored task:
    /// re-attach tasks whose worktrees survived, surface tasks that were mid-phase
    /// when the daemon died as `interrupted`, and abandon tasks whose worktrees are
    /// gone. A corrupt task file is a hard error naming the file — boot fails
    /// rather than silently dropping a task.
    pub fn with_task_store(mut self, dir: impl Into<std::path::PathBuf>) -> Result<Self, String> {
        let store = TaskStore::new(dir);
        let records = store.load_all().map_err(|e| e.to_string())?;
        self.task_store = Some(store);
        for record in records {
            self.recover_task(record)?;
        }
        Ok(self)
    }

    /// Re-attach one persisted task on boot. The durable core is authoritative;
    /// the PTY session died with the previous daemon, so a working state becomes
    /// `Interrupted(phase)` (persisted immediately, so the verdict survives the
    /// next restart too), and a non-terminal task without its worktree is
    /// abandoned — the worktree is removed and the branch kept, which is exactly
    /// what `Abandoned` means.
    fn recover_task(&mut self, record: PersistedTask) -> Result<(), String> {
        let task_id = record.id.clone();
        let mut task = Task::new(TaskId::new(&record.id), record.goal, record.kind);
        task.state = record.state;
        let worktree = Worktree {
            name: record.worktree_name,
            path: std::path::PathBuf::from(&record.worktree_path),
            branch: record.branch,
            base_branch: record.base_branch.clone(),
        };
        let mut active = ActiveTask::reattach(
            task,
            worktree,
            record.plan_path,
            record.last_summary,
            ModelChoice {
                model: record.model,
                effort: record.effort,
            },
            record.last_error,
            record.stages,
            record.current_stage_id,
            record.revising_stage_id,
            record.auto_advance,
            record.comments,
            record.adopted,
            record.pending_continuation,
        );

        let mut state_changed = false;
        if !active.task.state.is_terminal() {
            if !active.worktree.path.exists() {
                eprintln!(
                    "recover {task_id}: worktree {} is gone; abandoning (branch kept)",
                    active.worktree.path.display()
                );
                active
                    .task
                    .apply(TaskEvent::Abandon)
                    .map_err(|e| format!("recover {task_id}: {e}"))?;
                state_changed = true;
            } else if active.task.state.is_working() {
                active
                    .task
                    .apply(TaskEvent::Interrupt)
                    .map_err(|e| format!("recover {task_id}: {e}"))?;
                state_changed = true;
            }
        }

        // Project ids are re-minted each boot, so resolve by repo path — and
        // re-register the project if the config lost it but the repo survives.
        // Retain the record's path unconditionally: a parked task gets no
        // `task_project` entry, and without this fallback `persist_task` would
        // overwrite `project_path` with "" and orphan the task forever.
        self.task_project_path
            .insert(task_id.clone(), record.project_path.clone());
        let repo_path = std::path::PathBuf::from(&record.project_path);
        if repo_path.exists() {
            let project_id = self.add_project(repo_path, record.base_branch);
            self.task_project.insert(task_id.clone(), project_id);
        } else if !active.task.state.is_terminal() {
            if active.adopted {
                // Automated actions never touch (or write off) an adopted
                // worktree: park the task needs-attention instead of abandoning.
                // A working state is demoted to Interrupted (its session is gone
                // anyway); gate states (Review, Blocked, …) already need attention.
                eprintln!(
                    "recover {task_id}: project repo {} is gone; parking adopted task",
                    record.project_path
                );
                if active.task.state.is_working() {
                    active
                        .task
                        .apply(TaskEvent::Interrupt)
                        .map_err(|e| format!("recover {task_id}: {e}"))?;
                }
                active.last_error =
                    Some(format!("project repo missing at {}", record.project_path));
                state_changed = true;
            } else {
                // The repo itself is gone, so the task can never advance and — with
                // no project to route to — every later RPC would return "unknown
                // task_id". Abandon it so it stays legible on the board with a
                // reason, instead of becoming an untouchable orphan.
                eprintln!(
                    "recover {task_id}: project repo {} is gone; abandoning",
                    record.project_path
                );
                active
                    .task
                    .apply(TaskEvent::Abandon)
                    .map_err(|e| format!("recover {task_id}: {e}"))?;
                active.last_error =
                    Some(format!("project repo missing at {}", record.project_path));
                state_changed = true;
            }
        } else {
            eprintln!(
                "recover {task_id}: project repo {} is gone; task kept as history",
                record.project_path
            );
        }

        // Reserve the recovered id so new dispatches never collide with it.
        if let Some(n) = task_id
            .strip_prefix("task-")
            .and_then(|s| s.parse::<u64>().ok())
        {
            self.next_id = self.next_id.max(n + 1);
        }
        self.task_created_at
            .insert(task_id.clone(), record.created_at);
        if state_changed {
            self.persist_task(&task_id, &active)?;
        }
        self.tasks.insert(task_id, active);
        Ok(())
    }

    /// Write a task's durable core to the store (atomic replace). A no-op without
    /// a configured store (unit tests); an error is surfaced to the caller — a
    /// task the store cannot hold would silently vanish on the next restart.
    fn persist_task(&mut self, task_id: &str, active: &ActiveTask) -> Result<(), String> {
        if self.task_store.is_none() {
            return Ok(());
        }
        let now = now_rfc3339();
        let created_at = self
            .task_created_at
            .entry(task_id.to_string())
            .or_insert_with(|| now.clone())
            .clone();
        let project_path = self
            .task_project
            .get(task_id)
            .and_then(|pid| self.projects.iter().find(|p| &p.id == pid))
            .map(|p| p.repo_path.display().to_string())
            .or_else(|| self.task_project_path.get(task_id).cloned())
            .unwrap_or_default();
        let record = PersistedTask {
            id: task_id.to_string(),
            goal: active.task.goal.clone(),
            kind: active.task.kind,
            project_path,
            base_branch: active.worktree.base_branch.clone(),
            state: active.task.state.clone(),
            branch: active.worktree.branch.clone(),
            worktree_name: active.worktree.name.clone(),
            worktree_path: active.worktree.path.display().to_string(),
            plan_path: active.plan_path.clone(),
            last_summary: active.last_summary.clone(),
            model: active.model_choice.model.clone(),
            effort: active.model_choice.effort.clone(),
            last_error: active.last_error.clone(),
            stages: active.stages.clone(),
            current_stage_id: active.current_stage_id.clone(),
            revising_stage_id: active.revising_stage_id.clone(),
            auto_advance: active.auto_advance,
            comments: active.comments.clone(),
            adopted: active.adopted,
            pending_continuation: active.pending_continuation,
            created_at,
            updated_at: now,
        };
        self.task_store
            .as_ref()
            .expect("checked above")
            .save(&record)
            .map_err(|e| format!("task store: {e}"))
    }

    /// The shared tail of every task mutation: compute the response view, persist
    /// the durable core, and put the task back in the map. Returns the view and
    /// the persistence outcome separately so callers can order their errors.
    fn finish_mutation(
        &mut self,
        task_id: String,
        active: ActiveTask,
    ) -> (Value, Result<(), String>) {
        let view = self.task_view(&task_id, &active);
        let persisted = self.persist_task(&task_id, &active);
        self.push_notify_if_needed(&task_id, &active.task.state);
        self.tasks.insert(task_id, active);
        (view, persisted)
    }

    /// Fire one content-free web-push notify when a task-state change lands in a
    /// state that needs the human. Fire-and-forget: the POST runs off the app
    /// lock, and a delivery failure only logs — it never blocks the mutation.
    fn push_notify_if_needed(&mut self, task_id: &str, state: &TaskState) {
        let Some(notifier) = &self.notifier else {
            return;
        };
        if !self.notify_throttle.should_notify(task_id, state) {
            return;
        }
        // The throttle already gated on a push-worthy state, so a kind exists.
        let Some(kind) = crate::notify::kind_for_state(state) else {
            return;
        };
        let notifier = notifier.clone();
        let task_id = task_id.to_string();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    if let Err(e) = notifier.notify(&task_id, kind).await {
                        eprintln!("push notify: {e}");
                    }
                });
            }
            Err(_) => eprintln!("push notify: no async runtime; skipped"),
        }
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
        )
        .with_transcript_probe(self.transcript_probe.clone());
        self.projects.push(Project {
            id: id.clone(),
            name,
            repo_path,
            base_branch,
            orch,
            external_scan: None,
        });
        id
    }

    /// Canonical paths of every task-bound worktree (all states): they are
    /// Build's, never external. `fs::canonicalize` with the raw path as fallback.
    fn bound_worktree_paths(&self) -> std::collections::HashSet<std::path::PathBuf> {
        self.tasks
            .values()
            .map(|active| {
                std::fs::canonicalize(&active.worktree.path)
                    .unwrap_or_else(|_| active.worktree.path.clone())
            })
            .collect()
    }

    /// The project's external worktrees. Serves the cache when younger than
    /// `EXTERNAL_SCAN_INTERVAL`; `force` bypasses the cadence (adoption-time
    /// resolution). A scan error logs and returns the last-known list (or
    /// empty) — `task.list` must stay alive. Errors are only surfaced when
    /// `force` is set.
    fn external_worktrees(
        &mut self,
        project_id: &str,
        force: bool,
    ) -> Result<Vec<ExternalWorktree>, String> {
        let excluded = self.bound_worktree_paths();
        let base = self.base_for(project_id)?;
        let project = self
            .projects
            .iter_mut()
            .find(|p| p.id == project_id)
            .ok_or_else(|| format!("unknown project: {project_id}"))?;
        if !force {
            if let Some(cache) = &project.external_scan {
                if cache.scanned_at.elapsed() < EXTERNAL_SCAN_INTERVAL {
                    return Ok(cache.worktrees.clone());
                }
            }
        }
        match discover_external_worktrees(&project.repo_path, &base, &excluded) {
            Ok(worktrees) => {
                project.external_scan = Some(ExternalScanCache {
                    scanned_at: std::time::Instant::now(),
                    worktrees: worktrees.clone(),
                });
                Ok(worktrees)
            }
            Err(e) => {
                eprintln!("external_worktrees {project_id}: {e}");
                if force {
                    Err(e.to_string())
                } else {
                    Ok(project
                        .external_scan
                        .as_ref()
                        .map(|c| c.worktrees.clone())
                        .unwrap_or_default())
                }
            }
        }
    }

    /// Drop one project's cache so the next poll rescans (adopt/release just
    /// changed what is bound).
    fn invalidate_external_scan(&mut self, project_id: &str) {
        if let Some(project) = self.projects.iter_mut().find(|p| p.id == project_id) {
            project.external_scan = None;
        }
    }

    /// Resolve a client-supplied `worktree_id` against the discovered list
    /// only — a raw path is never accepted. Cache-first; a miss forces one
    /// fresh scan before failing, so a just-appeared worktree resolves without
    /// waiting out the cache.
    fn resolve_external_worktree(
        &mut self,
        project_id: &str,
        worktree_id: &str,
    ) -> Result<ExternalWorktree, String> {
        if let Some(w) = self
            .external_worktrees(project_id, false)?
            .into_iter()
            .find(|w| w.id == worktree_id)
        {
            return Ok(w);
        }
        self.external_worktrees(project_id, true)?
            .into_iter()
            .find(|w| w.id == worktree_id)
            .ok_or_else(|| format!("unknown worktree_id: {worktree_id}"))
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
            // A single accept error must not permanently stop `done` reporting, but
            // a *persistent* one (EMFILE/ENFILE on fd exhaustion) leaves the listener
            // readable so accept returns Err immediately — `continue` alone would spin
            // a worker at 100% CPU and flood the log. Back off between failed accepts;
            // reset the moment one succeeds.
            let mut accept_backoff =
                crate::backoff::Backoff::new(Duration::from_millis(100), Duration::from_secs(5));
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(pair) => {
                        accept_backoff.reset();
                        pair
                    }
                    Err(e) => {
                        let wait = accept_backoff.current();
                        eprintln!("done socket: accept error: {e}; retrying in {wait:?}");
                        tokio::time::sleep(wait).await;
                        accept_backoff.increase();
                        continue;
                    }
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
        let (_, persisted) = self.finish_mutation(task_id.to_string(), active);
        if let Err(e) = persisted {
            eprintln!("on_agent_done {task_id}: {e}");
        }
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
            "models.list" => Ok(json!({
                "models": models::catalog(),
                "efforts": models::EFFORT_LEVELS,
            })),
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
            "task.adopt" => self.task_adopt(params),
            "task.release" => self.task_release(params),
            "worktree.diff" => self.worktree_diff(params),
            "task.plan" => self.task_plan(params),
            "task.stages" => self.task_stages(params),
            "task.stage_doc" => self.task_stage_doc(params),
            "task.stage_approve" => self.task_stage_approve(params),
            "task.stage_dispatch" => self.task_stage_dispatch(params),
            "task.stage_send_notes" => self.task_stage_send_notes(params),
            "task.stage_fix" => self.task_stage_fix(params),
            "task.comment_add" => self.task_comment_add(params),
            "task.comment_delete" => self.task_comment_delete(params),
            "task.set_auto_advance" => self.task_set_auto_advance(params),
            "task.diff" => self.task_diff(params),
            "task.approve_plan" => self.task_approve_plan(params),
            "task.send_notes" => self.task_send_notes(params),
            "task.request_changes" => self.task_request_changes(params),
            "task.resume" => self.task_resume(params),
            "task.approve_merge" => self.task_approve_merge(params),
            "task.git_action" => self.task_git_action(params),
            "task.abandon" => self.task_abandon(params),
            "task.delete" => self.task_delete(params),
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

    /// A session ended: detach it from the terminal so the pump stops encrypting
    /// (and serializing) output frames into a session the relay will just drop.
    fn drop_session(&mut self, session_id: &str) {
        if let Some(term) = self.term.as_mut() {
            term.attached.retain(|snd| snd.session_id() != session_id);
        }
    }

    /// Demote every working task whose harness has crashed/exited or gone quiet
    /// without a `done` to `idle_unreported`, persisting the transition. Returns
    /// the demoted task ids. The scope's quiescence rule: silence is an anomaly
    /// signal, never a completion — without this a crashed agent would leave its
    /// task stuck in planning/building for the daemon's whole life.
    fn mark_idle_tasks(&mut self, quiet_threshold: Duration) -> Vec<String> {
        // For each demoted task, remember whether its harness *exited* (with which
        // code) versus merely fell silent — an exited harness gets the contract's
        // "agent exited unexpectedly (exit code N)" last_error so the crash is
        // legible; a quiet-but-alive one does not.
        let idle: Vec<(String, Option<i32>)> = self
            .tasks
            .iter()
            .filter(|(_, active)| {
                matches!(active.task.state, TaskState::Planning | TaskState::Building)
            })
            .filter_map(|(task_id, active)| {
                if active.harness_exited() {
                    Some((
                        task_id.clone(),
                        Some(active.harness_exit_code().unwrap_or(-1)),
                    ))
                } else if active
                    .harness_idle_for()
                    .is_some_and(|idle| idle >= quiet_threshold)
                {
                    Some((task_id.clone(), None))
                } else {
                    None
                }
            })
            .collect();

        let idle_ids: Vec<String> = idle.iter().map(|(task_id, _)| task_id.clone()).collect();
        for (task_id, exit_code) in idle {
            let Some(mut active) = self.tasks.remove(&task_id) else {
                continue;
            };
            let outcome = match self.project_of(&task_id) {
                Ok(project_id) => match self.orch_for(&project_id) {
                    Ok(orch) => orch.on_idle(&mut active).map_err(err),
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            };
            if let Err(e) = outcome {
                eprintln!("idle monitor {task_id}: {e}");
            }
            if let Some(code) = exit_code {
                active.last_error = Some(format!("agent exited unexpectedly (exit code {code})"));
            }
            let (_, persisted) = self.finish_mutation(task_id.clone(), active);
            if let Err(e) = persisted {
                eprintln!("idle monitor {task_id}: {e}");
            }
        }
        idle_ids
    }

    /// Watch every working task's harness and demote crashed/quiet ones to
    /// `idle_unreported` — the daemon-side driver for [`Self::mark_idle_tasks`].
    pub fn spawn_idle_monitor(
        state: Arc<Mutex<AppState>>,
        quiet_threshold: Duration,
        poll_interval: Duration,
    ) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(poll_interval).await;
                let demoted = state.lock().unwrap().mark_idle_tasks(quiet_threshold);
                for task_id in demoted {
                    eprintln!("idle monitor: {task_id} went idle without a done report");
                }
            }
        });
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

        let model_choice = model_choice_from(params)?;
        let mut active = self
            .orch_for(&project_id)?
            .dispatch(TaskId::new(&task_id), goal, kind, &base, model_choice)
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

        let (view, persisted) = self.finish_mutation(task_id, active);
        persisted?;
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
        // Absent model/effort params inherit the task's dispatch-time choice.
        let model_override = if params.get("model").is_some() || params.get("effort").is_some() {
            Some(model_choice_from(params)?)
        } else {
            None
        };
        let project_id = self.project_of(&task_id)?;
        if self
            .tasks
            .get(&task_id)
            .is_some_and(ActiveTask::is_multi_stage)
        {
            return Err(
                "multi-stage task: approve and dispatch stages individually \
                 (task.stage_approve, task.stage_dispatch)"
                    .to_string(),
            );
        }
        let mut active = self.take(&task_id)?;
        let outcome = (|| -> Result<(), String> {
            self.orch_for(&project_id)?
                .approve_plan(&mut active, model_override)
                .map_err(err)?;
            if self.qa_agent {
                self.simulate_build(&project_id, &mut active)?;
            }
            Ok(())
        })();
        let (view, persisted) = self.finish_mutation(task_id, active);
        outcome?;
        persisted?;
        Ok(view)
    }

    /// Send the reviewer's plan comments back to the agent to revise the plan
    /// (plan_review → planning → … → plan_review).
    fn task_send_notes(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let comments = require_str(params, "comments")?;
        let project_id = self.project_of(&task_id)?;
        if self
            .tasks
            .get(&task_id)
            .is_some_and(ActiveTask::is_multi_stage)
        {
            return Err(
                "multi-stage task: use task.comment_add + task.stage_send_notes".to_string(),
            );
        }
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
        let (view, persisted) = self.finish_mutation(task_id, active);
        outcome?;
        persisted?;
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
        let (view, persisted) = self.finish_mutation(task_id, active);
        outcome?;
        persisted?;
        Ok(view)
    }

    /// Re-dispatch a phase the daemon's death interrupted: a fresh session picks
    /// the surviving worktree back up.
    fn task_resume(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let project_id = self.project_of(&task_id)?;
        let mut active = self.take(&task_id)?;
        let outcome = (|| -> Result<(), String> {
            self.orch_for(&project_id)?
                .resume(&mut active)
                .map_err(err)?;
            if self.qa_agent {
                match active.task.state {
                    TaskState::Planning if active.revising_stage_id.is_some() => {
                        self.simulate_stage_revise(&project_id, &mut active)?;
                    }
                    TaskState::Planning => self.simulate_plan(&project_id, &mut active)?,
                    TaskState::Building if active.is_multi_stage() => {
                        self.qa_drive_stage_chain(&project_id, &mut active)?;
                    }
                    TaskState::Building => self.simulate_build(&project_id, &mut active)?,
                    _ => {}
                }
            }
            Ok(())
        })();
        let (view, persisted) = self.finish_mutation(task_id, active);
        outcome?;
        persisted?;
        Ok(view)
    }

    fn task_approve_merge(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let project_id = self.project_of(&task_id)?;
        let adopted = self
            .tasks
            .get(&task_id)
            .map(|a| a.adopted)
            .ok_or("unknown task_id")?;
        let cleanup = merge_cleanup_from(params, adopted)?;
        let mut active = self.take(&task_id)?;
        let result = self
            .orch_for(&project_id)?
            .approve_merge(&mut active)
            .map_err(err);
        // A merge failure leaves the task at its review gate; record the reason so
        // the board surfaces it (contract: `merge_failed:` message, state unchanged).
        // Only merge failures — a legality error (e.g. a duplicate approval of an
        // already-merged task) carries no `merge_failed:` prefix and must never
        // deface a terminal task with a spurious banner.
        if let Err(message) = &result {
            if message.starts_with("merge_failed:") {
                active.last_error = Some(message.clone());
            }
        }
        // Act on the worktree only after the Merged verdict is durably persisted
        // (contract #3).
        let merged_worktree = result.is_ok().then(|| active.worktree.clone());
        let (view, persisted) = self.finish_mutation(task_id.clone(), active);
        result?;
        persisted?;
        if let Some(worktree) = merged_worktree {
            self.apply_merge_cleanup(&task_id, &project_id, &worktree, cleanup);
        }
        Ok(view)
    }

    /// Prune a merged task's worktree + branch once its `Merged` verdict is durable.
    /// Best-effort and ordered strictly after the persist (contract #3): a crash
    /// before this leaves a surviving worktree that just re-merges as a no-op on
    /// re-approve, never a merged task that boot recovery mislabels as abandoned.
    fn prune_merged_worktree(&self, project_id: &str, worktree: &Worktree) {
        if let Ok(orch) = self.orch_for(project_id) {
            orch.discard_worktree(worktree);
        }
    }

    /// What happens to the worktree + branch after a user-approved merge lands
    /// (spec §5.7): the default keeps today's behavior; `keep` and `release` are
    /// the split-button's other choices.
    fn apply_merge_cleanup(
        &mut self,
        task_id: &str,
        project_id: &str,
        worktree: &Worktree,
        cleanup: MergeCleanup,
    ) {
        match cleanup {
            MergeCleanup::Prune => self.prune_merged_worktree(project_id, worktree),
            MergeCleanup::Keep => {}
            MergeCleanup::Release => {
                if let Some(store) = &self.task_store {
                    if let Err(e) = store.delete(task_id) {
                        eprintln!("merge cleanup release {task_id}: task store: {e}");
                    }
                }
                self.tasks.remove(task_id);
                self.task_project.remove(task_id);
                self.task_created_at.remove(task_id);
                self.invalidate_external_scan(project_id);
            }
        }
    }

    /// Finish-the-worktree git actions from the diff review: `commit` and `push`
    /// keep the worktree (no lifecycle change); `merge` and `merge_push` merge into
    /// the base and end the task. Every action commits outstanding work first.
    fn task_git_action(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let action = require_str(params, "action")?;
        let project_id = self.project_of(&task_id)?;
        let is_merge_action = matches!(action.as_str(), "merge" | "merge_push");
        if !is_merge_action && params.get("cleanup").is_some() {
            return Err("cleanup only applies to merge actions".to_string());
        }
        let adopted = self
            .tasks
            .get(&task_id)
            .map(|a| a.adopted)
            .ok_or("unknown task_id")?;
        let cleanup = if is_merge_action {
            merge_cleanup_from(params, adopted)?
        } else {
            MergeCleanup::Prune // unused for commit/push
        };
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
        // Merge-shaped failures leave the task in review; keep the reason legible.
        if let Err(message) = &result {
            if message.starts_with("merge_failed:") {
                active.last_error = Some(message.clone());
            }
        }
        // A `merge`/`merge_push` that landed leaves the task Merged; act on the
        // worktree only after that verdict is durably persisted (contract #3).
        // `commit`/`push` keep the worktree, so they never match.
        let merged_worktree = (result.is_ok() && matches!(active.task.state, TaskState::Merged))
            .then(|| active.worktree.clone());
        let (view, persisted) = self.finish_mutation(task_id.clone(), active);
        result?;
        persisted?;
        if let Some(worktree) = merged_worktree {
            self.apply_merge_cleanup(&task_id, &project_id, &worktree, cleanup);
        }
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
        let (view, persisted) = self.finish_mutation(task_id, active);
        result?;
        persisted?;
        Ok(view)
    }

    /// Delete a terminal task from the board: prune any leftover worktree + branch,
    /// remove the durable record, and drop the in-memory bookkeeping. Valid only for
    /// terminal tasks (merged/abandoned/failed) — a live task must be abandoned
    /// first, so this never discards work an agent might still be doing.
    fn task_delete(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let active = self.tasks.get(&task_id).ok_or("unknown task_id")?;
        let state = active.task.state.clone();
        if !matches!(
            state,
            TaskState::Merged | TaskState::Abandoned | TaskState::Failed(_)
        ) {
            return Err(format!(
                "task.delete: task is {} — only terminal tasks \
                 (merged/abandoned/failed) can be deleted",
                state_str(&state)
            ));
        }
        let worktree = active.worktree.clone();
        let adopted = active.adopted;
        let project_id = self.task_project.get(&task_id).cloned();

        // Delete the durable record first: if the store fails, nothing else has
        // changed yet, so the task stays intact and consistent on the board.
        if let Some(store) = &self.task_store {
            store
                .delete(&task_id)
                .map_err(|e| format!("task store: {e}"))?;
        }

        // Own the task so we can tear it down. A Failed task keeps its PTY session
        // alive (so the user could reply); kill and reap it, or the harness process
        // leaks and its worktree is pruned out from under a still-running agent.
        let mut active = self.tasks.remove(&task_id).expect("checked above");
        active.end_session();

        // A failed task still holds its worktree; merged/abandoned usually don't.
        // Best-effort prune — never fail the delete on leftover cleanup. Deleting
        // an adopted task's card must never delete the user's files (spec §5.7):
        // delete removes the card, not the worktree it was minted around.
        if worktree.path.exists() && !adopted {
            if let Some(orch) = project_id
                .as_deref()
                .and_then(|pid| self.orch_for(pid).ok())
            {
                orch.discard_worktree(&worktree);
            }
        }

        self.task_project.remove(&task_id);
        self.task_created_at.remove(&task_id);

        // If the worktree outlived the task (an adopted card deleted with its
        // files kept, or a native prune that failed), it is now unbound and must
        // resurface as an external card on the next poll — but the scan cache
        // still excludes the then-bound path. Invalidate so the board refreshes
        // immediately instead of after the ~10s scan cadence (spec §5.7).
        if worktree.path.exists() {
            if let Some(pid) = project_id {
                self.invalidate_external_scan(&pid);
            }
        }
        Ok(json!({ "ok": true }))
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
        if active.is_multi_stage() {
            return Err("multi-stage task: use task.stages / task.stage_doc".to_string());
        }
        // Defense in depth: plan_path is agent-reported (fenced at the `done`
        // tool), so never follow a record that would escape the worktree.
        if !crate::task::is_worktree_contained_path(&active.plan_path) {
            return Err(format!(
                "plan path escapes the worktree: {:?}",
                active.plan_path
            ));
        }
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

    fn task_list(&mut self) -> Value {
        let tasks: Vec<Value> = self
            .tasks
            .iter()
            .map(|(id, active)| self.task_view(id, active))
            .collect();
        let external_worktrees = self.external_worktrees_json();
        json!({ "tasks": tasks, "external_worktrees": external_worktrees })
    }

    /// Every project's external worktrees, ride-along shape for `task.list`
    /// (spec §5.3): scan order per project, projects concatenated in
    /// registration order. A per-project scan failure is already logged inside
    /// `external_worktrees`; it just contributes nothing here.
    fn external_worktrees_json(&mut self) -> Vec<Value> {
        let projects: Vec<(String, String, String)> = self
            .projects
            .iter()
            .map(|p| (p.id.clone(), p.name.clone(), p.base_branch.clone()))
            .collect();
        let mut entries = Vec::new();
        for (project_id, project_name, base_branch) in projects {
            let Ok(worktrees) = self.external_worktrees(&project_id, false) else {
                continue;
            };
            for w in worktrees {
                let adoptable = w.branch.as_deref().is_some_and(|b| b != base_branch);
                entries.push(json!({
                    "worktree_id": w.id,
                    "project_id": project_id,
                    "project": project_name,
                    "path": w.path.display().to_string(),
                    "branch": w.branch,
                    "head_sha": w.head_sha,
                    "head_subject": w.head_subject,
                    "head_age_seconds": w.head_age_seconds,
                    "dirty_files": w.dirty_files,
                    "diffstat": {
                        "files_changed": w.diffstat.files_changed,
                        "insertions": w.diffstat.insertions,
                        "deletions": w.diffstat.deletions,
                    },
                    "adoptable": adoptable,
                }));
            }
        }
        entries
    }

    /// Read-only browse of one external worktree's dirty diff (spec §5.4) —
    /// never adopts.
    fn worktree_diff(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let worktree_id = require_str(params, "worktree_id")?;
        let external = self.resolve_external_worktree(&project_id, &worktree_id)?;
        let base = self.base_for(&project_id)?;
        let diff = crate::diff::diff_against_merge_base(&external.path, &base)
            .map_err(|e| e.to_string())?;
        let files: Vec<Value> = diff
            .files()
            .iter()
            .map(|f| json!({ "path": f.path, "status": format!("{:?}", f.status) }))
            .collect();
        let stat = diff.stat();
        let adoptable = external.branch.as_deref().is_some_and(|b| b != base);
        Ok(json!({
            "worktree_id": external.id,
            "branch": external.branch,
            "head_subject": external.head_subject,
            "dirty_files": external.dirty_files,
            "path": external.path.display().to_string(),
            "adoptable": adoptable,
            "stat": {
                "files_changed": stat.files_changed,
                "insertions": stat.insertions,
                "deletions": stat.deletions,
            },
            "files": files,
            "patch": diff.patch(),
        }))
    }

    /// Mint a Quick-kind task around an external worktree — the first mutating
    /// action on a browsed worktree transparently adopts it (spec §5.5). No
    /// agent session is spawned.
    fn task_adopt(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let worktree_id = require_str(params, "worktree_id")?;
        let model_choice = model_choice_from(params)?;
        // Force a fresh scan: adoption must never act on a stale card (a
        // worktree adopted or removed since the last poll resolves to unknown
        // here).
        let external = self
            .external_worktrees(&project_id, true)?
            .into_iter()
            .find(|w| w.id == worktree_id)
            .ok_or_else(|| format!("unknown worktree_id: {worktree_id}"))?;
        let base = self.base_for(&project_id)?;
        let task_id = format!("task-{}", self.next_id);
        self.next_id += 1;
        let active = self
            .orch_for(&project_id)?
            .adopt(TaskId::new(&task_id), &external, &base, model_choice)
            .map_err(err)?;
        self.task_project
            .insert(task_id.clone(), project_id.clone());
        self.invalidate_external_scan(&project_id);
        let (view, persisted) = self.finish_mutation(task_id, active);
        persisted?;
        Ok(view)
    }

    /// Un-adopt: drop the task record and its binding, leaving the worktree,
    /// branch, and every file untouched (spec §5.6/§7). Legal on adopted tasks
    /// in any non-terminal state.
    fn task_release(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let active = self.tasks.get(&task_id).ok_or("unknown task_id")?;
        if !active.adopted {
            return Err("task.release: only adopted tasks can be released".to_string());
        }
        if active.task.state.is_terminal() {
            return Err(format!(
                "task.release: task is {} — use task.delete to clear it off the board",
                state_str(&active.task.state)
            ));
        }
        if let Some(store) = &self.task_store {
            store
                .delete(&task_id)
                .map_err(|e| format!("task store: {e}"))?;
        }
        let mut active = self.tasks.remove(&task_id).expect("checked above");
        active.end_session();
        let project_id = self.task_project.remove(&task_id);
        self.task_created_at.remove(&task_id);
        if let Some(pid) = project_id {
            self.invalidate_external_scan(&pid);
        }
        Ok(json!({ "ok": true }))
    }

    // --- multi-stage plan surface ----------------------------------------------

    /// The stage board: manifest order, sub-state, and every comment (open and
    /// addressed) per stage. Read-only, like `task.plan`.
    fn task_stages(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let active = self.tasks.get(&task_id).ok_or("unknown task_id")?;
        if !active.is_multi_stage() {
            return Err("not a multi-stage task".to_string());
        }
        let stages: Vec<Value> = active
            .stages
            .iter()
            .map(|stage| {
                let mut view = stage_json(active, stage);
                let comments: Vec<Value> = active
                    .comments
                    .iter()
                    .filter(|c| c.stage_id == stage.id)
                    .map(comment_json)
                    .collect();
                view.as_object_mut()
                    .expect("stage_json returns an object")
                    .insert("comments".to_string(), json!(comments));
                view
            })
            .collect();
        Ok(json!({
            "task_id": task_id,
            "auto_advance": active.auto_advance,
            "current_stage_id": active.current_stage_id,
            "stages": stages,
        }))
    }

    /// Read one stage's plan document from the worktree on demand, exactly as
    /// `task.plan` reads the legacy file.
    fn task_stage_doc(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let active = self.tasks.get(&task_id).ok_or("unknown task_id")?;
        if !active.is_multi_stage() {
            return Err("not a multi-stage task".to_string());
        }
        let stage = active.stage(&stage_id)?;
        // Defense in depth against a corrupted persisted manifest: the `done`
        // validation already fences agent-supplied paths, but never read outside
        // the worktree's plan dir regardless of what the record says.
        if !stage.path.starts_with(".build/plan/")
            || !crate::task::is_worktree_contained_path(&stage.path)
        {
            return Err(format!(
                "stage doc path escapes .build/plan/: {:?}",
                stage.path
            ));
        }
        let path = active.worktree.path.join(&stage.path);
        let contents =
            std::fs::read_to_string(&path).map_err(|e| format!("stage doc not available: {e}"))?;
        Ok(json!({ "stage_id": stage.id, "path": stage.path, "contents": contents }))
    }

    fn task_stage_approve(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let project_id = self.project_of(&task_id)?;
        let mut active = self.take(&task_id)?;
        let outcome = (|| -> Result<(), String> {
            self.orch_for(&project_id)?
                .approve_stage(&mut active, &stage_id)
                .map_err(err)
        })();
        let (view, persisted) = self.finish_mutation(task_id, active);
        outcome?;
        persisted?;
        Ok(view)
    }

    fn task_stage_dispatch(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let model_override = if params.get("model").is_some() || params.get("effort").is_some() {
            Some(model_choice_from(params)?)
        } else {
            None
        };
        let project_id = self.project_of(&task_id)?;
        let mut active = self.take(&task_id)?;
        let outcome = (|| -> Result<(), String> {
            self.orch_for(&project_id)?
                .dispatch_stage(&mut active, &stage_id, model_override)
                .map_err(err)?;
            self.qa_drive_stage_chain(&project_id, &mut active)
        })();
        let (view, persisted) = self.finish_mutation(task_id, active);
        outcome?;
        persisted?;
        Ok(view)
    }

    /// Send a stage's open comments to a fresh plan-revision session — the
    /// persisted open comments ARE the payload, no `comments` param.
    fn task_stage_send_notes(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let project_id = self.project_of(&task_id)?;
        let mut active = self.take(&task_id)?;
        let outcome = (|| -> Result<(), String> {
            self.orch_for(&project_id)?
                .send_stage_notes(&mut active, &stage_id)
                .map_err(err)?;
            if self.qa_agent {
                self.simulate_stage_revise(&project_id, &mut active)?;
            }
            Ok(())
        })();
        let (view, persisted) = self.finish_mutation(task_id, active);
        outcome?;
        persisted?;
        Ok(view)
    }

    fn task_stage_fix(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let note = params
            .get("note")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let project_id = self.project_of(&task_id)?;
        let mut active = self.take(&task_id)?;
        let outcome = (|| -> Result<(), String> {
            self.orch_for(&project_id)?
                .fix_stage(&mut active, &stage_id, &note)
                .map_err(err)?;
            self.qa_drive_stage_chain(&project_id, &mut active)
        })();
        let (view, persisted) = self.finish_mutation(task_id, active);
        outcome?;
        persisted?;
        Ok(view)
    }

    /// Add a structured plan-review comment to a stage. Legal while the task is
    /// non-terminal and the stage is still in plan review (`planned`/`approved`).
    fn task_comment_add(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let body = require_str(params, "body")?;
        let mut active = self.take(&task_id)?;
        let mut minted: Option<StageComment> = None;
        let outcome = (|| -> Result<(), String> {
            if body.trim().is_empty() {
                return Err("comment body must not be empty".to_string());
            }
            if active.task.state.is_terminal() {
                return Err("cannot comment on a terminal task".to_string());
            }
            let stage_state = active.stage(&stage_id)?.state;
            if !matches!(stage_state, StageState::Planned | StageState::Approved) {
                return Err(format!(
                    "comments are only accepted on planned/approved stages (stage is {})",
                    stage_state_str(&stage_state)
                ));
            }
            let anchor = parse_comment_anchor(params.get("anchor"))?;
            let comment = StageComment {
                id: active.mint_comment_id(),
                stage_id: stage_id.clone(),
                anchor,
                body: body.clone(),
                state: CommentState::Open,
                agent_reply: None,
            };
            active.comments.push(comment.clone());
            minted = Some(comment);
            Ok(())
        })();
        let (_, persisted) = self.finish_mutation(task_id, active);
        outcome?;
        persisted?;
        let comment = minted.expect("outcome Ok implies a comment was minted");
        Ok(json!({ "comment": comment_json(&comment) }))
    }

    fn task_comment_delete(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let comment_id = require_str(params, "comment_id")?;
        let mut active = self.take(&task_id)?;
        let outcome = (|| -> Result<(), String> {
            let index = active
                .comments
                .iter()
                .position(|c| c.id == comment_id)
                .ok_or_else(|| format!("unknown comment_id: {comment_id}"))?;
            if active.comments[index].state != CommentState::Open {
                return Err("only open comments can be deleted".to_string());
            }
            active.comments.remove(index);
            Ok(())
        })();
        let (_, persisted) = self.finish_mutation(task_id, active);
        outcome?;
        persisted?;
        Ok(json!({ "ok": true }))
    }

    /// "Run all": arm/disarm auto-advance, and — the run-all trigger — if
    /// enabling it lands on a dispatchable next stage right now, dispatch it
    /// immediately (same path as `task.stage_dispatch`).
    fn task_set_auto_advance(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let enabled = params
            .get("enabled")
            .and_then(Value::as_bool)
            .ok_or("missing required param: enabled")?;
        let project_id = self.project_of(&task_id)?;
        let mut active = self.take(&task_id)?;
        let outcome = (|| -> Result<(), String> {
            if active.task.state.is_terminal() {
                return Err("cannot set auto_advance on a terminal task".to_string());
            }
            active.auto_advance = enabled;
            if enabled && active.task.state == TaskState::PlanReview {
                if let Some(next_id) = dispatchable_next_stage(&active) {
                    self.orch_for(&project_id)?
                        .dispatch_stage(&mut active, &next_id, None)
                        .map_err(err)?;
                    self.qa_drive_stage_chain(&project_id, &mut active)?;
                }
            }
            Ok(())
        })();
        let (view, persisted) = self.finish_mutation(task_id, active);
        outcome?;
        persisted?;
        Ok(view)
    }

    // --- the scripted QA agent ------------------------------------------------

    fn simulate_plan(&self, project_id: &str, active: &mut ActiveTask) -> Result<(), String> {
        let goal = active.task.goal.clone();
        write_in_worktree(
            active,
            ".build/plan/01-first-half.md",
            &format!("# Stage: First half\n\n1. Implement the first half of: {goal}\n"),
        )?;
        write_in_worktree(
            active,
            ".build/plan/02-second-half.md",
            &format!("# Stage: Second half\n\n1. Implement the second half of: {goal}\n"),
        )?;
        let stages = vec![
            StageManifestEntry {
                id: "first-half".to_string(),
                title: "First half".to_string(),
                path: ".build/plan/01-first-half.md".to_string(),
                summary: "First half.".to_string(),
            },
            StageManifestEntry {
                id: "second-half".to_string(),
                title: "Second half".to_string(),
                path: ".build/plan/02-second-half.md".to_string(),
                summary: "Second half.".to_string(),
            },
        ];
        let manifest = serde_json::to_string_pretty(&stages).map_err(|e| e.to_string())?;
        write_in_worktree(active, STAGES_MANIFEST_PATH, &manifest)?;
        self.orch_for(project_id)?
            .on_done(
                active,
                DoneReport {
                    phase: DonePhase::Plan,
                    status: DoneStatus::Completed,
                    summary: format!("Planned: {goal}"),
                    outputs: DoneOutputs {
                        plan_path: Some(STAGES_MANIFEST_PATH.to_string()),
                        stages: Some(stages),
                        ..DoneOutputs::default()
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

    /// Simulate one stage's build session (if it is running) and, since that
    /// always hands off to a validation session, the validation session too —
    /// the QA harness plays both agents so one RPC call lands the stage on a
    /// verdict, exactly as the real pipeline would after two `done` reports.
    fn simulate_stage_build(
        &self,
        project_id: &str,
        active: &mut ActiveTask,
    ) -> Result<(), String> {
        let stage_id = active
            .current_stage_id
            .clone()
            .ok_or_else(|| "QA stage build: no current stage".to_string())?;
        if active.stage(&stage_id)?.state == StageState::Building {
            let content = format!("Implemented stage {stage_id}: {}\n", active.task.goal);
            write_in_worktree(active, &format!("result-{stage_id}.txt"), &content)?;
            self.orch_for(project_id)?
                .on_done(
                    active,
                    DoneReport {
                        phase: DonePhase::Build,
                        status: DoneStatus::Completed,
                        summary: format!("Built stage {stage_id}"),
                        outputs: DoneOutputs::default(),
                    },
                )
                .map_err(err)?;
        }
        if active.stage(&stage_id)?.state == StageState::Validating {
            self.orch_for(project_id)?
                .on_done(
                    active,
                    DoneReport {
                        phase: DonePhase::Validate,
                        status: DoneStatus::Completed,
                        summary: format!("Validated stage {stage_id}"),
                        outputs: DoneOutputs {
                            validation: Some(ValidationReport {
                                passed: true,
                                findings: "QA validation: pass.".to_string(),
                                notes_for_next_stage: "QA notes for the next stage.".to_string(),
                            }),
                            ..DoneOutputs::default()
                        },
                    },
                )
                .map_err(err)?;
        }
        Ok(())
    }

    /// Drive the QA harness through however many stages "run all" chains into:
    /// each `simulate_stage_build` hop may itself trigger the orchestrator's
    /// internal auto-advance dispatch (spec §6.2), which leaves the task
    /// `building` again on the next stage — keep going until it doesn't.
    fn qa_drive_stage_chain(
        &self,
        project_id: &str,
        active: &mut ActiveTask,
    ) -> Result<(), String> {
        if !self.qa_agent {
            return Ok(());
        }
        let max_hops = active.stages.len() + 1;
        for _ in 0..max_hops {
            if !(active.is_multi_stage() && active.task.state == TaskState::Building) {
                return Ok(());
            }
            self.simulate_stage_build(project_id, active)?;
        }
        Err("QA stage chain did not converge".to_string())
    }

    /// Simulate a per-stage plan-revision session: rewrite the stage doc and
    /// resolve every open comment on the stage being revised.
    fn simulate_stage_revise(
        &self,
        project_id: &str,
        active: &mut ActiveTask,
    ) -> Result<(), String> {
        let stage_id = active
            .revising_stage_id
            .clone()
            .ok_or_else(|| "QA stage revise: no stage revision in flight".to_string())?;
        let stage_path = active.stage(&stage_id)?.path.clone();
        let mut contents = std::fs::read_to_string(active.worktree.path.join(&stage_path))
            .map_err(|e| format!("QA stage revise: could not read stage doc: {e}"))?;
        contents.push_str("\n(revised)\n");
        write_in_worktree(active, &stage_path, &contents)?;
        let resolutions: Vec<CommentResolution> = active
            .open_comments_for(&stage_id)
            .into_iter()
            .map(|c| CommentResolution {
                comment_id: c.id.clone(),
                response: "QA: addressed.".to_string(),
            })
            .collect();
        self.orch_for(project_id)?
            .on_done(
                active,
                DoneReport {
                    phase: DonePhase::Revise,
                    status: DoneStatus::Completed,
                    summary: format!("Revised stage {stage_id}"),
                    outputs: DoneOutputs {
                        comment_resolutions: Some(resolutions),
                        ..DoneOutputs::default()
                    },
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
            "last_error": active.last_error,
            "project": project,
            "project_id": project_id,
            "harness": self.harness,
            "model": active.model_choice.model,
            "effort": active.model_choice.effort,
            "auto_advance": active.auto_advance,
            "current_stage_id": active.current_stage_id,
            "adopted": active.adopted,
            "stages": active
                .stages
                .iter()
                .map(|stage| stage_json(active, stage))
                .collect::<Vec<_>>(),
        })
    }

    fn take(&mut self, task_id: &str) -> Result<ActiveTask, String> {
        self.tasks.remove(task_id).ok_or("unknown task_id".into())
    }
}

/// The wire view of one stage (spec §7.3) — id/title/summary/path, its
/// sub-state, its open-comment count, and its validation report if any.
fn stage_json(active: &ActiveTask, stage: &Stage) -> Value {
    let open_comments = active
        .comments
        .iter()
        .filter(|c| c.stage_id == stage.id && c.state == CommentState::Open)
        .count();
    json!({
        "id": stage.id,
        "title": stage.title,
        "summary": stage.summary,
        "path": stage.path,
        "state": stage_state_str(&stage.state),
        "open_comments": open_comments,
        "validation": stage.validation.as_ref().map(|v| json!({
            "passed": v.passed,
            "findings": v.findings,
            "notes_for_next_stage": v.notes_for_next_stage,
        })),
    })
}

fn comment_json(c: &StageComment) -> Value {
    json!({
        "id": c.id,
        "stage_id": c.stage_id,
        "anchor": c.anchor.as_ref().map(|a| json!({
            "heading_path": a.heading_path,
            "snippet": a.snippet,
        })),
        "body": c.body,
        "state": match c.state {
            CommentState::Open => "open",
            CommentState::Addressed => "addressed",
        },
        "agent_reply": c.agent_reply,
    })
}

/// Parse the optional `anchor` param of `task.comment_add`: `null`/absent is a
/// general comment; present, it must carry a string-array `heading_path` and a
/// string `snippet` (capped server-side at 400 chars).
fn parse_comment_anchor(value: Option<&Value>) -> Result<Option<CommentAnchor>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(v) => {
            let heading_path = v
                .get("heading_path")
                .and_then(Value::as_array)
                .ok_or("anchor.heading_path must be an array of strings")?
                .iter()
                .map(|entry| {
                    entry.as_str().map(str::to_string).ok_or_else(|| {
                        "anchor.heading_path must be an array of strings".to_string()
                    })
                })
                .collect::<Result<Vec<String>, String>>()?;
            let snippet = v
                .get("snippet")
                .and_then(Value::as_str)
                .ok_or("anchor.snippet must be a string")?;
            let snippet: String = snippet.chars().take(400).collect();
            Ok(Some(CommentAnchor {
                heading_path,
                snippet,
            }))
        }
    }
}

/// The first stage that `task.stage_dispatch` would currently accept: the
/// earliest stage not yet `validated_passed` — provided it is itself `approved`
/// and every stage before it already passed validation. `None` when nothing is
/// dispatchable right now (mirrors `dispatch_stage`'s gate).
fn dispatchable_next_stage(active: &ActiveTask) -> Option<String> {
    for (index, stage) in active.stages.iter().enumerate() {
        if stage.state == (StageState::Validated { passed: true }) {
            continue;
        }
        return (stage.state == StageState::Approved
            && active.stages[..index]
                .iter()
                .all(|s| s.state == (StageState::Validated { passed: true })))
        .then(|| stage.id.clone());
    }
    None
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
pub(crate) fn expand_tilde(path: &str) -> std::path::PathBuf {
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

/// Parse and validate the optional `model`/`effort` params of a request.
fn model_choice_from(params: &Value) -> Result<ModelChoice, String> {
    let choice = ModelChoice {
        model: params
            .get("model")
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty())
            .map(str::to_string),
        effort: params
            .get("effort")
            .and_then(Value::as_str)
            .filter(|e| !e.is_empty())
            .map(str::to_string),
    };
    choice.validate()?;
    Ok(choice)
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

/// What happens to the worktree + branch after a user-approved merge lands
/// (spec §5.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeCleanup {
    /// Remove the worktree + branch — today's unconditional behavior.
    Prune,
    /// Merge into the base but keep the worktree and branch alive.
    Keep,
    /// Merge, then drop the task record entirely (un-adopt): the worktree and
    /// branch survive and resurface as an external card.
    Release,
}

/// Parse the optional `cleanup` param. Absent → `Prune` (today's behavior).
/// `"release"` is only meaningful for adopted tasks.
fn merge_cleanup_from(params: &Value, adopted: bool) -> Result<MergeCleanup, String> {
    // Distinguish "absent" (→ Prune, today's default) from "present but not a
    // string" — a non-string value must fail fast, never silently collapse to the
    // destructive Prune default (on an adopted task Prune deletes files Build did
    // not create).
    let cleanup = match params.get("cleanup") {
        None | Some(Value::Null) => return Ok(MergeCleanup::Prune),
        Some(value) => value
            .as_str()
            .ok_or_else(|| format!("invalid cleanup: {value} (expected prune|keep|release)"))?,
    };
    match cleanup {
        "prune" => Ok(MergeCleanup::Prune),
        "keep" => Ok(MergeCleanup::Keep),
        "release" if adopted => Ok(MergeCleanup::Release),
        "release" => Err("cleanup: \"release\" is only valid for adopted tasks".to_string()),
        other => Err(format!(
            "invalid cleanup: {other:?} (expected prune|keep|release)"
        )),
    }
}

/// Dispatch one decrypted request frame. `stream.start` and `term.attach` are
/// handled here because they need the shared `Arc` (background producer/pump) and
/// the `SessionSender` (to push live output to this client); everything else runs
/// under a short-held lock.
fn dispatch_frame(state: &Arc<Mutex<AppState>>, sender: SessionSender, frame: Frame) -> Value {
    // A session ended (client `close` frame, or the relay's session_closed on
    // browser disconnect): release its attachments so the bridge stops encrypting
    // terminal output into a session nobody will ever read.
    if frame.frame_type == "close" {
        state.lock().unwrap().drop_session(sender.session_id());
        return json!({ "ok": true });
    }
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
        TaskState::Interrupted(_) => "interrupted".into(),
        TaskState::Merged => "merged".into(),
        TaskState::Abandoned => "abandoned".into(),
    }
}

/// Render a stage sub-state as a stable snake_case string for the wire.
pub fn stage_state_str(state: &StageState) -> String {
    match state {
        StageState::Planned => "planned".into(),
        StageState::Approved => "approved".into(),
        StageState::Building => "building".into(),
        StageState::Built => "built".into(),
        StageState::Validating => "validating".into(),
        StageState::Validated { passed: true } => "validated_passed".into(),
        StageState::Validated { passed: false } => "validated_failed".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::process::Command;
    use tokio::io::AsyncWriteExt;

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

        // Dispatch a standard task → scripted multi-stage plan → plan_review.
        let res = state.handle(req("task.dispatch", json!({ "goal": "add a greeting" })));
        assert_eq!(res["ok"], true);
        let task_id = res["result"]["task_id"].as_str().unwrap().to_string();
        assert_eq!(res["result"]["state"], "plan_review");
        assert_eq!(res["result"]["stages"].as_array().unwrap().len(), 2);

        // The legacy plan surface is retired for a multi-stage task.
        let legacy_plan = state.handle(req("task.plan", json!({ "task_id": task_id })));
        assert_eq!(legacy_plan["ok"], false);

        // Each stage doc is readable through the multi-stage surface.
        let doc = state.handle(req(
            "task.stage_doc",
            json!({ "task_id": task_id, "stage_id": "first-half" }),
        ));
        assert!(doc["result"]["contents"]
            .as_str()
            .unwrap()
            .contains("add a greeting"));

        // Approve + dispatch stage 1 → scripted build + validation → back to
        // plan_review (stage 1 done, stage 2 still ahead).
        state.handle(req(
            "task.stage_approve",
            json!({ "task_id": task_id, "stage_id": "first-half" }),
        ));
        let s1 = state.handle(req(
            "task.stage_dispatch",
            json!({ "task_id": task_id, "stage_id": "first-half" }),
        ));
        assert_eq!(s1["result"]["state"], "plan_review", "{s1:?}");
        let stages = state.handle(req("task.stages", json!({ "task_id": task_id })));
        let first = stages["result"]["stages"][0].clone();
        assert_eq!(first["state"], "validated_passed");
        assert_eq!(first["validation"]["passed"], true);

        // Approve + dispatch stage 2 → the final stage's validation opens review.
        state.handle(req(
            "task.stage_approve",
            json!({ "task_id": task_id, "stage_id": "second-half" }),
        ));
        let s2 = state.handle(req(
            "task.stage_dispatch",
            json!({ "task_id": task_id, "stage_id": "second-half" }),
        ));
        assert_eq!(s2["result"]["state"], "review", "{s2:?}");

        let diff = state.handle(req("task.diff", json!({ "task_id": task_id })));
        let files: Vec<String> = diff["result"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap().to_string())
            .collect();
        assert!(files.contains(&"result-first-half.txt".to_string()));
        assert!(files.contains(&"result-second-half.txt".to_string()));

        // Approve & merge → merged, and the base branch has both stages' files.
        let merged = state.handle(req("task.approve_merge", json!({ "task_id": task_id })));
        assert_eq!(merged["result"]["state"], "merged");
        assert!(repo.join("result-first-half.txt").exists());
        assert!(repo.join("result-second-half.txt").exists());
    }

    #[test]
    fn stage_doc_read_refuses_a_stage_path_that_escapes_the_plan_dir() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let res = state.handle(req("task.dispatch", json!({ "goal": "add a greeting" })));
        let task_id = res["result"]["task_id"].as_str().unwrap().to_string();
        assert_eq!(res["result"]["state"], "plan_review");

        // A corrupted persisted manifest path (the defense-in-depth case: the
        // validation fence failed or an old record predates it) must never let
        // task.stage_doc read outside the worktree's plan dir.
        state
            .tasks
            .get_mut(&task_id)
            .unwrap()
            .stage_mut("first-half")
            .unwrap()
            .path = ".build/plan/../../../../../../etc/hosts".into();
        let res = state.handle(req(
            "task.stage_doc",
            json!({ "task_id": task_id, "stage_id": "first-half" }),
        ));
        assert_eq!(res["ok"], false, "{res:?}");
        assert!(
            res["error"].as_str().unwrap().contains("escapes"),
            "{res:?}"
        );
    }

    #[tokio::test]
    async fn attention_transitions_fire_one_push_notify_each() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/push/notify"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let identity = crate::transport::generate_identity_keypair();
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_notifier(crate::notify::Notifier::new(
            &server.uri(),
            "dev-1",
            &identity.private_key_b64,
        ));

        async fn notifies_after(server: &MockServer, expected: usize) -> usize {
            for _ in 0..100 {
                if server.received_requests().await.unwrap().len() >= expected {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            server.received_requests().await.unwrap().len()
        }

        // Dispatch → (scripted 2-stage plan) → plan_review: exactly one notify.
        let res = state.handle(req("task.dispatch", json!({ "goal": "add a greeting" })));
        assert_eq!(res["result"]["state"], "plan_review");
        assert_eq!(notifies_after(&server, 1).await, 1);

        // Reading the stage board mutates nothing → still one.
        let task_id = res["result"]["task_id"].as_str().unwrap().to_string();
        state.handle(req("task.stages", json!({ "task_id": task_id })));
        assert_eq!(notifies_after(&server, 1).await, 1);

        // Approve + dispatch stage 1: the whole build+validate cycle runs inside
        // this one RPC call (the QA agent drives it to completion before the
        // handler persists), landing back on plan_review — the *same* wire state
        // as before the call, so the throttle fires no notify for it.
        state.handle(req(
            "task.stage_approve",
            json!({ "task_id": task_id, "stage_id": "first-half" }),
        ));
        let s1 = state.handle(req(
            "task.stage_dispatch",
            json!({ "task_id": task_id, "stage_id": "first-half" }),
        ));
        assert_eq!(s1["result"]["state"], "plan_review", "{s1:?}");
        assert_eq!(notifies_after(&server, 1).await, 1);

        // Approve + dispatch stage 2 → the final stage's validation opens
        // review: a genuine state change, a second notify.
        state.handle(req(
            "task.stage_approve",
            json!({ "task_id": task_id, "stage_id": "second-half" }),
        ));
        let s2 = state.handle(req(
            "task.stage_dispatch",
            json!({ "task_id": task_id, "stage_id": "second-half" }),
        ));
        assert_eq!(s2["result"]["state"], "review", "{s2:?}");
        assert_eq!(notifies_after(&server, 2).await, 2);

        // Merge is human-driven; no third notify.
        let merged = state.handle(req("task.approve_merge", json!({ "task_id": task_id })));
        assert_eq!(merged["result"]["state"], "merged");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(server.received_requests().await.unwrap().len(), 2);

        // The notify body carries the opaque task id + a generic kind, and the
        // kinds match the transitions (plan_review → plan_ready, review →
        // task_done). It must never carry the goal text.
        let requests = server.received_requests().await.unwrap();
        let first: crate::notify::NotifyRequest =
            serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(first.task_id, task_id);
        assert_eq!(first.kind, "plan_ready");
        let second: crate::notify::NotifyRequest =
            serde_json::from_slice(&requests[1].body).unwrap();
        assert_eq!(second.kind, "task_done");
        let body = String::from_utf8(requests[0].body.clone()).unwrap();
        assert!(
            !body.contains("greeting"),
            "no goal text in the notify: {body}"
        );
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
    fn stage_send_notes_revises_the_stage_and_resolves_comments() {
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

        // A general comment (no anchor) and an anchored one on the first stage.
        let general = state.handle(req(
            "task.comment_add",
            json!({ "task_id": task_id, "stage_id": "first-half", "body": "split this further" }),
        ));
        assert_eq!(general["ok"], true, "{general:?}");
        assert_eq!(general["result"]["comment"]["anchor"], Value::Null);
        let anchored = state.handle(req(
            "task.comment_add",
            json!({
                "task_id": task_id, "stage_id": "first-half", "body": "use a timestamp",
                "anchor": { "heading_path": ["Stage: First half"], "snippet": "implement the first half" },
            }),
        ));
        assert_eq!(anchored["ok"], true, "{anchored:?}");
        let anchored_id = anchored["result"]["comment"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let stages = state.handle(req("task.stages", json!({ "task_id": task_id })));
        assert_eq!(stages["result"]["stages"][0]["open_comments"], 2);

        // Send the stage's comments back to a fresh plan-revision session (QA
        // rewrites the doc and resolves every open comment) → planned again.
        let upd = state.handle(req(
            "task.stage_send_notes",
            json!({ "task_id": task_id, "stage_id": "first-half" }),
        ));
        assert_eq!(upd["ok"], true, "{upd:?}");
        assert_eq!(upd["result"]["state"], "plan_review");

        let stages = state.handle(req("task.stages", json!({ "task_id": task_id })));
        let first = stages["result"]["stages"][0].clone();
        assert_eq!(first["state"], "planned");
        assert_eq!(first["open_comments"], 0);
        let comments = first["comments"].as_array().unwrap();
        assert!(comments
            .iter()
            .all(|c| c["state"] == "addressed" && c["agent_reply"] == "QA: addressed."));

        // The stage doc changed.
        let doc = state.handle(req(
            "task.stage_doc",
            json!({ "task_id": task_id, "stage_id": "first-half" }),
        ));
        assert!(doc["result"]["contents"]
            .as_str()
            .unwrap()
            .contains("(revised)"));

        // No open comments left → sending notes again is a clean error.
        let bad = state.handle(req(
            "task.stage_send_notes",
            json!({ "task_id": task_id, "stage_id": "first-half" }),
        ));
        assert_eq!(bad["ok"], false);
        assert!(bad["error"].as_str().unwrap().contains("no open comments"));

        // The legacy single-plan surface is off-limits for a multi-stage task.
        let legacy = state.handle(req(
            "task.send_notes",
            json!({ "task_id": task_id, "comments": "anything" }),
        ));
        assert_eq!(legacy["ok"], false);
        assert_eq!(
            legacy["error"],
            "multi-stage task: use task.comment_add + task.stage_send_notes"
        );

        // Deleting an already-addressed comment is rejected.
        let del = state.handle(req(
            "task.comment_delete",
            json!({ "task_id": task_id, "comment_id": anchored_id }),
        ));
        assert_eq!(del["ok"], false);
        assert_eq!(del["error"], "only open comments can be deleted");

        // An unknown comment id is a clean error.
        let unknown = state.handle(req(
            "task.comment_delete",
            json!({ "task_id": task_id, "comment_id": "c-999" }),
        ));
        assert_eq!(unknown["ok"], false);
        assert_eq!(unknown["error"], "unknown comment_id: c-999");
    }

    #[test]
    fn multi_stage_gate_rejections_and_unknown_stage_errors() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo,
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let res = state.handle(req("task.dispatch", json!({ "goal": "gate this" })));
        let task_id = res["result"]["task_id"].as_str().unwrap().to_string();

        // Unknown stage id, on every stage-scoped method.
        for (method, extra) in [
            ("task.stage_doc", json!({})),
            ("task.stage_approve", json!({})),
            ("task.stage_dispatch", json!({})),
            ("task.stage_send_notes", json!({})),
            ("task.stage_fix", json!({})),
            ("task.comment_add", json!({ "body": "hi" })),
        ] {
            let mut params = json!({ "task_id": task_id, "stage_id": "no-such-stage" });
            for (k, v) in extra.as_object().unwrap() {
                params[k] = v.clone();
            }
            let res = state.handle(req(method, params));
            assert_eq!(res["ok"], false, "{method}: {res:?}");
            assert_eq!(
                res["error"], "unknown stage_id: no-such-stage",
                "{method}: {res:?}"
            );
        }

        // Unknown task id.
        let res = state.handle(req("task.stages", json!({ "task_id": "task-999" })));
        assert_eq!(res["error"], "unknown task_id");

        // A legacy-only method on a multi-stage task, exact message (§7.2).
        let res = state.handle(req("task.approve_plan", json!({ "task_id": task_id })));
        assert_eq!(res["ok"], false);
        assert_eq!(
            res["error"],
            "multi-stage task: approve and dispatch stages individually \
             (task.stage_approve, task.stage_dispatch)"
        );

        // Dispatching an unapproved stage is rejected.
        let res = state.handle(req(
            "task.stage_dispatch",
            json!({ "task_id": task_id, "stage_id": "first-half" }),
        ));
        assert_eq!(res["ok"], false);
        assert!(
            res["error"].as_str().unwrap().contains("not approved"),
            "{res:?}"
        );

        // Dispatching stage 2 before stage 1 has passed validation is rejected,
        // even though stage 2 is itself approved.
        state.handle(req(
            "task.stage_approve",
            json!({ "task_id": task_id, "stage_id": "first-half" }),
        ));
        state.handle(req(
            "task.stage_approve",
            json!({ "task_id": task_id, "stage_id": "second-half" }),
        ));
        let res = state.handle(req(
            "task.stage_dispatch",
            json!({ "task_id": task_id, "stage_id": "second-half" }),
        ));
        assert_eq!(res["ok"], false);
        assert!(
            res["error"]
                .as_str()
                .unwrap()
                .contains("has not passed validation yet"),
            "{res:?}"
        );

        // Once stage 1 dispatches (QA drives it to validated_passed) and the
        // task reaches `review`, dispatching any stage again is illegal — the
        // task is no longer at the plan_review gate.
        state.handle(req(
            "task.stage_dispatch",
            json!({ "task_id": task_id, "stage_id": "first-half" }),
        ));
        let done = state.handle(req(
            "task.stage_dispatch",
            json!({ "task_id": task_id, "stage_id": "second-half" }),
        ));
        assert_eq!(done["result"]["state"], "review", "{done:?}");
        let stale = state.handle(req(
            "task.stage_dispatch",
            json!({ "task_id": task_id, "stage_id": "second-half" }),
        ));
        assert_eq!(stale["ok"], false);
        assert!(
            stale["error"]
                .as_str()
                .unwrap()
                .contains("cannot dispatch a stage"),
            "{stale:?}"
        );
    }

    #[test]
    fn comment_add_rejects_missing_stage_wrong_state_and_empty_body() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo,
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let res = state.handle(req("task.dispatch", json!({ "goal": "comment gates" })));
        let task_id = res["result"]["task_id"].as_str().unwrap().to_string();

        // Empty body is rejected.
        let empty = state.handle(req(
            "task.comment_add",
            json!({ "task_id": task_id, "stage_id": "first-half", "body": "   " }),
        ));
        assert_eq!(empty["ok"], false);

        // A malformed anchor is rejected.
        let bad_anchor = state.handle(req(
            "task.comment_add",
            json!({
                "task_id": task_id, "stage_id": "first-half", "body": "hi",
                "anchor": { "heading_path": "not-an-array", "snippet": "x" },
            }),
        ));
        assert_eq!(bad_anchor["ok"], false);

        // Once the stage is validated, comments are no longer accepted.
        state.handle(req(
            "task.stage_approve",
            json!({ "task_id": task_id, "stage_id": "first-half" }),
        ));
        state.handle(req(
            "task.stage_dispatch",
            json!({ "task_id": task_id, "stage_id": "first-half" }),
        ));
        let after = state.handle(req(
            "task.comment_add",
            json!({ "task_id": task_id, "stage_id": "first-half", "body": "too late" }),
        ));
        assert_eq!(after["ok"], false);
        assert_eq!(
            after["error"],
            "comments are only accepted on planned/approved stages (stage is validated_passed)"
        );
    }

    #[test]
    fn set_auto_advance_runs_every_approved_stage_to_review() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo,
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let res = state.handle(req("task.dispatch", json!({ "goal": "run all" })));
        let task_id = res["result"]["task_id"].as_str().unwrap().to_string();

        // Approve both stages up front, then arm run-all with one call: the
        // whole pipeline (stage 1 build+validate → auto-dispatch stage 2 →
        // build+validate) runs inside this single RPC.
        state.handle(req(
            "task.stage_approve",
            json!({ "task_id": task_id, "stage_id": "first-half" }),
        ));
        state.handle(req(
            "task.stage_approve",
            json!({ "task_id": task_id, "stage_id": "second-half" }),
        ));
        let armed = state.handle(req(
            "task.set_auto_advance",
            json!({ "task_id": task_id, "enabled": true }),
        ));
        assert_eq!(armed["ok"], true, "{armed:?}");
        assert_eq!(armed["result"]["state"], "review", "{armed:?}");
        assert_eq!(armed["result"]["auto_advance"], true);

        let stages = state.handle(req("task.stages", json!({ "task_id": task_id })));
        let list = stages["result"]["stages"].as_array().unwrap();
        assert!(list.iter().all(|s| s["state"] == "validated_passed"));

        // Disarming is a pure flag flip; no dispatch fires from a terminal-ish
        // review gate.
        let disarmed = state.handle(req(
            "task.set_auto_advance",
            json!({ "task_id": task_id, "enabled": false }),
        ));
        assert_eq!(disarmed["result"]["auto_advance"], false);
    }

    #[test]
    fn multi_stage_state_persists_and_reloads_mid_flight() {
        let (dir, repo) = init_repo();
        let tasks_dir = dir.path().join("tasks");
        let task_id;
        {
            let mut state = AppState::new(
                repo.clone(),
                dir.path().join("wt"),
                "main",
                true,
                "/tmp/test-mcp.sock",
            )
            .with_task_store(&tasks_dir)
            .unwrap();
            let res = state.handle(req("task.dispatch", json!({ "goal": "reload me" })));
            task_id = res["result"]["task_id"].as_str().unwrap().to_string();
            state.handle(req(
                "task.stage_approve",
                json!({ "task_id": task_id, "stage_id": "first-half" }),
            ));
            state.handle(req(
                "task.stage_dispatch",
                json!({ "task_id": task_id, "stage_id": "first-half" }),
            ));
            state.handle(req(
                "task.comment_add",
                json!({ "task_id": task_id, "stage_id": "second-half", "body": "note for later" }),
            ));
            // Stage 2 is not approved, so this arms run-all without dispatching.
            let armed = state.handle(req(
                "task.set_auto_advance",
                json!({ "task_id": task_id, "enabled": true }),
            ));
            assert_eq!(armed["result"]["state"], "plan_review", "{armed:?}");
        } // daemon dies

        let mut reloaded = AppState::new(
            repo,
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_task_store(&tasks_dir)
        .unwrap();

        let got = reloaded.handle(req("task.get", json!({ "task_id": task_id })));
        assert_eq!(got["result"]["state"], "plan_review", "{got:?}");
        assert_eq!(got["result"]["auto_advance"], true);
        assert_eq!(got["result"]["current_stage_id"], "first-half");
        assert_eq!(got["result"]["stages"][0]["state"], "validated_passed");

        let stages = reloaded.handle(req("task.stages", json!({ "task_id": task_id })));
        let list = stages["result"]["stages"].as_array().unwrap();
        assert_eq!(list[0]["validation"]["passed"], true);
        let second_comments = list[1]["comments"].as_array().unwrap();
        assert_eq!(second_comments.len(), 1);
        assert_eq!(second_comments[0]["body"], "note for later");
        assert_eq!(second_comments[0]["state"], "open");
    }

    #[test]
    fn interrupted_multi_stage_build_resumes_via_stage_routing() {
        use crate::store::TaskStore;

        let (dir, repo) = init_repo();
        let tasks_dir = dir.path().join("tasks");
        // A surviving worktree: its own tiny git repo, exactly like a real
        // worktree the daemon lost track of when it died mid-build.
        let (_wt_dir, surviving_worktree) = init_repo();

        let store = TaskStore::new(&tasks_dir);
        store
            .save(&PersistedTask {
                id: "task-9".into(),
                goal: "multi stage goal".into(),
                kind: TaskKind::Standard,
                project_path: repo.display().to_string(),
                base_branch: "main".into(),
                state: TaskState::Building,
                branch: "build/multi-stage-goal".into(),
                worktree_name: "multi-stage-goal".into(),
                worktree_path: surviving_worktree.display().to_string(),
                plan_path: STAGES_MANIFEST_PATH.to_string(),
                last_summary: None,
                model: None,
                effort: None,
                last_error: None,
                stages: vec![
                    Stage {
                        id: "first-half".into(),
                        title: "First half".into(),
                        path: ".build/plan/01-first-half.md".into(),
                        summary: "First half.".into(),
                        state: StageState::Validated { passed: true },
                        start_sha: Some("deadbeef".into()),
                        validation: Some(ValidationReport {
                            passed: true,
                            findings: "ok".into(),
                            notes_for_next_stage: "watch the seam".into(),
                        }),
                    },
                    Stage {
                        id: "second-half".into(),
                        title: "Second half".into(),
                        path: ".build/plan/02-second-half.md".into(),
                        summary: "Second half.".into(),
                        state: StageState::Building,
                        start_sha: Some("cafef00d".into()),
                        validation: None,
                    },
                ],
                current_stage_id: Some("second-half".into()),
                revising_stage_id: None,
                auto_advance: false,
                comments: Vec::new(),
                adopted: false,
                pending_continuation: false,
                created_at: "2026-07-01T10:00:00Z".into(),
                updated_at: "2026-07-01T10:00:00Z".into(),
            })
            .unwrap();

        let mut state = AppState::new(
            repo,
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_task_store(&tasks_dir)
        .unwrap();

        // The dead build session is legible as `interrupted`, stage state kept.
        let got = state.handle(req("task.get", json!({ "task_id": "task-9" })));
        assert_eq!(got["result"]["state"], "interrupted");
        assert_eq!(got["result"]["needs_attention"], true);
        assert_eq!(got["result"]["stages"][1]["state"], "building");

        // Resume respawns the stage-2 build session (routed by the persisted
        // sub-state, spec §1.3); the QA harness drives it through build →
        // validate, and — this being the final stage — lands on `review`.
        let resumed = state.handle(req("task.resume", json!({ "task_id": "task-9" })));
        assert_eq!(resumed["ok"], true, "{resumed:?}");
        assert_eq!(resumed["result"]["state"], "review", "{resumed:?}");
        assert_eq!(resumed["result"]["stages"][1]["state"], "validated_passed");
    }

    #[test]
    fn legacy_persisted_record_still_uses_the_single_plan_surface() {
        use crate::store::TaskStore;

        let (dir, repo) = init_repo();
        let tasks_dir = dir.path().join("tasks");
        let surviving_worktree = dir.path().join("wt-legacy");
        std::fs::create_dir_all(surviving_worktree.join(".build")).unwrap();
        std::fs::write(
            surviving_worktree.join(".build/plan.md"),
            "# Plan\n1. do the legacy thing.\n",
        )
        .unwrap();

        let store = TaskStore::new(&tasks_dir);
        store
            .save(&PersistedTask {
                id: "task-1".into(),
                goal: "legacy goal".into(),
                kind: TaskKind::Standard,
                project_path: repo.display().to_string(),
                base_branch: "main".into(),
                state: TaskState::PlanReview,
                branch: "build/legacy-goal".into(),
                worktree_name: "legacy-goal".into(),
                worktree_path: surviving_worktree.display().to_string(),
                plan_path: ".build/plan.md".into(),
                last_summary: None,
                model: None,
                effort: None,
                last_error: None,
                stages: Vec::new(),
                current_stage_id: None,
                revising_stage_id: None,
                auto_advance: false,
                comments: Vec::new(),
                adopted: false,
                pending_continuation: false,
                created_at: "2026-07-01T10:00:00Z".into(),
                updated_at: "2026-07-01T10:00:00Z".into(),
            })
            .unwrap();

        let mut state = AppState::new(
            repo,
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_task_store(&tasks_dir)
        .unwrap();

        // stages is empty ⇒ every multi-stage RPC treats it as legacy.
        let stages = state.handle(req("task.stages", json!({ "task_id": "task-1" })));
        assert_eq!(stages["ok"], false);
        assert_eq!(stages["error"], "not a multi-stage task");

        // task.plan still reads the legacy file.
        let plan = state.handle(req("task.plan", json!({ "task_id": "task-1" })));
        assert_eq!(plan["ok"], true, "{plan:?}");
        assert!(plan["result"]["contents"]
            .as_str()
            .unwrap()
            .contains("legacy thing"));

        // task.approve_plan still runs the legacy build (QA simulates it).
        let approved = state.handle(req("task.approve_plan", json!({ "task_id": "task-1" })));
        assert_eq!(approved["result"]["state"], "review", "{approved:?}");
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

        // Dispatch explicitly to project B (a quick task — routing is orthogonal
        // to the plan pipeline) → on merge the file lands in repo B only.
        let res = state.handle(req(
            "task.dispatch",
            json!({ "goal": "greet b", "project_id": proj_b, "kind": "quick" }),
        ));
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["project_id"], proj_b);
        assert_eq!(res["result"]["state"], "review");
        let task_id = res["result"]["task_id"].as_str().unwrap().to_string();
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
    fn tasks_survive_daemon_restart_including_history() {
        let (dir, repo) = init_repo();
        let tasks_dir = dir.path().join("tasks");
        {
            let mut state = AppState::new(
                repo.clone(),
                dir.path().join("wt"),
                "main",
                true,
                "/tmp/test-mcp.sock",
            )
            .with_task_store(&tasks_dir)
            .unwrap();
            let a = state.handle(req("task.dispatch", json!({ "goal": "standard goal" })));
            assert_eq!(a["result"]["state"], "plan_review");
            let b = state.handle(req(
                "task.dispatch",
                json!({ "goal": "quick goal", "kind": "quick" }),
            ));
            let b_id = b["result"]["task_id"].as_str().unwrap().to_string();
            let merged = state.handle(req("task.approve_merge", json!({ "task_id": b_id })));
            assert_eq!(merged["result"]["state"], "merged");
        } // daemon dies

        let mut reloaded = AppState::new(
            repo,
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_task_store(&tasks_dir)
        .unwrap();
        let list = reloaded.handle(req("task.list", json!({})));
        let tasks = list["result"]["tasks"].as_array().unwrap().clone();
        assert_eq!(tasks.len(), 2, "both tasks recovered: {tasks:?}");
        let by_goal = |g: &str| tasks.iter().find(|t| t["goal"] == g).unwrap().clone();
        // A gate state (no live agent) reattaches as-is.
        let standard = by_goal("standard goal");
        assert_eq!(standard["state"], "plan_review");
        assert!(standard["branch"].as_str().unwrap().starts_with("build/"));
        // Merged tasks stay listed: they are history.
        assert_eq!(by_goal("quick goal")["state"], "merged");

        // The recovered stage docs are still readable through the RPC.
        let standard_id = standard["task_id"].as_str().unwrap().to_string();
        let doc = reloaded.handle(req(
            "task.stage_doc",
            json!({ "task_id": standard_id.clone(), "stage_id": "first-half" }),
        ));
        assert_eq!(doc["ok"], true, "{doc:?}");
        assert!(doc["result"]["contents"]
            .as_str()
            .unwrap()
            .contains("standard goal"));

        // New dispatches never reuse a recovered id.
        let c = reloaded.handle(req(
            "task.dispatch",
            json!({ "goal": "third goal", "kind": "quick" }),
        ));
        assert_eq!(c["ok"], true, "{c:?}");
        let c_id = c["result"]["task_id"].as_str().unwrap();
        assert!(
            tasks.iter().all(|t| t["task_id"] != c_id),
            "fresh id after recovery"
        );

        // And a recovered task continues its lifecycle where it left off.
        reloaded.handle(req(
            "task.stage_approve",
            json!({ "task_id": standard_id.clone(), "stage_id": "first-half" }),
        ));
        let cont = reloaded.handle(req(
            "task.stage_dispatch",
            json!({ "task_id": standard_id, "stage_id": "first-half" }),
        ));
        assert_eq!(cont["result"]["state"], "plan_review", "{cont:?}");
    }

    #[test]
    fn working_tasks_surface_interrupted_on_boot_and_resume() {
        use crate::store::{PersistedTask, TaskStore};
        use crate::task::Phase;

        let (dir, repo) = init_repo();
        let tasks_dir = dir.path().join("tasks");
        // The daemon died mid-plan: a durable record in `planning` whose worktree
        // survived on disk.
        let surviving_worktree = dir.path().join("wt-survivor");
        std::fs::create_dir_all(&surviving_worktree).unwrap();
        let store = TaskStore::new(&tasks_dir);
        store
            .save(&PersistedTask {
                id: "task-7".into(),
                goal: "interrupted goal".into(),
                kind: TaskKind::Standard,
                project_path: repo.display().to_string(),
                base_branch: "main".into(),
                state: TaskState::Planning,
                branch: "build/interrupted-goal".into(),
                worktree_name: "interrupted-goal".into(),
                worktree_path: surviving_worktree.display().to_string(),
                plan_path: ".build/plan.md".into(),
                last_summary: None,
                model: None,
                effort: None,
                last_error: None,
                stages: Vec::new(),
                current_stage_id: None,
                revising_stage_id: None,
                auto_advance: false,
                comments: Vec::new(),
                adopted: false,
                pending_continuation: false,
                created_at: "2026-07-01T10:00:00Z".into(),
                updated_at: "2026-07-01T10:00:00Z".into(),
            })
            .unwrap();

        let mut state = AppState::new(
            repo,
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_task_store(&tasks_dir)
        .unwrap();

        // The dead session is legible: the task needs the user, it is not "working".
        let got = state.handle(req("task.get", json!({ "task_id": "task-7" })));
        assert_eq!(got["ok"], true, "{got:?}");
        assert_eq!(got["result"]["state"], "interrupted");
        assert_eq!(got["result"]["needs_attention"], true);
        // The verdict is durable — it survives the *next* restart too.
        assert_eq!(
            store.load_all().unwrap()[0].state,
            TaskState::Interrupted(Phase::Plan)
        );

        // Recovered ids are reserved: the next dispatch mints task-8, not task-1.
        let next = state.handle(req(
            "task.dispatch",
            json!({ "goal": "fresh goal", "kind": "quick" }),
        ));
        assert_eq!(next["result"]["task_id"], "task-8");

        // task.resume re-dispatches the interrupted phase (QA simulates the agent).
        let resumed = state.handle(req("task.resume", json!({ "task_id": "task-7" })));
        assert_eq!(resumed["ok"], true, "{resumed:?}");
        assert_eq!(resumed["result"]["state"], "plan_review");
    }

    #[test]
    fn missing_worktree_abandons_the_task_on_boot() {
        use crate::store::{PersistedTask, TaskStore};

        let (dir, repo) = init_repo();
        let tasks_dir = dir.path().join("tasks");
        let store = TaskStore::new(&tasks_dir);
        store
            .save(&PersistedTask {
                id: "task-3".into(),
                goal: "orphaned goal".into(),
                kind: TaskKind::Quick,
                project_path: repo.display().to_string(),
                base_branch: "main".into(),
                state: TaskState::Building,
                branch: "build/orphaned-goal".into(),
                worktree_name: "orphaned-goal".into(),
                worktree_path: dir.path().join("wt-deleted-by-hand").display().to_string(),
                plan_path: ".build/plan.md".into(),
                last_summary: None,
                model: None,
                effort: None,
                last_error: None,
                stages: Vec::new(),
                current_stage_id: None,
                revising_stage_id: None,
                auto_advance: false,
                comments: Vec::new(),
                adopted: false,
                pending_continuation: false,
                created_at: "2026-07-01T09:00:00Z".into(),
                updated_at: "2026-07-01T09:00:00Z".into(),
            })
            .unwrap();

        let mut state = AppState::new(
            repo,
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_task_store(&tasks_dir)
        .unwrap();

        // The worktree is gone, so the task cannot resume: it lands abandoned
        // (worktree removed, branch kept — exactly that state's meaning) and stays
        // listed as history rather than vanishing.
        let got = state.handle(req("task.get", json!({ "task_id": "task-3" })));
        assert_eq!(got["ok"], true, "{got:?}");
        assert_eq!(got["result"]["state"], "abandoned");
        assert_eq!(
            store.load_all().unwrap()[0].state,
            TaskState::Abandoned,
            "the abandon verdict is persisted"
        );
    }

    #[test]
    fn missing_project_repo_abandons_the_task_on_boot_with_a_reason() {
        // A non-terminal task whose project repo no longer exists can never advance
        // (there is no orchestrator to route to), so boot recovery abandons it with a
        // legible last_error rather than leaving an untouchable orphan. The worktree
        // itself is present, so this exercises the repo-missing branch specifically —
        // not the worktree-missing one.
        use crate::store::{PersistedTask, TaskStore};

        let (dir, repo) = init_repo();
        let tasks_dir = dir.path().join("tasks");
        // The worktree survives on disk; only the project repo is gone.
        let live_worktree = dir.path().join("live-worktree");
        std::fs::create_dir_all(&live_worktree).unwrap();
        let missing_repo = dir.path().join("repo-deleted-by-hand");

        let store = TaskStore::new(&tasks_dir);
        store
            .save(&PersistedTask {
                id: "task-4".into(),
                goal: "work whose repo vanished".into(),
                kind: TaskKind::Quick,
                project_path: missing_repo.display().to_string(),
                base_branch: "main".into(),
                state: TaskState::Building,
                branch: "build/vanished".into(),
                worktree_name: "vanished".into(),
                worktree_path: live_worktree.display().to_string(),
                plan_path: ".build/plan.md".into(),
                last_summary: None,
                model: None,
                effort: None,
                last_error: None,
                stages: Vec::new(),
                current_stage_id: None,
                revising_stage_id: None,
                auto_advance: false,
                comments: Vec::new(),
                adopted: false,
                pending_continuation: false,
                created_at: "2026-07-01T09:00:00Z".into(),
                updated_at: "2026-07-01T09:00:00Z".into(),
            })
            .unwrap();

        let mut state = AppState::new(
            repo,
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_task_store(&tasks_dir)
        .unwrap();

        let got = state.handle(req("task.get", json!({ "task_id": "task-4" })));
        assert_eq!(got["ok"], true, "{got:?}");
        assert_eq!(got["result"]["state"], "abandoned");
        assert_eq!(
            got["result"]["last_error"],
            format!("project repo missing at {}", missing_repo.display())
        );
        // The abandon verdict AND its reason are persisted, so they survive the next
        // restart too.
        let persisted = store.load_all().unwrap();
        assert_eq!(persisted[0].state, TaskState::Abandoned);
        assert!(persisted[0]
            .last_error
            .as_deref()
            .unwrap()
            .starts_with("project repo missing at"));
    }

    /// The pruning principle's automated-action gate (spec §0.5 / §5.8): boot
    /// recovery is automated, so a repo-missing ADOPTED task is parked
    /// needs-attention (Review stays Review; a working state demotes to
    /// Interrupted) instead of being written off as abandoned. Native records
    /// with the same setup still auto-abandon.
    #[test]
    fn boot_recovery_parks_adopted_tasks_when_the_repo_is_gone() {
        use crate::store::{PersistedTask, TaskStore};

        let (dir, repo) = init_repo();
        let tasks_dir = dir.path().join("tasks");
        let missing_repo = dir.path().join("repo-deleted-by-hand");
        let store = TaskStore::new(&tasks_dir);

        let record = |id: &str, state: TaskState, adopted: bool, created: &str| {
            // Each task's user worktree survives on disk; only the repo is gone.
            let worktree = dir.path().join(format!("user-wt-{id}"));
            std::fs::create_dir_all(&worktree).unwrap();
            PersistedTask {
                id: id.into(),
                goal: format!("goal for {id}"),
                kind: TaskKind::Quick,
                project_path: missing_repo.display().to_string(),
                base_branch: "main".into(),
                state,
                branch: format!("user/{id}"),
                worktree_name: id.into(),
                worktree_path: worktree.display().to_string(),
                plan_path: ".build/plan.md".into(),
                last_summary: None,
                model: None,
                effort: None,
                last_error: None,
                stages: Vec::new(),
                current_stage_id: None,
                revising_stage_id: None,
                auto_advance: false,
                comments: Vec::new(),
                adopted,
                pending_continuation: adopted,
                created_at: created.into(),
                updated_at: created.into(),
            }
        };
        store
            .save(&record(
                "task-1",
                TaskState::Review,
                true,
                "2026-07-01T09:00:00Z",
            ))
            .unwrap();
        store
            .save(&record(
                "task-2",
                TaskState::Building,
                true,
                "2026-07-01T09:01:00Z",
            ))
            .unwrap();
        store
            .save(&record(
                "task-3",
                TaskState::Review,
                false,
                "2026-07-01T09:02:00Z",
            ))
            .unwrap();

        let mut state = AppState::new(
            repo,
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_task_store(&tasks_dir)
        .unwrap();

        // Adopted, gate state: parked as-is — still in review, with the reason.
        let parked = state.handle(req("task.get", json!({ "task_id": "task-1" })));
        assert_eq!(parked["ok"], true, "{parked:?}");
        assert_eq!(parked["result"]["state"], "review");
        assert_eq!(
            parked["result"]["last_error"],
            format!("project repo missing at {}", missing_repo.display())
        );

        // Adopted, working state: its session is gone, so it parks interrupted.
        let interrupted = state.handle(req("task.get", json!({ "task_id": "task-2" })));
        assert_eq!(interrupted["result"]["state"], "interrupted");

        // Native record with the identical setup still auto-abandons (unchanged).
        let native = state.handle(req("task.get", json!({ "task_id": "task-3" })));
        assert_eq!(native["result"]["state"], "abandoned");

        // The parked verdicts (and their worktrees) are durable and untouched.
        let persisted = store.load_all().unwrap();
        assert_eq!(persisted[0].state, TaskState::Review);
        assert!(persisted[0]
            .last_error
            .as_deref()
            .unwrap()
            .starts_with("project repo missing at"));
        assert!(
            persisted[0].adopted,
            "the adopted flag survives the parking"
        );
        // The real repo path survives the parking write — without it a restored
        // repo could never un-park the task (it would read project_path "").
        assert_eq!(
            persisted[0].project_path,
            missing_repo.display().to_string(),
            "parking must not erase the project path"
        );
        assert_eq!(
            persisted[1].state,
            TaskState::Interrupted(crate::task::Phase::Build)
        );
        assert_eq!(persisted[2].state, TaskState::Abandoned);
        assert!(dir.path().join("user-wt-task-1").exists());
        assert!(dir.path().join("user-wt-task-2").exists());

        // A parked adopted task has no project mapping (§5.8) — task.release is
        // its escape hatch, and it works with no project resolution at all.
        let released = state.handle(req("task.release", json!({ "task_id": "task-1" })));
        assert_eq!(released["ok"], true, "{released:?}");
        assert_eq!(released["result"]["ok"], true);
        let got = state.handle(req("task.get", json!({ "task_id": "task-1" })));
        assert_eq!(got["ok"], false, "released task is gone from the board");
        assert!(
            dir.path().join("user-wt-task-1").exists(),
            "files untouched"
        );
    }

    #[test]
    fn corrupt_task_file_fails_boot_naming_the_file() {
        let (dir, repo) = init_repo();
        let tasks_dir = dir.path().join("tasks");
        std::fs::create_dir_all(&tasks_dir).unwrap();
        std::fs::write(tasks_dir.join("task-9.json"), "{ definitely not json").unwrap();

        let err = match AppState::new(
            repo,
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_task_store(&tasks_dir)
        {
            Ok(_) => panic!("a corrupt task file must fail boot, not drop the task"),
            Err(e) => e,
        };
        assert!(err.contains("task-9.json"), "error names the file: {err}");
    }

    #[tokio::test]
    async fn crashed_harness_is_marked_idle_with_the_exit_code() {
        // A working task whose harness exits non-zero without ever calling `done`
        // must land in idle_unreported within seconds, carrying the exit code so the
        // crash is legible (contract #5).
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let orch = Orchestrator::new(
            repo.clone(),
            dir.path().join("wt-side"),
            Agent::Warm(HarnessSpec::new("sh").arg("-c").arg("exit 7")),
            Templates::default(),
        );
        let active = orch
            .dispatch(
                crate::task::TaskId::new("task-9"),
                "crashy work",
                TaskKind::Quick,
                "main",
                Default::default(),
            )
            .unwrap();
        let project_id = state.projects[0].id.clone();
        state.task_project.insert("task-9".into(), project_id);
        state.tasks.insert("task-9".into(), active);

        // Poll like the 5s monitor does; a generous threshold so only the *exit*
        // (not quiescence) drives the demotion.
        let mut demoted = Vec::new();
        for _ in 0..50 {
            demoted = state.mark_idle_tasks(Duration::from_secs(3600));
            if !demoted.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(demoted, vec!["task-9".to_string()]);
        let got = state.handle(req("task.get", json!({ "task_id": "task-9" })));
        assert_eq!(got["result"]["state"], "idle_unreported");
        assert_eq!(
            got["result"]["last_error"],
            "agent exited unexpectedly (exit code 7)"
        );
    }

    #[tokio::test]
    async fn done_socket_routes_reports_and_keeps_serving_after_each_connection() {
        // The done control socket must process a forwarded `done` report AND keep
        // accepting further connections — a regression to break-on-first-connection
        // (or a spin/stall) would leave later agents unable to report.
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let project_id = state.projects[0].id.clone();

        // Two Building tasks (no live session needed — on_done only applies events).
        for id in ["task-1", "task-2"] {
            let mut task = Task::new(TaskId::new(id), "do work".to_string(), TaskKind::Quick);
            task.apply(TaskEvent::Dispatch).unwrap();
            assert_eq!(task.state, TaskState::Building);
            let worktree = Worktree {
                name: id.into(),
                path: dir.path().join(format!("wt-{id}")),
                branch: format!("build/{id}"),
                base_branch: "main".into(),
            };
            let active = ActiveTask::reattach(
                task,
                worktree,
                ".build/plan.md".into(),
                None,
                Default::default(),
                None,
                Vec::new(),
                None,
                None,
                false,
                Vec::new(),
                false,
                false,
            );
            state.task_project.insert(id.into(), project_id.clone());
            state.tasks.insert(id.into(), active);
        }

        let app = state.shared();
        let sock_path = dir.path().join("done.sock");
        AppState::spawn_done_socket(app.clone(), sock_path.display().to_string());

        let send_blocked = |id: &'static str| {
            let sock_path = sock_path.clone();
            async move {
                let report = DoneReport {
                    phase: DonePhase::Build,
                    status: DoneStatus::Blocked,
                    summary: format!("{id} is stuck"),
                    outputs: DoneOutputs::default(),
                };
                let line = format!(
                    "{}\n",
                    json!({ "task_id": id, "report": serde_json::to_value(&report).unwrap() })
                );
                // The listener binds asynchronously; retry until it is up.
                let mut stream = None;
                for _ in 0..100 {
                    if let Ok(s) = tokio::net::UnixStream::connect(&sock_path).await {
                        stream = Some(s);
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                let mut stream = stream.expect("done socket never came up");
                stream.write_all(line.as_bytes()).await.unwrap();
                stream.flush().await.unwrap();
                // Dropping the stream closes it, ending the server's per-conn reader.
            }
        };

        let wait_blocked = |id: &'static str| {
            let app = app.clone();
            async move {
                for _ in 0..100 {
                    let blocked = matches!(
                        app.lock()
                            .unwrap()
                            .tasks
                            .get(id)
                            .map(|a| a.task.state.clone()),
                        Some(TaskState::Blocked(_))
                    );
                    if blocked {
                        return true;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                false
            }
        };

        // First connection: task-1 is routed to blocked.
        send_blocked("task-1").await;
        assert!(wait_blocked("task-1").await, "first report must route");

        // A SECOND, independent connection proves the accept loop kept serving.
        send_blocked("task-2").await;
        assert!(
            wait_blocked("task-2").await,
            "the socket must keep accepting connections after the first"
        );
    }

    #[test]
    fn merge_failure_sets_last_error_and_keeps_the_review_state() {
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

        // Wander the primary checkout off base so the merge can't land.
        assert!(Command::new("git")
            .args(["checkout", "-b", "user-feature"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());

        let failed = state.handle(req("task.approve_merge", json!({ "task_id": task_id })));
        assert_eq!(failed["ok"], false);
        assert!(
            failed["error"]
                .as_str()
                .unwrap()
                .starts_with("merge_failed:"),
            "{failed:?}"
        );

        // The task stays in review, now with the reason recorded for the banner.
        let got = state.handle(req("task.get", json!({ "task_id": task_id })));
        assert_eq!(got["result"]["state"], "review");
        assert!(got["result"]["last_error"]
            .as_str()
            .unwrap()
            .starts_with("merge_failed:"));

        // Back on base, a retry merges and clears the error.
        assert!(Command::new("git")
            .args(["checkout", "main"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());
        let merged = state.handle(req("task.approve_merge", json!({ "task_id": task_id })));
        assert_eq!(merged["result"]["state"], "merged");
        assert!(merged["result"]["last_error"].is_null(), "cleared on merge");
    }

    #[test]
    fn a_merge_whose_persist_fails_keeps_its_worktree_for_self_healing() {
        // Contract #3 ordering: the Merged verdict is persisted BEFORE the worktree
        // and branch are pruned. If the persist fails (here: the task store dir made
        // unwritable), the RPC errors and the worktree must survive — on the next
        // boot the stored record still says `review`, its worktree is intact, and a
        // re-approve re-merges as a no-op. If cleanup ran before the persist, a crash
        // in that window would delete the branch and make boot recovery mislabel the
        // merged work as `Abandoned`.
        let (dir, repo) = init_repo();
        let tasks_dir = dir.path().join("tasks");
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_task_store(&tasks_dir)
        .unwrap();

        let quick = state.handle(req(
            "task.dispatch",
            json!({ "goal": "persist race", "kind": "quick" }),
        ));
        let quick_id = quick["result"]["task_id"].as_str().unwrap().to_string();
        assert_eq!(quick["result"]["state"], "review");
        let worktree_path = state.tasks[&quick_id].worktree.path.clone();
        assert!(worktree_path.exists());

        // Make the next persist fail: the store can no longer create its tmp file.
        std::fs::set_permissions(&tasks_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

        let merged = state.handle(req("task.approve_merge", json!({ "task_id": quick_id })));
        // Restore write access before any assertion can unwind and leak the temp dir.
        std::fs::set_permissions(&tasks_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(merged["ok"], false, "persist failure surfaces: {merged:?}");
        assert!(merged["error"].as_str().unwrap().contains("task store"));
        // The merge landed in git, but the worktree is untouched because the persist
        // never succeeded — cleanup is strictly after a durable Merged.
        assert!(
            worktree_path.exists(),
            "worktree pruned before the verdict was persisted"
        );
    }

    #[test]
    fn duplicate_approve_merge_never_defaces_a_merged_task() {
        // Contract #1: last_error carries ONLY a merge failure (or a harness crash).
        // A second approve_merge on an already-merged task (two open windows, or a
        // retried RPC) fails the up-front legality check with an "illegal transition"
        // error that has no `merge_failed:` prefix — that must NOT be recorded as
        // last_error, or the merged card shows a spurious ⚠ banner forever.
        let (dir, repo) = init_repo();
        let tasks_dir = dir.path().join("tasks");
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_task_store(&tasks_dir)
        .unwrap();

        let quick = state.handle(req(
            "task.dispatch",
            json!({ "goal": "quick work", "kind": "quick" }),
        ));
        let quick_id = quick["result"]["task_id"].as_str().unwrap().to_string();
        let merged = state.handle(req("task.approve_merge", json!({ "task_id": quick_id })));
        assert_eq!(merged["result"]["state"], "merged");
        assert!(merged["result"]["last_error"].is_null());

        // The duplicate approval errors, but the merged task stays clean.
        let dup = state.handle(req("task.approve_merge", json!({ "task_id": quick_id })));
        assert_eq!(dup["ok"], false);
        assert!(
            !dup["error"].as_str().unwrap().starts_with("merge_failed:"),
            "the legality error is not a merge failure: {dup:?}"
        );

        let got = state.handle(req("task.get", json!({ "task_id": quick_id })));
        assert_eq!(got["result"]["state"], "merged");
        assert!(
            got["result"]["last_error"].is_null(),
            "a merged task must never be defaced by a duplicate approval: {got:?}"
        );
        // The persisted record is clean too — the spurious banner must not survive a
        // restart either.
        let persisted = crate::store::TaskStore::new(&tasks_dir)
            .load_all()
            .unwrap()
            .into_iter()
            .find(|r| r.id == quick_id)
            .expect("record persisted");
        assert!(persisted.last_error.is_none(), "{persisted:?}");
    }

    #[test]
    fn delete_is_terminal_only_and_removes_the_task() {
        let (dir, repo) = init_repo();
        let tasks_dir = dir.path().join("tasks");
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_task_store(&tasks_dir)
        .unwrap();

        // A live (non-terminal) task cannot be deleted.
        let live = state.handle(req("task.dispatch", json!({ "goal": "still going" })));
        let live_id = live["result"]["task_id"].as_str().unwrap().to_string();
        assert_eq!(live["result"]["state"], "plan_review");
        let refused = state.handle(req("task.delete", json!({ "task_id": live_id })));
        assert_eq!(refused["ok"], false);
        assert!(refused["error"].as_str().unwrap().contains("terminal"));

        // A merged task can: it disappears from the board and its record is gone.
        let quick = state.handle(req(
            "task.dispatch",
            json!({ "goal": "quick work", "kind": "quick" }),
        ));
        let quick_id = quick["result"]["task_id"].as_str().unwrap().to_string();
        state.handle(req("task.approve_merge", json!({ "task_id": quick_id })));
        let deleted = state.handle(req("task.delete", json!({ "task_id": quick_id })));
        assert_eq!(deleted["ok"], true, "{deleted:?}");
        assert_eq!(deleted["result"]["ok"], true);
        let list = state.handle(req("task.list", json!({})));
        let ids: Vec<&str> = list["result"]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["task_id"].as_str().unwrap())
            .collect();
        assert!(
            !ids.contains(&quick_id.as_str()),
            "deleted task gone: {ids:?}"
        );
        assert!(!tasks_dir.join(format!("{quick_id}.json")).exists());

        // Deleting a task that doesn't exist is a clean error, not a panic.
        let missing = state.handle(req("task.delete", json!({ "task_id": "task-999" })));
        assert_eq!(missing["ok"], false);
    }

    #[tokio::test]
    async fn delete_kills_a_failed_tasks_live_harness() {
        // A Failed task keeps its PTY session alive on purpose (the user can reply).
        // Deleting it must kill AND reap that harness first — otherwise the agent
        // process leaks (or lingers as an unreaped zombie) and its worktree is pruned
        // out from under a still-running process.
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        // A warm harness that stays alive well past the test, standing in for a real
        // interactive CLI still running after a done(failed).
        let orch = Orchestrator::new(
            repo.clone(),
            dir.path().join("wt-side"),
            Agent::Warm(HarnessSpec::new("sh").arg("-c").arg("sleep 120")),
            Templates::default(),
        );
        let active = orch
            .dispatch(
                crate::task::TaskId::new("task-9"),
                "crashy work",
                TaskKind::Quick,
                "main",
                Default::default(),
            )
            .unwrap();
        let project_id = state.projects[0].id.clone();
        state.task_project.insert("task-9".into(), project_id);
        state.tasks.insert("task-9".into(), active);

        // The agent reports failed: the task goes Failed but its session stays live.
        state.on_agent_done(
            "task-9",
            DoneReport {
                phase: DonePhase::Build,
                status: DoneStatus::Failed,
                summary: "gave up".into(),
                outputs: DoneOutputs::default(),
            },
        );
        let got = state.handle(req("task.get", json!({ "task_id": "task-9" })));
        assert_eq!(got["result"]["state"], "failed");
        let pid = state.tasks["task-9"]
            .harness_pid()
            .expect("failed task still holds a live harness");

        // Delete the terminal task.
        let deleted = state.handle(req("task.delete", json!({ "task_id": "task-9" })));
        assert_eq!(deleted["result"]["ok"], true, "{deleted:?}");

        // The harness must be gone — kill -0 returns ESRCH once it is reaped.
        let alive = Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .unwrap()
            .success();
        assert!(
            !alive,
            "delete must kill and reap the failed task's harness (pid {pid})"
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

    #[tokio::test]
    async fn idle_monitor_demotes_a_quiet_harness_to_idle_unreported() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        // A warm harness that never speaks and never calls `done` — the crashed/
        // silent agent case. Dispatched through a side orchestrator so the task
        // sits in `Building` with a live but mute session.
        let orch = Orchestrator::new(
            repo.clone(),
            dir.path().join("wt-side"),
            Agent::Warm(HarnessSpec::new("sh").arg("-c").arg("sleep 30")),
            Templates::default(),
        );
        let active = orch
            .dispatch(
                crate::task::TaskId::new("task-9"),
                "quiet work",
                TaskKind::Quick,
                "main",
                Default::default(),
            )
            .unwrap();
        assert_eq!(active.task.state, TaskState::Building);
        let project_id = state.projects[0].id.clone();
        state.task_project.insert("task-9".into(), project_id);
        state.tasks.insert("task-9".into(), active);

        // Under a generous threshold nothing is idle yet.
        assert!(state.mark_idle_tasks(Duration::from_secs(3600)).is_empty());

        // The PTY has been silent since the prompt echo; a tiny threshold demotes.
        tokio::time::sleep(Duration::from_millis(150)).await;
        let demoted = state.mark_idle_tasks(Duration::from_millis(50));
        assert_eq!(demoted, vec!["task-9".to_string()]);
        assert_eq!(
            state.tasks.get("task-9").unwrap().task.state,
            TaskState::IdleUnreported(crate::task::Phase::Build)
        );

        // A second sweep is a no-op: the task is no longer in a working state.
        assert!(state.mark_idle_tasks(Duration::from_millis(50)).is_empty());
    }

    #[tokio::test]
    async fn a_close_frame_detaches_the_sessions_terminal_sender() {
        let (dir, repo) = init_repo();
        let state = AppState::new(
            repo,
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .shared();
        {
            let mut s = state.lock().unwrap();
            let (term, _rx) = TermSession::spawn(80, 24).unwrap();
            s.term = Some(term);
            let term = s.term.as_mut().unwrap();
            term.attached.push(SessionSender::detached("s-live"));
            term.attached.push(SessionSender::detached("s-dead"));
        }

        let close = Frame {
            session_id: "s-dead".into(),
            message_id: String::new(),
            frame_type: "close".into(),
            sender: "relay".into(),
            created_at: String::new(),
            payload: Value::Null,
        };
        let response = dispatch_frame(&state, SessionSender::detached("s-dead"), close);
        assert_eq!(response["ok"], true);

        let s = state.lock().unwrap();
        let attached: Vec<&str> = s
            .term
            .as_ref()
            .unwrap()
            .attached
            .iter()
            .map(SessionSender::session_id)
            .collect();
        assert_eq!(
            attached,
            vec!["s-live"],
            "only the closed session's sender is dropped"
        );
    }

    #[test]
    fn models_list_serves_the_catalog_and_effort_levels() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(repo, dir.path().join("wt"), "main", true, "/tmp/m.sock");
        let res = state.handle(req("models.list", json!({})));
        assert_eq!(res["ok"], true, "{res:?}");
        let models = res["result"]["models"].as_array().unwrap();
        assert!(models.iter().any(|m| m["id"] == "claude-opus-4-8"));
        assert!(models.iter().all(|m| m["supports_effort"].is_boolean()));
        let efforts = res["result"]["efforts"].as_array().unwrap();
        assert!(efforts.iter().any(|e| e == "xhigh"));
    }

    #[test]
    fn dispatch_stores_the_model_choice_and_rejects_bad_ones() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(repo, dir.path().join("wt"), "main", true, "/tmp/m.sock");
        let res = state.handle(req(
            "task.dispatch",
            json!({ "goal": "g", "model": "claude-sonnet-5", "effort": "high" }),
        ));
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["model"], "claude-sonnet-5");
        assert_eq!(res["result"]["effort"], "high");
        let task_id = res["result"]["task_id"].as_str().unwrap().to_string();
        let got = state.handle(req("task.get", json!({ "task_id": task_id })));
        assert_eq!(got["result"]["model"], "claude-sonnet-5");

        let bad = state.handle(req(
            "task.dispatch",
            json!({ "goal": "g2", "effort": "ultra" }),
        ));
        assert_eq!(bad["ok"], false);
        let bad = state.handle(req(
            "task.dispatch",
            json!({ "goal": "g3", "model": "--model" }),
        ));
        assert_eq!(bad["ok"], false);
    }

    #[test]
    fn real_harness_argv_includes_the_selected_model_and_effort() {
        let Agent::OneShot(build) = build_agent(false, "/tmp/m.sock".into()) else {
            panic!("real agent should be one-shot");
        };
        let choice = ModelChoice {
            model: Some("claude-opus-4-8".into()),
            effort: Some("xhigh".into()),
        };
        let spec = build("do the thing", &choice, &SpawnOptions::default());
        let args = spec.args.join(" ");
        assert!(args.contains("--model claude-opus-4-8"), "{args}");
        assert!(args.contains("--effort xhigh"), "{args}");
        assert!(!args.contains("--continue"), "{args}");
        // Defaults add nothing: the user's harness config decides.
        let spec = build(
            "do the thing",
            &ModelChoice::default(),
            &SpawnOptions::default(),
        );
        assert!(!spec.args.join(" ").contains("--model"));
        // A continuation spawn resumes the cwd's conversation, flag placed right
        // after the permission arg and before any model args.
        let spec = build(
            "do the thing",
            &choice,
            &SpawnOptions {
                continue_session: true,
            },
        );
        let args = spec.args.join(" ");
        assert!(
            args.contains("--dangerously-skip-permissions --continue --model"),
            "{args}"
        );
    }

    #[test]
    fn encode_claude_project_dir_and_probe() {
        // Claude Code's transcript dir encoding: '/' and '.' both become '-'.
        assert_eq!(
            encode_claude_project_dir(std::path::Path::new("/Users/z/proj.web")),
            "-Users-z-proj-web"
        );

        let root = tempfile::tempdir().unwrap();
        let cwd = std::path::Path::new("/Users/z/proj.web");
        assert!(
            !claude_transcript_exists(root.path(), cwd),
            "no encoded dir → no transcript"
        );
        let encoded_dir = root.path().join("-Users-z-proj-web");
        std::fs::create_dir_all(&encoded_dir).unwrap();
        assert!(
            !claude_transcript_exists(root.path(), cwd),
            "an empty dir holds no transcript"
        );
        std::fs::write(encoded_dir.join("notes.txt"), "not a transcript").unwrap();
        assert!(
            !claude_transcript_exists(root.path(), cwd),
            "only .jsonl files count"
        );
        std::fs::write(encoded_dir.join("session.jsonl"), "{}\n").unwrap();
        assert!(claude_transcript_exists(root.path(), cwd));
    }

    #[test]
    fn unrooted_state_has_no_phantom_project_until_one_is_added() {
        let (dir, repo) = init_repo();
        let mut state =
            AppState::new_unrooted(dir.path().join("wt"), "main", true, "/tmp/test-mcp.sock");
        let listed = state.dispatch("project.list", &json!({})).unwrap();
        assert_eq!(listed["projects"].as_array().unwrap().len(), 0);

        let added = state
            .dispatch("project.add", &json!({"path": repo.to_string_lossy()}))
            .unwrap();
        assert!(added["project_id"].as_str().unwrap().starts_with("proj-"));
        let listed = state.dispatch("project.list", &json!({})).unwrap();
        assert_eq!(listed["projects"].as_array().unwrap().len(), 1);
    }

    /// Add a git worktree Build did not create, at `dir/name` on `branch`, cut
    /// from `repo`'s current HEAD — the raw material of adoption tests.
    fn add_external_worktree(
        repo: &std::path::Path,
        dir: &std::path::Path,
        name: &str,
        branch: &str,
    ) -> PathBuf {
        let path = dir.join(name);
        assert!(Command::new("git")
            .args([
                "-C",
                repo.to_str().unwrap(),
                "worktree",
                "add",
                path.to_str().unwrap(),
                "-b",
                branch,
            ])
            .status()
            .unwrap()
            .success());
        path
    }

    #[test]
    fn task_list_carries_external_worktrees() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );

        let ext_path = add_external_worktree(&repo, dir.path(), "user-wt", "hotfix/thing");
        std::fs::write(ext_path.join("dirty.txt"), "dirty\n").unwrap();

        let list = state.handle(req("task.list", json!({})));
        let externals = list["result"]["external_worktrees"].as_array().unwrap();
        assert_eq!(externals.len(), 1, "{externals:?}");
        let entry = &externals[0];
        assert_eq!(entry["branch"], "hotfix/thing");
        assert_eq!(entry["dirty_files"], 1);
        assert_eq!(entry["adoptable"], true);
        assert!(entry["diffstat"]["files_changed"].as_u64().unwrap() >= 1);
        assert!(!entry["head_subject"].as_str().unwrap().is_empty());

        // A native task's worktree never appears as external.
        let dispatched = state.handle(req(
            "task.dispatch",
            json!({ "goal": "native work", "kind": "quick" }),
        ));
        assert_eq!(dispatched["result"]["state"], "review");
        let list2 = state.handle(req("task.list", json!({})));
        let externals2 = list2["result"]["external_worktrees"].as_array().unwrap();
        assert_eq!(
            externals2.len(),
            1,
            "still just the user worktree: {externals2:?}"
        );
    }

    #[test]
    fn external_scan_is_cached() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let project_id = state.projects[0].id.clone();

        let list = state.handle(req("task.list", json!({})));
        assert_eq!(
            list["result"]["external_worktrees"]
                .as_array()
                .unwrap()
                .len(),
            0
        );

        // A worktree that appears after the first scan is not picked up on the
        // very next poll — the cache is younger than EXTERNAL_SCAN_INTERVAL.
        add_external_worktree(&repo, dir.path(), "user-wt", "hotfix/thing");
        let list2 = state.handle(req("task.list", json!({})));
        assert_eq!(
            list2["result"]["external_worktrees"]
                .as_array()
                .unwrap()
                .len(),
            0,
            "cache not yet stale"
        );

        // Invalidating the cache exposes it on the next poll.
        state.invalidate_external_scan(&project_id);
        let list3 = state.handle(req("task.list", json!({})));
        assert_eq!(
            list3["result"]["external_worktrees"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn worktree_diff_browses_without_adopting() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let project_id = state.projects[0].id.clone();

        let ext_path = add_external_worktree(&repo, dir.path(), "user-wt", "hotfix/thing");
        std::fs::write(ext_path.join("dirty.txt"), "dirty\n").unwrap();

        let list = state.handle(req("task.list", json!({})));
        let worktree_id = list["result"]["external_worktrees"][0]["worktree_id"]
            .as_str()
            .unwrap()
            .to_string();

        let diff = state.handle(req(
            "worktree.diff",
            json!({ "project_id": project_id, "worktree_id": worktree_id }),
        ));
        assert_eq!(diff["ok"], true, "{diff:?}");
        assert!(diff["result"]["patch"]
            .as_str()
            .unwrap()
            .contains("dirty.txt"));
        assert_eq!(diff["result"]["dirty_files"], 1);

        // Browsing never mints a task.
        let after = state.handle(req("task.list", json!({})));
        assert_eq!(after["result"]["tasks"].as_array().unwrap().len(), 0);

        // Unknown id is a clean, exactly-worded error.
        let unknown = state.handle(req(
            "worktree.diff",
            json!({ "project_id": project_id, "worktree_id": "wt-deadbeef0000" }),
        ));
        assert_eq!(unknown["ok"], false);
        assert_eq!(unknown["error"], "unknown worktree_id: wt-deadbeef0000");

        // A raw path is rejected the same way — worktree_id is server-resolved.
        let raw_path = state.handle(req(
            "worktree.diff",
            json!({ "project_id": project_id, "worktree_id": ext_path.to_string_lossy() }),
        ));
        assert_eq!(raw_path["ok"], false);
        assert!(raw_path["error"]
            .as_str()
            .unwrap()
            .starts_with("unknown worktree_id:"));
    }

    #[test]
    fn adopt_mints_a_review_task_and_removes_the_card() {
        let (dir, repo) = init_repo();
        let tasks_dir = dir.path().join("tasks");
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_task_store(&tasks_dir)
        .unwrap();
        let project_id = state.projects[0].id.clone();

        add_external_worktree(&repo, dir.path(), "user-wt", "hotfix/thing");
        let list = state.handle(req("task.list", json!({})));
        let worktree_id = list["result"]["external_worktrees"][0]["worktree_id"]
            .as_str()
            .unwrap()
            .to_string();

        let adopted = state.handle(req(
            "task.adopt",
            json!({ "project_id": project_id, "worktree_id": worktree_id }),
        ));
        assert_eq!(adopted["ok"], true, "{adopted:?}");
        assert_eq!(adopted["result"]["state"], "review");
        assert_eq!(adopted["result"]["adopted"], true);
        assert_eq!(adopted["result"]["goal"], "hotfix/thing");
        let task_id = adopted["result"]["task_id"].as_str().unwrap().to_string();

        let list2 = state.handle(req("task.list", json!({})));
        assert_eq!(
            list2["result"]["external_worktrees"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert_eq!(list2["result"]["tasks"].as_array().unwrap().len(), 1);

        let persisted = crate::store::TaskStore::new(&tasks_dir)
            .load_all()
            .unwrap()
            .into_iter()
            .find(|r| r.id == task_id)
            .expect("record persisted");
        assert!(persisted.adopted);

        // Double-adopt of the same id — now bound to a task — is unknown.
        let dup = state.handle(req(
            "task.adopt",
            json!({ "project_id": project_id, "worktree_id": worktree_id }),
        ));
        assert_eq!(dup["ok"], false);
        assert!(dup["error"]
            .as_str()
            .unwrap()
            .starts_with("unknown worktree_id:"));
    }

    #[test]
    fn deleting_a_kept_adopted_task_resurfaces_the_worktree_immediately() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let project_id = state.projects[0].id.clone();

        add_external_worktree(&repo, dir.path(), "user-wt", "hotfix/thing");
        let list = state.handle(req("task.list", json!({})));
        let worktree_id = list["result"]["external_worktrees"][0]["worktree_id"]
            .as_str()
            .unwrap()
            .to_string();
        let adopted = state.handle(req(
            "task.adopt",
            json!({ "project_id": project_id, "worktree_id": worktree_id }),
        ));
        let task_id = adopted["result"]["task_id"].as_str().unwrap().to_string();

        // Merge & keep: the worktree survives, the task lands Merged (terminal).
        let merged = state.handle(req(
            "task.approve_merge",
            json!({ "task_id": task_id, "cleanup": "keep" }),
        ));
        assert_eq!(merged["ok"], true, "{merged:?}");

        // Deleting the (now unbound-once-deleted) card must resurface the kept
        // worktree as external on the VERY NEXT poll, not after the scan cadence.
        let deleted = state.handle(req("task.delete", json!({ "task_id": task_id })));
        assert_eq!(deleted["ok"], true, "{deleted:?}");
        let list2 = state.handle(req("task.list", json!({})));
        let externals = list2["result"]["external_worktrees"].as_array().unwrap();
        assert_eq!(
            externals.len(),
            1,
            "kept worktree is external again: {externals:?}"
        );
        assert_eq!(externals[0]["branch"], "hotfix/thing");
    }

    #[test]
    fn adopt_then_request_changes_simulates_like_any_build() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let project_id = state.projects[0].id.clone();

        let ext_path = add_external_worktree(&repo, dir.path(), "user-wt", "hotfix/thing");
        let list = state.handle(req("task.list", json!({})));
        let worktree_id = list["result"]["external_worktrees"][0]["worktree_id"]
            .as_str()
            .unwrap()
            .to_string();
        let adopted = state.handle(req(
            "task.adopt",
            json!({ "project_id": project_id, "worktree_id": worktree_id }),
        ));
        let task_id = adopted["result"]["task_id"].as_str().unwrap().to_string();

        let changed = state.handle(req(
            "task.request_changes",
            json!({ "task_id": task_id, "comments": "polish it" }),
        ));
        assert_eq!(changed["ok"], true, "{changed:?}");
        assert_eq!(changed["result"]["state"], "review");
        assert!(ext_path.join("result.txt").exists());
    }

    #[test]
    fn release_drops_the_record_and_keeps_the_files() {
        let (dir, repo) = init_repo();
        let tasks_dir = dir.path().join("tasks");
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_task_store(&tasks_dir)
        .unwrap();
        let project_id = state.projects[0].id.clone();

        let ext_path = add_external_worktree(&repo, dir.path(), "user-wt", "hotfix/thing");
        let list = state.handle(req("task.list", json!({})));
        let worktree_id = list["result"]["external_worktrees"][0]["worktree_id"]
            .as_str()
            .unwrap()
            .to_string();
        let adopted = state.handle(req(
            "task.adopt",
            json!({ "project_id": project_id, "worktree_id": worktree_id }),
        ));
        let task_id = adopted["result"]["task_id"].as_str().unwrap().to_string();

        let released = state.handle(req("task.release", json!({ "task_id": task_id })));
        assert_eq!(released["ok"], true, "{released:?}");
        assert_eq!(released["result"]["ok"], true);

        let list2 = state.handle(req("task.list", json!({})));
        assert_eq!(list2["result"]["tasks"].as_array().unwrap().len(), 0);
        // The card resurfaces once the cache is invalidated (release does that).
        assert_eq!(
            list2["result"]["external_worktrees"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        assert!(!tasks_dir.join(format!("{task_id}.json")).exists());
        assert!(ext_path.exists(), "worktree kept");
        let branch_kept = Command::new("git")
            .args([
                "-C",
                repo.to_str().unwrap(),
                "rev-parse",
                "--verify",
                "hotfix/thing",
            ])
            .status()
            .unwrap();
        assert!(branch_kept.success(), "branch kept");

        // Release on a native (never-adopted) task.
        let native = state.handle(req(
            "task.dispatch",
            json!({ "goal": "native", "kind": "quick" }),
        ));
        let native_id = native["result"]["task_id"].as_str().unwrap().to_string();
        let native_release = state.handle(req("task.release", json!({ "task_id": native_id })));
        assert_eq!(native_release["ok"], false);
        assert_eq!(
            native_release["error"],
            "task.release: only adopted tasks can be released"
        );

        // Release on an unknown task.
        let unknown = state.handle(req("task.release", json!({ "task_id": "task-999" })));
        assert_eq!(unknown["ok"], false);
        assert_eq!(unknown["error"], "unknown task_id");

        // Release on an adopted task that has since gone terminal.
        let ext_path2 = add_external_worktree(&repo, dir.path(), "user-wt-2", "hotfix/second");
        let ext_path2_canonical = std::fs::canonicalize(&ext_path2).unwrap();
        // Force past the scan cache so the just-added worktree is visible now.
        state.invalidate_external_scan(&project_id);
        let list3 = state.handle(req("task.list", json!({})));
        let worktree_id2 = list3["result"]["external_worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["path"] == ext_path2_canonical.display().to_string())
            .expect("second worktree listed")["worktree_id"]
            .as_str()
            .unwrap()
            .to_string();
        let adopted2 = state.handle(req(
            "task.adopt",
            json!({ "project_id": project_id, "worktree_id": worktree_id2 }),
        ));
        let task_id2 = adopted2["result"]["task_id"].as_str().unwrap().to_string();
        let merged2 = state.handle(req(
            "task.git_action",
            json!({ "task_id": task_id2, "action": "merge", "cleanup": "keep" }),
        ));
        assert_eq!(merged2["result"]["state"], "merged");
        let terminal_release = state.handle(req("task.release", json!({ "task_id": task_id2 })));
        assert_eq!(terminal_release["ok"], false);
        assert_eq!(
            terminal_release["error"],
            "task.release: task is merged — use task.delete to clear it off the board"
        );
    }

    #[test]
    fn merge_cleanup_rejects_non_string_values_instead_of_pruning() {
        // Absent / null → Prune (backward-compat default).
        assert!(matches!(
            merge_cleanup_from(&json!({}), true),
            Ok(MergeCleanup::Prune)
        ));
        assert!(matches!(
            merge_cleanup_from(&json!({ "cleanup": null }), true),
            Ok(MergeCleanup::Prune)
        ));
        // A present-but-non-string value must fail fast — never silently collapse
        // to the destructive Prune default (spec: a mis-typed client must error).
        for bad in [
            json!(true),
            json!(3),
            json!({ "mode": "keep" }),
            json!(["keep"]),
        ] {
            let params = json!({ "cleanup": bad });
            let err = merge_cleanup_from(&params, true).unwrap_err();
            assert!(
                err.starts_with("invalid cleanup:"),
                "non-string cleanup must be rejected, got: {err}"
            );
        }
    }

    #[test]
    fn approve_merge_cleanup_keep_keeps_the_worktree() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );

        // Default (absent cleanup) still prunes — the backward-compat pin.
        let native = state.handle(req(
            "task.dispatch",
            json!({ "goal": "prune me", "kind": "quick" }),
        ));
        let native_id = native["result"]["task_id"].as_str().unwrap().to_string();
        let worktree_path = state.tasks[&native_id].worktree.path.clone();
        let branch = state.tasks[&native_id].worktree.branch.clone();
        let merged = state.handle(req("task.approve_merge", json!({ "task_id": native_id })));
        assert_eq!(merged["result"]["state"], "merged");
        assert!(!worktree_path.exists(), "pruned by default");
        let branch_gone = Command::new("git")
            .args([
                "-C",
                repo.to_str().unwrap(),
                "rev-parse",
                "--verify",
                &branch,
            ])
            .status()
            .unwrap();
        assert!(!branch_gone.success(), "branch pruned by default");

        // cleanup: "keep" preserves both worktree and branch.
        let quick = state.handle(req(
            "task.dispatch",
            json!({ "goal": "keep me", "kind": "quick" }),
        ));
        let quick_id = quick["result"]["task_id"].as_str().unwrap().to_string();
        let worktree_path2 = state.tasks[&quick_id].worktree.path.clone();
        let branch2 = state.tasks[&quick_id].worktree.branch.clone();
        let merged2 = state.handle(req(
            "task.approve_merge",
            json!({ "task_id": quick_id, "cleanup": "keep" }),
        ));
        assert_eq!(merged2["ok"], true, "{merged2:?}");
        assert_eq!(merged2["result"]["state"], "merged");
        assert!(worktree_path2.exists(), "kept");
        let branch_kept = Command::new("git")
            .args([
                "-C",
                repo.to_str().unwrap(),
                "rev-parse",
                "--verify",
                &branch2,
            ])
            .status()
            .unwrap();
        assert!(branch_kept.success(), "branch kept");
    }

    #[test]
    fn approve_merge_cleanup_release_unadopts_after_merge() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let project_id = state.projects[0].id.clone();

        let ext_path = add_external_worktree(&repo, dir.path(), "user-wt", "hotfix/release-me");
        let list = state.handle(req("task.list", json!({})));
        let worktree_id = list["result"]["external_worktrees"][0]["worktree_id"]
            .as_str()
            .unwrap()
            .to_string();
        let adopted = state.handle(req(
            "task.adopt",
            json!({ "project_id": project_id, "worktree_id": worktree_id }),
        ));
        let task_id = adopted["result"]["task_id"].as_str().unwrap().to_string();

        // "release" on a native task is refused.
        let native = state.handle(req(
            "task.dispatch",
            json!({ "goal": "native", "kind": "quick" }),
        ));
        let native_id = native["result"]["task_id"].as_str().unwrap().to_string();
        let native_release_attempt = state.handle(req(
            "task.approve_merge",
            json!({ "task_id": native_id, "cleanup": "release" }),
        ));
        assert_eq!(native_release_attempt["ok"], false);
        assert_eq!(
            native_release_attempt["error"],
            "cleanup: \"release\" is only valid for adopted tasks"
        );

        let merged = state.handle(req(
            "task.approve_merge",
            json!({ "task_id": task_id, "cleanup": "release" }),
        ));
        assert_eq!(merged["ok"], true, "{merged:?}");
        assert_eq!(merged["result"]["state"], "merged");

        // The record is gone from the board.
        let list2 = state.handle(req("task.list", json!({})));
        assert!(list2["result"]["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["task_id"] != task_id));
        // The worktree + branch survive and the card resurfaces as external.
        assert!(ext_path.exists());
        let ext_canonical = std::fs::canonicalize(&ext_path).unwrap();
        let externals = list2["result"]["external_worktrees"].as_array().unwrap();
        assert!(externals
            .iter()
            .any(|w| w["path"] == ext_canonical.display().to_string()));
    }

    #[test]
    fn git_action_cleanup_rules() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );

        let quick = state.handle(req(
            "task.dispatch",
            json!({ "goal": "git actions", "kind": "quick" }),
        ));
        let task_id = quick["result"]["task_id"].as_str().unwrap().to_string();

        // commit with a cleanup param errors.
        let bad = state.handle(req(
            "task.git_action",
            json!({ "task_id": task_id, "action": "commit", "cleanup": "keep" }),
        ));
        assert_eq!(bad["ok"], false);
        assert_eq!(bad["error"], "cleanup only applies to merge actions");

        // Invalid cleanup value.
        let invalid = state.handle(req(
            "task.git_action",
            json!({ "task_id": task_id, "action": "merge", "cleanup": "bogus" }),
        ));
        assert_eq!(invalid["ok"], false);
        assert_eq!(
            invalid["error"],
            "invalid cleanup: \"bogus\" (expected prune|keep|release)"
        );

        // "release" on a native task.
        let native_release = state.handle(req(
            "task.git_action",
            json!({ "task_id": task_id, "action": "merge", "cleanup": "release" }),
        ));
        assert_eq!(native_release["ok"], false);
        assert_eq!(
            native_release["error"],
            "cleanup: \"release\" is only valid for adopted tasks"
        );

        // merge + keep works.
        let worktree_path = state.tasks[&task_id].worktree.path.clone();
        let ok = state.handle(req(
            "task.git_action",
            json!({ "task_id": task_id, "action": "merge", "cleanup": "keep" }),
        ));
        assert_eq!(ok["ok"], true, "{ok:?}");
        assert_eq!(ok["result"]["state"], "merged");
        assert!(worktree_path.exists());
    }

    #[test]
    fn task_delete_never_prunes_an_adopted_worktree() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let project_id = state.projects[0].id.clone();

        let ext_path = add_external_worktree(&repo, dir.path(), "user-wt", "hotfix/keep-on-delete");
        let list = state.handle(req("task.list", json!({})));
        let worktree_id = list["result"]["external_worktrees"][0]["worktree_id"]
            .as_str()
            .unwrap()
            .to_string();
        let adopted = state.handle(req(
            "task.adopt",
            json!({ "project_id": project_id, "worktree_id": worktree_id }),
        ));
        let task_id = adopted["result"]["task_id"].as_str().unwrap().to_string();

        let merged = state.handle(req(
            "task.git_action",
            json!({ "task_id": task_id, "action": "merge", "cleanup": "keep" }),
        ));
        assert_eq!(merged["result"]["state"], "merged");

        let deleted = state.handle(req("task.delete", json!({ "task_id": task_id })));
        assert_eq!(deleted["ok"], true, "{deleted:?}");
        assert!(ext_path.exists(), "adopted worktree survives delete");

        // The native counterpart still prunes on delete.
        let native = state.handle(req(
            "task.dispatch",
            json!({ "goal": "native delete", "kind": "quick" }),
        ));
        let native_id = native["result"]["task_id"].as_str().unwrap().to_string();
        let native_worktree = state.tasks[&native_id].worktree.path.clone();
        state.handle(req("task.approve_merge", json!({ "task_id": native_id })));
        state.handle(req("task.delete", json!({ "task_id": native_id })));
        assert!(
            !native_worktree.exists(),
            "native worktree pruned on delete"
        );
    }
}
