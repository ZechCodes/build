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
use std::io::Read as _;
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

/// A worktree-backed surface a terminal or fs call is scoped to. Scope roots are
/// resolved server-side ONLY (spec §1): ids map to roots through the bridge's own
/// records — a client-supplied filesystem path is never a scope root.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TermScope {
    Task {
        task_id: String,
    },
    ExternalWorktree {
        project_id: String,
        worktree_id: String,
    },
    Primary {
        project_id: String,
    },
}

impl TermScope {
    /// Parse the inline scope params: `task_id` wins, then
    /// `project_id`+`worktree_id`, then `project_id` alone.
    fn parse(params: &Value) -> Result<TermScope, String> {
        let field = |key: &str| {
            params
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        if let Some(task_id) = field("task_id") {
            return Ok(TermScope::Task { task_id });
        }
        let Some(project_id) = field("project_id") else {
            return Err("missing scope: task_id or project_id required".to_string());
        };
        match field("worktree_id") {
            Some(worktree_id) => Ok(TermScope::ExternalWorktree {
                project_id,
                worktree_id,
            }),
            None => Ok(TermScope::Primary { project_id }),
        }
    }

    /// Resolve to the scope's canonical root directory, server-side only.
    /// `&mut AppState` because the external-worktree arm may refresh the scan
    /// cache; it never accepts a raw path and never canonicalizes client input.
    fn resolve_root(&self, state: &mut AppState) -> Result<std::path::PathBuf, String> {
        match self {
            TermScope::Task { task_id } => {
                let active = state.tasks.get(task_id).ok_or("unknown task_id")?;
                let root = active.worktree.path.clone();
                if !root.exists() {
                    return Err("worktree no longer exists".to_string());
                }
                Ok(root)
            }
            TermScope::ExternalWorktree {
                project_id,
                worktree_id,
            } => Ok(state
                .resolve_external_worktree(project_id, worktree_id)?
                .path),
            TermScope::Primary { project_id } => state
                .projects
                .iter()
                .find(|p| &p.id == project_id)
                .map(|p| p.repo_path.clone())
                .ok_or_else(|| "unknown project_id".to_string()),
        }
    }
}

/// Authoritative server-side screen: vt100 model + attach list + coalescing
/// buffer + the monotonic byte cursor. Snapshot resync, not byte replay. Shared
/// by user terminals and the retained agent screens.
struct TermScreen {
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
/// At most this many user terminals daemon-wide, all scopes combined. Agent
/// screens don't count (at most one per task, bounded by tasks).
const MAX_USER_TERMINALS: usize = 16;
/// `fs.read` never returns more than this many content bytes in one response,
/// regardless of the file's real size (spec §4).
const FS_READ_MAX_BYTES: u64 = 1_048_576;

impl TermScreen {
    fn new(cols: u16, rows: u16) -> TermScreen {
        TermScreen {
            parser: vt100::Parser::new(rows, cols, 2000),
            attached: Vec::new(),
            pending: Vec::new(),
            total: 0,
            cols,
            rows,
        }
    }

    /// The current screen serialized as escape sequences — write it to a fresh
    /// terminal and the screen is reproduced.
    fn snapshot(&self) -> String {
        b64encode(&self.parser.screen().contents_formatted())
    }

    fn set_size(&mut self, cols: u16, rows: u16) {
        self.parser.set_size(rows, cols);
        self.cols = cols;
        self.rows = rows;
    }

    /// Feed PTY bytes: advance the screen model, the cursor, and the pending
    /// coalescing buffer.
    fn process(&mut self, chunk: &[u8]) {
        self.parser.process(chunk);
        self.total += chunk.len() as u64;
        self.pending.extend_from_slice(chunk);
    }

    /// Register a client for live output, dropping any prior sender with the
    /// same session id first (a reconnect on the same id).
    fn register(&mut self, sender: &SessionSender) {
        self.attached
            .retain(|snd| snd.session_id() != sender.session_id());
        self.attached.push(sender.clone());
    }

    /// Flush pending bytes as one keyed push to every attached client — raw
    /// output, or a screen snapshot when the backlog crosses the collapse
    /// threshold. Senders whose connection is gone are dropped.
    fn flush(&mut self, term_id: &str) {
        if self.pending.is_empty() {
            return;
        }
        let cursor = self.total;
        let payload = if self.pending.len() > TERM_SNAPSHOT_THRESHOLD {
            // Too much at once — skip the backlog, send the screen.
            json!({ "type": "term.reset", "term_id": term_id, "data": self.snapshot(), "cursor": cursor })
        } else {
            json!({ "type": "term.output", "term_id": term_id, "data": b64encode(&self.pending), "cursor": cursor })
        };
        self.pending.clear();
        self.attached.retain(|snd| snd.push(payload.clone()));
    }

    /// Tell every attached client this terminal ended, and why.
    fn push_closed(&self, term_id: &str, reason: &str) {
        let payload = json!({ "type": "term.closed", "term_id": term_id, "reason": reason });
        for snd in &self.attached {
            snd.push(payload.clone());
        }
    }
}

/// A live keyed terminal: a real PTY spawned in its scope's root, plus the
/// authoritative screen model that makes reconnect a *snapshot* (current
/// screen) rather than a byte replay.
struct TermSession {
    term_id: String,
    scope: TermScope,
    /// The scope's root directory, resolved server-side at create time.
    scope_root: std::path::PathBuf,
    created_at: String,
    session: PtySession,
    screen: TermScreen,
}

/// The shell user terminals run: `BRIDGE_TERM_SHELL` override → the daemon
/// env's `SHELL` → the account's passwd shell → bash. Terminals are windows
/// onto the user's machine — they get the user's own shell and rc files, not
/// a sanitized bash.
pub fn resolve_term_shell() -> String {
    for var in ["BRIDGE_TERM_SHELL", "SHELL"] {
        if let Ok(shell) = std::env::var(var) {
            if !shell.trim().is_empty() {
                return shell;
            }
        }
    }
    passwd_shell().unwrap_or_else(|| "/bin/bash".to_string())
}

/// The account's login shell from the passwd database.
fn passwd_shell() -> Option<String> {
    // SAFETY: getpwuid returns a pointer to static storage owned by libc; we
    // only read pw_shell out of it, on this thread, immediately.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() {
            return None;
        }
        let shell = (*pw).pw_shell;
        if shell.is_null() {
            return None;
        }
        let shell = std::ffi::CStr::from_ptr(shell)
            .to_string_lossy()
            .into_owned();
        (!shell.trim().is_empty()).then_some(shell)
    }
}

/// Ask a login shell what PATH looks like — the terminal-emulator trick.
/// launchd starts agents with a bare PATH, so user-installed tools (the
/// `claude` harness included) don't resolve until we adopt the login PATH.
/// Bounded by `timeout`; a hung rc file just means we keep the inherited PATH.
pub fn capture_login_path(shell: &str, timeout: std::time::Duration) -> Option<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let shell = shell.to_string();
    std::thread::spawn(move || {
        let out = std::process::Command::new(&shell)
            .args(["-ilc", "printf %s \"$PATH\""])
            .stdin(std::process::Stdio::null())
            .output();
        let _ = tx.send(out);
    });
    match rx.recv_timeout(timeout) {
        Ok(Ok(out)) if out.status.success() => {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            path.contains('/').then_some(path)
        }
        _ => None,
    }
}

impl TermSession {
    /// Spawn the user's interactive login shell in a PTY at the scope root,
    /// returning the session and a receiver for its output (subscribed
    /// immediately so no early bytes are missed).
    fn spawn(
        shell: &str,
        term_id: String,
        scope: TermScope,
        scope_root: std::path::PathBuf,
        cols: u16,
        rows: u16,
    ) -> Result<(TermSession, broadcast::Receiver<Vec<u8>>), String> {
        // -i -l: interactive login shell — rc files, the user's PATH, the
        // user's prompt. This is their machine, shown honestly.
        let spec = HarnessSpec::new(shell)
            .arg("-i")
            .arg("-l")
            .env("TERM", "xterm-256color");
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let session =
            PtySession::spawn(&spec, Some(scope_root.clone()), size).map_err(|e| e.to_string())?;
        let rx = session.subscribe();
        Ok((
            TermSession {
                term_id,
                scope,
                scope_root,
                created_at: now_rfc3339(),
                session,
                screen: TermScreen::new(cols, rows),
            },
            rx,
        ))
    }
}

/// The retained screen of a task's agent PTY stream. Created on first
/// `agent.attach`, retained until the task record is removed (reaper), so the
/// tab can show the last screen between sessions.
struct AgentScreen {
    screen: TermScreen,
    /// The [`ActiveTask`] session generation this screen's pump is consuming.
    /// 0 = no pump has ever run.
    pumped_generation: u64,
    /// Whether a live pump is currently feeding this screen.
    live: bool,
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
    /// Cached `task.list.primary_changes` entry for this project, refreshed at
    /// most every `PRIMARY_SUMMARY_TTL` (spec §5.3) — same discipline as
    /// `external_scan` / the task-stat cache.
    primary_summary: Option<(std::time::Instant, Value)>,
}

/// External-worktree scans are refreshed at most this often per project; the
/// board polls task.list every ~1.6 s and must never trigger a full rescan per
/// poll.
const EXTERNAL_SCAN_INTERVAL: Duration = Duration::from_secs(10);

/// How long a task's `task.list` diffstat is served from cache before the next
/// poll recomputes it (same reasoning as the external-worktree scan interval).
const TASK_STAT_TTL: Duration = Duration::from_secs(10);

