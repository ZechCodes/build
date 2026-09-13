use crate::api::clients::DeclaredClient;
use crate::api::API_VERSION;
use crate::app::{
    capture_conversation_names, err, record_idle_in_thread, require_str, spawn_tab_pumps, AppState,
    HarnessExit, IdleObservation, LifecycleDiagnostic, Tab, TabKey,
};
use crate::carrier::SessionSender;
use crate::changes::{Kind, ANNOUNCED_EVENTS, MAX_BATCH_MS, MIN_BATCH_MS};
use crate::encoding::b64decode;
use crate::models::AgentProvider;
use crate::pty::HarnessSpec;
use crate::screen::{AttachSnapshot, TerminalHandle};
use crate::thread::SessionInstance;
use crate::timing::FrameTimer;
use portable_pty::PtySize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A worktree-backed surface a terminal or fs call is scoped to. Scope roots are
/// resolved server-side ONLY (spec §1): ids map to roots through the bridge's own
/// records — a client-supplied filesystem path is never a scope root.
#[derive(Debug, Clone)]
pub(in crate::app) enum TermScope {
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
    pub(in crate::app) fn parse(params: &Value) -> Result<TermScope, String> {
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
    pub(in crate::app) fn resolve_root(
        &self,
        state: &mut AppState,
    ) -> Result<std::path::PathBuf, String> {
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

/// What a client is told about the tab it just attached to.
pub(in crate::app) struct TabFacts {
    pub(in crate::app) term_id: String,
    pub(in crate::app) live: bool,
    pub(in crate::app) provider: Option<AgentProvider>,
}

/// One tab's client-facing surface, taken out of the registry together: what
/// the reply says about the tab, and the terminal the client attaches to.
pub(in crate::app) struct TabAttachment {
    pub(in crate::app) facts: TabFacts,
    pub(in crate::app) terminal: TerminalHandle,
}

/// The one kind of program a user terminal runs, on the wire. `term.create`
/// echoes it and `term.list` carries it, so a reloaded client still labels the
/// tab by what is actually in it.
pub(in crate::app) const SHELL_TAB_KIND: &str = "shell";

/// At most this many user terminals daemon-wide, all worktrees combined. An
/// agent tab never counts against it — there is at most one per worktree, and
/// it must stay reachable however many shells are open.
pub(in crate::app) const MAX_USER_TERMINALS: usize = 16;

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
pub(in crate::app) fn require_shell_kind(params: &Value) -> Result<(), String> {
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
pub(in crate::app) fn no_terminal_here(term_id: &str) -> String {
    format!(
        "{term_id} has no terminal — this agent reports its reasoning, tool calls and \
         messages into its conversation, which is where its work is read"
    )
}

/// The harness a shell tab spawns in its worktree root: `-i -l`, so the user
/// gets their own rc files and prompt — their machine, shown honestly.
pub(in crate::app) fn shell_harness_spec(shell: &str) -> HarnessSpec {
    HarnessSpec::new(shell)
        .arg("-i")
        .arg("-l")
        .env("TERM", "xterm-256color")
}

pub(in crate::app) fn terminal_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Greet a browser session: announce what this bridge pushes, and subscribe the
/// session to it.
///
/// `"changes"` picks which push contract the session speaks (wire spec, step
/// 1.5). `"legacy"` — the default, and what every client that predates
/// subscriptions sends — is today's behaviour: the session hears
/// `board.changed` / `entity.changed` for every entity on the device.
/// `"subscriptions"` means it hears nothing until it calls
/// `changes.subscribe`, and drops any legacy subscription it already had, so
/// a reconnecting client that switches contracts is not served both.
///
/// `push_events: true` is the feature detection. A bridge that predates push
/// invalidation answers `unknown method: session.hello`, and a client that
/// predates it never asks — so a new SPA against an old bridge, and an old SPA
/// against this one, both fall back to polling with nothing to configure.
///
/// Idempotent: a client may greet again after a reconnect, and the bus keeps
/// one subscription per session id.
pub(in crate::app) fn session_hello(
    state: &Arc<Mutex<AppState>>,
    sender: &SessionSender,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, String> {
    // The subscribe and the client record both happen with the app mutex
    // released — each takes its own leaf lock, and nothing in this daemon may
    // nest one lock inside another it did not have to.
    let changes = timer.lock(state).changes();
    let subscriptions = params.get("changes").and_then(Value::as_str) == Some("subscriptions");
    if subscriptions {
        changes.unsubscribe_legacy(sender.session_id());
    } else {
        changes.subscribe_legacy(sender);
    }
    timer.clock().clients().record(
        sender.session_id(),
        DeclaredClient::from_hello_params(params),
    );
    Ok(json!({
        "api_version": API_VERSION,
        "push_events": true,
        "events": ANNOUNCED_EVENTS,
        "coalesce_window_ms": changes.window().as_millis() as u64,
        // What a Part 1 adapter reads instead of probing for
        // `changes.subscribe`: whether this bridge serves subscriptions, the
        // kinds it filters on, and the clamp on a batch interval.
        "changes": {
            "subscriptions": true,
            "mode": if subscriptions { "subscriptions" } else { "legacy" },
            "kinds": Kind::ALL.map(Kind::as_str),
            "batch_ms": { "min": MIN_BATCH_MS, "max": MAX_BATCH_MS },
        },
        "thread_post_operations": {
            "version": 1,
            "status_method": "thread.operation",
            "states": ["queued", "claimed", "delivered", "uncertain"],
        },
        "message_context": { "version": 1 },
    }))
}

/// The numeric suffix of a minted `term-<n>` id — the `term.list` sort key.
pub(in crate::app) fn term_id_suffix(term_id: &str) -> u64 {
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
pub(in crate::app) fn term_create(
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
        let tab_id = s.session_registry.next_terminal_id();
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
        let pumps = s.session_registry.insert_shell(key.clone(), tab, rx);
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
pub(in crate::app) fn term_attach(
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
pub(in crate::app) fn term_input(
    state: &Arc<Mutex<AppState>>,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, String> {
    let term_id = require_str(params, "term_id")?;
    let data = b64decode(&require_str(params, "data")?)?;
    let terminal = {
        let s = timer.lock(state);
        let key = s.tab_key_of_wire_id(&term_id)?;
        let (live, terminal) = s.session_registry.terminal_access(&key)?;
        if !live {
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
pub(in crate::app) fn term_resize(
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
        s.session_registry.terminal_resize_access(&key)?
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
pub(in crate::app) fn term_ack(
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
        // A client that was never allowed to attach has nothing to acknowledge,
        // so it hears the same refusal rather than acking into a screen that is
        // not there.
        s.session_registry.terminal_screen(&key)?
    };
    // The ack may push one resync snapshot to the caller, so it happens with the
    // app mutex released like every other write to a screen.
    screen.ack(sender.session_id(), cursor);
    Ok(json!({ "ok": true }))
}

/// The attach reply, written in one place so both verbs answer in one shape.
pub(in crate::app) fn attach_view(facts: TabFacts, screen: AttachSnapshot) -> Value {
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
pub(in crate::app) fn attach_to_tab(
    attachment: TabAttachment,
    sender: &SessionSender,
    cols: u16,
    rows: u16,
) -> Value {
    let viewport = attachment.facts.live.then_some((cols, rows));
    let reading = attachment.terminal.attach(sender, viewport);
    attach_view(attachment.facts, reading)
}

impl AppState {
    /// How many of the human's own shells the tab registry holds. The
    /// daemon-wide terminal cap counts these and never an agent tab: an agent
    /// is Build's, always reachable, and must not be crowded out by shells.
    pub(in crate::app) fn shell_tab_count(&self) -> usize {
        self.session_registry.shell_count()
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
    pub(in crate::app) fn tab_key_of_wire_id(&self, wire_id: &str) -> Result<TabKey, String> {
        self.session_registry.key_for_wire_id(wire_id)
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
    pub(in crate::app) fn term_list(&mut self, params: &Value) -> Result<Value, String> {
        let root = TermScope::parse(params)?.resolve_root(self)?;
        let mut terminals: Vec<(u64, Value)> = self
            .session_registry
            .shell_tabs_at(&root)
            .into_iter()
            .filter_map(|tab| {
                let (cols, rows) = tab.size?;
                Some((
                    term_id_suffix(&tab.key.tab_id),
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
    pub(in crate::app) fn term_close(&mut self, params: &Value) -> Result<Value, String> {
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
    pub(in crate::app) fn attachment(&self, key: &TabKey) -> Result<TabAttachment, String> {
        self.session_registry.attachment(key)
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
    pub(in crate::app) fn drop_session(&mut self, session_id: &str) {
        self.peers.end_session(session_id);
        for screen in self.session_registry.screen_handles() {
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
    pub(in crate::app) fn reap_orphaned_terminals(&mut self) -> Vec<String> {
        let vanished: Vec<TabKey> = self
            .session_registry
            .tab_keys()
            .into_iter()
            .filter(|key| !key.root.exists())
            .collect();
        let mut reaped = Vec::new();
        let mut killed_agents: Vec<SessionInstance> = Vec::new();
        for key in vanished {
            let provider = self.session_registry.agent_snapshot(&key).and_then(|tab| {
                tab.role
                    .agent()
                    .map(|(owner, agent)| (owner.to_string(), agent.to_string()))
            });
            let provider_thread_id = provider
                .as_ref()
                .and_then(|(owner, agent)| self.recorded_resume_id(owner, agent));
            let Some(retired) = self.session_registry.retire_tab(
                &key,
                "reaped",
                LifecycleDiagnostic {
                    event: "shutdown_requested",
                    origin: "tab_retirement",
                    reason: Some("reaped"),
                    operation_id: None,
                    provider_thread_id: provider_thread_id.as_deref(),
                    caller: Some(std::panic::Location::caller()),
                },
            ) else {
                continue;
            };
            if let Some(instance) = retired.instance {
                killed_agents.push(instance);
            }
            reaped.push(retired.wire_id);
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
            .session_registry
            .waiting_screen_keys()
            .into_iter()
            .filter(|key| !key.root.exists())
            .collect();
        for key in orphaned {
            if let Some(wire_id) = self.session_registry.remove_waiting_screen(&key, "reaped") {
                reaped.push(wire_id);
            }
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
    pub(in crate::app) fn mark_idle_tasks(&mut self, quiet_threshold: Duration) -> Vec<String> {
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
        let idle_check =
            |observation: Option<IdleObservation>, turn_undelivered: bool| match observation {
                None if turn_undelivered => None,
                None => Some(None),
                Some(IdleObservation::Active) => None,
                Some(IdleObservation::Idle {
                    exit_code: Some(code),
                    epitaph,
                }) => Some(Some(HarnessExit { code, epitaph })),
                Some(IdleObservation::Idle {
                    exit_code: None, ..
                }) => Some(None),
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
                    self.session_registry.idle_observation(
                        &TabKey::agent(&root, &a.agents.primary()?.id),
                        quiet_threshold,
                    ),
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
                    self.session_registry.idle_observation(
                        &TabKey::agent(&root, &a.agents.primary()?.id),
                        quiet_threshold,
                    ),
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
}
