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
    ActivePlan, ActiveRun, Agent, Orchestrator, OrchestratorError, RunSource, SpawnOptions,
    TranscriptProbe,
};
use crate::plan::StageManifestEntry;
use crate::plan::{
    CommentAnchor, CommentState, PlanEvent, PlanId, PlanState, StageComment, StageDoc,
    StageDocState,
};
use crate::pty::{HarnessSpec, PtySession};
use crate::relay::{FrameHandler, SessionSender};
use crate::run::ValidationReport;
use crate::run::{RunEvent, RunId, RunState, StageProgress, StageProgressState};
use crate::store::{now_rfc3339, PersistedPlan, PersistedRun, Store};
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
    Run {
        run_id: String,
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
    /// Parse the inline scope params: `run_id` wins (a run is worktree-scoped),
    /// then `project_id`+`worktree_id`, then `project_id` alone.
    fn parse(params: &Value) -> Result<TermScope, String> {
        let field = |key: &str| {
            params
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        if let Some(run_id) = field("run_id") {
            return Ok(TermScope::Run { run_id });
        }
        let Some(project_id) = field("project_id") else {
            return Err("missing scope: run_id or project_id required".to_string());
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
            TermScope::Run { run_id } => {
                let active = state.runs.get(run_id).ok_or("unknown run_id")?;
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
    /// The owning entity's (`ActivePlan`/`ActiveRun`) session generation this
    /// screen's pump is consuming. 0 = no pump has ever run.
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
    /// Registered projects (repos) plans and runs can be dispatched to.
    projects: Vec<Project>,
    /// entity id (plan or run) → the project it belongs to (routes every
    /// plan/run RPC and `done`). Plan and run ids are disjoint (`plan-…` /
    /// `run-…`), so one map serves both.
    entity_project: HashMap<String, String>,
    /// entity id → its project's repo path, retained even when the project is
    /// not registered (a parked repo-missing run has no `entity_project` entry,
    /// yet its record must keep the real path so a restored repo can un-park it).
    entity_project_path: HashMap<String, String>,
    worktrees_root: std::path::PathBuf,
    /// Where cloned repos land and the directory browser starts; user-configurable.
    projects_dir: std::path::PathBuf,
    /// Where to persist the projects + settings, if persistence is enabled.
    config_path: Option<std::path::PathBuf>,
    agent: Agent,
    harness: String,
    /// Project-scoped plans, keyed by `plan_id`.
    plans: HashMap<String, ActivePlan>,
    /// Worktree-scoped runs, keyed by `run_id`.
    runs: HashMap<String, ActiveRun>,
    /// Durable plan/run records under the bridge state dir, if persistence is
    /// enabled.
    store: Option<Store>,
    /// entity id → its RFC 3339 creation time, carried across saves (and restarts).
    entity_created_at: HashMap<String, String>,
    /// entity id → its RFC 3339 last-mutation time (stamped on every mutation).
    entity_updated_at: HashMap<String, String>,
    /// run id → cached `board.list` diffstat, so the poll surface never runs
    /// per-run git work more than once per TTL window.
    run_stat_cache: HashMap<String, (std::time::Instant, Value)>,
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
            entity_project: HashMap::new(),
            entity_project_path: HashMap::new(),
            worktrees_root: worktrees_root.into(),
            projects_dir: default_projects_dir(),
            config_path: None,
            agent: build_agent(qa_agent, mcp_socket.into()),
            harness,
            plans: HashMap::new(),
            runs: HashMap::new(),
            store: None,
            entity_created_at: HashMap::new(),
            entity_updated_at: HashMap::new(),
            run_stat_cache: HashMap::new(),
            term_shell: resolve_term_shell(),
            streams: HashMap::new(),
            terms: HashMap::new(),
            agent_screens: HashMap::new(),
            next_term: 1,
            self_handle: None,
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
        let store = Store::new(dir);
        // First: migrate any legacy fused-task records into the split's plan
        // and run records (idempotent; a store with only new-format records is
        // a no-op). This must run before the loaders so recovery only ever
        // sees the split shape.
        store.migrate_legacy_tasks().map_err(|e| e.to_string())?;
        let plans = store.load_all_plans().map_err(|e| e.to_string())?;
        let runs = store.load_all_runs().map_err(|e| e.to_string())?;
        self.store = Some(store);
        // Plans first: a run re-derives its `plan_path` from the owning plan's
        // record, so the plan must already be in the map.
        for record in plans {
            self.recover_plan(record)?;
        }
        for record in runs {
            self.recover_run(record)?;
        }
        Ok(self)
    }

    /// Re-attach one persisted plan on boot. The canonical docs live in the
    /// store, so a vanished planning worktree never abandons or archives a
    /// plan — a plan that was mid-draft simply surfaces `Interrupted` (its
    /// session died with the daemon); the next revision dispatch re-creates a
    /// worktree materialized from the store. Recovery never abandons a plan.
    fn recover_plan(&mut self, record: PersistedPlan) -> Result<(), String> {
        let plan_id = record.id.clone();
        let mut active = ActivePlan::reattach(&record);

        let mut state_changed = false;
        if active.plan.state.is_working() {
            // The drafting session died with the daemon; the docs are safe in
            // the store. Surface it interrupted (persisted so the verdict
            // survives the next restart too).
            active
                .plan
                .apply(PlanEvent::Interrupt)
                .map_err(|e| format!("recover {plan_id}: {e}"))?;
            state_changed = true;
        }

        // Project ids are re-minted each boot, so resolve by repo path. Retain
        // the record's path unconditionally so a parked repo-missing plan keeps
        // a real path to un-park to.
        self.entity_project_path
            .insert(plan_id.clone(), record.project_path.clone());
        let repo_path = std::path::PathBuf::from(&record.project_path);
        if repo_path.exists() {
            let project_id = self.add_project(repo_path, record.base_branch);
            self.entity_project.insert(plan_id.clone(), project_id);
        } else {
            // The repo is gone; the plan can't be re-dispatched, but its docs
            // remain readable from the store. Keep it legible with a reason.
            eprintln!(
                "recover {plan_id}: project repo {} is gone; plan kept (docs live in the store)",
                record.project_path
            );
            if !active.plan.state.is_terminal() {
                active.last_error =
                    Some(format!("project repo missing at {}", record.project_path));
                state_changed = true;
            }
        }

        self.entity_created_at
            .insert(plan_id.clone(), record.created_at);
        self.entity_updated_at
            .insert(plan_id.clone(), record.updated_at);
        if state_changed {
            self.persist_plan_record(&plan_id, &active)?;
        }
        self.plans.insert(plan_id, active);
        Ok(())
    }

    /// Re-attach one persisted run on boot. The worktree survived on disk (or
    /// it did not); the PTY session died with the previous daemon. A working
    /// state becomes `Interrupted`; a non-terminal run whose worktree vanished
    /// is `Abandoned` (worktree gone, branch kept — exactly what `Abandoned`
    /// means); the repo-missing arms park an adopted run needs-attention and
    /// abandon a native one, both with a reason.
    fn recover_run(&mut self, record: PersistedRun) -> Result<(), String> {
        let run_id = record.id.clone();
        // Re-derive `plan_path` from the owning plan (recovered first); quick
        // runs and orphaned links fall back to the convention default.
        let plan_path = record
            .plan_id
            .as_ref()
            .and_then(|pid| self.plans.get(pid))
            .map(|plan| plan.plan_path.clone())
            .unwrap_or_else(|| crate::templates::DEFAULT_PLAN_PATH.to_string());
        let mut active = ActiveRun::reattach(&record, plan_path);

        let mut state_changed = false;
        if !active.run.state.is_terminal() {
            if !active.worktree.path.exists() {
                eprintln!(
                    "recover {run_id}: worktree {} is gone; abandoning (branch kept)",
                    active.worktree.path.display()
                );
                active
                    .run
                    .apply(RunEvent::Abandon)
                    .map_err(|e| format!("recover {run_id}: {e}"))?;
                state_changed = true;
            } else if active.run.state.is_working() {
                active
                    .run
                    .apply(RunEvent::Interrupt)
                    .map_err(|e| format!("recover {run_id}: {e}"))?;
                state_changed = true;
            }
        }

        self.entity_project_path
            .insert(run_id.clone(), record.project_path.clone());
        let repo_path = std::path::PathBuf::from(&record.project_path);
        if repo_path.exists() {
            let project_id = self.add_project(repo_path, record.base_branch);
            self.entity_project.insert(run_id.clone(), project_id);
        } else if !active.run.state.is_terminal() {
            if active.adopted {
                // Automated actions never touch an adopted worktree: park the
                // run needs-attention instead of abandoning.
                eprintln!(
                    "recover {run_id}: project repo {} is gone; parking adopted run",
                    record.project_path
                );
                if active.run.state.is_working() {
                    active
                        .run
                        .apply(RunEvent::Interrupt)
                        .map_err(|e| format!("recover {run_id}: {e}"))?;
                }
                active.last_error =
                    Some(format!("project repo missing at {}", record.project_path));
                state_changed = true;
            } else {
                // No project to route to and no repo to advance on: abandon it
                // so it stays legible with a reason rather than an orphan.
                eprintln!(
                    "recover {run_id}: project repo {} is gone; abandoning",
                    record.project_path
                );
                active
                    .run
                    .apply(RunEvent::Abandon)
                    .map_err(|e| format!("recover {run_id}: {e}"))?;
                active.last_error =
                    Some(format!("project repo missing at {}", record.project_path));
                state_changed = true;
            }
        } else {
            eprintln!(
                "recover {run_id}: project repo {} is gone; run kept as history",
                record.project_path
            );
        }

        self.entity_created_at
            .insert(run_id.clone(), record.created_at);
        self.entity_updated_at
            .insert(run_id.clone(), record.updated_at);
        if state_changed {
            self.persist_run_record(&run_id, &active)?;
        }
        self.runs.insert(run_id, active);
        Ok(())
    }

    /// The project repo path to stamp on a persisted record: the live project's
    /// canonical path if it is registered, else the retained record path (a
    /// parked repo-missing entity), else empty.
    fn project_path_for(&self, entity_id: &str) -> String {
        self.entity_project
            .get(entity_id)
            .and_then(|pid| self.projects.iter().find(|p| &p.id == pid))
            .map(|p| p.repo_path.display().to_string())
            .or_else(|| self.entity_project_path.get(entity_id).cloned())
            .unwrap_or_default()
    }

    /// Write a plan's durable core to the store (atomic replace). A no-op
    /// without a configured store (unit tests); an error surfaces to the caller
    /// — a plan the store cannot hold would silently vanish on the next
    /// restart. Docs are NOT snapshotted here: the store copy is canonical and
    /// written transactionally at each plan/revise `done`.
    fn persist_plan_record(&mut self, plan_id: &str, active: &ActivePlan) -> Result<(), String> {
        if self.store.is_none() {
            return Ok(());
        }
        let now = now_rfc3339();
        let created_at = self
            .entity_created_at
            .entry(plan_id.to_string())
            .or_insert_with(|| now.clone())
            .clone();
        let updated_at = self.entity_updated_at.get(plan_id).cloned().unwrap_or(now);
        let project_path = self.project_path_for(plan_id);
        let worktree = active.worktree.as_ref();
        let record = PersistedPlan {
            id: plan_id.to_string(),
            goal: active.plan.goal.clone(),
            project_path,
            base_branch: active.base_branch.clone(),
            state: active.plan.state,
            worktree_name: worktree.map(|w| w.name.clone()),
            worktree_path: worktree.map(|w| w.path.display().to_string()),
            branch: worktree.map(|w| w.branch.clone()),
            plan_path: active.plan_path.clone(),
            stages: active.stages.clone(),
            comments: active.comments.clone(),
            model: active.model_choice.model.clone(),
            effort: active.model_choice.effort.clone(),
            last_summary: active.last_summary.clone(),
            last_error: active.last_error.clone(),
            created_at,
            updated_at,
        };
        self.store
            .as_ref()
            .expect("checked above")
            .save_plan(&record)
            .map_err(|e| format!("plan store: {e}"))
    }

    /// Write a run's durable core to the store (atomic replace). Same discipline
    /// as [`persist_plan_record`](Self::persist_plan_record); a run stores its
    /// `plan_id`, never plan docs.
    fn persist_run_record(&mut self, run_id: &str, active: &ActiveRun) -> Result<(), String> {
        if self.store.is_none() {
            return Ok(());
        }
        let now = now_rfc3339();
        let created_at = self
            .entity_created_at
            .entry(run_id.to_string())
            .or_insert_with(|| now.clone())
            .clone();
        let updated_at = self.entity_updated_at.get(run_id).cloned().unwrap_or(now);
        let project_path = self.project_path_for(run_id);
        let record = PersistedRun {
            id: run_id.to_string(),
            plan_id: active.run.plan_id.as_ref().map(|p| p.0.clone()),
            goal: active.run.goal.clone(),
            project_path,
            base_branch: active.worktree.base_branch.clone(),
            state: active.run.state,
            branch: active.worktree.branch.clone(),
            worktree_name: active.worktree.name.clone(),
            worktree_path: active.worktree.path.display().to_string(),
            base_sha: active.base_sha.clone(),
            stages: active.stages.clone(),
            current_stage_id: active.current_stage_id.clone(),
            revising_stage_id: active.revising_stage_id.clone(),
            auto_advance: active.auto_advance,
            adopted: active.adopted,
            pending_continuation: active.pending_continuation,
            model: active.model_choice.model.clone(),
            effort: active.model_choice.effort.clone(),
            last_summary: active.last_summary.clone(),
            last_error: active.last_error.clone(),
            created_at,
            updated_at,
        };
        self.store
            .as_ref()
            .expect("checked above")
            .save_run(&record)
            .map_err(|e| format!("run store: {e}"))
    }

    /// The shared tail of every plan mutation: stamp times, compute the response
    /// view, persist the durable core, throttle a notify, put the plan back in
    /// the map, and prompt terminal closure + pump start. Returns the view and
    /// the persistence outcome separately so callers can order their errors.
    fn finish_plan_mutation(
        &mut self,
        plan_id: String,
        active: ActivePlan,
    ) -> (Value, Result<(), String>) {
        let now = now_rfc3339();
        self.entity_created_at
            .entry(plan_id.clone())
            .or_insert_with(|| now.clone());
        self.entity_updated_at.insert(plan_id.clone(), now);
        let view = self.plan_view(&plan_id, &active);
        let persisted = self.persist_plan_record(&plan_id, &active);
        self.push_notify_plan(&plan_id, active.plan.state);
        self.plans.insert(plan_id, active);
        self.reap_orphaned_terminals();
        self.ensure_agent_pumps();
        (view, persisted)
    }

    /// The run-half twin of
    /// [`finish_plan_mutation`](Self::finish_plan_mutation).
    fn finish_run_mutation(
        &mut self,
        run_id: String,
        active: ActiveRun,
    ) -> (Value, Result<(), String>) {
        let now = now_rfc3339();
        self.entity_created_at
            .entry(run_id.clone())
            .or_insert_with(|| now.clone());
        self.entity_updated_at.insert(run_id.clone(), now);
        // The mutation likely changed the tree; drop the cached diffstat.
        self.run_stat_cache.remove(&run_id);
        let view = self.run_view(&run_id, &active);
        let persisted = self.persist_run_record(&run_id, &active);
        self.push_notify_run(&run_id, active.run.state);
        self.runs.insert(run_id, active);
        self.reap_orphaned_terminals();
        self.ensure_agent_pumps();
        (view, persisted)
    }

    /// Fire one content-free web-push notify when a plan-state change lands in
    /// a state that needs the human.
    fn push_notify_plan(&mut self, plan_id: &str, state: PlanState) {
        if self.notifier.is_none() || !self.notify_throttle.should_notify_plan(plan_id, &state) {
            return;
        }
        let Some(kind) = crate::notify::kind_for_plan_state(&state) else {
            return;
        };
        self.spawn_notify(plan_id.to_string(), kind);
    }

    /// The run-half twin of [`push_notify_plan`](Self::push_notify_plan).
    fn push_notify_run(&mut self, run_id: &str, state: RunState) {
        if self.notifier.is_none() || !self.notify_throttle.should_notify_run(run_id, &state) {
            return;
        }
        let Some(kind) = crate::notify::kind_for_run_state(&state) else {
            return;
        };
        self.spawn_notify(run_id.to_string(), kind);
    }

    /// Spawn the actual notify POST off the app lock. A delivery failure only
    /// logs — it never blocks the mutation.
    fn spawn_notify(&self, entity_id: String, kind: &'static str) {
        let Some(notifier) = &self.notifier else {
            return;
        };
        let notifier = notifier.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    if let Err(e) = notifier.notify(&entity_id, kind).await {
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

    /// Canonical paths of every Build-bound worktree — every run plus every live
    /// planning worktree: they are Build's, never external. `fs::canonicalize`
    /// with the raw path as fallback.
    fn bound_worktree_paths(&self) -> std::collections::HashSet<std::path::PathBuf> {
        let canonical = |path: &std::path::Path| {
            std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
        };
        // Run worktrees are Build's. Disposable *planning* worktrees join the
        // set too, so a live planning worktree never surfaces as an adoptable
        // external card while a plan is being authored.
        self.runs
            .values()
            .map(|active| canonical(&active.worktree.path))
            .chain(
                self.plans
                    .values()
                    .filter_map(|active| active.worktree.as_ref())
                    .map(|w| canonical(&w.path)),
            )
            .collect()
    }

    /// Whether an id (plan or run) names a live entity in either map.
    fn contains_entity(&self, entity_id: &str) -> bool {
        self.plans.contains_key(entity_id) || self.runs.contains_key(entity_id)
    }

    /// Subscribe to an entity's warm agent session (plan or run), with its
    /// generation — the entity-agnostic shim the agent-screen machinery rides.
    fn entity_subscribe_with_generation(
        &self,
        entity_id: &str,
    ) -> Option<(u64, broadcast::Receiver<Vec<u8>>)> {
        if let Some(plan) = self.plans.get(entity_id) {
            return plan.session.subscribe_with_generation();
        }
        if let Some(run) = self.runs.get(entity_id) {
            return run.session.subscribe_with_generation();
        }
        None
    }

    /// Write bytes into an entity's live agent PTY (plan or run).
    fn entity_write_input(&self, entity_id: &str, data: &[u8]) -> Result<(), String> {
        if let Some(plan) = self.plans.get(entity_id) {
            return plan.session.write_input_strict(data);
        }
        if let Some(run) = self.runs.get(entity_id) {
            return run.session.write_input_strict(data);
        }
        Err("unknown term_id".to_string())
    }

    /// Resize an entity's live agent PTY (plan or run); returns whether a live
    /// session was resized.
    fn entity_resize_session(
        &self,
        entity_id: &str,
        size: PtySize,
    ) -> Result<bool, OrchestratorError> {
        if let Some(plan) = self.plans.get(entity_id) {
            return plan.session.resize(size);
        }
        if let Some(run) = self.runs.get(entity_id) {
            return run.session.resize(size);
        }
        Ok(false)
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
                        // The MCP CLI stays `mcp --task <id>` (opaque): the
                        // `task_id` field carries whatever id — plan or run —
                        // owns this session; the daemon routes by owner lookup.
                        let entity_id = v.get("task_id").and_then(Value::as_str).unwrap_or("");
                        if let Ok(report) = serde_json::from_value::<DoneReport>(
                            v.get("report").cloned().unwrap_or(Value::Null),
                        ) {
                            state.lock().unwrap().on_agent_done(entity_id, report);
                        }
                    }
                });
            }
        });
    }

    /// Route an agent's `done` to its owner's lifecycle transition, by owner
    /// lookup (plans map, then runs map).
    fn on_agent_done(&mut self, entity_id: &str, report: DoneReport) {
        if self.plans.contains_key(entity_id) {
            self.on_plan_agent_done(entity_id, report);
        } else if self.runs.contains_key(entity_id) {
            self.on_run_agent_done(entity_id, report);
        } else {
            eprintln!("on_agent_done: unknown entity {entity_id}");
        }
    }

    /// A plan agent reported `done`: ingest + advance on the plan's orchestrator.
    fn on_plan_agent_done(&mut self, plan_id: &str, report: DoneReport) {
        let Some(mut active) = self.plans.remove(plan_id) else {
            return;
        };
        let outcome = (|| -> Result<(), String> {
            let project_id = self.project_of(plan_id)?;
            let store = self.require_store()?;
            self.orch_for(&project_id)?
                .on_plan_done(&mut active, store, report)
                .map_err(err)
        })();
        if let Err(e) = outcome {
            eprintln!("on_agent_done {plan_id}: {e}");
        }
        let (_, persisted) = self.finish_plan_mutation(plan_id.to_string(), active);
        if let Err(e) = persisted {
            eprintln!("on_agent_done {plan_id}: {e}");
        }
    }

    /// A run agent reported `done`. A mid-run stage-doc revision (revising_stage_id
    /// set) is a cross-entity store write-back to the owning plan; every other
    /// report advances the run on `on_run_done`, and a validation pass may then
    /// auto-advance the next approved stage when run-all is armed.
    fn on_run_agent_done(&mut self, run_id: &str, report: DoneReport) {
        let Some(mut active) = self.runs.remove(run_id) else {
            return;
        };
        let is_stage_revision = active.revising_stage_id.is_some()
            && report.phase == DonePhase::Revise
            && report.status == DoneStatus::Completed;
        if is_stage_revision {
            self.consume_run_stage_revision(run_id, active, report);
            return;
        }
        let plan_docs = self.owning_plan_stage_docs(&active);
        let outcome = (|| -> Result<(), String> {
            let project_id = self.project_of(run_id)?;
            self.orch_for(&project_id)?
                .on_run_done(&mut active, &plan_docs, report)
                .map_err(err)
        })();
        if let Err(e) = outcome {
            eprintln!("on_agent_done {run_id}: {e}");
        }
        let (_, persisted) = self.finish_run_mutation(run_id.to_string(), active);
        if let Err(e) = persisted {
            eprintln!("on_agent_done {run_id}: {e}");
        }
        self.auto_advance_run(run_id);
    }

    /// Consume a mid-run stage-doc revision's `done`: it writes the revised docs
    /// back to the owning plan's canonical store copy, so both the run (whose
    /// worktree holds the docs) and the plan (whose doc state + comments update)
    /// are mutated, then both re-persisted.
    fn consume_run_stage_revision(
        &mut self,
        run_id: &str,
        mut active: ActiveRun,
        report: DoneReport,
    ) {
        let plan_id = active.run.plan_id.as_ref().map(|p| p.0.clone());
        let mut plan = plan_id.as_ref().and_then(|pid| self.plans.remove(pid));
        let outcome = (|| -> Result<(), String> {
            let plan = plan
                .as_mut()
                .ok_or_else(|| "run stage revision: owning plan is gone".to_string())?;
            let project_id = self.project_of(run_id)?;
            let store = self.require_store()?;
            self.orch_for(&project_id)?
                .consume_run_stage_revision(&mut active, plan, store, &report)
                .map_err(err)
        })();
        if let Err(e) = outcome {
            eprintln!("on_agent_done {run_id}: {e}");
        }
        if let (Some(pid), Some(plan)) = (plan_id, plan) {
            let (_, persisted) = self.finish_plan_mutation(pid, plan);
            if let Err(e) = persisted {
                eprintln!("on_agent_done {run_id}: plan persist: {e}");
            }
        }
        let (_, persisted) = self.finish_run_mutation(run_id.to_string(), active);
        if let Err(e) = persisted {
            eprintln!("on_agent_done {run_id}: {e}");
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
            "git.fetch" => self.git_fetch(params),
            "git.pull" => self.git_pull(params),
            "git.push" => self.git_push(params),
            "git.branches" => self.git_branches(params),
            "git.checkout" => self.git_checkout(params),
            "git.branch_delete" => self.git_branch_delete(params),
            "git.stash" => self.git_stash(params),
            "git.stash_pop" => self.git_stash_pop(params),
            "git.discard" => self.git_discard(params),
            "git.merge_abort" => self.git_merge_abort(params),
            "settings.get" => Ok(self.settings_get()),
            "settings.set" => self.settings_set(params),
            "project.list" => Ok(self.project_list()),
            "project.add" => self.project_add(params),
            "project.create" => self.project_create(params),
            "project.clone" => self.project_clone(params),
            "project.set_remote" => self.project_set_remote(params),
            "board.list" => Ok(self.board_list()),
            // Plan surface (project-scoped): keyed by plan_id, docs from store.
            "plan.create" => self.plan_create(params),
            "plan.get" => self.plan_get(params),
            "plan.list" => Ok(self.plan_list()),
            "plan.doc" => self.plan_doc(params),
            "plan.stages" => self.plan_stages(params),
            "plan.stage_doc" => self.plan_stage_doc(params),
            "plan.approve" => self.plan_approve(params),
            "plan.send_notes" => self.plan_send_notes(params),
            "plan.stage_approve" => self.plan_stage_approve(params),
            "plan.stage_send_notes" => self.plan_stage_send_notes(params),
            "plan.comment_add" => self.plan_comment_add(params),
            "plan.comment_delete" => self.plan_comment_delete(params),
            "plan.message" => self.plan_message(params),
            "plan.abandon" => self.plan_abandon(params),
            "plan.delete" => self.plan_delete(params),
            // Run surface (worktree-scoped): keyed by run_id.
            "run.create" => self.run_create(params),
            "run.get" => self.run_get(params),
            "run.diff" => self.run_diff(params),
            "run.request_changes" => self.run_request_changes(params),
            "run.stage_dispatch" => self.run_stage_dispatch(params),
            "run.stage_fix" => self.run_stage_fix(params),
            "run.set_auto_advance" => self.run_set_auto_advance(params),
            "run.git_action" => self.run_git_action(params),
            "run.message" => self.run_message(params),
            "run.abandon" => self.run_abandon(params),
            "run.delete" => self.run_delete(params),
            "run.adopt" => self.run_adopt(params),
            "run.release" => self.run_release(params),
            "worktree.diff" => self.worktree_diff(params),
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
        if let Some(entity_id) = term_id.strip_prefix("agent:") {
            self.entity_write_input(entity_id, &data)?;
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
        if let Some(entity_id) = term_id.strip_prefix("agent:") {
            if !self.contains_entity(entity_id) {
                return Err("unknown term_id".to_string());
            }
            let live = self.entity_resize_session(entity_id, size).map_err(err)?;
            if live {
                // Keep the retained agent screen in step with the live PTY; a
                // dead resize touches nothing (the last screen stays intact).
                if let Some(agent) = self.agent_screens.get_mut(entity_id) {
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
        // Retained agent screens live exactly as long as their owning entity.
        let AppState {
            agent_screens,
            plans,
            runs,
            ..
        } = self;
        agent_screens
            .retain(|entity_id, _| plans.contains_key(entity_id) || runs.contains_key(entity_id));
        orphaned
    }

    /// Proactively capture every task's agent PTY into a retained screen from
    /// the moment its session spawns — independent of any client attach. The
    /// server-side screen model must exist for the life of the task (spec: the
    /// Agent tab shows the running-or-last agent even for an unattended,
    /// overnight run), so an unattended session's broadcast output is never
    /// dropped for want of a subscriber. Runs at the `finish_mutation` tail via
    /// the weak self-handle; without a handle or a runtime (sync unit tests)
    /// pump spawning is skipped.
    fn ensure_agent_pumps(&mut self) {
        let Some(state_arc) = self.self_handle.as_ref().and_then(std::sync::Weak::upgrade) else {
            return;
        };
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        // Every entity (plan or run) with a live session gets a screen + a
        // running pump from session spawn — get-or-create at the agent PTY's
        // default grid (120×40, matching agent_attach). The generation guard
        // (set under the lock, before spawning) means a screen is never
        // double-pumped, so the proactive pump here and agent_attach's own
        // pump-start stay idempotent. Plan and run ids are disjoint.
        let mut live: Vec<(String, u64, broadcast::Receiver<Vec<u8>>)> = Vec::new();
        for (id, active) in self.plans.iter() {
            if let Some((generation, rx)) = active.session.subscribe_with_generation() {
                live.push((id.clone(), generation, rx));
            }
        }
        for (id, active) in self.runs.iter() {
            if let Some((generation, rx)) = active.session.subscribe_with_generation() {
                live.push((id.clone(), generation, rx));
            }
        }
        let mut pumps = Vec::new();
        for (entity_id, generation, rx) in live {
            let agent = self
                .agent_screens
                .entry(entity_id.clone())
                .or_insert_with(|| AgentScreen {
                    screen: TermScreen::new(120, 40),
                    pumped_generation: 0,
                    live: false,
                });
            if generation != agent.pumped_generation {
                agent.pumped_generation = generation;
                agent.live = true;
                pumps.push((entity_id, generation, rx));
            }
        }
        for (entity_id, generation, rx) in pumps {
            spawn_agent_pump(Arc::clone(&state_arc), entity_id, generation, rx);
        }
    }

    /// Whether a terminal's scope still maps to a live surface. The check is
    /// cheap: a map lookup and/or one `Path::exists` over ≤ 16 entries.
    fn term_scope_resolves(&self, term: &TermSession) -> bool {
        match &term.scope {
            // A merged run with cleanup=keep keeps its worktree → terminals stay.
            TermScope::Run { run_id } => self.runs.contains_key(run_id) && term.scope_root.exists(),
            // An adopted worktree's path survives adoption — its terminal lives on.
            TermScope::ExternalWorktree { .. } => term.scope_root.exists(),
            TermScope::Primary { project_id } => {
                self.projects.iter().any(|p| &p.id == project_id) && term.scope_root.exists()
            }
        }
    }

    /// Periodically close terminals whose scope vanished out-of-band (nothing
    /// went through `finish_*_mutation` — e.g. the user deleted an external
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

    /// Demote every working plan/run whose harness has crashed/exited or gone
    /// quiet without a `done` to `idle_unreported`, persisting the transition.
    /// Returns the demoted ids. The scope's quiescence rule: silence is an
    /// anomaly signal, never a completion — without this a crashed agent would
    /// leave its entity stuck in drafting/building for the daemon's whole life.
    fn mark_idle_tasks(&mut self, quiet_threshold: Duration) -> Vec<String> {
        // For each demoted entity, remember whether its harness *exited* (with
        // which code) versus merely fell silent — an exited harness gets the
        // "agent exited unexpectedly (exit code N)" last_error so the crash is
        // legible; a quiet-but-alive one does not.
        let idle_check = |exited: bool, code: Option<i32>, idle: Option<Duration>| {
            if exited {
                Some(Some(code.unwrap_or(-1)))
            } else if idle.is_some_and(|d| d >= quiet_threshold) {
                Some(None)
            } else {
                None
            }
        };
        let idle_plans: Vec<(String, Option<i32>)> = self
            .plans
            .iter()
            .filter(|(_, a)| a.plan.state.is_working())
            .filter_map(|(id, a)| {
                idle_check(
                    a.session.harness_exited(),
                    a.session.harness_exit_code(),
                    a.session.harness_idle_for(),
                )
                .map(|code| (id.clone(), code))
            })
            .collect();
        let idle_runs: Vec<(String, Option<i32>)> = self
            .runs
            .iter()
            .filter(|(_, a)| a.run.state.is_working())
            .filter_map(|(id, a)| {
                idle_check(
                    a.session.harness_exited(),
                    a.session.harness_exit_code(),
                    a.session.harness_idle_for(),
                )
                .map(|code| (id.clone(), code))
            })
            .collect();

        let mut idle_ids: Vec<String> = Vec::new();
        for (plan_id, exit_code) in idle_plans {
            idle_ids.push(plan_id.clone());
            let Some(mut active) = self.plans.remove(&plan_id) else {
                continue;
            };
            let outcome = match self.project_of(&plan_id).and_then(|pid| {
                self.orch_for(&pid)
                    .and_then(|orch| orch.on_plan_idle(&mut active).map_err(err))
            }) {
                Ok(()) => Ok(()),
                Err(e) => Err(e),
            };
            if let Err(e) = outcome {
                eprintln!("idle monitor {plan_id}: {e}");
            }
            if let Some(code) = exit_code {
                active.last_error = Some(format!("agent exited unexpectedly (exit code {code})"));
            }
            let (_, persisted) = self.finish_plan_mutation(plan_id.clone(), active);
            if let Err(e) = persisted {
                eprintln!("idle monitor {plan_id}: {e}");
            }
        }
        for (run_id, exit_code) in idle_runs {
            idle_ids.push(run_id.clone());
            let Some(mut active) = self.runs.remove(&run_id) else {
                continue;
            };
            let outcome = match self.project_of(&run_id).and_then(|pid| {
                self.orch_for(&pid)
                    .and_then(|orch| orch.on_run_idle(&mut active).map_err(err))
            }) {
                Ok(()) => Ok(()),
                Err(e) => Err(e),
            };
            if let Err(e) = outcome {
                eprintln!("idle monitor {run_id}: {e}");
            }
            if let Some(code) = exit_code {
                active.last_error = Some(format!("agent exited unexpectedly (exit code {code})"));
            }
            let (_, persisted) = self.finish_run_mutation(run_id.clone(), active);
            if let Err(e) = persisted {
                eprintln!("idle monitor {run_id}: {e}");
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

    /// The project an entity (plan or run) belongs to.
    fn project_of(&self, entity_id: &str) -> Result<String, String> {
        self.entity_project
            .get(entity_id)
            .cloned()
            .ok_or_else(|| "unknown entity id".to_string())
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
        let run_id = params.get("run_id").and_then(Value::as_str);
        match (project_id, run_id) {
            (Some(project_id), None) => {
                let project = self
                    .projects
                    .iter()
                    .find(|p| p.id == project_id)
                    .ok_or_else(|| "unknown project_id".to_string())?;
                Ok(GitScope {
                    repo_path: project.repo_path.clone(),
                    project_id: Some(project.id.clone()),
                    run: None,
                })
            }
            (None, Some(run_id)) => {
                let active = self
                    .runs
                    .get(run_id)
                    .ok_or_else(|| "unknown run_id".to_string())?;
                Ok(GitScope {
                    repo_path: active.worktree.path.clone(),
                    project_id: None,
                    run: Some(GitScopeRun {
                        run_id: run_id.to_string(),
                        base_branch: active.worktree.base_branch.clone(),
                    }),
                })
            }
            _ => Err("provide exactly one of project_id or run_id".to_string()),
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
        let mark_ahead_of = scope.run.as_ref().map(|run| run.base_branch.clone());
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
        self.invalidate_git_scope_caches(&scope);
        let status = crate::gitgui::status_payload(&scope.repo_path)?;
        Ok(json!({
            "hash": commit["hash"],
            "short": commit["short"],
            "subject": commit["subject"],
            "status": status,
        }))
    }

    /// Drop the cached board summaries a scoped git mutation just invalidated —
    /// the task's diffstat + updated-at for task scope, the project's primary
    /// uncommitted-changes summary for project scope — so the next `task.list`
    /// / project poll recomputes instead of serving a stale summary for up to
    /// its TTL.
    fn invalidate_git_scope_caches(&mut self, scope: &GitScope) {
        if let Some(run) = &scope.run {
            self.run_stat_cache.remove(&run.run_id);
            self.entity_updated_at
                .insert(run.run_id.clone(), now_rfc3339());
        }
        if let Some(project_id) = &scope.project_id {
            if let Some(project) = self.projects.iter_mut().find(|p| &p.id == project_id) {
                project.primary_summary = None;
            }
        }
    }

    /// Resolve a **project-scope-only** git RPC (branch operations): a task
    /// worktree's branch is owned by the task lifecycle, so a `task_id` is
    /// refused outright. Returns the project id (for cache invalidation) and
    /// its primary-checkout path.
    fn resolve_project_repo(&self, params: &Value) -> Result<(String, std::path::PathBuf), String> {
        if params.get("task_id").is_some() {
            return Err("branch operations are project-scope only".to_string());
        }
        let project_id = require_str(params, "project_id")?;
        let project = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .ok_or_else(|| "unknown project_id".to_string())?;
        Ok((project.id.clone(), project.repo_path.clone()))
    }

    /// `git.fetch` — `git fetch --prune`, then the fresh status payload.
    fn git_fetch(&mut self, params: &Value) -> Result<Value, String> {
        let scope = self.resolve_git_scope(params)?;
        crate::gitgui::fetch(&scope.repo_path)?;
        self.invalidate_git_scope_caches(&scope);
        crate::gitgui::status_payload(&scope.repo_path)
    }

    /// `git.pull` — integrate the upstream in the requested mode (ff/merge/
    /// rebase), then the fresh status payload. Git's own errors pass through.
    fn git_pull(&mut self, params: &Value) -> Result<Value, String> {
        let scope = self.resolve_git_scope(params)?;
        let mode = params.get("mode").and_then(Value::as_str).unwrap_or("ff");
        crate::gitgui::pull(&scope.repo_path, mode)?;
        self.invalidate_git_scope_caches(&scope);
        crate::gitgui::status_payload(&scope.repo_path)
    }

    /// `git.push` — push the current branch (setting the upstream on first
    /// push), then the fresh status payload. `force` uses `--force-with-lease`.
    fn git_push(&mut self, params: &Value) -> Result<Value, String> {
        let scope = self.resolve_git_scope(params)?;
        let force = params
            .get("force")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        crate::gitgui::push(&scope.repo_path, force)?;
        self.invalidate_git_scope_caches(&scope);
        crate::gitgui::status_payload(&scope.repo_path)
    }

    /// `git.branches` (project scope only) — the local branch list.
    fn git_branches(&mut self, params: &Value) -> Result<Value, String> {
        let (_project_id, repo_path) = self.resolve_project_repo(params)?;
        crate::gitgui::branch_list(&repo_path)
    }

    /// `git.checkout` (project scope only) — switch to (or create) a branch,
    /// then the fresh status payload.
    fn git_checkout(&mut self, params: &Value) -> Result<Value, String> {
        let (project_id, repo_path) = self.resolve_project_repo(params)?;
        let branch = require_str(params, "branch")?;
        let create = params
            .get("create")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        crate::gitgui::checkout(&repo_path, &branch, create)?;
        // A branch switch swaps the whole primary tree, so the cached
        // uncommitted-changes summary is stale.
        if let Some(project) = self.projects.iter_mut().find(|p| p.id == project_id) {
            project.primary_summary = None;
        }
        crate::gitgui::status_payload(&repo_path)
    }

    /// `git.branch_delete` (project scope only) — delete a local branch, then
    /// the fresh branch list.
    fn git_branch_delete(&mut self, params: &Value) -> Result<Value, String> {
        let (_project_id, repo_path) = self.resolve_project_repo(params)?;
        let branch = require_str(params, "branch")?;
        let force = params
            .get("force")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        crate::gitgui::branch_delete(&repo_path, &branch, force)?;
        crate::gitgui::branch_list(&repo_path)
    }

    /// `git.stash` — `git stash push -u`, then the fresh status payload.
    fn git_stash(&mut self, params: &Value) -> Result<Value, String> {
        let scope = self.resolve_git_scope(params)?;
        crate::gitgui::stash_push(&scope.repo_path)?;
        self.invalidate_git_scope_caches(&scope);
        crate::gitgui::status_payload(&scope.repo_path)
    }

    /// `git.stash_pop` — `git stash pop`, then the fresh status payload.
    fn git_stash_pop(&mut self, params: &Value) -> Result<Value, String> {
        let scope = self.resolve_git_scope(params)?;
        crate::gitgui::stash_pop(&scope.repo_path)?;
        self.invalidate_git_scope_caches(&scope);
        crate::gitgui::status_payload(&scope.repo_path)
    }

    /// `git.discard` (**destructive**) — revert the given paths to HEAD
    /// (untracked ones are deleted), then the fresh status payload.
    fn git_discard(&mut self, params: &Value) -> Result<Value, String> {
        let scope = self.resolve_git_scope(params)?;
        let paths = require_path_list(params)?;
        crate::gitgui::discard_paths(&scope.repo_path, &paths)?;
        self.invalidate_git_scope_caches(&scope);
        crate::gitgui::status_payload(&scope.repo_path)
    }

    /// `git.merge_abort` — abort whatever operation is in progress (merge,
    /// rebase, cherry-pick, revert, or bisect), then the fresh status payload.
    fn git_merge_abort(&mut self, params: &Value) -> Result<Value, String> {
        let scope = self.resolve_git_scope(params)?;
        crate::gitgui::merge_abort(&scope.repo_path)?;
        self.invalidate_git_scope_caches(&scope);
        crate::gitgui::status_payload(&scope.repo_path)
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

    // ---- Store accessor + take/finish plumbing --------------------------------

    /// The durable store, or a clean error. Plans and runs both require one:
    /// plan docs are canonical in the store, and `dispatch_run` writes/reads
    /// through it. Only unit tests that never create a plan/run skip it.
    fn require_store(&self) -> Result<&Store, String> {
        self.store
            .as_ref()
            .ok_or_else(|| "no task store configured".to_string())
    }

    fn take_plan(&mut self, plan_id: &str) -> Result<ActivePlan, String> {
        self.plans.remove(plan_id).ok_or("unknown plan_id".into())
    }

    fn take_run(&mut self, run_id: &str) -> Result<ActiveRun, String> {
        self.runs.remove(run_id).ok_or("unknown run_id".into())
    }

    /// The owning plan's stage-doc manifest for a run (the caller's join by
    /// `plan_id`): empty for quick runs, single-doc plans, and orphaned links.
    fn owning_plan_stage_docs(&self, run: &ActiveRun) -> Vec<StageDoc> {
        run.run
            .plan_id
            .as_ref()
            .and_then(|pid| self.plans.get(&pid.0))
            .map(|plan| plan.stages.clone())
            .unwrap_or_default()
    }

    /// The default project id when the client doesn't choose one (the first
    /// registered project).
    fn default_project(&self) -> Result<String, String> {
        self.projects
            .first()
            .map(|p| p.id.clone())
            .ok_or_else(|| "no projects configured".to_string())
    }

    // ---- Plan surface ---------------------------------------------------------

    /// Author a new plan: spin up a disposable planning worktree and a plan
    /// agent session (the docs land canonically in the store on `done`).
    fn plan_create(&mut self, params: &Value) -> Result<Value, String> {
        let goal = require_str(params, "goal")?;
        let project_id = match params.get("project_id").and_then(Value::as_str) {
            Some(p) => p.to_string(),
            None => self.default_project()?,
        };
        let base = self.base_for(&project_id)?;
        let model_choice = model_choice_from(params)?;
        self.require_store()?;
        let plan_id = format!("plan-{}", uuid::Uuid::new_v4());
        let mut active = self
            .orch_for(&project_id)?
            .dispatch_plan(PlanId::new(&plan_id), goal, &base, model_choice)
            .map_err(err)?;
        self.entity_project
            .insert(plan_id.clone(), project_id.clone());
        if self.qa_agent {
            self.qa_simulate_plan(&project_id, &mut active)?;
        }
        let (view, persisted) = self.finish_plan_mutation(plan_id, active);
        persisted?;
        Ok(view)
    }

    fn plan_get(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let active = self.plans.get(&plan_id).ok_or("unknown plan_id")?;
        Ok(self.plan_view(&plan_id, active))
    }

    fn plan_list(&self) -> Value {
        let plans: Vec<Value> = self
            .plans
            .iter()
            .map(|(id, active)| self.plan_view(id, active))
            .collect();
        json!({ "plans": plans })
    }

    /// Read the single (non-staged) plan doc from the canonical store — never
    /// from a worktree (the worktree is disposable; the store is the truth).
    fn plan_doc(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let active = self.plans.get(&plan_id).ok_or("unknown plan_id")?;
        if active.is_multi_stage() {
            return Err("multi-stage plan: use plan.stages / plan.stage_doc".to_string());
        }
        let plan_path = active.plan_path.clone();
        let contents = self
            .require_store()?
            .read_plan_doc(&plan_id, &plan_path)
            .ok_or_else(|| format!("plan doc not available: {plan_path}"))?;
        Ok(json!({ "plan_path": plan_path, "contents": contents }))
    }

    /// The stage board for a plan: manifest order, doc sub-state, and every
    /// comment (open and addressed) per stage. Read-only.
    fn plan_stages(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let active = self.plans.get(&plan_id).ok_or("unknown plan_id")?;
        if !active.is_multi_stage() {
            return Err("not a multi-stage plan".to_string());
        }
        let stages: Vec<Value> = active
            .stages
            .iter()
            .map(|doc| {
                let mut view = plan_stage_json(active, doc);
                let comments: Vec<Value> = active
                    .comments
                    .iter()
                    .filter(|c| c.stage_id == doc.id)
                    .map(comment_json)
                    .collect();
                view.as_object_mut()
                    .expect("plan_stage_json returns an object")
                    .insert("comments".to_string(), json!(comments));
                view
            })
            .collect();
        Ok(json!({ "plan_id": plan_id, "stages": stages }))
    }

    /// Read one stage's plan doc from the canonical store.
    fn plan_stage_doc(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let active = self.plans.get(&plan_id).ok_or("unknown plan_id")?;
        if !active.is_multi_stage() {
            return Err("not a multi-stage plan".to_string());
        }
        let index = active.stage_doc_index(&stage_id)?;
        let doc = &active.stages[index];
        // Defense in depth against a corrupted manifest: never read outside the
        // plan dir regardless of what the record says.
        if !doc.path.starts_with(".build/plan/")
            || !crate::plan::is_worktree_contained_path(&doc.path)
        {
            return Err(format!(
                "stage doc path escapes .build/plan/: {:?}",
                doc.path
            ));
        }
        let path = doc.path.clone();
        let contents = self
            .require_store()?
            .read_plan_doc(&plan_id, &path)
            .ok_or_else(|| format!("stage doc not available: {path}"))?;
        Ok(json!({ "stage_id": stage_id, "path": path, "contents": contents }))
    }

    /// Approve the plan (the last human gate): the disposable planning worktree
    /// is torn down; the store docs are canonical.
    fn plan_approve(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let project_id = self.project_of(&plan_id)?;
        let mut active = self.take_plan(&plan_id)?;
        let outcome = self
            .orch_for(&project_id)
            .and_then(|orch| orch.approve_plan(&mut active).map_err(err));
        let (view, persisted) = self.finish_plan_mutation(plan_id, active);
        outcome?;
        persisted?;
        Ok(view)
    }

    /// Send a batch of plan notes back to a fresh revision session.
    fn plan_send_notes(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let comments = require_str(params, "comments")?;
        let project_id = self.project_of(&plan_id)?;
        if self
            .plans
            .get(&plan_id)
            .is_some_and(ActivePlan::is_multi_stage)
        {
            return Err(
                "multi-stage plan: use plan.comment_add + plan.stage_send_notes".to_string(),
            );
        }
        let mut active = self.take_plan(&plan_id)?;
        let outcome = (|| -> Result<(), String> {
            let store = self.require_store()?;
            self.orch_for(&project_id)?
                .send_plan_notes(&mut active, store, &comments)
                .map_err(err)?;
            if self.qa_agent {
                self.qa_simulate_plan(&project_id, &mut active)?;
            }
            Ok(())
        })();
        let (view, persisted) = self.finish_plan_mutation(plan_id, active);
        outcome?;
        persisted?;
        Ok(view)
    }

    fn plan_stage_approve(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let project_id = self.project_of(&plan_id)?;
        let mut active = self.take_plan(&plan_id)?;
        let outcome = self
            .orch_for(&project_id)
            .and_then(|orch| orch.approve_plan_stage(&mut active, &stage_id).map_err(err));
        let (view, persisted) = self.finish_plan_mutation(plan_id, active);
        outcome?;
        persisted?;
        Ok(view)
    }

    /// Send a stage's open comments to a fresh plan-revision session (the open
    /// comments ARE the payload).
    fn plan_stage_send_notes(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let project_id = self.project_of(&plan_id)?;
        let mut active = self.take_plan(&plan_id)?;
        let outcome = (|| -> Result<(), String> {
            let store = self.require_store()?;
            self.orch_for(&project_id)?
                .send_plan_stage_notes(&mut active, store, &stage_id)
                .map_err(err)?;
            if self.qa_agent {
                self.qa_simulate_plan_stage_revise(&project_id, &mut active)?;
            }
            Ok(())
        })();
        let (view, persisted) = self.finish_plan_mutation(plan_id, active);
        outcome?;
        persisted?;
        Ok(view)
    }

    /// A freeform human message to the plan's agent.
    fn plan_message(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let message = require_str(params, "message")?;
        let project_id = self.project_of(&plan_id)?;
        let mut active = self.take_plan(&plan_id)?;
        let outcome = (|| -> Result<(), String> {
            let store = self.require_store()?;
            self.orch_for(&project_id)?
                .message_plan(&mut active, store, &message)
                .map_err(err)?;
            if self.qa_agent && active.plan.state == PlanState::Drafting {
                if active.revising_stage_id.is_some() {
                    self.qa_simulate_plan_stage_revise(&project_id, &mut active)?;
                } else {
                    self.qa_simulate_plan(&project_id, &mut active)?;
                }
            }
            Ok(())
        })();
        let (view, persisted) = self.finish_plan_mutation(plan_id, active);
        outcome?;
        persisted?;
        Ok(view)
    }

    fn plan_abandon(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let project_id = self.project_of(&plan_id)?;
        let mut active = self.take_plan(&plan_id)?;
        let outcome = self
            .orch_for(&project_id)
            .and_then(|orch| orch.abandon_plan(&mut active).map_err(err));
        let (view, persisted) = self.finish_plan_mutation(plan_id, active);
        outcome?;
        persisted?;
        Ok(view)
    }

    /// Delete an abandoned plan from the board: drop its record + canonical
    /// docs and the in-memory bookkeeping. Valid only for `Abandoned` plans (a
    /// live or approved plan must be abandoned first), and refused while any
    /// non-terminal run still implements it (that run would lose its docs).
    fn plan_delete(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let active = self.plans.get(&plan_id).ok_or("unknown plan_id")?;
        if active.plan.state != PlanState::Abandoned {
            return Err(format!(
                "plan.delete: plan is {} — only an abandoned plan can be deleted",
                plan_state_str(&active.plan.state)
            ));
        }
        if self.runs.values().any(|r| {
            r.run.plan_id.as_ref().map(|p| &p.0) == Some(&plan_id) && !r.run.state.is_terminal()
        }) {
            return Err("plan.delete: a non-terminal run still implements this plan".to_string());
        }
        if let Some(store) = &self.store {
            store
                .delete_plan(&plan_id)
                .map_err(|e| format!("plan store: {e}"))?;
        }
        self.plans.remove(&plan_id);
        self.entity_project.remove(&plan_id);
        self.entity_project_path.remove(&plan_id);
        self.entity_created_at.remove(&plan_id);
        self.entity_updated_at.remove(&plan_id);
        self.reap_orphaned_terminals();
        Ok(json!({ "ok": true }))
    }

    fn plan_comment_add(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let body = require_str(params, "body")?;
        let mut active = self.take_plan(&plan_id)?;
        let mut minted: Option<StageComment> = None;
        let outcome = (|| -> Result<(), String> {
            if body.trim().is_empty() {
                return Err("comment body must not be empty".to_string());
            }
            if active.plan.state.is_terminal() {
                return Err("cannot comment on a terminal plan".to_string());
            }
            let index = active.stage_doc_index(&stage_id)?;
            let doc_state = active.stages[index].state;
            if !matches!(doc_state, StageDocState::Planned | StageDocState::Approved) {
                return Err(format!(
                    "comments are only accepted on planned/approved stages (stage is {})",
                    stage_doc_state_str(&doc_state)
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
        let (_, persisted) = self.finish_plan_mutation(plan_id, active);
        outcome?;
        persisted?;
        let comment = minted.expect("outcome Ok implies a comment was minted");
        Ok(json!({ "comment": comment_json(&comment) }))
    }

    fn plan_comment_delete(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let comment_id = require_str(params, "comment_id")?;
        let mut active = self.take_plan(&plan_id)?;
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
        let (_, persisted) = self.finish_plan_mutation(plan_id, active);
        outcome?;
        persisted?;
        Ok(json!({ "ok": true }))
    }

    // ---- Run surface ----------------------------------------------------------

    /// Create a run: implement an approved plan (`plan_id`) or a plan-less
    /// quick task (`goal`). Single-active-writer: a second concurrent run of
    /// the same plan is rejected at dispatch.
    fn run_create(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = params
            .get("plan_id")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let model_choice = model_choice_from(params)?;
        let base_override = params
            .get("base_branch")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let run_id = format!("run-{}", uuid::Uuid::new_v4());

        let (project_id, mut active) = if let Some(plan_id) = plan_id {
            if !self.plans.contains_key(&plan_id) {
                return Err("unknown plan_id".to_string());
            }
            let project_id = self.project_of(&plan_id)?;
            let base = base_override.unwrap_or_else(|| self.plans[&plan_id].base_branch.clone());
            let has_active_run = self.runs.values().any(|r| {
                r.run.plan_id.as_ref().map(|p| &p.0) == Some(&plan_id) && !r.run.state.is_terminal()
            });
            let store = self.require_store()?;
            let plan = &self.plans[&plan_id];
            let active = self
                .orch_for(&project_id)?
                .dispatch_run(
                    RunId::new(&run_id),
                    RunSource::Plan {
                        plan,
                        has_active_run,
                    },
                    &base,
                    model_choice,
                    store,
                )
                .map_err(err)?;
            (project_id, active)
        } else {
            let goal = require_str(params, "goal")?;
            let project_id = match params.get("project_id").and_then(Value::as_str) {
                Some(p) => p.to_string(),
                None => self.default_project()?,
            };
            let base = base_override.unwrap_or(self.base_for(&project_id)?);
            let store = self.require_store()?;
            let active = self
                .orch_for(&project_id)?
                .dispatch_run(
                    RunId::new(&run_id),
                    RunSource::Quick { goal: &goal },
                    &base,
                    model_choice,
                    store,
                )
                .map_err(err)?;
            (project_id, active)
        };

        self.entity_project
            .insert(run_id.clone(), project_id.clone());
        if self.qa_agent {
            let plan_docs = self.owning_plan_stage_docs(&active);
            self.qa_drive_run(&project_id, &mut active, &plan_docs)?;
        }
        let (_, persisted) = self.finish_run_mutation(run_id.clone(), active);
        persisted?;
        self.auto_advance_run(&run_id);
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        Ok(self.run_view(&run_id, active))
    }

    fn run_get(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        Ok(self.run_view(&run_id, active))
    }

    fn run_diff(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let project_id = self.project_of(&run_id)?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        let diff = self.orch_for(&project_id)?.run_diff(active).map_err(err)?;
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

    /// Send diff comments to the coding agent — from `review` or `building`.
    fn run_request_changes(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let comments = require_str(params, "comments")?;
        let project_id = self.project_of(&run_id)?;
        let plan_docs = {
            let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
            self.owning_plan_stage_docs(active)
        };
        let mut active = self.take_run(&run_id)?;
        let outcome = (|| -> Result<(), String> {
            self.orch_for(&project_id)?
                .run_request_changes(&mut active, &comments)
                .map_err(err)?;
            self.qa_drive_run(&project_id, &mut active, &plan_docs)
        })();
        let (view, persisted) = self.finish_run_mutation(run_id, active);
        outcome?;
        persisted?;
        Ok(view)
    }

    fn run_stage_dispatch(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let model_override = if params.get("model").is_some() || params.get("effort").is_some() {
            Some(model_choice_from(params)?)
        } else {
            None
        };
        let project_id = self.project_of(&run_id)?;
        let plan_docs = {
            let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
            self.owning_plan_stage_docs(active)
        };
        let mut active = self.take_run(&run_id)?;
        let outcome = (|| -> Result<(), String> {
            self.orch_for(&project_id)?
                .dispatch_run_stage(&mut active, &plan_docs, &stage_id, model_override)
                .map_err(err)?;
            self.qa_drive_run(&project_id, &mut active, &plan_docs)
        })();
        let (_, persisted) = self.finish_run_mutation(run_id.clone(), active);
        outcome?;
        persisted?;
        self.auto_advance_run(&run_id);
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        Ok(self.run_view(&run_id, active))
    }

    fn run_stage_fix(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let note = params
            .get("note")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let project_id = self.project_of(&run_id)?;
        let plan_docs = {
            let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
            self.owning_plan_stage_docs(active)
        };
        let mut active = self.take_run(&run_id)?;
        let outcome = (|| -> Result<(), String> {
            self.orch_for(&project_id)?
                .fix_run_stage(&mut active, &plan_docs, &stage_id, &note)
                .map_err(err)?;
            self.qa_drive_run(&project_id, &mut active, &plan_docs)
        })();
        let (_, persisted) = self.finish_run_mutation(run_id.clone(), active);
        outcome?;
        persisted?;
        self.auto_advance_run(&run_id);
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        Ok(self.run_view(&run_id, active))
    }

    /// "Run all": arm/disarm auto-advance, then (armed) run every dispatchable
    /// approved stage to its verdict.
    fn run_set_auto_advance(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let enabled = params
            .get("enabled")
            .and_then(Value::as_bool)
            .ok_or("missing required param: enabled")?;
        let mut active = self.take_run(&run_id)?;
        if active.run.state.is_terminal() {
            self.runs.insert(run_id, active);
            return Err("cannot set auto_advance on a terminal run".to_string());
        }
        active.auto_advance = enabled;
        let (_, persisted) = self.finish_run_mutation(run_id.clone(), active);
        persisted?;
        if enabled {
            self.auto_advance_run(&run_id);
        }
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        Ok(self.run_view(&run_id, active))
    }

    /// If a run is parked at the stage gate with run-all armed and a next
    /// approved+dispatchable stage, dispatch it (looping through as many stages
    /// as run-all chains into). In production one hop leaves the run `Building`
    /// and the loop returns; in QA `qa_drive_run` drives it back to the gate,
    /// so the loop advances to the next stage.
    fn auto_advance_run(&mut self, run_id: &str) {
        let max_hops = 64;
        for _ in 0..max_hops {
            let (project_id, plan_docs, next) = {
                let Some(active) = self.runs.get(run_id) else {
                    return;
                };
                if active.run.state != RunState::StageGate || !active.auto_advance {
                    return;
                }
                let plan_docs = self.owning_plan_stage_docs(active);
                let Some(next) = dispatchable_next_run_stage(active, &plan_docs) else {
                    return;
                };
                let Ok(project_id) = self.project_of(run_id) else {
                    return;
                };
                (project_id, plan_docs, next)
            };
            let Ok(mut active) = self.take_run(run_id) else {
                return;
            };
            let outcome = (|| -> Result<(), String> {
                self.orch_for(&project_id)?
                    .dispatch_run_stage(&mut active, &plan_docs, &next, None)
                    .map_err(err)?;
                self.qa_drive_run(&project_id, &mut active, &plan_docs)
            })();
            let (_, persisted) = self.finish_run_mutation(run_id.to_string(), active);
            if let Err(e) = outcome {
                eprintln!("auto-advance {run_id}: {e}");
                return;
            }
            if let Err(e) = persisted {
                eprintln!("auto-advance {run_id}: {e}");
                return;
            }
        }
        eprintln!("auto-advance {run_id}: did not converge");
    }

    /// Finish-the-worktree git actions from the diff review: `commit`/`push`
    /// keep the worktree; `merge`/`merge_push` merge into the base and end the
    /// run. Every action commits outstanding work first.
    fn run_git_action(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let action = require_str(params, "action")?;
        let project_id = self.project_of(&run_id)?;
        let is_merge_action = matches!(action.as_str(), "merge" | "merge_push");
        if !is_merge_action && params.get("cleanup").is_some() {
            return Err("cleanup only applies to merge actions".to_string());
        }
        let adopted = self
            .runs
            .get(&run_id)
            .map(|a| a.adopted)
            .ok_or("unknown run_id")?;
        let cleanup = if is_merge_action {
            merge_cleanup_from(params, adopted)?
        } else {
            MergeCleanup::Prune
        };
        let mut active = self.take_run(&run_id)?;
        let result = {
            let orch = self.orch_for(&project_id)?;
            match action.as_str() {
                "commit" => orch.run_commit(&active).map_err(err),
                "push" => orch.run_push(&active).map_err(err),
                "merge" => orch.run_approve_merge(&mut active).map_err(err),
                "merge_push" => orch.run_merge_and_push(&mut active).map_err(err),
                other => Err(format!("unknown git action: {other}")),
            }
        };
        if let Err(message) = &result {
            if message.starts_with("merge_failed:") {
                active.last_error = Some(message.clone());
            }
        }
        let merged_worktree = (result.is_ok() && active.run.state == RunState::Merged)
            .then(|| active.worktree.clone());
        let (view, persisted) = self.finish_run_mutation(run_id.clone(), active);
        result?;
        persisted?;
        if let Some(worktree) = merged_worktree {
            self.apply_merge_cleanup(&run_id, &project_id, &worktree, cleanup);
        }
        Ok(view)
    }

    /// Prune a merged run's worktree once its `Merged` verdict is durable.
    fn prune_merged_worktree(&self, project_id: &str, worktree: &Worktree) {
        if let Ok(orch) = self.orch_for(project_id) {
            orch.discard_worktree(worktree);
        }
    }

    /// What happens to the worktree after a user-approved merge lands.
    fn apply_merge_cleanup(
        &mut self,
        run_id: &str,
        project_id: &str,
        worktree: &Worktree,
        cleanup: MergeCleanup,
    ) {
        match cleanup {
            MergeCleanup::Prune => self.prune_merged_worktree(project_id, worktree),
            MergeCleanup::Keep => {}
            MergeCleanup::Release => {
                if let Some(store) = &self.store {
                    if let Err(e) = store.delete_run(run_id) {
                        eprintln!("merge cleanup release {run_id}: run store: {e}");
                    }
                }
                self.runs.remove(run_id);
                self.entity_project.remove(run_id);
                self.entity_project_path.remove(run_id);
                self.entity_created_at.remove(run_id);
                self.entity_updated_at.remove(run_id);
                self.run_stat_cache.remove(run_id);
                self.invalidate_external_scan(project_id);
            }
        }
    }

    /// A freeform message to the run's agent — redirects a live session or
    /// resumes a parked one.
    fn run_message(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let message = require_str(params, "message")?;
        let project_id = self.project_of(&run_id)?;
        let plan_docs = {
            let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
            self.owning_plan_stage_docs(active)
        };
        let mut active = self.take_run(&run_id)?;
        let outcome = (|| -> Result<(), String> {
            self.orch_for(&project_id)?
                .message_run(&mut active, &message)
                .map_err(err)?;
            if self.qa_agent && active.run.state == RunState::Building {
                self.qa_drive_run(&project_id, &mut active, &plan_docs)?;
            }
            Ok(())
        })();
        let (view, persisted) = self.finish_run_mutation(run_id, active);
        outcome?;
        persisted?;
        Ok(view)
    }

    fn run_abandon(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let project_id = self.project_of(&run_id)?;
        let mut active = self.take_run(&run_id)?;
        let result = self
            .orch_for(&project_id)
            .and_then(|orch| orch.abandon_run(&mut active).map_err(err));
        let (view, persisted) = self.finish_run_mutation(run_id, active);
        result?;
        persisted?;
        Ok(view)
    }

    /// Delete a terminal run from the board: prune any leftover worktree,
    /// remove the durable record, drop the bookkeeping. Terminal runs only
    /// (merged/abandoned/archived/failed) — a live run must be abandoned first.
    fn run_delete(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        let state = active.run.state;
        if !matches!(
            state,
            RunState::Merged | RunState::Abandoned | RunState::Archived | RunState::Failed
        ) {
            return Err(format!(
                "run.delete: run is {} — only terminal runs \
                 (merged/abandoned/archived/failed) can be deleted",
                run_state_str(&state)
            ));
        }
        let worktree = active.worktree.clone();
        let adopted = active.adopted;
        let project_id = self.entity_project.get(&run_id).cloned();

        if let Some(store) = &self.store {
            store
                .delete_run(&run_id)
                .map_err(|e| format!("run store: {e}"))?;
        }

        let mut active = self.runs.remove(&run_id).expect("checked above");
        active.session.end();

        // A failed run still holds its worktree; deleting an adopted run's card
        // must never delete the user's files (delete removes the card, not the
        // worktree it was minted around).
        if worktree.path.exists() && !adopted {
            if let Some(orch) = project_id
                .as_deref()
                .and_then(|pid| self.orch_for(pid).ok())
            {
                orch.discard_worktree(&worktree);
            }
        }

        self.entity_project.remove(&run_id);
        self.entity_project_path.remove(&run_id);
        self.entity_created_at.remove(&run_id);
        self.entity_updated_at.remove(&run_id);
        self.run_stat_cache.remove(&run_id);

        if worktree.path.exists() {
            if let Some(pid) = project_id {
                self.invalidate_external_scan(&pid);
            }
        }
        self.reap_orphaned_terminals();
        Ok(json!({ "ok": true }))
    }

    /// Mint a quick run around an existing external worktree (`plan_id` None).
    fn run_adopt(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let worktree_id = require_str(params, "worktree_id")?;
        let model_choice = model_choice_from(params)?;
        // Force a fresh scan: adoption must never act on a stale card.
        let external = self
            .external_worktrees(&project_id, true)?
            .into_iter()
            .find(|w| w.id == worktree_id)
            .ok_or_else(|| format!("unknown worktree_id: {worktree_id}"))?;
        let base = self.base_for(&project_id)?;
        let run_id = format!("run-{}", uuid::Uuid::new_v4());
        let active = self
            .orch_for(&project_id)?
            .adopt_run(RunId::new(&run_id), &external, &base, model_choice)
            .map_err(err)?;
        self.entity_project
            .insert(run_id.clone(), project_id.clone());
        self.invalidate_external_scan(&project_id);
        let (view, persisted) = self.finish_run_mutation(run_id, active);
        persisted?;
        Ok(view)
    }

    /// Un-adopt: drop the run record and its binding, leaving every file
    /// untouched. Legal on adopted runs in any non-terminal state.
    fn run_release(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        if !active.adopted {
            return Err("run.release: only adopted runs can be released".to_string());
        }
        if active.run.state.is_terminal() {
            return Err(format!(
                "run.release: run is {} — use run.delete to clear it off the board",
                run_state_str(&active.run.state)
            ));
        }
        if let Some(store) = &self.store {
            store
                .delete_run(&run_id)
                .map_err(|e| format!("run store: {e}"))?;
        }
        let mut active = self.runs.remove(&run_id).expect("checked above");
        active.session.end();
        let project_id = self.entity_project.remove(&run_id);
        self.entity_project_path.remove(&run_id);
        self.entity_created_at.remove(&run_id);
        self.entity_updated_at.remove(&run_id);
        self.run_stat_cache.remove(&run_id);
        if let Some(pid) = project_id {
            self.invalidate_external_scan(&pid);
        }
        self.reap_orphaned_terminals();
        Ok(json!({ "ok": true }))
    }

    // ---- Board + views --------------------------------------------------------

    /// The board: plans + runs (each run carries a live diffstat), plus the
    /// ride-along external-worktree and primary-changes summaries. Sweeps runs
    /// whose worktree was deleted out of band into `archived` first.
    fn board_list(&mut self) -> Value {
        self.archive_runs_with_deleted_worktrees();
        let plans: Vec<Value> = {
            let ids: Vec<String> = self.plans.keys().cloned().collect();
            ids.into_iter()
                .map(|id| {
                    let active = self.plans.get(&id).expect("listed above");
                    self.plan_view(&id, active)
                })
                .collect()
        };
        let runs: Vec<Value> = {
            let ids: Vec<String> = self.runs.keys().cloned().collect();
            ids.into_iter()
                .map(|id| {
                    let stat = self.run_stat(&id);
                    let active = self.runs.get(&id).expect("listed above");
                    let mut view = self.run_view(&id, active);
                    view.as_object_mut()
                        .expect("run_view returns an object")
                        .insert("stat".to_string(), stat);
                    view
                })
                .collect()
        };
        let external_worktrees = self.external_worktrees_json();
        let primary_changes = self.primary_changes_json();
        json!({
            "plans": plans,
            "runs": runs,
            "external_worktrees": external_worktrees,
            "primary_changes": primary_changes,
        })
    }

    /// A run the user deletes must disappear from Build. Any live run whose
    /// worktree vanished retires to Archived: session ended, git's stale
    /// worktree record pruned — the record stays as quiet history. `Created` is
    /// exempt (its worktree may legitimately not exist yet).
    fn archive_runs_with_deleted_worktrees(&mut self) {
        let doomed: Vec<String> = self
            .runs
            .iter()
            .filter(|(_, active)| {
                !active.run.state.is_terminal()
                    && active.run.state != RunState::Created
                    && !active.worktree.path.exists()
            })
            .map(|(id, _)| id.clone())
            .collect();
        for run_id in doomed {
            let Ok(mut active) = self.take_run(&run_id) else {
                continue;
            };
            match active.run.apply(RunEvent::Archive) {
                Ok(_) => {
                    active.session.end();
                    self.prune_worktree_records(&run_id);
                    eprintln!(
                        "archived {run_id}: its worktree {} was deleted outside Build",
                        active.worktree.path.display()
                    );
                }
                Err(e) => eprintln!("archive {run_id}: {e}"),
            }
            let (_, persisted) = self.finish_run_mutation(run_id.clone(), active);
            if let Err(e) = persisted {
                eprintln!("archive {run_id}: {e}");
            }
        }
    }

    /// Best-effort `git worktree prune` in the run's project repo.
    fn prune_worktree_records(&mut self, run_id: &str) {
        let Some(repo_path) = self
            .entity_project
            .get(run_id)
            .and_then(|pid| self.projects.iter().find(|p| &p.id == pid))
            .map(|p| p.repo_path.clone())
        else {
            return;
        };
        match std::process::Command::new("git")
            .args(["worktree", "prune"])
            .current_dir(&repo_path)
            .output()
        {
            Ok(out) if !out.status.success() => eprintln!(
                "git worktree prune {}: {}",
                repo_path.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) => eprintln!("git worktree prune {}: {e}", repo_path.display()),
            Ok(_) => {}
        }
    }

    /// The wire view of a plan (spec §board.list): identity, state, project,
    /// model, timestamps, its stage docs (with open-comment counts), and the
    /// id of its active run if any (single-active-writer → at most one).
    fn plan_view(&self, plan_id: &str, active: &ActivePlan) -> Value {
        let project_id = self
            .entity_project
            .get(plan_id)
            .cloned()
            .unwrap_or_default();
        let project = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let active_run_id = self.runs.iter().find_map(|(id, run)| {
            (run.run.plan_id.as_ref().map(|p| &p.0) == Some(&plan_id.to_string())
                && !run.run.state.is_terminal())
            .then(|| id.clone())
        });
        json!({
            "plan_id": plan_id,
            "goal": active.plan.goal,
            "state": plan_state_str(&active.plan.state),
            "needs_attention": active.plan.state.needs_attention(),
            "summary": active.last_summary,
            "last_error": active.last_error,
            "project": project,
            "project_id": project_id,
            "base_branch": active.base_branch,
            "harness": self.harness,
            "model": active.model_choice.model,
            "effort": active.model_choice.effort,
            "active_run_id": active_run_id,
            "created_at": self.entity_created_at.get(plan_id),
            "updated_at": self.entity_updated_at.get(plan_id),
            "stages": active
                .stages
                .iter()
                .map(|doc| plan_stage_json(active, doc))
                .collect::<Vec<_>>(),
        })
    }

    /// The wire view of a run (spec §board.list): identity + plan link, state,
    /// branch/base, per-stage execution progress (with validation), model, and
    /// timestamps. The live diffstat rides along in `board.list`.
    fn run_view(&self, run_id: &str, active: &ActiveRun) -> Value {
        let project_id = self.entity_project.get(run_id).cloned().unwrap_or_default();
        let project = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        json!({
            "run_id": run_id,
            "plan_id": active.run.plan_id.as_ref().map(|p| p.0.clone()),
            "goal": active.run.goal,
            "state": run_state_str(&active.run.state),
            "needs_attention": active.run.state.needs_attention(),
            "branch": active.worktree.branch,
            "base_branch": active.worktree.base_branch,
            "base_sha": active.base_sha,
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
            "created_at": self.entity_created_at.get(run_id),
            "updated_at": self.entity_updated_at.get(run_id),
            "stages": active
                .stages
                .iter()
                .map(run_stage_json)
                .collect::<Vec<_>>(),
        })
    }

    /// A run's live diffstat for the `board.list` poll surface, cached for
    /// [`TASK_STAT_TTL`]. Terminal runs (worktree pruned or about to be) report
    /// null.
    fn run_stat(&mut self, run_id: &str) -> Value {
        let Some(active) = self.runs.get(run_id) else {
            return Value::Null;
        };
        if active.run.state.is_terminal() || !active.worktree.path.exists() {
            return Value::Null;
        }
        if let Some((computed_at, stat)) = self.run_stat_cache.get(run_id) {
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
        self.run_stat_cache.insert(
            run_id.to_string(),
            (std::time::Instant::now(), stat.clone()),
        );
        stat
    }

    // ---- the scripted QA agent ------------------------------------------------

    /// Simulate a plan session: write the two-stage plan docs + manifest into
    /// the disposable planning worktree and report `done(phase=plan)`, so the
    /// orchestrator ingests them into the canonical store exactly as a real
    /// harness would over MCP.
    fn qa_simulate_plan(&self, project_id: &str, active: &mut ActivePlan) -> Result<(), String> {
        let worktree = active
            .worktree
            .as_ref()
            .ok_or("QA plan: no planning worktree")?
            .path
            .clone();
        let goal = active.plan.goal.clone();
        write_in_dir(
            &worktree,
            ".build/plan/01-first-half.md",
            &format!("# Stage: First half\n\n1. Implement the first half of: {goal}\n"),
        )?;
        write_in_dir(
            &worktree,
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
        write_in_dir(&worktree, STAGES_MANIFEST_PATH, &manifest)?;
        let store = self.require_store()?;
        self.orch_for(project_id)?
            .on_plan_done(
                active,
                store,
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

    /// Simulate a per-stage plan-revision session: rewrite the stage doc in the
    /// planning worktree and resolve every open comment on the revised stage.
    fn qa_simulate_plan_stage_revise(
        &self,
        project_id: &str,
        active: &mut ActivePlan,
    ) -> Result<(), String> {
        let stage_id = active
            .revising_stage_id
            .clone()
            .ok_or("QA plan revise: no stage revision in flight")?;
        let worktree = active
            .worktree
            .as_ref()
            .ok_or("QA plan revise: no planning worktree")?
            .path
            .clone();
        let index = active.stage_doc_index(&stage_id)?;
        let stage_path = active.stages[index].path.clone();
        let mut contents = std::fs::read_to_string(worktree.join(&stage_path))
            .map_err(|e| format!("QA plan revise: could not read stage doc: {e}"))?;
        contents.push_str("\n(revised)\n");
        write_in_dir(&worktree, &stage_path, &contents)?;
        let resolutions: Vec<CommentResolution> = active
            .open_comments_for(&stage_id)
            .into_iter()
            .map(|c| CommentResolution {
                comment_id: c.id.clone(),
                response: "QA: addressed.".to_string(),
            })
            .collect();
        let store = self.require_store()?;
        self.orch_for(project_id)?
            .on_plan_done(
                active,
                store,
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

    /// Simulate a run's build agent: write the result file and report
    /// `done(phase=build)`.
    fn qa_simulate_build(
        &self,
        project_id: &str,
        active: &mut ActiveRun,
        plan_docs: &[StageDoc],
    ) -> Result<(), String> {
        let content = format!("Implemented: {}\n", active.run.goal);
        write_in_dir(&active.worktree.path, "result.txt", &content)?;
        self.orch_for(project_id)?
            .on_run_done(
                active,
                plan_docs,
                DoneReport {
                    phase: DonePhase::Build,
                    status: DoneStatus::Completed,
                    summary: format!("Built: {}", active.run.goal),
                    outputs: DoneOutputs::default(),
                },
            )
            .map_err(err)
    }

    /// Simulate one stage's build session and — since that hands off to a
    /// validation session — the validation too, so one call lands the stage on
    /// a verdict exactly as two real `done` reports would.
    fn qa_simulate_stage_build(
        &self,
        project_id: &str,
        active: &mut ActiveRun,
        plan_docs: &[StageDoc],
    ) -> Result<(), String> {
        let stage_id = active
            .current_stage_id
            .clone()
            .ok_or("QA stage build: no current stage")?;
        if active.stage_progress(&stage_id).map(|p| p.state) == Some(StageProgressState::Building) {
            let content = format!("Implemented stage {stage_id}: {}\n", active.run.goal);
            write_in_dir(
                &active.worktree.path,
                &format!("result-{stage_id}.txt"),
                &content,
            )?;
            self.orch_for(project_id)?
                .on_run_done(
                    active,
                    plan_docs,
                    DoneReport {
                        phase: DonePhase::Build,
                        status: DoneStatus::Completed,
                        summary: format!("Built stage {stage_id}"),
                        outputs: DoneOutputs::default(),
                    },
                )
                .map_err(err)?;
        }
        if active.stage_progress(&stage_id).map(|p| p.state) == Some(StageProgressState::Validating)
        {
            self.orch_for(project_id)?
                .on_run_done(
                    active,
                    plan_docs,
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

    /// Drive the QA harness while a run is `Building`: a fresh stage runs its
    /// build+validate; a post-review change or a quick/single-plan build runs
    /// the plain build. Bounded so a non-converging chain fails loudly.
    fn qa_drive_run(
        &self,
        project_id: &str,
        active: &mut ActiveRun,
        plan_docs: &[StageDoc],
    ) -> Result<(), String> {
        if !self.qa_agent {
            return Ok(());
        }
        let max_hops = plan_docs.len() + 2;
        for _ in 0..max_hops {
            if active.run.state != RunState::Building {
                return Ok(());
            }
            let mid_stage = active
                .current_stage_id
                .as_ref()
                .and_then(|sid| active.stage_progress(sid))
                .map(|p| {
                    matches!(
                        p.state,
                        StageProgressState::Building | StageProgressState::Validating
                    )
                })
                .unwrap_or(false);
            if !plan_docs.is_empty() && mid_stage {
                self.qa_simulate_stage_build(project_id, active, plan_docs)?;
            } else {
                self.qa_simulate_build(project_id, active, plan_docs)?;
            }
        }
        Err("QA run did not converge".to_string())
    }
}

/// The wire view of a plan stage doc: id/title/summary/path, its plan-side
/// review sub-state, and its open-comment count.
fn plan_stage_json(active: &ActivePlan, doc: &StageDoc) -> Value {
    let open_comments = active
        .comments
        .iter()
        .filter(|c| c.stage_id == doc.id && c.state == CommentState::Open)
        .count();
    json!({
        "id": doc.id,
        "title": doc.title,
        "summary": doc.summary,
        "path": doc.path,
        "state": stage_doc_state_str(&doc.state),
        "open_comments": open_comments,
    })
}

/// The wire view of a run stage's execution progress: id, sub-state, start sha,
/// and its validation report if any.
fn run_stage_json(progress: &StageProgress) -> Value {
    json!({
        "id": progress.stage_id,
        "state": run_stage_progress_str(&progress.state),
        "start_sha": progress.start_sha,
        "validation": progress.validation.as_ref().map(|v| json!({
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

/// Parse the optional `anchor` param of `plan.comment_add`: `null`/absent is a
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

/// The first stage `run.stage_dispatch` would currently accept for a run: the
/// earliest stage not yet validated-passed — provided its plan doc is
/// `Approved` and every earlier stage already passed validation on this run.
/// `None` when nothing is dispatchable right now (mirrors `dispatch_run_stage`).
fn dispatchable_next_run_stage(run: &ActiveRun, plan_docs: &[StageDoc]) -> Option<String> {
    let passed = |stage_id: &str| {
        run.stage_progress(stage_id).map(|p| p.state)
            == Some(StageProgressState::Validated { passed: true })
    };
    for (index, doc) in plan_docs.iter().enumerate() {
        if passed(&doc.id) {
            continue;
        }
        return (doc.state == StageDocState::Approved
            && plan_docs[..index].iter().all(|d| passed(&d.id)))
        .then(|| doc.id.clone());
    }
    None
}

/// The wire string for a plan state (snake_case, matching the SPA's buckets).
fn plan_state_str(state: &PlanState) -> String {
    match state {
        PlanState::Created => "created",
        PlanState::Drafting => "drafting",
        PlanState::PlanReview => "plan_review",
        PlanState::Approved => "approved",
        PlanState::Blocked => "blocked",
        PlanState::Failed => "failed",
        PlanState::IdleUnreported => "idle_unreported",
        PlanState::Interrupted => "interrupted",
        PlanState::Abandoned => "abandoned",
    }
    .to_string()
}

/// The wire string for a run state.
fn run_state_str(state: &RunState) -> String {
    match state {
        RunState::Created => "created",
        RunState::Building => "building",
        RunState::StageGate => "stage_gate",
        RunState::Review => "review",
        RunState::Blocked => "blocked",
        RunState::Failed => "failed",
        RunState::IdleUnreported => "idle_unreported",
        RunState::Interrupted => "interrupted",
        RunState::Merged => "merged",
        RunState::Abandoned => "abandoned",
        RunState::Archived => "archived",
    }
    .to_string()
}

/// The wire string for a plan-side stage doc state.
fn stage_doc_state_str(state: &StageDocState) -> String {
    match state {
        StageDocState::Planned => "planned",
        StageDocState::Approved => "approved",
    }
    .to_string()
}

/// The wire string for a run-side stage progress state.
fn run_stage_progress_str(state: &StageProgressState) -> String {
    match state {
        StageProgressState::Building => "building",
        StageProgressState::Built => "built",
        StageProgressState::Validating => "validating",
        StageProgressState::Validated { passed: true } => "validated_passed",
        StageProgressState::Validated { passed: false } => "validated_failed",
    }
    .to_string()
}

/// Write a file under `dir`, creating parent dirs — the QA agent's file writer.
fn write_in_dir(dir: &std::path::Path, rel: &str, contents: &str) -> Result<(), String> {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, contents).map_err(|e| e.to_string())
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
/// plus — for run scope — what `git.log` needs to mark commits ahead of base
/// and `git.commit` needs to invalidate afterwards.
struct GitScope {
    repo_path: std::path::PathBuf,
    /// Set for project scope: the project whose primary checkout this is,
    /// so mutations can invalidate its cached `primary_changes` summary.
    project_id: Option<String>,
    run: Option<GitScopeRun>,
}

struct GitScopeRun {
    run_id: String,
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
pub(crate) fn fenced_scope_path(
    root: &std::path::Path,
    path: &str,
) -> Result<std::path::PathBuf, String> {
    if !path.is_empty() && !crate::plan::is_worktree_contained_path(path) {
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
    // The agent screen belongs to whichever entity — plan or run — owns the
    // session; the id is opaque (plan-… / run-…).
    let entity_id = require_str(params, "id")?;
    // Grid defaults = the orchestrator's agent PTY size (40 rows × 120 cols).
    let cols = params.get("cols").and_then(Value::as_u64).unwrap_or(120) as u16;
    let rows = params.get("rows").and_then(Value::as_u64).unwrap_or(40) as u16;

    let mut guard = state.lock().unwrap();
    let s = &mut *guard;
    if !s.contains_entity(&entity_id) {
        return Err("unknown id".to_string());
    }
    let live_session = s.entity_subscribe_with_generation(&entity_id);

    let AppState {
        agent_screens,
        plans,
        runs,
        ..
    } = s;
    let agent = agent_screens
        .entry(entity_id.clone())
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
        // screen is left untouched. Resize via disjoint-field access so it
        // never conflicts with the `agent` borrow of `agent_screens`.
        if agent.screen.cols != cols || agent.screen.rows != rows {
            agent.screen.set_size(cols, rows);
            let size = PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            };
            if let Some(plan) = plans.get(&entity_id) {
                let _ = plan.session.resize(size);
            } else if let Some(run) = runs.get(&entity_id) {
                let _ = run.session.resize(size);
            }
        }
    }

    agent.screen.register(sender);
    let response = json!({
        "term_id": format!("agent:{entity_id}"),
        "live": agent.live,
        "snapshot": agent.screen.snapshot(),
        "cursor": agent.screen.total,
        "cols": agent.screen.cols,
        "rows": agent.screen.rows,
    });
    drop(guard);

    if let Some((generation, rx)) = pump {
        spawn_agent_pump(Arc::clone(state), entity_id, generation, rx);
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
    entity_id: String,
    generation: u64,
    mut rx: broadcast::Receiver<Vec<u8>>,
) {
    tokio::spawn(async move {
        let term_id = format!("agent:{entity_id}");
        {
            let mut s = state.lock().unwrap();
            let Some(agent) = s
                .agent_screens
                .get_mut(&entity_id)
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
                            .get_mut(&entity_id)
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
                            .get_mut(&entity_id)
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
                        .get_mut(&entity_id)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    // --- fs.tree / fs.read (spec §4) --------------------------------------------

    // --- primary-checkout surface (spec §5) -------------------------------------

    // ---- git.* (browser git GUI) -------------------------------------------

    // ---- git GUI v2: repo management (fetch/pull/push, branches, stash,
    // discard, merge-abort) ------------------------------------------------

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
    fn git_log_and_show_cap_oversized_commit_messages() {
        let (dir, repo) = init_repo();
        // A prompt-injected agent can craft a multi-MB message with one
        // `git commit -F`; the display strings must degrade, not ride the
        // poll past the relay frame cap.
        let subject = "s".repeat(1_048_576);
        let body = "b".repeat(2 * 1_048_576);
        let msg_file = dir.path().join("msg.txt");
        std::fs::write(&msg_file, format!("{subject}\n\n{body}")).unwrap();
        std::fs::write(repo.join("x.txt"), "x\n").unwrap();
        git_in_dir(&repo, &["add", "x.txt"]);
        git_in_dir(&repo, &["commit", "-q", "-F", msg_file.to_str().unwrap()]);
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        let log = state.handle(req(
            "git.log",
            json!({ "project_id": project_id, "limit": 1 }),
        ));
        assert_eq!(log["ok"], true, "{log:?}");
        let entry = &log["result"]["commits"][0];
        assert_eq!(entry["subject"].as_str().unwrap().len(), 512);
        let hash = entry["hash"].as_str().unwrap().to_string();

        let shown = state.handle(req(
            "git.show",
            json!({ "project_id": project_id, "hash": hash }),
        ));
        assert_eq!(shown["ok"], true, "{shown:?}");
        assert_eq!(shown["result"]["subject"].as_str().unwrap().len(), 512);
        assert_eq!(shown["result"]["body"].as_str().unwrap().len(), 65_536);
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
        assert_eq!(status["files_truncated"], false);
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
    fn git_status_decomposes_a_staged_rename_into_delete_plus_add() {
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();
        git_in_dir(&repo, &["mv", "README.md", "RENAMED.md"]);

        // Both sides of the rename appear, matching the patch (which has no
        // rename detection): a staged delete at the old path, a staged add at
        // the new one. index_status "R" never occurs.
        let res = state.handle(req("git.status", json!({ "project_id": project_id })));
        assert_eq!(res["ok"], true, "{res:?}");
        let old = file_entry(&res["result"], "README.md");
        assert_eq!(old["index_status"], "D");
        assert_eq!(old["staged"], "full");
        let new = file_entry(&res["result"], "RENAMED.md");
        assert_eq!(new["index_status"], "A");
        assert_eq!(new["staged"], "full");

        // Unstaging both paths fully restores the index: the old path is back
        // (its only change is the worktree deletion), the new path untracked.
        let unstaged = state.handle(req(
            "git.unstage",
            json!({ "project_id": project_id, "paths": ["README.md", "RENAMED.md"] }),
        ));
        assert_eq!(unstaged["ok"], true, "{unstaged:?}");
        let old = file_entry(&unstaged["result"], "README.md");
        assert_eq!(old["staged"], "none");
        assert_eq!(old["worktree_status"], "D");
        let new = file_entry(&unstaged["result"], "RENAMED.md");
        assert_eq!(new["staged"], "none");
        assert_eq!(new["index_status"], "?");
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
    fn git_stage_treats_paths_as_literals_never_globs() {
        // A file literally named "*" alongside an innocent bystander: staging
        // "*" must stage only that file, never glob-expand.
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();
        std::fs::write(repo.join("*"), "star\n").unwrap();
        std::fs::write(repo.join("bystander.txt"), "hi\n").unwrap();

        let staged = state.handle(req(
            "git.stage",
            json!({ "project_id": project_id, "paths": ["*"] }),
        ));
        assert_eq!(staged["ok"], true, "{staged:?}");
        assert_eq!(file_entry(&staged["result"], "*")["staged"], "full");
        assert_eq!(
            file_entry(&staged["result"], "bystander.txt")["staged"],
            "none"
        );

        // Unstage is literal too.
        let unstaged = state.handle(req(
            "git.unstage",
            json!({ "project_id": project_id, "paths": ["*"] }),
        ));
        assert_eq!(unstaged["ok"], true, "{unstaged:?}");
        assert_eq!(file_entry(&unstaged["result"], "*")["staged"], "none");

        // Without a file actually named "*", the request errors instead of
        // matching everything.
        let (dir2, repo2) = init_repo();
        let mut state2 = git_gui_state(&dir2, &repo2);
        let project_id2 = state2.projects[0].id.clone();
        std::fs::write(repo2.join("bystander.txt"), "hi\n").unwrap();

        let res = state2.handle(req(
            "git.stage",
            json!({ "project_id": project_id2, "paths": ["*"] }),
        ));
        assert_eq!(res["ok"], false, "{res:?}");
        let status = state2.handle(req("git.status", json!({ "project_id": project_id2 })));
        assert_eq!(
            file_entry(&status["result"], "bystander.txt")["staged"],
            "none"
        );
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
    /// Run git without asserting success — for setting up conflict/rebase
    /// states whose whole point is a non-zero exit.
    fn git_try(dir: &std::path::Path, args: &[&str]) {
        let _ = Command::new("git").args(args).current_dir(dir).status();
    }

    /// A working repo wired to a bare "origin" it already tracks (main →
    /// origin/main, ahead 0 / behind 0).
    fn init_repo_with_origin() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let (dir, repo) = init_repo();
        let origin = dir.path().join("origin.git");
        git_in_dir(
            dir.path(),
            &[
                "clone",
                "--bare",
                repo.to_str().unwrap(),
                origin.to_str().unwrap(),
            ],
        );
        git_in_dir(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        git_in_dir(&repo, &["fetch", "origin"]);
        git_in_dir(&repo, &["branch", "--set-upstream-to=origin/main", "main"]);
        (dir, repo, origin)
    }

    /// A second working checkout of `origin`, standing in for another dev.
    fn clone_working(origin: &std::path::Path, dest: &std::path::Path) {
        git_in_dir(
            dest.parent().unwrap(),
            &["clone", origin.to_str().unwrap(), dest.to_str().unwrap()],
        );
        git_in_dir(dest, &["config", "user.email", "o@build.ing"]);
        git_in_dir(dest, &["config", "user.name", "O"]);
    }

    #[test]
    fn git_status_carries_the_repo_management_fields() {
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        let res = state.handle(req("git.status", json!({ "project_id": project_id })));
        assert_eq!(res["ok"], true, "{res:?}");
        let result = &res["result"];
        assert_eq!(result["repo_state"], "clean");
        // No remote configured → upstream/ahead/behind are null, not NaN.
        assert!(result["upstream"].is_null());
        assert!(result["ahead"].is_null());
        assert!(result["behind"].is_null());
        assert_eq!(result["stash_count"], 0);
    }

    #[test]
    fn git_fetch_pull_push_round_trip_through_a_bare_origin() {
        let (dir, repo, origin) = init_repo_with_origin();
        let other = dir.path().join("other");
        clone_working(&origin, &other);
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        // Another dev pushes a commit to origin.
        std::fs::write(other.join("remote.txt"), "remote\n").unwrap();
        git_in_dir(&other, &["add", "remote.txt"]);
        git_in_dir(&other, &["commit", "-m", "remote work"]);
        git_in_dir(&other, &["push", "origin", "main"]);

        // git.fetch updates the tracking ref: we are now behind by one.
        let fetched = state.handle(req("git.fetch", json!({ "project_id": project_id })));
        assert_eq!(fetched["ok"], true, "{fetched:?}");
        assert_eq!(fetched["result"]["upstream"], "origin/main");
        assert_eq!(fetched["result"]["behind"], 1);
        assert_eq!(fetched["result"]["ahead"], 0);

        // git.pull (ff) fast-forwards the branch onto the remote commit.
        let pulled = state.handle(req("git.pull", json!({ "project_id": project_id })));
        assert_eq!(pulled["ok"], true, "{pulled:?}");
        assert_eq!(pulled["result"]["behind"], 0);
        assert!(repo.join("remote.txt").exists());

        // A local commit, then git.push publishes it to origin.
        std::fs::write(repo.join("local.txt"), "local\n").unwrap();
        git_in_dir(&repo, &["add", "local.txt"]);
        git_in_dir(&repo, &["commit", "-m", "local work"]);
        let ahead = state.handle(req("git.status", json!({ "project_id": project_id })));
        assert_eq!(ahead["result"]["ahead"], 1);

        let pushed = state.handle(req("git.push", json!({ "project_id": project_id })));
        assert_eq!(pushed["ok"], true, "{pushed:?}");
        assert_eq!(pushed["result"]["ahead"], 0);
        assert_eq!(pushed["result"]["behind"], 0);

        // The other checkout can now fetch our commit — proof it reached origin.
        git_in_dir(&other, &["fetch", "origin"]);
        let log = Command::new("git")
            .args(["log", "--oneline", "origin/main"])
            .current_dir(&other)
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&log.stdout).contains("local work"));
    }

    #[test]
    fn git_push_sets_the_upstream_on_the_first_push() {
        let (dir, repo) = init_repo();
        let origin = dir.path().join("origin.git");
        git_in_dir(
            dir.path(),
            &["init", "--bare", "-b", "main", origin.to_str().unwrap()],
        );
        git_in_dir(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        // No upstream yet.
        let before = state.handle(req("git.status", json!({ "project_id": project_id })));
        assert!(before["result"]["upstream"].is_null());

        let pushed = state.handle(req("git.push", json!({ "project_id": project_id })));
        assert_eq!(pushed["ok"], true, "{pushed:?}");
        assert_eq!(pushed["result"]["upstream"], "origin/main");
        assert_eq!(pushed["result"]["ahead"], 0);
    }

    #[test]
    fn git_push_force_uses_force_with_lease_after_a_rewrite() {
        let (dir, repo, _origin) = init_repo_with_origin();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        // Publish a commit, then rewrite it so local diverges from origin.
        std::fs::write(repo.join("x.txt"), "one\n").unwrap();
        git_in_dir(&repo, &["add", "x.txt"]);
        git_in_dir(&repo, &["commit", "-m", "first"]);
        assert_eq!(
            state.handle(req("git.push", json!({ "project_id": project_id })))["ok"],
            true
        );
        std::fs::write(repo.join("x.txt"), "two\n").unwrap();
        git_in_dir(&repo, &["commit", "-a", "--amend", "-m", "rewritten"]);

        // A plain push is rejected (non-fast-forward); force-with-lease wins.
        let plain = state.handle(req("git.push", json!({ "project_id": project_id })));
        assert_eq!(plain["ok"], false, "{plain:?}");
        let forced = state.handle(req(
            "git.push",
            json!({ "project_id": project_id, "force": true }),
        ));
        assert_eq!(forced["ok"], true, "{forced:?}");
    }

    #[test]
    fn git_push_refuses_a_detached_head() {
        let (dir, repo, _origin) = init_repo_with_origin();
        git_in_dir(&repo, &["checkout", "--detach", "HEAD"]);
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        let res = state.handle(req("git.push", json!({ "project_id": project_id })));
        assert_eq!(res["ok"], false, "{res:?}");
        assert_eq!(res["error"], "cannot push a detached HEAD");
    }

    #[test]
    fn git_pull_ff_only_refuses_divergent_history() {
        let (dir, repo, origin) = init_repo_with_origin();
        let other = dir.path().join("other");
        clone_working(&origin, &other);
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        std::fs::write(other.join("theirs.txt"), "theirs\n").unwrap();
        git_in_dir(&other, &["add", "theirs.txt"]);
        git_in_dir(&other, &["commit", "-m", "theirs"]);
        git_in_dir(&other, &["push", "origin", "main"]);

        std::fs::write(repo.join("mine.txt"), "mine\n").unwrap();
        git_in_dir(&repo, &["add", "mine.txt"]);
        git_in_dir(&repo, &["commit", "-m", "mine"]);

        assert_eq!(
            state.handle(req("git.fetch", json!({ "project_id": project_id })))["ok"],
            true
        );
        let pulled = state.handle(req("git.pull", json!({ "project_id": project_id })));
        assert_eq!(pulled["ok"], false, "{pulled:?}");
        assert!(
            pulled["error"].as_str().unwrap().contains("fast-forward")
                || pulled["error"].as_str().unwrap().contains("fast forward"),
            "{pulled:?}"
        );
    }

    #[test]
    fn git_pull_conflict_leaves_a_visible_merging_state() {
        let (dir, repo, origin) = init_repo_with_origin();
        let other = dir.path().join("other");
        clone_working(&origin, &other);
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        // Both sides edit README differently; the other side lands first.
        std::fs::write(other.join("README.md"), "# theirs\n").unwrap();
        git_in_dir(&other, &["commit", "-am", "theirs"]);
        git_in_dir(&other, &["push", "origin", "main"]);
        std::fs::write(repo.join("README.md"), "# mine\n").unwrap();
        git_in_dir(&repo, &["commit", "-am", "mine"]);
        assert_eq!(
            state.handle(req("git.fetch", json!({ "project_id": project_id })))["ok"],
            true
        );

        let pulled = state.handle(req(
            "git.pull",
            json!({ "project_id": project_id, "mode": "merge" }),
        ));
        assert_eq!(pulled["ok"], false, "{pulled:?}");

        // The conflict is legible in the very next status: merging + a U file.
        let status = state.handle(req("git.status", json!({ "project_id": project_id })));
        assert_eq!(status["result"]["repo_state"], "merging");
        let readme = file_entry(&status["result"], "README.md");
        assert_eq!(readme["index_status"], "U");
        assert_eq!(readme["worktree_status"], "U");
    }

    #[test]
    fn git_branches_lists_locals_current_first() {
        let (dir, repo) = init_repo();
        git_in_dir(&repo, &["branch", "feature-a"]);
        git_in_dir(&repo, &["branch", "feature-b"]);
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        let res = state.handle(req("git.branches", json!({ "project_id": project_id })));
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["current"], "main");
        let branches = res["result"]["branches"].as_array().unwrap();
        assert_eq!(branches.len(), 3);
        // Current branch sorts first.
        assert_eq!(branches[0]["name"], "main");
        assert_eq!(branches[0]["is_current"], true);
        assert_eq!(branches[0]["ahead"], 0);
        assert_eq!(branches[0]["behind"], 0);
        assert!(branches[0]["upstream"].is_null());
        assert!(branches.iter().any(|b| b["name"] == "feature-a"));
    }

    #[test]
    fn git_checkout_switches_creates_and_validates() {
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        // Create a new branch and land on it.
        let created = state.handle(req(
            "git.checkout",
            json!({ "project_id": project_id, "branch": "feature-x", "create": true }),
        ));
        assert_eq!(created["ok"], true, "{created:?}");
        assert_eq!(created["result"]["branch"], "feature-x");

        // Switch back to an existing branch.
        let switched = state.handle(req(
            "git.checkout",
            json!({ "project_id": project_id, "branch": "main" }),
        ));
        assert_eq!(switched["result"]["branch"], "main");

        // Invalid ref names are refused before any git call.
        for bad in ["--force", "bad name", "has..dots", ""] {
            let res = state.handle(req(
                "git.checkout",
                json!({ "project_id": project_id, "branch": bad, "create": true }),
            ));
            assert_eq!(res["ok"], false, "{bad:?} -> {res:?}");
            assert!(
                res["error"]
                    .as_str()
                    .unwrap()
                    .starts_with("invalid branch name"),
                "{res:?}"
            );
        }
    }

    #[test]
    fn git_checkout_refuses_while_a_merge_is_in_progress() {
        let (dir, repo) = init_repo();
        // Manufacture a conflicting merge so the repo is left mid-merge.
        git_in_dir(&repo, &["checkout", "-b", "topic"]);
        std::fs::write(repo.join("README.md"), "# topic\n").unwrap();
        git_in_dir(&repo, &["commit", "-am", "topic"]);
        git_in_dir(&repo, &["checkout", "main"]);
        std::fs::write(repo.join("README.md"), "# mainline\n").unwrap();
        git_in_dir(&repo, &["commit", "-am", "mainline"]);
        git_try(&repo, &["merge", "topic"]);
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        let res = state.handle(req(
            "git.checkout",
            json!({ "project_id": project_id, "branch": "topic" }),
        ));
        assert_eq!(res["ok"], false, "{res:?}");
        assert_eq!(res["error"], "finish or abort the in-progress merge first");
    }

    #[test]
    fn git_branch_delete_removes_and_force_deletes() {
        let (dir, repo) = init_repo();
        git_in_dir(&repo, &["branch", "merged-branch"]);
        // An unmerged branch: a commit main cannot reach.
        git_in_dir(&repo, &["checkout", "-b", "unmerged"]);
        std::fs::write(repo.join("u.txt"), "u\n").unwrap();
        git_in_dir(&repo, &["add", "u.txt"]);
        git_in_dir(&repo, &["commit", "-m", "unmerged work"]);
        git_in_dir(&repo, &["checkout", "main"]);
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        // Deleting the current branch is git's error, passed through.
        let current = state.handle(req(
            "git.branch_delete",
            json!({ "project_id": project_id, "branch": "main" }),
        ));
        assert_eq!(current["ok"], false, "{current:?}");

        // A merged branch deletes with -d and the fresh list comes back.
        let ok = state.handle(req(
            "git.branch_delete",
            json!({ "project_id": project_id, "branch": "merged-branch" }),
        ));
        assert_eq!(ok["ok"], true, "{ok:?}");
        assert!(!ok["result"]["branches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b["name"] == "merged-branch"));

        // An unmerged branch refuses -d, then yields to force (-D).
        let refused = state.handle(req(
            "git.branch_delete",
            json!({ "project_id": project_id, "branch": "unmerged" }),
        ));
        assert_eq!(refused["ok"], false, "{refused:?}");
        let forced = state.handle(req(
            "git.branch_delete",
            json!({ "project_id": project_id, "branch": "unmerged", "force": true }),
        ));
        assert_eq!(forced["ok"], true, "{forced:?}");
    }

    #[test]
    fn git_stash_and_pop_round_trip() {
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        std::fs::write(repo.join("README.md"), "# edited\n").unwrap();
        std::fs::write(repo.join("fresh.txt"), "fresh\n").unwrap();

        // Stash includes the untracked file (-u), leaving a clean tree.
        let stashed = state.handle(req("git.stash", json!({ "project_id": project_id })));
        assert_eq!(stashed["ok"], true, "{stashed:?}");
        assert_eq!(stashed["result"]["stash_count"], 1);
        assert!(stashed["result"]["files"].as_array().unwrap().is_empty());
        assert!(!repo.join("fresh.txt").exists());

        // Pop restores both, and the stash stack is empty again.
        let popped = state.handle(req("git.stash_pop", json!({ "project_id": project_id })));
        assert_eq!(popped["ok"], true, "{popped:?}");
        assert_eq!(popped["result"]["stash_count"], 0);
        assert!(repo.join("fresh.txt").exists());

        // Popping an empty stack is git's error, passed through.
        let empty = state.handle(req("git.stash_pop", json!({ "project_id": project_id })));
        assert_eq!(empty["ok"], false, "{empty:?}");
    }

    #[test]
    fn git_discard_reverts_tracked_and_deletes_untracked() {
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        // A tracked edit (staged) and a fresh untracked file.
        std::fs::write(repo.join("README.md"), "# tampered\n").unwrap();
        git_in_dir(&repo, &["add", "README.md"]);
        std::fs::write(repo.join("junk.txt"), "junk\n").unwrap();

        let res = state.handle(req(
            "git.discard",
            json!({ "project_id": project_id, "paths": ["README.md", "junk.txt"] }),
        ));
        assert_eq!(res["ok"], true, "{res:?}");

        // Tracked file is back to its committed content, in both index and tree.
        assert_eq!(
            std::fs::read_to_string(repo.join("README.md")).unwrap(),
            "# project\n"
        );
        assert!(!has_file_entry(&res["result"], "README.md"));
        // Untracked file is gone from disk.
        assert!(!repo.join("junk.txt").exists());
        assert!(!has_file_entry(&res["result"], "junk.txt"));
    }

    #[test]
    fn git_discard_rejects_traversal_and_symlink_escapes() {
        let (dir, repo) = init_repo();
        // A secret outside the worktree, and an untracked symlink pointing at it.
        let secret = dir.path().join("secret.txt");
        std::fs::write(&secret, "top secret\n").unwrap();
        std::os::unix::fs::symlink(&secret, repo.join("leak")).unwrap();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();

        // Lexical traversal is refused before any git call.
        let traversal = state.handle(req(
            "git.discard",
            json!({ "project_id": project_id, "paths": ["../secret.txt"] }),
        ));
        assert_eq!(traversal["ok"], false, "{traversal:?}");

        // The symlink's components look Normal, so only the canonical fence
        // catches it — and the outside secret must survive.
        let symlink = state.handle(req(
            "git.discard",
            json!({ "project_id": project_id, "paths": ["leak"] }),
        ));
        assert_eq!(symlink["ok"], false, "{symlink:?}");
        assert!(
            secret.exists(),
            "the fence must not delete outside the worktree"
        );
    }

    #[test]
    fn git_merge_abort_handles_each_repo_state() {
        // Clean: nothing to abort.
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let project_id = state.projects[0].id.clone();
        let clean = state.handle(req("git.merge_abort", json!({ "project_id": project_id })));
        assert_eq!(clean["ok"], false, "{clean:?}");
        assert_eq!(clean["error"], "no abortable operation in progress");

        // Merging: abort returns to a clean state.
        git_in_dir(&repo, &["checkout", "-b", "topic"]);
        std::fs::write(repo.join("README.md"), "# topic\n").unwrap();
        git_in_dir(&repo, &["commit", "-am", "topic"]);
        git_in_dir(&repo, &["checkout", "main"]);
        std::fs::write(repo.join("README.md"), "# mainline\n").unwrap();
        git_in_dir(&repo, &["commit", "-am", "mainline"]);
        git_try(&repo, &["merge", "topic"]);
        let aborted = state.handle(req("git.merge_abort", json!({ "project_id": project_id })));
        assert_eq!(aborted["ok"], true, "{aborted:?}");
        assert_eq!(aborted["result"]["repo_state"], "clean");

        // Rebasing: a conflicting rebase leaves a rebasing state to abort.
        git_in_dir(&repo, &["checkout", "topic"]);
        git_try(&repo, &["rebase", "main"]);
        let status = state.handle(req("git.status", json!({ "project_id": project_id })));
        assert_eq!(status["result"]["repo_state"], "rebasing");
        let rebase_aborted =
            state.handle(req("git.merge_abort", json!({ "project_id": project_id })));
        assert_eq!(rebase_aborted["ok"], true, "{rebase_aborted:?}");
        assert_eq!(rebase_aborted["result"]["repo_state"], "clean");
    }

    // ==== Plan/Run split: new-protocol coverage ================================

    /// A QA state with a durable store — plans keep their canonical docs there
    /// and `run.create` reads/writes through it, so every lifecycle test needs
    /// one.
    fn qa_state(repo: &std::path::Path, dir: &std::path::Path) -> AppState {
        AppState::new(
            repo.to_path_buf(),
            dir.join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_task_store(dir.join("store"))
        .unwrap()
    }

    fn plan_id_of(res: &Value) -> String {
        res["result"]["plan_id"]
            .as_str()
            .unwrap_or_else(|| panic!("no plan_id: {res:?}"))
            .to_string()
    }

    fn run_id_of(res: &Value) -> String {
        res["result"]["run_id"]
            .as_str()
            .unwrap_or_else(|| panic!("no run_id: {res:?}"))
            .to_string()
    }

    #[test]
    fn plan_create_reaches_review_with_two_stages() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let res = state.handle(req("plan.create", json!({ "goal": "add a greeting" })));
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["state"], "plan_review");
        assert_eq!(res["result"]["stages"].as_array().unwrap().len(), 2);
        assert!(res["result"]["created_at"]
            .as_str()
            .is_some_and(|s| !s.is_empty()));
        let plan_id = plan_id_of(&res);

        // Each stage doc is readable from the canonical store, never a worktree.
        let doc = state.handle(req(
            "plan.stage_doc",
            json!({ "plan_id": plan_id, "stage_id": "first-half" }),
        ));
        assert_eq!(doc["ok"], true, "{doc:?}");
        assert!(doc["result"]["contents"]
            .as_str()
            .unwrap()
            .contains("add a greeting"));
    }

    #[test]
    fn full_multi_stage_lifecycle_plan_then_run() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());

        let plan = state.handle(req("plan.create", json!({ "goal": "add a greeting" })));
        let plan_id = plan_id_of(&plan);
        let approved = state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
        assert_eq!(approved["result"]["state"], "approved", "{approved:?}");
        state.handle(req(
            "plan.stage_approve",
            json!({ "plan_id": plan_id, "stage_id": "first-half" }),
        ));

        // Implement it: stage 1 auto-dispatches, QA drives it to the stage gate.
        let run = state.handle(req("run.create", json!({ "plan_id": plan_id })));
        assert_eq!(run["ok"], true, "{run:?}");
        assert_eq!(run["result"]["state"], "stage_gate", "{run:?}");
        assert_eq!(run["result"]["plan_id"], json!(plan_id));
        let run_id = run_id_of(&run);
        assert_eq!(run["result"]["stages"][0]["state"], "validated_passed");

        // Approve stage 2's doc, then dispatch it → the final validation opens review.
        state.handle(req(
            "plan.stage_approve",
            json!({ "plan_id": plan_id, "stage_id": "second-half" }),
        ));
        let s2 = state.handle(req(
            "run.stage_dispatch",
            json!({ "run_id": run_id, "stage_id": "second-half" }),
        ));
        assert_eq!(s2["result"]["state"], "review", "{s2:?}");

        let diff = state.handle(req("run.diff", json!({ "run_id": run_id })));
        let files: Vec<String> = diff["result"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap().to_string())
            .collect();
        assert!(
            files.contains(&"result-first-half.txt".to_string()),
            "{files:?}"
        );
        assert!(
            files.contains(&"result-second-half.txt".to_string()),
            "{files:?}"
        );

        let merged = state.handle(req(
            "run.git_action",
            json!({ "run_id": run_id, "action": "merge" }),
        ));
        assert_eq!(merged["result"]["state"], "merged", "{merged:?}");
        assert!(repo.join("result-first-half.txt").exists());
        assert!(repo.join("result-second-half.txt").exists());
    }

    #[test]
    fn quick_run_builds_reviews_and_merges_with_a_cached_diffstat() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let res = state.handle(req("run.create", json!({ "goal": "quick change" })));
        assert_eq!(res["result"]["state"], "review", "{res:?}");
        assert_eq!(res["result"]["plan_id"], Value::Null);
        let run_id = run_id_of(&res);

        let entry = |res: &Value| {
            res["result"]["runs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["run_id"] == json!(run_id.clone()))
                .unwrap()
                .clone()
        };
        let t = entry(&state.handle(req("board.list", json!({}))));
        assert!(t["stat"]["files_changed"].as_u64().unwrap() >= 1, "{t:?}");
        // served from cache on the next poll (identical).
        let t2 = entry(&state.handle(req("board.list", json!({}))));
        assert_eq!(t["stat"], t2["stat"]);

        state.handle(req(
            "run.git_action",
            json!({ "run_id": run_id, "action": "commit" }),
        ));
        assert!(!repo.join("result.txt").exists(), "commit keeps, no merge");

        let merged = state.handle(req(
            "run.git_action",
            json!({ "run_id": run_id, "action": "merge" }),
        ));
        assert_eq!(merged["result"]["state"], "merged");
        assert!(repo.join("result.txt").exists());
        let t3 = entry(&state.handle(req("board.list", json!({}))));
        assert!(t3["stat"].is_null(), "merged run has no worktree: {t3:?}");
    }

    #[test]
    fn board_list_carries_plans_runs_and_ride_alongs() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        state.handle(req("plan.create", json!({ "goal": "a plan" })));
        state.handle(req("run.create", json!({ "goal": "a quick run" })));
        let board = state.handle(req("board.list", json!({})));
        let r = &board["result"];
        assert_eq!(r["plans"].as_array().unwrap().len(), 1);
        assert_eq!(r["runs"].as_array().unwrap().len(), 1);
        assert!(r["external_worktrees"].is_array());
        assert!(r["primary_changes"].is_array());
    }

    #[test]
    fn deleting_a_run_worktree_archives_it_and_plan_docs_survive() {
        let (dir, repo) = init_repo();
        let plan_id;
        let run_id;
        let doc_before;
        {
            let mut state = qa_state(&repo, dir.path());
            let plan = state.handle(req("plan.create", json!({ "goal": "doomed run" })));
            plan_id = plan_id_of(&plan);
            state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
            state.handle(req(
                "plan.stage_approve",
                json!({ "plan_id": plan_id, "stage_id": "first-half" }),
            ));
            let doc = state.handle(req(
                "plan.stage_doc",
                json!({ "plan_id": plan_id, "stage_id": "first-half" }),
            ));
            doc_before = doc["result"]["contents"].as_str().unwrap().to_string();

            let run = state.handle(req("run.create", json!({ "plan_id": plan_id })));
            run_id = run_id_of(&run);
            let worktree = state.runs.get(&run_id).unwrap().worktree.path.clone();
            std::fs::remove_dir_all(&worktree).unwrap();

            // The next board poll retires the run to archived history.
            let board = state.handle(req("board.list", json!({})));
            let entry = board["result"]["runs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["run_id"] == json!(run_id.clone()))
                .unwrap()
                .clone();
            assert_eq!(entry["state"], "archived", "{entry:?}");
            assert_eq!(entry["needs_attention"], false);

            // The plan and its docs are untouched — they were never in the run.
            let doc = state.handle(req(
                "plan.stage_doc",
                json!({ "plan_id": plan_id, "stage_id": "first-half" }),
            ));
            assert_eq!(doc["result"]["contents"].as_str().unwrap(), doc_before);
        } // daemon dies

        let mut reloaded = qa_state(&repo, dir.path());
        let got = reloaded.handle(req("run.get", json!({ "run_id": run_id })));
        assert_eq!(got["result"]["state"], "archived", "{got:?}");
        let doc = reloaded.handle(req(
            "plan.stage_doc",
            json!({ "plan_id": plan_id, "stage_id": "first-half" }),
        ));
        assert_eq!(doc["result"]["contents"].as_str().unwrap(), doc_before);
        // An archived run can be cleared off the board.
        let deleted = reloaded.handle(req("run.delete", json!({ "run_id": run_id })));
        assert_eq!(deleted["ok"], true, "{deleted:?}");
    }

    #[test]
    fn single_active_writer_rejects_a_second_run_of_one_plan() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let plan = state.handle(req("plan.create", json!({ "goal": "one writer" })));
        let plan_id = plan_id_of(&plan);
        state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
        state.handle(req(
            "plan.stage_approve",
            json!({ "plan_id": plan_id, "stage_id": "first-half" }),
        ));
        let first = state.handle(req("run.create", json!({ "plan_id": plan_id })));
        assert_eq!(first["ok"], true, "{first:?}");
        let second = state.handle(req("run.create", json!({ "plan_id": plan_id })));
        assert_eq!(second["ok"], false, "{second:?}");
        assert!(
            second["error"]
                .as_str()
                .unwrap()
                .contains("single-active-writer"),
            "{second:?}"
        );
    }

    #[test]
    fn plan_comments_crud_and_stage_send_notes_resolve() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let plan = state.handle(req("plan.create", json!({ "goal": "comment me" })));
        let plan_id = plan_id_of(&plan);

        let general = state.handle(req(
            "plan.comment_add",
            json!({ "plan_id": plan_id, "stage_id": "first-half", "body": "split further" }),
        ));
        assert_eq!(
            general["result"]["comment"]["anchor"],
            Value::Null,
            "{general:?}"
        );
        let anchored = state.handle(req(
            "plan.comment_add",
            json!({
                "plan_id": plan_id, "stage_id": "first-half", "body": "use a timestamp",
                "anchor": { "heading_path": ["Stage: First half"], "snippet": "the first half" },
            }),
        ));
        let anchored_id = anchored["result"]["comment"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let stages = state.handle(req("plan.stages", json!({ "plan_id": plan_id })));
        assert_eq!(stages["result"]["stages"][0]["open_comments"], 2);

        let upd = state.handle(req(
            "plan.stage_send_notes",
            json!({ "plan_id": plan_id, "stage_id": "first-half" }),
        ));
        assert_eq!(upd["result"]["state"], "plan_review", "{upd:?}");
        let stages = state.handle(req("plan.stages", json!({ "plan_id": plan_id })));
        let first = stages["result"]["stages"][0].clone();
        assert_eq!(first["state"], "planned");
        assert_eq!(first["open_comments"], 0);
        assert!(first["comments"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["state"] == "addressed" && c["agent_reply"] == "QA: addressed."));
        let doc = state.handle(req(
            "plan.stage_doc",
            json!({ "plan_id": plan_id, "stage_id": "first-half" }),
        ));
        assert!(doc["result"]["contents"]
            .as_str()
            .unwrap()
            .contains("(revised)"));

        // No open comments left → sending notes again errors.
        let bad = state.handle(req(
            "plan.stage_send_notes",
            json!({ "plan_id": plan_id, "stage_id": "first-half" }),
        ));
        assert!(
            bad["error"].as_str().unwrap().contains("no open comments"),
            "{bad:?}"
        );
        // Deleting an already-addressed comment is rejected.
        let del = state.handle(req(
            "plan.comment_delete",
            json!({ "plan_id": plan_id, "comment_id": anchored_id }),
        ));
        assert_eq!(del["error"], "only open comments can be deleted");
    }

    #[test]
    fn multi_stage_run_gate_rejections() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let plan = state.handle(req("plan.create", json!({ "goal": "gate this" })));
        let plan_id = plan_id_of(&plan);
        state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
        state.handle(req(
            "plan.stage_approve",
            json!({ "plan_id": plan_id, "stage_id": "first-half" }),
        ));
        let run = state.handle(req("run.create", json!({ "plan_id": plan_id })));
        let run_id = run_id_of(&run);
        assert_eq!(run["result"]["state"], "stage_gate");

        // Unknown stage id.
        let unknown = state.handle(req(
            "run.stage_dispatch",
            json!({ "run_id": run_id, "stage_id": "no-such" }),
        ));
        assert!(
            unknown["error"].as_str().unwrap().contains("no-such"),
            "{unknown:?}"
        );

        // Stage 2 not yet approved → rejected.
        let unapproved = state.handle(req(
            "run.stage_dispatch",
            json!({ "run_id": run_id, "stage_id": "second-half" }),
        ));
        assert!(
            unapproved["error"]
                .as_str()
                .unwrap()
                .contains("not approved"),
            "{unapproved:?}"
        );
    }

    #[test]
    fn set_auto_advance_runs_every_stage_to_review() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let plan = state.handle(req("plan.create", json!({ "goal": "run all" })));
        let plan_id = plan_id_of(&plan);
        state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
        state.handle(req(
            "plan.stage_approve",
            json!({ "plan_id": plan_id, "stage_id": "first-half" }),
        ));
        state.handle(req(
            "plan.stage_approve",
            json!({ "plan_id": plan_id, "stage_id": "second-half" }),
        ));
        let run = state.handle(req("run.create", json!({ "plan_id": plan_id })));
        let run_id = run_id_of(&run);
        assert_eq!(run["result"]["state"], "stage_gate");

        let armed = state.handle(req(
            "run.set_auto_advance",
            json!({ "run_id": run_id, "enabled": true }),
        ));
        assert_eq!(armed["result"]["state"], "review", "{armed:?}");
        assert_eq!(armed["result"]["auto_advance"], true);
        assert!(armed["result"]["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["state"] == "validated_passed"));
    }

    #[test]
    fn run_request_changes_reruns_building_from_review() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let run = state.handle(req("run.create", json!({ "goal": "do work" })));
        let run_id = run_id_of(&run);
        assert_eq!(run["result"]["state"], "review");
        let rc = state.handle(req(
            "run.request_changes",
            json!({ "run_id": run_id, "comments": "rename the symbol" }),
        ));
        assert_eq!(rc["result"]["state"], "review", "{rc:?}");
        let bad = state.handle(req("run.request_changes", json!({ "run_id": run_id })));
        assert!(
            bad["error"].as_str().unwrap().contains("comments"),
            "{bad:?}"
        );
    }

    #[test]
    fn plan_message_rejects_the_review_gate_and_unknown_plans() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let plan = state.handle(req("plan.create", json!({ "goal": "at the gate" })));
        let plan_id = plan_id_of(&plan);
        let gated = state.handle(req(
            "plan.message",
            json!({ "plan_id": plan_id, "message": "hi" }),
        ));
        assert!(
            gated["error"].as_str().unwrap().contains("review gate"),
            "{gated:?}"
        );
        let unknown = state.handle(req(
            "plan.message",
            json!({ "plan_id": "plan-nope", "message": "hi" }),
        ));
        assert_eq!(unknown["ok"], false);
    }

    #[test]
    fn plan_delete_is_abandoned_only() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let plan = state.handle(req("plan.create", json!({ "goal": "kill me" })));
        let plan_id = plan_id_of(&plan);
        let live = state.handle(req("plan.delete", json!({ "plan_id": plan_id })));
        assert!(
            live["error"].as_str().unwrap().contains("abandoned"),
            "{live:?}"
        );
        state.handle(req("plan.abandon", json!({ "plan_id": plan_id })));
        let gone = state.handle(req("plan.delete", json!({ "plan_id": plan_id })));
        assert_eq!(gone["ok"], true, "{gone:?}");
        assert_eq!(
            state.handle(req("plan.get", json!({ "plan_id": plan_id })))["ok"],
            false
        );
    }

    #[test]
    fn on_agent_done_routes_by_owner_lookup() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        // A run parked at Building with no live session (owner-lookup target).
        let active = crate::orchestrator::ActiveRun::reattach(
            &fake_run_record("run-route"),
            ".build/plan.md".into(),
        );
        let project_id = state.projects[0].id.clone();
        state.entity_project.insert("run-route".into(), project_id);
        state.runs.insert("run-route".into(), active);
        // A completed build report routes to the runs map and opens review.
        state.on_agent_done(
            "run-route",
            DoneReport {
                phase: DonePhase::Build,
                status: DoneStatus::Completed,
                summary: "built".into(),
                outputs: DoneOutputs::default(),
            },
        );
        let got = state.handle(req("run.get", json!({ "run_id": "run-route" })));
        assert_eq!(got["result"]["state"], "review", "{got:?}");
        // An unknown owner id is a quiet no-op, never a panic.
        state.on_agent_done(
            "run-nope",
            DoneReport {
                phase: DonePhase::Build,
                status: DoneStatus::Completed,
                summary: "x".into(),
                outputs: DoneOutputs::default(),
            },
        );
    }

    #[test]
    fn mark_idle_demotes_a_quiet_plan_and_run() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        // A Building run whose harness exits immediately → demoted, with the
        // exit code recorded (the quiescence rule: silence is never completion).
        let store = crate::store::Store::new(dir.path().join("side-store"));
        let side = Orchestrator::new(
            repo.clone(),
            dir.path().join("side-wt"),
            Agent::Warm(HarnessSpec::new("sh").arg("-c").arg("exit 7")),
            Templates::default(),
        );
        let active = side
            .dispatch_run(
                RunId::new("run-idle"),
                RunSource::Quick { goal: "quiet" },
                "main",
                Default::default(),
                &store,
            )
            .unwrap();
        let project_id = state.projects[0].id.clone();
        state.entity_project.insert("run-idle".into(), project_id);
        state.runs.insert("run-idle".into(), active);
        std::thread::sleep(Duration::from_millis(300));
        let demoted = state.mark_idle_tasks(Duration::from_secs(3600));
        assert!(demoted.contains(&"run-idle".to_string()), "{demoted:?}");
        let got = state.handle(req("run.get", json!({ "run_id": "run-idle" })));
        assert_eq!(got["result"]["state"], "idle_unreported", "{got:?}");
        assert!(got["result"]["last_error"]
            .as_str()
            .unwrap()
            .contains("exit code 7"));
    }

    fn fake_run_record(id: &str) -> PersistedRun {
        PersistedRun {
            id: id.into(),
            plan_id: None,
            goal: "quiet".into(),
            project_path: String::new(),
            base_branch: "main".into(),
            state: RunState::Building,
            branch: "build/quiet".into(),
            worktree_name: "quiet".into(),
            worktree_path: "/tmp/nonexistent-run".into(),
            base_sha: None,
            stages: Vec::new(),
            current_stage_id: None,
            revising_stage_id: None,
            auto_advance: false,
            adopted: false,
            pending_continuation: false,
            model: None,
            effort: None,
            last_summary: None,
            last_error: None,
            created_at: "2026-07-01T10:00:00Z".into(),
            updated_at: "2026-07-01T10:00:00Z".into(),
        }
    }

    // ---- boot recovery + migration -------------------------------------------

    #[test]
    fn legacy_task_migrates_into_a_plan_and_run_on_boot() {
        let (dir, repo) = init_repo();
        let store_dir = dir.path().join("store");
        // A quick legacy task in review, with a real surviving worktree.
        let (_wt_dir, quick_wt) = init_repo();
        let store = crate::store::Store::new(&store_dir);
        store
            .save(&legacy_task(
                "task-quick",
                crate::legacy::TaskKind::Quick,
                crate::legacy::TaskState::Review,
                &repo,
                &quick_wt,
            ))
            .unwrap();
        // A standard legacy task still in plan review (planning phase) → plan only.
        let (_wt2, plan_wt) = init_repo();
        store
            .save(&legacy_task(
                "task-plan",
                crate::legacy::TaskKind::Standard,
                crate::legacy::TaskState::PlanReview,
                &repo,
                &plan_wt,
            ))
            .unwrap();
        // A standard legacy task past planning (building) → an approved plan AND
        // a run pointing back at it, both surfaced.
        let (_wt3, build_wt) = init_repo();
        store
            .save(&legacy_task(
                "task-build",
                crate::legacy::TaskKind::Standard,
                crate::legacy::TaskState::Building,
                &repo,
                &build_wt,
            ))
            .unwrap();

        let mut state = qa_state(&repo, dir.path());
        // The quick task became a plan-less run kept at review.
        let run = state.handle(req("run.get", json!({ "run_id": "task-quick" })));
        assert_eq!(run["result"]["state"], "review", "{run:?}");
        assert_eq!(run["result"]["plan_id"], Value::Null);
        // The standard task became a plan at plan_review, no run.
        let plan = state.handle(req("plan.get", json!({ "plan_id": "task-plan" })));
        assert_eq!(plan["result"]["state"], "plan_review", "{plan:?}");
        assert_eq!(
            state.handle(req("run.get", json!({ "run_id": "task-plan" })))["ok"],
            false
        );
        // The past-planning task became an approved plan plus a run linked back
        // to it. The plan is a resting Approved; the run was mid-build, so boot
        // recovery demotes it to Interrupted (its PTY died on the restart).
        let built_plan = state.handle(req("plan.get", json!({ "plan_id": "task-build" })));
        assert_eq!(built_plan["result"]["state"], "approved", "{built_plan:?}");
        let built_run = state.handle(req("run.get", json!({ "run_id": "task-build" })));
        assert_eq!(built_run["result"]["state"], "interrupted", "{built_run:?}");
        assert_eq!(built_run["result"]["plan_id"], "task-build");

        // Everything surfaces on the board, in the right collection.
        let board = state.handle(req("board.list", json!({})));
        let ids = |arr: &Value, key: &str| -> Vec<String> {
            arr.as_array()
                .unwrap()
                .iter()
                .map(|v| v[key].as_str().unwrap().to_string())
                .collect()
        };
        let plan_ids = ids(&board["result"]["plans"], "plan_id");
        let run_ids = ids(&board["result"]["runs"], "run_id");
        assert!(plan_ids.contains(&"task-plan".to_string()), "{plan_ids:?}");
        assert!(plan_ids.contains(&"task-build".to_string()), "{plan_ids:?}");
        assert!(
            !plan_ids.contains(&"task-quick".to_string()),
            "the quick task has no plan: {plan_ids:?}"
        );
        assert!(run_ids.contains(&"task-quick".to_string()), "{run_ids:?}");
        assert!(run_ids.contains(&"task-build".to_string()), "{run_ids:?}");
        assert!(
            !run_ids.contains(&"task-plan".to_string()),
            "the plan-only task has no run: {run_ids:?}"
        );
    }

    fn legacy_task(
        id: &str,
        kind: crate::legacy::TaskKind,
        st: crate::legacy::TaskState,
        repo: &std::path::Path,
        worktree: &std::path::Path,
    ) -> crate::store::PersistedTask {
        crate::store::PersistedTask {
            id: id.into(),
            goal: "legacy".into(),
            kind,
            project_path: repo.display().to_string(),
            base_branch: "main".into(),
            state: st,
            branch: format!("build/{id}"),
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
            adopted: false,
            pending_continuation: false,
            created_at: "2026-07-01T10:00:00Z".into(),
            updated_at: "2026-07-01T10:00:00Z".into(),
        }
    }

    #[test]
    fn working_plan_and_run_surface_interrupted_on_boot() {
        let (dir, repo) = init_repo();
        let (_a, plan_wt) = init_repo();
        let (_b, run_wt) = init_repo();
        let store = crate::store::Store::new(dir.path().join("store"));
        store
            .save_plan(&drafting_plan("plan-1", &repo, &plan_wt))
            .unwrap();
        store
            .save_run(&building_run("run-1", &repo, &run_wt))
            .unwrap();
        let mut state = qa_state(&repo, dir.path());
        let plan = state.handle(req("plan.get", json!({ "plan_id": "plan-1" })));
        assert_eq!(plan["result"]["state"], "interrupted", "{plan:?}");
        let run = state.handle(req("run.get", json!({ "run_id": "run-1" })));
        assert_eq!(run["result"]["state"], "interrupted", "{run:?}");
        assert_eq!(run["result"]["needs_attention"], true);
    }

    #[test]
    fn missing_run_worktree_abandons_and_missing_repo_too() {
        let (dir, repo) = init_repo();
        let store = crate::store::Store::new(dir.path().join("store"));
        // Worktree gone, repo present → abandoned (branch kept).
        let mut gone = building_run("run-gone", &repo, std::path::Path::new("/tmp/nope-run"));
        gone.state = RunState::Building;
        store.save_run(&gone).unwrap();
        // Repo gone, worktree present, native → abandoned with a reason.
        let (_w, wt) = init_repo();
        let mut norepo = building_run("run-norepo", std::path::Path::new("/tmp/nope-repo"), &wt);
        norepo.state = RunState::Building;
        store.save_run(&norepo).unwrap();

        let mut state = qa_state(&repo, dir.path());
        let g = state.handle(req("run.get", json!({ "run_id": "run-gone" })));
        assert_eq!(g["result"]["state"], "abandoned", "{g:?}");
        let n = state.handle(req("run.get", json!({ "run_id": "run-norepo" })));
        assert_eq!(n["result"]["state"], "abandoned", "{n:?}");
        assert!(n["result"]["last_error"]
            .as_str()
            .unwrap()
            .contains("project repo missing"));
    }

    #[test]
    fn corrupt_run_record_fails_boot_naming_the_file() {
        let (dir, repo) = init_repo();
        let runs_dir = dir.path().join("store").join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();
        std::fs::write(runs_dir.join("run-bad.json"), "{ not json").unwrap();
        let err = AppState::new(
            repo.clone(),
            dir.path().join("wt"),
            "main",
            true,
            "/tmp/test-mcp.sock",
        )
        .with_task_store(dir.path().join("store"))
        .err()
        .expect("boot should fail on a corrupt record");
        assert!(err.contains("run-bad.json"), "{err}");
    }

    fn drafting_plan(
        id: &str,
        repo: &std::path::Path,
        worktree: &std::path::Path,
    ) -> PersistedPlan {
        PersistedPlan {
            id: id.into(),
            goal: "drafting".into(),
            project_path: repo.display().to_string(),
            base_branch: "main".into(),
            state: PlanState::Drafting,
            worktree_name: Some(id.into()),
            worktree_path: Some(worktree.display().to_string()),
            branch: Some(format!("plan/{id}")),
            plan_path: ".build/plan.md".into(),
            stages: Vec::new(),
            comments: Vec::new(),
            model: None,
            effort: None,
            last_summary: None,
            last_error: None,
            created_at: "2026-07-01T10:00:00Z".into(),
            updated_at: "2026-07-01T10:00:00Z".into(),
        }
    }

    fn building_run(id: &str, repo: &std::path::Path, worktree: &std::path::Path) -> PersistedRun {
        PersistedRun {
            id: id.into(),
            plan_id: None,
            goal: "building".into(),
            project_path: repo.display().to_string(),
            base_branch: "main".into(),
            state: RunState::Building,
            branch: format!("build/{id}"),
            worktree_name: id.into(),
            worktree_path: worktree.display().to_string(),
            base_sha: None,
            stages: Vec::new(),
            current_stage_id: None,
            revising_stage_id: None,
            auto_advance: false,
            adopted: false,
            pending_continuation: false,
            model: None,
            effort: None,
            last_summary: None,
            last_error: None,
            created_at: "2026-07-01T10:00:00Z".into(),
            updated_at: "2026-07-01T10:00:00Z".into(),
        }
    }

    // ---- adopt / release / delete --------------------------------------------

    #[test]
    fn run_adopt_release_and_delete() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.projects[0].id.clone();
        let _ext_path = add_external_worktree(&repo, dir.path(), "feature-x", "feature-x");
        // Resolve the scanner-minted worktree id (match by branch; the scanner
        // canonicalizes paths, which differ from the raw join on macOS).
        let worktree_id = state
            .external_worktrees(&project_id, true)
            .unwrap()
            .into_iter()
            .find(|w| w.branch.as_deref() == Some("feature-x"))
            .expect("the external worktree is discoverable")
            .id;
        let adopted = state.handle(req(
            "run.adopt",
            json!({ "project_id": project_id, "worktree_id": worktree_id }),
        ));
        assert_eq!(adopted["result"]["state"], "review", "{adopted:?}");
        assert_eq!(adopted["result"]["adopted"], true);
        let run_id = run_id_of(&adopted);
        // Release drops the record, keeps the files.
        let released = state.handle(req("run.release", json!({ "run_id": run_id })));
        assert_eq!(released["ok"], true, "{released:?}");
        assert!(!state.runs.contains_key(&run_id));
    }

    #[test]
    fn run_delete_is_terminal_only() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let run = state.handle(req("run.create", json!({ "goal": "live" })));
        let run_id = run_id_of(&run);
        let live = state.handle(req("run.delete", json!({ "run_id": run_id })));
        assert!(
            live["error"].as_str().unwrap().contains("terminal runs"),
            "{live:?}"
        );
        state.handle(req("run.abandon", json!({ "run_id": run_id })));
        let gone = state.handle(req("run.delete", json!({ "run_id": run_id })));
        assert_eq!(gone["ok"], true, "{gone:?}");
    }

    #[test]
    fn merge_cleanup_keep_keeps_the_worktree() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let run = state.handle(req("run.create", json!({ "goal": "keep it" })));
        let run_id = run_id_of(&run);
        let worktree = state.runs.get(&run_id).unwrap().worktree.path.clone();
        let merged = state.handle(req(
            "run.git_action",
            json!({ "run_id": run_id, "action": "merge", "cleanup": "keep" }),
        ));
        assert_eq!(merged["result"]["state"], "merged");
        assert!(worktree.exists(), "cleanup=keep keeps the worktree");
    }

    // ---- terminals + agent screens over runs ---------------------------------

    fn insert_live_run(
        state: &Arc<Mutex<AppState>>,
        repo: &std::path::Path,
        side_root: std::path::PathBuf,
        run_id: &str,
    ) {
        let store = crate::store::Store::new(side_root.join("store"));
        let side = Orchestrator::new(
            repo.to_path_buf(),
            side_root.join("wt"),
            Agent::Warm(
                HarnessSpec::new("sh")
                    .arg("-c")
                    .arg("(while :; do echo agent-beat; sleep 0.05; done) & cat >/dev/null"),
            ),
            Templates::default(),
        );
        let active = side
            .dispatch_run(
                RunId::new(run_id),
                RunSource::Quick { goal: "live agent" },
                "main",
                Default::default(),
                &store,
            )
            .unwrap();
        let mut s = state.lock().unwrap();
        let project_id = s.projects[0].id.clone();
        s.entity_project.insert(run_id.to_string(), project_id);
        s.runs.insert(run_id.to_string(), active);
    }

    #[tokio::test]
    async fn agent_attach_streams_a_live_run_and_retains_the_last_screen() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        insert_live_run(&state, &repo, dir.path().join("side"), "run-9");

        let (sender, mut pushes, key) = SessionSender::observable("s1");
        let res = handler(
            sender,
            req(
                "agent.attach",
                json!({ "id": "run-9", "cols": 100, "rows": 30 }),
            ),
        );
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["term_id"], "agent:run-9");
        assert_eq!(res["result"]["live"], true);

        let seen = wait_for_pushes(&mut pushes, &key, |seen| {
            output_text(seen, "agent:run-9").contains("agent-beat")
        })
        .await;
        assert_eq!(seen[0]["type"], "term.reset", "{seen:?}");

        // End the session → clients hear agent_session_ended, screen retained.
        state
            .lock()
            .unwrap()
            .runs
            .get_mut("run-9")
            .unwrap()
            .session
            .end();
        wait_for_push(&mut pushes, &key, |p| {
            p["type"] == "term.closed"
                && p["term_id"] == "agent:run-9"
                && p["reason"] == "agent_session_ended"
        })
        .await;

        let again = handler(
            SessionSender::detached("s2"),
            req("agent.attach", json!({ "id": "run-9" })),
        );
        assert_eq!(again["result"]["live"], false, "{again:?}");
    }

    #[tokio::test]
    async fn keyed_run_terminal_roundtrips() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        insert_live_run(&state, &repo, dir.path().join("side"), "run-7");

        let created = handler(
            SessionSender::detached("s1"),
            req("term.create", json!({ "run_id": "run-7" })),
        );
        assert_eq!(created["ok"], true, "{created:?}");
        let term_id = created["result"]["term_id"].as_str().unwrap().to_string();

        let (sender, mut pushes, key) = SessionSender::observable("s1");
        let attached = handler(sender, req("term.attach", json!({ "term_id": term_id })));
        assert_eq!(attached["ok"], true, "{attached:?}");

        handler(
            SessionSender::detached("s1"),
            req(
                "term.input",
                json!({ "term_id": term_id, "data": b64encode(b"echo hi-there\n") }),
            ),
        );
        wait_for_push(&mut pushes, &key, |p| {
            p["type"] == "term.output"
                && output_text(std::slice::from_ref(p), &term_id).contains("hi-there")
        })
        .await;

        let listed = handler(
            SessionSender::detached("s1"),
            req("term.list", json!({ "run_id": "run-7" })),
        );
        assert!(!listed["result"]["terminals"].as_array().unwrap().is_empty());
    }

    #[test]
    fn term_scope_parses_run_project_and_external() {
        assert!(matches!(
            TermScope::parse(&json!({ "run_id": "run-1" })).unwrap(),
            TermScope::Run { .. }
        ));
        assert!(matches!(
            TermScope::parse(&json!({ "project_id": "proj-1" })).unwrap(),
            TermScope::Primary { .. }
        ));
        assert!(matches!(
            TermScope::parse(&json!({ "project_id": "proj-1", "worktree_id": "w" })).unwrap(),
            TermScope::ExternalWorktree { .. }
        ));
        assert!(TermScope::parse(&json!({})).is_err());
    }

    // ---- git scope keyed on a run --------------------------------------------

    #[test]
    fn git_status_and_commit_scope_to_a_run() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let run = state.handle(req("run.create", json!({ "goal": "git me" })));
        let run_id = run_id_of(&run);
        // The run's build wrote result.txt (committed by QA merge path? no — it
        // is committed on review); git.status over the run scope succeeds.
        let status = state.handle(req("git.status", json!({ "run_id": run_id })));
        assert_eq!(status["ok"], true, "{status:?}");
        // Exactly-one-scope is enforced.
        let both = state.handle(req(
            "git.status",
            json!({ "run_id": run_id, "project_id": "proj-1" }),
        ));
        assert_eq!(both["ok"], false, "{both:?}");
    }
}