/// How long the primary-checkout `task.list.primary_changes` summary is served
/// from cache before the next poll recomputes it (spec §5.3).
const PRIMARY_SUMMARY_TTL: Duration = Duration::from_secs(10);

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
    /// task id → its RFC 3339 last-mutation time (stamped on every mutation).
    task_updated_at: HashMap<String, String>,
    /// task id → cached `task.list` diffstat, so the poll surface never runs
    /// per-task git work more than once per TTL window.
    task_stat_cache: HashMap<String, (std::time::Instant, Value)>,
    /// The shell user terminals spawn (resolved once; see [`resolve_term_shell`]).
    term_shell: String,
    streams: HashMap<String, StreamState>,
    /// Live user terminals, keyed by `term_id` (`term-<n>`).
    terms: HashMap<String, TermSession>,
    /// Retained agent screens, keyed by task id (pushed as `agent:<task_id>`).
    agent_screens: HashMap<String, AgentScreen>,
    /// `term-<n>` mint counter — monotonic, never reused within a daemon life.
    next_term: u64,
    /// Weak self-handle set once at [`AppState::shared`] time, so `&mut self`
    /// hooks (`ensure_agent_pumps` at the `finish_mutation` tail) can spawn
    /// pump tasks that need the `Arc`. Dispatch paths that run in tests
    /// without an Arc simply skip pump spawning (they assert on state, not
    /// pushes).
    self_handle: Option<std::sync::Weak<Mutex<AppState>>>,
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
            task_updated_at: HashMap::new(),
            task_stat_cache: HashMap::new(),
            term_shell: resolve_term_shell(),
            streams: HashMap::new(),
            terms: HashMap::new(),
            agent_screens: HashMap::new(),
            next_term: 1,
            self_handle: None,
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
        self.task_updated_at
            .insert(task_id.clone(), record.updated_at);
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
        let updated_at = self.task_updated_at.get(task_id).cloned().unwrap_or(now);
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
            updated_at,
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
        // Stamp times before building the view so the response carries them,
        // and drop the cached diffstat — the mutation likely changed the tree.
        let now = now_rfc3339();
        self.task_created_at
            .entry(task_id.clone())
            .or_insert_with(|| now.clone());
        self.task_updated_at.insert(task_id.clone(), now);
        self.task_stat_cache.remove(&task_id);
        let view = self.task_view(&task_id, &active);
        let persisted = self.persist_task(&task_id, &active);
        self.push_notify_if_needed(&task_id, &active.task.state);
        self.tasks.insert(task_id, active);
        // Prompt terminal closure: an abandon/delete/merge-prune just changed
        // what resolves, so orphaned terminals close now, not at the next sweep.
        self.reap_orphaned_terminals();
        // And prompt pump start: this mutation may have spawned a session an
        // attached Agent tab is waiting on.
        self.ensure_agent_pumps();
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
            primary_summary: None,
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
    /// drive the same tasks. Stashes a weak self-handle so `&mut self` hooks
    /// can spawn pump tasks (see the `self_handle` field).
    pub fn shared(self) -> Arc<Mutex<AppState>> {
        let state = Arc::new(Mutex::new(self));
        state.lock().unwrap().self_handle = Some(Arc::downgrade(&state));
        state
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
            "fs.tree" => self.fs_tree(params),
            "fs.read" => self.fs_read(params),
            "project.diff" => self.project_diff(params),
            "git.log" => self.git_log(params),
            "git.show" => self.git_show(params),
            "git.status" => self.git_status(params),
            "git.stage" => self.git_stage(params),
            "git.unstage" => self.git_unstage(params),
            "git.commit" => self.git_commit(params),
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
            "task.message" => self.task_message(params),
            "task.approve_merge" => self.task_approve_merge(params),
            "task.git_action" => self.task_git_action(params),
            "task.abandon" => self.task_abandon(params),
            "task.delete" => self.task_delete(params),
            "stream.events" => self.stream_events(params),
            "stream.state" => self.stream_state(params),
            "term.list" => self.term_list(params),
            "term.close" => self.term_close(params),
            "term.input" => self.term_input(params),
            "term.resize" => self.term_resize(params),
            other => Err(format!("unknown method: {other}")),
        }
    }

    /// The user terminals whose scope matches the request, ordered by numeric
    /// id suffix. Agent screens never appear here. An unknown scope id still
    /// errors (the SPA treats an error as "no terminals").
    fn term_list(&mut self, params: &Value) -> Result<Value, String> {
        let scope = TermScope::parse(params)?;
        scope.resolve_root(self)?;
        let mut terminals: Vec<(u64, Value)> = self
            .terms
            .values()
            .filter(|t| t.scope == scope)
            .map(|t| {
                (
                    term_id_suffix(&t.term_id),
                    json!({
                        "term_id": t.term_id,
                        "cols": t.screen.cols,
                        "rows": t.screen.rows,
                        "created_at": t.created_at,
                    }),
                )
            })
            .collect();
        terminals.sort_by_key(|(suffix, _)| *suffix);
        let terminals: Vec<Value> = terminals.into_iter().map(|(_, entry)| entry).collect();
        Ok(json!({ "terminals": terminals }))
    }

    /// Close a user terminal: remove it, kill AND reap its shell (the existing
    /// zombie-prevention contract), and tell every attached client.
    fn term_close(&mut self, params: &Value) -> Result<Value, String> {
        let term_id = require_str(params, "term_id")?;
        if term_id.starts_with("agent:") {
            // Agent PTY lifetime belongs to the orchestrator, not the tab's ×.
            return Err("cannot close an agent terminal".to_string());
        }
        let term = self.terms.remove(&term_id).ok_or("unknown term_id")?;
        term.session.kill_and_reap();
        term.screen.push_closed(&term_id, "closed");
        Ok(json!({ "ok": true }))
    }

    /// Write client keystrokes (base64) to a terminal's PTY, by id. `agent:`
    /// ids route to the task's live session — input is allowed by design (the
    /// agent PTY is a full terminal on the user's machine; the terminal is the
    /// basement), and a dead session surfaces "no active agent session".
    fn term_input(&mut self, params: &Value) -> Result<Value, String> {
        let term_id = require_str(params, "term_id")?;
        let data = b64decode(&require_str(params, "data")?)?;
        if let Some(task_id) = term_id.strip_prefix("agent:") {
            let active = self.tasks.get(task_id).ok_or("unknown term_id")?;
            active.write_input_strict(&data)?;
            return Ok(json!({ "ok": true }));
        }
        let term = self.terms.get(&term_id).ok_or("unknown term_id")?;
        term.session.write_input(&data).map_err(|e| e.to_string())?;
        Ok(json!({ "ok": true }))
    }

    /// Resize a terminal's PTY and screen model, by id. For `agent:` ids the
    /// resize only applies while a session is live (`live: true`); a dead
    /// resize is a no-op `live: false` so the retained last screen is never
    /// garbled.
    fn term_resize(&mut self, params: &Value) -> Result<Value, String> {
        let term_id = require_str(params, "term_id")?;
        let cols = params.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16;
        let rows = params.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16;
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        if let Some(task_id) = term_id.strip_prefix("agent:") {
            let active = self.tasks.get(task_id).ok_or("unknown term_id")?;
            let live = active.resize_session(size).map_err(err)?;
            if live {
                // Keep the retained agent screen in step with the live PTY; a
                // dead resize touches nothing (the last screen stays intact).
                if let Some(agent) = self.agent_screens.get_mut(task_id) {
                    agent.screen.set_size(cols, rows);
                }
            }
            return Ok(json!({ "ok": true, "live": live }));
        }
        let term = self.terms.get_mut(&term_id).ok_or("unknown term_id")?;
        term.session.resize(size).map_err(|e| e.to_string())?;
        term.screen.set_size(cols, rows);
        Ok(json!({ "ok": true, "live": true }))
    }

    /// A session ended: detach it from every terminal so the pumps stop
    /// encrypting (and serializing) output frames into a session the relay
    /// will just drop.
    fn drop_session(&mut self, session_id: &str) {
        for term in self.terms.values_mut() {
            term.screen
                .attached
                .retain(|snd| snd.session_id() != session_id);
        }
        for agent in self.agent_screens.values_mut() {
            agent
                .screen
                .attached
                .retain(|snd| snd.session_id() != session_id);
        }
    }

    /// Close every user terminal whose scope no longer resolves (spec §2.6.3):
    /// its task record is gone, its project is unregistered, or its worktree
    /// vanished from disk. Every close kills AND reaps. Returns the closed ids.
    /// Called at the tail of `finish_mutation` (prompt closure right after
    /// abandon/delete/merge-prune) and by the periodic reaper loop (out-of-band
    /// disappearance, e.g. a user `rm -rf`ing an external worktree).
    fn reap_orphaned_terminals(&mut self) -> Vec<String> {
        let orphaned: Vec<String> = self
            .terms
            .values()
            .filter(|term| !self.term_scope_resolves(term))
            .map(|term| term.term_id.clone())
            .collect();
        for term_id in &orphaned {
            let Some(term) = self.terms.remove(term_id) else {
                continue;
            };
            term.session.kill_and_reap();
            term.screen.push_closed(term_id, "reaped");
        }
        // Retained agent screens live exactly as long as their task record.
        let AppState {
            agent_screens,
            tasks,
            ..
        } = self;
        agent_screens.retain(|task_id, _| tasks.contains_key(task_id));
        orphaned
    }

    /// Start an agent pump for every attached agent screen whose task has a
    /// live session that is not being pumped yet — a viewer staring at the
    /// Agent tab must see a session that starts *after* they attached (approve
    /// plan → build session spawns). Runs at the `finish_mutation` tail via
    /// the weak self-handle; without a handle or a runtime (sync unit tests)
    /// pump spawning is skipped.
    fn ensure_agent_pumps(&mut self) {
        let Some(state_arc) = self.self_handle.as_ref().and_then(std::sync::Weak::upgrade) else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let mut pumps = Vec::new();
        for (task_id, agent) in &mut self.agent_screens {
            if agent.screen.attached.is_empty() {
                continue;
            }
            let Some(active) = self.tasks.get(task_id) else {
                continue;
            };
            if let Some((generation, rx)) = active.subscribe_with_generation() {
                if generation != agent.pumped_generation {
                    agent.pumped_generation = generation;
                    agent.live = true;
                    pumps.push((task_id.clone(), generation, rx));
                }
            }
        }
        for (task_id, generation, rx) in pumps {
            spawn_agent_pump(Arc::clone(&state_arc), task_id, generation, rx);
        }
    }

    /// Whether a terminal's scope still maps to a live surface. The check is
    /// cheap: a map lookup and/or one `Path::exists` over ≤ 16 entries.
    fn term_scope_resolves(&self, term: &TermSession) -> bool {
        match &term.scope {
            // A merged task with cleanup=keep keeps its worktree → terminals stay.
            TermScope::Task { task_id } => {
                self.tasks.contains_key(task_id) && term.scope_root.exists()
            }
            // An adopted worktree's path survives adoption — its terminal lives on.
            TermScope::ExternalWorktree { .. } => term.scope_root.exists(),
            TermScope::Primary { project_id } => {
                self.projects.iter().any(|p| &p.id == project_id) && term.scope_root.exists()
            }
        }
    }

    /// Periodically close terminals whose scope vanished out-of-band (nothing
    /// went through `finish_mutation` — e.g. the user deleted an external
    /// worktree by hand). Runs beside `spawn_idle_monitor`.
    pub fn spawn_terminal_reaper(state: Arc<Mutex<AppState>>, interval: Duration) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let reaped = state.lock().unwrap().reap_orphaned_terminals();
                for term_id in reaped {
                    eprintln!("terminal reaper: closed {term_id} (scope gone)");
                }
            }
        });
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

    /// One directory level of a worktree-backed scope (spec §4.2): server-side
    /// scope resolution, the shared fence, `.git` skipped, dirs before
    /// files+symlinks, each group case-insensitive.
    fn fs_tree(&mut self, params: &Value) -> Result<Value, String> {
        let scope = TermScope::parse(params)?;
        let root = scope.resolve_root(self)?;
        let path = params
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let target = fenced_scope_path(&root, &path)?;
        if !target.is_dir() {
            return Err("not a directory".to_string());
        }
        let reader = std::fs::read_dir(&target).map_err(|e| format!("cannot read {path}: {e}"))?;
        let mut dirs: Vec<(String, Value)> = Vec::new();
        let mut rest: Vec<(String, Value)> = Vec::new();
        for entry in reader.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                dirs.push((name.clone(), json!({ "name": name, "kind": "dir" })));
            } else if file_type.is_symlink() {
                rest.push((name.clone(), json!({ "name": name, "kind": "symlink" })));
            } else {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                rest.push((
                    name.clone(),
                    json!({ "name": name, "kind": "file", "size": size }),
                ));
            }
        }
        dirs.sort_by_key(|(name, _)| name.to_lowercase());
        rest.sort_by_key(|(name, _)| name.to_lowercase());
        let entries: Vec<Value> = dirs.into_iter().chain(rest).map(|(_, v)| v).collect();
        Ok(json!({ "path": path, "entries": entries }))
    }

    /// Read one file from a worktree-backed scope, base64 always, capped at
    /// [`FS_READ_MAX_BYTES`] server-side (spec §4.3).
    fn fs_read(&mut self, params: &Value) -> Result<Value, String> {
        let scope = TermScope::parse(params)?;
        let root = scope.resolve_root(self)?;
        let path = require_str(params, "path")?;
        let target = fenced_scope_path(&root, &path)?;
        let leaf =
            std::fs::symlink_metadata(&target).map_err(|e| format!("cannot read {path}: {e}"))?;
        if leaf.file_type().is_symlink() {
            return Err("refusing to read a symlink".to_string());
        }
        if leaf.is_dir() {
            return Err("not a file".to_string());
        }
        let size = leaf.len();
        let file = std::fs::File::open(&target).map_err(|e| format!("cannot read {path}: {e}"))?;
        let mut content = Vec::with_capacity(size.min(FS_READ_MAX_BYTES) as usize);
        file.take(FS_READ_MAX_BYTES)
            .read_to_end(&mut content)
            .map_err(|e| format!("cannot read {path}: {e}"))?;
        let truncated = size > FS_READ_MAX_BYTES;
        let head_len = content.len().min(8192);
        let mime = mime_hint(&target, &content[..head_len]);
        Ok(json!({
            "path": path,
            "size": size,
            "truncated": truncated,
            "mime": mime,
            "content_b64": b64encode(&content),
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
    /// A freeform message to the task's agent — redirects a live session or
    /// resumes a parked one, riding the harness's own conversation when a
    /// transcript exists (see `Orchestrator::message`).
    fn task_message(&mut self, params: &Value) -> Result<Value, String> {
        let task_id = require_str(params, "task_id")?;
        let message = require_str(params, "message")?;
        let project_id = self.project_of(&task_id)?;
        let mut active = self.take(&task_id)?;
        let outcome = (|| -> Result<(), String> {
            self.orch_for(&project_id)?
                .message(&mut active, &message)
                .map_err(err)?;
            if self.qa_agent {
                match active.task.state {
                    TaskState::Planning if active.revising_stage_id.is_some() => {
                        self.simulate_stage_revise(&project_id, &mut active)?
                    }
                    TaskState::Planning => self.simulate_plan(&project_id, &mut active)?,
                    TaskState::Building if active.is_multi_stage() => {
                        self.qa_drive_stage_chain(&project_id, &mut active)?
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
                self.task_updated_at.remove(task_id);
                self.task_stat_cache.remove(task_id);
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
        self.task_updated_at.remove(&task_id);
        self.task_stat_cache.remove(&task_id);

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
        // The record (and possibly the worktree) is gone: close its terminals
        // and drop its retained agent screen now, not at the next sweep.
        self.reap_orphaned_terminals();
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
        let ids: Vec<String> = self.tasks.keys().cloned().collect();
        let tasks: Vec<Value> = ids
            .into_iter()
            .map(|id| {
                let stat = self.task_stat(&id);
                let active = self.tasks.get(&id).expect("listed above");
                let mut view = self.task_view(&id, active);
                view.as_object_mut()
                    .expect("task_view returns an object")
                    .insert("stat".to_string(), stat);
                view
            })
            .collect();
        let external_worktrees = self.external_worktrees_json();
        let primary_changes = self.primary_changes_json();
        json!({ "tasks": tasks, "external_worktrees": external_worktrees, "primary_changes": primary_changes })
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

    /// Every project's primary-checkout changes summary, cached per project
    /// for [`PRIMARY_SUMMARY_TTL`] (spec §5.3) — the `task.list` ride-along
    /// for the sidebar "main" row and the project page's MAIN bucket. A
    /// per-project failure (unborn HEAD, fs error) logs and contributes
    /// nothing, same posture as `external_worktrees_json`.
    fn primary_changes_json(&mut self) -> Vec<Value> {
        let mut entries = Vec::new();
        for i in 0..self.projects.len() {
            if let Some((computed_at, cached)) = &self.projects[i].primary_summary {
                if computed_at.elapsed() < PRIMARY_SUMMARY_TTL {
                    entries.push(cached.clone());
                    continue;
                }
            }
            let project = &self.projects[i];
            let project_id = project.id.clone();
            let repo = git2::Repository::open(&project.repo_path);
            let branch = repo
                .as_ref()
                .ok()
                .and_then(|r| r.head().ok())
                .and_then(|h| h.shorthand().map(str::to_string))
                .unwrap_or_else(|| "HEAD".to_string());
            let summary = match crate::diff::diff_against_head(&project.repo_path) {
                Ok(diff) => {
                    let stat = diff.stat();
                    Some(json!({
                        "project_id": project_id,
                        "branch": branch,
                        "files_changed": stat.files_changed,
                        "insertions": stat.insertions,
                        "deletions": stat.deletions,
                    }))
                }
                Err(e) => {
                    eprintln!("primary_changes {project_id}: {e}");
                    None
                }
            };
            self.projects[i].primary_summary =
                summary.clone().map(|s| (std::time::Instant::now(), s));
            if let Some(summary) = summary {
                entries.push(summary);
            }
        }
        entries
    }

    /// The primary checkout's uncommitted-changes review surface (spec §5.2):
    /// same shape as `worktree.diff` so `parseDiff`/`diffFilesHtml` reuse is
    /// mechanical.
    fn project_diff(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let project = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .ok_or_else(|| "unknown project_id".to_string())?;
        let repo_path = project.repo_path.clone();
        let repo = git2::Repository::open(&repo_path).ok();
        let branch = repo
            .as_ref()
            .and_then(|r| r.head().ok())
            .and_then(|h| h.shorthand().map(str::to_string))
            .unwrap_or_else(|| "HEAD".to_string());
        let diff = crate::diff::diff_against_head(&repo_path).map_err(|e| e.to_string())?;
        let files: Vec<Value> = diff
            .files()
            .iter()
            .map(|f| json!({ "path": f.path, "status": format!("{:?}", f.status) }))
            .collect();
        let stat = diff.stat();
        Ok(json!({
            "project_id": project_id,
            "branch": branch,
            "path": repo_path.display().to_string(),
            "stat": {
                "files_changed": stat.files_changed,
                "insertions": stat.insertions,
                "deletions": stat.deletions,
            },
            "files": files,
            "patch": diff.patch(),
        }))
    }

    /// Resolve the shared `git.*` scope: exactly one of `project_id` (the
    /// project's primary checkout) or `task_id` (the task's worktree). The
    /// repo path always comes from server state — a client can never name a
    /// filesystem path directly.
    fn resolve_git_scope(&self, params: &Value) -> Result<GitScope, String> {
        let project_id = params.get("project_id").and_then(Value::as_str);
        let task_id = params.get("task_id").and_then(Value::as_str);
        match (project_id, task_id) {
            (Some(project_id), None) => {
                let project = self
                    .projects
                    .iter()
                    .find(|p| p.id == project_id)
                    .ok_or_else(|| "unknown project_id".to_string())?;
                Ok(GitScope {
                    repo_path: project.repo_path.clone(),
                    task: None,
                })
            }
            (None, Some(task_id)) => {
                let active = self
                    .tasks
                    .get(task_id)
                    .ok_or_else(|| "unknown task_id".to_string())?;
                Ok(GitScope {
                    repo_path: active.worktree.path.clone(),
                    task: Some(GitScopeTask {
                        task_id: task_id.to_string(),
                        base_branch: active.worktree.base_branch.clone(),
                    }),
                })
            }
            _ => Err("provide exactly one of project_id or task_id".to_string()),
        }
    }

    /// `git.log` — one page of commit history for the scoped checkout. Task
    /// scope additionally marks each commit as ahead of (unreachable from)
    /// the base branch.
    fn git_log(&mut self, params: &Value) -> Result<Value, String> {
        let scope = self.resolve_git_scope(params)?;
        let limit = params
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(30)
            .clamp(1, 200) as usize;
        let skip = params.get("skip").and_then(Value::as_u64).unwrap_or(0) as usize;
        let mark_ahead_of = scope.task.as_ref().map(|task| task.base_branch.clone());
        crate::gitgui::log_page(&scope.repo_path, mark_ahead_of.as_deref(), limit, skip)
    }

    /// `git.show` — one commit's metadata, stat, and capped patch. The hash
    /// param is a strict object-id prefix, never a general revspec.
    fn git_show(&mut self, params: &Value) -> Result<Value, String> {
        let scope = self.resolve_git_scope(params)?;
        let hash = require_str(params, "hash")?;
        crate::gitgui::show_commit(&scope.repo_path, &hash)
    }

    /// `git.status` — branch/head plus per-file staging tri-state and the
    /// uncommitted patch for the scoped checkout.
    fn git_status(&mut self, params: &Value) -> Result<Value, String> {
        let scope = self.resolve_git_scope(params)?;
        crate::gitgui::status_payload(&scope.repo_path)
    }

    /// `git.stage` — stage the given repo-relative paths, answering with the
    /// fresh status payload so the UI repaints without waiting for a poll.
    fn git_stage(&mut self, params: &Value) -> Result<Value, String> {
        let scope = self.resolve_git_scope(params)?;
        let paths = require_path_list(params)?;
        crate::gitgui::stage_paths(&scope.repo_path, &paths)?;
        crate::gitgui::status_payload(&scope.repo_path)
    }

    /// `git.unstage` — the inverse of `git.stage`, same response shape.
    fn git_unstage(&mut self, params: &Value) -> Result<Value, String> {
        let scope = self.resolve_git_scope(params)?;
        let paths = require_path_list(params)?;
        crate::gitgui::unstage_paths(&scope.repo_path, &paths)?;
        crate::gitgui::status_payload(&scope.repo_path)
    }

    /// `git.commit` — commit exactly what is staged with the user's message.
    /// On a task scope the commit changes the tree the board summarizes, so
    /// the cached diffstat is dropped and the task's updated-at stamped; the
    /// task record itself is untouched (no lifecycle transition — a commit
    /// never advances a task past any gate).
    fn git_commit(&mut self, params: &Value) -> Result<Value, String> {
        let scope = self.resolve_git_scope(params)?;
        let message = require_str(params, "message")?;
        let commit = crate::gitgui::commit_staged(&scope.repo_path, &message)?;
        if let Some(task) = &scope.task {
            self.task_stat_cache.remove(&task.task_id);
            self.task_updated_at
                .insert(task.task_id.clone(), now_rfc3339());
        }
        let status = crate::gitgui::status_payload(&scope.repo_path)?;
        Ok(json!({
            "hash": commit["hash"],
            "short": commit["short"],
            "subject": commit["subject"],
            "status": status,
        }))
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
        self.task_updated_at.remove(&task_id);
        self.task_stat_cache.remove(&task_id);
        if let Some(pid) = project_id {
            self.invalidate_external_scan(&pid);
        }
        // Released = the task record is gone (the worktree resurfaces as
        // external): its task-scope terminals and agent screen go with it.
        self.reap_orphaned_terminals();
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
            "created_at": self.task_created_at.get(task_id),
            "updated_at": self.task_updated_at.get(task_id),
            "stages": active
                .stages
                .iter()
                .map(|stage| stage_json(active, stage))
                .collect::<Vec<_>>(),
        })
    }

    /// A task's live diffstat for the `task.list` poll surface, cached for
    /// [`TASK_STAT_TTL`] so polling never repeats per-task git work. Terminal
    /// tasks (worktree pruned or about to be) report null.
    fn task_stat(&mut self, task_id: &str) -> Value {
        let Some(active) = self.tasks.get(task_id) else {
            return Value::Null;
        };
        if active.task.state.is_terminal() || !active.worktree.path.exists() {
            return Value::Null;
        }
        if let Some((computed_at, stat)) = self.task_stat_cache.get(task_id) {
            if computed_at.elapsed() < TASK_STAT_TTL {
                return stat.clone();
            }
        }
        let stat =
            crate::diff::diff_against_base(&active.worktree.path, &active.worktree.base_branch)
                .map(|diff| {
                    let s = diff.stat();
                    json!({
                        "files_changed": s.files_changed,
                        "insertions": s.insertions,
                        "deletions": s.deletions,
                    })
                })
                .unwrap_or(Value::Null);
        self.task_stat_cache.insert(
            task_id.to_string(),
            (std::time::Instant::now(), stat.clone()),
        );
        stat
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

/// A resolved `git.*` scope: the repository directory the RPC operates on,
/// plus — for task scope — what `git.log` needs to mark commits ahead of base
/// and `git.commit` needs to invalidate afterwards.
struct GitScope {
    repo_path: std::path::PathBuf,
    task: Option<GitScopeTask>,
}

struct GitScopeTask {
    task_id: String,
    base_branch: String,
}

/// Parse the required `paths` param of `git.stage`/`git.unstage`: a non-empty
/// array of repo-relative strings.
fn require_path_list(params: &Value) -> Result<Vec<String>, String> {
    let paths = params
        .get("paths")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing required param: paths".to_string())?;
    if paths.is_empty() {
        return Err("paths must not be empty".to_string());
    }
    paths
        .iter()
        .map(|path| {
            path.as_str()
                .map(str::to_string)
                .ok_or_else(|| "paths must be an array of strings".to_string())
        })
        .collect()
}

/// Resolve a client-supplied relative `path` under a worktree-backed `root`,
/// fenced on both ends (spec §4.1): the lexical fence
/// (`is_worktree_contained_path` — no `..`, no root, no non-Normal component)
/// PLUS canonical containment, which is what actually defeats a symlink
/// pointing outside the root (a symlink's own path components are all
/// Normal, so the lexical fence alone cannot catch it). `path` empty means
/// the scope root itself. Returns the joined (not canonicalized) path — safe
/// to use for further fs calls once containment is established.
fn fenced_scope_path(root: &std::path::Path, path: &str) -> Result<std::path::PathBuf, String> {
    if !path.is_empty() && !crate::task::is_worktree_contained_path(path) {
        return Err("path escapes the worktree".to_string());
    }
    let joined = if path.is_empty() {
        root.to_path_buf()
    } else {
        root.join(path)
    };
    let canonical_root =
        std::fs::canonicalize(root).map_err(|e| format!("cannot resolve scope root: {e}"))?;
    let canonical_target =
        std::fs::canonicalize(&joined).map_err(|e| format!("cannot read {path}: {e}"))?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err("path escapes the worktree".to_string());
    }
    Ok(joined)
}

/// Extension-based mime hint for `fs.read` previews (spec §4.3's pinned
/// table). `head` is (at most) the first 8 KiB of the file's content — used
/// only to distinguish text from binary when the extension doesn't match.
fn mime_hint(path: &std::path::Path, head: &[u8]) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase);
    match ext.as_deref() {
        Some("md") | Some("markdown") => "text/markdown",
        Some("html") | Some("htm") => "text/html",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("bmp") => "image/bmp",
        Some("json") => "application/json",
        Some("pdf") => "application/pdf",
        _ if head.contains(&0u8) => "application/octet-stream",
        _ => "text/plain",
    }
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
        "term.create" => term_create(state, &params),
        "term.attach" => term_attach(state, &sender, &params),
        "agent.attach" => agent_attach(state, &sender, &params),
        _ => state.lock().unwrap().dispatch(&method, &params),
    };
    match result {
        Ok(result) => json!({ "id": id, "ok": true, "result": result }),
        Err(message) => json!({ "id": id, "ok": false, "error": message }),
    }
}

/// The numeric suffix of a minted `term-<n>` id — the `term.list` sort key.
fn term_id_suffix(term_id: &str) -> u64 {
    term_id
        .strip_prefix("term-")
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(u64::MAX)
}

/// Create a keyed terminal: parse + resolve the scope server-side (never a
/// client path), enforce the cap, spawn `bash` in the scope root, and start
/// its pump immediately — the screen model accumulates even before the first
/// attach.
fn term_create(state: &Arc<Mutex<AppState>>, params: &Value) -> Result<Value, String> {
    let cols = params.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16;
    let rows = params.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16;
    let scope = TermScope::parse(params)?;

    let (term_id, rx) = {
        let mut s = state.lock().unwrap();
        let scope_root = scope.resolve_root(&mut s)?;
        if s.terms.len() >= MAX_USER_TERMINALS {
            return Err(format!(
                "terminal limit reached ({MAX_USER_TERMINALS} open terminals) — close one first"
            ));
        }
        let term_id = format!("term-{}", s.next_term);
        s.next_term += 1;
        let shell = s.term_shell.clone();
        let (term, rx) =
            TermSession::spawn(&shell, term_id.clone(), scope, scope_root, cols, rows)?;
        s.terms.insert(term_id.clone(), term);
        (term_id, rx)
    };
    spawn_term_pump(Arc::clone(state), term_id.clone(), rx);
    Ok(json!({ "term_id": term_id, "cols": cols, "rows": rows }))
}

/// Attach this client to an existing keyed terminal: register the caller's
/// [`SessionSender`] for live output and return the current **screen
/// snapshot** + cursor. Reconnect is just another attach — a new session
/// re-registers and gets a fresh snapshot. Creation is `term.create`'s job;
/// `agent:` ids belong to `agent.attach`.
fn term_attach(
    state: &Arc<Mutex<AppState>>,
    sender: &SessionSender,
    params: &Value,
) -> Result<Value, String> {
    let term_id = require_str(params, "term_id")?;
    if term_id.starts_with("agent:") {
        return Err("use agent.attach".to_string());
    }
    let cols = params.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16;
    let rows = params.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16;

    // Snapshot the screen and register this client atomically under the lock, so
    // the pump pushes only bytes *after* the cursor to the new sender — no gap, no
    // dupe across a reconnect.
    let mut s = state.lock().unwrap();
    let term = s.terms.get_mut(&term_id).ok_or("unknown term_id")?;

    // Match the PTY + screen model to this client's viewport, or TUIs (which draw
    // to the reported size) render to the wrong width and garble.
    if term.screen.cols != cols || term.screen.rows != rows {
        let _ = term.session.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        term.screen.set_size(cols, rows);
    }
    term.screen.register(sender);
    Ok(json!({
        "term_id": term.term_id,
        "snapshot": term.screen.snapshot(),
        "cursor": term.screen.total,
        "cols": term.screen.cols,
        "rows": term.screen.rows,
    }))
}

/// Attach this client to a task's agent screen: get-or-create the retained
/// screen, start a pump when a live session isn't being pumped yet, register
/// the sender, and return the current snapshot + cursor + `live`.
/// **Never errors because no session is running** — `live: false` with the
/// last (or blank) snapshot is the contract; only an unknown task errors (the
/// SPA falls back to its quiet state).
fn agent_attach(
    state: &Arc<Mutex<AppState>>,
    sender: &SessionSender,
    params: &Value,
) -> Result<Value, String> {
    let task_id = require_str(params, "task_id")?;
    // Grid defaults = the orchestrator's agent PTY size (40 rows × 120 cols).
    let cols = params.get("cols").and_then(Value::as_u64).unwrap_or(120) as u16;
    let rows = params.get("rows").and_then(Value::as_u64).unwrap_or(40) as u16;

    let mut guard = state.lock().unwrap();
    let s = &mut *guard;
    let Some(active) = s.tasks.get(&task_id) else {
        return Err("unknown task_id".to_string());
    };
    let live_session = active.subscribe_with_generation();

    let agent = s
        .agent_screens
        .entry(task_id.clone())
        .or_insert_with(|| AgentScreen {
            screen: TermScreen::new(cols, rows),
            pumped_generation: 0,
            live: false,
        });

    let mut pump = None;
    if let Some((generation, rx)) = live_session {
        if generation != agent.pumped_generation {
            agent.pumped_generation = generation;
            agent.live = true;
            pump = Some((generation, rx));
        }
        // Mid-session resize is allowed — it is a full PTY on the user's
        // machine; TUIs repaint. With no session live the retained last
        // screen is left untouched.
        if agent.screen.cols != cols || agent.screen.rows != rows {
            agent.screen.set_size(cols, rows);
            let _ = active.resize_session(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }

    agent.screen.register(sender);
    let response = json!({
        "term_id": format!("agent:{task_id}"),
        "live": agent.live,
        "snapshot": agent.screen.snapshot(),
        "cursor": agent.screen.total,
        "cols": agent.screen.cols,
        "rows": agent.screen.rows,
    });
    drop(guard);

    if let Some((generation, rx)) = pump {
        spawn_agent_pump(Arc::clone(state), task_id, generation, rx);
    }
    Ok(response)
}

/// Pump one agent session's PTY stream into the task's retained screen — the
/// same coalescing loop as user terminals, keyed `agent:<task_id>`, with two
/// differences. Start-of-session: reset the parser to a blank screen of the
/// current grid and push `term.reset` (clients wipe; the new session starts
/// clean) — `total` is NEVER reset, cursor monotonicity is what client dedupe
/// rides on. Session end (`Closed`): the screen is RETAINED as the tab's
/// "last screen" (`live = false`, `term.closed{reason:"agent_session_ended"}`)
/// instead of removed. A newer generation's pump supersedes this one — the
/// generation filter makes the stale task return.
fn spawn_agent_pump(
    state: Arc<Mutex<AppState>>,
    task_id: String,
    generation: u64,
    mut rx: broadcast::Receiver<Vec<u8>>,
) {
    tokio::spawn(async move {
        let term_id = format!("agent:{task_id}");
        {
            let mut s = state.lock().unwrap();
            let Some(agent) = s
                .agent_screens
                .get_mut(&task_id)
                .filter(|a| a.pumped_generation == generation)
            else {
                return;
            };
            agent.screen.parser = vt100::Parser::new(agent.screen.rows, agent.screen.cols, 2000);
            agent.screen.pending.clear();
            let payload = json!({
                "type": "term.reset",
                "term_id": term_id,
                "data": agent.screen.snapshot(),
                "cursor": agent.screen.total,
            });
            agent
                .screen
                .attached
                .retain(|snd| snd.push(payload.clone()));
        }
        let mut flush = tokio::time::interval(Duration::from_millis(TERM_FLUSH_MS));
        flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                recv = rx.recv() => match recv {
                    Ok(chunk) => {
                        let mut s = state.lock().unwrap();
                        let Some(agent) = s
                            .agent_screens
                            .get_mut(&task_id)
                            .filter(|a| a.pumped_generation == generation)
                        else {
                            return;
                        };
                        agent.screen.process(&chunk);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        // The phase ended / harness exited: retain the screen.
                        let mut s = state.lock().unwrap();
                        let Some(agent) = s
                            .agent_screens
                            .get_mut(&task_id)
                            .filter(|a| a.pumped_generation == generation)
                        else {
                            return;
                        };
                        agent.live = false;
                        agent.screen.flush(&term_id);
                        agent.screen.push_closed(&term_id, "agent_session_ended");
                        return;
                    }
                },
                _ = flush.tick() => {
                    let mut s = state.lock().unwrap();
                    let Some(agent) = s
                        .agent_screens
                        .get_mut(&task_id)
                        .filter(|a| a.pumped_generation == generation)
                    else {
                        return;
                    };
                    agent.screen.flush(&term_id);
                }
            }
        }
    });
}

/// Pump one terminal's PTY output into its screen model, coalescing bytes and
/// flushing one keyed frame per ~`TERM_FLUSH_MS` to every attached client. A
/// huge burst collapses to a screen snapshot so frame size/rate stay bounded
/// and control frames (the liveness ping) are never head-of-line-blocked
/// behind megabytes of output. One pump task per terminal: flush timing stays
/// independent (one flooding terminal never delays another's flush) and the
/// task terminates naturally on PTY EOF.
fn spawn_term_pump(
    state: Arc<Mutex<AppState>>,
    term_id: String,
    mut rx: broadcast::Receiver<Vec<u8>>,
) {
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
                        // Closed under the pump (term.close / reaper): done.
                        let Some(term) = s.terms.get_mut(&term_id) else { return; };
                        term.screen.process(&chunk);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        // PTY EOF — the shell exited on its own. Reap the exit
                        // status (no zombies) and tell every attached client.
                        let mut s = state.lock().unwrap();
                        let Some(term) = s.terms.remove(&term_id) else { return; };
                        term.session.kill_and_reap();
                        term.screen.push_closed(&term_id, "exited");
                        return;
                    }
                },
                _ = flush.tick() => {
                    let mut s = state.lock().unwrap();
                    let Some(term) = s.terms.get_mut(&term_id) else { return; };
                    term.screen.flush(&term_id);
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

    #[test]
    fn resolve_term_shell_yields_an_absolute_shell_path() {
        // Whatever the source (override, $SHELL, passwd, fallback), the result
        // must be an executable path, never empty.
        let shell = resolve_term_shell();
        assert!(shell.starts_with('/'), "{shell}");
    }

    #[test]
    fn capture_login_path_returns_the_shells_path() {
        let path = capture_login_path("/bin/bash", Duration::from_secs(10))
            .expect("bash must yield a PATH");
        assert!(path.contains('/'), "{path}");
        assert!(path.contains("bin"), "{path}");
    }

    #[test]
    fn capture_login_path_survives_a_missing_shell() {
        assert_eq!(
            capture_login_path("/nonexistent-shell-for-test", Duration::from_secs(5)),
            None
        );
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
    fn task_views_carry_timestamps_and_a_cached_diffstat() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );

        // A quick task builds and lands in review with committed work.
        let res = state.handle(req(
            "task.dispatch",
            json!({ "goal": "quick change", "kind": "quick" }),
        ));
        let task_id = res["result"]["task_id"].as_str().unwrap().to_string();
        assert_eq!(res["result"]["state"], "review");
        assert!(res["result"]["created_at"]
            .as_str()
            .is_some_and(|s| !s.is_empty()));
        assert!(res["result"]["updated_at"]
            .as_str()
            .is_some_and(|s| !s.is_empty()));

        let entry = |res: &Value| {
            res["result"]["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["task_id"] == json!(task_id.clone()))
                .unwrap()
                .clone()
        };

        // task.list carries the live diffstat for a reviewable task…
        let t = entry(&state.handle(req("task.list", json!({}))));
        assert!(t["stat"]["files_changed"].as_u64().unwrap() >= 1, "{t:?}");
        assert!(t["stat"]["insertions"].as_u64().unwrap() >= 1);

        // …served from cache on the next poll (identical, no per-poll git churn).
        let t2 = entry(&state.handle(req("task.list", json!({}))));
        assert_eq!(t["stat"], t2["stat"]);

        // A merged task has no worktree left: stat null, timestamps remain.
        state.handle(req("task.approve_merge", json!({ "task_id": task_id })));
        let t3 = entry(&state.handle(req("task.list", json!({}))));
        assert!(t3["stat"].is_null(), "{t3:?}");
        assert!(t3["updated_at"].as_str().is_some_and(|s| !s.is_empty()));
    }

    #[test]
    fn task_message_rejects_gates_and_unknown_tasks() {
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
            json!({ "goal": "quick", "kind": "quick" }),
        ));
        let task_id = res["result"]["task_id"].as_str().unwrap().to_string();
        assert_eq!(res["result"]["state"], "review");

        // At a review gate the structured verbs own the conversation.
        let gated = state.handle(req(
            "task.message",
            json!({ "task_id": task_id, "message": "hi" }),
        ));
        assert_eq!(gated["ok"], false);
        assert!(
            gated["error"].as_str().unwrap().contains("review gate"),
            "{gated:?}"
        );

        let unknown = state.handle(req(
            "task.message",
            json!({ "task_id": "nope", "message": "hi" }),
        ));
        assert_eq!(unknown["ok"], false);
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

    /// Shared state + handler for the keyed-terminal tests: the handler drives
    /// the RPC surface while the state handle lets tests inspect internals.
    fn shared_state_and_handler(
        repo: &std::path::Path,
        dir: &std::path::Path,
    ) -> (Arc<Mutex<AppState>>, FrameHandler) {
        let mut app = AppState::new(
            repo.to_path_buf(),
            dir.join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        // Deterministic terminals for tests: plain bash regardless of the dev
        // machine's login shell (production resolves the user's own shell).
        app.term_shell = "/bin/bash".into();
        let state = app.shared();
        let handler = AppState::handler(Arc::clone(&state));
        (state, handler)
    }

    /// Poll an observable sender's captured pushes until the decrypted history
    /// satisfies `pred` (returning everything seen), or panic after 10 s.
    async fn wait_for_pushes(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<tokio_tungstenite::tungstenite::Message>,
        session_key: &str,
        pred: impl Fn(&[Value]) -> bool,
    ) -> Vec<Value> {
        let mut seen = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            while let Ok(message) = rx.try_recv() {
                seen.push(SessionSender::decrypt_push(session_key, &message));
            }
            if pred(&seen) {
                return seen;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for a matching push; saw: {seen:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Wait until one push matches `pred`.
    async fn wait_for_push(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<tokio_tungstenite::tungstenite::Message>,
        session_key: &str,
        pred: impl Fn(&Value) -> bool,
    ) -> Vec<Value> {
        wait_for_pushes(rx, session_key, |seen| seen.iter().any(&pred)).await
    }

    /// The concatenated bytes of every `term.output` push for `term_id`.
    fn output_text(pushes: &[Value], term_id: &str) -> String {
        let mut bytes = Vec::new();
        for p in pushes {
            if p["type"] == "term.output" && p["term_id"] == term_id {
                bytes.extend_from_slice(&b64decode(p["data"].as_str().unwrap()).unwrap());
            }
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    /// True once `pid` is fully gone from the process table (killed AND reaped —
    /// a zombie still shows up in `ps` with state Z).
    fn process_reaped(pid: u32) -> bool {
        let out = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        !out.status.success() || String::from_utf8_lossy(&out.stdout).trim().is_empty()
    }

    #[tokio::test]
    async fn keyed_terminal_create_attach_io_close_roundtrip() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let project_id = state.lock().unwrap().projects[0].id.clone();

        // Create in the primary scope: bash starts in the repo root and the
        // pump runs before any attach.
        let created = handler(
            SessionSender::detached("s1"),
            req(
                "term.create",
                json!({ "project_id": project_id, "cols": 80, "rows": 24 }),
            ),
        );
        assert_eq!(created["ok"], true, "{created:?}");
        assert_eq!(created["result"]["term_id"], "term-1");
        assert_eq!(created["result"]["cols"], 80);
        assert_eq!(created["result"]["rows"], 24);

        // Listed under its scope, with metadata.
        let listed = handler(
            SessionSender::detached("s1"),
            req("term.list", json!({ "project_id": project_id })),
        );
        let terminals = listed["result"]["terminals"].as_array().unwrap();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0]["term_id"], "term-1");
        assert!(terminals[0]["created_at"]
            .as_str()
            .is_some_and(|s| !s.is_empty()));

        // Attach with an observable sender, then type a command: the echo comes
        // back as keyed term.output pushes.
        let (sender, mut pushes, key) = SessionSender::observable("s1");
        let attached = handler(
            sender,
            req(
                "term.attach",
                json!({ "term_id": "term-1", "cols": 80, "rows": 24 }),
            ),
        );
        assert_eq!(attached["ok"], true, "{attached:?}");
        assert_eq!(attached["result"]["term_id"], "term-1");
        assert!(attached["result"]["snapshot"].is_string());
        assert!(attached["result"]["cursor"].is_u64());

        let input = b64encode(b"echo keyed-term-ok\r");
        let wrote = handler(
            SessionSender::detached("s1"),
            req("term.input", json!({ "term_id": "term-1", "data": input })),
        );
        assert_eq!(wrote["ok"], true, "{wrote:?}");
        wait_for_pushes(&mut pushes, &key, |seen| {
            output_text(seen, "term-1").contains("keyed-term-ok")
        })
        .await;

        // Close: the PTY is killed AND reaped, the entry is gone, and every
        // attached client hears term.closed{reason:"closed"}.
        let pid = state.lock().unwrap().terms["term-1"].session.pid().unwrap();
        let closed = handler(
            SessionSender::detached("s1"),
            req("term.close", json!({ "term_id": "term-1" })),
        );
        assert_eq!(closed["ok"], true, "{closed:?}");
        let seen = wait_for_push(&mut pushes, &key, |p| {
            p["type"] == "term.closed" && p["term_id"] == "term-1" && p["reason"] == "closed"
        })
        .await;
        assert!(!seen.is_empty());
        assert!(state.lock().unwrap().terms.is_empty());
        assert!(process_reaped(pid), "the shell must be killed and reaped");

        let relisted = handler(
            SessionSender::detached("s1"),
            req("term.list", json!({ "project_id": project_id })),
        );
        assert_eq!(relisted["result"]["terminals"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn keyed_terminal_snapshot_reflects_input_across_reattach() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let project_id = state.lock().unwrap().projects[0].id.clone();

        let created = handler(
            SessionSender::detached("s1"),
            req("term.create", json!({ "project_id": project_id })),
        );
        let term_id = created["result"]["term_id"].as_str().unwrap().to_string();
        let a = handler(
            SessionSender::detached("s1"),
            req(
                "term.attach",
                json!({ "term_id": term_id, "cols": 80, "rows": 24 }),
            ),
        );
        assert_eq!(a["ok"], true);
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send a command (the PTY echoes it and runs it).
        let input = b64encode(b"echo build-terminal-ok\n");
        handler(
            SessionSender::detached("s1"),
            req("term.input", json!({ "term_id": term_id, "data": input })),
        );
        tokio::time::sleep(Duration::from_millis(700)).await;

        // Reconnect = a fresh attach. The screen snapshot (vt100 model) must reflect
        // the prior output — that's snapshot-based resync, not byte replay.
        let b = handler(
            SessionSender::detached("s2"),
            req(
                "term.attach",
                json!({ "term_id": term_id, "cols": 80, "rows": 24 }),
            ),
        );
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
    async fn term_list_filters_by_scope_and_orders_numerically() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let project_id = state.lock().unwrap().projects[0].id.clone();

        // A task-scope terminal and two primary-scope terminals, with a minted
        // suffix jump so numeric ordering differs from lexicographic.
        let dispatched = handler(
            SessionSender::detached("s1"),
            req(
                "task.dispatch",
                json!({ "goal": "scope filter", "kind": "quick" }),
            ),
        );
        let task_id = dispatched["result"]["task_id"]
            .as_str()
            .unwrap()
            .to_string();

        let first = handler(
            SessionSender::detached("s1"),
            req("term.create", json!({ "project_id": project_id })),
        );
        assert_eq!(first["result"]["term_id"], "term-1");
        let task_term = handler(
            SessionSender::detached("s1"),
            req("term.create", json!({ "task_id": task_id })),
        );
        assert_eq!(task_term["result"]["term_id"], "term-2");
        state.lock().unwrap().next_term = 10;
        let tenth = handler(
            SessionSender::detached("s1"),
            req("term.create", json!({ "project_id": project_id })),
        );
        assert_eq!(tenth["result"]["term_id"], "term-10");

        let primary = handler(
            SessionSender::detached("s1"),
            req("term.list", json!({ "project_id": project_id })),
        );
        let ids: Vec<&str> = primary["result"]["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["term_id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec!["term-1", "term-10"],
            "scope-filtered, numeric order"
        );

        let task_scoped = handler(
            SessionSender::detached("s1"),
            req("term.list", json!({ "task_id": task_id })),
        );
        let ids: Vec<&str> = task_scoped["result"]["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["term_id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["term-2"]);

        // Unknown-scope ids still error (the SPA treats an error as "no terminals").
        let unknown = handler(
            SessionSender::detached("s1"),
            req("term.list", json!({ "project_id": "proj-99" })),
        );
        assert_eq!(unknown["ok"], false);
    }

    #[tokio::test]
    async fn term_create_enforces_the_daemon_wide_cap() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let project_id = state.lock().unwrap().projects[0].id.clone();

        for _ in 0..MAX_USER_TERMINALS {
            let created = handler(
                SessionSender::detached("s1"),
                req("term.create", json!({ "project_id": project_id })),
            );
            assert_eq!(created["ok"], true, "{created:?}");
        }
        let over = handler(
            SessionSender::detached("s1"),
            req("term.create", json!({ "project_id": project_id })),
        );
        assert_eq!(over["ok"], false);
        assert_eq!(
            over["error"],
            "terminal limit reached (16 open terminals) — close one first"
        );
    }

    #[tokio::test]
    async fn keyed_term_rpcs_reject_unknown_missing_and_agent_ids() {
        let (dir, repo) = init_repo();
        let (_state, handler) = shared_state_and_handler(&repo, dir.path());
        let call = |method: &str, params: Value| {
            handler(SessionSender::detached("s1"), req(method, params))
        };

        // The legacy un-keyed path is gone: no term_id is an error everywhere.
        for method in ["term.attach", "term.input", "term.resize", "term.close"] {
            let res = call(
                method,
                json!({ "data": b64encode(b"x"), "cols": 80, "rows": 24 }),
            );
            assert_eq!(res["ok"], false, "{method} without term_id must fail");
            assert_eq!(res["error"], "missing required param: term_id", "{method}");
        }

        // Unknown ids.
        let res = call("term.attach", json!({ "term_id": "term-99" }));
        assert_eq!(res["error"], "unknown term_id");
        let res = call("term.close", json!({ "term_id": "term-99" }));
        assert_eq!(res["error"], "unknown term_id");
        let res = call(
            "term.input",
            json!({ "term_id": "term-99", "data": b64encode(b"x") }),
        );
        assert_eq!(res["error"], "unknown term_id");
        let res = call(
            "term.resize",
            json!({ "term_id": "term-99", "cols": 80, "rows": 24 }),
        );
        assert_eq!(res["error"], "unknown term_id");

        // Agent ids: attach/close are gated to the agent surface; an unknown
        // task behind an agent id is still "unknown term_id".
        let res = call("term.attach", json!({ "term_id": "agent:task-1" }));
        assert_eq!(res["error"], "use agent.attach");
        let res = call("term.close", json!({ "term_id": "agent:task-1" }));
        assert_eq!(res["error"], "cannot close an agent terminal");
        let res = call(
            "term.input",
            json!({ "term_id": "agent:task-99", "data": b64encode(b"x") }),
        );
        assert_eq!(res["error"], "unknown term_id");
    }

    #[tokio::test]
    async fn term_input_and_resize_route_to_the_agent_session_by_id() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());

        // A task whose warm harness stays alive and drains stdin — the live
        // agent-session case.
        let side = Orchestrator::new(
            repo.clone(),
            dir.path().join("wt-side"),
            Agent::Warm(HarnessSpec::new("sh").arg("-c").arg("cat >/dev/null")),
            Templates::default(),
        );
        let active = side
            .dispatch(
                crate::task::TaskId::new("task-9"),
                "live agent",
                TaskKind::Quick,
                "main",
                Default::default(),
            )
            .unwrap();
        {
            let mut s = state.lock().unwrap();
            let project_id = s.projects[0].id.clone();
            s.task_project.insert("task-9".into(), project_id);
            s.tasks.insert("task-9".into(), active);
        }
        let call = |method: &str, params: Value| {
            handler(SessionSender::detached("s1"), req(method, params))
        };

        // Live: input drains into the PTY, resize reports live.
        let res = call(
            "term.input",
            json!({ "term_id": "agent:task-9", "data": b64encode(b"hi\r") }),
        );
        assert_eq!(res["ok"], true, "{res:?}");
        let res = call(
            "term.resize",
            json!({ "term_id": "agent:task-9", "cols": 100, "rows": 30 }),
        );
        assert_eq!(res["result"]["live"], true, "{res:?}");

        // Dead: input errors with the contract message, resize is a no-op that
        // reports not-live (never garbling the retained last screen).
        state
            .lock()
            .unwrap()
            .tasks
            .get_mut("task-9")
            .unwrap()
            .end_session();
        let res = call(
            "term.input",
            json!({ "term_id": "agent:task-9", "data": b64encode(b"hi\r") }),
        );
        assert_eq!(res["error"], "no active agent session");
        let res = call(
            "term.resize",
            json!({ "term_id": "agent:task-9", "cols": 100, "rows": 30 }),
        );
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["live"], false);
    }

    /// Dispatch a task through a side orchestrator whose warm harness both
    /// drains stdin (so prompt writes never block) and emits a heartbeat line
    /// (so live streaming is observable), then register it in the shared state.
    fn insert_live_task(
        state: &Arc<Mutex<AppState>>,
        repo: &std::path::Path,
        side_root: std::path::PathBuf,
        task_id: &str,
    ) {
        let side = Orchestrator::new(
            repo.to_path_buf(),
            side_root,
            Agent::Warm(
                HarnessSpec::new("sh")
                    .arg("-c")
                    .arg("(while :; do echo agent-beat; sleep 0.05; done) & cat >/dev/null"),
            ),
            Templates::default(),
        );
        let active = side
            .dispatch(
                crate::task::TaskId::new(task_id),
                "live agent",
                TaskKind::Quick,
                "main",
                Default::default(),
            )
            .unwrap();
        let mut s = state.lock().unwrap();
        let project_id = s.projects[0].id.clone();
        s.task_project.insert(task_id.to_string(), project_id);
        s.tasks.insert(task_id.to_string(), active);
    }

    #[tokio::test]
    async fn agent_attach_streams_the_live_session_and_retains_the_last_screen() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        insert_live_task(&state, &repo, dir.path().join("wt-side"), "task-9");

        let (sender, mut pushes, key) = SessionSender::observable("s1");
        let res = handler(
            sender,
            req(
                "agent.attach",
                json!({ "task_id": "task-9", "cols": 100, "rows": 30 }),
            ),
        );
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["term_id"], "agent:task-9");
        assert_eq!(res["result"]["live"], true);
        assert_eq!(res["result"]["cols"], 100);
        assert_eq!(res["result"]["rows"], 30);

        // The pump announces the session start with a reset, then streams live
        // output under the reserved agent id.
        let seen = wait_for_pushes(&mut pushes, &key, |seen| {
            output_text(seen, "agent:task-9").contains("agent-beat")
        })
        .await;
        assert_eq!(
            seen[0]["type"], "term.reset",
            "start-of-session reset first: {seen:?}"
        );

        // A live resize through term.resize reshapes the agent screen too.
        let resized = handler(
            SessionSender::detached("s1"),
            req(
                "term.resize",
                json!({ "term_id": "agent:task-9", "cols": 90, "rows": 28 }),
            ),
        );
        assert_eq!(resized["result"]["live"], true, "{resized:?}");
        {
            let s = state.lock().unwrap();
            assert_eq!(s.agent_screens["task-9"].screen.cols, 90);
            assert_eq!(s.agent_screens["task-9"].screen.rows, 28);
        }

        // The phase ends: clients hear agent_session_ended and the screen is
        // RETAINED — the next attach shows the last screen, quietly not-live.
        state
            .lock()
            .unwrap()
            .tasks
            .get_mut("task-9")
            .unwrap()
            .end_session();
        wait_for_push(&mut pushes, &key, |p| {
            p["type"] == "term.closed"
                && p["term_id"] == "agent:task-9"
                && p["reason"] == "agent_session_ended"
        })
        .await;

        let res = handler(
            SessionSender::detached("s2"),
            req("agent.attach", json!({ "task_id": "task-9" })),
        );
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["live"], false);
        let snap = String::from_utf8_lossy(
            &b64decode(res["result"]["snapshot"].as_str().unwrap()).unwrap(),
        )
        .into_owned();
        assert!(
            snap.contains("agent-beat"),
            "last screen retained: {snap:?}"
        );
    }

    #[tokio::test]
    async fn agent_attach_is_quiet_with_no_session_and_errors_on_unknown_tasks() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());

        // A recovered task with no live session (e.g. after a daemon restart).
        let idle = ActiveTask::reattach(
            crate::task::Task::new(crate::task::TaskId::new("task-9"), "quiet", TaskKind::Quick),
            crate::worktree::Worktree {
                name: "wt".into(),
                path: repo.clone(),
                branch: "build/quiet".into(),
                base_branch: "main".into(),
            },
            ".build/plan.md".into(),
            None,
            Default::default(),
            None,
            vec![],
            None,
            None,
            false,
            vec![],
            false,
            false,
        );
        state.lock().unwrap().tasks.insert("task-9".into(), idle);

        // Attach must NOT error: live:false, a blank screen at the agent PTY's
        // default grid (120×40), cursor 0.
        let (sender, _pushes, _key) = SessionSender::observable("s-dead");
        let res = handler(sender, req("agent.attach", json!({ "task_id": "task-9" })));
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["live"], false);
        assert_eq!(res["result"]["cols"], 120);
        assert_eq!(res["result"]["rows"], 40);
        assert_eq!(res["result"]["cursor"], 0);
        assert!(b64decode(res["result"]["snapshot"].as_str().unwrap()).is_ok());

        // Only an unknown task errors.
        let res = handler(
            SessionSender::detached("s1"),
            req("agent.attach", json!({ "task_id": "task-99" })),
        );
        assert_eq!(res["error"], "unknown task_id");

        // A close frame detaches the session's sender from the agent screen.
        let close = Frame {
            session_id: "s-dead".into(),
            message_id: String::new(),
            frame_type: "close".into(),
            sender: "relay".into(),
            created_at: String::new(),
            payload: Value::Null,
        };
        dispatch_frame(&state, SessionSender::detached("s-dead"), close);
        assert!(state.lock().unwrap().agent_screens["task-9"]
            .screen
            .attached
            .is_empty());
    }

    #[tokio::test]
    async fn a_session_starting_after_attach_reaches_the_attached_viewer() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());

        // The viewer attaches while the task has no session at all.
        let idle = ActiveTask::reattach(
            crate::task::Task::new(crate::task::TaskId::new("task-9"), "quiet", TaskKind::Quick),
            crate::worktree::Worktree {
                name: "wt".into(),
                path: repo.clone(),
                branch: "build/quiet".into(),
                base_branch: "main".into(),
            },
            ".build/plan.md".into(),
            None,
            Default::default(),
            None,
            vec![],
            None,
            None,
            false,
            vec![],
            false,
            false,
        );
        {
            let mut s = state.lock().unwrap();
            let project_id = s.projects[0].id.clone();
            s.task_project.insert("task-9".into(), project_id);
            s.tasks.insert("task-9".into(), idle);
        }
        let (sender, mut pushes, key) = SessionSender::observable("s1");
        let res = handler(sender, req("agent.attach", json!({ "task_id": "task-9" })));
        assert_eq!(res["result"]["live"], false, "{res:?}");

        // A session spawns (approve → build). The next mutation's
        // finish_mutation tail must start a pump for the attached viewer —
        // through the Weak self-handle set by shared().
        insert_live_task(&state, &repo, dir.path().join("wt-side"), "task-9");
        let res = handler(
            SessionSender::detached("s1"),
            req(
                "task.set_auto_advance",
                json!({ "task_id": "task-9", "enabled": false }),
            ),
        );
        assert_eq!(res["ok"], true, "{res:?}");

        let seen = wait_for_pushes(&mut pushes, &key, |seen| {
            output_text(seen, "agent:task-9").contains("agent-beat")
        })
        .await;
        assert_eq!(seen[0]["type"], "term.reset", "{seen:?}");
        let s = state.lock().unwrap();
        assert!(s.agent_screens["task-9"].live);
        assert_eq!(s.agent_screens["task-9"].pumped_generation, 1);
    }

    #[tokio::test]
    async fn task_delete_prompt_closes_terminals_and_drops_the_agent_screen() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let call = |method: &str, params: Value| {
            handler(SessionSender::detached("s1"), req(method, params))
        };

        let dispatched = call(
            "task.dispatch",
            json!({ "goal": "delete me", "kind": "quick" }),
        );
        let task_id = dispatched["result"]["task_id"]
            .as_str()
            .unwrap()
            .to_string();
        let created = call("term.create", json!({ "task_id": task_id }));
        let term_id = created["result"]["term_id"].as_str().unwrap().to_string();
        let attached = call("agent.attach", json!({ "task_id": task_id }));
        assert_eq!(attached["ok"], true, "{attached:?}");
        let pid = state.lock().unwrap().terms[&term_id].session.pid().unwrap();

        // Keep the worktree through the merge so only the delete closes things.
        let merged = call(
            "task.approve_merge",
            json!({ "task_id": task_id, "cleanup": "keep" }),
        );
        assert_eq!(merged["result"]["state"], "merged", "{merged:?}");
        assert!(state.lock().unwrap().terms.contains_key(&term_id));

        let deleted = call("task.delete", json!({ "task_id": task_id }));
        assert_eq!(deleted["ok"], true, "{deleted:?}");
        let s = state.lock().unwrap();
        assert!(
            !s.terms.contains_key(&term_id),
            "delete prompt-closes the task's terminals"
        );
        assert!(
            !s.agent_screens.contains_key(&task_id),
            "delete drops the retained agent screen"
        );
        drop(s);
        assert!(process_reaped(pid));
    }

    #[tokio::test]
    async fn reaper_closes_terminals_whose_scope_vanished_out_of_band() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let project_id = state.lock().unwrap().projects[0].id.clone();

        let ext_path = add_external_worktree(&repo, dir.path(), "user-wt", "hotfix/thing");
        handler(SessionSender::detached("s1"), req("task.list", json!({})));
        let worktree_id = {
            let mut s = state.lock().unwrap();
            let externals = s.external_worktrees_json();
            externals[0]["worktree_id"].as_str().unwrap().to_string()
        };
        let created = handler(
            SessionSender::detached("s1"),
            req(
                "term.create",
                json!({ "project_id": project_id, "worktree_id": worktree_id }),
            ),
        );
        assert_eq!(created["ok"], true, "{created:?}");
        let term_id = created["result"]["term_id"].as_str().unwrap().to_string();
        let (sender, mut pushes, key) = SessionSender::observable("s1");
        handler(sender, req("term.attach", json!({ "term_id": term_id })));
        let pid = state.lock().unwrap().terms[&term_id].session.pid().unwrap();

        // Nothing to reap while the worktree exists.
        assert!(state.lock().unwrap().reap_orphaned_terminals().is_empty());

        // The user rm -rf's the external worktree: the reaper closes its
        // terminal, reaps the shell, and tells the attached clients.
        std::fs::remove_dir_all(&ext_path).unwrap();
        let reaped = state.lock().unwrap().reap_orphaned_terminals();
        assert_eq!(reaped, vec![term_id.clone()]);
        wait_for_push(&mut pushes, &key, |p| {
            p["type"] == "term.closed" && p["term_id"] == term_id && p["reason"] == "reaped"
        })
        .await;
        assert!(state.lock().unwrap().terms.is_empty());
        assert!(process_reaped(pid));
    }

    #[tokio::test]
    async fn task_mutations_reap_task_scope_terminals() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let call = |method: &str, params: Value| {
            handler(SessionSender::detached("s1"), req(method, params))
        };

        // Abandon prunes the worktree → its terminal is reaped via the
        // finish_mutation tail, with no reaper loop involved.
        let dispatched = call(
            "task.dispatch",
            json!({ "goal": "reap me", "kind": "quick" }),
        );
        let task_id = dispatched["result"]["task_id"]
            .as_str()
            .unwrap()
            .to_string();
        let created = call("term.create", json!({ "task_id": task_id }));
        let doomed_term = created["result"]["term_id"].as_str().unwrap().to_string();
        let pid = state.lock().unwrap().terms[&doomed_term]
            .session
            .pid()
            .unwrap();
        call("task.abandon", json!({ "task_id": task_id }));
        assert!(
            !state.lock().unwrap().terms.contains_key(&doomed_term),
            "abandon prunes the worktree, so its terminal closes"
        );
        assert!(process_reaped(pid));

        // A merge with cleanup=keep keeps the worktree → the terminal survives.
        let dispatched = call(
            "task.dispatch",
            json!({ "goal": "keep me", "kind": "quick" }),
        );
        let task_id = dispatched["result"]["task_id"]
            .as_str()
            .unwrap()
            .to_string();
        let created = call("term.create", json!({ "task_id": task_id }));
        let kept_term = created["result"]["term_id"].as_str().unwrap().to_string();
        let merged = call(
            "task.approve_merge",
            json!({ "task_id": task_id, "cleanup": "keep" }),
        );
        assert_eq!(merged["result"]["state"], "merged", "{merged:?}");
        assert!(
            state.lock().unwrap().terms.contains_key(&kept_term),
            "cleanup=keep keeps the worktree, so its terminal survives"
        );
    }

    #[tokio::test]
    async fn pump_eof_reaps_the_terminal_and_pushes_exited() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let project_id = state.lock().unwrap().projects[0].id.clone();

        handler(
            SessionSender::detached("s1"),
            req("term.create", json!({ "project_id": project_id })),
        );
        let (sender, mut pushes, key) = SessionSender::observable("s1");
        handler(sender, req("term.attach", json!({ "term_id": "term-1" })));
        let pid = state.lock().unwrap().terms["term-1"].session.pid().unwrap();

        // The user types `exit`: the shell ends on its own (PTY EOF).
        handler(
            SessionSender::detached("s1"),
            req(
                "term.input",
                json!({ "term_id": "term-1", "data": b64encode(b"exit\r") }),
            ),
        );
        wait_for_push(&mut pushes, &key, |p| {
            p["type"] == "term.closed" && p["term_id"] == "term-1" && p["reason"] == "exited"
        })
        .await;
        assert!(state.lock().unwrap().terms.is_empty());
        assert!(process_reaped(pid), "an exited shell must still be reaped");
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
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let project_id = state.lock().unwrap().projects[0].id.clone();
        handler(
            SessionSender::detached("s-live"),
            req("term.create", json!({ "project_id": project_id })),
        );
        handler(
            SessionSender::detached("s-live"),
            req("term.attach", json!({ "term_id": "term-1" })),
        );
        handler(
            SessionSender::detached("s-dead"),
            req("term.attach", json!({ "term_id": "term-1" })),
        );

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
        let attached: Vec<&str> = s.terms["term-1"]
            .screen
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
    fn term_scope_parses_the_wire_table() {
        // task_id wins even when project_id is also present.
        assert_eq!(
            TermScope::parse(&json!({ "task_id": "task-1", "project_id": "proj-1" })).unwrap(),
            TermScope::Task {
                task_id: "task-1".into()
            }
        );
        assert_eq!(
            TermScope::parse(&json!({ "project_id": "proj-1", "worktree_id": "wt-abc" })).unwrap(),
            TermScope::ExternalWorktree {
                project_id: "proj-1".into(),
                worktree_id: "wt-abc".into()
            }
        );
        assert_eq!(
            TermScope::parse(&json!({ "project_id": "proj-1" })).unwrap(),
            TermScope::Primary {
                project_id: "proj-1".into()
            }
        );
        let missing = "missing scope: task_id or project_id required";
        assert_eq!(TermScope::parse(&json!({})).unwrap_err(), missing);
        // A worktree_id without its project is not a scope.
        assert_eq!(
            TermScope::parse(&json!({ "worktree_id": "wt-abc" })).unwrap_err(),
            missing
        );
    }

    #[test]
    fn term_scope_resolves_roots_from_server_records_only() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let project_id = state.projects[0].id.clone();

        // Primary → the registered repo path.
        let root = TermScope::Primary {
            project_id: project_id.clone(),
        }
        .resolve_root(&mut state)
        .unwrap();
        assert_eq!(root, std::fs::canonicalize(&repo).unwrap());
        assert_eq!(
            TermScope::Primary {
                project_id: "proj-99".into()
            }
            .resolve_root(&mut state)
            .unwrap_err(),
            "unknown project_id"
        );

        // Task → the task's worktree path, which must still exist on disk.
        let res = state.handle(req(
            "task.dispatch",
            json!({ "goal": "scope me", "kind": "quick" }),
        ));
        let task_id = res["result"]["task_id"].as_str().unwrap().to_string();
        let scope = TermScope::Task {
            task_id: task_id.clone(),
        };
        let worktree_path = state.tasks[&task_id].worktree.path.clone();
        assert_eq!(scope.resolve_root(&mut state).unwrap(), worktree_path);
        assert_eq!(
            TermScope::Task {
                task_id: "task-99".into()
            }
            .resolve_root(&mut state)
            .unwrap_err(),
            "unknown task_id"
        );
        std::fs::remove_dir_all(&worktree_path).unwrap();
        assert_eq!(
            scope.resolve_root(&mut state).unwrap_err(),
            "worktree no longer exists"
        );

        // External → resolved through the discovery scan; ids never raw paths.
        let ext_path = add_external_worktree(&repo, dir.path(), "user-wt", "hotfix/thing");
        let list = state.handle(req("task.list", json!({})));
        let worktree_id = list["result"]["external_worktrees"][0]["worktree_id"]
            .as_str()
            .unwrap()
            .to_string();
        let root = TermScope::ExternalWorktree {
            project_id: project_id.clone(),
            worktree_id,
        }
        .resolve_root(&mut state)
        .unwrap();
        assert_eq!(root, std::fs::canonicalize(&ext_path).unwrap());
        let unknown = TermScope::ExternalWorktree {
            project_id,
            worktree_id: "wt-nope".into(),
        }
        .resolve_root(&mut state)
        .unwrap_err();
        assert!(unknown.contains("unknown worktree_id"), "{unknown}");
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

    // --- fs.tree / fs.read (spec §4) --------------------------------------------

    #[test]
    fn fs_tree_lists_one_level_dirs_first_case_insensitive_and_skips_git() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let project_id = state.projects[0].id.clone();

        std::fs::create_dir(repo.join("Zdir")).unwrap();
        std::fs::create_dir(repo.join("adir")).unwrap();
        std::fs::write(repo.join("adir/nested.txt"), "nested\n").unwrap();
        std::fs::write(repo.join("B.txt"), "b\n").unwrap();
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();

        let res = state.handle(req("fs.tree", json!({ "project_id": project_id })));
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["path"], "");
        let entries = res["result"]["entries"].as_array().unwrap();
        let names: Vec<&str> = entries
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert!(!names.contains(&".git"), "{names:?}");

        // dirs-first, each group case-insensitive.
        let kinds: Vec<&str> = entries
            .iter()
            .map(|e| e["kind"].as_str().unwrap())
            .collect();
        let last_dir = kinds.iter().rposition(|k| *k == "dir");
        let first_file = kinds.iter().position(|k| *k == "file");
        if let (Some(last_dir), Some(first_file)) = (last_dir, first_file) {
            assert!(last_dir < first_file, "{kinds:?}");
        }
        let dir_names: Vec<&str> = entries
            .iter()
            .filter(|e| e["kind"] == "dir")
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(dir_names, vec!["adir", "Zdir"]);
        let file_names: Vec<&str> = entries
            .iter()
            .filter(|e| e["kind"] == "file")
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(file_names, vec!["a.txt", "B.txt", "README.md"]);
        let readme = entries.iter().find(|e| e["name"] == "README.md").unwrap();
        assert!(readme["size"].as_u64().unwrap() > 0);

        // One level only: nested.txt is not listed at the root.
        assert!(!names.contains(&"nested.txt"));

        // Recurse one level via `path`.
        let sub = state.handle(req(
            "fs.tree",
            json!({ "project_id": project_id, "path": "adir" }),
        ));
        assert_eq!(sub["ok"], true, "{sub:?}");
        assert_eq!(sub["result"]["path"], "adir");
        let sub_entries = sub["result"]["entries"].as_array().unwrap();
        assert_eq!(sub_entries.len(), 1);
        assert_eq!(sub_entries[0]["name"], "nested.txt");
        assert_eq!(sub_entries[0]["kind"], "file");
    }

    #[test]
    fn fs_tree_rejects_escapes_and_non_directories() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let project_id = state.projects[0].id.clone();

        let escape = state.handle(req(
            "fs.tree",
            json!({ "project_id": project_id, "path": "../../../etc" }),
        ));
        assert_eq!(escape["ok"], false, "{escape:?}");
        assert!(
            escape["error"].as_str().unwrap().contains("escapes"),
            "{escape:?}"
        );

        let not_dir = state.handle(req(
            "fs.tree",
            json!({ "project_id": project_id, "path": "README.md" }),
        ));
        assert_eq!(not_dir["ok"], false, "{not_dir:?}");
        assert_eq!(not_dir["error"], "not a directory");
    }

    #[test]
    fn fs_tree_rejects_a_symlinked_directory_escape() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let project_id = state.projects[0].id.clone();

        // A directory symlink inside the worktree pointing outside it: every
        // lexical component is Normal, so only canonical containment (the same
        // fence fs.read rides) can refuse listing through it.
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret\n").unwrap();
        std::os::unix::fs::symlink(&outside, repo.join("linkdir")).unwrap();

        let escape = state.handle(req(
            "fs.tree",
            json!({ "project_id": project_id, "path": "linkdir" }),
        ));
        assert_eq!(escape["ok"], false, "{escape:?}");
        assert!(
            escape["error"].as_str().unwrap().contains("escapes"),
            "{escape:?}"
        );

        // Nested through the symlinked directory is refused the same way.
        let nested = state.handle(req(
            "fs.tree",
            json!({ "project_id": project_id, "path": "linkdir/sub" }),
        ));
        assert_eq!(nested["ok"], false, "{nested:?}");
    }

    #[test]
    fn fs_tree_serves_all_three_scope_kinds() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let project_id = state.projects[0].id.clone();

        // Task scope.
        let dispatched = state.handle(req(
            "task.dispatch",
            json!({ "goal": "fs tree scope", "kind": "quick" }),
        ));
        let task_id = dispatched["result"]["task_id"]
            .as_str()
            .unwrap()
            .to_string();
        let task_tree = state.handle(req("fs.tree", json!({ "task_id": task_id })));
        assert_eq!(task_tree["ok"], true, "{task_tree:?}");
        let task_names: Vec<String> = task_tree["result"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap().to_string())
            .collect();
        assert!(
            task_names.contains(&"README.md".to_string()),
            "{task_names:?}"
        );
        assert!(!task_names.contains(&".git".to_string()));

        // External worktree scope.
        let ext_path = add_external_worktree(&repo, dir.path(), "fs-tree-wt", "fs/tree");
        std::fs::write(ext_path.join("extra.txt"), "extra\n").unwrap();
        let list = state.handle(req("task.list", json!({})));
        let worktree_id = list["result"]["external_worktrees"][0]["worktree_id"]
            .as_str()
            .unwrap()
            .to_string();
        let ext_tree = state.handle(req(
            "fs.tree",
            json!({ "project_id": project_id, "worktree_id": worktree_id }),
        ));
        assert_eq!(ext_tree["ok"], true, "{ext_tree:?}");
        let ext_names: Vec<String> = ext_tree["result"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap().to_string())
            .collect();
        assert!(
            ext_names.contains(&"extra.txt".to_string()),
            "{ext_names:?}"
        );
        assert!(!ext_names.contains(&".git".to_string()));
    }

    #[test]
    fn fs_read_round_trips_content_and_infers_mime() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let project_id = state.projects[0].id.clone();

        std::fs::write(repo.join("notes.md"), "# hi\n").unwrap();
        std::fs::write(repo.join("page.html"), "<h1>hi</h1>\n").unwrap();
        std::fs::write(repo.join("icon.svg"), "<svg></svg>\n").unwrap();
        std::fs::write(repo.join("pic.png"), b"\x89PNG\r\n\x1a\nrest").unwrap();
        std::fs::write(repo.join("plain.txt"), "just text\n").unwrap();
        std::fs::write(repo.join("blob.bin"), [0u8, 1, 2, 3, 0, 4]).unwrap();

        let mut read = |path: &str| {
            state.handle(req(
                "fs.read",
                json!({ "project_id": project_id, "path": path }),
            ))
        };

        let md = read("notes.md");
        assert_eq!(md["result"]["mime"], "text/markdown");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(md["result"]["content_b64"].as_str().unwrap())
            .unwrap();
        assert_eq!(decoded, b"# hi\n");
        assert_eq!(md["result"]["truncated"], false);
        assert_eq!(md["result"]["size"], 5);

        assert_eq!(read("page.html")["result"]["mime"], "text/html");
        assert_eq!(read("icon.svg")["result"]["mime"], "image/svg+xml");
        assert_eq!(read("pic.png")["result"]["mime"], "image/png");
        assert_eq!(read("plain.txt")["result"]["mime"], "text/plain");
        assert_eq!(
            read("blob.bin")["result"]["mime"],
            "application/octet-stream"
        );

        let missing = read("nope.txt");
        assert_eq!(missing["ok"], false, "{missing:?}");

        let dir_read = read("");
        assert_eq!(dir_read["ok"], false, "{dir_read:?}");
    }

    #[test]
    fn fs_read_truncates_oversized_files() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let project_id = state.projects[0].id.clone();

        let real_size = FS_READ_MAX_BYTES as usize + 4096;
        std::fs::write(repo.join("big.bin"), vec![b'a'; real_size]).unwrap();

        let res = state.handle(req(
            "fs.read",
            json!({ "project_id": project_id, "path": "big.bin" }),
        ));
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["size"], real_size as u64);
        assert_eq!(res["result"]["truncated"], true);
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(res["result"]["content_b64"].as_str().unwrap())
            .unwrap();
        assert_eq!(decoded.len(), FS_READ_MAX_BYTES as usize);
    }

    #[test]
    fn fs_read_rejects_lexical_and_symlink_escapes() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let project_id = state.projects[0].id.clone();

        // Lexical escape: caught before any filesystem access.
        let lexical = state.handle(req(
            "fs.read",
            json!({ "project_id": project_id, "path": "../../../etc/passwd" }),
        ));
        assert_eq!(lexical["ok"], false, "{lexical:?}");
        assert!(
            lexical["error"].as_str().unwrap().contains("escapes"),
            "{lexical:?}"
        );

        // Symlink leaf pointing inside the root: still refused (leaf check,
        // regardless of target).
        std::os::unix::fs::symlink(repo.join("README.md"), repo.join("inside-link")).unwrap();
        let inside_link = state.handle(req(
            "fs.read",
            json!({ "project_id": project_id, "path": "inside-link" }),
        ));
        assert_eq!(inside_link["ok"], false, "{inside_link:?}");
        assert!(
            inside_link["error"].as_str().unwrap().contains("symlink"),
            "{inside_link:?}"
        );

        // Symlinked directory pointing outside the root: canonical containment
        // catches it even though every lexical component is Normal.
        let outside = dir.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "secret\n").unwrap();
        std::os::unix::fs::symlink(&outside, repo.join("linkdir")).unwrap();
        let dir_escape = state.handle(req(
            "fs.read",
            json!({ "project_id": project_id, "path": "linkdir/secret.txt" }),
        ));
        assert_eq!(dir_escape["ok"], false, "{dir_escape:?}");
        assert!(
            dir_escape["error"].as_str().unwrap().contains("escapes"),
            "{dir_escape:?}"
        );
    }

    // --- primary-checkout surface (spec §5) -------------------------------------

    #[test]
    fn project_diff_shape_and_unknown_project() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let project_id = state.projects[0].id.clone();
        std::fs::write(repo.join("uncommitted.txt"), "dirty\n").unwrap();

        let res = state.handle(req("project.diff", json!({ "project_id": project_id })));
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["project_id"], project_id);
        assert_eq!(res["result"]["branch"], "main");
        assert!(res["result"]["path"].as_str().unwrap().contains("repo"));
        assert!(res["result"]["stat"]["files_changed"].as_u64().unwrap() >= 1);
        let files = res["result"]["files"].as_array().unwrap();
        assert!(files.iter().any(|f| f["path"] == "uncommitted.txt"));
        assert!(res["result"]["patch"]
            .as_str()
            .unwrap()
            .contains("uncommitted.txt"));

        let unknown = state.handle(req("project.diff", json!({ "project_id": "proj-99" })));
        assert_eq!(unknown["ok"], false, "{unknown:?}");
        assert_eq!(unknown["error"], "unknown project_id");
    }

    #[test]
    fn task_list_carries_a_cached_primary_changes_summary() {
        let (dir, repo) = init_repo();
        let mut state = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        );
        let project_id = state.projects[0].id.clone();
        std::fs::write(repo.join("dirty.txt"), "dirty\n").unwrap();

        let entry = |res: &Value| {
            res["result"]["primary_changes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["project_id"] == json!(project_id.clone()))
                .unwrap()
                .clone()
        };

        let list = state.handle(req("task.list", json!({})));
        let summary = entry(&list);
        assert_eq!(summary["branch"], "main");
        assert!(
            summary["files_changed"].as_u64().unwrap() >= 1,
            "{summary:?}"
        );

        // Served from cache on the next poll (identical, no per-poll git
        // churn) — same discipline as the task-stat and external-scan caches.
        let list2 = state.handle(req("task.list", json!({})));
        let summary2 = entry(&list2);
        assert_eq!(summary, summary2, "served from cache, not recomputed");
    }

    // ---- git.* (browser git GUI) -------------------------------------------

    /// Run a git command inside `dir`, asserting success (fixture plumbing).
    fn git_in_dir(dir: &std::path::Path, args: &[&str]) {
        assert!(Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap()
            .success());
    }

    /// A repo initialized on `main` but with no commits yet (unborn HEAD).
    fn init_unborn_repo() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git_in_dir(&repo, &["init", "-b", "main"]);
        git_in_dir(&repo, &["config", "user.email", "t@build.ing"]);
        git_in_dir(&repo, &["config", "user.name", "T"]);
        (dir, repo)
    }

    fn git_gui_state(dir: &tempfile::TempDir, repo: &std::path::Path) -> AppState {
        AppState::new(
            repo.to_path_buf(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
    }

    /// The `files` entry for `path` in a git.status-shaped payload.
    fn file_entry<'a>(status: &'a Value, path: &str) -> &'a Value {
        status["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == json!(path))
            .unwrap_or_else(|| panic!("no {path} in {status:?}"))
    }

    fn has_file_entry(status: &Value, path: &str) -> bool {
        status["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == json!(path))
    }

    #[test]
    fn git_rpcs_require_exactly_one_scope() {
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        let neither = state.handle(req("git.status", json!({})));
        assert_eq!(neither["ok"], false);
        assert_eq!(
            neither["error"],
            "provide exactly one of project_id or task_id"
        );

        let both = state.handle(req(
            "git.status",
            json!({ "project_id": project_id, "task_id": "task-1" }),
        ));
        assert_eq!(both["ok"], false);
        assert_eq!(
            both["error"],
            "provide exactly one of project_id or task_id"
        );

        let unknown_project = state.handle(req("git.log", json!({ "project_id": "proj-99" })));
        assert_eq!(unknown_project["ok"], false);
        assert_eq!(unknown_project["error"], "unknown project_id");

        let unknown_task = state.handle(req("git.log", json!({ "task_id": "task-99" })));
        assert_eq!(unknown_task["ok"], false);
        assert_eq!(unknown_task["error"], "unknown task_id");
    }

    #[test]
    fn git_log_pages_newest_first() {
        let (dir, repo) = init_repo();
        std::fs::write(repo.join("a.txt"), "a\n").unwrap();
        git_in_dir(&repo, &["add", "a.txt"]);
        git_in_dir(&repo, &["commit", "-m", "second"]);
        std::fs::write(repo.join("b.txt"), "b\n").unwrap();
        git_in_dir(&repo, &["add", "b.txt"]);
        git_in_dir(&repo, &["commit", "-m", "third"]);
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        // Default page: everything, newest first, no more pages.
        let res = state.handle(req("git.log", json!({ "project_id": project_id })));
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["branch"], "main");
        assert_eq!(res["result"]["more"], false);
        let commits = res["result"]["commits"].as_array().unwrap();
        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0]["subject"], "third");
        assert_eq!(commits[1]["subject"], "second");
        assert_eq!(commits[2]["subject"], "initial");
        let hash = commits[0]["hash"].as_str().unwrap();
        assert_eq!(hash.len(), 40);
        assert_eq!(commits[0]["short"], hash[..7]);
        assert_eq!(commits[0]["author"], "T");
        assert_eq!(commits[0]["email"], "t@build.ing");
        assert!(commits[0]["time"].as_i64().unwrap() > 0);
        // Project scope never carries the task-only ahead marker.
        assert!(commits[0].get("ahead_of_base").is_none());

        // limit pages, and `more` says another page exists.
        let page = state.handle(req(
            "git.log",
            json!({ "project_id": project_id, "limit": 2 }),
        ));
        assert_eq!(page["result"]["commits"].as_array().unwrap().len(), 2);
        assert_eq!(page["result"]["more"], true);

        // skip advances into the tail page.
        let tail = state.handle(req(
            "git.log",
            json!({ "project_id": project_id, "limit": 2, "skip": 2 }),
        ));
        let tail_commits = tail["result"]["commits"].as_array().unwrap();
        assert_eq!(tail_commits.len(), 1);
        assert_eq!(tail_commits[0]["subject"], "initial");
        assert_eq!(tail["result"]["more"], false);

        // limit clamps into 1..=200 rather than erroring.
        let clamped = state.handle(req(
            "git.log",
            json!({ "project_id": project_id, "limit": 0 }),
        ));
        assert_eq!(clamped["result"]["commits"].as_array().unwrap().len(), 1);
        assert_eq!(clamped["result"]["more"], true);
    }

    #[test]
    fn git_log_on_an_unborn_head_reports_the_branch_and_no_commits() {
        let (dir, repo) = init_unborn_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        let res = state.handle(req("git.log", json!({ "project_id": project_id })));
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["branch"], "main");
        assert_eq!(res["result"]["commits"].as_array().unwrap().len(), 0);
        assert_eq!(res["result"]["more"], false);
    }

    #[test]
    fn git_log_marks_task_commits_ahead_of_base() {
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let res = state.handle(req(
            "task.dispatch",
            json!({ "goal": "quick change", "kind": "quick" }),
        ));
        let task_id = res["result"]["task_id"].as_str().unwrap().to_string();
        let worktree = state.tasks.get(&task_id).unwrap().worktree.path.clone();

        // A commit on the task branch that main cannot reach.
        std::fs::write(worktree.join("ahead.txt"), "ahead\n").unwrap();
        git_in_dir(&worktree, &["add", "ahead.txt"]);
        git_in_dir(&worktree, &["commit", "-m", "ahead work"]);

        let log = state.handle(req("git.log", json!({ "task_id": task_id })));
        assert_eq!(log["ok"], true, "{log:?}");
        let commits = log["result"]["commits"].as_array().unwrap();
        let ahead = commits
            .iter()
            .find(|c| c["subject"] == "ahead work")
            .unwrap();
        assert_eq!(ahead["ahead_of_base"], true);
        let base = commits.iter().find(|c| c["subject"] == "initial").unwrap();
        assert_eq!(base["ahead_of_base"], false);
    }

    #[test]
    fn git_show_shapes_a_commit_and_its_root_parent() {
        let (dir, repo) = init_repo();
        std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
        git_in_dir(&repo, &["add", "a.txt"]);
        git_in_dir(&repo, &["commit", "-m", "subject line", "-m", "body text"]);
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        let log = state.handle(req("git.log", json!({ "project_id": project_id })));
        let commits = log["result"]["commits"].as_array().unwrap().clone();
        let top_hash = commits[0]["hash"].as_str().unwrap().to_string();
        let root_hash = commits[1]["hash"].as_str().unwrap().to_string();

        let shown = state.handle(req(
            "git.show",
            json!({ "project_id": project_id, "hash": top_hash }),
        ));
        assert_eq!(shown["ok"], true, "{shown:?}");
        assert_eq!(shown["result"]["hash"], top_hash.as_str());
        assert_eq!(shown["result"]["short"], top_hash[..7]);
        assert_eq!(shown["result"]["subject"], "subject line");
        assert_eq!(shown["result"]["body"], "body text");
        assert_eq!(shown["result"]["author"], "T");
        assert_eq!(shown["result"]["email"], "t@build.ing");
        assert!(shown["result"]["time"].as_i64().unwrap() > 0);
        assert_eq!(shown["result"]["stat"]["files_changed"], 1);
        assert_eq!(shown["result"]["stat"]["insertions"], 1);
        assert_eq!(shown["result"]["stat"]["deletions"], 0);
        assert!(shown["result"]["patch"]
            .as_str()
            .unwrap()
            .contains("+hello"));
        assert_eq!(shown["result"]["truncated"], false);

        // A short (7-char) prefix resolves to the same commit.
        let by_prefix = state.handle(req(
            "git.show",
            json!({ "project_id": project_id, "hash": top_hash[..7] }),
        ));
        assert_eq!(by_prefix["result"]["hash"], top_hash.as_str());

        // The root commit diffs against the empty tree.
        let root = state.handle(req(
            "git.show",
            json!({ "project_id": project_id, "hash": root_hash }),
        ));
        assert_eq!(root["ok"], true, "{root:?}");
        assert!(root["result"]["patch"]
            .as_str()
            .unwrap()
            .contains("+# project"));
    }

    #[test]
    fn git_show_rejects_malformed_and_unknown_hashes() {
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        for bad in ["HEAD", "abc", "ABCDEF12", "main", "deadbeef^", ""] {
            let res = state.handle(req(
                "git.show",
                json!({ "project_id": project_id, "hash": bad }),
            ));
            assert_eq!(res["ok"], false, "hash {bad:?} must be rejected: {res:?}");
        }

        let unknown = state.handle(req(
            "git.show",
            json!({ "project_id": project_id, "hash": "ffffffffff" }),
        ));
        assert_eq!(unknown["ok"], false, "{unknown:?}");
    }

    #[test]
    fn git_show_truncates_an_oversized_patch() {
        let (dir, repo) = init_repo();
        let line_count = 80_000; // ~1.36 MiB of "+…" patch lines, over the 1 MiB cap
        let big: String = "0123456789abcdef\n".repeat(line_count);
        std::fs::write(repo.join("big.txt"), &big).unwrap();
        git_in_dir(&repo, &["add", "big.txt"]);
        git_in_dir(&repo, &["commit", "-m", "big"]);
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        let log = state.handle(req(
            "git.log",
            json!({ "project_id": project_id, "limit": 1 }),
        ));
        let hash = log["result"]["commits"][0]["hash"]
            .as_str()
            .unwrap()
            .to_string();

        let shown = state.handle(req(
            "git.show",
            json!({ "project_id": project_id, "hash": hash }),
        ));
        assert_eq!(shown["ok"], true, "{shown:?}");
        assert_eq!(shown["result"]["truncated"], true);
        assert!(shown["result"]["patch"].as_str().unwrap().len() <= 1_048_576);
        // The stat stays exact even though the patch degraded.
        assert_eq!(shown["result"]["stat"]["insertions"], line_count);
    }

    #[test]
    fn git_status_reports_tristate_staging_and_excludes_the_mcp_config() {
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        // full: a new file, staged, with no further worktree edits.
        std::fs::write(repo.join("full.txt"), "staged\n").unwrap();
        git_in_dir(&repo, &["add", "full.txt"]);
        // partial: staged edits AND later worktree edits on the same path.
        std::fs::write(repo.join("README.md"), "# project\nstaged edit\n").unwrap();
        git_in_dir(&repo, &["add", "README.md"]);
        std::fs::write(
            repo.join("README.md"),
            "# project\nstaged edit\nunstaged edit\n",
        )
        .unwrap();
        // none: untracked.
        std::fs::write(repo.join("loose.txt"), "loose\n").unwrap();
        // Machine-local scaffolding never surfaces.
        std::fs::create_dir_all(repo.join(".build")).unwrap();
        std::fs::write(repo.join(".build/mcp.json"), "{}\n").unwrap();

        let res = state.handle(req("git.status", json!({ "project_id": project_id })));
        assert_eq!(res["ok"], true, "{res:?}");
        let status = &res["result"];
        assert_eq!(status["branch"], "main");
        assert!(status["path"].as_str().unwrap().contains("repo"));
        assert_eq!(status["head"].as_str().unwrap().len(), 40);

        let full = file_entry(status, "full.txt");
        assert_eq!(full["staged"], "full");
        assert_eq!(full["index_status"], "A");
        let partial = file_entry(status, "README.md");
        assert_eq!(partial["staged"], "partial");
        assert_eq!(partial["index_status"], "M");
        assert_eq!(partial["worktree_status"], "M");
        let untracked = file_entry(status, "loose.txt");
        assert_eq!(untracked["staged"], "none");
        assert_eq!(untracked["index_status"], "?");
        assert_eq!(untracked["worktree_status"], "?");
        assert!(!has_file_entry(status, ".build/mcp.json"));

        assert_eq!(status["stat"]["files_changed"], 3);
        let patch = status["patch"].as_str().unwrap();
        assert!(patch.contains("+loose"));
        assert!(!patch.contains("mcp.json"));
        assert_eq!(status["truncated"], false);
    }

    #[test]
    fn git_status_surfaces_merge_conflicts_as_u_entries() {
        let (dir, repo) = init_repo();
        // A real content conflict: two branches editing the same line.
        git_in_dir(&repo, &["checkout", "-q", "-b", "side"]);
        std::fs::write(repo.join("README.md"), "# side\n").unwrap();
        git_in_dir(&repo, &["add", "README.md"]);
        git_in_dir(&repo, &["commit", "-q", "-m", "side edit"]);
        git_in_dir(&repo, &["checkout", "-q", "main"]);
        std::fs::write(repo.join("README.md"), "# main\n").unwrap();
        git_in_dir(&repo, &["add", "README.md"]);
        git_in_dir(&repo, &["commit", "-q", "-m", "main edit"]);
        let merge = Command::new("git")
            .args(["merge", "side"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(!merge.status.success(), "merge must conflict");
        // A clean untracked file must still classify as before.
        std::fs::write(repo.join("loose.txt"), "loose\n").unwrap();

        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();
        let res = state.handle(req("git.status", json!({ "project_id": project_id })));
        assert_eq!(res["ok"], true, "{res:?}");

        let conflicted = file_entry(&res["result"], "README.md");
        assert_eq!(conflicted["index_status"], "U");
        assert_eq!(conflicted["worktree_status"], "U");
        assert_eq!(conflicted["staged"], "none");

        let untracked = file_entry(&res["result"], "loose.txt");
        assert_eq!(untracked["staged"], "none");
        assert_eq!(untracked["index_status"], "?");
        assert_eq!(untracked["worktree_status"], "?");
    }

    #[test]
    fn git_status_on_an_unborn_head_has_a_null_head() {
        let (dir, repo) = init_unborn_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();
        std::fs::write(repo.join("first.txt"), "hello\n").unwrap();

        let res = state.handle(req("git.status", json!({ "project_id": project_id })));
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["branch"], "main");
        assert!(res["result"]["head"].is_null());
        let untracked = file_entry(&res["result"], "first.txt");
        assert_eq!(untracked["staged"], "none");
        assert_eq!(untracked["index_status"], "?");
        assert!(res["result"]["patch"].as_str().unwrap().contains("+hello"));
    }

    #[test]
    fn git_stage_and_unstage_round_trip_through_status() {
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();
        std::fs::write(repo.join("work.txt"), "work\n").unwrap();

        let staged = state.handle(req(
            "git.stage",
            json!({ "project_id": project_id, "paths": ["work.txt"] }),
        ));
        assert_eq!(staged["ok"], true, "{staged:?}");
        // The response IS the fresh status payload.
        let entry = file_entry(&staged["result"], "work.txt");
        assert_eq!(entry["staged"], "full");
        assert_eq!(entry["index_status"], "A");

        let unstaged = state.handle(req(
            "git.unstage",
            json!({ "project_id": project_id, "paths": ["work.txt"] }),
        ));
        assert_eq!(unstaged["ok"], true, "{unstaged:?}");
        let entry = file_entry(&unstaged["result"], "work.txt");
        assert_eq!(entry["staged"], "none");
        assert_eq!(entry["index_status"], "?");
    }

    #[test]
    fn git_stage_rejects_paths_that_escape_the_worktree() {
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();
        std::fs::write(repo.join("ok.txt"), "ok\n").unwrap();

        for bad in ["../../../etc/passwd", "/etc/passwd", "./ok.txt", ""] {
            let res = state.handle(req(
                "git.stage",
                json!({ "project_id": project_id, "paths": ["ok.txt", bad] }),
            ));
            assert_eq!(res["ok"], false, "path {bad:?} must be rejected: {res:?}");
        }
        // One bad path failed the whole request: nothing got staged.
        let status = state.handle(req("git.status", json!({ "project_id": project_id })));
        assert_eq!(file_entry(&status["result"], "ok.txt")["staged"], "none");

        // paths is required and must be a non-empty array of strings.
        let missing = state.handle(req("git.stage", json!({ "project_id": project_id })));
        assert_eq!(missing["ok"], false);
        let empty = state.handle(req(
            "git.stage",
            json!({ "project_id": project_id, "paths": [] }),
        ));
        assert_eq!(empty["ok"], false);
    }

    #[test]
    fn git_stage_silently_drops_the_mcp_config() {
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();
        std::fs::create_dir_all(repo.join(".build")).unwrap();
        std::fs::write(repo.join(".build/mcp.json"), "{}\n").unwrap();

        // The list collapses to empty → a successful no-op.
        let res = state.handle(req(
            "git.stage",
            json!({ "project_id": project_id, "paths": [".build/mcp.json"] }),
        ));
        assert_eq!(res["ok"], true, "{res:?}");
        assert!(!has_file_entry(&res["result"], ".build/mcp.json"));
    }

    #[test]
    fn git_unstage_works_on_an_unborn_head() {
        let (dir, repo) = init_unborn_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();
        std::fs::write(repo.join("first.txt"), "hello\n").unwrap();
        let staged = state.handle(req(
            "git.stage",
            json!({ "project_id": project_id, "paths": ["first.txt"] }),
        ));
        assert_eq!(file_entry(&staged["result"], "first.txt")["staged"], "full");

        // No HEAD to reset to — the index entry is dropped instead.
        let unstaged = state.handle(req(
            "git.unstage",
            json!({ "project_id": project_id, "paths": ["first.txt"] }),
        ));
        assert_eq!(unstaged["ok"], true, "{unstaged:?}");
        let entry = file_entry(&unstaged["result"], "first.txt");
        assert_eq!(entry["staged"], "none");
        assert_eq!(entry["index_status"], "?");
    }

    #[test]
    fn git_unstage_on_an_unborn_head_survives_a_post_stage_edit() {
        let (dir, repo) = init_unborn_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();
        std::fs::write(repo.join("first.txt"), "v1\n").unwrap();
        state.handle(req(
            "git.stage",
            json!({ "project_id": project_id, "paths": ["first.txt"] }),
        ));
        // Edit after staging: the staged copy now differs from the worktree
        // copy, which `git rm --cached` refuses without -f.
        std::fs::write(repo.join("first.txt"), "v2\n").unwrap();

        let unstaged = state.handle(req(
            "git.unstage",
            json!({ "project_id": project_id, "paths": ["first.txt"] }),
        ));
        assert_eq!(unstaged["ok"], true, "{unstaged:?}");
        let entry = file_entry(&unstaged["result"], "first.txt");
        assert_eq!(entry["staged"], "none");
        assert_eq!(entry["index_status"], "?");
        // --cached never touches the worktree file: the edit survives.
        assert_eq!(
            std::fs::read_to_string(repo.join("first.txt")).unwrap(),
            "v2\n"
        );
    }

    #[test]
    fn git_commit_commits_only_what_is_staged() {
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();
        std::fs::write(repo.join("staged.txt"), "staged\n").unwrap();
        std::fs::write(repo.join("README.md"), "# project\nunstaged edit\n").unwrap();
        state.handle(req(
            "git.stage",
            json!({ "project_id": project_id, "paths": ["staged.txt"] }),
        ));

        let res = state.handle(req(
            "git.commit",
            json!({ "project_id": project_id, "message": "add staged file" }),
        ));
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["hash"].as_str().unwrap().len(), 40);
        assert_eq!(res["result"]["subject"], "add staged file");
        let hash = res["result"]["hash"].as_str().unwrap();
        assert_eq!(res["result"]["short"], hash[..7]);

        // The unstaged edit survived, uncommitted; the staged file is gone
        // from status.
        let status = &res["result"]["status"];
        assert!(!has_file_entry(status, "staged.txt"), "{status:?}");
        assert_eq!(file_entry(status, "README.md")["staged"], "none");
        assert_eq!(status["head"], json!(hash));

        // And the commit is on top of the log.
        let log = state.handle(req(
            "git.log",
            json!({ "project_id": project_id, "limit": 1 }),
        ));
        assert_eq!(log["result"]["commits"][0]["subject"], "add staged file");
    }

    #[test]
    fn git_commit_rejects_empty_messages_and_an_empty_stage() {
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();
        std::fs::write(repo.join("staged.txt"), "staged\n").unwrap();
        state.handle(req(
            "git.stage",
            json!({ "project_id": project_id, "paths": ["staged.txt"] }),
        ));

        for empty in ["", "   \n\t"] {
            let res = state.handle(req(
                "git.commit",
                json!({ "project_id": project_id, "message": empty }),
            ));
            assert_eq!(res["ok"], false, "{res:?}");
            assert_eq!(res["error"], "commit message must not be empty");
        }

        // Drain the stage, then a commit has nothing to do.
        state.handle(req(
            "git.unstage",
            json!({ "project_id": project_id, "paths": ["staged.txt"] }),
        ));
        let nothing = state.handle(req(
            "git.commit",
            json!({ "project_id": project_id, "message": "msg" }),
        ));
        assert_eq!(nothing["ok"], false, "{nothing:?}");
        assert_eq!(nothing["error"], "nothing staged to commit");
    }

    #[test]
    fn git_commit_on_a_task_scope_refreshes_the_diffstat_cache() {
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let res = state.handle(req(
            "task.dispatch",
            json!({ "goal": "quick change", "kind": "quick" }),
        ));
        let task_id = res["result"]["task_id"].as_str().unwrap().to_string();
        let worktree = state.tasks.get(&task_id).unwrap().worktree.path.clone();

        let entry = |res: &Value| {
            res["result"]["tasks"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["task_id"] == json!(task_id.clone()))
                .unwrap()
                .clone()
        };

        // Warm the diffstat cache.
        let before = entry(&state.handle(req("task.list", json!({}))));
        let before_changed = before["stat"]["files_changed"].as_u64().unwrap();

        // Stage and commit two new files through the git GUI verbs.
        std::fs::write(worktree.join("one.txt"), "one\n").unwrap();
        std::fs::write(worktree.join("two.txt"), "two\n").unwrap();
        state.handle(req(
            "git.stage",
            json!({ "task_id": task_id, "paths": ["one.txt", "two.txt"] }),
        ));
        let committed = state.handle(req(
            "git.commit",
            json!({ "task_id": task_id, "message": "user commit" }),
        ));
        assert_eq!(committed["ok"], true, "{committed:?}");

        // The cached stat was dropped, so the very next poll sees the commit
        // (TASK_STAT_TTL alone would have served the stale stat for 10s).
        let after = entry(&state.handle(req("task.list", json!({}))));
        let after_changed = after["stat"]["files_changed"].as_u64().unwrap();
        assert_eq!(after_changed, before_changed + 2, "{after:?}");
    }
}
