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

mod captures;
mod config;
mod projects;

#[cfg(test)]
use crate::orchestrator::Orchestrator;
#[cfg(test)]
use crate::templates::Templates;
use captures::RouteRecorded;
pub use captures::RoutedCapture;
#[cfg(test)]
use captures::{capture_after_routing, RoutedIssueDrafting};
pub use config::ConfigError;
pub(crate) use config::{announce_isolation_downgrade, expand_tilde};
use config::{default_state_root, DEFAULT_HARNESS};
#[cfg(test)]
use config::{read_config, ConfigPersistStep};
use projects::{default_projects_dir, Project};
pub use projects::{ProjectAdded, ProjectRemoteSet};

use std::collections::HashMap;
use std::io::Read as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use portable_pty::PtySize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

use crate::agent_modes::AgentModes;
use crate::carrier::{FrameHandler, SessionSender};
use crate::changes::{ChangeBus, ANNOUNCED_EVENTS, DEFAULT_COALESCE_WINDOW};
use crate::delivery::{AgentSpawnPlan, ReadyToSpawn, SessionProbes};
use crate::encoding::b64decode;
use crate::harness::{
    harness_for, open_session, open_terminal_session, AgentSession, AgentStatus, HarnessContext,
    SessionIdentitySource, SessionOpenRequest, SessionOutput, TerminalOpenOptions, Turn,
    TurnChoiceSupport,
};
use crate::isolation::{Isolation, IsolationAvailability};
use crate::lifecycle::holders::{BranchHolder, ProjectCheckouts};
#[cfg(test)]
use crate::lifecycle::{fail_dispatch_at, BranchDispatchStep};
use crate::lifecycle::{
    AdoptCheckout, AdoptImplementation, AdoptionTarget, CreateWorktree, DiscardCheckout,
    DiscardedCheckout, DispatchCheckout, DispatchTarget, ImplementationCheckout, LifecycleEpilogue,
    LifecycleOutcome, OpenImplementation, OpenPlanWorkspace, PendingRow, Performed,
    RestoreImplementationCheckout, WorktreeChange, WorktreeLifecycleJob, WorktreeMutation,
};
use crate::mcp::{BridgeAction, CommentResolution, DoneOutputs, DonePhase, DoneReport, DoneStatus};
use crate::models::{self, AgentProvider, ModelChoice};
use crate::notify::{Notifier, NotifyThrottle};
use crate::operation::{OperationReceipt, OperationStatus};
use crate::orchestrator::{
    ActivePlan, ActiveRun, AdoptableCheckout, AdoptionScope, Agent, AgentTurn, ImplementableIssue,
    OrchestratorError, PreparedAgentLaunch, PreparedImplementation, ReportConsumed, ReportOutcome,
    ResumeIdProbe, RunSource, SessionLocatorFactory, SpawnOptions,
};
use crate::plan::StageManifestEntry;
use crate::plan::{
    ImplementationActivity, ImplementationIntent, PlanEvent, PlanId, PlanState, StageDoc,
    StageDocState,
};
use crate::pty::HarnessSpec;
use crate::reaper::Retirement;
use crate::rtc::{NoPeerFactory, SessionPeerFactory, SessionPeers};
use crate::run::ValidationReport;
use crate::run::{
    run_transition, PublicationAttempt, RunEvent, RunId, RunState, StageProgress,
    StageProgressState, StagePublication,
};
use crate::screen::{AttachSnapshot, ScreenHandle, TerminalHandle, TERM_FLUSH_MS};
use crate::store::{
    now_rfc3339, PersistedArchivedWorktree, PersistedPlan, PersistedRun, Store,
    WorktreeFinishAction, WorktreeFinishStatus,
};
use crate::templates::STAGES_MANIFEST_PATH;
pub use crate::terminal_environment::{capture_login_path, resolve_term_shell};
use crate::thread::{SessionInstance, SessionStart, ThreadDetail};
use crate::timing::{FrameClock, FrameTimer};
use crate::transport::{self, Frame};
use crate::worktree::{
    bounded_git_fetch, configured_remote_for_branch, git_remote_origin, git_stdout,
    ExternalWorktree, Worktree, WorktreeManager,
};
pub(crate) use crate::{encoding::b64encode, fs_scope::fenced_scope_path};

mod conversations;
mod qa;
pub use conversations::ATTACHMENT_MAX_BYTES;
use conversations::{
    append_user_thread_messages, apply_thread_action, locate_conversations, media_mime_hint,
    mime_hint, parse_thread_inputs, thread_cursor, thread_detail, view_thread_detail,
    with_post_receipt, ReadReport,
};
use qa::write_in_dir;

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
        // Any provider Build knows how to dispatch, not a list kept here: a
        // client asking for a harness by name gets the same answer whichever
        // one it named, including one added after this client shipped.
        Some(named_agent) if AgentProvider::from_wire(named_agent).is_some() => Err(format!(
            "a user terminal cannot run {named_agent} — an agent is created with \
             agent.add and lives in the agent rail, which is what makes every \
             agent in a worktree Build-owned"
        )),
        Some(other) => Err(format!(
            "unknown terminal kind {other:?} — a user terminal is always the shell"
        )),
    }
}

/// Refuse a terminal call on an agent whose session has no terminal.
///
/// The terminal is the escape hatch into a harness Build can only see the
/// outside of. A harness that reports its own reasoning and tool calls is not
/// opaque, so it has nothing to escape to and offers no basement to drop into
/// — and a client that asks anyway is told where that agent's work actually is.
///
/// Same precedent as [`require_shell_kind`], for the same reason: falling back
/// would attach a grid nothing paints into, or answer `ok` to keystrokes no
/// process will ever read, which is the silent-wrong-thing failure loud
/// refusals exist to prevent.
fn no_terminal_here(term_id: &str) -> String {
    format!(
        "{term_id} has no terminal — this agent reports its reasoning, tool calls and \
         messages into its conversation, which is where its work is read"
    )
}

/// The harness a shell tab spawns in its worktree root: `-i -l`, so the user
/// gets their own rc files and prompt — their machine, shown honestly.
fn shell_harness_spec(shell: &str) -> HarnessSpec {
    HarnessSpec::new(shell)
        .arg("-i")
        .arg("-l")
        .env("TERM", "xterm-256color")
}

fn terminal_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// A worktree-backed surface a terminal or fs call is scoped to. Scope roots are
/// resolved server-side ONLY (spec §1): ids map to roots through the bridge's own
/// records — a client-supplied filesystem path is never a scope root.
#[derive(Debug, Clone)]
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

/// At most this many user terminals daemon-wide, all worktrees combined. An
/// agent tab never counts against it — there is at most one per worktree, and
/// it must stay reachable however many shells are open.
const MAX_USER_TERMINALS: usize = 16;

/// Source/document previews stay tightly capped; playable media gets a larger
/// bounded response because browsers cannot decode a truncated data URL.
const FS_READ_MAX_BYTES: u64 = 1_048_576;
const FS_MEDIA_READ_MAX_BYTES: u64 = 32 * 1_048_576;

/// The tab id of one Build-owned agent. Every other tab in a worktree is a
/// `term-<n>` shell the human drives.
///
/// Keyed by the AGENT, not by the worktree: a branch can carry several agents
/// sharing one checkout (spec: UX Redesign Decisions, "Agents and
/// conversations"), and each of them needs its own PTY.
fn agent_tab_id(agent_id: &str) -> String {
    format!("agent:{agent_id}")
}

/// A tab's identity: the canonical worktree it is rooted in, and which tab of
/// that worktree it is.
///
/// Canonical because the same worktree reaches the daemon under three different
/// scope shapes (run / external / primary) and, on macOS, under two different
/// literal paths (`/tmp` is `/private/tmp`). Keying by path rather than by
/// entity id is what keeps an agent's PTY where the human left it as the entity
/// around it is adopted, released and re-adopted.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TabKey {
    root: std::path::PathBuf,
    tab_id: String,
}

impl TabKey {
    /// The key of one agent's tab in `root`. `root` must already be canonical —
    /// see [`AppState::canonical_root`].
    fn agent(root: &std::path::Path, agent_id: &str) -> TabKey {
        TabKey {
            root: root.to_path_buf(),
            tab_id: agent_tab_id(agent_id),
        }
    }

    /// Whether this key names an agent rather than one of the human's shells.
    fn is_agent(&self) -> bool {
        self.tab_id.starts_with("agent:")
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
    /// One of Build's agents in this worktree. `owner` is the opaque plan/run
    /// id whose lifecycle this agent drives; `agent_id` is the identity baked
    /// into the harness's `mcp --task <id>` argv, so a `done` report names the
    /// agent that sent it and routes to the entity through it. `provider` is
    /// what was spawned.
    Agent {
        owner: String,
        agent_id: String,
        provider: AgentProvider,
    },
}

impl TabRole {
    /// The entity this tab's agent drives and the agent's own id — `None` for
    /// the human's own shell, which drives nothing.
    fn agent(&self) -> Option<(&str, &str)> {
        match self {
            TabRole::Agent {
                owner, agent_id, ..
            } => Some((owner, agent_id)),
            TabRole::Shell => None,
        }
    }
}

/// A live tab: one agent session rooted in a worktree, plus the authoritative
/// screen model that makes reconnect a snapshot (current screen + cursor)
/// rather than a byte replay.
///
/// The session is held behind [`AgentSession`], so nothing a tab does knows
/// which harness — or which kind of session — is on the other end.
struct Tab {
    tab_id: String,
    root: std::path::PathBuf,
    role: TabRole,
    /// Surfaced by `term.list` so a reloaded client can order the tab row the
    /// way the human opened it.
    created_at: String,
    /// Shared rather than owned outright: a turn is handed over with the
    /// app-wide state lock RELEASED, so the delivery takes a handle out of the
    /// registry instead of holding the registry open across the turn.
    session: Arc<dyn AgentSession>,
    /// Exact conversation-lineage row opened for this process. Captured once
    /// at publication and carried by every callback; never rediscovered from
    /// whichever session is newest when the callback finally runs.
    session_instance: Option<SessionInstance>,
    /// The grid this tab's terminal paints into — `None` for a session with no
    /// terminal, because there is no grid without one. The terminal is a
    /// capability, not a guarantee, and a screen kept for a session that has
    /// none would be a second answer to a question with one:
    /// [`AgentSession::terminal`](crate::harness::AgentSession::terminal).
    screen: Option<ScreenHandle>,
    /// False once the PTY stream has ended. An agent tab is RETAINED after its
    /// process dies so the tab still shows the last screen; a shell tab is
    /// removed by its pump instead, so this is only ever false for an agent.
    live: bool,
    call_sequences: HashMap<String, MintedCallRow>,
    /// When Build last submitted a turn here.
    ///
    /// The quiescence rule ("silence is an anomaly, never completion") used to
    /// read a phase session that was killed at every gate, so silence really
    /// was anomalous. A tab's agent outlives every phase and spends most of its
    /// life idle at a prompt, so silence only means something measured from the
    /// last thing Build asked of it.
    last_delivered_at: Option<std::time::Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MintedCallRow {
    sequence: u64,
    answered: bool,
}

struct LifecycleDiagnostic<'a> {
    event: &'a str,
    origin: &'a str,
    reason: Option<&'a str>,
    operation_id: Option<&'a str>,
    provider_thread_id: Option<&'a str>,
    caller: Option<&'a std::panic::Location<'a>>,
}

impl Tab {
    fn log_lifecycle(&self, diagnostic: LifecycleDiagnostic<'_>) {
        let Some((owner_id, agent_id)) = self.role.agent() else {
            return;
        };
        let ts_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or_default();
        let mut entry = json!({
            "component": "app",
            "event": diagnostic.event,
            "ts_utc": now_rfc3339(),
            "ts_unix_ms": ts_unix_ms,
            "agent_id": agent_id,
            "owner_id": owner_id,
            "origin": diagnostic.origin,
        });
        if let Some(instance) = &self.session_instance {
            entry["session_instance_id"] = json!(instance.id);
            entry["conversation_id"] = json!(instance.conversation_id);
        }
        if let Some(reason) = diagnostic.reason {
            entry["reason"] = json!(reason);
        }
        if let Some(operation_id) = diagnostic.operation_id {
            entry["operation_id"] = json!(operation_id);
        }
        if let Some(provider_thread_id) = diagnostic.provider_thread_id {
            entry["provider_thread_id"] = json!(provider_thread_id);
        }
        if let Some(caller) = diagnostic.caller {
            entry["caller"] = json!(format!("{}:{}", caller.file(), caller.line()));
        }
        eprintln!("build_lifecycle {entry}");
    }

    /// The wire id this tab is demuxed by on the shared terminal socket:
    /// `term-<n>` for a shell, `agent:<agent_id>` for an agent. An agent is
    /// addressed by its own durable identity, never by the run that happens to
    /// own it — that is what lets adoption, release and re-adoption leave the
    /// human's tab where it was, and what lets two agents share a checkout.
    fn wire_id(&self) -> String {
        match &self.role {
            TabRole::Shell => self.tab_id.clone(),
            TabRole::Agent { agent_id, .. } => agent_tab_id(agent_id),
        }
    }

    /// Whether this tab's agent session is still running.
    ///
    /// Two conjuncts, each ruling out a different corpse. An agent tab is
    /// RETAINED after its stream ends so the human still sees the last screen,
    /// so `live` is the tab's own answer; and a session that reports `Ended` is
    /// over whatever the tab still holds. `has_exited` was how a terminal asked
    /// the second — a process poll — and [`AgentStatus::Ended`] is how every
    /// session does.
    fn session_is_live(&self) -> bool {
        self.live && !matches!(self.session.status(), AgentStatus::Ended { .. })
    }

    /// The terminal and the grid it paints into, owned rather than borrowed:
    /// the caller takes it out of the registry and writes to it with the app
    /// mutex released, which is what keeps a child that stopped draining its
    /// PTY from wedging the daemon.
    ///
    /// One question answers for both halves: they are made together in
    /// [`Tab::spawn`] and a session with no terminal has neither, so there is
    /// no state in which a tab has a screen to hand a client and nothing
    /// behind it.
    fn terminal_handle(&self) -> Result<TerminalHandle, String> {
        TerminalHandle::of(&self.session, &self.screen)
            .ok_or_else(|| no_terminal_here(&self.wire_id()))
    }

    /// Paint this session onto the grid its predecessor left behind.
    ///
    /// Reconnect is snapshot + cursor: a replacement process must never rewind
    /// that cursor, and clients already attached stay attached. The new PTY
    /// takes the retained grid so the two agree — and a replacement that paints
    /// nothing has no grid to become, so the clients on it are told rather than
    /// left there (see [`NO_TERMINAL_LEFT`]).
    fn adopt_screen(&mut self, screen: ScreenHandle) {
        self.screen = Some(screen);
        let Ok(terminal) = self.terminal_handle() else {
            if let Some(orphan) = self.screen.take() {
                orphan.close(NO_TERMINAL_LEFT);
            }
            return;
        };
        terminal.fit_child_to_screen();
    }

    /// Everything a tab's pumps need, taken before the tab is handed to the
    /// registry: they run for the tab's whole life and must not have to ask
    /// the registry for the handles they hold.
    fn pumps(&self, output: SessionOutput) -> TabPumps {
        TabPumps {
            session: Arc::clone(&self.session),
            session_instance: self.session_instance.clone(),
            screen: self.screen.clone(),
            output,
        }
    }

    /// Open an agent's session through its provider and wrap it in a tab, with
    /// the output subscribed before the first word can be missed.
    ///
    /// The grid is made together with the terminal, or not at all: a session
    /// with no terminal paints nothing, so there is no screen to hold and no
    /// byte pump to run — its work reaches the conversation through the
    /// activity pump instead.
    fn spawn_agent(
        owner: String,
        agent_id: String,
        request: SessionOpenRequest,
    ) -> Result<(Tab, SessionOutput), String> {
        let provider = request.choice.provider;
        let tab_id = agent_tab_id(&agent_id);
        let root = request.root.clone();
        let size = request.terminal.size;
        let opened = open_session(provider, request).map_err(|error| error.to_string())?;
        Ok(Self::from_opened_session(
            TabRole::Agent {
                owner,
                agent_id,
                provider,
            },
            tab_id,
            root,
            size,
            opened,
        ))
    }

    /// The human's own shell: always a terminal, and the one session never
    /// handed a turn, so it is not waited on — a login shell may never announce
    /// a line editor at all and `term.create` holds the state lock across this.
    fn spawn_shell(
        spec: &HarnessSpec,
        tab_id: String,
        root: std::path::PathBuf,
        size: PtySize,
    ) -> Result<(Tab, SessionOutput), String> {
        let opened = open_terminal_session(
            spec,
            root.clone(),
            TerminalOpenOptions {
                size,
                turn_ready_grace: None,
                identity: None,
            },
        )
        .map_err(|error| error.to_string())?;
        Ok(Self::from_opened_session(
            TabRole::Shell,
            tab_id,
            root,
            size,
            opened,
        ))
    }

    fn from_opened_session(
        role: TabRole,
        tab_id: String,
        root: std::path::PathBuf,
        size: PtySize,
        opened: crate::harness::OpenedSession,
    ) -> (Tab, SessionOutput) {
        let (cols, rows) = (size.cols, size.rows);
        let session = opened.session;
        let screen = session
            .terminal()
            .map(|_| ScreenHandle::new(&tab_id, cols, rows));
        (
            Tab {
                tab_id,
                root,
                role,
                created_at: now_rfc3339(),
                screen,
                session,
                session_instance: None,
                live: true,
                call_sequences: HashMap::new(),
                last_delivered_at: None,
            },
            opened.output,
        )
    }
}

/// What a tab's pumps run on: the session they watch, the screen they paint
/// into, and the streams they read.
///
/// Taken off the tab before it is handed to the registry, so a pump holds
/// everything it needs for the tab's whole life and never asks the app mutex
/// for it. The session travels because the EOF a pump sees belongs to the
/// session it was started for and to no replacement that took the tab since.
struct TabPumps {
    session: Arc<dyn AgentSession>,
    session_instance: Option<SessionInstance>,
    screen: Option<ScreenHandle>,
    output: SessionOutput,
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

/// The agent a verb's parameters name, resolved whole.
///
/// Five readings that only make sense together and are only ever taken
/// together: which entity, which of its agents, the checkout that agent works
/// in, the harness it runs, and whether its thread holds anything it has not
/// been told about. A verb that has one of these has everything it needs to
/// queue a turn and to answer.
struct AddressedAgent {
    entity_id: String,
    agent_id: String,
    conversation_id: String,
    root: std::path::PathBuf,
    model_choice: ModelChoice,
    choice_revision: u64,
    has_unread: bool,
}

struct AgentSpawnRequest<'a> {
    owner: &'a str,
    agent_id: &'a str,
    conversation_id: &'a str,
    model_choice: &'a ModelChoice,
    force_fresh: bool,
    phase: &'a str,
}

/// Who is on the other end of an authenticated MCP control frame.
///
/// The kind decides the tool surface, and it is read off the identity the frame
/// authenticated with — never off the frame's own claim about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AddressedSession {
    /// An agent working a plan or a run.
    Coding { entity_id: String, agent_id: String },
    /// A router deciding where one capture goes.
    Router {
        capture_id: String,
        agent_id: String,
    },
}

/// An [`AgentTurn`] addressed to a worktree, waiting for the state lock to be
/// free.
///
/// Every lifecycle verb runs inside `state.lock().unwrap().dispatch(..)`, and
/// [`deliver`] takes that same lock and holds nothing while it blocks for
/// seconds spawning a cold harness. So a verb records what it wants said and
/// [`dispatch_frame`] — which holds the `Arc` and no guard — sends it the moment
/// the verb returns.
struct PendingAgentTurn {
    /// Durable reviewer operation this turn delivers. Lifecycle turns predate
    /// operation receipts and carry none.
    operation_id: Option<String>,
    /// The worktree the agent that hears this turn works in. Canonical at
    /// construction — every site that builds a turn passes it through
    /// `AppState::canonical_root` — so [`Self::tab_key`] is a field read and
    /// makes no filesystem call under the app mutex.
    root: std::path::PathBuf,
    /// The plan/run whose lifecycle this turn moves.
    owner: String,
    /// The agent that hears it. Entity-level work addresses the entity's first
    /// agent; a verb the rail addressed names the agent whose bubble was open.
    agent_id: String,
    /// Canonical conversation binding captured with the addressed agent. A
    /// queued turn must not follow an agent id after that binding was removed
    /// or changed while the delivery sat outside the app lock.
    conversation_id: String,
    model_choice: ModelChoice,
    choice_revision: u64,
    interrupt: bool,
    /// What to say once the tab is open — `None` for a turn that only wants
    /// the agent there.
    ///
    /// "Start this agent" and "tell this agent something" are one job with one
    /// queue: the tab has to exist either way, and the spawn is the same spawn.
    /// The difference is whether anything is written into it afterwards.
    say: Option<TurnText>,
    /// The phase recorded on the conversation's session lineage if the turn
    /// turns out to be cold — a cold delivery is a new agent process.
    phase: &'static str,
    /// Whether `cold` is closed with the durable conversation — the catch-up
    /// packet and the previous completion report — when the turn is handed
    /// over.
    ///
    /// True for every turn addressed to an entity's conversation, false for
    /// the router's: a router is one decision long, works no conversation, and
    /// its prompt deliberately carries none.
    wants_catch_up: bool,
    /// Whether this turn outlives a refusal of the request that queued it.
    ///
    /// False for almost everything: a turn speaks for a mutation, and a request
    /// that failed wrote no mutation to speak for. A recovery is the exception
    /// — it is written down and started, and the verb then refuses its caller
    /// to say exactly that, so the agent handed the recovery must still hear
    /// it.
    survives_refusal: bool,
}

/// The two halves of one turn's text: which one travels is decided by whether
/// the tab had to be spawned to hear it.
///
/// `cold` carries the full run context, because an agent that was just started
/// has none to read the words into; `warm` is the bare instruction, because a
/// live agent is already in the conversation and everything said to it is
/// already durable on the thread for `read_unread_messages` to pull.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TurnText {
    cold: String,
    warm: String,
}

/// The live implementation an Issue's conversation actually speaks to: the
/// Issue owns the words, its implementation owns the checkout and the PTY they
/// have to reach. Read off the run before the Issue is taken out of the map, so
/// the agent can be woken — or brought back — without borrowing it again.
struct ImplementationTarget {
    run_id: String,
    worktree_path: std::path::PathBuf,
    agent_id: String,
    model_choice: ModelChoice,
    choice_revision: u64,
}

impl PendingAgentTurn {
    fn for_delivery_operation(receipt: &OperationReceipt) -> Option<Self> {
        let delivery = receipt.delivery.as_ref()?;
        let payload = delivery.payload.as_ref()?;
        Some(Self {
            operation_id: Some(receipt.operation_id.clone()),
            root: AppState::canonical_root(&delivery.root),
            owner: delivery.owner_id.clone(),
            agent_id: delivery.agent_id.clone(),
            conversation_id: receipt.conversation_id.clone(),
            model_choice: delivery.model_choice.clone(),
            choice_revision: delivery.choice_revision,
            interrupt: delivery.interrupt,
            say: Some(TurnText {
                cold: payload.delivery_prompt(&receipt.operation_id, true),
                warm: payload.delivery_prompt(&receipt.operation_id, false),
            }),
            phase: "revive",
            wants_catch_up: false,
            survives_refusal: true,
        })
    }

    /// What this turn says, for a test that queued one that says something.
    #[cfg(test)]
    fn said(&self) -> &TurnText {
        self.say.as_ref().expect("this turn carries text")
    }

    /// Whether the agent is TOLD anything once the tab is open. A turn that
    /// says nothing opens a harness and sends it nothing, so it promises the
    /// agent nothing to read.
    fn says_something(&self) -> bool {
        self.say.is_some()
    }

    /// The registry entry this turn is on its way to. The same key
    /// [`ensure_agent_tab`] will reserve, so a turn in the queue, a turn
    /// mid-delivery and a spawn in flight are all one agent's under one name.
    fn tab_key(&self) -> TabKey {
        TabKey::agent(&self.root, &self.agent_id)
    }

    /// Address a run's turn to the run's worktree. Canonical, because the same
    /// worktree reaches the tab registry under several scope shapes.
    ///
    /// The turn goes to the run's primary agent, and mints one on a roster the
    /// human emptied: this is Build about to run something, and something Build
    /// runs must be heard by somebody.
    fn for_run(owner: &str, active: &mut ActiveRun, turn: AgentTurn) -> Self {
        let choice = active.model_choice.clone();
        let agent_id = active
            .agents
            .ensure_primary(owner, choice, &now_rfc3339())
            .id
            .clone();
        Self::for_run_agent(owner, &agent_id, active, turn)
    }

    /// The same, for a caller that knows which of the run's agents it means:
    /// `run.request_changes` addresses the agent whose conversation the
    /// reviewer was reading, and — when that is the first one — swaps the run's
    /// roster for its Issue's so the comments land on the conversation the
    /// Issue renders. Either way the turn must reach the run's OWN agent.
    fn for_run_agent(owner: &str, agent_id: &str, active: &ActiveRun, turn: AgentTurn) -> Self {
        let agent = active
            .agents
            .by_id(agent_id)
            .expect("a run turn is addressed to one of its agents");
        PendingAgentTurn {
            operation_id: None,
            root: AppState::canonical_root(&active.worktree.path),
            owner: owner.to_string(),
            agent_id: agent_id.to_string(),
            conversation_id: agent.conversation_id().to_string(),
            model_choice: agent.choice.clone(),
            choice_revision: agent.choice_revision,
            interrupt: false,
            say: Some(TurnText {
                cold: turn.cold,
                warm: turn.warm,
            }),
            phase: turn.phase,
            wants_catch_up: true,
            survives_refusal: false,
        }
    }

    /// Address a plan's turn to the primary checkout its planning agent runs
    /// in. `None` once the workspace is gone (approve/abandon drop it): a plan
    /// with no workspace has no agent, and every plan surface renders the empty
    /// state rather than a tab that cannot exist.
    fn for_plan(owner: &str, active: &ActivePlan, turn: AgentTurn) -> Option<Self> {
        let workspace = active.workspace.as_ref()?;
        let agent_id = active.agents.sole().id.clone();
        Some(PendingAgentTurn {
            operation_id: None,
            root: AppState::canonical_root(&workspace.checkout),
            owner: owner.to_string(),
            conversation_id: active.agents.sole().conversation_id().to_string(),
            model_choice: active.agents.sole().choice.clone(),
            choice_revision: active.agents.sole().choice_revision,
            interrupt: false,
            agent_id,
            say: Some(TurnText {
                cold: turn.cold,
                warm: turn.warm,
            }),
            phase: turn.phase,
            wants_catch_up: true,
            survives_refusal: false,
        })
    }

    /// Address a recovery to the run's primary agent. `None` on an agentless
    /// run: a recovery only exists for an entity that has run, so this is a
    /// refusal rather than a case — and refusing beats minting an agent to
    /// hand a recovery nobody asked for.
    fn for_recovery(
        owner: &str,
        active: &ActiveRun,
        project_root: &std::path::Path,
        prompt: String,
    ) -> Option<Self> {
        // A recovery may replace a dead process or take over a warm
        // implementation tab. In either case it is a distinct Issue agent and
        // must re-establish the durable conversation protocol before touching
        // refs. Wrapping both variants also gives a warm recovery the unread
        // pull instruction instead of assuming an earlier phase primed it.
        //
        // Only the cold half is closed with the catch-up packet at delivery: a
        // warm recovery is a live process that lived this conversation, and
        // the protocol block it keeps already tells it to read what it missed.
        let primed = crate::orchestrator::conversation_prompt(&prompt);
        let agent_id = active.agents.primary()?.id.clone();
        Some(PendingAgentTurn {
            operation_id: None,
            root: AppState::canonical_root(project_root),
            owner: owner.to_string(),
            conversation_id: active
                .agents
                .primary()
                .expect("the recovery agent was just resolved")
                .conversation_id()
                .to_string(),
            model_choice: active
                .agents
                .primary()
                .expect("the recovery agent was just resolved")
                .choice
                .clone(),
            choice_revision: active
                .agents
                .primary()
                .expect("the recovery agent was just resolved")
                .choice_revision,
            interrupt: false,
            agent_id,
            say: Some(TurnText {
                cold: primed.clone(),
                warm: primed,
            }),
            phase: "recover",
            wants_catch_up: true,
            survives_refusal: true,
        })
    }
}

/// External-worktree scans are refreshed at most this often per project; the
/// board polls task.list every ~1.6 s and must never trigger a full rescan per
/// poll.
const EXTERNAL_SCAN_INTERVAL: Duration = Duration::from_secs(10);

/// Whether a tab holds an agent that is working right now.
///
/// Three things have to be true, and each rules out a different lie: the tab
/// is an agent's (a shell is the human's own hands, however busy it looks), its
/// stream is still open (a dead agent's retained screen is not a heartbeat),
/// and the session itself reports [`AgentStatus::Working`].
///
/// That last one used to be the age of the last paint, read straight off the
/// PTY. It is now the session's own answer, because the paint clock is a guess
/// only a terminal is forced to make — a harness that knows when its turn began
/// and ended has a better one, and must be able to give it. For a PTY the guess
/// is unchanged: [`crate::pty::PtySession`] synthesizes `Working` from exactly
/// the two conjuncts that moved, so this reports what it always has.
fn agent_is_working(tab: &Tab) -> bool {
    matches!(tab.role, TabRole::Agent { .. })
        && tab.live
        && matches!(tab.session.status(), AgentStatus::Working)
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

/// The `state` a branch row reports when nothing is driving it: a checkout
/// exists on that branch, and no run owns its lifecycle.
const CHECKOUT_IDLE_STATE: &str = "idle";

/// One inbox row that no entity stands behind: a project's primary checkout, or
/// a branch checked out somewhere Build never cut.
///
/// It has no entity id, so it is identified by what it is, and no conversation,
/// so the line a dismissal draws is the commit it is sitting on.
struct EntitylessRow {
    /// Where the dismissal is written in the attention map.
    key: String,
    /// The commit the row is on, as the feed reads it. `None` when its history
    /// cannot be read at all.
    head: Option<String>,
    project_id: String,
    /// The branch the row shows, or `None` for a detached checkout.
    branch: Option<String>,
    /// Whether this is the project's own checkout — the repository itself.
    primary: bool,
}

/// The +/− block every work-item row carries, in one shape whatever source it
/// was read off, plus the facts Done warns about (see
/// [`crate::branch::branch_finish_warnings`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct WorkItemStat {
    /// Everything the branch carries against its base.
    files_changed: u64,
    insertions: u64,
    deletions: u64,
    /// What is sitting in the tree unsaved.
    uncommitted_files: u64,
    uncommitted_insertions: u64,
    uncommitted_deletions: u64,
    ahead: Option<u64>,
    behind: Option<u64>,
    upstream: Option<String>,
    /// The one ref both counts are measured against: the tracking branch when
    /// the branch has one, otherwise the project's base branch. It is what
    /// makes ahead/behind readable — "3 unpushed" and "3 unmerged" are the same
    /// number against different refs, and Done warns differently about each.
    comparison_ref: Option<String>,
    /// When this branch last got a commit (RFC 3339), or `None` when the
    /// checkout could not be read.
    head_committed_at: Option<String>,
}

impl WorkItemStat {
    /// Read off a run's cached diffstat ([`AppState::run_stat`]). A terminal
    /// run reports a null stat, which reads as an empty one.
    fn from_run_stat(stat: &Value) -> Self {
        Self {
            files_changed: stat["files_changed"].as_u64().unwrap_or(0),
            insertions: stat["insertions"].as_u64().unwrap_or(0),
            deletions: stat["deletions"].as_u64().unwrap_or(0),
            uncommitted_files: stat["uncommitted"]["files_changed"].as_u64().unwrap_or(0),
            uncommitted_insertions: stat["uncommitted"]["insertions"].as_u64().unwrap_or(0),
            uncommitted_deletions: stat["uncommitted"]["deletions"].as_u64().unwrap_or(0),
            ahead: stat["ahead"].as_u64(),
            behind: stat["behind"].as_u64(),
            upstream: stat["upstream"].as_str().map(str::to_string),
            comparison_ref: stat["comparison_ref"].as_str().map(str::to_string),
            head_committed_at: stat["head_committed_at"].as_str().map(str::to_string),
        }
    }

    /// Read off one entry of the external-worktree scan. `dirty_files` is the
    /// status count, so it sees untracked files the diff cannot.
    fn from_external_entry(entry: &Value) -> Self {
        Self {
            files_changed: entry["diffstat"]["files_changed"].as_u64().unwrap_or(0),
            insertions: entry["diffstat"]["insertions"].as_u64().unwrap_or(0),
            deletions: entry["diffstat"]["deletions"].as_u64().unwrap_or(0),
            uncommitted_files: entry["dirty_files"].as_u64().unwrap_or(0),
            uncommitted_insertions: entry["uncommitted"]["insertions"].as_u64().unwrap_or(0),
            uncommitted_deletions: entry["uncommitted"]["deletions"].as_u64().unwrap_or(0),
            ahead: entry["ahead"].as_u64(),
            behind: entry["behind"].as_u64(),
            upstream: entry["upstream"].as_str().map(str::to_string),
            comparison_ref: entry["comparison_ref"].as_str().map(str::to_string),
            head_committed_at: entry["head_committed_at"].as_str().map(str::to_string),
        }
    }

    /// Read off one entry of the primary-changes summary, whose counts are the
    /// working tree against HEAD — uncommitted work, and all a checkout with no
    /// base to compare against can honestly report.
    fn from_primary_entry(entry: &Value) -> Self {
        let files_changed = entry["files_changed"].as_u64().unwrap_or(0);
        let insertions = entry["insertions"].as_u64().unwrap_or(0);
        let deletions = entry["deletions"].as_u64().unwrap_or(0);
        Self {
            files_changed,
            insertions,
            deletions,
            uncommitted_files: files_changed,
            uncommitted_insertions: insertions,
            uncommitted_deletions: deletions,
            ahead: entry["ahead"].as_u64(),
            behind: entry["behind"].as_u64(),
            upstream: entry["upstream"].as_str().map(str::to_string),
            comparison_ref: entry["comparison_ref"].as_str().map(str::to_string),
            head_committed_at: entry["head_committed_at"].as_str().map(str::to_string),
        }
    }

    fn sync(&self) -> crate::branch::BranchSync {
        crate::branch::BranchSync {
            uncommitted_files: self.uncommitted_files,
            ahead: self.ahead,
            upstream: self.upstream.clone(),
            comparison_ref: self.comparison_ref.clone(),
        }
    }

    /// What Done on this branch is about to lose, ready for the wire.
    fn finish_warnings_json(&self, branch: &str) -> Value {
        crate::branch::warnings_json(&crate::branch::branch_finish_warnings(branch, &self.sync()))
    }

    fn to_json(&self) -> Value {
        json!({
            "files_changed": self.files_changed,
            "insertions": self.insertions,
            "deletions": self.deletions,
            "uncommitted": {
                "files_changed": self.uncommitted_files,
                "insertions": self.uncommitted_insertions,
                "deletions": self.uncommitted_deletions,
            },
            "ahead": self.ahead,
            "behind": self.behind,
            "upstream": self.upstream,
            "comparison_ref": self.comparison_ref,
        })
    }
}

/// How long the turn in flight has been running, for the toolbar's clock.
/// `since` is what a ticking client re-reads; `seconds` is the same fact
/// resolved against the bridge's clock, so a client with a skewed one still
/// agrees. Null when nothing is being worked.
fn working_time_json(since: Option<&str>) -> Value {
    let Some(since) = since else {
        return Value::Null;
    };
    json!({ "since": since, "seconds": seconds_since(since) })
}

/// Whole seconds between an RFC 3339 timestamp and now, never negative.
/// `None` when the timestamp cannot be parsed.
fn seconds_since(started_at: &str) -> Option<u64> {
    let started =
        time::OffsetDateTime::parse(started_at, &time::format_description::well_known::Rfc3339)
            .ok()?;
    Some(
        (time::OffsetDateTime::now_utc() - started)
            .whole_seconds()
            .max(0) as u64,
    )
}

/// What Done asks of a run before it archives the run's worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FinishRequirement {
    /// `run.finish`: the run reached a review gate — completed work.
    CompletedWork,
    /// `branch.finish`: nothing. Done on a branch deletes it, and what that
    /// costs is reported as warnings on the row (see
    /// [`crate::branch::branch_finish_warnings`]) for the user to confirm
    /// through. The bridge does not second-guess a confirmed decision.
    Unconditional,
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
    ("plan.message", "plan_id"),
    ("plan.abandon", "plan_id"),
    ("run.request_changes", "run_id"),
    ("run.message", "run_id"),
    ("run.git_action", "run_id"),
    ("run.stage_dispatch", "run_id"),
    ("run.stage_send_notes", "run_id"),
    ("run.set_auto_advance", "run_id"),
    ("run.abandon", "run_id"),
    ("run.release", "run_id"),
    ("run.adopt", "worktree_id"),
    ("thread.post", "entity_id"),
];

/// The entity ids a frame names — in the params it was called with, and in the
/// result it produced. Order-preserving and deduped.
///
/// One list rather than a per-verb table, because the surface addresses an
/// entity three ways: by a bare `id` where the caller holds one entity and
/// knows nothing else about it, by its kind (`issue_id` / `run_id` /
/// `worktree_id`) where the verb is that kind's, and out of the result where
/// the call is what minted it. `project_id` is deliberately absent: a project
/// is not an entity a browser holds a detail view of.
fn entity_ids_of(params: &Value, result: &Value) -> Vec<String> {
    const ENTITY_KEYS: [&str; 6] = [
        "id",
        "entity_id",
        "issue_id",
        "plan_id",
        "run_id",
        "worktree_id",
    ];
    let mut ids: Vec<String> = Vec::new();
    for source in [params, result] {
        for key in ENTITY_KEYS {
            let Some(id) = source.get(key).and_then(Value::as_str) else {
                continue;
            };
            if id.is_empty() || ids.iter().any(|seen| seen == id) {
                continue;
            }
            ids.push(id.to_string());
        }
    }
    ids
}

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

/// The board's checkout rows, and whether every project behind them has been
/// scanned at least once. A board that has not finished looking says so rather
/// than shipping an empty rail as the answer.
#[derive(Default)]
struct ExternalWorktreeRows {
    rows: Vec<Value>,
    scanning: bool,
}

/// What a reader gets back from a project's checkout scan: the last list, and
/// whether any scan attempt has settled — one that landed a list, or one that
/// found a repository this daemon could not read. An empty list with nothing
/// behind it is a board still waiting, not a project with no worktrees, and the
/// two render differently.
#[derive(Default)]
struct ScanRead {
    worktrees: Vec<ExternalWorktree>,
    settled: bool,
}

/// Why a checkout a caller named might not be in the last scan — the one
/// sentence every such refusal ends with, so "nothing has looked yet" and
/// "there is no such checkout" stop being the same answer. Both resolve on the
/// scan the missed read has already claimed.
fn scan_may_yet_show_it(settled: bool) -> &'static str {
    if settled {
        "a checkout made outside Build since the last scan is resolvable once the scan now \
         running lands"
    } else {
        "no scan of this project's checkouts has landed yet, and the scan now running settles it"
    }
}

/// One entry of the diff caches the poll surfaces read: a run's diffstat, a
/// project's external-worktree scan, a project's primary-checkout summary.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum DiffCacheKey {
    RunStat(String),
    ExternalScan(String),
    PrimarySummary(String),
}

/// A diff-cache entry to compute, carrying every input the git work needs.
///
/// It borrows nothing from [`AppState`] on purpose: a refresh runs on a thread
/// that holds no lock. That is the whole point — the 2026-08-13 wedge was a
/// poll sitting inside libgit2 for seconds with the app mutex in its hand,
/// which stopped the relay read loop and got the device declared dead.
#[derive(Clone, Debug)]
enum DiffCacheRefresh {
    RunStat {
        run_id: String,
        worktree: std::path::PathBuf,
        base_branch: String,
    },
    ExternalScan {
        project_id: String,
        /// The project's own checkout seam, cloned off the app mutex so the
        /// scan that walks every checkout runs without it.
        worktrees: WorktreeManager,
        base_branch: String,
        excluded: std::collections::HashSet<std::path::PathBuf>,
    },
    PrimarySummary {
        project_id: String,
        repo_path: std::path::PathBuf,
        base_branch: String,
    },
}

/// What a refresh computed, on its way back into the cache.
enum DiffCacheEntry {
    RunStat {
        run_id: String,
        stat: Value,
    },
    ExternalScan {
        project_id: String,
        worktrees: Vec<ExternalWorktree>,
    },
    /// The scan ran and could not read the repository. Stored so a project
    /// whose repo is gone settles instead of being walked again by every poll.
    ExternalScanUnreadable {
        project_id: String,
    },
    PrimarySummary {
        project_id: String,
        summary: Value,
    },
}

/// Called on the thread that is about to compute a diff-cache entry, before the
/// git work. The seam the stale-while-revalidate tests observe: it is how they
/// hold a compute open on purpose and check the app mutex is free while it
/// runs. `None` in production — nothing outside tests ever sets it.
type DiffComputeObserver = Arc<dyn Fn(&DiffCacheKey) + Send + Sync>;

/// Tests only: a gate the deferred git work trips as its lock-free phase
/// starts, and waits on until the test lets it go. It is how a test holds a
/// finish inside its `git worktree remove` and proves the app mutex is free
/// while it sits there. `None` in production — nothing outside tests sets it.
#[cfg(test)]
#[derive(Clone)]
pub struct OffLockGate {
    arrived: std::sync::mpsc::Sender<()>,
    /// One permit per arrival. Shared because the job clones the gate.
    permits: Arc<Mutex<std::sync::mpsc::Receiver<()>>>,
}

/// The test's end of an [`OffLockGate`].
#[cfg(test)]
struct OffLockGateHandle {
    arrivals: std::sync::mpsc::Receiver<()>,
    permits: std::sync::mpsc::Sender<()>,
}

#[cfg(test)]
impl OffLockGate {
    fn new() -> (OffLockGate, OffLockGateHandle) {
        let (arrived, arrivals) = std::sync::mpsc::channel();
        let (permits, waiting) = std::sync::mpsc::channel();
        (
            OffLockGate {
                arrived,
                permits: Arc::new(Mutex::new(waiting)),
            },
            OffLockGateHandle { arrivals, permits },
        )
    }

    /// Announce that the lock-free phase has begun, then wait to be let go.
    pub fn arrive(&self) {
        let _ = self.arrived.send(());
        let _ = self.permits.lock().unwrap().recv();
    }
}

#[cfg(test)]
impl OffLockGateHandle {
    /// Block until the work has reached its lock-free phase.
    fn wait_for_arrival(&self) {
        self.arrivals
            .recv_timeout(Duration::from_secs(30))
            .expect("the deferred work reached its lock-free phase");
    }

    /// Let one waiting (or one future) arrival through.
    fn release(&self) {
        self.permits.send(()).expect("the gate is still open");
    }
}

impl DiffCacheRefresh {
    fn key(&self) -> DiffCacheKey {
        match self {
            Self::RunStat { run_id, .. } => DiffCacheKey::RunStat(run_id.clone()),
            Self::ExternalScan { project_id, .. } => DiffCacheKey::ExternalScan(project_id.clone()),
            Self::PrimarySummary { project_id, .. } => {
                DiffCacheKey::PrimarySummary(project_id.clone())
            }
        }
    }

    /// The git work, on a thread that holds no lock. `None` means the compute
    /// failed and the cache keeps whatever it was already serving.
    fn compute(&self, observer: Option<&DiffComputeObserver>) -> Option<DiffCacheEntry> {
        if let Some(observer) = observer {
            observer(&self.key());
        }
        match self {
            Self::RunStat {
                run_id,
                worktree,
                base_branch,
            } => Some(DiffCacheEntry::RunStat {
                run_id: run_id.clone(),
                stat: run_diffstat(worktree, base_branch),
            }),
            Self::ExternalScan {
                project_id,
                worktrees,
                base_branch,
                excluded,
            } => match worktrees.discover(base_branch, excluded) {
                Ok(scanned) => Some(DiffCacheEntry::ExternalScan {
                    project_id: project_id.clone(),
                    worktrees: scanned,
                }),
                Err(e) => {
                    eprintln!("external_worktrees {project_id}: {e}");
                    Some(DiffCacheEntry::ExternalScanUnreadable {
                        project_id: project_id.clone(),
                    })
                }
            },
            Self::PrimarySummary {
                project_id,
                repo_path,
                base_branch,
            } => primary_changes_summary(project_id, repo_path, base_branch).map(|summary| {
                DiffCacheEntry::PrimarySummary {
                    project_id: project_id.clone(),
                    summary,
                }
            }),
        }
    }
}

/// One run's diffstat against its base, plus what sits uncommitted in its tree.
///
/// Counts only: this poll surface ships numbers, so it must never pay to render
/// (or even load) the worktree's patch text.
fn run_diffstat(worktree: &std::path::Path, base_branch: &str) -> Value {
    let git_state = git2::Repository::open(worktree).ok().and_then(|repo| {
        let head_ref = repo.head().ok()?;
        let checked_out_branch = head_ref.shorthand().map(str::to_string);
        let head = head_ref.peel_to_commit().ok()?;
        let head_committed_at = crate::worktree::rfc3339_from_unix(head.time().seconds());
        let comparison = crate::worktree::branch_comparison(
            &repo,
            &head,
            checked_out_branch.as_deref(),
            base_branch,
        );
        Some((checked_out_branch, comparison, head_committed_at))
    });
    let checked_out_branch = git_state
        .as_ref()
        .and_then(|(branch, _, _)| branch.as_deref());
    let comparison = git_state.as_ref().map(|(_, comparison, _)| comparison);
    let head_committed_at = git_state
        .as_ref()
        .and_then(|(_, _, committed_at)| committed_at.as_deref());
    let uncommitted = crate::diff::stat_uncommitted(worktree)
        .map(|stat| stat.to_json())
        .unwrap_or(Value::Null);
    crate::diff::stat_against_base(worktree, base_branch)
        .map(|stat| {
            json!({
                "files_changed": stat.files_changed,
                "insertions": stat.insertions,
                "deletions": stat.deletions,
                "branch": checked_out_branch,
                "comparison_ref": comparison.and_then(|value| value.reference.as_deref()),
                "upstream": comparison.and_then(|value| value.upstream.as_deref()),
                "ahead": comparison.and_then(|value| value.ahead),
                "behind": comparison.and_then(|value| value.behind),
                // When this branch last got a commit. The inbox's floor for how
                // recently the work moved, computed in the cached walk that has
                // the commit in its hand already.
                "head_committed_at": head_committed_at,
                "uncommitted": uncommitted,
            })
        })
        .unwrap_or(Value::Null)
}

/// One project's primary-checkout changes summary, minus the run ownership
/// stamped on at serve time. `None` on a failure (unborn HEAD, fs error): it
/// logs and the cache keeps what it had.
fn primary_changes_summary(
    project_id: &str,
    repo_path: &std::path::Path,
    base_branch: &str,
) -> Option<Value> {
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

    let repo = git2::Repository::open(repo_path);
    let branch = repo
        .as_ref()
        .ok()
        .and_then(|r| r.head().ok())
        .and_then(|h| h.shorthand().map(str::to_string))
        .unwrap_or_else(|| "HEAD".to_string());
    // What this checkout's history looks like right now. A bare checkout has no
    // conversation and no lifecycle, so its own commits are all the inbox has —
    // to date it by (`head_committed_at`), and to tell whether it has said
    // anything since the human cleared its row (`head_sha`). Both are computed
    // HERE, inside the cached walk, never on the poll path.
    let head_commit = repo
        .as_ref()
        .ok()
        .and_then(|repo| repo.head().ok())
        .and_then(|head| head.peel_to_commit().ok());
    let head_sha = head_commit.as_ref().map(|commit| commit.id().to_string());
    let head_committed_at = head_commit
        .as_ref()
        .and_then(|commit| crate::worktree::rfc3339_from_unix(commit.time().seconds()));
    let (upstream, comparison_ref, ahead, behind) = repo
        .as_ref()
        .ok()
        .map(|repo| head_sync_counts(repo, base_branch))
        .unwrap_or((None, None, None, None));
    match crate::diff::stat_against_head(repo_path) {
        Ok(stat) => Some(json!({
            "project_id": project_id,
            "branch": branch,
            "upstream": upstream,
            "comparison_ref": comparison_ref,
            "ahead": ahead,
            "behind": behind,
            "head_sha": head_sha,
            "head_committed_at": head_committed_at,
            "files_changed": stat.files_changed,
            "insertions": stat.insertions,
            "deletions": stat.deletions,
        })),
        Err(e) => {
            eprintln!("primary_changes {project_id}: {e}");
            None
        }
    }
}

/// One unit of work split the way the rule splits everything: `decide` runs
/// with the state lock released, `apply` under it. The caller has already
/// taken this job's single-flight claim; `claim` names it, so a decide phase
/// that never returns — a panic on the blocking pool — can still hand it back
/// through `abandon`, or that claim would never be taken again.
trait OffLockJob: Send + 'static {
    type Claim: Send + 'static;
    type Decided: Send + 'static;
    fn claim(&self) -> Self::Claim;
    /// MUST run with the state lock released.
    fn decide(self) -> Self::Decided;
    fn apply(state: &mut AppState, claim: Self::Claim, decided: Self::Decided);
    fn abandon(state: &mut AppState, claim: Self::Claim);
}

/// Run a claimed job on the runtime, off every lock, and apply it under the
/// lock when it has decided.
///
/// `spawn_blocking` on purpose: the decide phase is libgit2 walking a worktree
/// or a bounded fetch, and it must not sit on a runtime worker the relay's read
/// loop needs. Returns the job back when there is no runtime to spawn onto
/// (the synchronous unit tests), so the caller can decide what to do with it.
fn spawn_off_lock<J: OffLockJob>(state: Arc<Mutex<AppState>>, job: J) -> Result<(), J> {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return Err(job);
    };
    runtime.spawn(async move {
        let claim = job.claim();
        let decided = tokio::task::spawn_blocking(move || job.decide()).await;
        let mut app = state.lock().unwrap();
        match decided {
            Ok(decided) => J::apply(&mut app, claim, decided),
            Err(_) => J::abandon(&mut app, claim),
        }
    });
    Ok(())
}

/// A claimed diff-cache refresh on its way to the blocking pool: the git work
/// and the test seam that watches it start.
struct DiffRefreshJob {
    refresh: DiffCacheRefresh,
    observer: Option<DiffComputeObserver>,
}

impl OffLockJob for DiffRefreshJob {
    type Claim = DiffCacheKey;
    type Decided = Option<DiffCacheEntry>;

    fn claim(&self) -> DiffCacheKey {
        self.refresh.key()
    }

    fn decide(self) -> Option<DiffCacheEntry> {
        self.refresh.compute(self.observer.as_ref())
    }

    fn apply(state: &mut AppState, key: DiffCacheKey, entry: Option<DiffCacheEntry>) {
        state.publish_diff_refresh(&key, entry);
    }

    fn abandon(state: &mut AppState, key: DiffCacheKey) {
        state.release_diff_refresh(&key);
    }
}

/// Build the warm agent adapter shared by every project's orchestrator. Build is
/// a UI layer over an agent session: every provider is launched interactively,
/// the rendered prompt is injected into that session, and the same session is
/// streamed to attached clients. The closure is shared across projects via
/// `Agent: Clone`; which provider it builds for is decided per spawn, by the
/// `ModelChoice` the entity carries.
fn build_agent(qa_agent: bool, context: HarnessContext) -> Agent {
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
        // Real agents are interactive TUIs. The provider's own `Harness` owns
        // argv, environment and whatever the worktree needs to be prepared
        // with; Orchestrator submits the prompt through the session.
        Agent::WarmBuilder(Arc::new(
            move |_prompt: &str, choice: &ModelChoice, options: &SpawnOptions| {
                let harness = harness_for(choice.provider);
                harness.prepare_workspace(&options.cwd);
                harness.spec(choice, options, &context)
            },
        ))
    }
}

fn default_resume_id_probe() -> ResumeIdProbe {
    Arc::new(|cwd: &std::path::Path, provider, id: &str| {
        let Ok(home) = std::env::var("HOME") else {
            // No home to read means no grounds to refuse: a recorded id is
            // cleared only where the tree that would hold it was READ and did
            // not.
            return true;
        };
        harness_for(provider).holds_conversation(std::path::Path::new(&home), cwd, id)
    })
}

fn default_session_locator_factory() -> SessionLocatorFactory {
    Arc::new(|cwd: &std::path::Path, provider| {
        let home = std::env::var("HOME").ok()?;
        harness_for(provider).session_locator(std::path::Path::new(&home), cwd)
    })
}

/// What a mutation tail found on the conversation it just wrote: the thread it
/// looked at, how far that thread has got, and the newest attention-class item
/// to land since a tail last looked (`None` when nothing did, or when this is
/// the first look at that conversation).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ConversationNews {
    thread_id: String,
    sequence: u64,
    attention_reason: Option<&'static str>,
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
    /// The harness a new agent is created on when nobody names one. An agent is
    /// locked to its harness for life, so this is asked once on the Account
    /// page and spent at creation — never re-read to move an agent that
    /// already exists.
    default_harness: AgentProvider,
    /// Independent launch presentation for each agent family.
    agent_modes: AgentModes,
    /// How a new checkout is isolated from the project it comes from, for
    /// every project that names no isolation of its own. Spent at creation,
    /// like `default_harness`: an existing checkout says what it is itself.
    isolation: Isolation,
    /// Whether completed diffs automatically receive a review-prioritization pass.
    /// Missing from older configs means off, so upgrading never starts new agent work.
    triage_enabled: bool,
    /// Where to persist the projects + settings, if persistence is enabled.
    config_path: Option<std::path::PathBuf>,
    #[cfg(test)]
    config_persist_failure: Option<ConfigPersistStep>,
    agent: Agent,
    harness: String,
    /// Project-scoped plans, keyed by `plan_id`.
    plans: HashMap<String, ActivePlan>,
    /// Worktree-scoped runs, keyed by `run_id`.
    runs: HashMap<String, ActiveRun>,
    /// Durable plan/run records under the bridge state dir, if persistence is
    /// enabled.
    store: Option<Store>,
    /// What the user said, before anything decided where it goes, keyed by
    /// capture id. Durable from the moment it is taken — the router runs after
    /// the write, never instead of it.
    captures: HashMap<String, crate::capture::Capture>,
    /// The routing decisions in flight, keyed by the capture each one is about.
    /// Single-flight per capture: one router at a time decides where one thing
    /// the user said goes, however many times something asks for it to.
    router_sessions: HashMap<String, crate::router::RouterSession>,
    /// Build's own state directory, fixed at construction. Router scratch is
    /// cut here, beside the store; attaching a store validates its parent and
    /// never changes this root.
    state_root: std::path::PathBuf,
    /// The canonical executable fact supplied by [`HarnessContext`]. Every
    /// project orchestrator receives this same path for MCP scaffolding.
    bridge_exe: std::path::PathBuf,
    /// The provider/model routing runs on when the config file names one.
    /// `None` is the account default at low effort.
    router_choice: Option<ModelChoice>,
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
    /// thread id → how far that conversation had got when a mutation tail last
    /// looked at it. A push fires when an attention-class item lands past it.
    /// Keyed by conversation rather than by entity because a planned run and
    /// its Issue share one thread, and one piece of news is one notification.
    conversation_attention_sequence: HashMap<String, u64>,
    /// entity id → the wire state string last seen by a mutation tail, so
    /// `entity_state_changed_at` only moves on real transitions.
    entity_last_state: HashMap<String, String>,
    /// run id → cached `board.list` diffstat, so the poll surface never runs
    /// per-run git work more than once per TTL window.
    run_stat_cache: HashMap<String, (std::time::Instant, Value)>,
    /// run id → when this run's files were last seen to change (RFC 3339).
    ///
    /// The diff cache above IS the watcher: its numbers are recomputed from the
    /// checkout on a cadence, and two consecutive computes disagreeing means
    /// work landed on disk. Stamping it there costs one comparison of values
    /// already in hand — a real per-checkout watcher would cost a file handle
    /// per worktree and a thread to drain it. Derived, so it is not persisted:
    /// after a restart a run dates itself by its HEAD commit and its
    /// conversation until the next change is observed.
    run_files_changed_at: HashMap<String, String>,
    /// Diff-cache entries with a refresh running right now. Single-flight: a
    /// poll that finds one of these stale serves the value it has and adds no
    /// second worktree scan to the disk.
    diff_refreshes_in_flight: std::collections::HashSet<DiffCacheKey>,
    /// Of those, the ones a mutation has overtaken — see
    /// [`supersede_diff_refresh`](AppState::supersede_diff_refresh). The claim
    /// is also the right to publish, and these have lost it: what they compute
    /// describes the tree as it was before the mutation, and is dropped rather
    /// than put back on the board.
    diff_refreshes_superseded: std::collections::HashSet<DiffCacheKey>,
    /// Test seam: see [`DiffComputeObserver`]. `None` in production.
    diff_compute_observer: Option<DiffComputeObserver>,
    /// Whether a [`sweep_vanished_runs`](AppState::sweep_vanished_runs) is
    /// deciding right now. Single-flight, for the same reason a diff refresh
    /// is: the board polls faster than a fetch per stage returns.
    vanished_run_sweep_in_flight: bool,
    /// Test seam: see [`OffLockGate`]. `None` in production.
    #[cfg(test)]
    off_lock_gate: Option<OffLockGate>,
    /// Project-list-only gate, kept separate so a lifecycle test's global git
    /// gate does not also stop the unrelated project.list probe it uses to
    /// prove the mutex is free.
    #[cfg(test)]
    off_lock_project_list_gate: Option<OffLockGate>,
    /// The git work a verb handed to the drain, to run with this mutex
    /// released. Set by exactly one verb per dispatch and taken by the drain
    /// in the same breath, so the `Ok` the verb returned meanwhile is a
    /// placeholder no client ever sees.
    ///
    /// DELIBERATE, and the same split as [`AppState::pending_agent_turns`]:
    /// under the lock a verb DECIDES (validates, claims the checkout,
    /// snapshots the paths), and the drain — [`dispatch_frame`], or
    /// [`AppState::dispatch`] itself where there is no `Arc` to release
    /// through — DOES the git with the lock free. A `git worktree remove` of a
    /// six-gigabyte checkout takes minutes and a `git status` there takes
    /// seconds; every other frame, every terminal pump and the relay's own
    /// read loop need this mutex while they run.
    deferred_work: Option<DeferredWork>,
    /// Rows a lifecycle verb has claimed and not yet settled: the board's
    /// carrier for a checkout being cut or discarded right now, and the claim
    /// that keeps a second verb off the same name, branch or checkout while its
    /// git runs. Never persisted — everything one leaves behind on a crash is
    /// re-derived by the scan (see `Bridge Concurrency Primitives.md` §5).
    pending_rows: Vec<Arc<crate::lifecycle::PendingRow>>,
    /// Checkouts whose finish is running right now with the mutex released.
    /// A finish is the one verb whose git work outlives its lock hold, so the
    /// checkout it acts on is claimed here for the duration: a second finish
    /// of the same checkout refuses cleanly instead of racing the first one's
    /// branch delete and worktree removal.
    finishing_worktrees: std::collections::HashSet<String>,
    /// Tests only: read every diff cache as aged out, so a stale-poll test does
    /// not have to sleep out a ten-second TTL.
    #[cfg(test)]
    force_stale_diff_caches: bool,
    /// Tests only: see [`BranchDispatchStep`]. `None` everywhere else.
    #[cfg(test)]
    dispatch_fault: Option<BranchDispatchStep>,
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
    agent_spawns_in_flight: std::collections::HashSet<TabKey>,
    /// Signalled whenever a spawn releases its claim above. A caller that lost
    /// the race waits here with the app mutex given back, which is what makes
    /// losing the race free: the winner needs this mutex to publish its tab,
    /// and a loser polling for it every 25 ms was taking the mutex away from
    /// the spawn it was waiting for.
    agent_spawn_finished: Arc<std::sync::Condvar>,
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
    agent_screens_awaiting_spawn: HashMap<TabKey, ScreenHandle>,
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
    /// lock a verb RECORDS what to say (a `PendingAgentTurn`), and
    /// [`DeliveryRunner`] SAYS it on a thread of its own, after the frame that
    /// queued it has answered. Everything that has to look agentless-versus-in-flight
    /// ([`AppState::turns_in_flight`], the idle sweep) exists to cover
    /// the gap this split opens; none of it is optional.
    pending_agent_turns: Vec<PendingAgentTurn>,
    /// Receipts loaded from SQLite plus operations accepted in this process.
    /// The database remains authoritative; this mirror keeps retry/status
    /// checks under the app's existing single state lock.
    operations: HashMap<String, OperationReceipt>,
    /// The one post currently entering persistence. The canonical
    /// conversation-owner save consumes it so message, receipt and delivery
    /// intent use one SQLite commit.
    pending_operation_acceptance: Option<PendingOperationAcceptance>,
    /// The turns that have left [`AppState::pending_agent_turns`] and are being
    /// delivered right now. Between a verb's transition and the tab its turn
    /// spawns, a working entity legitimately has no agent tab yet — the queue
    /// and this are what tell the daemon the difference between an agent on its
    /// way and an agent that never arrived.
    turns_in_flight: TurnsInFlight,
    /// Current unlogged MCP capability per lifecycle owner. Knowing an Issue or
    /// implementation id is intentionally insufficient to forge local control
    /// frames; replacing an agent tab rotates this token.
    mcp_session_tokens: HashMap<String, String>,
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
    /// Builds the watcher that names the conversation a spawning session is
    /// having — consulted once per agent-tab spawn, before the child exists.
    /// Never builds one in QA mode: the scripted harness has no transcript
    /// tree to watch.
    session_locator_factory: SessionLocatorFactory,
    /// Whether a recorded resume id still names a conversation the provider
    /// holds — consulted at every spawn that has one to spend, so a dead one is
    /// cleared where it is read instead of costing a session to find out.
    resume_id_probe: ResumeIdProbe,
    /// Web-push notifier for attention transitions, if configured. Content-free
    /// by contract — it only ever says "a task needs you".
    notifier: Option<Notifier>,
    /// At most one push per task-state change.
    notify_throttle: NotifyThrottle,
    /// Which peer connection each E2EE session has (spec §Signaling), and the
    /// factory that builds them. The bridge is always the answerer, so there is
    /// nothing here until a browser offers; a bridge with no peer transport
    /// built in refuses every offer and its clients keep working over the relay
    /// carrier.
    peers: Arc<SessionPeers>,
    /// Push invalidation: every browser session that asked to be told when
    /// state moves, and the changes waiting to reach them.
    ///
    /// Held as an `Arc` behind its own leaf mutexes rather than as plain
    /// fields, because the two halves run in opposite places: mutations NOTE
    /// changes holding this state's mutex, and the flusher SENDS them holding
    /// no lock at all. See [`crate::changes`].
    changes: Arc<ChangeBus>,
    /// What every frame's four durations are recorded against.
    ///
    /// It lives on the state rather than beside it because the state is what
    /// every path that takes this mutex can already reach: the relay's frame
    /// handler, the MCP done socket, and the delivery path both of them share.
    /// A clock built per handler would leave the socket's frames, which have
    /// no handler behind them, counted nowhere.
    /// Answering `bridge.stats` never goes through here — a frame parked on
    /// this mutex is exactly when the counters are needed, so the frame reads
    /// them off the clock its own timer holds.
    frame_clock: Arc<FrameClock>,
}

struct StoredTasks {
    store: Store,
    plans: Vec<PersistedPlan>,
    runs: Vec<PersistedRun>,
    archived_worktrees: Vec<PersistedArchivedWorktree>,
    captures: Vec<crate::capture::Capture>,
    attention: HashMap<String, crate::attention::Attention>,
    operations: Vec<OperationReceipt>,
}

struct PendingOperationAcceptance {
    conversation_owner_id: String,
    receipt: OperationReceipt,
}

fn load_stored_tasks(dir: std::path::PathBuf) -> Result<StoredTasks, String> {
    let store = Store::new(dir).map_err(|error| error.to_string())?;
    store
        .refuse_a_rolled_back_store()
        .map_err(|error| error.to_string())?;
    match store.import_json_store() {
        Ok(0) => {}
        Ok(imported) => eprintln!("store: imported {imported} records from the JSON store"),
        Err(error) => return Err(format!("store import failed: {error}")),
    }
    Ok(StoredTasks {
        plans: store.load_all_plans().map_err(|error| error.to_string())?,
        runs: store.load_all_runs().map_err(|error| error.to_string())?,
        archived_worktrees: store
            .load_all_archived_worktrees()
            .map_err(|error| error.to_string())?,
        captures: store
            .load_all_captures()
            .map_err(|error| error.to_string())?,
        attention: store.load_attention(),
        operations: store
            .recover_operations()
            .map_err(|error| error.to_string())?,
        store,
    })
}

fn constant_time_token_eq(actual: &str, expected: &str) -> bool {
    let actual = actual.as_bytes();
    let expected = expected.as_bytes();
    let mut difference = actual.len() ^ expected.len();
    let width = actual.len().max(expected.len());
    for index in 0..width {
        difference |= usize::from(
            actual.get(index).copied().unwrap_or(0) ^ expected.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

/// What a frame off the daemon's control socket — an agent's `done` report or
/// one of its MCP actions — is timed under. It is not a relay method, so it
/// gets a name of its own rather than borrowing one from the wire.
const MCP_CONTROL_METHOD: &str = "mcp.control";

/// Resolve a control frame only when it carries the current per-session
/// capability, returning the AGENT that sent it. The token is never included in
/// errors or logs.
///
/// `task_id` is the legacy spelling of the identity baked into the harness's
/// argv; that identity is the agent now, and the entity behind it is a lookup
/// away.
fn authenticated_mcp_owner<'a>(
    frame: &'a Value,
    sessions: &HashMap<String, String>,
) -> Option<&'a str> {
    let owner = frame.get("task_id")?.as_str()?;
    let supplied = frame.get("session_token")?.as_str()?;
    let expected = sessions.get(owner)?;
    constant_time_token_eq(supplied, expected).then_some(owner)
}

#[cfg(unix)]
/// Run the git a socket line handed back — with the guard released, on a
/// blocking thread so several harnesses at once park no runtime worker — and
/// write it down under the same timer. The socket's twin of the drain in
/// [`dispatch_frame`], for a router's tool and a coding agent's report alike.
async fn apply_off_the_socket(
    state: &Arc<Mutex<AppState>>,
    timer: &FrameTimer,
    deferred: DeferredWork,
) -> Result<Value, String> {
    let done = tokio::task::spawn_blocking(move || deferred.run())
        .await
        .expect("the lifecycle job panicked");
    timer
        .lock(state)
        .apply_deferred(MCP_CONTROL_METHOD, &Value::Null, done)
}

fn bind_done_listener(path: &std::path::Path) -> std::io::Result<tokio::net::UnixListener> {
    use std::os::unix::fs::PermissionsExt;

    let listener = tokio::net::UnixListener::bind(path)?;
    // bind(2) applies the process umask, but a permissive or changed umask must
    // never make the lifecycle control plane available to other local users.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

#[cfg(unix)]
async fn serve_done_listener(state: Arc<Mutex<AppState>>, listener: tokio::net::UnixListener) {
    let clock = Arc::clone(&state.lock().unwrap().frame_clock);
    let mut accept_backoff =
        crate::backoff::Backoff::new(Duration::from_millis(100), Duration::from_secs(5));
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                accept_backoff.reset();
                tokio::spawn(handle_done_stream(
                    Arc::clone(&state),
                    stream,
                    Arc::clone(&clock),
                ));
            }
            Err(error) => {
                let wait = accept_backoff.current();
                eprintln!("done socket: accept error: {error}; retrying in {wait:?}");
                tokio::time::sleep(wait).await;
                accept_backoff.increase();
            }
        }
    }
}

#[cfg(unix)]
async fn handle_done_stream(
    state: Arc<Mutex<AppState>>,
    stream: tokio::net::UnixStream,
    clock: Arc<FrameClock>,
) {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = tokio::io::BufReader::new(read_half).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let timer = clock.frame(MCP_CONTROL_METHOD);
        if let Some(response) = handle_authenticated_mcp_frame(&state, &frame, &timer).await {
            let _ = write_half.write_all(response.to_string().as_bytes()).await;
            let _ = write_half.write_all(b"\n").await;
            let _ = write_half.flush().await;
        }
    }
}

async fn handle_authenticated_mcp_frame(
    state: &Arc<Mutex<AppState>>,
    frame: &Value,
    timer: &FrameTimer,
) -> Option<Value> {
    let addressed = {
        let app = timer.lock(state);
        authenticated_mcp_owner(frame, &app.mcp_session_tokens)
            .map(str::to_string)
            .and_then(|agent_id| app.addressed_session(agent_id))
    };
    match addressed {
        Some(AddressedSession::Router { capture_id, .. }) => {
            handle_router_mcp_frame(state, frame, &capture_id, timer).await
        }
        Some(AddressedSession::Coding {
            entity_id,
            agent_id,
        }) => handle_coding_mcp_frame(state, frame, &entity_id, &agent_id, timer).await,
        None => Some(json!({ "ok": false, "error": "unauthorized MCP session" })),
    }
}

async fn handle_router_mcp_frame(
    state: &Arc<Mutex<AppState>>,
    frame: &Value,
    capture_id: &str,
    timer: &FrameTimer,
) -> Option<Value> {
    if let Ok(report) =
        serde_json::from_value::<DoneReport>(frame.get("report").cloned().unwrap_or(Value::Null))
    {
        timer.lock(state).on_router_done(capture_id, report);
        return None;
    }
    let action = serde_json::from_value::<BridgeAction>(
        frame.get("request").cloned().unwrap_or(Value::Null),
    )
    .ok()?;
    let (answered, deferred) = timer.lock(state).router_deferring(capture_id, action);
    let result = match deferred {
        Some(deferred) => apply_off_the_socket(state, timer, deferred).await,
        None => answered,
    };
    DeliveryRunner::drain(state, timer);
    Some(mcp_action_response(result))
}

async fn handle_coding_mcp_frame(
    state: &Arc<Mutex<AppState>>,
    frame: &Value,
    entity_id: &str,
    agent_id: &str,
    timer: &FrameTimer,
) -> Option<Value> {
    if let Ok(report) =
        serde_json::from_value::<DoneReport>(frame.get("report").cloned().unwrap_or(Value::Null))
    {
        let deferred = timer
            .lock(state)
            .done_deferring_for_agent(entity_id, agent_id, report);
        if let Some(deferred) = deferred {
            if let Err(error) = apply_off_the_socket(state, timer, deferred).await {
                eprintln!("done report {entity_id}: {error}");
            }
        }
        DeliveryRunner::drain(state, timer);
        return None;
    }
    let action = serde_json::from_value::<BridgeAction>(
        frame.get("request").cloned().unwrap_or(Value::Null),
    )
    .ok()?;
    let result = timer
        .lock(state)
        .on_agent_mcp_action(entity_id, agent_id, action);
    Some(mcp_action_response(result))
}

fn mcp_action_response(result: Result<Value, String>) -> Value {
    match result {
        Ok(result) => json!({ "ok": true, "result": result }),
        Err(error) => json!({ "ok": false, "error": error }),
    }
}

impl AppState {
    pub fn new(
        repo_path: impl Into<std::path::PathBuf>,
        worktrees_root: impl Into<std::path::PathBuf>,
        base_branch: impl Into<String>,
        qa_agent: bool,
        mcp_socket: impl Into<String>,
    ) -> Self {
        let context = HarnessContext::resolved(mcp_socket.into().into(), default_state_root())
            .expect("resolve the default harness context");
        Self::new_with_context(
            Some(repo_path.into()),
            worktrees_root.into(),
            base_branch.into(),
            qa_agent,
            context,
        )
    }

    pub fn new_configured(
        repo_path: impl Into<std::path::PathBuf>,
        worktrees_root: impl Into<std::path::PathBuf>,
        base_branch: impl Into<String>,
        qa_agent: bool,
        context: HarnessContext,
    ) -> Self {
        Self::new_with_context(
            Some(repo_path.into()),
            worktrees_root.into(),
            base_branch.into(),
            qa_agent,
            context,
        )
    }

    fn new_with_context(
        repo_path: Option<std::path::PathBuf>,
        worktrees_root: std::path::PathBuf,
        base_branch: String,
        qa_agent: bool,
        context: HarnessContext,
    ) -> Self {
        let harness = if qa_agent { "QA agent" } else { "Claude Code" }.to_string();
        let session_locator_factory: SessionLocatorFactory = if qa_agent {
            Arc::new(|_, _| None)
        } else {
            default_session_locator_factory()
        };
        let state_root = context.state_root.clone();
        let bridge_exe = context.bridge_exe.clone();
        let agent = build_agent(qa_agent, context);
        let mut state = AppState {
            projects: Vec::new(),
            entity_project: HashMap::new(),
            entity_project_path: HashMap::new(),
            worktrees_root,
            projects_dir: default_projects_dir(),
            default_harness: DEFAULT_HARNESS,
            agent_modes: AgentModes::from_legacy_default(DEFAULT_HARNESS),
            isolation: Isolation::default(),
            triage_enabled: false,
            config_path: None,
            #[cfg(test)]
            config_persist_failure: None,
            agent,
            harness,
            plans: HashMap::new(),
            runs: HashMap::new(),
            store: None,
            captures: HashMap::new(),
            router_sessions: HashMap::new(),
            state_root,
            bridge_exe,
            router_choice: None,
            archived_worktrees: HashMap::new(),
            entity_created_at: HashMap::new(),
            entity_updated_at: HashMap::new(),
            entity_state_changed_at: HashMap::new(),
            attention: HashMap::new(),
            conversation_attention_sequence: HashMap::new(),
            entity_last_state: HashMap::new(),
            run_stat_cache: HashMap::new(),
            run_files_changed_at: HashMap::new(),
            diff_refreshes_in_flight: std::collections::HashSet::new(),
            diff_refreshes_superseded: std::collections::HashSet::new(),
            vanished_run_sweep_in_flight: false,
            diff_compute_observer: None,
            #[cfg(test)]
            off_lock_gate: None,
            #[cfg(test)]
            off_lock_project_list_gate: None,
            deferred_work: None,
            pending_rows: Vec::new(),
            finishing_worktrees: std::collections::HashSet::new(),
            #[cfg(test)]
            force_stale_diff_caches: false,
            #[cfg(test)]
            dispatch_fault: None,
            term_shell: resolve_term_shell(),
            streams: HashMap::new(),
            tabs: HashMap::new(),
            agent_spawns_in_flight: std::collections::HashSet::new(),
            agent_spawn_finished: Arc::new(std::sync::Condvar::new()),
            agent_screens_awaiting_spawn: HashMap::new(),
            pending_agent_turns: Vec::new(),
            operations: HashMap::new(),
            pending_operation_acceptance: None,
            turns_in_flight: TurnsInFlight::default(),
            mcp_session_tokens: HashMap::new(),
            next_term: 1,
            self_handle: None,
            next_stream: 1,
            next_project: 1,
            qa_agent,
            session_locator_factory,
            resume_id_probe: default_resume_id_probe(),
            notifier: None,
            notify_throttle: NotifyThrottle::default(),
            peers: SessionPeers::with_factory(Arc::new(NoPeerFactory)),
            changes: ChangeBus::new(DEFAULT_COALESCE_WINDOW),
            frame_clock: FrameClock::new(),
        };
        if let Some(repo_path) = repo_path {
            state.add_project(repo_path, base_branch);
        }
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
        let context = HarnessContext::resolved(mcp_socket.into().into(), default_state_root())
            .expect("resolve the default harness context");
        Self::new_with_context(
            None,
            worktrees_root.into(),
            base_branch.into(),
            qa_agent,
            context,
        )
    }

    pub fn new_unrooted_configured(
        worktrees_root: impl Into<std::path::PathBuf>,
        base_branch: impl Into<String>,
        qa_agent: bool,
        context: HarnessContext,
    ) -> Self {
        Self::new_with_context(
            None,
            worktrees_root.into(),
            base_branch.into(),
            qa_agent,
            context,
        )
    }

    /// Enable web-push attention notifications: every task-state change into a
    /// state that needs the human fires one signed, content-free notify at the api.
    pub fn with_notifier(mut self, notifier: Notifier) -> Self {
        self.notifier = Some(notifier);
        self
    }

    /// Answer `rtc.offer` with peer connections `factory` builds. Without one
    /// the bridge has no peer transport and every offer is refused.
    ///
    /// Settable after the state is shared because a real factory is built from
    /// the intake, the intake from this state's own handler: the peer transport
    /// is the last thing the daemon hands the app, not something it is born
    /// with.
    pub fn set_peer_factory(&mut self, factory: Arc<dyn SessionPeerFactory>) {
        self.peers = SessionPeers::with_factory(factory);
    }

    /// Enable durable task persistence at `dir` and recover every stored task:
    /// re-attach tasks whose worktrees survived, surface tasks that were mid-phase
    /// when the daemon died as `interrupted`, and abandon tasks whose worktrees are
    /// gone. A corrupt task file is a hard error naming the file — boot fails
    /// rather than silently dropping a task.
    pub fn with_task_store(mut self, dir: impl Into<std::path::PathBuf>) -> Result<Self, String> {
        let dir = dir.into();
        let parent = dir
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        let resolved_parent = std::fs::canonicalize(parent)
            .map_err(|error| format!("resolve task store parent {}: {error}", parent.display()))?;
        if resolved_parent != self.state_root {
            return Err(format!(
                "task store parent {} does not match configured state root {}",
                resolved_parent.display(),
                self.state_root.display()
            ));
        }
        let stored = load_stored_tasks(dir)?;
        self.restore_stored_tasks(stored)?;
        Ok(self)
    }

    fn restore_stored_tasks(&mut self, stored: StoredTasks) -> Result<(), String> {
        self.attention = stored.attention;
        self.store = Some(stored.store);
        self.archived_worktrees = stored
            .archived_worktrees
            .into_iter()
            .map(|record| (record.worktree_id.clone(), record))
            .collect();
        self.recover_captures(stored.captures)?;
        self.recover_completed_worktree_finishes();
        self.restore_plans_before_runs(stored.plans, stored.runs)?;
        self.restore_operations(stored.operations);
        self.seed_conversation_attention_sequences();
        self.seed_anchors_for_records_without_one();
        self.migrate_legacy_dismissals();
        self.close_recovered_working_intervals();
        self.resume_stored_issue_schedulers()
    }

    fn restore_operations(&mut self, operations: Vec<OperationReceipt>) {
        for receipt in operations {
            if receipt.status == OperationStatus::Queued {
                if let Some(turn) = PendingAgentTurn::for_delivery_operation(&receipt) {
                    self.pending_agent_turns.push(turn);
                }
            }
        }
    }

    fn restore_plans_before_runs(
        &mut self,
        plans: Vec<PersistedPlan>,
        runs: Vec<PersistedRun>,
    ) -> Result<(), String> {
        for record in plans {
            self.recover_plan(record)?;
        }
        for record in runs {
            self.recover_run(record)?;
        }
        Ok(())
    }

    fn resume_stored_issue_schedulers(&mut self) -> Result<(), String> {
        let issue_ids = self
            .plans
            .iter()
            .filter(|(_, issue)| issue.plan.implementation_intent != ImplementationIntent::None)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for issue_id in issue_ids {
            self.advance_issue_scheduler_here(&issue_id, &json!({ "issue_id": issue_id }))?;
        }
        Ok(())
    }

    /// Re-attach one persisted plan on boot. The canonical docs live in the
    /// store, so a vanished scratch docs dir never abandons or archives a
    /// plan — a plan that was mid-draft simply surfaces `Interrupted` (its
    /// session died with the daemon); the next revision dispatch remakes the
    /// workspace from the store. Recovery never abandons a plan.
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
            // Say so on the conversation: unread is event-driven, so a parked
            // plan whose session died goes quiet unless the event exists.
            active.agents.sole_thread_mut().push_event(
                crate::thread::ThreadEventKind::Interrupted,
                Some("Build restarted; the drafting session did not survive".to_string()),
                None,
                None,
                now_rfc3339(),
            );
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
    #[allow(clippy::cognitive_complexity)] // ratchet: recover_run is at 29, threshold 15 — bring it under, then remove
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

        // Resolve an interrupted publication from repository evidence before
        // considering worktree recovery. The write-ahead record names the exact
        // candidate, and classification refreshes its configured remote ref.
        if let Some(attempt) = active.publication_attempt.clone() {
            let publication = project_id
                .as_deref()
                .and_then(|id| self.orch_for(id).ok())
                .map_or(StagePublication::Local, |orch| {
                    classify_stage_publication(
                        orch.worktrees(),
                        &active.worktree.path,
                        &active.worktree.branch(),
                        &active.worktree.base_branch,
                        &attempt.candidate_sha,
                    )
                });
            let proven = match attempt.action.as_str() {
                "push" => matches!(
                    publication,
                    StagePublication::Pushed | StagePublication::Merged
                ),
                "merge" | "merge_push" => publication == StagePublication::Merged,
                _ => false,
            };
            if proven {
                for progress in &mut active.stages {
                    if progress.completion_sha.is_some() {
                        progress.publication = publication;
                        progress.invalidation_reason = None;
                    }
                }
                if matches!(attempt.action.as_str(), "merge" | "merge_push") {
                    active.run.state = RunState::Merged;
                }
                active.publication_attempt = None;
                active.last_error = None;
                recovery_event = Some((
                    if publication == StagePublication::Merged {
                        crate::thread::ThreadEventKind::Merged
                    } else {
                        crate::thread::ThreadEventKind::Pushed
                    },
                    format!(
                        "Recovered interrupted {} from verified refs at {}",
                        attempt.action, attempt.candidate_sha
                    ),
                ));
                state_changed = true;
            } else {
                active.last_error = Some(format!(
                    "interrupted {} is not yet proven by configured refs (candidate {})",
                    attempt.action, attempt.candidate_sha
                ));
                state_changed = true;
            }
        }

        if !active.run.state.is_terminal() && !active.worktree.path.exists() {
            let restored = project_id
                .as_deref()
                .filter(|_| !active.adopted)
                .ok_or_else(|| "the original project/branch is unavailable".to_string())
                .and_then(|project_id| {
                    let resolved = self.resolved_isolation(project_id);
                    self.orch_for(project_id)?
                        .restore_run_worktree(
                            &active.worktree,
                            unregistered_restore_for(&active),
                            resolved.isolation,
                        )
                        .map(|worktree| (worktree, resolved.downgrade))
                        .map_err(err)
                });
            match restored {
                Ok((worktree, downgrade)) => {
                    active.worktree = worktree;
                    if let Some(reason) = downgrade {
                        self.note_isolation_downgrade(&run_id, &mut active, &reason);
                    }
                    recovery_event = Some((
                        crate::thread::ThreadEventKind::WorktreeRecreated,
                        format!(
                            "Recreated the Issue worktree from branch {}",
                            active.worktree.branch()
                        ),
                    ));
                    state_changed = true;
                }
                Err(error) if active.run.plan_id.is_some() && project_id.is_some() => {
                    let issue_id = active.run.plan_id.as_ref().expect("guarded").0.clone();
                    let existing = active
                        .recovery
                        .as_ref()
                        .filter(|attempt| attempt.state == crate::run::RecoveryState::Started)
                        .cloned();
                    let requested_stage_id = existing
                        .as_ref()
                        .map(|attempt| attempt.requested_stage_id.clone())
                        .or_else(|| {
                            self.plans.get(&issue_id).and_then(|issue| {
                                match &issue.plan.implementation_intent {
                                    ImplementationIntent::Stage(stage_id) => Some(stage_id.clone()),
                                    ImplementationIntent::All => {
                                        next_unsettled_stage(&issue.stages, Some(&active))
                                            .map(|doc| doc.id.clone())
                                    }
                                    ImplementationIntent::None => active.current_stage_id.clone(),
                                }
                            })
                        })
                        .unwrap_or_default();
                    let recovery_id = existing
                        .as_ref()
                        .map(|attempt| attempt.id.clone())
                        .unwrap_or_else(|| format!("recovery-{}", uuid::Uuid::new_v4()));
                    let started_at = existing
                        .as_ref()
                        .map(|attempt| attempt.started_at.clone())
                        .unwrap_or_else(now_rfc3339);
                    if existing.is_none() {
                        active.recovery = Some(crate::run::RecoveryAttempt {
                            id: recovery_id.clone(),
                            requested_stage_id: requested_stage_id.clone(),
                            branch: active.worktree.recorded_branch.clone(),
                            state: crate::run::RecoveryState::Started,
                            report: None,
                            started_at: started_at.clone(),
                            completed_at: None,
                        });
                        recovery_event = Some((
                            crate::thread::ThreadEventKind::RecoveryStarted,
                            format!("Verified recovery {recovery_id} started: {error}"),
                        ));
                    }
                    active.last_error = Some(format!(
                        "automatic branch restoration failed: {error}; verified recovery {recovery_id} is running"
                    ));
                    let stages = self
                        .plans
                        .get(&issue_id)
                        .map(|issue| issue.stages.clone())
                        .unwrap_or_default();
                    let prompt = recovery_agent_prompt(
                        &recovery_id,
                        &issue_id,
                        &run_id,
                        &requested_stage_id,
                        &active.worktree,
                        &error,
                        &stages,
                    );
                    match PendingAgentTurn::for_recovery(&run_id, &active, &repo_path, prompt) {
                        Some(turn) => self.pending_agent_turns.push(turn),
                        None => eprintln!("recover {run_id}: no agent to hand the recovery to"),
                    }
                    state_changed = true;
                }
                Err(error) => {
                    let published = self.classify_stages_now(&run_id, &active);
                    let affected = reconcile_missing_run_worktree(&mut active, &published);
                    active
                        .run
                        .apply(RunEvent::Abandon)
                        .map_err(|e| format!("recover {run_id}: {e}"))?;
                    active.last_error = Some(format!("worktree recovery failed: {error}"));
                    recovery_event = Some((
                        crate::thread::ThreadEventKind::RecoveryFailed,
                        format!(
                            "Could not recover the worktree: {error}. {} stage(s) were marked incomplete",
                            affected.len()
                        ),
                    ));
                    state_changed = true;
                }
            }
        }
        let mut interrupted = false;
        if !active.run.state.is_terminal() && active.run.state.is_working() {
            active
                .run
                .apply(RunEvent::Interrupt)
                .map_err(|e| format!("recover {run_id}: {e}"))?;
            interrupted = true;
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
                    interrupted = true;
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
                let mut links = vec![crate::thread::ThreadLink::Implementation {
                    issue_id: issue_id.clone(),
                    implementation_id: run_id.clone(),
                }];
                if let Some(recovery) = &active.recovery {
                    links.push(crate::thread::ThreadLink::Recovery {
                        recovery_id: recovery.id.clone(),
                    });
                    if let Some(stage) = issue
                        .stages
                        .iter()
                        .find(|stage| stage.id == recovery.requested_stage_id)
                    {
                        links.push(crate::thread::ThreadLink::IssueStage {
                            issue_id: issue_id.clone(),
                            stage_id: stage.id.clone(),
                            path: stage.path.clone(),
                        });
                    }
                }
                if active.worktree.path.exists() {
                    links.push(crate::thread::ThreadLink::Worktree {
                        worktree_id: crate::worktree::external_worktree_id(&active.worktree.path),
                    });
                }
                issue.agents.sole_thread_mut().push_event_with_links(
                    event,
                    Some(summary),
                    None,
                    None,
                    links,
                    now_rfc3339(),
                );
                let persisted = self.finish_plan_mutation(issue_id, issue);
                persisted?;
            }
        }

        if interrupted {
            self.record_on_run_conversation(&mut active, |thread| {
                thread.push_event(
                    crate::thread::ThreadEventKind::Interrupted,
                    Some("Build restarted; the agent session did not survive".to_string()),
                    None,
                    None,
                    now_rfc3339(),
                );
            })?;
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

    /// Write a plan's durable core to the store (atomic replace). A no-op
    /// without a configured store (unit tests); an error surfaces to the caller
    /// — a plan the store cannot hold would silently vanish on the next
    /// restart. Docs are NOT snapshotted here: the store copy is canonical and
    /// written transactionally at each plan/revise `done`.
    fn persist_plan_record(&mut self, plan_id: &str, active: &ActivePlan) -> Result<(), String> {
        if self.store.is_none() {
            self.remember_in_memory_acceptance(plan_id);
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
        let record = PersistedPlan {
            id: plan_id.to_string(),
            goal: active.plan.goal.clone(),
            project_path,
            base_branch: active.base_branch.clone(),
            state: active.plan.state,
            archived_at: active.plan.archived_at.clone(),
            implementation_intent: active.plan.implementation_intent.clone(),
            implementation_activity: active.plan.implementation_activity.clone(),
            plan_path: active.plan_path.clone(),
            stages: active.stages.clone(),
            provider: active.model_choice.provider,
            model: active.model_choice.model.clone(),
            effort: active.model_choice.effort.clone(),
            agents: active.agents.agents().to_vec(),
            legacy_thread: crate::thread::Thread::default(),
            last_summary: active.last_summary.clone(),
            last_error: active.last_error.clone(),
            created_at,
            updated_at,
            state_changed_at: self.entity_state_changed_at.get(plan_id).cloned(),
        };
        let acceptance = self.take_operation_acceptance(plan_id);
        match acceptance.as_ref() {
            Some(acceptance) => self
                .store
                .as_ref()
                .expect("checked above")
                .save_issue_plan_accepting_operation(&record, &acceptance.receipt)
                .map(Some),
            None => self
                .store
                .as_ref()
                .expect("checked above")
                .save_issue_plan(&record)
                .map(|()| None),
        }
        .map_err(|e| format!("issue store: {e}"))?;
        Ok(())
    }

    /// Write a run's durable core to the store (atomic replace). Same discipline
    /// as [`persist_plan_record`](Self::persist_plan_record); a run stores its
    /// `plan_id`, never plan docs.
    fn persist_run_record(&mut self, run_id: &str, active: &ActiveRun) -> Result<(), String> {
        if self.store.is_none() {
            self.remember_in_memory_acceptance(run_id);
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
            branch: active.worktree.branch(),
            worktree_name: active.worktree.name.clone(),
            worktree_path: active.worktree.path.display().to_string(),
            base_sha: active.base_sha.clone(),
            stages: active.stages.clone(),
            current_stage_id: active.current_stage_id.clone(),
            revising_stage_id: active.revising_stage_id.clone(),
            auto_advance: active.auto_advance,
            adopted: active.adopted,
            triage: active.triage.clone(),
            recovery: active.recovery.clone(),
            publication_attempt: active.publication_attempt.clone(),
            provider: active.model_choice.provider,
            model: active.model_choice.model.clone(),
            effort: active.model_choice.effort.clone(),
            agents: active.agents.agents().to_vec(),
            legacy_thread: crate::thread::Thread::default(),
            last_summary: active.last_summary.clone(),
            last_error: active.last_error.clone(),
            created_at,
            updated_at,
            state_changed_at: self.entity_state_changed_at.get(run_id).cloned(),
        };
        // One write path, whether or not the run belongs to an Issue: the
        // `issue_id` column is `record.plan_id`, so asking the store whether
        // the Issue exists first only bought a lock acquisition per save.
        let acceptance = self.take_operation_acceptance(run_id);
        match acceptance.as_ref() {
            Some(acceptance) => self
                .store
                .as_ref()
                .expect("checked above")
                .save_run_accepting_operation(&record, &acceptance.receipt)
                .map(Some),
            None => self
                .store
                .as_ref()
                .expect("checked above")
                .save_run(&record)
                .map(|()| None),
        }
        .map_err(|e| format!("run store: {e}"))?;
        Ok(())
    }

    fn take_operation_acceptance(&mut self, owner_id: &str) -> Option<PendingOperationAcceptance> {
        if self
            .pending_operation_acceptance
            .as_ref()
            .is_some_and(|pending| pending.conversation_owner_id == owner_id)
        {
            self.pending_operation_acceptance.take()
        } else {
            None
        }
    }

    fn remember_in_memory_acceptance(&mut self, owner_id: &str) {
        if let Some(acceptance) = self.take_operation_acceptance(owner_id) {
            self.operations
                .insert(acceptance.receipt.operation_id.clone(), acceptance.receipt);
        }
    }

    /// The shared tail of every plan mutation: stamp times, persist the durable
    /// core, throttle a notify, put the plan back in the map, and prompt
    /// terminal closure + pump start.
    fn finish_plan_mutation(&mut self, plan_id: String, active: ActivePlan) -> Result<(), String> {
        let now = now_rfc3339();
        self.entity_created_at
            .entry(plan_id.clone())
            .or_insert_with(|| now.clone());
        self.entity_updated_at.insert(plan_id.clone(), now.clone());
        self.stamp_state_change(&plan_id, plan_state_str(&active.plan.state), now);
        let persisted = self.persist_plan_record(&plan_id, &active);
        let news = self.conversation_news(active.agents.sole_thread());
        let state_kind = crate::notify::kind_for_plan_state(&active.plan.state);
        self.push_attention_notify(&plan_id, news, state_kind);
        self.plans.insert(plan_id.clone(), active);
        // After the insert: the attention file is pruned to what exists when it
        // is written, and an anchor stamped while the record was checked out
        // would be dropped on the way to disk.
        self.seed_anchor(&plan_id);
        self.reap_orphaned_terminals();
        // Every plan mutation ends here — an RPC's, an agent's `done`, a
        // delivery failure — so this is the one place that has to tell the
        // browsers, whatever started it.
        self.note_entity_changed(&plan_id);
        persisted
    }

    fn answer_plan_mutation(
        &mut self,
        plan_id: String,
        active: ActivePlan,
        thread_detail: ThreadDetail,
    ) -> (Value, Result<(), String>) {
        let settled = self.finish_plan_mutation(plan_id.clone(), active);
        let active = self.plans.get(&plan_id).expect("the finish put it back");
        (
            self.plan_view(&plan_id, active, thread_detail, DigestScope::Detail),
            settled,
        )
    }

    /// The run-half twin of
    /// [`finish_plan_mutation`](Self::finish_plan_mutation).
    fn finish_run_mutation(&mut self, run_id: String, active: ActiveRun) -> Result<(), String> {
        let now = now_rfc3339();
        self.entity_created_at
            .entry(run_id.clone())
            .or_insert_with(|| now.clone());
        self.entity_updated_at.insert(run_id.clone(), now.clone());
        self.stamp_state_change(&run_id, run_state_str(&active.run.state), now);
        // The mutation likely changed the tree; drop the cached diffstat.
        self.invalidate_run_stat(&run_id);
        let persisted = self.persist_run_record(&run_id, &active);
        let news = self
            .conversation_thread_for_run(&active)
            .map(|thread| self.conversation_news(thread));
        let state_kind = crate::notify::kind_for_run_state(&active.run.state);
        if let Some(news) = news {
            self.push_attention_notify(&run_id, news, state_kind);
        }
        self.runs.insert(run_id.clone(), active);
        // See `finish_plan_mutation`: seeded once the record is back in its map.
        self.seed_anchor(&run_id);
        self.reap_orphaned_terminals();
        self.note_entity_changed(&run_id);
        persisted
    }

    /// The run-half twin of
    /// [`answer_plan_mutation`](Self::answer_plan_mutation).
    fn answer_run_mutation(
        &mut self,
        run_id: String,
        active: ActiveRun,
        thread_detail: ThreadDetail,
    ) -> (Value, Result<(), String>) {
        let settled = self.finish_run_mutation(run_id.clone(), active);
        let active = self.runs.get(&run_id).expect("the finish put it back");
        (
            self.run_view(&run_id, active, thread_detail, DigestScope::Detail),
            settled,
        )
    }

    /// A cold delivery started a new harness for `turn.owner`: open the
    /// conversation's session lineage for it, exactly as the phase spawner used
    /// to. A warm delivery never calls this — the session it continues is
    /// already open, and a second `start_session` would read back as an agent
    /// restart that never happened.
    fn record_agent_session_start(
        &mut self,
        owner: &str,
        agent_id: &str,
        checkout: &std::path::Path,
        model_choice: &ModelChoice,
        phase: &str,
    ) -> Option<SessionInstance> {
        // A router owns no conversation — it decides which one the capture
        // becomes. What its session start records is that there is now a
        // process to have lost, which is what makes a dead one detectable.
        if let Some(session) = self.router_sessions.get_mut(owner) {
            session.mark_started();
            self.note_board_changed();
            return None;
        }
        let mut opened = None;
        let checkout = checkout.display().to_string();
        if let Err(error) = self.edit_agent_conversation(owner, agent_id, |thread, _artifact| {
            opened = Some(open_session_lineage(
                thread,
                owner,
                agent_id,
                &checkout,
                model_choice,
                phase,
            ));
            Ok(())
        }) {
            eprintln!("record_agent_session_start {owner}: {error}");
        }
        self.note_board_changed();
        opened
    }

    /// The agent process an id owns has ended: close the conversation's session
    /// lineage for it, and close the turn it died holding. The mirror of
    /// [`record_agent_session_start`](Self::record_agent_session_start), and it
    /// is the PUMP that calls it — the only place that learns a harness died on
    /// its own. An owner that no longer exists (its record was deleted with the
    /// tab) has no lineage left to close, which is why this is quiet.
    fn record_agent_session_end(
        &mut self,
        owner: &str,
        agent_id: &str,
        instance: &SessionInstance,
    ) {
        let now = now_rfc3339();
        let mut ended_current = false;
        if let Err(error) = self.edit_agent_conversation(owner, agent_id, |thread, _artifact| {
            let was_current = thread.open_session_instance(agent_id).as_ref() == Some(instance);
            ended_current = thread.finish_session_instance(instance, &now) && was_current;
            Ok(())
        }) {
            eprintln!("record_agent_session_end {owner}: {error}");
        }
        if !ended_current {
            return;
        }
        self.close_turn_of_dead_agent(owner, agent_id);
        self.record_agent_working_since(owner, agent_id, None);
        if !self.entity_agents_working(owner)
            && self
                .attention
                .entry(owner.to_string())
                .or_default()
                .observe_working(false, &now)
        {
            self.persist_attention();
        }
        // Agent liveness is feed state, and it is the one kind that moves with
        // no verb behind it: the pump learned a harness died. Noted here rather
        // than left to the mutation tails above, which do nothing at all for an
        // owner whose record is already gone.
        self.note_board_changed();
    }

    /// `agent_id`'s process is gone. If it died mid-turn, say so on the
    /// conversation that agent speaks in — which closes the turn, and with it
    /// the row's and the bubble's claim that work is in flight.
    ///
    /// The conversation is chosen exactly the way
    /// [`agent_conversation`](Self::agent_conversation) reads it: through the
    /// addressed agent's stable binding. A turn that is already closed —
    /// the agent replied, reported `done`, or was told the branch was abandoned
    /// — is left alone: this marker exists only for a turn nobody else will
    /// ever close.
    fn close_turn_of_dead_agent(&mut self, owner: &str, agent_id: &str) {
        let died_mid_turn = self
            .entity_agents(owner)
            .ok()
            .and_then(|agents| agents.by_id(agent_id))
            .is_some_and(|agent| agent.working_since.is_some());
        if !died_mid_turn {
            return;
        }
        let now = now_rfc3339();
        if let Err(error) = self.edit_agent_conversation(owner, agent_id, |thread, _artifact| {
            record_session_death_in_thread(thread, &now);
            Ok(())
        }) {
            eprintln!("close_turn_of_dead_agent {owner}: {error}");
        }
    }

    /// Apply `edit` to `agent_id`'s own RECORD — what the agent IS, not what it
    /// said — and persist the entity that owns it.
    ///
    /// Separate from [`edit_agent_conversation`](Self::edit_agent_conversation), which edits
    /// the conversation an agent SPEAKS in: for a planned implementation's
    /// first agent that is the Issue's thread, which is not the agent.
    ///
    /// Quiet about an owner or agent it cannot find: a record deleted with its
    /// tab has no roster left to write onto.
    fn edit_agent_record(
        &mut self,
        context: &str,
        owner: &str,
        agent_id: &str,
        edit: impl FnOnce(&mut crate::agent::Agent),
    ) {
        if self.plans.contains_key(owner) {
            let Ok(mut active) = self.take_plan(owner) else {
                return;
            };
            if let Some(agent) = active.agents.by_id_mut(agent_id) {
                edit(agent);
            }
            if let Err(error) = self.finish_plan_mutation(owner.to_string(), active) {
                eprintln!("{context} {owner}: {error}");
            }
            return;
        }
        let Ok(mut active) = self.take_run(owner) else {
            return;
        };
        if let Some(agent) = active.agents.by_id_mut(agent_id) {
            edit(agent);
        }
        if let Err(error) = self.finish_run_mutation(owner.to_string(), active) {
            eprintln!("{context} {owner}: {error}");
        }
    }

    /// The transcript-tree reads a spawn makes, taken together so the
    /// disk work can be handed over in one piece.
    fn session_probes(&self) -> SessionProbes {
        SessionProbes {
            resume_id: Arc::clone(&self.resume_id_probe),
            locator: Arc::clone(&self.session_locator_factory),
        }
    }

    /// The name the agent's record says its conversation has — `None` for one
    /// no session of its has ever announced.
    fn recorded_resume_id(&self, owner: &str, agent_id: &str) -> Option<String> {
        self.entity_agents(owner)
            .ok()?
            .by_id(agent_id)?
            .resume_session_id
            .clone()
    }

    /// An exact provider id is resumable only when persisted lineage binds it
    /// to this agent, provider, and checkout. Legacy/partial records start
    /// fresh and catch up from canonical history instead of guessing.
    fn resumable_session_id(
        &self,
        owner: &str,
        agent_id: &str,
        root: &std::path::Path,
        provider: AgentProvider,
    ) -> Option<String> {
        let named = self.recorded_resume_id(owner, agent_id)?;
        let checkout = root.display().to_string();
        self.agent_conversation(owner, Some(agent_id))
            .ok()?
            .sessions
            .iter()
            .rev()
            .any(|session| {
                session.agent_id == agent_id
                    && session.checkout.as_deref() == Some(checkout.as_str())
                    && session.provider == provider.label()
                    && session.resume_session_id.as_deref() == Some(named.as_str())
            })
            .then_some(named)
    }

    /// Whether a queued turn still names the same executable agent and
    /// canonical conversation it named when accepted.
    ///
    /// This is deliberately stricter than owner liveness. A drained batch can
    /// outlive `agent.remove`, and an Issue implementation can share history
    /// with its Issue while retaining a distinct process identity. Neither may
    /// be reconstructed from roster position after the turn left the queue.
    fn queued_agent_target_exists(&self, turn: &PendingAgentTurn) -> bool {
        self.agent_target_exists(
            &turn.owner,
            &turn.agent_id,
            &turn.conversation_id,
            &turn.root,
        )
    }

    fn agent_target_exists(
        &self,
        owner: &str,
        agent_id: &str,
        conversation_id: &str,
        root: &std::path::Path,
    ) -> bool {
        if crate::router::is_router_agent(agent_id) {
            return self.router_sessions.get(owner).is_some_and(|session| {
                session.agent_id() == agent_id
                    && conversation_id == agent_id
                    && Self::canonical_root(session.scratch_dir()) == root
            });
        }
        let Ok(address) = self.resolve_conversation_address(owner, Some(agent_id)) else {
            return false;
        };
        address.conversation_id == conversation_id
            && self
                .entity_agent_root(owner)
                .is_ok_and(|actual| Self::canonical_root(&actual) == root)
    }

    /// Whether the exact process behind `instance` was launched with `choice`.
    fn session_instance_uses_choice(
        &self,
        instance: &SessionInstance,
        choice: &ModelChoice,
    ) -> bool {
        self.agent_conversation(&instance.entity_id, Some(&instance.agent_id))
            .ok()
            .and_then(|thread| {
                thread
                    .sessions
                    .iter()
                    .find(|session| session.id == instance.id)
            })
            .is_some_and(|session| {
                session.conversation_id == instance.conversation_id
                    && session.checkout.as_deref().unwrap_or_default() == instance.checkout
                    && session.provider == choice.provider.label()
                    && session.model == choice.model
                    && session.effort == choice.effort
            })
    }

    /// Write down the name the agent's live session gave its conversation, so
    /// the next spawn resumes it BY NAME instead of guessing the newest
    /// transcript in the checkout.
    ///
    /// `None` clears it, which is what a session that ended having never
    /// announced one asks for: that is the shape of a spawn whose `--resume`
    /// id no longer resolved, and clearing costs one restart where keeping it
    /// would cost every restart.
    fn record_agent_resume_id(&mut self, owner: &str, agent_id: &str, named: Option<String>) {
        self.edit_agent_record("record_agent_resume_id", owner, agent_id, |agent| {
            agent.resume_session_id = named;
        });
    }

    fn recorded_active_model(&self, owner: &str, agent_id: &str) -> Option<String> {
        self.entity_agents(owner)
            .ok()?
            .by_id(agent_id)?
            .active_model
            .clone()
    }

    /// A turn is on its way to this agent, so why the LAST one never arrived is
    /// history: the row the client is about to wear a "starting" state on must
    /// not be answered by the failure before it.
    ///
    /// Written only when there is one to forget, so an ordinary delivery costs
    /// no store write.
    fn forget_agent_start_error(&mut self, owner: &str, agent_id: &str) {
        let recorded = self
            .entity_agents(owner)
            .ok()
            .and_then(|roster| roster.by_id(agent_id))
            .and_then(|agent| agent.start_error.as_ref());
        if recorded.is_none() {
            return;
        }
        self.edit_agent_record("forget_agent_start_error", owner, agent_id, |agent| {
            agent.start_error = None;
        });
    }

    fn record_agent_active_model(&mut self, owner: &str, agent_id: &str, running: Option<String>) {
        if self.recorded_active_model(owner, agent_id) == running {
            return;
        }
        self.edit_agent_record("record_agent_active_model", owner, agent_id, |agent| {
            agent.active_model = running;
        });
    }

    /// Set the current execution clock on the addressed agent record, never on
    /// the canonical conversation it may share with another agent.
    fn record_agent_working_since(
        &mut self,
        owner: &str,
        agent_id: &str,
        working_since: Option<String>,
    ) {
        let unchanged = self
            .entity_agents(owner)
            .ok()
            .and_then(|agents| agents.by_id(agent_id))
            .is_some_and(|agent| agent.working_since == working_since);
        if unchanged {
            return;
        }
        self.edit_agent_record("record_agent_working_since", owner, agent_id, |agent| {
            agent.working_since = working_since;
        });
    }

    /// Start one agent's execution interval without moving an interval already
    /// in flight. A second read or a turn queued onto a native session is more
    /// work for the same execution, not a new start time.
    fn start_agent_working(&mut self, owner: &str, agent_id: &str, now: &str) {
        let already_working = self
            .entity_agents(owner)
            .ok()
            .and_then(|agents| agents.by_id(agent_id))
            .is_some_and(|agent| agent.working_since.is_some());
        if !already_working {
            self.record_agent_working_since(owner, agent_id, Some(now.to_string()));
        }
    }

    /// Apply one provider-reported boundary to the exact agent whose session
    /// emitted it. The entity attention clock remains the aggregate of all
    /// agent intervals, while the execution clock itself never moves onto the
    /// canonical conversation another agent may share.
    fn record_agent_status_snapshot(
        &mut self,
        owner: &str,
        agent_id: &str,
        snapshot: &crate::harness::SessionStatusSnapshot,
    ) {
        let working_since =
            matches!(snapshot.status, AgentStatus::Working).then(|| snapshot.changed_at.clone());
        self.record_agent_working_since(owner, agent_id, working_since);
        let working = self.entity_agents_working(owner);
        if self
            .attention
            .entry(owner.to_string())
            .or_default()
            .observe_status(
                working,
                &snapshot.changed_at,
                snapshot.last_worked_at.as_deref(),
            )
        {
            self.persist_attention();
            self.note_entity_changed(owner);
        }
    }

    /// Post one thing the agent reported doing into the conversation it speaks
    /// in.
    ///
    /// Activity is conversation: an event-stream harness has no second tab and
    /// no second scrollback, so its reasoning, tool calls and narration ride the
    /// timeline the human already reads, told apart from what the agent SAID by
    /// class rather than by living somewhere else.
    ///
    /// Quiet about an owner it cannot find, for the reason
    /// [`edit_owner_thread`](Self::edit_owner_thread) is: a router owns no
    /// conversation, and an entity whose record was deleted with its tab has
    /// none left to speak in. Neither is worth a line per tool call.
    fn record_agent_activity(
        &mut self,
        owner: &str,
        agent_id: &str,
        activity: &crate::harness::AgentActivity,
        parent_sequence: Option<u64>,
    ) -> Option<u64> {
        self.record_activity_row(
            owner,
            agent_id,
            activity_event_kind(activity),
            activity.summary().to_string(),
            parent_sequence,
        )
    }

    /// Mint one activity row, and hand back the counter value it was minted at
    /// — the handle a tool call's answer comes back on. `None` for an owner
    /// with no conversation to speak in, for the reason above.
    fn record_activity_row(
        &mut self,
        owner: &str,
        agent_id: &str,
        event: crate::thread::ThreadEventKind,
        summary: String,
        parent_sequence: Option<u64>,
    ) -> Option<u64> {
        let now = now_rfc3339();
        self.edit_agent_conversation(owner, agent_id, |thread, _artifact| {
            let session_id = open_session_id(thread, agent_id);
            Ok(thread.push_drafted_event(
                crate::thread::ThreadEventDraft {
                    event,
                    summary: Some(summary),
                    session_id,
                    revision_id: None,
                    links: Vec::new(),
                    parent_sequence,
                },
                now,
            ))
        })
        .ok()
    }

    /// Land a tool call's answer on the row the call minted, closing it.
    ///
    /// `false` when that row is not there to be closed — the conversation is
    /// gone, or a reload left the call under the resident tail — which is what
    /// sends the pump back to minting the answer as a row of its own rather
    /// than losing it.
    fn resolve_agent_tool_call(
        &mut self,
        owner: &str,
        agent_id: &str,
        sequence: u64,
        outcome: crate::thread::ToolCallOutcome,
        answer: &str,
    ) -> bool {
        self.edit_agent_conversation(owner, agent_id, |thread, _artifact| {
            Ok(thread.resolve_tool_call(sequence, outcome, answer))
        })
        .unwrap_or(false)
    }

    /// Edit the conversation `agent_id` speaks in, and persist the entity that
    /// owns it.
    ///
    /// Which conversation that is has one rule and this is where it lives: the
    /// addressed agent's persisted binding names the storage owner. The artifact
    /// that conversation is about travels with the addressed entity, since a
    /// plan's links resolve against a document and a run's against a diff.
    ///
    /// The record is persisted whether the edit succeeded or not, and the
    /// edit's own error is what the caller hears: a rejected action must not
    /// take the writes made before it down with it.
    fn edit_agent_conversation<T>(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        edit: impl FnOnce(&mut crate::thread::Thread, crate::thread::ArtifactKind) -> Result<T, String>,
    ) -> Result<T, String> {
        let address = self.resolve_conversation_address(entity_id, Some(agent_id))?;
        if self.plans.contains_key(&address.conversation_entity_id) {
            let mut active = self.take_plan(&address.conversation_entity_id)?;
            let result = active
                .agents
                .resolve_mut(Some(&address.conversation_id))
                .and_then(|agent| edit(&mut agent.thread, address.artifact));
            let persisted =
                self.finish_plan_mutation(address.conversation_entity_id.clone(), active);
            let value = result?;
            persisted?;
            return Ok(value);
        }
        if self.runs.contains_key(&address.conversation_entity_id) {
            let mut active = self.take_run(&address.conversation_entity_id)?;
            let result = active
                .agents
                .resolve_mut(Some(&address.conversation_id))
                .and_then(|agent| edit(&mut agent.thread, address.artifact));
            let persisted =
                self.finish_run_mutation(address.conversation_entity_id.clone(), active);
            let value = result?;
            persisted?;
            return Ok(value);
        }
        Err(format!("unknown conversation owner: {entity_id}"))
    }

    /// A turn never reached an agent: record why on the entity and on the agent
    /// itself, and persist it, so the surface says what happened instead of
    /// showing a working task with nobody working and a row still starting.
    ///
    /// The state is deliberately left alone. The transition that queued this
    /// turn is already durable, and demotion belongs to one place — the idle
    /// sweep, which now reads a working entity with no agent tab as the anomaly
    /// it is. This method's whole job is the reason.
    fn record_agent_delivery_failure(&mut self, turn: &PendingAgentTurn, error: &str) {
        // A router that never reached a harness is a route that failed, and the
        // capture is where that has to show — there is no entity behind it to
        // carry the reason.
        if self.router_sessions.contains_key(&turn.owner) {
            eprintln!("router {}: {error}", turn.owner);
            self.settle_router_session(&turn.owner);
            return;
        }
        let reason = format!("could not reach the agent: {error}");
        // Both facts land in the one mutation. The entity's `last_error` is
        // the surface's line about the work; the agent's `start_error` is its
        // own word about the session it was asked to open, which is what the
        // client laid a "starting" state over the row waiting for.
        // Plan and run ids are disjoint, so the owner lookup is the router.
        if self.plans.contains_key(&turn.owner) {
            let Ok(mut active) = self.take_plan(&turn.owner) else {
                return;
            };
            if let Some(agent) = active.agents.by_id_mut(&turn.agent_id) {
                agent.start_error = Some(reason.clone());
            }
            active.last_error = Some(reason);
            let persisted = self.finish_plan_mutation(turn.owner.clone(), active);
            if let Err(error) = persisted {
                eprintln!("record_agent_delivery_failure {}: {error}", turn.owner);
            }
            return;
        }
        let Ok(mut active) = self.take_run(&turn.owner) else {
            return;
        };
        if let Some(agent) = active.agents.by_id_mut(&turn.agent_id) {
            agent.start_error = Some(reason.clone());
        }
        active.last_error = Some(reason);
        let persisted = self.finish_run_mutation(turn.owner.clone(), active);
        if let Err(error) = persisted {
            eprintln!("record_agent_delivery_failure {}: {error}", turn.owner);
        }
    }

    /// A turn found no session to open: the entity's session is over, so no
    /// agent is spawned for it. Not a failure of the work — the entity's
    /// `last_error` is left alone — but the client laid a "starting" state
    /// over the agent it addressed, and only the agent's own word takes it off.
    fn record_agent_start_declined(&mut self, turn: &PendingAgentTurn) {
        self.edit_agent_record(
            "record_agent_start_declined",
            &turn.owner,
            &turn.agent_id,
            |agent| agent.start_error = Some(AGENT_START_DECLINED_SESSION_OVER.to_string()),
        );
    }

    /// Is this entity's agent merely on its way — a turn still queued, or one
    /// off the queue and mid-delivery? Between the verb that transitions an
    /// entity (under the state lock) and the tab its turn spawns (lock free,
    /// seconds for a cold harness), a working entity has no agent tab and is
    /// perfectly healthy. Everywhere else, a working entity without one is an
    /// anomaly.
    fn agent_turn_is_undelivered(&self, owner: &str) -> bool {
        self.turns_in_flight.holds_owner(owner)
            || self
                .pending_agent_turns
                .iter()
                .any(|turn| turn.owner == owner)
    }

    /// Is a turn that will TELL this agent to read its thread already coming?
    ///
    /// Asked by every verb that would otherwise queue a SECOND turn for it: two
    /// harnesses in one checkout both report `done` for the same owner, and
    /// even where the spawn claim prevents that, the second turn survives as a
    /// duplicate `read_unread_messages` nudge. Keyed per (root, agent), so a
    /// branch's second agent is never suppressed by its first agent's turn.
    ///
    /// Only a turn with words counts. The verb that asks is about to leave a
    /// message durable on the thread, and a harness that opens on a cold
    /// prompt is told to call `read_unread_messages` — so that turn reads the
    /// message, and a second turn is a duplicate. A turn that says nothing
    /// (`agent.start` with nothing unread) opens a harness and sends it
    /// nothing: it promises the agent nothing to read, so the message has to
    /// queue its own turn, which lands Warm on the tab the start opened. The
    /// spawn claim is not consulted: a textful delivery gives its mark back
    /// only after settling the claim it became, so the mark already covers the
    /// claim's whole lifetime, and a claim with no textful mark behind it is a
    /// textless spawn.
    ///
    /// Two states, then, and a turn with words is never in neither: queued,
    /// and taken off the queue and mid-delivery.
    fn agent_is_on_its_way(&self, root: &std::path::Path, agent_id: &str) -> bool {
        let key = TabKey::agent(&Self::canonical_root(root), agent_id);
        self.turns_in_flight.holds_agent(&key)
            || self
                .pending_agent_turns
                .iter()
                .any(|queued| queued.says_something() && queued.tab_key() == key)
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

    /// Record where every live conversation has got to, without pushing. Called
    /// once at boot: the store holds history that was announced when it
    /// happened, and a restart must not announce it again.
    fn seed_conversation_attention_sequences(&mut self) {
        let addresses: Vec<(String, String)> = self
            .plans
            .iter()
            .map(|(id, plan)| (id, &plan.agents))
            .chain(self.runs.iter().map(|(id, run)| (id, &run.agents)))
            .flat_map(|(id, roster)| {
                roster
                    .iter()
                    .map(|agent| (id.clone(), agent.id.clone()))
                    .collect::<Vec<_>>()
            })
            .collect();
        let sequences: Vec<_> = addresses
            .into_iter()
            .filter_map(|(entity_id, agent_id)| {
                self.agent_conversation(&entity_id, Some(&agent_id))
                    .ok()
                    .map(|thread| (thread.id.clone(), thread.last_sequence()))
            })
            .collect();
        self.conversation_attention_sequence.extend(sequences);
    }

    /// Boot migration: give every stored entity the inbox anchor it would have
    /// had, and leave every anchored one exactly where it is.
    ///
    /// Seeding at creation alone would file a two-week-old issue you picked
    /// back up yesterday under two weeks ago, so the seed is walked forward
    /// through the user messages its conversation already holds — the same rule
    /// a live message goes through, replayed over the history that predates it.
    /// The first boot after this ships does the work; every boot after finds
    /// the anchors it wrote and does nothing.
    fn seed_anchors_for_records_without_one(&mut self) {
        let histories: Vec<(String, String, Vec<String>)> = self
            .plans
            .iter()
            .map(|(id, plan)| (id, &plan.agents))
            .chain(self.runs.iter().map(|(id, run)| (id, &run.agents)))
            .filter(|(id, _)| {
                self.attention
                    .get(id.as_str())
                    .is_none_or(|attention| attention.anchor_at.is_none())
            })
            .map(|(id, roster)| {
                // Every user message the entity ever had, not the tail the load
                // left resident: an anchor is replayed over the whole history,
                // and one replayed over part of it puts the entry in a place it
                // never had. A history the store cannot hand back leaves the
                // anchor to what is held rather than leaving the entry unseeded.
                let mut said_at: Vec<String> = roster
                    .iter()
                    .flat_map(|agent| {
                        let thread = self
                            .agent_conversation(id, Some(&agent.id))
                            .unwrap_or(&agent.thread);
                        match self.whole_conversation(thread) {
                            Ok(items) => crate::thread::Thread::user_message_times_in(&items)
                                .map(str::to_string)
                                .collect::<Vec<String>>(),
                            Err(error) => {
                                eprintln!("anchor seeding: {error}");
                                thread.user_message_times().map(str::to_string).collect()
                            }
                        }
                    })
                    .collect();
                said_at.sort();
                let created_at = self
                    .entity_created_at
                    .get(id)
                    .cloned()
                    .unwrap_or_else(now_rfc3339);
                (id.clone(), created_at, said_at)
            })
            .collect();
        if histories.is_empty() {
            return;
        }
        for (entity_id, created_at, said_at) in histories {
            let attention = self.attention.entry(entity_id).or_default();
            attention.seed_anchor(&created_at);
            for at in said_at {
                attention.note_user_message(&at);
            }
        }
        self.persist_attention();
    }

    /// What the mutation tail found on the conversation it just wrote: how far
    /// it has got, and the newest attention-class item to land since a tail
    /// last looked.
    fn conversation_news(&self, thread: &crate::thread::Thread) -> ConversationNews {
        let observed = self
            .conversation_attention_sequence
            .get(&thread.id)
            .copied();
        ConversationNews {
            thread_id: thread.id.clone(),
            sequence: thread.last_sequence(),
            attention_reason: observed.and_then(|seen| thread.unread_since(seen).reason),
        }
    }

    /// Fire one content-free web-push notify when an attention-class item lands
    /// on an entity's conversation.
    ///
    /// Event-driven, not state-driven: the state only chooses the kind's label
    /// (a plan at its review gate is `plan_ready`, not a generic attention).
    /// The watermark moves whether or not the push goes out, so one piece of
    /// news notifies once even when two entities share the conversation.
    fn push_attention_notify(
        &mut self,
        entity_id: &str,
        news: ConversationNews,
        state_kind: Option<&'static str>,
    ) {
        let first_look = self
            .conversation_attention_sequence
            .insert(news.thread_id, news.sequence)
            .is_none();
        if first_look || self.notifier.is_none() {
            return;
        }
        if let Some(kind) = self.attention_push_kind(entity_id, news.attention_reason, state_kind) {
            self.spawn_notify(entity_id.to_string(), kind);
        }
    }

    /// What one piece of news pushes as, or `None` when the phone stays dark: a
    /// muted entry, news that needs nobody, a reason with no push label, or a
    /// second push inside the entity's debounce window.
    ///
    /// Mute is checked before the window is spent, so a silence costs nothing:
    /// the first news after unmuting pushes instead of sitting out a window it
    /// never entered.
    fn attention_push_kind(
        &mut self,
        entity_id: &str,
        reason: Option<&str>,
        state_kind: Option<&'static str>,
    ) -> Option<&'static str> {
        if self.is_muted(entity_id) {
            return None;
        }
        let kind = crate::notify::kind_for_attention(reason?, state_kind)?;
        self.notify_throttle
            .should_notify(entity_id, crate::notify::unix_seconds())
            .then_some(kind)
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

    fn observe_conversation_working(&mut self, entity_id: &str, now: &str) {
        let working = self.entity_effectively_working(entity_id);
        self.observe_working_state(entity_id, working, now);
    }

    fn entity_effectively_working(&self, entity_id: &str) -> bool {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return false;
        };
        roster.iter().any(|agent| agent.working_since.is_some())
    }

    fn observe_working_state(&mut self, entity_id: &str, working: bool, now: &str) {
        if self
            .attention
            .entry(entity_id.to_string())
            .or_default()
            .observe_working(working, now)
        {
            self.persist_attention();
        }
    }

    /// The canonical form of a worktree root — the tab registry's key. Every
    /// entry point funnels through this: a run's worktree arrives as
    /// `worktrees_root/<name>` and is NOT canonical, while an external
    /// worktree's path already is, and on macOS the same directory has two
    /// literal spellings. Falls back to the raw path when the directory is
    /// gone, so a vanished worktree still keys consistently for the reaper.
    fn canonical_root(path: &std::path::Path) -> std::path::PathBuf {
        crate::worktree::canonical_root(path)
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

    /// The canonical checkout an entity's agents work in — the key they are
    /// registered under. A run's is its worktree; an issue's is the project's
    /// primary checkout, because issue agents never get a worktree.
    ///
    /// An issue whose workspace is gone (approved, abandoned) has no agent;
    /// that is a refusal, not a blank tab, because there is nothing for an
    /// agent to run in.
    fn entity_agent_root(&self, entity_id: &str) -> Result<std::path::PathBuf, String> {
        if let Some(plan) = self.plans.get(entity_id) {
            return plan
                .workspace
                .as_ref()
                .map(|workspace| Self::canonical_root(&workspace.checkout))
                .ok_or_else(|| "the issue has no session, so it has no agent".to_string());
        }
        if let Some(run) = self.runs.get(entity_id) {
            return Ok(Self::canonical_root(&run.worktree.path));
        }
        Err("unknown id".to_string())
    }

    /// An entity's agents, whichever kind of entity it is.
    fn entity_agents(&self, entity_id: &str) -> Result<&crate::agent::AgentRoster, String> {
        if let Some(plan) = self.plans.get(entity_id) {
            return Ok(&plan.agents);
        }
        if let Some(run) = self.runs.get(entity_id) {
            return Ok(&run.agents);
        }
        Err("unknown id".to_string())
    }

    /// The agent a verb means — the one it named, or the entity's first, so
    /// every verb that predates agents keeps addressing the agent it always
    /// did.
    fn resolve_agent(
        &self,
        entity_id: &str,
        agent_id: Option<&str>,
    ) -> Result<crate::agent::Agent, String> {
        self.entity_agents(entity_id)?.resolve(agent_id).cloned()
    }

    /// What an authenticated agent id turns out to be: a coding session working
    /// an entity, or a router session deciding a capture. Nothing else can
    /// reach the control socket, and the two get different tools.
    fn addressed_session(&self, agent_id: String) -> Option<AddressedSession> {
        if crate::router::is_router_agent(&agent_id) {
            return self.capture_of_router_agent(&agent_id).map(|capture_id| {
                AddressedSession::Router {
                    capture_id,
                    agent_id,
                }
            });
        }
        self.entity_of_agent(&agent_id)
            .map(|entity_id| AddressedSession::Coding {
                entity_id,
                agent_id,
            })
    }

    /// Which entity owns an agent id. The MCP control plane authenticates an
    /// AGENT (the harness knows its own id and its own token); the lifecycle it
    /// drives belongs to the entity behind it.
    fn entity_of_agent(&self, agent_id: &str) -> Option<String> {
        self.plans
            .iter()
            .map(|(id, plan)| (id, &plan.agents))
            .chain(self.runs.iter().map(|(id, run)| (id, &run.agents)))
            .find(|(_, roster)| roster.by_id(agent_id).is_some())
            .map(|(id, _)| id.clone())
    }

    /// The agent the system delivers to on `entity_id`, minting one when the
    /// human has left the branch with none.
    ///
    /// The door every path that MUST be heard takes — a post, a start, a
    /// routed instruction — so none of them reaches for a roster that may be
    /// empty. The agent is created on the entity's own persisted choice, which
    /// is the account's default harness unless somebody named another one when
    /// the entity was created or adopted.
    ///
    /// An issue always holds its one agent, so only a branch is ever minted on.
    fn ensure_primary_agent(&mut self, entity_id: &str) -> Result<String, String> {
        if let Some(primary) = self.entity_agents(entity_id)?.primary() {
            return Ok(primary.id.clone());
        }
        let choice = self.entity_model_choice(entity_id)?;
        let mut active = self.take_run(entity_id)?;
        let agent_id = active
            .agents
            .ensure_primary(entity_id, choice, &now_rfc3339())
            .id
            .clone();
        self.finish_run_mutation(entity_id.to_string(), active)?;
        Ok(agent_id)
    }

    /// Resolve the agent a verb's parameters address, minting one where asking
    /// for an agent is what the verb means.
    ///
    /// The order is the rule. The provider picker rides the parameters, so it
    /// is parsed and written FIRST: an unrunnable provider refuses before
    /// anything has been opened, and the agent resolved after it is resolved on
    /// the choice the human just made. Then the entity's own agent — named, or
    /// the primary, which a branch the human emptied is given — then the
    /// checkout it works in and the harness it runs, which is the AGENT's and
    /// not the entity's: the several agents on a branch need not share one.
    fn addressed_agent(&mut self, params: &Value) -> Result<AddressedAgent, String> {
        // `run_id` is the adopting caller's spelling: a worktree surface with no
        // run yet mints one and forwards the verb, and that helper names the id
        // it just minted. Same entity either way.
        let entity_id = params
            .get("id")
            .or_else(|| params.get("run_id"))
            .or_else(|| params.get("plan_id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or("missing id")?
            .to_string();
        let named = named_agent_id(params)?;
        let roster_is_empty = self.entity_agents(&entity_id)?.is_empty();
        let expected_conversation = optional_nonempty_string(params, "conversation_id")?;
        if roster_is_empty && named.is_some() {
            self.resolve_agent(&entity_id, named.as_deref())?;
        }
        if roster_is_empty && expected_conversation.is_some() {
            return Err("no conversation exists for the expected conversation_id".to_string());
        }
        // A provider card on an empty branch seeds the one agent it is about to
        // create. Once an agent exists, settings are written only on that exact
        // agent and its provider is its durable harness identity.
        if roster_is_empty && has_agent_choice(params) {
            let chosen = model_choice_from(params, self.default_harness)?;
            self.set_entity_model_choice(&entity_id, chosen)?;
        }
        let agent_id = match named.as_deref() {
            None if roster_is_empty => self.ensure_primary_agent(&entity_id)?,
            named => self.resolve_agent(&entity_id, named)?.id,
        };
        if let Some(expected) = expected_conversation {
            let actual = self
                .entity_agents(&entity_id)?
                .resolve(Some(&agent_id))?
                .conversation_id();
            if expected != actual {
                return Err(format!(
                    "stale conversation_id {expected}; agent {agent_id} is bound to {actual}"
                ));
            }
        }
        if !roster_is_empty && has_agent_choice(params) {
            let locked = self
                .entity_agents(&entity_id)?
                .resolve(Some(&agent_id))?
                .choice
                .provider;
            let chosen = model_choice_from(params, locked)?;
            if chosen.provider != locked {
                return Err(format!(
                    "agent.start: the agent is locked to {}",
                    locked.label()
                ));
            }
            self.set_agent_model_choice(&entity_id, &agent_id, chosen)?;
        }
        let root = self.entity_agent_root(&entity_id)?;
        let has_unread = self
            .agent_conversation(&entity_id, Some(&agent_id))?
            .has_unread();
        let roster = self.entity_agents(&entity_id)?;
        let agent = roster
            .by_id(&agent_id)
            .expect("the agent was just resolved on this roster");
        Ok(AddressedAgent {
            has_unread,
            model_choice: agent.choice.clone(),
            choice_revision: agent.choice_revision,
            conversation_id: agent.conversation_id().to_string(),
            entity_id,
            agent_id,
            root,
        })
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

    /// Whether any of an entity's agents currently holds a running harness
    /// process. An entity with no worktree has no agent and therefore none
    /// running.
    fn entity_agent_is_live(&self, entity_id: &str) -> bool {
        let Ok(root) = self.entity_agent_root(entity_id) else {
            return false;
        };
        let Ok(roster) = self.entity_agents(entity_id) else {
            return false;
        };
        roster
            .iter()
            .any(|agent| self.agent_is_live(&root, &agent.id))
    }

    /// Whether one agent's harness process is running right now.
    fn agent_is_live(&self, root: &std::path::Path, agent_id: &str) -> bool {
        self.tabs
            .get(&TabKey::agent(root, agent_id))
            .is_some_and(Tab::session_is_live)
    }

    /// Point an entity's agents at a different provider/model. The persisted
    /// choice is what every later start and turn reads, so a switch made at
    /// the Agent tab has to outlive both this process and this daemon.
    ///
    /// A running harness cannot be re-provisioned under itself: the process
    /// would keep the old provider while the record claimed the new one, and
    /// the human would have no owner-side handle on what is actually running.
    /// So a PROVIDER move is refused while a session is live. Re-asserting the
    /// choice the entity already has is not a move and passes through, which is
    /// what keeps a start that always names its provider idempotent — and a
    /// model or effort edit is not a move either: it is what the next start
    /// spends, so it persists and waits for one.
    fn set_entity_model_choice(
        &mut self,
        entity_id: &str,
        choice: ModelChoice,
    ) -> Result<(), String> {
        if self.entity_model_choice(entity_id)? == choice {
            return Ok(());
        }
        // Only a PROVIDER move is refused while a session runs: that is the one
        // that would leave the process on the old harness while the record
        // claimed the new one. A model or effort edit is what the next start
        // spends, so it persists under a live session and waits for it.
        if self.entity_model_choice(entity_id)?.provider != choice.provider
            && self.entity_agent_is_live(entity_id)
        {
            return Err(format!(
                "agent.start: an agent session is already running on {} — stop the current \
                 session first, then start it on {}",
                self.entity_model_choice(entity_id)?.provider.label(),
                choice.provider.label()
            ));
        }
        if self.plans.contains_key(entity_id) {
            let mut active = self.take_plan(entity_id)?;
            active.model_choice = choice;
            let persisted = self.persist_plan_record(entity_id, &active);
            self.plans.insert(entity_id.to_string(), active);
            self.note_entity_changed(entity_id);
            return persisted;
        }
        let mut active = self.take_run(entity_id)?;
        active.model_choice = choice;
        let persisted = self.persist_run_record(entity_id, &active);
        self.runs.insert(entity_id.to_string(), active);
        self.note_entity_changed(entity_id);
        persisted
    }

    /// Kill, reap, and forget EVERY agent rooted in a worktree, telling every
    /// attached client the tabs are gone.
    ///
    /// An agent whose owner no longer exists is worse than no agent: it keeps
    /// working and reports `done` into the unknown-entity log forever. So the
    /// two verbs that remove a run while KEEPING its worktree — release
    /// (un-adopt) and delete — close them here, all of them, because a branch
    /// may carry several. A worktree that vanishes takes its agents with it
    /// through the reaper instead.
    /// A merge that prunes its checkout takes the run's agents with it: the
    /// directory they live in is about to go, so their sessions end here,
    /// recorded on the thread, rather than lingering live until the reaper
    /// notices the root is gone.
    #[track_caller]
    fn retire_agents_of_pruned_worktree(&mut self, root: &std::path::Path) {
        let root = Self::canonical_root(root);
        let ended: Vec<SessionInstance> = self
            .tabs
            .iter()
            .filter(|(key, _)| key.is_agent() && key.root == root)
            .filter_map(|(_, tab)| tab.session_instance.clone())
            .collect();
        let _retiring = self.retire_agent_tabs(&root);
        for instance in ended {
            self.record_agent_session_end(&instance.entity_id, &instance.agent_id, &instance);
        }
    }

    #[track_caller]
    fn retire_agent_tabs(&mut self, root: &std::path::Path) -> Vec<Retirement> {
        let caller = std::panic::Location::caller();
        let root = Self::canonical_root(root);
        let keys: Vec<TabKey> = self
            .tabs
            .keys()
            .filter(|key| key.is_agent() && key.root == root)
            .cloned()
            .collect();
        keys.iter()
            .filter_map(|key| self.retire_tab_at(key, "closed", caller))
            .collect()
    }

    /// Remove one tab, tell its clients `reason`, and retire its process.
    ///
    /// The kill and the reap leave for a thread of their own
    /// ([`Retirement`]); the close push stays here, under the app mutex,
    /// because it is bounded — the screen's own lock and one channel send per
    /// client, exactly what it has always been.
    #[track_caller]
    fn retire_tab(&mut self, key: &TabKey, reason: &str) -> Option<Retirement> {
        self.retire_tab_at(key, reason, std::panic::Location::caller())
    }

    fn retire_tab_at(
        &mut self,
        key: &TabKey,
        reason: &str,
        caller: &std::panic::Location<'_>,
    ) -> Option<Retirement> {
        let provider_thread_id = self.tabs.get(key).and_then(|tab| {
            let (owner_id, agent_id) = tab.role.agent()?;
            self.recorded_resume_id(owner_id, agent_id)
        });
        let tab = self.tabs.remove(key)?;
        tab.log_lifecycle(LifecycleDiagnostic {
            event: "shutdown_requested",
            origin: "tab_retirement",
            reason: Some(reason),
            operation_id: None,
            provider_thread_id: provider_thread_id.as_deref(),
            caller: Some(caller),
        });
        if let Some(screen) = &tab.screen {
            screen.close(reason);
        }
        Some(Retirement::begin(tab.session))
    }

    /// Remove one tab and retire its process, keeping its screen for the
    /// session that replaces it.
    ///
    /// The clients are told nothing and stay attached, which is what keeps a
    /// browser's terminal where the human left it across an agent restart.
    /// [`ensure_agent_tab`]'s dead-tab replacement, and nothing else.
    #[track_caller]
    fn retire_tab_keeping_screen(
        &mut self,
        key: &TabKey,
    ) -> Option<(Retirement, Option<ScreenHandle>)> {
        let provider_thread_id = self.tabs.get(key).and_then(|tab| {
            let (owner_id, agent_id) = tab.role.agent()?;
            self.recorded_resume_id(owner_id, agent_id)
        });
        let tab = self.tabs.remove(key)?;
        tab.log_lifecycle(LifecycleDiagnostic {
            event: "shutdown_requested",
            origin: "dead_tab_replacement",
            reason: Some("replaced"),
            operation_id: None,
            provider_thread_id: provider_thread_id.as_deref(),
            caller: Some(std::panic::Location::caller()),
        });
        Some((Retirement::begin(tab.session), tab.screen))
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

    // ---- the poll surfaces' diff caches (stale-while-revalidate) -------------

    /// Whether an entry stamped at `computed_at` has aged out of its window.
    fn diff_cache_is_stale(&self, computed_at: std::time::Instant, ttl: Duration) -> bool {
        #[cfg(test)]
        if self.force_stale_diff_caches {
            return true;
        }
        computed_at.elapsed() >= ttl
    }

    /// The refresh that recomputes one run's diffstat. `None` when the run has
    /// no diff to take: it is gone, terminal, or its worktree is already pruned.
    fn run_stat_refresh(&self, run_id: &str) -> Option<DiffCacheRefresh> {
        let active = self.runs.get(run_id)?;
        if active.run.state.is_terminal() || !active.worktree.path.exists() {
            return None;
        }
        Some(DiffCacheRefresh::RunStat {
            run_id: run_id.to_string(),
            worktree: active.worktree.path.clone(),
            base_branch: active.worktree.base_branch.clone(),
        })
    }

    /// The refresh that rescans one project's external worktrees.
    fn external_scan_refresh(&self, project_id: &str) -> Option<DiffCacheRefresh> {
        let project = self.project(project_id)?;
        Some(DiffCacheRefresh::ExternalScan {
            project_id: project.id.clone(),
            worktrees: project.orch.worktrees().clone(),
            base_branch: project.base_branch.clone(),
            excluded: self.bound_worktree_paths(),
        })
    }

    /// The refresh that recomputes one project's primary-checkout summary.
    fn primary_summary_refresh(&self, project_id: &str) -> Option<DiffCacheRefresh> {
        let project = self.project(project_id)?;
        Some(DiffCacheRefresh::PrimarySummary {
            project_id: project.id.clone(),
            repo_path: project.repo_path.clone(),
            base_branch: project.base_branch.clone(),
        })
    }

    /// This daemon, held the way a background job has to hold it — see
    /// [`SettlingHandle`].
    fn settling_handle(&self) -> SettlingHandle {
        SettlingHandle(self.self_handle.clone())
    }

    /// Whether a refresh of this entry is running right now. Nothing in the
    /// daemon asks — a claim is taken and released where it is made — but a
    /// test that holds a compute open has no other way to see it.
    #[cfg(test)]
    fn diff_refresh_is_running(&self, key: &DiffCacheKey) -> bool {
        self.diff_refreshes_in_flight.contains(key)
    }

    /// Recompute this entry behind whatever the caller is about to answer with.
    ///
    /// Single-flight and non-blocking: a refresh already running absorbs this
    /// call, and one that starts here runs on a thread that holds nothing. No
    /// age test — the caller has already decided it wants the git work done.
    fn refresh_now(&mut self, refresh: DiffCacheRefresh) {
        if !self.diff_refreshes_in_flight.insert(refresh.key()) {
            return;
        }
        self.run_off_lock(DiffRefreshJob {
            refresh,
            observer: self.diff_compute_observer.clone(),
        });
    }

    /// Run a job whose claim the caller has just taken: on the runtime with
    /// the lock released, or — with no runtime and no shared handle to apply
    /// through — as [`decide_without_a_runtime`](Self::decide_without_a_runtime)
    /// has it. The one place "decide off the lock, apply under it" is written.
    fn run_off_lock<J: OffLockJob>(&mut self, job: J) {
        let spawned = match self.self_handle.as_ref().and_then(std::sync::Weak::upgrade) {
            Some(shared) => spawn_off_lock(shared, job),
            // No shared handle: nothing could apply what a thread decided.
            None => Err(job),
        };
        if let Err(unspawned) = spawned {
            self.decide_without_a_runtime(unspawned);
        }
    }

    /// [`AppState::refresh_now`] unless what it would replace is younger than
    /// `ttl`. The stamp comes from the caller because the caller has just read
    /// it: nothing here looks a timestamp up by which cache it belongs to.
    fn refresh_if_stale(
        &mut self,
        computed_at: Option<std::time::Instant>,
        ttl: Duration,
        refresh: DiffCacheRefresh,
    ) {
        let stale = computed_at.is_none_or(|at| self.diff_cache_is_stale(at, ttl));
        if stale {
            self.refresh_now(refresh);
        }
    }

    /// What a claimed job does when there is no runtime to carry it.
    ///
    /// In production that means the daemon is shutting down or was built
    /// unrooted: the claim goes straight back and the next caller tries again.
    /// The synchronous tests have no runtime and no mutex — nobody is waiting
    /// on this thread — so there the job decides here, and the read that
    /// claimed it is answered from what it found.
    fn decide_without_a_runtime<J: OffLockJob>(&mut self, job: J) {
        let claim = job.claim();
        #[cfg(test)]
        J::apply(self, claim, job.decide());
        #[cfg(not(test))]
        {
            let _ = job;
            J::abandon(self, claim);
        }
    }

    /// Store what a refresh computed and let its claim go. A refresh that was
    /// superseded while it ran describes a tree the daemon has since changed
    /// on purpose, so what it computed is dropped and only the claim goes back.
    fn publish_diff_refresh(&mut self, key: &DiffCacheKey, entry: Option<DiffCacheEntry>) {
        let claimed = self.diff_refreshes_in_flight.remove(key);
        let superseded = self.diff_refreshes_superseded.remove(key);
        if !claimed || superseded {
            return;
        }
        if let Some(entry) = entry {
            self.store_diff_entry(entry);
        }
    }

    /// Let a claim go without publishing anything.
    fn release_diff_refresh(&mut self, key: &DiffCacheKey) {
        self.diff_refreshes_in_flight.remove(key);
        self.diff_refreshes_superseded.remove(key);
    }

    /// Overtake whatever refresh of this entry is running: the caller has just
    /// written something newer than that refresh can possibly know about, so
    /// its result is dropped when it lands.
    ///
    /// The claim is deliberately kept until then. Releasing it instead — which
    /// is what the caches did before — lets the very next read start a second
    /// compute of the same thing behind the first, and then lets the first,
    /// pre-edit one land on top of the edit and discard the second's answer.
    fn supersede_diff_refresh(&mut self, key: &DiffCacheKey) {
        if self.diff_refreshes_in_flight.contains(key) {
            self.diff_refreshes_superseded.insert(key.clone());
        }
    }

    /// Write a computed entry into the cache it belongs to. The one place a
    /// kind of entry names the cache it settles in; each arm below is that
    /// cache's own write, and an entry whose run or project has since gone is
    /// dropped by it.
    fn store_diff_entry(&mut self, entry: DiffCacheEntry) {
        let now = std::time::Instant::now();
        match entry {
            DiffCacheEntry::RunStat { run_id, stat } => self.store_run_stat(run_id, stat, now),
            DiffCacheEntry::ExternalScan {
                project_id,
                worktrees,
            } => self.store_external_scan(&project_id, worktrees, now),
            DiffCacheEntry::ExternalScanUnreadable { project_id } => {
                self.store_scan_failure(&project_id, now)
            }
            DiffCacheEntry::PrimarySummary {
                project_id,
                summary,
            } => self.store_primary_summary(&project_id, summary, now),
        }
    }

    /// A run's diffstat, as of `now`.
    fn store_run_stat(&mut self, run_id: String, stat: Value, now: std::time::Instant) {
        // Two computes that disagree are files that changed. Only when there
        // was something to disagree with: an invalidated entry recomputes from
        // nothing, and that is a mutation, not a filesystem event.
        let (first, changed) = match self.run_stat_cache.get(&run_id) {
            Some((_, previous)) => (false, previous != &stat),
            None => (true, false),
        };
        if changed {
            self.run_files_changed_at
                .insert(run_id.clone(), now_rfc3339());
            // This cache IS the git watcher: two computes that disagree are
            // files that landed in the checkout, which is exactly what an
            // entity's diff surface is showing. It fires as fast as an agent
            // writes files, so the entity's own event is paced.
            self.note_entity_settled(&run_id);
        }
        self.run_stat_cache.insert(run_id, (now, stat));
        // A board answered `stat: null` for this run and claimed this refresh;
        // nothing else will ever tell it the number arrived.
        if first {
            self.note_board_changed();
        }
    }

    /// A project's checkouts, as one walk of its repository found them.
    fn store_external_scan(
        &mut self,
        project_id: &str,
        worktrees: Vec<ExternalWorktree>,
        now: std::time::Instant,
    ) {
        let Some(project) = self.project_mut(project_id) else {
            return;
        };
        // A board answered "still scanning", or answered from a list this one
        // disagrees with. Either way the rows the browser is holding are not
        // the rows this daemon would send now, so it is told to ask again.
        let changed = project
            .external_scan
            .as_ref()
            .is_none_or(|cache| cache.worktrees != worktrees);
        project.external_scan = Some(ExternalScanCache {
            scanned_at: now,
            worktrees,
        });
        project.external_scan_failed_at = None;
        if changed {
            self.note_board_changed();
        }
    }

    /// A repository this daemon could not read, so the interval is measured
    /// from the attempt rather than from a list that never arrived.
    fn store_scan_failure(&mut self, project_id: &str, now: std::time::Instant) {
        let Some(project) = self.project_mut(project_id) else {
            return;
        };
        let settling = project.external_scan_failed_at.is_none();
        project.external_scan_failed_at = Some(now);
        if settling {
            self.note_board_changed();
        }
    }

    /// A project's primary-checkout summary, as of `now`.
    fn store_primary_summary(&mut self, project_id: &str, summary: Value, now: std::time::Instant) {
        let Some(project) = self.project_mut(project_id) else {
            return;
        };
        let changed = project
            .primary_summary
            .as_ref()
            .is_none_or(|(_, previous)| previous != &summary);
        project.primary_summary = Some((now, summary));
        if changed {
            self.note_board_changed();
        }
    }

    /// Drop a run's cached diffstat — the mutation that calls this just changed
    /// the tree it described. Any refresh in flight is superseded with it.
    fn invalidate_run_stat(&mut self, run_id: &str) {
        self.run_stat_cache.remove(run_id);
        self.supersede_diff_refresh(&DiffCacheKey::RunStat(run_id.to_string()));
    }

    /// Drop a project's cached primary-checkout summary, same reasoning.
    fn invalidate_primary_summary(&mut self, project_id: &str) {
        if let Some(project) = self.project_mut(project_id) {
            project.primary_summary = None;
        }
        self.supersede_diff_refresh(&DiffCacheKey::PrimarySummary(project_id.to_string()));
    }

    /// The runs of a project that have not finished, with the id each is
    /// keyed by. Reading a run's branch costs a HEAD read on disk, so callers
    /// that want one branch stop at it rather than describing them all.
    fn live_runs_of<'a>(
        &'a self,
        project_id: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a ActiveRun)> {
        self.runs.iter().filter(move |(run_id, active)| {
            !active.run.state.is_terminal()
                && self.entity_project.get(*run_id).map(String::as_str) == Some(project_id)
        })
    }

    /// The live run that owns a branch in a project, if one does.
    fn run_on_branch(&self, project_id: &str, branch: &str) -> Option<String> {
        self.live_runs_of(project_id)
            .find(|(_, active)| active.worktree.branch() == branch)
            .map(|(run_id, _)| run_id.clone())
    }

    /// The project's external worktrees, as the last scan left them, plus
    /// whether a scan has ever landed. A read never scans: it serves what it
    /// has and claims the rescan it needs, which runs off every lock and
    /// invalidates the browser when it lands.
    fn external_worktrees(&mut self, project_id: &str) -> ScanRead {
        if let Some(refresh) = self.external_scan_refresh(project_id) {
            let settled_at = self.scan_settled_at(project_id);
            self.refresh_if_stale(settled_at, EXTERNAL_SCAN_INTERVAL, refresh);
        }
        ScanRead {
            worktrees: self
                .external_scan_of(project_id)
                .map(|cache| cache.worktrees.clone())
                .unwrap_or_default(),
            settled: self.scan_settled_at(project_id).is_some(),
        }
    }

    /// When this project's last scan attempt settled, whether it landed a list
    /// or gave up on a repository it could not read. What the interval is
    /// measured from, so a broken repo is not walked again by every poll.
    fn scan_settled_at(&self, project_id: &str) -> Option<std::time::Instant> {
        let project = self.project(project_id)?;
        project
            .external_scan
            .as_ref()
            .map(|cache| cache.scanned_at)
            .max(project.external_scan_failed_at)
    }

    /// The last scan of a project's checkouts, if one has ever landed.
    fn external_scan_of(&self, project_id: &str) -> Option<&ExternalScanCache> {
        self.projects
            .iter()
            .find(|p| p.id == project_id)?
            .external_scan
            .as_ref()
    }

    /// Scan one project's checkouts here and now, with the app mutex in hand.
    ///
    /// Tests only, and nothing else: every verb that has to decide against the
    /// checkouts that exist — a dispatch, an adoption, an implementation handed
    /// a card — asks for them in the lock-free run phase of a
    /// [`WorktreeLifecycleJob`]. What is left here is the tests' way to settle
    /// the cache before they read an id out of it.
    #[cfg(test)]
    fn scan_external_worktrees_now(
        &mut self,
        project_id: &str,
    ) -> Result<Vec<ExternalWorktree>, String> {
        let excluded = self.bound_worktree_paths();
        let base = self.base_for(project_id)?;
        let worktrees = self.orch_for(project_id)?.worktrees().clone();
        match worktrees.discover(&base, &excluded) {
            Ok(scanned) => {
                self.store_diff_entry(DiffCacheEntry::ExternalScan {
                    project_id: project_id.to_string(),
                    worktrees: scanned.clone(),
                });
                Ok(scanned)
            }
            Err(e) => {
                eprintln!("external_worktrees {project_id}: {e}");
                Err(e.to_string())
            }
        }
    }

    /// Rescan a project's checkouts behind whatever the board is serving: Build
    /// just changed something about this repository that the cached list cannot
    /// be amended for.
    fn rescan_external_worktrees(&mut self, project_id: &str) {
        if let Some(refresh) = self.external_scan_refresh(project_id) {
            self.refresh_now(refresh);
        }
    }

    /// A checkout Build just put on disk, or handed back: it joins the last
    /// scan rather than emptying it, so the very next board poll shows it.
    fn note_worktree_appeared(&mut self, project_id: &str, worktree: ExternalWorktree) {
        self.amend_external_scan(project_id, |worktrees| {
            let replaced = worktrees
                .iter()
                .position(|known| known.path == worktree.path)
                .map(|index| worktrees.remove(index));
            let changed = replaced.as_ref() != Some(&worktree);
            worktrees.push(worktree);
            crate::worktree::sort_checkouts(worktrees);
            changed
        });
    }

    /// A checkout that is gone, or that a run has taken ownership of: it leaves
    /// the last scan, which is what the rail lists as unbound. A checkout bound
    /// to a run was never in the list, so this is routinely a no-op.
    fn note_worktree_gone(&mut self, project_id: &str, path: &std::path::Path) {
        let canonical = Self::canonical_root(path);
        self.amend_external_scan(project_id, |worktrees| {
            let before = worktrees.len();
            worktrees.retain(|known| known.path != canonical);
            before != worktrees.len()
        });
    }

    /// Edit a project's last scan in place. A scan in flight described the
    /// repository as it was before this change, so the edit supersedes it and
    /// whatever it finds is dropped — the amended list is the newer truth. A
    /// project that has never been scanned is left alone, and so is the scan it
    /// has running: there is nothing here that scan is out of date about, and
    /// its first list is what shows the checkout.
    ///
    /// `amend` answers whether it changed the list. An amendment that changed
    /// nothing is not an edit: it neither overtakes the running scan nor
    /// tells the browser about a board that is as it was.
    fn amend_external_scan(
        &mut self,
        project_id: &str,
        amend: impl FnOnce(&mut Vec<ExternalWorktree>) -> bool,
    ) {
        let amended = self
            .project_mut(project_id)
            .and_then(|project| project.external_scan.as_mut())
            // The stamp is not touched: this edit knows about one checkout, and
            // the rest of the list is exactly as old as it was.
            .is_some_and(|cache| amend(&mut cache.worktrees));
        if !amended {
            return;
        }
        self.supersede_diff_refresh(&DiffCacheKey::ExternalScan(project_id.to_string()));
        self.note_board_changed();
    }

    /// The one checkout of a project that `is_it` names, or the refusal that
    /// says why there is none.
    ///
    /// A miss claims a scan and the refusal promises it, because both ways to
    /// miss are worth retrying: a checkout made outside Build since the last
    /// scan, and a project whose checkouts nothing has looked at yet. `refusal`
    /// is what the caller was asking for, in its own words; how the scan bears
    /// on it is [`scan_may_yet_show_it`], which is the same sentence wherever a
    /// checkout is missed.
    fn find_checkout(
        &mut self,
        project_id: &str,
        refusal: &str,
        is_it: impl Fn(&ExternalWorktree) -> bool,
    ) -> Result<ExternalWorktree, String> {
        let scan = self.external_worktrees(project_id);
        if let Some(checkout) = scan.worktrees.into_iter().find(|c| is_it(c)) {
            return Ok(checkout);
        }
        self.rescan_external_worktrees(project_id);
        Err(format!(
            "{refusal} ({})",
            scan_may_yet_show_it(scan.settled)
        ))
    }

    /// Resolve a client-supplied `worktree_id` against the discovered list
    /// only — a raw path is never accepted.
    fn resolve_external_worktree(
        &mut self,
        project_id: &str,
        worktree_id: &str,
    ) -> Result<ExternalWorktree, String> {
        self.find_checkout(
            project_id,
            &format!("unknown worktree_id: {worktree_id}"),
            |checkout| checkout.id == worktree_id,
        )
    }

    /// Share this state so the relay handler and the done-socket listener both
    /// drive the same tasks. Stashes a weak self-handle so `&mut self` hooks
    /// can spawn pump tasks (see the `self_handle` field).
    pub fn shared(self) -> Arc<Mutex<AppState>> {
        let state = Arc::new(Mutex::new(self));
        let changes = {
            let mut app = state.lock().unwrap();
            app.self_handle = Some(Arc::downgrade(&state));
            Arc::clone(&app.changes)
        };
        // The flusher runs on a task of its own and never takes this mutex —
        // that is the whole reason the bus is not a field it would have to
        // lock. A build with no runtime under it (the synchronous unit tests)
        // gets no flusher and simply never sends.
        ChangeBus::spawn_flusher(changes);
        state
    }

    /// The push-invalidation bus — how a frame handler subscribes the session it
    /// is serving, and how the flusher finds its subscribers.
    pub fn changes(&self) -> Arc<ChangeBus> {
        Arc::clone(&self.changes)
    }

    /// Tests only: coalesce over a shorter window, so a push test does not have
    /// to sleep out the production one. Must precede [`AppState::shared`] —
    /// that is where the flusher takes its handle.
    #[cfg(test)]
    fn with_change_window(mut self, window: Duration) -> Self {
        self.changes = ChangeBus::new(window);
        self
    }

    /// The feed moved: task lifecycle, inbox/attention, capture, agent
    /// liveness. Queues only — the send happens with this mutex released.
    fn note_board_changed(&self) {
        self.changes.note_board();
    }

    /// One entity's detail moved: its thread, stages, git state or diff. The
    /// feed shows a row for it, so this stales that too.
    fn note_entity_changed(&self, entity_id: &str) {
        self.changes.note_entity(entity_id);
    }

    /// The same, from an origin that fires on every file an agent writes: the
    /// entity's event is paced at [`crate::changes::ENTITY_SETTLE_WINDOW`].
    fn note_entity_settled(&self, entity_id: &str) {
        self.changes.note_entity_settled(entity_id);
    }

    /// The relay's frame handler over a shared state. `stream.start`/`term.attach`
    /// need the shared handle (background producers/pumps), so it dispatches
    /// through [`dispatch_frame`].
    ///
    /// The clock comes off the state, not the handler: the MCP done socket takes
    /// this mutex with no handler behind it, and both must record against one
    /// set of since-boot counters.
    pub fn handler(state: Arc<Mutex<AppState>>) -> FrameHandler {
        let clock = Arc::clone(&state.lock().unwrap().frame_clock);
        FrameHandler::new(clock, move |sender, frame, timer| {
            dispatch_frame(&state, sender, frame, timer)
        })
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
            // A socket file that ANSWERS belongs to a running daemon: its
            // harnesses dial this path for every `done`, and unlinking it out
            // from under them silently breaks each one's report. A second
            // bridge (a dev stack launched from inside an agent session that
            // inherited BRIDGE_MCP_SOCKET, say) must refuse loudly instead of
            // stealing the control plane. Only a DEAD file — one nothing
            // accepts on — is stale debris to clear.
            if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                eprintln!(
                    "done socket: {path} is already served by a live daemon; refusing to \
                     replace it. Set BRIDGE_MCP_SOCKET to a private path for this instance."
                );
                return;
            }
            let _ = std::fs::remove_file(&path);
            let listener = match bind_done_listener(std::path::Path::new(&path)) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("done socket: bind {path} failed: {e}");
                    return;
                }
            };
            eprintln!("done socket: listening on {path}");
            serve_done_listener(state, listener).await;
        });
    }

    /// Route an agent's `done` to its owner's lifecycle transition, by owner
    /// lookup (plans map, then runs map), without draining: a report that
    /// carries an Issue's scheduler on to its next stage hands that git back
    /// HERE, to the socket that can release the guard before running it. The
    /// `done` twin of [`AppState::dispatch_deferring`].
    fn done_deferring(&mut self, entity_id: &str, report: DoneReport) -> Option<DeferredWork> {
        if self.plans.contains_key(entity_id) {
            self.on_plan_agent_done(entity_id, report);
        } else if self.runs.contains_key(entity_id) {
            self.on_run_agent_done(entity_id, report);
        } else {
            eprintln!("on_agent_done: unknown entity {entity_id}");
        }
        self.deferred_work.take()
    }

    /// Route a report while retaining the authenticated actor long enough to
    /// stop only that agent's execution clock.
    fn done_deferring_for_agent(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        report: DoneReport,
    ) -> Option<DeferredWork> {
        self.record_agent_working_since(entity_id, agent_id, None);
        self.done_deferring(entity_id, report)
    }

    /// One agent's `done`, drained — the synchronous twin of
    /// [`AppState::done_deferring`], for the tests that own the state directly
    /// and have no guard to release. Running it is what the MCP control socket
    /// does with the guard released.
    #[cfg(test)]
    fn on_agent_done(&mut self, entity_id: &str, report: DoneReport) {
        let agent_id = self
            .entity_agents(entity_id)
            .ok()
            .and_then(|agents| agents.primary())
            .map(|agent| agent.id.clone());
        let deferred = match agent_id {
            Some(agent_id) => self.done_deferring_for_agent(entity_id, &agent_id, report),
            None => self.done_deferring(entity_id, report),
        };
        if let Some(deferred) = deferred {
            let done = deferred.run();
            if let Err(error) = self.apply_deferred(MCP_CONTROL_METHOD, &Value::Null, done) {
                eprintln!("on_agent_done {entity_id}: {error}");
            }
        }
    }

    /// Execute an MCP thread request against the conversation owner resolved
    /// from the agent identity baked into that session's MCP command. The
    /// authenticated agent resolves to its current plan or run; planned runs
    /// resolve unread/reply actions to the owning Issue (legacy plan id), while
    /// planless adopted runs retain their independent worktree conversation.
    #[cfg(test)]
    fn on_mcp_action(&mut self, entity_id: &str, action: BridgeAction) -> Result<Value, String> {
        let agent_id = self.entity_agents(entity_id)?.resolve(None)?.id.clone();
        self.on_agent_mcp_action(entity_id, &agent_id, action)
    }

    /// The same, for a caller that knows WHICH agent is speaking — every real
    /// one, since the MCP control plane authenticates an agent.
    fn on_agent_mcp_action(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        action: BridgeAction,
    ) -> Result<Value, String> {
        // The router's tools reach across every project and create work. A
        // coding agent is scoped to the checkout it was given, and stays there
        // however its harness frames the request.
        if action.surface() != crate::mcp::McpSurface::Coding {
            return Err(format!(
                "{} is a router tool; this session works one checkout",
                action.tool_name()
            ));
        }
        if let BridgeAction::ReadOperationMessages { operation_id } = &action {
            return self.read_operation_messages_for_agent(entity_id, agent_id, operation_id);
        }
        if let BridgeAction::SearchConversation { query } = &action {
            return self.search_agent_conversations(entity_id, agent_id, query);
        }
        if let BridgeAction::PostThreadMessage { links, .. } = &action {
            self.validate_thread_links_for_owner(entity_id, links)?;
        }
        let posted_still_working = match &action {
            BridgeAction::PostThreadMessage { still_working, .. } => Some(*still_working),
            _ => None,
        };
        let reads_unread = matches!(action, BridgeAction::ReadUnreadMessages);
        // Where an agent speaks — its own conversation, or its Issue's when it
        // is the implementation's first — is one rule, and it is
        // `edit_agent_conversation`'s.
        let now = now_rfc3339();
        let result = self.edit_agent_conversation(entity_id, agent_id, |thread, artifact| {
            apply_thread_action(thread, artifact, action, &now)
        });
        if let Ok(value) = &result {
            if reads_unread && value["working"].is_string() {
                self.start_agent_working(entity_id, agent_id, &now);
            }
            match posted_still_working {
                Some(true) => self.start_agent_working(entity_id, agent_id, &now),
                Some(false) => self.record_agent_working_since(entity_id, agent_id, None),
                None => {}
            }
            self.observe_conversation_working(entity_id, &now);
        }
        result
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
            append_plan_stage_announcements(active.agents.sole_thread_mut(), plan_id, &new_stages);
        }
        record_report_in_thread(
            active.agents.sole_thread_mut(),
            &report_for_thread,
            outcome.as_ref().err().map(String::as_str),
        );
        if outcome.is_ok() && report_for_thread.status == DoneStatus::Completed {
            if let Some(contents) = self.plan_revision_contents(plan_id, &active) {
                active.agents.sole_thread_mut().add_revision(
                    crate::thread::ArtifactKind::Plan,
                    &contents,
                    &now_rfc3339(),
                );
            }
        }
        let persisted = self.finish_plan_mutation(plan_id.to_string(), active);
        if let Err(e) = persisted {
            eprintln!("on_agent_done {plan_id}: {e}");
        }
    }

    /// A run agent reported `done`. A mid-run stage-doc revision (revising_stage_id
    /// set) is a cross-entity store write-back to the owning plan; every other
    /// report advances the run on `on_run_done`, and a validation pass may then
    /// auto-advance the next approved stage when run-all is armed.
    #[allow(clippy::cognitive_complexity)] // ratchet: on_run_agent_done is at 20, threshold 15 — bring it under, then remove
    fn on_run_agent_done(&mut self, run_id: &str, report: DoneReport) {
        let Some(mut active) = self.runs.remove(run_id) else {
            return;
        };
        if report.phase == DonePhase::Recover {
            self.consume_recovery_report(run_id, active, report);
            return;
        }
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
        let mut triage_due = false;
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
            // Build's agents are persistent and a branch carries several, so a
            // report arrives whenever any of them finishes a turn — a turn the
            // human started at a review gate, or one a dispatch handed a second
            // agent. No lifecycle event accepts those, and none should:
            // enforcement is by observation, so the report is recorded on the
            // conversation below and the branch's own state stays put.
            Ok(ReportConsumed { outcome, next }) => {
                if let ReportOutcome::OutOfPhase(illegal) = &outcome {
                    eprintln!("{}", out_of_phase_log(run_id, illegal));
                }
                // A stage that built hands itself to validation: the same
                // agent, a new turn. Queued rather than written here — the done
                // socket holds the state lock and a cold delivery needs it free.
                triage_due = self.triage_enabled
                    && crate::orchestrator::triage_is_due(&report_for_thread, next.is_some());
                if let Some(turn) = next {
                    self.pending_agent_turns.push(PendingAgentTurn::for_run(
                        run_id,
                        &mut active,
                        turn,
                    ));
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
        let failed_stage_event = (report_for_thread.status == DoneStatus::Failed
            || report_for_thread
                .outputs
                .validation
                .as_ref()
                .is_some_and(|validation| !validation.passed)
            || outcome.is_err())
        .then(|| {
            active.current_stage_id.as_deref().and_then(|stage_id| {
                plan_docs.iter().find(|doc| doc.id == stage_id).map(|doc| {
                    let summary = report_for_thread
                        .outputs
                        .validation
                        .as_ref()
                        .filter(|validation| !validation.passed)
                        .map(|validation| validation.findings.clone())
                        .unwrap_or_else(|| report_for_thread.summary.clone());
                    (stage_id.to_string(), doc.path.clone(), summary)
                })
            })
        })
        .flatten();
        let completed_stage_event = if matches!(outcome, Ok(ReportOutcome::Applied))
            && report_for_thread.phase == DonePhase::Validate
            && report_for_thread.status == DoneStatus::Completed
        {
            active.current_stage_id.as_deref().and_then(|stage_id| {
                active
                    .stage_progress(stage_id)
                    .filter(|progress| {
                        progress.state == StageProgressState::Validated { passed: true }
                    })
                    .map(|progress| {
                        (
                            stage_id.to_string(),
                            progress.completion_sha.clone(),
                            plan_docs
                                .iter()
                                .find(|doc| doc.id == stage_id)
                                .map(|doc| doc.path.clone()),
                        )
                    })
            })
        } else {
            None
        };
        let mut issue = issue_id
            .as_ref()
            .and_then(|issue_id| self.plans.remove(issue_id));
        let conversation = run_report_conversation(run_id, &mut active, issue.as_mut());
        record_report_in_thread(
            conversation,
            &report_for_thread,
            outcome.as_ref().err().map(String::as_str),
        );
        if let (Some(issue_id), Some((stage_id, stage_path, summary))) =
            (issue_id.as_deref(), failed_stage_event)
        {
            conversation.push_event_with_links(
                crate::thread::ThreadEventKind::StageFailed,
                Some(summary),
                None,
                None,
                vec![
                    crate::thread::ThreadLink::IssueStage {
                        issue_id: issue_id.to_string(),
                        stage_id,
                        path: stage_path,
                    },
                    crate::thread::ThreadLink::Implementation {
                        issue_id: issue_id.to_string(),
                        implementation_id: run_id.to_string(),
                    },
                ],
                now_rfc3339(),
            );
        }
        if let (Some(issue_id), Some((stage_id, completion_sha, stage_path))) =
            (issue_id.as_deref(), completed_stage_event)
        {
            let mut links = vec![crate::thread::ThreadLink::Implementation {
                issue_id: issue_id.to_string(),
                implementation_id: run_id.to_string(),
            }];
            if let Some(path) = stage_path {
                links.push(crate::thread::ThreadLink::IssueStage {
                    issue_id: issue_id.to_string(),
                    stage_id: stage_id.clone(),
                    path,
                });
            }
            if let Some(sha) = completion_sha {
                links.push(crate::thread::ThreadLink::Commit { sha });
            }
            conversation.push_event_with_links(
                crate::thread::ThreadEventKind::StageCompleted,
                Some(format!("Completed stage {stage_id}")),
                None,
                None,
                links,
                now_rfc3339(),
            );
        }
        // The revision names the diff the reviewer will see, and the triage
        // pass is asked to classify THAT revision — so the pass is rendered
        // from the same patch the revision was minted from, and its `based_on`
        // is what makes a later diff visibly move out from under it.
        let triage_seed = diff_revision.map(|patch| {
            let revision = conversation.add_revision(
                crate::thread::ArtifactKind::Diff,
                &patch,
                &now_rfc3339(),
            );
            (patch, revision.content_hash)
        });
        // A revision that has already been triaged is not triaged again: the
        // agent's turn is worth more than a second opinion on an unchanged diff.
        let already_triaged = |revision_sha: &String| {
            active
                .triage
                .as_ref()
                .is_some_and(|triage| triage.based_on == *revision_sha)
        };
        if triage_due {
            if let Some((patch, revision_sha)) = triage_seed
                .as_ref()
                .filter(|(_, revision_sha)| !already_triaged(revision_sha))
            {
                let turn = self.project_of(run_id).ok().and_then(|project_id| {
                    self.orch_for(&project_id).ok().and_then(|orch| {
                        orch.triage_turn(
                            &active,
                            patch,
                            revision_sha,
                            report_for_thread.outputs.completion_report.as_ref(),
                        )
                    })
                });
                if let Some(turn) = turn {
                    self.pending_agent_turns.push(PendingAgentTurn::for_run(
                        run_id,
                        &mut active,
                        turn,
                    ));
                }
            }
        }
        let persisted = self.finish_run_mutation(run_id.to_string(), active);
        if let Err(e) = persisted {
            eprintln!("on_agent_done {run_id}: {e}");
        }
        if let (Some(issue_id), Some(issue)) = (issue_id, issue) {
            let persisted = self.finish_plan_mutation(issue_id, issue);
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

    #[allow(clippy::cognitive_complexity)] // ratchet: consume_recovery_report is at 16, threshold 15 — bring it under, then remove
    fn consume_recovery_report(&mut self, run_id: &str, mut active: ActiveRun, report: DoneReport) {
        let issue_id = active.run.plan_id.as_ref().map(|id| id.0.clone());
        let now = now_rfc3339();
        let reported = report.outputs.recovery.clone();
        let mut restored_isolation_downgrade = None;
        let verification = (|| -> Result<crate::mcp::RecoveryReport, String> {
            if report.status != DoneStatus::Completed {
                return Err(format!(
                    "recovery agent reported {:?}: {}",
                    report.status, report.summary
                ));
            }
            let reported = reported.clone().ok_or("recovery report missing")?;
            let attempt = active
                .recovery
                .as_ref()
                .filter(|attempt| attempt.state == crate::run::RecoveryState::Started)
                .ok_or("no recovery attempt is awaiting a report")?;
            if reported.recovery_id != attempt.id {
                return Err("recovery nonce does not match the persisted attempt".to_string());
            }
            if !reported.recovered {
                return Err(format!(
                    "exact lineage was not recovered: {}",
                    reported.findings
                ));
            }
            if reported.branch != active.worktree.recorded_branch {
                return Err("recovery report names a different branch".to_string());
            }
            let project_id = self.project_of(run_id)?;
            let resolved = self.resolved_isolation(&project_id);
            let worktree = self
                .orch_for(&project_id)?
                .restore_run_worktree(
                    &active.worktree,
                    unregistered_restore_for(&active),
                    resolved.isolation,
                )
                .map_err(err)?;
            restored_isolation_downgrade = resolved.downgrade;
            let checkout =
                git2::Repository::open(&worktree.path).map_err(|error| error.to_string())?;
            let verified_head = checkout
                .head()
                .and_then(|head| head.peel_to_commit())
                .map_err(|error| error.to_string())?
                .id()
                .to_string();
            if reported.head_sha != verified_head {
                return Err(format!(
                    "recovery HEAD verification failed: agent reported {}, checkout is {verified_head}",
                    reported.head_sha
                ));
            }
            active.worktree = worktree;
            Ok(reported)
        })();
        if let Some(reason) = restored_isolation_downgrade {
            self.note_isolation_downgrade(run_id, &mut active, &reason);
        }

        let (event, summary, recovery_id, requested_stage_id) = match verification {
            Ok(verified) => {
                let attempt = active.recovery.as_mut().expect("verified attempt exists");
                attempt.state = crate::run::RecoveryState::Succeeded;
                attempt.report = Some(verified.clone());
                attempt.completed_at = Some(now.clone());
                active.last_error = None;
                (
                    crate::thread::ThreadEventKind::RecoverySucceeded,
                    format!(
                        "Verified recovery restored {} at {}",
                        verified.branch, verified.head_sha
                    ),
                    attempt.id.clone(),
                    attempt.requested_stage_id.clone(),
                )
            }
            Err(reason) => {
                let (recovery_id, requested_stage_id) = active
                    .recovery
                    .as_ref()
                    .map(|attempt| (attempt.id.clone(), attempt.requested_stage_id.clone()))
                    .unwrap_or_else(|| ("recovery-unmatched".to_string(), String::new()));
                if let Some(attempt) = active.recovery.as_mut() {
                    attempt.state = crate::run::RecoveryState::Failed;
                    attempt.report = reported;
                    attempt.completed_at = Some(now.clone());
                }
                active.last_error = Some(format!("verified recovery failed: {reason}"));
                if let Some(issue_id) = issue_id.as_deref() {
                    if let Some(issue) = self.plans.get(issue_id) {
                        if let Some(index) = issue
                            .stages
                            .iter()
                            .position(|stage| stage.id == requested_stage_id)
                        {
                            if let Some(predecessor) = index.checked_sub(1).and_then(|previous| {
                                active
                                    .stages
                                    .iter_mut()
                                    .find(|progress| progress.stage_id == issue.stages[previous].id)
                            }) {
                                if matches!(
                                    predecessor.publication,
                                    StagePublication::Local | StagePublication::LegacyUnknown
                                ) {
                                    predecessor.invalidation_reason = Some(format!(
                                        "preceding unpublished stage invalidated after verified recovery failed: {reason}"
                                    ));
                                }
                            }
                        }
                    }
                }
                (
                    crate::thread::ThreadEventKind::RecoveryFailed,
                    format!("Verified recovery failed: {reason}"),
                    recovery_id,
                    requested_stage_id,
                )
            }
        };
        let succeeded = event == crate::thread::ThreadEventKind::RecoverySucceeded;
        let persisted = self.finish_run_mutation(run_id.to_string(), active);
        if let Err(error) = persisted {
            eprintln!("recovery {run_id}: run persist failed: {error}");
            return;
        }
        if let Some(issue_id) = issue_id {
            if let Ok(mut issue) = self.take_plan(&issue_id) {
                let mut links = vec![
                    crate::thread::ThreadLink::Implementation {
                        issue_id: issue_id.clone(),
                        implementation_id: run_id.to_string(),
                    },
                    crate::thread::ThreadLink::Recovery { recovery_id },
                ];
                if let Some(stage) = issue
                    .stages
                    .iter()
                    .find(|stage| stage.id == requested_stage_id)
                {
                    links.push(crate::thread::ThreadLink::IssueStage {
                        issue_id: issue_id.clone(),
                        stage_id: stage.id.clone(),
                        path: stage.path.clone(),
                    });
                }
                issue.agents.sole_thread_mut().push_event_with_links(
                    event,
                    Some(summary),
                    None,
                    None,
                    links,
                    &now,
                );
                let persisted = self.finish_plan_mutation(issue_id.clone(), issue);
                if let Err(error) = persisted {
                    eprintln!("recovery {run_id}: Issue persist failed: {error}");
                    return;
                }
            }
            if succeeded {
                // The stage this recovery was for is next, and reaching it
                // cuts or puts back a checkout. The socket that carried this
                // report runs that git with the guard released, as a frame
                // does; a refusal is already written onto the Issue here.
                if let Err(error) =
                    self.defer_issue_scheduler(&issue_id, &json!({ "issue_id": issue_id }), None)
                {
                    eprintln!("recovery {run_id}: scheduler blocked: {error}");
                }
            } else if let Err(error) = self.refresh_issue_scheduler_activity(&issue_id) {
                eprintln!("recovery {run_id}: scheduler refresh failed: {error}");
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
            run_report_conversation(run_id, &mut active, plan.as_mut()),
            &report_for_thread,
            outcome.as_ref().err().map(String::as_str),
        );
        if outcome.is_ok() {
            if let (Some(pid), Some(plan_ref)) = (plan_id.as_deref(), plan.as_mut()) {
                if let Some(contents) = self.plan_revision_contents(pid, plan_ref) {
                    plan_ref.agents.sole_thread_mut().add_revision(
                        crate::thread::ArtifactKind::Plan,
                        &contents,
                        &now_rfc3339(),
                    );
                }
            }
        }
        if let (Some(pid), Some(plan)) = (plan_id, plan) {
            let persisted = self.finish_plan_mutation(pid, plan);
            if let Err(e) = persisted {
                eprintln!("on_agent_done {run_id}: plan persist: {e}");
            }
        }
        let persisted = self.finish_run_mutation(run_id.to_string(), active);
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

    /// Record that the human has seen `id` as of its current state clock, and
    /// has read its conversations through to the end — every agent's, or one
    /// named agent's. The read cursors are what unread is derived against, so
    /// this is the one place a badge clears.
    ///
    /// `report` is what the panel claims to have read — the window it holds and
    /// the message its viewport reached. See
    /// [`conversation_last_sequences`](Self::conversation_last_sequences).
    fn see_attention(&mut self, id: &str, agent_id: Option<&str>, report: ReadReport) {
        let Some(state_changed_at) = self.entity_state_clock(id) else {
            return;
        };
        let read_through = self.conversation_last_sequences(id, agent_id, report);
        let attention = self.attention.entry(id.to_string()).or_default();
        attention.see(&state_changed_at);
        for (agent_id, sequence) in read_through {
            attention.read_through(&agent_id, sequence);
        }
        self.persist_attention();
    }

    /// How far each of an entity's conversations has got, as
    /// `(agent_id, sequence)` — for one agent when the caller named one.
    ///
    /// The entity-level conversation of a planned run is its Issue's, so its
    /// first agent is read through to the end of THAT thread; every other agent
    /// speaks in its own.
    ///
    /// A conversation reaches a client as a window on its newest items, and
    /// `read_from_sequence` is where that window starts. The end of it is not
    /// the end of the conversation, so a report from such a reader carries no
    /// claim about the items below the floor: an unread message calling the
    /// human down there leaves that conversation out of the answer entirely,
    /// keeping its badge until the reader scrolls back far enough to be sent
    /// it. A caller that names no floor is one holding the whole conversation.
    fn conversation_last_sequences(
        &self,
        entity_id: &str,
        agent_id: Option<&str>,
        report: ReadReport,
    ) -> Vec<(String, u64)> {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return Vec::new();
        };
        roster
            .iter()
            .filter(|agent| agent_id.is_none_or(|named| named == agent.id))
            .filter_map(|agent| {
                let thread = self
                    .agent_conversation(entity_id, Some(&agent.id))
                    .unwrap_or(&agent.thread);
                let hidden_below = report.window_floor.is_some_and(|floor| {
                    thread.unread_attention_below(floor, self.read_cursor(entity_id, &agent.id))
                });
                (!hidden_below).then(|| (agent.id.clone(), report.reached(thread.last_sequence())))
            })
            .collect()
    }

    /// Where each of an entity's conversations stands, as
    /// `(agent_id, last_attention_sequence)` in roster order — the lines a
    /// dismissal draws, and the lines it is judged against afterwards.
    ///
    /// The entity-level conversation of a planned run is its Issue's, so the
    /// first agent's line is drawn in THAT thread; every other agent's in its
    /// own. The first pair is the roster's first agent — the only one the
    /// pre-agent dismissal folds onto.
    fn dismissal_lines(&self, entity_id: &str) -> Vec<(String, u64)> {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return Vec::new();
        };
        roster
            .iter()
            .map(|agent| {
                let thread = self
                    .agent_conversation(entity_id, Some(&agent.id))
                    .unwrap_or(&agent.thread);
                (agent.id.clone(), thread.last_message_sequence())
            })
            .collect()
    }

    fn migrate_legacy_dismissals(&mut self) {
        let entity_ids: Vec<String> = self.plans.keys().chain(self.runs.keys()).cloned().collect();
        let mut changed = false;
        for id in entity_ids {
            let Some(attention) = self.attention.get(&id) else {
                continue;
            };
            if attention.dismissal_tracks_messages {
                continue;
            }
            let message_lines = self.dismissal_lines(&id);
            let old_lines = self.legacy_dismissal_lines(&id);
            let was_still_dismissed = !old_lines.is_empty()
                && old_lines.iter().enumerate().all(
                    |(position, (agent_id, latest_attention_sequence))| {
                        attention.is_dismissed_for(
                            agent_id,
                            position == 0,
                            *latest_attention_sequence,
                        )
                    },
                );
            changed |= self.attention.get_mut(&id).is_some_and(|attention| {
                attention.migrate_dismissal_to_messages(&message_lines, was_still_dismissed)
            });
        }
        if changed {
            self.persist_attention();
        }
    }

    fn close_recovered_working_intervals(&mut self) {
        let mut changed = false;
        for attention in self.attention.values_mut() {
            changed |= attention.close_recovered_working_interval();
        }
        if changed {
            self.persist_attention();
        }
    }

    fn legacy_dismissal_lines(&self, entity_id: &str) -> Vec<(String, u64)> {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return Vec::new();
        };
        roster
            .iter()
            .map(|agent| {
                let thread = self
                    .agent_conversation(entity_id, Some(&agent.id))
                    .unwrap_or(&agent.thread);
                (agent.id.clone(), thread.last_attention_sequence())
            })
            .collect()
    }

    /// The conversation an entity's own surfaces render: its currently implicit
    /// addressed agent's stable binding.
    fn entity_conversation(&self, entity_id: &str) -> Option<&crate::thread::Thread> {
        self.agent_conversation(entity_id, None).ok()
    }

    /// The conversation a caller means: the agent it named, or the entity's
    /// implicit primary for legacy callers. The resolved agent's persisted
    /// binding, never its current roster position, selects the history.
    fn agent_conversation(
        &self,
        entity_id: &str,
        agent_id: Option<&str>,
    ) -> Result<&crate::thread::Thread, String> {
        let address = self.resolve_conversation_address(entity_id, agent_id)?;
        self.conversation_at(&address)
    }

    /// The `thread` a detail poll ships when it asked for one in particular:
    /// the named agent's conversation, cut to what the poll can hold — only
    /// what has happened past the cursor it already holds, or as much of the
    /// newest conversation as its `thread_limit` allows. `None` when the poll
    /// named no agent and carried no cursor — the view's own thread already is
    /// exactly that.
    fn detail_thread_value(
        &self,
        entity_id: &str,
        params: &Value,
    ) -> Result<Option<Value>, String> {
        let addressed = named_agent_id(params)?;
        let cursor = thread_cursor(params);
        if addressed.is_none() && cursor.is_none() && params.get("conversation_id").is_none() {
            return Ok(None);
        }
        let address = self.resolve_conversation_params(entity_id, params)?;
        let thread = self.conversation_at(&address)?;
        Ok(Some(match cursor {
            // A conversation is loaded as its tail, so a cursor from before a
            // restart can be owed news memory does not hold: an item under the
            // tail that the process before this one mutated in place. Where it
            // is, the delta is completed out of the store.
            Some(after_sequence) if thread.cursor_reaches_stored_history(after_sequence) => {
                self.stored_thread_delta(thread, after_sequence)?
            }
            Some(after_sequence) => thread.wire_value_after(after_sequence),
            None => match thread_detail(params) {
                ThreadDetail::Page(limit) => self.thread_page_at(thread, None, limit)?,
                _ => thread.wire_value(),
            },
        }))
    }

    /// What an entry says about itself in the inbox: whether an attention-class
    /// item landed past the human's read cursor, how many, and why the newest
    /// one needs them.
    ///
    /// This is the whole of `needs_attention` now. A state that needs the human
    /// is only an input to it, by way of the event that state transition emits.
    fn unread_for(
        &self,
        entity_id: &str,
        thread: Option<&crate::thread::Thread>,
    ) -> crate::thread::UnreadSummary {
        // Muted is told here rather than at the cursor: the entry says nothing
        // is waiting while the cursor keeps the truth, so unmuting shows what
        // arrived instead of a conversation silently marked read.
        if self.is_muted(entity_id) {
            return crate::thread::UnreadSummary::default();
        }
        // The entry's badge is the union of its agents': the first agent's
        // count comes off the conversation the entity's own surfaces render
        // (an Issue's, for a planned implementation), every other agent's off
        // its own.
        let Ok(roster) = self.entity_agents(entity_id) else {
            return match thread {
                Some(thread) => self.unread_including_history(
                    entity_id,
                    thread,
                    self.read_cursor(entity_id, ""),
                ),
                None => crate::thread::UnreadSummary::default(),
            };
        };
        let mut summary = crate::thread::UnreadSummary::default();
        for agent in roster.iter() {
            let agent_thread = self
                .agent_conversation(entity_id, Some(&agent.id))
                .unwrap_or_else(|_| thread.unwrap_or(&agent.thread));
            let unread = self.unread_including_history(
                &agent_thread.agent.id,
                agent_thread,
                self.read_cursor(entity_id, &agent.id),
            );
            summary.count += unread.count;
            summary.reason = unread.reason.or(summary.reason);
        }
        summary
    }

    /// What one agent's bubble says: how much of its conversation has needed
    /// the human since they last read it. A muted entry silences every bubble
    /// under it — the entry's badge is the union of theirs, so one that still
    /// counted would contradict the entry above it.
    fn agent_unread(
        &self,
        entity_id: &str,
        agent: &crate::agent::Agent,
        thread: &crate::thread::Thread,
    ) -> crate::thread::UnreadSummary {
        if self.is_muted(entity_id) {
            return crate::thread::UnreadSummary::default();
        }
        self.unread_including_history(
            &thread.agent.id,
            thread,
            self.read_cursor(entity_id, &agent.id),
        )
    }

    /// One conversation's unread, counting the part of it this process did not
    /// load.
    ///
    /// A conversation is held as its newest items, so counting the badge off
    /// what is resident under-reports exactly when it matters most — the human
    /// has not read in a while and the unread has fallen under the tail. The
    /// badge is the one number they use to decide whether to look, so it is
    /// counted in the database rather than guessed from the tail. A
    /// conversation that was loaded whole has no history under it and asks
    /// nothing.
    fn unread_including_history(
        &self,
        agent_id: &str,
        thread: &crate::thread::Thread,
        cursor: u64,
    ) -> crate::thread::UnreadSummary {
        let mut summary = thread.unread_since(cursor);
        let floor = thread.resident_from_sequence();
        if floor == 0 || floor <= cursor {
            return summary;
        }
        let Some(store) = &self.store else {
            return summary;
        };
        match store.unread_attention_between(agent_id, cursor, floor) {
            Ok(under) => summary.count += under,
            // A badge is not worth failing a poll over: report what is
            // resident, which is an undercount rather than a wrong kind of
            // answer.
            Err(error) => eprintln!("unread under the tail for {agent_id}: {error}"),
        }
        summary
    }

    /// How far the human has read one agent's conversation, folding in the
    /// pre-agent cursor the entity's first agent inherited.
    fn read_cursor(&self, entity_id: &str, agent_id: &str) -> u64 {
        let Some(attention) = self.attention.get(entity_id) else {
            return 0;
        };
        let inherited = if self
            .entity_agents(entity_id)
            .is_ok_and(|roster| roster.is_primary(agent_id))
        {
            attention.last_read_sequence
        } else {
            0
        };
        attention.cursor_for(agent_id).max(inherited)
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
        // The inbox is ordered and coloured by this map, so a stamp on it IS a
        // feed change — an interaction, a seen, a mute, a dismissal. Noted
        // ahead of the write, because the map moved whether or not there is a
        // store under this bridge to write it to.
        self.note_board_changed();
        let Ok(store) = self.require_store() else {
            return;
        };
        let live: std::collections::HashSet<String> = self
            .runs
            .keys()
            .chain(self.plans.keys())
            .cloned()
            .chain(self.attention_worktree_ids())
            .chain(self.live_row_keys())
            .collect();
        if let Err(e) = store.save_attention(&self.attention, &live) {
            eprintln!("attention: {e}");
        }
    }

    /// Keys worth keeping for the rows no entity stands behind: the ones whose
    /// project is still registered.
    ///
    /// That is the whole liveness test. The branch such a row names may be
    /// checked out anywhere, or nowhere yet, so pruning its dismissal against a
    /// checkout would lose it every time the user moved one — and a branch that
    /// becomes a run has its record dropped at adoption
    /// ([`forget_row_dismissals`](Self::forget_row_dismissals)) rather than
    /// waiting to be pruned.
    fn live_row_keys(&self) -> Vec<String> {
        self.attention
            .keys()
            .filter(|key| {
                crate::attention::RowKey::parse(key)
                    .is_some_and(|row| self.projects.iter().any(|p| p.id == row.project_id()))
            })
            .cloned()
            .collect()
    }

    /// Worktree ids worth keeping attention for: every one the scan can still
    /// see. Their records live nowhere else, so the scan IS the liveness test —
    /// and a project whose scan has never landed is no evidence that its
    /// checkouts are gone. Until every project has a list, every key that names
    /// a checkout is kept and the write after the first scan prunes; a key of
    /// any other shape answers to the map that owns it either way.
    fn attention_worktree_ids(&self) -> Vec<String> {
        if self.projects.iter().any(|p| p.external_scan.is_none()) {
            return self
                .attention
                .keys()
                .filter(|key| crate::worktree::is_checkout_id(key))
                .cloned()
                .collect();
        }
        self.projects
            .iter()
            .filter_map(|p| p.external_scan.as_ref())
            .flat_map(|cache| cache.worktrees.iter().map(|w| w.id.clone()))
            .collect()
    }

    /// Route a verb, record it if it counts as the human touching something,
    /// and run the deferred git work inline — the synchronous entry
    /// point ([`AppState::handle`] and the unit tests), which owns the state
    /// directly and has no mutex to release. Everything running is
    /// [`dispatch_frame`], which runs the same work with the lock free.
    #[cfg(test)]
    fn dispatch(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        let (outcome, deferred) = self.dispatch_deferring(method, params);
        // No `Arc` to release the mutex through — the synchronous entry point.
        // The git work runs right here, exactly as it did before the split;
        // [`dispatch_frame`] is the caller that runs it with the lock free.
        match deferred {
            Some(deferred) => {
                let done = deferred.run();
                self.apply_deferred(method, params, done)
            }
            None => outcome,
        }
    }

    /// Dispatch without draining: a verb that handed its git work to
    /// [`AppState::deferred_work`] hands it back out HERE, to a caller that
    /// can release the app mutex before running it. The `Ok` returned
    /// alongside a deferral is the placeholder that field documents.
    fn dispatch_deferring(
        &mut self,
        method: &str,
        params: &Value,
    ) -> (Result<Value, String>, Option<DeferredWork>) {
        let queued_before = self.pending_agent_turns.len();
        let outcome = self.route(method, params);
        if outcome.is_err() {
            self.drop_turns_queued_since(queued_before);
        }
        match self.deferred_work.take() {
            // Nothing is settled until the git work returns, so the stamp waits
            // for `apply_deferred` too.
            Some(deferred) => (outcome, Some(deferred)),
            None => {
                if let Ok(result) = &outcome {
                    // Only a verb that SUCCEEDED counts: a rejected action never happened.
                    self.stamp_interaction_for(method, params, result);
                }
                (outcome, None)
            }
        }
    }

    /// Write back what the lock-free git work found, and stamp the verb that
    /// deferred it — the second half of [`AppState::dispatch_deferring`].
    fn apply_deferred(
        &mut self,
        method: &str,
        params: &Value,
        done: DeferredOutcome,
    ) -> Result<Value, String> {
        // Whether the git that just ran off-lock CHANGED anything. A read
        // deferred its work to keep the mutex free and writes nothing back, so
        // nothing about it is worth telling a browser; a mutating git verb
        // moved the tree every diff surface is showing.
        let mutating = match &done {
            DeferredOutcome::Lifecycle(_) => true,
            DeferredOutcome::Finish { .. } => true,
            DeferredOutcome::Git { git, .. } => git.invalidates,
            DeferredOutcome::Read(_) => false,
        };
        let queued_before = self.pending_agent_turns.len();
        let applied = match done {
            DeferredOutcome::Lifecycle(outcome) => self.apply_lifecycle(*outcome),
            DeferredOutcome::Finish { epilogue, finished } => {
                self.apply_finish(*epilogue, *finished)
            }
            DeferredOutcome::Git { git, result } => self.apply_git(&git, result),
            // A read writes nothing back: its answer is the whole result.
            DeferredOutcome::Read(result) => result,
        };
        match &applied {
            Ok(result) => {
                self.stamp_interaction_for(method, params, result);
                // HERE, not before the drain: the decide half only claimed the
                // checkout, and a browser told to refetch then would have read
                // the state this write-back is about to replace.
                if mutating {
                    for entity_id in entity_ids_of(params, result) {
                        self.note_entity_changed(&entity_id);
                    }
                    self.note_board_changed();
                }
            }
            Err(_) => self.drop_turns_queued_since(queued_before),
        }
        applied
    }

    /// Forget what a failed request queued for an agent. A turn is not
    /// deliverable until the mutation that queued it is durable, and only this
    /// request's turns are dropped: a later harmless verb's drain would
    /// otherwise deliver work that nothing was ever written down for. What was
    /// written down before the refusal stays — see
    /// [`PendingAgentTurn::survives_refusal`].
    ///
    /// The two halves of a request both end here — [`dispatch_deferring`] for
    /// what refused before the git ran, [`apply_deferred`] for what failed
    /// writing the git down — so every drain in the daemon, frame, MCP control
    /// socket and test twin alike, inherits the rule.
    ///
    /// The queue can be SHORTER than it was measured: a request that retires
    /// an agent drops that agent's turns however early they were queued, and
    /// a refusal after that must find nothing of its own left, not a panic.
    ///
    /// [`dispatch_deferring`]: AppState::dispatch_deferring
    /// [`apply_deferred`]: AppState::apply_deferred
    fn drop_turns_queued_since(&mut self, queued_before: usize) {
        let queued_by_others = queued_before.min(self.pending_agent_turns.len());
        let mut queued_by_this_request = self.pending_agent_turns.split_off(queued_by_others);
        queued_by_this_request.retain(|turn| turn.survives_refusal);
        self.pending_agent_turns.append(&mut queued_by_this_request);
    }

    fn route(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        match method {
            // `push_events` rides the probe as well as the greeting: a client
            // that only ever pings can still tell whether this bridge will
            // invalidate for it, and an old client ignores the extra field.
            "ping" => Ok(json!({ "pong": true, "push_events": true })),
            // What a start leads with is the account's answer, so the default
            // provider is the account's default harness. `models`/`efforts` are
            // that harness's catalog, repeated at the top level for clients
            // that predate `providers`.
            "models.list" => Ok(json!({
                "models": harness_for(self.default_harness).models(),
                "efforts": harness_for(self.default_harness).effort_levels(),
                "default_provider": self.default_harness,
                "agent_modes": self.agent_modes,
                "providers": models::provider_catalogs(),
            })),
            "thread.revision" => self.thread_revision(params),
            "thread.page" => self.thread_page(params),
            "thread.activity" => self.thread_activity(params),
            "thread.post" => self.thread_post(params),
            "thread.operation" => self.thread_operation(params),
            "thread.attach" => self.thread_attach(params),
            "thread.attachment" => self.thread_attachment(params),
            "fs.list" => self.fs_list(params),
            "fs.tree" => self.fs_tree(params),
            "fs.read" => self.fs_read(params),
            "project.diff" => self.project_diff(params),
            "git.log" => self.git_log(params),
            "git.show" => self.git_show(params),
            "git.status" => self.git_status(params),
            "git.diff" => self.git_diff(params),
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
            "project.list" => Ok(self.defer_project_list()),
            "project.add" => self.project_add(params),
            "project.create" => self.project_create(params),
            "project.clone" => self.project_clone(params),
            "project.set_remote" => self.project_set_remote(params),
            "project.set_isolation" => self.project_set_isolation(params),
            "board.list" => Ok(self.board_list()),
            // Capture surface: what the user said, kept before anything routes it.
            "capture.create" => self.capture_create(params),
            "capture.list" => Ok(self.capture_list()),
            "capture.get" => self.capture_get(params),
            "capture.answer" => self.capture_answer(params),
            "capture.reroute" => self.capture_reroute(params),
            "capture.cancel" => self.capture_cancel(params),
            "archive.list" => self.archive_list(params),
            "archived.list" => Ok(self.archived_list()),
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
            // Branch surface: the work item the feed and the URLs speak, over
            // whichever of run / worktree / primary checkout stores it.
            "branch.get" => self.branch_get(params),
            "branch.dispatch" => self.branch_dispatch(params),
            "branch.finish" => self.branch_finish(params),
            "worktree.create" => self.worktree_create(params),
            "worktree.finish" => self.worktree_finish(params),
            "entity.seen" => self.entity_seen(params),
            "entity.mute" => self.entity_mute(params),
            "entity.dismiss" => self.entity_dismiss(params),
            "triage.override" => self.triage_override(params),
            "agent.add" => self.agent_add(params),
            "agent.choose" => self.agent_choose(params),
            "agent.remove" => self.agent_remove(params),
            "agent.list" => self.agent_list(params),
            "worktree.diff" => self.worktree_diff(params),
            "stream.events" => self.stream_events(params),
            "stream.state" => self.stream_state(params),
            "term.list" => self.term_list(params),
            "term.close" => self.term_close(params),
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
            .filter_map(|(key, tab)| {
                // A shell IS its terminal, so the filter above already excluded
                // the only role that can be without one.
                let (cols, rows) = tab.screen.as_ref()?.size();
                Some((
                    term_id_suffix(&key.tab_id),
                    json!({
                        "term_id": tab.tab_id,
                        "kind": SHELL_TAB_KIND,
                        "cols": cols,
                        "rows": rows,
                        "created_at": tab.created_at,
                    }),
                ))
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
        if key.is_agent() {
            // The agent tab is not one of the human's tabs to close: it is
            // always reachable, and its life is bound to the worktree.
            return Err("cannot close an agent terminal".to_string());
        }
        self.retire_tab(&key, "closed").ok_or("unknown term_id")?;
        Ok(json!({ "ok": true }))
    }

    /// What one tab hands a client that attaches to it, taken out of the
    /// registry so the attach itself runs with the app mutex released.
    fn attachment(&self, key: &TabKey) -> Result<TabAttachment, String> {
        let tab = self.tabs.get(key).ok_or("unknown term_id")?;
        Ok(TabAttachment {
            facts: TabFacts {
                term_id: tab.wire_id(),
                live: tab.live,
                // Which harness is behind this screen. Null for a shell, and
                // null for a worktree nothing has ever run in — the client
                // leads its start offer with its own default there instead.
                provider: match tab.role {
                    TabRole::Agent { provider, .. } => Some(provider),
                    TabRole::Shell => None,
                },
            },
            terminal: tab.terminal_handle()?,
        })
    }

    /// The sessions' peer connections, for a caller that must not hold the app
    /// mutex while a peer negotiates.
    fn peers(&self) -> Arc<SessionPeers> {
        self.peers.clone()
    }

    /// A session ended: detach it from every tab so the pumps stop encrypting
    /// (and serializing) output frames into a session the relay will just drop.
    ///
    /// Its peer connection goes the same way: an ICE negotiation belongs to the
    /// session that offered it, and this runs only on a real session end — the
    /// teardown rule (`carrier.rs`), never a bare relay-socket loss.
    ///
    /// A screen waiting for its first spawn is a tab one step early and follows
    /// the same rule: [`ensure_agent_tab`] carries its clients onto the real
    /// screen, so a session left behind here would be pushed to for the life of
    /// that tab. The screen itself stays in the registry however empty it is:
    /// an attach clones it under the app mutex and registers on it with the
    /// mutex released, so the client arriving as the last one leaves must
    /// still find it where the spawn will look. Emptied, it carries no
    /// viewport — the spawn is sized the way an unwatched spawn always was —
    /// and the next attach's viewport resizes it. It is bounded at one per
    /// agent key and leaves with the spawn that inherits it, the agent's
    /// retirement, or the reaper.
    fn drop_session(&mut self, session_id: &str) {
        self.peers.end_session(session_id);
        for screen in self
            .tabs
            .values()
            .filter_map(|tab| tab.screen.as_ref())
            .chain(self.agent_screens_awaiting_spawn.values())
        {
            screen.detach(session_id);
        }
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
    /// close Build's agent themselves ([`AppState::retire_agent_tabs`]), because
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
        let mut killed_agents: Vec<SessionInstance> = Vec::new();
        for key in vanished {
            let Some(tab) = self.tabs.get(&key) else {
                continue;
            };
            let wire_id = tab.wire_id();
            if let Some(instance) = &tab.session_instance {
                killed_agents.push(instance.clone());
            }
            self.retire_tab(&key, "reaped");
            reaped.push(wire_id);
        }
        // The kill above is one the pump can never report: the tab left the
        // registry before the process died, so the pump's EOF finds no tab and
        // records nothing. This is the one teardown where the OWNER may stay on
        // the board (a worktree deleted by hand out from under a live run) — so
        // the session lineage and any turn the dead agent was holding close
        // here, or the row reads as working forever. Owners that left the board
        // in the same mutation (delete, merge-prune) make this a quiet no-op,
        // and abandon already closed its own. The loop runs after every removal
        // above so the nested reap inside `finish_run_mutation` finds nothing
        // left to take.
        for instance in killed_agents {
            self.record_agent_session_end(&instance.entity_id, &instance.agent_id, &instance);
        }
        // The screens waiting for a first spawn go the same way: a worktree
        // that is gone will never host the agent their clients are watching
        // for, and a screen nothing can ever paint is not one to keep.
        let orphaned: Vec<TabKey> = self
            .agent_screens_awaiting_spawn
            .keys()
            .filter(|key| !key.root.exists())
            .cloned()
            .collect();
        for key in orphaned {
            let Some(screen) = self.agent_screens_awaiting_spawn.remove(&key) else {
                continue;
            };
            screen.close("reaped");
            reaped.push(key.tab_id);
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
            // Asked once, so the two questions below cannot be answered by two
            // different moments of the same session.
            let status = tab.session.status();
            if let AgentStatus::Ended { code } = status {
                return Some(Some(HarnessExit {
                    code: code.unwrap_or(-1),
                    // The screen first, the session second. A retained screen is
                    // the last words of a harness Build could only see the
                    // outside of; a session that reports its own errors was told
                    // them, and hands back what it was told.
                    epitaph: tab
                        .screen
                        .as_ref()
                        .and_then(ScreenHandle::epitaph)
                        .or_else(|| tab.session.epitaph()),
                }));
            }
            // A session that reports its own turn boundaries cannot be
            // demoted mid-turn: a model reasoning for forty minutes is working
            // and silent, and silence is the only instrument the two clocks
            // below own. For a PTY this changes nothing — paint inside 30s is
            // what makes one `Working`, so a tab quiet past a threshold minutes
            // long can never claim it.
            if matches!(status, AgentStatus::Working) {
                return None;
            }
            let quiet_for = quiet_threshold;
            let heard_from_recently = tab.session.quiet_for() < quiet_for;
            let spoken_to_recently = tab
                .last_delivered_at
                .is_some_and(|at| at.elapsed() < quiet_for);
            if heard_from_recently || spoken_to_recently {
                None
            } else {
                Some(None)
            }
        };
        let idle_plans: Vec<(String, Option<HarnessExit>)> = self
            .plans
            .iter()
            .filter(|(_, a)| a.plan.state.is_working())
            .filter_map(|(id, a)| {
                let root = a
                    .workspace
                    .as_ref()
                    .map(|workspace| Self::canonical_root(&workspace.checkout))?;
                idle_check(
                    self.tabs
                        .get(&TabKey::agent(&root, &a.agents.primary()?.id)),
                    self.agent_turn_is_undelivered(id),
                )
                .map(|exit| (id.clone(), exit))
            })
            .collect();
        let idle_runs: Vec<(String, Option<HarnessExit>)> = self
            .runs
            .iter()
            .filter(|(_, a)| a.run.state.is_working())
            .filter_map(|(id, a)| {
                let root = Self::canonical_root(&a.worktree.path);
                idle_check(
                    self.tabs
                        .get(&TabKey::agent(&root, &a.agents.primary()?.id)),
                    self.agent_turn_is_undelivered(id),
                )
                .map(|exit| (id.clone(), exit))
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
            if let Some(exit) = &exit_code {
                active.last_error = Some(exit.describe());
            }
            record_idle_in_thread(active.agents.sole_thread_mut(), exit_code.as_ref());
            let persisted = self.finish_plan_mutation(plan_id.clone(), active);
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
            if let Some(exit) = &exit_code {
                active.last_error = Some(exit.describe());
            }
            if let Err(e) = self.record_on_run_conversation(&mut active, |thread| {
                record_idle_in_thread(thread, exit_code.as_ref())
            }) {
                eprintln!("idle monitor {run_id}: {e}");
            }
            let persisted = self.finish_run_mutation(run_id.clone(), active);
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
                let (demoted, routers) = {
                    let mut app = state.lock().unwrap();
                    // A router process that died mid-decision told nobody, and
                    // its capture would otherwise read as being routed forever.
                    (
                        app.mark_idle_tasks(quiet_threshold),
                        app.reap_finished_router_sessions(),
                    )
                };
                for task_id in demoted {
                    eprintln!("idle monitor: {task_id} went idle without a done report");
                }
                for capture_id in routers {
                    eprintln!("idle monitor: the router on {capture_id} stopped");
                }
                // Asked with the lock RELEASED: a terminal answers this off its
                // harness's transcript tree, which is a filesystem read.
                capture_conversation_names(&state);
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

    /// The live run that already owns the checkout a client names by
    /// `worktree_id`. Adoption takes the worktree off the external list, so the
    /// id is re-derived from each run's canonical worktree root — the same way
    /// the scanner minted it. A terminal run has let go of the checkout, so it
    /// does not answer here.
    fn run_owning_worktree_id(&self, project_id: &str, worktree_id: &str) -> Option<String> {
        self.runs
            .iter()
            .find(|(run_id, active)| {
                !active.run.state.is_terminal()
                    && self.entity_project.get(*run_id).map(String::as_str) == Some(project_id)
                    && crate::worktree::external_worktree_id(&Self::canonical_root(
                        &active.worktree.path,
                    )) == worktree_id
            })
            .map(|(run_id, _)| run_id.clone())
    }

    /// Every project's external worktrees, ride-along shape for `task.list`
    /// (spec §5.3): scan order per project, projects concatenated in
    /// registration order. A per-project scan failure is already logged inside
    /// `external_worktrees`; it just contributes nothing here.
    fn external_worktrees_json(&mut self) -> ExternalWorktreeRows {
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
        let mut rows = ExternalWorktreeRows::default();
        for (project_id, project_name, base_branch) in projects {
            let scan = self.external_worktrees(&project_id);
            rows.scanning |= !scan.settled;
            for w in scan.worktrees {
                let adoptable = w.branch.as_deref().is_some_and(|b| b != base_branch);
                let (agent_working, can_finish) =
                    agent_signals.get(&w.id).copied().unwrap_or((false, false));
                rows.rows.push(json!({
                    "worktree_id": w.id,
                    "project_id": project_id,
                    "project": project_name,
                    "path": w.path.display().to_string(),
                    "isolation": w.isolation.wire(),
                    "branch": w.branch,
                    "head_sha": w.head_sha,
                    "head_subject": w.head_subject,
                    "head_age_seconds": w.head_age_seconds,
                    "head_committed_at": w.head_committed_at,
                    "dirty_files": w.dirty_files,
                    // Ahead and behind always share one comparison ref. The
                    // working-tree delta is reported separately below.
                    "comparison_ref": w.comparison_ref,
                    "ahead": w.ahead,
                    "behind": w.behind,
                    "base_branch": base_branch,
                    "unpushed": w.unpushed,
                    "upstream": w.upstream,
                    "diffstat": w.diffstat.to_json(),
                    // What is sitting in the tree unsaved — the rail's +/−.
                    "uncommitted": w.uncommitted.to_json(),
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
        rows
    }

    /// Every project's primary-checkout changes summary, held per project for
    /// [`PRIMARY_SUMMARY_TTL`] (spec §5.3) and then served stale while it
    /// refreshes — the `task.list` ride-along for the sidebar "main" row and the
    /// project page's MAIN bucket. A per-project failure (unborn HEAD, fs error)
    /// logs and contributes nothing, same posture as `external_worktrees_json`.
    fn primary_changes_json(&mut self) -> Vec<Value> {
        // Who owns each primary checkout, so the main row can route to its run
        // after a reload. Resolved up front: the loop below holds a &mut borrow
        // of the summary cache.
        let owners: HashMap<String, String> = self
            .projects
            .iter()
            .filter_map(|project| {
                self.primary_run_of(&project.id)
                    .map(|run_id| (project.id.clone(), run_id))
            })
            .collect();
        // Ownership changes on its own schedule, so it is stamped onto the
        // outgoing entry rather than into the cached git summary.
        let with_owner = |mut entry: Value, project_id: &str| {
            entry["run_id"] = owners
                .get(project_id)
                .cloned()
                .map_or(Value::Null, Value::String);
            entry
        };

        let project_ids: Vec<String> = self.projects.iter().map(|p| p.id.clone()).collect();
        project_ids
            .into_iter()
            .filter_map(|project_id| {
                let summary = self.primary_summary(&project_id)?;
                Some(with_owner(summary, &project_id))
            })
            .collect()
    }

    /// One project's primary-checkout summary as the last walk left it, or
    /// `None` until the first one lands. Claims the walk it needs; never takes
    /// one itself.
    fn primary_summary(&mut self, project_id: &str) -> Option<Value> {
        if let Some(refresh) = self.primary_summary_refresh(project_id) {
            let computed_at = self.primary_summary_of(project_id).map(|(at, _)| *at);
            self.refresh_if_stale(computed_at, PRIMARY_SUMMARY_TTL, refresh);
        }
        self.primary_summary_of(project_id)
            .map(|(_, summary)| summary.clone())
    }

    /// The last walk of a project's primary checkout, if one has ever landed.
    fn primary_summary_of(&self, project_id: &str) -> Option<&(std::time::Instant, Value)> {
        self.projects
            .iter()
            .find(|p| p.id == project_id)?
            .primary_summary
            .as_ref()
    }

    /// The primary checkout's uncommitted-changes review surface (spec §5.2):
    /// same shape as `worktree.diff` so `parseDiff`/`diffFilesHtml` reuse is
    /// mechanical.
    fn project_diff(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let repo_path = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|project| project.repo_path.clone())
            .ok_or_else(|| "unknown project_id".to_string())?;
        Ok(self.defer_read(
            ReadSubject::Project {
                project_id,
                repo_path,
            },
            None,
        ))
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

    /// Resolve a `git.*` verb's checkout under the lock and hand the git call
    /// itself to the drain, which makes it with the mutex released.
    ///
    /// `invalidates` says a successful call leaves the board's cached
    /// summaries describing a tree that has since changed.
    fn defer_git(
        &mut self,
        params: &Value,
        invalidates: bool,
        work: fn(&GitScope, &Value) -> Result<Value, String>,
    ) -> Result<Value, String> {
        let scope = self.resolve_git_scope(params)?;
        Ok(self.defer_git_work(scope, work, params, invalidates))
    }

    /// [`AppState::defer_git`] for the verbs that address the repository's
    /// branches rather than one checkout's working tree.
    fn defer_branch_git(
        &mut self,
        params: &Value,
        invalidates: bool,
        work: fn(&BranchScope, &Value) -> Result<Value, String>,
    ) -> Result<Value, String> {
        let scope = self.resolve_branch_scope(params)?;
        Ok(self.defer_git_work(scope, work, params, invalidates))
    }

    /// [`AppState::defer_branch_git`] for the verbs that render the project's
    /// branches. They alone carry [`ProjectCheckouts`], which a checkout verb
    /// has no row to stamp with; the drain asks git who holds what.
    fn defer_branch_listing(
        &mut self,
        params: &Value,
        invalidates: bool,
        work: fn(&BranchListingScope, &Value) -> Result<Value, String>,
    ) -> Result<Value, String> {
        let checkout = self.resolve_branch_scope(params)?;
        let checkouts = self.project_checkouts(&checkout.project_id)?;
        let scope = BranchListingScope {
            checkout,
            checkouts,
        };
        Ok(self.defer_git_work(scope, work, params, invalidates))
    }

    /// Hand a resolved diff to the drain, which renders it with the mutex
    /// released. The `Value` returned is the placeholder
    /// [`AppState::deferred_work`] documents.
    fn defer_read(&mut self, subject: ReadSubject, issue_id: Option<String>) -> Value {
        self.defer_conditional_read(subject, issue_id, None)
    }

    fn defer_conditional_read(
        &mut self,
        subject: ReadSubject,
        issue_id: Option<String>,
        if_diff_key: Option<&str>,
    ) -> Value {
        self.deferred_work = Some(DeferredWork::Read(Box::new(DeferredRead {
            subject,
            issue_id,
            if_diff_key: if_diff_key.map(str::to_string),
            #[cfg(test)]
            gate: self.off_lock_gate.clone(),
        })));
        Value::Null
    }

    fn defer_git_work<S: GitCallScope + 'static>(
        &mut self,
        scope: S,
        work: fn(&S, &Value) -> Result<Value, String>,
        params: &Value,
        invalidates: bool,
    ) -> Value {
        self.deferred_work = Some(DeferredWork::Git(Box::new(DeferredGit {
            call: Box::new(ScopedGitCall { scope, work }),
            params: params.clone(),
            invalidates,
            #[cfg(test)]
            gate: self.off_lock_gate.clone(),
        })));
        Value::Null
    }

    /// Write back a git verb that ran with the mutex released: drop the cached
    /// summaries its mutation made stale.
    ///
    /// STALENESS: the checkout may have left the board while the git ran (its
    /// run released, finished, or deleted). The answer still stands — it
    /// describes what the tree did — but the cache write is dropped rather
    /// than stamping an entity that is gone back into the daemon's maps.
    fn apply_git(
        &mut self,
        git: &DeferredGit,
        result: Result<Value, String>,
    ) -> Result<Value, String> {
        if !git.invalidates || result.is_err() {
            return result;
        }
        git.call.invalidate(self);
        result
    }

    /// A branch switch swaps the whole tree, so whichever summary described
    /// the scoped checkout is stale.
    fn invalidate_branch_scope_caches(&mut self, scope: &BranchScope) {
        if !self.projects.iter().any(|p| p.id == scope.project_id) {
            return;
        }
        if scope.external_worktree {
            self.rescan_external_worktrees(&scope.project_id);
        } else {
            self.invalidate_primary_summary(&scope.project_id);
        }
    }

    /// Whether the entity a git scope spoke for is still on the board.
    fn git_scope_is_current(&self, scope: &GitScope) -> bool {
        if let Some(run) = &scope.run {
            return self.runs.contains_key(&run.run_id);
        }
        let project_id = scope
            .project_id
            .as_deref()
            .or(scope.worktree.as_ref().map(|w| w.project_id.as_str()));
        project_id.is_some_and(|project_id| self.projects.iter().any(|p| p.id == project_id))
    }

    /// `git.log` — one page of commit history for the scoped checkout. Task
    /// scope additionally marks each commit as ahead of (unreachable from)
    /// the base branch.
    fn git_log(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, false, |scope, params| {
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(30)
                .clamp(1, 200) as usize;
            let skip = params.get("skip").and_then(Value::as_u64).unwrap_or(0) as usize;
            crate::gitgui::log_page(&scope.repo_path, scope.mark_ahead_of(), limit, skip)
        })
    }

    /// `git.show` — one commit's metadata, stat, and capped patch. The hash
    /// param is a strict object-id prefix, never a general revspec.
    fn git_show(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, false, |scope, params| {
            let hash = require_str(params, "hash")?;
            crate::gitgui::show_commit(&scope.repo_path, &hash)
        })
    }

    /// `git.status` — branch/head, per-file staging tri-state, content keys and
    /// line counts for the scoped checkout. No patch: a file's body comes from
    /// `git.diff`, per path.
    ///
    /// `if_status_key` is what the browser is already painting; when it still
    /// names the working tree the answer is `{"unchanged": true}` and its key.
    fn git_status(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, false, |scope, params| {
            let if_status_key = params.get("if_status_key").and_then(Value::as_str);
            crate::gitgui::status_payload_unless(&scope.repo_path, if_status_key)
        })
    }

    /// `git.diff` — the uncommitted patch of the named paths, one entry each,
    /// keyed by content so a browser caches a body until that file moves.
    fn git_diff(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, false, |scope, params| {
            let paths = require_path_list(params)?;
            crate::gitgui::file_patches(&scope.repo_path, &paths)
        })
    }

    /// `git.stage` — stage the given repo-relative paths, answering with the
    /// fresh status payload so the UI repaints without waiting for a poll.
    fn git_stage(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, false, |scope, params| {
            let paths = require_path_list(params)?;
            crate::gitgui::stage_paths(&scope.repo_path, &paths)?;
            crate::gitgui::status_payload(&scope.repo_path)
        })
    }

    /// `git.unstage` — the inverse of `git.stage`, same response shape.
    fn git_unstage(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, false, |scope, params| {
            let paths = require_path_list(params)?;
            crate::gitgui::unstage_paths(&scope.repo_path, &paths)?;
            crate::gitgui::status_payload(&scope.repo_path)
        })
    }

    /// `git.commit` — commit exactly what is staged with the user's message.
    /// On a task scope the commit changes the tree the board summarizes, so
    /// the cached diffstat is dropped and the task's updated-at stamped; the
    /// task record itself is untouched (no lifecycle transition — a commit
    /// never advances a task past any gate).
    fn git_commit(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, params| {
            let message = require_str(params, "message")?;
            let commit = crate::gitgui::commit_staged(&scope.repo_path, &message)?;
            let status = crate::gitgui::status_payload(&scope.repo_path)?;
            Ok(json!({
                "hash": commit["hash"],
                "short": commit["short"],
                "subject": commit["subject"],
                "status": status,
            }))
        })
    }

    /// Drop the cached board summaries a scoped git mutation just invalidated —
    /// the task's diffstat + updated-at for task scope, the project's primary
    /// uncommitted-changes summary for project scope, the external-worktree
    /// scan for worktree scope — so the next `task.list` / project poll
    /// recomputes instead of serving a stale summary for up to its TTL.
    fn invalidate_git_scope_caches(&mut self, scope: &GitScope) {
        if let Some(run) = &scope.run {
            let run_id = run.run_id.clone();
            self.invalidate_run_stat(&run_id);
            self.entity_updated_at.insert(run_id, now_rfc3339());
        }
        if let Some(project_id) = scope.project_id.clone() {
            self.invalidate_primary_summary(&project_id);
        }
        if let Some(worktree) = &scope.worktree {
            let project_id = worktree.project_id.clone();
            self.rescan_external_worktrees(&project_id);
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
        let project = self.project_for(&project_id)?;
        let base_branch = project.base_branch.clone();
        let primary_repo_path = project.repo_path.clone();
        let (repo_path, external_worktree) = match params.get("worktree_id").and_then(Value::as_str)
        {
            Some(worktree_id) => (
                self.resolve_external_worktree(&project_id, worktree_id)?
                    .path,
                true,
            ),
            None => (primary_repo_path, false),
        };
        Ok(BranchScope {
            project_id,
            repo_path,
            base_branch,
            external_worktree,
        })
    }

    /// The checkouts of a project the mutex can name without touching the
    /// disk: the external scan the board already holds, every live run's
    /// checkout, and the primary. Which branch each one holds is git's to
    /// answer, and [`ProjectCheckouts::holders`] asks it.
    ///
    /// Only owned inputs are captured here. Every caller asks `holders` in
    /// its off-lock phase, including one-shot creates and dispatches.
    fn project_checkouts(&self, project_id: &str) -> Result<ProjectCheckouts, String> {
        Ok(ProjectCheckouts {
            project: self.orch_for(project_id)?.clone(),
            primary_repo_path: self.repo_path_for(project_id)?,
            base_branch: self.base_for(project_id)?,
            excluded: self.bound_worktree_paths(),
            run_checkouts: self
                .live_runs_of(project_id)
                .map(|(id, run)| (id.clone(), run.worktree.clone()))
                .collect(),
        })
    }

    /// Recheck the records the off-lock holder reading was based on.
    fn validate_checkout_snapshot(
        &self,
        project_id: &str,
        snapshot: &ProjectCheckouts,
    ) -> Result<(), String> {
        let project = self.project_for(project_id)?;
        let runs = self.live_runs_of(project_id).collect::<Vec<_>>();
        let unchanged = project.repo_path == snapshot.primary_repo_path
            && project.base_branch == snapshot.base_branch
            && runs.len() == snapshot.run_checkouts.len()
            && snapshot.run_checkouts.iter().all(|(id, checkout)| {
                runs.iter()
                    .any(|(current_id, active)| *current_id == id && active.worktree == *checkout)
            });
        if unchanged {
            Ok(())
        } else {
            Err("the project's branch holders changed while Git ran; retry the action".to_string())
        }
    }

    /// `git.fetch` — `git fetch --prune`, then the fresh status payload.
    fn git_fetch(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, _| {
            crate::gitgui::fetch(&scope.repo_path)?;
            crate::gitgui::status_payload(&scope.repo_path)
        })
    }

    /// `git.pull` — integrate the upstream in the requested mode (ff/merge/
    /// rebase), then the fresh status payload. Git's own errors pass through.
    fn git_pull(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, params| {
            let mode = params.get("mode").and_then(Value::as_str).unwrap_or("ff");
            crate::gitgui::pull(&scope.repo_path, mode)?;
            crate::gitgui::status_payload(&scope.repo_path)
        })
    }

    /// `git.push` — push the current branch (setting the upstream on first
    /// push), then the fresh status payload. `force` uses `--force-with-lease`.
    fn git_push(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, params| {
            let force = params
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            crate::gitgui::push(&scope.repo_path, force)?;
            crate::gitgui::status_payload(&scope.repo_path)
        })
    }

    /// `git.branches` — every branch the project can offer, once each (the same
    /// list whichever checkout is scoped: branches are the repository's, not
    /// one checkout's), each stamped with the checkout that holds it.
    fn git_branches(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_branch_listing(params, false, |scope, _| stamped_branch_list(scope))
    }

    /// `git.checkout` — switch the scoped checkout to (or create) a branch,
    /// then the fresh status payload.
    fn git_checkout(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_branch_git(params, true, |scope, params| {
            let branch = require_str(params, "branch")?;
            let create = params
                .get("create")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            crate::gitgui::checkout(&scope.repo_path, &branch, create)?;
            crate::gitgui::status_payload(&scope.repo_path)
        })
    }

    /// `git.branch_delete` — delete a local branch, then the fresh branch list.
    fn git_branch_delete(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_branch_listing(params, false, |scope, params| {
            let branch = require_str(params, "branch")?;
            let force = params
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            crate::gitgui::branch_delete(&scope.checkout.repo_path, &branch, force)?;
            stamped_branch_list(scope)
        })
    }

    /// `git.stash` — `git stash push -u`, then the fresh status payload.
    fn git_stash(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, _| {
            crate::gitgui::stash_push(&scope.repo_path)?;
            crate::gitgui::status_payload(&scope.repo_path)
        })
    }

    /// `git.stash_pop` — `git stash pop`, then the fresh status payload.
    fn git_stash_pop(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, _| {
            crate::gitgui::stash_pop(&scope.repo_path)?;
            crate::gitgui::status_payload(&scope.repo_path)
        })
    }

    /// `git.discard` (**destructive**) — revert the given paths to HEAD
    /// (untracked ones are deleted), then the fresh status payload.
    fn git_discard(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, params| {
            let paths = require_path_list(params)?;
            crate::gitgui::discard_paths(&scope.repo_path, &paths)?;
            crate::gitgui::status_payload(&scope.repo_path)
        })
    }

    /// `git.merge_abort` — abort whatever operation is in progress (merge,
    /// rebase, cherry-pick, revert, or bisect), then the fresh status payload.
    fn git_merge_abort(&mut self, params: &Value) -> Result<Value, String> {
        self.defer_git(params, true, |scope, _| {
            crate::gitgui::merge_abort(&scope.repo_path)?;
            crate::gitgui::status_payload(&scope.repo_path)
        })
    }

    /// Read-only browse of one external worktree's dirty diff (spec §5.4) —
    /// never adopts.
    /// Mint a bare worktree — no run, no agent, no session. It is the
    /// "somewhere to work" affordance beside issue creation: the human opens a
    /// terminal or an agent tab in it, and it stays unbound (the scan reports
    /// it like any hand-made worktree) until a mutating action adopts it.
    ///
    /// The two slots mean opposite things, and exactly one is given. `branch`
    /// names a branch that already exists — here or on a remote — and Build
    /// borrows it a directory, cutting nothing. `name` is words to cut a new
    /// branch after, and no branch of that spelling is consulted.
    fn worktree_create(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let (title, existing_branch, branch) = match (
            params.get("branch").and_then(Value::as_str),
            params.get("name").and_then(Value::as_str),
        ) {
            (Some(branch), None) => {
                if !crate::worktree::is_ref_name(branch) {
                    return Err(format!("{branch:?} is not a branch name"));
                }
                (branch.to_string(), Some(branch.to_string()), branch.to_string())
            }
            (None, Some(name)) => {
                if !name.chars().any(|c| c.is_ascii_alphanumeric()) {
                    return Err("a worktree name needs at least one letter or number".to_string());
                }
                (name.to_string(), None, crate::worktree::branch_name_for(&crate::worktree::slugify(name)))
            }
            _ => return Err("worktree.create takes exactly one of branch (a branch that already exists) and name (words to cut a new branch after)".to_string()),
        };
        let slug = crate::worktree::slugify(&title);
        let placeholder_id = if existing_branch.is_some() {
            format!("pending-worktree-{}", uuid::Uuid::new_v4())
        } else {
            self.planned_checkout_id(&project_id, &slug)?
        };
        let checkouts = self.project_checkouts(&project_id)?;
        let mutation = CreateWorktree {
            project: self.orch_for(&project_id)?.clone(),
            base_branch: self.base_for(&project_id)?,
            project_id: project_id.clone(),
            slug,
            existing_branch,
            checkouts,
            placeholder_id: placeholder_id.clone(),
            resolved: self.resolved_isolation(&project_id),
        };
        let row = PendingRow::creating(placeholder_id, Some(project_id), title)
            .on_branch(branch)
            .isolated_as(mutation.resolved.isolation);
        self.defer_lifecycle(row, Box::new(mutation))
    }

    /// Finish an external worktree selected only by server-resolved ids.
    ///
    /// Two halves. HERE, under the app mutex: resolve the project, settle the
    /// idempotent replays out of memory, and claim the checkout. Then
    /// [`WorktreeFinishJob::run`] with the mutex released: the forced rescan
    /// (which is both stale-id protection and the execution-time status
    /// recheck), the checkpoint, and the destructive git. Client paths are
    /// ignored and never become an authority in either half.
    fn worktree_finish(&mut self, params: &Value) -> Result<Value, String> {
        match self.plan_worktree_finish(params)? {
            PlannedFinish::Settled(value) => Ok(value),
            PlannedFinish::Deferred(job) => {
                let epilogue = job.epilogue(FinishKind::Worktree);
                Ok(self.defer_finish(job, epilogue))
            }
        }
    }

    /// The lock-held half of every finish verb: what the app mutex decides
    /// before any disk is touched.
    fn plan_worktree_finish(&mut self, params: &Value) -> Result<PlannedFinish, String> {
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
        let mut resume = None;
        if let Some(record) = self.archived_worktrees.get(&worktree_id).cloned() {
            if record.project_path == canonical_project_path {
                if record.action != action {
                    return Err(format!(
                        "worktree.finish already started with action {:?}",
                        record.action
                    ));
                }
                if record.status == WorktreeFinishStatus::Archived {
                    return Ok(PlannedFinish::Settled(archived_worktree_json(&record)));
                }
                resume = Some(record);
            }
        }

        // The claim is the last thing taken and the first thing the epilogue
        // gives back: past this point the checkout belongs to this finish until
        // its git work returns.
        let store = self.require_store()?.clone();
        let excluded = self.bound_worktree_paths();
        if !self.finishing_worktrees.insert(worktree_id.clone()) {
            return Err(format!(
                "worktree {worktree_id} is already finishing — wait for that to complete"
            ));
        }
        Ok(PlannedFinish::Deferred(Box::new(WorktreeFinishJob {
            worktrees: self.orch_for(&project_id)?.worktrees().clone(),
            project_id,
            base_branch,
            worktree_id,
            action,
            excluded,
            resume,
            store,
            #[cfg(test)]
            gate: self.off_lock_gate.clone(),
        })))
    }

    /// Hand a claimed finish to the drain. The `Ok` returned here is the
    /// placeholder [`AppState::deferred_work`] documents: whichever drain runs
    /// the job replaces it with what [`AppState::apply_finish`] answers.
    fn defer_finish(&mut self, job: Box<WorktreeFinishJob>, epilogue: FinishEpilogue) -> Value {
        self.deferred_work = Some(DeferredWork::Finish { job, epilogue });
        Value::Null
    }

    /// Put a placeholder on the board for a verb that is about to run git, and
    /// refuse a second verb claiming the same thing while it stands. The row is
    /// visible to every reader from this acquisition until the epilogue
    /// replaces it with the real record.
    ///
    /// Reached only through [`AppState::defer_lifecycle`], which is what makes
    /// the row's release certain: only a job can release one, so a row is never
    /// reserved without one.
    fn reserve_row(&mut self, row: PendingRow) -> Result<Arc<PendingRow>, String> {
        if let Some(held) = self.row_claiming(&row) {
            return Err(format!(
                "{:?} is already {} — wait for that to finish",
                held.title,
                held.state.as_str()
            ));
        }
        let row = Arc::new(row);
        self.pending_rows.push(Arc::clone(&row));
        self.note_board_changed();
        Ok(row)
    }

    /// The standing row that already claims what `row` would: the same record,
    /// branch, checkout or issue in the same project. One rule for what two
    /// lifecycle verbs collide on.
    fn row_claiming(&self, row: &PendingRow) -> Option<&Arc<PendingRow>> {
        self.pending_rows.iter().find(|held| {
            held.project_id == row.project_id
                && (held.entity_id == row.entity_id
                    || (held.branch.is_some() && held.branch == row.branch)
                    || (held.checkout_id.is_some() && held.checkout_id == row.checkout_id)
                    || (held.implements.is_some() && held.implements == row.implements))
        })
    }

    /// Retire a placeholder, whichever way its verb went. The real record — or
    /// nothing at all, on a failure — stands where it was.
    fn release_row(&mut self, entity_id: &str) {
        self.pending_rows.retain(|row| row.entity_id != entity_id);
        self.note_board_changed();
    }

    /// Whether a lifecycle verb is holding this entity's checkout open right
    /// now. Nothing may touch that directory while its git runs — see
    /// [`AppState::take_pending_turns`], which is what would.
    fn checkout_is_in_flight(&self, entity_id: &str) -> bool {
        self.pending_rows
            .iter()
            .any(|row| row.entity_id == entity_id)
    }

    /// The lifecycle verbs in flight, as rows the board shows beside the
    /// checkouts that already exist.
    ///
    /// Only the rows that stand for a card: a project verb reserves the folder
    /// it is reaching for, and a folder is not something the board lists, so
    /// nothing about it belongs in a list of cards.
    fn pending_rows_json(&self) -> Vec<Value> {
        self.pending_rows
            .iter()
            .filter_map(|row| {
                let project_id = row.project_id.as_ref()?;
                Some(json!({
                    "entity_id": row.entity_id,
                    "project_id": project_id,
                    "project": self.project_name_by_id(project_id),
                    "title": row.title,
                    "branch": row.branch,
                    "state": row.state.as_str(),
                    "checkout_id": row.checkout_id,
                    // The project's own checkout is listed under no id of its
                    // own, so a row standing on it is matched by this instead.
                    "primary": row.primary,
                    "implements": row.implements,
                    // How the checkout being made is isolated, said the way a
                    // settled card says it. A verb that makes none says
                    // nothing: what is already on disk describes itself.
                    "isolation": row.isolation.map(crate::isolation::Isolation::wire),
                    // How long this row has stood. A row older than a scan
                    // interval reads as stuck rather than as work in flight.
                    "pending_seconds": row.since.elapsed().as_secs(),
                }))
            })
            .collect()
    }

    /// The id the board carries for a checkout that does not exist yet: the id
    /// its path will hash to once `git worktree add` has made it.
    /// [`WorktreeManager::create`] suffixes a slug something is already using,
    /// which the decide phase cannot know, so this is what the epilogue settles
    /// under unless it had to.
    ///
    /// [`WorktreeManager::create`]: crate::worktree::WorktreeManager::create
    fn planned_checkout_id(&self, project_id: &str, slug: &str) -> Result<String, String> {
        let planned = self.orch_for(project_id)?.planned_checkout_path(slug);
        Ok(crate::worktree::external_worktree_id(
            &crate::worktree::canonical_planned_path(&planned),
        ))
    }

    /// Reserve one lifecycle verb's row and hand its git to the drain, in one
    /// call. Reserving and deferring are the same step so that nothing fallible
    /// can run between them: a row put on the board with no job behind it would
    /// stand there forever, refusing every later verb that claims its name.
    ///
    /// The `Ok` returned here is the placeholder [`AppState::deferred_work`]
    /// documents: whichever drain runs the job replaces it with what
    /// [`AppState::apply_lifecycle`] answers.
    fn defer_lifecycle(
        &mut self,
        row: PendingRow,
        mutation: Box<dyn WorktreeMutation>,
    ) -> Result<Value, String> {
        let job = self.reserve_lifecycle(row, mutation)?;
        Ok(self.defer_job(job))
    }

    /// The same reservation, handed back rather than deferred — for a caller
    /// that has to decide where the git runs. Consume it with
    /// [`AppState::defer_job`] or [`AppState::run_lifecycle_here`]: a job
    /// dropped instead leaves its row on the board forever.
    fn reserve_lifecycle(
        &mut self,
        row: PendingRow,
        mutation: Box<dyn WorktreeMutation>,
    ) -> Result<WorktreeLifecycleJob, String> {
        let row = self.reserve_row(row)?;
        Ok(self.lifecycle_job(row, mutation))
    }

    /// Reserve a verb's row, take out of the registry whatever it has to hold
    /// while its git runs, and hand that git to the drain — one call, so the
    /// row is claimed before anything is torn down and nothing fallible runs
    /// between the row and the job that releases it.
    ///
    /// `take` runs with the row already on the board and cannot refuse: every
    /// refusal a verb has belongs before this call.
    fn defer_lifecycle_holding(
        &mut self,
        row: PendingRow,
        take: impl FnOnce(&mut AppState) -> Box<dyn WorktreeMutation>,
    ) -> Result<Value, String> {
        let row = self.reserve_row(row)?;
        let mutation = take(self);
        let job = self.lifecycle_job(row, mutation);
        Ok(self.defer_job(job))
    }

    /// One reserved row's job, held open for the tests in one place so no verb
    /// has to remember to offer them a seam.
    fn lifecycle_job(
        &self,
        row: Arc<PendingRow>,
        mutation: Box<dyn WorktreeMutation>,
    ) -> WorktreeLifecycleJob {
        let job = WorktreeLifecycleJob::reserving(row, mutation);
        #[cfg(test)]
        let job = {
            let mut job = job;
            job.hold_at(self.off_lock_gate.clone());
            job
        };
        job
    }

    /// Hand one reserved job to the drain, which runs it with the app mutex
    /// released. The `Value` is the placeholder [`AppState::deferred_work`]
    /// documents: whichever drain runs the job replaces it with what
    /// [`AppState::apply_lifecycle`] answers.
    fn defer_job(&mut self, job: WorktreeLifecycleJob) -> Value {
        self.deferred_work = Some(DeferredWork::Lifecycle(Box::new(job)));
        Value::Null
    }

    /// Run one reserved job right here instead, with no mutex to release —
    /// boot and an agent's own report have no frame to hand git to, and ran it
    /// under the app mutex before this split too.
    fn run_lifecycle_here(&mut self, job: WorktreeLifecycleJob) -> Result<Value, String> {
        let outcome = job.run();
        self.apply_lifecycle(outcome)
    }

    /// Write back what one lifecycle verb's git did: retire the placeholder,
    /// amend the project's checkout list with what moved, and then let the
    /// verb's own epilogue settle the record. A failure rolls the reservation
    /// back instead, and answers with the error the git gave.
    ///
    /// An epilogue that fails is the harder half: the git already ran, so what
    /// it made is on disk whatever the records say. The amendment is re-applied
    /// over whatever the epilogue got through before it failed, which puts the
    /// checkout back on the board as the unowned card it is — invisible until
    /// the next full rescan is how a minted checkout gets lost.
    fn apply_lifecycle(&mut self, outcome: LifecycleOutcome) -> Result<Value, String> {
        let LifecycleOutcome {
            reservation,
            result,
        } = outcome;
        let project_id = reservation.row().project_id.clone();
        self.release_row(&reservation.row().entity_id);
        let Performed { change, epilogue } = match result {
            Ok(performed) => performed,
            Err(error) => {
                reservation.roll_back(self);
                return Err(error);
            }
        };
        self.amend_checkouts(project_id.as_deref(), &change);
        epilogue.apply(self).inspect_err(|_| {
            self.amend_checkouts(project_id.as_deref(), &change);
            reservation.roll_back(self);
        })
    }

    /// Move what one mutation did to the checkouts on disk into the list the
    /// board reads: what appeared, what went, and — when the mutation touched a
    /// checkout it could not describe — the rescan that finds it.
    ///
    /// A project verb has no project to amend and moves no checkout, so there
    /// is nothing here for it to do.
    fn amend_checkouts(&mut self, project_id: Option<&str>, change: &WorktreeChange) {
        let Some(project_id) = project_id else {
            return;
        };
        for worktree in &change.appeared {
            self.note_worktree_appeared(project_id, worktree.clone());
        }
        for path in &change.gone {
            self.note_worktree_gone(project_id, path);
        }
        if change.rescan {
            self.rescan_external_worktrees(project_id);
        }
    }

    /// Write back what the lock-free git work found: release the claim, take
    /// the scan it paid for and the record it left, and then run whatever
    /// bookkeeping the verb that deferred it still owes.
    fn apply_finish(
        &mut self,
        epilogue: FinishEpilogue,
        outcome: WorktreeFinishOutcome,
    ) -> Result<Value, String> {
        self.finishing_worktrees.remove(&epilogue.worktree_id);
        // The scan the preflight paid for, whichever way the preflight went.
        // `store_diff_entry` drops it if the project has since gone.
        if let Some(worktrees) = outcome.scan {
            self.store_diff_entry(DiffCacheEntry::ExternalScan {
                project_id: epilogue.project_id.clone(),
                worktrees,
            });
        }
        // Memory mirrors the store: Archived after a completed finish, Pending
        // after a failed destructive step (which is the resume point).
        let finished_path = outcome
            .record
            .as_ref()
            .map(|record| std::path::PathBuf::from(&record.worktree_path));
        if let Some(record) = outcome.record {
            self.archived_worktrees
                .insert(record.worktree_id.clone(), record);
        }
        let archived = outcome.result.inspect(|_| {
            self.reap_orphaned_terminals();
            // The checkout is archived, so it leaves the scan the preflight
            // above just stored — which was taken while it still stood.
            if let Some(path) = &finished_path {
                self.note_worktree_gone(&epilogue.project_id, path);
            }
            self.persist_attention();
        });
        match epilogue.kind {
            FinishKind::Worktree => archived,
            FinishKind::Run(run) => self.apply_run_finish(run, archived),
            FinishKind::Branch(branch) => self.apply_branch_finish(branch, archived),
        }
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
        // A named agent reads one bubble through; no agent reads the whole
        // entry, which is what opening the entry means.
        let named = named_agent_id(params)?;
        let agent_id = if named.is_some() || params.get("conversation_id").is_some() {
            Some(
                self.resolve_conversation_params(&entity_id, params)?
                    .agent_id,
            )
        } else {
            None
        };
        let report = ReadReport {
            window_floor: params.get("read_from_sequence").and_then(Value::as_u64),
            through: params.get("read_through_sequence").and_then(Value::as_u64),
        };
        self.see_attention(&entity_id, agent_id.as_deref(), report);
        Ok(json!({ "ok": true }))
    }

    /// `entity.mute` — the human telling one entry to stop asking, or to start
    /// again.
    ///
    /// A muted entry keeps its place in the inbox with live status: it pushes
    /// nothing and badges nothing, and that is all mute does. The read cursors
    /// are untouched, so unmuting shows exactly what was waiting.
    fn entity_mute(&mut self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
        let muted = params
            .get("muted")
            .and_then(Value::as_bool)
            .ok_or("entity.mute: muted must be true or false")?;
        if !self.entity_takes_attention(&entity_id) {
            return Err(format!("entity.mute: unknown entity {entity_id}"));
        }
        self.attention.entry(entity_id.clone()).or_default().muted = muted;
        self.persist_attention();
        Ok(json!({ "entity_id": entity_id, "muted": muted }))
    }

    /// `entity.dismiss` — the human clearing one row out of the inbox until the
    /// work speaks again. Every row can be cleared, including the ones nothing
    /// stands behind.
    ///
    /// A row with an entity behind it is named by that entity's id, and the
    /// line is drawn at the end of every owned conversation: the row stays out
    /// of the list until a user or agent sends another message.
    ///
    /// A row with no entity — a project's primary checkout, a branch checked
    /// out somewhere Build never cut — is named by what it IS:
    /// `{ project_id, branch }`, or `{ project_id, primary: true }` for the
    /// checkout that is the repository. With no conversation, unrelated git
    /// changes cannot revive it; adoption gives it a conversation and identity.
    ///
    /// There is no un-dismiss verb because there is nothing to undo: the next
    /// thing the work says brings the row back by itself, which is the whole
    /// feature.
    ///
    /// Clearing something out of the way is not picking it up, so this is not
    /// an interaction: it moves no anchor and no resume point. It touches no
    /// read cursor (what was waiting is still waiting), no mute (silencing is
    /// mute's job), and no push.
    fn entity_dismiss(&mut self, params: &Value) -> Result<Value, String> {
        if params.get("entity_id").is_none() {
            let row = self.dismissable_row(params)?;
            self.clear_row(&row);
            return Ok(json!({
                "project_id": row.project_id,
                "branch": row.branch,
                "primary": row.primary,
                "dismissed": true,
            }));
        }
        let entity_id = require_str(params, "entity_id")?;
        if !self.entity_takes_attention(&entity_id) {
            return Err(format!("entity.dismiss: unknown entity {entity_id}"));
        }
        // A checkout Build never cut carries an id but no conversation: it is
        // one of the entity-less rows above wearing the id the feed ships, and
        // it is cleared as that row so both ways of naming it land in one
        // place.
        if let Some(row) = self.checkout_row(&entity_id) {
            self.clear_row(&row);
            return Ok(json!({ "entity_id": entity_id, "dismissed": true }));
        }
        // Every agent on the row gets its own line, drawn where its own
        // conversation stands right now — clearing the row IS reading it, and
        // the client relies on that. One line could never speak for the rest:
        // each agent numbers its conversation from 1, so a sequence taken off
        // the first agent says nothing about where the second one has got to.
        let lines = self.dismissal_lines(&entity_id);
        let attention = self.attention.entry(entity_id.clone()).or_default();
        attention.dismiss_messages();
        for (agent_id, last_attention_sequence) in lines {
            attention.dismiss_agent_through(&agent_id, last_attention_sequence);
        }
        self.persist_attention();
        Ok(json!({ "entity_id": entity_id, "dismissed": true }))
    }

    /// Write one entity-less row's dismissal: cleared at the commit it is
    /// sitting on, which is what brings it back.
    fn clear_row(&mut self, row: &EntitylessRow) {
        self.attention
            .entry(row.key.clone())
            .or_default()
            .dismiss_at_head(row.head.as_deref());
        self.persist_attention();
    }

    /// The entity-less row `{ project_id, branch | primary }` names.
    ///
    /// A branch is resolved against the feed's own sources, so a name that is
    /// not a row anyone can clear is refused rather than written as a
    /// dismissal nothing will ever read.
    fn dismissable_row(&mut self, params: &Value) -> Result<EntitylessRow, String> {
        let project_id = params
            .get("project_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                "entity.dismiss: name a row — an entity_id, or a project_id with a branch or \
                 primary: true"
                    .to_string()
            })?;
        if !self.projects.iter().any(|p| p.id == project_id) {
            return Err(format!("entity.dismiss: unknown project {project_id}"));
        }
        if params
            .get("primary")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return self.primary_row(&project_id);
        }
        let branch = params
            .get("branch")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                format!(
                    "entity.dismiss: {project_id} is a project, not a row — name a branch, or \
                     primary: true for its checkout"
                )
            })?;
        if let Some(run_id) = self.unarchived_run_on_branch(&project_id, &branch) {
            return Err(format!(
                "entity.dismiss: {branch} is run {run_id} — clear it by entity_id, so the line \
                 is drawn in its conversation"
            ));
        }
        let refusal = match self.find_checkout(
            &project_id,
            &format!("entity.dismiss: {project_id} has no row for {branch}"),
            |checkout| checkout.branch.as_deref() == Some(branch.as_str()),
        ) {
            Ok(checkout) => {
                return Ok(EntitylessRow {
                    key: crate::attention::branch_row_key(&project_id, &branch),
                    head: Some(checkout.head_sha),
                    project_id,
                    branch: Some(branch),
                    primary: false,
                })
            }
            Err(refusal) => refusal,
        };
        // The one row left that a branch name can mean: the project's own
        // checkout, named the way it appears on the feed rather than by the
        // `primary` flag beside it.
        let primary = self.primary_row(&project_id)?;
        if primary.branch.as_deref() == Some(branch.as_str()) {
            return Ok(primary);
        }
        Err(refusal)
    }

    /// A project's primary-checkout row, read off the same summary the feed
    /// builds that row from — the row and its dismissal have to agree about
    /// which commit the checkout is on.
    ///
    /// Refused until that summary has landed: a dismissal written against no
    /// head is one the walk's own first result revokes, so the client is told
    /// to ask again rather than answered with a click that did nothing.
    fn primary_row(&mut self, project_id: &str) -> Result<EntitylessRow, String> {
        let summary = self.primary_summary(project_id).ok_or_else(|| {
            format!(
                "entity.dismiss: the primary checkout of {project_id} has not been read yet, and \
                 the walk now running settles it"
            )
        })?;
        Ok(EntitylessRow {
            key: crate::attention::primary_row_key(project_id),
            head: summary["head_sha"].as_str().map(str::to_string),
            project_id: project_id.to_string(),
            branch: summary["branch"].as_str().map(str::to_string),
            primary: true,
        })
    }

    /// The entity-less row an external worktree's id names, or `None` when the
    /// id is a run's or an issue's. A checkout on a branch IS that branch's
    /// row; one with no branch is only ever itself.
    fn checkout_row(&mut self, worktree_id: &str) -> Option<EntitylessRow> {
        let project_ids: Vec<String> = self.projects.iter().map(|p| p.id.clone()).collect();
        for project_id in project_ids {
            let Some(checkout) = self
                .external_worktrees(&project_id)
                .worktrees
                .into_iter()
                .find(|worktree| worktree.id == worktree_id)
            else {
                continue;
            };
            return Some(EntitylessRow {
                key: match &checkout.branch {
                    Some(branch) => crate::attention::branch_row_key(&project_id, branch),
                    None => worktree_id.to_string(),
                },
                head: Some(checkout.head_sha),
                project_id,
                branch: checkout.branch,
                primary: false,
            });
        }
        None
    }

    /// The run whose ROW holds a branch in this project, if one does — where
    /// that row's dismissal belongs, because a run's line is drawn in its
    /// conversation.
    ///
    /// Wider than [`run_on_branch`](Self::run_on_branch): a merged or abandoned
    /// run keeps its row until it is archived, and a row on the feed is a row
    /// the human can clear.
    fn unarchived_run_on_branch(&self, project_id: &str, branch: &str) -> Option<String> {
        self.runs
            .iter()
            .find(|(run_id, active)| {
                active.run.state != RunState::Archived
                    && active.worktree.branch() == branch
                    && self.entity_project.get(*run_id).map(String::as_str) == Some(project_id)
            })
            .map(|(run_id, _)| run_id.clone())
    }

    /// Forget what was cleared against the entity-less rows a checkout has just
    /// stopped being. These records hold a dismissal and nothing else, so
    /// dropping the record IS forgetting the dismissal.
    fn take_row_dismissal(
        &mut self,
        project_id: &str,
        branch: Option<&str>,
        primary: bool,
    ) -> (bool, Option<String>) {
        let keys: Vec<String> = branch
            .map(|branch| crate::attention::branch_row_key(project_id, branch))
            .into_iter()
            .chain(primary.then(|| crate::attention::primary_row_key(project_id)))
            .collect();
        let removed: Vec<crate::attention::Attention> = keys
            .iter()
            .filter_map(|key| self.attention.remove(key))
            .collect();
        let dismissed = removed
            .iter()
            .any(|attention| attention.is_dismissed_at_head(None));
        let first_observed_at = removed
            .iter()
            .filter_map(|attention| attention.first_observed_at.clone())
            .min();
        if !removed.is_empty() {
            self.persist_attention();
        }
        (dismissed, first_observed_at)
    }

    /// Whether an entity-less row has been cleared out of the inbox: the human
    /// dismissed it, and it is still sitting on the commit they left it on.
    fn row_is_dismissed(&self, key: &str, head: Option<&str>) -> bool {
        self.attention
            .get(key)
            .is_some_and(|attention| attention.is_dismissed_at_head(head))
    }

    /// `triage.override` — the reviewer disagreed with how a hunk was
    /// classified, and says so once, in the three places it has to land.
    ///
    /// On the run, against the hunk: that is what the review surface renders,
    /// and it is the reviewer's level from then on. In the agent's
    /// conversation, as status: a collapse decision is only as honest as its
    /// rationale, so a rationale the reviewer rejected has to be visible to the
    /// one that wrote it — and nothing is being asked of anyone, because the
    /// reviewer has already done what they wanted. And in the project's
    /// `.build/review-rules.json`, generalized to a pattern and counted, which
    /// is the durable half: runs end, and this is what outlives them.
    ///
    /// Everything fallible happens before anything is written. A disagreement
    /// recorded in two of the three places is worse than one recorded in none.
    fn triage_override(&mut self, params: &Value) -> Result<Value, String> {
        if !self.triage_enabled {
            return Err("triage.override: triage is disabled".to_string());
        }
        let run_id = require_str(params, "run_id")?;
        let hunk_id = require_str(params, "hunk_id")?;
        let direction = crate::run::OverrideDirection::parse(&require_str(params, "direction")?)
            .map_err(|error| format!("triage.override: {error}"))?;
        let note = params
            .get("note")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|note| !note.is_empty())
            .map(str::to_string);

        let (path, rationale) = self.triaged_hunk_context(&run_id, &hunk_id)?;
        let pattern = crate::review_rules::pattern_for_path(&path);
        let checkout = self.primary_checkout_of(&run_id)?;
        let now = now_rfc3339();

        let mut active = self.take_run(&run_id)?;
        let triage = active
            .triage
            .as_mut()
            .expect("triaged_hunk_context already found the pass this hunk belongs to");
        let is_new_disagreement = triage.record_override(crate::run::TriageOverride {
            hunk_id: hunk_id.clone(),
            direction,
            note: note.clone(),
            at: now.clone(),
        });
        // A reviewer toggling the same hunk the same way twice has said one
        // thing, not two, so the project-level count does not move for it.
        let count = if is_new_disagreement {
            let (document, count) = crate::review_rules::merge_override(
                crate::review_rules::read(&checkout)?,
                &pattern,
                direction,
                &now,
            )?;
            crate::review_rules::write(&checkout, &document)?;
            Some(count)
        } else {
            None
        };
        let summary =
            triage_override_summary(direction, &path, rationale.as_deref(), note.as_deref());
        let recorded = self.record_on_run_conversation(&mut active, |conversation| {
            conversation.push_event(
                crate::thread::ThreadEventKind::TriageOverridden,
                Some(summary),
                None,
                None,
                &now,
            );
        });
        let triage = self.triage_json(&active);
        let persisted = self.finish_run_mutation(run_id.clone(), active);
        recorded?;
        persisted?;
        Ok(json!({
            "run_id": run_id,
            "hunk_id": hunk_id,
            "direction": direction.as_str(),
            "path": path,
            "rule": { "pattern": pattern, "direction": direction.as_str(), "count": count },
            "triage": triage,
        }))
    }

    /// What the conversation has to say about a hunk the reviewer disagreed
    /// with: the file it lives in, and the rationale the pass gave for putting
    /// it where it put it.
    ///
    /// The file is read off the diff revision the pass classified — not off the
    /// current diff, and never off the caller: the reviewer is disagreeing with
    /// what they were shown, so the patch that produced the hunk ids they are
    /// pointing at is the only one that can name the file they meant. A hunk
    /// this pass never classified has nothing to disagree with and is refused.
    fn triaged_hunk_context(
        &self,
        run_id: &str,
        hunk_id: &str,
    ) -> Result<(String, Option<String>), String> {
        let active = self
            .runs
            .get(run_id)
            .ok_or_else(|| format!("triage.override: unknown run_id {run_id}"))?;
        let triage = active.triage.as_ref().ok_or_else(|| {
            format!("triage.override: {run_id} has no triage pass to disagree with")
        })?;
        let rationale = triage
            .hunks
            .iter()
            .find(|hunk| hunk.hunk_id == hunk_id)
            .ok_or_else(|| {
                format!("triage.override: the pass on {run_id} did not classify {hunk_id}")
            })?
            .rationale
            .clone();
        let patch = self
            .conversation_thread_for_run(active)
            .into_iter()
            .flat_map(|thread| thread.revisions.iter())
            .rev()
            .find(|revision| {
                revision.artifact == crate::thread::ArtifactKind::Diff
                    && revision.content_hash == triage.based_on
            })
            .and_then(|revision| revision.snapshot.as_deref())
            .ok_or_else(|| {
                format!(
                    "triage.override: the diff revision {} the pass read is no longer on the \
                     conversation",
                    triage.based_on
                )
            })?;
        let path = crate::diff::patch_hunks(patch)
            .into_iter()
            .find(|hunk| hunk.hunk_id == hunk_id)
            .map(|hunk| hunk.path)
            .ok_or_else(|| format!("triage.override: {hunk_id} is not in the revision it names"))?;
        Ok((path, rationale))
    }

    /// The primary checkout of the project an entity belongs to — where a
    /// project-wide artifact like the review rules lives, rather than in
    /// whichever worktree happened to notice it.
    fn primary_checkout_of(&self, entity_id: &str) -> Result<std::path::PathBuf, String> {
        let project_id = self.project_of(entity_id)?;
        self.repo_path_for(&project_id)
    }

    /// Whether `entity_id` names something the attention map keeps a record
    /// for: a run, an issue, or a worktree the scan can still see. Anything
    /// else would be written and pruned in the same breath.
    fn entity_takes_attention(&self, entity_id: &str) -> bool {
        self.runs.contains_key(entity_id)
            || self.plans.contains_key(entity_id)
            || self
                .attention_worktree_ids()
                .iter()
                .any(|id| id == entity_id)
    }

    /// `agent.add` — give a branch another agent, with its own conversation.
    ///
    /// Branches only: an issue carries exactly one agent session, because
    /// implementing an issue is a handoff to a new agent on a branch rather
    /// than a second agent on the issue itself. The agent is a record and a
    /// conversation; no process is spawned until something is said to it.
    ///
    /// The FIRST agent of a branch that had none also seeds the entity's legacy
    /// default. That field is only a creation template after migration: every
    /// existing agent keeps and edits its own settings, including agents on the
    /// same provider.
    fn agent_add(&mut self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
        let creation_id = optional_nonempty_string(params, "creation_id")?.map(str::to_string);
        if creation_id.as_ref().is_some_and(|id| id.len() > 128) {
            return Err("agent.add: creation_id is too long".to_string());
        }
        if self.plans.contains_key(&entity_id) {
            return Err(format!(
                "agent.add: {entity_id} is an issue, and an issue carries exactly one agent \
                 session — implement it to hand the work to a new agent on a branch"
            ));
        }
        if !self.runs.contains_key(&entity_id) {
            return Err(format!("agent.add: unknown entity {entity_id}"));
        }
        // The provider is parsed before anything is touched, so an unrunnable
        // one refuses instead of leaving an agent nothing can start.
        let existing_creation = creation_id.as_deref().and_then(|creation_id| {
            self.entity_agents(&entity_id)
                .ok()?
                .iter()
                .find(|agent| agent.creation_id.as_deref() == Some(creation_id))
                .cloned()
        });
        let retried_choice = existing_creation
            .as_ref()
            .and_then(|agent| agent.creation_choice.clone());
        let choice = if has_agent_choice(params) {
            model_choice_from(params, self.default_harness)?
        } else {
            retried_choice.unwrap_or(self.entity_model_choice(&entity_id)?)
        };
        if let Some(existing) = existing_creation {
            if existing.creation_choice.as_ref() != Some(&choice) {
                return Err(format!(
                    "agent.add: creation_id {} was already used with different agent settings",
                    creation_id
                        .as_deref()
                        .expect("an existing creation has an id")
                ));
            }
            let root = self.entity_agent_root(&entity_id).ok();
            return Ok(json!({
                "entity_id": entity_id,
                "created": false,
                "agent": self.agent_digest(
                    &entity_id,
                    &existing,
                    root.as_deref(),
                    DigestScope::List,
                ),
            }));
        }
        let before_agents = self.runs[&entity_id].agents.clone();
        let before_entity_choice = self.runs[&entity_id].model_choice.clone();
        let mut active = self.take_run(&entity_id)?;
        if active.agents.is_empty() {
            active.model_choice = choice.clone();
        }
        let (added, created) = match creation_id.as_deref() {
            Some(creation_id) => active
                .agents
                .add_idempotent(&entity_id, choice, &now_rfc3339(), creation_id)
                .expect("the creation id and any prior use were validated before taking the run"),
            None => (
                active
                    .agents
                    .add(&entity_id, choice, &now_rfc3339())
                    .clone(),
                true,
            ),
        };
        let persisted = self.finish_run_mutation(entity_id.clone(), active);
        if let Err(error) = persisted {
            let restored = self
                .runs
                .get_mut(&entity_id)
                .expect("the failed finish put the run back");
            restored.agents = before_agents;
            restored.model_choice = before_entity_choice;
            return Err(error);
        }
        self.touch_attention(&entity_id);
        let root = self.entity_agent_root(&entity_id).ok();
        Ok(json!({
            "entity_id": entity_id,
            "created": created,
            "agent": self.agent_digest(&entity_id, &added, root.as_deref(), DigestScope::List),
        }))
    }

    /// `agent.choose` — set the model and reasoning effort one exact agent runs
    /// on. The composer's model menu, and the only verb that persists an
    /// agent-owned choice without spawning anything.
    ///
    /// The provider is not a question here: an agent is locked to the harness
    /// it was created on, and a caller that names one is refused by that
    /// harness's name. A live session is untouched —
    /// the choice is what the NEXT start spends, which is exactly what the
    /// menu offers.
    fn agent_choose(&mut self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
        let requested_agent = named_agent_id(params)?;
        let agent = self
            .entity_agents(&entity_id)?
            .resolve(requested_agent.as_deref())?;
        let agent_id = agent.id.clone();
        let locked = agent.choice.provider;
        if let Some(expected) = optional_nonempty_string(params, "conversation_id")? {
            if expected != agent.conversation_id() {
                return Err(format!(
                    "agent.choose: stale conversation_id {expected}; agent {agent_id} is bound to {}",
                    agent.conversation_id()
                ));
            }
        }
        if let Some(named) = params.get("provider").and_then(Value::as_str) {
            if !named.is_empty() {
                return Err(format!(
                    "agent.choose: the agent is locked to {} — model and effort only",
                    locked.label()
                ));
            }
        }
        let choice = model_choice_from(params, locked)?;
        let expected_revision = params
            .get("expected_choice_revision")
            .map(|value| {
                value.as_u64().ok_or_else(|| {
                    "agent.choose: expected_choice_revision must be an unsigned integer".to_string()
                })
            })
            .transpose()?;
        if let Some(expected) = expected_revision {
            if expected != agent.choice_revision {
                return Err(format!(
                    "agent.choose: stale choice revision {expected}; current revision is {}",
                    agent.choice_revision
                ));
            }
        }
        let choice_revision = self.set_agent_model_choice(&entity_id, &agent_id, choice.clone())?;
        Ok(json!({
            "entity_id": entity_id,
            "agent_id": agent_id,
            "provider": choice.provider,
            "model": choice.model,
            "effort": choice.effort,
            "choice_revision": choice_revision,
        }))
    }

    fn set_agent_model_choice(
        &mut self,
        entity_id: &str,
        agent_id: &str,
        choice: ModelChoice,
    ) -> Result<u64, String> {
        let previous = self
            .entity_agents(entity_id)?
            .resolve(Some(agent_id))?
            .clone();
        let revision = if self.plans.contains_key(entity_id) {
            let mut active = self.take_plan(entity_id)?;
            let revision = active
                .agents
                .resolve_mut(Some(agent_id))
                .expect("the agent was validated before its issue was taken")
                .choose(choice);
            if let Err(error) = self.finish_plan_mutation(entity_id.to_string(), active) {
                *self
                    .plans
                    .get_mut(entity_id)
                    .expect("the failed finish put the issue back")
                    .agents
                    .resolve_mut(Some(agent_id))
                    .expect("the previous agent still belongs to the issue") = previous;
                return Err(error);
            }
            revision
        } else if self.runs.contains_key(entity_id) {
            let mut active = self.take_run(entity_id)?;
            let revision = active
                .agents
                .resolve_mut(Some(agent_id))
                .expect("the agent was validated before its run was taken")
                .choose(choice);
            if let Err(error) = self.finish_run_mutation(entity_id.to_string(), active) {
                *self
                    .runs
                    .get_mut(entity_id)
                    .expect("the failed finish put the run back")
                    .agents
                    .resolve_mut(Some(agent_id))
                    .expect("the previous agent still belongs to the run") = previous;
                return Err(error);
            }
            revision
        } else {
            return Err("unknown id".to_string());
        };
        self.note_entity_changed(entity_id);
        Ok(revision)
    }

    /// `agent.remove` — take an agent back off a branch's rail.
    ///
    /// The mirror of [`agent_add`](Self::agent_add), and it validates the same
    /// way: branches only, because an issue's one agent IS the issue's
    /// conversation — there is nothing to remove there, only an issue to
    /// abandon.
    ///
    /// Any of a branch's agents may go, the primary and the last one included.
    /// A branch with none is a working branch: its chat tab shows the
    /// new-agent view, and the next thing the system has to say to it mints an
    /// agent through [`ensure_primary_agent`](Self::ensure_primary_agent).
    ///
    /// A removed agent's harness must not outlive it. An agent with no roster
    /// entry keeps working in the checkout and reports `done` for an identity
    /// nothing can route to — the same hazard
    /// [`retire_agent_tabs`](Self::retire_agent_tabs) exists for — so its session is
    /// killed and reaped and everything that could reach it goes too.
    fn agent_remove(&mut self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
        let agent_id = require_str(params, "agent_id")?;
        if self.plans.contains_key(&entity_id) {
            return Err(format!(
                "agent.remove: {entity_id} is an issue, and its one agent is the issue's own \
                 conversation — abandon the issue instead"
            ));
        }
        if !self.runs.contains_key(&entity_id) {
            return Err(format!("agent.remove: unknown entity {entity_id}"));
        }
        let root = self.entity_agent_root(&entity_id)?;
        // A harness being spawned right now cannot be killed: the tab it will
        // land in does not exist yet, so the reservation is the only handle on
        // it, and the human can ask again a moment later.
        if self
            .agent_spawns_in_flight
            .contains(&TabKey::agent(&root, &agent_id))
        {
            return Err(format!(
                "agent.remove: {agent_id} is starting a session right now — remove it once the \
                 session is running"
            ));
        }
        let removed_agent_revived_clear = self
            .entity_agents(&entity_id)
            .ok()
            .and_then(|roster| {
                let position = roster.iter().position(|agent| agent.id == agent_id)?;
                let agent = roster.by_id(&agent_id)?;
                let thread = if roster.is_primary(&agent.id) {
                    self.entity_conversation(&entity_id)
                        .unwrap_or(&agent.thread)
                } else {
                    &agent.thread
                };
                let attention = self.attention.get(&entity_id)?;
                let sequence = thread.last_message_sequence();
                let crossed = match attention.dismissed_line_for(&agent.id, position == 0) {
                    Some(line) => sequence > line,
                    None => sequence > 0 && attention.has_message_dismissal(),
                };
                Some(crossed)
            })
            .unwrap_or(false);
        let mut active = self.take_run(&entity_id)?;
        let removed = match active.agents.remove(&agent_id) {
            Ok(removed) => removed,
            Err(refused) => {
                // Nothing was touched, so the run goes back exactly as it came.
                self.runs.insert(entity_id, active);
                return Err(format!("agent.remove: {refused}"));
            }
        };
        let persisted = self.finish_run_mutation(entity_id.clone(), active);
        self.retire_agent(&root, &removed.id);
        if let Some(attention) = self.attention.get_mut(&entity_id) {
            // Nothing prunes cursors by agent, so one left behind here would
            // outlive the daemon it was written in.
            attention.agent_read_sequences.remove(&removed.id);
            if removed_agent_revived_clear {
                attention.invalidate_dismissal();
            }
        }
        self.persist_attention();
        persisted?;
        self.touch_attention(&entity_id);
        Ok(json!({
            "entity_id": entity_id,
            "agent_id": removed.id,
            "agents": self.agent_digests(&entity_id, DigestScope::List),
        }))
    }

    /// Kill, reap and forget ONE agent's session, plus everything else that
    /// could still reach it: the capability its harness authenticates control
    /// frames with, the screen a client is waiting on a first spawn for, and
    /// any turn still queued to be said to it.
    ///
    /// The per-agent twin of [`retire_agent_tabs`](Self::retire_agent_tabs), which
    /// takes every agent in a worktree because its owner is going away.
    #[track_caller]
    fn retire_agent(&mut self, root: &std::path::Path, agent_id: &str) {
        let key = TabKey::agent(&Self::canonical_root(root), agent_id);
        self.retire_tab(&key, "closed");
        if let Some(screen) = self.agent_screens_awaiting_spawn.remove(&key) {
            screen.close("closed");
        }
        self.mcp_session_tokens.remove(agent_id);
        self.pending_agent_turns
            .retain(|turn| turn.agent_id != agent_id);
    }

    /// End an issue's agent session, because the gate that just closed ended
    /// it. An issue agent works in the PRIMARY checkout, which never goes
    /// away, so nothing else would ever stop it: it would keep working there
    /// and report `done` for an issue no longer taking reports. Only this
    /// issue's own agent goes — the checkout's other agents belong to the main
    /// branch and are none of this verb's business.
    #[track_caller]
    fn retire_issue_session(&mut self, session: Option<(std::path::PathBuf, String)>) {
        if let Some((checkout, agent_id)) = session {
            self.retire_agent(&checkout, &agent_id);
        }
    }

    /// `agent.list` — the entity's agents, in rail order.
    fn agent_list(&mut self, params: &Value) -> Result<Value, String> {
        let entity_id = require_str(params, "entity_id")?;
        Ok(json!({
            "entity_id": entity_id,
            "agents": self.agent_digests(&entity_id, DigestScope::List),
        }))
    }

    /// What the rail's bubble strip renders for one entity: one digest per
    /// agent, in rail order.
    fn agent_digests(&self, entity_id: &str, scope: DigestScope) -> Vec<Value> {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return Vec::new();
        };
        let root = self.entity_agent_root(entity_id).ok();
        roster
            .iter()
            .map(|agent| self.agent_digest(entity_id, agent, root.as_deref(), scope))
            .collect()
    }

    /// One bubble: who the agent is, what it runs on, whether it is live, and
    /// how much of its conversation is waiting for the human.
    ///
    /// Unread is counted from the agent's canonical conversation binding.
    fn agent_digest(
        &self,
        entity_id: &str,
        agent: &crate::agent::Agent,
        root: Option<&std::path::Path>,
        scope: DigestScope,
    ) -> Value {
        let thread = self
            .agent_conversation(entity_id, Some(&agent.id))
            .unwrap_or(&agent.thread);
        let unread = self.agent_unread(entity_id, agent, thread);
        let tab = root.map(|root| TabKey::agent(root, &agent.id));
        let tab = tab.as_ref().and_then(|key| self.tabs.get(key));
        let live = tab.is_some_and(|tab| tab.session_is_live());
        let next_start = agent.choice.clone();
        let mut digest = json!({
            "id": agent.id,
            "conversation_id": agent.conversation_id(),
            "ordinal": agent.ordinal,
            "provider": agent.choice.provider,
            "model": next_start.model.clone().unwrap_or_default(),
            "effort": next_start.effort.clone().unwrap_or_default(),
            "active_model": agent
                .active_model
                .clone()
                .or(next_start.model)
                .unwrap_or_default(),
            "state": if live {
                crate::agent::AgentLifecycle::Live.as_str()
            } else {
                agent.state.as_str()
            },
            "unread_count": unread.count,
            "unread_reason": unread.reason,
            // Where the reader got to, so the panel can rule its unread divider
            // and open on the first message they have not seen.
            "read_through_sequence": self.read_cursor(entity_id, &agent.id),
            "working": tab.is_some_and(agent_is_working),
            "working_time": working_time_json(agent.working_since.as_deref()),
            "choice_revision": agent.choice_revision,
            // Whether the rail offers this agent a basement. The live session
            // answers for an agent that is running, since it is the only thing
            // that can; before there is one the PROVIDER answers, because it
            // knows whether its spawn will open a terminal. Same authority either
            // side of the spawn, so the rail never offers a TUI button that the
            // spawn then refuses.
            "has_terminal": match tab {
                Some(tab) => tab.session.terminal().is_some(),
                None => harness_for(agent.choice.provider).has_terminal(),
            },
            // Whether the composer offers "Interrupt & send". Unlike
            // `has_terminal` the PROVIDER cannot answer this one: the capability
            // is announced by the child in its own `init` line rather than
            // decided by the argv, so the same provider answers differently on
            // two versions of the same CLI. No session, no turn to stop.
            "can_interrupt": tab.is_some_and(|tab| tab.session.can_interrupt()),
            // Why the last turn queued for this agent never reached a harness.
            // The client's "starting" state is laid on before there is any
            // session to report, and this is what takes it off when none ever
            // opened — the only word a start that failed ever gets to say.
            "start_error": agent.start_error,
            "created_at": agent.created_at,
        });
        if let Some(surfaces) = digest_surfaces(tab, scope) {
            digest["surfaces"] = surfaces;
        }
        digest
    }

    fn worktree_diff(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let worktree_id = require_str(params, "worktree_id")?;
        let external = self.resolve_external_worktree(&project_id, &worktree_id)?;
        let base_branch = self.base_for(&project_id)?;
        Ok(self.defer_conditional_read(
            ReadSubject::Worktree {
                external: Box::new(external),
                base_branch,
            },
            None,
            params.get("if_diff_key").and_then(Value::as_str),
        ))
    }

    // ---- Store accessor + take/finish plumbing --------------------------------

    /// The durable store, or a clean error. Plans and runs both require one:
    /// plan docs are canonical in the store, and `prepare_run_checkout`
    /// writes/reads through it. Only unit tests that never create a plan/run skip it.
    fn require_store(&self) -> Result<&Store, String> {
        self.store
            .as_ref()
            .ok_or_else(|| "no task store configured".to_string())
    }

    /// Take a plan out for mutation, having told its conversations which
    /// checkout they are about: an Issue's is the checkout of the
    /// implementation working it right now, or the primary checkout its own
    /// agent runs in when nothing is implementing it yet.
    fn take_plan(&mut self, plan_id: &str) -> Result<ActivePlan, String> {
        let implementation_checkout = self
            .current_issue_implementation(plan_id)
            .map(|run| run.worktree.path.clone());
        let mut active = self
            .plans
            .remove(plan_id)
            .ok_or_else(|| "unknown plan_id".to_string())?;
        let checkout = implementation_checkout
            .or_else(|| active.workspace.as_ref().map(|w| w.checkout.clone()));
        if let Some(checkout) = checkout {
            locate_conversations(&mut active.agents, &checkout);
        }
        Ok(active)
    }

    fn take_run(&mut self, run_id: &str) -> Result<ActiveRun, String> {
        let mut active = self
            .runs
            .remove(run_id)
            .ok_or_else(|| "unknown run_id".to_string())?;
        let checkout = active.worktree.path.clone();
        locate_conversations(&mut active.agents, &checkout);
        Ok(active)
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

    /// Write to the conversation a run speaks in — its Issue's when it has one,
    /// its own otherwise — and persist whichever record owns it.
    ///
    /// The Issue's thread is what every surface of a planned run renders, so a
    /// report written anywhere else is invisible: the run reads as busy while
    /// nothing is happening in it.
    fn record_on_run_conversation(
        &mut self,
        active: &mut ActiveRun,
        write: impl FnOnce(&mut crate::thread::Thread),
    ) -> Result<(), String> {
        let Some(primary) = active.agents.primary() else {
            return Ok(());
        };
        let agent_id = primary.id.clone();
        let conversation_id = primary.conversation_id().to_string();
        if conversation_id == agent_id {
            write(&mut active.agents.primary_mut().expect("just resolved").thread);
            return Ok(());
        }
        let owner = self.entity_of_agent(&conversation_id).ok_or_else(|| {
            format!("agent {agent_id} is bound to missing conversation {conversation_id}")
        })?;
        if self.plans.contains_key(&owner) {
            let mut issue = self.take_plan(&owner)?;
            write(&mut issue.agents.resolve_mut(Some(&conversation_id))?.thread);
            return self.finish_plan_mutation(owner, issue);
        }
        Err(format!(
            "run conversation {conversation_id} is not owned by an issue"
        ))
    }

    /// Announce on `active`'s conversation, and on the log, that the checkout
    /// it was just given is not the isolation the settings asked for.
    ///
    /// Says, never fails. A volume that cannot clone is a fact to tell the
    /// human, and by the time it is told the checkout stands, the branch is
    /// cut and the record is written — so a telling that does not land is a
    /// line in the log, never a create undone or a run left off the board.
    /// Every creation site announces through here, so no caller can choose
    /// another policy.
    fn note_isolation_downgrade(&mut self, run_id: &str, active: &mut ActiveRun, reason: &str) {
        let note = announce_isolation_downgrade(reason);
        let written = self.record_on_run_conversation(active, |thread| {
            thread.push_event(
                crate::thread::ThreadEventKind::WorktreeCreated,
                Some(note),
                None,
                None,
                now_rfc3339(),
            );
        });
        if let Err(error) = written {
            eprintln!("{run_id}: the isolation fallback went unrecorded: {error}");
        }
    }

    /// Tell the Issue where its implementation got to.
    ///
    /// The Issue's conversation is the place the human follows work they asked
    /// for, and an implementation is a different conversation entirely — so an
    /// outcome that needs them (done, blocked, failed, merged, abandoned) is
    /// mirrored there as an event naming the implementation it came from.
    /// Progress is not mirrored: the Issue's surfaces already read where the
    /// work got to off the implementation itself.
    ///
    /// The feed's dedup rule keeps this from asking twice: while an
    /// implementation is live the Issue has no row of its own, so a mirrored
    /// outcome makes exactly one entry unread.
    fn mirror_run_outcome_to_issue(
        &mut self,
        run_id: &str,
        issue_id: &str,
        event: crate::thread::ThreadEventKind,
        summary: String,
    ) -> Result<(), String> {
        if !run_outcome_mirrors_to_issue(event) {
            return Ok(());
        }
        let Ok(mut issue) = self.take_plan(issue_id) else {
            return Ok(());
        };
        issue.agents.sole_thread_mut().push_event_with_links(
            event,
            Some(summary),
            None,
            None,
            vec![crate::thread::ThreadLink::Implementation {
                issue_id: issue_id.to_string(),
                implementation_id: run_id.to_string(),
            }],
            now_rfc3339(),
        );
        self.finish_plan_mutation(issue_id.to_string(), issue)
    }

    /// Tell an issue that the branch implementing it is gone, and that nothing
    /// was merged out of it.
    ///
    /// The issue is about to come BACK to the inbox — the branch was what had
    /// been speaking for it — and a row that reappears with no explanation
    /// reads as the list losing track of its own work. So the conversation
    /// records what happened, naming the branch, in the one place the user will
    /// look when they wonder why this is in front of them again.
    ///
    /// Attention-class on purpose: the issue needs somebody to decide what
    /// happens to it next, which is the definition of unread.
    fn note_implementation_abandoned(
        &mut self,
        issue_id: &str,
        run_id: &str,
        branch: &str,
        how: &str,
    ) {
        let Ok(mut issue) = self.take_plan(issue_id) else {
            return;
        };
        let worktree_id = self
            .runs
            .get(run_id)
            .map(|run| crate::worktree::external_worktree_id(&run.worktree.path));
        let mut links = vec![crate::thread::ThreadLink::Implementation {
            issue_id: issue_id.to_string(),
            implementation_id: run_id.to_string(),
        }];
        if let Some(worktree_id) = worktree_id {
            links.push(crate::thread::ThreadLink::Worktree { worktree_id });
        }
        issue.agents.sole_thread_mut().push_event_with_links(
            crate::thread::ThreadEventKind::Abandoned,
            Some(abandoned_branch_summary(branch, how)),
            None,
            None,
            links,
            now_rfc3339(),
        );
        let persisted = self.finish_plan_mutation(issue_id.to_string(), issue);
        if let Err(error) = persisted {
            eprintln!("{issue_id}: could not record the abandoned branch {branch}: {error}");
        }
    }

    /// Canonical conversation owner for a run. Planned runs are implementation
    /// lineage of the Issue and therefore project the Issue thread; planless
    /// adopted runs remain independent worktree entities. `None` for a branch
    /// with no agents — it has no conversation until somebody speaks to it.
    fn conversation_thread_for_run<'a>(
        &'a self,
        run: &'a ActiveRun,
    ) -> Option<&'a crate::thread::Thread> {
        let primary = run.agents.primary()?;
        let conversation_id = primary.conversation_id();
        if conversation_id == primary.id {
            return Some(&primary.thread);
        }
        let owner = self.entity_of_agent(conversation_id)?;
        self.entity_agents(&owner)
            .ok()?
            .by_id(conversation_id)
            .map(|agent| &agent.thread)
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
            record_current_stage_started(issue.agents.sole_thread_mut(), run, stages);
        }
        self.finish_plan_mutation(issue_id, issue)
    }

    /// Queue a plan's turn for its agent in the primary checkout. A plan whose
    /// workspace is gone has no agent to hear it; the turn is dropped rather
    /// than delivered somewhere it does not belong.
    fn queue_plan_turn(&mut self, plan_id: &str, active: &ActivePlan, turn: AgentTurn) {
        if let Some(pending) = PendingAgentTurn::for_plan(plan_id, active, turn) {
            self.pending_agent_turns.push(pending);
        }
    }

    // ---- Captures -------------------------------------------------------------

    // ---- Routing --------------------------------------------------------------

    /// The catch-up packet for one conversation: the last `limit` messages to
    /// and from the agent, read out of the store when the tail this process
    /// booted onto does not hold them.
    ///
    /// The gate is answered off two integers, so a conversation held whole —
    /// the common case, and every storeless test daemon — is byte-identical to
    /// what the tail alone said and touches no SQL. A store that cannot answer
    /// falls back to the tail-built packet: a starved packet is today's
    /// behaviour, and it is a far smaller loss than dropping the turn.
    fn catch_up_packet(&self, thread: &crate::thread::Thread, limit: usize) -> String {
        if !thread.catch_up_reaches_stored_history(limit) {
            return thread.catch_up_markdown(limit);
        }
        let Some(store) = self.store.as_ref() else {
            return thread.catch_up_markdown(limit);
        };
        match store.thread_message_page(&thread.agent.id, limit) {
            Ok(history) => thread.catch_up_markdown_including_history(&history, limit),
            Err(error) => {
                eprintln!(
                    "catch-up packet for {}: {error}; the resident tail is what it carries",
                    thread.agent.id
                );
                thread.catch_up_markdown(limit)
            }
        }
    }

    /// The cold prompt a turn is actually handed over with: the prompt and its
    /// protocol block, closed with the durable conversation.
    ///
    /// Composed here, at the drain, rather than where the turn was built:
    /// transitions hold a conversation and no store, and a packet baked when
    /// the turn was queued would miss whatever was said while it waited for
    /// the lock. An owner whose conversation has gone (an entity closed under
    /// a queued turn) is handed the prompt as it stands — the delivery gate
    /// above has the last word on whether it travels at all.
    fn cold_prompt_with_catch_up(&self, owner: &str, agent_id: &str, cold: &str) -> String {
        let Ok(thread) = self.agent_conversation(owner, Some(agent_id)) else {
            return cold.to_string();
        };
        crate::orchestrator::append_durable_conversation(
            cold.to_string(),
            &self.catch_up_packet(thread, crate::orchestrator::CATCH_UP_MESSAGES),
            thread,
        )
    }

    /// Take everything the verbs that just ran queued, and mark it in flight in
    /// the same breath.
    ///
    /// One acquisition for both halves, because between them a turn on its way
    /// would be in neither the queue nor the marks: the idle sweep reading that
    /// demotes a run whose agent is coming, and a second message reading it
    /// queues a duplicate turn behind the one already on its way.
    ///
    /// A turn for an entity whose checkout is being cut, put back or removed
    /// right now stays in the queue — the same acquisition reads the rows the
    /// lifecycle verbs reserved. Spawning that entity's agent scaffolds its
    /// checkout directory, and `git worktree add` refuses a path that has
    /// reappeared under it, which a restore reads as a lost branch and answers
    /// by handing a healthy run to the recovery agent.
    fn take_pending_turns(&mut self) -> PendingTurns {
        let (held, mut queued): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending_agent_turns)
            .into_iter()
            .partition(|turn| self.checkout_is_in_flight(&turn.owner));
        // Back in the queue, in the order they were made: the drain that runs
        // after the job's epilogue takes them, and every frame drains.
        self.pending_agent_turns = held;
        for turn in &mut queued {
            self.forget_agent_start_error(&turn.owner, &turn.agent_id);
            // The one door every cold prompt passes: the conversation is read and
            // closed onto the prompt HERE, so the packet carries what the store
            // holds under the tail and what was said while the turn waited.
            if !turn.wants_catch_up {
                continue;
            }
            if let Some(say) = turn.say.as_mut() {
                say.cold = self.cold_prompt_with_catch_up(&turn.owner, &turn.agent_id, &say.cold);
            }
        }
        let state = self.settling_handle();
        let turns = queued
            .into_iter()
            .map(|turn| {
                let mark = self.turns_in_flight.take(&turn, state.clone());
                (turn, mark)
            })
            .collect();
        PendingTurns {
            turns,
            state,
            clock: Arc::clone(&self.frame_clock),
        }
    }

    // ---- Plan surface ---------------------------------------------------------

    /// Author a new plan: open a planning workspace (the primary checkout plus
    /// a scratch docs dir) and a plan agent session (the docs land canonically
    /// in the store on `done`).
    ///
    /// `dispatch: false` files the record and starts nothing — an inert issue,
    /// which is what the router and the toolbar's New issue create. The first
    /// `thread.post` to it starts the planning session.
    fn plan_create(&mut self, params: &Value) -> Result<Value, String> {
        let goal = require_str(params, "goal")?;
        let project_id = match params.get("project_id").and_then(Value::as_str) {
            Some(p) => p.to_string(),
            None => self.default_project()?,
        };
        let base = self.base_for(&project_id)?;
        let model_choice = model_choice_from(params, self.default_harness)?;
        self.require_store()?;
        let plan_id = format!("plan-{}", uuid::Uuid::new_v4());
        if !params
            .get("dispatch")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            let active = self.orch_for(&project_id)?.create_plan(
                PlanId::new(&plan_id),
                goal,
                &base,
                model_choice,
            );
            self.entity_project
                .insert(plan_id.clone(), project_id.clone());
            let (view, persisted) =
                self.answer_plan_mutation(plan_id, active, thread_detail(params));
            persisted?;
            return Ok(view);
        }
        let job = self.reserve_plan_workspace(
            &plan_id,
            project_id.clone(),
            goal.clone(),
            Box::new(IssueOpened {
                project_id,
                plan_id: plan_id.clone(),
                goal,
                base_branch: base,
                model_choice,
                detail: thread_detail(params),
            }),
        )?;
        Ok(self.defer_job(job))
    }

    /// `plan.create`'s apply half: the Issue's record, its first turn, and the
    /// view the caller asked for. The workspace its agent works in is on disk
    /// by now, which is why nothing here can fail on a directory.
    ///
    /// What a failure here leaves is the workspace: a scratch docs dir for an
    /// Issue that never opened, and the `.build/` config the next plan this
    /// project drafts overwrites. Removing either is filesystem work, which an
    /// epilogue may not do; neither is a checkout or a branch, so no board is
    /// missing anything.
    fn open_planned_issue(
        &mut self,
        opened: IssueOpened,
        workspace: crate::orchestrator::PlanWorkspace,
    ) -> Result<Value, String> {
        let IssueOpened {
            project_id,
            plan_id,
            goal,
            base_branch,
            model_choice,
            detail,
        } = opened;
        let project = self.orch_for(&project_id)?.clone();
        let mut active =
            project.create_plan(PlanId::new(&plan_id), goal, &base_branch, model_choice);
        let turn = project
            .open_plan_drafting(&mut active, workspace)
            .map_err(err)?;
        self.entity_project
            .insert(plan_id.clone(), project_id.clone());
        self.queue_plan_turn(&plan_id, &active, turn);
        if self.qa_agent {
            self.qa_simulate_plan(&project_id, &mut active)?;
        }
        let (view, persisted) = self.answer_plan_mutation(plan_id, active, detail);
        persisted?;
        Ok(view)
    }

    /// Reserve the workspace an inert Issue's first planning session needs.
    ///
    /// `Ok(None)` is "there is nothing to start": the session is already
    /// running, or another caller's spawn is already on its way to the same
    /// checkout. `Err` is "no session can start here at all" — the caller
    /// decides whether that is fatal, because a routed capture keeps its
    /// destination either way.
    ///
    /// The Issue must be in its map: this reads the record it is about to
    /// reserve a row for.
    fn reserve_plan_drafting(
        &mut self,
        issue_id: &str,
        opening: Box<dyn PlanSessionOpening>,
    ) -> Result<Option<WorktreeLifecycleJob>, String> {
        let active = self
            .plans
            .get(issue_id)
            .ok_or_else(|| format!("unknown issue_id: {issue_id}"))?;
        // Already has a session: this is a first turn, not a nudge, and a
        // second harness in the same checkout would report `done` twice.
        if active.workspace.is_some() {
            return Ok(None);
        }
        crate::plan::plan_transition(&active.plan.state, crate::plan::PlanEvent::Dispatch)
            .map_err(|error| error.to_string())?;
        let title = active.plan.goal.clone();
        let agent_id = active.agents.sole().id.clone();
        let checkout = self.primary_checkout_of(issue_id)?;
        if self.agent_is_on_its_way(&checkout, &agent_id) {
            return Ok(None);
        }
        let project_id = self.project_of(issue_id)?;
        self.reserve_plan_workspace(issue_id, project_id, title, opening)
            .map(Some)
    }

    /// Reserve the workspace one door to an Issue's planning agent needs, and
    /// build the job that writes it. The row stands on the Issue itself: what
    /// it holds is the one workspace every door writes into, so a second door
    /// waits rather than racing this one's `.build/` config.
    ///
    /// The project comes from the caller: an Issue being created is not in
    /// `entity_project` until its epilogue runs, and `plan.create` reserves
    /// through here like every other door.
    fn reserve_plan_workspace(
        &mut self,
        issue_id: &str,
        project_id: String,
        title: String,
        opening: Box<dyn PlanSessionOpening>,
    ) -> Result<WorktreeLifecycleJob, String> {
        let project = self.orch_for(&project_id)?.clone();
        let store = self.require_store()?.clone();
        // A plan cuts no branch and claims no checkout: it is written against
        // the primary one, so nothing else can collide with it.
        let row = PendingRow::creating(issue_id.to_string(), Some(project_id), title);
        self.reserve_lifecycle(
            row,
            Box::new(OpenPlanWorkspace {
                project,
                plan_id: issue_id.to_string(),
                store,
                opening,
            }),
        )
    }

    /// Start the planning session an inert Issue has never had, now that its
    /// workspace is on disk: the dispatch reads everything said to it so far,
    /// and the turn that spawns the session is queued.
    fn open_inert_plan_drafting(
        &mut self,
        issue_id: &str,
        workspace: crate::orchestrator::PlanWorkspace,
        detail: ThreadDetail,
    ) -> Result<Value, String> {
        let project_id = self.project_of(issue_id)?;
        self.settle_plan_session(issue_id, detail, |state, active| {
            let turn = state
                .orch_for(&project_id)?
                .open_plan_drafting(active, workspace)
                .map_err(err)?;
            state.queue_plan_turn(issue_id, active, turn);
            if state.qa_agent {
                state.qa_simulate_plan(&project_id, active)?;
            }
            Ok(())
        })
    }

    /// The tail every door to a planning agent shares: take the Issue's record
    /// out, open the session its workspace was written for, put the record back
    /// and answer with it. What differs is the middle, which is the door's own.
    ///
    /// The record is persisted either way — a session that could not open
    /// leaves the Issue as it was, with what was said still on its thread, and
    /// the error is what the caller hears.
    fn settle_plan_session(
        &mut self,
        plan_id: &str,
        detail: ThreadDetail,
        open: impl FnOnce(&mut AppState, &mut ActivePlan) -> Result<(), String>,
    ) -> Result<Value, String> {
        let mut active = self.take_plan(plan_id)?;
        let opened = open(self, &mut active);
        let (view, persisted) = self.answer_plan_mutation(plan_id.to_string(), active, detail);
        opened?;
        persisted?;
        Ok(view)
    }

    fn plan_get(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let active = self.plans.get(&plan_id).ok_or("unknown plan_id")?;
        // See `run_get`. An issue carries exactly one agent, so naming it is a
        // check rather than a choice — but the check still holds.
        let detail_thread = self.detail_thread_value(&plan_id, params)?;
        let mut view = self.plan_view(
            &plan_id,
            active,
            view_thread_detail(&detail_thread, params),
            DigestScope::Detail,
        );
        if let Some(thread) = detail_thread {
            view.as_object_mut()
                .expect("plan_view returns an object")
                .insert("thread".to_string(), thread);
        }
        Ok(view)
    }

    fn plan_list(&self) -> Value {
        let plans: Vec<Value> = self
            .plans
            .iter()
            .map(|(id, active)| self.plan_view(id, active, ThreadDetail::Digest, DigestScope::List))
            .collect();
        json!({ "plans": plans })
    }

    fn issue_list(&self) -> Value {
        let issues: Vec<Value> = self
            .plans
            .iter()
            .map(|(id, active)| self.plan_view(id, active, ThreadDetail::Digest, DigestScope::List))
            .collect();
        json!({ "issues": issues, "plans": issues })
    }

    fn issue_stages(&mut self, params: &Value) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        let issue = self.plans.get(&issue_id).ok_or("unknown issue_id")?;
        // An issue with no stages yet answers with the empty list that is the
        // truth: the surface asks issue.get and issue.stages together on every
        // poll, and refusing here left a fresh issue's page loading forever.
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
                        .agents
                        .sole_thread()
                        .doc_comments()
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

    /// The whole Issue an implementation RPC answers with — every field of the
    /// board's issue view, with `thread_detail` saying how much of its
    /// conversation rides along.
    fn issue_view_full(
        &self,
        issue_id: &str,
        thread_detail: ThreadDetail,
    ) -> Result<Value, String> {
        let issue = self.plans.get(issue_id).ok_or("unknown issue_id")?;
        Ok(self.plan_view(issue_id, issue, thread_detail, DigestScope::Detail))
    }

    fn issue_implement_all(&mut self, params: &Value) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        self.arm_issue_scheduler(&issue_id, ImplementationIntent::All)?;
        self.implement_issue(&issue_id, params, None)
    }

    /// Advance one Issue's scheduler for a frame: either it is settled here and
    /// the Issue is the answer, or it is waiting on git, which goes to the
    /// drain and answers for it. A refusal before any git blocks the scheduler,
    /// the same way the job's own refusal does.
    fn implement_issue(
        &mut self,
        issue_id: &str,
        params: &Value,
        blocked_stage: Option<String>,
    ) -> Result<Value, String> {
        match self.defer_issue_scheduler(issue_id, params, blocked_stage)? {
            Some(placeholder) => Ok(placeholder),
            None => self.issue_view_full(issue_id, thread_detail(params)),
        }
    }

    /// Advance one Issue's scheduler and hand whatever git it owes to the
    /// drain. `Some` is the placeholder the drain replaces with the job's own
    /// answer; `None` means the pass settled here and the caller answers.
    ///
    /// Every caller that HAS a drain comes through here — a frame asking for
    /// an implementation, a stage approval that wakes a parked scheduler, an
    /// agent's own recovery report — so no request cuts a checkout under the
    /// app mutex. A refusal before any git blocks the scheduler, the same way
    /// the job's own refusal does.
    fn defer_issue_scheduler(
        &mut self,
        issue_id: &str,
        request: &Value,
        blocked_stage: Option<String>,
    ) -> Result<Option<Value>, String> {
        match self.advance_issue_scheduler(issue_id, request) {
            Ok(Some(job)) => Ok(Some(self.defer_job(job))),
            Ok(None) => Ok(None),
            Err(error) => {
                self.block_issue_scheduler(issue_id, blocked_stage, &error);
                Err(error)
            }
        }
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
        self.implement_issue(&issue_id, params, Some(stage_id))
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
        self.finish_plan_mutation(issue_id.to_string(), issue)
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
        self.finish_plan_mutation(issue_id.to_string(), issue)
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

    /// Reconcile one Issue's durable intent with its implementation lineage,
    /// and run whatever git that owes right here — for boot, which reconciles
    /// every armed Issue before the first frame is served and has no drain to
    /// hand git to. Everything with one uses
    /// [`AppState::defer_issue_scheduler`] instead.
    fn advance_issue_scheduler_here(
        &mut self,
        issue_id: &str,
        request: &Value,
    ) -> Result<(), String> {
        match self.advance_issue_scheduler(issue_id, request)? {
            Some(job) => self.run_lifecycle_here(job).map(|_| ()),
            None => Ok(()),
        }
    }

    /// Reconcile one Issue's durable intent with its implementation lineage.
    /// This is deliberately idempotent: boot, approval, and completion may all
    /// call it, but the single-active-writer gate prevents duplicate checkouts.
    ///
    /// What comes back is the git the next step needs — cutting the checkout,
    /// or putting back one that was deleted — reserved but not yet run. The
    /// job's own epilogue carries on from where this stopped, so the caller
    /// decides only where the git runs, never what happens after it.
    fn advance_issue_scheduler(
        &mut self,
        issue_id: &str,
        request: &Value,
    ) -> Result<Option<WorktreeLifecycleJob>, String> {
        let intent = self
            .plans
            .get(issue_id)
            .ok_or("unknown issue_id")?
            .plan
            .implementation_intent
            .clone();
        if intent == ImplementationIntent::None {
            return Ok(None);
        }

        let target_stage = match &intent {
            ImplementationIntent::Stage(stage_id) => Some(stage_id.clone()),
            ImplementationIntent::All => next_unsettled_stage(
                &self.plans[issue_id].stages,
                self.current_issue_implementation(issue_id),
            )
            .map(|doc| doc.id.clone()),
            ImplementationIntent::None => None,
        };
        let Some(target_stage) = target_stage else {
            return self
                .set_issue_scheduler_activity(
                    issue_id,
                    Some(ImplementationIntent::None),
                    ImplementationActivity::Idle,
                )
                .map(|()| None);
        };
        let approved = self.plans[issue_id]
            .stages
            .iter()
            .find(|stage| stage.id == target_stage)
            .is_some_and(|stage| stage.state == StageDocState::Approved);
        if !approved {
            return self
                .set_issue_scheduler_activity(
                    issue_id,
                    None,
                    ImplementationActivity::WaitingApproval(target_stage),
                )
                .map(|()| None);
        }

        // No implementation yet: cutting its checkout is the next step, and
        // the job's epilogue resumes this scheduler on the run it opened.
        let Some(run_id) = self.current_issue_implementation_id(issue_id) else {
            self.set_issue_scheduler_activity(issue_id, None, ImplementationActivity::Preparing)?;
            let waiting = self.issue_scheduler_waiting_on(issue_id, request, &intent);
            return self
                .open_implementation(issue_id, request, waiting)
                .map(Some);
        };

        if let Some(attempt) = self.runs[&run_id]
            .recovery
            .as_ref()
            .filter(|attempt| attempt.state == crate::run::RecoveryState::Started)
        {
            return self
                .set_issue_scheduler_activity(
                    issue_id,
                    None,
                    ImplementationActivity::Blocked {
                        stage_id: attempt.requested_stage_id.clone(),
                        reason: self.runs[&run_id].last_error.clone().unwrap_or_else(|| {
                            format!("verified recovery {} is running", attempt.id)
                        }),
                    },
                )
                .map(|()| None);
        }

        let waiting = self.issue_scheduler_waiting_on(issue_id, request, &intent);
        if let Some(job) = self.ensure_issue_implementation_worktree(issue_id, &run_id, waiting)? {
            return Ok(Some(job));
        }
        self.dispatch_ready_stage(issue_id, &run_id, request)
            .map(|()| None)
    }

    /// Who the scheduler is: what it hears when the checkout it is waiting on
    /// exists, and which stage it marks blocked if that checkout never comes.
    fn issue_scheduler_waiting_on(
        &self,
        issue_id: &str,
        request: &Value,
        intent: &ImplementationIntent,
    ) -> Box<dyn ImplementationCaller> {
        Box::new(IssueSchedulerWaiting {
            issue_id: issue_id.to_string(),
            request: request.clone(),
            blocked_stage: match intent {
                ImplementationIntent::Stage(stage_id) => Some(stage_id.clone()),
                ImplementationIntent::All | ImplementationIntent::None => None,
            },
        })
    }

    /// The rest of one scheduler pass, once the implementation's checkout is on
    /// disk: dispatch the stage the intent named, or arm run-all and let the
    /// run chain through the stages itself.
    ///
    /// This is where a pass that had to stop for git resumes — the job's
    /// epilogue calls it with the run the git settled.
    fn dispatch_ready_stage(
        &mut self,
        issue_id: &str,
        run_id: &str,
        request: &Value,
    ) -> Result<(), String> {
        let intent = self
            .plans
            .get(issue_id)
            .ok_or("unknown issue_id")?
            .plan
            .implementation_intent
            .clone();
        let run_id = run_id.to_string();
        match intent {
            ImplementationIntent::Stage(stage_id) => {
                let already_started = self
                    .runs
                    .get(&run_id)
                    .ok_or("unknown run_id")?
                    .stage_progress(&stage_id)
                    .is_some();
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

    /// Make sure the implementation's checkout is where its run says it is.
    /// `None` means it already was; a job means it is being put back with
    /// `git worktree add` — and, when the branch is only on a remote, a fetch —
    /// and the scheduler carries on from that job's epilogue.
    fn ensure_issue_implementation_worktree(
        &mut self,
        issue_id: &str,
        run_id: &str,
        caller: Box<dyn ImplementationCaller>,
    ) -> Result<Option<WorktreeLifecycleJob>, String> {
        let (adopted, checkout_stood, worktree, title) = {
            let active = self
                .runs
                .get(run_id)
                .ok_or_else(|| format!("unknown run_id: {run_id}"))?;
            (
                active.adopted,
                active.worktree.path.exists(),
                active.worktree.clone(),
                active.run.goal.clone(),
            )
        };
        // An adopted checkout is somebody else's directory: Build never cut it,
        // so it cannot cut it again. Standing is all this can ask of one — and
        // when it is gone there is no git to run, only the recovery agent to
        // start and the Issue to tell.
        if adopted {
            if checkout_stood {
                return Ok(None);
            }
            return Err(self.start_checkout_recovery(
                issue_id,
                run_id,
                "adopted worktree is missing; its original checkout cannot be recreated safely",
            )?);
        }
        let project_id = self.project_of(run_id)?;
        let resolved = self.resolved_isolation(&project_id);
        let project = self.orch_for(&project_id)?.clone();
        let mut row = PendingRow::creating(run_id.to_string(), Some(project_id), title)
            .on_checkout(crate::worktree::external_worktree_id(&worktree.path))
            .implementing(issue_id.to_string());
        // A checkout that is still standing is verified and reused, not made,
        // and what is on disk describes itself: only a restore that has to put
        // one back names the isolation it is putting back.
        if !checkout_stood {
            row = row.isolated_as(resolved.isolation);
        }
        self.reserve_lifecycle(
            row,
            Box::new(RestoreImplementationCheckout {
                project,
                issue_id: issue_id.to_string(),
                run_id: run_id.to_string(),
                worktree,
                checkout_stood,
                caller,
                resolved,
            }),
        )
        .map(Some)
    }

    /// Write down what the restore found: the checkout is back (or was never
    /// really gone), and the Issue's conversation says which. A restore that
    /// failed hands the run to the verified recovery agent instead — the run's
    /// exact lineage is what is at stake, and only an agent can confirm it.
    fn settle_restored_checkout(&mut self, restored: RestoredCheckout) -> Result<Value, String> {
        let RestoredCheckout {
            issue_id,
            run_id,
            checkout_stood,
            restored,
            caller,
            downgrade,
        } = restored;
        let worktree = match restored {
            Ok(worktree) => worktree,
            Err(error) => {
                let recovering = self
                    .start_checkout_recovery(&issue_id, &run_id, &error)
                    .unwrap_or_else(|persist_failure| persist_failure);
                return caller.settle(self, Err(recovering));
            }
        };
        let settled = (|| -> Result<(), String> {
            let mut active = self.take_run(&run_id)?;
            active.worktree = worktree;
            active.last_error = None;
            if let Some(reason) = downgrade {
                self.note_isolation_downgrade(&run_id, &mut active, &reason);
            }
            let worktree_id = crate::worktree::external_worktree_id(&active.worktree.path);
            self.finish_run_mutation(run_id.clone(), active)?;
            let mut issue = self.take_plan(&issue_id)?;
            issue.agents.sole_thread_mut().push_event_with_links(
                if checkout_stood {
                    crate::thread::ThreadEventKind::WorktreeReused
                } else {
                    crate::thread::ThreadEventKind::WorktreeRecreated
                },
                Some(if checkout_stood {
                    "Verified and reused the original Issue worktree".to_string()
                } else {
                    "Recreated the Issue worktree from its original branch".to_string()
                }),
                None,
                None,
                vec![
                    crate::thread::ThreadLink::Implementation {
                        issue_id: issue_id.clone(),
                        implementation_id: run_id.clone(),
                    },
                    crate::thread::ThreadLink::Worktree { worktree_id },
                ],
                now_rfc3339(),
            );
            self.finish_plan_mutation(issue_id.clone(), issue)
        })();
        caller.settle(self, settled.map(|()| run_id.as_str()))
    }

    /// Hand a run whose checkout could not be put back to the verified recovery
    /// agent: a nonce-bound attempt on the record, the prompt that asks the
    /// agent to prove the exact lineage, and the Issue told what happened. What
    /// comes back is the message the caller refuses with.
    fn start_checkout_recovery(
        &mut self,
        issue_id: &str,
        run_id: &str,
        error: &str,
    ) -> Result<String, String> {
        let project_id = self.project_of(run_id)?;
        let mut active = self.take_run(run_id)?;
        if active
            .recovery
            .as_ref()
            .is_some_and(|attempt| attempt.state == crate::run::RecoveryState::Started)
        {
            self.runs.insert(run_id.to_string(), active);
            return Ok("verified Issue recovery is already running".to_string());
        }
        let issue = self.plans.get(issue_id).ok_or("unknown issue_id")?;
        let requested_stage_id = recovery_target_stage(issue, &active);
        let recovery_id = format!("recovery-{}", uuid::Uuid::new_v4());
        let started_at = now_rfc3339();
        let prompt = recovery_agent_prompt(
            &recovery_id,
            issue_id,
            run_id,
            &requested_stage_id,
            &active.worktree,
            error,
            &issue.stages,
        );
        active.recovery = Some(crate::run::RecoveryAttempt {
            id: recovery_id.clone(),
            requested_stage_id: requested_stage_id.clone(),
            branch: active.worktree.recorded_branch.clone(),
            state: crate::run::RecoveryState::Started,
            report: None,
            started_at: started_at.clone(),
            completed_at: None,
        });
        active.last_error = Some(format!(
            "automatic branch restoration failed: {error}; verified recovery agent started"
        ));
        self.queue_recovery_turn(run_id, &active, &project_id, prompt)?;
        self.finish_run_mutation(run_id.to_string(), active)?;
        self.note_recovery_started(
            issue_id,
            run_id,
            &recovery_id,
            &requested_stage_id,
            error,
            started_at,
        )?;
        Ok(format!(
            "automatic restore failed; verified recovery {recovery_id} started"
        ))
    }

    /// Hand the recovery prompt to the agent that will prove the lineage. An
    /// agentless run is logged rather than refused: the attempt is on the
    /// record either way, and a human can start an agent against it.
    fn queue_recovery_turn(
        &mut self,
        run_id: &str,
        active: &ActiveRun,
        project_id: &str,
        prompt: String,
    ) -> Result<(), String> {
        let project_root = self
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .map(|project| project.repo_path.clone())
            .ok_or("unknown project_id")?;
        match PendingAgentTurn::for_recovery(run_id, active, &project_root, prompt) {
            Some(turn) => self.pending_agent_turns.push(turn),
            None => eprintln!("recover {run_id}: no agent to hand the recovery to"),
        }
        Ok(())
    }

    /// Tell the Issue's conversation that a verified recovery is running,
    /// linked to the implementation it is for, the attempt itself, and the
    /// stage the agent is being asked to prove.
    fn note_recovery_started(
        &mut self,
        issue_id: &str,
        run_id: &str,
        recovery_id: &str,
        requested_stage_id: &str,
        error: &str,
        started_at: String,
    ) -> Result<(), String> {
        let mut issue = self.take_plan(issue_id)?;
        let mut links = vec![
            crate::thread::ThreadLink::Implementation {
                issue_id: issue_id.to_string(),
                implementation_id: run_id.to_string(),
            },
            crate::thread::ThreadLink::Recovery {
                recovery_id: recovery_id.to_string(),
            },
        ];
        if let Some(stage) = issue
            .stages
            .iter()
            .find(|stage| stage.id == requested_stage_id)
        {
            links.push(crate::thread::ThreadLink::IssueStage {
                issue_id: issue_id.to_string(),
                stage_id: stage.id.clone(),
                path: stage.path.clone(),
            });
        }
        issue.agents.sole_thread_mut().push_event_with_links(
            crate::thread::ThreadEventKind::RecoveryStarted,
            Some(format!(
                "Verified recovery started after automatic restore failed: {error}"
            )),
            None,
            None,
            links,
            started_at,
        );
        self.finish_plan_mutation(issue_id.to_string(), issue)
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
        if let Some(attempt) = run.recovery.as_ref().filter(|attempt| {
            matches!(
                attempt.state,
                crate::run::RecoveryState::Started | crate::run::RecoveryState::Failed
            )
        }) {
            return self.set_issue_scheduler_activity(
                issue_id,
                None,
                ImplementationActivity::Blocked {
                    stage_id: attempt.requested_stage_id.clone(),
                    reason: run
                        .last_error
                        .clone()
                        .unwrap_or_else(|| match attempt.state {
                            crate::run::RecoveryState::Started => {
                                format!("verified recovery {} is running", attempt.id)
                            }
                            crate::run::RecoveryState::Failed => {
                                format!("verified recovery {} failed", attempt.id)
                            }
                            crate::run::RecoveryState::Succeeded => unreachable!("filtered above"),
                        }),
                },
            );
        }
        let (intent, activity) = match run.run.state {
            RunState::Review | RunState::Merged => (
                Some(ImplementationIntent::None),
                ImplementationActivity::Idle,
            ),
            RunState::StageGate => {
                let next = next_unsettled_stage(&issue.stages, Some(run))
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
        self.issue_view_full(&issue_id, thread_detail(params))
    }

    fn issue_stage_diff(&mut self, params: &Value) -> Result<Value, String> {
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
        self.plan_run_stage_diff(&run_params, Some(issue_id))
    }

    fn issue_run_action(&mut self, params: &Value, action: &str) -> Result<Value, String> {
        let issue_id = require_str(params, "issue_id")?;
        let run_id = self
            .current_issue_implementation_id(&issue_id)
            .ok_or("issue has no active implementation")?;
        let mut run_params = params.clone();
        let object = run_params
            .as_object_mut()
            .ok_or("issue params must be an object")?;
        object.insert("run_id".to_string(), json!(run_id));
        // An `agent_id` an Issue surface sends names the ISSUE's one agent,
        // which is not on the implementation's roster: the implementation agent
        // it maps to is that run's first, which is what the verb defaults to.
        object.remove("agent_id");
        match action {
            "fix" => self.run_stage_fix(&run_params)?,
            "diff" => return self.plan_run_diff(&run_params, Some(issue_id)),
            "request_changes" => self.run_request_changes(&run_params)?,
            "git_action" => self.run_git_action(&run_params)?,
            _ => unreachable!("known issue run action"),
        };
        self.issue_view_full(&issue_id, thread_detail(params))
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
                    .agents
                    .sole_thread()
                    .doc_comments()
                    .iter()
                    .filter(|comment| comment.stage_id == doc.id)
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

    /// Approve the plan (the last human gate): the planning session ends and
    /// its scratch docs are dropped; the store docs are canonical.
    fn plan_approve(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let project_id = self.project_of(&plan_id)?;
        let mut active = self.take_plan(&plan_id)?;
        let session = issue_session(&active);
        let outcome = self
            .orch_for(&project_id)
            .and_then(|orch| orch.approve_plan(&mut active).map_err(err));
        if outcome.is_ok() {
            self.retire_issue_session(session);
            active.agents.sole_thread_mut().push_event(
                crate::thread::ThreadEventKind::Approved,
                Some("Plan approved".to_string()),
                None,
                None,
                now_rfc3339(),
            );
        }
        let (view, persisted) = self.answer_plan_mutation(plan_id, active, thread_detail(params));
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
        append_user_thread_messages(active.agents.sole_thread_mut(), messages);
        // Pure legality first, before any disk is asked for — and the notes are
        // durable either way: an illegal revise leaves what the reviewer wrote
        // on the conversation.
        let gated =
            crate::plan::plan_transition(&active.plan.state, crate::plan::PlanEvent::SendNotes)
                .map_err(|error| error.to_string());
        let title = active.plan.goal.clone();
        let persisted = self.finish_plan_mutation(plan_id.clone(), active);
        gated?;
        persisted?;
        let job = self.reserve_plan_workspace(
            &plan_id,
            project_id.clone(),
            title,
            Box::new(PlanNotesSent {
                plan_id: plan_id.clone(),
                project_id,
                detail: thread_detail(params),
            }),
        )?;
        Ok(self.defer_job(job))
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
            active.agents.sole_thread_mut().push_event(
                crate::thread::ThreadEventKind::StageApproved,
                Some(format!("Approved stage “{stage_title}”")),
                None,
                None,
                now_rfc3339(),
            );
        }
        let (view, persisted) =
            self.answer_plan_mutation(plan_id.clone(), active, thread_detail(params));
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
            // The approval this frame just made is what an armed Implement All
            // was parked on, so this hop cuts the whole implementation
            // checkout. It goes to the drain like every other frame's git —
            // and the Issue view it answers with is the one this verb was
            // going to answer with anyway, read after the hop rather than
            // before it.
            return self.implement_issue(
                &plan_id,
                &scheduler_request(&plan_id, params),
                Some(stage_id),
            );
        }
        Ok(view)
    }

    /// Send a stage's open comments to a fresh plan-revision session (the open
    /// comments ARE the payload).
    fn plan_stage_send_notes(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let project_id = self.project_of(&plan_id)?;
        let active = self.plans.get(&plan_id).ok_or("unknown plan_id")?;
        // Pure legality first — nothing is scaffolded for a revise that will be
        // refused, and this changes nothing to have to put back.
        crate::orchestrator::gate_plan_stage_notes(active, &stage_id).map_err(err)?;
        let title = active.plan.goal.clone();
        let job = self.reserve_plan_workspace(
            &plan_id,
            project_id.clone(),
            title,
            Box::new(StageNotesSent {
                plan_id: plan_id.clone(),
                project_id,
                stage_id,
                detail: thread_detail(params),
            }),
        )?;
        Ok(self.defer_job(job))
    }

    /// A freeform human message to the plan's agent.
    fn plan_message(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let message = require_str(params, "message")?;
        let project_id = self.project_of(&plan_id)?;
        // The user's own words, so the anchor gets its chance — before the
        // record leaves its map (see `note_user_message`).
        self.note_user_message(&plan_id);
        let mut active = self.take_plan(&plan_id)?;
        active
            .agents
            .sole_thread_mut()
            .post_user(&message, None, now_rfc3339());
        // Pure legality first, and the message is durable either way: a plan
        // that refuses the freeform channel still heard what was said.
        let gated = crate::orchestrator::gate_plan_message(&active, NEW_THREAD_MESSAGES_PROMPT)
            .map_err(err);
        let title = active.plan.goal.clone();
        let persisted = self.finish_plan_mutation(plan_id.clone(), active);
        gated?;
        persisted?;
        let job = self.reserve_plan_workspace(
            &plan_id,
            project_id.clone(),
            title,
            Box::new(PlanMessaged {
                plan_id: plan_id.clone(),
                project_id,
                detail: thread_detail(params),
            }),
        )?;
        Ok(self.defer_job(job))
    }

    fn plan_abandon(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let project_id = self.project_of(&plan_id)?;
        let mut active = self.take_plan(&plan_id)?;
        let session = issue_session(&active);
        let outcome = self
            .orch_for(&project_id)
            .and_then(|orch| orch.abandon_plan(&mut active).map_err(err));
        if outcome.is_ok() {
            self.retire_issue_session(session);
            active.agents.sole_thread_mut().push_event(
                crate::thread::ThreadEventKind::Abandoned,
                Some("Plan abandoned".to_string()),
                None,
                None,
                now_rfc3339(),
            );
        }
        let (view, persisted) = self.answer_plan_mutation(plan_id, active, thread_detail(params));
        outcome?;
        persisted?;
        Ok(view)
    }

    /// `issue.archive` — Done, for an issue: file it away without changing its
    /// lifecycle state or deleting canonical docs/run history. Repeating the
    /// request preserves the first archive timestamp.
    ///
    /// Never refused for what was or was not built: an issue the user is done
    /// with is done, and an issue no branch ever implemented says so as a
    /// warning on the row (`finish.warnings`) for them to confirm through.
    fn plan_archive(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let mut active = self.take_plan(&plan_id)?;
        if active.plan.archived_at.is_none() {
            active.plan.archived_at = Some(now_rfc3339());
        }
        let (view, persisted) = self.answer_plan_mutation(plan_id, active, thread_detail(params));
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

    /// Comment on one stage document.
    ///
    /// The comment IS a post on the Issue agent's conversation, anchored to the
    /// passage it is about — the same path a diff comment takes. There is no
    /// second record: the agent reads it with the tool it reads its messages
    /// with, and deleting the post deletes the comment.
    fn plan_comment_add(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let body = require_str(params, "body")?;
        let mut active = self.take_plan(&plan_id)?;
        let mut minted: Option<crate::thread::DocComment> = None;
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
            let path = active.stages[index].path.clone();
            let id = active.agents.sole_thread_mut().post_doc_comment(
                &plan_id,
                &stage_id,
                &path,
                anchor,
                body.clone(),
                now_rfc3339(),
            );
            minted = active
                .agents
                .sole_thread()
                .doc_comments()
                .into_iter()
                .find(|comment| comment.id == id);
            Ok(())
        })();
        let persisted = self.finish_plan_mutation(plan_id, active);
        outcome?;
        persisted?;
        let comment = minted.expect("outcome Ok implies a comment was posted");
        Ok(json!({ "comment": comment_json(&comment) }))
    }

    fn plan_comment_delete(&mut self, params: &Value) -> Result<Value, String> {
        let plan_id = require_str(params, "plan_id")?;
        let comment_id = require_str(params, "comment_id")?;
        let mut active = self.take_plan(&plan_id)?;
        let outcome = (|| -> Result<(), String> {
            let known = active
                .agents
                .sole_thread()
                .doc_comments()
                .into_iter()
                .find(|comment| comment.id == comment_id)
                .ok_or_else(|| format!("unknown comment_id: {comment_id}"))?;
            if known.state != crate::thread::DocCommentState::Open {
                return Err("only open comments can be deleted".to_string());
            }
            active
                .agents
                .sole_thread_mut()
                .remove_doc_comment(&comment_id)
                .map(|_| ())
                .ok_or_else(|| format!("unknown comment_id: {comment_id}"))
        })();
        let persisted = self.finish_plan_mutation(plan_id, active);
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
        let job = self.open_implementation(
            &plan_id,
            params,
            Box::new(RunOpenedView {
                detail: thread_detail(params),
            }),
        )?;
        Ok(self.defer_job(job))
    }

    /// Settle everything an Issue's implementation needs before any git runs —
    /// the run's id, the checkout it works in, the model its agent runs on —
    /// and hand the git itself to the drain.
    ///
    /// Shared by `run.create` and by the Issue scheduler, which differ only in
    /// `caller`: who is waiting for the run, and what a failure leaves written
    /// on the Issue.
    fn open_implementation(
        &mut self,
        issue_id: &str,
        params: &Value,
        caller: Box<dyn ImplementationCaller>,
    ) -> Result<WorktreeLifecycleJob, String> {
        // Targeting: an Issue can be implemented into a checkout that already
        // exists instead of one cut for it (Decisions §Issue view — the stage
        // column's assignment control).
        if let Some(worktree_id) = params
            .get("worktree_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            let worktree_id = worktree_id.to_string();
            return self.adopt_implementation_checkout(issue_id, &worktree_id, params, caller);
        }
        if !self.plans.contains_key(issue_id) {
            return Err("unknown plan_id".to_string());
        }
        let project_id = self.project_of(issue_id)?;
        let requested_choice = model_choice_from(params, self.default_harness)?;
        let base = params
            .get("base_branch")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| self.plans[issue_id].base_branch.clone());
        let has_active_run = self.runs.values().any(|r| {
            r.run.plan_id.as_ref().map(|p| p.0.as_str()) == Some(issue_id)
                && !r.run.state.is_terminal()
        });
        let store = self.require_store()?.clone();
        let plan = &self.plans[issue_id];
        let model_choice = if has_agent_choice(params) {
            requested_choice
        } else {
            plan.model_choice.clone()
        };
        let title = plan.plan.goal.clone();
        let issue = ImplementableIssue::judge(RunSource {
            plan,
            has_active_run,
        })
        .map_err(err)?;
        let project = self.orch_for(&project_id)?.clone();
        let resolved = self.resolved_isolation(&project_id);
        let run_id = format!("run-{}", uuid::Uuid::new_v4());
        // The ref this implementation is about to cut is on the row, so a
        // create or a dispatch claiming the same one collides here rather than
        // in git.
        let row = PendingRow::creating(run_id.clone(), Some(project_id.clone()), title)
            .on_branch(crate::worktree::branch_name_for(issue.slug()))
            .implementing(issue_id.to_string())
            .isolated_as(resolved.isolation);
        self.reserve_lifecycle(
            row,
            Box::new(OpenImplementation {
                project,
                project_id,
                issue_id: issue_id.to_string(),
                issue,
                base_branch: base,
                run_id,
                store,
                model_choice,
                caller,
                resolved,
            }),
        )
    }

    /// Implement an Issue into a checkout that already exists, named by the
    /// `worktree_id` the feed's branch rows carry.
    ///
    /// The branch's run adopts the implementation — one branch, one run — so a
    /// checkout Build has never seen is adopted first, and a branch already
    /// implementing a DIFFERENT Issue is refused: two Issues writing one branch
    /// would make neither one's diff readable.
    fn adopt_implementation_checkout(
        &mut self,
        issue_id: &str,
        worktree_id: &str,
        params: &Value,
        caller: Box<dyn ImplementationCaller>,
    ) -> Result<WorktreeLifecycleJob, String> {
        if !self.plans.contains_key(issue_id) {
            return Err("unknown plan_id".to_string());
        }
        let project_id = self.project_of(issue_id)?;
        let requested_choice = model_choice_from(params, self.default_harness)?;
        let (run_id, checkout) = match self.run_owning_worktree_id(&project_id, worktree_id) {
            Some(run_id) => {
                // The primary checkout is the repository, not a worktree to
                // hand an Issue: committing stage docs there lands them on the
                // branch the human is standing on.
                if self.owns_primary_checkout(&run_id, &self.runs[&run_id]) {
                    return Err(
                        "cannot implement into the primary checkout — it is the repository, not a \
                         worktree to hand over"
                            .to_string(),
                    );
                }
                if let Some(other) = self.runs[&run_id]
                    .run
                    .plan_id
                    .as_ref()
                    .filter(|id| id.0 != issue_id)
                {
                    return Err(format!(
                        "cannot implement into {}: it is already implementing Issue {} — finish \
                         or abandon that implementation first",
                        self.runs[&run_id].worktree.branch(),
                        other.0
                    ));
                }
                let checkout = self.runs[&run_id].worktree.path.clone();
                (run_id, ImplementationCheckout::Owned(checkout))
            }
            // Nobody owns it yet, so this implementation's git takes it over
            // first — the scan that resolves the card and the checkpoint commit
            // that writes Build's ownership into it, both off the lock, and the
            // run they mint is the one the implementation is written onto.
            None => (
                format!("run-{}", uuid::Uuid::new_v4()),
                ImplementationCheckout::Unowned {
                    target: AdoptionTarget::Card {
                        worktree_id: worktree_id.to_string(),
                        excluded: self.bound_worktree_paths(),
                    },
                    base_branch: self.base_for(&project_id)?,
                },
            ),
        };

        let has_active_run = self.runs.iter().any(|(id, run)| {
            id != &run_id
                && run.run.plan_id.as_ref().map(|p| p.0.as_str()) == Some(issue_id)
                && !run.run.state.is_terminal()
        });
        let model_choice = if has_agent_choice(params) {
            requested_choice
        } else {
            self.plans[issue_id].model_choice.clone()
        };
        let store = self.require_store()?.clone();
        let plan = &self.plans[issue_id];
        let title = plan.plan.goal.clone();
        let issue = ImplementableIssue::judge(RunSource {
            plan,
            has_active_run,
        })
        .map_err(err)?;
        let project = self.orch_for(&project_id)?.clone();
        // The run stays on the board while its checkout is checkpointed: it is
        // the same run either way, and a run that vanished from every poll for
        // the length of two commits would read as one that had been abandoned.
        // What the row holds is the checkout, which nothing else may claim
        // until this hand-over is written down.
        let row = PendingRow::creating(run_id.clone(), Some(project_id.clone()), title)
            .on_checkout(worktree_id.to_string())
            .implementing(issue_id.to_string());
        self.reserve_lifecycle(
            row,
            Box::new(AdoptImplementation {
                project,
                project_id,
                issue_id: issue_id.to_string(),
                issue,
                run_id,
                checkout,
                store,
                model_choice,
                caller,
            }),
        )
    }

    /// `run.create`'s apply half on a checkout that was cut for it: open the
    /// run around what the git prepared, and answer whoever asked.
    fn open_prepared_implementation(
        &mut self,
        opened: ImplementationOpened,
    ) -> Result<Value, String> {
        let ImplementationOpened {
            project_id,
            issue_id,
            run_id,
            prepared,
            model_choice,
            caller,
            downgrade,
        } = opened;
        let opened = (|| -> Result<OpenedImplementation, String> {
            let plan = self.plans.get(&issue_id).ok_or("unknown plan_id")?;
            let conversation_id = plan.agents.sole().conversation_id().to_string();
            let (mut active, turn) = self
                .orch_for(&project_id)?
                .open_prepared_run(RunId::new(&run_id), plan, prepared, model_choice)
                .map_err(err)?;
            active
                .agents
                .primary_mut()
                .expect("a dispatched run opens with its agent")
                .bind_conversation(conversation_id);
            let agent_id = active
                .agents
                .primary()
                .expect("a dispatched run opens with its agent")
                .id
                .clone();
            Ok(OpenedImplementation {
                checkout_summary: format!("Created the Issue implementation worktree for {run_id}"),
                run_id: run_id.clone(),
                project_id,
                issue_id,
                active,
                turn,
                agent_id,
                checkout_event: crate::thread::ThreadEventKind::WorktreeCreated,
            })
        })()
        .and_then(|mut opened| {
            if let Some(reason) = downgrade {
                self.note_isolation_downgrade(&run_id, &mut opened.active, &reason);
            }
            self.open_implementation_run(opened)
        });
        caller.settle(self, opened.map(|()| run_id.as_str()))
    }

    /// The same, on a checkout an existing run already owns: the run is taken
    /// out, handed the implementation the git prepared it for, and put back.
    fn open_adopted_implementation(
        &mut self,
        opened: ImplementationAdopted,
    ) -> Result<Value, String> {
        let ImplementationAdopted {
            project_id,
            issue_id,
            run_id,
            base_sha,
            adopted,
            model_choice,
            caller,
        } = opened;
        let opened = (|| -> Result<(), String> {
            // The run this is written onto: the one the branch already had, or
            // the one the adoption in this job's git phase just earned.
            let mut active = match &adopted {
                Some(adopted) => adopted.open_run(self)?,
                None => self.take_run(&run_id)?,
            };
            let handed_over = (|| -> Result<(AgentTurn, String), String> {
                let plan = self.plans.get(&issue_id).ok_or("unknown plan_id")?;
                self.orch_for(&project_id)?
                    .open_adopted_implementation(&mut active, plan, base_sha, model_choice)
                    .map_err(err)
            })();
            let (turn, agent_id) = match handed_over {
                Ok(opened) => opened,
                Err(error) => {
                    // Nothing was handed over. The branch keeps the run it had
                    // — or, when this job earned it one, keeps the plain
                    // adopted run its checkout is now Build's under.
                    let put_back = self.finish_run_mutation(run_id.clone(), active);
                    return Err(match put_back {
                        Ok(()) => error,
                        Err(store) => format!("{error}; and the run could not be saved: {store}"),
                    });
                }
            };
            let branch = active.worktree.branch();
            self.open_implementation_run(OpenedImplementation {
                checkout_summary: format!("Implementing into the existing checkout on {branch}"),
                run_id: run_id.clone(),
                project_id,
                issue_id,
                active,
                turn,
                agent_id,
                checkout_event: crate::thread::ThreadEventKind::WorktreeReused,
            })
        })();
        caller.settle(self, opened.map(|()| run_id.as_str()))
    }

    /// The tail every implementation dispatch shares: address the first turn to
    /// the agent that will hear it, drive it under QA, persist the run, and
    /// record on the Issue's conversation which checkout the work went into.
    ///
    /// The answer is not built here — who asked is what decides that, and by
    /// this point they are as far apart as `run.create` and a scheduled stage.
    fn open_implementation_run(&mut self, opened: OpenedImplementation) -> Result<(), String> {
        let OpenedImplementation {
            run_id,
            project_id,
            issue_id,
            mut active,
            turn,
            agent_id,
            checkout_event,
            checkout_summary,
        } = opened;
        let plan_docs = self.owning_plan_stage_docs(&active);

        self.entity_project
            .insert(run_id.clone(), project_id.clone());
        self.pending_agent_turns
            .push(PendingAgentTurn::for_run_agent(
                &run_id, &agent_id, &active, turn,
            ));
        if self.qa_agent {
            self.qa_drive_run(&project_id, &mut active, &plan_docs)?;
        }
        let worktree_id = crate::worktree::external_worktree_id(&active.worktree.path);
        // The checkout belongs to a run from here on, so it leaves the unbound
        // list its mutation named it on. A checkout that was already bound was
        // never in that list, so this is routinely a no-op.
        let checkout = active.worktree.path.clone();
        let persisted = self.finish_run_mutation(run_id.clone(), active);
        persisted?;
        self.note_worktree_gone(&project_id, &checkout);
        let mut plan = self.take_plan(&issue_id)?;
        let implementation_link = crate::thread::ThreadLink::Implementation {
            issue_id: issue_id.clone(),
            implementation_id: run_id.clone(),
        };
        plan.agents.sole_thread_mut().push_event_with_links(
            checkout_event,
            Some(checkout_summary),
            None,
            None,
            vec![
                implementation_link.clone(),
                crate::thread::ThreadLink::Worktree { worktree_id },
            ],
            now_rfc3339(),
        );
        plan.agents.sole_thread_mut().push_event_with_links(
            crate::thread::ThreadEventKind::ImplementationStarted,
            Some(format!("Implementation started as {run_id}")),
            None,
            None,
            vec![implementation_link],
            now_rfc3339(),
        );
        if let Some(run) = self.runs.get(&run_id) {
            record_current_stage_started(plan.agents.sole_thread_mut(), run, &plan_docs);
        }
        let plan_persisted = self.finish_plan_mutation(issue_id, plan);
        plan_persisted?;
        self.auto_advance_run(&run_id);
        Ok(())
    }

    fn run_get(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        // Which conversation, and how much of it: the rail's open bubble names
        // the agent, and the client's cursor says what it already holds.
        // Neither → the thread the view builds itself, cut the same way.
        let detail_thread = self.detail_thread_value(&run_id, params)?;
        let mut view = self.run_view(
            &run_id,
            active,
            view_thread_detail(&detail_thread, params),
            DigestScope::Detail,
        );
        if let Some(thread) = detail_thread {
            view.as_object_mut()
                .expect("run_view returns an object")
                .insert("thread".to_string(), thread);
        }
        Ok(view)
    }

    fn run_diff(&mut self, params: &Value) -> Result<Value, String> {
        self.plan_run_diff(params, None)
    }

    /// `run.diff`, with the issue that asked for it when an issue surface did.
    fn plan_run_diff(&mut self, params: &Value, issue_id: Option<String>) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        self.project_of(&run_id)?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        let subject = ReadSubject::Run {
            worktree_path: active.worktree.path.clone(),
            base_sha: active.base_sha.clone(),
            base_branch: active.worktree.base_branch.clone(),
        };
        Ok(self.defer_conditional_read(
            subject,
            issue_id,
            params.get("if_diff_key").and_then(Value::as_str),
        ))
    }

    /// Immutable stage review surface. Unlike `run.diff`, this never reads the
    /// working directory or current HEAD: it resolves only the two object ids
    /// persisted when the stage was dispatched and successfully validated.
    fn run_stage_diff(&mut self, params: &Value) -> Result<Value, String> {
        self.plan_run_stage_diff(params, None)
    }

    /// `run.stage_diff`, with the issue that asked for it when an issue
    /// surface did.
    fn plan_run_stage_diff(
        &mut self,
        params: &Value,
        issue_id: Option<String>,
    ) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        let progress = active
            .stage_progress(&stage_id)
            .ok_or_else(|| format!("unknown stage_id: {stage_id}"))?;
        let (Some(start_sha), Some(completion_sha)) =
            (progress.start_sha.clone(), progress.completion_sha.clone())
        else {
            let mut unavailable = json!({
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
            });
            if let Some(issue_id) = issue_id {
                unavailable
                    .as_object_mut()
                    .expect("built as an object")
                    .insert("issue_id".to_string(), json!(issue_id));
            }
            return Ok(unavailable);
        };
        let worktree_path = active.worktree.path.clone();
        let object_database = if worktree_path.exists() {
            worktree_path
        } else {
            std::path::PathBuf::from(self.project_path_for(&run_id))
        };
        let subject = ReadSubject::Stage {
            run_id,
            stage_id,
            object_database,
            start_sha,
            completion_sha,
        };
        Ok(self.defer_read(subject, issue_id))
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
        let run_agent_id = self.resolve_conversation_params(&run_id, params)?.agent_id;
        self.edit_agent_conversation(&run_id, &run_agent_id, |thread, _artifact| {
            append_user_thread_messages(thread, messages);
            Ok(())
        })?;
        let mut active = self.take_run(&run_id)?;
        let outcome = (|| -> Result<(), String> {
            let turn = self
                .orch_for(&project_id)?
                .run_request_changes(
                    &mut active,
                    &plan_docs,
                    NEW_THREAD_MESSAGES_PROMPT,
                    Some(&run_agent_id),
                )
                .map_err(err)?;
            self.pending_agent_turns
                .push(PendingAgentTurn::for_run_agent(
                    &run_id,
                    &run_agent_id,
                    &active,
                    turn,
                ));
            self.qa_drive_run(&project_id, &mut active, &plan_docs)
        })();
        let (view, persisted) = self.answer_run_mutation(run_id, active, thread_detail(params));
        outcome?;
        persisted?;
        Ok(view)
    }

    fn run_stage_dispatch(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let stage_id = require_str(params, "stage_id")?;
        let model_override = if has_agent_choice(params) {
            Some(model_choice_from(params, self.default_harness)?)
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
                .push(PendingAgentTurn::for_run(&run_id, &mut active, turn));
            self.qa_drive_run(&project_id, &mut active, &plan_docs)
        })();
        let persisted = self.finish_run_mutation(run_id.clone(), active);
        outcome?;
        persisted?;
        self.record_issue_current_stage_started(&run_id, &plan_docs)?;
        self.auto_advance_run(&run_id);
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        Ok(self.run_view(&run_id, active, thread_detail(params), DigestScope::Detail))
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
                .push(PendingAgentTurn::for_run(&run_id, &mut active, turn));
            self.qa_drive_run(&project_id, &mut active, &plan_docs)
        })();
        let persisted = self.finish_run_mutation(run_id.clone(), active);
        outcome?;
        persisted?;
        self.record_issue_current_stage_started(&run_id, &plan_docs)?;
        self.auto_advance_run(&run_id);
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        Ok(self.run_view(&run_id, active, thread_detail(params), DigestScope::Detail))
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
            let turn = self
                .orch_for(&project_id)?
                .send_run_stage_notes(&mut active, plan, &stage_id)
                .map_err(err)?;
            self.pending_agent_turns
                .push(PendingAgentTurn::for_run(&run_id, &mut active, turn));
            if self.qa_agent {
                self.qa_simulate_run_stage_revise(&project_id, &mut active, plan)?;
            }
            Ok(())
        })();
        // Both entities re-insert before any error propagates — the
        // take → finish_mutation invariant covers the plan here too.
        let plan_persisted = plan.map(|plan| self.finish_plan_mutation(plan_id, plan));
        let persisted = self.finish_run_mutation(run_id.clone(), active);
        outcome?;
        if let Some(persisted) = plan_persisted {
            persisted?;
        }
        persisted?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        Ok(self.run_view(&run_id, active, thread_detail(params), DigestScope::Detail))
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
        let persisted = self.finish_run_mutation(run_id.clone(), active);
        persisted?;
        if enabled {
            self.auto_advance_run(&run_id);
        }
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        Ok(self.run_view(&run_id, active, thread_detail(params), DigestScope::Detail))
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
                    .push(PendingAgentTurn::for_run(run_id, &mut active, turn));
                self.qa_drive_run(&project_id, &mut active, &plan_docs)
            })();
            let persisted = self.finish_run_mutation(run_id.to_string(), active);
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
    #[allow(clippy::cognitive_complexity)] // ratchet: run_git_action is at 18, threshold 15 — bring it under, then remove
    fn run_git_action(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let action = require_str(params, "action")?;
        let project_id = self.project_of(&run_id)?;
        let is_merge_action = matches!(action.as_str(), "merge" | "merge_push");
        if !is_merge_action && params.get("cleanup").is_some() {
            return Err("cleanup only applies to merge actions".to_string());
        }
        let bound = self.runs.get(&run_id).ok_or("unknown run_id")?;
        let adopted = bound.adopted;
        // A merge lands the run's branch on the base branch through the primary
        // checkout. For a primary run that target IS the checkout being merged
        // — a no-op when it sits on the base branch, and a merge into the wrong
        // tree when it does not.
        if is_merge_action && self.owns_primary_checkout(&run_id, bound) {
            return Err(
                "run.git_action: the primary checkout cannot be merged — its branch is what a \
                 merge would target"
                    .to_string(),
            );
        }
        let cleanup = if is_merge_action {
            merge_cleanup_from(params, adopted)?
        } else {
            MergeCleanup::Prune
        };
        let mut active = self.take_run(&run_id)?;
        let issue_id = active.run.plan_id.as_ref().map(|id| id.0.clone());

        // Push and merge cross a process boundary: git may complete and the
        // daemon may die before the resulting state is saved. Commit first so
        // the exact candidate is known, then fsync a write-ahead intent before
        // invoking the externally visible side effect.
        if matches!(action.as_str(), "push" | "merge" | "merge_push") {
            let prepared = self
                .orch_for(&project_id)
                .and_then(|orch| orch.run_commit(&active).map_err(err))
                .and_then(|()| git_stdout(&active.worktree.path, &["rev-parse", "HEAD"]))
                .map(|candidate_sha| PublicationAttempt {
                    action: action.clone(),
                    candidate_sha: candidate_sha.trim().to_string(),
                    started_at: now_rfc3339(),
                });
            let attempt = match prepared {
                Ok(attempt) => attempt,
                Err(error) => {
                    self.runs.insert(run_id, active);
                    return Err(error);
                }
            };
            active.publication_attempt = Some(attempt);
            if let Err(error) = self.persist_run_record(&run_id, &active) {
                self.runs.insert(run_id, active);
                return Err(error);
            }
        }
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
                // Clearing this field and the lifecycle/thread update below
                // share the post-side-effect atomic record write. If that write
                // is interrupted, boot still sees the prior journal.
                active.publication_attempt = None;
            }
        }
        let links = issue_id
            .as_ref()
            .and_then(|issue_id| self.plans.get(issue_id).map(|issue| (issue_id, issue)))
            .map(|(issue_id, issue)| {
                let mut links = vec![crate::thread::ThreadLink::Implementation {
                    issue_id: issue_id.clone(),
                    implementation_id: run_id.clone(),
                }];
                links.extend(issue.stages.iter().map(|stage| {
                    crate::thread::ThreadLink::IssueStage {
                        issue_id: issue_id.clone(),
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
        if let (None, Some(primary)) = (&issue_id, active.agents.primary_mut()) {
            primary.thread.push_event_with_links(
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
        let (view, persisted) =
            self.answer_run_mutation(run_id.clone(), active, thread_detail(params));
        let issue_persisted = if let Some(issue_id) = issue_id {
            let mut issue = self.take_plan(&issue_id)?;
            issue.agents.sole_thread_mut().push_event_with_links(
                event,
                Some(summary),
                None,
                None,
                links,
                now_rfc3339(),
            );
            Some(self.finish_plan_mutation(issue_id, issue))
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
            orch.discard_checkout(worktree, /* keep_branch */ false);
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
            MergeCleanup::Prune => {
                self.retire_agents_of_pruned_worktree(&worktree.path);
                self.prune_merged_worktree(project_id, worktree);
            }
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
                self.run_files_changed_at.remove(run_id);
                self.invalidate_run_stat(run_id);
                self.rescan_external_worktrees(project_id);
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
        // See `plan_message`: the user is speaking, so the anchor may move.
        self.note_user_message(&run_id);
        let agent_id = self.ensure_primary_agent(&run_id)?;
        self.edit_agent_conversation(&run_id, &agent_id, |thread, _artifact| {
            thread.post_user(&message, None, now_rfc3339());
            Ok(())
        })?;
        let mut active = self.take_run(&run_id)?;
        let outcome = (|| -> Result<(), String> {
            let turn = self
                .orch_for(&project_id)?
                .message_run(&mut active, &plan_docs, NEW_THREAD_MESSAGES_PROMPT)
                .map_err(err)?;
            self.pending_agent_turns
                .push(PendingAgentTurn::for_run(&run_id, &mut active, turn));
            if self.qa_agent && active.run.state == RunState::Building {
                self.qa_drive_run(&project_id, &mut active, &plan_docs)?;
            }
            Ok(())
        })();
        let (view, persisted) = self.answer_run_mutation(run_id, active, thread_detail(params));
        outcome?;
        persisted?;
        Ok(view)
    }

    /// Abandon a run: end it, take its agents down, and take back the checkout
    /// it was working in — the directory, never the branch, because a run's
    /// work outlives the run so it can be re-attempted.
    ///
    /// Every refusal is spent here, before anything is torn down. What the
    /// drain runs is git that cannot fail the verb: the stage publications the
    /// removal is about to make unreadable, the wait for the agents to die, and
    /// the removal itself.
    fn run_abandon(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let project_id = self.project_of(&run_id)?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        // Judged before a single agent is killed: an abandon that is not legal
        // must leave the run exactly as it found it.
        run_transition(&active.run.state, RunEvent::Abandon)
            .map_err(|illegal| illegal.to_string())?;
        // Abandoning removes the run's worktree — which for a primary run is
        // the repository. That run ends by letting go of the checkout instead.
        let keeps_checkout = self.owns_primary_checkout(&run_id, active);
        let title = active.run.goal.clone();
        let issue_id = active.run.plan_id.as_ref().map(|id| id.0.clone());
        let stages = self.stage_publication_query(&run_id, active);
        let project = self.orch_for(&project_id)?.clone();
        let settlement = Box::new(RunAbandoned {
            run_id: run_id.clone(),
            project_id: project_id.clone(),
            issue_id,
            detail: thread_detail(params),
            stages,
            published: StagePublications::default(),
        });
        self.discard_run(
            run_id,
            Some(project_id),
            title,
            move |worktree| match keeps_checkout {
                true => DiscardedCheckout::Kept,
                false => DiscardedCheckout::Removed {
                    project,
                    worktree: worktree.clone(),
                },
            },
            settlement,
        )
    }

    /// Take one run off the board and let go of the checkout it was working in.
    ///
    /// `run.abandon` and `run.delete` differ in three things: what happens to
    /// the directory, what is still owed the records once it is gone, and what
    /// the row standing in the run's place is called. Everything around
    /// them — the row, the run coming out of the map, the stat the board
    /// cached, the agents that were writing into the directory — is the same
    /// decide phase, and it is this one.
    ///
    /// The caller's refusals are all spent before it gets here: `take` runs
    /// with the row already on the board and cannot fail.
    fn discard_run(
        &mut self,
        run_id: String,
        project_id: Option<String>,
        title: String,
        checkout: impl FnOnce(&crate::worktree::Worktree) -> DiscardedCheckout,
        settlement: Box<dyn DiscardSettlement>,
    ) -> Result<Value, String> {
        let worktree_path = self
            .runs
            .get(&run_id)
            .ok_or("unknown run_id")?
            .worktree
            .path
            .clone();
        let row = PendingRow::discarding(run_id.clone(), project_id, title).on_checkout(
            crate::worktree::external_worktree_id(&Self::canonical_root(&worktree_path)),
        );
        self.defer_lifecycle_holding(row, move |state| {
            let active = state
                .runs
                .remove(&run_id)
                .expect("the run was read out of the map above");
            state.invalidate_run_stat(&run_id);
            // A human who took a run off the board must not keep paying for the
            // agent that was working on it, so the kill is explicit — and the
            // removal waits it out rather than walking a directory a live child
            // is still writing into.
            let retirements = state.retire_agent_tabs(&active.worktree.path);
            Box::new(DiscardCheckout {
                checkout: checkout(&active.worktree),
                retirements,
                settlement,
                active: Box::new(active),
                run_id,
            })
        })
    }

    /// Write down an abandon whose git has returned: the run's verdict, the
    /// stages the removal made unverifiable, and the Issue's lineage.
    ///
    /// The run comes back on the board here whichever way the rest goes —
    /// `answer_run_mutation` is what puts it back — so nothing below can strand
    /// it.
    fn settle_abandoned_run(
        &mut self,
        abandoned: RunAbandoned,
        mut active: ActiveRun,
    ) -> Result<Value, String> {
        let RunAbandoned {
            run_id,
            project_id,
            issue_id,
            detail,
            published,
            ..
        } = abandoned;
        let branch = active.worktree.branch();
        let worktree_id = crate::worktree::external_worktree_id(&active.worktree.path);
        let verdict = self
            .orch_for(&project_id)
            .and_then(|orch| orch.abandon_run_keeping_checkout(&mut active).map_err(err));
        let affected_stages = reconcile_missing_run_worktree(&mut active, &published);
        if verdict.is_ok() {
            close_abandoned_run_conversations(&mut active);
        }
        let (view, persisted) = self.answer_run_mutation(run_id.clone(), active, detail);
        verdict?;
        persisted?;
        if let Some(issue_id) = issue_id {
            self.record_abandon_on_issue(
                &issue_id,
                &run_id,
                &branch,
                worktree_id,
                &affected_stages,
            )?;
        }
        Ok(view)
    }

    /// Tell the Issue this run was implementing what it lost: the branch it was
    /// on, and the stages whose commits the removal made unverifiable.
    fn record_abandon_on_issue(
        &mut self,
        issue_id: &str,
        run_id: &str,
        branch: &str,
        worktree_id: String,
        affected_stages: &[String],
    ) -> Result<(), String> {
        // Abandoning is deleting the branch with nothing merged out of it, so
        // the issue this was implementing comes back to the inbox — and its
        // conversation says which branch it lost and why.
        self.mirror_run_outcome_to_issue(
            run_id,
            issue_id,
            crate::thread::ThreadEventKind::Abandoned,
            abandoned_branch_summary(branch, "abandoned"),
        )?;
        let mut issue = self.take_plan(issue_id)?;
        let mut links = vec![
            crate::thread::ThreadLink::Implementation {
                issue_id: issue_id.to_string(),
                implementation_id: run_id.to_string(),
            },
            crate::thread::ThreadLink::Worktree { worktree_id },
        ];
        links.extend(
            issue
                .stages
                .iter()
                .filter(|stage| affected_stages.contains(&stage.id))
                .map(|stage| crate::thread::ThreadLink::IssueStage {
                    issue_id: issue_id.to_string(),
                    stage_id: stage.id.clone(),
                    path: stage.path.clone(),
                }),
        );
        issue.agents.sole_thread_mut().push_event_with_links(
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
        self.finish_plan_mutation(issue_id.to_string(), issue)?;
        Ok(())
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
        let checkout_path = active.worktree.path.clone();
        // A failed run still holds its worktree; deleting an adopted run's card
        // must never delete the user's files (delete removes the card, not the
        // worktree it was minted around).
        let prunes_checkout = checkout_path.exists() && !active.adopted;
        let title = active.run.goal.clone();
        // A run recovered after its repository was moved or deleted has no
        // project mapping at all, and that stale card is exactly what a delete
        // is for. There is then no orchestrator to prune with, so the delete
        // clears the card and leaves whatever is on disk alone.
        let project_id = self.entity_project.get(&run_id).cloned();
        let project = project_id
            .as_deref()
            .and_then(|id| self.orch_for(id).ok())
            .cloned();

        let settlement = Box::new(RunDeleted {
            run_id: run_id.clone(),
            project_id: project_id.clone(),
            checkout: checkout_path,
        });
        self.discard_run(
            run_id,
            project_id,
            title,
            move |worktree| match (prunes_checkout, project) {
                (true, Some(project)) => DiscardedCheckout::Pruned {
                    project,
                    worktree: worktree.clone(),
                },
                _ => DiscardedCheckout::Kept,
            },
            settlement,
        )
    }

    /// Forget every trace of a run whose record has been deleted. The map entry
    /// itself went in the decide phase; this is the bookkeeping beside it.
    fn forget_run(&mut self, run_id: &str) {
        self.entity_project.remove(run_id);
        self.entity_project_path.remove(run_id);
        self.entity_created_at.remove(run_id);
        self.entity_updated_at.remove(run_id);
        self.entity_state_changed_at.remove(run_id);
        self.entity_last_state.remove(run_id);
        self.run_files_changed_at.remove(run_id);
        self.invalidate_run_stat(run_id);
    }

    /// Mint a plan-less run around an existing checkout (`plan_id` None): one of
    /// the project's external worktrees (`worktree_id`), or its primary
    /// checkout (`primary: true`) — the repo root as a super-worktree.
    ///
    /// The primary checkout has exactly one owner per project, enforced here.
    /// External adoption can rely on a client-side latch because a worktree
    /// card is adopted from one place; the repo root is reachable from every
    /// reload and every second browser, and they must all converge on the run
    /// that already owns it.
    fn run_adopt(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let model_choice = model_choice_from(params, self.default_harness)?;
        let base = self.base_for(&project_id)?;
        let adopting_primary = params
            .get("primary")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let target = if adopting_primary {
            if let Some(run_id) = self.primary_run_of(&project_id) {
                return Ok(self.owning_run_view(&run_id, params));
            }
            AdoptionTarget::Primary {
                repo_path: self.repo_path_for(&project_id)?,
            }
        } else {
            let worktree_id = require_str(params, "worktree_id")?;
            if let Some(run_id) = self.run_owning_worktree_id(&project_id, &worktree_id) {
                return Ok(self.owning_run_view(&run_id, params));
            }
            AdoptionTarget::Card {
                worktree_id,
                excluded: self.bound_worktree_paths(),
            }
        };
        let checkout_id = target.checkout_id();
        // The repo root is reachable from every reload and every second
        // browser, and a card is adoptable from more than one surface. An asker
        // who arrives while the checkout is being taken over is told so, and
        // is handed no run id: the run that will carry it is not in the map
        // until the adoption's epilogue lands, and an adoption that fails never
        // mints it at all. The asker asks again — the same thing it does when
        // its own adopt outlived its timer — and by then the owner is real and
        // `primary_run_of` / `run_owning_worktree_id` above answer with it.
        let run_id = format!("run-{}", uuid::Uuid::new_v4());
        let row = target.reserve(
            run_id.clone(),
            &project_id,
            self.checkout_title(&project_id, &checkout_id),
        );
        if self.row_claiming(&row).is_some() {
            return Ok(json!({ "adopting": true }));
        }
        let project = self.orch_for(&project_id)?.clone();
        self.defer_lifecycle(
            row,
            Box::new(AdoptCheckout {
                project,
                project_id,
                base_branch: base,
                run_id,
                target,
                model_choice,
                detail: thread_detail(params),
            }),
        )
    }

    /// The view an adopting caller gets when the checkout it named already has
    /// an owner: that run, in full.
    fn owning_run_view(&self, run_id: &str, params: &Value) -> Value {
        let active = self
            .runs
            .get(run_id)
            .expect("the caller found this run by scanning the map");
        self.run_view(run_id, active, thread_detail(params), DigestScope::Detail)
    }

    /// What to call a checkout on the row standing in for it: the branch the
    /// last scan saw it on, or the project it belongs to when no card does.
    fn checkout_title(&self, project_id: &str, checkout_id: &str) -> String {
        self.external_scan_of(project_id)
            .and_then(|cache| {
                cache
                    .worktrees
                    .iter()
                    .find(|checkout| checkout.id == checkout_id)
            })
            .and_then(|checkout| checkout.branch.clone())
            .unwrap_or_else(|| self.project_name_by_id(project_id))
    }

    /// Finish a completed run through the same durable worktree archive path as
    /// a bare worktree's Done control. The run is removed from the active map
    /// only while the server resolves and executes the id-only finish request;
    /// a pre-mutation failure restores it for retry.
    fn run_finish(&mut self, params: &Value) -> Result<Value, String> {
        let run_id = require_str(params, "run_id")?;
        let action_name = require_str(params, "action")?;
        match self.plan_finish_run(&run_id, &action_name, FinishRequirement::CompletedWork)? {
            PlannedRunFinish::Settled(value) => Ok(value),
            PlannedRunFinish::Replay { archived, run } => self.apply_run_finish(run, Ok(archived)),
            PlannedRunFinish::Deferred { job, run } => {
                let epilogue = job.epilogue(FinishKind::Run(run));
                Ok(self.defer_finish(job, epilogue))
            }
        }
    }

    /// The shared Done path: check what this caller requires of the run, take
    /// it off the board, and hand its checkout to the finish drain. The run is
    /// held out of the active map only while the finish runs; a failure that
    /// left the checkout standing puts it back for retry.
    fn plan_finish_run(
        &mut self,
        run_id: &str,
        action_name: &str,
        requirement: FinishRequirement,
    ) -> Result<PlannedRunFinish, String> {
        let run_id = run_id.to_string();
        parse_worktree_finish_action(action_name)?;
        let project_id = self.project_of(&run_id)?;
        let active = self.runs.get(&run_id).ok_or("unknown run_id")?;
        // Finishing archives a worktree and then removes it. The primary
        // checkout is the repository itself: there is nothing to file away,
        // and everything to lose.
        if self.owns_primary_checkout(&run_id, active) {
            return Err(
                "run.finish: the primary checkout cannot be finished or archived — it is the \
                 repository, not a worktree to clean up"
                    .to_string(),
            );
        }
        match requirement {
            FinishRequirement::CompletedWork => {
                if !matches!(active.run.state, RunState::Review | RunState::Merged) {
                    return Err(format!(
                        "run.finish: run is {} — Done requires completed work",
                        run_state_str(&active.run.state)
                    ));
                }
            }
            FinishRequirement::Unconditional => {}
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
            let persisted = self.finish_run_mutation(run_id, active);
            persisted?;
            return Ok(PlannedRunFinish::Settled(json!({ "archived": true })));
        }

        let root = Self::canonical_root(&active.worktree.path);
        let worktree_id = crate::worktree::external_worktree_id(&root);
        let active = self.runs.remove(&run_id).expect("checked above");
        self.invalidate_run_stat(&run_id);
        self.retire_agent_tabs(&root);
        self.rescan_external_worktrees(&project_id);
        let epilogue = RunFinishEpilogue {
            run_id,
            project_id: project_id.clone(),
            active: Box::new(active),
            root,
        };
        // The run comes off the board FIRST: a checkout a run still owns is
        // excluded from the scan that has to find it, and a run whose checkout
        // is being deleted must answer no verbs meanwhile. A refused plan puts
        // it straight back.
        let planned = match self.plan_worktree_finish(&json!({
            "project_id": project_id,
            "worktree_id": worktree_id,
            "action": action_name,
        })) {
            Ok(planned) => planned,
            Err(error) => {
                let restored = self.apply_run_finish(epilogue, Err(error));
                return Err(restored.expect_err("a refused finish answers with its refusal"));
            }
        };
        Ok(match planned {
            PlannedFinish::Settled(archived) => PlannedRunFinish::Replay {
                archived,
                run: epilogue,
            },
            PlannedFinish::Deferred(job) => PlannedRunFinish::Deferred { job, run: epilogue },
        })
    }

    /// Retire the run whose checkout has just been finished — or put it back
    /// when the finish failed with the checkout still standing.
    fn apply_run_finish(
        &mut self,
        run: RunFinishEpilogue,
        archived: Result<Value, String>,
    ) -> Result<Value, String> {
        let RunFinishEpilogue {
            run_id,
            project_id,
            active,
            root,
        } = run;
        let mut active = *active;
        let archived_worktree = match archived {
            Ok(archived) => archived,
            Err(error) => {
                if root.exists() {
                    self.runs.insert(run_id.clone(), active);
                    self.note_worktree_gone(&project_id, &root);
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
            let persisted = self.finish_run_mutation(run_id, active);
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
        self.run_files_changed_at.remove(&run_id);
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
        let project_id = self.entity_project.get(&run_id).cloned();
        let active = self.runs.remove(&run_id).expect("checked above");
        // Un-adopting hands the worktree back to the human; Build's agent in it
        // reported `done` to a run that no longer exists, so it goes with the
        // run. Reopening the Agent tab there adopts again. The receipts are
        // dropped: the tabs left the registry, which is what makes the agents
        // unaddressable, and nothing here is waiting to delete a directory.
        self.retire_agent_tabs(&active.worktree.path);
        self.forget_run(&run_id);
        // Build touches no disk here, so the checkout it hands back is
        // described by the scan this claims rather than by an amendment.
        if let Some(project_id) = project_id {
            self.rescan_external_worktrees(&project_id);
        }
        self.reap_orphaned_terminals();
        Ok(json!({ "ok": true }))
    }

    // ---- Board + views --------------------------------------------------------

    /// The board: plans + runs (each run carries a live diffstat), plus the
    /// ride-along external-worktree and primary-changes summaries. Sweeps runs
    /// whose worktree was deleted out of band into `archived` first.
    fn board_list(&mut self) -> Value {
        self.sweep_vanished_runs();
        let plans: Vec<Value> = {
            let ids: Vec<String> = self.plans.keys().cloned().collect();
            ids.into_iter()
                .filter(|id| self.plans[id].plan.archived_at.is_none())
                .map(|id| {
                    let active = self.plans.get(&id).expect("listed above");
                    self.plan_view(&id, active, ThreadDetail::Digest, DigestScope::List)
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
                    let stat = self.run_stat(&id).unwrap_or(Value::Null);
                    let active = self.runs.get(&id).expect("listed above");
                    let mut view =
                        self.run_view(&id, active, ThreadDetail::Digest, DigestScope::List);
                    view.as_object_mut()
                        .expect("run_view returns an object")
                        .insert("stat".to_string(), stat);
                    view
                })
                .collect()
        };
        let checkouts = self.external_worktrees_json();
        let primary_changes = self.primary_changes_json();
        // The inbox is in-flight work the user started in Build, nothing else:
        // a branch Build never cut or adopted (no run behind it, and it is not
        // the project's own primary checkout) earns no row here, ever — not
        // while an agent happens to be active in it, not on a fresh commit.
        // Adopting it (or sending it a first message, which adopts on the way)
        // is what brings it in; from there its row leaves the same way every
        // other row does — Done, deleted, or dismissed — never on its own.
        // `branch.get` still resolves it directly (deep-linking); this filter
        // is the feed list's alone.
        let items: Vec<Value> = self
            .work_items(&checkouts.rows, &primary_changes)
            .into_iter()
            .filter(|row| {
                row["kind"] != crate::branch::WorkItemKind::Branch.as_str()
                    || !row["run_id"].is_null()
                    || row["primary"] == json!(true)
            })
            .collect();
        json!({
            // The feed: one row per work item, branches and issues (Decisions
            // §Entity model). The keys below it are the same state told the way
            // the pre-redesign SPA reads it, and keep shipping until it stops.
            "items": items,
            "issues": plans,
            "plans": plans,
            "runs": runs,
            "external_worktrees": checkouts.rows,
            // Lifecycle verbs whose git is running right now. A checkout being
            // cut is on the board from the moment it is asked for, under the id
            // it will settle as.
            "pending": self.pending_rows_json(),
            // The rail has not finished looking. An empty list under this flag
            // is a board still working, not a project with no checkouts, and
            // the scan that lands invalidates the board so the client asks
            // again.
            "scanning": checkouts.scanning,
            "primary_changes": primary_changes,
        })
    }

    /// The feed's work items: one row per branch or issue.
    ///
    /// Branch rows fold the four ways a branch can be stored — a run, an
    /// adopted worktree, a worktree Build never cut, the primary checkout —
    /// into one shape keyed `(project_id, branch)`, and the primary checkout is
    /// the `main` row. An issue whose implementation is still in flight is
    /// spoken for by that implementation's branch row and emits none of its
    /// own. Both rules live in [`crate::branch`].
    ///
    /// Every branch a worktree names resolves here, whether or not it belongs
    /// on the INBOX list — `branch.get` (deep-linking) and `board_list`'s
    /// `items` both read this, and only the latter additionally filters out a
    /// worktree Build never adopted with no agent presently in it (see
    /// `board_list`). This function stays the unfiltered source of truth for
    /// "what branch is this," not "what does the inbox show."
    ///
    /// The scans are passed in rather than taken again: `board_list` already
    /// paid for them, and re-running them here would double every poll's git
    /// work.
    fn work_items(
        &mut self,
        external_worktrees: &[Value],
        primary_changes: &[Value],
    ) -> Vec<Value> {
        self.observe_conversationless_rows(external_worktrees, primary_changes);
        self.reconcile_crossed_dismissal_lines();
        let run_ids: Vec<String> = self
            .runs
            .iter()
            .filter(|(_, active)| active.run.state != RunState::Archived)
            .map(|(id, _)| id.clone())
            .collect();
        // Diffstats first: they are the one part of a row that needs `&mut`.
        let stats: HashMap<String, Value> = run_ids
            .iter()
            .filter_map(|run_id| Some((run_id.clone(), self.run_stat(run_id)?)))
            .collect();
        let mut candidates: Vec<crate::branch::WorkItemCandidate> = run_ids
            .iter()
            .map(|run_id| {
                self.branch_candidate_from_run(run_id, stats.get(run_id).unwrap_or(&Value::Null))
            })
            .collect();
        candidates.extend(
            primary_changes
                .iter()
                .filter_map(|entry| self.branch_candidate_from_primary(entry)),
        );
        candidates.extend(
            external_worktrees
                .iter()
                .map(|entry| self.branch_candidate_from_external(entry)),
        );
        let issue_ids: Vec<String> = self
            .plans
            .iter()
            .filter(|(_, active)| active.plan.archived_at.is_none())
            .map(|(id, _)| id.clone())
            .collect();
        candidates.extend(
            issue_ids
                .iter()
                .map(|issue_id| self.issue_candidate(issue_id)),
        );
        candidates.extend(self.capture_candidates());
        let mut items = crate::branch::fold_work_items(candidates);
        // The inbox reads oldest first, and the order it reads in is decided
        // here rather than by every client that renders it.
        crate::branch::sort_by_anchor(&mut items);
        items
    }

    fn reconcile_crossed_dismissal_lines(&mut self) {
        let entity_ids: Vec<String> = self.plans.keys().chain(self.runs.keys()).cloned().collect();
        let crossed: Vec<String> = entity_ids
            .into_iter()
            .filter(|id| {
                let Some(attention) = self.attention.get(id) else {
                    return false;
                };
                attention.dismissal_tracks_messages
                    && self.dismissal_lines(id).iter().enumerate().any(
                        |(position, (agent_id, sequence))| match attention
                            .dismissed_line_for(agent_id, position == 0)
                        {
                            Some(line) => *sequence > line,
                            None => *sequence > 0 && attention.has_message_dismissal(),
                        },
                    )
            })
            .collect();
        if crossed.is_empty() {
            return;
        }
        for id in crossed {
            if let Some(attention) = self.attention.get_mut(&id) {
                attention.invalidate_dismissal();
            }
        }
        self.persist_attention();
    }

    fn observe_conversationless_rows(
        &mut self,
        external_worktrees: &[Value],
        primary_changes: &[Value],
    ) {
        let now = now_rfc3339();
        let primary_keys = primary_changes.iter().filter_map(|entry| {
            entry["project_id"]
                .as_str()
                .map(crate::attention::primary_row_key)
        });
        let external_keys = external_worktrees.iter().filter_map(|entry| {
            let project_id = entry["project_id"].as_str()?;
            Some(match entry["branch"].as_str() {
                Some(branch) => crate::attention::branch_row_key(project_id, branch),
                None => entry["worktree_id"].as_str()?.to_string(),
            })
        });
        let mut changed = false;
        for key in primary_keys.chain(external_keys) {
            changed |= self.attention.entry(key).or_default().observe(&now);
        }
        if changed {
            self.persist_attention();
        }
    }

    /// The branch row for a run: the source that knows the most, because it is
    /// the only one that carries a lifecycle, a conversation and agents.
    fn branch_candidate_from_run(
        &self,
        run_id: &str,
        stat: &Value,
    ) -> crate::branch::WorkItemCandidate {
        let active = self.runs.get(run_id).expect("caller listed this run");
        let branch = active.worktree.branch();
        let issue_id = active.run.plan_id.as_ref().map(|id| id.0.clone());
        let sync = WorkItemStat::from_run_stat(stat);
        let thread = self.conversation_thread_for_run(active);
        let unread = self.unread_for(run_id, thread);
        let working = self.entity_agents_working(run_id);
        let working_since = working
            .then(|| self.working_since_for(run_id, thread))
            .flatten();
        let primary = self.owns_primary_checkout(run_id, active);
        let title = if active.run.goal.trim().is_empty() {
            branch.clone()
        } else {
            active.run.goal.clone()
        };
        let row = json!({
            "kind": crate::branch::WorkItemKind::Branch.as_str(),
            "project_id": self.entity_project.get(run_id).cloned().unwrap_or_default(),
            "project": self.project_name_of(run_id),
            "branch": branch,
            "title": title,
            "state": run_state_str(&active.run.state),
            "unread": unread.is_unread(),
            "unread_count": unread.count,
            "unread_reason": unread.reason,
            "working": working,
            "working_time": working_time_json(working_since.as_deref()),
            "agents": self.agent_digests(run_id, DigestScope::List),
            "stat": sync.to_json(),
            "resume_at": self.attention_json(run_id)["resume_at"],
            // Where this row sits in the inbox, and how long it has been quiet.
            // Every row carries both, whatever it was read off.
            "anchor": self.anchor_of(run_id),
            "last_activity": self.last_activity_of(Some(run_id), thread),
            // Done deletes the branch and its records. It is offered whenever
            // there is something to delete: the primary checkout is the
            // repository, so there is nothing to file away and everything to
            // lose. What the deletion would cost is `finish.warnings`, which
            // the client confirms through — never a refusal here.
            "can_finish": !primary,
            "finish": { "warnings": if primary {
                json!([])
            } else {
                sync.finish_warnings_json(&active.worktree.branch())
            } },
            "muted": self.is_muted(run_id),
            // Cleared out of the inbox until the work speaks again. The client
            // hides the row on it; nothing here changes because of it.
            "dismissed": self.is_dismissed(run_id),
            "worktree_path": active.worktree.path.display().to_string(),
            "worktree_id": crate::worktree::external_worktree_id(&Self::canonical_root(&active.worktree.path)),
            "run_id": run_id,
            "issue_id": issue_id,
            "primary": primary,
        });
        crate::branch::WorkItemCandidate {
            kind: crate::branch::WorkItemKind::Branch,
            key: crate::branch::WorkItemKey::Branch {
                project_id: self.entity_project.get(run_id).cloned().unwrap_or_default(),
                branch: active.worktree.branch(),
            },
            source: Some(crate::branch::BranchSource::Run),
            issue_id: active.run.plan_id.as_ref().map(|id| id.0.clone()),
            implementation_active: !active.run.state.is_terminal(),
            row,
        }
    }

    /// The branch row for a project's primary checkout — the `main` row.
    /// `None` when the checkout has no branch to name it by.
    fn branch_candidate_from_primary(
        &self,
        entry: &Value,
    ) -> Option<crate::branch::WorkItemCandidate> {
        let project_id = entry["project_id"].as_str()?.to_string();
        let branch = entry["branch"].as_str()?.to_string();
        let project = self.project(&project_id)?;
        let repo_path = project.repo_path.display().to_string();
        let sync = WorkItemStat::from_primary_entry(entry);
        let row = json!({
            "kind": crate::branch::WorkItemKind::Branch.as_str(),
            "project_id": project_id,
            "project": project.name,
            "branch": branch,
            "title": branch,
            "state": CHECKOUT_IDLE_STATE,
            "unread": false,
            "unread_count": 0,
            "unread_reason": Value::Null,
            "working": self.checkout_agent_working(&project.repo_path),
            "working_time": Value::Null,
            "agents": Vec::<Value>::new(),
            "stat": sync.to_json(),
            "resume_at": Value::Null,
            // A checkout with no run behind it has no record to anchor: it
            // dates itself by its own last commit, which is the only history it
            // has. Same for its last activity, plus whatever its agent painted.
            "anchor": sync.head_committed_at,
            "last_activity": self.attention
                .get(&crate::attention::primary_row_key(&project_id))
                .and_then(|attention| attention.first_observed_at.clone()),
            // The repository is not a worktree to file away.
            "can_finish": false,
            "finish": { "warnings": [] },
            "muted": false,
            // Cleared out of the inbox until the project has something new to
            // say. The repository holds no conversation to fall quiet, so the
            // line is drawn at the commit the checkout was cleared on.
            "dismissed": self.row_is_dismissed(
                &crate::attention::primary_row_key(&project_id),
                entry["head_sha"].as_str(),
            ),
            "worktree_path": repo_path,
            "worktree_id": Value::Null,
            "run_id": Value::Null,
            "issue_id": Value::Null,
            "primary": true,
        });
        Some(crate::branch::WorkItemCandidate {
            kind: crate::branch::WorkItemKind::Branch,
            key: crate::branch::WorkItemKey::Branch { project_id, branch },
            source: Some(crate::branch::BranchSource::PrimaryCheckout),
            issue_id: None,
            implementation_active: false,
            row,
        })
    }

    /// The branch row for a worktree Build never cut: everything git can see
    /// about it, and nothing else — it has no run, so it has no agents and no
    /// conversation.
    fn branch_candidate_from_external(&self, entry: &Value) -> crate::branch::WorkItemCandidate {
        let project_id = entry["project_id"].as_str().unwrap_or_default().to_string();
        let worktree_id = entry["worktree_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let branch = entry["branch"].as_str().map(str::to_string);
        let path = entry["path"].as_str().unwrap_or_default().to_string();
        let sync = WorkItemStat::from_external_entry(entry);
        let title = match (entry["head_subject"].as_str(), branch.as_deref()) {
            (Some(subject), _) if !subject.trim().is_empty() => subject.to_string(),
            (_, Some(branch)) => branch.to_string(),
            _ => path.clone(),
        };
        let row = json!({
            "kind": crate::branch::WorkItemKind::Branch.as_str(),
            "project_id": project_id,
            "project": entry["project"],
            "branch": branch,
            "title": title,
            "state": CHECKOUT_IDLE_STATE,
            "unread": false,
            "unread_count": 0,
            "unread_reason": Value::Null,
            "working": entry["agent_working"].as_bool().unwrap_or(false),
            "working_time": Value::Null,
            "agents": Vec::<Value>::new(),
            "stat": sync.to_json(),
            "resume_at": entry["attention"]["resume_at"],
            // See the primary row: a bare checkout is dated by its own commits,
            // unless the user has acted on it here and given it an anchor.
            "anchor": self
                .attention
                .get(&worktree_id)
                .and_then(|attention| attention.anchor_at.clone())
                .or_else(|| sync.head_committed_at.clone()),
            "last_activity": self.attention
                .get(&match &branch {
                    Some(branch) => crate::attention::branch_row_key(&project_id, branch),
                    None => worktree_id.clone(),
                })
                .and_then(|attention| attention.first_observed_at.clone()),
            "can_finish": true,
            "finish": { "warnings": sync.finish_warnings_json(
                branch.as_deref().unwrap_or("this checkout"),
            ) },
            "muted": self.is_muted(&worktree_id),
            // A checkout has no conversation, so git changes cannot revive a
            // clear. Adoption turns it into a run that can speak for itself.
            "dismissed": self.row_is_dismissed(
                &match &branch {
                    Some(branch) => crate::attention::branch_row_key(&project_id, branch),
                    None => worktree_id.clone(),
                },
                entry["head_sha"].as_str(),
            ),
            "worktree_path": path,
            "worktree_id": worktree_id.clone(),
            "run_id": Value::Null,
            "issue_id": Value::Null,
            "primary": false,
        });
        crate::branch::WorkItemCandidate {
            kind: crate::branch::WorkItemKind::Branch,
            key: match &branch {
                Some(branch) => crate::branch::WorkItemKey::Branch {
                    project_id,
                    branch: branch.clone(),
                },
                None => crate::branch::WorkItemKey::Checkout { worktree_id },
            },
            source: Some(crate::branch::BranchSource::ExternalWorktree),
            issue_id: None,
            implementation_active: false,
            row,
        }
    }

    /// The row for an issue: a project-level work item, with no branch and no
    /// checkout of its own until it is implemented.
    fn issue_candidate(&self, issue_id: &str) -> crate::branch::WorkItemCandidate {
        let active = self.plans.get(issue_id).expect("caller listed this issue");
        let conversation = &active.agents.sole_thread();
        let unread = self.unread_for(issue_id, Some(conversation));
        let working = self.entity_agents_working(issue_id);
        let working_since = working
            .then(|| self.working_since_for(issue_id, Some(conversation)))
            .flatten();
        let implementation = self.current_issue_implementation(issue_id);
        // The implementation still in flight, which is narrower than the newest
        // one: a merged or abandoned branch has stopped speaking for its issue,
        // and the issue is back in the inbox on its own.
        let live_implementation = implementation.filter(|run| !run.run.state.is_terminal());
        let execution_context = self.issue_execution_context(issue_id);
        let row = json!({
            "kind": crate::branch::WorkItemKind::Issue.as_str(),
            "project_id": self.entity_project.get(issue_id).cloned().unwrap_or_default(),
            "project": self.project_name_of(issue_id),
            "branch": Value::Null,
            "title": active.plan.goal,
            "state": plan_state_str(&active.plan.state),
            "unread": unread.is_unread(),
            "unread_count": unread.count,
            "unread_reason": unread.reason,
            "working": working,
            "working_time": working_time_json(working_since.as_deref()),
            "agents": self.agent_digests(issue_id, DigestScope::List),
            "execution_context": execution_context,
            "stat": Value::Null,
            "resume_at": self.attention_json(issue_id)["resume_at"],
            "anchor": self.anchor_of(issue_id),
            // An issue has no checkout and no commits of its own: its
            // conversation is the whole of its activity.
            "last_activity": self.last_activity_of(Some(issue_id), Some(active.agents.sole_thread())),
            // Done on an issue archives it, and archiving is never refused.
            // What it costs — an issue nothing was ever built for — is a
            // warning the client confirms through.
            "can_finish": true,
            "finish": { "warnings": crate::branch::warnings_json(
                &crate::branch::issue_finish_warnings(implementation.is_some()),
            ) },
            "muted": self.is_muted(issue_id),
            // See the branch row: dismissed until its conversation asks again.
            "dismissed": self.is_dismissed(issue_id),
            "worktree_path": Value::Null,
            "worktree_id": Value::Null,
            "run_id": implementation.map(|run| run.run.id.0.clone()),
            "issue_id": issue_id,
            // Whether a branch is implementing this issue RIGHT NOW — the same
            // fact that hides the issue's row behind that branch's, said out
            // loud so a surface holding an issue can explain where it went.
            "implementing_branch": live_implementation.map(|run| run.worktree.branch()),
            "implementation_active": live_implementation.is_some(),
            "primary": false,
        });
        crate::branch::WorkItemCandidate {
            kind: crate::branch::WorkItemKind::Issue,
            key: crate::branch::WorkItemKey::Issue {
                issue_id: issue_id.to_string(),
            },
            source: None,
            issue_id: Some(issue_id.to_string()),
            implementation_active: false,
            row,
        }
    }

    fn is_muted(&self, entity_id: &str) -> bool {
        self.attention
            .get(entity_id)
            .is_some_and(|attention| attention.muted)
    }

    /// Whether this row was cleared and no user or agent has spoken on any of
    /// its conversations since.
    ///
    /// Every agent has to still be cleared, which is the complement of the
    /// badge above it: `unread_for` unions unread across the roster, so a row
    /// is out of the list only while nothing anywhere on it has spoken past
    /// the line the human drew.
    ///
    fn is_dismissed(&self, entity_id: &str) -> bool {
        let Some(attention) = self.attention.get(entity_id) else {
            return false;
        };
        let lines = self.dismissal_lines(entity_id);
        // A row with no roster behind it holds no conversation to have been
        // cleared: it is dismissed at a commit instead, by `row_is_dismissed`.
        attention.has_message_dismissal()
            && (lines.is_empty()
                || lines.iter().enumerate().all(
                    |(position, (agent_id, latest_attention_sequence))| {
                        attention.is_dismissed_for(
                            agent_id,
                            position == 0,
                            *latest_attention_sequence,
                        ) || (*latest_attention_sequence == 0 && attention.has_message_dismissal())
                    },
                ))
    }

    /// When this work item's oldest turn still in flight started — how long the
    /// ITEM has been working, rather than how long its newest agent has.
    fn working_since_for(
        &self,
        entity_id: &str,
        thread: Option<&crate::thread::Thread>,
    ) -> Option<String> {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return thread
                .and_then(|thread| thread.working_since())
                .map(str::to_string);
        };
        roster
            .iter()
            .filter_map(|agent| agent.working_since.as_deref())
            .min()
            .map(str::to_string)
    }

    /// Whether a Build-owned agent is painting in this checkout right now. The
    /// tab registry is the only place an agent can be, and its key is the
    /// checkout root, so a checkout reports its own agent whatever entity (or
    /// none) currently owns it.
    fn checkout_agent_working(&self, root: &std::path::Path) -> bool {
        let root = Self::canonical_root(root);
        self.tabs
            .iter()
            .any(|(key, tab)| key.root == root && key.is_agent() && agent_is_working(tab))
    }

    fn entity_agents_working(&self, entity_id: &str) -> bool {
        let Ok(roster) = self.entity_agents(entity_id) else {
            return false;
        };
        roster.iter().any(|agent| agent.working_since.is_some())
    }

    /// `branch.get` — resolve `(project_id, branch)` to the work item behind
    /// it, with the full underlying run view (`run_view`) when a run owns the
    /// branch and `run: null` when the checkout is bare.
    fn branch_get(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let branch = require_str(params, "branch")?;
        if !self.projects.iter().any(|p| p.id == project_id) {
            return Err(format!("unknown project_id: {project_id}"));
        }
        let checkouts = self.external_worktrees_json();
        let primary_changes = self.primary_changes_json();
        let found = self
            .work_items(&checkouts.rows, &primary_changes)
            .into_iter()
            .find(|row| {
                row["kind"] == crate::branch::WorkItemKind::Branch.as_str()
                    && row["project_id"] == json!(project_id)
                    && row["branch"] == json!(branch)
            });
        let Some(mut row) = found else {
            // This project's own scan, not the rail's board-wide flag: what a
            // neighbour has or has not been scanned for says nothing about the
            // branch that was asked for here.
            let settled = self.scan_settled_at(&project_id).is_some();
            self.rescan_external_worktrees(&project_id);
            return Err(format!(
                "branch.get: no checkout of this project is on branch {branch} ({})",
                scan_may_yet_show_it(settled)
            ));
        };
        let run = match row["run_id"].as_str().map(str::to_string) {
            Some(run_id) => {
                let active = self.runs.get(&run_id).expect("the row named a live run");
                // The branch surface sits under the rail: it reads the
                // conversation of whichever agent's bubble is open. See
                // `run_get`.
                let detail_thread = self.detail_thread_value(&run_id, params)?;
                let mut view = self.run_view(
                    &run_id,
                    active,
                    view_thread_detail(&detail_thread, params),
                    DigestScope::Detail,
                );
                if let Some(thread) = detail_thread {
                    view["thread"] = thread;
                }
                view
            }
            // A checkout Build owns no run in has no agent to name.
            None => match named_agent_id(params)? {
                Some(agent_id) => return Err(format!("unknown agent_id: {agent_id}")),
                None => Value::Null,
            },
        };
        if let Some(run_agents) = run.get("agents") {
            row["agents"] = run_agents.clone();
        }
        row["run"] = run;
        Ok(row)
    }

    /// `branch.finish` — the inbox entry's Done, for a branch.
    ///
    /// Done on a branch DELETES it: the checkout goes through the same durable
    /// path as `worktree.finish`, the branch goes with it, and the run's
    /// records and conversation leave the inbox. It is never refused for the
    /// state of the work — an unpushed commit, an unmerged branch and an
    /// uncommitted edit are warnings the row carries (`finish.warnings`) and
    /// the user confirms through. `action` chooses how the checkout is retired
    /// and defaults to `delete`, which is what Done means.
    ///
    /// An issue the branch implements only ends with it when the work landed:
    /// a merge finishes the issue too, and any other ending hands the issue
    /// back to the inbox with an event naming the branch it lost.
    /// `unlink: true` leaves the issue alone either way.
    fn branch_finish(&mut self, params: &Value) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        let branch = require_str(params, "branch")?;
        let action_name = params
            .get("action")
            .and_then(Value::as_str)
            .filter(|action| !action.is_empty())
            .unwrap_or("delete")
            .to_string();
        parse_worktree_finish_action(&action_name)?;
        let unlink = params
            .get("unlink")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let Some(run_id) = self.run_on_branch(&project_id, &branch) else {
            // No run behind the branch: it is a bare checkout, and the durable
            // archive path is the same one `run.finish` delegates to.
            //
            // The last scan is what maps the branch to a checkout id. Resolving
            // it is not the authority for what gets deleted — the job rescans
            // and re-resolves the id itself — so a stale hit fails closed there
            // rather than costing every other frame a scan under this lock.
            let worktree = self.find_checkout(
                &project_id,
                &format!("branch.finish: no checkout of this project is on branch {branch}"),
                |checkout| checkout.branch.as_deref() == Some(branch.as_str()),
            )?;
            let planned = self.plan_worktree_finish(&json!({
                "project_id": project_id,
                "worktree_id": worktree.id,
                "action": action_name,
            }))?;
            let epilogue = BranchFinishEpilogue {
                branch,
                run: None,
                issue_id: None,
                orphaned_issue_id: None,
            };
            return Ok(match planned {
                PlannedFinish::Settled(archived) => {
                    self.apply_branch_finish(epilogue, Ok(archived))?
                }
                PlannedFinish::Deferred(job) => {
                    let epilogue = job.epilogue(FinishKind::Branch(epilogue));
                    self.defer_finish(job, epilogue)
                }
            });
        };
        let implemented_issue_id = self.runs[&run_id]
            .run
            .plan_id
            .as_ref()
            .map(|id| id.0.clone())
            // An issue already filed away has nothing left to hear about this.
            .filter(|issue_id| {
                self.plans
                    .get(issue_id)
                    .is_some_and(|issue| issue.plan.archived_at.is_none())
            });
        // Whether the work landed. That is the whole question an issue's fate
        // turns on: a merge publishes it into the base branch and the issue is
        // done with the branch; anything else deletes work the issue was
        // waiting for, so the issue comes back to the inbox and has to be told
        // what happened to the branch that was speaking for it.
        let merged = matches!(
            parse_worktree_finish_action(&action_name),
            Ok(WorktreeFinishAction::Merge)
        ) || self.runs[&run_id].run.state == RunState::Merged;
        let issue_id = implemented_issue_id.clone().filter(|_| !unlink && merged);
        let orphaned_issue_id = implemented_issue_id.filter(|_| !merged);
        let planned =
            self.plan_finish_run(&run_id, &action_name, FinishRequirement::Unconditional)?;
        let epilogue = |run| BranchFinishEpilogue {
            branch,
            run,
            issue_id,
            orphaned_issue_id,
        };
        match planned {
            // The checkout was already gone: there is no branch left to finish
            // and no issue news to file, only the run's own retirement.
            PlannedRunFinish::Settled(archived) => {
                self.apply_branch_finish(epilogue(None), Ok(archived))
            }
            PlannedRunFinish::Replay { archived, run } => {
                self.apply_branch_finish(epilogue(Some(run)), Ok(archived))
            }
            PlannedRunFinish::Deferred { job, run } => {
                let epilogue = job.epilogue(FinishKind::Branch(epilogue(Some(run))));
                Ok(self.defer_finish(job, epilogue))
            }
        }
    }

    /// Settle the issue behind a finished branch, once the branch is actually
    /// gone: a merge files the issue away with it, anything else hands the
    /// issue back to the inbox with an event naming the branch it lost.
    fn apply_branch_finish(
        &mut self,
        epilogue: BranchFinishEpilogue,
        archived: Result<Value, String>,
    ) -> Result<Value, String> {
        let BranchFinishEpilogue {
            branch,
            run,
            issue_id,
            orphaned_issue_id,
        } = epilogue;
        let run_id = run.as_ref().map(|run| run.run_id.clone());
        let finished = match run {
            Some(run) => self.apply_run_finish(run, archived)?,
            None => archived?,
        };
        if let (Some(orphaned_issue_id), Some(run_id)) = (&orphaned_issue_id, &run_id) {
            self.note_implementation_abandoned(
                orphaned_issue_id,
                run_id,
                &branch,
                "finished off the board",
            );
        }
        let issue_archived = match &issue_id {
            Some(issue_id) => {
                self.plan_archive(&json!({ "plan_id": issue_id }))?;
                true
            }
            None => false,
        };
        Ok(json!({
            "branch": branch,
            "run_id": run_id,
            "issue_id": issue_id,
            "issue_archived": issue_archived,
            "issue_abandoned": orphaned_issue_id.is_some(),
            "worktree": finished,
        }))
    }

    /// `branch.dispatch` — one call from "here is what I want done" to an agent
    /// doing it (Decisions §Capture and router, "One-call dispatch").
    ///
    /// The router decides a destination and then has to reach it, and reaching
    /// it is four mutations that each create something: cut or find the
    /// checkout, take ownership of it, put an agent on it, hand that agent the
    /// words. A caller driving those one at a time owns the unwinding when the
    /// third fails — and the router is an agent, which is the worst possible
    /// owner for a half-built branch. So the four are one verb, and the verb
    /// owns the unwinding.
    ///
    /// `branch` names where the work goes; with none, the instruction names the
    /// branch it cuts. The agent is always brand new: an instruction is never
    /// dropped into a conversation someone else is having.
    fn branch_dispatch(&mut self, params: &Value) -> Result<Value, String> {
        self.dispatch_branch(params, None)
    }

    /// `branch.dispatch`, and the capture it is the destination of when a route
    /// is what asked for it. Every dispatch runs its git through the drain: a
    /// router reaching a branch is the same verb as a browser dispatching one.
    fn dispatch_branch(
        &mut self,
        params: &Value,
        routed: Option<RoutedCapture>,
    ) -> Result<Value, String> {
        let project_id = require_str(params, "project_id")?;
        if !self.projects.iter().any(|project| project.id == project_id) {
            return Err(format!("branch.dispatch: unknown project_id: {project_id}"));
        }
        let instruction = require_str(params, "instruction")?;
        if instruction.trim().is_empty() {
            return Err(
                "branch.dispatch: the instruction is empty — there is nothing to dispatch"
                    .to_string(),
            );
        }
        let branch = params
            .get("branch")
            .and_then(Value::as_str)
            .filter(|branch| !branch.is_empty())
            .map(str::to_string);
        // The provider is parsed before anything is created, so an unrunnable
        // one refuses instead of leaving a branch nothing can work on.
        let requested_choice = model_choice_from(params, self.default_harness)?;
        // The ref, before anything is reserved or cut: it is what one dispatch
        // reserves against another, and deriving it in the git phase left two
        // calls on one instruction racing into `git worktree add`.
        let target = DispatchTarget::of(branch.as_deref(), &instruction)?;

        let mutation = DispatchCheckout {
            project: self.orch_for(&project_id)?.clone(),
            base_branch: self.base_for(&project_id)?,
            checkouts: self.project_checkouts(&project_id)?,
            project_id: project_id.clone(),
            run_id: format!("run-{}", uuid::Uuid::new_v4()),
            target,
            instruction: instruction.clone(),
            model_choice: requested_choice,
            explicit_choice: has_agent_choice(params),
            routed,
            resolved: self.resolved_isolation(&project_id),
            #[cfg(test)]
            fault: self.dispatch_fault,
        };
        let row = PendingRow::creating(
            mutation.run_id.clone(),
            Some(project_id),
            branch.unwrap_or(instruction),
        )
        .on_branch(mutation.target.branch().to_string())
        .isolated_as(mutation.resolved.isolation);
        self.defer_lifecycle(row, Box::new(mutation))
    }

    /// Open the run `branch.dispatch` just checkpointed a checkout for, and put
    /// its agent to work — the apply half of [`BranchDispatched`], and the only
    /// half that touches state.
    ///
    /// One store write, made after every decision: the git has already cut a
    /// branch, checked the repository out into it and written a checkpoint
    /// commit, so a second fallible step here would be a way to strand all of
    /// that under no run at all. The route a capture took to get here is one
    /// of those decisions, and is written first: nothing that can refuse sits
    /// on the far side of the write.
    fn open_dispatched_run(&mut self, dispatched: BranchDispatched) -> Result<Value, String> {
        let BranchDispatched {
            adopted,
            instruction,
            routed,
            checkouts,
            downgrade,
        } = dispatched;
        self.validate_checkout_snapshot(&adopted.project_id, &checkouts)?;
        #[cfg(test)]
        fail_dispatch_at(self.dispatch_fault, BranchDispatchStep::Open)?;
        let project_id = adopted.project_id.clone();
        let run_id = adopted.run_id.clone();
        let choice = adopted.model_choice.clone();
        let route =
            self.record_dispatch_route(routed, &project_id, &run_id, &adopted.checkout.branch)?;
        let mut active = adopted.open_run(self)?;
        let agent = self.dispatch_to_run(
            &run_id,
            &mut active,
            &instruction,
            choice,
            &adopted.checkout.branch,
            adopted.checkout.path.clone(),
        );
        if let Some(reason) = downgrade {
            self.note_isolation_downgrade(&run_id, &mut active, &reason);
        }
        #[cfg(test)]
        fail_dispatch_at(self.dispatch_fault, BranchDispatchStep::Settle)?;
        self.finish_run_mutation(run_id.clone(), active)?;
        self.touch_attention(&run_id);
        Ok(RouteRecorded::answer(
            route,
            agent.json(&project_id, &run_id),
        ))
    }

    /// `branch.dispatch` onto a branch Build already runs: the run is there,
    /// its checkout is there, and no git runs at all — so the run is taken,
    /// told, and put back under this one acquisition. The route is written
    /// before the run is taken, so a refused route leaves the run in its map.
    fn join_dispatched_run(
        &mut self,
        joined: BranchJoined,
        choice: ModelChoice,
    ) -> Result<Value, String> {
        let BranchJoined {
            project_id,
            run_id,
            branch,
            instruction,
            root,
            routed,
            ..
        } = joined;
        let route = self.record_dispatch_route(routed, &project_id, &run_id, &branch)?;
        let mut active = self.take_run(&run_id)?;
        let agent = self.dispatch_to_run(&run_id, &mut active, &instruction, choice, &branch, root);
        #[cfg(test)]
        fail_dispatch_at(self.dispatch_fault, BranchDispatchStep::Settle)?;
        self.finish_run_mutation(run_id.to_string(), active)?;
        self.touch_attention(&run_id);
        Ok(RouteRecorded::answer(
            route,
            agent.json(&project_id, &run_id),
        ))
    }

    /// Add the agent a dispatch speaks through to a run that has a checkout,
    /// and hand it the words. The half every dispatch shares — the branch Build
    /// already ran, and the one it has just taken ownership of.
    ///
    /// The run is mutated where its owner is holding it, and handed back
    /// unwritten: whoever took it out of the map is the one that puts it back,
    /// in the single write that settles the dispatch.
    fn dispatch_to_run(
        &mut self,
        run_id: &str,
        active: &mut ActiveRun,
        instruction: &str,
        choice: ModelChoice,
        branch: &str,
        root: std::path::PathBuf,
    ) -> DispatchedAgent {
        let now = now_rfc3339();
        // A dispatch always adds the agent it is about to speak to — an
        // adoption mints none, and a branch Build already runs keeps the agents
        // it has.
        let agent_id = active.agents.add(run_id, choice, &now).id.clone();
        let branch = branch.to_string();
        let agent = active
            .agents
            .resolve_mut(Some(&agent_id))
            .expect("the agent was just put on this roster");
        let model_choice = agent.choice.clone();
        let choice_revision = agent.choice_revision;
        let conversation_id = agent.conversation_id().to_string();
        agent.thread.post_user(instruction, None, &now);
        // Told the same way `agent.start` tells an agent what is waiting for
        // it: the instruction is already durable on the thread, so a warm
        // harness gets the read-your-messages nudge `thread.post` writes, and a
        // cold one gets that nudge wrapped in the packet it has no other way to
        // reconstruct. The spawn itself happens in `DeliveryRunner`, with the
        // state lock free and this frame already answered.
        self.pending_agent_turns.push(PendingAgentTurn {
            operation_id: None,
            root,
            owner: run_id.to_string(),
            agent_id: agent_id.clone(),
            conversation_id,
            model_choice,
            choice_revision,
            interrupt: false,
            say: Some(TurnText {
                cold: crate::orchestrator::conversation_prompt(NEW_THREAD_MESSAGES_PROMPT),
                warm: NEW_THREAD_MESSAGES_PROMPT.to_string(),
            }),
            phase: "dispatch",
            wants_catch_up: true,
            survives_refusal: false,
        });
        DispatchedAgent { branch, agent_id }
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
            .map(|(plan_id, active)| {
                self.plan_view(plan_id, active, ThreadDetail::Digest, DigestScope::List)
            })
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

    /// Everything finished, across every project, newest first: archived
    /// issues, archived runs, and the archived worktrees no run stands behind.
    ///
    /// The archive is the user's, not a project's, which is why this cannot be
    /// `archive.list` with the project left off — and it speaks the feed's two
    /// work items (Decisions §Entity model), so a finished run and the worktree
    /// record it left behind are ONE branch row. `(project, branch)` is the
    /// join: a deleted checkout's path no longer canonicalizes, so the worktree
    /// id cannot be recomputed from a run whose files are gone.
    fn archived_list(&self) -> Value {
        let mut items: Vec<Value> = self
            .plans
            .iter()
            .filter(|(_, active)| active.plan.archived_at.is_some())
            .map(|(issue_id, active)| {
                let mut row = self.archived_row("issue", issue_id);
                let object = row.as_object_mut().expect("archived_row is an object");
                object.insert("title".into(), json!(active.plan.goal));
                object.insert("state".into(), json!(plan_state_str(&active.plan.state)));
                object.insert("finished_at".into(), json!(active.plan.archived_at));
                object.insert("issue_id".into(), json!(issue_id));
                object.insert("stages".into(), json!(active.stages.len()));
                row
            })
            .collect();

        let mut branches_taken: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        for (run_id, active) in &self.runs {
            if active.run.state != RunState::Archived {
                continue;
            }
            let branch = active.worktree.branch();
            let project_path = self.project_path_for(run_id);
            let record = self.archived_worktrees.values().find(|record| {
                record.status == WorktreeFinishStatus::Archived
                    && record.project_path == project_path
                    && record.branch.as_deref() == Some(branch.as_str())
            });
            branches_taken.insert((project_path, branch.clone()));
            let mut row = self.archived_row("branch", run_id);
            let object = row.as_object_mut().expect("archived_row is an object");
            let title = if active.run.goal.trim().is_empty() {
                branch.clone()
            } else {
                active.run.goal.clone()
            };
            object.insert("title".into(), json!(title));
            object.insert("branch".into(), json!(branch));
            object.insert("state".into(), json!(run_state_str(&active.run.state)));
            object.insert(
                "finished_at".into(),
                json!(self.entity_state_changed_at.get(run_id)),
            );
            object.insert("run_id".into(), json!(run_id));
            object.insert(
                "issue_id".into(),
                json!(active.run.plan_id.as_ref().map(|id| id.0.clone())),
            );
            object.insert(
                "worktree_path".into(),
                json!(active.worktree.path.display().to_string()),
            );
            if let Some(record) = record {
                merge_archived_worktree_facts(object, record);
            }
            items.push(row);
        }

        for record in self.archived_worktrees.values() {
            if record.status != WorktreeFinishStatus::Archived {
                continue;
            }
            let branch = record.branch.clone();
            if branch.as_ref().is_some_and(|branch| {
                branches_taken.contains(&(record.project_path.clone(), branch.clone()))
            }) {
                continue;
            }
            let project = self
                .projects
                .iter()
                .find(|project| project.repo_path.display().to_string() == record.project_path);
            let mut row = json!({
                "kind": "branch",
                "project_id": project.map(|project| project.id.clone()),
                "project": project.map(|project| project.name.clone()),
                "title": branch.clone().unwrap_or_else(|| record.worktree_name.clone()),
                "branch": branch,
                "state": "archived",
                "finished_at": record.archived_at,
                "run_id": Value::Null,
                "issue_id": Value::Null,
                "stages": Value::Null,
                "worktree_path": record.worktree_path,
            });
            merge_archived_worktree_facts(row.as_object_mut().expect("built as an object"), record);
            items.push(row);
        }

        // Newest first; an item whose stamp was never written sorts last rather
        // than jumping the queue.
        items.sort_by(|left, right| {
            let stamp = |row: &Value| row["finished_at"].as_str().unwrap_or_default().to_string();
            stamp(right).cmp(&stamp(left))
        });
        json!({ "items": items })
    }

    /// The keys every archived row carries, with the entity's project already
    /// resolved. The caller fills in the rest for its kind.
    fn archived_row(&self, kind: &str, entity_id: &str) -> Value {
        json!({
            "kind": kind,
            "project_id": self.entity_project.get(entity_id),
            "project": self.project_name_of(entity_id),
            "title": Value::Null,
            "branch": Value::Null,
            "state": Value::Null,
            "action": Value::Null,
            "finished_at": Value::Null,
            "run_id": Value::Null,
            "issue_id": Value::Null,
            "stages": Value::Null,
            "worktree_id": Value::Null,
            "worktree_path": Value::Null,
            "head_sha": Value::Null,
            "upstream": Value::Null,
            "unpushed": Value::Null,
            "dirty_files": Value::Null,
        })
    }

    /// What one run's stages have to be judged against, taken under the lock so
    /// [`StagePublicationQuery::classify`] can ask git without it.
    fn stage_publication_query(&self, run_id: &str, active: &ActiveRun) -> StagePublicationQuery {
        StagePublicationQuery {
            run_id: run_id.to_string(),
            worktrees: self
                .entity_project
                .get(run_id)
                .and_then(|project_id| {
                    self.projects
                        .iter()
                        .find(|project| &project.id == project_id)
                })
                .map(|project| project.orch.worktrees().clone()),
            checkout: active.worktree.path.clone(),
            branch: active.worktree.branch(),
            base_branch: active.worktree.base_branch.clone(),
            completions: active
                .stages
                .iter()
                .filter_map(|progress| {
                    Some((progress.stage_id.clone(), progress.completion_sha.clone()?))
                })
                .collect(),
        }
    }

    /// Ask git about this run's stages here and now, with the state lock in
    /// hand. Boot's failed-recovery arm alone: it decides against refs that are
    /// about to be deleted, and runs before the first frame is served, so
    /// nothing waits on the mutex it holds. Every other caller — `run.abandon`,
    /// the vanished-run sweep — asks through [`StagePublicationQuery::classify`]
    /// in a lock-free run phase.
    fn classify_stages_now(&self, run_id: &str, active: &ActiveRun) -> StagePublications {
        self.stage_publication_query(run_id, active).classify()
    }

    /// Runs whose checkout vanished, on their way to Archived, decided off the
    /// state lock.
    ///
    /// Whether a stage's commits ever left this machine is a fetch and two
    /// graph walks per stage, and every board read used to pay for it inline.
    /// Now the board answers with the runs it still has and the sweep archives
    /// them behind it — the same bargain the scan and the diffstats make.
    /// Single-flight: a sweep already running absorbs the next poll's.
    fn sweep_vanished_runs(&mut self) {
        if self.vanished_run_sweep_in_flight {
            return;
        }
        let queries: Vec<StagePublicationQuery> = self
            .runs
            .iter()
            .filter(|(_, active)| {
                !active.run.state.is_terminal()
                    && active.run.state != RunState::Created
                    && active.recovery.is_none()
                    && !active.worktree.path.exists()
            })
            .map(|(run_id, active)| self.stage_publication_query(run_id, active))
            .collect();
        if queries.is_empty() {
            return;
        }
        self.vanished_run_sweep_in_flight = true;
        self.run_off_lock(VanishedRunSweep {
            queries,
            #[cfg(test)]
            gate: self.off_lock_gate.clone(),
        });
    }

    /// A run the user deletes must disappear from Build. Any live run whose
    /// worktree vanished retires to Archived: session ended, git's stale
    /// worktree record pruned — the record stays as quiet history. `Created` is
    /// exempt (its worktree may legitimately not exist yet).
    fn archive_vanished_runs(&mut self, decided: Vec<DecidedVanishedRun>) {
        self.vanished_run_sweep_in_flight = false;
        for DecidedVanishedRun { run_id, published } in decided {
            // The checkout may have come back, or the run may have been
            // abandoned outright, while the sweep was asking git.
            if self
                .runs
                .get(&run_id)
                .is_none_or(|active| active.worktree.path.exists())
            {
                continue;
            }
            let Ok(mut active) = self.take_run(&run_id) else {
                continue;
            };
            let issue_id = active.run.plan_id.as_ref().map(|id| id.0.clone());
            let branch = active.worktree.branch();
            let affected_stages = reconcile_missing_run_worktree(&mut active, &published);
            let worktree_id = crate::worktree::external_worktree_id(&active.worktree.path);
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
            let persisted = self.finish_run_mutation(run_id.clone(), active);
            if let Err(e) = persisted {
                eprintln!("archive {run_id}: {e}");
            }
            if let Some(issue_id) = issue_id {
                if let Ok(mut issue) = self.take_plan(&issue_id) {
                    issue.agents.sole_thread_mut().push_event_with_links(
                        crate::thread::ThreadEventKind::WorktreeDeleted,
                        Some(format!(
                            "Implementation worktree disappeared; {} stage(s) were reconciled",
                            affected_stages.len()
                        )),
                        None,
                        None,
                        vec![
                            crate::thread::ThreadLink::Implementation {
                                issue_id: issue_id.clone(),
                                implementation_id: run_id.clone(),
                            },
                            crate::thread::ThreadLink::Worktree {
                                worktree_id: worktree_id.clone(),
                            },
                        ],
                        now_rfc3339(),
                    );
                    for stage_id in &affected_stages {
                        if let Some(stage) = issue.stages.iter().find(|stage| &stage.id == stage_id)
                        {
                            issue.agents.sole_thread_mut().push_event_with_links(
                                crate::thread::ThreadEventKind::StageInvalidated,
                                Some(format!("Stage “{}” is incomplete", stage.title)),
                                None,
                                None,
                                vec![
                                    crate::thread::ThreadLink::IssueStage {
                                        issue_id: issue_id.clone(),
                                        stage_id: stage.id.clone(),
                                        path: stage.path.clone(),
                                    },
                                    crate::thread::ThreadLink::Implementation {
                                        issue_id: issue_id.clone(),
                                        implementation_id: run_id.clone(),
                                    },
                                ],
                                now_rfc3339(),
                            );
                        }
                    }
                    let persisted = self.finish_plan_mutation(issue_id.clone(), issue);
                    if let Err(e) = persisted {
                        eprintln!("archive {run_id}: issue event persist failed: {e}");
                    }
                    // The checkout went away under Build with nothing merged,
                    // so the issue is back in the inbox. Say which branch it
                    // lost, or its reappearance is unexplained.
                    self.note_implementation_abandoned(
                        &issue_id,
                        &run_id,
                        &branch,
                        "deleted outside Build",
                    );
                }
            }
        }
    }

    /// Clear every stale record of a checkout in the run's project — the
    /// sweep after one went away outside Build. Best effort, which is the
    /// façade's own policy for it: nothing the caller asked for depends on it.
    fn prune_worktree_records(&self, run_id: &str) {
        let Some(project) = self
            .entity_project
            .get(run_id)
            .and_then(|pid| self.projects.iter().find(|p| &p.id == pid))
        else {
            return;
        };
        project.orch.worktrees().prune();
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
            // The inbox's own key, on every surface that renders an entity, so
            // a detail view and the list it was opened from agree about where
            // this piece of work sits.
            "anchor": attention.anchor(&created_at),
        })
    }

    /// Where this entity sits in the inbox.
    ///
    /// The anchor, or — for a record written before anchors that boot has not
    /// reached — the day it was created, which is what the seed would have
    /// made it.
    fn anchor_of(&self, entity_id: &str) -> String {
        let created_at = self
            .entity_created_at
            .get(entity_id)
            .cloned()
            .unwrap_or_else(now_rfc3339);
        match self.attention.get(entity_id) {
            Some(attention) => attention.anchor(&created_at),
            None => created_at,
        }
    }

    /// Give a newly created entity its place in the inbox. Idempotent, so every
    /// mutation can call it and only the first one does anything.
    fn seed_anchor(&mut self, entity_id: &str) {
        let created_at = self
            .entity_created_at
            .get(entity_id)
            .cloned()
            .unwrap_or_else(now_rfc3339);
        let attention = self.attention.entry(entity_id.to_string()).or_default();
        if attention.anchor_at.is_some() {
            return;
        }
        attention.seed_anchor(&created_at);
        self.persist_attention();
    }

    /// The user said something to this entity: move its anchor if they had gone
    /// quiet for [`crate::attention::ANCHOR_GAP`], and leave it exactly where it
    /// is otherwise.
    ///
    /// Only the user's own words reach here. An agent filling a conversation
    /// all night is the work happening, and the work happening must never
    /// reorder the inbox under the person reading it.
    ///
    /// Called with the entity still in its map: the attention file is pruned to
    /// what exists when it is written, so a stamp taken while a record is
    /// checked out would be dropped on the way to disk.
    fn note_user_message(&mut self, entity_id: &str) {
        let created_at = self
            .entity_created_at
            .get(entity_id)
            .cloned()
            .unwrap_or_else(now_rfc3339);
        let now = now_rfc3339();
        let attention = self.attention.entry(entity_id.to_string()).or_default();
        attention.seed_anchor(&created_at);
        attention.note_user_message(&now);
        self.persist_attention();
    }

    /// Take a capture's anchor onto the work it just became, so the inbox holds
    /// ONE entry for a thing the user said and not two.
    fn inherit_capture_anchor(&mut self, entity_id: &str, capture_id: &str) {
        let Some(capture) = self.captures.get(capture_id) else {
            return;
        };
        let anchor = capture.anchor().to_string();
        self.attention
            .entry(entity_id.to_string())
            .or_default()
            .inherit_anchor(&anchor, None);
        self.persist_attention();
    }

    /// When somebody last spoke on this work item, extended through the end of
    /// its most recent in-flight turn. Git, files, tools and terminal output do
    /// not make an inbox entry recent.
    ///
    /// Every input is already in hand — no clock here starts new git work.
    fn last_activity_of(
        &self,
        entity_id: Option<&str>,
        conversation: Option<&crate::thread::Thread>,
    ) -> Option<String> {
        if let Some(id) = entity_id {
            if let Ok(roster) = self.entity_agents(id) {
                let conversation_at = roster
                    .iter()
                    .filter_map(|agent| {
                        let thread = self
                            .agent_conversation(id, Some(&agent.id))
                            .unwrap_or(&agent.thread);
                        thread.conversation_activity_at()
                    })
                    .max()
                    .map(str::to_string);
                let worked_at = self
                    .attention
                    .get(id)
                    .and_then(|attention| attention.last_worked_at.clone());
                let first_observed_at = self
                    .attention
                    .get(id)
                    .and_then(|attention| attention.first_observed_at.clone());
                return conversation_at
                    .into_iter()
                    .chain(worked_at)
                    .max()
                    .or(first_observed_at)
                    .or_else(|| Some(self.anchor_of(id)));
            }
        }
        conversation
            .and_then(crate::thread::Thread::conversation_activity_at)
            .map(str::to_string)
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

    /// The run agent currently executing an Issue's canonical conversation.
    /// The UI addresses chat and settings to this identity while retaining the
    /// Issue conversation id as a stale-binding guard. Once the alias agent is
    /// removed there is no replacement: a private secondary never inherits it.
    fn issue_execution_context(&self, issue_id: &str) -> Option<Value> {
        let conversation_id = self.plans.get(issue_id)?.agents.sole().conversation_id();
        let run = self
            .current_issue_implementation(issue_id)
            .filter(|run| !run.run.state.is_terminal())?;
        let agent = run
            .agents
            .iter()
            .find(|agent| agent.conversation_id() == conversation_id)?;
        let root = Some(AppState::canonical_root(&run.worktree.path));
        Some(json!({
            "entity_id": run.run.id.0,
            "agent_id": agent.id,
            "conversation_id": conversation_id,
            "agent": self.agent_digest(
                &run.run.id.0,
                agent,
                root.as_deref(),
                DigestScope::List,
            ),
        }))
    }

    fn plan_view(
        &self,
        plan_id: &str,
        active: &ActivePlan,
        thread_detail: ThreadDetail,
        scope: DigestScope,
    ) -> Value {
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
        // Narrower than the newest implementation: a merged or abandoned branch
        // has stopped speaking for its issue.
        let live_implementation = current_implementation.filter(|run| !run.run.state.is_terminal());
        let execution_context = self.issue_execution_context(plan_id);
        let mut implementation_lineage = self
            .runs
            .values()
            .filter(|run| run.run.plan_id.as_ref().map(|id| id.0.as_str()) == Some(plan_id))
            .map(|run| {
                json!({
                    "implementation_id": run.run.id.0,
                    "run_id": run.run.id.0,
                    "state": run_state_str(&run.run.state),
                    "branch": run.worktree.branch(),
                    "worktree_path": run.worktree.path.display().to_string(),
                    "recovery": run.recovery,
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
        let unread = self.unread_for(plan_id, Some(active.agents.sole_thread()));
        json!({
            "issue_id": plan_id,
            "plan_id": plan_id,
            "goal": active.plan.goal,
            "state": plan_state_str(&active.plan.state),
            // Event-driven, and `needs_attention` is the same fact under the
            // name the SPA already reads.
            "needs_attention": unread.is_unread(),
            "unread": unread.is_unread(),
            "unread_count": unread.count,
            "unread_reason": unread.reason,
            // See `run_view`.
            "muted": self.is_muted(plan_id),
            "dismissed": self.is_dismissed(plan_id),
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
                ThreadDetail::Digest => active.agents.sole_thread().digest_value(),
                ThreadDetail::Full => active.agents.sole_thread().wire_value(),
                ThreadDetail::Page(limit) => self.first_thread_page(active.agents.sole_thread(), limit),
            },
            // The rail's bubble strip: one entry per agent, on every surface
            // that renders an entity, so status stays legible fully collapsed.
            "agents": self.agent_digests(plan_id, scope),
            "execution_context": execution_context,
            "active_run_id": active_run_id,
            // Whether a branch is implementing this issue RIGHT NOW, and which
            // one. The same fact that hides the issue's row behind that
            // branch's in the feed, said out loud: an issue that has gone quiet
            // because something is being built for it must be able to say so
            // rather than simply vanish.
            "implementation_active": live_implementation.is_some(),
            "implementing_branch": live_implementation.map(|run| run.worktree.branch()),
            "current_implementation_id": current_implementation.map(|run| run.run.id.0.clone()),
            "current_implementation": current_implementation.map(|run| json!({
                "implementation_id": run.run.id.0,
                "run_id": run.run.id.0,
                "state": run_state_str(&run.run.state),
                "branch": run.worktree.branch(),
                "worktree_path": run.worktree.path.display().to_string(),
                "recovery": run.recovery,
            })),
            "implementation_lineage": implementation_lineage,
            "implementation_intent": active.plan.implementation_intent,
            "implementation_activity": active.plan.implementation_activity,
            "implementation_complete": implementation_complete,
            // Done on an issue archives it, whatever was or was not built for
            // it: an issue only stops being archivable once it already is.
            "can_archive": active.plan.archived_at.is_none(),
            // …and what archiving would gloss over rides along, so the surface
            // that offers Done can say it before the user confirms.
            "finish": { "warnings": crate::branch::warnings_json(
                &crate::branch::issue_finish_warnings(current_implementation.is_some()),
            ) },
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
    fn run_view(
        &self,
        run_id: &str,
        active: &ActiveRun,
        thread_detail: ThreadDetail,
        scope: DigestScope,
    ) -> Value {
        let project_id = self.entity_project.get(run_id).cloned().unwrap_or_default();
        let project = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let primary = self.owns_primary_checkout(run_id, active);
        let unread = self.unread_for(run_id, self.conversation_thread_for_run(active));
        // A branch with no agents has no conversation yet, and an empty one is
        // what says so: the client paints the new-agent view under it.
        let no_conversation = crate::thread::Thread::default();
        let conversation = self
            .conversation_thread_for_run(active)
            .unwrap_or(&no_conversation);
        json!({
            "run_id": run_id,
            "implementation_id": run_id,
            "issue_id": active.run.plan_id.as_ref().map(|p| p.0.clone()),
            "plan_id": active.run.plan_id.as_ref().map(|p| p.0.clone()),
            "goal": active.run.goal,
            "state": run_state_str(&active.run.state),
            // Event-driven, and `needs_attention` is the same fact under the
            // name the SPA already reads.
            "needs_attention": unread.is_unread(),
            "unread": unread.is_unread(),
            "unread_count": unread.count,
            "unread_reason": unread.reason,
            // Told the entry to stop asking. The badge above is already zeroed
            // by it; this is what the inbox renders the control from.
            "muted": self.is_muted(run_id),
            // Cleared out of the inbox until the conversation asks again. Mute
            // silences a row that stays; this one is not in the list at all.
            "dismissed": self.is_dismissed(run_id),
            "attention": self.attention_json(run_id),
            "branch": active.worktree.branch(),
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
                ThreadDetail::Digest => conversation.digest_value(),
                ThreadDetail::Full => conversation.wire_value(),
                ThreadDetail::Page(limit) => self.first_thread_page(conversation, limit),
            },
            // The rail's bubble strip — see `plan_view`.
            "agents": self.agent_digests(run_id, scope),
            // Review prioritization: an overlay on the diff, never a gate.
            "triage": self.triage_json(active),
            "triage_enabled": self.triage_enabled,
            "auto_advance": active.auto_advance,
            "current_stage_id": active.current_stage_id,
            "adopted": active.adopted,
            // Adopted around the repo root, not a worktree beside it: the rail
            // renders it as the project's "main" row, never as one more
            // worktree, and its finish/merge controls do not apply.
            "primary": primary,
            "recovery": active.recovery,
            // `run.finish` refuses the primary checkout, so it is never offered.
            "can_finish": !primary
                && (active.run.state == RunState::Merged
                    || (active.run.state == RunState::Review && active.worktree.path.exists())),
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

    /// The run's triage pass, with the one thing the SPA cannot derive: whether
    /// the diff has moved since the pass read it.
    ///
    /// `stale` is derived here and never stored — the diff moves under a triage
    /// constantly, and a stored flag would be a second thing to keep true. A
    /// stale pass still ships: an ordering from the previous revision beats no
    /// ordering at all while the re-triage runs, and the SPA labels it.
    fn triage_json(&self, active: &ActiveRun) -> Value {
        let Some(triage) = &active.triage else {
            return Value::Null;
        };
        let current_revision = self
            .conversation_thread_for_run(active)
            .and_then(|thread| thread.current_revision(crate::thread::ArtifactKind::Diff))
            .map(|revision| revision.content_hash.clone());
        json!({
            "based_on": triage.based_on,
            "hunks": triage.hunks,
            // Where the reviewer already disagreed with the pass. Renders as
            // the level they chose, over the one the agent chose.
            "overrides": triage.overrides,
            // No revision recorded yet means nothing has been observed to move.
            "stale": current_revision.is_some_and(|current| current != triage.based_on),
        })
    }

    /// A run's diffstat for the `board.list` poll surface, held for
    /// [`TASK_STAT_TTL`] and then served stale while it refreshes. `None` until
    /// the first refresh lands, and for a terminal run (worktree pruned or
    /// about to be) forever.
    fn run_stat(&mut self, run_id: &str) -> Option<Value> {
        if let Some(refresh) = self.run_stat_refresh(run_id) {
            let computed_at = self.run_stat_cache.get(run_id).map(|(at, _)| *at);
            self.refresh_if_stale(computed_at, TASK_STAT_TTL, refresh);
        }
        self.run_stat_cache
            .get(run_id)
            .map(|(_, stat)| stat.clone())
    }

    // ---- the scripted QA agent ------------------------------------------------

    /// Simulate a plan session: write the two-stage plan docs + manifest into
    /// the issue's scratch docs dir and report `done(phase=plan)`, so the
    /// orchestrator ingests them into the canonical store exactly as a real
    /// harness would over MCP.
    fn qa_simulate_plan(&self, project_id: &str, active: &mut ActivePlan) -> Result<(), String> {
        let previous_stage_ids: Vec<String> =
            active.stages.iter().map(|stage| stage.id.clone()).collect();
        let docs_dir = active
            .workspace
            .as_ref()
            .ok_or("QA plan: no planning workspace")?
            .docs_dir
            .clone();
        let goal = active.plan.goal.clone();
        write_in_dir(
            &docs_dir,
            ".build/plan/01-first-half.md",
            &format!("# Stage: First half\n\n1. Implement the first half of: {goal}\n"),
        )?;
        write_in_dir(
            &docs_dir,
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
        write_in_dir(&docs_dir, STAGES_MANIFEST_PATH, &manifest)?;
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
        append_plan_stage_announcements(active.agents.sole_thread_mut(), &plan_id, &new_stages);
        Ok(())
    }

    /// Simulate a per-stage plan-revision session: rewrite the stage doc in the
    /// scratch docs dir and resolve every open comment on the revised stage.
    fn qa_simulate_plan_stage_revise(
        &self,
        project_id: &str,
        active: &mut ActivePlan,
    ) -> Result<(), String> {
        let stage_id = active
            .revising_stage_id
            .clone()
            .ok_or("QA plan revise: no stage revision in flight")?;
        let docs_dir = active
            .workspace
            .as_ref()
            .ok_or("QA plan revise: no planning workspace")?
            .docs_dir
            .clone();
        let index = active.stage_doc_index(&stage_id)?;
        let stage_path = active.stages[index].path.clone();
        let mut contents = std::fs::read_to_string(docs_dir.join(&stage_path))
            .map_err(|e| format!("QA plan revise: could not read stage doc: {e}"))?;
        contents.push_str("\n(revised)\n");
        write_in_dir(&docs_dir, &stage_path, &contents)?;
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
        .agents
        .sole_thread()
        .open_doc_comments_for(&doc.id)
        .len();
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
    json!({
        "stat": diff.stat().to_json(),
        "files": files,
        "patch": diff.patch(),
    })
}

/// Modification times for changed paths that still exist in a checkout.
/// Deleted paths are omitted because neither Git nor the filesystem retains
/// their last worktree modification time.
fn diff_file_edited_at(
    worktree_path: &std::path::Path,
    diff: &crate::diff::WorktreeDiff,
) -> serde_json::Map<String, Value> {
    diff.files()
        .iter()
        .filter_map(|file| {
            let edited_at = crate::diff::file_edited_at(worktree_path, &file.path)?;
            Some((file.path.clone(), json!(edited_at)))
        })
        .collect()
}

fn worktree_diff_json(worktree_path: &std::path::Path, diff: &crate::diff::WorktreeDiff) -> Value {
    let mut value = diff_json(diff);
    value["file_edited_at"] = Value::Object(diff_file_edited_at(worktree_path, diff));
    value
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

/// One stage comment on the wire, read off the conversation that holds it.
fn comment_json(comment: &crate::thread::DocComment) -> Value {
    json!({
        "id": comment.id,
        "stage_id": comment.stage_id,
        "path": comment.path,
        "anchor": comment.anchor.as_ref().map(|anchor| json!({
            "heading_path": anchor.heading_path,
            "snippet": anchor.snippet,
            "line_start": anchor.line_start,
            "line_end": anchor.line_end,
        })),
        "body": comment.body,
        "state": match comment.state {
            crate::thread::DocCommentState::Open => "open",
            crate::thread::DocCommentState::Addressed => "addressed",
        },
        "agent_reply": comment.agent_reply,
    })
}

/// Parse the optional `anchor` param of `plan.comment_add`: `null`/absent is a
/// general comment; present, it must carry a string-array `heading_path` and a
/// string `snippet` (capped server-side at 400 chars). Optional `line_start` /
/// `line_end` say where the passage sat when it was selected.
fn parse_comment_anchor(value: Option<&Value>) -> Result<Option<crate::thread::DocAnchor>, String> {
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
            Ok(Some(crate::thread::DocAnchor {
                heading_path,
                snippet,
                line_start: parse_anchor_line(v, "line_start")?,
                line_end: parse_anchor_line(v, "line_end")?,
            }))
        }
    }
}

/// One optional line number of a comment anchor. Absent and `null` both mean
/// the reviewer selected a passage without line context; anything else must be
/// a line number.
fn parse_anchor_line(anchor: &Value, field: &str) -> Result<Option<u32>, String> {
    match anchor.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|line| u32::try_from(line).ok())
            .map(Some)
            .ok_or_else(|| format!("anchor.{field} must be a line number")),
    }
}

/// The first stage an Issue still owes work on: the earliest one this run has
/// recorded no progress against, or whose progress is not a passed validation,
/// or whose pass a later change invalidated. `None` once every stage of the
/// manifest has settled. A run that does not exist yet has settled nothing, so
/// the first stage is the answer.
///
/// One predicate, four readers — boot's activity reconstruction, the
/// scheduler's target stage, a recovery's requested stage, and the Issue's
/// rendered activity — because what counts as settled has to move for all of
/// them at once.
fn next_unsettled_stage<'a>(
    stages: &'a [StageDoc],
    run: Option<&ActiveRun>,
) -> Option<&'a StageDoc> {
    stages.iter().find(|doc| {
        run.and_then(|run| run.stage_progress(&doc.id))
            .is_none_or(|progress| {
                progress.state != StageProgressState::Validated { passed: true }
                    || progress.invalidation_reason.is_some()
            })
    })
}

/// The stage a verified recovery is being asked to prove: the one the Issue's
/// durable intent names, the first stage it still owes work on when the intent
/// is the whole Issue, or — with nothing armed — whatever the run was last
/// building.
fn recovery_target_stage(issue: &ActivePlan, active: &ActiveRun) -> String {
    match &issue.plan.implementation_intent {
        ImplementationIntent::Stage(stage_id) => stage_id.clone(),
        ImplementationIntent::All => next_unsettled_stage(&issue.stages, Some(active))
            .map(|doc| doc.id.clone())
            .unwrap_or_default(),
        ImplementationIntent::None => active.current_stage_id.clone().unwrap_or_default(),
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

/// Parse and validate the optional provider/model/effort params of a request,
/// falling back to `default` — the account's default harness — when the caller
/// names no provider.
///
/// This is the one place a wire provider becomes a persisted one. Every token
/// names one harness concretely, so what this mints is what the agent is locked
/// to: nothing is ever resolved a second time.
fn model_choice_from(params: &Value, default: AgentProvider) -> Result<ModelChoice, String> {
    let provider = match params.get("provider").and_then(Value::as_str) {
        // No preference means the account's answer; a named one means itself.
        None | Some("") => default,
        Some(named) => AgentProvider::from_wire(named)
            .ok_or_else(|| format!("unknown agent provider: {named}"))?,
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

/// Which of the router's options the user tapped, named either by id or by
/// position — `None` when they typed an answer instead.
///
/// An id or an index that names nothing is refused rather than falling through
/// to the typed answer: a tap that misses is a tap the user thinks landed.
fn chosen_option_id(
    capture: &crate::capture::Capture,
    params: &Value,
) -> Result<Option<String>, String> {
    if let Some(option_id) = params
        .get("option_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|option_id| !option_id.is_empty())
    {
        return Ok(Some(option_id.to_string()));
    }
    let Some(index) = params.get("option_index").and_then(Value::as_u64) else {
        return Ok(None);
    };
    capture
        .question
        .as_ref()
        .and_then(|question| question.option_at(index as usize))
        .map(|option| Some(option.id.clone()))
        .ok_or_else(|| format!("capture.answer: no option was offered at position {index}"))
}

/// How this bridge tells a client a param it needed was not there — written
/// once, so every required param reads the same to the client.
fn missing_param(key: &str) -> String {
    format!("missing required param: {key}")
}

fn require_value(params: &Value, key: &str) -> Result<Value, String> {
    params.get(key).cloned().ok_or_else(|| missing_param(key))
}

fn require_str(params: &Value, key: &str) -> Result<String, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| missing_param(key))
}

fn require_array(params: &Value, key: &str) -> Result<Vec<Value>, String> {
    require_value(params, key)?
        .as_array()
        .cloned()
        .ok_or_else(|| missing_param(key))
}

/// What an Issue's scheduler is asked with when the frame that woke it was
/// about something else: the Issue to advance, and the thread paging that
/// frame's own answer is cut to.
fn scheduler_request(issue_id: &str, params: &Value) -> Value {
    json!({ "issue_id": issue_id, "thread_limit": params.get("thread_limit") })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DigestScope {
    List,
    Detail,
}

/// The optional `agent_id` a verb was addressed to. Only omission or null means
/// the entity's primary; an explicitly empty or malformed identity is refused
/// before it can broaden into somebody else's conversation.
fn named_agent_id(params: &Value) -> Result<Option<String>, String> {
    optional_nonempty_string(params, "agent_id").map(|id| id.map(str::to_string))
}

fn optional_nonempty_string<'a>(params: &'a Value, field: &str) -> Result<Option<&'a str>, String> {
    match params.get(field) {
        None => Ok(None),
        Some(Value::String(value)) if value.is_empty() => Err(format!("{field} cannot be empty")),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(format!("{field} must be a string")),
    }
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
    base_branch: String,
    external_worktree: bool,
}

/// A branch verb that answers with the project's branches: the checkout it
/// was scoped to, plus the checkouts the rows are stamped from.
struct BranchListingScope {
    checkout: BranchScope,
    checkouts: ProjectCheckouts,
}

/// What a run can prove about its own checkout when git's registration for it
/// is gone. A run Build dispatched works in a checkout
/// [`crate::worktree::WorktreeManager::create`] made and nothing else, so the
/// value the pruned registration carried is known. An adopted run's checkout
/// may be one Build only checked out over somebody's branch, and with the
/// registration gone nothing on disk says which — so it is not restored at
/// all, rather than restored under a guess that could delete the branch.
fn unregistered_restore_for(active: &ActiveRun) -> crate::worktree::UnregisteredRestore {
    if active.adopted {
        crate::worktree::UnregisteredRestore::Refuse
    } else {
        crate::worktree::UnregisteredRestore::Write(crate::worktree::BranchTeardown::DeletesBranch)
    }
}

/// The answer `git.branches` and `git.branch_delete` share: gitgui's git facts
/// about every offerable branch, each row stamped with the checkout holding it.
/// Every held branch is published first, so a row weighs what its checkout
/// holds rather than what the project last saw of it.
fn stamped_branch_list(scope: &BranchListingScope) -> Result<Value, String> {
    let ownership = scope.checkouts.holders()?;
    scope.checkouts.publish_held_branches(&ownership)?;
    let listing =
        crate::gitgui::branch_list(&scope.checkout.repo_path, &scope.checkout.base_branch)?;
    let branches: Vec<Value> = listing
        .rows
        .into_iter()
        .map(|row| {
            let holder = BranchHolder::of(&ownership, &row.name);
            let mut fields = row.into_json();
            fields["holder"] = holder.into_json();
            fields
        })
        .collect();
    Ok(json!({ "current": listing.current, "branches": branches }))
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

fn err(e: OrchestratorError) -> String {
    e.to_string()
}

/// What the lock-held half of a finish decided.
enum PlannedFinish {
    /// Answered out of memory alone — an idempotent replay of a finish that
    /// already completed. No disk, no claim, nothing to defer.
    Settled(Value),
    /// The checkout is claimed and its git work is ready to run with the mutex
    /// released.
    Deferred(Box<WorktreeFinishJob>),
}

/// What the lock-held half of a run's Done decided. The run is already off the
/// board in every variant but a refusal.
enum PlannedRunFinish {
    /// The checkout was already gone, so the run was retired from memory alone.
    Settled(Value),
    /// The checkout's finish already completed (an idempotent replay): only the
    /// run's own retirement is left.
    Replay {
        archived: Value,
        run: RunFinishEpilogue,
    },
    Deferred {
        job: Box<WorktreeFinishJob>,
        run: RunFinishEpilogue,
    },
}

/// One finish's git work, lifted out from under the app mutex: the forced
/// rescan, the eligibility recheck, the checkpoint, the durable intent record,
/// and the destructive steps (merge, branch delete, worktree removal). Seconds
/// to minutes on a large checkout, and none of it touching [`AppState`].
struct WorktreeFinishJob {
    project_id: String,
    /// The project's checkout seam, cloned off its orchestrator: every
    /// checkout, branch and scan this job touches goes through it, with the
    /// app mutex released.
    worktrees: WorktreeManager,
    base_branch: String,
    worktree_id: String,
    action: WorktreeFinishAction,
    /// Checkouts a run already owns, excluded from the scan exactly as
    /// [`AppState::external_worktrees`] excludes them.
    excluded: std::collections::HashSet<std::path::PathBuf>,
    /// A pending record found in memory: resume it rather than preflight again.
    resume: Option<PersistedArchivedWorktree>,
    store: Store,
    #[cfg(test)]
    gate: Option<OffLockGate>,
}

/// What the lock-free git work brought back for the app mutex to write down.
struct WorktreeFinishOutcome {
    /// The fresh external scan the preflight paid for, for the cache that would
    /// otherwise pay for it again on the next poll.
    scan: Option<Vec<ExternalWorktree>>,
    /// The archive record as it now stands on disk: Archived after a completed
    /// finish, Pending after a failed destructive step. `None` when nothing was
    /// ever written.
    record: Option<PersistedArchivedWorktree>,
    result: Result<Value, String>,
}

/// Work a verb handed to the drain, to run with the app mutex released.
enum DeferredWork {
    /// One lifecycle verb's git — `git worktree add`, a checkpoint, a scan —
    /// and the row reserved on the board until it returns.
    Lifecycle(Box<WorktreeLifecycleJob>),
    /// A claimed finish and the bookkeeping still owed once its git returns.
    Finish {
        job: Box<WorktreeFinishJob>,
        epilogue: FinishEpilogue,
    },
    /// One `git.*` verb against one resolved checkout.
    Git(Box<DeferredGit>),
    /// One diff to render for a review surface.
    Read(Box<DeferredRead>),
}

/// What the lock-free phase brought back, for the app mutex to write down.
enum DeferredOutcome {
    Lifecycle(Box<LifecycleOutcome>),
    Finish {
        epilogue: Box<FinishEpilogue>,
        finished: Box<WorktreeFinishOutcome>,
    },
    Git {
        git: Box<DeferredGit>,
        result: Result<Value, String>,
    },
    Read(Result<Value, String>),
}

impl DeferredWork {
    /// The lock-free phase. Consumes the work so nothing can run it twice.
    fn run(self) -> DeferredOutcome {
        match self {
            Self::Lifecycle(job) => DeferredOutcome::Lifecycle(Box::new(job.run())),
            Self::Finish { job, epilogue } => DeferredOutcome::Finish {
                epilogue: Box::new(epilogue),
                finished: Box::new(job.run()),
            },
            Self::Git(git) => {
                #[cfg(test)]
                if let Some(gate) = &git.gate {
                    gate.arrive();
                }
                let result = git.run();
                DeferredOutcome::Git { git, result }
            }
            Self::Read(read) => {
                #[cfg(test)]
                if let Some(gate) = &read.gate {
                    gate.arrive();
                }
                DeferredOutcome::Read(read.run())
            }
        }
    }
}

/// A read whose git work needs nothing the app mutex holds: the lock resolves
/// its inputs — paths, refs, ids — and the drain renders the answer from them
/// alone.
///
/// Rendering a patch reads every changed blob, so a review surface asking for
/// one is seconds of libgit2 on a large checkout. Nothing is written back
/// afterwards, so there is no staleness to check: the answer describes the tree
/// as it was read, which is what was asked for.
struct DeferredRead {
    subject: ReadSubject,
    /// The issue that asked, when the read came in through an issue surface —
    /// stamped onto the answer, as the issue verbs did before the split.
    issue_id: Option<String>,
    /// The complete aggregate held by the caller. We still recompute to avoid
    /// stale filesystem answers, then suppress the equal payload on the wire.
    if_diff_key: Option<String>,
    #[cfg(test)]
    gate: Option<OffLockGate>,
}

/// Which diff a deferred read renders.
enum ReadSubject {
    /// `project.list` — immutable row inputs captured at request time, with
    /// repository and volume metadata read while the app mutex is released.
    ProjectList { projects: Vec<ProjectListRow> },
    /// `project.diff` — a primary checkout's uncommitted work.
    Project {
        project_id: String,
        repo_path: std::path::PathBuf,
    },
    /// `worktree.diff` — one external checkout against its base branch.
    Worktree {
        external: Box<ExternalWorktree>,
        base_branch: String,
    },
    /// `run.diff` — one run against its baseline: the sha it started from, or
    /// its merge base with the base branch when it has none.
    Run {
        worktree_path: std::path::PathBuf,
        base_sha: Option<String>,
        base_branch: String,
    },
    /// `run.stage_diff` — one immutable stage boundary, sha to sha.
    Stage {
        run_id: String,
        stage_id: String,
        /// Where the two commits can still be read: the checkout while it
        /// exists, the project's repository once it does not.
        object_database: std::path::PathBuf,
        start_sha: String,
        completion_sha: String,
    },
}

struct ProjectListRow {
    project_id: String,
    name: String,
    repo_path: std::path::PathBuf,
    worktrees_root: std::path::PathBuf,
    base_branch: String,
    isolation: Option<Isolation>,
    isolation_default: Isolation,
}

impl DeferredRead {
    fn run(&self) -> Result<Value, String> {
        let conditional_key = self.subject.conditional_key()?;
        if let Some(diff_key) = conditional_key.as_deref() {
            if self.if_diff_key.as_deref() == Some(diff_key) {
                return Ok(json!({ "unchanged": true, "diff_key": diff_key }));
            }
        }
        let mut rendered = self.subject.render()?;
        if let (Some(issue_id), Some(object)) = (&self.issue_id, rendered.as_object_mut()) {
            object.insert("issue_id".to_string(), json!(issue_id));
        }
        if let Some(diff_key) = conditional_key {
            if let Some(object) = rendered.as_object_mut() {
                object.insert("diff_key".to_string(), json!(diff_key));
            }
        }
        Ok(rendered)
    }
}

impl ReadSubject {
    fn conditional_key(&self) -> Result<Option<String>, String> {
        let material = match self {
            Self::Worktree {
                external,
                base_branch,
            } => format!(
                "worktree\0{}\0{}\0{:?}\0{}\0{}\0{}",
                external.id,
                base_branch,
                external.branch,
                external.head_subject,
                external.dirty_files,
                crate::diff::key_against_merge_base(&external.path, base_branch)
                    .map_err(|error| error.to_string())?
            ),
            Self::Run {
                worktree_path,
                base_sha,
                base_branch,
            } => {
                let (base, delta_key) = match base_sha {
                    Some(sha) => (
                        sha.as_str(),
                        crate::diff::key_against_base(worktree_path, sha)
                            .map_err(|error| error.to_string())?,
                    ),
                    None => (
                        base_branch.as_str(),
                        crate::diff::key_against_merge_base(worktree_path, base_branch)
                            .map_err(|error| error.to_string())?,
                    ),
                };
                format!("run\0{}\0{}", base, delta_key)
            }
            _ => return Ok(None),
        };
        Ok(Some(sha256_hex(material.as_bytes())))
    }

    fn render(&self) -> Result<Value, String> {
        match self {
            Self::ProjectList { projects } => {
                let projects = projects
                    .iter()
                    .map(ProjectListRow::render)
                    .collect::<Vec<_>>();
                Ok(json!({ "projects": projects }))
            }
            Self::Project {
                project_id,
                repo_path,
            } => {
                let branch = git2::Repository::open(repo_path)
                    .ok()
                    .and_then(|repo| {
                        repo.head()
                            .ok()
                            .and_then(|head| head.shorthand().map(str::to_string))
                    })
                    .unwrap_or_else(|| "HEAD".to_string());
                let diff =
                    crate::diff::diff_against_head(repo_path).map_err(|error| error.to_string())?;
                Ok(json!({
                    "project_id": project_id,
                    "branch": branch,
                    "path": repo_path.display().to_string(),
                    "stat": diff.stat().to_json(),
                    "files": diff_file_rows(&diff),
                    "patch": diff.patch(),
                }))
            }
            Self::Worktree {
                external,
                base_branch,
            } => {
                let diff = crate::diff::diff_against_merge_base(&external.path, base_branch)
                    .map_err(|error| error.to_string())?;
                let adoptable = external
                    .branch
                    .as_deref()
                    .is_some_and(|branch| branch != base_branch);
                Ok(json!({
                    "worktree_id": external.id,
                    "branch": external.branch,
                    // The branch this diff is anchored on, so the surface can
                    // name it instead of saying "the base branch".
                    "base_branch": base_branch,
                    "head_subject": external.head_subject,
                    "dirty_files": external.dirty_files,
                    "path": external.path.display().to_string(),
                    "adoptable": adoptable,
                    "stat": diff.stat().to_json(),
                    "files": diff_file_rows(&diff),
                    "file_edited_at": diff_file_edited_at(&external.path, &diff),
                    "patch": diff.patch(),
                }))
            }
            Self::Run {
                worktree_path,
                base_sha,
                base_branch,
            } => {
                let diff = match base_sha {
                    Some(sha) => crate::diff::diff_against_base(worktree_path, sha),
                    None => crate::diff::diff_against_merge_base(worktree_path, base_branch),
                }
                .map_err(|error| error.to_string())?;
                Ok(worktree_diff_json(worktree_path, &diff))
            }
            Self::Stage {
                run_id,
                stage_id,
                object_database,
                start_sha,
                completion_sha,
            } => {
                let diff =
                    crate::diff::diff_between_commits(object_database, start_sha, completion_sha)
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
        }
    }
}

impl ProjectListRow {
    fn render(&self) -> Value {
        let available = IsolationAvailability::of(&self.repo_path, &self.worktrees_root);
        let requested = self.isolation.unwrap_or(self.isolation_default);
        let effective = if available.lock_reason(requested).is_none() {
            requested
        } else {
            Isolation::default()
        };
        json!({
            "project_id": self.project_id,
            "name": self.name,
            "path": self.repo_path.display().to_string(),
            "base_branch": self.base_branch,
            "remote": git_remote_origin(&self.repo_path),
            "isolation": self.isolation,
            "isolation_default": self.isolation_default,
            "isolation_effective": effective,
            "isolation_available": available,
        })
    }
}

/// The per-file rows a diff surface lists beside its patch.
fn diff_file_rows(diff: &crate::diff::WorktreeDiff) -> Vec<Value> {
    diff.files()
        .iter()
        .map(|file| json!({ "path": file.path, "status": format!("{:?}", file.status) }))
        .collect()
}

/// One `git.*` verb: the checkout the app mutex resolved for it, the git call
/// to make there with the mutex released, and whether its answer invalidates
/// the summaries the board reads.
///
/// A `git status` walks the whole worktree and a `git fetch` waits on a
/// network; the review surfaces poll both. Neither may hold the daemon still.
struct DeferredGit {
    call: Box<dyn DeferredGitWork>,
    params: Value,
    /// Whether a successful call made the scope's cached summaries stale.
    invalidates: bool,
    #[cfg(test)]
    gate: Option<OffLockGate>,
}

/// The git call a verb handed to the drain: what to run with the mutex
/// released, and which cached summaries to drop once it has changed the tree
/// underneath them.
trait DeferredGitWork: Send {
    fn run(&self, params: &Value) -> Result<Value, String>;
    fn invalidate(&self, app: &mut AppState);
}

/// A resolution the app mutex made for a git verb — which checkout, which
/// project, which cached summaries describe it — and what to drop from those
/// caches once a call against it has changed the tree.
trait GitCallScope: Send {
    fn invalidate(&self, app: &mut AppState);
}

impl GitCallScope for GitScope {
    fn invalidate(&self, app: &mut AppState) {
        if app.git_scope_is_current(self) {
            app.invalidate_git_scope_caches(self);
        }
    }
}

impl GitCallScope for BranchScope {
    fn invalidate(&self, app: &mut AppState) {
        app.invalidate_branch_scope_caches(self);
    }
}

impl GitCallScope for BranchListingScope {
    fn invalidate(&self, app: &mut AppState) {
        self.checkout.invalidate(app);
    }
}

/// One resolved scope and the call to make against it.
///
/// The scope and the function are one value because they are one decision —
/// the verb that defers the work picks both at once, and no other pairing can
/// be spelled. The call itself is a plain function of the scope and the
/// request, so it holds no state and cannot reach the daemon while it runs.
struct ScopedGitCall<S: GitCallScope> {
    scope: S,
    work: fn(&S, &Value) -> Result<Value, String>,
}

impl<S: GitCallScope> DeferredGitWork for ScopedGitCall<S> {
    fn run(&self, params: &Value) -> Result<Value, String> {
        (self.work)(&self.scope, params)
    }

    fn invalidate(&self, app: &mut AppState) {
        self.scope.invalidate(app);
    }
}

impl DeferredGit {
    fn run(&self) -> Result<Value, String> {
        self.call.run(&self.params)
    }
}

/// `worktree.create`'s apply half. The checkout is on disk and already in the
/// project's scan list — every write this verb owes is the shared half of
/// [`AppState::apply_lifecycle`] — so all that is left is the answer.
pub struct WorktreeCreated {
    pub project_id: String,
    /// The id the board carried while the git ran.
    pub placeholder_id: String,
    pub worktree_id: String,
    pub branch: String,
    pub name: String,
    pub path: std::path::PathBuf,
    pub branch_was_cut: bool,
    pub checkouts: ProjectCheckouts,
    /// What the checkout turned out to be, read off it once the git had made
    /// it.
    pub isolation: Option<Isolation>,
    /// [`ResolvedIsolation::downgrade`], said in the answer to the ask: a bare
    /// worktree has no run, no agent and no conversation to say it on.
    pub downgrade: Option<String>,
}

impl LifecycleEpilogue for WorktreeCreated {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        let note = self.downgrade.as_deref().map(announce_isolation_downgrade);
        state.validate_checkout_snapshot(&self.project_id, &self.checkouts)?;
        Ok(json!({
            "project_id": self.project_id,
            "worktree_id": self.worktree_id,
            // Both ids, because `WorktreeManager::create` suffixes a slug
            // something was already using and the placeholder cannot know: a
            // client showing the pending row replaces that row rather than
            // adding a second one beside it.
            "pending_worktree_id": self.placeholder_id,
            "branch_was_cut": self.branch_was_cut,
            "branch": self.branch,
            "name": self.name,
            "path": self.path.display().to_string(),
            "isolation": self.isolation,
            "isolation_note": note,
        }))
    }
}

/// A run opened around a checkout that is ready for it: the record, the agent
/// its first turn is addressed to, and what the Issue's conversation says about
/// where the work went.
struct OpenedImplementation {
    run_id: String,
    project_id: String,
    issue_id: String,
    active: ActiveRun,
    turn: AgentTurn,
    agent_id: String,
    checkout_event: crate::thread::ThreadEventKind,
    checkout_summary: String,
}

/// Who asked for an implementation: what they hear once the run behind it is
/// open, and what is left written down when the git that would have opened it
/// failed.
///
/// It travels with the job, so one object answers both ways — a verb asking
/// for a run hears the run; a scheduler asking for one carries on to the stage
/// it was cutting the checkout for, and marks the Issue blocked if it cannot.
pub trait ImplementationCaller: Send {
    fn opened(self: Box<Self>, state: &mut AppState, run_id: &str) -> Result<Value, String>;
    /// The message the frame gets, after whatever the decide phase armed on the
    /// strength of this implementation has been settled.
    fn refused(self: Box<Self>, state: &mut AppState, error: String) -> String;

    /// Hand the implementation back: the run it opened, or the error that
    /// stopped it. This is how the two halves above are used — every one of
    /// them, so a fourth implementation mutation cannot pair them a fifth way.
    fn settle(
        self: Box<Self>,
        state: &mut AppState,
        opened: Result<&str, String>,
    ) -> Result<Value, String> {
        match opened {
            Ok(run_id) => self.opened(state, run_id),
            Err(error) => Err(self.refused(state, error)),
        }
    }
}

/// `run.create` asked: it hears the run it opened, and a failure is its own
/// answer — nothing was armed on the way in.
struct RunOpenedView {
    detail: ThreadDetail,
}

impl ImplementationCaller for RunOpenedView {
    fn opened(self: Box<Self>, state: &mut AppState, run_id: &str) -> Result<Value, String> {
        let active = state.runs.get(run_id).ok_or("unknown run_id")?;
        Ok(state.run_view(run_id, active, self.detail, DigestScope::Detail))
    }

    fn refused(self: Box<Self>, _state: &mut AppState, error: String) -> String {
        error
    }
}

/// An Issue's scheduler asked, on its way to a stage: it carries on from where
/// the git stopped it, and answers with the Issue rather than the run — the
/// scheduler is what the frame called, and the run is an implementation detail
/// of the stage it was after.
struct IssueSchedulerWaiting {
    issue_id: String,
    request: Value,
    /// The stage a failure is recorded against. `None` for run-all, which
    /// blocks on whichever stage the Issue is standing at.
    blocked_stage: Option<String>,
}

impl ImplementationCaller for IssueSchedulerWaiting {
    fn opened(self: Box<Self>, state: &mut AppState, run_id: &str) -> Result<Value, String> {
        match state.dispatch_ready_stage(&self.issue_id, run_id, &self.request) {
            Ok(()) => state.issue_view_full(&self.issue_id, thread_detail(&self.request)),
            Err(error) => Err(self.refused(state, error)),
        }
    }

    fn refused(self: Box<Self>, state: &mut AppState, error: String) -> String {
        // The Issue said it was preparing something. Nothing is preparing it
        // now, and a spinner nothing will ever clear is worse than the failure.
        state.block_issue_scheduler(&self.issue_id, self.blocked_stage, &error);
        error
    }
}

/// A run's checkout, as the restore left it, on its way back under the app
/// mutex. `Err` is not a failure of the job — it is the finding that the branch
/// is gone, which the recovery agent is started for.
pub struct RestoredCheckout {
    pub issue_id: String,
    pub run_id: String,
    /// Whether the directory was still standing when the decide phase looked:
    /// what the Issue's conversation says happened, reused or recreated.
    pub checkout_stood: bool,
    pub restored: Result<crate::worktree::Worktree, String>,
    pub caller: Box<dyn ImplementationCaller>,
    /// [`ResolvedIsolation::downgrade`], said on the Issue's conversation
    /// beside what the restore found.
    pub downgrade: Option<String>,
}

impl LifecycleEpilogue for RestoredCheckout {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        state.settle_restored_checkout(*self)
    }
}

/// `run.create`'s apply half on a checkout cut for it: the git left a prepared
/// checkout, and the run that stands for it is opened here, where the maps are.
pub struct ImplementationOpened {
    pub project_id: String,
    pub issue_id: String,
    pub run_id: String,
    pub prepared: PreparedImplementation,
    pub model_choice: ModelChoice,
    pub caller: Box<dyn ImplementationCaller>,
    /// [`ResolvedIsolation::downgrade`], said on the Issue's conversation
    /// before the run is written down.
    pub downgrade: Option<String>,
}

impl LifecycleEpilogue for ImplementationOpened {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        state.open_prepared_implementation(*self)
    }
}

/// The same, on a checkout an existing run already owns: the git left a
/// checkpoint and a baseline commit, and the run is reset onto them.
pub struct ImplementationAdopted {
    pub project_id: String,
    pub issue_id: String,
    pub run_id: String,
    pub base_sha: String,
    /// The adoption this job's git phase ran on the way in, when the checkout
    /// had no owner. The run it minted is opened here rather than taken off the
    /// board, and nothing else about the implementation differs.
    pub adopted: Option<RunAdopted>,
    pub model_choice: ModelChoice,
    pub caller: Box<dyn ImplementationCaller>,
}

impl LifecycleEpilogue for ImplementationAdopted {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        state.open_adopted_implementation(*self)
    }
}

/// The git that would have opened an implementation failed. It comes back as an
/// epilogue rather than as an error because what a refusal leaves behind is
/// state — an Issue that says it is preparing something nobody is preparing any
/// more — and state is written under the app mutex.
pub struct ImplementationRefused {
    pub error: String,
    pub caller: Box<dyn ImplementationCaller>,
}

impl LifecycleEpilogue for ImplementationRefused {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        self.caller.settle(state, Err(self.error))
    }
}

/// One door to an Issue's planning agent, waiting on the workspace it works
/// in: `plan.create`'s first dispatch, the first message to an inert Issue, a
/// batch of notes, one stage's comments, a freeform message.
///
/// Every door is gated before any disk work and settled here, after it — so
/// the plan event, the prompt and the queued turn are the only things a door
/// writes for itself. The disk is [`OpenPlanWorkspace`]'s, once.
///
/// [`OpenPlanWorkspace`]: crate::lifecycle::OpenPlanWorkspace
pub trait PlanSessionOpening: Send {
    /// The workspace is on disk: apply the event this door was gated on,
    /// render the prompt, queue the turn, and answer whoever asked.
    fn open(
        self: Box<Self>,
        state: &mut AppState,
        workspace: crate::orchestrator::PlanWorkspace,
    ) -> Result<Value, String>;

    /// The workspace could not be written. A refusal is the caller's error for
    /// every door that asked for a session; a door that had already reached its
    /// destination (a routed capture) says so instead and overrides this.
    fn refused(self: Box<Self>, _state: &mut AppState, error: String) -> Result<Value, String> {
        Err(error)
    }
}

/// The planning workspace is written; what is left is the door that asked for
/// it.
pub struct PlanWorkspaceOpened {
    pub workspace: crate::orchestrator::PlanWorkspace,
    pub opening: Box<dyn PlanSessionOpening>,
}

impl LifecycleEpilogue for PlanWorkspaceOpened {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        self.opening.open(state, self.workspace)
    }
}

/// The planning workspace could not be written. What that leaves behind is the
/// door's own business, so it comes back as an epilogue rather than an error.
pub struct PlanWorkspaceRefused {
    pub error: String,
    pub opening: Box<dyn PlanSessionOpening>,
}

impl LifecycleEpilogue for PlanWorkspaceRefused {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        self.opening.refused(state, self.error)
    }
}

/// `plan.create` asked: the Issue's record, its first turn, and the view the
/// caller wanted.
struct IssueOpened {
    project_id: String,
    plan_id: String,
    goal: String,
    base_branch: String,
    model_choice: ModelChoice,
    detail: ThreadDetail,
}

impl PlanSessionOpening for IssueOpened {
    fn open(
        self: Box<Self>,
        state: &mut AppState,
        workspace: crate::orchestrator::PlanWorkspace,
    ) -> Result<Value, String> {
        state.open_planned_issue(*self, workspace)
    }
}

/// The first message to an inert Issue asked: the session it never had, and the
/// Issue's own view — with the sequence the message landed at, which is what
/// the composer is waiting for.
struct PlanDraftingStarted {
    issue_id: String,
    detail: ThreadDetail,
    posted_sequence: Option<u64>,
    receipt: Option<OperationReceipt>,
}

impl PlanSessionOpening for PlanDraftingStarted {
    fn open(
        self: Box<Self>,
        state: &mut AppState,
        workspace: crate::orchestrator::PlanWorkspace,
    ) -> Result<Value, String> {
        match state.open_inert_plan_drafting(&self.issue_id, workspace, self.detail) {
            Ok(view) => {
                if let Some(receipt) = self.receipt.as_ref() {
                    if let Err(error) = attach_plan_operation_turn(state, receipt) {
                        return Ok(state.settle_accepted_operation_error(receipt, error));
                    }
                }
                Ok(with_post_receipt(
                    view,
                    self.posted_sequence,
                    self.receipt.as_ref(),
                ))
            }
            Err(error) => match self.receipt.as_ref() {
                Some(receipt) => Ok(state.settle_accepted_operation_error(receipt, error)),
                None => Err(error),
            },
        }
    }

    fn refused(self: Box<Self>, state: &mut AppState, error: String) -> Result<Value, String> {
        match self.receipt.as_ref() {
            Some(receipt) => Ok(state.settle_accepted_operation_error(receipt, error)),
            None => Err(error),
        }
    }
}

fn attach_plan_operation_turn(
    state: &mut AppState,
    receipt: &OperationReceipt,
) -> Result<(), String> {
    let delivery = receipt
        .delivery
        .as_ref()
        .ok_or("thread.post: accepted plan operation has no delivery intent")?;
    let payload = delivery
        .payload
        .as_ref()
        .ok_or("thread.post: accepted plan operation has no bounded payload")?;
    let turn = state
        .pending_agent_turns
        .iter_mut()
        .rev()
        .find(|turn| {
            turn.operation_id.is_none()
                && turn.owner == delivery.owner_id
                && turn.agent_id == delivery.agent_id
        })
        .ok_or("thread.post: plan session opened without a delivery turn")?;
    turn.operation_id = Some(receipt.operation_id.clone());
    turn.conversation_id = receipt.conversation_id.clone();
    turn.model_choice = delivery.model_choice.clone();
    turn.choice_revision = delivery.choice_revision;
    turn.interrupt = delivery.interrupt;
    let exact_cold = payload.delivery_prompt(&receipt.operation_id, true);
    let exact_warm = payload.delivery_prompt(&receipt.operation_id, false);
    if let Some(say) = turn.say.as_mut() {
        say.cold.push_str("\n\n");
        say.cold.push_str(&exact_cold);
        say.warm = exact_warm;
    }
    turn.wants_catch_up = false;
    turn.survives_refusal = true;
    Ok(())
}

/// A batch of plan notes asked: the revision session they go to.
struct PlanNotesSent {
    plan_id: String,
    project_id: String,
    detail: ThreadDetail,
}

impl PlanSessionOpening for PlanNotesSent {
    fn open(
        self: Box<Self>,
        state: &mut AppState,
        workspace: crate::orchestrator::PlanWorkspace,
    ) -> Result<Value, String> {
        state.settle_plan_session(&self.plan_id, self.detail, |state, active| {
            let turn = state
                .orch_for(&self.project_id)?
                .open_plan_notes(active, workspace, NEW_THREAD_MESSAGES_PROMPT)
                .map_err(err)?;
            state.queue_plan_turn(&self.plan_id, active, turn);
            if state.qa_agent {
                state.qa_simulate_plan(&self.project_id, active)?;
            }
            Ok(())
        })
    }
}

/// One stage's open comments asked: the revision session they are rendered
/// into.
struct StageNotesSent {
    plan_id: String,
    project_id: String,
    stage_id: String,
    detail: ThreadDetail,
}

impl PlanSessionOpening for StageNotesSent {
    fn open(
        self: Box<Self>,
        state: &mut AppState,
        workspace: crate::orchestrator::PlanWorkspace,
    ) -> Result<Value, String> {
        state.settle_plan_session(&self.plan_id, self.detail, |state, active| {
            let turn = state
                .orch_for(&self.project_id)?
                .open_plan_stage_notes(active, workspace, &self.stage_id)
                .map_err(err)?;
            state.queue_plan_turn(&self.plan_id, active, turn);
            if state.qa_agent {
                state.qa_simulate_plan_stage_revise(&self.project_id, active)?;
            }
            Ok(())
        })
    }
}

/// A freeform message asked: the session that hears it, drafting or resumed.
struct PlanMessaged {
    plan_id: String,
    project_id: String,
    detail: ThreadDetail,
}

impl PlanSessionOpening for PlanMessaged {
    fn open(
        self: Box<Self>,
        state: &mut AppState,
        workspace: crate::orchestrator::PlanWorkspace,
    ) -> Result<Value, String> {
        state.settle_plan_session(&self.plan_id, self.detail, |state, active| {
            let turn = state
                .orch_for(&self.project_id)?
                .open_plan_message(active, workspace, NEW_THREAD_MESSAGES_PROMPT)
                .map_err(err)?;
            state.queue_plan_turn(&self.plan_id, active, turn);
            if state.qa_agent && active.plan.state == PlanState::Drafting {
                if active.revising_stage_id.is_some() {
                    state.qa_simulate_plan_stage_revise(&self.project_id, active)?;
                } else {
                    state.qa_simulate_plan(&self.project_id, active)?;
                }
            }
            Ok(())
        })
    }
}

/// The agent one dispatch put on a branch, and the branch it is working.
struct DispatchedAgent {
    branch: String,
    agent_id: String,
}

impl DispatchedAgent {
    /// What every dispatch answers with, whichever way it reached its run.
    fn json(&self, project_id: &str, run_id: &str) -> Value {
        json!({
            "project_id": project_id,
            "branch": self.branch,
            "run_id": run_id,
            "agent_id": self.agent_id,
        })
    }
}

/// A checkout Build has taken ownership of on disk, and the run that is about
/// to stand for it. The apply half of every adoption, and the one place the
/// order of those writes is spelled.
///
/// What is deliberately NOT here is the write that settles the run: it comes
/// back out of [`RunAdopted::open_run`] un-persisted so its caller can add what
/// it still owes — a dispatch's agent and first turn — and write once, leaving
/// no window where a run exists that a later failure would strand.
pub struct RunAdopted {
    pub project_id: String,
    pub run_id: String,
    pub base_branch: String,
    pub checkout: AdoptableCheckout,
    pub scope: AdoptionScope,
    pub model_choice: ModelChoice,
}

impl RunAdopted {
    /// Open the run around the checkout, and move the board's bookkeeping onto
    /// it. Nothing here can fail once the run record is minted.
    fn open_run(&self, state: &mut AppState) -> Result<ActiveRun, String> {
        let active = state
            .orch_for(&self.project_id)?
            .adopt_run(
                RunId::new(&self.run_id),
                &self.checkout,
                &self.base_branch,
                self.model_choice.clone(),
            )
            .map_err(err)?;
        state
            .entity_project
            .insert(self.run_id.clone(), self.project_id.clone());
        // The row this checkout showed as belongs to a run from here on, and a
        // run is cleared through its conversation: whatever was dismissed
        // against the entity-less row is spent, and must not come back with the
        // bare row if the run is ever released.
        let (was_dismissed, first_observed_at) = state.take_row_dismissal(
            &self.project_id,
            Some(&self.checkout.branch),
            self.scope == AdoptionScope::PrimaryCheckout,
        );
        if was_dismissed || first_observed_at.is_some() {
            let attention = state.attention.entry(self.run_id.clone()).or_default();
            attention.first_observed_at = first_observed_at;
            if was_dismissed {
                attention.dismiss_messages();
            }
            state.persist_attention();
        }
        state.note_worktree_gone(&self.project_id, &self.checkout.path);
        Ok(active)
    }
}

/// `run.adopt`'s apply half: the checkout is Build's on disk, and the run that
/// stands for it is opened, persisted and answered with here. Nothing is owed
/// on top of the adoption, so this is [`RunAdopted`] and the reply alone.
pub struct RunAdoptionSettled {
    pub adopted: RunAdopted,
    pub detail: crate::thread::ThreadDetail,
}

impl LifecycleEpilogue for RunAdoptionSettled {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        let active = self.adopted.open_run(state)?;
        let (view, persisted) =
            state.answer_run_mutation(self.adopted.run_id.clone(), active, self.detail);
        persisted?;
        Ok(view)
    }
}

/// What a run whose checkout has just been let go of still owes the records.
/// `run.abandon` writes a verdict onto it; `run.delete` clears its card away.
///
/// The run arrives as an argument because the git phase carried it: it rides
/// through the removal so that no failure can strand it off the board.
pub trait DiscardSettlement: Send {
    /// Ask git whatever this settlement has to know before the checkout is let
    /// go of — the refs an answer depends on are readable only until then. Runs
    /// inside [`DiscardCheckout::perform`], with the app mutex released.
    ///
    /// Most settlements have nothing to ask, and a question nobody asked costs
    /// nothing: it is the abandon that judges a run's stages, and saying so
    /// here is what keeps a delete from paying for the verdict.
    fn judge_before_removal(&mut self) {}

    fn settle(self: Box<Self>, state: &mut AppState, active: ActiveRun) -> Result<Value, String>;
}

/// `run.abandon`'s apply half: the agents are dead, the checkout is gone, and
/// what is left is the verdict — on the run, on the stages the removal made
/// unverifiable, and on the Issue the run was implementing.
struct RunAbandoned {
    run_id: String,
    project_id: String,
    /// The Issue this run was implementing, told what it lost.
    issue_id: Option<String>,
    detail: crate::thread::ThreadDetail,
    /// What the run's stages are judged against, and git's answer once
    /// [`DiscardSettlement::judge_before_removal`] has asked. An abandon is the
    /// only verb that asks, so it is the only one that carries the query.
    stages: StagePublicationQuery,
    published: StagePublications,
}

impl DiscardSettlement for RunAbandoned {
    fn judge_before_removal(&mut self) {
        self.published = self.stages.classify();
    }

    fn settle(self: Box<Self>, state: &mut AppState, active: ActiveRun) -> Result<Value, String> {
        state.settle_abandoned_run(*self, active)
    }
}

/// `run.delete`'s apply half: the durable record goes, and every trace of the
/// run in memory goes with it.
///
/// The record is deleted here and not in the decide phase because the decide
/// phase can still be refused — the checkout's row may already be claimed by
/// another verb — and a refusal must leave the card whole. Getting here is what
/// says the delete is happening: the removal cannot fail. A crash in between
/// leaves the record for boot to reload and the vanished-run sweep to archive,
/// the same story every other reservation has.
///
/// A store that refuses the delete puts the run back where the decide phase
/// took it from: the record still stands, so the card must too, and the delete
/// is retried like any other failed write.
struct RunDeleted {
    run_id: String,
    /// The project whose board loses the card, when the run still has one: a
    /// run recovered after its repository moved has no project mapping, and
    /// clearing that stale card is exactly what a delete is for.
    project_id: Option<String>,
    /// The directory the run worked in, consulted to tell a checkout that
    /// survived the delete — the user's own files — from one that was pruned.
    checkout: std::path::PathBuf,
}

impl DiscardSettlement for RunDeleted {
    fn settle(self: Box<Self>, state: &mut AppState, active: ActiveRun) -> Result<Value, String> {
        if let Some(store) = &state.store {
            if let Err(error) = store.delete_run(&self.run_id) {
                state.runs.insert(self.run_id.clone(), active);
                return Err(format!("run store: {error}"));
            }
        }
        state.forget_run(&self.run_id);
        if let Some(project_id) = self.project_id.filter(|_| self.checkout.exists()) {
            // The checkout outlived its card — it was the user's — so it goes
            // back to the board as the bare one it is.
            state.rescan_external_worktrees(&project_id);
        }
        state.reap_orphaned_terminals();
        Ok(json!({ "ok": true }))
    }
}

/// `branch.dispatch`'s apply half: the checkout is checkpointed and scaffolded,
/// and the run that owns it — with the agent that will hear the instruction —
/// is opened here, under the mutex, where the records live.
pub struct BranchDispatched {
    pub adopted: RunAdopted,
    pub instruction: String,
    pub routed: Option<RoutedCapture>,
    pub checkouts: ProjectCheckouts,
    /// [`ResolvedIsolation::downgrade`], said on the dispatched run's own
    /// conversation.
    pub downgrade: Option<String>,
}

/// The holder read found an existing run; validate that snapshot before joining.
pub struct BranchJoined {
    pub project_id: String,
    pub run_id: String,
    pub branch: String,
    pub root: std::path::PathBuf,
    pub instruction: String,
    pub model_choice: ModelChoice,
    pub explicit_choice: bool,
    pub routed: Option<RoutedCapture>,
    pub checkouts: ProjectCheckouts,
}

impl LifecycleEpilogue for BranchJoined {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        state.validate_checkout_snapshot(&self.project_id, &self.checkouts)?;
        #[cfg(test)]
        fail_dispatch_at(state.dispatch_fault, BranchDispatchStep::Post)?;
        let choice = if self.explicit_choice {
            self.model_choice.clone()
        } else {
            state.entity_model_choice(&self.run_id)?
        };
        state.join_dispatched_run(*self, choice)
    }
}

impl LifecycleEpilogue for BranchDispatched {
    fn apply(self: Box<Self>, state: &mut AppState) -> Result<Value, String> {
        state.open_dispatched_run(*self)
    }
}

/// What [`AppState::apply_finish`] has to settle after the git work: always the
/// claim and the project's caches, plus whatever the verb that deferred it owes
/// on top.
struct FinishEpilogue {
    /// The claim taken in the plan phase, released here.
    worktree_id: String,
    project_id: String,
    kind: FinishKind,
}

/// Which verb deferred the finish, and what it still owes.
enum FinishKind {
    /// `worktree.finish` — the archive record is the whole answer.
    Worktree,
    /// `run.finish` — retire the run behind the checkout.
    Run(RunFinishEpilogue),
    /// `branch.finish` — retire the run (if any) and settle the issue the
    /// branch was implementing.
    Branch(BranchFinishEpilogue),
}

/// The run taken off the board while its checkout is being finished, so it can
/// be retired on success or put back on failure.
struct RunFinishEpilogue {
    run_id: String,
    project_id: String,
    /// The run record itself, held here rather than in `runs` — a run whose
    /// checkout is being deleted must not answer verbs meanwhile.
    active: Box<ActiveRun>,
    /// The canonical checkout root, consulted to tell a failure that left the
    /// worktree standing (retryable) from one that did not.
    root: std::path::PathBuf,
}

/// The issue bookkeeping `branch.finish` owes once the branch is gone.
struct BranchFinishEpilogue {
    branch: String,
    /// `None` for a bare checkout: there was no run behind the branch.
    run: Option<RunFinishEpilogue>,
    /// The issue this branch implemented and landed — archived on success.
    issue_id: Option<String>,
    /// The issue whose implementation this finish threw away — told so, and
    /// left in the inbox.
    orphaned_issue_id: Option<String>,
}

impl WorktreeFinishJob {
    /// The epilogue for this job, addressed to the verb that owns it.
    fn epilogue(&self, kind: FinishKind) -> FinishEpilogue {
        FinishEpilogue {
            worktree_id: self.worktree_id.clone(),
            project_id: self.project_id.clone(),
            kind,
        }
    }

    /// Every step of a finish that touches a disk, with the app mutex released.
    fn run(mut self) -> WorktreeFinishOutcome {
        #[cfg(test)]
        if let Some(gate) = self.gate.take() {
            gate.arrive();
        }
        let (mut record, scan) = match self.resume.take() {
            Some(record) => (record, None),
            None => {
                let scanned = match self.worktrees.discover(&self.base_branch, &self.excluded) {
                    Ok(scanned) => scanned,
                    Err(error) => {
                        return WorktreeFinishOutcome {
                            scan: None,
                            record: None,
                            result: Err(error.to_string()),
                        }
                    }
                };
                match self.preflight(&scanned) {
                    Ok(record) => (record, Some(scanned)),
                    // The scan is worth keeping even when the preflight refuses.
                    Err(error) => {
                        return WorktreeFinishOutcome {
                            scan: Some(scanned),
                            record: None,
                            result: Err(error),
                        }
                    }
                }
            }
        };

        // The durable intent, before anything destructive: a finish that dies
        // between here and the end resumes from this record.
        if let Err(error) = self.store.save_archived_worktree(&record) {
            return WorktreeFinishOutcome {
                scan,
                record: None,
                result: Err(format!("worktree finish intent store: {error}")),
            };
        }
        if let Err(error) = run_finish_git_steps(&self.worktrees, &self.base_branch, &record) {
            return WorktreeFinishOutcome {
                scan,
                record: Some(record),
                result: Err(error),
            };
        }

        let pending = record.clone();
        record.status = WorktreeFinishStatus::Archived;
        record.archived_at = Some(now_rfc3339());
        match self.store.save_archived_worktree(&record) {
            Ok(()) => WorktreeFinishOutcome {
                scan,
                result: Ok(archived_worktree_json(&record)),
                record: Some(record),
            },
            // The git is done but the archive is not recorded: the record stays
            // Pending, and the recovery sweep finishes it.
            Err(error) => WorktreeFinishOutcome {
                scan,
                record: Some(pending),
                result: Err(format!("worktree archive store: {error}")),
            },
        }
    }

    /// Resolve the requested checkout against a scan taken just now, recheck
    /// that it may be finished, checkpoint what the action would otherwise
    /// throw away, and describe it for the archive.
    fn preflight(&self, scanned: &[ExternalWorktree]) -> Result<PersistedArchivedWorktree, String> {
        let external = scanned
            .iter()
            .find(|worktree| worktree.id == self.worktree_id)
            .cloned()
            .ok_or_else(|| format!("unknown worktree_id: {}", self.worktree_id))?;

        ensure_worktree_finish_eligible(&external, self.action, &self.base_branch)?;
        let dirty_metadata = external.clone();
        if matches!(
            self.action,
            WorktreeFinishAction::Push | WorktreeFinishAction::Merge
        ) && external.dirty_files > 0
        {
            checkpoint_worktree(&external.path, self.action)?;
        }
        let head_sha = git_stdout(&external.path, &["rev-parse", "HEAD"])?
            .trim()
            .to_string();

        Ok(PersistedArchivedWorktree {
            status: WorktreeFinishStatus::Pending,
            project_path: self.worktrees.repo_path().display().to_string(),
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
            action: self.action,
            archived_at: None,
        })
    }
}

/// The destructive half of a finish, over the steps the chosen action owns.
/// The record is the only authority for what is acted on — a client path never
/// reaches here — and every checkout and branch it touches goes through the
/// façade, so a clone and a linked worktree are finished by one function.
fn run_finish_git_steps(
    worktrees: &WorktreeManager,
    base_branch: &str,
    record: &PersistedArchivedWorktree,
) -> Result<(), String> {
    let checkout = validate_finish_record_path(record, worktrees.repo_path())?;
    (record.action.git_steps())(&FinishContext {
        worktrees,
        base_branch,
        record,
        checkout: &checkout,
    })
}

/// Everything a finish action's steps may act on, resolved once. Each step
/// reads the fields its own work needs and nothing else.
struct FinishContext<'a> {
    worktrees: &'a WorktreeManager,
    base_branch: &'a str,
    record: &'a PersistedArchivedWorktree,
    checkout: &'a std::path::Path,
}

/// What one finish action does to the repository, once its checkout has been
/// resolved.
type FinishGitSteps = fn(&FinishContext<'_>) -> Result<(), String>;

/// How a finish action lands the work it promised to keep before the checkout
/// goes away. An action that promises nothing lands nothing.
type FinishLanding = fn(&FinishContext<'_>, &str) -> Result<(), String>;

impl WorktreeFinishAction {
    /// The steps this action owns — the one place a finish action decides
    /// anything from its own kind.
    fn git_steps(self) -> FinishGitSteps {
        match self {
            WorktreeFinishAction::Cleanup => remove_finished_checkout,
            WorktreeFinishAction::Push => push_then_remove_finished_checkout,
            WorktreeFinishAction::Merge => merge_finished_checkout,
            WorktreeFinishAction::Delete => delete_finished_checkout,
        }
    }

    /// What the human called for, as the wire spells it — the word a failure
    /// names this finish by.
    fn verb(self) -> &'static str {
        match self {
            WorktreeFinishAction::Cleanup => "cleanup",
            WorktreeFinishAction::Push => "push",
            WorktreeFinishAction::Merge => "merge",
            WorktreeFinishAction::Delete => "delete",
        }
    }
}

/// Be rid of the checkout a finish is done with. Absence is the goal, so a
/// checkout somebody already deleted is nothing to report.
fn remove_finished_checkout(context: &FinishContext<'_>) -> Result<(), String> {
    context
        .worktrees
        .remove_checkout(context.checkout)
        .map_err(|error| error.to_string())
}

fn push_then_remove_finished_checkout(context: &FinishContext<'_>) -> Result<(), String> {
    if !context.checkout.exists() {
        return Ok(());
    }
    crate::gitgui::push(context.checkout, false)?;
    remove_finished_checkout(context)
}

fn merge_finished_checkout(context: &FinishContext<'_>) -> Result<(), String> {
    finish_by_landing_then_removing(context, Some(merge_finished_branch_into_base))
}

fn delete_finished_checkout(context: &FinishContext<'_>) -> Result<(), String> {
    finish_by_landing_then_removing(context, None)
}

fn merge_finished_branch_into_base(
    context: &FinishContext<'_>,
    branch: &str,
) -> Result<(), String> {
    context
        .worktrees
        .merge_into_base(context.checkout, branch, context.base_branch)
        .map_err(|error| error.to_string())
}

/// Land what the action promised to keep, then take the checkout away — and
/// its branch with it when the checkout says teardown owns it.
fn finish_by_landing_then_removing(
    context: &FinishContext<'_>,
    land: Option<FinishLanding>,
) -> Result<(), String> {
    let standing = context.checkout.exists();
    let branch = finish_branch_still_present(context, standing)?;
    if !refuse_finish_that_lost_its_checkout(context, standing, branch)? {
        return Ok(());
    }
    let deleted_branch = match branch {
        Some(branch) => land_then_delete_branch_if_owned(context, branch, land)?,
        None => false,
    };
    remove_finished_checkout_restoring_branch_on_failure(context, deleted_branch)
}

/// The branch this finish acts on, if the record names one and the project repo
/// still has it. Spec §0.4's publish-before-read step is here: whatever a
/// standing checkout holds reaches the project repo first — for a clone the only
/// way its branch is there at all, for a linked worktree nothing — and only then
/// is the project repo asked. A checkout with no branch (a detached HEAD) or
/// whose branch is already gone has nothing for the finish to land or delete.
fn finish_branch_still_present<'a>(
    context: &FinishContext<'a>,
    standing: bool,
) -> Result<Option<&'a str>, String> {
    let Some(branch) = context.record.branch.as_deref() else {
        return Ok(None);
    };
    if standing {
        context
            .worktrees
            .publish(context.checkout, branch)
            .map_err(|error| error.to_string())?;
    }
    let stands = context
        .worktrees
        .branch_exists(branch)
        .map_err(|error| error.to_string())?;
    Ok(stands.then_some(branch))
}

/// Whether there is still a checkout to act on. A checkout that vanished
/// while a branch is still at stake is refused: the finish promised to land
/// or delete that branch from a checkout it no longer has. One that vanished
/// with no branch behind it has simply finished already.
fn refuse_finish_that_lost_its_checkout(
    context: &FinishContext<'_>,
    standing: bool,
    branch: Option<&str>,
) -> Result<bool, String> {
    if standing {
        return Ok(true);
    }
    match branch {
        Some(_) => Err(format!(
            "worktree.finish {} lost its worktree before branch deletion",
            context.record.action.verb()
        )),
        None => Ok(false),
    }
}

/// Land the branch as the action promised, then delete it when the checkout
/// says teardown owns it. Answers whether the branch was deleted, so a
/// removal that fails afterwards knows what to put back.
///
/// The teardown is read here, before the removal prunes the admin directory
/// the answer lives in.
fn land_then_delete_branch_if_owned(
    context: &FinishContext<'_>,
    branch: &str,
    land: Option<FinishLanding>,
) -> Result<bool, String> {
    let deletes_branch = crate::worktree::branch_teardown(context.checkout)
        .map_err(|error| error.to_string())?
        .deletes_branch();
    if let Some(land) = land {
        land(context, branch)?;
    }
    if deletes_branch {
        context
            .worktrees
            .delete_branch_at(branch, &context.record.head_sha)
            .map_err(|error| error.to_string())?;
    }
    Ok(deletes_branch)
}

fn remove_finished_checkout_restoring_branch_on_failure(
    context: &FinishContext<'_>,
    deleted_branch: bool,
) -> Result<(), String> {
    let Err(remove_error) = remove_finished_checkout(context) else {
        return Ok(());
    };
    if !deleted_branch {
        return Err(remove_error);
    }
    let Some(branch) = context.record.branch.as_deref() else {
        return Err(remove_error);
    };
    match context
        .worktrees
        .restore_branch(branch, &context.record.head_sha)
    {
        Ok(()) => Err(remove_error),
        Err(restore_error) => Err(format!(
            "{remove_error}; restoring branch {branch:?} after removal failure also failed: \
             {restore_error}"
        )),
    }
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

/// Whether a Pending finish record's git is in fact already done — the boot
/// question that turns an interrupted finish into an archived one. It is asked
/// before any project is registered, so the record's own two paths are the
/// only thing that can say which repository to put it to.
fn finish_git_steps_are_complete(record: &PersistedArchivedWorktree) -> bool {
    if std::path::Path::new(&record.worktree_path).exists() {
        return false;
    }
    match record.action {
        WorktreeFinishAction::Cleanup | WorktreeFinishAction::Push => true,
        WorktreeFinishAction::Merge | WorktreeFinishAction::Delete => {
            record.branch.as_deref().is_none_or(|branch| {
                worktrees_of_record(record)
                    .branch_exists(branch)
                    .is_ok_and(|exists| !exists)
            })
        }
    }
}

/// The checkout seam for the project a finish record names, rooted where that
/// record's own checkout stood — the two paths a record carries, and all a
/// completeness check needs to ask the project repo about its branches.
fn worktrees_of_record(record: &PersistedArchivedWorktree) -> WorktreeManager {
    let checkout = std::path::Path::new(&record.worktree_path);
    WorktreeManager::new(
        std::path::PathBuf::from(&record.project_path),
        checkout.parent().unwrap_or(checkout),
    )
}

/// Everything one run's stage publications have to be decided against, taken
/// under the state lock so the deciding needs none.
struct StagePublicationQuery {
    run_id: String,
    /// The project's checkout seam, when the project is still registered. A
    /// classification reads the project repo's refs, which the checkout's
    /// branch has to reach through `publish` first.
    worktrees: Option<WorktreeManager>,
    checkout: std::path::PathBuf,
    branch: String,
    base_branch: String,
    /// One entry per stage that reached a completion commit. A stage without
    /// one published nothing by definition and costs no git at all.
    completions: Vec<(String, String)>,
}

/// What git says about each of a run's completed stages.
#[derive(Default)]
struct StagePublications(HashMap<String, StagePublication>);

/// One vanished run, with git's verdict on its stages already in hand.
struct DecidedVanishedRun {
    run_id: String,
    published: StagePublications,
}

/// The vanished runs one board read found, on their way to a verdict.
struct VanishedRunSweep {
    queries: Vec<StagePublicationQuery>,
    #[cfg(test)]
    gate: Option<OffLockGate>,
}

impl OffLockJob for VanishedRunSweep {
    type Claim = ();
    type Decided = Vec<DecidedVanishedRun>;

    fn claim(&self) {}

    /// The git half — a bounded fetch and two graph walks per completed stage,
    /// per run.
    fn decide(self) -> Vec<DecidedVanishedRun> {
        #[cfg(test)]
        if let Some(gate) = self.gate {
            gate.arrive();
        }
        self.queries
            .into_iter()
            .map(DecidedVanishedRun::decide)
            .collect()
    }

    fn apply(state: &mut AppState, (): (), decided: Vec<DecidedVanishedRun>) {
        state.archive_vanished_runs(decided);
    }

    fn abandon(state: &mut AppState, (): ()) {
        state.vanished_run_sweep_in_flight = false;
    }
}

impl StagePublicationQuery {
    /// The git half: a bounded fetch and two graph walks per completed stage.
    /// MUST run with the state lock released.
    fn classify(&self) -> StagePublications {
        let Some(worktrees) = self.worktrees.as_ref() else {
            return StagePublications::default();
        };
        StagePublications(
            self.completions
                .iter()
                .map(|(stage_id, completion_sha)| {
                    (
                        stage_id.clone(),
                        classify_stage_publication(
                            worktrees,
                            &self.checkout,
                            &self.branch,
                            &self.base_branch,
                            completion_sha,
                        ),
                    )
                })
                .collect(),
        )
    }
}

impl DecidedVanishedRun {
    fn decide(query: StagePublicationQuery) -> DecidedVanishedRun {
        DecidedVanishedRun {
            published: query.classify(),
            run_id: query.run_id,
        }
    }
}

impl StagePublications {
    /// A stage nobody asked git about published nothing: no completion commit,
    /// or no repository left to open.
    fn of(&self, stage_id: &str) -> StagePublication {
        self.0
            .get(stage_id)
            .copied()
            .unwrap_or(StagePublication::Local)
    }
}

/// Write git's verdict onto a vanished run's stages: one whose commits never
/// left this machine, or that was never validated, is marked incomplete.
/// Returns the stages that were. Pure bookkeeping — the git it judges by is
/// [`StagePublicationQuery::classify`].
fn reconcile_missing_run_worktree(
    active: &mut ActiveRun,
    published: &StagePublications,
) -> Vec<String> {
    let mut affected = Vec::new();
    for progress in &mut active.stages {
        let publication = published.of(&progress.stage_id);
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

/// How far a stage's completion commit has travelled, read from the project
/// repo's own refs — which the checkout's branch reaches first, because a
/// clone's tip is invisible there until it is published. A publish that fails
/// is no verdict: classification still has to answer, so it is said and passed
/// over.
fn classify_stage_publication(
    worktrees: &WorktreeManager,
    checkout: &std::path::Path,
    branch: &str,
    base_branch: &str,
    completion_sha: &str,
) -> StagePublication {
    if checkout.exists() {
        if let Err(error) = worktrees.publish(checkout, branch) {
            eprintln!(
                "classify_stage_publication {branch}: publishing {} failed: {error}",
                checkout.display()
            );
        }
    }
    let repo_path = worktrees.repo_path();
    let Ok(repo) = git2::Repository::open(repo_path) else {
        return StagePublication::Local;
    };
    let Ok(completion) = git2::Oid::from_str(completion_sha) else {
        return StagePublication::Local;
    };
    // Push success is remote evidence, not merely a local command result. Make
    // the configured remote-tracking ref current before classifying; the fetch
    // is noninteractive and timeout-bounded by the shared recovery helper.
    if let Some(remote) = configured_remote_for_branch(&repo, branch) {
        let refspec = format!("+refs/heads/{branch}:refs/remotes/{remote}/{branch}");
        let _ = bounded_git_fetch(repo_path, &remote, &refspec);
    }
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

/// What the checkout's archive record adds to an archived row: how it was
/// finished, and where it stood when it was. Overwrites only the keys it owns,
/// so a run's own facts (title, state, the branch it ran on) survive.
fn merge_archived_worktree_facts(
    row: &mut serde_json::Map<String, Value>,
    record: &PersistedArchivedWorktree,
) {
    row.insert("worktree_id".into(), json!(record.worktree_id));
    row.insert("action".into(), json!(record.action));
    row.insert("head_sha".into(), json!(record.head_sha));
    row.insert("upstream".into(), json!(record.upstream));
    row.insert("unpushed".into(), json!(record.unpushed));
    row.insert("dirty_files".into(), json!(record.dirty_files));
    if row.get("finished_at").is_none_or(Value::is_null) {
        row.insert("finished_at".into(), json!(record.archived_at));
    }
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

/// What a read has to tell the agent about the indicator it just started.
/// Reading stamps `seen_at`, which is what the reviewer sees as "Working" with
/// a running timer; posting a reply (`still_working` omitted or false) hands the
/// turn back and stops it.
const WORKING_INDICATOR_NOTICE: &str = "Reading these marked them seen, which started the reviewer's \"Working\" indicator and its timer on the newest message. It runs until you post a reply with post_thread_message — an ordinary reply (still_working omitted or false) hands the turn back and stops it; a progress note (still_working: true) keeps it running. The `done` tool also stops it. Do not leave it running after you have finished.";

const NEW_THREAD_MESSAGES_PROMPT: &str =
    "New reviewer messages are available. Call `read_unread_messages` now, then act on every unread message. Reply with `post_thread_message` only when the conversation policy requires a written response.";

/// Tell the worktree's agent, in place, that unread thread messages await.
///
/// [`deliver`]'s warm branch without the cold half: it notifies whatever agent
/// is ALIVE in that worktree, whatever its entity is parked as, and does
/// nothing at all for one that is not. Deciding between the two — and starting
/// the agent that is not running — belongs to
/// [`AppState::tell_the_agent_a_message_is_waiting`], the only caller.
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
///
/// Unlike [`deliver`], this speaks from under the app-wide state lock — it
/// reads the caller's own tab registry — which is why
/// [`AgentSession::send_turn`] must return promptly. A session that blocked
/// there would stall every RPC and every terminal pump behind one nudge.
#[cfg(test)]
fn nudge_live_agent_tab(
    tabs: &HashMap<TabKey, Tab>,
    root: &std::path::Path,
    agent_id: &str,
    entity_id: &str,
    interrupt: bool,
) {
    let Some(tab) = tabs.get(&TabKey::agent(&AppState::canonical_root(root), agent_id)) else {
        return;
    };
    if !tab.session_is_live() {
        return;
    }
    // Stop first, then hand over — the order is the whole point of the flag
    // riding the message rather than arriving as a verb of its own, which would
    // leave a window in which the child starts a fresh turn or the agent calls
    // `done`. Both calls return promptly by contract, which is what lets them
    // speak from under the state lock.
    //
    // A refusal is not a failed post. Where the session cannot stop a turn —
    // a capability lost between the digest the client read and the post it sent
    // — the message is delivered as an ordinary queued turn, which reaches the
    // running turn at its next step boundary anyway. The alternative is an
    // error the human must read for a difference they cannot act on and did not
    // cause.
    if interrupt {
        if let Err(refused) = tab.session.interrupt() {
            eprintln!("thread.post {entity_id}: interrupt refused: {refused}");
        }
    }
    // As a turn, not a raw write with a hardcoded Enter: the nudge is one of
    // Build's turns, so it travels the way every other one does and the
    // session decides what that means. Hardcoding \r submits into a SubmitKey::None
    // harness that never asked for it, leaves the notification unframed — and
    // says nothing at all to a session with no keyboard.
    if let Err(error) = tab
        .session
        .send_turn(&Turn::new(NEW_THREAD_MESSAGES_PROMPT))
    {
        eprintln!("thread.post {entity_id}: agent notify failed: {error}");
    }
}

/// Where an issue's one agent is running right now — its checkout and its
/// agent id — or `None` when the issue has no session. Read BEFORE a verb that
/// ends the session, since ending it is what clears the workspace.
fn issue_session(active: &ActivePlan) -> Option<(std::path::PathBuf, String)> {
    let workspace = active.workspace.as_ref()?;
    Some((workspace.checkout.clone(), active.agents.sole().id.clone()))
}

/// Open a conversation's session lineage for a newly spawned agent process,
/// chaining it off the previous session so the thread still reads as a chain.
fn open_session_lineage(
    thread: &mut crate::thread::Thread,
    entity_id: &str,
    agent_id: &str,
    checkout: &str,
    model_choice: &ModelChoice,
    phase: &str,
) -> SessionInstance {
    let now = now_rfc3339();
    let instance = thread.start_agent_session(SessionStart {
        entity_id,
        agent_id,
        checkout,
        provider: model_choice.provider.label(),
        model: model_choice.model.as_deref(),
        effort: model_choice.effort.as_deref(),
        phase,
        now: &now,
    });
    thread.push_event(
        crate::thread::ThreadEventKind::RunStarted,
        Some(format!("{phase} run started")),
        Some(instance.id.clone()),
        None,
        now_rfc3339(),
    );
    instance
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
            vec![crate::thread::ThreadLink::IssueStage {
                issue_id: plan_id.to_string(),
                stage_id: stage.id.clone(),
                path: stage.path.clone(),
            }],
            &now,
        );
    }
}

fn recovery_agent_prompt(
    recovery_id: &str,
    issue_id: &str,
    run_id: &str,
    requested_stage_id: &str,
    worktree: &Worktree,
    restore_error: &str,
    stages: &[StageDoc],
) -> String {
    let catalog = if stages.is_empty() {
        "- No stage plans exist yet.".to_string()
    } else {
        stages
            .iter()
            .enumerate()
            .map(|(index, stage)| {
                format!(
                    "{}. {} — {} — {} — {:?}",
                    index + 1,
                    stage.id,
                    stage.title,
                    stage.path,
                    stage.state
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "You are a RECOVERY agent for an Issue implementation. Work read-only except for restoring the exact persisted branch ref and its registered worktree.\n\nRecovery nonce: {recovery_id}\nIssue: {issue_id}\nImplementation: {run_id}\nRequested stage: {requested_stage_id}\nExact branch: {}\nExpected worktree path: {}\nInitial restore error: {restore_error}\n\nOrdered Issue stage-plan catalog (authoritative order):\n{catalog}\n\nInspect local refs, configured remotes, reflogs, and reachable commits. Never recreate from the moving base. If you can restore the exact branch lineage, do so, then call `done` with phase=\"recover\", status=\"completed\", outputs.recovery={{\"recovery_id\":\"{recovery_id}\",\"recovered\":true,\"branch\":\"{}\",\"head_sha\":\"<40 lowercase hex>\",\"findings\":\"verified evidence\"}}. If exact lineage cannot be recovered, report recovered=false with the same nonce and verified findings.",
        worktree.recorded_branch,
        worktree.path.display(),
        worktree.recorded_branch,
    )
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
        vec![
            crate::thread::ThreadLink::IssueStage {
                issue_id: plan_id.clone(),
                stage_id: stage.id.clone(),
                path: stage.path.clone(),
            },
            crate::thread::ThreadLink::Implementation {
                issue_id: plan_id,
                implementation_id: active.run.id.0.clone(),
            },
        ],
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
fn finish_open_session(thread: &mut crate::thread::Thread, agent_id: &str, now: &str) {
    let Some(instance) = thread.open_session_instance(agent_id) else {
        return;
    };
    thread.finish_session_instance(&instance, now);
}

/// The conversation's open session, if one is open — the agent process
/// speaking right now, which is what an event it produces belongs to.
fn open_session_id(thread: &crate::thread::Thread, agent_id: &str) -> Option<String> {
    thread
        .open_session_instance(agent_id)
        .map(|instance| instance.id)
}

/// Close every conversation an abandoned run was holding open. The run is out
/// of the map by now, so its lineage closes on the record this call holds
/// rather than through the owner lookup.
fn close_abandoned_run_conversations(active: &mut ActiveRun) {
    let now = now_rfc3339();
    if let Some(primary) = active.agents.primary_mut() {
        let agent_id = primary.id.clone();
        finish_open_session(&mut primary.thread, &agent_id, &now);
        primary.thread.push_event(
            crate::thread::ThreadEventKind::Abandoned,
            Some("Run abandoned".to_string()),
            None,
            None,
            now.clone(),
        );
    }
    // A branch may carry several agents and the decide phase took every one of
    // them. `Abandoned` closed the first agent's turn (and, for a planned
    // implementation, its Issue's — see `mirror_run_outcome_to_issue`); the
    // agents beside it were told nothing, so each one that died mid-turn is
    // closed on its own conversation. Every other teardown path removes the run
    // from the board entirely, so there is no row left to read as working.
    for agent in active.agents.iter_mut() {
        if agent.thread.working_since().is_some() {
            record_session_death_in_thread(&mut agent.thread, &now);
        }
    }
}

/// What an issue's conversation says when the branch implementing it is gone
/// and nothing was merged out of it. `how` is the way it went: abandoned by the
/// user, deleted outside Build, finished off the board.
fn abandoned_branch_summary(branch: &str, how: &str) -> String {
    format!(
        "The branch {branch} was {how} without being merged, so this issue is waiting for work \
         again"
    )
}

/// Whether one of an implementation's events travels to the Issue that owns
/// it. Exactly the attention class: what needs the human is news wherever they
/// are watching from, what merely reports progress belongs to the run.
///
/// Events only, because an outcome is no longer one: a report the agent made on
/// a planned implementation is written straight onto the Issue's conversation
/// as the agent's own message (`record_report_in_thread`, whose caller picks
/// that conversation), which is the same timeline this mirror copies onto and
/// the same one unread entry. What still travels this way is what Build
/// observed about the implementation itself — an abandoned branch.
fn run_outcome_mirrors_to_issue(event: crate::thread::ThreadEventKind) -> bool {
    event.class() == crate::thread::EventClass::Attention
}

/// What the daemon says about a `done` the branch's lifecycle does not accept.
///
/// Branch state belongs to the branch, not to whoever is talking to it
/// (Decisions §Entity model): a branch carries several agents, and one
/// dispatched onto a branch that is already sitting at a review gate finishing
/// its own instruction is that model working exactly as designed. So the report
/// is recorded, its attention event fires, the branch stays where it is — and
/// the line says so, instead of reading like a rejected transition somebody
/// needs to go and fix.
fn out_of_phase_log(run_id: &str, illegal: &crate::run::IllegalRunTransition) -> String {
    format!(
        "on_agent_done {run_id}: dispatched-agent report recorded; branch state unchanged \
         ({:?} while the branch is {:?})",
        illegal.event, illegal.from
    )
}

/// What the conversation says when the reviewer disagrees with a hunk's level.
///
/// It names the file rather than the hunk id, because the id is a hash and the
/// agent that has to learn from this reads in files. It says which way the pass
/// was wrong, which is the whole content of the disagreement. It quotes the
/// rationale the pass gave, so the agent can see which of its own claims was
/// not believed rather than having to go and find it. And the reviewer's own
/// note, when they left one, goes last and unedited.
fn triage_override_summary(
    direction: crate::run::OverrideDirection,
    path: &str,
    rationale: Option<&str>,
    note: Option<&str>,
) -> String {
    let mut lines = vec![match direction {
        crate::run::OverrideDirection::Surface => {
            format!("The reviewer opened {path}: triage collapsed a change that needed reading.")
        }
        crate::run::OverrideDirection::Collapse => {
            format!("The reviewer collapsed {path}: triage surfaced a change that did not.")
        }
    }];
    if let Some(rationale) = rationale.map(str::trim).filter(|it| !it.is_empty()) {
        lines.push(format!("Triage said: {rationale}"));
    }
    if let Some(note) = note {
        lines.push(note.to_string());
    }
    lines.join("\n\n")
}

/// How a report is written down.
///
/// An outcome the agent reported is a status on the agent's own message: it
/// said this, so there is one record of it and the conversation carries it.
/// An event is Build's own reading of the report — a triage pass nobody has to
/// answer, a validation Build judged — which has no agent message to hang on.
enum ReportRecord {
    Outcome(crate::thread::MessageOutcome, String),
    Event(crate::thread::ThreadEventKind, String),
}

/// The conversation a RUN's report is written on: the owning Issue's when the
/// run is a planned one — that is the conversation its surfaces render, and a
/// report on the run's own thread would never be seen — else the run's own.
///
/// The run's own is reached through the mint door: the report came from an
/// agent of this run, so it must land somewhere even if the human emptied the
/// roster mid-turn.
fn run_report_conversation<'a>(
    run_id: &str,
    active: &'a mut ActiveRun,
    issue: Option<&'a mut ActivePlan>,
) -> &'a mut crate::thread::Thread {
    match issue {
        Some(issue) => issue.agents.sole_thread_mut(),
        None => {
            let choice = active.model_choice.clone();
            &mut active
                .agents
                .ensure_primary(run_id, choice, &now_rfc3339())
                .thread
        }
    }
}

fn record_report_in_thread(
    thread: &mut crate::thread::Thread,
    report: &DoneReport,
    orchestration_error: Option<&str>,
) {
    let now = now_rfc3339();
    let recorded = match orchestration_error {
        // Still the agent's report, and still its outcome: Build's note about
        // why it could not be applied rides the same body.
        Some(error) => ReportRecord::Outcome(
            crate::thread::MessageOutcome::Failed,
            format!(
                "{}\n\nBuild could not apply the report: {error}",
                report.summary
            ),
        ),
        None if report.status == DoneStatus::Blocked => ReportRecord::Outcome(
            crate::thread::MessageOutcome::Blocked,
            report.summary.clone(),
        ),
        // A finished triage pass is not an agent handing work back: nothing
        // waits on it and nobody has to answer it. It updates the review
        // surface, and says so quietly.
        None if report.phase == DonePhase::Triage => ReportRecord::Event(
            crate::thread::ThreadEventKind::Triaged,
            report.summary.clone(),
        ),
        None if report.status == DoneStatus::Failed => ReportRecord::Outcome(
            crate::thread::MessageOutcome::Failed,
            report.summary.clone(),
        ),
        None if report
            .outputs
            .validation
            .as_ref()
            .is_some_and(|validation| !validation.passed) =>
        {
            ReportRecord::Event(
                crate::thread::ThreadEventKind::ReviewBlocked,
                report
                    .outputs
                    .validation
                    .as_ref()
                    .map(|validation| validation.findings.clone())
                    .unwrap_or_else(|| report.summary.clone()),
            )
        }
        _ => ReportRecord::Outcome(
            crate::thread::MessageOutcome::Completed,
            report.summary.clone(),
        ),
    };
    let completion = report.outputs.completion_report.as_ref();
    match recorded {
        ReportRecord::Outcome(outcome, summary) => {
            thread.post_outcome(outcome, summary, completion, &now);
        }
        ReportRecord::Event(event, summary) => {
            thread.push_event(event, Some(summary), None, None, &now);
        }
    }
    if let Some(completion) = completion {
        thread.remember_completion(completion);
    }
}

/// How a harness's session ended, as the idle sweep saw it: the exit code, and
/// the last thing it painted. A crash's only explanation is usually on its own
/// screen — codex refusing to start a required MCP server, a provider saying
/// the account is out of quota — and a bare code throws that away.
#[derive(Debug, Clone)]
struct HarnessExit {
    code: i32,
    epitaph: Option<String>,
}

impl HarnessExit {
    /// The crash as one line of `last_error`.
    fn describe(&self) -> String {
        match &self.epitaph {
            Some(said) => format!(
                "agent exited unexpectedly (exit code {}): {said}",
                self.code
            ),
            None => format!("agent exited unexpectedly (exit code {})", self.code),
        }
    }
}

/// An entity went quiet (or its agent exited) without reporting: record the
/// reason. The session lineage is deliberately left alone — a quiet agent is
/// still an agent, and one that exited has already had its session closed by
/// the pump that saw the EOF.
fn record_idle_in_thread(thread: &mut crate::thread::Thread, exit: Option<&HarnessExit>) {
    let now = now_rfc3339();
    let (event, summary) = match exit {
        Some(exit) => (
            crate::thread::ThreadEventKind::RunFailed,
            match &exit.epitaph {
                Some(said) => {
                    format!("Agent exited unexpectedly with code {}: {said}", exit.code)
                }
                None => format!("Agent exited unexpectedly with code {}", exit.code),
            },
        ),
        None => (
            crate::thread::ThreadEventKind::IdleUnreported,
            "Agent went quiet without reporting done".to_string(),
        ),
    };
    thread.push_event(event, Some(summary), None, None, now);
}

/// What a conversation says when the agent's PROCESS ended with a turn still in
/// flight: nobody is coming back to hand it over, so the turn is closed here.
///
/// `Interrupted` rather than `IdleUnreported`, because the two differ by whether
/// the agent is still there. `IdleUnreported` reads "went quiet without
/// reporting done" — an agent alive at its prompt with nothing to say. A killed
/// harness is not quiet, it is gone, which is exactly what `Interrupted` already
/// means ("the session did not survive"); boot recovery writes the same event
/// for the same reason after a daemon restart. It is attention-class, and that
/// class is what ENDS a turn — so `working_since` goes `None`, the row and the
/// agent's bubble stop claiming work is happening, and the entry says why.
fn record_session_death_in_thread(thread: &mut crate::thread::Thread, now: &str) {
    thread.push_event(
        crate::thread::ThreadEventKind::Interrupted,
        Some(SESSION_DIED_SUMMARY.to_string()),
        None,
        None,
        now,
    );
}

/// What the conversation reads when a harness died mid-turn.
const SESSION_DIED_SUMMARY: &str = "The agent's session ended without reporting back";

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
fn dispatch_frame(
    state: &Arc<Mutex<AppState>>,
    sender: SessionSender,
    frame: Frame,
    timer: FrameTimer,
) -> Value {
    // A session ended (client `close` frame, or the relay's session_closed on
    // browser disconnect): release its attachments so the bridge stops encrypting
    // terminal output into a session nobody will ever read.
    if frame.frame_type == transport::CLOSE_FRAME_TYPE {
        let changes = {
            let mut app = timer.lock(state);
            app.drop_session(sender.session_id());
            app.changes()
        };
        changes.unsubscribe(sender.session_id());
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
        // The greeting: what this bridge can do for the session, and — for the
        // capabilities that need somewhere to send to — the subscription
        // itself. Needs the caller's own `SessionSender`, which is why it is
        // here and not in `route`.
        "session.hello" => session_hello(state, &sender, &timer),
        // Answered from the frame clock alone, never from `AppState`: the frame
        // that asks what is wedging the daemon must not queue behind the wedge.
        "bridge.stats" => Ok(timer.clock().stats()),
        "stream.start" => stream_start(state, &params, &timer),
        "rtc.offer" => rtc_offer(state, &sender, &params, &timer),
        "rtc.ice" => rtc_ice(state, sender.session_id(), &params, &timer),
        "rtc.close" => rtc_close(state, sender.session_id(), &timer),
        // Opening a terminal or an agent in a worktree IS interacting with it in
        // Build — it is the reason a hand-made worktree graduates out of the
        // Worktrees row. This arm bypasses `dispatch`, so it stamps for itself.
        "term.create" => {
            let created = term_create(state, &params, &timer);
            if created.is_ok() {
                if let Some(scope_id) = params
                    .get("run_id")
                    .or_else(|| params.get("worktree_id"))
                    .and_then(Value::as_str)
                {
                    timer.lock(state).touch_attention(scope_id);
                }
            }
            created
        }
        "term.attach" => term_attach(state, &sender, &params, &timer),
        // A write to a child's pty blocks while the child is not draining, so
        // both of these take the handle under the lock and write with it
        // released.
        "term.input" => term_input(state, &params, &timer),
        "term.resize" => term_resize(state, &params, &timer),
        // Needs the caller's own session: an ack speaks for one client's
        // receive queue, not for the screen.
        "term.ack" => term_ack(state, &sender, &params, &timer),
        "agent.attach" => agent_attach(state, &sender, &params, &timer),
        // Bypasses `dispatch` because it hands its queued turn to
        // `DeliveryRunner`, which needs the shared handle `dispatch` does not
        // have.
        "agent.start" => agent_start(state, &params, &timer),
        _ => {
            // A verb whose git work must not run under the lock hands that
            // work back rather than doing it here; the drain below runs it with
            // the mutex released. See `AppState::deferred_work`.
            let (dispatched, deferred) = timer.lock(state).dispatch_deferring(&method, &params);
            let dispatched = match deferred {
                Some(deferred) => {
                    // THE POINT OF ALL THIS: seconds to minutes of git — a
                    // status walk, a fetch, a merge, a `git worktree remove` of
                    // a six-gigabyte checkout — with every other frame, every
                    // terminal pump and the relay's own read loop free to make
                    // progress meanwhile.
                    let done = deferred.run();
                    timer.lock(state).apply_deferred(&method, &params, done)
                }
                None => dispatched,
            };
            // A verb speaks to a worktree's agent by queuing a turn, and the
            // frame's own answer never waits for it to arrive: the mutation is
            // durable, and a cold spawn blocks for seconds on the harness's
            // readiness wait while the browser gives up at twelve.
            if dispatched.is_ok() {
                DeliveryRunner::drain(state, &timer);
            }
            dispatched
        }
    };
    match result {
        Ok(result) => json!({ "id": id, "ok": true, "result": result }),
        Err(message) => json!({ "id": id, "ok": false, "error": message }),
    }
}

/// Answer the browser's offer for this session (spec §Signaling), opening the
/// session's one peer connection if this is its first offer.
///
/// The ICE servers the browser fetched from the api ride with every offer and
/// with nothing else, so the bridge needs no Cloudflare credential of its own
/// and a restart carries fresh ones to the peer it already has.
fn rtc_offer(
    state: &Arc<Mutex<AppState>>,
    sender: &SessionSender,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, String> {
    let sdp = require_str(params, "sdp")?;
    let ice_servers = require_array(params, "ice_servers")?;
    let peers = timer.lock(state).peers();
    #[cfg(test)]
    let gate = timer.lock(state).off_lock_gate.clone();
    #[cfg(test)]
    if let Some(gate) = gate {
        gate.arrive();
    }
    let answer = peers
        .offer(sender.session_id(), &sdp, &ice_servers, sender.clone())
        .map_err(|e| e.to_string())?;
    Ok(json!({ "sdp": answer }))
}

fn rtc_ice(
    state: &Arc<Mutex<AppState>>,
    session_id: &str,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, String> {
    let candidate = require_value(params, "candidate")?;
    let peers = timer.lock(state).peers();
    peers
        .candidate(session_id, candidate)
        .map_err(|e| e.to_string())?;
    Ok(json!({}))
}

/// The browser gave up on the peer carrier: tear this session's peer down and
/// leave the session working over the relay.
fn rtc_close(
    state: &Arc<Mutex<AppState>>,
    session_id: &str,
    timer: &FrameTimer,
) -> Result<Value, String> {
    let peers = timer.lock(state).peers();
    peers.close(session_id).map_err(|e| e.to_string())?;
    Ok(json!({}))
}

/// Greet a browser session: announce what this bridge pushes, and subscribe the
/// session to it.
///
/// `push_events: true` is the feature detection. A bridge that predates push
/// invalidation answers `unknown method: session.hello`, and a client that
/// predates it never asks — so a new SPA against an old bridge, and an old SPA
/// against this one, both fall back to polling with nothing to configure.
///
/// Idempotent: a client may greet again after a reconnect, and the bus keeps
/// one subscription per session id.
fn session_hello(
    state: &Arc<Mutex<AppState>>,
    sender: &SessionSender,
    timer: &FrameTimer,
) -> Result<Value, String> {
    // The subscribe happens with the app mutex released — it takes the bus's
    // own leaf lock, and nothing in this daemon may nest one lock inside
    // another it did not have to.
    let changes = timer.lock(state).changes();
    changes.subscribe(sender);
    Ok(json!({
        "push_events": true,
        "events": ANNOUNCED_EVENTS,
        "coalesce_window_ms": changes.window().as_millis() as u64,
        "thread_post_operations": {
            "version": 1,
            "status_method": "thread.operation",
            "states": ["queued", "claimed", "delivered", "uncertain"],
        },
    }))
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
fn term_create(
    state: &Arc<Mutex<AppState>>,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, String> {
    let cols = params.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16;
    let rows = params.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16;
    let scope = TermScope::parse(params)?;
    require_shell_kind(params)?;

    let (key, pumps) = {
        let mut s = timer.lock(state);
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
        let (tab, rx) = Tab::spawn_shell(
            &shell_harness_spec(&shell),
            tab_id,
            root,
            terminal_size(cols, rows),
        )?;
        let pumps = tab.pumps(rx);
        s.tabs.insert(key.clone(), tab);
        (key, pumps)
    };
    spawn_tab_pumps(state, key.clone(), pumps);
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
    timer: &FrameTimer,
) -> Result<Value, String> {
    let term_id = require_str(params, "term_id")?;
    let cols = params.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16;
    let rows = params.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16;

    let attachment = {
        let s = timer.lock(state);
        let key = s.tab_key_of_wire_id(&term_id)?;
        s.attachment(&key)?
    };
    Ok(attach_to_tab(attachment, sender, cols, rows))
}

/// Write client keystrokes (base64) to a tab's PTY, by id. Input to the agent
/// tab is allowed by design — its PTY is a full terminal on the user's machine
/// and the terminal is the basement — and an agent whose process has ended
/// surfaces "no active agent session" rather than swallowing the keystrokes.
///
/// An agent with no terminal has no basement to type into, and hears about it
/// ([`no_terminal_here`]) before its state is consulted: that is a property of
/// the session, not of whether it happens to be running.
///
/// The handle is taken under the lock and written to with it RELEASED. A child
/// that has stopped draining its pty blocks the write for as long as it likes;
/// under the mutex that one child wedges the whole daemon, and off it, one
/// worker.
fn term_input(
    state: &Arc<Mutex<AppState>>,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, String> {
    let term_id = require_str(params, "term_id")?;
    let data = b64decode(&require_str(params, "data")?)?;
    let terminal = {
        let s = timer.lock(state);
        let key = s.tab_key_of_wire_id(&term_id)?;
        let tab = s.tabs.get(&key).ok_or("unknown term_id")?;
        let terminal = tab.terminal_handle()?;
        if !tab.session_is_live() {
            return Err("no active agent session".to_string());
        }
        terminal
    };
    terminal.write_input(&data)?;
    Ok(json!({ "ok": true }))
}

/// Resize a tab's PTY and screen model, by id. The resize only applies while
/// the session is live; a dead resize is a no-op `live: false` so a retained
/// last screen is never garbled.
///
/// A session with no terminal refuses instead, live or not: a viewport means
/// nothing to a session with no grid, so `live: false` there would be a quiet
/// "nothing to do" in place of a reason.
///
/// Off the lock for the same reason as [`term_input`]: the ioctl goes to a
/// child that may not answer.
fn term_resize(
    state: &Arc<Mutex<AppState>>,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, String> {
    let term_id = require_str(params, "term_id")?;
    let cols = params.get("cols").and_then(Value::as_u64).unwrap_or(80) as u16;
    let rows = params.get("rows").and_then(Value::as_u64).unwrap_or(24) as u16;
    let (live, terminal) = {
        let s = timer.lock(state);
        let key = s.tab_key_of_wire_id(&term_id)?;
        let tab = s.tabs.get(&key).ok_or("unknown term_id")?;
        (tab.session_is_live(), tab.terminal_handle()?)
    };
    if live {
        terminal.resize(cols, rows)?;
    }
    Ok(json!({ "ok": true, "live": live }))
}

/// Report how far this client has applied a tab's output — the client half of
/// terminal flow control, on the same id space as every other `term.*` verb
/// (`term-<n>` or `agent:<worktree_id>`).
///
/// Advisory by design: it moves one number and may push one resync snapshot to
/// the caller. An unknown id errors like the rest of the family, so a stale
/// client drops the tab rather than acking into a terminal that is gone.
fn term_ack(
    state: &Arc<Mutex<AppState>>,
    sender: &SessionSender,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, String> {
    let term_id = require_str(params, "term_id")?;
    let cursor = params
        .get("cursor")
        .and_then(Value::as_u64)
        .ok_or("missing cursor")?;
    let screen = {
        let s = timer.lock(state);
        let key = s.tab_key_of_wire_id(&term_id)?;
        let tab = s.tabs.get(&key).ok_or("unknown term_id")?;
        // A client that was never allowed to attach has nothing to acknowledge,
        // so it hears the same refusal rather than acking into a screen that is
        // not there.
        tab.terminal_handle()?.screen().clone()
    };
    // The ack may push one resync snapshot to the caller, so it happens with the
    // app mutex released like every other write to a screen.
    screen.ack(sender.session_id(), cursor);
    Ok(json!({ "ok": true }))
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
/// The reply carries `provider` — the harness the tab runs, or null where no
/// agent has ever run — because a dead agent's retained screen is the only
/// record of which harness painted it, and the tab's start-again offer leads
/// with that one.
///
/// **Never errors because no agent is running** — `live: false` with the last
/// (or a blank) snapshot is the contract, because a tab must still show what
/// its agent did before it died, and because the Agent tab is a fixture on
/// every worktree surface: mounting it must not spawn anything. An unknown
/// entity or scope errors, and so does an entity with no worktree (an approved
/// or abandoned plan): its disposable worktree is gone, so there is no worktree
/// to host an agent and the surface renders its empty state instead.
///
/// It errors for an agent whose session has no terminal ([`no_terminal_here`]).
/// A client that reads `has_terminal` never asks, and one that predates the
/// field gets a sentence rather than a blank grid it will sit in forever.
fn agent_attach(
    state: &Arc<Mutex<AppState>>,
    sender: &SessionSender,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, String> {
    // Grid defaults = the orchestrator's agent PTY size (40 rows × 120 cols).
    let cols = params.get("cols").and_then(Value::as_u64).unwrap_or(120) as u16;
    let rows = params.get("rows").and_then(Value::as_u64).unwrap_or(40) as u16;

    let requested_agent = named_agent_id(params)?;

    let mut guard = timer.lock(state);
    let s = &mut *guard;
    // The id is opaque (plan-… / run-…); what it resolves to is a worktree,
    // because that is what an agent works in. Without one, the scope params
    // resolve to the same thing — never a client-supplied path (spec §1).
    let entity_id = params
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string);
    let root = match &entity_id {
        Some(entity_id) => s.entity_agent_root(entity_id)?,
        None => TermScope::parse(params)?.resolve_root(s)?,
    };
    // Which agent: the one the rail named, or the entity's first — so a
    // surface that predates the rail still attaches to the agent it always did.
    // A scope-addressed attach with no entity can only mean the agent already
    // running there.
    let agent_id = match (&entity_id, requested_agent.as_deref()) {
        (Some(entity_id), requested) => s.resolve_agent(entity_id, requested)?.id,
        (None, Some(requested)) => requested.to_string(),
        // A worktree Build owns nothing in yet — an unadopted checkout, the
        // primary one — has no agent to name, so the screen a client mounts
        // there is addressed by the worktree itself until one is born.
        (None, None) => s
            .tabs
            .keys()
            .find(|key| key.is_agent() && key.root == root)
            .and_then(|key| key.tab_id.strip_prefix("agent:").map(str::to_string))
            .unwrap_or_else(|| crate::worktree::external_worktree_id(&root)),
    };
    if let Some(expected) = optional_nonempty_string(params, "conversation_id")? {
        let entity_id = entity_id
            .as_deref()
            .ok_or("conversation_id requires an entity id")?;
        let actual = s
            .resolve_conversation_address(entity_id, Some(&agent_id))?
            .conversation_id;
        if expected != actual {
            return Err(format!(
                "stale conversation_id {expected}; agent {agent_id} is bound to {actual}"
            ));
        }
    }
    let key = TabKey::agent(&root, &agent_id);
    if !s.tabs.contains_key(&key) {
        // No agent has run here yet: a blank, dead screen, and the tab opens on
        // the first delivery. The client still registers — on the screen this
        // worktree's agent will be born onto — because it must go live where it
        // stands when that delivery comes, not sit blank until the human
        // unmounts and remounts the tab.
        //
        // The handle is cloned under the lock and registered on with it
        // released, so a spawn can carry this screen's clients away in between;
        // the register follows them, because a carried screen points at the one
        // its clients went to.
        let term_id = agent_tab_id(&agent_id);
        let screen = s
            .agent_screens_awaiting_spawn
            .entry(key.clone())
            .or_insert_with(|| ScreenHandle::new(&term_id, cols, rows))
            .clone();
        drop(guard);
        let reading = screen.attach(sender, Some((cols, rows)));
        // A screen with no session behind it is dead by definition, and names
        // no harness: nothing has ever run here to name one.
        return Ok(attach_view(
            TabFacts {
                term_id,
                live: false,
                provider: None,
            },
            reading,
        ));
    }
    let attachment = s.attachment(&key)?;
    drop(guard);
    Ok(attach_to_tab(attachment, sender, cols, rows))
}

/// Open a worktree's agent with nothing to say to it — the surface's "Start
/// agent" button, and the "Restart" the human needs when the harness exits on
/// its own (codex running a self-update and quitting, claude crashing).
///
/// Every other way to get an agent is a turn: you say something and the agent
/// is spawned to hear it. That leaves no way to simply have one running, and no
/// way back after an exit short of inventing a message. This verb is that way,
/// and it is the only spawn with no prompt behind it — which is why the turn it
/// queues carries no text.
///
/// It takes the same queue every other turn does, so the reply is the entity's
/// state and never the harness's: the tab id it answers with is the one the
/// agent's own identity mints, reserved here and filled in when the session
/// opens. The owner must be an entity that owns a worktree — `.build/mcp.json`
/// routes `done` per owner, so an agent with nobody to report to is worse than
/// none.
fn agent_start(
    state: &Arc<Mutex<AppState>>,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, String> {
    let agent = {
        let mut s = timer.lock(state);
        let agent = s.addressed_agent(params)?;
        s.pending_agent_turns.push(PendingAgentTurn {
            operation_id: None,
            root: agent.root.clone(),
            owner: agent.entity_id.clone(),
            agent_id: agent.agent_id.clone(),
            conversation_id: agent.conversation_id.clone(),
            model_choice: agent.model_choice.clone(),
            choice_revision: agent.choice_revision,
            interrupt: false,
            // The button means "give me an agent", not "go do something" — so a
            // start with nothing waiting says nothing, and the human drives from
            // there. But the reviewer's words are durable on the thread and an
            // agent only learns of them by being TOLD to call
            // `read_unread_messages`; a fresh harness has no reason to.
            // Restarting after a crash with messages outstanding would silently
            // ignore every one of them. A hand-started agent has no context, so
            // what waits for it gets the cold form: the conversation protocol
            // and the catch-up packet around the nudge.
            say: agent.has_unread.then(|| TurnText {
                cold: crate::orchestrator::conversation_prompt(NEW_THREAD_MESSAGES_PROMPT),
                warm: NEW_THREAD_MESSAGES_PROMPT.to_string(),
            }),
            phase: "start",
            wants_catch_up: true,
            survives_refusal: false,
        });
        s.touch_attention(&agent.entity_id);
        agent
    };
    DeliveryRunner::drain(state, timer);

    Ok(json!({
        "term_id": agent_tab_id(&agent.agent_id),
        "agent_id": agent.agent_id,
        "notified": agent.has_unread,
    }))
}

/// What a client is told about the tab it just attached to.
struct TabFacts {
    term_id: String,
    live: bool,
    provider: Option<AgentProvider>,
}

/// One tab's client-facing surface, taken out of the registry together: what
/// the reply says about the tab, and the terminal the client attaches to.
struct TabAttachment {
    facts: TabFacts,
    terminal: TerminalHandle,
}

/// The attach reply, written in one place so both verbs answer in one shape.
fn attach_view(facts: TabFacts, screen: AttachSnapshot) -> Value {
    json!({
        "term_id": facts.term_id,
        "live": facts.live,
        "provider": facts.provider,
        "snapshot": screen.snapshot,
        "cursor": screen.cursor,
        "cols": screen.cols,
        "rows": screen.rows,
    })
}

/// Register `sender` on a tab's screen and describe what it should render.
///
/// The one attach body both verbs run: match the PTY to this client's viewport
/// (a TUI draws to the size it was told, so a mismatch garbles), then hand back
/// the snapshot and the monotonic cursor the pump will push from. A DEAD tab is
/// never resized — its retained screen is the last thing its agent painted and
/// must stay legible.
///
/// Called with the app mutex RELEASED. What makes the snapshot and the
/// registration atomic is the screen's OWN lock, which the pump feeds through:
/// no byte can land between them.
fn attach_to_tab(attachment: TabAttachment, sender: &SessionSender, cols: u16, rows: u16) -> Value {
    let viewport = attachment.facts.live.then_some((cols, rows));
    let reading = attachment.terminal.attach(sender, viewport);
    attach_view(attachment.facts, reading)
}

/// Find-or-create the one agent tab rooted at `root`.
///
/// Three phases, one call each. **Reserve** decides under the lock: hand back a
/// live tab, wait out the spawn somebody else is already making, or take the
/// reservation. **Open** does the disk work and starts the child with the lock
/// released. **Publish** puts the tab in the registry.
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
    request: AgentSpawnRequest<'_>,
    timer: &FrameTimer,
) -> Result<Option<(String, Spawned)>, String> {
    let key = TabKey::agent(&AppState::canonical_root(root), request.agent_id);
    let reserved = match claim_agent_spawn(state, &key, &request, timer)? {
        SpawnDecision::Live(wire_id) => return Ok(Some((wire_id, Spawned::Warm))),
        SpawnDecision::NoSession => return Ok(None),
        SpawnDecision::Reserved(reserved) => *reserved,
    };
    let Some(opened) = open_agent_session(state, reserved, &key, request.conversation_id, timer)?
    else {
        return Ok(None);
    };
    Ok(publish_agent_tab(
        state,
        &key,
        opened,
        request.conversation_id,
        request.model_choice,
        request.phase,
        timer,
    )
    .map(|wire_id| (wire_id, Spawned::Fresh)))
}

/// What the lock-held half of a spawn decided.
enum SpawnDecision {
    /// A live tab of this owner's — found on arrival, or waited out.
    Live(String),
    /// The entity's session is over, so there is no agent to open.
    NoSession,
    /// Nobody else is opening this tab, so this caller is. Boxed because a
    /// whole spawn plan dwarfs a wire id, and every decision would pay for it.
    Reserved(Box<ReservedSpawn>),
}

/// Decide, under the lock, what this caller is to do about the tab.
///
/// The wait gives the mutex back for its whole duration and wakes on the
/// winner's claim being released, so a caller that lost the race costs the
/// daemon nothing while it waits.
fn claim_agent_spawn(
    state: &Arc<Mutex<AppState>>,
    key: &TabKey,
    request: &AgentSpawnRequest<'_>,
    timer: &FrameTimer,
) -> Result<SpawnDecision, String> {
    let deadline = std::time::Instant::now() + AGENT_SPAWN_WAIT;
    let mut s = timer.lock(state);
    loop {
        if !s.agent_target_exists(
            request.owner,
            request.agent_id,
            request.conversation_id,
            &key.root,
        ) {
            return Ok(SpawnDecision::NoSession);
        }
        if let Some(tab) = s.tabs.get(key) {
            let same_target = tab.role.agent().is_some_and(|(owner, agent_id)| {
                owner == request.owner && agent_id == request.agent_id
            });
            if same_target && tab.session_is_live() && !request.force_fresh {
                return Ok(SpawnDecision::Live(tab.wire_id()));
            }
        }
        if !s.agent_spawns_in_flight.contains(key) {
            return reserve_agent_spawn(&mut s, key, request)
                .map(|reserved| SpawnDecision::Reserved(Box::new(reserved)));
        }
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(format!(
                "timed out waiting for the agent starting in {}",
                key.root.display()
            ));
        }
        let finished = Arc::clone(&s.agent_spawn_finished);
        s = s.wait_until(&finished, left, |state| {
            !state.agent_spawns_in_flight.contains(key)
        });
    }
}

/// What the lock-held half of a spawn hands to the lock-free half.
struct ReservedSpawn {
    plan: AgentSpawnPlan,
    role: TabRole,
    holding: SpawnHolding,
}

/// What a reservation is holding on the registry's behalf until the tab opens.
///
/// Three things the registry gave up when the reservation was taken, and all
/// three go back together if the spawn never opens.
struct SpawnHolding {
    claim: SpawnClaim,
    /// The grid of the dead session this spawn replaces, kept for the session
    /// about to paint it.
    carried: Option<ScreenHandle>,
    /// The agent whose MCP token was registered before its child existed.
    agent_id: String,
    session_token: String,
}

impl SpawnHolding {
    /// Give everything back, and hand the caller the reason the spawn never
    /// opened.
    ///
    /// The reservation took the dead session's tab out of the registry and kept
    /// its grid, telling the clients on it NOTHING, because they were about to
    /// be handed to the session replacing it. There is no such session now, and
    /// the grid is in no registry for a reaper or a close to reach: they are
    /// told here or they are told never.
    fn abandon(self, state: &Arc<Mutex<AppState>>, error: String, timer: &FrameTimer) -> String {
        if let Some(screen) = &self.carried {
            screen.close(SPAWN_NEVER_OPENED);
        }
        let mut s = timer.lock(state);
        if s.mcp_session_tokens
            .get(&self.agent_id)
            .is_some_and(|current| constant_time_token_eq(current, &self.session_token))
        {
            s.mcp_session_tokens.remove(&self.agent_id);
        }
        self.claim.settle(&mut s);
        error
    }
}

/// Take the spawn reservation and read everything the disk work will need.
///
/// Every field of the plan is owned — the project's orchestrator is cloned, the
/// probes are `Arc`s — so nothing it does afterwards can reach back into the
/// registry this read it out of.
///
/// Every read that can fail runs FIRST, with the registry untouched. Retiring
/// the dead tab and registering the MCP token are the reservation giving
/// things up on the registry's behalf, and [`SpawnHolding::abandon`] is the one
/// primitive that gives them back — a failure between the take and the holding
/// would bypass it, leaving browsers on a grid no registry can reach and a
/// token no child holds. So the only failure arm here fails before anything is
/// taken.
fn reserve_agent_spawn(
    s: &mut AppState,
    key: &TabKey,
    request: &AgentSpawnRequest<'_>,
) -> Result<ReservedSpawn, String> {
    let owner = request.owner;
    let agent_id = request.agent_id;
    let conversation_id = request.conversation_id;
    let model_choice = request.model_choice;
    let force_fresh = request.force_fresh;
    if !s.agent_target_exists(owner, agent_id, conversation_id, &key.root) {
        return Err(format!("agent {agent_id} is no longer attached to {owner}"));
    }
    // A router session belongs to no project — deciding which one
    // the capture belongs to is its job. Any project's
    // orchestrator builds the same harness spec for it, since the
    // spec is made from the cwd and the owner id alone.
    let project_id = match s.project_of(owner) {
        Ok(project_id) => project_id,
        Err(unknown) if crate::router::is_router_agent(agent_id) => {
            s.default_project().map_err(|_| unknown)?
        }
        Err(unknown) => return Err(unknown),
    };
    let project = s.orch_for(&project_id)?.clone();
    let replaced = s.tabs.get(key).and_then(|tab| tab.session_instance.clone());
    let carried = s
        .retire_tab_keeping_screen(key)
        .and_then(|(_reaping, screen)| screen);
    if let Some(instance) = replaced {
        s.record_agent_session_end(&instance.entity_id, &instance.agent_id, &instance);
    }
    // A checkout outlives the entity that owned it — a planning
    // worktree is torn down and a run cuts a new one at the same
    // path, an adopted worktree is released and re-adopted. Agents
    // of the entity that USED to own this directory are stale: they
    // would keep working in it and report `done` for an owner that
    // no longer holds it. Several agents of the CURRENT owner are
    // exactly what a branch is allowed to have, so only the others
    // go.
    let stale: Vec<TabKey> = s
        .tabs
        .iter()
        .filter(|(other, tab)| {
            other.root == key.root
                && tab.role.agent().is_some_and(|(had, other_agent)| {
                    had != owner
                        && tab.session_instance.as_ref().is_none_or(|instance| {
                            !s.agent_target_exists(
                                had,
                                other_agent,
                                &instance.conversation_id,
                                &other.root,
                            )
                        })
                })
        })
        .map(|(other, _)| other.clone())
        .collect();
    for other in stale {
        let instance = s
            .tabs
            .get(&other)
            .and_then(|tab| tab.session_instance.clone());
        s.retire_tab(&other, "closed");
        if let Some(instance) = instance {
            s.record_agent_session_end(&instance.entity_id, &instance.agent_id, &instance);
        }
    }
    let session_token = uuid::Uuid::new_v4().to_string();
    // Before the child exists, because the child dials the done socket as soon
    // as it is up and an unregistered token is an unauthorized report.
    s.mcp_session_tokens
        .insert(agent_id.to_string(), session_token.clone());
    let recorded_resume_id = (!force_fresh)
        .then(|| s.resumable_session_id(owner, agent_id, &key.root, model_choice.provider))
        .flatten();
    if (force_fresh || recorded_resume_id.is_none())
        && s.recorded_resume_id(owner, agent_id).is_some()
    {
        s.record_agent_resume_id(owner, agent_id, None);
    }
    Ok(ReservedSpawn {
        plan: AgentSpawnPlan {
            project,
            root: key.root.clone(),
            agent_id: agent_id.to_string(),
            model_choice: model_choice.clone(),
            recorded_resume_id,
            probes: s.session_probes(),
            session_token: session_token.clone(),
        },
        role: TabRole::Agent {
            owner: owner.to_string(),
            agent_id: agent_id.to_string(),
            provider: model_choice.provider,
        },
        holding: SpawnHolding {
            claim: SpawnClaim::take(s, key),
            carried,
            agent_id: agent_id.to_string(),
            session_token,
        },
    })
}

/// The child a spawn opened, and the one thing publishing it has to write down.
struct OpenedSession {
    tab: Tab,
    output: SessionOutput,
    /// The provider no longer holds the conversation name this agent's record
    /// carries, so the record has to forget it.
    recorded_name_is_gone: bool,
    claim: SpawnClaim,
}

/// Open the child, with the app mutex RELEASED.
///
/// The three transcript reads, the `.build/` scaffold and the harness spawn
/// itself: between them they walk a tree the daemon does not own and wait on a
/// harness's readiness, and every terminal pump needs the app mutex while they
/// do. Either step failing gives the whole reservation back before it answers.
/// Everything a provider needs to open the agent `prepared` describes.
///
/// A terminal names its conversation from the launch contract when the spec
/// fixes it, and otherwise from the provider's pre-spawn transcript watcher;
/// a protocol carrier ignores the terminal mechanics and announces its own id.
fn agent_open_request(
    prepared: PreparedAgentLaunch,
    root: std::path::PathBuf,
    model_choice: &ModelChoice,
    resume_session_id: Option<String>,
    _locator: Option<Box<dyn crate::harness::SessionLocator>>,
) -> SessionOpenRequest {
    let identity = match &prepared.spec.known_session_id {
        Some(known) => Some(SessionIdentitySource::Known(known.clone())),
        None => resume_session_id
            .as_ref()
            .map(|verified| SessionIdentitySource::Known(verified.clone())),
    };
    SessionOpenRequest {
        spec: prepared.spec,
        root,
        choice: model_choice.clone(),
        terminal: TerminalOpenOptions {
            size: prepared.pty_size,
            turn_ready_grace: Some(crate::orchestrator::HARNESS_READY_GRACE),
            identity,
        },
        resume_session_id,
    }
}

/// `Ok(None)`: the owner's session ended while the spawn was reserved (a merge
/// pruned the checkout, an issue was approved). The reservation is released
/// and nothing is written to disk, so the pruned directory is not resurrected
/// by the scaffold a spawn would otherwise lay down.
fn open_agent_session(
    state: &Arc<Mutex<AppState>>,
    reserved: ReservedSpawn,
    key: &TabKey,
    conversation_id: &str,
    timer: &FrameTimer,
) -> Result<Option<OpenedSession>, String> {
    let ReservedSpawn {
        plan,
        role,
        holding,
    } = reserved;
    let session_is_over = match &role {
        TabRole::Agent {
            owner, agent_id, ..
        } => !timer
            .lock(state)
            .agent_target_exists(owner, agent_id, conversation_id, &key.root),
        TabRole::Shell => false,
    };
    if session_is_over {
        holding.abandon(state, String::new(), timer);
        return Ok(None);
    }
    let choice = plan.model_choice.clone();
    let opened = plan.probe_and_scaffold().and_then(|ready| {
        let ReadyToSpawn {
            spec,
            size,
            locator,
            resume_session_id,
            recorded_name_is_gone,
        } = ready;
        let TabRole::Agent {
            owner, agent_id, ..
        } = role
        else {
            unreachable!("an agent reservation always names an agent")
        };
        Tab::spawn_agent(
            owner,
            agent_id,
            agent_open_request(
                PreparedAgentLaunch {
                    spec,
                    pty_size: size,
                },
                key.root.clone(),
                &choice,
                resume_session_id,
                locator,
            ),
        )
        .map(|(tab, output)| (tab, output, recorded_name_is_gone))
    });
    match opened {
        Ok((mut tab, output, recorded_name_is_gone)) => {
            if let Some(screen) = holding.carried {
                tab.adopt_screen(screen);
            }
            Ok(Some(OpenedSession {
                tab,
                output,
                recorded_name_is_gone,
                claim: holding.claim,
            }))
        }
        Err(error) => Err(holding.abandon(state, error, timer)),
    }
}

/// Put the opened tab in the registry and start its pumps.
///
/// `None` means the tab was stranded: the entity lost its session while this
/// harness was starting — an issue approved under its own planning agent. The
/// insert is the instant the agent becomes addressable, so it is the instant
/// the gate that closed has to reach it, and no earlier check is atomic with
/// it.
fn publish_agent_tab(
    state: &Arc<Mutex<AppState>>,
    key: &TabKey,
    opened: OpenedSession,
    conversation_id: &str,
    model_choice: &ModelChoice,
    phase: &str,
    timer: &FrameTimer,
) -> Option<String> {
    let OpenedSession {
        tab,
        output,
        recorded_name_is_gone,
        claim,
    } = opened;
    let (owner, agent_id) = tab
        .role
        .agent()
        .map(|(owner, agent_id)| (owner.to_string(), agent_id.to_string()))
        .expect("an agent spawn opens an agent tab");
    let wire_id = tab.wire_id();
    let mut pumps = None;
    let inherited;
    let stranded;
    {
        let mut s = timer.lock(state);
        // The probe read the recorded name and the provider no longer holds it.
        // Forgetting it is a state write, so it happens here rather than in the
        // probe that found out.
        if recorded_name_is_gone {
            s.record_agent_resume_id(&owner, &agent_id, None);
        }
        inherited = inherit_waiting_clients(&mut s, key, &tab);
        let running = tab
            .session
            .active_model()
            .or_else(|| model_choice.model.clone());
        s.tabs.insert(key.clone(), tab);
        claim.settle(&mut s);
        s.record_agent_active_model(&owner, &agent_id, running);
        stranded = !s.agent_target_exists(&owner, &agent_id, conversation_id, &key.root);
        if stranded {
            s.retire_tab(key, "closed");
        } else {
            let instance =
                s.record_agent_session_start(&owner, &agent_id, &key.root, model_choice, phase);
            let tab = s
                .tabs
                .get_mut(key)
                .expect("the published agent tab was just inserted");
            tab.session_instance = instance;
            pumps = Some(tab.pumps(output));
        }
    }
    if stranded {
        return None;
    }
    if let Some(inherited) = inherited {
        inherited.fit_child_to_screen();
    }
    spawn_tab_pumps(
        state,
        key.clone(),
        pumps.expect("a non-stranded tab starts its pumps"),
    );
    Some(wire_id)
}

/// The daemon itself, held the way a background job has to hold it.
///
/// Weakly, because a job that outlives the daemon has nothing to give back to,
/// and through a poisoned mutex deliberately, because a job that ended by
/// panicking still has to settle and a destructor that panics during an unwind
/// aborts the process. Every background job that took something out of the
/// registry before it left gives it back through one of these.
#[derive(Clone)]
struct SettlingHandle(Option<std::sync::Weak<Mutex<AppState>>>);

impl SettlingHandle {
    /// Run `settle` under the app mutex, or not at all if the daemon is gone.
    fn settle(&self, settle: impl FnOnce(&mut AppState)) {
        let Some(state) = self.0.as_ref().and_then(std::sync::Weak::upgrade) else {
            return;
        };
        settle(
            &mut state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
    }
}

/// One worktree's agent spawn, reserved.
///
/// Held from the acquisition that found no live tab to the acquisition that
/// publishes the new one, so two callers of one tab produce one harness — two
/// agents in one worktree would both report `done` for the same owner, and the
/// second report is an illegal transition that lands on the thread as a bogus
/// failure. Every way out of a spawn releases it: [`SpawnClaim::settle`] under
/// a lock the caller already holds, and [`Drop`] on any path that never got
/// there, a panic included.
struct SpawnClaim {
    key: TabKey,
    state: SettlingHandle,
    finished: Arc<std::sync::Condvar>,
    settled: bool,
}

impl SpawnClaim {
    fn take(s: &mut AppState, key: &TabKey) -> SpawnClaim {
        s.agent_spawns_in_flight.insert(key.clone());
        SpawnClaim {
            key: key.clone(),
            state: s.settling_handle(),
            finished: Arc::clone(&s.agent_spawn_finished),
            settled: false,
        }
    }

    /// Release the claim under a lock the caller is already holding, and wake
    /// everyone waiting behind it.
    fn settle(mut self, s: &mut AppState) {
        s.agent_spawns_in_flight.remove(&self.key);
        self.settled = true;
        self.finished.notify_all();
    }
}

impl Drop for SpawnClaim {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        let key = &self.key;
        self.state.settle(|s| {
            s.agent_spawns_in_flight.remove(key);
        });
        self.finished.notify_all();
    }
}
/// Move the clients that were waiting for `tab`'s agent onto the screen it will
/// paint, and hand back the child's half of the move.
///
/// Clients that mounted the Agent tab before this worktree had one are attached
/// to a screen with no PTY. They are carried — with the viewport they render
/// at, the same rule an attach to a live tab follows — onto the real screen.
/// The carry is what makes the waiting screen point at this one, so a client
/// attaching during the spawn is on one screen or the other and never between
/// them however the two acquisitions fall. The waiting screen's cursor is not
/// carried: it painted nothing, while a retained screen's cursor is the one
/// that must never rewind.
///
/// The screen half is bounded and belongs under the app mutex, beside the
/// insert that publishes the tab. The child half is an ioctl to a process that
/// may not answer, so what comes back is the terminal that inherited them, for
/// the caller to fit to its screen with the lock down.
fn inherit_waiting_clients(s: &mut AppState, key: &TabKey, tab: &Tab) -> Option<TerminalHandle> {
    let first_here = !s
        .tabs
        .keys()
        .any(|other| other.is_agent() && other.root == key.root);
    let waiting = s.agent_screens_awaiting_spawn.remove(key).or_else(|| {
        // Clients that mounted the tab before this worktree had an agent
        // addressed it by the WORKTREE; the first agent born here is the one
        // they were waiting for.
        first_here.then(|| {
            s.agent_screens_awaiting_spawn.remove(&TabKey::agent(
                &key.root,
                &crate::worktree::external_worktree_id(&key.root),
            ))
        })?
    })?;
    match tab.terminal_handle() {
        Ok(terminal) => terminal
            .screen()
            .carry_clients_from(&waiting)
            .then_some(terminal),
        // There is no real screen to carry them onto — see
        // [`NO_TERMINAL_LEFT`].
        Err(_) => {
            waiting.close(NO_TERMINAL_LEFT);
            None
        }
    }
}

/// What a delivery reports when the tab it just ensured is already gone.
const TAB_CLOSED_UNDER_A_TURN: &str = "the agent tab closed before its turn could be delivered";
/// What a start says on its agent when the entity's session is over and no
/// harness is opened for it — the third answer to a start, beside "live" and
/// a spawn that failed.
const AGENT_START_DECLINED_SESSION_OVER: &str = "no session to open: this entity's session is over";

/// Why a screen the spawn was supposed to fill is closed instead.
///
/// Both screens [`ensure_agent_tab`] may be holding — the retained grid of the
/// session being replaced, and the one clients that mounted the Agent tab early
/// are waiting on — exist to be carried onto the new session's screen. A
/// session with no terminal has none, so there is nothing to carry them to and
/// the carry would drop their clients silently: attached to a grid nothing will
/// ever paint, waiting on a basement that is never coming.
///
/// So they are told, the way [`AppState::retire_agent`] and the orphan reaper
/// tell one. The rail reads `has_terminal: false` off the digest by then and
/// stops offering the terminal; this is what closes the door for a client that
/// was already through it.
const NO_TERMINAL_LEFT: &str = "no_terminal";

/// Why a screen a spawn was holding is closed when that spawn never opened.
///
/// The reservation takes the dead session's tab out of the registry and keeps
/// its grid, telling the clients on it NOTHING, because they are about to be
/// handed to the session replacing it. A spawn that fails has nobody to hand
/// them to, and the grid it is holding is in no registry for a reaper or a
/// close to reach: they are told here or they are told never.
const SPAWN_NEVER_OPENED: &str = "spawn_failed";

enum DeliveryOutcome {
    Delivered(Option<(String, Spawned)>),
    /// The exact destination still exists, but changing its frozen model
    /// requires a restart and the current turn has not reached a safe boundary.
    /// No provider call has happened; the durable intent is safe to queue.
    Deferred,
}

enum DeliveryPreflight {
    Proceed { force_fresh: bool },
    Deferred,
    Declined,
}

/// Decide how one frozen turn reaches its exact captured destination before
/// any provider-facing operation occurs.
fn preflight_delivery(
    state: &Arc<Mutex<AppState>>,
    turn: &PendingAgentTurn,
    timer: &FrameTimer,
) -> DeliveryPreflight {
    let key = turn.tab_key();
    let (plan, interrupt_session) = {
        let s = timer.lock(state);
        if !s.queued_agent_target_exists(turn) {
            return DeliveryPreflight::Declined;
        }
        let Some(tab) = s.tabs.get(&key) else {
            let force_fresh = s
                .resumable_session_id(
                    &turn.owner,
                    &turn.agent_id,
                    &turn.root,
                    turn.model_choice.provider,
                )
                .and_then(|named| {
                    s.agent_conversation(&turn.owner, Some(&turn.agent_id))
                        .ok()?
                        .sessions
                        .iter()
                        .rev()
                        .find(|session| {
                            session.agent_id == turn.agent_id
                                && session.resume_session_id.as_deref() == Some(named.as_str())
                        })
                        .map(|session| {
                            session.model != turn.model_choice.model
                                || session.effort != turn.model_choice.effort
                        })
                })
                .unwrap_or(false);
            return DeliveryPreflight::Proceed { force_fresh };
        };
        let exact_tab = tab
            .role
            .agent()
            .is_some_and(|(owner, agent_id)| owner == turn.owner && agent_id == turn.agent_id)
            && tab.session_instance.as_ref().is_some_and(|instance| {
                instance.conversation_id == turn.conversation_id
                    && instance.checkout == turn.root.display().to_string()
            });
        if !exact_tab {
            return DeliveryPreflight::Proceed { force_fresh: false };
        }
        let instance = tab
            .session_instance
            .as_ref()
            .expect("an exact agent tab has its session instance");
        if !tab.session_is_live() {
            return DeliveryPreflight::Proceed {
                force_fresh: !s.session_instance_uses_choice(instance, &turn.model_choice),
            };
        }
        if s.session_instance_uses_choice(instance, &turn.model_choice)
            || tab.session.turn_choice_support(&turn.model_choice) == TurnChoiceSupport::Native
        {
            return DeliveryPreflight::Proceed { force_fresh: false };
        }
        let working = s
            .entity_agents(&turn.owner)
            .ok()
            .and_then(|agents| agents.by_id(&turn.agent_id))
            .is_some_and(|agent| agent.working_since.is_some());
        if working && !turn.interrupt {
            return DeliveryPreflight::Deferred;
        }
        if turn.interrupt {
            let provider_thread_id = s.recorded_resume_id(&turn.owner, &turn.agent_id);
            tab.log_lifecycle(LifecycleDiagnostic {
                event: "interrupt_requested",
                origin: "delivery_preflight_model_change",
                reason: Some("replace_session_for_turn_choice"),
                operation_id: turn.operation_id.as_deref(),
                provider_thread_id: provider_thread_id.as_deref(),
                caller: None,
            });
        }
        (
            DeliveryPreflight::Proceed { force_fresh: true },
            turn.interrupt.then(|| Arc::clone(&tab.session)),
        )
    };
    if let Some(session) = interrupt_session {
        if let Err(refused) = session.interrupt() {
            eprintln!("thread.post {}: interrupt refused: {refused}", turn.owner);
        }
    }
    plan
}

/// The one pipe from Build to a worktree's agent.
///
/// Ensures the tab exists, then hands the agent the turn it was queued with —
/// a value the carrier decides how to say, which for a PTY is the harness's own
/// submit key and bracketed paste framing and never a raw write with a
/// hardcoded `\r`. Which half of the text travels is decided by whether the tab
/// had to be spawned; a turn that says nothing at all is a start, and the tab
/// existing is the whole of it. Returns the tab's wire id and which half
/// travelled — a `Fresh` delivery is a new agent process, which the
/// conversation records as the start of a session.
fn deliver(
    state: &Arc<Mutex<AppState>>,
    turn: &PendingAgentTurn,
    timer: &FrameTimer,
) -> Result<DeliveryOutcome, String> {
    let PendingAgentTurn {
        root,
        owner,
        agent_id,
        model_choice,
        choice_revision,
        interrupt,
        phase,
        say,
        ..
    } = turn;
    let force_fresh = match preflight_delivery(state, turn, timer) {
        DeliveryPreflight::Proceed { force_fresh } => force_fresh,
        DeliveryPreflight::Deferred => return Ok(DeliveryOutcome::Deferred),
        DeliveryPreflight::Declined => return Ok(DeliveryOutcome::Delivered(None)),
    };
    let Some((wire_id, spawned)) = ensure_agent_tab(
        state,
        root,
        AgentSpawnRequest {
            owner,
            agent_id,
            conversation_id: &turn.conversation_id,
            model_choice,
            force_fresh,
            phase,
        },
        timer,
    )?
    else {
        return Ok(DeliveryOutcome::Delivered(None));
    };
    let Some(say) = say else {
        return Ok(DeliveryOutcome::Delivered(Some((wire_id, spawned))));
    };
    let prompt = match spawned {
        Spawned::Fresh => &say.cold,
        Spawned::Warm => &say.warm,
    };
    let key = TabKey::agent(&AppState::canonical_root(root), agent_id);
    // The handle comes out of the registry so the turn travels with the
    // app-wide state lock RELEASED: every RPC, every terminal pump and the idle
    // sweep wait on that lock, and how long a session takes to accept a turn is
    // its own business — a protocol write to a full pipe, an ack a harness
    // answers late, the exit-race wait below.
    let (session, instance) = {
        let s = timer.lock(state);
        if !s.queued_agent_target_exists(turn) {
            return Ok(DeliveryOutcome::Delivered(None));
        }
        let tab = s.tabs.get(&key).ok_or(TAB_CLOSED_UNDER_A_TURN)?;
        let exact_instance = tab.session_instance.as_ref().is_some_and(|instance| {
            instance.entity_id == turn.owner
                && instance.agent_id == turn.agent_id
                && instance.conversation_id == turn.conversation_id
        });
        if !exact_instance {
            return Ok(DeliveryOutcome::Delivered(None));
        }
        if *interrupt && spawned == Spawned::Warm {
            let provider_thread_id = s.recorded_resume_id(owner, agent_id);
            tab.log_lifecycle(LifecycleDiagnostic {
                event: "interrupt_requested",
                origin: "deliver_warm_turn",
                reason: Some("thread_post_interrupt"),
                operation_id: turn.operation_id.as_deref(),
                provider_thread_id: provider_thread_id.as_deref(),
                caller: None,
            });
        }
        (
            Arc::clone(&tab.session),
            tab.session_instance
                .clone()
                .expect("an exact delivery tab has its session instance"),
        )
    };
    if *interrupt && spawned == Spawned::Warm {
        if let Err(refused) = session.interrupt() {
            eprintln!("thread.post {owner}: interrupt refused: {refused}");
        }
    }
    let reports_turn_boundaries = session.status_changed().is_some();
    if let Err(error) = session.send_turn(&Turn::with_choice(
        prompt,
        model_choice.clone(),
        *choice_revision,
    )) {
        // A harness that exits immediately still owns its tab: PTYs return EIO
        // once the child's side is closed, and the child closes it BEFORE the
        // OS makes its exit status reapable, so a single poll here races the
        // kernel. The bounded wait covers that lag; a genuinely wedged session
        // (live but unwritable) still surfaces its error.
        if !session.exited_within(crate::orchestrator::PROMPT_WRITE_EXIT_GRACE) {
            return Err(error.to_string());
        }
    }
    // The quiescence clock restarts here: whatever the agent was silent about
    // before, it now has something to answer for. A tab that closed while the
    // turn was in flight has no clock left to restart — and the turn still
    // travelled, so that is not a delivery failure to report.
    let now = now_rfc3339();
    let mut app = timer.lock(state);
    if still_pumping_instance(&app, &key, &session, &instance) {
        let was_working = app
            .entity_agents(owner)
            .ok()
            .and_then(|agents| agents.by_id(agent_id))
            .is_some_and(|agent| agent.working_since.is_some());
        let tab = app
            .tabs
            .get_mut(&key)
            .expect("the exact delivered session is still registered");
        tab.last_delivered_at = Some(std::time::Instant::now());
        if !reports_turn_boundaries && !was_working {
            app.record_agent_working_since(owner, &turn.agent_id, Some(now.clone()));
            app.observe_working_state(owner, true, &now);
        }
    }
    Ok(DeliveryOutcome::Delivered(Some((wire_id, spawned))))
}

/// The turns one lock acquisition took off the queue, on their way to their
/// agents.
///
/// A frame answers the moment its own state change is durable, so the queue is
/// taken here and delivered somewhere else. The owners are marked in flight
/// under the SAME acquisition that empties the queue, so there is no instant in
/// which a queued turn is invisible to the idle sweep and its entity looks
/// agentless.
/// The batch OWES those marks back. Each turn settles its own as it lands, and
/// [`Drop`] settles whatever is left, because an owner still marked in flight is
/// spared by the idle sweep forever — a run left Working with no agent and
/// nothing in the daemon able to demote it.
///
/// A mark travels with the turn it was taken for, so the turn that landed is
/// the only one whose mark can be given back. Found by owner instead, a batch
/// carrying two turns for one owner on two agents could settle the OTHER
/// agent's mark and leave its undelivered turn reading as absent.
struct PendingTurns {
    turns: std::collections::VecDeque<(PendingAgentTurn, TurnMark)>,
    state: SettlingHandle,
    /// The clock the delivery times itself by, taken under the acquisition
    /// that took the turns so the runner never takes the app mutex just to
    /// find it.
    clock: Arc<FrameClock>,
}

/// The turns that have left [`AppState::pending_agent_turns`] and have not yet
/// reached an agent.
///
/// Counted under two keys, because two questions are asked of the same fact and
/// neither answers the other. The idle sweep asks about an OWNER: between a
/// verb's transition and the tab its turn spawns, a working entity legitimately
/// has no agent tab. The verbs that would queue a second turn ask about an
/// AGENT TAB: a harness already on its way with words for it is the one that
/// reads the next message, and a turn queued behind it is a duplicate nudge.
/// Every turn counts under its owner; only a turn that says something counts
/// under its agent, because only that turn tells the agent to read.
///
/// Counted rather than flagged, because one batch can carry several turns for
/// one owner and several for one agent.
#[derive(Default)]
struct TurnsInFlight {
    owners: HashMap<String, usize>,
    agents: HashMap<TabKey, usize>,
}

/// One turn's pair of marks, owed back by whoever took them.
///
/// Given back by [`TurnMark::settle`] under a lock the caller holds once the
/// turn has landed, and by [`Drop`] on any path that never got there — a
/// delivery that panicked after the turn left its batch and before it was
/// settled. The same guard [`SpawnClaim`] is, one phase earlier: a mark that
/// outlived its delivery would spare its owner from the idle sweep forever.
struct TurnMark {
    owner: String,
    /// The agent this turn will tell to read its thread — `None` for a turn
    /// that says nothing.
    told_agent: Option<TabKey>,
    state: SettlingHandle,
    settled: bool,
}

impl TurnMark {
    /// Give this turn's marks back, under a lock the caller holds. Consumes the
    /// mark, so one turn settles once.
    fn settle(mut self, s: &mut AppState) {
        s.turns_in_flight.give_back(&self);
        self.settled = true;
    }
}

impl Drop for TurnMark {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        self.state.settle(|s| s.turns_in_flight.give_back(self));
    }
}

impl TurnsInFlight {
    fn take(&mut self, turn: &PendingAgentTurn, state: SettlingHandle) -> TurnMark {
        let mark = TurnMark {
            owner: turn.owner.clone(),
            told_agent: turn.says_something().then(|| turn.tab_key()),
            state,
            settled: false,
        };
        *self.owners.entry(mark.owner.clone()).or_default() += 1;
        if let Some(agent) = &mark.told_agent {
            *self.agents.entry(agent.clone()).or_default() += 1;
        }
        mark
    }

    fn give_back(&mut self, mark: &TurnMark) {
        Self::drop_one(&mut self.owners, &mark.owner);
        if let Some(agent) = &mark.told_agent {
            Self::drop_one(&mut self.agents, agent);
        }
    }

    fn holds_owner(&self, owner: &str) -> bool {
        self.owners.contains_key(owner)
    }

    fn holds_agent(&self, key: &TabKey) -> bool {
        self.agents.contains_key(key)
    }

    /// Nothing is being delivered, for a test waiting out the deliveries a verb
    /// it called triggered.
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.owners.is_empty() && self.agents.is_empty()
    }

    fn drop_one<K: std::hash::Hash + Eq>(counts: &mut HashMap<K, usize>, key: &K) {
        let Some(count) = counts.get_mut(key) else {
            return;
        };
        *count -= 1;
        if *count == 0 {
            counts.remove(key);
        }
    }
}

impl PendingTurns {
    fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// The next turn to deliver, with the mark to settle once it has landed.
    fn next_turn(&mut self) -> Option<(PendingAgentTurn, TurnMark)> {
        self.turns.pop_front()
    }
}

impl Drop for PendingTurns {
    fn drop(&mut self) {
        let undelivered = std::mem::take(&mut self.turns);
        if undelivered.is_empty() {
            return;
        }
        self.state.settle(|s| {
            for (_, mark) in undelivered {
                mark.settle(s);
            }
        });
    }
}

/// Where a delivery's own time is charged. A spawn is not the frame that asked
/// for it, and counting it there would make every verb that speaks to an agent
/// look like the daemon's slowest.
const AGENT_DELIVERY_METHOD: &str = "agent.deliver";

/// Sending the queued turns, off the frame that queued them.
///
/// DELIBERATE, and the whole of spec step 2: a cold delivery spawns a harness
/// and waits on its readiness for up to [`HARNESS_READY_GRACE`], and the
/// browser gives up at twelve seconds. The verb's own state change is durable
/// before the queue is even taken, so nothing about the reply depends on the
/// agent being up. What the delivery does reaches the browser the way every
/// other background outcome does: the entity's own record
/// ([`AppState::record_agent_session_start`],
/// [`AppState::record_agent_delivery_failure`]) and a push invalidation.
///
/// [`HARNESS_READY_GRACE`]: crate::orchestrator::HARNESS_READY_GRACE
struct DeliveryRunner;

impl DeliveryRunner {
    /// Take whatever the verbs that just ran queued, under one acquisition
    /// charged to `timer`, and deliver it off this thread. The one call every
    /// path that queues a turn makes once its own state change is durable.
    fn drain(state: &Arc<Mutex<AppState>>, timer: &FrameTimer) {
        let turns = timer.lock(state).take_pending_turns();
        DeliveryRunner::spawn(state, turns);
    }

    /// Deliver `turns` on a thread of the runtime's, and return at once.
    ///
    /// With no runtime under it — the synchronous unit tests — there is no
    /// thread to hand the work to and it runs here, which is the same
    /// delivery, made on the caller's time.
    ///
    /// The delivery is JOINED by a task of its own rather than detached, so a
    /// delivery that panicked, or one a shutting-down runtime never ran, says
    /// so on the log instead of vanishing. Either way the batch's in-flight
    /// marks come back with it: [`PendingTurns`] settles what it owes on drop.
    ///
    /// The blocking half is submitted BEFORE the joiner, and not from inside
    /// it: a single-threaded runtime runs a spawned task only when something
    /// awaits, and a delivery that waited for its caller to await would be a
    /// delivery that never left the frame.
    fn spawn(state: &Arc<Mutex<AppState>>, turns: PendingTurns) {
        if turns.is_empty() {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            DeliveryRunner::run(state, turns);
            return;
        };
        let state = Arc::clone(state);
        let delivering = runtime.spawn_blocking(move || DeliveryRunner::run(&state, turns));
        runtime.spawn(async move {
            if let Err(joined) = delivering.await {
                eprintln!("agent delivery failed: {joined}");
            }
        });
    }

    /// Send every turn, one at a time, and write down what each one did.
    ///
    /// A cold delivery starts a new harness process, so it opens the
    /// conversation's session lineage — the record the thread reads back as
    /// "the revise agent started here". A warm delivery continues the session
    /// already open.
    fn run(state: &Arc<Mutex<AppState>>, mut turns: PendingTurns) {
        let timer = turns.clock.frame(AGENT_DELIVERY_METHOD);
        while let Some((turn, mark)) = turns.next_turn() {
            // A triage turn may have left the app queue before the account setting
            // was switched off. Recheck at the last point before delivery; a turn
            // already handed to its agent is allowed to finish and report normally.
            if turn.phase == "triage" {
                let mut app = timer.lock(state);
                if !app.triage_enabled {
                    mark.settle(&mut app);
                    continue;
                }
            }
            if let Some(operation_id) = turn.operation_id.as_deref() {
                let claimed = timer.lock(state).transition_delivery_operation(
                    operation_id,
                    OperationStatus::Queued,
                    OperationStatus::Claimed,
                    None,
                );
                match claimed {
                    Ok(true) => {}
                    Ok(false) => {
                        mark.settle(&mut timer.lock(state));
                        continue;
                    }
                    Err(error) => {
                        eprintln!("claim delivery {operation_id}: {error}");
                        let mut app = timer.lock(state);
                        app.pending_agent_turns.push(turn);
                        mark.settle(&mut app);
                        continue;
                    }
                }
            }
            let delivered = deliver(state, &turn, &timer);
            let mut s = timer.lock(state);
            if let Some(operation_id) = turn.operation_id.as_deref() {
                let next = match &delivered {
                    Ok(DeliveryOutcome::Deferred) => OperationStatus::Queued,
                    Ok(DeliveryOutcome::Delivered(_)) => OperationStatus::Delivered,
                    Err(_) => OperationStatus::Uncertain,
                };
                let execution_error = match &delivered {
                    Ok(DeliveryOutcome::Delivered(None)) => Some(AGENT_START_DECLINED_SESSION_OVER),
                    Err(error) => Some(error.as_str()),
                    _ => None,
                };
                if let Err(error) = s.transition_delivery_operation(
                    operation_id,
                    OperationStatus::Claimed,
                    next,
                    execution_error,
                ) {
                    eprintln!("settle delivery {operation_id}: {error}");
                }
            }
            match delivered {
                // An issue whose session is over (approved, abandoned) holds no
                // workspace — and its checkout is the project's primary one,
                // which is emphatically not a place to spawn a replacement for
                // work nobody is doing. The turn stays on its thread; the
                // agent says why nothing opened.
                Ok(DeliveryOutcome::Delivered(None)) => s.record_agent_start_declined(&turn),
                Ok(DeliveryOutcome::Delivered(Some(_))) => {}
                Ok(DeliveryOutcome::Deferred) => {
                    s.pending_agent_turns.push(turn);
                }
                // The turn stays durable on the thread — the agent picks it up
                // with `read_unread_messages` the next time a tab opens — but
                // nothing is reading that thread right now, so the entity
                // itself has to carry the reason. The idle sweep finishes the
                // job: an entity left working with no agent tab is demoted on
                // the next pass.
                Err(error) => {
                    eprintln!("deliver to {}: {error}", turn.owner);
                    s.record_agent_delivery_failure(&turn, &error);
                }
            }
            // Off the queue and out of flight: from here the entity's agent tab
            // is the whole truth about whether an agent is there.
            mark.settle(&mut s);
        }
    }
}

/// Start whichever pump this tab's session needs: the byte pump for a terminal,
/// the activity pump for a session that reports its own work.
///
/// One or the other and never both, because the two capabilities are
/// alternatives — and never neither, because the death rites hang off a stream
/// closing ([`open_session`] refuses a session with no stream at all).
fn spawn_tab_pumps(state: &Arc<Mutex<AppState>>, key: TabKey, pumps: TabPumps) {
    let TabPumps {
        session,
        session_instance,
        screen,
        output,
    } = pumps;
    spawn_tab_pump(
        state,
        key.clone(),
        Arc::clone(&session),
        session_instance.clone(),
        screen,
        output.bytes,
    );
    let status_changed = session.status_changed();
    spawn_activity_pump(
        state,
        key.clone(),
        Arc::clone(&session),
        session_instance.clone(),
        output.activity,
        output.surfaces,
    );
    spawn_status_pump(state, key, session, session_instance, status_changed);
}

fn spawn_status_pump(
    state: &Arc<Mutex<AppState>>,
    key: TabKey,
    session: Arc<dyn AgentSession>,
    instance: Option<SessionInstance>,
    mut changed: Option<tokio::sync::watch::Receiver<crate::harness::SessionStatusSnapshot>>,
) {
    let Some(mut changed) = changed.take() else {
        return;
    };
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    let state = Arc::downgrade(state);
    let session = Arc::downgrade(&session);
    tokio::spawn(async move {
        loop {
            let snapshot = changed.borrow_and_update().clone();
            let (ended, retry_deferred, clock, delivery_state) = {
                let Some(state) = state.upgrade() else {
                    return;
                };
                let mut app = state.lock().unwrap();
                let Some(session) = session.upgrade() else {
                    return;
                };
                let Some(instance) = instance.as_ref() else {
                    return;
                };
                if !still_pumping_instance(&app, &key, &session, instance) {
                    return;
                }
                let owner = &instance.entity_id;
                app.record_agent_status_snapshot(owner, &instance.agent_id, &snapshot);
                let retry_deferred = !matches!(snapshot.status, AgentStatus::Working)
                    && app
                        .pending_agent_turns
                        .iter()
                        .any(|turn| turn.owner == *owner && turn.agent_id == instance.agent_id);
                (
                    matches!(snapshot.status, AgentStatus::Ended { .. }),
                    retry_deferred,
                    Arc::clone(&app.frame_clock),
                    Arc::clone(&state),
                )
            };
            if retry_deferred {
                let timer = clock.frame(AGENT_DELIVERY_METHOD);
                DeliveryRunner::drain(&delivery_state, &timer);
            }
            if ended {
                return;
            }
            if changed.changed().await.is_err() {
                return;
            }
        }
    });
}

/// Pump one tab's PTY into its screen model, coalescing at `TERM_FLUSH_MS` and
/// flushing one keyed frame to every attached client.
///
/// **It never takes the app mutex to paint.** The screen is its own lock, held
/// by this task for the microseconds a chunk takes to parse, so three agents
/// flooding at once contend with each other's readers and with nothing else in
/// the daemon. The app mutex is taken exactly twice, at EOF, to write down that
/// the session ended.
///
/// Start of session: the parser is reset to a blank screen of the current grid
/// and `term.reset` is pushed (clients wipe; a replacement process starts
/// clean) — the cursor is NEVER reset, because client dedupe rides it. On EOF a
/// Shell tab is removed, reaped, and pushed `term.closed{exited}`; an Agent tab
/// is RETAINED with `live = false` and pushed `term.closed{agent_session_ended}`,
/// because the tab must still show the last screen.
///
/// One pump per tab for the tab's whole life, and it pumps the session it was
/// started for: a kill is asynchronous now, so a replaced session's EOF can
/// arrive after its replacement is already in the registry. The tab is only
/// ended by the pump that holds that tab's own session.
///
/// A closed screen ends the pump without the registry: a retired tab's child
/// may keep producing until its kill lands, and nobody is watching.
fn spawn_tab_pump(
    state: &Arc<Mutex<AppState>>,
    key: TabKey,
    session: Arc<dyn AgentSession>,
    instance: Option<SessionInstance>,
    screen: Option<ScreenHandle>,
    rx: Option<broadcast::Receiver<Vec<u8>>>,
) {
    // No terminal, no bytes: the pump exists to paint a stream into a grid, and
    // a session that offers none has nothing for it to do.
    let (Some(mut rx), Some(screen)) = (rx, screen) else {
        return;
    };
    if tokio::runtime::Handle::try_current().is_err() {
        // Sync unit tests drive the registry without a runtime; there is
        // nothing to spawn the pump onto and nothing attached to feed.
        return;
    }
    let state = Arc::clone(state);
    tokio::spawn(async move {
        screen.restart();
        let mut flush = tokio::time::interval(Duration::from_millis(TERM_FLUSH_MS));
        flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let still_open = tokio::select! {
                recv = rx.recv() => match recv {
                    Ok(chunk) => screen.feed(&chunk),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        end_of_session(&state, &key, &session, instance.as_ref(), &screen);
                        return;
                    }
                },
                _ = flush.tick() => screen.flush(),
            };
            if !still_open {
                return;
            }
        }
    });
}

/// Whether the tab at `key` is still the one `session` was pumped for.
///
/// A retirement kills and reaps on a thread of its own, so a replaced session's
/// stream can end after its replacement is already in the registry — and a
/// pump's death rites take the app mutex more than once, with a filesystem read
/// between, so the tab can turn over mid-rite. Every acquisition asks, not just
/// the first: writing a dead session's findings onto a live one closes the
/// replacement's turn and records the wrong conversation against it.
fn still_pumping(s: &AppState, key: &TabKey, session: &Arc<dyn AgentSession>) -> bool {
    s.tabs
        .get(key)
        .is_some_and(|tab| Arc::ptr_eq(&tab.session, session))
}

/// Stronger guard for an agent callback: the tab still holds both the process
/// and the exact lineage instance captured when its pumps were spawned.
fn still_pumping_instance(
    s: &AppState,
    key: &TabKey,
    session: &Arc<dyn AgentSession>,
    instance: &SessionInstance,
) -> bool {
    s.tabs.get(key).is_some_and(|tab| {
        Arc::ptr_eq(&tab.session, session)
            && tab.session_instance.as_ref() == Some(instance)
            && tab.role.agent() == Some((instance.entity_id.as_str(), instance.agent_id.as_str()))
    })
}

/// The death rites of the session a byte pump was watching.
///
/// The app mutex is taken twice, with the reading a dying session owes between
/// them: what the tab becomes and what its clients are told are one bounded
/// step and are taken together, while the reading is a filesystem walk that
/// belongs under no lock at all. Both acquisitions are guarded by
/// [`still_pumping`], because that walk is the gap between them.
fn end_of_session(
    state: &Arc<Mutex<AppState>>,
    key: &TabKey,
    session: &Arc<dyn AgentSession>,
    instance: Option<&SessionInstance>,
    screen: &ScreenHandle,
) {
    let ended_agent = {
        let mut s = state.lock().unwrap();
        if !still_pumping(&s, key, session) {
            return;
        }
        let role = s
            .tabs
            .get(key)
            .expect("the tab this pump holds was just found")
            .role
            .clone();
        let provider_thread_id = role
            .agent()
            .and_then(|(owner_id, agent_id)| s.recorded_resume_id(owner_id, agent_id));
        match role.agent() {
            Some(_) => {
                let Some(instance) = instance else {
                    return;
                };
                if !still_pumping_instance(&s, key, session, instance) {
                    return;
                }
                let ended = instance.clone();
                let tab = s
                    .tabs
                    .get_mut(key)
                    .expect("the guarded agent tab still exists");
                tab.log_lifecycle(LifecycleDiagnostic {
                    event: "session_ended_observed",
                    origin: "session_output_closed",
                    reason: Some("agent_session_ended"),
                    operation_id: None,
                    provider_thread_id: provider_thread_id.as_deref(),
                    caller: None,
                });
                tab.live = false;
                // Told in the same acquisition that marks the tab, because a
                // marked tab is a REPLACEABLE one: the next spawn takes this
                // screen, clients and all, onto its own session without a
                // word. A close pushed after the release would land on
                // browsers already watching the replacement, in among its
                // opening reset. The screen's lock and one send per client is
                // bounded work, which is what makes it allowed here.
                screen.flush();
                // The clients hear the session ended and STAY: the grid they
                // are watching is the last thing this agent painted, and the
                // session that replaces it paints onto the same screen, with
                // the same clients still on it.
                screen.session_ended("agent_session_ended");
                Some(ended)
            }
            None => {
                s.retire_tab(key, "exited");
                None
            }
        }
    };
    let Some(instance) = ended_agent else {
        return;
    };
    // One final reading, so a session shorter than a sweep tick is still named
    // — and the respawn that needs the name is the very next thing after a
    // close. It RECORDS; it never clears: a terminal resumed in place writes no
    // new transcript, so a locator finding nothing is its normal answer here,
    // and clearing on that would throw a good name away at every restart. A
    // name that no longer resolves is caught at the reservation instead. The
    // reading itself is a filesystem walk, so it happens here, between the two
    // acquisitions, and not inside either.
    let report = SelfReport::read(session);
    let mut s = state.lock().unwrap();
    // A replacement can have taken the tab over while that walk ran. Its turn
    // is in flight and its conversation is its own; this session's findings
    // would close the one and overwrite the other.
    if !still_pumping_instance(&s, key, session, &instance) {
        return;
    }
    s.note_self_report(&instance.entity_id, &instance.agent_id, &instance, report);
    // The process is what a session IS, so this is where the conversation's
    // lineage closes — and where a turn the dead process was holding is closed,
    // so the row stops reading as working.
    s.record_agent_session_end(&instance.entity_id, &instance.agent_id, &instance);
}

/// Pump one session's reported activity into the conversation it speaks in.
///
/// The mirror of [`spawn_tab_pump`] for a session protocol that has no bytes. Where the
/// byte pump paints a stream into a grid, this one posts what the agent
/// reported doing as the activity kinds — reasoning, tool calls, narration and
/// background work — which are conversation, classed `Status`: they move no
/// unread count, reach no Issue conversation and pull nobody in.
///
/// It owes the same death rites, minus the screen's half: on close the tab goes
/// not live and the conversation's session lineage ends. There is no
/// `term.closed` to push because there is no screen — the step-3 refusals
/// already keep every client off one — and the tab is RETAINED for the same
/// reason the byte pump retains an agent's, so the rail still shows the agent
/// that was here.
///
/// It carries the session it pumps for the same reason the byte pump does, and
/// asks [`still_pumping`] at every acquisition: what it writes belongs to that
/// session, and a tab holding a different one is somebody else's.
fn spawn_activity_pump(
    state: &Arc<Mutex<AppState>>,
    key: TabKey,
    session: Arc<dyn AgentSession>,
    instance: Option<SessionInstance>,
    rx: Option<broadcast::Receiver<crate::harness::ActivityReport>>,
    mut surfaces_changed: Option<tokio::sync::watch::Receiver<u64>>,
) {
    let Some(mut rx) = rx else {
        return;
    };
    if tokio::runtime::Handle::try_current().is_err() {
        // Sync unit tests drive the registry without a runtime; there is
        // nothing to spawn the pump onto.
        return;
    }
    let state = Arc::clone(state);
    tokio::spawn(async move {
        loop {
            let woke = match surfaces_changed.as_mut() {
                Some(revision) => tokio::select! {
                    reported = rx.recv() => PumpWake::Reported(reported),
                    noticed = revision.changed() => match noticed {
                        Ok(()) => PumpWake::SurfacesMoved,
                        Err(_) => PumpWake::SurfacesUnwatchable,
                    },
                },
                None => PumpWake::Reported(rx.recv().await),
            };
            let reported = match woke {
                PumpWake::SurfacesMoved => {
                    let s = state.lock().unwrap();
                    let Some(instance) = instance.as_ref() else {
                        return;
                    };
                    if !still_pumping_instance(&s, &key, &session, instance) {
                        return;
                    }
                    s.note_entity_changed(&instance.entity_id);
                    continue;
                }
                PumpWake::SurfacesUnwatchable => {
                    surfaces_changed = None;
                    continue;
                }
                PumpWake::Reported(reported) => reported,
            };
            match reported {
                Ok(report) => {
                    let Some(instance) = instance.as_ref() else {
                        return;
                    };
                    let said = SelfReport::read(&session);
                    let mut s = state.lock().unwrap();
                    if !still_pumping_instance(&s, &key, &session, instance) {
                        return;
                    }
                    s.note_self_report(&instance.entity_id, &instance.agent_id, instance, said);
                    record_activity(
                        &mut s,
                        &key,
                        &instance.entity_id,
                        &instance.agent_id,
                        &report,
                    );
                }
                // A turn that called forty tools while the lock was busy is a
                // reader problem, not a reason to stop reading: what is lost is
                // lost, and the events after it still belong in the timeline.
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    let Some(instance) = instance.as_ref() else {
                        return;
                    };
                    let said = SelfReport::read(&session);
                    // Protocol sessions publish their final status before
                    // closing the activity stream. Consume that exact boundary
                    // here as well as in the watch pump: the two tasks race,
                    // and EOF must not call a completed turn an interruption
                    // merely because it acquired the app lock first.
                    let final_status = session
                        .status_changed()
                        .map(|status| status.borrow().clone());
                    let mut s = state.lock().unwrap();
                    // The reading is a filesystem walk, and a replacement can
                    // have taken the tab over while it ran: what follows ends a
                    // session, and ending the live one would mark it dead,
                    // harvest its open tool calls and close its turn.
                    if !still_pumping_instance(&s, &key, &session, instance) {
                        return;
                    }
                    let tab = s
                        .tabs
                        .get_mut(&key)
                        .expect("the tab this pump holds was just found");
                    tab.live = false;
                    let unanswered_call_sequences = take_unanswered_call_sequences(tab);
                    match said.named {
                        Some(_) => s.note_self_report(
                            &instance.entity_id,
                            &instance.agent_id,
                            instance,
                            said,
                        ),
                        // A session that ended having never announced a
                        // conversation of its own is the shape of one spawned
                        // with an id that no longer resolves: the child exits
                        // without an init line. Clearing sends the next spawn
                        // back to the transcript probe, so one dead id costs
                        // one restart rather than every restart — and where the
                        // child died at startup for an unrelated reason, the
                        // probe is what would have answered anyway.
                        None => {
                            s.record_agent_resume_id(&instance.entity_id, &instance.agent_id, None)
                        }
                    }
                    // A call still open when the child's stream ended never got
                    // an answer and never will: it is closed here, saying so,
                    // BEFORE the session ends — so the timeline reads
                    // calls-closed-then-session-ended rather than a session
                    // ending over work that still claims to run.
                    for sequence in unanswered_call_sequences {
                        s.resolve_agent_tool_call(
                            &instance.entity_id,
                            &instance.agent_id,
                            sequence,
                            crate::thread::ToolCallOutcome::Unanswered,
                            NO_ANSWER_SESSION_ENDED,
                        );
                    }
                    if let Some(snapshot) = final_status.as_ref() {
                        s.record_agent_status_snapshot(
                            &instance.entity_id,
                            &instance.agent_id,
                            snapshot,
                        );
                    }
                    // The process is what a session IS, so this is where the
                    // conversation's lineage closes — and where a turn the dead
                    // process was holding is closed, so the row stops reading as
                    // working.
                    s.record_agent_session_end(&instance.entity_id, &instance.agent_id, instance);
                    return;
                }
            }
        }
    });
}

enum PumpWake {
    Reported(Result<crate::harness::ActivityReport, broadcast::error::RecvError>),
    SurfacesMoved,
    SurfacesUnwatchable,
}

/// What a session says about itself: the conversation it is having, and the
/// model it is running.
///
/// Read from the session, never through the registry. For a terminal the name
/// comes from a locator listing the harness's transcript tree — a filesystem
/// walk that grows with every conversation the human has ever had — so the
/// caller holds an `Arc` and asks with the app mutex released.
struct SelfReport {
    named: Option<String>,
    model: Option<String>,
}

impl SelfReport {
    fn read(session: &Arc<dyn AgentSession>) -> SelfReport {
        SelfReport {
            named: session.session_id(),
            model: session.active_model(),
        }
    }
}

impl AppState {
    /// Write down what a session said about itself.
    ///
    /// Compared before it is written, so a session that names its conversation
    /// once costs one write however long it lives. A name that has not arrived
    /// leaves the record alone: what it carries is the last session's, which is
    /// exactly what a resume should use if this one dies before naming its own.
    ///
    /// Both carriers' capture points come through here, so a name a child
    /// announced and a name a locator found are the same record written by the
    /// same hand.
    fn note_self_report(
        &mut self,
        owner: &str,
        agent_id: &str,
        instance: &SessionInstance,
        report: SelfReport,
    ) {
        if let Some(named) = report.named {
            if let Err(error) =
                self.edit_agent_conversation(owner, agent_id, |thread, _artifact| {
                    if thread.name_session_instance(instance, &named) {
                        Ok(())
                    } else {
                        Err(
                            "session instance no longer matches its conversation lineage"
                                .to_string(),
                        )
                    }
                })
            {
                eprintln!("note_self_report {owner}: {error}");
                return;
            }
            if self.recorded_resume_id(owner, agent_id).as_deref() != Some(named.as_str()) {
                self.record_agent_resume_id(owner, agent_id, Some(named));
            }
        }
        if let Some(running) = report.model {
            self.record_agent_active_model(owner, agent_id, Some(running));
        }
    }
}

/// The terminal's capture point: ask every live agent session for the
/// name its conversation has, and write down each answer that moved.
///
/// A terminal announces nothing, so no task wakes on its behalf the way the
/// activity pump wakes on a session protocol's events — which is why the sweep
/// is daemon-owned and fixed-cadence rather than hung off the status poll. The
/// poll is client-driven: with no browser open nothing would ever be captured,
/// and every attached client would multiply this filesystem read by its own
/// poll rate, on the RPC path that answers from under the state lock.
///
/// The lock is HELD only to collect the live agents and to write the answers.
/// The one call that may touch the filesystem — a locator listing the harness's
/// transcript tree — is made between the two, with the lock released, the way a
/// turn is handed over.
fn capture_conversation_names(state: &Arc<Mutex<AppState>>) {
    /// One live agent, taken out of the registry so the name can be asked for
    /// with the lock released, and put back by `key` once it is known.
    struct LiveAgent {
        key: TabKey,
        instance: SessionInstance,
        session: Arc<dyn AgentSession>,
        recorded: Option<String>,
        recorded_model: Option<String>,
    }

    let live: Vec<LiveAgent> = {
        let s = state.lock().unwrap();
        s.tabs
            .iter()
            .filter(|(_, tab)| tab.live)
            .filter_map(|(key, tab)| {
                let instance = tab.session_instance.clone()?;
                Some(LiveAgent {
                    key: key.clone(),
                    instance: instance.clone(),
                    session: Arc::clone(&tab.session),
                    recorded: s.recorded_resume_id(&instance.entity_id, &instance.agent_id),
                    recorded_model: s
                        .recorded_active_model(&instance.entity_id, &instance.agent_id),
                })
            })
            .collect()
    };
    let moved: Vec<(TabKey, SessionInstance, Arc<dyn AgentSession>, SelfReport)> = live
        .into_iter()
        .filter_map(|agent| {
            let said = SelfReport::read(&agent.session);
            let name_moved = said.named.is_some() && agent.recorded != said.named;
            let model_moved = said.model.is_some() && agent.recorded_model != said.model;
            (name_moved || model_moved).then_some((agent.key, agent.instance, agent.session, said))
        })
        .collect();
    if moved.is_empty() {
        return;
    }
    let mut s = state.lock().unwrap();
    for (key, instance, session, said) in moved {
        if still_pumping_instance(&s, &key, &session, &instance) {
            s.note_self_report(&instance.entity_id, &instance.agent_id, &instance, said);
        }
    }
}

fn digest_surfaces(tab: Option<&Tab>, scope: DigestScope) -> Option<Value> {
    let tab = match scope {
        DigestScope::List => return None,
        DigestScope::Detail => tab?,
    };
    let snapshot = tab.session.surfaces()?;
    Some(snapshot.wire_value(&|call_id| tab.call_sequences.get(call_id).map(|row| row.sequence)))
}

/// The conversation event one reported activity becomes. The five kinds are the
/// same five, named once here so the mapping cannot drift.
fn activity_event_kind(activity: &crate::harness::AgentActivity) -> crate::thread::ThreadEventKind {
    use crate::harness::AgentActivity;
    use crate::thread::ThreadEventKind;
    match activity {
        AgentActivity::Reasoning { .. } => ThreadEventKind::Reasoning,
        AgentActivity::ToolUse { .. } => ThreadEventKind::ToolUse,
        AgentActivity::ToolResult { .. } => ThreadEventKind::ToolResult,
        AgentActivity::Narration { .. } => ThreadEventKind::Narration,
        AgentActivity::TaskUpdate { .. } => ThreadEventKind::TaskUpdate,
    }
}

/// What a call's row says when its answer never came, named by the boundary
/// that closed it. Pending is a claim too — "this is still running" — so a call
/// nothing will ever answer says which thing ended instead.
const NO_ANSWER_TURN_ENDED: &str = "no answer — turn ended";
const NO_ANSWER_SESSION_ENDED: &str = "no answer — session ended";

fn record_activity(
    state: &mut AppState,
    key: &TabKey,
    owner: &str,
    agent_id: &str,
    report: &crate::harness::ActivityReport,
) {
    use crate::harness::AgentActivity;
    let parent_sequence = parent_row_sequence(state, key, report.parent_call_id.as_deref());
    match &report.activity {
        AgentActivity::ToolUse { call_id, .. } => {
            let minted =
                state.record_agent_activity(owner, agent_id, &report.activity, parent_sequence);
            if let (Some(sequence), Some(tab)) = (minted, state.tabs.get_mut(key)) {
                tab.call_sequences.insert(
                    call_id.clone(),
                    MintedCallRow {
                        sequence,
                        answered: false,
                    },
                );
            }
        }
        AgentActivity::ToolResult {
            call_id,
            outcome,
            summary,
        } => {
            let answer = match outcome {
                crate::harness::ToolOutcome::Unanswered => NO_ANSWER_TURN_ENDED,
                _ => summary.as_str(),
            };
            let landed = mark_call_answered(state, key, call_id).is_some_and(|sequence| {
                state.resolve_agent_tool_call(
                    owner,
                    agent_id,
                    sequence,
                    tool_call_outcome(*outcome),
                    answer,
                )
            });
            // An empty answer mints nothing, exactly as it never did: a row
            // saying only that some tool answered says nothing at all.
            if !landed && !answer.is_empty() {
                state.record_activity_row(
                    owner,
                    agent_id,
                    crate::thread::ThreadEventKind::ToolResult,
                    answer.to_string(),
                    parent_sequence,
                );
            }
        }
        _ => {
            state.record_agent_activity(owner, agent_id, &report.activity, parent_sequence);
        }
    }
}

fn parent_row_sequence(
    state: &AppState,
    key: &TabKey,
    parent_call_id: Option<&str>,
) -> Option<u64> {
    let call_id = parent_call_id?;
    Some(state.tabs.get(key)?.call_sequences.get(call_id)?.sequence)
}

fn mark_call_answered(state: &mut AppState, key: &TabKey, call_id: &str) -> Option<u64> {
    let row = state.tabs.get_mut(key)?.call_sequences.get_mut(call_id)?;
    row.answered = true;
    Some(row.sequence)
}

fn take_unanswered_call_sequences(tab: &mut Tab) -> Vec<u64> {
    let mut unanswered: Vec<u64> = std::mem::take(&mut tab.call_sequences)
        .into_values()
        .filter(|row| !row.answered)
        .map(|row| row.sequence)
        .collect();
    unanswered.sort_unstable();
    unanswered
}

/// The conversation's word for what the harness saw.
fn tool_call_outcome(outcome: crate::harness::ToolOutcome) -> crate::thread::ToolCallOutcome {
    use crate::harness::ToolOutcome;
    use crate::thread::ToolCallOutcome;
    match outcome {
        ToolOutcome::Ok => ToolCallOutcome::Ok,
        ToolOutcome::Error => ToolCallOutcome::Error,
        ToolOutcome::Unanswered => ToolCallOutcome::Unanswered,
    }
}

/// Start a deterministic agent output stream: register it, then spawn a background
/// producer that appends `count` ordered output events (one per `interval_ms`) and
/// a terminal `done` event into the authoritative log.
fn stream_start(
    state: &Arc<Mutex<AppState>>,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, String> {
    let count = params.get("count").and_then(Value::as_u64).unwrap_or(20);
    let interval_ms = params
        .get("interval_ms")
        .and_then(Value::as_u64)
        .unwrap_or(5)
        .clamp(0, 1000);

    let stream_id = {
        let mut s = timer.lock(state);
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

#[cfg(test)]
mod tests;
