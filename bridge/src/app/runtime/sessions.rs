mod registry;
#[cfg(test)]
mod registry_tests;

#[cfg(test)]
pub(in crate::app) use registry::constant_time_token_eq;
pub(in crate::app) use registry::{
    IdleObservation, McpTokenLease, SessionRegistry, SpawnAvailability, SpawnClaimToken,
};

use crate::app::{no_terminal_here, AppState, NO_TERMINAL_LEFT};
use crate::harness::{
    harness_for, open_session, open_terminal_session, AgentSession, AgentStatus, HarnessContext,
    SessionOpenRequest, SessionOutput, TerminalOpenOptions,
};
use crate::models::{AgentProvider, ModelChoice};
use crate::orchestrator::{Agent, ResumeIdProbe, SessionLocatorFactory, SpawnOptions};
use crate::pty::HarnessSpec;
use crate::screen::{ScreenHandle, TerminalHandle};
use crate::store::now_rfc3339;
use crate::thread::SessionInstance;
use portable_pty::PtySize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;

/// A tab's identity: the canonical worktree it is rooted in, and which tab of
/// that worktree it is.
///
/// Canonical because the same worktree reaches the daemon under three different
/// scope shapes (run / external / primary) and, on macOS, under two different
/// literal paths (`/tmp` is `/private/tmp`). Keying by path rather than by
/// entity id is what keeps an agent's PTY where the human left it as the entity
/// around it is adopted, released and re-adopted.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(in crate::app) struct TabKey {
    pub(in crate::app) root: std::path::PathBuf,
    pub(in crate::app) tab_id: String,
}

impl TabKey {
    /// The key of one agent's tab in `root`. `root` must already be canonical —
    /// see [`AppState::canonical_root`].
    pub(in crate::app) fn agent(root: &std::path::Path, agent_id: &str) -> TabKey {
        TabKey {
            root: root.to_path_buf(),
            tab_id: agent_tab_id(agent_id),
        }
    }

    /// Whether this key names an agent rather than one of the human's shells.
    pub(in crate::app) fn is_agent(&self) -> bool {
        self.tab_id.starts_with("agent:")
    }
}

/// What is running in a tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::app) enum TabRole {
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
    pub(in crate::app) fn agent(&self) -> Option<(&str, &str)> {
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
pub(in crate::app) struct Tab {
    pub(in crate::app) tab_id: String,
    pub(in crate::app) root: std::path::PathBuf,
    pub(in crate::app) role: TabRole,
    /// Surfaced by `term.list` so a reloaded client can order the tab row the
    /// way the human opened it.
    pub(in crate::app) created_at: String,
    /// Shared rather than owned outright: a turn is handed over with the
    /// app-wide state lock RELEASED, so the delivery takes a handle out of the
    /// registry instead of holding the registry open across the turn.
    pub(in crate::app) session: Arc<dyn AgentSession>,
    /// Exact conversation-lineage row opened for this process. Captured once
    /// at publication and carried by every callback; never rediscovered from
    /// whichever session is newest when the callback finally runs.
    pub(in crate::app) session_instance: Option<SessionInstance>,
    /// The grid this tab's terminal paints into — `None` for a session with no
    /// terminal, because there is no grid without one. The terminal is a
    /// capability, not a guarantee, and a screen kept for a session that has
    /// none would be a second answer to a question with one:
    /// [`AgentSession::terminal`](crate::harness::AgentSession::terminal).
    pub(in crate::app) screen: Option<ScreenHandle>,
    /// False once the PTY stream has ended. An agent tab is RETAINED after its
    /// process dies so the tab still shows the last screen; a shell tab is
    /// removed by its pump instead, so this is only ever false for an agent.
    pub(in crate::app) live: bool,
    pub(in crate::app) call_sequences: HashMap<String, MintedCallRow>,
    /// When Build last submitted a turn here.
    ///
    /// The quiescence rule ("silence is an anomaly, never completion") used to
    /// read a phase session that was killed at every gate, so silence really
    /// was anomalous. A tab's agent outlives every phase and spends most of its
    /// life idle at a prompt, so silence only means something measured from the
    /// last thing Build asked of it.
    pub(in crate::app) last_delivered_at: Option<std::time::Instant>,
}

impl Tab {
    pub(in crate::app) fn log_lifecycle(&self, diagnostic: LifecycleDiagnostic<'_>) {
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
    pub(in crate::app) fn wire_id(&self) -> String {
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
    pub(in crate::app) fn session_is_live(&self) -> bool {
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
    pub(in crate::app) fn terminal_handle(&self) -> Result<TerminalHandle, String> {
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
    pub(in crate::app) fn adopt_screen(&mut self, screen: ScreenHandle) {
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
    pub(in crate::app) fn pumps(&self, output: SessionOutput) -> TabPumps {
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
    pub(in crate::app) fn spawn_agent(
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
    pub(in crate::app) fn spawn_shell(
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

    pub(in crate::app) fn from_opened_session(
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
pub(in crate::app) struct TabPumps {
    pub(in crate::app) session: Arc<dyn AgentSession>,
    pub(in crate::app) session_instance: Option<SessionInstance>,
    pub(in crate::app) screen: Option<ScreenHandle>,
    pub(in crate::app) output: SessionOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct MintedCallRow {
    pub(in crate::app) sequence: u64,
    pub(in crate::app) answered: bool,
}

pub(in crate::app) struct LifecycleDiagnostic<'a> {
    pub(in crate::app) event: &'a str,
    pub(in crate::app) origin: &'a str,
    pub(in crate::app) reason: Option<&'a str>,
    pub(in crate::app) operation_id: Option<&'a str>,
    pub(in crate::app) provider_thread_id: Option<&'a str>,
    pub(in crate::app) caller: Option<&'a std::panic::Location<'a>>,
}

/// The tab id of one Build-owned agent. Every other tab in a worktree is a
/// `term-<n>` shell the human drives.
///
/// Keyed by the AGENT, not by the worktree: a branch can carry several agents
/// sharing one checkout (spec: UX Redesign Decisions, "Agents and
/// conversations"), and each of them needs its own PTY.
pub(in crate::app) fn agent_tab_id(agent_id: &str) -> String {
    format!("agent:{agent_id}")
}

/// Build the warm agent adapter shared by every project's orchestrator. Build is
/// a UI layer over an agent session: every provider is launched interactively,
/// the rendered prompt is injected into that session, and the same session is
/// streamed to attached clients. The closure is shared across projects via
/// `Agent: Clone`; which provider it builds for is decided per spawn, by the
/// `ModelChoice` the entity carries.
pub(in crate::app) fn build_agent(qa_agent: bool, context: HarnessContext) -> Agent {
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

pub(in crate::app) fn default_resume_id_probe() -> ResumeIdProbe {
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

pub(in crate::app) fn default_session_locator_factory() -> SessionLocatorFactory {
    Arc::new(|cwd: &std::path::Path, provider| {
        let home = std::env::var("HOME").ok()?;
        harness_for(provider).session_locator(std::path::Path::new(&home), cwd)
    })
}

impl AppState {
    /// The canonical form of a worktree root — the tab registry's key. Every
    /// entry point funnels through this: a run's worktree arrives as
    /// `worktrees_root/<name>` and is NOT canonical, while an external
    /// worktree's path already is, and on macOS the same directory has two
    /// literal spellings. Falls back to the raw path when the directory is
    /// gone, so a vanished worktree still keys consistently for the reaper.
    pub(in crate::app) fn canonical_root(path: &std::path::Path) -> std::path::PathBuf {
        crate::worktree::canonical_root(path)
    }
}
