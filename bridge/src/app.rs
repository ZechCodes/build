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

use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

use crate::mcp::{BridgeAction, CommentResolution, DoneOutputs, DonePhase, DoneReport, DoneStatus};
use crate::models::{self, AgentProvider, ModelChoice};
use crate::notify::{Notifier, NotifyThrottle};
use crate::orchestrator::{
    ActivePlan, ActiveRun, Agent, AgentTurn, Orchestrator, OrchestratorError, ReportConsumed,
    ReportOutcome, RunSource, SpawnOptions, TranscriptProbe,
};
use crate::plan::StageManifestEntry;
use crate::plan::{
    CommentAnchor, CommentState, ImplementationActivity, ImplementationIntent, PlanEvent, PlanId,
    PlanState, StageComment, StageDoc, StageDocState,
};
use crate::pty::{HarnessSpec, PtySession};
use crate::relay::{FrameHandler, SessionSender};
use crate::run::ValidationReport;
use crate::run::{RunEvent, RunId, RunState, StageProgress, StageProgressState, StagePublication};
use crate::store::{
    now_rfc3339, PersistedArchivedWorktree, PersistedPlan, PersistedRun, Store,
    WorktreeFinishAction, WorktreeFinishStatus,
};
use crate::templates::{Templates, STAGES_MANIFEST_PATH};
use crate::thread::ThreadDetail;
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

/// The one kind of program a user terminal runs, on the wire. `term.create`
/// echoes it and `term.list` carries it, so a reloaded client still labels the
/// tab by what is actually in it.
const SHELL_TAB_KIND: &str = "shell";

/// Refuse a `term.create` that asks for anything but the user's shell.
///
/// A user terminal used to be able to spawn `claude`/`codex` directly — the
/// provider's CLI with its approvals bypass and NO `done` MCP server. That was
/// an agent in a worktree Build could not talk to, could not route a report
/// from, and did not count as the worktree's one agent: the only way to get a
/// second agent into a directory. It is gone, so "one worktree, one agent,
/// Build owns it" is a structural property rather than an intention.
///
/// An old client that asks for one is told loudly where the agent lives.
/// Falling back to a shell would run a different program than was asked for,
/// silently, which is the failure mode this refusal exists to prevent.
fn require_shell_kind(params: &Value) -> Result<(), String> {
    match params.get("kind").and_then(Value::as_str) {
        None | Some("") | Some(SHELL_TAB_KIND) => Ok(()),
        Some(named_agent @ ("claude" | "codex")) => Err(format!(
            "a user terminal cannot run {named_agent} — Build's one agent for a \
             worktree lives in its Agent tab"
        )),
        Some(other) => Err(format!(
            "unknown terminal kind {other:?} — a user terminal is always the shell"
        )),
    }
}

/// The harness a shell tab spawns in its worktree root: `-i -l`, so the user
/// gets their own rc files and prompt — their machine, shown honestly.
fn shell_harness_spec(shell: &str) -> HarnessSpec {
    HarnessSpec::new(shell)
        .arg("-i")
        .arg("-l")
        .env("TERM", "xterm-256color")
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
    ///
    /// The result goes through [`AppState::canonical_root`] because the same
    /// directory arrives here in two literal forms — a run's worktree is
    /// `worktrees_root.join(name)` while the scanner canonicalizes, and on
    /// macOS `/tmp` is `/private/tmp`. The tab registry is keyed by this path,
    /// so one un-canonicalized entry point would silently split one worktree
    /// into two and orphan whatever was already open in it.
    fn resolve_root(&self, state: &mut AppState) -> Result<std::path::PathBuf, String> {
        let root = match self {
            TermScope::Run { run_id } => {
                let active = state.runs.get(run_id).ok_or("unknown run_id")?;
                let root = active.worktree.path.clone();
                if !root.exists() {
                    return Err("worktree no longer exists".to_string());
                }
                root
            }
            TermScope::ExternalWorktree {
                project_id,
                worktree_id,
            } => {
                state
                    .resolve_external_worktree(project_id, worktree_id)?
                    .path
            }
            TermScope::Primary { project_id } => state
                .projects
                .iter()
                .find(|p| &p.id == project_id)
                .map(|p| p.repo_path.clone())
                .ok_or_else(|| "unknown project_id".to_string())?,
        };
        Ok(AppState::canonical_root(&root))
    }
}

/// Authoritative server-side screen: vt100 model + attach list + coalescing
/// buffer + the monotonic byte cursor. Snapshot resync, not byte replay. One
/// model for every tab — a shell and an agent reconnect the same way.
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
/// At most this many user terminals daemon-wide, all worktrees combined. An
/// agent tab never counts against it — there is at most one per worktree, and
/// it must stay reachable however many shells are open.
const MAX_USER_TERMINALS: usize = 16;
/// Source/document previews stay tightly capped; playable media gets a larger
/// bounded response because browsers cannot decode a truncated data URL.
const FS_READ_MAX_BYTES: u64 = 1_048_576;
const FS_MEDIA_READ_MAX_BYTES: u64 = 32 * 1_048_576;

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
/// launchd starts agents with a bare PATH, so user-installed coding-agent
/// harnesses don't resolve until we adopt the login PATH.
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
            path_widens_launchd_default(&path).then_some(path)
        }
        _ => None,
    }
}

/// launchd's own PATH. A captured PATH that adds nothing to it is not worth
/// adopting — it would mask the real problem behind a "PATH adopted" log line.
const LAUNCHD_BARE_PATH: [&str; 4] = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"];

fn path_widens_launchd_default(path: &str) -> bool {
    path.split(':')
        .any(|dir| !dir.is_empty() && !LAUNCHD_BARE_PATH.contains(&dir))
}

/// The reserved tab id of a worktree's one Build-owned agent. Every other tab
/// in a worktree is a `term-<n>` shell the human drives.
const AGENT_TAB_ID: &str = "agent";

/// A tab's identity: the canonical worktree it is rooted in, and which tab of
/// that worktree it is.
///
/// Canonical because the same worktree reaches the daemon under three different
/// scope shapes (run / external / primary) and, on macOS, under two different
/// literal paths (`/tmp` is `/private/tmp`). Keying by path rather than by
/// entity id is what makes "one worktree, one agent" a structural property
/// instead of a rule every call site has to remember.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TabKey {
    root: std::path::PathBuf,
    tab_id: String,
}

impl TabKey {
    /// The key of `root`'s one agent tab. `root` must already be canonical —
    /// see [`AppState::canonical_root`].
    fn agent(root: &std::path::Path) -> TabKey {
        TabKey {
            root: root.to_path_buf(),
            tab_id: AGENT_TAB_ID.to_string(),
        }
    }
}

/// What is running in a tab.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TabRole {
    /// The user's own interactive login shell — a window onto their machine.
    /// The daemon-wide terminal cap counts these and never an agent: sixteen
    /// open shells must not be able to crowd a worktree's agent out of the
    /// registry they share.
    Shell,
    /// Build's one agent in this worktree. `owner` is the opaque plan/run id
    /// baked into the harness's `mcp --task <id>` argv, so `done` reports route
    /// back through the owner lookup; `provider` is what was spawned.
    Agent {
        owner: String,
        provider: AgentProvider,
    },
}

/// A live tab: a real PTY rooted in a worktree, plus the authoritative screen
/// model that makes reconnect a snapshot (current screen + cursor) rather than
/// a byte replay.
struct Tab {
    tab_id: String,
    root: std::path::PathBuf,
    role: TabRole,
    /// Surfaced by `term.list` so a reloaded client can order the tab row the
    /// way the human opened it.
    created_at: String,
    session: PtySession,
    screen: TermScreen,
    /// False once the PTY stream has ended. An agent tab is RETAINED after its
    /// process dies so the tab still shows the last screen; a shell tab is
    /// removed by its pump instead, so this is only ever false for an agent.
    live: bool,
    /// When Build last submitted a turn here.
    ///
    /// The quiescence rule ("silence is an anomaly, never completion") used to
    /// read a phase session that was killed at every gate, so silence really
    /// was anomalous. A tab's agent outlives every phase and spends most of its
    /// life idle at a prompt, so silence only means something measured from the
    /// last thing Build asked of it.
    last_delivered_at: Option<std::time::Instant>,
}

impl Tab {
    /// The wire id this tab is demuxed by on the shared terminal socket:
    /// `term-<n>` for a shell, `agent:<worktree_id>` for an agent. An agent is
    /// addressed by its WORKTREE, never by the run that happens to own it —
    /// that is what lets adoption, release, and re-adoption leave the human's
    /// tab where it was.
    fn wire_id(&self) -> String {
        match self.role {
            TabRole::Shell => self.tab_id.clone(),
            TabRole::Agent { .. } => {
                format!(
                    "agent:{}",
                    crate::worktree::external_worktree_id(&self.root)
                )
            }
        }
    }

    /// Spawn `role`'s program in a PTY at `root`, returning the tab and a
    /// receiver subscribed before the first byte can be missed.
    fn spawn(
        role: TabRole,
        spec: &HarnessSpec,
        tab_id: String,
        root: std::path::PathBuf,
        cols: u16,
        rows: u16,
    ) -> Result<(Tab, broadcast::Receiver<Vec<u8>>), String> {
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let session =
            PtySession::spawn(spec, Some(root.clone()), size).map_err(|e| e.to_string())?;
        let rx = session.subscribe();
        Ok((
            Tab {
                tab_id,
                root,
                role,
                created_at: now_rfc3339(),
                session,
                screen: TermScreen::new(cols, rows),
                live: true,
                last_delivered_at: None,
            },
            rx,
        ))
    }
}

/// Whether [`ensure_agent_tab`] found the tab or created it — the ONE input to
/// the cold/warm decision. Coldness is never re-derived from a transcript
/// probe: a transcript can exist while the process is dead, and a context-free
/// nudge into a resumed session whose structured state has moved is exactly the
/// failure this replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spawned {
    /// The tab already existed and its process is live: the agent is mid
    /// conversation and the thread messages are already durable.
    Warm,
    /// The tab was just created (or its dead process was replaced): the agent
    /// has no context to read messages into.
    Fresh,
}

/// How long a caller that lost the spawn race waits for the winner's tab before
/// giving up. Comfortably past a harness's own readiness grace, because the
/// winner holds the reservation across it.
const AGENT_SPAWN_WAIT: Duration = Duration::from_secs(30);

/// An [`AgentTurn`] addressed to a worktree, waiting for the state lock to be
/// free.
///
/// Every lifecycle verb runs inside `state.lock().unwrap().dispatch(..)`, and
/// [`deliver`] takes that same lock and holds nothing while it blocks for
/// seconds spawning a cold harness. So a verb records what it wants said and
/// [`dispatch_frame`] — which holds the `Arc` and no guard — sends it the moment
/// the verb returns.
struct PendingAgentTurn {
    /// The worktree whose one agent hears this turn.
    root: std::path::PathBuf,
    /// The plan/run the harness reports `done` for.
    owner: String,
    model_choice: ModelChoice,
    /// For a tab that had to be spawned: the full run context.
    cold: String,
    /// For a tab already in the conversation: the bare instruction.
    warm: String,
    /// The phase recorded on the conversation's session lineage if the turn
    /// turns out to be cold — a cold delivery is a new agent process.
    phase: &'static str,
}

impl PendingAgentTurn {
    /// Address a run's turn to the run's worktree. Canonical, because the same
    /// worktree reaches the tab registry under several scope shapes.
    fn for_run(owner: &str, active: &ActiveRun, turn: AgentTurn) -> Self {
        PendingAgentTurn {
            root: AppState::canonical_root(&active.worktree.path),
            owner: owner.to_string(),
            model_choice: active.model_choice.clone(),
            cold: turn.cold,
            warm: turn.warm,
            phase: turn.phase,
        }
    }

    /// Address a plan's turn to its disposable planning worktree. `None` once
    /// that worktree is gone (approve/abandon tear it down): a plan with no
    /// worktree has no agent, and every plan surface renders the empty state
    /// rather than a tab that cannot exist.
    fn for_plan(owner: &str, active: &ActivePlan, turn: AgentTurn) -> Option<Self> {
        let worktree = active.worktree.as_ref()?;
        Some(PendingAgentTurn {
            root: AppState::canonical_root(&worktree.path),
            owner: owner.to_string(),
            model_choice: active.model_choice.clone(),
            cold: turn.cold,
            warm: turn.warm,
            phase: turn.phase,
        })
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
    /// Cached `task.list.primary_changes` entry for this project, refreshed at
    /// most every `PRIMARY_SUMMARY_TTL` (spec §5.3) — same discipline as
    /// `external_scan` / the task-stat cache.
    primary_summary: Option<(std::time::Instant, Value)>,
}

/// External-worktree scans are refreshed at most this often per project; the
/// board polls task.list every ~1.6 s and must never trigger a full rescan per
/// poll.
const EXTERNAL_SCAN_INTERVAL: Duration = Duration::from_secs(10);

/// How recently an agent's PTY must have painted for it to count as WORKING.
///
/// Aliveness alone is the wrong signal: an agent tab opened yesterday and left
/// at its prompt is alive and doing nothing, and a rail that pulses at it
/// forever teaches you to ignore the pulse. A working agent paints — spinners,
/// tool output, tokens — so silence means it is waiting for you, which is the
/// state the dot must NOT claim is progress.
const AGENT_WORKING_WINDOW: Duration = Duration::from_secs(30);

/// Whether a tab holds an agent that is working right now.
///
/// Three things have to be true, and each rules out a different lie: the tab
/// is an agent's (a shell is the human's own hands, however busy it looks),
/// its process is still alive (a dead agent's retained screen is not a
/// heartbeat), and it has painted inside [`AGENT_WORKING_WINDOW`] (an agent
/// parked at its prompt is waiting for you, not working).
fn agent_is_working(tab: &Tab) -> bool {
    matches!(tab.role, TabRole::Agent { .. })
        && tab.live
        && !tab.session.has_exited()
        && tab.session.idle_for() < AGENT_WORKING_WINDOW
}

/// `(agent_working, can_finish)` for one worktree's managed agent tab. Finish
/// is advisory and must remain false until that managed agent has existed.
fn worktree_agent_signals(agent_tab: Option<&Tab>) -> (bool, bool) {
    let Some(agent_tab) = agent_tab.filter(|tab| matches!(tab.role, TabRole::Agent { .. })) else {
        return (false, false);
    };
    let agent_working = agent_is_working(agent_tab);
    (agent_working, !agent_working)
}

/// The verbs that count as the human acting on an entity, and the param naming
/// it. Deliberately asymmetric: opening a stage doc counts, because an issue is
/// a queue you triage by reading and reading one IS engaging with it — while a
/// worktree needs an action, since looking at a diff is not the same as doing
/// something about it. An agent's own work never appears here; if it did, the
/// rail would reorder itself while you watched.
const INTERACTION_VERBS: &[(&str, &str)] = &[
    ("plan.stage_doc", "plan_id"),
    ("plan.approve", "plan_id"),
    ("plan.stage_approve", "plan_id"),
    ("plan.send_notes", "plan_id"),
    ("plan.stage_send_notes", "plan_id"),
    ("plan.comment_add", "plan_id"),
    ("plan.comment_resolve", "plan_id"),
    ("plan.message", "plan_id"),
    ("plan.resume", "plan_id"),
    ("plan.abandon", "plan_id"),
    ("run.request_changes", "run_id"),
    ("run.message", "run_id"),
    ("run.git_action", "run_id"),
    ("run.stage_dispatch", "run_id"),
    ("run.stage_send_notes", "run_id"),
    ("run.set_auto_advance", "run_id"),
    ("run.resume", "run_id"),
    ("run.abandon", "run_id"),
    ("run.release", "run_id"),
    ("run.adopt", "worktree_id"),
    ("thread.post", "entity_id"),
];

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

/// Build the warm TUI adapter shared by every project's orchestrator. Build is a
/// UI layer over the agent's PTY: every provider is launched interactively, the
/// rendered prompt is injected into that PTY, and the same session is streamed
/// to attached clients. The closure is shared across projects via `Agent: Clone`.
/// How long a real harness TUI must stop painting before its input is live.
/// Measured against claude 2.1.219: the largest gap inside its startup burst is
/// ~400ms (and the alternate-screen clear lands after a 311ms lull), so the
/// window has to clear that comfortably or the prompt is typed into a screen
/// that is about to be wiped.
const REAL_TUI_SETTLE: Duration = Duration::from_millis(750);

/// Session markers a parent agent leaves in the environment. A harness that
/// finds its own markers treats itself as a nested child of that session rather
/// than its own — claude disables transcript saving, which breaks the
/// `--continue` adoption path Build depends on. Build's agents are always their
/// own sessions.
const INHERITED_AGENT_MARKERS: [&str; 6] = [
    "CLAUDECODE",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_EFFORT",
];

fn build_agent(qa_agent: bool, mcp_socket: String) -> Agent {
    if qa_agent {
        // A warm no-op harness that drains stdin like a real interactive CLI
        // (a non-reading child would let the PTY input queue fill and block
        // prompt writes) and enables bracketed-paste mode like a real TUI's
        // line editor, so the spawn's readiness wait resolves on the same
        // signal production does instead of idling out its grace. Modelling
        // that signal matters: a `cat` that merely echoed could not tell a
        // delivered prompt from one eaten by a startup dialog, which is how a
        // fully green suite once hid exactly that bug. The scripted agent does
        // the file writing.
        Agent::Warm(
            HarnessSpec::new("sh")
                .arg("-c")
                .arg("printf '\\033[?2004h'; cat >/dev/null"),
        )
    } else {
        // Real agents are interactive TUIs. The builder configures argv and the
        // per-entity MCP server; Orchestrator submits the prompt through the PTY.
        let bridge_exe = std::env::current_exe()
            .ok()
            .and_then(|path| path.to_str().map(str::to_string))
            .unwrap_or_else(|| "build-bridge".to_string());
        Agent::WarmBuilder(Arc::new(
            move |_prompt: &str, choice: &ModelChoice, options: &SpawnOptions| match choice.provider
            {
                AgentProvider::Claude => {
                    pre_trust_worktree_for_claude(&options.cwd);
                    let mut spec = HarnessSpec::new("claude")
                        .settle(REAL_TUI_SETTLE)
                        .unset_all(INHERITED_AGENT_MARKERS)
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
                }
                AgentProvider::Codex => {
                    let mut spec = HarnessSpec::new("codex")
                        .settle(REAL_TUI_SETTLE)
                        .unset_all(INHERITED_AGENT_MARKERS)
                        .arg("--dangerously-bypass-approvals-and-sandbox");
                    for arg in choice.harness_args() {
                        spec = spec.arg(arg);
                    }
                    let mcp_args =
                        serde_json::to_string(&vec!["mcp", "--task", options.owner_id.as_str()])
                            .expect("MCP args serialize");
                    for override_arg in [
                        format!(
                            "mcp_servers.build.command={}",
                            serde_json::to_string(&bridge_exe).expect("path serializes")
                        ),
                        format!("mcp_servers.build.args={mcp_args}"),
                        format!(
                            "mcp_servers.build.env.BRIDGE_MCP_SOCKET={}",
                            serde_json::to_string(&mcp_socket).expect("socket serializes")
                        ),
                        format!(
                            "projects.{}.trust_level=\"trusted\"",
                            serde_json::to_string(&options.cwd.to_string_lossy())
                                .expect("worktree path serializes")
                        ),
                        "mcp_servers.build.required=true".to_string(),
                        "mcp_servers.build.enabled_tools=[\"read_unread_messages\",\"post_thread_message\",\"done\"]".to_string(),
                        "mcp_servers.build.default_tools_approval_mode=\"approve\"".to_string(),
                        // Build writes prompt bytes and Enter back-to-back. Codex's
                        // fallback detector otherwise classifies that stream as a
                        // paste burst and turns Enter into a newline, so the prompt
                        // remains visible but unsent. This PTY advertises and frames
                        // real pastes explicitly; the fallback is unnecessary.
                        "disable_paste_burst=true".to_string(),
                    ] {
                        spec = spec.arg("--config").arg(override_arg);
                    }
                    if options.continue_session {
                        spec = spec.arg("resume").arg("--last");
                    }
                    spec
                }
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

/// Record a Build-created worktree as trusted in claude's project registry, so
/// the interactive session skips its workspace-trust dialog.
///
/// Build mints a fresh worktree per run and the dialog fires for any directory
/// claude has not seen. It owns the keyboard until answered, so the prompt Build
/// injects lands in the dialog and the trailing Enter answers it — the agent
/// receives nothing and the run sits in `building` until the idle sweep demotes
/// it. Codex takes the same grant as a per-invocation `--config`; claude keeps
/// trust in shared state, so this is the one place Build writes outside its own
/// tree. It only ever ADDS the flag for a path Build itself created.
///
/// Best-effort by design: claude rewrites this file too, so an interleaved write
/// could drop the insert. Failing the spawn over that would be worse than the
/// dialog it prevents, so every error here is swallowed — the caller still gets
/// a session, and the worst case is today's behavior.
fn pre_trust_worktree_for_claude(cwd: &std::path::Path) {
    let Some(config) = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(|dir| std::path::PathBuf::from(dir).join(".claude.json"))
        .or_else(|| dirs_home().map(|home| home.join(".claude.json")))
    else {
        return;
    };
    if let Err(error) = record_claude_workspace_trust(&config, cwd) {
        eprintln!("pre-trust {}: {error}", cwd.display());
    }
}

fn dirs_home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

/// The read-modify-write half, split out so tests drive a temp registry instead
/// of the developer's real one. Writes through a temp file + rename so a crash
/// mid-write cannot truncate a registry holding every project's state.
fn record_claude_workspace_trust(
    config: &std::path::Path,
    cwd: &std::path::Path,
) -> Result<(), String> {
    let key = cwd.to_string_lossy().to_string();
    let mut registry: Value = match std::fs::read_to_string(config) {
        Ok(raw) => {
            serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", config.display()))?
        }
        // No registry yet: claude will merge its own defaults into ours.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(error) => return Err(format!("read {}: {error}", config.display())),
    };
    let projects = registry
        .as_object_mut()
        .ok_or_else(|| format!("{} is not a JSON object", config.display()))?
        .entry("projects")
        .or_insert_with(|| json!({}));
    let project = projects
        .as_object_mut()
        .ok_or_else(|| "projects is not a JSON object".to_string())?
        .entry(key)
        .or_insert_with(|| json!({}));
    let project = project
        .as_object_mut()
        .ok_or_else(|| "project entry is not a JSON object".to_string())?;
    if project.get("hasTrustDialogAccepted") == Some(&json!(true)) {
        return Ok(());
    }
    project.insert("hasTrustDialogAccepted".to_string(), json!(true));

    if let Some(parent) = config.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let staged = config.with_extension("json.build-tmp");
    std::fs::write(
        &staged,
        serde_json::to_vec_pretty(&registry).map_err(|e| format!("serialize: {e}"))?,
    )
    .map_err(|e| format!("write {}: {e}", staged.display()))?;
    std::fs::rename(&staged, config).map_err(|e| format!("rename {}: {e}", config.display()))
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

/// Codex stores dated JSONL rollouts. The first line is session metadata with
/// the canonical cwd; scanning that small header is enough to decide whether
/// `codex resume --last` has a cwd-scoped conversation to continue.
pub(crate) fn codex_transcript_exists(root: &std::path::Path, cwd: &std::path::Path) -> bool {
    use std::io::BufRead;

    let wanted = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            // Never follow a user-created symlink loop while looking through
            // Codex's dated session directories.
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                dirs.push(path);
                continue;
            }
            if !kind.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(file) = std::fs::File::open(path) else {
                continue;
            };
            let mut first = String::new();
            if std::io::BufReader::new(file).read_line(&mut first).is_err() {
                continue;
            }
            let session_cwd = serde_json::from_str::<Value>(&first).ok().and_then(|meta| {
                meta.pointer("/payload/cwd")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
            if session_cwd.is_some_and(|path| {
                let path = std::path::PathBuf::from(path);
                std::fs::canonicalize(&path).unwrap_or(path) == wanted
            }) {
                return true;
            }
        }
    }
    false
}

fn default_transcript_probe() -> TranscriptProbe {
    Arc::new(|cwd: &std::path::Path, provider| {
        let Ok(home) = std::env::var("HOME") else {
            return false;
        };
        let home = std::path::Path::new(&home);
        match provider {
            AgentProvider::Claude => claude_transcript_exists(&home.join(".claude/projects"), cwd),
            AgentProvider::Codex => codex_transcript_exists(&home.join(".codex/sessions"), cwd),
        }
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
    /// Finished external worktrees keyed by their stable path-derived id.
    /// Loaded from the store at boot; project association is resolved by the
    /// canonical project path because project ids are re-minted.
    archived_worktrees: HashMap<String, PersistedArchivedWorktree>,
    /// entity id → its RFC 3339 creation time, carried across saves (and restarts).
    entity_created_at: HashMap<String, String>,
    /// entity id → its RFC 3339 last-mutation time (stamped on every mutation).
    entity_updated_at: HashMap<String, String>,
    /// entity id → the RFC 3339 time of its last *state transition* (vs
    /// `entity_updated_at`, which moves on every mutation).
    entity_state_changed_at: HashMap<String, String>,
    /// When the human last touched each entity, and whether they have seen where
    /// it got to — the rail's ordering and colour. Keyed by run id, plan id, or
    /// worktree id alike (a bare worktree has no record of its own).
    attention: HashMap<String, crate::attention::Attention>,
    /// entity id → the wire state string last seen by a mutation tail, so
    /// `entity_state_changed_at` only moves on real transitions.
    entity_last_state: HashMap<String, String>,
    /// run id → cached `board.list` diffstat, so the poll surface never runs
    /// per-run git work more than once per TTL window.
    run_stat_cache: HashMap<String, (std::time::Instant, Value)>,
    /// The shell user terminals spawn (resolved once; see [`resolve_term_shell`]).
    term_shell: String,
    streams: HashMap<String, StreamState>,
    /// Every live PTY the daemon owns — the human's shells and each worktree's
    /// one agent alike — keyed by (canonical worktree root, tab id). One
    /// registry over one id space: there is no second place a terminal can be,
    /// so no verb has to ask which kind of thing an id names before serving it.
    tabs: HashMap<TabKey, Tab>,
    /// Roots with an agent spawn in flight. The state lock is dropped across
    /// the spawn (it blocks for seconds), so the reservation — taken under the
    /// same lock acquisition that observed the tab's absence — is what keeps a
    /// second delivery from starting a second harness in one worktree.
    agent_spawns_in_flight: std::collections::HashSet<std::path::PathBuf>,
    /// Canonical worktree root → the screen its Agent tab shows before any
    /// agent has ever run there.
    ///
    /// The Agent tab is a fixture on every worktree surface, so clients attach
    /// to worktrees whose agent does not exist yet — the state every worktree
    /// is in after a daemon restart. They register HERE, and
    /// [`ensure_agent_tab`] carries the screen onto the tab it spawns, so the
    /// session's first frames reach a client that mounted the tab long before
    /// it: the alternative is a screen that stays blank until the human
    /// unmounts and remounts. An entry lives only until that first spawn.
    agent_screens_awaiting_spawn: HashMap<std::path::PathBuf, TermScreen>,
    /// Turns queued by the verbs running under the state lock, drained by
    /// [`dispatch_frame`] once that lock is free. The synchronous test entry
    /// point ([`AppState::handle`]) has no `Arc` to deliver over, so it leaves
    /// the queue for the test to inspect instead.
    ///
    /// DELIBERATE DIVERGENCE — do not "fix" this back into an inline `deliver`
    /// at each verb. A verb runs holding this lock; a delivery spawns a harness
    /// and waits seconds on its readiness, and every terminal pump needs the
    /// same lock to make progress, so delivering inline deadlocks the daemon
    /// for as long as the spawn takes. The split is the contract: under the
    /// lock a verb RECORDS what to say (a `PendingAgentTurn`), and the drain
    /// sites — [`dispatch_frame`] and the done-socket — SAY it with the lock
    /// free. Everything that has to look agentless-versus-in-flight
    /// ([`AppState::agent_turns_in_flight`], the idle sweep) exists to cover
    /// the gap this split opens; none of it is optional.
    pending_agent_turns: Vec<PendingAgentTurn>,
    /// Owners whose turn has left [`AppState::pending_agent_turns`] and is
    /// being delivered right now, counted because one drain can carry several
    /// turns for the same owner. Between a verb's transition and the tab its
    /// turn spawns, a working entity legitimately has no agent tab yet — the
    /// queue and this counter are what tell the idle sweep the difference
    /// between an agent on its way and an agent that never arrived.
    agent_turns_in_flight: HashMap<String, usize>,
    /// `term-<n>` mint counter — monotonic, never reused within a daemon life.
    next_term: u64,
    /// Weak self-handle set once at [`AppState::shared`] time, so `&mut self`
    /// hooks can spawn pump tasks that need the `Arc`. Dispatch paths that run
    /// in tests without an Arc simply skip pump spawning (they assert on
    /// state, not pushes).
    self_handle: Option<std::sync::Weak<Mutex<AppState>>>,
    next_stream: u64,
    next_project: u64,
    /// When true, simulate the agent deterministically (local QA, no LLM).
    qa_agent: bool,
    /// Whether the harness has a prior conversation for a worktree cwd —
    /// consulted on every agent-tab spawn to decide `--continue`.
    ///
    /// It answers exactly one question: is there a conversation here that this
    /// process did not start? A tab respawned after a daemon restart or a
    /// crash, and a worktree where the user ran the agent by hand before Build
    /// looked at it, are the same case, and both want the transcript picked
    /// back up. Never true in QA mode.
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
            Arc::new(|_, _| false)
        } else {
            default_transcript_probe()
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
            archived_worktrees: HashMap::new(),
            entity_created_at: HashMap::new(),
            entity_updated_at: HashMap::new(),
            entity_state_changed_at: HashMap::new(),
            attention: HashMap::new(),
            entity_last_state: HashMap::new(),
            run_stat_cache: HashMap::new(),
            term_shell: resolve_term_shell(),
            streams: HashMap::new(),
            tabs: HashMap::new(),
            agent_spawns_in_flight: std::collections::HashSet::new(),
            agent_screens_awaiting_spawn: HashMap::new(),
            pending_agent_turns: Vec::new(),
            agent_turns_in_flight: HashMap::new(),
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
        let archived_worktrees = store
            .load_all_archived_worktrees()
            .map_err(|e| e.to_string())?;
        // Attention survives a restart, or Monday would look like a fresh install.
        self.attention = store.load_attention();
        self.store = Some(store);
        self.archived_worktrees = archived_worktrees
            .into_iter()
            .map(|record| (record.worktree_id.clone(), record))
            .collect();
        self.recover_completed_worktree_finishes();
        // Plans first: a run re-derives its `plan_path` from the owning plan's
        // record, so the plan must already be in the map.
        for record in plans {
            self.recover_plan(record)?;
        }
        for record in runs {
            self.recover_run(record)?;
        }
        // Issue implementation intent is the scheduler's durable source of
        // truth. Reconcile it only after every implementation lineage record
        // has been restored, so an approved waiting stage can resume without
        // minting a duplicate worktree after a daemon restart.
        let issue_ids = self
            .plans
            .iter()
            .filter(|(_, issue)| issue.plan.implementation_intent != ImplementationIntent::None)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for issue_id in issue_ids {
            self.advance_issue_scheduler(&issue_id, &json!({ "issue_id": issue_id }))?;
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

        // A boot transition (e.g. drafting → interrupted) is a real state
        // change and stamps now; otherwise keep the record's stamp. Old
        // records carry none — fall back to their updated_at. Seed the
        // last-observed state from the (post-recovery) plan so the first
        // post-boot mutation in the same state doesn't false-stamp.
        self.entity_state_changed_at.insert(
            plan_id.clone(),
            if state_changed {
                now_rfc3339()
            } else {
                record
                    .state_changed_at
                    .unwrap_or_else(|| record.updated_at.clone())
            },
        );
        self.entity_last_state
            .insert(plan_id.clone(), plan_state_str(&active.plan.state));
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
        // Re-derive `plan_path` from the owning plan (recovered first); adopted
        // runs and orphaned links fall back to the convention default.
        let plan_path = record
            .plan_id
            .as_ref()
            .and_then(|pid| self.plans.get(pid))
            .map(|plan| plan.plan_path.clone())
            .unwrap_or_else(|| crate::templates::DEFAULT_PLAN_PATH.to_string());
        let mut active = ActiveRun::reattach(&record, plan_path);

        self.entity_project_path
            .insert(run_id.clone(), record.project_path.clone());
        let repo_path = std::path::PathBuf::from(&record.project_path);
        let project_id = if repo_path.exists() {
            let project_id = self.add_project(repo_path.clone(), record.base_branch.clone());
            self.entity_project
                .insert(run_id.clone(), project_id.clone());
            Some(project_id)
        } else {
            None
        };

        let mut state_changed = false;
        let mut recovery_event = None;
        if !active.run.state.is_terminal() && !active.worktree.path.exists() {
            let restored = project_id
                .as_deref()
                .filter(|_| !active.adopted)
                .ok_or_else(|| "the original project/branch is unavailable".to_string())
                .and_then(|project_id| {
                    self.orch_for(project_id)?
                        .restore_run_worktree(&active.worktree)
                        .map_err(err)
                });
            match restored {
                Ok(worktree) => {
                    active.worktree = worktree;
                    recovery_event = Some((
                        crate::thread::ThreadEventKind::WorktreeRecovered,
                        format!(
                            "Recreated the Issue worktree from branch {}",
                            active.worktree.branch
                        ),
                    ));
                    state_changed = true;
                }
                Err(error) => {
                    let affected = self.reconcile_missing_run_worktree(&run_id, &mut active);
                    // Issue-linked runs remain archived as durable lineage;
                    // legacy/standalone runs preserve their established
                    // abandoned recovery state.
                    let terminal_event = if active.run.plan_id.is_some() {
                        RunEvent::Archive
                    } else {
                        RunEvent::Abandon
                    };
                    active
                        .run
                        .apply(terminal_event)
                        .map_err(|e| format!("recover {run_id}: {e}"))?;
                    active.last_error = Some(format!("worktree recovery failed: {error}"));
                    recovery_event = Some((
                        crate::thread::ThreadEventKind::RecoveryFailed,
                        format!(
                            "Could not recover the Issue worktree: {error}. {} stage(s) were marked incomplete",
                            affected.len()
                        ),
                    ));
                    state_changed = true;
                }
            }
        }
        if !active.run.state.is_terminal() && active.run.state.is_working() {
            active
                .run
                .apply(RunEvent::Interrupt)
                .map_err(|e| format!("recover {run_id}: {e}"))?;
            state_changed = true;
        }

        if project_id.is_none() && !active.run.state.is_terminal() {
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
        } else if project_id.is_none() {
            eprintln!(
                "recover {run_id}: project repo {} is gone; run kept as history",
                record.project_path
            );
        }

        if let (Some(issue_id), Some((event, summary))) = (
            active.run.plan_id.as_ref().map(|id| id.0.clone()),
            recovery_event,
        ) {
            if let Ok(mut issue) = self.take_plan(&issue_id) {
                issue.thread.push_event_with_links(
                    event,
                    Some(summary),
                    None,
                    None,
                    vec![crate::thread::ThreadLink::Run {
                        run_id: run_id.clone(),
                    }],
                    now_rfc3339(),
                );
                let (_, persisted) = self.finish_plan_mutation(issue_id, issue);
                persisted?;
            }
        }

        // Same restore discipline as recover_plan: a boot transition stamps
        // now, otherwise keep the record's stamp (falling back to updated_at
        // for pre-field records); seed last-state from the recovered run.
        self.entity_state_changed_at.insert(
            run_id.clone(),
            if state_changed {
                now_rfc3339()
            } else {
                record
                    .state_changed_at
                    .unwrap_or_else(|| record.updated_at.clone())
            },
        );
        self.entity_last_state
            .insert(run_id.clone(), run_state_str(&active.run.state));
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
            archived_at: active.plan.archived_at.clone(),
            implementation_intent: active.plan.implementation_intent.clone(),
            implementation_activity: active.plan.implementation_activity.clone(),
            worktree_name: worktree.map(|w| w.name.clone()),
            worktree_path: worktree.map(|w| w.path.display().to_string()),
            branch: worktree.map(|w| w.branch.clone()),
            plan_path: active.plan_path.clone(),
            stages: active.stages.clone(),
            comments: active.comments.clone(),
            provider: active.model_choice.provider,
            model: active.model_choice.model.clone(),
            effort: active.model_choice.effort.clone(),
            thread: active.thread.clone(),
            last_summary: active.last_summary.clone(),
            last_error: active.last_error.clone(),
            created_at,
            updated_at,
            state_changed_at: self.entity_state_changed_at.get(plan_id).cloned(),
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
            provider: active.model_choice.provider,
            model: active.model_choice.model.clone(),
            effort: active.model_choice.effort.clone(),
            thread: active.thread.clone(),
            last_summary: active.last_summary.clone(),
            last_error: active.last_error.clone(),
            created_at,
            updated_at,
            state_changed_at: self.entity_state_changed_at.get(run_id).cloned(),
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
        self.entity_updated_at.insert(plan_id.clone(), now.clone());
        self.stamp_state_change(&plan_id, plan_state_str(&active.plan.state), now);
        let view = self.plan_view(&plan_id, &active, ThreadDetail::Full);
        let persisted = self.persist_plan_record(&plan_id, &active);
        self.push_notify_plan(&plan_id, active.plan.state);
        self.plans.insert(plan_id, active);
        self.reap_orphaned_terminals();
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
        self.entity_updated_at.insert(run_id.clone(), now.clone());
        self.stamp_state_change(&run_id, run_state_str(&active.run.state), now);
        // The mutation likely changed the tree; drop the cached diffstat.
        self.run_stat_cache.remove(&run_id);
        let view = self.run_view(&run_id, &active, ThreadDetail::Full);
        let persisted = self.persist_run_record(&run_id, &active);
        self.push_notify_run(&run_id, active.run.state);
        self.runs.insert(run_id, active);
        self.reap_orphaned_terminals();
        (view, persisted)
    }

    /// A cold delivery started a new harness for `turn.owner`: open the
    /// conversation's session lineage for it, exactly as the phase spawner used
    /// to. A warm delivery never calls this — the session it continues is
    /// already open, and a second `start_session` would read back as an agent
    /// restart that never happened.
    fn record_agent_session_start(&mut self, turn: &PendingAgentTurn) {
        self.edit_owner_thread("record_agent_session_start", &turn.owner, |thread| {
            open_session_lineage(thread, turn)
        });
    }

    /// The agent process an id owns has ended: close the conversation's session
    /// lineage for it. The mirror of
    /// [`record_agent_session_start`](Self::record_agent_session_start), and it
    /// is the PUMP that calls it — the only place that learns a harness died on
    /// its own. An owner that no longer exists (its record was deleted with the
    /// tab) has no lineage left to close, which is why this is quiet.
    fn record_agent_session_end(&mut self, owner: &str) {
        self.edit_owner_thread("record_agent_session_end", owner, |thread| {
            finish_open_session(thread, &now_rfc3339())
        });
    }

    /// Apply `edit` to `owner`'s conversation and persist the result, whichever
    /// kind of entity the id names. Plan and run ids are disjoint, so the owner
    /// lookup is the router; an id that names neither is a no-op, because a
    /// thread that no longer exists cannot be wrong.
    fn edit_owner_thread(
        &mut self,
        context: &str,
        owner: &str,
        edit: impl FnOnce(&mut crate::thread::Thread),
    ) {
        if self.plans.contains_key(owner) {
            let Ok(mut active) = self.take_plan(owner) else {
                return;
            };
            edit(&mut active.thread);
            let (_, persisted) = self.finish_plan_mutation(owner.to_string(), active);
            if let Err(error) = persisted {
                eprintln!("{context} {owner}: {error}");
            }
            return;
        }
        let Ok(mut active) = self.take_run(owner) else {
            return;
        };
        edit(&mut active.thread);
        let (_, persisted) = self.finish_run_mutation(owner.to_string(), active);
        if let Err(error) = persisted {
            eprintln!("{context} {owner}: {error}");
        }
    }

    /// A turn never reached an agent: record why on the entity and persist it,
    /// so the surface says what happened instead of showing a working task with
    /// nobody working.
    ///
    /// The state is deliberately left alone. The transition that queued this
    /// turn is already durable, and demotion belongs to one place — the idle
    /// sweep, which now reads a working entity with no agent tab as the anomaly
    /// it is. This method's whole job is the reason.
    fn record_agent_delivery_failure(&mut self, turn: &PendingAgentTurn, error: &str) {
        let reason = format!("could not reach the agent: {error}");
        // Plan and run ids are disjoint, so the owner lookup is the router.
        if self.plans.contains_key(&turn.owner) {
            let Ok(mut active) = self.take_plan(&turn.owner) else {
                return;
            };
            active.last_error = Some(reason);
            let (_, persisted) = self.finish_plan_mutation(turn.owner.clone(), active);
            if let Err(error) = persisted {
                eprintln!("record_agent_delivery_failure {}: {error}", turn.owner);
            }
            return;
        }
        let Ok(mut active) = self.take_run(&turn.owner) else {
            return;
        };
        active.last_error = Some(reason);
        let (_, persisted) = self.finish_run_mutation(turn.owner.clone(), active);
        if let Err(error) = persisted {
            eprintln!("record_agent_delivery_failure {}: {error}", turn.owner);
        }
    }

    /// Is this entity's agent merely on its way — a turn still queued, or one
    /// off the queue and mid-delivery? Between the verb that transitions an
    /// entity (under the state lock) and the tab its turn spawns (lock free,
    /// seconds for a cold harness), a working entity has no agent tab and is
    /// perfectly healthy. Everywhere else, a working entity without one is an
    /// anomaly.
    fn agent_turn_is_undelivered(&self, owner: &str) -> bool {
        self.agent_turns_in_flight.contains_key(owner)
            || self
                .pending_agent_turns
                .iter()
                .any(|turn| turn.owner == owner)
    }

    /// Move `entity_state_changed_at` only when the entity's wire state
    /// actually differs from the last one a mutation tail observed. A fresh
    /// entity's first mutation stamps it — creation is a state change.
    fn stamp_state_change(&mut self, entity_id: &str, state: String, now: String) {
        if self.entity_last_state.get(entity_id) == Some(&state) {
            return;
        }
        self.entity_state_changed_at
            .insert(entity_id.to_string(), now);
        self.entity_last_state.insert(entity_id.to_string(), state);
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
        );
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

    /// The canonical form of a worktree root — the tab registry's key. Every
    /// entry point funnels through this: a run's worktree arrives as
    /// `worktrees_root/<name>` and is NOT canonical, while an external
    /// worktree's path already is, and on macOS the same directory has two
    /// literal spellings. Falls back to the raw path when the directory is
    /// gone, so a vanished worktree still keys consistently for the reaper.
    fn canonical_root(path: &std::path::Path) -> std::path::PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    /// How many of the human's own shells the tab registry holds. The
    /// daemon-wide terminal cap counts these and never an agent tab: an agent
    /// is Build's, always reachable, and must not be crowded out by shells.
    fn shell_tab_count(&self) -> usize {
        self.tabs
            .values()
            .filter(|tab| tab.role == TabRole::Shell)
            .count()
    }

    /// The canonical worktree an entity (plan or run) is working in — the key
    /// its agent is registered under.
    ///
    /// A plan whose disposable planning worktree has been torn down (approved,
    /// abandoned) has no worktree and therefore no agent; that is a refusal,
    /// not a blank tab, because there is nothing for an agent to run in.
    fn entity_worktree_root(&self, entity_id: &str) -> Result<std::path::PathBuf, String> {
        if let Some(plan) = self.plans.get(entity_id) {
            return plan
                .worktree
                .as_ref()
                .map(|w| Self::canonical_root(&w.path))
                .ok_or_else(|| "the plan has no worktree, so it has no agent".to_string());
        }
        if let Some(run) = self.runs.get(entity_id) {
            return Ok(Self::canonical_root(&run.worktree.path));
        }
        Err("unknown id".to_string())
    }

    /// An entity's conversation, for a caller that needs to read it without
    /// changing it — chiefly "is anything waiting for this agent".
    fn entity_thread(&self, entity_id: &str) -> Result<&crate::thread::Thread, String> {
        if let Some(plan) = self.plans.get(entity_id) {
            return Ok(&plan.thread);
        }
        if let Some(run) = self.runs.get(entity_id) {
            return Ok(&run.thread);
        }
        Err("unknown id".to_string())
    }

    /// The agent an entity dispatches with. A start with no turn behind it still
    /// has to honor the provider/model the human chose for this worktree — the
    /// sheet's answer, or the run's own — rather than silently defaulting.
    fn entity_model_choice(&self, entity_id: &str) -> Result<ModelChoice, String> {
        if let Some(plan) = self.plans.get(entity_id) {
            return Ok(plan.model_choice.clone());
        }
        if let Some(run) = self.runs.get(entity_id) {
            return Ok(run.model_choice.clone());
        }
        Err("unknown id".to_string())
    }

    /// Kill, reap, and forget a worktree's agent, telling every attached client
    /// the tab is gone.
    ///
    /// An agent whose owner no longer exists is worse than no agent: it keeps
    /// working and reports `done` into the unknown-entity log forever. So the
    /// two verbs that remove a run while KEEPING its worktree — release
    /// (un-adopt) and delete — close it here. A worktree that vanishes takes
    /// its agent with it through the reaper instead.
    fn close_agent_tab(&mut self, root: &std::path::Path) {
        let key = TabKey::agent(&Self::canonical_root(root));
        let Some(tab) = self.tabs.remove(&key) else {
            return;
        };
        let wire_id = tab.wire_id();
        tab.session.kill_and_reap();
        tab.screen.push_closed(&wire_id, "closed");
    }

    /// The registry key a wire id addresses — `term-<n>` for a shell,
    /// `agent:<worktree_id>` for a worktree's agent.
    ///
    /// One id space, one resolver. Every wire-facing verb funnels through here,
    /// so a stale client holding a tab that no longer exists gets one
    /// consistent "unknown term_id" and drops the tab — rather than a tab that
    /// attaches and then silently swallows every keystroke, which is what a
    /// second, half-migrated `starts_with("agent:")` branch would produce.
    ///
    /// A scan, not a map hit: the registry is keyed by worktree and there are
    /// only ever a handful of live tabs.
    fn tab_key_of_wire_id(&self, wire_id: &str) -> Result<TabKey, String> {
        self.tabs
            .iter()
            .find(|(_, tab)| tab.wire_id() == wire_id)
            .map(|(key, _)| key.clone())
            .ok_or_else(|| "unknown term_id".to_string())
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
                    let (read_half, mut write_half) = stream.into_split();
                    let mut lines = tokio::io::BufReader::new(read_half).lines();
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
                            // A report can start the next phase (a built stage
                            // hands itself to validation). The turn is queued
                            // under the lock above; sending it needs the lock
                            // free, exactly as on the relay's frame path.
                            deliver_pending_agent_turns(&state);
                            continue;
                        }
                        if let Ok(action) = serde_json::from_value::<BridgeAction>(
                            v.get("request").cloned().unwrap_or(Value::Null),
                        ) {
                            let response =
                                match state.lock().unwrap().on_mcp_action(entity_id, action) {
                                    Ok(result) => json!({ "ok": true, "result": result }),
                                    Err(error) => json!({ "ok": false, "error": error }),
                                };
                            let _ = write_half.write_all(response.to_string().as_bytes()).await;
                            let _ = write_half.write_all(b"\n").await;
                            let _ = write_half.flush().await;
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

    /// Execute an MCP thread request against the conversation owner resolved
    /// from the lifecycle owner baked into that session's MCP command. Planned
    /// implementations report `done` as their run id, but all unread/reply
    /// actions resolve to the owning Issue (legacy plan id). Planless adopted
    /// runs retain their independent worktree conversation.
    fn on_mcp_action(&mut self, entity_id: &str, action: BridgeAction) -> Result<Value, String> {
        if let BridgeAction::PostThreadMessage { links, .. } = &action {
            self.validate_thread_links_for_owner(entity_id, links)?;
        }
        let now = now_rfc3339();
        if self.plans.contains_key(entity_id) {
            let mut active = self.take_plan(entity_id)?;
            let result = apply_thread_action(
                &mut active.thread,
                crate::thread::ArtifactKind::Plan,
                action,
                &now,
            );
            let (_, persisted) = self.finish_plan_mutation(entity_id.to_string(), active);
            let value = result?;
            persisted?;
            return Ok(value);
        }
        if let Some(issue_id) = self
            .runs
            .get(entity_id)
            .and_then(|run| run.run.plan_id.as_ref())
            .map(|id| id.0.clone())
            .filter(|issue_id| self.plans.contains_key(issue_id))
        {
            let mut issue = self.take_plan(&issue_id)?;
            let result = apply_thread_action(
                &mut issue.thread,
                crate::thread::ArtifactKind::Diff,
                action,
                &now,
            );
            let (_, persisted) = self.finish_plan_mutation(issue_id, issue);
            let value = result?;
            persisted?;
            return Ok(value);
        }
        if self.runs.contains_key(entity_id) {
            let mut active = self.take_run(entity_id)?;
            let result = apply_thread_action(
                &mut active.thread,
                crate::thread::ArtifactKind::Diff,
                action,
                &now,
            );
            let (_, persisted) = self.finish_run_mutation(entity_id.to_string(), active);
            let value = result?;
            persisted?;
            return Ok(value);
        }
        Err(format!("unknown conversation owner: {entity_id}"))
    }

    /// Validate agent-supplied navigation against the conversation owner. Shape
    /// checks alone let a compromised agent forge links into another Issue;
    /// ownership and canonical stage paths are bridge-resolved here.
    fn validate_thread_links_for_owner(
        &self,
        entity_id: &str,
        links: &[crate::thread::ThreadLink],
    ) -> Result<(), String> {
        validate_thread_links(links)?;
        let issue_id = self
            .plans
            .contains_key(entity_id)
            .then(|| entity_id.to_string())
            .or_else(|| {
                self.runs
                    .get(entity_id)
                    .and_then(|run| run.run.plan_id.as_ref())
                    .map(|id| id.0.clone())
            });
        for link in links {
            match link {
                crate::thread::ThreadLink::PlanStage {
                    plan_id,
                    stage_id,
                    path,
                } => {
                    if issue_id.as_deref() != Some(plan_id.as_str()) {
                        return Err("plan stage link does not belong to this Issue".to_string());
                    }
                    let exact = self.plans.get(plan_id).and_then(|issue| {
                        issue
                            .stages
                            .iter()
                            .find(|stage| stage.id == *stage_id)
                            .map(|stage| stage.path.as_str())
                    });
                    if exact != Some(path.as_str()) {
                        return Err("plan stage link is not a canonical Issue stage".to_string());
                    }
                }
                crate::thread::ThreadLink::Run { run_id } => {
                    let belongs = self.runs.get(run_id).is_some_and(|run| match &issue_id {
                        Some(issue_id) => {
                            run.run.plan_id.as_ref().map(|id| id.0.as_str())
                                == Some(issue_id.as_str())
                        }
                        None => run_id == entity_id,
                    });
                    if !belongs {
                        return Err("run link does not belong to this Issue".to_string());
                    }
                }
                crate::thread::ThreadLink::File { .. } => {
                    let has_scope = self.runs.contains_key(entity_id)
                        || self.plans.get(entity_id).is_some_and(|issue| {
                            issue.worktree.is_some()
                                || self.current_issue_implementation(entity_id).is_some()
                        });
                    if !has_scope {
                        return Err("file link has no Issue worktree scope".to_string());
                    }
                }
            }
        }
        Ok(())
    }

    /// A plan agent reported `done`: ingest + advance on the plan's orchestrator.
    fn on_plan_agent_done(&mut self, plan_id: &str, report: DoneReport) {
        let Some(mut active) = self.plans.remove(plan_id) else {
            return;
        };
        let previous_stage_ids: Vec<String> =
            active.stages.iter().map(|stage| stage.id.clone()).collect();
        let report_for_thread = report.clone();
        let outcome = (|| -> Result<(), String> {
            let project_id = self.project_of(plan_id)?;
            let store = self.require_store()?;
            self.orch_for(&project_id)?
                .on_plan_done(&mut active, store, report)
                .map_err(err)
        })();
        if let Err(e) = &outcome {
            eprintln!("on_agent_done {plan_id}: {e}");
        }
        if outcome.is_ok()
            && report_for_thread.phase == DonePhase::Plan
            && report_for_thread.status == DoneStatus::Completed
        {
            let new_stages: Vec<(usize, StageDoc)> = active
                .stages
                .iter()
                .enumerate()
                .filter(|(_, stage)| !previous_stage_ids.contains(&stage.id))
                .map(|(index, stage)| (index, stage.clone()))
                .collect();
            append_plan_stage_announcements(&mut active.thread, plan_id, &new_stages);
        }
        record_report_in_thread(
            &mut active.thread,
            &report_for_thread,
            outcome.as_ref().err().map(String::as_str),
        );
        if outcome.is_ok() && report_for_thread.status == DoneStatus::Completed {
            if let Some(contents) = self.plan_revision_contents(plan_id, &active) {
                active.thread.add_revision(
                    crate::thread::ArtifactKind::Plan,
                    &contents,
                    &now_rfc3339(),
                );
            }
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
        let issue_id = active.run.plan_id.as_ref().map(|id| id.0.clone());
        let report_for_thread = report.clone();
        let consumed = (|| -> Result<ReportConsumed, String> {
            let project_id = self.project_of(run_id)?;
            self.orch_for(&project_id)?
                .on_run_done(&mut active, &plan_docs, report)
                .map_err(err)
        })();
        let outcome = match consumed {
            Err(e) => {
                eprintln!("on_agent_done {run_id}: {e}");
                Err(e)
            }
            // Build's agent is persistent, so it reports whenever it finishes a
            // turn — including one the human started at a review gate, which no
            // lifecycle event accepts. Enforcement is by observation: the report
            // is recorded on the conversation below and nothing moves.
            Ok(ReportConsumed { outcome, next }) => {
                if let ReportOutcome::OutOfPhase(illegal) = &outcome {
                    eprintln!(
                        "on_agent_done {run_id}: out-of-phase report ({illegal}); recorded only"
                    );
                }
                // A stage that built hands itself to validation: the same
                // agent, a new turn. Queued rather than written here — the done
                // socket holds the state lock and a cold delivery needs it free.
                if let Some(turn) = next {
                    self.pending_agent_turns
                        .push(PendingAgentTurn::for_run(run_id, &active, turn));
                }
                Ok(outcome)
            }
        };
        let diff_revision = if outcome.is_ok() && report_for_thread.status == DoneStatus::Completed
        {
            self.project_of(run_id).ok().and_then(|project_id| {
                self.orch_for(&project_id)
                    .and_then(|orch| orch.run_diff(&active).map_err(err))
                    .ok()
                    .map(|diff| diff.patch().to_string())
            })
        } else {
            None
        };
        let mut issue = issue_id
            .as_ref()
            .and_then(|issue_id| self.plans.remove(issue_id));
        let conversation = issue
            .as_mut()
            .map(|issue| &mut issue.thread)
            .unwrap_or(&mut active.thread);
        record_report_in_thread(
            conversation,
            &report_for_thread,
            outcome.as_ref().err().map(String::as_str),
        );
        if let Some(patch) = diff_revision {
            conversation.add_revision(crate::thread::ArtifactKind::Diff, &patch, &now_rfc3339());
        }
        let (_, persisted) = self.finish_run_mutation(run_id.to_string(), active);
        if let Err(e) = persisted {
            eprintln!("on_agent_done {run_id}: {e}");
        }
        if let (Some(issue_id), Some(issue)) = (issue_id, issue) {
            let (_, persisted) = self.finish_plan_mutation(issue_id, issue);
            if let Err(e) = persisted {
                eprintln!("on_agent_done {run_id}: issue conversation persist failed: {e}");
            }
        }
        self.auto_advance_run(run_id);
        if let Some(issue_id) = self
            .runs
            .get(run_id)
            .and_then(|run| run.run.plan_id.as_ref())
            .map(|id| id.0.clone())
        {
            if let Err(error) = self.refresh_issue_scheduler_activity(&issue_id) {
                eprintln!("issue scheduler {issue_id}: {error}");
            }
        }
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
        let report_for_thread = report.clone();
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
        if let Err(e) = &outcome {
            eprintln!("on_agent_done {run_id}: {e}");
        }
        record_report_in_thread(
            &mut active.thread,
            &report_for_thread,
            outcome.as_ref().err().map(String::as_str),
        );
        if outcome.is_ok() {
            if let (Some(pid), Some(plan_ref)) = (plan_id.as_deref(), plan.as_mut()) {
                if let Some(contents) = self.plan_revision_contents(pid, plan_ref) {
                    plan_ref.thread.add_revision(
                        crate::thread::ArtifactKind::Plan,
                        &contents,
                        &now_rfc3339(),
                    );
                    active.thread.add_revision(
                        crate::thread::ArtifactKind::Plan,
                        &contents,
                        &now_rfc3339(),
                    );
                }
            }
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

    fn plan_revision_contents(&self, plan_id: &str, active: &ActivePlan) -> Option<String> {
        let store = self.store.as_ref()?;
        if active.stages.is_empty() {
            return store.read_plan_doc(plan_id, &active.plan_path);
        }
        let mut combined = String::new();
        for stage in &active.stages {
            let contents = store.read_plan_doc(plan_id, &stage.path)?;
            combined.push_str(&stage.path);
            combined.push('\n');
            combined.push_str(&contents);
            combined.push('\n');
        }
        Some(combined)
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

    /// Stamp the entity a successful verb acted on, if that verb counts as an
    /// interaction. One table rather than fifteen call sites: the policy is the
    /// kind of thing that drifts when it lives next to the code it describes.
    fn stamp_interaction_for(&mut self, method: &str, params: &Value, result: &Value) {
        let param = |key: &str| params.get(key).and_then(Value::as_str).map(str::to_string);
        let mut touched: Vec<String> = INTERACTION_VERBS
            .iter()
            .filter(|(verb, _)| *verb == method)
            .filter_map(|(_, key)| param(key))
            .collect();
        // Implementing an issue is an interaction with BOTH: the plan you acted
        // on and the run you just made.
        if method == "run.create" {
            touched.extend(param("plan_id"));
            touched.extend(
                result
                    .get("run_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            );
        }
        // A worktree Build itself cut enters the rail as already-interacted: you
        // made it on purpose, and it is waiting for you to do something in it.
        // (A worktree made outside Build stays in the Worktrees row until you
        // act on it here — nothing stamps it, so nothing surfaces it.)
        if method == "worktree.create" {
            touched.extend(
                result
                    .get("worktree_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            );
        }
        for id in touched {
            self.touch_attention(&id);
        }
    }

    /// Record that the human acted on `id`, now.
    fn touch_attention(&mut self, id: &str) {
        let now = now_rfc3339();
        self.attention
            .entry(id.to_string())
            .or_default()
            .interact(&now);
        self.persist_attention();
    }

    /// Record that the human has seen `id` as of its current state clock.
    fn see_attention(&mut self, id: &str) {
        let Some(state_changed_at) = self.entity_state_clock(id) else {
            return;
        };
        self.attention
            .entry(id.to_string())
            .or_default()
            .see(&state_changed_at);
        self.persist_attention();
    }

    /// The entity's state clock — what a `seen` stamp is versioned against. A
    /// bare worktree has no lifecycle of its own, so seeing it is simply now.
    fn entity_state_clock(&self, id: &str) -> Option<String> {
        Some(
            self.entity_state_changed_at
                .get(id)
                .cloned()
                .unwrap_or_else(now_rfc3339),
        )
    }

    /// Write the attention map, pruned to the entities that still exist. Cheap
    /// (one small file) and done on every stamp, so a crash costs at most the
    /// action in flight rather than the day's ordering.
    fn persist_attention(&mut self) {
        let Ok(store) = self.require_store() else {
            return;
        };
        let live: std::collections::HashSet<String> = self
            .runs
            .keys()
            .chain(self.plans.keys())
            .cloned()
            .chain(self.attention_worktree_ids())
            .collect();
        if let Err(e) = store.save_attention(&self.attention, &live) {
            eprintln!("attention: {e}");
        }
    }

    /// Worktree ids worth keeping attention for: every one the scan can still
    /// see. Their records live nowhere else, so the scan IS the liveness test.
    fn attention_worktree_ids(&self) -> Vec<String> {
        self.projects
            .iter()
            .filter_map(|p| p.external_scan.as_ref())
            .flat_map(|cache| cache.worktrees.iter().map(|w| w.id.clone()))
            .collect()
    }

    /// Route a verb, then record it if it counts as the human touching
    /// something. Wrapped here rather than in `handle` because the relay calls
    /// `dispatch` directly — `handle` is a test convenience, so stamping there
    /// would have worked in every test and in no real session.
    fn dispatch(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        let outcome = self.route(method, params);
        if let Ok(result) = &outcome {
            // Only a verb that SUCCEEDED counts: a rejected action never happened.
            self.stamp_interaction_for(method, params, result);
        }
        outcome
    }

    fn route(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        match method {
            "ping" => Ok(json!({ "pong": true })),
            "models.list" => Ok(json!({
                "models": models::catalog(),
                "efforts": models::EFFORT_LEVELS,
                "default_provider": AgentProvider::Claude,
                "providers": models::provider_catalogs(),
            })),
            "thread.revision" => self.thread_revision(params),
            "thread.post" => self.thread_post(params),
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
            "archive.list" => self.archive_list(params),
            // Canonical Issue surface. The existing plan id and plan-store path
            // remain the durable identity/location; plan.* below is the
            // deprecated wire adapter for existing clients.
            "issue.create" => self.plan_create(params),
            "issue.get" => self.plan_get(&alias_param(params, "issue_id", "plan_id")),
            "issue.list" => Ok(self.issue_list()),
            "issue.doc" => self.plan_doc(&alias_param(params, "issue_id", "plan_id")),
            "issue.stages" => self.issue_stages(params),
            "issue.stage_doc" => self.plan_stage_doc(&alias_param(params, "issue_id", "plan_id")),
            "issue.approve" => self.plan_approve(&alias_param(params, "issue_id", "plan_id")),
            "issue.send_notes" => self.plan_send_notes(&alias_param(params, "issue_id", "plan_id")),
            "issue.stage_approve" => {
                self.plan_stage_approve(&alias_param(params, "issue_id", "plan_id"))
            }
            "issue.stage_revise" => {
                self.plan_stage_send_notes(&alias_param(params, "issue_id", "plan_id"))
            }
            "issue.implement_stage" => self.issue_implement_stage(params),
            "issue.implement_all" => self.issue_implement_all(params),
            "issue.set_auto_advance" => self.issue_set_auto_advance(params),
            "issue.stage_fix" => self.issue_run_action(params, "fix"),
            "issue.stage_diff" => self.issue_stage_diff(params),
            "issue.diff" => self.issue_run_action(params, "diff"),
            "issue.request_changes" => self.issue_run_action(params, "request_changes"),
            "issue.git_action" => self.issue_run_action(params, "git_action"),
            "issue.comment_add" => {
                self.plan_comment_add(&alias_param(params, "issue_id", "plan_id"))
            }
            "issue.comment_delete" => {
                self.plan_comment_delete(&alias_param(params, "issue_id", "plan_id"))
            }
            "issue.archive" => self.plan_archive(&alias_param(params, "issue_id", "plan_id")),
            "issue.delete" => self.plan_delete(&alias_param(params, "issue_id", "plan_id")),
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
            "plan.archive" => self.plan_archive(params),
            // Run surface (worktree-scoped): keyed by run_id.
            "run.create" => self.run_create(&alias_param(params, "issue_id", "plan_id")),
            "run.get" => self.run_get(params),
            "run.diff" => self.run_diff(params),
            "run.stage_diff" => self.run_stage_diff(params),
            "run.request_changes" => self.run_request_changes(params),
            "run.stage_dispatch" => self.run_stage_dispatch(params),
            "run.stage_fix" => self.run_stage_fix(params),
            "run.stage_send_notes" => self.run_stage_send_notes(params),
            "run.set_auto_advance" => self.run_set_auto_advance(params),
            "run.git_action" => self.run_git_action(params),
            "run.message" => self.run_message(params),
            "run.abandon" => self.run_abandon(params),
            "run.delete" => self.run_delete(params),
            "run.adopt" => self.run_adopt(params),
            "run.release" => self.run_release(params),
            "run.finish" => self.run_finish(params),
            "worktree.create" => self.worktree_create(params),
            "worktree.finish" => self.worktree_finish(params),
            "entity.seen" => self.entity_seen(params),
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

    /// The user's shells in the requested scope's worktree, ordered by numeric
    /// id suffix. The worktree's agent never appears here — it is not one of
    /// the tabs the human opens and closes. An unknown scope id still errors
    /// (the SPA treats an error as "no terminals").
    ///
    /// The filter is on the resolved canonical ROOT, not on the scope shape
    /// that was asked with. A client addresses an unadopted worktree as
    /// `{project_id, worktree_id}` and the same directory as `{run_id}` once a
    /// run adopts it; filtering by scope made every open shell vanish from the
    /// tab row at adoption while its process kept running.
    fn term_list(&mut self, params: &Value) -> Result<Value, String> {
        let root = TermScope::parse(params)?.resolve_root(self)?;
        let mut terminals: Vec<(u64, Value)> = self
            .tabs
            .iter()
            .filter(|(key, tab)| key.root == root && tab.role == TabRole::Shell)
            .map(|(key, tab)| {
                (
                    term_id_suffix(&key.tab_id),
                    json!({
                        "term_id": tab.tab_id,
                        "kind": SHELL_TAB_KIND,
                        "cols": tab.screen.cols,
                        "rows": tab.screen.rows,
                        "created_at": tab.created_at,
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
        let key = self.tab_key_of_wire_id(&term_id)?;
        if key.tab_id == AGENT_TAB_ID {
            // The agent tab is not one of the human's tabs to close: it is
            // always reachable, and its life is bound to the worktree.
            return Err("cannot close an agent terminal".to_string());
        }
        let tab = self.tabs.remove(&key).ok_or("unknown term_id")?;
        tab.session.kill_and_reap();
        tab.screen.push_closed(&term_id, "closed");
        Ok(json!({ "ok": true }))
    }

    /// Write client keystrokes (base64) to a tab's PTY, by id. Input to the
    /// agent tab is allowed by design — its PTY is a full terminal on the
    /// user's machine and the terminal is the basement — and an agent whose
    /// process has ended surfaces "no active agent session" rather than
    /// swallowing the keystrokes.
    fn term_input(&mut self, params: &Value) -> Result<Value, String> {
        let term_id = require_str(params, "term_id")?;
        let data = b64decode(&require_str(params, "data")?)?;
        let key = self.tab_key_of_wire_id(&term_id)?;
        let tab = self.tabs.get(&key).ok_or("unknown term_id")?;
        if !tab.live || tab.session.has_exited() {
            return Err("no active agent session".to_string());
        }
        tab.session.write_input(&data).map_err(|e| e.to_string())?;
        Ok(json!({ "ok": true }))
    }

    /// Resize a tab's PTY and screen model, by id. The resize only applies
    /// while the session is live; a dead resize is a no-op `live: false` so a
    /// retained last screen is never garbled.
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
        let key = self.tab_key_of_wire_id(&term_id)?;
        let tab = self.tabs.get_mut(&key).ok_or("unknown term_id")?;
        let live = tab.live && !tab.session.has_exited();
        if live {
            tab.session.resize(size).map_err(|e| e.to_string())?;
            tab.screen.set_size(cols, rows);
        }
        Ok(json!({ "ok": true, "live": live }))
    }

    /// A session ended: detach it from every tab so the pumps stop encrypting
    /// (and serializing) output frames into a session the relay will just drop.
    ///
    /// A screen waiting for its first spawn is a tab one step early and follows
    /// the same rule: [`ensure_agent_tab`] carries its clients onto the real
    /// screen, so a session left behind here would be pushed to for the life of
    /// that tab. Such a screen exists only to hold clients and the viewport
    /// they render at — with the last one gone there is nothing to hold, and
    /// the spawn is sized the way an unwatched spawn always was.
    fn drop_session(&mut self, session_id: &str) {
        for tab in self.tabs.values_mut() {
            tab.screen
                .attached
                .retain(|snd| snd.session_id() != session_id);
        }
        self.agent_screens_awaiting_spawn.retain(|_, screen| {
            screen.attached.retain(|snd| snd.session_id() != session_id);
            !screen.attached.is_empty()
        });
    }

    /// Close every tab whose worktree is gone from disk (spec §2.6.3), killing
    /// AND reaping each one, and tell every attached client. Returns the closed
    /// wire ids. Called at the tail of `finish_mutation` (prompt closure right
    /// after abandon/delete/merge-prune) and by the periodic reaper loop
    /// (out-of-band disappearance, e.g. a user `rm -rf`ing a worktree).
    ///
    /// A tab lives as long as the WORKTREE it is rooted in — not as long as the
    /// entity that happens to own it. That is one rule for shells and agents
    /// alike, and it is the right one for both: a merged run kept with
    /// `cleanup=keep` keeps its directory and everything open in it, and
    /// releasing or deleting an adopted run leaves the human's worktree exactly
    /// where it was. The two verbs that remove a run while keeping its worktree
    /// close Build's agent themselves ([`AppState::close_agent_tab`]), because
    /// an agent whose owner is gone reports `done` into the unknown-entity log
    /// forever.
    fn reap_orphaned_terminals(&mut self) -> Vec<String> {
        let vanished: Vec<TabKey> = self
            .tabs
            .keys()
            .filter(|key| !key.root.exists())
            .cloned()
            .collect();
        let mut reaped = Vec::new();
        for key in vanished {
            let Some(tab) = self.tabs.remove(&key) else {
                continue;
            };
            let wire_id = tab.wire_id();
            tab.session.kill_and_reap();
            tab.screen.push_closed(&wire_id, "reaped");
            reaped.push(wire_id);
        }
        // The screens waiting for a first spawn go the same way: a worktree
        // that is gone will never host the agent their clients are watching
        // for, and a screen nothing can ever paint is not one to keep.
        let orphaned: Vec<std::path::PathBuf> = self
            .agent_screens_awaiting_spawn
            .keys()
            .filter(|root| !root.exists())
            .cloned()
            .collect();
        for root in orphaned {
            let Some(screen) = self.agent_screens_awaiting_spawn.remove(&root) else {
                continue;
            };
            let wire_id = format!("agent:{}", crate::worktree::external_worktree_id(&root));
            screen.push_closed(&wire_id, "reaped");
            reaped.push(wire_id);
        }
        reaped
    }

    /// Periodically close tabs whose worktree vanished out-of-band (nothing
    /// went through `finish_*_mutation` — e.g. the user deleted a worktree by
    /// hand). Runs beside `spawn_idle_monitor`.
    pub fn spawn_terminal_reaper(state: Arc<Mutex<AppState>>, interval: Duration) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let reaped = state.lock().unwrap().reap_orphaned_terminals();
                for term_id in reaped {
                    eprintln!("terminal reaper: closed {term_id} (worktree gone)");
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
        //
        // The signal comes from the worktree's agent TAB, and silence is
        // measured from the last turn Build submitted there. A tab's agent
        // survives every phase boundary, so raw PTY silence would demote a run
        // the moment it was re-dispatched after a long quiet review.
        //
        // NO tab at all is the loudest anomaly of the three, not a reason to
        // look away: Build owns every agent and keeps it as a tab for the life
        // of its worktree, so a working entity without one has a delivery that
        // failed or a worktree that was reaped out from under it. Only a turn
        // still on its way (`turn_undelivered`) explains a missing tab
        // innocently, and it explains it for seconds, not for the daemon's
        // life. A tabless demotion claims no exit code — nothing exited.
        let idle_check = |tab: Option<&Tab>, turn_undelivered: bool| {
            let Some(tab) = tab else {
                return if turn_undelivered { None } else { Some(None) };
            };
            if tab.session.has_exited() {
                return Some(Some(tab.session.exit_code().unwrap_or(-1)));
            }
            let quiet_for = quiet_threshold;
            let painted_recently = tab.session.idle_for() < quiet_for;
            let spoken_to_recently = tab
                .last_delivered_at
                .is_some_and(|at| at.elapsed() < quiet_for);
            if painted_recently || spoken_to_recently {
                None
            } else {
                Some(None)
            }
        };
        let idle_plans: Vec<(String, Option<i32>)> = self
            .plans
            .iter()
            .filter(|(_, a)| a.plan.state.is_working())
            .filter_map(|(id, a)| {
                let root = a.worktree.as_ref().map(|w| Self::canonical_root(&w.path))?;
                idle_check(
                    self.tabs.get(&TabKey::agent(&root)),
                    self.agent_turn_is_undelivered(id),
                )
                .map(|code| (id.clone(), code))
            })
            .collect();
        let idle_runs: Vec<(String, Option<i32>)> = self
            .runs
            .iter()
            .filter(|(_, a)| a.run.state.is_working())
            .filter_map(|(id, a)| {
                let root = Self::canonical_root(&a.worktree.path);
                idle_check(
                    self.tabs.get(&TabKey::agent(&root)),
                    self.agent_turn_is_undelivered(id),
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
            record_idle_in_thread(&mut active.thread, exit_code);
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
            record_idle_in_thread(&mut active.thread, exit_code);
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

    /// Read one file from a worktree-backed scope, base64 always, capped at the
    /// source limit or the larger bounded media limit server-side.
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
        let read_limit = if media_mime_hint(&target).is_some() {
            FS_MEDIA_READ_MAX_BYTES
        } else {
            FS_READ_MAX_BYTES
        };
        let mut content = Vec::with_capacity(size.min(read_limit) as usize);
        file.take(read_limit)
            .read_to_end(&mut content)
            .map_err(|e| format!("cannot read {path}: {e}"))?;
        let truncated = size > read_limit;
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
        // Resolved up front: the loop below holds a &mut borrow of the scan
        // cache, and attention_json needs &self.
        let attention_of: std::collections::HashMap<String, Value> = self
            .attention
            .keys()
            .map(|id| (id.clone(), self.attention_json(id)))
            .collect();
        // Read off the tab registry, which is where an agent can be — there is
        // no longer anywhere else for one to run. The worktree id is derived
        // from the tab's root, so a worktree reports its own agent whatever
        // entity (or none) currently owns it.
        let agent_signals: HashMap<String, (bool, bool)> = self
            .tabs
            .values()
            .filter(|tab| matches!(tab.role, TabRole::Agent { .. }))
            .map(|tab| {
                (
                    crate::worktree::external_worktree_id(&tab.root),
                    worktree_agent_signals(Some(tab)),
                )
            })
            .collect();
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
                let (agent_working, can_finish) =
                    agent_signals.get(&w.id).copied().unwrap_or((false, false));
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
                    // Ahead and behind always share one comparison ref. The
                    // working-tree delta is reported separately below.
                    "comparison_ref": w.comparison_ref,
                    "ahead": w.ahead,
                    "behind": w.behind,
                    "base_branch": base_branch,
                    "unpushed": w.unpushed,
                    "upstream": w.upstream,
                    "diffstat": {
                        "files_changed": w.diffstat.files_changed,
                        "insertions": w.diffstat.insertions,
                        "deletions": w.diffstat.deletions,
                    },
                    // What is sitting in the tree unsaved — the rail's +/−.
                    "uncommitted": {
                        "files_changed": w.uncommitted.files_changed,
                        "insertions": w.uncommitted.insertions,
                        "deletions": w.uncommitted.deletions,
                    },
                    "adoptable": adoptable,
                    "agent_working": agent_working,
                    "can_finish": can_finish,
                    // A worktree Build cut carries attention from birth, so it
                    // surfaces in the rail as something waiting for you. One made
                    // outside Build has none until you act on it here, and stays
                    // in the Worktrees row until then.
                    "attention": attention_of.get(&w.id).cloned().unwrap_or_else(|| json!({
                        "resume_at": Value::Null,
                        "interacted": false,
                        "seen": false,
                    })),
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
        type SyncCounts = (Option<String>, Option<String>, Option<u64>, Option<u64>);
        fn head_sync_counts(repo: &git2::Repository, base_branch: &str) -> SyncCounts {
            const NONE: SyncCounts = (None, None, None, None);
            let head = match repo.head() {
                Ok(head) if head.is_branch() => head,
                _ => return NONE,
            };
            let Some(branch) = head.shorthand() else {
                return NONE;
            };
            let Ok(commit) = head.peel_to_commit() else {
                return NONE;
            };
            let comparison =
                crate::worktree::branch_comparison(repo, &commit, Some(branch), base_branch);
            (
                comparison.upstream,
                comparison.reference,
                comparison.ahead,
                comparison.behind,
            )
        }

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
            let (upstream, comparison_ref, ahead, behind) = repo
                .as_ref()
                .ok()
                .map(|repo| head_sync_counts(repo, &project.base_branch))
                .unwrap_or((None, None, None, None));
            let summary = match crate::diff::diff_against_head(&project.repo_path) {
                Ok(diff) => {
                    let stat = diff.stat();
                    Some(json!({
                        "project_id": project_id,
                        "branch": branch,
                        "upstream": upstream,
                        "comparison_ref": comparison_ref,
                        "ahead": ahead,
                        "behind": behind,
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

    /// Resolve the shared `git.*` scope: the project's primary checkout
    /// (`project_id` alone), a run's worktree (`run_id`), or one of the
    /// project's external worktrees (`project_id` + `worktree_id`). The repo
    /// path always comes from server state — a client can never name a
    /// filesystem path directly.
    fn resolve_git_scope(&mut self, params: &Value) -> Result<GitScope, String> {
        let project_id = params
            .get("project_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let run_id = params
            .get("run_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        let worktree_id = params
            .get("worktree_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        match (project_id, run_id, worktree_id) {
            // A worktree is named within its project, so both ids arrive
            // together — and the worktree, being the narrower of the two, is
            // what the RPC operates on.
            (Some(project_id), None, Some(worktree_id)) => {
                let external = self.resolve_external_worktree(&project_id, &worktree_id)?;
                let base_branch = self.base_for(&project_id)?;
                Ok(GitScope {
                    repo_path: external.path,
                    project_id: None,
                    run: None,
                    worktree: Some(GitScopeWorktree {
                        project_id,
                        base_branch,
                    }),
                })
            }
            (Some(project_id), None, None) => {
                let project = self
                    .projects
                    .iter()
                    .find(|p| p.id == project_id)
                    .ok_or_else(|| "unknown project_id".to_string())?;
                Ok(GitScope {
                    repo_path: project.repo_path.clone(),
                    project_id: Some(project.id.clone()),
                    run: None,
                    worktree: None,
                })
            }
            (None, Some(run_id), None) => {
                let active = self
                    .runs
                    .get(&run_id)
                    .ok_or_else(|| "unknown run_id".to_string())?;
                Ok(GitScope {
                    repo_path: active.worktree.path.clone(),
                    project_id: None,
                    run: Some(GitScopeRun {
                        run_id,
                        base_branch: active.worktree.base_branch.clone(),
                    }),
                    worktree: None,
                })
            }
            _ => Err(
                "provide exactly one of project_id, run_id, or project_id + worktree_id"
                    .to_string(),
            ),
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
        crate::gitgui::log_page(&scope.repo_path, scope.mark_ahead_of(), limit, skip)
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
    /// uncommitted-changes summary for project scope, the external-worktree
    /// scan for worktree scope — so the next `task.list` / project poll
    /// recomputes instead of serving a stale summary for up to its TTL.
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
        if let Some(worktree) = &scope.worktree {
            let project_id = worktree.project_id.clone();
            self.invalidate_external_scan(&project_id);
        }
    }

    /// Resolve the checkout a branch operation acts on: the project's primary
    /// checkout, or one of its external worktrees when the caller names one. A
    /// run worktree's branch is owned by the run lifecycle, so a `run_id` (or
    /// its legacy `task_id` spelling) is refused outright.
    fn resolve_branch_scope(&mut self, params: &Value) -> Result<BranchScope, String> {
        if params.get("task_id").is_some() || params.get("run_id").is_some() {
            return Err("branch operations are project- or worktree-scope only".to_string());
        }
        let project_id = require_str(params, "project_id")?;
        if let Some(worktree_id) = params.get("worktree_id").and_then(Value::as_str) {
            let external = self.resolve_external_worktree(&project_id, worktree_id)?;
            return Ok(BranchScope {
                project_id,
                repo_path: external.path,
                external_worktree: true,
            });
        }
        let project = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .ok_or_else(|| "unknown project_id".to_string())?;
        Ok(BranchScope {
            project_id: project.id.clone(),
            repo_path: project.repo_path.clone(),
            external_worktree: false,
        })
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

    /// `git.branches` — the local branch list of the scoped checkout (the same
    /// list either way: branches are the repository's, not one checkout's).
    fn git_branches(&mut self, params: &Value) -> Result<Value, String> {
        let scope = self.resolve_branch_scope(params)?;
        crate::gitgui::branch_list(&scope.repo_path)
    }

    /// `git.checkout` — switch the scoped checkout to (or create) a branch,
    /// then the fresh status payload.
    fn git_checkout(&mut self, params: &Value) -> Result<Value, String> {
        let scope = self.resolve_branch_scope(params)?;
        let branch = require_str(params, "branch")?;
        let create = params
            .get("create")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        crate::gitgui::checkout(&scope.repo_path, &branch, create)?;
        // A branch switch swaps the whole tree, so whichever summary described
        // it is stale.
        if scope.external_worktree {
            self.invalidate_external_scan(&scope.project_id);
        } else if let Some(project) = self.projects.iter_mut().find(|p| p.id == scope.project_id) {
            project.primary_summary = None;
        }
        crate::gitgui::status_payload(&scope.repo_path)
    }

    /// `git.branch_delete` — delete a local branch, then the fresh branch list.
    fn git_branch_delete(&mut self, params: &Value) -> Result<Value, String> {
        let repo_path = self.resolve_branch_scope(params)?.repo_path;
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
    /// Mint a bare worktree on a fresh branch off the project's base — no run,
    /// no agent, no session. It is the "somewhere to work" affordance beside
    /// issue creation: the human opens a terminal or an agent tab in it, and it
    /// stays unbound (the scan reports it like any hand-made worktree) until a
    /// mutating action adopts it.
    ///
    /// `name` is what the human typed, and it decides both the directory and the
    /// branch. It is UNTRUSTED text on its way to a path and a `git` argv, so it
    /// goes through the same slugifier every branch name does: ASCII alphanumerics
    /// and single hyphens, nothing else, so no separator, dot-segment or leading
    /// dash can survive it. A name that would slugify away to nothing is refused
    /// rather than silently replaced — being handed a worktree you did not name is
    /// worse than being told the name will not do.
    fn worktree_create(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let name = require_str(params, "name")?;
        if !name.chars().any(|c| c.is_ascii_alphanumeric()) {
            return Err("a worktree name needs at least one letter or number".to_string());
        }
        let slug = crate::worktree::slugify(&name);
        let base = self.base_for(&project_id)?;
        let worktree = self
            .orch_for(&project_id)?
            .create_bare_worktree(&slug, &base)
            .map_err(err)?;
        // The scan keys worktrees by canonical path; mirror that here so the
        // caller can navigate to the surface without waiting for a rescan.
        let canonical = std::fs::canonicalize(&worktree.path).unwrap_or(worktree.path.clone());
        // The new worktree must be visible to the very next board poll, not up
        // to EXTERNAL_SCAN_INTERVAL later.
        self.invalidate_external_scan(&project_id);
        Ok(json!({
            "project_id": project_id,
            "worktree_id": crate::worktree::external_worktree_id(&canonical),
            "branch": worktree.branch,
            "name": worktree.name,
            "path": canonical.display().to_string(),
        }))
    }

    /// Finish an external worktree selected only by server-resolved ids. The
    /// forced scan is both stale-id protection and the execution-time status
    /// recheck; client paths are ignored and never become an authority.
    fn worktree_finish(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let worktree_id = require_str(params, "worktree_id")?;
        let action = parse_worktree_finish_action(&require_str(params, "action")?)?;
        let (project_path, base_branch) = self
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .map(|project| (project.repo_path.clone(), project.base_branch.clone()))
            .ok_or("unknown project_id")?;
        let canonical_project_path = project_path.display().to_string();

        self.require_store()?;
        // A completed record makes the mutation idempotent. A pending record is
        // the crash/failure-safe resume point and uses only the same server ids.
        if let Some(record) = self.archived_worktrees.get(&worktree_id).cloned() {
            if record.project_path == canonical_project_path {
                if record.action != action {
                    return Err(format!(
                        "worktree.finish already started with action {:?}",
                        record.action
                    ));
                }
                if record.status == WorktreeFinishStatus::Archived {
                    return Ok(archived_worktree_json(&record));
                }
                return self.resume_worktree_finish(
                    &project_id,
                    &project_path,
                    &base_branch,
                    record,
                );
            }
        }

        let external = self
            .external_worktrees(&project_id, true)?
            .into_iter()
            .find(|worktree| worktree.id == worktree_id)
            .ok_or_else(|| format!("unknown worktree_id: {worktree_id}"))?;

        ensure_worktree_finish_eligible(&external, action, &base_branch)?;
        let dirty_metadata = external.clone();
        if matches!(
            action,
            WorktreeFinishAction::Push | WorktreeFinishAction::Merge
        ) && external.dirty_files > 0
        {
            checkpoint_worktree(&external.path, action)?;
        }
        let head_sha = git_stdout(&external.path, &["rev-parse", "HEAD"])?
            .trim()
            .to_string();

        let record = PersistedArchivedWorktree {
            status: WorktreeFinishStatus::Pending,
            project_path: canonical_project_path,
            worktree_id: external.id,
            worktree_name: external.name,
            worktree_path: external.path.display().to_string(),
            branch: external.branch,
            head_sha,
            upstream: dirty_metadata.upstream,
            unpushed: dirty_metadata.unpushed,
            dirty_files: dirty_metadata.dirty_files,
            uncommitted_files: dirty_metadata.uncommitted.files_changed,
            uncommitted_insertions: dirty_metadata.uncommitted.insertions,
            uncommitted_deletions: dirty_metadata.uncommitted.deletions,
            action,
            archived_at: None,
        };
        self.store
            .as_ref()
            .expect("store required before destructive git mutation")
            .save_archived_worktree(&record)
            .map_err(|error| format!("worktree finish intent store: {error}"))?;
        self.archived_worktrees
            .insert(record.worktree_id.clone(), record.clone());
        self.resume_worktree_finish(&project_id, &project_path, &base_branch, record)
    }

    fn resume_worktree_finish(
        &mut self,
        project_id: &str,
        project_path: &std::path::Path,
        base_branch: &str,
        mut record: PersistedArchivedWorktree,
    ) -> Result<Value, String> {
        let worktree_path = validate_finish_record_path(&record, project_path)?;

        match record.action {
            WorktreeFinishAction::Cleanup => {
                if worktree_path.exists() {
                    remove_registered_worktree(project_path, &worktree_path, false)?;
                }
            }
            WorktreeFinishAction::Push => {
                if worktree_path.exists() {
                    crate::gitgui::push(&worktree_path, false)?;
                    remove_registered_worktree(project_path, &worktree_path, false)?;
                }
            }
            WorktreeFinishAction::Merge => {
                let branch_exists = record
                    .branch
                    .as_deref()
                    .map(|branch| local_branch_exists(project_path, branch))
                    .transpose()?
                    .unwrap_or(false);
                if !worktree_path.exists() && branch_exists {
                    return Err(
                        "worktree.finish merge lost its worktree before branch deletion"
                            .to_string(),
                    );
                }
                if worktree_path.exists() {
                    let deleted_branch =
                        if let Some(branch) = record.branch.as_deref().filter(|_| branch_exists) {
                            merge_external_branch(project_path, branch, base_branch)?;
                            delete_local_branch_for_finish(project_path, branch, &record.head_sha)?;
                            true
                        } else {
                            false
                        };
                    if let Err(remove_error) =
                        remove_registered_worktree(project_path, &worktree_path, true)
                    {
                        restore_finish_branch_after_removal_failure(
                            project_path,
                            &record,
                            deleted_branch,
                            &remove_error,
                        )?;
                        return Err(remove_error);
                    }
                }
            }
            WorktreeFinishAction::Delete => {
                let branch_exists = record
                    .branch
                    .as_deref()
                    .map(|branch| local_branch_exists(project_path, branch))
                    .transpose()?
                    .unwrap_or(false);
                if !worktree_path.exists() && branch_exists {
                    return Err(
                        "worktree.finish delete lost its worktree before branch deletion"
                            .to_string(),
                    );
                }
                if worktree_path.exists() {
                    let deleted_branch =
                        if let Some(branch) = record.branch.as_deref().filter(|_| branch_exists) {
                            delete_local_branch_for_finish(project_path, branch, &record.head_sha)?;
                            true
                        } else {
                            false
                        };
                    if let Err(remove_error) =
                        remove_registered_worktree(project_path, &worktree_path, true)
                    {
                        restore_finish_branch_after_removal_failure(
                            project_path,
                            &record,
                            deleted_branch,
                            &remove_error,
                        )?;
                        return Err(remove_error);
                    }
                }
            }
        }

        record.status = WorktreeFinishStatus::Archived;
        record.archived_at = Some(now_rfc3339());
        self.store
            .as_ref()
            .expect("store required before destructive git mutation")
            .save_archived_worktree(&record)
            .map_err(|error| format!("worktree archive store: {error}"))?;
        self.archived_worktrees
            .insert(record.worktree_id.clone(), record.clone());
        self.reap_orphaned_terminals();
        self.invalidate_external_scan(project_id);
        self.persist_attention();
        Ok(archived_worktree_json(&record))
    }

    fn recover_completed_worktree_finishes(&mut self) {
        let recoverable = self
            .archived_worktrees
            .values()
            .filter(|record| {
                record.status == WorktreeFinishStatus::Pending
                    && finish_git_steps_are_complete(record)
            })
            .map(|record| record.worktree_id.clone())
            .collect::<Vec<_>>();
        for worktree_id in recoverable {
            let mut record = self.archived_worktrees[&worktree_id].clone();
            record.status = WorktreeFinishStatus::Archived;
            record.archived_at = Some(now_rfc3339());
            let result = self
                .store
                .as_ref()
                .expect("recovery only runs with a store")
                .save_archived_worktree(&record);
            match result {
                Ok(()) => {
                    self.archived_worktrees.insert(worktree_id, record);
                }
                Err(error) => {
                    eprintln!("recover worktree finish {worktree_id}: {error}");
                }
            }
        }
    }

    /// `entity.seen` — the human has looked at this run/plan/worktree as it
    /// stands. Versioned against the entity's state clock, so a later change
    /// makes it unseen again rather than staying read forever.
    fn entity_seen(&mut self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
        self.see_attention(&entity_id);
        Ok(json!({ "ok": true }))
    }

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
            // The branch this diff is anchored on, so the surface can name it
            // instead of saying "the base branch".
            "base_branch": base,
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
    /// `plan_id`): empty for adopted runs, single-doc plans, and orphaned links.
    fn owning_plan_stage_docs(&self, run: &ActiveRun) -> Vec<StageDoc> {
        run.run
            .plan_id
            .as_ref()
            .and_then(|pid| self.plans.get(&pid.0))
            .map(|plan| plan.stages.clone())
            .unwrap_or_default()
    }

    /// Canonical conversation owner for a run. Planned runs are implementation
    /// lineage of the Issue and therefore project the Issue thread; planless
    /// adopted runs remain independent worktree entities.
    fn conversation_thread_for_run<'a>(&'a self, run: &'a ActiveRun) -> &'a crate::thread::Thread {
        run.run
            .plan_id
            .as_ref()
            .and_then(|issue_id| self.plans.get(&issue_id.0))
            .map(|issue| &issue.thread)
            .unwrap_or(&run.thread)
    }

    fn record_issue_current_stage_started(
        &mut self,
        run_id: &str,
        stages: &[StageDoc],
    ) -> Result<(), String> {
        let issue_id = self
            .runs
            .get(run_id)
            .and_then(|run| run.run.plan_id.as_ref())
            .map(|id| id.0.clone());
        let Some(issue_id) = issue_id else {
            return Ok(());
        };
        let mut issue = self.take_plan(&issue_id)?;
        if let Some(run) = self.runs.get(run_id) {
            record_current_stage_started(&mut issue.thread, run, stages);
        }
        let (_, persisted) = self.finish_plan_mutation(issue_id, issue);
        persisted
    }

    /// Queue a plan's turn for its planning worktree's agent. A plan whose
    /// worktree is gone has no agent to hear it; the turn is dropped rather
    /// than delivered somewhere it does not belong.
    fn queue_plan_turn(&mut self, plan_id: &str, active: &ActivePlan, turn: AgentTurn) {
        if let Some(pending) = PendingAgentTurn::for_plan(plan_id, active, turn) {
            self.pending_agent_turns.push(pending);
        }
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
        let (mut active, turn) = self
            .orch_for(&project_id)?
            .dispatch_plan(PlanId::new(&plan_id), goal, &base, model_choice)
            .map_err(err)?;
        self.entity_project
            .insert(plan_id.clone(), project_id.clone());
        self.queue_plan_turn(&plan_id, &active, turn);
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
        let mut view = self.plan_view(&plan_id, active, ThreadDetail::Full);
        // The detail poll's optional cursor: ship only conversation items the
        // client does not already hold. Absent → the full backward-compatible
        // thread.
        if let Some(after_sequence) = thread_cursor(params) {
            view.as_object_mut()
                .expect("plan_view returns an object")
                .insert(
                    "thread".to_string(),
                    active.thread.wire_value_after(after_sequence),
                );
        }
        Ok(view)
    }

    fn plan_list(&self) -> Value {
        let plans: Vec<Value> = self
            .plans
            .iter()
            .map(|(id, active)| self.plan_view(id, active, ThreadDetail::Digest))
            .collect();
        json!({ "plans": plans })
    }

    fn issue_list(&self) -> Value {
        let issues: Vec<Value> = self
            .plans
            .iter()
            .map(|(id, active)| self.plan_view(id, active, ThreadDetail::Digest))
            .collect();
        json!({ "issues": issues, "plans": issues })
    }

    fn issue_stages(&mut self, params: &Value) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        let issue = self.plans.get(&issue_id).ok_or("unknown issue_id")?;
        if !issue.is_multi_stage() {
            return Err("not a multi-stage issue".to_string());
        }
        let implementation = self.current_issue_implementation(&issue_id);
        let stages = issue
            .stages
            .iter()
            .map(|doc| {
                let progress = implementation.and_then(|run| run.stage_progress(&doc.id));
                let mut stage = plan_stage_json(issue, doc);
                let object = stage
                    .as_object_mut()
                    .expect("plan_stage_json returns an object");
                object.insert(
                    "approval".to_string(),
                    json!(stage_doc_state_str(&doc.state)),
                );
                object.insert(
                    "execution".to_string(),
                    json!(progress.map_or("pending", canonical_stage_execution)),
                );
                object.insert(
                    "start_sha".to_string(),
                    json!(progress.and_then(|p| p.start_sha.as_ref())),
                );
                object.insert(
                    "built_sha".to_string(),
                    json!(progress.and_then(|p| p.built_sha.as_ref())),
                );
                object.insert(
                    "completion_sha".to_string(),
                    json!(progress.and_then(|p| p.completion_sha.as_ref())),
                );
                object.insert(
                    "publication".to_string(),
                    json!(progress.map(|p| p.publication)),
                );
                object.insert(
                    "validation".to_string(),
                    json!(progress.and_then(|p| p.validation.as_ref())),
                );
                object.insert(
                    "invalidation_reason".to_string(),
                    json!(progress.and_then(|p| p.invalidation_reason.as_ref())),
                );
                object.insert(
                    "comments".to_string(),
                    json!(issue
                        .comments
                        .iter()
                        .filter(|comment| comment.stage_id == doc.id)
                        .map(comment_json)
                        .collect::<Vec<_>>()),
                );
                stage
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "issue_id": issue_id,
            "plan_id": issue_id,
            "implementation_id": implementation.map(|run| run.run.id.0.clone()),
            "auto_advance": implementation.is_some_and(|run| run.auto_advance),
            "stages": stages,
        }))
    }

    fn current_issue_implementation_id(&self, issue_id: &str) -> Option<String> {
        self.current_issue_implementation(issue_id)
            .filter(|run| !run.run.state.is_terminal())
            .map(|run| run.run.id.0.clone())
    }

    fn issue_view_full(&self, issue_id: &str) -> Result<Value, String> {
        let issue = self.plans.get(issue_id).ok_or("unknown issue_id")?;
        Ok(self.plan_view(issue_id, issue, ThreadDetail::Full))
    }

    fn issue_implement_all(&mut self, params: &Value) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        self.arm_issue_scheduler(&issue_id, ImplementationIntent::All)?;
        if let Err(error) = self.advance_issue_scheduler(&issue_id, params) {
            self.block_issue_scheduler(&issue_id, None, &error);
            return Err(error);
        }
        self.issue_view_full(&issue_id)
    }

    fn issue_implement_stage(&mut self, params: &Value) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let issue = self.plans.get(&issue_id).ok_or("unknown issue_id")?;
        let index = issue
            .stages
            .iter()
            .position(|stage| stage.id == stage_id)
            .ok_or_else(|| format!("unknown stage_id: {stage_id}"))?;
        if issue.stages[index].state != StageDocState::Approved {
            return Err(format!("stage {stage_id} is not approved"));
        }
        if self.current_issue_implementation_id(&issue_id).is_none() && index != 0 {
            return Err(format!(
                "cannot implement stage {stage_id}: no current implementation contains its completed predecessors"
            ));
        }
        self.arm_issue_scheduler(&issue_id, ImplementationIntent::Stage(stage_id.clone()))?;
        if let Err(error) = self.advance_issue_scheduler(&issue_id, params) {
            self.block_issue_scheduler(&issue_id, Some(stage_id), &error);
            return Err(error);
        }
        self.issue_view_full(&issue_id)
    }

    /// Persist scheduler intent before any worktree/git/agent side effect. The
    /// Issue record is the recovery journal; run.auto_advance is only the live
    /// implementation's execution flag.
    fn arm_issue_scheduler(
        &mut self,
        issue_id: &str,
        intent: ImplementationIntent,
    ) -> Result<(), String> {
        let mut issue = self.take_plan(issue_id)?;
        if issue.plan.state != PlanState::Approved {
            self.plans.insert(issue_id.to_string(), issue);
            return Err("only a ready Issue can be implemented".to_string());
        }
        issue.plan.implementation_intent = intent;
        issue.plan.implementation_activity = ImplementationActivity::Preparing;
        let (_, persisted) = self.finish_plan_mutation(issue_id.to_string(), issue);
        persisted
    }

    fn set_issue_scheduler_activity(
        &mut self,
        issue_id: &str,
        intent: Option<ImplementationIntent>,
        activity: ImplementationActivity,
    ) -> Result<(), String> {
        let mut issue = self.take_plan(issue_id)?;
        if let Some(intent) = intent {
            issue.plan.implementation_intent = intent;
        }
        issue.plan.implementation_activity = activity;
        let (_, persisted) = self.finish_plan_mutation(issue_id.to_string(), issue);
        persisted
    }

    fn block_issue_scheduler(&mut self, issue_id: &str, stage_id: Option<String>, reason: &str) {
        let stage_id = stage_id
            .or_else(|| {
                self.plans
                    .get(issue_id)
                    .and_then(|issue| issue.stages.first())
                    .map(|stage| stage.id.clone())
            })
            .unwrap_or_default();
        if let Err(error) = self.set_issue_scheduler_activity(
            issue_id,
            None,
            ImplementationActivity::Blocked {
                stage_id,
                reason: reason.to_string(),
            },
        ) {
            eprintln!("issue scheduler {issue_id}: could not persist failure: {error}");
        }
    }

    /// Reconcile one Issue's durable intent with its implementation lineage.
    /// This is deliberately idempotent: boot, approval, and completion may all
    /// call it, but the single-active-writer gate prevents duplicate checkouts.
    fn advance_issue_scheduler(&mut self, issue_id: &str, request: &Value) -> Result<(), String> {
        let intent = self
            .plans
            .get(issue_id)
            .ok_or("unknown issue_id")?
            .plan
            .implementation_intent
            .clone();
        if intent == ImplementationIntent::None {
            return Ok(());
        }

        let target_stage = match &intent {
            ImplementationIntent::Stage(stage_id) => Some(stage_id.clone()),
            ImplementationIntent::All => self.plans[issue_id]
                .stages
                .iter()
                .find(|doc| {
                    self.current_issue_implementation(issue_id)
                        .and_then(|run| run.stage_progress(&doc.id))
                        .is_none_or(|progress| {
                            progress.state != StageProgressState::Validated { passed: true }
                                || progress.invalidation_reason.is_some()
                        })
                })
                .map(|doc| doc.id.clone()),
            ImplementationIntent::None => None,
        };
        let Some(target_stage) = target_stage else {
            return self.set_issue_scheduler_activity(
                issue_id,
                Some(ImplementationIntent::None),
                ImplementationActivity::Idle,
            );
        };
        let approved = self.plans[issue_id]
            .stages
            .iter()
            .find(|stage| stage.id == target_stage)
            .is_some_and(|stage| stage.state == StageDocState::Approved);
        if !approved {
            return self.set_issue_scheduler_activity(
                issue_id,
                None,
                ImplementationActivity::WaitingApproval(target_stage),
            );
        }

        let run_id = match self.current_issue_implementation_id(issue_id) {
            Some(run_id) => run_id,
            None => {
                self.set_issue_scheduler_activity(
                    issue_id,
                    None,
                    ImplementationActivity::Preparing,
                )?;
                let created = self.run_create(&alias_param(request, "issue_id", "plan_id"))?;
                created
                    .get("run_id")
                    .and_then(Value::as_str)
                    .ok_or("run.create returned no run_id")?
                    .to_string()
            }
        };

        self.ensure_issue_implementation_worktree(issue_id, &run_id)?;

        match intent {
            ImplementationIntent::Stage(stage_id) => {
                let already_started = self.runs[&run_id].stage_progress(&stage_id).is_some();
                if !already_started {
                    let mut params = request.clone();
                    let object = params
                        .as_object_mut()
                        .ok_or("issue params must be an object")?;
                    object.insert("run_id".to_string(), json!(run_id));
                    object.insert("stage_id".to_string(), json!(stage_id));
                    self.run_stage_dispatch(&params)?;
                }
                self.set_issue_scheduler_activity(
                    issue_id,
                    Some(ImplementationIntent::None),
                    ImplementationActivity::Idle,
                )
            }
            ImplementationIntent::All => {
                let mut params = request.clone();
                let object = params
                    .as_object_mut()
                    .ok_or("issue params must be an object")?;
                object.insert("run_id".to_string(), json!(run_id.clone()));
                object.insert("enabled".to_string(), json!(true));
                self.run_set_auto_advance(&params)?;
                self.refresh_issue_scheduler_activity(issue_id)
            }
            ImplementationIntent::None => Ok(()),
        }
    }

    fn ensure_issue_implementation_worktree(
        &mut self,
        issue_id: &str,
        run_id: &str,
    ) -> Result<(), String> {
        if self
            .runs
            .get(run_id)
            .is_some_and(|run| run.worktree.path.exists())
        {
            return Ok(());
        }
        let project_id = self.project_of(run_id)?;
        let mut active = self.take_run(run_id)?;
        let restored = if active.adopted {
            Err(
                "adopted worktree is missing; its original checkout cannot be recreated safely"
                    .to_string(),
            )
        } else {
            self.orch_for(&project_id)?
                .restore_run_worktree(&active.worktree)
                .map_err(err)
        };
        match restored {
            Ok(worktree) => {
                active.worktree = worktree;
                active.last_error = None;
                let (_, persisted) = self.finish_run_mutation(run_id.to_string(), active);
                persisted?;
                let mut issue = self.take_plan(issue_id)?;
                issue.thread.push_event_with_links(
                    crate::thread::ThreadEventKind::WorktreeRecovered,
                    Some("Recreated the Issue worktree from its original branch".to_string()),
                    None,
                    None,
                    vec![crate::thread::ThreadLink::Run {
                        run_id: run_id.to_string(),
                    }],
                    now_rfc3339(),
                );
                let (_, persisted) = self.finish_plan_mutation(issue_id.to_string(), issue);
                persisted
            }
            Err(error) => {
                let affected = self.reconcile_missing_run_worktree(run_id, &mut active);
                active.last_error = Some(format!("worktree recovery failed: {error}"));
                if !active.run.state.is_terminal() {
                    active
                        .run
                        .apply(RunEvent::Archive)
                        .map_err(|error| error.to_string())?;
                }
                let (_, persisted) = self.finish_run_mutation(run_id.to_string(), active);
                persisted?;
                let mut issue = self.take_plan(issue_id)?;
                let mut links = vec![crate::thread::ThreadLink::Run {
                    run_id: run_id.to_string(),
                }];
                links.extend(
                    issue
                        .stages
                        .iter()
                        .filter(|stage| affected.contains(&stage.id))
                        .map(|stage| crate::thread::ThreadLink::PlanStage {
                            plan_id: issue_id.to_string(),
                            stage_id: stage.id.clone(),
                            path: stage.path.clone(),
                        }),
                );
                issue.thread.push_event_with_links(
                    crate::thread::ThreadEventKind::RecoveryFailed,
                    Some(format!(
                        "Issue worktree recovery failed: {error}. {} stage(s) are incomplete and dependent stages are blocked",
                        affected.len()
                    )),
                    None,
                    None,
                    links,
                    now_rfc3339(),
                );
                let (_, issue_persisted) = self.finish_plan_mutation(issue_id.to_string(), issue);
                issue_persisted?;
                Err(format!("Issue worktree recovery failed: {error}"))
            }
        }
    }

    fn refresh_issue_scheduler_activity(&mut self, issue_id: &str) -> Result<(), String> {
        let Some(issue) = self.plans.get(issue_id) else {
            return Err("unknown issue_id".to_string());
        };
        if issue.plan.implementation_intent == ImplementationIntent::None {
            return Ok(());
        }
        let Some(run) = self.current_issue_implementation(issue_id) else {
            return Ok(());
        };
        let (intent, activity) = match run.run.state {
            RunState::Review | RunState::Merged => (
                Some(ImplementationIntent::None),
                ImplementationActivity::Idle,
            ),
            RunState::StageGate => {
                let next = issue
                    .stages
                    .iter()
                    .find(|doc| {
                        run.stage_progress(&doc.id).is_none_or(|progress| {
                            progress.state != StageProgressState::Validated { passed: true }
                                || progress.invalidation_reason.is_some()
                        })
                    })
                    .map(|doc| (doc.id.clone(), doc.state));
                match next {
                    Some((stage_id, StageDocState::Planned)) => {
                        (None, ImplementationActivity::WaitingApproval(stage_id))
                    }
                    Some((stage_id, StageDocState::Approved)) => {
                        (None, ImplementationActivity::Running(stage_id))
                    }
                    None => (
                        Some(ImplementationIntent::None),
                        ImplementationActivity::Idle,
                    ),
                }
            }
            RunState::Building => (
                None,
                ImplementationActivity::Running(
                    run.current_stage_id
                        .clone()
                        .unwrap_or_else(|| "implementation".into()),
                ),
            ),
            RunState::Blocked
            | RunState::Failed
            | RunState::IdleUnreported
            | RunState::Interrupted
            | RunState::Abandoned
            | RunState::Archived => (
                None,
                ImplementationActivity::Blocked {
                    stage_id: run.current_stage_id.clone().unwrap_or_default(),
                    reason: run.last_error.clone().unwrap_or_else(|| {
                        format!("implementation is {}", run_state_str(&run.run.state))
                    }),
                },
            ),
            RunState::Created => (None, ImplementationActivity::Preparing),
        };
        self.set_issue_scheduler_activity(issue_id, intent, activity)
    }

    fn issue_set_auto_advance(&mut self, params: &Value) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        let run_id = self
            .current_issue_implementation_id(&issue_id)
            .ok_or("issue has no active implementation")?;
        let mut run_params = params.clone();
        run_params
            .as_object_mut()
            .ok_or("issue params must be an object")?
            .insert("run_id".to_string(), json!(run_id));
        self.run_set_auto_advance(&run_params)?;
        self.issue_view_full(&issue_id)
    }

    fn issue_stage_diff(&self, params: &Value) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        let stage_id = require_str(params, "stage_id")?;
        // Resolve the lineage that actually owns this immutable boundary, not
        // merely the newest attempt. A later failed/restarted implementation
        // must not hide a completed stage from an earlier retained lineage.
        let mut lineages = self
            .runs
            .values()
            .filter(|run| run.run.plan_id.as_ref().map(|id| id.0.as_str()) == Some(&issue_id))
            .filter(|run| {
                run.stage_progress(&stage_id).is_some_and(|progress| {
                    progress.start_sha.is_some() && progress.completion_sha.is_some()
                })
            })
            .collect::<Vec<_>>();
        lineages.sort_by_key(|run| {
            self.entity_created_at
                .get(&run.run.id.0)
                .cloned()
                .unwrap_or_default()
        });
        let run_id = lineages
            .last()
            .map(|run| run.run.id.0.clone())
            .or_else(|| {
                self.current_issue_implementation(&issue_id)
                    .map(|run| run.run.id.0.clone())
            })
            .ok_or("issue has no implementation lineage")?;
        let mut run_params = params.clone();
        run_params
            .as_object_mut()
            .ok_or("issue params must be an object")?
            .insert("run_id".to_string(), json!(run_id));
        let mut result = self.run_stage_diff(&run_params)?;
        result
            .as_object_mut()
            .expect("run stage diff returns an object")
            .insert("issue_id".to_string(), json!(issue_id));
        Ok(result)
    }

    fn issue_run_action(&mut self, params: &Value, action: &str) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        let run_id = self
            .current_issue_implementation_id(&issue_id)
            .ok_or("issue has no active implementation")?;
        let mut run_params = params.clone();
        run_params
            .as_object_mut()
            .ok_or("issue params must be an object")?
            .insert("run_id".to_string(), json!(run_id));
        match action {
            "fix" => self.run_stage_fix(&run_params)?,
            "diff" => self.run_diff(&run_params)?,
            "request_changes" => self.run_request_changes(&run_params)?,
            "git_action" => self.run_git_action(&run_params)?,
            _ => unreachable!("known issue run action"),
        };
        self.issue_view_full(&issue_id)
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
        Ok(json!({ "issue_id": plan_id, "plan_id": plan_id, "stages": stages }))
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
        Ok(json!({
            "issue_id": plan_id,
            "plan_id": plan_id,
            "stage_id": stage_id,
            "path": path,
            "contents": contents,
        }))
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
        if outcome.is_ok() {
            active.thread.push_event(
                crate::thread::ThreadEventKind::Approved,
                Some("Plan approved".to_string()),
                None,
                None,
                now_rfc3339(),
            );
        }
        let (view, persisted) = self.finish_plan_mutation(plan_id, active);
        outcome?;
        persisted?;
        Ok(view)
    }

    /// Send a batch of plan notes back to a fresh revision session.
    fn plan_send_notes(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let messages = parse_thread_inputs(params, crate::thread::ArtifactKind::Plan, "comments")?;
        let project_id = self.project_of(&plan_id)?;
        let mut active = self.take_plan(&plan_id)?;
        append_user_thread_messages(&mut active.thread, messages);
        let outcome = (|| -> Result<(), String> {
            let store = self.require_store()?;
            let turn = self
                .orch_for(&project_id)?
                .send_plan_notes(&mut active, store, NEW_THREAD_MESSAGES_PROMPT)
                .map_err(err)?;
            self.queue_plan_turn(&plan_id, &active, turn);
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
        let stage_title = active
            .stages
            .iter()
            .find(|stage| stage.id == stage_id)
            .map(|stage| stage.title.clone())
            .unwrap_or_else(|| stage_id.clone());
        let outcome = self
            .orch_for(&project_id)
            .and_then(|orch| orch.approve_plan_stage(&mut active, &stage_id).map_err(err));
        if outcome.is_ok() {
            active.thread.push_event(
                crate::thread::ThreadEventKind::StageApproved,
                Some(format!("Approved stage “{stage_title}”")),
                None,
                None,
                now_rfc3339(),
            );
        }
        let (view, persisted) = self.finish_plan_mutation(plan_id.clone(), active);
        outcome?;
        persisted?;
        // Implement All remains armed while it waits on an unapproved stage.
        // Approval is durable before this scheduler hop, so a restart can
        // safely observe the approved doc and resume the same intent.
        let waiting_runs: Vec<String> = self
            .runs
            .iter()
            .filter(|(_, run)| {
                run.auto_advance
                    && run.run.state == RunState::StageGate
                    && run.run.plan_id.as_ref().map(|id| id.0.as_str()) == Some(plan_id.as_str())
            })
            .map(|(run_id, _)| run_id.clone())
            .collect();
        for run_id in waiting_runs {
            self.auto_advance_run(&run_id);
        }
        if self
            .plans
            .get(&plan_id)
            .is_some_and(|issue| issue.plan.implementation_intent != ImplementationIntent::None)
        {
            let request = json!({ "issue_id": plan_id });
            if let Err(error) = self.advance_issue_scheduler(&plan_id, &request) {
                self.block_issue_scheduler(&plan_id, Some(stage_id), &error);
                return Err(error);
            }
        }
        Ok(view)
    }

    /// Send a stage's open comments to a fresh plan-revision session (the open
    /// comments ARE the payload).
    fn plan_stage_send_notes(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let project_id = self.project_of(&plan_id)?;
        let mut active = self.take_plan(&plan_id)?;
        if let Some(stage) = active
            .stages
            .iter()
            .find(|stage| stage.id == stage_id)
            .cloned()
        {
            let comments: Vec<StageComment> = active
                .open_comments_for(&stage_id)
                .into_iter()
                .cloned()
                .collect();
            append_stage_comments_to_thread(&mut active.thread, &stage, &comments);
        }
        let outcome = (|| -> Result<(), String> {
            let store = self.require_store()?;
            let turn = self
                .orch_for(&project_id)?
                .send_plan_stage_notes(&mut active, store, &stage_id)
                .map_err(err)?;
            self.queue_plan_turn(&plan_id, &active, turn);
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
        active.thread.post_user(&message, None, now_rfc3339());
        let outcome = (|| -> Result<(), String> {
            let store = self.require_store()?;
            let turn = self
                .orch_for(&project_id)?
                .message_plan(&mut active, store, NEW_THREAD_MESSAGES_PROMPT)
                .map_err(err)?;
            self.queue_plan_turn(&plan_id, &active, turn);
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
        if outcome.is_ok() {
            active.thread.push_event(
                crate::thread::ThreadEventKind::Abandoned,
                Some("Plan abandoned".to_string()),
                None,
                None,
                now_rfc3339(),
            );
        }
        let (view, persisted) = self.finish_plan_mutation(plan_id, active);
        outcome?;
        persisted?;
        Ok(view)
    }

    /// Archive a completed plan without changing its lifecycle state or
    /// deleting canonical docs/run history. Repeating the request preserves the
    /// first archive timestamp.
    fn plan_archive(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let mut active = self.take_plan(&plan_id)?;
        let outcome = if active.plan.archived_at.is_some() {
            Ok(())
        } else if self.plan_implementation_complete(&plan_id, &active) {
            active.plan.archived_at = Some(now_rfc3339());
            Ok(())
        } else {
            Err("plan.archive: plan implementation is incomplete".to_string())
        };
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
        self.entity_state_changed_at.remove(&plan_id);
        self.entity_last_state.remove(&plan_id);
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
            let thread_anchor = anchor.as_ref().map(|anchor| crate::thread::MessageAnchor {
                artifact: crate::thread::ArtifactKind::Plan,
                revision_id: None,
                path: Some(active.stages[index].path.clone()),
                side: None,
                line_start: None,
                line_end: None,
                heading_path: anchor.heading_path.clone(),
                snippet: anchor.snippet.clone(),
            });
            let comment = StageComment {
                id: active.mint_comment_id(),
                stage_id: stage_id.clone(),
                anchor,
                body: body.clone(),
                state: CommentState::Open,
                agent_reply: None,
            };
            active.comments.push(comment.clone());
            active
                .thread
                .post_user(body.clone(), thread_anchor, now_rfc3339());
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
            let comment = active.comments.remove(index);
            let stage_path = active
                .stages
                .iter()
                .find(|stage| stage.id == comment.stage_id)
                .map(|stage| stage.path.as_str());
            if let Some(message_index) = active.thread.items.iter().rposition(|item| {
                matches!(
                    item,
                    crate::thread::ThreadItem::Message(message)
                        if message.role == crate::thread::MessageRole::User
                            && message.body == comment.body
                            && message.anchor.as_ref().and_then(|anchor| anchor.path.as_deref())
                                == comment.anchor.as_ref().and(stage_path)
                )
            }) {
                active.thread.items.remove(message_index);
            }
            Ok(())
        })();
        let (_, persisted) = self.finish_plan_mutation(plan_id, active);
        outcome?;
        persisted?;
        Ok(json!({ "ok": true }))
    }

    // ---- Run surface ----------------------------------------------------------

    /// Create a run implementing an approved plan. Single-active-writer: a
    /// second concurrent run of the same plan is rejected at dispatch.
    ///
    /// A run always has a plan behind it: the goal-only dispatch is gone, and an
    /// unplanned coding session is now an agent tab (`term.create` with `kind`),
    /// driven by the human who opened it.
    fn run_create(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let source_plan_id = plan_id.clone();
        let requested_choice = model_choice_from(params)?;
        let base_override = params
            .get("base_branch")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let run_id = format!("run-{}", uuid::Uuid::new_v4());

        let (project_id, mut active, turn) = {
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
            let model_choice = if has_agent_choice(params) {
                requested_choice
            } else {
                plan.model_choice.clone()
            };
            let (active, turn) = self
                .orch_for(&project_id)?
                .dispatch_run(
                    RunId::new(&run_id),
                    RunSource {
                        plan,
                        has_active_run,
                    },
                    &base,
                    model_choice,
                    store,
                )
                .map_err(err)?;
            (project_id, active, turn)
        };

        let plan_docs = self.owning_plan_stage_docs(&active);

        self.entity_project
            .insert(run_id.clone(), project_id.clone());
        self.pending_agent_turns
            .push(PendingAgentTurn::for_run(&run_id, &active, turn));
        if self.qa_agent {
            self.qa_drive_run(&project_id, &mut active, &plan_docs)?;
        }
        let (_, persisted) = self.finish_run_mutation(run_id.clone(), active);
        persisted?;
        let mut plan = self.take_plan(&source_plan_id)?;
        plan.thread.push_event_with_links(
            crate::thread::ThreadEventKind::ImplementationStarted,
            Some(format!("Implementation started as {run_id}")),
            None,
            None,
            vec![crate::thread::ThreadLink::Run {
                run_id: run_id.clone(),
            }],
            now_rfc3339(),
        );
        if let Some(run) = self.runs.get(&run_id) {
            record_current_stage_started(&mut plan.thread, run, &plan_docs);
        }
        let (_, plan_persisted) = self.finish_plan_mutation(source_plan_id, plan);
        plan_persisted?;
        self.auto_advance_run(&run_id);
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        Ok(self.run_view(&run_id, active, ThreadDetail::Full))
    }

    fn run_get(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        let mut view = self.run_view(&run_id, active, ThreadDetail::Full);
        // The detail poll's optional cursor: ship only conversation items the
        // client does not already hold. Absent → the full backward-compatible
        // thread.
        if let Some(after_sequence) = thread_cursor(params) {
            view.as_object_mut()
                .expect("run_view returns an object")
                .insert(
                    "thread".to_string(),
                    self.conversation_thread_for_run(active)
                        .wire_value_after(after_sequence),
                );
        }
        Ok(view)
    }

    fn thread_revision(&self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
        let revision_id = require_str(params, "revision_id")?;
        let planned_issue_id = self
            .runs
            .get(&entity_id)
            .and_then(|run| run.run.plan_id.as_ref())
            .map(|id| id.0.as_str());
        let thread = self
            .plans
            .get(&entity_id)
            .map(|active| &active.thread)
            .or_else(|| {
                planned_issue_id.and_then(|id| self.plans.get(id).map(|active| &active.thread))
            })
            .or_else(|| self.runs.get(&entity_id).map(|active| &active.thread))
            .ok_or("unknown conversation owner")?;
        let revision = thread
            .revisions
            .iter()
            .find(|revision| revision.id == revision_id)
            .ok_or("unknown revision_id")?;
        let contents = revision
            .snapshot
            .as_ref()
            .ok_or("revision snapshot is no longer retained")?;
        Ok(json!({
            "revision_id": revision.id,
            "artifact": revision.artifact,
            "created_at": revision.created_at,
            "contents": contents,
        }))
    }

    /// Post a reviewer message to an entity's conversation WITHOUT dispatching
    /// work — the review-surface write path. Resolves a plan OR a run (the
    /// `thread.revision` idiom), appends the body as an unread user message,
    /// and nudges the worktree's live agent in place through its PTY so it
    /// calls `read_unread_messages` — whatever the entity is parked as, because
    /// that agent is the one the human is looking at. Never ends or spawns a
    /// session and never moves plan/run state — with no live agent the message
    /// simply waits for the next session's catch-up. Refused only where no
    /// conversation remains to post to: a terminal or unknown entity.
    fn thread_post(&mut self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
        if let Some(active) = self.plans.get(&entity_id) {
            if active.plan.state.is_terminal() {
                return Err(format!(
                    "thread.post: plan is {} — the conversation is closed",
                    plan_state_str(&active.plan.state)
                ));
            }
            let messages = parse_thread_post_input(params, crate::thread::ArtifactKind::Plan)?;
            let implementation_target =
                self.current_issue_implementation_id(&entity_id)
                    .and_then(|run_id| {
                        self.runs
                            .get(&run_id)
                            .map(|run| (run_id, run.worktree.path.clone()))
                    });
            let mut active = self.take_plan(&entity_id)?;
            append_user_thread_messages(&mut active.thread, messages);
            if let Some((run_id, worktree_path)) = implementation_target {
                // The Issue owns the conversation, but its live implementation
                // owns the checkout/PTY. Addressing thread.post to the Issue
                // must therefore wake that implementation agent.
                nudge_live_agent_tab(&self.tabs, &worktree_path, &run_id);
            } else if let Some(worktree) = &active.worktree {
                nudge_live_agent_tab(&self.tabs, &worktree.path, &entity_id);
            }
            let (view, persisted) = self.finish_plan_mutation(entity_id, active);
            persisted?;
            return Ok(view);
        }
        if let Some(active) = self.runs.get(&entity_id) {
            if active.run.state.is_terminal() {
                return Err(format!(
                    "thread.post: run is {} — the conversation is closed",
                    run_state_str(&active.run.state)
                ));
            }
            let messages = parse_thread_post_input(params, crate::thread::ArtifactKind::Diff)?;
            let issue_id = active.run.plan_id.as_ref().map(|id| id.0.clone());
            let worktree_path = active.worktree.path.clone();
            if let Some(issue_id) = issue_id.filter(|id| self.plans.contains_key(id)) {
                let mut issue = self.take_plan(&issue_id)?;
                append_user_thread_messages(&mut issue.thread, messages);
                nudge_live_agent_tab(&self.tabs, &worktree_path, &entity_id);
                let (_, persisted) = self.finish_plan_mutation(issue_id, issue);
                persisted?;
                let active = self.runs.get(&entity_id).ok_or("unknown run_id")?;
                return Ok(self.run_view(&entity_id, active, ThreadDetail::Full));
            }
            let mut active = self.take_run(&entity_id)?;
            append_user_thread_messages(&mut active.thread, messages);
            nudge_live_agent_tab(&self.tabs, &active.worktree.path, &entity_id);
            let (view, persisted) = self.finish_run_mutation(entity_id, active);
            persisted?;
            return Ok(view);
        }
        Err("unknown conversation owner".to_string())
    }

    fn run_diff(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let project_id = self.project_of(&run_id)?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        let diff = self.orch_for(&project_id)?.run_diff(active).map_err(err)?;
        Ok(diff_json(&diff))
    }

    /// Immutable stage review surface. Unlike `run.diff`, this never reads the
    /// working directory or current HEAD: it resolves only the two object ids
    /// persisted when the stage was dispatched and successfully validated.
    fn run_stage_diff(&self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        let progress = active
            .stage_progress(&stage_id)
            .ok_or_else(|| format!("unknown stage_id: {stage_id}"))?;
        let (Some(start_sha), Some(completion_sha)) =
            (&progress.start_sha, &progress.completion_sha)
        else {
            return Ok(json!({
                "run_id": run_id,
                "stage_id": stage_id,
                "status": "unavailable",
                "reason": if progress.publication == crate::run::StagePublication::LegacyUnknown {
                    "legacy_unpinned"
                } else {
                    "stage_not_complete"
                },
                "start_sha": progress.start_sha,
                "completion_sha": progress.completion_sha,
            }));
        };
        let object_database = if active.worktree.path.exists() {
            active.worktree.path.clone()
        } else {
            std::path::PathBuf::from(self.project_path_for(&run_id))
        };
        let diff = crate::diff::diff_between_commits(&object_database, start_sha, completion_sha)
            .map_err(|error| format!("stage diff unavailable: {error}"))?;
        let mut value = diff_json(&diff);
        let object = value.as_object_mut().expect("diff_json returns an object");
        object.insert("run_id".to_string(), json!(run_id));
        object.insert("stage_id".to_string(), json!(stage_id));
        object.insert("status".to_string(), json!("available"));
        object.insert("start_sha".to_string(), json!(start_sha));
        object.insert("completion_sha".to_string(), json!(completion_sha));
        Ok(value)
    }

    /// Send diff comments to the coding agent — from `review` or `building`.
    ///
    /// The comments land on the durable thread first, so the turn that travels
    /// is only ever an instruction to read them: a warm agent gets exactly that,
    /// and a cold one gets it wrapped in enough run context to act on. The
    /// worktree's agent is delivered to, never killed and replaced.
    fn run_request_changes(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let messages = parse_thread_inputs(params, crate::thread::ArtifactKind::Diff, "comments")?;
        let project_id = self.project_of(&run_id)?;
        let plan_docs = {
            let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
            self.owning_plan_stage_docs(active)
        };
        let mut active = self.take_run(&run_id)?;
        let issue_id = active.run.plan_id.as_ref().map(|id| id.0.clone());
        let mut issue = issue_id
            .as_ref()
            .and_then(|issue_id| self.plans.remove(issue_id));
        let legacy_run_thread = issue
            .as_ref()
            .map(|issue| std::mem::replace(&mut active.thread, issue.thread.clone()));
        append_user_thread_messages(&mut active.thread, messages);
        let outcome = (|| -> Result<(), String> {
            let turn = self
                .orch_for(&project_id)?
                .run_request_changes(&mut active, &plan_docs, NEW_THREAD_MESSAGES_PROMPT)
                .map_err(err)?;
            self.pending_agent_turns
                .push(PendingAgentTurn::for_run(&run_id, &active, turn));
            self.qa_drive_run(&project_id, &mut active, &plan_docs)
        })();
        if let (Some(issue), Some(legacy_thread)) = (&mut issue, legacy_run_thread) {
            issue.thread = std::mem::replace(&mut active.thread, legacy_thread);
        }
        let issue_persisted = match (issue_id, issue) {
            (Some(issue_id), Some(issue)) => Some(self.finish_plan_mutation(issue_id, issue).1),
            _ => None,
        };
        let (view, persisted) = self.finish_run_mutation(run_id, active);
        outcome?;
        if let Some(issue_persisted) = issue_persisted {
            issue_persisted?;
        }
        persisted?;
        Ok(view)
    }

    fn run_stage_dispatch(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let model_override = if has_agent_choice(params) {
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
            let turn = self
                .orch_for(&project_id)?
                .dispatch_run_stage(&mut active, &plan_docs, &stage_id, model_override)
                .map_err(err)?;
            self.pending_agent_turns
                .push(PendingAgentTurn::for_run(&run_id, &active, turn));
            self.qa_drive_run(&project_id, &mut active, &plan_docs)
        })();
        let (_, persisted) = self.finish_run_mutation(run_id.clone(), active);
        outcome?;
        persisted?;
        self.record_issue_current_stage_started(&run_id, &plan_docs)?;
        self.auto_advance_run(&run_id);
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        Ok(self.run_view(&run_id, active, ThreadDetail::Full))
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
            let turn = self
                .orch_for(&project_id)?
                .fix_run_stage(&mut active, &plan_docs, &stage_id, &note)
                .map_err(err)?;
            self.pending_agent_turns
                .push(PendingAgentTurn::for_run(&run_id, &active, turn));
            self.qa_drive_run(&project_id, &mut active, &plan_docs)
        })();
        let (_, persisted) = self.finish_run_mutation(run_id.clone(), active);
        outcome?;
        persisted?;
        self.record_issue_current_stage_started(&run_id, &plan_docs)?;
        self.auto_advance_run(&run_id);
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        Ok(self.run_view(&run_id, active, ThreadDetail::Full))
    }

    /// Send a stage's open comments (persisted on the owning plan) to a fresh
    /// mid-run revision session in the RUN's worktree — the stage-gate
    /// analogue of `plan.stage_send_notes`, which is illegal once the plan is
    /// Approved. The run owns the session; the plan owns the docs; the
    /// revision's `done` ingests the rewritten docs back to the store.
    fn run_stage_send_notes(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let project_id = self.project_of(&run_id)?;
        let plan_id = self
            .runs
            .get(&run_id)
            .ok_or("unknown run_id")?
            .run
            .plan_id
            .as_ref()
            .map(|p| p.0.clone())
            .ok_or("this run implements no plan — there are no plan docs to revise")?;
        let mut active = self.take_run(&run_id)?;
        let mut plan = self.plans.remove(&plan_id);
        let outcome = (|| -> Result<(), String> {
            let plan = plan.as_mut().ok_or("unknown plan_id")?;
            let stage = plan
                .stages
                .iter()
                .find(|stage| stage.id == stage_id)
                .cloned()
                .ok_or_else(|| format!("unknown stage_id: {stage_id}"))?;
            let comments: Vec<StageComment> = plan
                .open_comments_for(&stage_id)
                .into_iter()
                .cloned()
                .collect();
            append_stage_comments_to_thread(&mut active.thread, &stage, &comments);
            append_stage_comments_to_thread(&mut plan.thread, &stage, &comments);
            let turn = self
                .orch_for(&project_id)?
                .send_run_stage_notes(&mut active, plan, &stage_id)
                .map_err(err)?;
            self.pending_agent_turns
                .push(PendingAgentTurn::for_run(&run_id, &active, turn));
            if self.qa_agent {
                self.qa_simulate_run_stage_revise(&project_id, &mut active, plan)?;
            }
            Ok(())
        })();
        // Both entities re-insert before any error propagates — the
        // take → finish_mutation invariant covers the plan here too.
        let plan_persisted = plan.map(|plan| self.finish_plan_mutation(plan_id, plan).1);
        let (_, persisted) = self.finish_run_mutation(run_id.clone(), active);
        outcome?;
        if let Some(persisted) = plan_persisted {
            persisted?;
        }
        persisted?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        Ok(self.run_view(&run_id, active, ThreadDetail::Full))
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
        Ok(self.run_view(&run_id, active, ThreadDetail::Full))
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
                let turn = self
                    .orch_for(&project_id)?
                    .dispatch_run_stage(&mut active, &plan_docs, &next, None)
                    .map_err(err)?;
                self.pending_agent_turns
                    .push(PendingAgentTurn::for_run(run_id, &active, turn));
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
            if let Err(e) = self.record_issue_current_stage_started(run_id, &plan_docs) {
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
        let issue_id = active.run.plan_id.as_ref().map(|id| id.0.clone());
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
        let (event, summary) = match (&result, action.as_str()) {
            (Ok(()), "commit") => (
                crate::thread::ThreadEventKind::Committed,
                "Changes committed".to_string(),
            ),
            (Ok(()), "push") => (
                crate::thread::ThreadEventKind::Pushed,
                "Changes committed and pushed".to_string(),
            ),
            (Ok(()), "merge") => (
                crate::thread::ThreadEventKind::Merged,
                "Changes merged into the base branch".to_string(),
            ),
            (Ok(()), "merge_push") => (
                crate::thread::ThreadEventKind::Merged,
                "Changes merged and pushed".to_string(),
            ),
            (Err(error), _) => (
                crate::thread::ThreadEventKind::RunFailed,
                format!("Git action {action} failed: {error}"),
            ),
            _ => unreachable!("validated git action"),
        };
        if result.is_ok() {
            let publication = match action.as_str() {
                "push" => Some(StagePublication::Pushed),
                "merge" | "merge_push" => Some(StagePublication::Merged),
                _ => None,
            };
            if let Some(publication) = publication {
                for progress in &mut active.stages {
                    if progress.completion_sha.is_some() {
                        progress.publication = publication;
                        progress.invalidation_reason = None;
                    }
                }
            }
        }
        let links = issue_id
            .as_ref()
            .and_then(|issue_id| self.plans.get(issue_id).map(|issue| (issue_id, issue)))
            .map(|(issue_id, issue)| {
                let mut links = vec![crate::thread::ThreadLink::Run {
                    run_id: run_id.clone(),
                }];
                links.extend(issue.stages.iter().map(|stage| {
                    crate::thread::ThreadLink::PlanStage {
                        plan_id: issue_id.clone(),
                        stage_id: stage.id.clone(),
                        path: stage.path.clone(),
                    }
                }));
                links
            })
            .unwrap_or_else(|| {
                vec![crate::thread::ThreadLink::Run {
                    run_id: run_id.clone(),
                }]
            });
        if issue_id.is_none() {
            active.thread.push_event_with_links(
                event,
                Some(summary.clone()),
                None,
                None,
                links.clone(),
                now_rfc3339(),
            );
        }
        let merged_worktree = (result.is_ok() && active.run.state == RunState::Merged)
            .then(|| active.worktree.clone());
        let (view, persisted) = self.finish_run_mutation(run_id.clone(), active);
        let issue_persisted = if let Some(issue_id) = issue_id {
            let mut issue = self.take_plan(&issue_id)?;
            issue.thread.push_event_with_links(
                event,
                Some(summary),
                None,
                None,
                links,
                now_rfc3339(),
            );
            Some(self.finish_plan_mutation(issue_id, issue).1)
        } else {
            None
        };
        result?;
        persisted?;
        if let Some(issue_persisted) = issue_persisted {
            issue_persisted?;
        }
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
                self.entity_state_changed_at.remove(run_id);
                self.entity_last_state.remove(run_id);
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
        active.thread.post_user(&message, None, now_rfc3339());
        let outcome = (|| -> Result<(), String> {
            let turn = self
                .orch_for(&project_id)?
                .message_run(&mut active, &plan_docs, NEW_THREAD_MESSAGES_PROMPT)
                .map_err(err)?;
            self.pending_agent_turns
                .push(PendingAgentTurn::for_run(&run_id, &active, turn));
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
        let issue_id = active.run.plan_id.as_ref().map(|id| id.0.clone());
        // Reconcile publication while the checkout and refs are still
        // inspectable. Every Build-owned removal path must decide completion
        // before deleting the evidence it needs to decide it.
        let affected_stages = self.reconcile_missing_run_worktree(&run_id, &mut active);
        let result = self
            .orch_for(&project_id)
            .and_then(|orch| orch.abandon_run(&mut active).map_err(err));
        if result.is_ok() {
            // The worktree removal inside `abandon_run` is best-effort, so the
            // orphan reaper — which only sweeps tabs whose root is GONE —
            // cannot be trusted to take the agent with it. A human who
            // abandoned a run must not keep paying for the agent that was
            // working on it, so the kill is explicit, the way release and
            // delete kill theirs.
            self.close_agent_tab(&active.worktree.path);
            // The run is out of the map, so its lineage closes on the thread
            // this call holds rather than through the owner lookup.
            finish_open_session(&mut active.thread, &now_rfc3339());
            active.thread.push_event(
                crate::thread::ThreadEventKind::Abandoned,
                Some("Run abandoned".to_string()),
                None,
                None,
                now_rfc3339(),
            );
        }
        let (view, persisted) = self.finish_run_mutation(run_id.clone(), active);
        result?;
        persisted?;
        if let Some(issue_id) = issue_id {
            let mut issue = self.take_plan(&issue_id)?;
            let mut links = vec![crate::thread::ThreadLink::Run {
                run_id: run_id.clone(),
            }];
            links.extend(
                issue
                    .stages
                    .iter()
                    .filter(|stage| affected_stages.contains(&stage.id))
                    .map(|stage| crate::thread::ThreadLink::PlanStage {
                        plan_id: issue_id.clone(),
                        stage_id: stage.id.clone(),
                        path: stage.path.clone(),
                    }),
            );
            issue.thread.push_event_with_links(
                crate::thread::ThreadEventKind::WorktreeDeleted,
                Some(format!(
                    "Issue worktree deleted; {} unpublished stage(s) are incomplete",
                    affected_stages.len()
                )),
                None,
                None,
                links,
                now_rfc3339(),
            );
            let (_, issue_persisted) = self.finish_plan_mutation(issue_id, issue);
            issue_persisted?;
        }
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
        // A planned implementation is durable Issue lineage: its immutable
        // stage boundaries and publication evidence must outlive card cleanup.
        // Archived lineages are already filtered from board.list, so preserve
        // the record while keeping the legacy delete call idempotently useful.
        if active.run.plan_id.is_some() {
            return Ok(json!({ "ok": true, "retained_as_issue_lineage": true }));
        }
        let worktree = active.worktree.clone();
        let adopted = active.adopted;
        let project_id = self.entity_project.get(&run_id).cloned();

        if let Some(store) = &self.store {
            store
                .delete_run(&run_id)
                .map_err(|e| format!("run store: {e}"))?;
        }

        let active = self.runs.remove(&run_id).expect("checked above");
        // The run is gone, so its agent's `done` reports would have no owner to
        // route to. Close the worktree's agent — the worktree itself survives
        // (deleting an adopted run's card must never touch the user's files),
        // and reopening the Agent tab there re-adopts.
        self.close_agent_tab(&active.worktree.path);

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
        self.entity_state_changed_at.remove(&run_id);
        self.entity_last_state.remove(&run_id);
        self.run_stat_cache.remove(&run_id);

        if worktree.path.exists() {
            if let Some(pid) = project_id {
                self.invalidate_external_scan(&pid);
            }
        }
        self.reap_orphaned_terminals();
        Ok(json!({ "ok": true }))
    }

    /// Mint a plan-less run around an existing external worktree (`plan_id` None).
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

    /// Finish a completed run through the same durable worktree archive path as
    /// a bare worktree's Done control. The run is removed from the active map
    /// only while the server resolves and executes the id-only finish request;
    /// a pre-mutation failure restores it for retry.
    fn run_finish(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let action_name = require_str(params, "action")?;
        parse_worktree_finish_action(&action_name)?;
        let project_id = self.project_of(&run_id)?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        if !matches!(active.run.state, RunState::Review | RunState::Merged) {
            return Err(format!(
                "run.finish: run is {} — Done requires completed work",
                run_state_str(&active.run.state)
            ));
        }
        if !active.worktree.path.exists() {
            if active.run.state != RunState::Merged {
                return Err("run.finish: worktree no longer exists".to_string());
            }
            let mut active = self.runs.remove(&run_id).expect("checked above");
            active
                .run
                .apply(RunEvent::Archive)
                .map_err(|error| error.to_string())?;
            let (_, persisted) = self.finish_run_mutation(run_id, active);
            persisted?;
            return Ok(json!({ "archived": true }));
        }

        let root = Self::canonical_root(&active.worktree.path);
        let worktree_id = crate::worktree::external_worktree_id(&root);
        let mut active = self.runs.remove(&run_id).expect("checked above");
        self.run_stat_cache.remove(&run_id);
        self.close_agent_tab(&root);
        self.invalidate_external_scan(&project_id);

        let archived_worktree = match self.worktree_finish(&json!({
            "project_id": project_id,
            "worktree_id": worktree_id,
            "action": action_name,
        })) {
            Ok(archived) => archived,
            Err(error) => {
                if root.exists() {
                    self.runs.insert(run_id.clone(), active);
                    self.invalidate_external_scan(&project_id);
                } else {
                    eprintln!("run.finish {run_id}: worktree vanished after failure: {error}");
                }
                return Err(error);
            }
        };

        if active.run.plan_id.is_some() {
            // Plans determine their own Done eligibility from retained run
            // lineage. Keep this run internally as Archived while board.list
            // filters it out; deleting it would make a completed plan look
            // incomplete again.
            active
                .run
                .apply(RunEvent::Archive)
                .map_err(|error| error.to_string())?;
            let (_, persisted) = self.finish_run_mutation(run_id, active);
            persisted?;
            self.reap_orphaned_terminals();
            return Ok(archived_worktree);
        }

        if let Some(store) = &self.store {
            if let Err(error) = store.delete_run(&run_id) {
                // The durable worktree archive is already complete. A stale run
                // record self-heals to Archived on restart because its checkout
                // is gone; do not resurrect it in the live rail now.
                eprintln!("run.finish {run_id}: stale run record: {error}");
            }
        }
        self.entity_project.remove(&run_id);
        self.entity_project_path.remove(&run_id);
        self.entity_created_at.remove(&run_id);
        self.entity_updated_at.remove(&run_id);
        self.entity_state_changed_at.remove(&run_id);
        self.entity_last_state.remove(&run_id);
        self.reap_orphaned_terminals();
        Ok(archived_worktree)
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
        let active = self.runs.remove(&run_id).expect("checked above");
        // Un-adopting hands the worktree back to the human; Build's agent in it
        // reported `done` to a run that no longer exists, so it goes with the
        // run. Reopening the Agent tab there adopts again.
        self.close_agent_tab(&active.worktree.path);
        let project_id = self.entity_project.remove(&run_id);
        self.entity_project_path.remove(&run_id);
        self.entity_created_at.remove(&run_id);
        self.entity_updated_at.remove(&run_id);
        self.entity_state_changed_at.remove(&run_id);
        self.entity_last_state.remove(&run_id);
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
                .filter(|id| self.plans[id].plan.archived_at.is_none())
                .map(|id| {
                    let active = self.plans.get(&id).expect("listed above");
                    self.plan_view(&id, active, ThreadDetail::Digest)
                })
                .collect()
        };
        let runs: Vec<Value> = {
            let ids: Vec<String> = self
                .runs
                .iter()
                .filter(|(_, active)| active.run.state != RunState::Archived)
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .map(|id| {
                    let stat = self.run_stat(&id);
                    let active = self.runs.get(&id).expect("listed above");
                    let mut view = self.run_view(&id, active, ThreadDetail::Digest);
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
            "issues": plans,
            "plans": plans,
            "runs": runs,
            "external_worktrees": external_worktrees,
            "primary_changes": primary_changes,
        })
    }

    /// Archived plans and external worktrees for one project, grouped by kind.
    /// Canonical project path is the durable join because project ids remint.
    fn archive_list(&self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let project = self
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .ok_or("unknown project_id")?;
        let project_path = project.repo_path.display().to_string();
        let plans = self
            .plans
            .iter()
            .filter(|(plan_id, active)| {
                active.plan.archived_at.is_some() && self.project_path_for(plan_id) == project_path
            })
            .map(|(plan_id, active)| self.plan_view(plan_id, active, ThreadDetail::Digest))
            .collect::<Vec<_>>();
        let worktrees = self
            .archived_worktrees
            .values()
            .filter(|record| {
                record.project_path == project_path
                    && record.status == WorktreeFinishStatus::Archived
            })
            .map(archived_worktree_json)
            .collect::<Vec<_>>();
        Ok(json!({ "plans": plans, "worktrees": worktrees }))
    }

    /// A run the user deletes must disappear from Build. Any live run whose
    /// worktree vanished retires to Archived: session ended, git's stale
    /// worktree record pruned — the record stays as quiet history. `Created` is
    /// exempt (its worktree may legitimately not exist yet).
    fn reconcile_missing_run_worktree(&self, run_id: &str, active: &mut ActiveRun) -> Vec<String> {
        let repo_path = self
            .entity_project
            .get(run_id)
            .and_then(|project_id| {
                self.projects
                    .iter()
                    .find(|project| &project.id == project_id)
            })
            .map(|project| project.repo_path.clone());
        let mut affected = Vec::new();
        for progress in &mut active.stages {
            let publication = match (&repo_path, progress.completion_sha.as_deref()) {
                (Some(repo_path), Some(completion_sha)) => classify_stage_publication(
                    repo_path,
                    &active.worktree.branch,
                    &active.worktree.base_branch,
                    completion_sha,
                ),
                _ => StagePublication::Local,
            };
            progress.publication = publication;
            let in_flight = !matches!(
                progress.state,
                StageProgressState::Validated { passed: true }
            );
            if publication == StagePublication::Local || in_flight {
                progress.invalidation_reason = Some(
                    "Issue worktree disappeared before this stage's commits were verified pushed or merged"
                        .to_string(),
                );
                affected.push(progress.stage_id.clone());
            }
        }
        affected
    }

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
            let issue_id = active.run.plan_id.as_ref().map(|id| id.0.clone());
            let affected_stages = self.reconcile_missing_run_worktree(&run_id, &mut active);
            match active.run.apply(RunEvent::Archive) {
                Ok(_) => {
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
            if let Some(issue_id) = issue_id {
                if let Ok(mut issue) = self.take_plan(&issue_id) {
                    issue.thread.push_event_with_links(
                        crate::thread::ThreadEventKind::WorktreeDeleted,
                        Some(format!(
                            "Implementation worktree disappeared; {} stage(s) were reconciled",
                            affected_stages.len()
                        )),
                        None,
                        None,
                        vec![crate::thread::ThreadLink::Run {
                            run_id: run_id.clone(),
                        }],
                        now_rfc3339(),
                    );
                    for stage_id in &affected_stages {
                        if let Some(stage) = issue.stages.iter().find(|stage| &stage.id == stage_id)
                        {
                            issue.thread.push_event_with_links(
                                crate::thread::ThreadEventKind::StageInvalidated,
                                Some(format!("Stage “{}” is incomplete", stage.title)),
                                None,
                                None,
                                vec![crate::thread::ThreadLink::PlanStage {
                                    plan_id: issue_id.clone(),
                                    stage_id: stage.id.clone(),
                                    path: stage.path.clone(),
                                }],
                                now_rfc3339(),
                            );
                        }
                    }
                    let (_, persisted) = self.finish_plan_mutation(issue_id, issue);
                    if let Err(e) = persisted {
                        eprintln!("archive {run_id}: issue event persist failed: {e}");
                    }
                }
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
    /// `thread_detail` picks a bounded digest (list surfaces) or the full
    /// conversation (detail surfaces + mutation responses).
    /// The rail's two facts about an entity: when this stretch of work on it
    /// began (its sort key) and whether the human has seen where it got to. The
    /// seen comparison happens HERE, against the state clock, so every surface
    /// agrees on it rather than each re-deriving it.
    fn attention_json(&self, id: &str) -> Value {
        let attention = self.attention.get(id).cloned().unwrap_or_default();
        let created_at = self
            .entity_created_at
            .get(id)
            .cloned()
            .unwrap_or_else(now_rfc3339);
        let state_changed_at = self
            .entity_state_changed_at
            .get(id)
            .cloned()
            .unwrap_or_else(|| created_at.clone());
        json!({
            "resume_at": attention.sort_key(&created_at),
            "interacted": attention.last_interaction_at.is_some(),
            "seen": attention.has_seen(&state_changed_at),
        })
    }

    fn current_issue_implementation(&self, issue_id: &str) -> Option<&ActiveRun> {
        let mut implementations = self
            .runs
            .values()
            .filter(|run| run.run.plan_id.as_ref().map(|id| id.0.as_str()) == Some(issue_id))
            .collect::<Vec<_>>();
        implementations.sort_by_key(|run| {
            self.entity_created_at
                .get(&run.run.id.0)
                .cloned()
                .unwrap_or_default()
        });
        implementations
            .iter()
            .rev()
            .find(|run| !run.run.state.is_terminal())
            .copied()
            .or_else(|| implementations.last().copied())
    }

    fn plan_view(&self, plan_id: &str, active: &ActivePlan, thread_detail: ThreadDetail) -> Value {
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
        let implementation_complete = self.plan_implementation_complete(plan_id, active);
        let current_implementation = self.current_issue_implementation(plan_id);
        let mut implementation_lineage = self
            .runs
            .values()
            .filter(|run| run.run.plan_id.as_ref().map(|id| id.0.as_str()) == Some(plan_id))
            .map(|run| {
                json!({
                    "implementation_id": run.run.id.0,
                    "run_id": run.run.id.0,
                    "state": run_state_str(&run.run.state),
                    "branch": run.worktree.branch,
                    "worktree_path": run.worktree.path.display().to_string(),
                    "created_at": self.entity_created_at.get(&run.run.id.0),
                })
            })
            .collect::<Vec<_>>();
        implementation_lineage.sort_by_key(|implementation| {
            implementation["created_at"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        });
        json!({
            "issue_id": plan_id,
            "plan_id": plan_id,
            "goal": active.plan.goal,
            "state": plan_state_str(&active.plan.state),
            "needs_attention": active.plan.state.needs_attention(),
            "attention": self.attention_json(plan_id),
            "summary": active.last_summary,
            "last_error": active.last_error,
            "project": project,
            "project_id": project_id,
            "base_branch": active.base_branch,
            "plan_path": active.plan_path,
            "harness": if self.qa_agent { self.harness.as_str() } else { active.model_choice.provider.label() },
            "provider": active.model_choice.provider,
            "model": active.model_choice.model,
            "effort": active.model_choice.effort,
            "thread": match thread_detail {
                ThreadDetail::Digest => active.thread.digest_value(),
                ThreadDetail::Full => active.thread.wire_value(),
            },
            "active_run_id": active_run_id,
            "current_implementation_id": current_implementation.map(|run| run.run.id.0.clone()),
            "current_implementation": current_implementation.map(|run| json!({
                "implementation_id": run.run.id.0,
                "run_id": run.run.id.0,
                "state": run_state_str(&run.run.state),
                "branch": run.worktree.branch,
                "worktree_path": run.worktree.path.display().to_string(),
            })),
            "implementation_lineage": implementation_lineage,
            "implementation_intent": active.plan.implementation_intent,
            "implementation_activity": active.plan.implementation_activity,
            "implementation_complete": implementation_complete,
            "can_archive": implementation_complete && active.plan.archived_at.is_none(),
            "archived_at": active.plan.archived_at,
            // False when the store holds no docs (a migrated plan whose docs
            // were unrecoverable): the client disables doc reads + Implement
            // instead of retrying reads that can never succeed.
            "docs_available": self
                .store
                .as_ref()
                .is_some_and(|store| store.has_plan_docs(plan_id)),
            "created_at": self.entity_created_at.get(plan_id),
            "updated_at": self.entity_updated_at.get(plan_id),
            "state_changed_at": self.entity_state_changed_at.get(plan_id),
            "stages": active
                .stages
                .iter()
                .map(|doc| plan_stage_json(active, doc))
                .collect::<Vec<_>>(),
        })
    }

    /// Server-derived implementation completion. Multi-stage completion must
    /// be proven by one linked run that passed every manifest stage and reached
    /// Review/Merged; single-doc plans only need that run-level human gate.
    fn plan_implementation_complete(&self, plan_id: &str, plan: &ActivePlan) -> bool {
        self.runs.values().any(|run| {
            if run.run.plan_id.as_ref().map(|id| id.0.as_str()) != Some(plan_id) {
                return false;
            }
            let passed_human_gate = matches!(run.run.state, RunState::Review | RunState::Merged);
            if plan.stages.is_empty() {
                return passed_human_gate;
            }
            matches!(
                run.run.state,
                RunState::Review | RunState::Merged | RunState::Archived
            ) && plan.stages.iter().all(|stage| {
                run.stages.iter().any(|progress| {
                    progress.stage_id == stage.id
                        && progress.state == StageProgressState::Validated { passed: true }
                        && (progress.completion_sha.is_some()
                            || progress.publication == StagePublication::LegacyUnknown)
                        && progress.invalidation_reason.is_none()
                })
            })
        })
    }

    /// The wire view of a run (spec §board.list): identity + plan link, state,
    /// branch/base, per-stage execution progress (with validation), model, and
    /// timestamps. The live diffstat rides along in `board.list`.
    /// `thread_detail` picks a bounded digest (list surfaces) or the full
    /// conversation (detail surfaces + mutation responses).
    fn run_view(&self, run_id: &str, active: &ActiveRun, thread_detail: ThreadDetail) -> Value {
        let project_id = self.entity_project.get(run_id).cloned().unwrap_or_default();
        let project = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        json!({
            "run_id": run_id,
            "implementation_id": run_id,
            "issue_id": active.run.plan_id.as_ref().map(|p| p.0.clone()),
            "plan_id": active.run.plan_id.as_ref().map(|p| p.0.clone()),
            "goal": active.run.goal,
            "state": run_state_str(&active.run.state),
            "needs_attention": active.run.state.needs_attention(),
            "attention": self.attention_json(run_id),
            "branch": active.worktree.branch,
            "base_branch": active.worktree.base_branch,
            "base_sha": active.base_sha,
            "worktree_path": active.worktree.path.display().to_string(),
            "summary": active.last_summary,
            "last_error": active.last_error,
            "project": project,
            "project_id": project_id,
            "harness": if self.qa_agent { self.harness.as_str() } else { active.model_choice.provider.label() },
            "provider": active.model_choice.provider,
            "model": active.model_choice.model,
            "effort": active.model_choice.effort,
            "thread": match thread_detail {
                ThreadDetail::Digest => self.conversation_thread_for_run(active).digest_value(),
                ThreadDetail::Full => self.conversation_thread_for_run(active).wire_value(),
            },
            "auto_advance": active.auto_advance,
            "current_stage_id": active.current_stage_id,
            "adopted": active.adopted,
            "can_finish": active.run.state == RunState::Merged
                || (active.run.state == RunState::Review && active.worktree.path.exists()),
            "created_at": self.entity_created_at.get(run_id),
            "updated_at": self.entity_updated_at.get(run_id),
            "state_changed_at": self.entity_state_changed_at.get(run_id),
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
        let git_state = git2::Repository::open(&active.worktree.path)
            .ok()
            .and_then(|repo| {
                let head_ref = repo.head().ok()?;
                let checked_out_branch = head_ref.shorthand().map(str::to_string);
                let head = head_ref.peel_to_commit().ok()?;
                let comparison = crate::worktree::branch_comparison(
                    &repo,
                    &head,
                    checked_out_branch.as_deref(),
                    &active.worktree.base_branch,
                );
                Some((checked_out_branch, comparison))
            });
        let checked_out_branch = git_state.as_ref().and_then(|(branch, _)| branch.as_deref());
        let comparison = git_state.as_ref().map(|(_, comparison)| comparison);
        let uncommitted = crate::diff::diff_uncommitted(&active.worktree.path)
            .map(|diff| {
                let stat = diff.stat();
                json!({
                    "files_changed": stat.files_changed,
                    "insertions": stat.insertions,
                    "deletions": stat.deletions,
                })
            })
            .unwrap_or(Value::Null);
        let stat =
            crate::diff::diff_against_base(&active.worktree.path, &active.worktree.base_branch)
                .map(|diff| {
                    let s = diff.stat();
                    json!({
                        "files_changed": s.files_changed,
                        "insertions": s.insertions,
                        "deletions": s.deletions,
                        "branch": checked_out_branch,
                        "comparison_ref": comparison.and_then(|value| value.reference.as_deref()),
                        "upstream": comparison.and_then(|value| value.upstream.as_deref()),
                        "ahead": comparison.and_then(|value| value.ahead),
                        "behind": comparison.and_then(|value| value.behind),
                        "uncommitted": uncommitted,
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
        let previous_stage_ids: Vec<String> =
            active.stages.iter().map(|stage| stage.id.clone()).collect();
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
            .map_err(err)?;
        let new_stages: Vec<(usize, StageDoc)> = active
            .stages
            .iter()
            .enumerate()
            .filter(|(_, stage)| !previous_stage_ids.contains(&stage.id))
            .map(|(index, stage)| (index, stage.clone()))
            .collect();
        let plan_id = active.plan.id.0.clone();
        append_plan_stage_announcements(&mut active.thread, &plan_id, &new_stages);
        Ok(())
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

    /// Simulate a mid-run stage-revision agent: rewrite the stage doc in the
    /// RUN's worktree, resolve the plan's open comments, and route the
    /// `done(revise)` through the cross-entity consume seam.
    fn qa_simulate_run_stage_revise(
        &self,
        project_id: &str,
        active: &mut ActiveRun,
        plan: &mut ActivePlan,
    ) -> Result<(), String> {
        let stage_id = active
            .revising_stage_id
            .clone()
            .ok_or("QA run revise: no stage revision in flight")?;
        let index = plan.stage_doc_index(&stage_id)?;
        let stage_path = plan.stages[index].path.clone();
        let worktree = active.worktree.path.clone();
        let mut contents = std::fs::read_to_string(worktree.join(&stage_path))
            .map_err(|e| format!("QA run revise: could not read stage doc: {e}"))?;
        contents.push_str("\n(revised mid-run)\n");
        write_in_dir(&worktree, &stage_path, &contents)?;
        let resolutions: Vec<CommentResolution> = plan
            .open_comments_for(&stage_id)
            .into_iter()
            .map(|c| CommentResolution {
                comment_id: c.id.clone(),
                response: "QA: addressed.".to_string(),
            })
            .collect();
        let store = self.require_store()?;
        self.orch_for(project_id)?
            .consume_run_stage_revision(
                active,
                plan,
                store,
                &DoneReport {
                    phase: DonePhase::Revise,
                    status: DoneStatus::Completed,
                    summary: format!("Revised stage {stage_id} mid-run"),
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
            .map_err(err)?;
        Ok(())
    }

    /// Simulate one stage's build session and — since that hands off to a
    /// validation session — the validation too, so one call lands the stage on
    /// a verdict exactly as two real `done` reports would.
    /// The build→validate hand-off turn each `on_run_done` returns is dropped
    /// on purpose here: the scripted agent plays BOTH sides, so it validates
    /// the stage itself in the next arm rather than asking a harness to. That
    /// means NO test driven by this simulator exercises the hand-off delivery —
    /// `a_built_stage_queues_its_validation_turn_for_the_worktrees_agent` and
    /// `a_done_over_the_socket_delivers_the_validation_turn_to_the_same_agent`
    /// switch the simulator off precisely so the real path is covered.
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
    /// build+validate; a post-review change or a single-doc-plan build runs
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

fn diff_json(diff: &crate::diff::WorktreeDiff) -> Value {
    let files: Vec<Value> = diff
        .files()
        .iter()
        .map(|file| json!({ "path": file.path, "status": format!("{:?}", file.status) }))
        .collect();
    let stat = diff.stat();
    json!({
        "stat": {
            "files_changed": stat.files_changed,
            "insertions": stat.insertions,
            "deletions": stat.deletions,
        },
        "files": files,
        "patch": diff.patch(),
    })
}

/// The wire view of a run stage's execution progress: id, sub-state, immutable
/// commit boundaries, publication evidence, and its validation report if any.
fn run_stage_json(progress: &StageProgress) -> Value {
    json!({
        "id": progress.stage_id,
        "state": run_stage_progress_str(&progress.state),
        "start_sha": progress.start_sha,
        "built_sha": progress.built_sha,
        "completion_sha": progress.completion_sha,
        "publication": progress.publication,
        "invalidation_reason": progress.invalidation_reason,
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

fn canonical_stage_execution(progress: &StageProgress) -> &'static str {
    match progress.state {
        _ if progress.invalidation_reason.is_some() => "incomplete",
        StageProgressState::Building => "building",
        StageProgressState::Built => "built",
        StageProgressState::Validating => "validating",
        StageProgressState::Validated { passed: true } if progress.completion_sha.is_some() => {
            "complete"
        }
        StageProgressState::Validated { passed: true } => "legacy_unpinned",
        StageProgressState::Validated { passed: false } => "validation_failed",
    }
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

/// Parse and validate the optional provider/model/effort params of a request.
fn model_choice_from(params: &Value) -> Result<ModelChoice, String> {
    let provider = match params.get("provider").and_then(Value::as_str) {
        None | Some("") | Some("claude") => AgentProvider::Claude,
        Some("codex") => AgentProvider::Codex,
        Some(other) => return Err(format!("unknown agent provider: {other}")),
    };
    let choice = ModelChoice {
        provider,
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

fn has_agent_choice(params: &Value) -> bool {
    ["provider", "model", "effort"]
        .iter()
        .any(|key| params.get(key).is_some())
}

/// Copy a canonical opaque id into the legacy parameter name consumed by the
/// compatibility implementation. If an old client already sent the legacy
/// name it remains untouched.
fn alias_param(params: &Value, canonical: &str, legacy: &str) -> Value {
    let mut aliased = params.clone();
    if aliased.get(legacy).is_none() {
        if let Some(value) = aliased.get(canonical).cloned() {
            if let Some(object) = aliased.as_object_mut() {
                object.insert(legacy.to_string(), value);
            }
        }
    }
    aliased
}

fn require_str(params: &Value, key: &str) -> Result<String, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("missing required param: {key}"))
}

/// The detail polls' optional `thread_after_sequence` cursor. A missing or
/// garbage (non-integer, negative) value reads as absent — the poll then gets
/// the full backward-compatible thread instead of an error.
fn thread_cursor(params: &Value) -> Option<u64> {
    params.get("thread_after_sequence").and_then(Value::as_u64)
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
    worktree: Option<GitScopeWorktree>,
}

impl GitScope {
    /// The branch history is measured against, so `git.log` can mark which
    /// commits this checkout carries on top of it. The primary checkout has
    /// none — its history IS the base.
    fn mark_ahead_of(&self) -> Option<&str> {
        self.run
            .as_ref()
            .map(|run| run.base_branch.as_str())
            .or_else(|| self.worktree.as_ref().map(|wt| wt.base_branch.as_str()))
    }
}

struct GitScopeRun {
    run_id: String,
    base_branch: String,
}

/// An external worktree's git scope: the project that owns it (so a mutation
/// invalidates the scan the rail reads) and the branch its history is measured
/// against.
struct GitScopeWorktree {
    project_id: String,
    base_branch: String,
}

/// The checkout a branch operation acts on, and which cached summary describes
/// it: a project's primary checkout, or one of that project's worktrees.
struct BranchScope {
    project_id: String,
    repo_path: std::path::PathBuf,
    external_worktree: bool,
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
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("m4a") => "audio/mp4",
        Some("aac") => "audio/aac",
        Some("flac") => "audio/flac",
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mov") => "video/quicktime",
        Some("json") => "application/json",
        Some("pdf") => "application/pdf",
        _ if head.contains(&0u8) => "application/octet-stream",
        _ => "text/plain",
    }
}

fn media_mime_hint(path: &std::path::Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    match ext.as_str() {
        "mp3" => Some("audio/mpeg"),
        "wav" => Some("audio/wav"),
        "m4a" => Some("audio/mp4"),
        "aac" => Some("audio/aac"),
        "flac" => Some("audio/flac"),
        "mp4" | "m4v" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        "mov" => Some("video/quicktime"),
        _ => None,
    }
}

fn err(e: OrchestratorError) -> String {
    e.to_string()
}

fn parse_worktree_finish_action(action: &str) -> Result<WorktreeFinishAction, String> {
    match action {
        "cleanup" => Ok(WorktreeFinishAction::Cleanup),
        "push" => Ok(WorktreeFinishAction::Push),
        "merge" => Ok(WorktreeFinishAction::Merge),
        "delete" => Ok(WorktreeFinishAction::Delete),
        other => Err(format!(
            "unknown worktree finish action {other:?} — expected cleanup, push, merge, or delete"
        )),
    }
}

fn ensure_worktree_finish_eligible(
    worktree: &ExternalWorktree,
    action: WorktreeFinishAction,
    base_branch: &str,
) -> Result<(), String> {
    let repo = git2::Repository::open(&worktree.path).map_err(|error| error.to_string())?;
    let conflicted = repo
        .statuses(Some(
            git2::StatusOptions::new()
                .include_untracked(true)
                .recurse_untracked_dirs(true),
        ))
        .map_err(|error| error.to_string())?
        .iter()
        .any(|entry| entry.status().is_conflicted());
    match action {
        WorktreeFinishAction::Cleanup => {
            if worktree.dirty_files > 0
                || conflicted
                || repo.state() != git2::RepositoryState::Clean
            {
                return Err(
                    "worktree.finish cleanup requires no uncommitted, staged, untracked, or conflicted changes"
                        .to_string(),
                );
            }
        }
        WorktreeFinishAction::Push => {
            if worktree.upstream.is_none() {
                return Err("worktree.finish push requires an upstream/tracking branch".to_string());
            }
            if conflicted || repo.state() != git2::RepositoryState::Clean {
                return Err(
                    "worktree.finish push cannot checkpoint conflicted git state".to_string(),
                );
            }
        }
        WorktreeFinishAction::Merge => {
            let branch = worktree
                .branch
                .as_deref()
                .ok_or("worktree.finish merge requires an attached branch")?;
            if branch == base_branch {
                return Err(format!(
                    "worktree.finish merge requires a branch different from base {base_branch:?}"
                ));
            }
            if conflicted || repo.state() != git2::RepositoryState::Clean {
                return Err(
                    "worktree.finish merge cannot checkpoint conflicted git state".to_string(),
                );
            }
        }
        WorktreeFinishAction::Delete => {}
    }
    Ok(())
}

fn checkpoint_worktree(
    worktree_path: &std::path::Path,
    action: WorktreeFinishAction,
) -> Result<(), String> {
    git_stdout(worktree_path, &["add", "-A", "--", "."])?;
    let staged = git_stdout(worktree_path, &["diff", "--cached", "--name-only"])?;
    if staged.trim().is_empty() {
        return Ok(());
    }
    let message = match action {
        WorktreeFinishAction::Push => "Build checkpoint before push",
        WorktreeFinishAction::Merge => "Build checkpoint before merge",
        WorktreeFinishAction::Cleanup | WorktreeFinishAction::Delete => {
            unreachable!("only push/merge checkpoint")
        }
    };
    git_stdout(worktree_path, &["commit", "-m", message]).map(|_| ())
}

fn validate_finish_record_path(
    record: &PersistedArchivedWorktree,
    project_path: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    if record.project_path != project_path.display().to_string() {
        return Err("worktree.finish record belongs to another project".to_string());
    }
    let worktree_path = std::path::PathBuf::from(&record.worktree_path);
    let canonical_project = std::fs::canonicalize(project_path)
        .map_err(|error| format!("worktree.finish project path: {error}"))?;
    let resolved_worktree = if worktree_path.exists() {
        std::fs::canonicalize(&worktree_path)
            .map_err(|error| format!("worktree.finish worktree path: {error}"))?
    } else {
        worktree_path.clone()
    };
    if resolved_worktree == canonical_project {
        return Err("worktree.finish never acts on the primary checkout".to_string());
    }
    if crate::worktree::external_worktree_id(&resolved_worktree) != record.worktree_id {
        return Err(
            "worktree.finish record id no longer matches its server-resolved path".to_string(),
        );
    }
    Ok(resolved_worktree)
}

fn local_branch_exists(project_path: &std::path::Path, branch: &str) -> Result<bool, String> {
    let repo = git2::Repository::open(project_path).map_err(|error| error.to_string())?;
    let result = match repo.find_branch(branch, git2::BranchType::Local) {
        Ok(_) => Ok(true),
        Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    };
    result
}

fn delete_local_branch_for_finish(
    project_path: &std::path::Path,
    branch: &str,
    expected_head: &str,
) -> Result<(), String> {
    let reference = format!("refs/heads/{branch}");
    git_stdout(
        project_path,
        &["update-ref", "-d", &reference, expected_head],
    )
    .map(|_| ())
}

fn restore_finish_branch_after_removal_failure(
    project_path: &std::path::Path,
    record: &PersistedArchivedWorktree,
    deleted_branch: bool,
    remove_error: &str,
) -> Result<(), String> {
    let Some(branch) = record.branch.as_deref().filter(|_| deleted_branch) else {
        return Ok(());
    };
    let reference = format!("refs/heads/{branch}");
    git_stdout(project_path, &["update-ref", &reference, &record.head_sha])
        .map(|_| ())
        .map_err(|restore_error| {
            format!(
                "{remove_error}; restoring branch {branch:?} after removal failure also failed: {restore_error}"
            )
        })
}

fn finish_git_steps_are_complete(record: &PersistedArchivedWorktree) -> bool {
    if std::path::Path::new(&record.worktree_path).exists() {
        return false;
    }
    match record.action {
        WorktreeFinishAction::Cleanup | WorktreeFinishAction::Push => true,
        WorktreeFinishAction::Merge | WorktreeFinishAction::Delete => {
            record.branch.as_deref().is_none_or(|branch| {
                local_branch_exists(std::path::Path::new(&record.project_path), branch)
                    .is_ok_and(|exists| !exists)
            })
        }
    }
}

fn merge_external_branch(
    project_path: &std::path::Path,
    branch: &str,
    base_branch: &str,
) -> Result<(), String> {
    let checked_out = git_stdout(project_path, &["symbolic-ref", "--short", "HEAD"])?;
    if checked_out.trim() != base_branch {
        return Err(format!(
            "primary checkout is on {:?}, not configured base {base_branch:?}",
            checked_out.trim()
        ));
    }
    if let Err(merge_error) = git_stdout(project_path, &["merge", "--no-edit", "--", branch]) {
        if let Err(abort_error) = git_stdout(project_path, &["merge", "--abort"]) {
            eprintln!("worktree.finish merge {branch}: abort failed: {abort_error}");
        }
        return Err(merge_error);
    }
    Ok(())
}

fn remove_registered_worktree(
    project_path: &std::path::Path,
    worktree_path: &std::path::Path,
    force: bool,
) -> Result<(), String> {
    let path = worktree_path
        .to_str()
        .ok_or("worktree path is not valid UTF-8")?;
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.extend(["--", path]);
    git_stdout(project_path, &args).map(|_| ())
}

fn classify_stage_publication(
    repo_path: &std::path::Path,
    branch: &str,
    base_branch: &str,
    completion_sha: &str,
) -> StagePublication {
    let Ok(repo) = git2::Repository::open(repo_path) else {
        return StagePublication::Local;
    };
    let Ok(completion) = git2::Oid::from_str(completion_sha) else {
        return StagePublication::Local;
    };
    let reachable = |reference: &str| {
        repo.find_reference(reference)
            .ok()
            .and_then(|reference| reference.peel_to_commit().ok())
            .is_some_and(|tip| {
                tip.id() == completion
                    || repo
                        .graph_descendant_of(tip.id(), completion)
                        .unwrap_or(false)
            })
    };
    let merge_ref = format!("refs/heads/{base_branch}");
    if reachable(&merge_ref) {
        return StagePublication::Merged;
    }
    let upstream_ref = repo
        .find_branch(branch, git2::BranchType::Local)
        .ok()
        .and_then(|local| local.upstream().ok())
        .and_then(|upstream| upstream.get().name().map(str::to_string));
    if upstream_ref.as_deref().is_some_and(reachable) {
        StagePublication::Pushed
    } else {
        StagePublication::Local
    }
}

fn git_stdout(dir: &std::path::Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| format!("could not run git: {error}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = [stderr.trim(), stdout.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!("git {args:?}: {detail}"))
}

fn archived_worktree_json(record: &PersistedArchivedWorktree) -> Value {
    json!({
        "worktree_id": record.worktree_id,
        "name": record.worktree_name,
        "path": record.worktree_path,
        "branch": record.branch,
        "head_sha": record.head_sha,
        "upstream": record.upstream,
        "unpushed": record.unpushed,
        "dirty_files": record.dirty_files,
        "uncommitted": {
            "files_changed": record.uncommitted_files,
            "insertions": record.uncommitted_insertions,
            "deletions": record.uncommitted_deletions,
        },
        "action": record.action,
        "archived_at": record.archived_at,
    })
}

fn apply_thread_action(
    thread: &mut crate::thread::Thread,
    artifact: crate::thread::ArtifactKind,
    action: BridgeAction,
    now: &str,
) -> Result<Value, String> {
    match action {
        BridgeAction::ReadUnreadMessages => Ok(json!({
            "thread_id": thread.id,
            "agent_id": thread.agent.id,
            "messages": thread.read_unread(now),
        })),
        BridgeAction::PostThreadMessage {
            body,
            anchor,
            links,
        } => {
            let body = body.trim();
            if body.is_empty() {
                return Err("message body must not be empty".to_string());
            }
            if body.len() > 32_000 {
                return Err("message body exceeds 32000 bytes".to_string());
            }
            if anchor.as_ref().is_some_and(|anchor| {
                anchor.artifact != artifact
                    && !thread.items.iter().any(|item| {
                        matches!(
                            item,
                            crate::thread::ThreadItem::Message(message)
                                if message.anchor.as_ref().is_some_and(|existing| existing.artifact == anchor.artifact)
                        )
                    })
            }) {
                return Err(format!(
                    "anchor artifact must be {} for this conversation",
                    artifact.as_str()
                ));
            }
            validate_thread_links(&links)?;
            let message_id = thread.post_agent_with_links(body, anchor, links, now);
            Ok(json!({ "message_id": message_id }))
        }
    }
}

fn validate_thread_links(links: &[crate::thread::ThreadLink]) -> Result<(), String> {
    if links.len() > 20 {
        return Err("message links must contain at most 20 entries".to_string());
    }
    for link in links {
        match link {
            crate::thread::ThreadLink::File {
                path,
                line_start,
                line_end,
            } => {
                if path.is_empty() || !crate::plan::is_worktree_contained_path(path) {
                    return Err("file link path escapes the worktree".to_string());
                }
                if line_start.is_some_and(|line| line == 0)
                    || line_end.is_some_and(|line| line == 0)
                    || matches!((line_start, line_end), (Some(start), Some(end)) if start > end)
                {
                    return Err("file link line range is invalid".to_string());
                }
            }
            crate::thread::ThreadLink::PlanStage {
                plan_id,
                stage_id,
                path,
            } => {
                if plan_id.is_empty()
                    || stage_id.is_empty()
                    || !path.starts_with(".build/plan/")
                    || !crate::plan::is_worktree_contained_path(path)
                {
                    return Err("plan stage link is invalid".to_string());
                }
            }
            crate::thread::ThreadLink::Run { run_id } if run_id.is_empty() => {
                return Err("run link is invalid".to_string());
            }
            crate::thread::ThreadLink::Run { .. } => {}
        }
    }
    Ok(())
}

const NEW_THREAD_MESSAGES_PROMPT: &str =
    "New reviewer messages are available. Call `read_unread_messages` now, then act on every unread message. Reply with `post_thread_message` only when the conversation policy requires a written response.";

fn parse_thread_inputs(
    params: &Value,
    artifact: crate::thread::ArtifactKind,
    legacy_field: &str,
) -> Result<Vec<(String, Option<crate::thread::MessageAnchor>)>, String> {
    if let Some(messages) = params.get("messages") {
        let messages = messages
            .as_array()
            .ok_or_else(|| "messages must be an array".to_string())?;
        if messages.is_empty() {
            return Err("messages must not be empty".to_string());
        }
        if messages.len() > 100 {
            return Err("messages must contain at most 100 entries".to_string());
        }
        return messages
            .iter()
            .map(|message| {
                let body = message
                    .get("body")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|body| !body.is_empty())
                    .ok_or_else(|| "every message requires a non-empty body".to_string())?;
                if body.len() > 32_000 {
                    return Err("message body exceeds 32000 bytes".to_string());
                }
                let anchor = match message.get("anchor") {
                    None | Some(Value::Null) => None,
                    Some(value) => {
                        let anchor: crate::thread::MessageAnchor =
                            serde_json::from_value(value.clone())
                                .map_err(|error| format!("invalid message anchor: {error}"))?;
                        if anchor.artifact != artifact {
                            return Err(format!(
                                "message anchor artifact must be {}",
                                artifact.as_str()
                            ));
                        }
                        Some(anchor)
                    }
                };
                Ok((body.to_string(), anchor))
            })
            .collect();
    }
    let body = params
        .get(legacy_field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|body| !body.is_empty())
        .ok_or_else(|| format!("missing required param: {legacy_field} or messages"))?;
    if body.len() > 32_000 {
        return Err(format!("{legacy_field} exceeds 32000 bytes"));
    }
    Ok(vec![(body.to_string(), None)])
}

/// Parse `thread.post`'s single `body` (+ optional `anchor`) by funneling it
/// through [`parse_thread_inputs`]'s batch validator, so body limits and
/// anchor artifact-matching stay single-sourced.
fn parse_thread_post_input(
    params: &Value,
    artifact: crate::thread::ArtifactKind,
) -> Result<Vec<(String, Option<crate::thread::MessageAnchor>)>, String> {
    let wrapped = json!({
        "messages": [{
            "body": params.get("body").cloned().unwrap_or(Value::Null),
            "anchor": params.get("anchor").cloned().unwrap_or(Value::Null),
        }]
    });
    parse_thread_inputs(&wrapped, artifact, "body")
}

/// Tell the worktree's agent, in place, that unread thread messages await.
///
/// [`deliver`]'s warm branch without the cold half, and deliberately so:
/// `thread.post` never starts a process. It notifies whatever agent is ALIVE in
/// that worktree, whatever its entity is parked as.
///
/// This was once gated on `building`/`drafting`, from when the agent existed
/// only while working: any other state meant no process to talk to. A worktree's
/// agent now outlives every phase and is sitting in the Agent tab the human is
/// typing into, so gating on entity state meant a message could be typed into a
/// live conversation and silently not arrive. A `done` the run machine does not
/// accept from that state is recorded as out-of-phase and moves nothing, which
/// is a far smaller cost than a conversation that lies about itself.
///
/// No tab, or a tab whose process has ended, swallows the nudge, and a write
/// failure against an exiting harness is logged, never surfaced: the message is
/// durable either way.
fn nudge_live_agent_tab(tabs: &HashMap<TabKey, Tab>, root: &std::path::Path, entity_id: &str) {
    let Some(tab) = tabs.get(&TabKey::agent(&AppState::canonical_root(root))) else {
        return;
    };
    if !tab.live || tab.session.has_exited() {
        return;
    }
    // Through write_prompt, not a raw write with a hardcoded Enter: the nudge is
    // a turn, so it must honor the harness's SubmitKey and paste framing exactly
    // as a dispatched prompt does. Hardcoding \r submits into a SubmitKey::None
    // harness that never asked for it, and leaves the notification unframed —
    // safe today only because it happens to be one line.
    if let Err(error) = tab.session.write_prompt(NEW_THREAD_MESSAGES_PROMPT) {
        eprintln!("thread.post {entity_id}: agent notify failed: {error}");
    }
}

/// Open a conversation's session lineage for a newly spawned agent process,
/// chaining it off the previous session so the thread still reads as a chain.
fn open_session_lineage(thread: &mut crate::thread::Thread, turn: &PendingAgentTurn) {
    let session_id = thread.start_session(
        turn.model_choice.provider.label(),
        turn.model_choice.model.as_deref(),
        turn.model_choice.effort.as_deref(),
        turn.phase,
        &now_rfc3339(),
    );
    thread.push_event(
        crate::thread::ThreadEventKind::RunStarted,
        Some(format!("{} run started", turn.phase)),
        Some(session_id),
        None,
        now_rfc3339(),
    );
}

fn append_user_thread_messages(
    thread: &mut crate::thread::Thread,
    messages: Vec<(String, Option<crate::thread::MessageAnchor>)>,
) {
    let now = now_rfc3339();
    for (body, anchor) in messages {
        thread.post_user(body, anchor, &now);
    }
}

fn append_stage_comments_to_thread(
    thread: &mut crate::thread::Thread,
    stage: &StageDoc,
    comments: &[StageComment],
) {
    let now = now_rfc3339();
    for comment in comments {
        let body = comment.body.clone();
        let already_posted = thread.items.iter().any(|item| {
            matches!(
                item,
                crate::thread::ThreadItem::Message(message)
                    if message.role == crate::thread::MessageRole::User
                        && message.body == body
                        && message.anchor.as_ref().and_then(|anchor| anchor.path.as_deref())
                            == comment.anchor.as_ref().map(|_| stage.path.as_str())
            )
        });
        if already_posted {
            continue;
        }
        let anchor = comment
            .anchor
            .as_ref()
            .map(|anchor| crate::thread::MessageAnchor {
                artifact: crate::thread::ArtifactKind::Plan,
                revision_id: None,
                path: Some(stage.path.clone()),
                side: None,
                line_start: None,
                line_end: None,
                heading_path: anchor.heading_path.clone(),
                snippet: anchor.snippet.clone(),
            });
        thread.post_user(body, anchor, &now);
    }
}

fn append_plan_stage_announcements(
    thread: &mut crate::thread::Thread,
    plan_id: &str,
    stages: &[(usize, StageDoc)],
) {
    let now = now_rfc3339();
    for (index, stage) in stages {
        let explanation = if stage.summary.trim().is_empty() {
            format!("**Stage {}: {}**", index + 1, stage.title)
        } else {
            format!(
                "**Stage {}: {}**\n\n{}",
                index + 1,
                stage.title,
                stage.summary.trim()
            )
        };
        thread.post_agent_with_links(
            explanation,
            None,
            vec![crate::thread::ThreadLink::PlanStage {
                plan_id: plan_id.to_string(),
                stage_id: stage.id.clone(),
                path: stage.path.clone(),
            }],
            &now,
        );
    }
}

fn record_current_stage_started(
    thread: &mut crate::thread::Thread,
    active: &ActiveRun,
    stages: &[StageDoc],
) {
    let Some(plan_id) = active.run.plan_id.as_ref().map(|id| id.0.clone()) else {
        return;
    };
    let Some(stage_id) = active.current_stage_id.as_deref() else {
        return;
    };
    let Some(stage) = stages.iter().find(|stage| stage.id == stage_id) else {
        return;
    };
    thread.push_event_with_links(
        crate::thread::ThreadEventKind::StageStarted,
        Some(format!("Started plan stage “{}”", stage.title)),
        None,
        None,
        vec![crate::thread::ThreadLink::PlanStage {
            plan_id,
            stage_id: stage.id.clone(),
            path: stage.path.clone(),
        }],
        now_rfc3339(),
    );
}

/// Close the conversation's open session, if one is open. Nothing to close is
/// the normal case for a thread whose agent never started, so it is silence,
/// not an error.
///
/// The mirror of [`open_session_lineage`], and the ONLY way a session ends: a
/// session is the life of an agent PROCESS, so it closes when that process
/// does (the tab pump's EOF) or when Build kills it — never when the agent
/// merely finishes a turn.
fn finish_open_session(thread: &mut crate::thread::Thread, now: &str) {
    let Some(session_id) = thread
        .sessions
        .iter()
        .rev()
        .find(|session| session.ended_at.is_none())
        .map(|session| session.id.clone())
    else {
        return;
    };
    thread.finish_session(&session_id, now);
}

fn record_report_in_thread(
    thread: &mut crate::thread::Thread,
    report: &DoneReport,
    orchestration_error: Option<&str>,
) {
    let now = now_rfc3339();
    let (event, summary) = match orchestration_error {
        Some(error) => (
            crate::thread::ThreadEventKind::RunFailed,
            format!(
                "{}\n\nBuild could not apply the report: {error}",
                report.summary
            ),
        ),
        None if report.status == DoneStatus::Blocked => (
            crate::thread::ThreadEventKind::Blocked,
            report.summary.clone(),
        ),
        None if report.status == DoneStatus::Failed => (
            crate::thread::ThreadEventKind::RunFailed,
            report.summary.clone(),
        ),
        None if report
            .outputs
            .validation
            .as_ref()
            .is_some_and(|validation| !validation.passed) =>
        {
            (
                crate::thread::ThreadEventKind::ReviewBlocked,
                report
                    .outputs
                    .validation
                    .as_ref()
                    .map(|validation| validation.findings.clone())
                    .unwrap_or_else(|| report.summary.clone()),
            )
        }
        _ => (crate::thread::ThreadEventKind::Done, report.summary.clone()),
    };
    thread.push_event(event, Some(summary), None, None, &now);
    if event == crate::thread::ThreadEventKind::Done {
        thread.post_completion(&report.summary, &now);
    }
    if let Some(completion) = &report.outputs.completion_report {
        thread.remember_completion(completion);
    }
}

/// An entity went quiet (or its agent exited) without reporting: record the
/// reason. The session lineage is deliberately left alone — a quiet agent is
/// still an agent, and one that exited has already had its session closed by
/// the pump that saw the EOF.
fn record_idle_in_thread(thread: &mut crate::thread::Thread, exit_code: Option<i32>) {
    let now = now_rfc3339();
    let (event, summary) = match exit_code {
        Some(code) => (
            crate::thread::ThreadEventKind::RunFailed,
            format!("Agent exited unexpectedly with code {code}"),
        ),
        None => (
            crate::thread::ThreadEventKind::IdleUnreported,
            "Agent went quiet without reporting done".to_string(),
        ),
    };
    thread.push_event(event, Some(summary), None, None, now);
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
        // Opening a terminal or an agent in a worktree IS interacting with it in
        // Build — it is the reason a hand-made worktree graduates out of the
        // Worktrees row. This arm bypasses `dispatch`, so it stamps for itself.
        "term.create" => {
            let created = term_create(state, &params);
            if created.is_ok() {
                if let Some(scope_id) = params
                    .get("run_id")
                    .or_else(|| params.get("worktree_id"))
                    .and_then(Value::as_str)
                {
                    state.lock().unwrap().touch_attention(scope_id);
                }
            }
            created
        }
        "term.attach" => term_attach(state, &sender, &params),
        "agent.attach" => agent_attach(state, &sender, &params),
        // Bypasses `dispatch` for the same reason `deliver` does: opening a
        // harness blocks for seconds on its readiness wait, and every terminal
        // pump needs the state lock free while it does.
        "agent.start" => agent_start(state, &params),
        _ => {
            let dispatched = {
                let mut app = state.lock().unwrap();
                let queued_before = app.pending_agent_turns.len();
                let result = app.dispatch(&method, &params);
                if result.is_err() {
                    // A turn is not deliverable until the mutation that queued
                    // it is durable. Drop only this request's turns on failure;
                    // otherwise a later harmless RPC would deliver work the
                    // failed request never committed.
                    app.pending_agent_turns.truncate(queued_before);
                }
                result
            };
            // A verb speaks to a worktree's agent by queuing a turn: it runs
            // under the state lock and `deliver` needs that lock free (a cold
            // spawn blocks for seconds on the harness's readiness wait).
            if dispatched.is_ok() {
                deliver_pending_agent_turns(state);
            }
            dispatched
        }
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

/// Create one of the human's shells: parse + resolve the scope server-side
/// (never a client path), enforce the cap, spawn their login shell in the
/// worktree root, and start its pump immediately — the screen model
/// accumulates even before the first attach.
///
/// Only a shell. A worktree's agent is not created here; it is
/// [`ensure_agent_tab`]'s, and it is the only agent the worktree gets.
fn term_create(state: &Arc<Mutex<AppState>>, params: &Value) -> Result<Value, String> {
    let cols = params.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16;
    let rows = params.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16;
    let scope = TermScope::parse(params)?;
    require_shell_kind(params)?;

    let (key, rx) = {
        let mut s = state.lock().unwrap();
        let root = scope.resolve_root(&mut s)?;
        // The cap counts the human's shells and never an agent: sixteen open
        // terminals must not be able to crowd a worktree's agent out of a
        // registry they now share.
        if s.shell_tab_count() >= MAX_USER_TERMINALS {
            return Err(format!(
                "terminal limit reached ({MAX_USER_TERMINALS} open terminals) — close one first"
            ));
        }
        let tab_id = format!("term-{}", s.next_term);
        s.next_term += 1;
        let shell = s.term_shell.clone();
        let key = TabKey {
            root: root.clone(),
            tab_id: tab_id.clone(),
        };
        let (tab, rx) = Tab::spawn(
            TabRole::Shell,
            &shell_harness_spec(&shell),
            tab_id,
            root,
            cols,
            rows,
        )?;
        s.tabs.insert(key.clone(), tab);
        (key, rx)
    };
    spawn_tab_pump(state, key.clone(), rx);
    Ok(json!({
        "term_id": key.tab_id,
        "kind": SHELL_TAB_KIND,
        "cols": cols,
        "rows": rows,
    }))
}

/// Attach this client to a tab by its wire id — `term-<n>` or
/// `agent:<worktree_id>`, one verb over one id space.
///
/// Registers the caller's [`SessionSender`] for live output and returns the
/// current **screen snapshot** + cursor. Reconnect is just another attach: a
/// new session re-registers and gets a fresh snapshot. Creation is
/// `term.create`'s (a shell) or a delivery's (the agent) job.
fn term_attach(
    state: &Arc<Mutex<AppState>>,
    sender: &SessionSender,
    params: &Value,
) -> Result<Value, String> {
    let term_id = require_str(params, "term_id")?;
    let cols = params.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16;
    let rows = params.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16;

    // Snapshot the screen and register this client atomically under the lock, so
    // the pump pushes only bytes *after* the cursor to the new sender — no gap, no
    // dupe across a reconnect.
    let mut s = state.lock().unwrap();
    let key = s.tab_key_of_wire_id(&term_id)?;
    Ok(attach_to_tab(&mut s, &key, sender, cols, rows))
}

/// Attach this client to the agent of a WORKTREE, addressed the way the
/// calling surface already knows it.
///
/// The same attach as `term.attach`, but by what a surface holds rather than by
/// a wire id it cannot compute: `id` for a surface that is a plan or a run, and
/// otherwise the scope shapes `term.create`/`term.list` take (`run_id`,
/// `project_id`+`worktree_id`, `project_id` for the primary checkout). Both
/// resolve server-side to the same canonical root — the tab registry's key — so
/// a run and the directory it works in reach one agent, not two.
///
/// **Never errors because no agent is running** — `live: false` with the last
/// (or a blank) snapshot is the contract, because a tab must still show what
/// its agent did before it died, and because the Agent tab is a fixture on
/// every worktree surface: mounting it must not spawn anything. An unknown
/// entity or scope errors, and so does an entity with no worktree (an approved
/// or abandoned plan): its disposable worktree is gone, so there is no worktree
/// to host an agent and the surface renders its empty state instead.
fn agent_attach(
    state: &Arc<Mutex<AppState>>,
    sender: &SessionSender,
    params: &Value,
) -> Result<Value, String> {
    // Grid defaults = the orchestrator's agent PTY size (40 rows × 120 cols).
    let cols = params.get("cols").and_then(Value::as_u64).unwrap_or(120) as u16;
    let rows = params.get("rows").and_then(Value::as_u64).unwrap_or(40) as u16;

    let mut guard = state.lock().unwrap();
    let s = &mut *guard;
    // The id is opaque (plan-… / run-…); what it resolves to is a worktree,
    // because that is what an agent belongs to. Without one, the scope params
    // resolve to the same thing — never a client-supplied path (spec §1).
    let root = match params
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    {
        Some(entity_id) => s.entity_worktree_root(entity_id)?,
        None => TermScope::parse(params)?.resolve_root(s)?,
    };
    let key = TabKey::agent(&root);
    if !s.tabs.contains_key(&key) {
        // No agent has run here yet: a blank, dead screen, and the tab opens on
        // the first delivery. The client still registers — on the screen this
        // worktree's agent will be born onto — because it must go live where it
        // stands when that delivery comes, not sit blank until the human
        // unmounts and remounts the tab.
        let screen = s
            .agent_screens_awaiting_spawn
            .entry(root.clone())
            .or_insert_with(|| TermScreen::new(cols, rows));
        if screen.cols != cols || screen.rows != rows {
            screen.set_size(cols, rows);
        }
        screen.register(sender);
        return Ok(json!({
            "term_id": format!("agent:{}", crate::worktree::external_worktree_id(&root)),
            "live": false,
            "snapshot": screen.snapshot(),
            "cursor": screen.total,
            "cols": screen.cols,
            "rows": screen.rows,
        }));
    }
    Ok(attach_to_tab(s, &key, sender, cols, rows))
}

/// Open a worktree's agent with nothing to say to it — the surface's "Start
/// agent" button, and the "Restart" the human needs when the harness exits on
/// its own (codex running a self-update and quitting, claude crashing).
///
/// Every other way to get an agent is a turn: you say something and the agent
/// is spawned to hear it. That leaves no way to simply have one running, and no
/// way back after an exit short of inventing a message. This verb is that way,
/// and it is the only spawn path with no prompt behind it.
///
/// It carries no turn, so it needs no queue: `ensure_agent_tab` is idempotent on
/// a live tab (`Warm`, same process) and replaces a dead one (`Fresh`, screen
/// carried), which is exactly start-vs-restart. The owner must be an entity that
/// owns a worktree — `.build/mcp.json` routes `done` per owner, so an agent with
/// nobody to report to is worse than none.
fn agent_start(state: &Arc<Mutex<AppState>>, params: &Value) -> Result<Value, String> {
    // `run_id` is the adopting caller's spelling: a worktree surface with no run
    // yet mints one and forwards the verb, and that helper names the id it just
    // minted. Same entity either way.
    let entity_id = params
        .get("id")
        .or_else(|| params.get("run_id"))
        .or_else(|| params.get("plan_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .ok_or("missing id")?;
    let (turn, waiting) = {
        let s = state.lock().unwrap();
        let thread = s.entity_thread(&entity_id)?;
        (
            PendingAgentTurn {
                root: s.entity_worktree_root(&entity_id)?,
                owner: entity_id.clone(),
                model_choice: s.entity_model_choice(&entity_id)?,
                // Only sent when something is actually waiting (below). A hand-
                // started agent has no context, so it gets the cold form: the
                // conversation protocol and the catch-up packet around the nudge.
                cold: crate::orchestrator::conversation_prompt(NEW_THREAD_MESSAGES_PROMPT, thread),
                warm: NEW_THREAD_MESSAGES_PROMPT.to_string(),
                phase: "start",
            },
            thread.has_unread(),
        )
    };

    // The button means "give me an agent", not "go do something" — so a start
    // with nothing waiting says nothing, and the human drives from there.
    // But the reviewer's words are durable on the thread and an agent only
    // learns of them by being TOLD to call `read_unread_messages`; a fresh
    // harness has no reason to. Restarting after a crash with messages
    // outstanding would silently ignore every one of them.
    let (term_id, spawned) = if waiting {
        deliver(
            state,
            &turn.root,
            &turn.owner,
            &turn.model_choice,
            &turn.cold,
            &turn.warm,
        )?
    } else {
        ensure_agent_tab(state, &turn.root, &turn.owner, &turn.model_choice)?
    };

    let mut s = state.lock().unwrap();
    // A fresh process is a new session either way — the lineage must not depend
    // on whether there happened to be mail.
    if spawned == Spawned::Fresh {
        s.record_agent_session_start(&turn);
    }
    s.touch_attention(&entity_id);
    Ok(json!({
        "term_id": term_id,
        "live": true,
        "spawned": match spawned {
            Spawned::Fresh => "fresh",
            Spawned::Warm => "warm",
        },
        "notified": waiting,
    }))
}

/// Register `sender` on a tab's screen and describe what it should render.
///
/// The one attach body both verbs run: match the PTY and screen model to this
/// client's viewport (a TUI draws to the size it was told, so a mismatch
/// garbles), then hand back the snapshot and the monotonic cursor the pump
/// will push from. A DEAD tab is never resized — its retained screen is the
/// last thing its agent painted and must stay legible.
///
/// The caller holds the state lock across this, which is what makes the
/// snapshot and the registration atomic: no bytes land between them.
fn attach_to_tab(
    state: &mut AppState,
    key: &TabKey,
    sender: &SessionSender,
    cols: u16,
    rows: u16,
) -> Value {
    let tab = state
        .tabs
        .get_mut(key)
        .expect("the key came from the registry");
    if tab.live && (tab.screen.cols != cols || tab.screen.rows != rows) {
        let _ = tab.session.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        tab.screen.set_size(cols, rows);
    }
    tab.screen.register(sender);
    json!({
        "term_id": tab.wire_id(),
        "live": tab.live,
        "snapshot": tab.screen.snapshot(),
        "cursor": tab.screen.total,
        "cols": tab.screen.cols,
        "rows": tab.screen.rows,
    })
}

/// Find-or-create the one agent tab rooted at `root`.
///
/// Idempotent per root: the find half and the in-flight reservation are taken
/// under the SAME lock acquisition, so two concurrent callers produce one
/// harness — two agents in one worktree would both report `done` for the same
/// owner, and the second report is an illegal transition that lands on the
/// thread as a bogus failure. A tab whose process has died is replaced (a dead
/// agent is not an agent), and that replacement reports `Fresh` while carrying
/// the retained screen — and its monotonic cursor — forward.
///
/// The create half needs an owner for the MCP `--task` argv, so it requires a
/// bound plan/run: `owner` resolves the project whose orchestrator builds the
/// spec (the MCP socket lives inside that closure and is unreachable from here).
fn ensure_agent_tab(
    state: &Arc<Mutex<AppState>>,
    root: &std::path::Path,
    owner: &str,
    model_choice: &ModelChoice,
) -> Result<(String, Spawned), String> {
    let root = AppState::canonical_root(root);
    let key = TabKey::agent(&root);
    let deadline = std::time::Instant::now() + AGENT_SPAWN_WAIT;
    loop {
        // Under the lock: hand back a live tab, or reserve the spawn. The lock
        // is dropped across the spawn below (it blocks for seconds on the
        // harness's readiness wait, and every terminal pump needs this lock),
        // so the reservation is what the losing caller waits on.
        let reserved = {
            let mut s = state.lock().unwrap();
            if let Some(tab) = s.tabs.get(&key) {
                if tab.live && !tab.session.has_exited() {
                    return Ok((tab.wire_id(), Spawned::Warm));
                }
            }
            if s.agent_spawns_in_flight.contains(&root) {
                None
            } else {
                let carried = s.tabs.remove(&key).map(|dead| {
                    dead.session.kill_and_reap();
                    dead.screen
                });
                let project_id = s.project_of(owner)?;
                let orch = s.orch_for(&project_id)?;
                // Unconditional: under `--strict-mcp-config` a missing config
                // kills the harness before it reads a byte of the prompt, and
                // the scaffold is idempotent.
                orch.scaffold_agent_worktree(&root, owner).map_err(err)?;
                // A Build-owned tab respawned after a crash should always pick
                // its own transcript back up, so the probe is unconditional too.
                let continue_session = (s.transcript_probe)(&root, model_choice.provider);
                let spec = orch.agent_harness_spec(owner, &root, model_choice, continue_session);
                let size = orch.pty_size();
                s.agent_spawns_in_flight.insert(root.clone());
                Some((spec, size, carried))
            }
        };

        let Some((spec, size, carried)) = reserved else {
            // Someone else is spawning this root's agent: wait for their tab
            // rather than start a second harness beside it.
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "timed out waiting for the agent starting in {}",
                    root.display()
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
            continue;
        };

        let spawned = Tab::spawn(
            TabRole::Agent {
                owner: owner.to_string(),
                provider: model_choice.provider,
            },
            &spec,
            AGENT_TAB_ID.to_string(),
            root.clone(),
            size.cols,
            size.rows,
        );
        let (mut tab, rx) = match spawned {
            Ok(spawned) => spawned,
            Err(error) => {
                state.lock().unwrap().agent_spawns_in_flight.remove(&root);
                return Err(error);
            }
        };
        if let Some(screen) = carried {
            // Reconnect is snapshot + cursor: a replacement process must never
            // rewind that cursor, and clients already attached stay attached.
            // The new PTY takes the retained screen's grid so the two agree.
            let _ = tab.session.resize(PtySize {
                rows: screen.rows,
                cols: screen.cols,
                pixel_width: 0,
                pixel_height: 0,
            });
            tab.screen = screen;
        }
        // An interactive TUI must be servicing its PTY before a turn is written
        // into it, or the prompt lands on a startup screen.
        tab.session
            .ready_within(crate::orchestrator::HARNESS_READY_GRACE);

        let wire_id = tab.wire_id();
        {
            let mut s = state.lock().unwrap();
            // Clients that mounted the Agent tab before this worktree had one
            // are attached to a screen with no PTY. Carry them — and the
            // viewport they render at, the same rule an attach to a live tab
            // follows — onto the real screen, under the SAME lock acquisition
            // that publishes the tab, so a client attaching during the spawn is
            // on one screen or the other and never between them. The waiting
            // screen's cursor is not carried: it painted nothing, while a
            // retained screen's cursor is the one that must never rewind.
            if let Some(waiting) = s.agent_screens_awaiting_spawn.remove(&root) {
                let _ = tab.session.resize(PtySize {
                    rows: waiting.rows,
                    cols: waiting.cols,
                    pixel_width: 0,
                    pixel_height: 0,
                });
                tab.screen.set_size(waiting.cols, waiting.rows);
                for client in &waiting.attached {
                    tab.screen.register(client);
                }
            }
            s.tabs.insert(key.clone(), tab);
            s.agent_spawns_in_flight.remove(&root);
        }
        spawn_tab_pump(state, key, rx);
        return Ok((wire_id, Spawned::Fresh));
    }
}

/// The one pipe from Build to a worktree's agent.
///
/// Ensures the tab exists, then submits exactly one turn through
/// [`PtySession::write_prompt`] — the harness's own submit key and bracketed
/// paste framing, never a raw write with a hardcoded `\r`. Which text travels
/// is decided by whether the tab had to be spawned: `cold` for an agent with no
/// context to read messages into, `warm` for one already in the conversation,
/// whose messages are already durable in the thread for `read_unread_messages`
/// to pull. Returns the tab's wire id and which half travelled — a `Fresh`
/// delivery is a new agent process, which the conversation records as the start
/// of a session.
fn deliver(
    state: &Arc<Mutex<AppState>>,
    root: &std::path::Path,
    owner: &str,
    model_choice: &ModelChoice,
    cold: &str,
    warm: &str,
) -> Result<(String, Spawned), String> {
    let (wire_id, spawned) = ensure_agent_tab(state, root, owner, model_choice)?;
    let prompt = match spawned {
        Spawned::Fresh => cold,
        Spawned::Warm => warm,
    };
    let key = TabKey::agent(&AppState::canonical_root(root));
    let mut s = state.lock().unwrap();
    let tab = s
        .tabs
        .get_mut(&key)
        .ok_or("the agent tab closed before its turn could be delivered")?;
    if let Err(error) = tab.session.write_prompt(prompt) {
        // A harness that exits immediately still owns its tab: PTYs return EIO
        // once the child's side is closed, and the child closes it BEFORE the
        // OS makes its exit status reapable, so a single poll here races the
        // kernel. The bounded wait covers that lag; a genuinely wedged PTY
        // (live but unwritable) still surfaces its error.
        if !tab
            .session
            .exited_within(crate::orchestrator::PROMPT_WRITE_EXIT_GRACE)
        {
            return Err(error.to_string());
        }
    }
    // The quiescence clock restarts here: whatever the agent was silent about
    // before, it now has something to answer for.
    tab.last_delivered_at = Some(std::time::Instant::now());
    Ok((wire_id, spawned))
}

/// Send every turn the verbs that just ran queued, now that the state lock is
/// free.
///
/// A cold delivery starts a new harness process, so it opens the conversation's
/// session lineage — the record the thread reads back as "the revise agent
/// started here". A warm delivery continues the session already open.
fn deliver_pending_agent_turns(state: &Arc<Mutex<AppState>>) {
    // Taking the queue and marking those owners in flight happen under ONE lock
    // acquisition, so there is no instant in which a queued turn is invisible to
    // the idle sweep and its entity looks agentless.
    let queued = {
        let mut s = state.lock().unwrap();
        let queued = std::mem::take(&mut s.pending_agent_turns);
        for turn in &queued {
            *s.agent_turns_in_flight
                .entry(turn.owner.clone())
                .or_default() += 1;
        }
        queued
    };
    for turn in queued {
        let delivered = deliver(
            state,
            &turn.root,
            &turn.owner,
            &turn.model_choice,
            &turn.cold,
            &turn.warm,
        );
        let mut s = state.lock().unwrap();
        match delivered {
            Ok((_, Spawned::Fresh)) => s.record_agent_session_start(&turn),
            Ok((_, Spawned::Warm)) => {}
            // The turn stays durable on the thread — the agent picks it up with
            // `read_unread_messages` the next time a tab opens — but nothing is
            // reading that thread right now, so the entity itself has to carry
            // the reason. The idle sweep finishes the job: an entity left
            // working with no agent tab is demoted on the next pass.
            Err(error) => {
                eprintln!("deliver to {}: {error}", turn.owner);
                s.record_agent_delivery_failure(&turn, &error);
            }
        }
        // Off the queue and out of flight: from here the entity's agent tab is
        // the whole truth about whether an agent is there.
        if let std::collections::hash_map::Entry::Occupied(mut in_flight) =
            s.agent_turns_in_flight.entry(turn.owner.clone())
        {
            *in_flight.get_mut() -= 1;
            if *in_flight.get() == 0 {
                in_flight.remove();
            }
        }
    }
}

/// Pump one tab's PTY into its screen model, coalescing at `TERM_FLUSH_MS` and
/// flushing one keyed frame to every attached client.
///
/// Start of session: the parser is reset to a blank screen of the current grid
/// and `term.reset` is pushed (clients wipe; a replacement process starts
/// clean) — `screen.total` is NEVER reset, because client dedupe rides the
/// monotonic cursor. On EOF a Shell tab is removed, reaped, and pushed
/// `term.closed{exited}`; an Agent tab is RETAINED with `live = false` and
/// pushed `term.closed{agent_session_ended}`, because the tab must still show
/// the last screen.
///
/// One pump per tab for the tab's whole life: with one PTY per worktree there
/// is no phase boundary to generation-guard against — a missing tab is the
/// only stop condition.
fn spawn_tab_pump(state: &Arc<Mutex<AppState>>, key: TabKey, mut rx: broadcast::Receiver<Vec<u8>>) {
    if tokio::runtime::Handle::try_current().is_err() {
        // Sync unit tests drive the registry without a runtime; there is
        // nothing to spawn the pump onto and nothing attached to feed.
        return;
    }
    let state = Arc::clone(state);
    tokio::spawn(async move {
        let term_id = {
            let mut s = state.lock().unwrap();
            let Some(tab) = s.tabs.get_mut(&key) else {
                return;
            };
            let term_id = tab.wire_id();
            tab.screen.parser = vt100::Parser::new(tab.screen.rows, tab.screen.cols, 2000);
            tab.screen.pending.clear();
            let payload = json!({
                "type": "term.reset",
                "term_id": term_id,
                "data": tab.screen.snapshot(),
                "cursor": tab.screen.total,
            });
            tab.screen.attached.retain(|snd| snd.push(payload.clone()));
            term_id
        };
        let mut flush = tokio::time::interval(Duration::from_millis(TERM_FLUSH_MS));
        flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                recv = rx.recv() => match recv {
                    Ok(chunk) => {
                        let mut s = state.lock().unwrap();
                        let Some(tab) = s.tabs.get_mut(&key) else { return; };
                        tab.screen.process(&chunk);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        let mut s = state.lock().unwrap();
                        let Some(tab) = s.tabs.get_mut(&key) else { return; };
                        let ended_owner = match &tab.role {
                            TabRole::Agent { owner, .. } => Some(owner.clone()),
                            TabRole::Shell => None,
                        };
                        match ended_owner {
                            Some(owner) => {
                                tab.live = false;
                                tab.screen.flush(&term_id);
                                tab.screen.push_closed(&term_id, "agent_session_ended");
                                // The process is what a session IS, so this is
                                // where the conversation's lineage closes.
                                s.record_agent_session_end(&owner);
                            }
                            None => {
                                let Some(tab) = s.tabs.remove(&key) else { return; };
                                tab.session.kill_and_reap();
                                tab.screen.push_closed(&term_id, "exited");
                            }
                        }
                        return;
                    }
                },
                _ = flush.tick() => {
                    let mut s = state.lock().unwrap();
                    let Some(tab) = s.tabs.get_mut(&key) else { return; };
                    tab.screen.flush(&term_id);
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
    fn capture_login_path_rejects_a_bare_launchd_path() {
        // A shell whose rc files never widen PATH leaves us with launchd's bare
        // default. Adopting it is worse than useless: it *looks* like the fix
        // worked while `claude` still can't be found. Reject it so the daemon
        // logs the truth and keeps whatever it inherited.
        let shell = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            shell.path(),
            "#!/bin/sh\nprintf %s /usr/bin:/bin:/usr/sbin:/sbin\n",
        )
        .unwrap();
        std::fs::set_permissions(
            shell.path(),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();

        assert_eq!(
            capture_login_path(&shell.path().to_string_lossy(), Duration::from_secs(5)),
            None
        );
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

    /// The OS pid behind a tab, addressed the way a client addresses it.
    fn tab_pid(state: &Arc<Mutex<AppState>>, wire_id: &str) -> Option<u32> {
        let s = state.lock().unwrap();
        let key = s.tab_key_of_wire_id(wire_id).ok()?;
        s.tabs[&key].session.pid()
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
        let pid = tab_pid(&state, "term-1").expect("the shell is registered");
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
        assert_eq!(state.lock().unwrap().shell_tab_count(), 0);
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

    /// A terminal belongs to the WORKTREE it was opened in, not to whichever
    /// entity happened to name that worktree when it was created.
    ///
    /// The client addresses an unadopted worktree as `{project_id,
    /// worktree_id}` and an adopted one as `{run_id}` — two scope shapes over
    /// one directory. Keying the registry by the canonical root is what makes
    /// adoption invisible to an open shell; keying it by scope made the shell
    /// vanish from the tab row while its process kept running.
    #[tokio::test]
    async fn term_list_follows_a_worktree_across_adoption() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let project_id = state.lock().unwrap().projects[0].id.clone();
        add_external_worktree(&repo, dir.path(), "feature-x", "feature-x");
        let worktree_id = state
            .lock()
            .unwrap()
            .external_worktrees(&project_id, true)
            .unwrap()
            .into_iter()
            .find(|w| w.branch.as_deref() == Some("feature-x"))
            .expect("the external worktree is discoverable")
            .id;

        let created = handler(
            SessionSender::detached("s1"),
            req(
                "term.create",
                json!({ "project_id": project_id, "worktree_id": worktree_id }),
            ),
        );
        assert_eq!(created["ok"], true, "{created:?}");
        let term_id = created["result"]["term_id"].as_str().unwrap().to_string();
        let shell_pid = tab_pid(&state, &term_id).expect("the shell is registered");

        let adopted = handler(
            SessionSender::detached("s1"),
            req(
                "run.adopt",
                json!({ "project_id": project_id, "worktree_id": worktree_id }),
            ),
        );
        assert_eq!(adopted["ok"], true, "{adopted:?}");
        let run_id = run_id_of(&adopted);

        let listed = handler(
            SessionSender::detached("s1"),
            req("term.list", json!({ "run_id": run_id })),
        );
        let terminals = listed["result"]["terminals"].as_array().unwrap();
        assert_eq!(
            terminals.len(),
            1,
            "the shell survives adoption on the run scope: {listed:?}"
        );
        assert_eq!(terminals[0]["term_id"], json!(term_id));
        // The SAME shell, not a fresh one: adoption is a record change, and the
        // process the human was typing into never noticed it.
        assert_eq!(
            tab_pid(&state, &term_id),
            Some(shell_pid),
            "adoption must not restart the human's shell"
        );
    }

    /// One directory, two spellings, ONE tab registry.
    ///
    /// The same worktree reaches the daemon under literally different paths: a
    /// run's is `worktrees_root/<name>` while the scanner canonicalizes, and on
    /// macOS `/tmp` IS `/private/tmp`. Every scope funnels through the canonical
    /// form for exactly this reason — keyed by the spelling it was asked with, a
    /// shell opened through one path is invisible through the other while its
    /// process keeps running, which is the orphan tab this design dissolves.
    #[tokio::test]
    async fn a_worktree_spelled_two_ways_holds_one_set_of_tabs() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
        let canonical_root = {
            let mut s = state.lock().unwrap();
            let root = insert_run(
                &mut s,
                &repo,
                dir.path(),
                "run-canonical",
                RunState::Building,
            );
            insert_run(&mut s, &repo, dir.path(), "run-aliased", RunState::Building);
            // The second run addresses that SAME directory under another name.
            let alias = dir.path().join("alias-to-worktree");
            std::os::unix::fs::symlink(&root, &alias).unwrap();
            s.runs.get_mut("run-aliased").unwrap().worktree.path = alias;
            root
        };

        let created = call(&handler, "term.create", json!({ "run_id": "run-aliased" }));
        assert_eq!(created["ok"], true, "{created:?}");
        let term_id = created["result"]["term_id"].as_str().unwrap().to_string();

        // Asked about through the OTHER spelling, the same directory holds the
        // same shell.
        let listed = call(&handler, "term.list", json!({ "run_id": "run-canonical" }));
        let listed_ids: Vec<&str> = listed["result"]["terminals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["term_id"].as_str().unwrap())
            .collect();
        assert_eq!(
            listed_ids,
            vec![term_id.as_str()],
            "both spellings name one worktree, so both see its shell: {listed:?}"
        );

        let s = state.lock().unwrap();
        let keys: Vec<&TabKey> = s.tabs.keys().collect();
        assert!(
            keys.iter().all(|key| key.root == canonical_root),
            "every tab is keyed by the canonical root ({canonical_root:?}): {keys:?}"
        );
    }

    /// A worktree's tab row is its own. The registry is one map over every
    /// worktree the daemon holds, so the only thing keeping one directory's
    /// shells out of another's row is the filter on the resolved root.
    #[tokio::test]
    async fn term_list_shows_only_the_shells_of_the_worktree_asked_about() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
        {
            let mut s = state.lock().unwrap();
            insert_run(&mut s, &repo, dir.path(), "run-here", RunState::Building);
            insert_run(&mut s, &repo, dir.path(), "run-there", RunState::Building);
        }
        let shell_in = |run_id: &str| {
            let created = call(&handler, "term.create", json!({ "run_id": run_id }));
            assert_eq!(created["ok"], true, "{created:?}");
            created["result"]["term_id"].as_str().unwrap().to_string()
        };
        let here = shell_in("run-here");
        let there = shell_in("run-there");
        assert_ne!(here, there, "two worktrees, two shells");

        for (run_id, own, other) in [("run-here", &here, &there), ("run-there", &there, &here)] {
            let listed = call(&handler, "term.list", json!({ "run_id": run_id }));
            let ids: Vec<&str> = listed["result"]["terminals"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["term_id"].as_str().unwrap())
                .collect();
            assert_eq!(
                ids,
                vec![own.as_str()],
                "{run_id} must show its own shell and not {other}: {listed:?}"
            );
        }
    }

    /// One attach verb over one id space. A client holds a row of tabs — some
    /// shells, one agent — and must not need to know which RPC each one
    /// answers to; the id says everything.
    #[tokio::test]
    async fn one_attach_verb_serves_shells_and_the_agent() {
        let (dir, repo) = init_repo();
        let (state, handler, root) = agent_tab_fixture(&repo, dir.path(), "run-attach");
        let (agent_wire_id, _) =
            ensure_agent_tab(&state, &root, "run-attach", &ModelChoice::default()).unwrap();

        let attached = handler(
            SessionSender::detached("s1"),
            req(
                "term.attach",
                json!({ "term_id": agent_wire_id, "cols": 120, "rows": 40 }),
            ),
        );
        assert_eq!(
            attached["ok"], true,
            "term.attach serves an agent id too: {attached:?}"
        );
        assert_eq!(attached["result"]["term_id"], json!(agent_wire_id));
        assert_eq!(attached["result"]["live"], true);
        assert!(attached["result"]["snapshot"].is_string());
        assert!(attached["result"]["cursor"].is_u64());

        // A well-formed agent id for a worktree with no tab is still "unknown
        // term_id", so a stale client drops the tab instead of hanging on one
        // that swallows every keystroke.
        let stale = handler(
            SessionSender::detached("s1"),
            req("term.attach", json!({ "term_id": "agent:nope" })),
        );
        assert_eq!(stale["ok"], false, "{stale:?}");
        assert_eq!(stale["error"], "unknown term_id");
    }

    /// The agent tab is not one of the human's tabs to close.
    ///
    /// `term.close` serves one id space, so the agent's wire id resolves there
    /// like any other — and closing it would kill the one PTY every human→agent
    /// path lands in, from a surface that renders it as a `×`-less fixture. The
    /// refusal is what makes "always reachable" survive a stale or hand-rolled
    /// client; the tab's life belongs to the worktree.
    #[tokio::test]
    async fn term_close_refuses_the_agent_tab() {
        let (dir, repo) = init_repo();
        let (state, handler, root) = agent_tab_fixture(&repo, dir.path(), "run-unclosable");
        let (agent_wire_id, _) =
            ensure_agent_tab(&state, &root, "run-unclosable", &ModelChoice::default()).unwrap();

        let refused = handler(
            SessionSender::detached("s1"),
            req("term.close", json!({ "term_id": agent_wire_id })),
        );
        assert_eq!(refused["ok"], false, "{refused:?}");
        assert_eq!(refused["error"], "cannot close an agent terminal");

        let s = state.lock().unwrap();
        let key = TabKey::agent(&AppState::canonical_root(&root));
        let tab = s.tabs.get(&key).expect("the agent tab is still registered");
        assert!(tab.live, "and its session was never killed");
        assert!(!tab.session.has_exited());
    }

    /// A user terminal is the user's own login shell and nothing else. The
    /// daemon owns the argv, so a client naming a kind can never turn a tab
    /// into an arbitrary command line.
    #[test]
    fn a_user_terminal_only_ever_launches_the_login_shell() {
        let shell = shell_harness_spec("/bin/zsh");
        assert_eq!(shell.binary, "/bin/zsh");
        assert_eq!(shell.args, ["-i", "-l"]);
    }

    /// The `+` menu no longer offers to start an agent, and the daemon refuses
    /// to if asked.
    ///
    /// A `claude`/`codex` tab carried the provider's approvals bypass and NO
    /// `done` MCP server: an agent in a worktree that Build could not talk to,
    /// could not route a report from, and did not count as the worktree's one
    /// agent. It was the only way to get a second agent into a worktree, so
    /// removing it is what makes "one worktree, one agent, Build owns it" true
    /// rather than merely intended. An old client asking must fail loudly and
    /// be told where the agent actually lives — never fall back to a shell,
    /// which would silently run a different program than was asked for.
    #[test]
    fn a_terminal_kind_naming_an_agent_is_refused_and_points_at_the_agent_tab() {
        assert!(require_shell_kind(&json!({})).is_ok());
        assert!(require_shell_kind(&json!({ "kind": "" })).is_ok());
        assert!(require_shell_kind(&json!({ "kind": "shell" })).is_ok());

        for named_agent in ["claude", "codex"] {
            let refused = require_shell_kind(&json!({ "kind": named_agent })).unwrap_err();
            assert!(
                refused.contains("Agent tab"),
                "{named_agent}: {refused:?} must name where the agent lives"
            );
        }
        assert_eq!(
            require_shell_kind(&json!({ "kind": "sh -c curl evil" })).unwrap_err(),
            "unknown terminal kind \"sh -c curl evil\" — a user terminal is always the shell"
        );
    }

    /// The kind rides the wire both ways: `term.create` echoes it and
    /// `term.list` carries it, so a reloaded client labels the tab by what is
    /// actually running in it. There is only one answer now — `shell` — and it
    /// stays on the wire because the SPA reads it.
    #[tokio::test]
    async fn term_create_carries_its_kind_onto_the_tab_list() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let project_id = state.lock().unwrap().projects[0].id.clone();

        let created = handler(
            SessionSender::detached("s1"),
            req(
                "term.create",
                json!({ "project_id": project_id, "kind": "shell" }),
            ),
        );
        assert_eq!(created["ok"], true, "{created:?}");
        assert_eq!(created["result"]["kind"], "shell");

        let listed = handler(
            SessionSender::detached("s1"),
            req("term.list", json!({ "project_id": project_id })),
        );
        let terminals = listed["result"]["terminals"].as_array().unwrap();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0]["kind"], "shell");

        // An unknown kind is refused BEFORE anything is spawned.
        let bogus = handler(
            SessionSender::detached("s1"),
            req(
                "term.create",
                json!({ "project_id": project_id, "kind": "bash -c evil" }),
            ),
        );
        assert_eq!(bogus["ok"], false, "{bogus:?}");
        assert!(
            bogus["error"]
                .as_str()
                .unwrap()
                .contains("unknown terminal kind"),
            "{bogus:?}"
        );
        assert_eq!(state.lock().unwrap().shell_tab_count(), 1);
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
        let pid = tab_pid(&state, "term-1").expect("the shell is registered");

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
        assert_eq!(state.lock().unwrap().shell_tab_count(), 0);
        assert!(process_reaped(pid), "an exited shell must still be reaped");
    }

    // ---- the agent tab primitive: a path-keyed, tab-backed PTY --------------

    /// A shared QA state, a worktree-like root to run an agent in, and an owner
    /// id bound to the state's project — the three things an agent tab needs
    /// (the owner resolves the project whose orchestrator builds the harness).
    fn agent_tab_fixture(
        repo: &std::path::Path,
        dir: &std::path::Path,
        owner: &str,
    ) -> (Arc<Mutex<AppState>>, FrameHandler, PathBuf) {
        let (state, handler) = shared_state_and_handler(repo, dir);
        let root = dir.join("agent-root");
        std::fs::create_dir_all(&root).unwrap();
        {
            let mut s = state.lock().unwrap();
            let project_id = s.projects[0].id.clone();
            s.entity_project.insert(owner.to_string(), project_id);
        }
        (state, handler, root)
    }

    /// The agent tab's screen, rendered — what an attaching client would see.
    fn agent_screen_text(state: &Arc<Mutex<AppState>>, root: &std::path::Path) -> String {
        let key = TabKey::agent(&AppState::canonical_root(root));
        let s = state.lock().unwrap();
        let Some(tab) = s.tabs.get(&key) else {
            return String::new();
        };
        String::from_utf8_lossy(&b64decode(&tab.screen.snapshot()).unwrap()).into_owned()
    }

    /// Poll the agent tab's screen until it shows `needle` (the pump feeds it),
    /// returning what was on screen at the end either way.
    async fn wait_for_agent_screen(
        state: &Arc<Mutex<AppState>>,
        root: &std::path::Path,
        needle: &str,
    ) -> String {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let text = agent_screen_text(state, root);
            if text.contains(needle) || std::time::Instant::now() >= deadline {
                return text;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// One worktree, one agent: find-or-create keyed by the canonical root, so
    /// a second call hands back the SAME tab (warm) rather than a second
    /// harness in the same directory.
    #[tokio::test]
    async fn ensure_agent_tab_is_idempotent_for_one_root() {
        let (dir, repo) = init_repo();
        let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-one-agent");
        let choice = ModelChoice::default();

        let (first_id, first) =
            ensure_agent_tab(&state, &root, "run-one-agent", &choice).expect("the agent spawns");
        let (second_id, second) =
            ensure_agent_tab(&state, &root, "run-one-agent", &choice).expect("the agent is found");

        assert_eq!(first, Spawned::Fresh, "the first call creates the tab");
        assert_eq!(second, Spawned::Warm, "the second call finds it");
        assert_eq!(first_id, second_id, "both calls address one tab");
        assert_eq!(
            first_id,
            format!(
                "agent:{}",
                crate::worktree::external_worktree_id(&std::fs::canonicalize(&root).unwrap())
            ),
            "an agent tab is addressed by its worktree, not by its owner"
        );

        let s = state.lock().unwrap();
        assert_eq!(s.tabs.len(), 1, "exactly one tab in the registry");
        assert!(
            s.agent_spawns_in_flight.is_empty(),
            "the spawn reservation is released"
        );
        // Under --strict-mcp-config a missing config kills the harness before it
        // reads a byte of the prompt, so the scaffold is part of the spawn.
        assert!(root.join(".build/mcp.json").exists());
    }

    /// Two deliveries racing on one worktree must produce ONE harness: the find
    /// and the in-flight reservation are taken under the same lock, so the
    /// loser waits for the winner's tab instead of spawning a second agent
    /// (two agents in one worktree both report `done` for the same owner, and
    /// the second report lands as a bogus failure on the thread).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_ensure_agent_tab_spawns_one_agent() {
        let (dir, repo) = init_repo();
        let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-race");

        let mut racers = Vec::new();
        for _ in 0..4 {
            let state = Arc::clone(&state);
            let root = root.clone();
            racers.push(tokio::task::spawn_blocking(move || {
                ensure_agent_tab(&state, &root, "run-race", &ModelChoice::default())
            }));
        }
        let mut outcomes = Vec::new();
        for racer in racers {
            outcomes.push(racer.await.unwrap().expect("every racer gets the tab"));
        }

        let fresh = outcomes
            .iter()
            .filter(|(_, spawned)| *spawned == Spawned::Fresh)
            .count();
        assert_eq!(fresh, 1, "exactly one caller spawned: {outcomes:?}");
        assert!(
            outcomes.iter().all(|(id, _)| id == &outcomes[0].0),
            "every caller addresses the same tab: {outcomes:?}"
        );
        let s = state.lock().unwrap();
        assert_eq!(s.tabs.len(), 1, "one worktree, one agent");
        assert!(s.agent_spawns_in_flight.is_empty());
    }

    /// The cold/warm rule: a tab that had to be spawned gets the full prompt (a
    /// cold agent has no context to read messages into), and a tab that was
    /// already alive gets the short nudge — the messages are already durable in
    /// the thread. The PTY echoes what is written to it, so the tab's screen is
    /// the proof of which one travelled.
    #[tokio::test]
    async fn deliver_sends_the_cold_prompt_on_a_fresh_tab_and_the_nudge_on_a_warm_one() {
        let (dir, repo) = init_repo();
        let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-deliver");
        let choice = ModelChoice::default();

        let (cold_id, cold_spawned) = deliver(
            &state,
            &root,
            "run-deliver",
            &choice,
            "COLD-CONTEXT-PROMPT",
            "WARM-NUDGE-PROMPT",
        )
        .expect("a cold delivery spawns and submits");
        assert_eq!(cold_spawned, Spawned::Fresh);
        let cold_screen = wait_for_agent_screen(&state, &root, "COLD-CONTEXT-PROMPT").await;
        assert!(
            cold_screen.contains("COLD-CONTEXT-PROMPT"),
            "a fresh tab hears the cold prompt: {cold_screen:?}"
        );
        assert!(
            !cold_screen.contains("WARM-NUDGE-PROMPT"),
            "a fresh tab must NOT hear the nudge: {cold_screen:?}"
        );

        let (warm_id, warm_spawned) = deliver(
            &state,
            &root,
            "run-deliver",
            &choice,
            "COLD-CONTEXT-PROMPT",
            "WARM-NUDGE-PROMPT",
        )
        .expect("a warm delivery reuses the tab");
        assert_eq!(warm_spawned, Spawned::Warm);
        assert_eq!(warm_id, cold_id, "both deliveries address one tab");
        let warm_screen = wait_for_agent_screen(&state, &root, "WARM-NUDGE-PROMPT").await;
        assert!(
            warm_screen.contains("WARM-NUDGE-PROMPT"),
            "a warm tab hears the nudge: {warm_screen:?}"
        );
        assert_eq!(state.lock().unwrap().tabs.len(), 1);
    }

    /// The conversation's open session for `owner`, if it has one.
    fn open_session_count(state: &Arc<Mutex<AppState>>, owner: &str) -> usize {
        state.lock().unwrap().runs[owner]
            .thread
            .sessions
            .iter()
            .filter(|session| session.ended_at.is_none())
            .count()
    }

    /// A session belongs to the agent PROCESS, not to a phase. `done` is the
    /// agent finishing a turn at its prompt — it is still there, still in the
    /// same session — so the thread must not record a session end, and the warm
    /// turn that follows must not read back as a turn taken outside any
    /// session.
    #[tokio::test]
    async fn done_then_a_warm_turn_stays_in_one_open_session() {
        let (dir, repo) = init_repo();
        let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-lineage");
        state.lock().unwrap().runs.insert(
            "run-lineage".into(),
            crate::orchestrator::ActiveRun::reattach(
                &fake_run_record("run-lineage"),
                ".build/plan.md".into(),
            ),
        );
        let queue_turn = || {
            state
                .lock()
                .unwrap()
                .pending_agent_turns
                .push(PendingAgentTurn {
                    root: AppState::canonical_root(&root),
                    owner: "run-lineage".into(),
                    model_choice: ModelChoice::default(),
                    cold: "COLD-TURN".into(),
                    warm: "WARM-TURN".into(),
                    phase: "build",
                });
        };

        queue_turn();
        deliver_pending_agent_turns(&state);
        assert_eq!(
            open_session_count(&state, "run-lineage"),
            1,
            "a cold delivery opens the session"
        );

        state.lock().unwrap().on_agent_done(
            "run-lineage",
            DoneReport {
                phase: DonePhase::Build,
                status: DoneStatus::Completed,
                summary: "built".into(),
                outputs: DoneOutputs::default(),
            },
        );
        queue_turn();
        deliver_pending_agent_turns(&state);

        let s = state.lock().unwrap();
        let thread = &s.runs["run-lineage"].thread;
        assert_eq!(
            thread.sessions.len(),
            1,
            "a warm turn continues the one session: {:?}",
            thread.sessions
        );
        assert!(
            thread.sessions[0].ended_at.is_none(),
            "the agent is still at its prompt: {:?}",
            thread.sessions
        );
        assert!(
            !thread.items.iter().any(|item| matches!(
                item,
                crate::thread::ThreadItem::Event(event)
                    if event.event == crate::thread::ThreadEventKind::SessionEnded
            )),
            "no session ended, so the thread must not say one did: {:?}",
            thread.items
        );
    }

    /// A session ends where it really ends: when the agent's process does. The
    /// pump's EOF is the only place that knows, so that is where the thread
    /// learns it — otherwise a run whose agent died reads back as forever in
    /// session.
    #[tokio::test]
    async fn the_session_closes_when_the_agent_process_exits() {
        let (dir, repo) = init_repo();
        let (state, _handler) = shared_state_and_handler(&repo, dir.path());
        let (tab_key, _wire_id) =
            insert_live_run(&state, &repo, dir.path().join("side"), "run-eof");
        state
            .lock()
            .unwrap()
            .record_agent_session_start(&PendingAgentTurn {
                root: tab_key.root.clone(),
                owner: "run-eof".into(),
                model_choice: ModelChoice::default(),
                cold: String::new(),
                warm: String::new(),
                phase: "build",
            });
        assert_eq!(open_session_count(&state, "run-eof"), 1);

        state.lock().unwrap().tabs[&tab_key].session.kill_and_reap();

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while open_session_count(&state, "run-eof") > 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "the agent's process ended and the session never closed"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let s = state.lock().unwrap();
        assert_eq!(
            s.runs["run-eof"].thread.sessions.len(),
            1,
            "the dead session is closed, not replaced"
        );
    }

    /// Point a QA state's only project at a different agent — the seam every
    /// test that cares about what actually gets spawned goes through.
    fn use_agent(state: &Arc<Mutex<AppState>>, repo: &std::path::Path, wt: PathBuf, agent: Agent) {
        state.lock().unwrap().projects[0].orch =
            Orchestrator::new(repo.to_path_buf(), wt, agent, Templates::default());
    }

    /// The rendered turn is always multi-line (the conversation protocol block
    /// is appended), so through a real TUI it must arrive as ONE bracketed
    /// paste. The capture harness never paints, so this also proves the
    /// readiness grace expires into a write rather than a silently lost prompt.
    #[tokio::test]
    async fn a_delivered_turn_reaches_a_silent_harness_as_one_bracketed_paste() {
        let (dir, repo) = init_repo();
        let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-paste");
        let capture = dir.path().join("agent-stdin.txt");
        let capture_for_builder = capture.clone();
        use_agent(
            &state,
            &repo,
            dir.path().join("wt2"),
            Agent::WarmBuilder(std::sync::Arc::new(
                move |_prompt: &str, _choice: &ModelChoice, _options: &SpawnOptions| {
                    HarnessSpec::new("sh")
                        .arg("-c")
                        .arg("cat > \"$1\"")
                        .arg("build-agent-capture")
                        .arg(capture_for_builder.to_string_lossy())
                },
            )),
        );

        deliver(
            &state,
            &root,
            "run-paste",
            &ModelChoice::default(),
            "Paste framing marker\nsecond line",
            "warm",
        )
        .expect("the delivery reaches a silent harness");

        let captured = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Ok(contents) = std::fs::read_to_string(&capture) {
                    if contents.contains("\u{1b}[201~") {
                        return contents;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the framed turn should reach the harness despite its silence");

        assert!(
            captured.starts_with("\u{1b}[200~"),
            "the turn opens as a bracketed paste: {captured:?}"
        );
        assert!(
            captured.contains("Paste framing marker"),
            "the rendered turn rides inside the frame: {captured:?}"
        );
    }

    /// The prompt-write race, end to end: a harness that exits instantly closes
    /// its PTY (the write fails with EIO) *before* the OS makes its exit status
    /// reapable, so a single `has_exited` poll says "running". That must not
    /// fail the delivery — the tab is still the agent's tab, and the crash is
    /// the idle monitor's to report, not the delivery's.
    #[tokio::test]
    async fn a_harness_that_exits_under_the_prompt_write_still_keeps_its_tab() {
        let (dir, repo) = init_repo();
        let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-exits");
        use_agent(&state, &repo, dir.path().join("wt2"), instant_exit_agent());

        let delivered = deliver(
            &state,
            &root,
            "run-exits",
            &ModelChoice::default(),
            "cold",
            "warm",
        );

        assert!(
            delivered.is_ok(),
            "a write against an exiting harness is benign: {delivered:?}"
        );
        let s = state.lock().unwrap();
        assert!(
            s.tabs
                .contains_key(&TabKey::agent(&AppState::canonical_root(&root))),
            "the tab is retained so the crash is legible"
        );
    }

    /// An agent that is gone before it reads a byte.
    fn instant_exit_agent() -> Agent {
        Agent::WarmBuilder(std::sync::Arc::new(
            |_prompt: &str, _choice: &ModelChoice, _options: &SpawnOptions| {
                HarnessSpec::new("sh").arg("-c").arg("exit 0")
            },
        ))
    }

    /// The daemon-wide terminal cap counts the human's own shells WHEREVER they
    /// are held — including the tab registry — and never counts an agent tab.
    ///
    /// Both halves have teeth. A shell that escapes the cap by living on the
    /// new registry is the cap quietly doubling; an agent that sixteen open
    /// shells could crowd out is not "always reachable", which is the
    /// invariant's direct negation. So the fixture holds one agent tab and one
    /// shell tab: exactly fifteen more shells must fit, and the sixteenth must
    /// not.
    #[tokio::test]
    async fn the_terminal_cap_counts_shell_tabs_and_never_the_agent() {
        let (dir, repo) = init_repo();
        let (state, handler, root) = agent_tab_fixture(&repo, dir.path(), "run-cap");
        let project_id = state.lock().unwrap().projects[0].id.clone();

        ensure_agent_tab(&state, &root, "run-cap", &ModelChoice::default()).unwrap();
        // One of the human's own shells, held as a tab rather than in `terms`.
        let shell_root = AppState::canonical_root(&repo);
        let shell_key = TabKey {
            root: shell_root.clone(),
            tab_id: "term-99".to_string(),
        };
        let (shell_tab, _shell_rx) = Tab::spawn(
            TabRole::Shell,
            &shell_harness_spec("/bin/bash"),
            "term-99".to_string(),
            shell_root,
            80,
            24,
        )
        .expect("a shell tab spawns");
        state.lock().unwrap().tabs.insert(shell_key, shell_tab);

        // The agent takes none of the sixteen, so fifteen more shells fit
        // beside the one shell tab...
        for n in 1..MAX_USER_TERMINALS {
            let created = handler(
                SessionSender::detached("s1"),
                req("term.create", json!({ "project_id": project_id })),
            );
            assert_eq!(created["ok"], true, "shell {n} of the cap: {created:?}");
        }
        // ...and the sixteenth does not: the shell tab is one of them.
        let over = handler(
            SessionSender::detached("s1"),
            req("term.create", json!({ "project_id": project_id })),
        );
        assert_eq!(over["ok"], false, "the shell tab is one of the sixteen");
        assert!(
            over["error"]
                .as_str()
                .unwrap_or_default()
                .contains("terminal limit reached"),
            "{over:?}"
        );
    }

    /// An agent tab is a MANAGED agent: the spec it spawns from carries Build's
    /// `done` MCP server and the owner id that routes reports back through the
    /// owner lookup. The socket lives inside the harness builder's closure, so
    /// `agent_harness_spec` is the only way the app layer can reach it — and a
    /// spec that dropped the config or the owner would open an agent Build
    /// cannot talk to, in a tab that looks entirely healthy.
    #[test]
    fn agent_harness_spec_carries_the_done_mcp_server_and_the_owner_id() {
        // claude's pre-trust writes a registry; keep it off the developer's own.
        let config_dir = tempfile::tempdir().unwrap();
        std::env::set_var("CLAUDE_CONFIG_DIR", config_dir.path());
        let orch = Orchestrator::new(
            "/repo",
            "/repo/.worktrees",
            build_agent(false, "/tmp/build mcp.sock".into()),
            Templates::default(),
        );
        let cwd = std::path::Path::new("/repo/.worktrees/wt-1");
        let claude = ModelChoice {
            provider: AgentProvider::Claude,
            model: None,
            effort: None,
        };
        let codex = ModelChoice {
            provider: AgentProvider::Codex,
            model: None,
            effort: None,
        };

        let spec = orch.agent_harness_spec("run-42", cwd, &claude, false);
        assert_eq!(spec.binary, "claude");
        let args = spec.args.join(" ");
        assert!(
            args.contains("--mcp-config .build/mcp.json --strict-mcp-config"),
            "{args}"
        );
        assert!(!args.contains("--continue"), "{args}");
        assert!(
            spec.env
                .iter()
                .any(|(key, value)| key == "BRIDGE_MCP_SOCKET" && value == "/tmp/build mcp.sock"),
            "{:?}",
            spec.env
        );
        // A replaced tab picks its own conversation back up.
        let resumed = orch.agent_harness_spec("run-42", cwd, &claude, true);
        assert!(resumed.args.join(" ").contains("--continue"));

        let spec = orch.agent_harness_spec("run-42", cwd, &codex, false);
        assert_eq!(spec.binary, "codex");
        let args = spec.args.join(" ");
        assert!(
            args.contains(r#"mcp_servers.build.args=["mcp","--task","run-42"]"#),
            "{args}"
        );
        assert!(
            args.contains(r#"mcp_servers.build.env.BRIDGE_MCP_SOCKET="/tmp/build mcp.sock""#),
            "{args}"
        );
        assert!(
            args.contains(r#"projects."/repo/.worktrees/wt-1".trust_level="trusted""#),
            "{args}"
        );
        assert!(!args.ends_with("resume --last"), "{args}");
        let resumed = orch.agent_harness_spec("run-42", cwd, &codex, true);
        assert!(resumed.args.join(" ").ends_with("resume --last"));

        std::env::remove_var("CLAUDE_CONFIG_DIR");
    }

    /// The tab spawns the spec the orchestrator built FOR IT: the run as
    /// `owner_id`, the canonical worktree as cwd, and continuation decided by
    /// the transcript probe — a Build-owned tab replaced after a crash should
    /// always pick its own conversation back up. The empty prompt is the
    /// contract too: a turn never rides in argv, it travels through the PTY.
    #[tokio::test]
    async fn an_agent_tab_spawns_the_harness_the_orchestrator_built() {
        let (dir, repo) = init_repo();
        let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-wired");
        let specs_built: Arc<Mutex<Vec<SpawnOptions>>> = Arc::new(Mutex::new(Vec::new()));
        {
            let recorder = Arc::clone(&specs_built);
            let agent = Agent::WarmBuilder(Arc::new(
                move |prompt: &str, _choice: &ModelChoice, options: &SpawnOptions| {
                    assert!(prompt.is_empty(), "a turn never rides in argv: {prompt:?}");
                    recorder.lock().unwrap().push(options.clone());
                    HarnessSpec::new("sh").arg("-c").arg(
                        "printf 'SPEC-FROM-THE-ORCHESTRATOR'; printf '\\033[?2004h'; cat >/dev/null",
                    )
                },
            ));
            let mut s = state.lock().unwrap();
            let worktrees = s.worktrees_root.clone();
            s.transcript_probe = Arc::new(|_, _| true);
            s.projects[0].orch =
                Orchestrator::new(repo.clone(), worktrees, agent, Templates::default());
        }

        ensure_agent_tab(&state, &root, "run-wired", &ModelChoice::default())
            .expect("the agent spawns");

        let built = specs_built.lock().unwrap().clone();
        assert_eq!(built.len(), 1, "one spawn, one spec: {built:?}");
        assert_eq!(
            built[0].owner_id, "run-wired",
            "`done` routes back by owner id"
        );
        assert_eq!(
            built[0].cwd,
            AppState::canonical_root(&root),
            "the spec is built for the canonical root"
        );
        assert!(
            built[0].continue_session,
            "a replaced tab picks its own transcript back up"
        );
        let screen = wait_for_agent_screen(&state, &root, "SPEC-FROM-THE-ORCHESTRATOR").await;
        assert!(
            screen.contains("SPEC-FROM-THE-ORCHESTRATOR"),
            "the tab runs the orchestrator's spec: {screen:?}"
        );
    }

    /// The registry key is the CANONICAL worktree path, so the same worktree
    /// reaching the daemon by a different spelling — a run scope hands back
    /// `worktrees_root/<name>` uncanonicalized while an external worktree is
    /// already canonical, and on macOS `/tmp` is `/private/tmp` — is one tab,
    /// not two agents in one directory.
    #[tokio::test]
    async fn the_agent_tab_key_survives_the_same_root_by_another_path() {
        let (dir, repo) = init_repo();
        let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-alias");
        let alias = dir.path().join("alias-root");
        std::os::unix::fs::symlink(&root, &alias).unwrap();

        let (direct_id, direct) =
            ensure_agent_tab(&state, &root, "run-alias", &ModelChoice::default()).unwrap();
        let (aliased_id, aliased) =
            ensure_agent_tab(&state, &alias, "run-alias", &ModelChoice::default()).unwrap();

        assert_eq!(direct, Spawned::Fresh);
        assert_eq!(aliased, Spawned::Warm, "the alias finds the same tab");
        assert_eq!(direct_id, aliased_id);
        assert_eq!(state.lock().unwrap().tabs.len(), 1);
    }

    /// One registry, one detach loop: a closed relay session must come off
    /// EVERY tab's screen — the agent's as much as a shell's. A pump still
    /// encrypting output into a session the relay has dropped fails silently
    /// and shows up only as CPU.
    #[tokio::test]
    async fn a_close_frame_detaches_the_sessions_terminal_sender() {
        let (dir, repo) = init_repo();
        let (state, handler, root) = agent_tab_fixture(&repo, dir.path(), "run-detach");
        let project_id = state.lock().unwrap().projects[0].id.clone();
        let (agent_wire_id, _) =
            ensure_agent_tab(&state, &root, "run-detach", &ModelChoice::default()).unwrap();
        handler(
            SessionSender::detached("s-live"),
            req("term.create", json!({ "project_id": project_id })),
        );
        for session_id in ["s-live", "s-dead"] {
            handler(
                SessionSender::detached(session_id),
                req("term.attach", json!({ "term_id": "term-1" })),
            );
            handler(
                SessionSender::detached(session_id),
                req("term.attach", json!({ "term_id": agent_wire_id.clone() })),
            );
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
        let attached_to = |wire_id: &str| -> Vec<String> {
            s.tabs
                .values()
                .find(|tab| tab.wire_id() == wire_id)
                .expect("the tab is still registered")
                .screen
                .attached
                .iter()
                .map(|snd| snd.session_id().to_string())
                .collect()
        };
        assert_eq!(
            attached_to("term-1"),
            vec!["s-live".to_string()],
            "only the closed session's sender is dropped from a shell"
        );
        assert_eq!(
            attached_to(&agent_wire_id),
            vec!["s-live".to_string()],
            "…and from the agent tab too"
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
        let providers = res["result"]["providers"].as_array().unwrap();
        let codex = providers.iter().find(|p| p["id"] == "codex").unwrap();
        assert!(codex["models"]
            .as_array()
            .unwrap()
            .iter()
            .any(|model| model["id"] == "gpt-5.6-sol"));
        assert!(codex["efforts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e == "ultra"));
    }

    /// Build mints a fresh worktree per run, and an interactive harness gates a
    /// directory it has not seen behind a workspace-trust dialog. That dialog
    /// owns the keyboard, so the injected prompt lands in it and the trailing
    /// submit key answers it — the agent receives nothing and the run parks
    /// until the idle sweep demotes it. Codex takes the grant as a per-invocation
    /// `--config`, so nothing outside this spawn is touched.
    #[test]
    fn codex_argv_pre_trusts_the_worktree_it_will_run_in() {
        let Agent::WarmBuilder(build) = build_agent(false, "/tmp/m.sock".into()) else {
            panic!("real agent should be a provider-aware warm TUI");
        };
        let choice = ModelChoice {
            provider: AgentProvider::Codex,
            ..ModelChoice::default()
        };
        let spec = build(
            "do the thing",
            &choice,
            &SpawnOptions {
                cwd: std::path::PathBuf::from("/tmp/build worktrees/run-9"),
                ..SpawnOptions::default()
            },
        );
        let args = spec.args.join(" ");
        assert!(
            args.contains(r#"projects."/tmp/build worktrees/run-9".trust_level="trusted""#),
            "{args}"
        );
    }

    /// Build injects a prompt and its submit key back-to-back. Codex otherwise
    /// classifies that rapid character stream as a paste burst and consumes the
    /// trailing Enter as a newline inside the paste, leaving the prompt visible
    /// but unsent. Build's PTY supports bracketed paste, so the fallback burst
    /// detector must be disabled for every Codex process it owns.
    #[test]
    fn codex_argv_disables_the_fallback_paste_burst_detector() {
        let Agent::WarmBuilder(build) = build_agent(false, "/tmp/m.sock".into()) else {
            panic!("real agent should be a provider-aware warm TUI");
        };
        let choice = ModelChoice {
            provider: AgentProvider::Codex,
            ..ModelChoice::default()
        };
        let spec = build("one line", &choice, &SpawnOptions::default());

        assert!(
            spec.args
                .windows(2)
                .any(|args| { args[0] == "--config" && args[1] == "disable_paste_burst=true" }),
            "Codex must not swallow Build's immediate submit key: {:?}",
            spec.args
        );
    }

    /// Claude keeps workspace trust in a shared registry that also holds every
    /// other project's state, so the grant must be additive and idempotent.
    #[test]
    fn claude_workspace_trust_is_added_without_disturbing_the_registry() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join(".claude.json");
        std::fs::write(
            &config,
            serde_json::to_vec(&json!({
                "firstStartTime": "2026-01-01",
                "projects": {
                    "/Users/someone/other": { "hasTrustDialogAccepted": true, "lastCost": 1.5 }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let worktree = std::path::Path::new("/tmp/build worktrees/run-9");
        record_claude_workspace_trust(&config, worktree).unwrap();
        record_claude_workspace_trust(&config, worktree).expect("idempotent");

        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(
            written["projects"]["/tmp/build worktrees/run-9"]["hasTrustDialogAccepted"],
            json!(true)
        );
        // Neighbouring state survives: this file is not Build's to own.
        assert_eq!(written["firstStartTime"], json!("2026-01-01"));
        assert_eq!(
            written["projects"]["/Users/someone/other"]["lastCost"],
            json!(1.5)
        );
        assert!(!dir.path().join(".claude.json.build-tmp").exists());
    }

    #[test]
    fn claude_workspace_trust_creates_a_registry_that_does_not_exist_yet() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("nested").join(".claude.json");
        record_claude_workspace_trust(&config, std::path::Path::new("/tmp/wt")).unwrap();
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(
            written["projects"]["/tmp/wt"]["hasTrustDialogAccepted"],
            json!(true)
        );
    }

    #[test]
    fn real_tui_argv_includes_the_selected_model_and_effort() {
        let Agent::WarmBuilder(build) = build_agent(false, "/tmp/m.sock".into()) else {
            panic!("real agent should be a provider-aware warm TUI");
        };
        let choice = ModelChoice {
            provider: AgentProvider::Claude,
            model: Some("claude-opus-4-8".into()),
            effort: Some("xhigh".into()),
        };
        let spec = build("do the thing", &choice, &SpawnOptions::default());
        let args = spec.args.join(" ");
        assert_eq!(spec.binary, "claude");
        assert!(!args
            .split_whitespace()
            .any(|arg| arg == "-p" || arg == "--print"));
        assert!(!args.contains("do the thing"), "{args}");
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
                ..SpawnOptions::default()
            },
        );
        let args = spec.args.join(" ");
        assert!(
            args.contains("--dangerously-skip-permissions --continue --model"),
            "{args}"
        );
    }

    #[test]
    fn codex_tui_argv_wires_done_mcp_and_resumes_by_cwd() {
        let Agent::WarmBuilder(build) = build_agent(false, "/tmp/build mcp.sock".into()) else {
            panic!("real agent should be a provider-aware warm TUI");
        };
        let choice = ModelChoice {
            provider: AgentProvider::Codex,
            model: Some("gpt-5.6-sol".into()),
            effort: Some("ultra".into()),
        };
        let options = SpawnOptions {
            continue_session: false,
            owner_id: "run-7".into(),
            ..SpawnOptions::default()
        };
        let spec = build("do the thing", &choice, &options);
        assert_eq!(spec.binary, "codex");
        let args = spec.args.join(" ");
        assert!(!args.contains("exec"), "{args}");
        assert!(
            args.contains("--dangerously-bypass-approvals-and-sandbox"),
            "{args}"
        );
        assert!(args.contains("--model gpt-5.6-sol"), "{args}");
        assert!(args.contains("model_reasoning_effort=\"ultra\""), "{args}");
        assert!(
            args.contains("mcp_servers.build.args=[\"mcp\",\"--task\",\"run-7\"]"),
            "{args}"
        );
        assert!(
            args.contains("mcp_servers.build.env.BRIDGE_MCP_SOCKET=\"/tmp/build mcp.sock\""),
            "{args}"
        );
        assert!(!args.contains("do the thing"), "{args}");

        let resumed = build(
            "a follow-up",
            &choice,
            &SpawnOptions {
                continue_session: true,
                owner_id: "run-7".into(),
                ..SpawnOptions::default()
            },
        );
        assert!(resumed.args.join(" ").ends_with("resume --last"));
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
    fn codex_transcript_probe_reads_nested_session_metadata_by_cwd() {
        let root = tempfile::tempdir().unwrap();
        let cwd = root.path().join("repo");
        std::fs::create_dir_all(&cwd).unwrap();
        let dated = root.path().join("sessions/2026/07/22");
        std::fs::create_dir_all(&dated).unwrap();
        std::fs::write(
            dated.join("rollout.jsonl"),
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":{}}}}}\n{{}}\n",
                serde_json::to_string(&cwd.display().to_string()).unwrap()
            ),
        )
        .unwrap();

        assert!(codex_transcript_exists(&root.path().join("sessions"), &cwd));
        assert!(!codex_transcript_exists(
            &root.path().join("sessions"),
            &root.path().join("other")
        ));
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
        std::fs::write(repo.join("sound.mp3"), b"ID3audio").unwrap();
        std::fs::write(repo.join("clip.mp4"), b"media").unwrap();
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
        assert_eq!(read("sound.mp3")["result"]["mime"], "audio/mpeg");
        assert_eq!(read("clip.mp4")["result"]["mime"], "video/mp4");
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
        std::fs::write(repo.join("clip.mp4"), vec![b'm'; real_size]).unwrap();

        let res = state.handle(req(
            "fs.read",
            json!({ "project_id": project_id.clone(), "path": "big.bin" }),
        ));
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["size"], real_size as u64);
        assert_eq!(res["result"]["truncated"], true);
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(res["result"]["content_b64"].as_str().unwrap())
            .unwrap();
        assert_eq!(decoded.len(), FS_READ_MAX_BYTES as usize);

        let media = state.handle(req(
            "fs.read",
            json!({ "project_id": project_id, "path": "clip.mp4" }),
        ));
        assert_eq!(media["result"]["mime"], "video/mp4");
        assert_eq!(media["result"]["truncated"], false);
        let media_decoded = base64::engine::general_purpose::STANDARD
            .decode(media["result"]["content_b64"].as_str().unwrap())
            .unwrap();
        assert_eq!(media_decoded.len(), real_size);
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

    /// A QA daemon behind the shared `Arc` plus its frame handler — the entry
    /// point the relay uses, and the only one that delivers the agent turns a
    /// verb queues while it holds the state lock.
    fn shared_qa_state_and_handler(
        repo: &std::path::Path,
        dir: &std::path::Path,
    ) -> (Arc<Mutex<AppState>>, FrameHandler) {
        let mut app = qa_state(repo, dir);
        app.term_shell = "/bin/bash".into();
        let state = app.shared();
        let handler = AppState::handler(Arc::clone(&state));
        (state, handler)
    }

    /// One RPC over the frame handler.
    fn call(handler: &FrameHandler, method: &str, params: Value) -> Value {
        handler(SessionSender::detached("qa"), req(method, params))
    }

    /// [`planned_run_in_review`] over the frame handler — the entry point that
    /// actually delivers the turn each verb queues. A conversation only gains
    /// its session lineage when a turn is delivered COLD (a new agent process),
    /// so a test about what the thread carries has to go this way.
    fn planned_run_in_review_delivered(handler: &FrameHandler, goal: &str) -> (String, String) {
        let plan = call(handler, "plan.create", json!({ "goal": goal }));
        let plan_id = plan_id_of(&plan);
        for stage_id in ["first-half", "second-half"] {
            call(
                handler,
                "plan.stage_approve",
                json!({ "plan_id": plan_id, "stage_id": stage_id }),
            );
        }
        call(handler, "plan.approve", json!({ "plan_id": plan_id }));
        let run = call(handler, "run.create", json!({ "plan_id": plan_id }));
        let run_id = run_id_of(&run);
        let last_stage = call(
            handler,
            "run.stage_dispatch",
            json!({ "run_id": run_id, "stage_id": "second-half" }),
        );
        assert_eq!(last_stage["result"]["state"], "review", "{last_stage:?}");
        (plan_id, run_id)
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

    /// A run in review, minted the only way runs are minted now: author a plan
    /// with the scripted agent, approve it and both of its stage docs, implement
    /// stage one, then dispatch stage two (whose validation opens review).
    /// Returns `(plan_id, run_id)`. The goal-only "Quick task" dispatch this
    /// replaces is gone — see `run_create_refuses_a_goal_without_a_plan`.
    fn planned_run_in_review(state: &mut AppState, goal: &str) -> (String, String) {
        let plan = state.handle(req("plan.create", json!({ "goal": goal })));
        let plan_id = plan_id_of(&plan);
        for stage_id in ["first-half", "second-half"] {
            let approved = state.handle(req(
                "plan.stage_approve",
                json!({ "plan_id": plan_id, "stage_id": stage_id }),
            ));
            assert_eq!(approved["ok"], true, "{approved:?}");
        }
        let approved = state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
        assert_eq!(approved["ok"], true, "{approved:?}");
        let run = state.handle(req("run.create", json!({ "plan_id": plan_id })));
        assert_eq!(run["ok"], true, "{run:?}");
        let run_id = run_id_of(&run);
        let last_stage = state.handle(req(
            "run.stage_dispatch",
            json!({ "run_id": run_id, "stage_id": "second-half" }),
        ));
        assert_eq!(last_stage["result"]["state"], "review", "{last_stage:?}");
        (plan_id, run_id)
    }

    /// A plan-less run: adopt an external worktree. Adoption is the only
    /// remaining source of runs that implement no plan, so it stands in wherever
    /// a test just needs a live run with no plan behind it.
    fn adopted_run(
        state: &mut AppState,
        repo: &std::path::Path,
        dir: &std::path::Path,
        branch: &str,
    ) -> String {
        add_external_worktree(repo, dir, branch, branch);
        let project_id = state.projects[0].id.clone();
        let worktree_id = state
            .external_worktrees(&project_id, true)
            .unwrap()
            .into_iter()
            .find(|w| w.branch.as_deref() == Some(branch))
            .expect("the external worktree is discoverable")
            .id;
        let adopted = state.handle(req(
            "run.adopt",
            json!({ "project_id": project_id, "worktree_id": worktree_id }),
        ));
        assert_eq!(adopted["ok"], true, "{adopted:?}");
        run_id_of(&adopted)
    }

    #[test]
    fn plan_create_reaches_review_with_two_stages() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let res = state.handle(req("plan.create", json!({ "goal": "add a greeting" })));
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(res["result"]["state"], "plan_review");
        assert_eq!(res["result"]["docs_available"], true, "{res:?}");
        assert_eq!(res["result"]["stages"].as_array().unwrap().len(), 2);
        assert!(res["result"]["created_at"]
            .as_str()
            .is_some_and(|s| !s.is_empty()));
        let first_item = &res["result"]["thread"]["items"][0];
        assert_eq!(first_item["type"], "message", "{res:?}");
        assert_eq!(first_item["data"]["role"], "user", "{res:?}");
        assert_eq!(first_item["data"]["body"], "add a greeting", "{res:?}");
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
    fn plan_send_notes_accepts_a_conversation_message_for_multi_stage_plans() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let plan = state.handle(req("plan.create", json!({ "goal": "make it staged" })));
        let plan_id = plan_id_of(&plan);

        let revised = state.handle(req(
            "plan.send_notes",
            json!({
                "plan_id": plan_id,
                "messages": [{ "body": "Keep the second stage reversible.", "anchor": null }]
            }),
        ));

        assert_eq!(revised["ok"], true, "{revised:?}");
        assert_eq!(revised["result"]["state"], "plan_review", "{revised:?}");
        assert!(revised["result"]["thread"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["type"] == "message"
                && item["data"]["role"] == "user"
                && item["data"]["body"] == "Keep the second stage reversible."));
    }

    #[test]
    fn full_multi_stage_lifecycle_plan_then_run() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());

        let plan = state.handle(req(
            "plan.create",
            json!({
                "goal": "add a greeting",
                "provider": "codex",
                "model": "gpt-5.6-sol",
                "effort": "ultra"
            }),
        ));
        let plan_id = plan_id_of(&plan);
        assert_eq!(plan["result"]["provider"], "codex", "{plan:?}");
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
        assert_eq!(run["result"]["provider"], "codex", "{run:?}");
        assert_eq!(run["result"]["model"], "gpt-5.6-sol", "{run:?}");
        assert_eq!(run["result"]["effort"], "ultra", "{run:?}");
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

    /// Every run implements a plan. The goal-only dispatch ("Quick task") is
    /// gone: an ad-hoc coding session is now a `claude`/`codex` tab the human
    /// drives (`term.create`), not a task-lifecycle run nobody planned.
    #[test]
    fn run_create_refuses_a_goal_without_a_plan() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let res = state.handle(req("run.create", json!({ "goal": "quick change" })));
        assert_eq!(res["ok"], false, "{res:?}");
        assert!(
            res["error"].as_str().unwrap().contains("plan_id"),
            "{res:?}"
        );
        assert!(state.runs.is_empty(), "nothing was dispatched");
    }

    #[test]
    fn planned_run_builds_reviews_and_merges_with_a_cached_diffstat() {
        let (dir, repo, _origin) = init_repo_with_origin();
        let mut state = qa_state(&repo, dir.path());
        let (_, run_id) = planned_run_in_review(&mut state, "quick change");
        let worktree_path = state.runs[&run_id].worktree.path.clone();
        let branch = state.runs[&run_id].worktree.branch.clone();
        git_in_dir(&worktree_path, &["push", "-u", "origin", &branch]);
        std::fs::write(worktree_path.join("uncommitted.txt"), "one\ntwo\n").unwrap();
        let res = state.handle(req("run.get", json!({ "run_id": run_id })));
        assert_eq!(res["result"]["state"], "review", "{res:?}");

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
        assert_eq!(
            t["stat"]["comparison_ref"],
            format!("origin/{branch}"),
            "{t:?}"
        );
        assert_eq!(t["stat"]["ahead"], 0, "pushed branch must be level: {t:?}");
        assert_eq!(t["stat"]["behind"], 0, "{t:?}");
        assert_eq!(t["stat"]["uncommitted"]["files_changed"], 1, "{t:?}");
        assert_eq!(t["stat"]["uncommitted"]["insertions"], 2, "{t:?}");
        assert_eq!(t["stat"]["uncommitted"]["deletions"], 0, "{t:?}");
        // served from cache on the next poll (identical).
        let t2 = entry(&state.handle(req("board.list", json!({}))));
        assert_eq!(t["stat"], t2["stat"]);

        state.handle(req(
            "run.git_action",
            json!({ "run_id": run_id, "action": "commit" }),
        ));
        assert!(
            !repo.join("result-first-half.txt").exists(),
            "commit keeps, no merge"
        );

        let merged = state.handle(req(
            "run.git_action",
            json!({ "run_id": run_id, "action": "merge" }),
        ));
        assert_eq!(merged["result"]["state"], "merged");
        assert!(repo.join("result-first-half.txt").exists());
        let t3 = entry(&state.handle(req("board.list", json!({}))));
        assert!(t3["stat"].is_null(), "merged run has no worktree: {t3:?}");
    }

    #[test]
    fn run_stat_uses_the_checked_out_branch_upstream_after_a_rename() {
        let (dir, repo, _origin) = init_repo_with_origin();
        let mut state = qa_state(&repo, dir.path());
        let (_, run_id) = planned_run_in_review(&mut state, "renamed branch");
        let worktree_path = state.runs[&run_id].worktree.path.clone();
        let original_branch = state.runs[&run_id].worktree.branch.clone();
        git_in_dir(&worktree_path, &["push", "-u", "origin", &original_branch]);
        git_in_dir(
            &worktree_path,
            &["branch", "-m", "build/actually-checked-out"],
        );
        git_in_dir(
            &worktree_path,
            &["push", "-u", "origin", "build/actually-checked-out"],
        );

        let board = state.handle(req("board.list", json!({})));
        let run = board["result"]["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["run_id"] == run_id)
            .unwrap();
        assert_eq!(
            run["stat"]["comparison_ref"], "origin/build/actually-checked-out",
            "{run:?}"
        );
        assert_eq!(run["stat"]["ahead"], 0, "{run:?}");
        assert_eq!(run["stat"]["behind"], 0, "{run:?}");
    }

    #[test]
    fn issue_facade_preserves_plan_identity_and_board_compatibility() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let created = state.handle(req("issue.create", json!({ "goal": "canonical issue" })));
        assert_eq!(created["ok"], true, "{created:?}");
        let issue_id = created["result"]["issue_id"].as_str().unwrap().to_string();
        assert_eq!(created["result"]["plan_id"], issue_id);

        let issue = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
        let legacy = state.handle(req("plan.get", json!({ "plan_id": issue_id })));
        assert_eq!(issue["result"]["issue_id"], issue_id);
        assert_eq!(issue["result"]["plan_id"], issue_id);
        assert_eq!(legacy["result"]["issue_id"], issue_id);
        assert_eq!(issue["result"]["goal"], legacy["result"]["goal"]);

        let listed = state.handle(req("issue.list", json!({})));
        assert_eq!(listed["result"]["issues"].as_array().unwrap().len(), 1);
        let board = state.handle(req("board.list", json!({})));
        assert_eq!(board["result"]["issues"], board["result"]["plans"]);

        let stages = state.handle(req("issue.stages", json!({ "issue_id": issue_id })));
        assert_eq!(stages["result"]["issue_id"], issue_id);
        assert_eq!(stages["result"]["plan_id"], issue_id);
        assert_eq!(stages["result"]["stages"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn disappearing_issue_worktree_invalidates_local_stage_completion_but_preserves_boundaries() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (issue_id, run_id) = planned_run_in_review(&mut state, "lost local worktree");
        let before = state.runs[&run_id].stages[0].clone();
        assert!(before.completion_sha.is_some());
        let worktree = state.runs[&run_id].worktree.path.clone();
        assert!(Command::new("git")
            .args([
                "worktree",
                "remove",
                "--force",
                "--",
                worktree.to_str().unwrap()
            ])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());

        state.handle(req("board.list", json!({})));
        let stages = state.handle(req("issue.stages", json!({ "issue_id": issue_id })));
        let first = &stages["result"]["stages"][0];
        assert_eq!(first["execution"], "incomplete", "{stages:?}");
        assert_eq!(first["start_sha"], before.start_sha.unwrap());
        assert_eq!(first["completion_sha"], before.completion_sha.unwrap());
        assert!(first["invalidation_reason"]
            .as_str()
            .unwrap()
            .contains("worktree"));
    }

    #[test]
    fn abandoning_an_unpublished_issue_worktree_marks_completed_stages_incomplete() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (issue_id, run_id) = planned_run_in_review(&mut state, "abandon local lineage");
        let abandoned = state.handle(req("run.abandon", json!({ "run_id": run_id })));
        assert_eq!(abandoned["ok"], true, "{abandoned:?}");
        let stages = state.handle(req("issue.stages", json!({ "issue_id": issue_id })));
        assert!(
            stages["result"]["stages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|stage| stage["execution"] == "incomplete"
                    && stage["invalidation_reason"].as_str().is_some()),
            "{stages:?}"
        );
        let issue = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
        assert!(issue["result"]["thread"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["type"] == "event" && item["data"]["event"] == "worktree_deleted"));
    }

    #[test]
    fn disappearing_worktree_keeps_pushed_stage_commits_complete() {
        let (dir, repo, _origin) = init_repo_with_origin();
        let mut state = qa_state(&repo, dir.path());
        let (issue_id, run_id) = planned_run_in_review(&mut state, "published worktree");
        let pushed = state.handle(req(
            "run.git_action",
            json!({ "run_id": run_id, "action": "push" }),
        ));
        assert_eq!(pushed["ok"], true, "{pushed:?}");
        let issue = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
        assert!(
            issue["result"]["thread"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["type"] == "event"
                    && item["data"]["event"] == "pushed"
                    && item["data"]["links"].as_array().is_some_and(|links| links
                        .iter()
                        .any(|link| link["kind"] == "run" && link["run_id"] == run_id))),
            "{issue:?}"
        );
        let before_delete = state.handle(req("issue.stages", json!({ "issue_id": issue_id })));
        assert!(
            before_delete["result"]["stages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|stage| stage["publication"] == "pushed"),
            "{before_delete:?}"
        );
        let worktree = state.runs[&run_id].worktree.path.clone();
        assert!(Command::new("git")
            .args([
                "worktree",
                "remove",
                "--force",
                "--",
                worktree.to_str().unwrap()
            ])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());

        state.handle(req("board.list", json!({})));
        let stages = state.handle(req("issue.stages", json!({ "issue_id": issue_id })));
        assert!(
            stages["result"]["stages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|stage| stage["execution"] == "complete"
                    && stage["publication"] == "pushed"
                    && stage["invalidation_reason"].is_null()),
            "{stages:?}"
        );
        let stable = state.handle(req(
            "issue.stage_diff",
            json!({ "issue_id": issue_id, "stage_id": "first-half" }),
        ));
        assert_eq!(stable["ok"], true, "{stable:?}");
        assert_eq!(stable["result"]["status"], "available", "{stable:?}");
        assert!(stable["result"]["patch"]
            .as_str()
            .unwrap()
            .contains("result-first-half.txt"));
    }

    #[test]
    fn issue_implement_all_runs_sequentially_and_exposes_stable_stage_diff() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let issue = state.handle(req("issue.create", json!({ "goal": "canonical all" })));
        let issue_id = issue["result"]["issue_id"].as_str().unwrap().to_string();
        for stage_id in ["first-half", "second-half"] {
            state.handle(req(
                "issue.stage_approve",
                json!({ "issue_id": issue_id, "stage_id": stage_id }),
            ));
        }
        state.handle(req("issue.approve", json!({ "issue_id": issue_id })));
        let implemented = state.handle(req("issue.implement_all", json!({ "issue_id": issue_id })));
        assert_eq!(implemented["ok"], true, "{implemented:?}");
        assert_eq!(
            implemented["result"]["current_implementation"]["state"],
            "review"
        );
        assert_eq!(
            implemented["result"]["implementation_lineage"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let diff = state.handle(req(
            "issue.stage_diff",
            json!({ "issue_id": issue_id, "stage_id": "first-half" }),
        ));
        assert_eq!(diff["result"]["issue_id"], issue_id);
        assert_eq!(diff["result"]["status"], "available");
        assert!(diff["result"]["patch"]
            .as_str()
            .unwrap()
            .contains("result-first-half.txt"));
        assert!(!diff["result"]["patch"]
            .as_str()
            .unwrap()
            .contains("result-second-half.txt"));
    }

    #[test]
    fn implement_all_persists_intent_before_stage_one_approval_and_resumes() {
        let (dir, repo) = init_repo();
        let issue_id;
        {
            let mut state = qa_state(&repo, dir.path());
            let issue = state.handle(req("issue.create", json!({ "goal": "durable all" })));
            issue_id = issue["result"]["issue_id"].as_str().unwrap().to_string();
            state.handle(req("issue.approve", json!({ "issue_id": issue_id })));

            let waiting = state.handle(req("issue.implement_all", json!({ "issue_id": issue_id })));
            assert_eq!(waiting["ok"], true, "{waiting:?}");
            assert!(waiting["result"]["current_implementation"].is_null());
            assert_eq!(waiting["result"]["implementation_intent"], "all");
            assert_eq!(
                waiting["result"]["implementation_activity"],
                json!({ "waiting_approval": "first-half" })
            );
        }

        // The intent is durable even though no worktree/run existed yet.
        let mut state = qa_state(&repo, dir.path());
        let restored = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
        assert_eq!(restored["result"]["implementation_intent"], "all");
        assert_eq!(
            restored["result"]["implementation_activity"],
            json!({ "waiting_approval": "first-half" })
        );

        let first = state.handle(req(
            "issue.stage_approve",
            json!({ "issue_id": issue_id, "stage_id": "first-half" }),
        ));
        assert_eq!(first["ok"], true, "{first:?}");
        let waiting = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
        assert_eq!(
            waiting["result"]["current_implementation"]["state"],
            "stage_gate"
        );
        assert_eq!(
            waiting["result"]["implementation_activity"],
            json!({ "waiting_approval": "second-half" })
        );

        let second = state.handle(req(
            "issue.stage_approve",
            json!({ "issue_id": issue_id, "stage_id": "second-half" }),
        ));
        assert_eq!(second["ok"], true, "{second:?}");
        let completed = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
        assert_eq!(
            completed["result"]["current_implementation"]["state"],
            "review"
        );
        assert_eq!(completed["result"]["implementation_intent"], "none");
        assert_eq!(completed["result"]["implementation_activity"], "idle");
    }

    #[test]
    fn implement_stage_recreates_the_original_missing_issue_worktree() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let issue = state.handle(req("issue.create", json!({ "goal": "reuse branch" })));
        let issue_id = issue["result"]["issue_id"].as_str().unwrap().to_string();
        state.handle(req(
            "issue.stage_approve",
            json!({ "issue_id": issue_id, "stage_id": "first-half" }),
        ));
        state.handle(req("issue.approve", json!({ "issue_id": issue_id })));
        let run = state.handle(req("run.create", json!({ "plan_id": issue_id })));
        let run_id = run_id_of(&run);
        assert_eq!(run["result"]["state"], "stage_gate", "{run:?}");
        state.handle(req(
            "issue.stage_approve",
            json!({ "issue_id": issue_id, "stage_id": "second-half" }),
        ));
        let worktree = state.runs[&run_id].worktree.path.clone();
        assert!(Command::new("git")
            .args([
                "worktree",
                "remove",
                "--force",
                "--",
                worktree.to_str().unwrap()
            ])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());

        let implemented = state.handle(req(
            "issue.implement_stage",
            json!({ "issue_id": issue_id, "stage_id": "second-half" }),
        ));
        assert_eq!(implemented["ok"], true, "{implemented:?}");
        assert!(worktree.exists());
        assert_eq!(
            implemented["result"]["current_implementation"]["state"],
            "review"
        );
        assert!(implemented["result"]["thread"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["type"] == "event" && item["data"]["event"] == "worktree_recovered"));
    }

    #[test]
    fn implement_all_resumes_when_the_waiting_stage_plan_is_approved() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let issue = state.handle(req("issue.create", json!({ "goal": "resume all" })));
        let issue_id = issue["result"]["issue_id"].as_str().unwrap().to_string();
        state.handle(req(
            "issue.stage_approve",
            json!({ "issue_id": issue_id, "stage_id": "first-half" }),
        ));
        state.handle(req("issue.approve", json!({ "issue_id": issue_id })));
        let run = state.handle(req("run.create", json!({ "plan_id": issue_id })));
        let run_id = run_id_of(&run);
        assert_eq!(run["result"]["state"], "stage_gate", "{run:?}");

        let waiting = state.handle(req(
            "run.set_auto_advance",
            json!({ "run_id": run_id, "enabled": true }),
        ));
        assert_eq!(waiting["result"]["state"], "stage_gate", "{waiting:?}");
        assert_eq!(waiting["result"]["auto_advance"], true);

        let approved = state.handle(req(
            "issue.stage_approve",
            json!({ "issue_id": issue_id, "stage_id": "second-half" }),
        ));
        assert_eq!(approved["ok"], true, "{approved:?}");
        let implementation = state.handle(req("run.get", json!({ "run_id": run_id })));
        assert_eq!(
            implementation["result"]["state"], "review",
            "{implementation:?}"
        );
        assert!(implementation["result"]["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| stage["state"] == "validated_passed"));
    }

    #[test]
    fn board_list_carries_plans_runs_and_ride_alongs() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        planned_run_in_review(&mut state, "a plan");
        let board = state.handle(req("board.list", json!({})));
        let r = &board["result"];
        assert_eq!(r["plans"].as_array().unwrap().len(), 1);
        assert_eq!(r["runs"].as_array().unwrap().len(), 1);
        assert!(r["external_worktrees"].is_array());
        assert!(r["primary_changes"].is_array());
    }

    #[test]
    fn board_list_views_carry_state_changed_at_and_run_worktree_path() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (_, run_id) = planned_run_in_review(&mut state, "progress facts");
        state.handle(req("plan.create", json!({ "goal": "a plan" })));

        let board = state.handle(req("board.list", json!({})));
        let run_entry = board["result"]["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["run_id"] == json!(run_id.clone()))
            .unwrap()
            .clone();
        assert!(
            run_entry["state_changed_at"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "{run_entry:?}"
        );
        let worktree_path = run_entry["worktree_path"].as_str().unwrap();
        assert!(
            std::path::Path::new(worktree_path).exists(),
            "{run_entry:?}"
        );

        let plan_entry = &board["result"]["plans"].as_array().unwrap()[0];
        assert!(
            plan_entry["state_changed_at"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "{plan_entry:?}"
        );
    }

    #[tokio::test]
    async fn list_surfaces_carry_thread_digests_without_message_bodies() {
        let (dir, repo) = init_repo();
        let (_state, handler) = shared_qa_state_and_handler(&repo, dir.path());
        let plan = call(
            &handler,
            "plan.create",
            json!({ "goal": "digest the board" }),
        );
        let plan_id = plan_id_of(&plan);
        let (_, run_id) = planned_run_in_review_delivered(&handler, "a run to digest");
        // Seed a real user message into each conversation so the assertions
        // below prove bodies are omitted, not merely absent.
        call(
            &handler,
            "plan.send_notes",
            json!({
                "plan_id": plan_id,
                "messages": [{ "body": "plan-only-body-marker", "anchor": null }]
            }),
        );
        call(
            &handler,
            "run.request_changes",
            json!({
                "run_id": run_id,
                "messages": [{ "body": "run-only-body-marker", "anchor": null }]
            }),
        );

        let board = call(&handler, "board.list", json!({}));
        for thread in [
            &board["result"]["plans"][0]["thread"],
            &board["result"]["runs"][0]["thread"],
        ] {
            assert!(thread.get("items").is_none(), "{thread:?}");
            assert!(thread["item_count"].as_u64().unwrap() > 0, "{thread:?}");
            assert!(thread["last_sequence"].as_u64().unwrap() > 0, "{thread:?}");
            assert!(thread["last_event"]["event"].is_string(), "{thread:?}");
        }
        let serialized_board = board.to_string();
        assert!(!serialized_board.contains("plan-only-body-marker"));
        assert!(!serialized_board.contains("run-only-body-marker"));

        let listed = call(&handler, "plan.list", json!({}));
        assert!(
            listed["result"]["plans"][0]["thread"]
                .get("items")
                .is_none(),
            "{listed:?}"
        );

        // The detail surfaces must not regress: full threads, bodies intact.
        let plan_view = call(&handler, "plan.get", json!({ "plan_id": plan_id }));
        assert!(plan_view.to_string().contains("plan-only-body-marker"));
        let run_view = call(&handler, "run.get", json!({ "run_id": run_id }));
        assert!(run_view.to_string().contains("run-only-body-marker"));
    }

    #[tokio::test]
    async fn detail_gets_with_a_cursor_ship_only_newer_thread_items() {
        let (dir, repo) = init_repo();
        let (_state, handler) = shared_qa_state_and_handler(&repo, dir.path());
        let (_, run_id) = planned_run_in_review_delivered(&handler, "cursor the thread");
        call(
            &handler,
            "run.request_changes",
            json!({
                "run_id": run_id,
                "messages": [{ "body": "tighten the loop", "anchor": null }]
            }),
        );

        // Without a cursor the wire is exactly as before: every item, no totals.
        let full = call(&handler, "run.get", json!({ "run_id": run_id }));
        let full_items = full["result"]["thread"]["items"].as_array().unwrap();
        assert!(full_items.len() >= 2, "{full:?}");
        assert!(full["result"]["thread"].get("thread_total").is_none());
        let total = full_items.len() as u64;
        let last_sequence = full_items.last().unwrap()["data"]["sequence"]
            .as_u64()
            .unwrap();
        let cursor = full_items[full_items.len() - 2]["data"]["sequence"]
            .as_u64()
            .unwrap();

        let delta = call(
            &handler,
            "run.get",
            json!({ "run_id": run_id, "thread_after_sequence": cursor }),
        );
        let delta_thread = &delta["result"]["thread"];
        let delta_items = delta_thread["items"].as_array().unwrap();
        assert!(!delta_items.is_empty(), "{delta:?}");
        assert!(delta_items
            .iter()
            .all(|item| item["data"]["sequence"].as_u64().unwrap() > cursor));
        assert_eq!(delta_thread["thread_total"], total);
        assert_eq!(delta_thread["thread_last_sequence"], last_sequence);

        // A cursor past the end is an empty delta, never an error.
        let drained = call(
            &handler,
            "run.get",
            json!({ "run_id": run_id, "thread_after_sequence": last_sequence + 100 }),
        );
        assert_eq!(drained["ok"], true, "{drained:?}");
        assert_eq!(
            drained["result"]["thread"]["items"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert_eq!(drained["result"]["thread"]["thread_total"], total);

        // A garbage cursor is treated as absent: the full backward-compatible thread.
        let garbage = call(
            &handler,
            "run.get",
            json!({ "run_id": run_id, "thread_after_sequence": "junk" }),
        );
        assert_eq!(garbage["ok"], true, "{garbage:?}");
        assert_eq!(
            garbage["result"]["thread"]["items"]
                .as_array()
                .unwrap()
                .len(),
            full_items.len()
        );
        assert!(garbage["result"]["thread"].get("thread_total").is_none());
    }

    #[tokio::test]
    async fn plan_get_honors_the_thread_cursor() {
        let (dir, repo) = init_repo();
        let (_state, handler) = shared_qa_state_and_handler(&repo, dir.path());
        let plan = call(
            &handler,
            "plan.create",
            json!({ "goal": "cursor the plan" }),
        );
        let plan_id = plan_id_of(&plan);

        let full = call(&handler, "plan.get", json!({ "plan_id": plan_id }));
        let full_items = full["result"]["thread"]["items"].as_array().unwrap();
        assert!(!full_items.is_empty(), "{full:?}");
        let last_sequence = full_items.last().unwrap()["data"]["sequence"]
            .as_u64()
            .unwrap();

        let delta = call(
            &handler,
            "plan.get",
            json!({ "plan_id": plan_id, "thread_after_sequence": last_sequence }),
        );
        let delta_thread = &delta["result"]["thread"];
        assert_eq!(delta_thread["items"].as_array().unwrap().len(), 0);
        assert_eq!(delta_thread["thread_total"], full_items.len() as u64);
        assert_eq!(delta_thread["thread_last_sequence"], last_sequence);
    }

    #[test]
    fn state_changed_at_moves_on_transitions_but_not_same_state_mutations() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (_, run_id) = planned_run_in_review(&mut state, "changed change");
        let entry = |res: &Value| {
            res["result"]["runs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["run_id"] == json!(run_id.clone()))
                .unwrap()
                .clone()
        };
        let at_review = entry(&state.handle(req("board.list", json!({}))));
        assert_eq!(at_review["state"], "review", "{at_review:?}");
        let review_stamp = at_review["state_changed_at"].as_str().unwrap().to_string();
        let review_updated = at_review["updated_at"].as_str().unwrap().to_string();

        // A same-state git mutation advances updated_at but never the stamp.
        // Every built stage commits its own work, so dirty the worktree first.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let worktree = state.runs[&run_id].worktree.path.clone();
        std::fs::write(worktree.join("scratch.txt"), "reviewer edit\n").unwrap();
        let staged = state.handle(req(
            "git.stage",
            json!({ "run_id": run_id, "paths": ["scratch.txt"] }),
        ));
        assert_eq!(staged["ok"], true, "{staged:?}");
        let committed = state.handle(req(
            "git.commit",
            json!({ "run_id": run_id, "message": "keep" }),
        ));
        assert_eq!(committed["ok"], true, "{committed:?}");
        let after_commit = entry(&state.handle(req("board.list", json!({}))));
        assert_eq!(after_commit["state"], "review", "{after_commit:?}");
        assert_eq!(after_commit["state_changed_at"], json!(review_stamp));
        assert_ne!(after_commit["updated_at"], json!(review_updated));

        // Merging is a real transition: the stamp moves with it.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let merged = state.handle(req(
            "run.git_action",
            json!({ "run_id": run_id, "action": "merge" }),
        ));
        assert_eq!(merged["result"]["state"], "merged", "{merged:?}");
        assert_ne!(merged["result"]["state_changed_at"], json!(review_stamp));
    }

    #[test]
    fn state_changed_at_survives_a_daemon_restart() {
        let (dir, repo) = init_repo();
        let run_id;
        let stamp;
        {
            let mut state = qa_state(&repo, dir.path());
            run_id = planned_run_in_review(&mut state, "restartable").1;
            let got = state.handle(req("run.get", json!({ "run_id": run_id })));
            assert_eq!(got["result"]["state"], "review", "{got:?}");
            stamp = got["result"]["state_changed_at"]
                .as_str()
                .unwrap()
                .to_string();
        } // daemon dies

        let mut reloaded = qa_state(&repo, dir.path());
        let got = reloaded.handle(req("run.get", json!({ "run_id": run_id })));
        assert_eq!(got["result"]["state"], "review", "{got:?}");
        assert_eq!(got["result"]["state_changed_at"], json!(stamp));

        // A restored entity's first same-state mutation must not false-stamp:
        // the last-observed state is seeded from the record on boot. A commit
        // runs the full mutation tail but keeps the run in review.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let committed = reloaded.handle(req(
            "run.git_action",
            json!({ "run_id": run_id, "action": "commit" }),
        ));
        assert_eq!(committed["result"]["state"], "review", "{committed:?}");
        assert_eq!(committed["result"]["state_changed_at"], json!(stamp));
    }

    #[test]
    fn deleting_a_run_worktree_removes_it_from_the_board_and_plan_docs_survive() {
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

            // The next board poll retires the run to internal archived history.
            let board = state.handle(req("board.list", json!({})));
            assert!(board["result"]["runs"]
                .as_array()
                .unwrap()
                .iter()
                .all(|run| run["run_id"] != run_id));
            assert_eq!(state.runs[&run_id].run.state, RunState::Archived);

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
        assert_eq!(deleted["result"]["retained_as_issue_lineage"], true);
        assert!(reloaded.runs.contains_key(&run_id));
    }

    #[test]
    fn boot_recreates_a_missing_issue_worktree_from_its_original_branch() {
        let (dir, repo) = init_repo();
        let issue_id;
        let run_id;
        let worktree;
        {
            let mut state = qa_state(&repo, dir.path());
            (issue_id, run_id) = planned_run_in_review(&mut state, "restore lineage");
            worktree = state.runs[&run_id].worktree.path.clone();
            assert!(Command::new("git")
                .args([
                    "worktree",
                    "remove",
                    "--force",
                    "--",
                    worktree.to_str().unwrap()
                ])
                .current_dir(&repo)
                .status()
                .unwrap()
                .success());
            assert!(!worktree.exists());
        }

        let mut restored = qa_state(&repo, dir.path());
        assert!(worktree.exists(), "the original checkout path is recreated");
        let run = restored.handle(req("run.get", json!({ "run_id": run_id })));
        assert_eq!(run["result"]["state"], "review", "{run:?}");
        let issue = restored.handle(req("issue.get", json!({ "issue_id": issue_id })));
        assert!(
            issue["result"]["thread"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["type"] == "event"
                    && item["data"]["event"] == "worktree_recovered"
                    && item["data"]["links"][0]["run_id"] == run_id),
            "{issue:?}"
        );
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
    fn mid_run_stage_send_notes_revises_the_plan_doc_from_the_stage_gate() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let plan = state.handle(req("plan.create", json!({ "goal": "revise me mid-run" })));
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

        // Comment on the upcoming stage, then send the notes through the run.
        state.handle(req(
            "plan.comment_add",
            json!({ "plan_id": plan_id, "stage_id": "second-half", "body": "tighten this" }),
        ));
        let revised = state.handle(req(
            "run.stage_send_notes",
            json!({ "run_id": run_id, "stage_id": "second-half" }),
        ));
        assert_eq!(revised["ok"], true, "{revised:?}");
        assert_eq!(revised["result"]["state"], "stage_gate", "{revised:?}");

        // The doc revision landed in the canonical store and the approval
        // is stale again (planned), with the comment addressed.
        let doc = state.handle(req(
            "plan.stage_doc",
            json!({ "plan_id": plan_id, "stage_id": "second-half" }),
        ));
        assert!(
            doc["result"]["contents"]
                .as_str()
                .unwrap()
                .contains("(revised mid-run)"),
            "{doc:?}"
        );
        let stages = state.handle(req("plan.stages", json!({ "plan_id": plan_id })));
        let second = stages["result"]["stages"][1].clone();
        assert_eq!(second["state"], "planned", "{second:?}");
        assert_eq!(second["open_comments"], 0, "{second:?}");

        // An adopted run implements no plan, so it has nothing to revise.
        let adopted_id = adopted_run(&mut state, &repo, dir.path(), "adopted-branch");
        let refused = state.handle(req(
            "run.stage_send_notes",
            json!({ "run_id": adopted_id, "stage_id": "second-half" }),
        ));
        assert!(
            refused["error"]
                .as_str()
                .unwrap()
                .contains("implements no plan"),
            "{refused:?}"
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
        let first = &armed["result"]["stages"][0];
        assert!(first["start_sha"].is_string(), "{first:?}");
        assert!(first["built_sha"].is_string(), "{first:?}");
        assert_eq!(first["completion_sha"], first["built_sha"], "{first:?}");
        assert_eq!(first["publication"], "local", "{first:?}");

        let stable = state.handle(req(
            "run.stage_diff",
            json!({ "run_id": run_id, "stage_id": "first-half" }),
        ));
        assert_eq!(stable["ok"], true, "{stable:?}");
        assert_eq!(stable["result"]["status"], "available");
        assert_eq!(stable["result"]["start_sha"], first["start_sha"]);
        assert_eq!(stable["result"]["completion_sha"], first["completion_sha"]);
        assert!(stable["result"]["patch"]
            .as_str()
            .unwrap()
            .contains("result-first-half.txt"));
        assert!(!stable["result"]["patch"]
            .as_str()
            .unwrap()
            .contains("result-second-half.txt"));
    }

    /// Requesting changes talks to the worktree's agent instead of killing it
    /// and spawning a replacement: the comments land on the durable thread, a
    /// turn is queued for the worktree's one agent, and the run's phase-session
    /// slot is never touched.
    #[test]
    fn run_request_changes_delivers_to_the_agent_instead_of_respawning() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (_, run_id) = planned_run_in_review(&mut state, "do work");
        let worktree_root = AppState::canonical_root(&state.runs[&run_id].worktree.path);
        let rc = state.handle(req(
            "run.request_changes",
            json!({ "run_id": run_id, "comments": "rename the symbol" }),
        ));
        assert_eq!(rc["result"]["state"], "review", "{rc:?}");
        assert!(
            state.tabs.is_empty(),
            "a verb queues a turn; only delivery — off the state lock — spawns"
        );
        let queued = state
            .pending_agent_turns
            .last()
            .expect("a change request is a turn for the worktree's agent");
        assert_eq!(queued.owner, run_id);
        assert_eq!(
            queued.root, worktree_root,
            "the turn is addressed to the worktree, not to the run"
        );
        assert_eq!(
            queued.warm, NEW_THREAD_MESSAGES_PROMPT,
            "an agent already in the conversation is only told to read the thread"
        );
        assert!(
            queued.cold.contains(NEW_THREAD_MESSAGES_PROMPT)
                && queued.cold.contains("rename the symbol")
                && queued.cold.contains("Ordered Issue stage-plan catalog")
                && queued.cold.find("\n- first-half").unwrap()
                    < queued.cold.find("\n- second-half").unwrap(),
            "a cold agent gets the run context, ordered stage catalog, AND the reviewer's words: {}",
            queued.cold
        );
        let structured = state.handle(req(
            "run.request_changes",
            json!({
                "run_id": run_id,
                "messages": [{
                    "body": "Use the public name",
                    "anchor": {
                        "artifact": "diff",
                        "path": "src/lib.rs",
                        "side": "new",
                        "line_start": 12,
                        "line_end": 12,
                        "heading_path": [],
                        "snippet": "fn old_name()"
                    }
                }]
            }),
        ));
        assert_eq!(structured["ok"], true, "{structured:?}");
        let messages = structured["result"]["thread"]["items"].as_array().unwrap();
        assert!(messages.iter().any(|item| {
            item["type"] == "message"
                && item["data"]["body"] == "Use the public name"
                && item["data"]["anchor"]["path"] == "src/lib.rs"
        }));
        let bad = state.handle(req("run.request_changes", json!({ "run_id": run_id })));
        assert!(
            bad["error"].as_str().unwrap().contains("comments"),
            "{bad:?}"
        );
    }

    /// A freeform message to a working run's agent talks to the process the
    /// reviewer is already in conversation with. The words land on the durable
    /// thread, the turn is addressed to the WORKTREE, and the run's phase
    /// session is never ended or replaced.
    #[test]
    fn run_message_delivers_to_the_agent_instead_of_respawning() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let root = insert_run(
            &mut state,
            &repo,
            dir.path(),
            "run-message",
            RunState::Building,
        );

        let sent = state.handle(req(
            "run.message",
            json!({ "run_id": "run-message", "message": "prefer the smaller helper" }),
        ));
        assert_eq!(sent["ok"], true, "{sent:?}");
        assert!(
            state.tabs.is_empty(),
            "messaging the agent must not spawn a harness under the state lock"
        );

        let queued = state
            .pending_agent_turns
            .last()
            .expect("a message is a turn for the worktree's agent");
        assert_eq!(queued.owner, "run-message");
        assert_eq!(
            queued.root, root,
            "the turn is addressed to the worktree, not to the run"
        );
        assert_eq!(
            queued.warm, NEW_THREAD_MESSAGES_PROMPT,
            "an agent already in the conversation is only told to read the thread"
        );
        assert!(
            queued.cold.contains(NEW_THREAD_MESSAGES_PROMPT)
                && queued.cold.contains("Build conversation protocol"),
            "a cold agent gets the run context and the conversation protocol: {}",
            queued.cold
        );
        let posted = state.runs["run-message"].thread.items.iter().any(|item| {
            matches!(item, crate::thread::ThreadItem::Message(m)
                if m.body == "prefer the smaller helper")
        });
        assert!(posted, "the reviewer's words stay durable on the thread");
    }

    /// Dispatching a stage is a turn, not a new process. The stage prompt is
    /// queued for the worktree's one agent; a warm agent hears the stage
    /// instruction alone (it lived the conversation), a cold one hears the same
    /// instruction wrapped in the conversation protocol and catch-up packet.
    #[test]
    fn dispatching_a_stage_queues_its_prompt_for_the_worktrees_one_agent() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let plan = state.handle(req("plan.create", json!({ "goal": "two stages" })));
        let plan_id = plan_id_of(&plan);
        for stage_id in ["first-half", "second-half"] {
            state.handle(req(
                "plan.stage_approve",
                json!({ "plan_id": plan_id, "stage_id": stage_id }),
            ));
        }
        state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
        let run = state.handle(req("run.create", json!({ "plan_id": plan_id })));
        let run_id = run_id_of(&run);
        let root = AppState::canonical_root(&state.runs[&run_id].worktree.path);
        state.pending_agent_turns.clear();

        let dispatched = state.handle(req(
            "run.stage_dispatch",
            json!({ "run_id": run_id, "stage_id": "second-half" }),
        ));
        assert_eq!(dispatched["ok"], true, "{dispatched:?}");
        assert!(
            state.tabs.is_empty(),
            "dispatching a stage must not spawn a harness under the state lock"
        );

        let queued = state
            .pending_agent_turns
            .first()
            .expect("a stage dispatch is a turn for the worktree's agent");
        assert_eq!(queued.owner, run_id);
        assert_eq!(queued.root, root);
        assert!(
            queued.warm.contains("Second half"),
            "the stage instruction travels whether the agent is warm or cold: {}",
            queued.warm
        );
        assert!(
            !queued.warm.contains("Build conversation protocol"),
            "a warm agent is not re-taught the protocol it is already following: {}",
            queued.warm
        );
        assert!(
            queued.cold.starts_with(&queued.warm)
                && queued.cold.contains("Build conversation protocol"),
            "a cold agent gets the same instruction plus the conversation it missed: {}",
            queued.cold
        );
    }

    /// A multi-stage run parked at its stage gate after a REAL first-stage build
    /// and validation verdict — the shape `run.stage_fix` and run-all act on,
    /// reached without the scripted agent playing both sides of the stage.
    /// Returns `(run_id, worktree root)`.
    fn run_at_the_stage_gate_after_a_real_first_stage(
        state: &mut AppState,
        goal: &str,
        first_stage_passed: bool,
    ) -> (String, std::path::PathBuf) {
        let (run_id, root) = run_awaiting_a_real_stage_build(state, goal);
        state.on_agent_done(
            &run_id,
            DoneReport {
                phase: DonePhase::Build,
                status: DoneStatus::Completed,
                summary: "the first stage is built".into(),
                outputs: DoneOutputs::default(),
            },
        );
        state.on_agent_done(
            &run_id,
            DoneReport {
                phase: DonePhase::Validate,
                status: DoneStatus::Completed,
                summary: "the first stage is validated".into(),
                outputs: DoneOutputs {
                    validation: Some(ValidationReport {
                        passed: first_stage_passed,
                        findings: if first_stage_passed {
                            String::new()
                        } else {
                            "- the migration is missing".into()
                        },
                        notes_for_next_stage: String::new(),
                    }),
                    ..DoneOutputs::default()
                },
            },
        );
        assert_eq!(
            state.runs[&run_id].run.state,
            RunState::StageGate,
            "a verdict on a non-final stage parks the run at the gate"
        );
        (run_id, root)
    }

    /// Fixing a failed stage is a turn, not a new process. `run.stage_fix` moves
    /// the stage back to `Building` — and unless the fix prompt is QUEUED for the
    /// worktree's one agent, the run sits in `Building` forever with nobody ever
    /// told to fix anything. Moving the state is not dispatching the work.
    #[test]
    fn fixing_a_failed_stage_queues_its_fix_prompt_for_the_worktrees_one_agent() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (run_id, root) =
            run_at_the_stage_gate_after_a_real_first_stage(&mut state, "fix the stage", false);
        assert_eq!(
            state.runs[&run_id].stages[0].state,
            StageProgressState::Validated { passed: false },
            "the fix verb only applies to a stage whose validation failed"
        );
        state.pending_agent_turns.clear();

        let fixed = state.handle(req(
            "run.stage_fix",
            json!({
                "run_id": run_id,
                "stage_id": "first-half",
                "note": "add the migration",
            }),
        ));
        assert_eq!(fixed["ok"], true, "{fixed:?}");
        assert_eq!(
            state.runs[&run_id].stages[0].state,
            StageProgressState::Building,
            "the stage went back to work"
        );
        assert!(
            state.tabs.is_empty(),
            "a verb queues a turn; only delivery — off the state lock — spawns"
        );

        let queued = state
            .pending_agent_turns
            .last()
            .expect("a stage fix is a turn for the worktree's agent");
        assert_eq!(queued.owner, run_id);
        assert_eq!(
            queued.root, root,
            "the stage is fixed in the worktree it was built in"
        );
        assert_eq!(queued.phase, "build");
        assert!(
            queued.warm.contains("add the migration")
                && queued.warm.contains("the migration is missing"),
            "the reviewer's note and the failed findings both travel: {}",
            queued.warm
        );
        assert!(
            !queued.warm.contains("Build conversation protocol"),
            "the agent that just failed validation is not re-taught the protocol: {}",
            queued.warm
        );
        assert!(
            queued.cold.starts_with(&queued.warm)
                && queued.cold.contains("Build conversation protocol"),
            "a replacement agent gets the same fix plus the conversation it missed: {}",
            queued.cold
        );
    }

    /// Sending a stage's open comments mid-run is a turn for the RUN's worktree
    /// agent (the plan doc is revised where the run can see it). The comments
    /// land durably on both threads first — but unless the revision turn is
    /// queued, nothing ever asks the agent to revise the doc and the run stalls
    /// at the gate with `revising_stage_id` set and no agent working.
    #[test]
    fn sending_stage_notes_mid_run_queues_a_revision_turn_for_the_worktrees_one_agent() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let plan = state.handle(req("plan.create", json!({ "goal": "revise mid-run" })));
        let plan_id = plan_id_of(&plan);
        for stage_id in ["first-half", "second-half"] {
            let approved = state.handle(req(
                "plan.stage_approve",
                json!({ "plan_id": plan_id, "stage_id": stage_id }),
            ));
            assert_eq!(approved["ok"], true, "{approved:?}");
        }
        state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
        let run = state.handle(req("run.create", json!({ "plan_id": plan_id })));
        let run_id = run_id_of(&run);
        assert_eq!(run["result"]["state"], "stage_gate", "{run:?}");
        let root = AppState::canonical_root(&state.runs[&run_id].worktree.path);
        let comment = state.handle(req(
            "plan.comment_add",
            json!({ "plan_id": plan_id, "stage_id": "second-half", "body": "tighten this" }),
        ));
        assert_eq!(comment["ok"], true, "{comment:?}");
        // The scripted agent answers the revision itself, which would swallow
        // the very dispatch under test.
        state.qa_agent = false;
        state.pending_agent_turns.clear();

        let sent = state.handle(req(
            "run.stage_send_notes",
            json!({ "run_id": run_id, "stage_id": "second-half" }),
        ));
        assert_eq!(sent["ok"], true, "{sent:?}");
        assert!(
            state.tabs.is_empty(),
            "a verb queues a turn; only delivery — off the state lock — spawns"
        );

        let queued = state
            .pending_agent_turns
            .last()
            .expect("stage notes are a turn for the run worktree's agent");
        assert_eq!(queued.owner, run_id);
        assert_eq!(
            queued.root, root,
            "the doc is revised in the run's worktree, not the plan's"
        );
        assert_eq!(queued.phase, "revise");
        assert!(
            queued.warm.contains("read_unread_messages"),
            "the comments travel through MCP; the turn only points at them: {}",
            queued.warm
        );
        assert!(
            !queued.warm.contains("Build conversation protocol"),
            "an agent already in the run is not re-taught the protocol: {}",
            queued.warm
        );
        assert!(
            queued.cold.contains("02-second-half.md")
                && queued.cold.contains("Build conversation protocol")
                && queued.cold.contains("Ordered Issue stage-plan catalog")
                && queued.cold.find("\n- first-half").unwrap()
                    < queued.cold.find("\n- second-half").unwrap(),
            "a cold agent is primed with the ordered catalog and stage doc it must revise: {}",
            queued.cold
        );
        let durable = state.runs[&run_id].thread.items.iter().any(|item| {
            matches!(item, crate::thread::ThreadItem::Message(m)
                if m.body.contains("tighten this"))
        });
        assert!(durable, "the comments stay durable on the run's thread");
    }

    /// Run-all is the one dispatcher with no human behind each hop: arming it at
    /// a stage gate must QUEUE the next stage's turn, not merely walk the run's
    /// state forward. Drop the queueing and every stage advances while no agent
    /// is ever asked to build one — the failure is silent by construction.
    #[test]
    fn auto_advance_queues_the_next_stages_turn_for_the_worktrees_one_agent() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (run_id, root) =
            run_at_the_stage_gate_after_a_real_first_stage(&mut state, "run them all", true);
        assert_eq!(
            state.runs[&run_id].stages[0].state,
            StageProgressState::Validated { passed: true },
            "the first stage passed, so the next one is dispatchable"
        );
        state.pending_agent_turns.clear();

        let armed = state.handle(req(
            "run.set_auto_advance",
            json!({ "run_id": run_id, "enabled": true }),
        ));
        assert_eq!(armed["ok"], true, "{armed:?}");
        assert_eq!(
            state.runs[&run_id].run.state,
            RunState::Building,
            "run-all dispatched the next stage"
        );
        assert!(
            state.tabs.is_empty(),
            "auto-advance queues a turn; only delivery — off the state lock — spawns"
        );

        let queued = state
            .pending_agent_turns
            .last()
            .expect("run-all dispatches the next stage as a turn for the worktree's agent");
        assert_eq!(queued.owner, run_id);
        assert_eq!(
            queued.root, root,
            "the next stage is built in the run's one worktree"
        );
        assert_eq!(queued.phase, "build");
        assert!(
            queued.warm.contains("Second half"),
            "the next stage's instruction travels warm or cold: {}",
            queued.warm
        );
        assert!(
            !queued.warm.contains("Build conversation protocol"),
            "the agent that built stage one is not re-taught the protocol: {}",
            queued.warm
        );
        assert!(
            queued.cold.starts_with(&queued.warm)
                && queued.cold.contains("Build conversation protocol"),
            "a replacement agent gets the same instruction plus the conversation: {}",
            queued.cold
        );
    }

    /// Run-all end to end: no human types anything between stages, so the whole
    /// path — queue under the state lock, drain off it — has to carry the next
    /// stage's prompt into the SAME agent process. A break anywhere along it is
    /// invisible from the run's state, which advances either way.
    #[tokio::test]
    async fn run_all_delivers_the_next_stages_prompt_to_the_same_agent_process() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
        let (run_id, root) = {
            let mut s = state.lock().unwrap();
            run_at_the_stage_gate_after_a_real_first_stage(&mut s, "run them all", true)
        };
        // The turns the fixture queued were queued on the state directly; the
        // handler is the thing that delivers, and delivering opens the agent.
        let opened = call(&handler, "run.get", json!({ "run_id": run_id }));
        assert_eq!(opened["ok"], true, "{opened:?}");
        let key = TabKey::agent(&root);
        let first_stage_pid = {
            let s = state.lock().unwrap();
            s.tabs
                .get(&key)
                .expect("the first stage's turns opened the worktree's agent")
                .session
                .pid()
                .expect("a live harness has a pid")
        };

        let armed = call(
            &handler,
            "run.set_auto_advance",
            json!({ "run_id": run_id, "enabled": true }),
        );
        assert_eq!(armed["ok"], true, "{armed:?}");

        let screen = wait_for_agent_screen(&state, &root, "Second half").await;
        assert!(
            screen.contains("Second half"),
            "run-all's next stage must reach the agent's PTY: {screen:?}"
        );
        let s = state.lock().unwrap();
        assert_eq!(
            s.runs[&run_id].run.state,
            RunState::Building,
            "the run is building the stage run-all dispatched"
        );
        assert_eq!(
            s.tabs.get(&key).and_then(|tab| tab.session.pid()),
            Some(first_stage_pid),
            "the agent that built stage one is the one asked to build stage two"
        );
        assert_eq!(s.tabs.len(), 1, "one worktree, one agent");
    }

    /// Every phase of a multi-stage run — the first stage's build, the
    /// validation hand-off its `done` triggers, and the next stage's build —
    /// reaches ONE process in the run's worktree. The phase boundary stopped
    /// being a process boundary: that is the whole point of the tab.
    #[tokio::test]
    async fn a_multi_stage_run_drives_one_agent_process_through_every_phase() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
        let plan = call(&handler, "plan.create", json!({ "goal": "one agent" }));
        let plan_id = plan_id_of(&plan);
        for stage_id in ["first-half", "second-half"] {
            call(
                &handler,
                "plan.stage_approve",
                json!({ "plan_id": plan_id, "stage_id": stage_id }),
            );
        }
        call(&handler, "plan.approve", json!({ "plan_id": plan_id }));
        let run = call(&handler, "run.create", json!({ "plan_id": plan_id }));
        assert_eq!(run["ok"], true, "{run:?}");
        let run_id = run_id_of(&run);
        let key = {
            let s = state.lock().unwrap();
            TabKey::agent(&AppState::canonical_root(&s.runs[&run_id].worktree.path))
        };
        let first_pid = {
            let s = state.lock().unwrap();
            let tab = s
                .tabs
                .get(&key)
                .expect("dispatching a run opens the worktree's agent");
            assert!(
                tab.live && !tab.session.has_exited(),
                "the agent is running"
            );
            tab.session.pid().expect("a live harness has a pid")
        };

        let next = call(
            &handler,
            "run.stage_dispatch",
            json!({ "run_id": run_id, "stage_id": "second-half" }),
        );
        assert_eq!(next["ok"], true, "{next:?}");
        let s = state.lock().unwrap();
        assert_eq!(
            s.tabs.get(&key).and_then(|tab| tab.session.pid()),
            Some(first_pid),
            "every phase must reach the process the dispatch woke"
        );
        assert_eq!(
            s.tabs
                .keys()
                .filter(|k| k.tab_id == AGENT_TAB_ID && k.root == key.root)
                .count(),
            1,
            "one worktree, one agent"
        );
        assert_eq!(
            s.tabs.len(),
            1,
            "no harness may be spawned beside the tab's agent"
        );
    }

    /// The agent Build talks to is one process for the worktree's life. A
    /// second round of comments reaches the SAME harness — same pid, one tab —
    /// because a warm tab is delivered to, never replaced.
    #[tokio::test]
    async fn a_second_request_changes_reaches_the_same_agent_process() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
        // The fixture only needs state, so it goes through the synchronous
        // path; the change requests below go through the frame handler, which
        // is what actually delivers a queued turn.
        let (_, run_id) = planned_run_in_review(&mut state.lock().unwrap(), "keep the agent");
        let root = {
            let s = state.lock().unwrap();
            AppState::canonical_root(&s.runs[&run_id].worktree.path)
        };
        let key = TabKey::agent(&root);

        let first = call(
            &handler,
            "run.request_changes",
            json!({
                "run_id": run_id, "comments": "rename the symbol"
            }),
        );
        assert_eq!(first["ok"], true, "{first:?}");
        let first_pid = {
            let s = state.lock().unwrap();
            let tab = s
                .tabs
                .get(&key)
                .expect("a change request opens the worktree's agent");
            assert!(
                tab.live && !tab.session.has_exited(),
                "the agent is running"
            );
            tab.session.pid().expect("a live harness has a pid")
        };

        let second = call(
            &handler,
            "run.request_changes",
            json!({
                "run_id": run_id, "comments": "and inline the helper"
            }),
        );
        assert_eq!(second["ok"], true, "{second:?}");
        let s = state.lock().unwrap();
        assert_eq!(s.tabs.len(), 1, "one worktree, one agent");
        assert_eq!(
            s.tabs.get(&key).and_then(|tab| tab.session.pid()),
            Some(first_pid),
            "the second request must reach the process the first one woke"
        );
    }

    /// A persistent agent outlives the phase it was dispatched for: talk to it
    /// at a review gate and it reports `done` from a state the run machine does
    /// not accept. Enforcement is by observation, not permission — the report
    /// is recorded on the conversation and moves nothing, rather than landing
    /// as a failure the human never caused.
    #[test]
    fn an_out_of_phase_done_is_recorded_and_moves_nothing() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (_, run_id) = planned_run_in_review(&mut state, "already reviewed");

        state.on_agent_done(
            &run_id,
            DoneReport {
                phase: DonePhase::Build,
                status: DoneStatus::Completed,
                summary: "Tidied the imports you mentioned".into(),
                outputs: DoneOutputs::default(),
            },
        );

        let got = state.handle(req("run.get", json!({ "run_id": run_id })));
        assert_eq!(
            got["result"]["state"], "review",
            "an out-of-phase report moves nothing: {got:?}"
        );
        let events: Vec<&Value> = got["result"]["thread"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "event")
            .collect();
        assert!(
            events.iter().any(|e| {
                e["data"]["event"] == "done"
                    && e["data"]["summary"] == "Tidied the imports you mentioned"
            }),
            "the report is recorded: {events:?}"
        );
        assert!(
            !events.iter().any(|e| e["data"]["event"] == "run_failed"),
            "a report Build cannot apply is not a failure: {events:?}"
        );
    }

    /// A multi-stage run parked mid-build, waiting on a real `done` — the shape
    /// production has and the scripted agent never reaches, because it plays
    /// both sides of a stage itself. Returns `(state, run_id, worktree root)`.
    fn run_awaiting_a_real_stage_build(
        state: &mut AppState,
        goal: &str,
    ) -> (String, std::path::PathBuf) {
        let plan = state.handle(req("plan.create", json!({ "goal": goal })));
        let plan_id = plan_id_of(&plan);
        for stage_id in ["first-half", "second-half"] {
            let approved = state.handle(req(
                "plan.stage_approve",
                json!({ "plan_id": plan_id, "stage_id": stage_id }),
            ));
            assert_eq!(approved["ok"], true, "{approved:?}");
        }
        let approved = state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
        assert_eq!(approved["ok"], true, "{approved:?}");
        // From here the scripted agent must stop answering for the harness:
        // `qa_simulate_stage_build` consumes the build AND the validation in one
        // call, so it would swallow the very hand-off under test.
        state.qa_agent = false;
        let run = state.handle(req("run.create", json!({ "plan_id": plan_id })));
        assert_eq!(run["ok"], true, "{run:?}");
        let run_id = run_id_of(&run);
        let active = &state.runs[&run_id];
        assert_eq!(
            active.stages[0].state,
            StageProgressState::Building,
            "the run is waiting on its stage-build agent"
        );
        let root = AppState::canonical_root(&active.worktree.path);
        (run_id, root)
    }

    /// A stage that reports its build complete hands ITSELF to validation. The
    /// orchestrator returns that hand-off as a turn and `on_run_agent_done` is
    /// the only thing that queues it — a `done` is the one input to the daemon
    /// that starts a phase without a human verb behind it. Drop the queueing and
    /// a built stage is simply never asked to validate itself.
    #[test]
    fn a_built_stage_queues_its_validation_turn_for_the_worktrees_agent() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (run_id, root) = run_awaiting_a_real_stage_build(&mut state, "hand off to validation");
        state.pending_agent_turns.clear();

        state.on_agent_done(
            &run_id,
            DoneReport {
                phase: DonePhase::Build,
                status: DoneStatus::Completed,
                summary: "stage one is built".into(),
                outputs: DoneOutputs::default(),
            },
        );

        assert_eq!(
            state.runs[&run_id].stages[0].state,
            StageProgressState::Validating,
            "the report moved the stage to its validation gate"
        );
        let queued = state
            .pending_agent_turns
            .last()
            .expect("a built stage hands itself to validation as a turn");
        assert_eq!(queued.phase, "validate");
        assert_eq!(queued.owner, run_id);
        assert_eq!(
            queued.root, root,
            "the stage is validated in the worktree it was built in"
        );
        assert!(
            queued.warm.contains("VALIDATION agent"),
            "the validation instruction travels warm or cold: {}",
            queued.warm
        );
        assert!(
            !queued.warm.contains("Build conversation protocol"),
            "the agent that just reported is not re-taught the protocol: {}",
            queued.warm
        );
        assert!(
            queued.cold.starts_with(&queued.warm)
                && queued.cold.contains("Build conversation protocol"),
            "a replacement agent gets the same instruction plus the conversation: {}",
            queued.cold
        );
    }

    /// Connect to a unix socket a spawned worker is still binding.
    async fn connect_when_bound(path: &std::path::Path) -> tokio::net::UnixStream {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match tokio::net::UnixStream::connect(path).await {
                Ok(stream) => return stream,
                Err(error) if std::time::Instant::now() >= deadline => {
                    panic!("done socket never came up at {}: {error}", path.display())
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    }

    /// The hand-off has to survive the path it actually travels: a `done` line
    /// on the daemon's control socket, where the turn is queued while the socket
    /// worker holds the state lock. Only draining that queue after the lock is
    /// free gets the validation prompt written — and it must be written to the
    /// SAME process that just reported the stage built, not a replacement.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_done_over_the_socket_delivers_the_validation_turn_to_the_same_agent() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
        let (run_id, root) = {
            // The fixture only needs state; the frame handler below is what
            // delivered the dispatch turn that opened the agent.
            let mut s = state.lock().unwrap();
            run_awaiting_a_real_stage_build(&mut s, "hand off over the socket")
        };
        // `run.create` above ran on the shared state directly, so its dispatch
        // turn is still queued; the handler is the thing that delivers.
        let opened = call(&handler, "run.get", json!({ "run_id": run_id }));
        assert_eq!(opened["ok"], true, "{opened:?}");
        let key = TabKey::agent(&root);
        let build_pid = {
            let s = state.lock().unwrap();
            let tab = s
                .tabs
                .get(&key)
                .expect("dispatching a stage opens the worktree's agent");
            tab.session.pid().expect("a live harness has a pid")
        };

        let socket_path = dir.path().join("done.sock");
        AppState::spawn_done_socket(
            Arc::clone(&state),
            socket_path.to_string_lossy().into_owned(),
        );
        let mut socket = connect_when_bound(&socket_path).await;
        let report = json!({
            "task_id": run_id,
            "report": {
                "phase": "build",
                "status": "completed",
                "summary": "stage one is built",
                "outputs": {},
            },
        });
        socket
            .write_all(format!("{report}\n").as_bytes())
            .await
            .unwrap();
        socket.flush().await.unwrap();

        let screen = wait_for_agent_screen(&state, &root, "VALIDATION agent").await;
        assert!(
            screen.contains("VALIDATION agent"),
            "the validation turn must reach the agent's PTY: {screen:?}"
        );
        let s = state.lock().unwrap();
        assert_eq!(
            s.runs[&run_id].stages[0].state,
            StageProgressState::Validating
        );
        assert_eq!(
            s.tabs.get(&key).and_then(|tab| tab.session.pid()),
            Some(build_pid),
            "the agent that built the stage is the one asked to validate it"
        );
        assert_eq!(s.tabs.len(), 1, "one worktree, one agent");
    }

    /// A planning worktree is a worktree, so every plan verb is a turn
    /// addressed to it — never to the repo, never to a fresh process — and it
    /// splits cold/warm exactly as the run verbs do: the reviewer's words are
    /// already durable on the plan's thread, so a warm agent is only told to
    /// read them, while a cold one gets the same instruction wrapped in the plan
    /// context it has no way to reconstruct.
    #[test]
    fn plan_verbs_are_turns_addressed_to_the_planning_worktree() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let notes_plan = plan_id_of(&state.handle(req("plan.create", json!({ "goal": "notes" }))));
        let stage_plan = plan_id_of(&state.handle(req("plan.create", json!({ "goal": "stages" }))));
        let comment = state.handle(req(
            "plan.comment_add",
            json!({ "plan_id": stage_plan, "stage_id": "first-half", "body": "split further" }),
        ));
        assert_eq!(comment["ok"], true, "{comment:?}");
        let planning_root = |state: &AppState, plan_id: &str| {
            AppState::canonical_root(
                &state.plans[plan_id]
                    .worktree
                    .as_ref()
                    .expect("a drafting plan has a planning worktree")
                    .path,
            )
        };
        let notes_root = planning_root(&state, &notes_plan);
        let stage_root = planning_root(&state, &stage_plan);
        assert_ne!(
            notes_root, stage_root,
            "each plan drafts in its own worktree"
        );
        // The scripted agent answers every verb itself and drives the plan back
        // to its gate; from here each plan must stay where its verb puts it.
        state.qa_agent = false;

        state.pending_agent_turns.clear();
        let sent = state.handle(req(
            "plan.send_notes",
            json!({ "plan_id": notes_plan, "comments": "make stage two smaller" }),
        ));
        assert_eq!(sent["ok"], true, "{sent:?}");
        assert!(
            state.tabs.is_empty(),
            "a verb queues a turn; only delivery — off the state lock — spawns"
        );
        let queued = state
            .pending_agent_turns
            .last()
            .expect("plan notes are a turn for the planning worktree's agent");
        assert_eq!(queued.owner, notes_plan);
        assert_eq!(
            queued.root, notes_root,
            "a plan's turn goes to its planning worktree"
        );
        assert_eq!(queued.phase, "revise");
        assert_eq!(
            queued.warm, NEW_THREAD_MESSAGES_PROMPT,
            "an agent already drafting is only told to read the thread"
        );
        assert!(
            queued.cold.contains(NEW_THREAD_MESSAGES_PROMPT)
                && queued.cold.contains("Build conversation protocol"),
            "a cold agent gets the plan context AND the instruction: {}",
            queued.cold
        );
        let durable = state.plans[&notes_plan].thread.items.iter().any(|item| {
            matches!(item, crate::thread::ThreadItem::Message(m)
                if m.body == "make stage two smaller")
        });
        assert!(durable, "the notes stay durable on the plan's thread");

        // A freeform message reaches the same agent while the plan drafts.
        state.pending_agent_turns.clear();
        let messaged = state.handle(req(
            "plan.message",
            json!({ "plan_id": notes_plan, "message": "prefer smaller stages" }),
        ));
        assert_eq!(messaged["ok"], true, "{messaged:?}");
        let queued = state
            .pending_agent_turns
            .last()
            .expect("a plan message is a turn for the planning worktree's agent");
        assert_eq!(queued.owner, notes_plan);
        assert_eq!(queued.root, notes_root);
        assert_eq!(queued.phase, "message");
        assert_eq!(queued.warm, NEW_THREAD_MESSAGES_PROMPT);
        assert!(
            queued.cold.contains(NEW_THREAD_MESSAGES_PROMPT)
                && queued.cold.contains("Build conversation protocol"),
            "{}",
            queued.cold
        );
        let durable = state.plans[&notes_plan].thread.items.iter().any(|item| {
            matches!(item, crate::thread::ThreadItem::Message(m)
                if m.body == "prefer smaller stages")
        });
        assert!(durable, "the message stays durable on the plan's thread");

        // A stage's open comments are the payload of a per-stage revision.
        state.pending_agent_turns.clear();
        let stage_notes = state.handle(req(
            "plan.stage_send_notes",
            json!({ "plan_id": stage_plan, "stage_id": "first-half" }),
        ));
        assert_eq!(stage_notes["ok"], true, "{stage_notes:?}");
        let queued = state
            .pending_agent_turns
            .last()
            .expect("stage notes are a turn for the planning worktree's agent");
        assert_eq!(queued.owner, stage_plan);
        assert_eq!(queued.root, stage_root);
        assert_eq!(queued.phase, "revise");
        assert!(
            queued.warm.contains("read_unread_messages"),
            "the comments travel through MCP; the turn only points at them: {}",
            queued.warm
        );
        assert!(
            !queued.warm.contains("Build conversation protocol"),
            "an agent already drafting is not re-taught the protocol: {}",
            queued.warm
        );
        assert!(
            queued.cold.contains(".build/plan/01-first-half.md")
                && queued.cold.contains("Build conversation protocol"),
            "a cold agent is pointed at the stage doc it must revise: {}",
            queued.cold
        );
    }

    /// The plan half of "one worktree, one agent": authoring a plan opens the
    /// planning worktree's agent, and every later plan verb reaches THAT
    /// process. Same pid, one tab — a plan revision is a turn, not a
    /// replacement.
    #[tokio::test]
    async fn every_plan_verb_reaches_the_planning_worktrees_one_agent() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
        let plan = call(&handler, "plan.create", json!({ "goal": "one plan agent" }));
        assert_eq!(plan["ok"], true, "{plan:?}");
        let plan_id = plan_id_of(&plan);
        let key = {
            let s = state.lock().unwrap();
            TabKey::agent(&AppState::canonical_root(
                &s.plans[&plan_id]
                    .worktree
                    .as_ref()
                    .expect("a plan at its gate keeps its planning worktree")
                    .path,
            ))
        };
        let drafting_pid = {
            let s = state.lock().unwrap();
            let tab = s
                .tabs
                .get(&key)
                .expect("authoring a plan opens the planning worktree's agent");
            assert!(
                tab.live && !tab.session.has_exited(),
                "the plan's agent is running"
            );
            tab.session.pid().expect("a live harness has a pid")
        };
        // The scripted agent would answer each verb itself and drive the plan
        // straight back to its gate; from here it must stay where a verb puts it.
        state.lock().unwrap().qa_agent = false;

        for (method, params) in [
            (
                "plan.send_notes",
                json!({ "plan_id": plan_id, "comments": "make stage two smaller" }),
            ),
            (
                "plan.message",
                json!({ "plan_id": plan_id, "message": "prefer smaller stages" }),
            ),
        ] {
            let done = call(&handler, method, params);
            assert_eq!(done["ok"], true, "{method}: {done:?}");
            let s = state.lock().unwrap();
            assert_eq!(
                s.tabs.get(&key).and_then(|tab| tab.session.pid()),
                Some(drafting_pid),
                "{method} must reach the process that authored the plan"
            );
            assert_eq!(s.tabs.len(), 1, "{method}: one worktree, one agent");
        }
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
    fn conversation_records_status_details_and_agent_authored_done_messages() {
        let mut thread = crate::thread::Thread::new("run-activity");
        record_report_in_thread(
            &mut thread,
            &DoneReport {
                phase: DonePhase::Build,
                status: DoneStatus::Blocked,
                summary: "Needs production credentials".into(),
                outputs: DoneOutputs::default(),
            },
            None,
        );
        assert!(thread.items.iter().any(|item| matches!(
            item,
            crate::thread::ThreadItem::Event(event)
                if event.event == crate::thread::ThreadEventKind::Blocked
                    && event.summary.as_deref() == Some("Needs production credentials")
        )));

        record_report_in_thread(
            &mut thread,
            &DoneReport {
                phase: DonePhase::Validate,
                status: DoneStatus::Completed,
                summary: "Validation completed".into(),
                outputs: DoneOutputs {
                    validation: Some(crate::run::ValidationReport {
                        passed: false,
                        findings: "The migration is not reversible".into(),
                        notes_for_next_stage: String::new(),
                    }),
                    completion_report: Some(crate::thread::CompletionReport {
                        critical_files: vec!["src/app.rs".into()],
                        ..crate::thread::CompletionReport::default()
                    }),
                    ..DoneOutputs::default()
                },
            },
            None,
        );
        assert!(thread.items.iter().any(|item| matches!(
            item,
            crate::thread::ThreadItem::Event(event)
                if event.event == crate::thread::ThreadEventKind::ReviewBlocked
                    && event.summary.as_deref() == Some("The migration is not reversible")
        )));
        assert!(!thread.items.iter().any(|item| matches!(
            item,
            crate::thread::ThreadItem::Message(message)
                if message.role == crate::thread::MessageRole::Agent && message.done
        )));
        assert_eq!(
            thread.last_completion.as_ref().unwrap().critical_files,
            vec!["src/app.rs"]
        );

        record_report_in_thread(
            &mut thread,
            &DoneReport {
                phase: DonePhase::Build,
                status: DoneStatus::Completed,
                summary: "Fixed and deployed the renderer.".into(),
                outputs: DoneOutputs::default(),
            },
            None,
        );
        assert!(thread.items.iter().any(|item| matches!(
            item,
            crate::thread::ThreadItem::Message(message)
                if message.role == crate::thread::MessageRole::Agent
                    && message.done
                    && message.body == "Fixed and deployed the renderer."
        )));
    }

    #[test]
    fn review_actions_are_recorded_in_the_plan_conversation() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let plan = state.handle(req("plan.create", json!({ "goal": "review activity" })));
        let plan_id = plan_id_of(&plan);

        state.handle(req(
            "plan.stage_approve",
            json!({ "plan_id": plan_id, "stage_id": "first-half" }),
        ));
        state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
        state.handle(req("run.create", json!({ "plan_id": plan_id })));
        let view = state.handle(req("plan.get", json!({ "plan_id": plan_id })));
        let items = view["result"]["thread"]["items"].as_array().unwrap();
        assert!(items
            .iter()
            .any(|item| item["data"]["event"] == "stage_approved"));
        assert!(items.iter().any(|item| item["data"]["event"] == "approved"));
        assert!(items
            .iter()
            .any(|item| item["data"]["event"] == "implementation_started"));
    }

    #[test]
    fn planning_announces_each_stage_with_a_link_to_its_document() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let plan = state.handle(req("plan.create", json!({ "goal": "linked stages" })));
        let plan_id = plan_id_of(&plan);
        let view = state.handle(req("plan.get", json!({ "plan_id": plan_id })));
        let stage_messages: Vec<&Value> = view["result"]["thread"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| {
                item["type"] == "message"
                    && item["data"]["role"] == "agent"
                    && item["data"]["links"][0]["kind"] == "plan_stage"
            })
            .collect();

        assert_eq!(stage_messages.len(), 2, "{stage_messages:?}");
        assert_eq!(stage_messages[0]["data"]["links"][0]["plan_id"], plan_id);
        assert_eq!(
            stage_messages[0]["data"]["links"][0]["stage_id"],
            "first-half"
        );
        assert_eq!(
            stage_messages[1]["data"]["links"][0]["stage_id"],
            "second-half"
        );
    }

    #[test]
    fn run_conversation_links_every_started_stage_back_to_the_plan() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let plan = state.handle(req("plan.create", json!({ "goal": "linked dispatch" })));
        let plan_id = plan_id_of(&plan);
        for stage_id in ["first-half", "second-half"] {
            state.handle(req(
                "plan.stage_approve",
                json!({ "plan_id": plan_id, "stage_id": stage_id }),
            ));
        }
        state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
        let run = state.handle(req("run.create", json!({ "plan_id": plan_id })));
        let run_id = run_id_of(&run);
        state.handle(req(
            "run.stage_dispatch",
            json!({ "run_id": run_id, "stage_id": "second-half" }),
        ));
        let view = state.handle(req("run.get", json!({ "run_id": run_id })));
        let starts: Vec<&Value> = view["result"]["thread"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["data"]["event"] == "stage_started")
            .collect();

        assert_eq!(starts.len(), 2, "{starts:?}");
        assert_eq!(starts[0]["data"]["links"][0]["plan_id"], plan_id);
        assert_eq!(starts[0]["data"]["links"][0]["stage_id"], "first-half");
        assert_eq!(starts[1]["data"]["links"][0]["stage_id"], "second-half");
    }

    #[test]
    fn mcp_thread_actions_are_owner_scoped_and_revision_snapshots_are_on_demand() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let mut active = crate::orchestrator::ActiveRun::reattach(
            &fake_run_record("run-thread"),
            ".build/plan.md".into(),
        );
        active.thread.post_user("rename it", None, now_rfc3339());
        let revision = active.thread.add_revision(
            crate::thread::ArtifactKind::Diff,
            "diff --git a/a b/a\n+new",
            &now_rfc3339(),
        );
        let project_id = state.projects[0].id.clone();
        state.entity_project.insert("run-thread".into(), project_id);
        state.runs.insert("run-thread".into(), active);

        let unread = state
            .on_mcp_action("run-thread", BridgeAction::ReadUnreadMessages)
            .unwrap();
        assert_eq!(unread["messages"][0]["body"], "rename it");
        assert!(state
            .on_mcp_action("run-thread", BridgeAction::ReadUnreadMessages)
            .unwrap()["messages"]
            .as_array()
            .unwrap()
            .is_empty());
        state
            .on_mcp_action(
                "run-thread",
                BridgeAction::PostThreadMessage {
                    body: "Which name?".into(),
                    anchor: None,
                    links: Vec::new(),
                },
            )
            .unwrap();
        let escaping_link = state.on_mcp_action(
            "run-thread",
            BridgeAction::PostThreadMessage {
                body: "Open this".into(),
                anchor: None,
                links: vec![crate::thread::ThreadLink::File {
                    path: "../../etc/passwd".into(),
                    line_start: None,
                    line_end: None,
                }],
            },
        );
        assert_eq!(
            escaping_link.unwrap_err(),
            "file link path escapes the worktree"
        );
        let view = state.handle(req("run.get", json!({ "run_id": "run-thread" })));
        assert!(view["result"]["thread"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["data"]["body"] == "Which name?"));
        assert!(view["result"]["thread"]["revisions"][0]
            .get("snapshot")
            .is_none());

        let historical = state.handle(req(
            "thread.revision",
            json!({ "entity_id": "run-thread", "revision_id": revision.id }),
        ));
        assert_eq!(historical["result"]["contents"], "diff --git a/a b/a\n+new");
        assert!(state
            .on_mcp_action("another-run", BridgeAction::ReadUnreadMessages)
            .is_err());
    }

    #[test]
    fn mcp_typed_links_cannot_forge_another_issues_lineage() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (issue_a, run_a) = planned_run_in_review(&mut state, "owner a");
        let (issue_b, run_b) = planned_run_in_review(&mut state, "owner b");
        let stage_b = state.plans[&issue_b].stages[0].clone();

        let forged_stage = state.on_mcp_action(
            &run_a,
            BridgeAction::PostThreadMessage {
                body: "look elsewhere".into(),
                anchor: None,
                links: vec![crate::thread::ThreadLink::PlanStage {
                    plan_id: issue_b.clone(),
                    stage_id: stage_b.id,
                    path: stage_b.path,
                }],
            },
        );
        assert_eq!(
            forged_stage.unwrap_err(),
            "plan stage link does not belong to this Issue"
        );
        let forged_run = state.on_mcp_action(
            &run_a,
            BridgeAction::PostThreadMessage {
                body: "open another implementation".into(),
                anchor: None,
                links: vec![crate::thread::ThreadLink::Run { run_id: run_b }],
            },
        );
        assert_eq!(
            forged_run.unwrap_err(),
            "run link does not belong to this Issue"
        );
        assert!(state.plans.contains_key(&issue_a));
    }

    #[test]
    fn a_cursored_poll_reships_a_message_after_the_agent_marks_it_seen() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let mut active = crate::orchestrator::ActiveRun::reattach(
            &fake_run_record("run-seen"),
            ".build/plan.md".into(),
        );
        active.thread.post_user("rename it", None, now_rfc3339());
        let project_id = state.projects[0].id.clone();
        state.entity_project.insert("run-seen".into(), project_id);
        state.runs.insert("run-seen".into(), active);

        // The client holds the full thread: its cursor is the last sequence.
        let full = state.handle(req("run.get", json!({ "run_id": "run-seen" })));
        let full_items = full["result"]["thread"]["items"].as_array().unwrap();
        let cursor = full_items.last().unwrap()["data"]["sequence"]
            .as_u64()
            .unwrap();

        // The agent reads the message: an in-place mutation of an item the
        // client already holds. Pre-fix regression: the cursored poll skipped
        // it and the message rendered "Unread" forever.
        state
            .on_mcp_action("run-seen", BridgeAction::ReadUnreadMessages)
            .unwrap();
        let delta = state.handle(req(
            "run.get",
            json!({ "run_id": "run-seen", "thread_after_sequence": cursor }),
        ));
        let delta_thread = &delta["result"]["thread"];
        let reshipped = delta_thread["items"].as_array().unwrap();
        assert!(
            reshipped
                .iter()
                .any(|item| item["data"]["body"] == "rename it"
                    && item["data"]["seen_at"].is_string()),
            "{delta_thread:?}"
        );
        // And the advanced high-water mark drains: the client does not loop.
        let advanced = delta_thread["thread_last_sequence"].as_u64().unwrap();
        assert!(advanced > cursor, "{delta_thread:?}");
        let drained = state.handle(req(
            "run.get",
            json!({ "run_id": "run-seen", "thread_after_sequence": advanced }),
        ));
        assert_eq!(
            drained["result"]["thread"]["items"]
                .as_array()
                .unwrap()
                .len(),
            0,
            "{drained:?}"
        );
    }

    // ---- thread.post: the non-dispatching conversation write ---------------

    /// Talking to the agent you are looking at must reach it, whatever the run
    /// happens to be parked as.
    ///
    /// The nudge used to fire only while a run was `Building`, from the era when
    /// the agent EXISTED only while building — every other state meant no
    /// process to talk to. A worktree's agent now outlives every phase and sits
    /// right there in the Agent tab at the review gate, so gating on run state
    /// meant typing into a live conversation and having it silently not arrive.
    #[tokio::test]
    async fn thread_post_reaches_the_live_agent_at_a_review_gate() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let run_id = "run-at-the-gate".to_string();
        let key = insert_run_with_agent_tab(
            &mut state,
            &repo,
            &dir.path().join("side"),
            &run_id,
            RunState::Review,
            warm_tui_spec(),
        );
        let pid_before = state.tabs[&key].session.pid().expect("a live agent");
        let mut output = state.tabs[&key].session.subscribe();

        let posted = state.handle(req(
            "thread.post",
            json!({ "entity_id": run_id, "body": "why did you drop the index?" }),
        ));
        assert_eq!(posted["ok"], true, "{posted:?}");
        assert_eq!(
            posted["result"]["state"], "review",
            "the gate does not move"
        );
        assert_eq!(
            state.tabs[&key].session.pid(),
            Some(pid_before),
            "a post talks to the agent, it never replaces it"
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut echoed = String::new();
        while std::time::Instant::now() < deadline && !echoed.contains("read_unread_messages") {
            match output.try_recv() {
                Ok(chunk) => echoed.push_str(&String::from_utf8_lossy(&chunk)),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => break,
            }
        }
        assert!(
            echoed.contains("read_unread_messages"),
            "the agent at the gate must hear the message: {echoed:?}"
        );
    }

    /// The review surface's whole point: a message lands in the run's thread
    /// as unread WITHOUT respawning the agent or moving the run's state, and
    /// the agent's catch-up tool then drains it.
    #[test]
    fn thread_post_in_review_posts_unread_and_moves_no_state() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (_, run_id) = planned_run_in_review(&mut state, "post-only path");

        let posted = state.handle(req(
            "thread.post",
            json!({ "entity_id": run_id, "body": "just a review note" }),
        ));
        assert_eq!(posted["ok"], true, "{posted:?}");
        // The same Full-thread view shape the dispatching verbs return, so the
        // caller can render optimistically.
        assert_eq!(posted["result"]["state"], "review", "{posted:?}");
        let items = posted["result"]["thread"]["items"].as_array().unwrap();
        let message = items
            .iter()
            .find(|item| item["data"]["body"] == "just a review note")
            .unwrap_or_else(|| panic!("posted message missing: {posted:?}"));
        assert_eq!(message["data"]["role"], "user", "{message:?}");
        assert!(
            message["data"].get("seen_at").is_none(),
            "the post must land unread: {message:?}"
        );

        let active = state.runs.get(&run_id).unwrap();
        assert_eq!(active.run.state, RunState::Review, "no state transition");
        assert!(state.tabs.is_empty(), "a post starts no agent");

        let unread = state
            .on_mcp_action(&run_id, BridgeAction::ReadUnreadMessages)
            .unwrap();
        assert_eq!(unread["messages"][0]["body"], "just a review note");
    }

    #[tokio::test]
    async fn thread_post_addressed_to_issue_nudges_its_live_implementation_agent() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (issue_id, run_id) = planned_run_in_review(&mut state, "issue-addressed post");
        let root = AppState::canonical_root(&state.runs[&run_id].worktree.path);
        let (tab, _rx) = Tab::spawn(
            TabRole::Agent {
                owner: run_id.clone(),
                provider: AgentProvider::default(),
            },
            &warm_tui_spec(),
            AGENT_TAB_ID.to_string(),
            root.clone(),
            120,
            40,
        )
        .expect("implementation agent tab spawns");
        let mut output = tab.session.subscribe();
        state.tabs.insert(TabKey::agent(&root), tab);

        let posted = state.handle(req(
            "thread.post",
            json!({ "entity_id": issue_id, "body": "read this in the implementation" }),
        ));
        assert_eq!(posted["ok"], true, "{posted:?}");

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut echoed = String::new();
        while std::time::Instant::now() < deadline && !echoed.contains("read_unread_messages") {
            match output.try_recv() {
                Ok(chunk) => echoed.push_str(&String::from_utf8_lossy(&chunk)),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => break,
            }
        }
        assert!(
            echoed.contains("read_unread_messages"),
            "the active implementation agent must be nudged: {echoed:?}"
        );
        let unread = state
            .on_mcp_action(&run_id, BridgeAction::ReadUnreadMessages)
            .unwrap();
        assert!(unread["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["body"] == "read this in the implementation"));
    }

    #[test]
    fn planned_run_conversation_and_mcp_alias_the_issue_thread() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (plan_id, run_id) = planned_run_in_review(&mut state, "one issue thread");
        let plan_state = state.plans[&plan_id].plan.state;
        let run_state = state.runs[&run_id].run.state;

        let posted = state.handle(req(
            "thread.post",
            json!({ "entity_id": run_id, "body": "shared implementation note" }),
        ));
        assert_eq!(posted["ok"], true, "{posted:?}");
        assert_eq!(state.plans[&plan_id].plan.state, plan_state);
        assert_eq!(state.runs[&run_id].run.state, run_state);
        assert!(state.runs[&run_id]
            .thread
            .items
            .iter()
            .all(|item| !matches!(item, crate::thread::ThreadItem::Message(message) if message.body == "shared implementation note")));

        let run_view = state.handle(req("run.get", json!({ "run_id": run_id })));
        assert!(run_view["result"]["thread"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["data"]["body"] == "shared implementation note"));
        let unread = state
            .on_mcp_action(&run_id, BridgeAction::ReadUnreadMessages)
            .unwrap();
        assert_eq!(unread["thread_id"], format!("thread:{plan_id}"));
        assert!(unread["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["body"] == "shared implementation note"));
    }

    /// A mid-build post must leave the worktree's agent running (same process,
    /// same tab) and nudge it in place through its PTY — the PTY echoes written
    /// input back to its reader, so the nudge is observable on the tab's output
    /// stream.
    #[test]
    fn thread_post_in_building_nudges_the_live_session_without_ending_it() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let run_id = "run-nudge".to_string();
        let key = insert_run_with_agent_tab(
            &mut state,
            &repo,
            &dir.path().join("side"),
            &run_id,
            RunState::Building,
            warm_tui_spec(),
        );
        let pid_before = state.tabs[&key].session.pid().expect("a live agent");
        let mut output = state.tabs[&key].session.subscribe();

        let posted = state.handle(req(
            "thread.post",
            json!({ "entity_id": run_id, "body": "while you build" }),
        ));
        assert_eq!(posted["ok"], true, "{posted:?}");

        let active = state.runs.get(&run_id).unwrap();
        assert_eq!(active.run.state, RunState::Building, "no state transition");
        assert_eq!(
            state.tabs[&key].session.pid(),
            Some(pid_before),
            "the worktree's agent must not be respawned"
        );
        assert!(
            !state.tabs[&key].session.has_exited(),
            "the worktree's agent must not be ended"
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut echoed = String::new();
        while std::time::Instant::now() < deadline && !echoed.contains("read_unread_messages") {
            match output.try_recv() {
                Ok(chunk) => echoed.push_str(&String::from_utf8_lossy(&chunk)),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => break,
            }
        }
        assert!(
            echoed.contains("read_unread_messages"),
            "the live PTY must hear the nudge: {echoed:?}"
        );
    }

    /// The only test that can prove prompt delivery actually works.
    ///
    /// Everything else in this suite runs against a scripted harness, which by
    /// construction cannot tell a delivered prompt from one eaten by a startup
    /// dialog or shredded into per-line turns — that blind spot is exactly how
    /// three rounds of green suites hid a dispatch that delivered nothing. This
    /// spawns the REAL `claude` binary through the REAL adapter, in a fresh
    /// worktree-like directory (so the workspace-trust dialog is armed), with a
    /// deliberately MULTI-LINE prompt (so bracketed-paste framing is exercised),
    /// and asserts the agent acted on the whole prompt.
    ///
    /// Ignored by default: it needs `claude` installed, authenticated, and a
    /// network round trip, none of which belong in `cargo test`. Run it by hand
    /// after touching anything in the spawn path:
    ///
    /// ```text
    /// cargo test --lib real_claude -- --ignored --nocapture
    /// BUILD_E2E_TIMING=1     # timestamp every chunk, flag paste-mode/alt-screen
    /// BUILD_E2E_TRANSCRIPT=/tmp/e2e.txt   # dump the full raw stream
    /// BUILD_E2E_WAIT=45      # shorten the wait while iterating
    /// ```
    ///
    /// STATUS: currently FAILS against claude 2.1.219, and that failure is real
    /// — warm-TUI dispatch does not deliver. What it has already established:
    ///   - Workspace trust is fixed. The dialog no longer appears in a brand-new
    ///     directory, so `pre_trust_worktree_for_claude` works.
    ///   - Readiness and settle are necessary but not sufficient. With
    ///     REAL_TUI_SETTLE the write now lands ~750ms after the final startup
    ///     paint (measured: last paint 2716ms, write 3466ms) instead of into the
    ///     alternate-screen clear.
    ///   - The prompt text never appears on screen at all, and the TUI emits
    ///     ZERO output for the following two minutes. A composer receiving
    ///     keystrokes would redraw, so the remaining fault is below paste
    ///     framing and below the submit key — the bytes are not reaching
    ///     claude's input reader. Cause not yet identified.
    ///
    /// Do not treat the warm-TUI path as working until this passes.
    ///
    /// Shortened via BUILD_E2E_WAIT while iterating on the spawn path.
    fn e2e_wait() -> Duration {
        Duration::from_secs(
            std::env::var("BUILD_E2E_WAIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(180),
        )
    }

    #[test]
    #[ignore = "spawns the real claude binary; needs auth + network"]
    fn real_claude_session_receives_the_whole_multiline_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("fresh-worktree");
        std::fs::create_dir_all(&workspace).unwrap();
        // The adapter passes --mcp-config .build/mcp.json --strict-mcp-config,
        // so the file must exist or claude exits before reading a byte of the
        // prompt. Real dispatch scaffolds this (Orchestrator::scaffold_build_dir);
        // mirror the shape here. The server is never called — this test asserts
        // prompt DELIVERY, not the done round trip.
        let build_dir = workspace.join(".build");
        std::fs::create_dir_all(&build_dir).unwrap();
        std::fs::write(
            build_dir.join("mcp.json"),
            serde_json::to_vec_pretty(&json!({ "mcpServers": {} })).unwrap(),
        )
        .unwrap();

        // The marker is split across prompt LINES on purpose: only a prompt that
        // arrived as one turn can reassemble it. A prompt submitted line-by-line
        // leaves the agent acting on a fragment, which is the exact production
        // failure this guards.
        let prompt = "You are being driven by an automated test.\n\
             Do exactly this and nothing else, then stop.\n\
             \n\
             Create a file named `handshake.txt` in the current directory.\n\
             Its only contents must be these two words joined by a hyphen:\n\
             first word: BUILD\n\
             second word: DELIVERED\n\
             \n\
             So the file contains exactly: BUILD-DELIVERED\n";

        let Agent::WarmBuilder(build) = build_agent(false, "/tmp/unused-e2e.sock".into()) else {
            panic!("real agent should be a provider-aware warm TUI");
        };
        let choice = ModelChoice {
            provider: AgentProvider::Claude,
            model: Some("haiku".into()),
            effort: None,
        };
        let options = SpawnOptions {
            continue_session: false,
            owner_id: "e2e".into(),
            cwd: workspace.clone(),
        };
        // Building the spec is what pre-trusts the workspace — the dialog this
        // guards against fires precisely because the directory is brand new.
        let spec = build(prompt, &choice, &options);

        // The three lines under test, mirroring ensure_agent_tab + deliver.
        let session = PtySession::spawn(
            &spec,
            Some(workspace.clone()),
            PtySize {
                rows: 40,
                cols: 120,
                pixel_width: 0,
                pixel_height: 0,
            },
        )
        .expect("claude should spawn — is it installed and on PATH?");
        // Capture the session so a failure reports what the harness actually did
        // — a trust dialog, an argv rejection and an unsubmitted prompt all look
        // identical from the filesystem alone.
        let mut output = session.subscribe();
        let transcript = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        {
            let transcript = std::sync::Arc::clone(&transcript);
            let started = std::time::Instant::now();
            std::thread::spawn(move || {
                while let Ok(chunk) = output.blocking_recv() {
                    let text = String::from_utf8_lossy(&chunk).into_owned();
                    if std::env::var("BUILD_E2E_TIMING").is_ok() {
                        eprintln!(
                            "[{:>6}ms] {:>5}B{}{}",
                            started.elapsed().as_millis(),
                            chunk.len(),
                            if text.contains("\u{1b}[?2004h") {
                                " PASTE-MODE"
                            } else {
                                ""
                            },
                            if text.contains("\u{1b}[?1049h") {
                                " ALT-SCREEN"
                            } else {
                                ""
                            },
                        );
                    }
                    transcript.lock().unwrap().push_str(&text);
                }
            });
        }

        let ready = session.ready_within(Duration::from_secs(30));
        let written = session.write_prompt(prompt);

        let handshake = workspace.join("handshake.txt");
        let deadline = std::time::Instant::now() + e2e_wait();
        while std::time::Instant::now() < deadline {
            if std::fs::read_to_string(&handshake)
                .is_ok_and(|body| body.contains("BUILD-DELIVERED"))
            {
                session.kill_and_reap();
                return;
            }
            if session.has_exited() {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }

        let observed = std::fs::read_to_string(&handshake).unwrap_or_default();
        session.kill_and_reap();
        let seen = transcript.lock().unwrap().clone();
        if let Ok(dump) = std::env::var("BUILD_E2E_TRANSCRIPT") {
            let _ = std::fs::write(&dump, &seen);
        }
        panic!(
            "the agent never acted on the delivered prompt.\n\
             ready={ready} write={written:?} handshake={observed:?}\n\
             Either the prompt landed in a startup dialog, was never submitted, \
             or arrived as fragmented turns.\n\
             ---- harness output ----\n{}\n---- end ----",
            &seen[seen.len().saturating_sub(4000)..]
        );
    }

    /// A post at a review gate reaches the agent, and still moves nothing.
    ///
    /// This used to assert the opposite — that a parked harness was left alone,
    /// because waking it produced work whose `done` is an illegal transition
    /// from `review`. That reasoning died twice over: an out-of-phase `done` is
    /// now RECORDED rather than rejected, and the agent no longer parks at all —
    /// it is live in the Agent tab the human is typing into. Withholding the
    /// message made the conversation lie about itself, which is worse than a
    /// report that moves nothing. What must still hold is everything else: no
    /// respawn, no state change, and durability either way.
    #[test]
    fn thread_post_at_a_review_gate_reaches_the_agent_without_moving_the_run() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let run_id = "run-parked".to_string();
        let key = insert_run_with_agent_tab(
            &mut state,
            &repo,
            &dir.path().join("side"),
            &run_id,
            RunState::Review,
            warm_tui_spec(),
        );
        assert!(
            !state.tabs[&key].session.has_exited(),
            "precondition: the agent is still live at the gate"
        );
        let mut output = state.tabs[&key].session.subscribe();

        let posted = state.handle(req(
            "thread.post",
            json!({ "entity_id": run_id, "body": "a note for later" }),
        ));
        assert_eq!(posted["ok"], true, "{posted:?}");

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut echoed = String::new();
        while std::time::Instant::now() < deadline && !echoed.contains("read_unread_messages") {
            match output.try_recv() {
                Ok(chunk) => echoed.push_str(&String::from_utf8_lossy(&chunk)),
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => break,
            }
        }
        assert!(
            echoed.contains("read_unread_messages"),
            "the agent the human is looking at must hear them: {echoed:?}"
        );
        assert_eq!(
            state.runs[&run_id].run.state,
            RunState::Review,
            "hearing a message is not a state transition"
        );
        assert!(
            !state.tabs[&key].session.has_exited(),
            "the agent is talked to, never replaced"
        );
        // Durable regardless: the next session's catch-up carries it.
        let unread = state
            .on_mcp_action(&run_id, BridgeAction::ReadUnreadMessages)
            .unwrap();
        assert_eq!(unread["messages"][0]["body"], "a note for later");
    }

    /// Abandon must leave no agent behind. Worktree removal is best-effort by
    /// contract — a leftover worktree is logged, never a reason to fail the
    /// abandon — so when it fails the worktree stays on disk and the orphan
    /// reaper (which only sweeps tabs whose root is GONE) never fires. The kill
    /// has to be the abandon's own, or the human is left paying for an agent
    /// working on something they abandoned.
    #[test]
    fn abandon_closes_the_agent_even_when_the_worktree_survives() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (_, run_id) = planned_run_in_review(&mut state, "abandon me");
        let worktree = state.runs[&run_id].worktree.path.clone();
        let root = AppState::canonical_root(&worktree);
        let (tab, _rx) = Tab::spawn(
            TabRole::Agent {
                owner: run_id.clone(),
                provider: AgentProvider::default(),
            },
            &HarnessSpec::new("cat"),
            AGENT_TAB_ID.to_string(),
            root.clone(),
            120,
            40,
        )
        .expect("the agent tab spawns");
        let agent_pid = tab.session.pid().expect("the agent has a pid");
        state.tabs.insert(TabKey::agent(&root), tab);
        state.runs.get_mut(&run_id).unwrap().thread.start_session(
            "claude",
            None,
            None,
            "build",
            &now_rfc3339(),
        );

        // Cleanup will fail before it touches the worktree: the orchestrator's
        // repo is not a repo, so `remove` errors on the very first step and the
        // worktree survives the abandon.
        state.projects[0].orch = Orchestrator::new(
            dir.path().join("not-a-repo"),
            dir.path().join("wt"),
            Agent::Warm(HarnessSpec::new("true")),
            Templates::default(),
        );

        let abandoned = state.handle(req("run.abandon", json!({ "run_id": run_id })));
        assert_eq!(abandoned["result"]["state"], "abandoned", "{abandoned:?}");
        assert!(
            worktree.exists(),
            "this test is only meaningful while the failed cleanup leaves the worktree behind"
        );
        assert!(
            !state.tabs.contains_key(&TabKey::agent(&root)),
            "an abandoned run's agent is gone from the registry"
        );
        assert!(
            process_reaped(agent_pid),
            "an abandoned run's agent process is killed and reaped"
        );
        let session = state.runs[&run_id]
            .thread
            .sessions
            .last()
            .expect("the run had a session");
        assert!(
            session.ended_at.is_some(),
            "abandon ends the session it just killed: {session:?}"
        );
    }

    /// Only entities with no meaningful conversation left refuse a post:
    /// terminal states and unknown ids.
    #[test]
    fn thread_post_refuses_terminal_and_unknown_entities() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());

        let (_, run_id) = planned_run_in_review(&mut state, "goes away");
        state.handle(req("run.abandon", json!({ "run_id": run_id })));
        let refused = state.handle(req(
            "thread.post",
            json!({ "entity_id": run_id, "body": "anyone home?" }),
        ));
        assert_eq!(refused["ok"], false, "{refused:?}");
        assert!(
            refused["error"].as_str().unwrap().contains("abandoned"),
            "{refused:?}"
        );

        let (_, archived_id) = planned_run_in_review(&mut state, "swept away");
        state.runs.get_mut(&archived_id).unwrap().run.state = RunState::Archived;
        let refused_archived = state.handle(req(
            "thread.post",
            json!({ "entity_id": archived_id, "body": "anyone home?" }),
        ));
        assert_eq!(refused_archived["ok"], false, "{refused_archived:?}");
        assert!(
            refused_archived["error"]
                .as_str()
                .unwrap()
                .contains("archived"),
            "{refused_archived:?}"
        );

        let plan = state.handle(req("plan.create", json!({ "goal": "dropped plan" })));
        let plan_id = plan_id_of(&plan);
        state.handle(req("plan.abandon", json!({ "plan_id": plan_id })));
        let refused_plan = state.handle(req(
            "thread.post",
            json!({ "entity_id": plan_id, "body": "anyone home?" }),
        ));
        assert_eq!(refused_plan["ok"], false, "{refused_plan:?}");
        assert!(
            refused_plan["error"]
                .as_str()
                .unwrap()
                .contains("abandoned"),
            "{refused_plan:?}"
        );

        let unknown = state.handle(req(
            "thread.post",
            json!({ "entity_id": "nope", "body": "hi" }),
        ));
        assert_eq!(unknown["ok"], false, "{unknown:?}");
        assert!(
            unknown["error"]
                .as_str()
                .unwrap()
                .contains("unknown conversation owner"),
            "{unknown:?}"
        );
    }

    /// A stage awaiting its validation verdict refuses the dispatching verb
    /// (`run.message`) but must NOT block a post-only write.
    #[test]
    fn thread_post_is_not_blocked_by_a_stage_awaiting_validation() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (_, run_id) = planned_run_in_review(&mut state, "stage gate wait");
        let active = state.runs.get_mut(&run_id).unwrap();
        active.run.state = RunState::Building;
        active.current_stage_id = Some("stage-1".into());
        active.stages = vec![StageProgress {
            stage_id: "stage-1".into(),
            state: StageProgressState::Built,
            start_sha: None,
            built_sha: None,
            completion_sha: None,
            publication: crate::run::StagePublication::Local,
            invalidation_reason: None,
            validation: None,
        }];

        let dispatching = state.handle(req(
            "run.message",
            json!({ "run_id": run_id, "message": "hurry it up" }),
        ));
        assert_eq!(dispatching["ok"], false, "{dispatching:?}");
        assert!(
            dispatching["error"]
                .as_str()
                .unwrap()
                .contains("awaiting validation"),
            "{dispatching:?}"
        );

        let posted = state.handle(req(
            "thread.post",
            json!({ "entity_id": run_id, "body": "for the record" }),
        ));
        assert_eq!(posted["ok"], true, "{posted:?}");
        assert!(
            posted["result"]["thread"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["data"]["body"] == "for the record"),
            "{posted:?}"
        );
    }

    /// The plan review gate refuses `plan.message` (the dispatching verb) but
    /// accepts a post; a mismatched anchor artifact is rejected by the shared
    /// validator.
    #[test]
    fn thread_post_reaches_a_plan_at_its_review_gate() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let plan = state.handle(req("plan.create", json!({ "goal": "gate keeping" })));
        let plan_id = plan_id_of(&plan);
        assert_eq!(plan["result"]["state"], "plan_review", "{plan:?}");

        let dispatching = state.handle(req(
            "plan.message",
            json!({ "plan_id": plan_id, "message": "psst" }),
        ));
        assert_eq!(dispatching["ok"], false, "{dispatching:?}");

        let posted = state.handle(req(
            "thread.post",
            json!({ "entity_id": plan_id, "body": "a note at the gate" }),
        ));
        assert_eq!(posted["ok"], true, "{posted:?}");
        assert_eq!(posted["result"]["state"], "plan_review", "{posted:?}");
        let items = posted["result"]["thread"]["items"].as_array().unwrap();
        let message = items
            .iter()
            .find(|item| item["data"]["body"] == "a note at the gate")
            .unwrap_or_else(|| panic!("posted message missing: {posted:?}"));
        assert_eq!(message["data"]["role"], "user", "{message:?}");
        assert!(message["data"].get("seen_at").is_none(), "{message:?}");

        let bad_anchor = state.handle(req(
            "thread.post",
            json!({
                "entity_id": plan_id,
                "body": "anchored wrong",
                "anchor": { "artifact": "diff", "heading_path": ["A"], "snippet": "x" }
            }),
        ));
        assert_eq!(bad_anchor["ok"], false, "{bad_anchor:?}");
        assert!(
            bad_anchor["error"]
                .as_str()
                .unwrap()
                .contains("anchor artifact must be plan"),
            "{bad_anchor:?}"
        );
    }

    /// An approved single-doc plan on a side orchestrator, played by hand (write
    /// the doc, report `done(phase=plan)`, approve) — the app-level twin of the
    /// orchestrator's own `approved_plan` fixture. Runs only ever implement a
    /// plan, so a test that needs a run driven by a PARTICULAR harness builds a
    /// plan on that harness's orchestrator first.
    fn approved_side_plan(orch: &Orchestrator, store: &Store, id: &str) -> ActivePlan {
        let (mut plan, _turn) = orch
            .dispatch_plan(PlanId::new(id), "side goal", "main", Default::default())
            .unwrap();
        let worktree = plan
            .worktree
            .as_ref()
            .expect("the plan has a planning worktree")
            .path
            .clone();
        std::fs::write(worktree.join(".build/plan.md"), "# Plan\n").unwrap();
        orch.on_plan_done(
            &mut plan,
            store,
            DoneReport {
                phase: DonePhase::Plan,
                status: DoneStatus::Completed,
                summary: "planned".to_string(),
                outputs: DoneOutputs {
                    plan_path: Some(".build/plan.md".to_string()),
                    ..DoneOutputs::default()
                },
            },
        )
        .unwrap();
        orch.approve_plan(&mut plan).unwrap();
        plan
    }

    /// A run built on a side orchestrator (its plan lives there, not in
    /// `state`), installed under `state`'s first project with `run_state`
    /// stamped on. No process: a worktree's agent lives on the tab registry, so
    /// a test that needs one asks for [`insert_run_with_agent_tab`].
    fn insert_run(
        state: &mut AppState,
        repo: &std::path::Path,
        side_root: &std::path::Path,
        run_id: &str,
        run_state: RunState,
    ) -> std::path::PathBuf {
        let store = crate::store::Store::new(side_root.join("store"));
        let side = Orchestrator::new(
            repo.to_path_buf(),
            side_root.join("wt"),
            Agent::Warm(HarnessSpec::new("true")),
            Templates::default(),
        );
        let plan = approved_side_plan(&side, &store, &format!("plan-of-{run_id}"));
        let (mut active, _turn) = side
            .dispatch_run(
                RunId::new(run_id),
                RunSource {
                    plan: &plan,
                    has_active_run: false,
                },
                "main",
                Default::default(),
                &store,
            )
            .unwrap();
        active.run.state = run_state;
        let root = AppState::canonical_root(&active.worktree.path);
        let project_id = state.projects[0].id.clone();
        state.entity_project.insert(run_id.to_string(), project_id);
        state.runs.insert(run_id.to_string(), active);
        root
    }

    /// Such a run PLUS the live agent tab its worktree owns, running `spec`.
    /// Returns the tab's key. This is the shape the daemon actually holds: the
    /// run carries lifecycle and thread, the worktree carries the process.
    fn insert_run_with_agent_tab(
        state: &mut AppState,
        repo: &std::path::Path,
        side_root: &std::path::Path,
        run_id: &str,
        run_state: RunState,
        spec: HarnessSpec,
    ) -> TabKey {
        let root = insert_run(state, repo, side_root, run_id, run_state);
        let (tab, _rx) = Tab::spawn(
            TabRole::Agent {
                owner: run_id.to_string(),
                provider: AgentProvider::default(),
            },
            &spec,
            AGENT_TAB_ID.to_string(),
            root.clone(),
            120,
            40,
        )
        .expect("the agent tab spawns");
        let key = TabKey::agent(&root);
        state.tabs.insert(key.clone(), tab);
        key
    }

    /// The warm stand-in for a real TUI: it enables bracketed-paste mode (so a
    /// prompt write's readiness wait resolves) and drains stdin forever.
    fn warm_tui_spec() -> HarnessSpec {
        HarnessSpec::new("sh")
            .arg("-c")
            .arg("printf '\\033[?2004h'; cat >/dev/null")
    }

    /// Silence is an anomaly only when measured from the last thing Build
    /// asked. A tab's agent outlives every phase and idles at a prompt between
    /// them, so raw PTY silence would demote a run the instant it is
    /// re-dispatched after a long quiet review — the agent has said nothing for
    /// an hour because nobody spoke to it.
    #[test]
    fn an_agent_is_only_quiet_if_it_has_been_silent_since_the_last_turn() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let key = insert_run_with_agent_tab(
            &mut state,
            &repo,
            dir.path(),
            "run-quiet",
            RunState::Building,
            warm_tui_spec(),
        );
        // Long enough that the PTY has been silent past the threshold below.
        std::thread::sleep(Duration::from_millis(200));

        state.tabs.get_mut(&key).unwrap().last_delivered_at = Some(std::time::Instant::now());
        assert!(
            state.mark_idle_tasks(Duration::from_millis(50)).is_empty(),
            "an agent that was just given a turn is working, not quiet"
        );
        assert_eq!(state.runs["run-quiet"].run.state, RunState::Building);

        state.tabs.get_mut(&key).unwrap().last_delivered_at =
            Some(std::time::Instant::now() - Duration::from_secs(1));
        assert_eq!(
            state.mark_idle_tasks(Duration::from_millis(50)),
            vec!["run-quiet".to_string()],
            "silence that outlasts the turn that provoked it is an anomaly"
        );
    }

    /// Wait until a tab's PTY has been silent for `quiet` — the harness has
    /// finished echoing whatever it was just handed. A test that then backdates
    /// the stamp is not racing the reader thread for the last word on when this
    /// terminal painted.
    async fn wait_for_pty_quiet(state: &Arc<Mutex<AppState>>, key: &TabKey, quiet: Duration) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let idle = state.lock().unwrap().tabs[key].session.idle_for();
            if idle >= quiet {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the agent's PTY never settled"
            );
            tokio::time::sleep(quiet - idle).await;
        }
    }

    /// And the clock the rule reads is started by DELIVERY itself.
    ///
    /// Nothing else can start it: the agent tab outlives every phase, so the
    /// only moment that means "you now owe an answer" is the moment Build
    /// submitted a turn. If a delivery left the stamp alone, an agent handed a
    /// long job would be demoted the first time it thought quietly for longer
    /// than the threshold — silence read as an anomaly when it is the work.
    #[tokio::test]
    async fn delivering_a_turn_starts_the_quiescence_clock() {
        let (dir, repo) = init_repo();
        let (state, _handler) = shared_qa_state_and_handler(&repo, dir.path());
        let root = {
            let mut s = state.lock().unwrap();
            insert_run(
                &mut s,
                &repo,
                dir.path(),
                "run-spoken-to",
                RunState::Building,
            )
        };

        let (_, spawned) = deliver(
            &state,
            &root,
            "run-spoken-to",
            &ModelChoice::default(),
            "get to work",
            "there is more",
        )
        .expect("the turn reaches an agent");
        assert_eq!(spawned, Spawned::Fresh, "the tab did not exist yet");

        let key = TabKey::agent(&root);
        // The tty echoes a written prompt back through the reader thread, so
        // wait for the PTY to go quiet before speaking about its silence.
        wait_for_pty_quiet(&state, &key, Duration::from_millis(200)).await;

        let mut s = state.lock().unwrap();
        // It has painted nothing since — it is chewing on what it was asked.
        s.tabs
            .get_mut(&key)
            .unwrap()
            .session
            .backdate_last_output(Duration::from_secs(600));
        assert!(
            s.mark_idle_tasks(Duration::from_secs(60)).is_empty(),
            "an agent Build has just spoken to is working, however quiet it is"
        );
        assert_eq!(s.runs["run-spoken-to"].run.state, RunState::Building);

        // Control: the run was demotable all along — it is the turn's stamp,
        // and only that, holding it up.
        s.tabs.get_mut(&key).unwrap().last_delivered_at =
            Some(std::time::Instant::now() - Duration::from_secs(600));
        assert_eq!(
            s.mark_idle_tasks(Duration::from_secs(60)),
            vec!["run-spoken-to".to_string()],
            "silence that outlasts the turn that provoked it is an anomaly"
        );
    }

    /// A quiet agent is still an agent. The idle sweep records WHY an entity
    /// went quiet, and that record must not also claim the session ended: the
    /// process is sitting at its prompt, and the next turn continues the very
    /// session the thread would have closed.
    #[test]
    fn an_idle_demotion_leaves_the_live_agents_session_open() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let key = insert_run_with_agent_tab(
            &mut state,
            &repo,
            dir.path(),
            "run-still-there",
            RunState::Building,
            warm_tui_spec(),
        );
        state
            .runs
            .get_mut("run-still-there")
            .unwrap()
            .thread
            .start_session("claude", None, None, "build", &now_rfc3339());
        // Long enough that the PTY has been silent past the threshold below.
        std::thread::sleep(Duration::from_millis(200));
        state.tabs.get_mut(&key).unwrap().last_delivered_at =
            Some(std::time::Instant::now() - Duration::from_secs(1));

        assert_eq!(
            state.mark_idle_tasks(Duration::from_millis(50)),
            vec!["run-still-there".to_string()],
            "the quiet agent's entity is demoted"
        );
        assert!(
            !state.tabs[&key].session.has_exited(),
            "this test is only meaningful while the agent is still alive"
        );
        let thread = &state.runs["run-still-there"].thread;
        assert!(
            thread.sessions.last().unwrap().ended_at.is_none(),
            "a quiet agent is still in its session: {:?}",
            thread.sessions
        );
    }

    #[test]
    fn mark_idle_demotes_a_quiet_plan_and_run() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        // A Building run whose agent exits immediately → demoted, with the exit
        // code recorded (the quiescence rule: silence is never completion).
        insert_run_with_agent_tab(
            &mut state,
            &repo,
            dir.path(),
            "run-idle",
            RunState::Building,
            HarnessSpec::new("sh").arg("-c").arg("exit 7"),
        );
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

    /// A turn addressed to a worktree that cannot host an agent — the scaffold
    /// step fails on a path that is not a directory.
    fn unreachable_turn(owner: &str) -> PendingAgentTurn {
        PendingAgentTurn {
            root: std::path::PathBuf::from("/dev/null/there-is-no-worktree-here"),
            owner: owner.to_string(),
            model_choice: ModelChoice::default(),
            cold: "cold turn".into(),
            warm: "warm turn".into(),
            phase: "build",
        }
    }

    /// A delivery that never reached an agent used to be a silent `eprintln!`:
    /// the run had already transitioned to `Building` and been persisted, so it
    /// sat there working with nobody working, forever. The failure has to land
    /// on the entity where a surface can read it.
    #[test]
    fn a_delivery_that_never_reaches_an_agent_is_visible_on_its_entity() {
        let (dir, repo) = init_repo();
        let mut app = qa_state(&repo, dir.path());
        insert_run(
            &mut app,
            &repo,
            dir.path(),
            "run-unreachable",
            RunState::Building,
        );
        let state = app.shared();
        state
            .lock()
            .unwrap()
            .pending_agent_turns
            .push(unreachable_turn("run-unreachable"));

        deliver_pending_agent_turns(&state);

        let got = state
            .lock()
            .unwrap()
            .handle(req("run.get", json!({ "run_id": "run-unreachable" })));
        let last_error = got["result"]["last_error"].as_str().unwrap_or_default();
        assert!(
            last_error.contains("could not reach the agent"),
            "the failure must be legible on the run: {got:?}"
        );
        // And it is durable: the reason survives a re-read from the store.
        let record = state
            .lock()
            .unwrap()
            .store
            .as_ref()
            .unwrap()
            .load_all_runs()
            .unwrap()
            .into_iter()
            .find(|r| r.id == "run-unreachable")
            .expect("the run is persisted");
        assert!(
            record.last_error.unwrap_or_default().contains("agent"),
            "the failure must be persisted, not just held in memory"
        );
    }

    /// The idle sweep used to skip an entity with no agent tab (`let tab =
    /// tab?`), which is exactly the entity a failed delivery leaves behind.
    /// Build owns every agent and always keeps it as a tab, so a working entity
    /// with none is an anomaly, not an absence of evidence.
    #[test]
    fn a_working_run_with_no_agent_tab_is_an_anomaly_not_a_skip() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        insert_run(
            &mut state,
            &repo,
            dir.path(),
            "run-tabless",
            RunState::Building,
        );
        assert_eq!(
            state.mark_idle_tasks(Duration::from_secs(3600)),
            vec!["run-tabless".to_string()],
            "a working run with no agent at all must be demoted, not skipped"
        );
        let got = state.handle(req("run.get", json!({ "run_id": "run-tabless" })));
        assert_eq!(got["result"]["state"], "idle_unreported", "{got:?}");
        // No harness exited here, so no exit-code claim is invented.
        assert!(got["result"]["last_error"].is_null(), "{got:?}");
    }

    /// The plan half of the same hole. A plan's turns are queued and delivered
    /// by exactly the same path as a run's, so a planning agent that never
    /// starts must land on the PLAN the same way — `plan.get` says why, and the
    /// reason survives a restart.
    #[test]
    fn a_delivery_that_never_reaches_an_agent_is_visible_on_its_plan() {
        let (dir, repo) = init_repo();
        let mut app = qa_state(&repo, dir.path());
        let plan_id = plan_id_of(&app.handle(req("plan.create", json!({ "goal": "unreachable" }))));
        let state = app.shared();
        {
            let mut app = state.lock().unwrap();
            app.pending_agent_turns.clear();
            app.pending_agent_turns.push(unreachable_turn(&plan_id));
        }

        deliver_pending_agent_turns(&state);

        let got = state
            .lock()
            .unwrap()
            .handle(req("plan.get", json!({ "plan_id": plan_id })));
        let last_error = got["result"]["last_error"].as_str().unwrap_or_default();
        assert!(
            last_error.contains("could not reach the agent"),
            "the failure must be legible on the plan: {got:?}"
        );
        let record = state
            .lock()
            .unwrap()
            .store
            .as_ref()
            .unwrap()
            .load_all_plans()
            .unwrap()
            .into_iter()
            .find(|p| p.id == plan_id)
            .expect("the plan is persisted");
        assert!(
            record.last_error.unwrap_or_default().contains("agent"),
            "the failure must be persisted, not just held in memory"
        );
    }

    /// The plan half of the tabless anomaly: a drafting plan whose agent never
    /// arrived is demoted by the sweep, and one whose turn is still queued is
    /// left alone.
    #[test]
    fn a_working_plan_with_no_agent_tab_is_an_anomaly_not_a_skip() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let plan_id = plan_id_of(&state.handle(req("plan.create", json!({ "goal": "tabless" }))));
        state.plans.get_mut(&plan_id).unwrap().plan.state = PlanState::Drafting;
        state.pending_agent_turns.clear();
        state.pending_agent_turns.push(unreachable_turn(&plan_id));
        assert!(
            state.mark_idle_tasks(Duration::from_secs(3600)).is_empty(),
            "a queued turn means the planning agent is coming, not missing"
        );

        state.pending_agent_turns.clear();
        assert_eq!(
            state.mark_idle_tasks(Duration::from_secs(3600)),
            vec![plan_id.clone()],
            "a drafting plan with no agent at all must be demoted, not skipped"
        );
        let got = state.handle(req("plan.get", json!({ "plan_id": plan_id })));
        assert_eq!(got["result"]["state"], "idle_unreported", "{got:?}");
        // No harness exited here, so no exit-code claim is invented.
        assert!(got["result"]["last_error"].is_null(), "{got:?}");
    }

    /// The tabless anomaly must not fire on the gap the queue opens: a verb
    /// transitions the run under the state lock and the turn is delivered after
    /// it, so for the seconds a cold spawn takes there is a working run whose
    /// agent is legitimately still on its way.
    #[test]
    fn a_run_whose_turn_is_still_on_its_way_is_not_demoted() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        insert_run(
            &mut state,
            &repo,
            dir.path(),
            "run-dispatching",
            RunState::Building,
        );
        state
            .pending_agent_turns
            .push(unreachable_turn("run-dispatching"));
        assert!(
            state.mark_idle_tasks(Duration::from_secs(3600)).is_empty(),
            "a queued turn means the agent is coming, not missing"
        );

        // Mid-delivery — off the queue, not yet a tab — is the same story.
        state.pending_agent_turns.clear();
        *state
            .agent_turns_in_flight
            .entry("run-dispatching".into())
            .or_default() += 1;
        assert!(
            state.mark_idle_tasks(Duration::from_secs(3600)).is_empty(),
            "a turn mid-delivery means the agent is coming, not missing"
        );

        // Once the delivery is over and no tab appeared, it IS the anomaly.
        state.agent_turns_in_flight.clear();
        assert_eq!(
            state.mark_idle_tasks(Duration::from_secs(3600)),
            vec!["run-dispatching".to_string()]
        );
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
            provider: AgentProvider::Claude,
            model: None,
            effort: None,
            thread: crate::thread::Thread::new(id),
            last_summary: None,
            last_error: None,
            created_at: "2026-07-01T10:00:00Z".into(),
            updated_at: "2026-07-01T10:00:00Z".into(),
            state_changed_at: None,
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
        // The run half takes a derived, disjoint id — plan and run ids never
        // collide, or done-report routing (plans-first) would swallow the
        // run's reports.
        let built_run = state.handle(req("run.get", json!({ "run_id": "run-task-build" })));
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
        assert!(
            run_ids.contains(&"run-task-build".to_string()),
            "{run_ids:?}"
        );
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
            archived_at: None,
            implementation_intent: crate::plan::ImplementationIntent::None,
            implementation_activity: crate::plan::ImplementationActivity::Idle,
            worktree_name: Some(id.into()),
            worktree_path: Some(worktree.display().to_string()),
            branch: Some(format!("plan/{id}")),
            plan_path: ".build/plan.md".into(),
            stages: Vec::new(),
            comments: Vec::new(),
            provider: AgentProvider::Claude,
            model: None,
            effort: None,
            thread: crate::thread::Thread::new(id),
            last_summary: None,
            last_error: None,
            created_at: "2026-07-01T10:00:00Z".into(),
            updated_at: "2026-07-01T10:00:00Z".into(),
            state_changed_at: None,
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
            provider: AgentProvider::Claude,
            model: None,
            effort: None,
            thread: crate::thread::Thread::new(id),
            last_summary: None,
            last_error: None,
            created_at: "2026-07-01T10:00:00Z".into(),
            updated_at: "2026-07-01T10:00:00Z".into(),
            state_changed_at: None,
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
        // Build's agent in that worktree reports `done` for THIS run.
        let root = AppState::canonical_root(&state.runs[&run_id].worktree.path);
        let (tab, _rx) = Tab::spawn(
            TabRole::Agent {
                owner: run_id.clone(),
                provider: AgentProvider::default(),
            },
            &warm_tui_spec(),
            AGENT_TAB_ID.to_string(),
            root.clone(),
            120,
            40,
        )
        .unwrap();
        let agent_pid = tab.session.pid().expect("a live agent");
        state.tabs.insert(TabKey::agent(&root), tab);

        // Release drops the record, keeps the files.
        let released = state.handle(req("run.release", json!({ "run_id": run_id })));
        assert_eq!(released["ok"], true, "{released:?}");
        assert!(!state.runs.contains_key(&run_id));
        assert!(
            root.exists(),
            "un-adopting must never touch the user's files"
        );
        // …and takes Build's agent with it: an agent whose owner is gone would
        // report `done` into the unknown-entity log forever.
        assert!(
            !state.tabs.contains_key(&TabKey::agent(&root)),
            "releasing a run closes the agent it owned"
        );
        assert!(process_reaped(agent_pid), "the agent is killed AND reaped");
    }

    #[test]
    fn adopted_review_run_is_finishable_from_the_rail() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let run_id = adopted_run(&mut state, &repo, dir.path(), "finishable-run");

        let board = state.handle(req("board.list", json!({})));
        let run = board["result"]["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["run_id"] == run_id)
            .unwrap();
        assert_eq!(run["state"], "review", "{run:?}");
        assert_eq!(run["can_finish"], true, "{run:?}");
        assert_eq!(run["stat"]["branch"], "finishable-run", "{run:?}");
    }

    #[test]
    fn run_finish_cleans_up_and_archives_the_bound_worktree() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let run_id = adopted_run(&mut state, &repo, dir.path(), "archive-finished-run");
        let project_id = state.projects[0].id.clone();
        let worktree = state.runs[&run_id].worktree.path.clone();
        git_in_dir(&worktree, &["add", "-A"]);
        git_in_dir(&worktree, &["commit", "-m", "Finish adopted work"]);

        let finished = state.handle(req(
            "run.finish",
            json!({ "run_id": run_id, "action": "cleanup" }),
        ));
        assert_eq!(finished["ok"], true, "{finished:?}");
        assert!(!state.runs.contains_key(&run_id));
        assert!(!worktree.exists(), "Done removes the finished checkout");
        assert!(
            repo.join(".git/refs/heads/archive-finished-run").exists(),
            "cleanup preserves the branch"
        );

        let archive = state.handle(req("archive.list", json!({ "project_id": project_id })));
        let archived = archive["result"]["worktrees"].as_array().unwrap();
        assert_eq!(archived.len(), 1, "{archive:?}");
        assert_eq!(archived[0]["action"], "cleanup", "{archive:?}");
    }

    #[test]
    fn finishing_a_planned_run_keeps_archived_lineage_for_plan_done() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (plan_id, run_id) = planned_run_in_review(&mut state, "archive planned run");
        let worktree = state.runs[&run_id].worktree.path.clone();
        git_in_dir(&worktree, &["add", "-A"]);
        let staged = git_stdout(&worktree, &["diff", "--cached", "--name-only"]).unwrap();
        if !staged.trim().is_empty() {
            git_in_dir(&worktree, &["commit", "-m", "Finish planned work"]);
        }

        let finished = state.handle(req(
            "run.finish",
            json!({ "run_id": run_id, "action": "cleanup" }),
        ));
        assert_eq!(finished["ok"], true, "{finished:?}");
        assert_eq!(state.runs[&run_id].run.state, RunState::Archived);
        assert!(!worktree.exists());

        let plan = state.handle(req("plan.get", json!({ "plan_id": plan_id })));
        assert_eq!(plan["result"]["can_archive"], true, "{plan:?}");
    }

    #[test]
    fn merged_run_with_no_checkout_still_gets_done_to_leave_the_rail() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (_, run_id) = planned_run_in_review(&mut state, "dismiss merged run");
        let merged = state.handle(req(
            "run.git_action",
            json!({ "run_id": run_id, "action": "merge" }),
        ));
        assert_eq!(merged["result"]["state"], "merged", "{merged:?}");
        assert!(!state.runs[&run_id].worktree.path.exists());

        let board = state.handle(req("board.list", json!({})));
        let run = board["result"]["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|run| run["run_id"] == run_id)
            .unwrap();
        assert_eq!(run["can_finish"], true, "{run:?}");

        let finished = state.handle(req(
            "run.finish",
            json!({ "run_id": run_id, "action": "cleanup" }),
        ));
        assert_eq!(finished["ok"], true, "{finished:?}");
        assert_eq!(state.runs[&run_id].run.state, RunState::Archived);
        let board = state.handle(req("board.list", json!({})));
        assert!(board["result"]["runs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|run| run["run_id"] != run_id));
    }

    #[test]
    fn failed_run_finish_restores_the_live_run_for_retry() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let run_id = adopted_run(&mut state, &repo, dir.path(), "retry-finish-run");
        let worktree = state.runs[&run_id].worktree.path.clone();

        // QA adoption writes .build/.gitignore, so cleanup must refuse this
        // dirty tree before any destructive step.
        let failed = state.handle(req(
            "run.finish",
            json!({ "run_id": run_id, "action": "cleanup" }),
        ));
        assert_eq!(failed["ok"], false, "{failed:?}");
        assert!(failed["error"]
            .as_str()
            .unwrap()
            .contains("requires no uncommitted"));
        assert!(state.runs.contains_key(&run_id), "the rail entry survives");
        assert!(worktree.exists(), "the checkout survives");
    }

    #[test]
    fn run_delete_is_terminal_only() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (_, run_id) = planned_run_in_review(&mut state, "live");
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
        let (_, run_id) = planned_run_in_review(&mut state, "keep it");
        let worktree = state.runs.get(&run_id).unwrap().worktree.path.clone();
        let merged = state.handle(req(
            "run.git_action",
            json!({ "run_id": run_id, "action": "merge", "cleanup": "keep" }),
        ));
        assert_eq!(merged["result"]["state"], "merged");
        assert!(worktree.exists(), "cleanup=keep keeps the worktree");
    }

    // ---- terminals + agent screens over runs ---------------------------------

    /// A run in `state` whose WORKTREE has a live agent tab, painting steadily
    /// — the shape an attaching client meets. Returns the tab's key and the
    /// wire id it is addressed by.
    fn insert_live_run(
        state: &Arc<Mutex<AppState>>,
        repo: &std::path::Path,
        side_root: std::path::PathBuf,
        run_id: &str,
    ) -> (TabKey, String) {
        let store = crate::store::Store::new(side_root.join("store"));
        let side = Orchestrator::new(
            repo.to_path_buf(),
            side_root.join("wt"),
            Agent::Warm(HarnessSpec::new("true")),
            Templates::default(),
        );
        let plan = approved_side_plan(&side, &store, &format!("plan-of-{run_id}"));
        let (active, _turn) = side
            .dispatch_run(
                RunId::new(run_id),
                RunSource {
                    plan: &plan,
                    has_active_run: false,
                },
                "main",
                Default::default(),
                &store,
            )
            .unwrap();
        let root = AppState::canonical_root(&active.worktree.path);
        let (tab, rx) = Tab::spawn(
            TabRole::Agent {
                owner: run_id.to_string(),
                provider: AgentProvider::default(),
            },
            &HarnessSpec::new("sh").arg("-c").arg(
                "printf '\\033[?2004h'; (while :; do echo agent-beat; sleep 0.05; done) & cat >/dev/null",
            ),
            AGENT_TAB_ID.to_string(),
            root.clone(),
            120,
            40,
        )
        .expect("the agent tab spawns");
        let key = TabKey::agent(&root);
        let wire_id = tab.wire_id();
        {
            let mut s = state.lock().unwrap();
            let project_id = s.projects[0].id.clone();
            s.entity_project.insert(run_id.to_string(), project_id);
            s.runs.insert(run_id.to_string(), active);
            s.tabs.insert(key.clone(), tab);
        }
        spawn_tab_pump(state, key.clone(), rx);
        (key, wire_id)
    }

    /// A run with a worktree but no agent tab — the state every worktree is in
    /// before anyone speaks to it, and the one a surface's "Start agent" button
    /// acts on. Returns the run's canonical root.
    fn insert_run_without_agent(
        state: &Arc<Mutex<AppState>>,
        repo: &std::path::Path,
        side_root: std::path::PathBuf,
        run_id: &str,
    ) -> std::path::PathBuf {
        let store = crate::store::Store::new(side_root.join("store"));
        let side = Orchestrator::new(
            repo.to_path_buf(),
            side_root.join("wt"),
            Agent::Warm(HarnessSpec::new("true")),
            Templates::default(),
        );
        let plan = approved_side_plan(&side, &store, &format!("plan-of-{run_id}"));
        let (active, _turn) = side
            .dispatch_run(
                RunId::new(run_id),
                RunSource {
                    plan: &plan,
                    has_active_run: false,
                },
                "main",
                Default::default(),
                &store,
            )
            .unwrap();
        let root = AppState::canonical_root(&active.worktree.path);
        let mut s = state.lock().unwrap();
        let project_id = s.projects[0].id.clone();
        s.entity_project.insert(run_id.to_string(), project_id);
        s.runs.insert(run_id.to_string(), active);
        root
    }

    /// `agent.start` is the surface's "Start agent" button: it opens the
    /// worktree's one agent WITHOUT a turn to deliver. Attaching never spawns
    /// (mounting a tab is a look), so before this verb the only way to get an
    /// agent was to send it work — which is no help when the human just wants
    /// the thing running, or wants it back after it exited.
    #[tokio::test]
    async fn agent_start_opens_the_worktrees_agent_and_is_idempotent() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-start");
        let key = TabKey::agent(&root);
        assert!(
            !state.lock().unwrap().tabs.contains_key(&key),
            "the worktree has no agent until someone asks for one"
        );

        let started = call(&handler, "agent.start", json!({ "id": "run-start" }));
        assert_eq!(started["ok"], true, "{started:?}");
        assert_eq!(
            started["result"]["spawned"], "fresh",
            "the first start opens the agent"
        );
        assert_eq!(started["result"]["live"], true);
        let wire_id = started["result"]["term_id"].as_str().unwrap().to_string();
        assert!(
            wire_id.starts_with("agent:"),
            "an agent is addressed by its worktree: {wire_id}"
        );
        let pid = {
            let s = state.lock().unwrap();
            s.tabs
                .get(&key)
                .expect("the agent tab exists")
                .session
                .pid()
        };

        let again = call(&handler, "agent.start", json!({ "id": "run-start" }));
        assert_eq!(again["result"]["spawned"], "warm", "{again:?}");
        assert_eq!(
            again["result"]["term_id"], wire_id,
            "a second start addresses the same tab"
        );
        assert_eq!(
            state.lock().unwrap().tabs.get(&key).unwrap().session.pid(),
            pid,
            "starting an agent that is already running must not spawn a second one"
        );
    }

    /// The restart case the human actually hits: the harness exited (codex ran
    /// its self-update and quit, claude crashed), the tab retains the dead
    /// screen, and the button has to bring a NEW process back on the same tab.
    #[tokio::test]
    async fn agent_start_restarts_an_agent_that_exited() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-restart");
        let key = TabKey::agent(&root);

        let first = call(&handler, "agent.start", json!({ "id": "run-restart" }));
        assert_eq!(first["ok"], true, "{first:?}");
        let first_pid = {
            let s = state.lock().unwrap();
            s.tabs.get(&key).unwrap().session.pid()
        };

        // The harness dies the way a real one does, and the tab is RETAINED so
        // the human can still read the last screen.
        {
            let mut s = state.lock().unwrap();
            let tab = s.tabs.get_mut(&key).unwrap();
            tab.session.kill_and_reap();
            tab.live = false;
        }

        let restarted = call(&handler, "agent.start", json!({ "id": "run-restart" }));
        assert_eq!(restarted["ok"], true, "{restarted:?}");
        assert_eq!(
            restarted["result"]["spawned"], "fresh",
            "a dead agent is replaced, not reported as running"
        );
        assert_eq!(restarted["result"]["live"], true);
        let s = state.lock().unwrap();
        let tab = s.tabs.get(&key).expect("the tab came back");
        assert!(tab.live, "the restarted agent is live");
        assert_ne!(
            tab.session.pid(),
            first_pid,
            "restart means a NEW process, not the corpse reported as alive"
        );
    }

    /// Starting an agent by hand must not strand what is already waiting for
    /// it. The reviewer's words are durable on the thread, and the ONLY way an
    /// agent learns of them is being told to call `read_unread_messages` — a
    /// fresh harness has no reason to. Without this, pressing Restart after a
    /// crash brings back an agent that silently ignores every message posted
    /// while it was down.
    #[tokio::test]
    async fn agent_start_tells_a_fresh_agent_what_is_waiting_for_it() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-waiting");
        {
            let mut s = state.lock().unwrap();
            let run = s.runs.get_mut("run-waiting").unwrap();
            run.thread
                .post_user("look at the migration", None, "2026-07-29T12:00:00Z");
        }

        let started = call(&handler, "agent.start", json!({ "id": "run-waiting" }));
        assert_eq!(started["ok"], true, "{started:?}");
        let screen = wait_for_agent_screen(&state, &root, "read_unread_messages").await;
        assert!(
            screen.contains("read_unread_messages"),
            "a started agent must be told to read what is waiting: {screen:?}"
        );
    }

    /// The other half: a start with nothing waiting says NOTHING. The button
    /// means "give me an agent", not "go do something" — the human drives it
    /// from there. An unsolicited prompt would put a fresh agent to work nobody
    /// asked it to do.
    #[tokio::test]
    async fn agent_start_says_nothing_when_nothing_is_waiting() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-quiet");

        let started = call(&handler, "agent.start", json!({ "id": "run-quiet" }));
        assert_eq!(started["ok"], true, "{started:?}");
        // Give a prompt every chance to appear before concluding none did.
        tokio::time::sleep(Duration::from_millis(400)).await;
        let screen = agent_screen_text(&state, &root);
        assert!(
            !screen.contains("read_unread_messages"),
            "an agent with nothing waiting must be left alone: {screen:?}"
        );
    }

    /// An id that owns no worktree cannot have an agent started in it — the
    /// MCP `done` route is scaffolded per owner, so there is nothing to own it.
    #[tokio::test]
    async fn agent_start_refuses_an_id_that_owns_no_worktree() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let _ = &state;
        let refused = call(&handler, "agent.start", json!({ "id": "run-nowhere" }));
        assert_eq!(refused["ok"], false, "{refused:?}");
        assert!(
            refused["error"].as_str().unwrap().contains("unknown id"),
            "{refused:?}"
        );
    }

    /// Attaching to an entity's agent finds the tab of the WORKTREE it works
    /// in, streams it, and — when that agent's process ends — retains the last
    /// screen with `live: false` rather than erroring or going blank.
    #[tokio::test]
    async fn agent_attach_streams_a_live_run_and_retains_the_last_screen() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let (tab_key, wire_id) = insert_live_run(&state, &repo, dir.path().join("side"), "run-9");

        let (sender, mut pushes, key) = SessionSender::observable("s1");
        let res = handler(
            sender,
            req(
                "agent.attach",
                json!({ "id": "run-9", "cols": 100, "rows": 30 }),
            ),
        );
        assert_eq!(res["ok"], true, "{res:?}");
        assert_eq!(
            res["result"]["term_id"], wire_id,
            "an agent is addressed by its worktree, not by the run that owns it"
        );
        assert_eq!(res["result"]["live"], true);

        let seen = wait_for_pushes(&mut pushes, &key, |seen| {
            output_text(seen, &wire_id).contains("agent-beat")
        })
        .await;
        assert_eq!(seen[0]["type"], "term.reset", "{seen:?}");

        // The agent's process ends → clients hear agent_session_ended and the
        // tab keeps showing the last screen.
        state.lock().unwrap().tabs[&tab_key].session.kill_and_reap();
        wait_for_push(&mut pushes, &key, |p| {
            p["type"] == "term.closed"
                && p["term_id"] == wire_id
                && p["reason"] == "agent_session_ended"
        })
        .await;

        let again = handler(
            SessionSender::detached("s2"),
            req("agent.attach", json!({ "id": "run-9" })),
        );
        assert_eq!(again["result"]["live"], false, "{again:?}");
        assert!(
            !b64decode(again["result"]["snapshot"].as_str().unwrap())
                .unwrap()
                .is_empty(),
            "a dead agent still shows what it last painted"
        );
    }

    /// The Agent tab is a fixture on every worktree surface, and most of those
    /// surfaces have no entity to name: an unadopted external worktree and the
    /// project's primary checkout are directories, not runs. So `agent.attach`
    /// takes the same scope shapes `term.create`/`term.list` take, resolves
    /// them server-side, and answers for the tab rooted there — empty when no
    /// agent has run, the live tab once one has.
    #[tokio::test]
    async fn agent_attach_addresses_a_worktree_by_scope_before_any_run_owns_it() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let project_id = state.lock().unwrap().projects[0].id.clone();
        add_external_worktree(&repo, dir.path(), "feature-x", "feature-x");
        let external = state
            .lock()
            .unwrap()
            .external_worktrees(&project_id, true)
            .unwrap()
            .into_iter()
            .find(|w| w.branch.as_deref() == Some("feature-x"))
            .expect("the external worktree is discoverable");

        // Nothing has ever run here: the tab exists as an empty screen, never
        // as an error — mounting it must not spawn anything.
        let empty = handler(
            SessionSender::detached("s1"),
            req(
                "agent.attach",
                json!({ "project_id": project_id, "worktree_id": external.id }),
            ),
        );
        assert_eq!(empty["ok"], true, "{empty:?}");
        assert_eq!(empty["result"]["live"], false);
        assert_eq!(
            empty["result"]["term_id"],
            json!(format!("agent:{}", external.id))
        );

        // The primary checkout answers to the project scope alone, the same way
        // its shells do.
        let primary = handler(
            SessionSender::detached("s1"),
            req("agent.attach", json!({ "project_id": project_id })),
        );
        assert_eq!(primary["ok"], true, "{primary:?}");
        assert_eq!(primary["result"]["live"], false);
        assert_eq!(
            primary["result"]["term_id"],
            json!(format!(
                "agent:{}",
                crate::worktree::external_worktree_id(&AppState::canonical_root(&repo))
            ))
        );

        // Once an agent runs in that worktree, the same scope reaches the tab
        // itself — one agent, one wire id, whichever shape asked for it.
        let root = AppState::canonical_root(&external.path);
        let (tab, rx) = Tab::spawn(
            TabRole::Agent {
                owner: "run-x".to_string(),
                provider: AgentProvider::default(),
            },
            &HarnessSpec::new("cat"),
            AGENT_TAB_ID.to_string(),
            root.clone(),
            120,
            40,
        )
        .expect("the agent tab spawns");
        let wire_id = tab.wire_id();
        let key = TabKey::agent(&root);
        state.lock().unwrap().tabs.insert(key.clone(), tab);
        spawn_tab_pump(&state, key.clone(), rx);

        let live = handler(
            SessionSender::detached("s2"),
            req(
                "agent.attach",
                json!({ "project_id": project_id, "worktree_id": external.id }),
            ),
        );
        assert_eq!(live["ok"], true, "{live:?}");
        assert_eq!(live["result"]["live"], true);
        assert_eq!(live["result"]["term_id"], json!(wire_id));
    }

    /// The Agent tab is a fixture on every worktree surface, so clients mount
    /// it long before anything has ever run there — the state EVERY worktree is
    /// in right after a daemon restart. Such a client is attached to a screen
    /// with no PTY, and when the agent finally starts it must go live WHERE IT
    /// STANDS: the session's first frames reach it without an unmount and
    /// remount. The viewport it attached at is the one the new PTY is sized to,
    /// the same rule a live attach follows.
    #[tokio::test]
    async fn a_client_attached_before_the_first_spawn_streams_the_session_it_waited_for() {
        let (dir, repo) = init_repo();
        let (state, handler, _) = agent_tab_fixture(&repo, dir.path(), "run-waited-for");
        let project_id = state.lock().unwrap().projects[0].id.clone();
        let wire_id = format!(
            "agent:{}",
            crate::worktree::external_worktree_id(&AppState::canonical_root(&repo))
        );

        // Nothing has ever run in the primary checkout: a blank, dead screen.
        let (sender, mut pushes, key) = SessionSender::observable("s1");
        let empty = handler(
            sender,
            req(
                "agent.attach",
                json!({ "project_id": project_id, "cols": 100, "rows": 30 }),
            ),
        );
        assert_eq!(empty["ok"], true, "{empty:?}");
        assert_eq!(empty["result"]["live"], false);
        assert_eq!(empty["result"]["term_id"], json!(wire_id));
        assert_eq!(
            empty["result"]["cursor"], 0,
            "a screen that has painted nothing starts the cursor at zero"
        );

        // The agent starts later, from a delivery. This client never re-attached.
        let (delivered_to, spawned) = deliver(
            &state,
            &repo,
            "run-waited-for",
            &ModelChoice::default(),
            "COLD-PROMPT-FOR-A-WAITING-CLIENT",
            "WARM-NUDGE",
        )
        .expect("the delivery spawns the worktree's agent");
        assert_eq!(spawned, Spawned::Fresh);
        assert_eq!(delivered_to, wire_id);

        let seen = wait_for_pushes(&mut pushes, &key, |seen| {
            output_text(seen, &wire_id).contains("COLD-PROMPT-FOR-A-WAITING-CLIENT")
        })
        .await;
        assert_eq!(
            seen[0]["type"], "term.reset",
            "the waiting client hears the session start: {seen:?}"
        );
        assert_eq!(
            seen[0]["cursor"], 0,
            "the opening reset lands at the cursor the attach handed out — \
             no gap, and the client applies it rather than deduping it away"
        );
        let mut cursor = 0;
        for push in &seen {
            let pushed = push["cursor"].as_u64().expect("every frame carries one");
            assert!(pushed >= cursor, "the cursor never rewinds: {seen:?}");
            cursor = pushed;
        }
        assert!(
            cursor > 0,
            "the session's output moved the cursor: {seen:?}"
        );

        let s = state.lock().unwrap();
        let screen = &s.tabs[&TabKey::agent(&AppState::canonical_root(&repo))].screen;
        assert_eq!(
            (screen.cols, screen.rows),
            (100, 30),
            "the spawned agent is sized to the viewport of the client already watching it"
        );
    }

    /// The window inside a respawn: the reservation has taken the dead tab out
    /// of the registry, so a client mounting the Agent tab right then lands on
    /// a waiting screen even though this worktree HAS a retained screen with a
    /// cursor. That client must be carried onto the replacement — but its
    /// screen must not be: the retained cursor is what reconnect dedupes on and
    /// it never rewinds, so the waiting screen contributes its clients and
    /// nothing else.
    #[tokio::test]
    async fn a_client_attaching_inside_a_respawn_is_carried_without_rewinding_the_cursor() {
        let (dir, repo) = init_repo();
        let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-respawn-race");
        let choice = ModelChoice::default();
        let canonical = AppState::canonical_root(&root);
        let key = TabKey::agent(&canonical);

        // A first session paints, then dies: its screen and cursor are retained.
        deliver(
            &state,
            &root,
            "run-respawn-race",
            &choice,
            "FIRST-SESSION",
            "warm",
        )
        .expect("the first delivery spawns");
        wait_for_agent_screen(&state, &root, "FIRST-SESSION").await;
        state.lock().unwrap().tabs[&key].session.kill_and_reap();
        let retained_total = loop {
            {
                let s = state.lock().unwrap();
                let tab = &s.tabs[&key];
                if !tab.live {
                    break tab.screen.total;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert!(retained_total > 0, "the dead session left a cursor behind");

        // The state a client that attached inside the spawn window is in.
        let (sender, mut pushes, session_key) = SessionSender::observable("late");
        {
            let mut s = state.lock().unwrap();
            let mut waiting = TermScreen::new(90, 25);
            waiting.register(&sender);
            s.agent_screens_awaiting_spawn
                .insert(canonical.clone(), waiting);
        }

        let (wire_id, spawned) = deliver(
            &state,
            &root,
            "run-respawn-race",
            &choice,
            "SECOND-SESSION",
            "warm",
        )
        .expect("a dead agent is replaced");
        assert_eq!(spawned, Spawned::Fresh, "a dead agent is not an agent");

        let seen = wait_for_pushes(&mut pushes, &session_key, |seen| {
            output_text(seen, &wire_id).contains("SECOND-SESSION")
        })
        .await;
        let opening = seen
            .iter()
            .find(|push| push["type"] == "term.reset")
            .expect("the waiting client hears the new session start");
        assert!(
            opening["cursor"].as_u64().unwrap() >= retained_total,
            "the retained cursor is carried forward, never rewound to the \
             waiting screen's zero: {seen:?}"
        );
    }

    /// A client can be waiting on the Agent tab of a worktree that is then
    /// deleted out from under it. The reaper closes the tabs of a vanished
    /// worktree; the screen its agent was going to be born onto is the same
    /// thing one step earlier, so it goes the same way — the client hears
    /// `reaped` instead of waiting forever on a directory that is gone, and no
    /// future spawn inherits it.
    #[test]
    fn the_reaper_drops_an_agent_screen_whose_worktree_vanished_before_a_spawn() {
        let (dir, repo) = init_repo();
        let (state, _handler) = shared_state_and_handler(&repo, dir.path());
        let vanishing = dir.path().join("vanishing");
        std::fs::create_dir_all(&vanishing).unwrap();
        let root = AppState::canonical_root(&vanishing);
        let wire_id = format!("agent:{}", crate::worktree::external_worktree_id(&root));

        let (sender, mut pushes, session_key) = SessionSender::observable("s1");
        {
            let mut s = state.lock().unwrap();
            let mut waiting = TermScreen::new(80, 24);
            waiting.register(&sender);
            s.agent_screens_awaiting_spawn.insert(root.clone(), waiting);
        }
        std::fs::remove_dir_all(&vanishing).unwrap();

        let reaped = state.lock().unwrap().reap_orphaned_terminals();
        assert_eq!(reaped, vec![wire_id.clone()]);
        assert!(
            state
                .lock()
                .unwrap()
                .agent_screens_awaiting_spawn
                .is_empty(),
            "a screen for a directory that is gone is never handed to a future spawn"
        );
        let closed = SessionSender::decrypt_push(
            &session_key,
            &pushes.try_recv().expect("the client hears its tab is gone"),
        );
        assert_eq!(closed["type"], "term.closed", "{closed:?}");
        assert_eq!(closed["term_id"], wire_id);
        assert_eq!(closed["reason"], "reaped");
    }

    /// A human can close the browser while waiting on the Agent tab of a
    /// worktree whose agent has not started yet. `drop_session` detaches an
    /// ended session from every tab so the pumps stop encrypting frames into a
    /// session the relay will only drop — and a screen waiting for its first
    /// spawn is a tab one step early, so it goes the same way. Otherwise the
    /// spawn that finally comes carries a dead client onto the real screen and
    /// pushes to it for the life of the tab, and sizes the new PTY to a
    /// viewport nobody is looking at.
    #[tokio::test]
    async fn a_session_that_ended_while_waiting_is_not_carried_onto_the_agent() {
        let (dir, repo) = init_repo();
        let (state, handler, _) = agent_tab_fixture(&repo, dir.path(), "run-closed-client");
        let project_id = state.lock().unwrap().projects[0].id.clone();

        let (sender, _pushes, _key) = SessionSender::observable("closing");
        let waiting = handler(
            sender,
            req(
                "agent.attach",
                json!({ "project_id": project_id, "cols": 90, "rows": 25 }),
            ),
        );
        assert_eq!(waiting["ok"], true, "{waiting:?}");
        assert_eq!(waiting["result"]["live"], false, "nothing runs here yet");

        state.lock().unwrap().drop_session("closing");
        assert!(
            state
                .lock()
                .unwrap()
                .agent_screens_awaiting_spawn
                .values()
                .all(|screen| screen.attached.is_empty()),
            "an ended session is detached from the screen it was waiting on"
        );

        deliver(
            &state,
            &repo,
            "run-closed-client",
            &ModelChoice::default(),
            "COLD-PROMPT",
            "WARM-NUDGE",
        )
        .expect("the delivery spawns the worktree's agent");

        let s = state.lock().unwrap();
        let tab = &s.tabs[&TabKey::agent(&AppState::canonical_root(&repo))];
        assert!(
            tab.screen.attached.is_empty(),
            "a session that ended is never carried onto the agent it waited for"
        );
        assert_eq!(
            (tab.screen.cols, tab.screen.rows),
            (120, 40),
            "with nobody left waiting, the spawn keeps the size Build chose"
        );
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
        let (_, run_id) = planned_run_in_review(&mut state, "git me");
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

    // ---- git scope keyed on an external worktree ------------------------------

    /// Mint an unbound worktree and hand back (project_id, worktree_id, path).
    fn bare_worktree(state: &mut AppState, name: &str) -> (String, String, PathBuf) {
        let project_id = state.projects[0].id.clone();
        let created = state.handle(req(
            "worktree.create",
            json!({ "project_id": project_id, "name": name }),
        ));
        assert_eq!(created["ok"], true, "{created:?}");
        let result = &created["result"];
        (
            project_id,
            result["worktree_id"].as_str().unwrap().to_string(),
            PathBuf::from(result["path"].as_str().unwrap()),
        )
    }

    /// The whole git GUI — status, staging, commit, history — works on a
    /// worktree the same way it does on the primary checkout, and reads the
    /// worktree's own tree rather than the project's.
    #[test]
    fn the_git_gui_scopes_to_an_external_worktree() {
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let (project_id, worktree_id, path) = bare_worktree(&mut state, "scratch");
        let scope = json!({ "project_id": project_id, "worktree_id": worktree_id });
        std::fs::write(path.join("only-here.txt"), "in the worktree\n").unwrap();

        let status = state.handle(req("git.status", scope.clone()));
        assert_eq!(status["ok"], true, "{status:?}");
        assert!(has_file_entry(&status["result"], "only-here.txt"));
        assert_eq!(status["result"]["branch"], json!("build/scratch"));

        // The project's own checkout is untouched by any of it.
        let primary = state.handle(req("git.status", json!({ "project_id": project_id })));
        assert!(!has_file_entry(&primary["result"], "only-here.txt"));

        let staged = state.handle(req(
            "git.stage",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "paths": ["only-here.txt"] }),
        ));
        assert_eq!(staged["ok"], true, "{staged:?}");
        let committed = state.handle(req(
            "git.commit",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "message": "in the worktree" }),
        ));
        assert_eq!(committed["ok"], true, "{committed:?}");

        // History is the worktree's, and the new commit is marked ahead of the
        // base branch — the same affordance a run's history carries.
        let log = state.handle(req("git.log", scope.clone()));
        let commits = log["result"]["commits"].as_array().unwrap();
        assert_eq!(commits[0]["subject"], json!("in the worktree"));
        assert_eq!(commits[0]["ahead_of_base"], json!(true));
        assert_eq!(commits[1]["ahead_of_base"], json!(false));

        // And the rail sees the commit without waiting out the scan cache.
        let listed = state.external_worktrees(&project_id, false).unwrap();
        let entry = listed.iter().find(|w| w.id == worktree_id).unwrap();
        assert_eq!(entry.unpushed, Some(1));
        assert_eq!(entry.uncommitted.files_changed, 0, "committed, so clean");
    }

    /// Branch operations name a checkout, and a worktree scope means THAT
    /// worktree — never the project's primary checkout standing in for it.
    #[test]
    fn branch_operations_switch_the_worktree_they_are_scoped_to() {
        let (dir, repo) = init_repo();
        let mut state = git_gui_state(&dir, &repo);
        let (project_id, worktree_id, path) = bare_worktree(&mut state, "scratch");

        let checked_out = state.handle(req(
            "git.checkout",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "branch": "side-quest", "create": true }),
        ));
        assert_eq!(checked_out["ok"], true, "{checked_out:?}");
        assert_eq!(checked_out["result"]["branch"], json!("side-quest"));

        // The worktree moved; the project's checkout stayed on main.
        let head = std::process::Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&path)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "side-quest");
        let primary = state.handle(req("git.status", json!({ "project_id": project_id })));
        assert_eq!(primary["result"]["branch"], json!("main"));
    }

    /// The rail's worktree affordance (the FAB's smaller sibling): mint a
    /// worktree with NO run, no agent and no session — a directory the human
    /// then opens a terminal or an agent tab in. It is unbound, so the scan
    /// reports it exactly like a worktree made by hand, and the usual
    /// adopt-on-first-mutation path still applies.
    #[test]
    fn worktree_create_mints_an_unbound_worktree_the_scan_can_see() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.projects[0].id.clone();

        let created = state.handle(req(
            "worktree.create",
            json!({ "project_id": project_id, "name": "scratch" }),
        ));
        assert_eq!(created["ok"], true, "{created:?}");
        let result = &created["result"];
        let worktree_id = result["worktree_id"].as_str().unwrap().to_string();
        assert!(
            result["branch"].as_str().unwrap().starts_with("build/"),
            "{result:?}"
        );
        assert!(std::path::Path::new(result["path"].as_str().unwrap()).is_dir());
        assert_eq!(result["project_id"], json!(project_id));

        // Nothing was dispatched: no run, no session, no task lifecycle.
        assert!(state.runs.is_empty(), "a bare worktree is not a run");

        // The scan sees it under the id the create returned, so the client can
        // navigate straight to its surface.
        let listed = state.external_worktrees(&project_id, true).unwrap();
        assert!(
            listed.iter().any(|w| w.id == worktree_id),
            "{worktree_id} missing from {listed:?}"
        );

        // A second one does not collide with the first.
        let second = state.handle(req(
            "worktree.create",
            json!({ "project_id": project_id, "name": "scratch" }),
        ));
        assert_eq!(second["ok"], true, "{second:?}");
        assert_ne!(second["result"]["branch"], result["branch"]);
        assert_ne!(second["result"]["worktree_id"], result["worktree_id"]);
    }

    /// The name the human typed decides the directory and the branch, through the
    /// same slugifier every other branch name goes through — it is UNTRUSTED text
    /// on its way to a path and a `git` argv.
    #[test]
    fn worktree_create_names_the_branch_after_the_name() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.projects[0].id.clone();

        let created = state.handle(req(
            "worktree.create",
            json!({ "project_id": project_id, "name": "Mascot Model Spike!" }),
        ));
        assert_eq!(created["ok"], true, "{created:?}");
        assert_eq!(created["result"]["branch"], "build/mascot-model-spike");
        assert!(created["result"]["path"]
            .as_str()
            .unwrap()
            .ends_with("mascot-model-spike"));

        // The same name twice cannot collide on disk or on a ref.
        let again = state.handle(req(
            "worktree.create",
            json!({ "project_id": project_id, "name": "Mascot Model Spike!" }),
        ));
        assert_eq!(again["ok"], true, "{again:?}");
        assert_ne!(again["result"]["branch"], created["result"]["branch"]);
    }

    /// A name that slugifies to nothing would silently become some fallback word,
    /// so it is refused instead — the human named it, and the name has to survive.
    #[test]
    fn worktree_create_requires_a_usable_name() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.projects[0].id.clone();

        for name in ["", "   ", "***", "!!!"] {
            let res = state.handle(req(
                "worktree.create",
                json!({ "project_id": project_id, "name": name }),
            ));
            assert_eq!(res["ok"], false, "{name:?} -> {res:?}");
        }
        assert!(
            state.handle(req("worktree.create", json!({ "project_id": project_id })))["ok"]
                == false
        );
    }

    #[test]
    fn worktree_create_rejects_an_unknown_project() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let res = state.handle(req(
            "worktree.create",
            json!({ "project_id": "proj-nope", "name": "scratch" }),
        ));
        assert_eq!(res["ok"], false, "{res:?}");
    }

    /// The rail shows a project's checkout as its branch plus its git status, so
    /// the summary has to carry the sync counts too — not only the working-tree
    /// diffstat. No upstream means no counts, which is a different thing from
    /// "level with upstream" and is reported as such.
    #[test]
    fn primary_changes_carries_ahead_behind_beside_the_diffstat() {
        let (dir, repo, origin) = init_repo_with_origin();
        let mut state = qa_state(&repo, dir.path());

        // A local commit that origin has not seen: ahead 1, behind 0.
        std::fs::write(repo.join("ahead.txt"), "local\n").unwrap();
        git_in_dir(&repo, &["add", "."]);
        git_in_dir(&repo, &["commit", "-m", "local only"]);
        // …and an uncommitted edit, so both halves are non-zero at once.
        std::fs::write(repo.join("dirty.txt"), "wip\n").unwrap();

        let board = state.handle(req("board.list", json!({})));
        let entry = board["result"]["primary_changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["branch"] == "main")
            .cloned()
            .unwrap_or_else(|| panic!("no primary entry: {board:?}"));
        assert_eq!(entry["ahead"], 1, "{entry:?}");
        assert_eq!(entry["behind"], 0, "{entry:?}");
        assert!(entry["files_changed"].as_u64().unwrap() >= 1, "{entry:?}");
        let _ = origin;
    }

    #[test]
    fn primary_changes_compares_an_untracked_branch_with_local_main() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        git_in_dir(&repo, &["checkout", "-b", "topic"]);
        std::fs::write(repo.join("topic.txt"), "topic\n").unwrap();
        git_in_dir(&repo, &["add", "."]);
        git_in_dir(&repo, &["commit", "-m", "topic"]);

        let board = state.handle(req("board.list", json!({})));
        let entry = board["result"]["primary_changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["branch"] == "topic")
            .unwrap_or_else(|| panic!("no topic entry: {board:?}"));
        assert!(entry["upstream"].is_null(), "{entry:?}");
        assert_eq!(entry["comparison_ref"], "main", "{entry:?}");
        assert_eq!(entry["ahead"], 1, "{entry:?}");
        assert_eq!(entry["behind"], 0, "{entry:?}");
    }

    // ---- attention: what the rail orders and colours itself by ---------------

    fn attention_of(state: &mut AppState, id: &str) -> Value {
        let board = state.handle(req("board.list", json!({})));
        for key in ["runs", "plans", "external_worktrees"] {
            if let Some(list) = board["result"][key].as_array() {
                for entry in list {
                    let entry_id = entry["run_id"]
                        .as_str()
                        .or_else(|| entry["plan_id"].as_str())
                        .or_else(|| entry["worktree_id"].as_str());
                    if entry_id == Some(id) {
                        return entry["attention"].clone();
                    }
                }
            }
        }
        panic!("{id} not on the board: {board:?}");
    }

    /// Reading a stage doc IS engaging with an issue — they are a queue you
    /// triage by reading — so it stamps. A run needs an action.
    #[test]
    fn opening_a_stage_counts_as_touching_an_issue_but_reading_a_run_does_not() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (plan_id, run_id) = planned_run_in_review(&mut state, "attention");

        // A fresh run made by implementing: the implement stamped it.
        assert_eq!(attention_of(&mut state, &run_id)["interacted"], true);

        // Reading the run changes nothing about interaction.
        let before = attention_of(&mut state, &run_id);
        state.handle(req("run.get", json!({ "run_id": run_id })));
        state.handle(req("run.diff", json!({ "run_id": run_id })));
        assert_eq!(
            attention_of(&mut state, &run_id),
            before,
            "reading is not acting"
        );

        // Opening a stage doc stamps the issue.
        let mut fresh = qa_state(&repo, dir.path());
        let plan = fresh.handle(req("plan.create", json!({ "goal": "queue item" })));
        let queued = plan_id_of(&plan);
        assert_eq!(attention_of(&mut fresh, &queued)["interacted"], false);
        fresh.handle(req(
            "plan.stage_doc",
            json!({ "plan_id": queued, "stage_id": "first-half" }),
        ));
        assert_eq!(attention_of(&mut fresh, &queued)["interacted"], true);
        let _ = plan_id;
    }

    /// A rejected verb never happened, so it cannot count as touching anything.
    #[test]
    fn a_refused_action_does_not_stamp() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let plan = state.handle(req("plan.create", json!({ "goal": "never approved" })));
        let plan_id = plan_id_of(&plan);
        let refused = state.handle(req(
            "plan.stage_approve",
            json!({ "plan_id": plan_id, "stage_id": "no-such-stage" }),
        ));
        assert_eq!(refused["ok"], false, "{refused:?}");
        assert_eq!(attention_of(&mut state, &plan_id)["interacted"], false);
    }

    /// A worktree Build cut is something you asked for, so it arrives already
    /// touched and surfaces in the rail. One made outside Build waits in the
    /// Worktrees row until you act on it here.
    #[test]
    fn a_build_made_worktree_arrives_touched_and_a_hand_made_one_does_not() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.projects[0].id.clone();

        let created = state.handle(req(
            "worktree.create",
            json!({ "project_id": project_id, "name": "spike" }),
        ));
        let build_made = created["result"]["worktree_id"]
            .as_str()
            .unwrap()
            .to_string();

        add_external_worktree(&repo, dir.path(), "by-hand", "by-hand");
        state.external_worktrees(&project_id, true).unwrap();
        let hand_made = state
            .external_worktrees(&project_id, true)
            .unwrap()
            .into_iter()
            .find(|w| w.branch.as_deref() == Some("by-hand"))
            .expect("the hand-made worktree is discoverable")
            .id;

        assert_eq!(attention_of(&mut state, &build_made)["interacted"], true);
        assert_eq!(attention_of(&mut state, &hand_made)["interacted"], false);
    }

    /// Seen is versioned: looking at something does not make it seen forever.
    #[test]
    fn seeing_an_entity_lasts_only_until_it_moves() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (_, run_id) = planned_run_in_review(&mut state, "seen versioning");

        assert_eq!(attention_of(&mut state, &run_id)["seen"], false);
        let seen = state.handle(req("entity.seen", json!({ "entity_id": run_id })));
        assert_eq!(seen["ok"], true, "{seen:?}");
        assert_eq!(attention_of(&mut state, &run_id)["seen"], true);

        // It moves on: unseen again, without anyone clearing a flag.
        std::thread::sleep(Duration::from_millis(1100));
        state.handle(req("run.abandon", json!({ "run_id": run_id })));
        assert_eq!(attention_of(&mut state, &run_id)["seen"], false);
    }

    /// The pulse means "an agent is working here", which is a different claim
    /// from "a tab is open". A shell is never an agent; an agent that has
    /// stopped painting is waiting for you, not working; and a dead agent's
    /// retained screen is not a heartbeat.
    ///
    /// The signal is read off the worktree's agent TAB now, not off a
    /// terminal's kind — a tab is the only place an agent can be, so there is
    /// nowhere else for the pulse to come from.
    #[tokio::test]
    async fn only_a_recently_painting_agent_counts_as_working() {
        let (dir, repo) = init_repo();
        let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-pulse");
        ensure_agent_tab(&state, &root, "run-pulse", &ModelChoice::default()).unwrap();

        let key = TabKey::agent(&AppState::canonical_root(&root));
        let mut s = state.lock().unwrap();
        let agent = s.tabs.get(&key).expect("the agent tab");
        assert!(
            agent_is_working(agent),
            "a freshly spawned agent has just painted"
        );

        // A dead agent's retained screen is not a heartbeat: the tab still holds
        // the last thing it painted, and that is a corpse, not progress.
        let dead = {
            let agent = s.tabs.get_mut(&key).unwrap();
            agent.live = false;
            let dead = agent_is_working(agent);
            agent.live = true; // restore: the next case is about a LIVE agent
            dead
        };
        assert!(!dead, "a dead agent's retained screen is not a heartbeat");

        // Left at its prompt overnight: the tab is live, the process is running,
        // and it has painted nothing since the window closed. That agent is
        // waiting for YOU — a pulse here teaches the human to ignore the pulse.
        let parked = {
            let agent = s.tabs.get_mut(&key).unwrap();
            assert!(
                agent_is_working(agent),
                "still working right up until it falls silent"
            );
            agent
                .session
                .backdate_last_output(AGENT_WORKING_WINDOW + Duration::from_secs(1));
            assert!(agent.live, "the tab is live");
            assert!(!agent.session.has_exited(), "and its process still running");
            agent_is_working(agent)
        };
        assert!(
            !parked,
            "an agent parked at its prompt is waiting for you, not working"
        );

        // The human's own shell is never an agent, however busy it looks.
        let shell_root = AppState::canonical_root(&repo);
        let (shell, _rx) = Tab::spawn(
            TabRole::Shell,
            &shell_harness_spec("/bin/bash"),
            "term-77".to_string(),
            shell_root.clone(),
            80,
            24,
        )
        .expect("a shell tab spawns");
        assert!(!agent_is_working(&shell));
        shell.session.kill_and_reap();
    }

    /// The board reports it per worktree, so a bare worktree — which has no run
    /// state to read — can still say whether something is happening in it.
    #[tokio::test]
    async fn the_board_reports_whether_an_agent_is_working_in_a_worktree() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let project_id = state.lock().unwrap().projects[0].id.clone();
        add_external_worktree(&repo, dir.path(), "hand-made", "hand-made");
        let worktree_id = state
            .lock()
            .unwrap()
            .external_worktrees(&project_id, true)
            .unwrap()
            .into_iter()
            .find(|w| w.branch.as_deref() == Some("hand-made"))
            .expect("discoverable")
            .id;

        let entry_of = |state: &Arc<Mutex<AppState>>| {
            let board = state.lock().unwrap().handle(req("board.list", json!({})));
            board["result"]["external_worktrees"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["worktree_id"] == json!(worktree_id.clone()))
                .cloned()
                .unwrap_or_else(|| panic!("worktree missing: {board:?}"))
        };
        assert_eq!(
            entry_of(&state)["agent_working"],
            false,
            "nothing running yet"
        );
        assert_eq!(entry_of(&state)["can_finish"], false);

        // A shell is not an agent, so opening one must not start the pulse.
        let created = handler(
            SessionSender::detached("s1"),
            req(
                "term.create",
                json!({ "project_id": project_id, "worktree_id": worktree_id, "kind": "shell" }),
            ),
        );
        assert_eq!(created["ok"], true, "{created:?}");
        assert_eq!(
            entry_of(&state)["agent_working"],
            false,
            "a shell is the human's own hands"
        );
        assert_eq!(entry_of(&state)["can_finish"], false);

        // Build's agent starts in that same worktree: the pulse is on, and it
        // is reported against the WORKTREE — the run that owns the agent is
        // not what the board asked about.
        let root = {
            let mut s = state.lock().unwrap();
            let owner = "run-in-the-worktree".to_string();
            s.entity_project.insert(owner, project_id.clone());
            s.resolve_external_worktree(&project_id, &worktree_id)
                .unwrap()
                .path
        };
        ensure_agent_tab(
            &state,
            &root,
            "run-in-the-worktree",
            &ModelChoice::default(),
        )
        .expect("the agent spawns");
        assert_eq!(
            entry_of(&state)["agent_working"],
            true,
            "an agent painting in this worktree is the pulse"
        );
        assert_eq!(entry_of(&state)["can_finish"], false);

        let key = TabKey::agent(&AppState::canonical_root(&root));
        state.lock().unwrap().tabs[&key]
            .session
            .backdate_last_output(AGENT_WORKING_WINDOW + Duration::from_secs(1));
        assert_eq!(entry_of(&state)["agent_working"], false);
        assert_eq!(
            entry_of(&state)["can_finish"],
            true,
            "a managed agent that has stopped working makes finish advisable"
        );
        state.lock().unwrap().tabs[&key].session.kill_and_reap();
    }

    /// The relay calls `dispatch` directly — `handle` is a test convenience — so
    /// a stamp wired into `handle` would pass every test and fire in no real
    /// session. This drives the wire path the daemon actually uses.
    #[tokio::test]
    async fn stamping_happens_on_the_path_the_relay_uses() {
        let (dir, repo) = init_repo();
        let (state, handler) = shared_state_and_handler(&repo, dir.path());
        let project_id = state.lock().unwrap().projects[0].id.clone();

        let created = handler(
            SessionSender::detached("s1"),
            req(
                "worktree.create",
                json!({ "project_id": project_id, "name": "over the wire" }),
            ),
        );
        assert_eq!(created["ok"], true, "{created:?}");
        let worktree_id = created["result"]["worktree_id"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(
            state.lock().unwrap().attention.contains_key(&worktree_id),
            "the wire path must stamp too"
        );
    }

    /// Attention outlives the daemon, or Monday would look like a fresh install.
    #[test]
    fn attention_survives_a_restart() {
        let (dir, repo) = init_repo();
        let run_id;
        {
            let mut state = qa_state(&repo, dir.path());
            let (_, id) = planned_run_in_review(&mut state, "durable attention");
            run_id = id;
            state.handle(req("entity.seen", json!({ "entity_id": run_id })));
            assert_eq!(attention_of(&mut state, &run_id)["seen"], true);
        }
        let mut reloaded = qa_state(&repo, dir.path());
        let after = attention_of(&mut reloaded, &run_id);
        assert_eq!(after["seen"], true, "{after:?}");
        assert_eq!(after["interacted"], true, "{after:?}");
    }

    #[test]
    fn completed_plan_archives_idempotently_and_moves_off_the_board() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (plan_id, run_id) = planned_run_in_review(&mut state, "archive completed plan");
        let project_id = state.projects[0].id.clone();

        let before = state.handle(req("plan.get", json!({ "plan_id": plan_id })));
        assert_eq!(
            before["result"]["implementation_complete"], true,
            "{before:?}"
        );
        assert_eq!(before["result"]["can_archive"], true, "{before:?}");
        assert!(before["result"]["archived_at"].is_null());

        let archived = state.handle(req("plan.archive", json!({ "plan_id": plan_id })));
        assert_eq!(archived["ok"], true, "{archived:?}");
        let archived_at = archived["result"]["archived_at"]
            .as_str()
            .expect("archive timestamp")
            .to_string();
        assert_eq!(archived["result"]["can_archive"], false);

        let repeated = state.handle(req("plan.archive", json!({ "plan_id": plan_id })));
        assert_eq!(repeated["result"]["archived_at"], archived_at);
        let board = state.handle(req("board.list", json!({})));
        assert!(board["result"]["plans"]
            .as_array()
            .unwrap()
            .iter()
            .all(|plan| plan["plan_id"] != plan_id));
        assert!(board["result"]["runs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|run| run["run_id"] == run_id));

        let listed = state.handle(req("plan.list", json!({})));
        assert!(listed["result"]["plans"]
            .as_array()
            .unwrap()
            .iter()
            .any(|plan| plan["plan_id"] == plan_id));
        let archive = state.handle(req("archive.list", json!({ "project_id": project_id })));
        assert_eq!(archive["result"]["plans"].as_array().unwrap().len(), 1);
        assert!(archive["result"]["worktrees"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn plan_archive_rejects_incomplete_plans_and_legacy_completion_uses_run_gate() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let incomplete = state.handle(req("plan.create", json!({ "goal": "not implemented" })));
        let incomplete_id = plan_id_of(&incomplete);
        let rejected = state.handle(req("plan.archive", json!({ "plan_id": incomplete_id })));
        assert_eq!(rejected["ok"], false, "{rejected:?}");
        assert!(rejected["error"].as_str().unwrap().contains("incomplete"));

        let (legacy_plan_id, legacy_run_id) =
            planned_run_in_review(&mut state, "legacy completion");
        state.plans.get_mut(&legacy_plan_id).unwrap().stages.clear();
        state.runs.get_mut(&legacy_run_id).unwrap().stages.clear();
        let legacy = state.handle(req("plan.get", json!({ "plan_id": legacy_plan_id })));
        assert_eq!(
            legacy["result"]["implementation_complete"], true,
            "{legacy:?}"
        );
    }

    #[test]
    fn multi_stage_completion_requires_every_plan_stage_to_have_passed_in_one_run() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (plan_id, run_id) = planned_run_in_review(&mut state, "all stages");
        state.runs.get_mut(&run_id).unwrap().stages.pop();

        let incomplete = state.handle(req("plan.get", json!({ "plan_id": plan_id })));
        assert_eq!(incomplete["result"]["implementation_complete"], false);
        assert_eq!(incomplete["result"]["can_archive"], false);
    }

    #[test]
    fn legacy_validated_stages_without_pinned_boundaries_preserve_completion() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (issue_id, run_id) = planned_run_in_review(&mut state, "legacy completion");
        for stage in &mut state.runs.get_mut(&run_id).unwrap().stages {
            stage.completion_sha = None;
            stage.publication = StagePublication::LegacyUnknown;
        }
        let issue = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
        assert_eq!(
            issue["result"]["implementation_complete"], true,
            "{issue:?}"
        );
        let stages = state.handle(req("issue.stages", json!({ "issue_id": issue_id })));
        assert!(stages["result"]["stages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|stage| stage["execution"] == "legacy_unpinned"));
    }

    #[test]
    fn archived_local_only_run_no_longer_counts_as_issue_completion() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let (plan_id, run_id) = planned_run_in_review(&mut state, "completed then removed");
        let worktree = state.runs[&run_id].worktree.path.clone();
        std::fs::remove_dir_all(worktree).unwrap();

        let board = state.handle(req("board.list", json!({})));
        assert!(board["result"]["runs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|run| run["run_id"] != run_id));
        assert_eq!(state.runs[&run_id].run.state, RunState::Archived);
        let plan = state.handle(req("plan.get", json!({ "plan_id": plan_id })));
        assert_eq!(plan["result"]["implementation_complete"], false, "{plan:?}");
        assert_eq!(plan["result"]["can_archive"], false, "{plan:?}");
    }

    #[test]
    fn archived_plan_metadata_survives_restart_with_docs_and_runs() {
        let (dir, repo) = init_repo();
        let plan_id;
        let run_id;
        let canonical_doc;
        {
            let mut state = qa_state(&repo, dir.path());
            (plan_id, run_id) = planned_run_in_review(&mut state, "durable archive");
            let before = state.handle(req(
                "plan.stage_doc",
                json!({ "plan_id": plan_id, "stage_id": "first-half" }),
            ));
            canonical_doc = before["result"]["contents"]
                .as_str()
                .expect("canonical stage doc")
                .to_string();
            let archived = state.handle(req("plan.archive", json!({ "plan_id": plan_id })));
            assert_eq!(archived["ok"], true, "{archived:?}");
            let repeated = state.handle(req("plan.archive", json!({ "plan_id": plan_id })));
            assert_eq!(
                repeated["result"]["archived_at"],
                archived["result"]["archived_at"]
            );
            let after = state.handle(req(
                "plan.stage_doc",
                json!({ "plan_id": plan_id, "stage_id": "first-half" }),
            ));
            assert_eq!(after["result"]["contents"], canonical_doc);
        }

        let mut reloaded = qa_state(&repo, dir.path());
        let plan = reloaded.handle(req("plan.get", json!({ "plan_id": plan_id })));
        assert!(plan["result"]["archived_at"].is_string(), "{plan:?}");
        assert_eq!(plan["result"]["implementation_complete"], true);
        assert_eq!(
            reloaded.handle(req("run.get", json!({ "run_id": run_id })))["ok"],
            true
        );
        let doc = reloaded.handle(req(
            "plan.stage_doc",
            json!({ "plan_id": plan_id, "stage_id": "first-half" }),
        ));
        assert_eq!(doc["ok"], true, "{doc:?}");
        assert_eq!(doc["result"]["contents"], canonical_doc);
    }

    fn external_id(state: &mut AppState, project_id: &str, branch: Option<&str>) -> String {
        state
            .external_worktrees(project_id, true)
            .unwrap()
            .into_iter()
            .find(|worktree| worktree.branch.as_deref() == branch)
            .expect("external worktree is discoverable")
            .id
    }

    #[test]
    fn worktree_finish_cleanup_requires_clean_and_preserves_branch() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.projects[0].id.clone();
        let path = add_external_worktree(&repo, dir.path(), "cleanup", "cleanup");
        let worktree_id = external_id(&mut state, &project_id, Some("cleanup"));

        std::fs::write(path.join("dirty.txt"), "dirty\n").unwrap();
        let rejected = state.handle(req(
            "worktree.finish",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "cleanup" }),
        ));
        assert_eq!(rejected["ok"], false, "{rejected:?}");
        assert!(path.exists());

        std::fs::remove_file(path.join("dirty.txt")).unwrap();
        let finished = state.handle(req(
            "worktree.finish",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "cleanup" }),
        ));
        assert_eq!(finished["ok"], true, "{finished:?}");
        assert_eq!(finished["result"]["action"], "cleanup");
        assert!(!path.exists());
        assert!(git2::Repository::open(&repo)
            .unwrap()
            .find_branch("cleanup", git2::BranchType::Local)
            .is_ok());

        let repeated = state.handle(req(
            "worktree.finish",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "cleanup" }),
        ));
        assert_eq!(repeated["ok"], true, "{repeated:?}");
    }

    #[test]
    fn worktree_finish_store_failure_happens_before_worktree_removal() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.projects[0].id.clone();
        let path = add_external_worktree(&repo, dir.path(), "store-failure", "store-failure");
        let worktree_id = external_id(&mut state, &project_id, Some("store-failure"));
        let store_root = dir.path().join("store");
        std::fs::create_dir_all(&store_root).unwrap();
        std::fs::write(store_root.join("archived-worktrees"), "not a directory").unwrap();

        let failed = state.handle(req(
            "worktree.finish",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "cleanup" }),
        ));

        assert_eq!(failed["ok"], false, "{failed:?}");
        assert!(path.exists(), "store failure must precede removal");
        assert!(git2::Repository::open(&repo)
            .unwrap()
            .find_branch("store-failure", git2::BranchType::Local)
            .is_ok());
    }

    #[test]
    fn worktree_finish_merge_checkpoints_dirty_work_deletes_branch_and_archives() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.projects[0].id.clone();
        let path = add_external_worktree(&repo, dir.path(), "merge-me", "merge-me");
        std::fs::write(path.join("feature.txt"), "finished\n").unwrap();
        let worktree_id = external_id(&mut state, &project_id, Some("merge-me"));

        let finished = state.handle(req(
            "worktree.finish",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "merge" }),
        ));
        assert_eq!(finished["ok"], true, "{finished:?}");
        assert!(repo.join("feature.txt").is_file());
        assert!(!path.exists());
        assert!(git2::Repository::open(&repo)
            .unwrap()
            .find_branch("merge-me", git2::BranchType::Local)
            .is_err());
        let log = Command::new("git")
            .args(["log", "--format=%s", "-2"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&log.stdout).contains("Build checkpoint before merge"));

        let archive = state.handle(req("archive.list", json!({ "project_id": project_id })));
        let worktree = &archive["result"]["worktrees"][0];
        assert_eq!(worktree["worktree_id"], worktree_id);
        assert_eq!(worktree["dirty_files"], 1);
        assert_eq!(worktree["action"], "merge");
    }

    #[test]
    fn worktree_finish_push_requires_tracking_then_checkpoints_pushes_and_keeps_branch() {
        let (dir, repo, _origin) = init_repo_with_origin();
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.projects[0].id.clone();
        let path = add_external_worktree(&repo, dir.path(), "push-me", "push-me");
        let worktree_id = external_id(&mut state, &project_id, Some("push-me"));

        let rejected = state.handle(req(
            "worktree.finish",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "push" }),
        ));
        assert_eq!(rejected["ok"], false, "{rejected:?}");
        assert!(rejected["error"].as_str().unwrap().contains("upstream"));

        git_in_dir(&path, &["push", "-u", "origin", "push-me"]);
        std::fs::write(path.join("pushed.txt"), "published\n").unwrap();
        let finished = state.handle(req(
            "worktree.finish",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "push" }),
        ));
        assert_eq!(finished["ok"], true, "{finished:?}");
        assert!(!path.exists());
        assert!(git2::Repository::open(&repo)
            .unwrap()
            .find_branch("push-me", git2::BranchType::Local)
            .is_ok());
        let remote_subject = Command::new("git")
            .args(["log", "--format=%s", "-1", "origin/push-me"])
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&remote_subject.stdout)
            .contains("Build checkpoint before push"));
    }

    #[test]
    fn worktree_finish_delete_accepts_dirty_detached_head_without_deleting_a_branch() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.projects[0].id.clone();
        let path = add_external_worktree(&repo, dir.path(), "detached", "detached-source");
        git_in_dir(&path, &["checkout", "--detach"]);
        std::fs::write(path.join("discarded.txt"), "discard me\n").unwrap();
        let worktree_id = external_id(&mut state, &project_id, None);

        let finished = state.handle(req(
            "worktree.finish",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "delete" }),
        ));
        assert_eq!(finished["ok"], true, "{finished:?}");
        assert!(finished["result"]["branch"].is_null());
        assert!(!path.exists());
        assert!(git2::Repository::open(&repo)
            .unwrap()
            .find_branch("detached-source", git2::BranchType::Local)
            .is_ok());

        let attached_path =
            add_external_worktree(&repo, dir.path(), "attached-delete", "attached-delete");
        std::fs::write(attached_path.join("discarded.txt"), "discard me too\n").unwrap();
        let attached_id = external_id(&mut state, &project_id, Some("attached-delete"));
        let attached = state.handle(req(
            "worktree.finish",
            json!({ "project_id": project_id, "worktree_id": attached_id, "action": "delete" }),
        ));
        assert_eq!(attached["ok"], true, "{attached:?}");
        assert!(git2::Repository::open(&repo)
            .unwrap()
            .find_branch("attached-delete", git2::BranchType::Local)
            .is_err());
    }

    #[test]
    fn worktree_finish_branch_delete_failure_is_retryable_and_not_archived() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.projects[0].id.clone();
        let path = add_external_worktree(&repo, dir.path(), "delete-retry", "delete-retry");
        std::fs::write(path.join("discarded.txt"), "discard me\n").unwrap();
        let worktree_id = external_id(&mut state, &project_id, Some("delete-retry"));
        let lock = repo.join(".git/refs/heads/delete-retry.lock");
        std::fs::write(&lock, "locked\n").unwrap();

        let failed = state.handle(req(
            "worktree.finish",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "delete" }),
        ));
        assert_eq!(failed["ok"], false, "{failed:?}");
        assert!(
            path.exists(),
            "branch failure must leave a retryable worktree"
        );
        assert_eq!(
            external_id(&mut state, &project_id, Some("delete-retry")),
            worktree_id,
            "the rail must still resolve the original attached worktree"
        );
        let archive = state.handle(req("archive.list", json!({ "project_id": project_id })));
        assert!(archive["result"]["worktrees"]
            .as_array()
            .unwrap()
            .is_empty());

        std::fs::remove_file(lock).unwrap();
        let finished = state.handle(req(
            "worktree.finish",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "delete" }),
        ));
        assert_eq!(finished["ok"], true, "{finished:?}");
        assert!(!path.exists());
        assert!(git2::Repository::open(&repo)
            .unwrap()
            .find_branch("delete-retry", git2::BranchType::Local)
            .is_err());
    }

    #[test]
    fn worktree_finish_never_accepts_paths_and_git_failure_does_not_archive() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.projects[0].id.clone();
        let path = add_external_worktree(&repo, dir.path(), "conflict", "conflict");
        std::fs::write(path.join("README.md"), "feature\n").unwrap();
        git_in_dir(&path, &["commit", "-am", "feature"]);
        std::fs::write(repo.join("README.md"), "mainline\n").unwrap();
        git_in_dir(&repo, &["commit", "-am", "mainline"]);
        let worktree_id = external_id(&mut state, &project_id, Some("conflict"));

        let forged = state.handle(req(
            "worktree.finish",
            json!({
                "project_id": project_id,
                "worktree_id": "wt-not-real",
                "path": path,
                "action": "delete"
            }),
        ));
        assert_eq!(forged["ok"], false, "{forged:?}");
        assert!(path.exists());

        let failed = state.handle(req(
            "worktree.finish",
            json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "merge" }),
        ));
        assert_eq!(failed["ok"], false, "{failed:?}");
        assert!(path.exists(), "a failed merge must not remove the worktree");
        let archive = state.handle(req("archive.list", json!({ "project_id": project_id })));
        assert!(archive["result"]["worktrees"]
            .as_array()
            .unwrap()
            .is_empty());

        let primary_id =
            crate::worktree::external_worktree_id(&std::fs::canonicalize(&repo).unwrap());
        let primary = state.handle(req(
            "worktree.finish",
            json!({
                "project_id": project_id,
                "worktree_id": primary_id,
                "action": "delete"
            }),
        ));
        assert_eq!(primary["ok"], false, "{primary:?}");
        assert!(repo.join("README.md").exists());
    }

    #[test]
    fn archive_list_is_scoped_by_project_canonical_path() {
        let (dir, repo) = init_repo();
        let (_other_dir, other_repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let first_project = state.projects[0].id.clone();
        let second_project = state.add_project(other_repo.clone(), "main".into());
        let first_path = add_external_worktree(&repo, dir.path(), "first", "first");
        let second_path = add_external_worktree(&other_repo, dir.path(), "second", "second");
        let first_id = external_id(&mut state, &first_project, Some("first"));
        let second_id = external_id(&mut state, &second_project, Some("second"));
        assert_eq!(state.handle(req(
            "worktree.finish",
            json!({ "project_id": first_project, "worktree_id": first_id, "action": "cleanup" }),
        ))["ok"], true);
        assert_eq!(state.handle(req(
            "worktree.finish",
            json!({ "project_id": second_project, "worktree_id": second_id, "action": "cleanup" }),
        ))["ok"], true);
        assert!(!first_path.exists() && !second_path.exists());

        let first = state.handle(req("archive.list", json!({ "project_id": first_project })));
        let second = state.handle(req("archive.list", json!({ "project_id": second_project })));
        assert_eq!(first["result"]["worktrees"].as_array().unwrap().len(), 1);
        assert_eq!(second["result"]["worktrees"].as_array().unwrap().len(), 1);
        assert_ne!(
            first["result"]["worktrees"][0]["worktree_id"],
            second["result"]["worktrees"][0]["worktree_id"]
        );
    }

    #[test]
    fn archived_worktrees_load_into_archive_list_after_restart() {
        let (dir, repo) = init_repo();
        let worktree_id;
        {
            let mut state = qa_state(&repo, dir.path());
            let project_id = state.projects[0].id.clone();
            add_external_worktree(&repo, dir.path(), "durable-finish", "durable-finish");
            worktree_id = external_id(&mut state, &project_id, Some("durable-finish"));
            let finished = state.handle(req(
                "worktree.finish",
                json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "cleanup" }),
            ));
            assert_eq!(finished["ok"], true, "{finished:?}");
        }

        let mut reloaded = qa_state(&repo, dir.path());
        let reminted_project_id = reloaded.projects[0].id.clone();
        let archive = reloaded.handle(req(
            "archive.list",
            json!({ "project_id": reminted_project_id }),
        ));
        assert!(archive["result"]["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .any(|worktree| worktree["worktree_id"] == worktree_id));
    }

    #[test]
    fn restart_completes_a_durable_finish_intent_after_worktree_removal() {
        let (dir, repo) = init_repo();
        let worktree_id;
        {
            let mut state = qa_state(&repo, dir.path());
            let project_id = state.projects[0].id.clone();
            add_external_worktree(
                &repo,
                dir.path(),
                "interrupted-finish",
                "interrupted-finish",
            );
            worktree_id = external_id(&mut state, &project_id, Some("interrupted-finish"));
            let finished = state.handle(req(
                "worktree.finish",
                json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "cleanup" }),
            ));
            assert_eq!(finished["ok"], true, "{finished:?}");
        }

        let record_path = dir
            .path()
            .join("store/archived-worktrees")
            .join(format!("{worktree_id}.json"));
        let mut record: Value =
            serde_json::from_str(&std::fs::read_to_string(&record_path).unwrap()).unwrap();
        record["status"] = json!("pending");
        record["archived_at"] = Value::Null;
        std::fs::write(&record_path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();

        let mut reloaded = qa_state(&repo, dir.path());
        let project_id = reloaded.projects[0].id.clone();
        let archive = reloaded.handle(req("archive.list", json!({ "project_id": project_id })));
        assert!(archive["result"]["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .any(|worktree| worktree["worktree_id"] == worktree_id));
        let recovered: Value =
            serde_json::from_str(&std::fs::read_to_string(record_path).unwrap()).unwrap();
        assert_eq!(recovered["status"], "archived");
        assert!(recovered["archived_at"].is_string());
    }

    #[test]
    fn pushed_worktrees_load_into_archive_list_after_restart() {
        let (dir, repo, _origin) = init_repo_with_origin();
        let worktree_id;
        {
            let mut state = qa_state(&repo, dir.path());
            let project_id = state.projects[0].id.clone();
            let path = add_external_worktree(&repo, dir.path(), "durable-push", "durable-push");
            git_in_dir(&path, &["push", "-u", "origin", "durable-push"]);
            std::fs::write(path.join("pushed.txt"), "published\n").unwrap();
            worktree_id = external_id(&mut state, &project_id, Some("durable-push"));
            let finished = state.handle(req(
                "worktree.finish",
                json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "push" }),
            ));
            assert_eq!(finished["ok"], true, "{finished:?}");
        }

        let mut reloaded = qa_state(&repo, dir.path());
        let project_id = reloaded.projects[0].id.clone();
        let archive = reloaded.handle(req("archive.list", json!({ "project_id": project_id })));
        let record = archive["result"]["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .find(|worktree| worktree["worktree_id"] == worktree_id)
            .unwrap();
        assert_eq!(record["action"], "push");
    }

    #[test]
    fn deleted_worktrees_load_into_archive_list_after_restart() {
        let (dir, repo) = init_repo();
        let worktree_id;
        {
            let mut state = qa_state(&repo, dir.path());
            let project_id = state.projects[0].id.clone();
            let path = add_external_worktree(&repo, dir.path(), "durable-delete", "durable-delete");
            std::fs::write(path.join("discarded.txt"), "discard me\n").unwrap();
            worktree_id = external_id(&mut state, &project_id, Some("durable-delete"));
            let finished = state.handle(req(
                "worktree.finish",
                json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "delete" }),
            ));
            assert_eq!(finished["ok"], true, "{finished:?}");
        }

        let mut reloaded = qa_state(&repo, dir.path());
        let project_id = reloaded.projects[0].id.clone();
        let archive = reloaded.handle(req("archive.list", json!({ "project_id": project_id })));
        let record = archive["result"]["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .find(|worktree| worktree["worktree_id"] == worktree_id)
            .unwrap();
        assert_eq!(record["action"], "delete");
    }

    #[test]
    fn external_worktree_json_sets_can_finish_for_an_idle_agent_tab() {
        let (dir, repo) = init_repo();
        let mut state = qa_state(&repo, dir.path());
        let project_id = state.projects[0].id.clone();
        let path = add_external_worktree(&repo, dir.path(), "idle-agent", "idle-agent");
        let worktree_id = external_id(&mut state, &project_id, Some("idle-agent"));
        let root = AppState::canonical_root(&path);
        let (tab, _rx) = Tab::spawn(
            TabRole::Agent {
                owner: "idle-agent-owner".to_string(),
                provider: AgentProvider::default(),
            },
            &HarnessSpec::new("sh").arg("-c").arg("cat >/dev/null"),
            AGENT_TAB_ID.to_string(),
            root.clone(),
            80,
            24,
        )
        .unwrap();
        tab.session
            .backdate_last_output(AGENT_WORKING_WINDOW + Duration::from_secs(1));
        let key = TabKey::agent(&root);
        state.tabs.insert(key.clone(), tab);

        let board = state.handle(req("board.list", json!({})));
        let entry = board["result"]["external_worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["worktree_id"] == worktree_id)
            .unwrap();
        assert_eq!(entry["agent_working"], false, "{entry:?}");
        assert_eq!(entry["can_finish"], true, "{entry:?}");
        state.tabs.remove(&key).unwrap().session.kill_and_reap();
    }

    #[tokio::test]
    async fn worktree_finish_closes_and_reaps_scoped_terminals() {
        let (dir, repo) = init_repo();
        let mut app = qa_state(&repo, dir.path());
        app.term_shell = "/bin/bash".into();
        let project_id = app.projects[0].id.clone();
        add_external_worktree(&repo, dir.path(), "terminal-finish", "terminal-finish");
        let worktree_id = external_id(&mut app, &project_id, Some("terminal-finish"));
        let state = app.shared();
        let handler = AppState::handler(Arc::clone(&state));
        let created = handler(
            SessionSender::detached("s1"),
            req(
                "term.create",
                json!({ "project_id": project_id, "worktree_id": worktree_id }),
            ),
        );
        assert_eq!(created["ok"], true, "{created:?}");
        let term_id = created["result"]["term_id"].as_str().unwrap().to_string();
        let (term_key, pid) = {
            let app = state.lock().unwrap();
            let term_key = app.tab_key_of_wire_id(&term_id).unwrap();
            let pid = app.tabs[&term_key].session.pid().unwrap();
            (term_key, pid)
        };

        let finished = handler(
            SessionSender::detached("s1"),
            req(
                "worktree.finish",
                json!({ "project_id": project_id, "worktree_id": worktree_id, "action": "cleanup" }),
            ),
        );
        assert_eq!(finished["ok"], true, "{finished:?}");
        assert!(!state.lock().unwrap().tabs.contains_key(&term_key));
        assert!(
            process_reaped(pid),
            "scoped terminal must be killed and reaped"
        );
    }
}
