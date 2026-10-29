use crate::api::clients::DeclaredClient;
use crate::api::API_VERSION;
use crate::app::{
    capture_conversation_names, err, off_the_workers, record_idle_in_thread, require_str, AppState,
    HarnessExit, IdleObservation, TabKey,
};
use crate::changes::{Kind, ANNOUNCED_EVENTS, MAX_BATCH_MS, MIN_BATCH_MS};
use crate::encoding::b64decode;
use crate::execution::CallContext as SessionSender;
use crate::models::AgentProvider;
use crate::pty::HarnessSpec;
use crate::screen::{AttachSnapshot, TerminalHandle};
use crate::timing::FrameTimer;
use portable_pty::PtySize;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

mod cleanup;
mod create;
mod create_scope;
mod holds;
mod scope;

use scope::resolve_terminal_scope;

pub(in crate::app) fn terminal_scope_root(
    state: &mut AppState,
    params: &Value,
) -> Result<std::path::PathBuf, String> {
    Ok(resolve_terminal_scope(state, params)?.root)
}
pub(in crate::app) use scope::source_scope_requested;
pub(in crate::app) use scope::{TermScope, TerminalScope};
pub(crate) use scope::{MOVED_TERMINAL_SOURCE, SOURCE_SHELLS_LIVE, SOURCE_TERMINAL_BUSY};

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
        .as_terminal()
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
/// session to the update-status and models pushes.
///
/// Every session speaks the subscriptions contract: it hears no change until
/// it calls `changes.subscribe`. The `"changes"` param a client greets with is
/// still accepted and changes nothing — the legacy `board.changed` /
/// `entity.changed` contract it once chose between was cut (#207). A tab still
/// running an older SPA greets without it first, reads `changes.subscriptions`
/// off the answer, and greets again asking for subscriptions.
///
/// `push_events: true` is the feature detection. A bridge that predates push
/// invalidation answers `unknown method: session.hello`, and a client that
/// predates it never asks — so a new SPA against an old bridge, and an old SPA
/// against this one, both fall back to polling with nothing to configure.
///
/// Idempotent: a client may greet again after a reconnect.
pub(in crate::app) fn session_hello(
    state: &Arc<Mutex<AppState>>,
    sender: &SessionSender,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, String> {
    // The client record happens with the app mutex released — it takes its
    // own leaf lock, and nothing in this daemon may nest one lock inside
    // another it did not have to.
    let greeting = timer.lock(state).session_greeting(sender.bridge_epoch());
    if sender.has_events() {
        let mut app = timer.lock(state);
        app.subscribe_update_status(sender);
        app.subscribe_models_changed(sender);
        app.subscribe_harnesses_changed(sender);
    }
    timer.clock().clients().record(
        sender.session_id(),
        DeclaredClient::from_hello_params(params),
    );
    Ok(greeting)
}

impl AppState {
    /// Authoritative hello metadata without capturing an application event sink.
    pub(crate) fn session_greeting(&self, bridge_epoch: &str) -> Value {
        let changes = self.changes();
        let qa_agent = self.qa_agent;
        let journal_incarnation = self
            .store
            .as_ref()
            .map(|store| store.journal_incarnation().to_owned());
        json!({
            "api_version": API_VERSION,
            "bridge_epoch": bridge_epoch,
            "journal_incarnation": journal_incarnation,
            "capabilities": crate::api::capabilities(qa_agent),
            "ack_confirm_methods": crate::api::writes::ACK_CONFIRM_METHODS,
            "push_events": true,
            "events": ANNOUNCED_EVENTS,
            "coalesce_window_ms": changes.window().as_millis() as u64,
            // What a Part 1 adapter reads instead of probing for
            // `changes.subscribe`: whether this bridge serves subscriptions, the
            // kinds it filters on, whether an item carries what moved or only
            // its name, and the clamp on a batch interval.
            "changes": {
                "subscriptions": true,
                "mode": "subscriptions",
                "kinds": Kind::ALL.map(Kind::as_str),
                // `"bodies"`: every item carries the shape it names, so a client
                // paints from the push instead of refetching after it. The one
                // older answer this replaces is `"names"`, which no bridge that
                // reports this field ever sends.
                "items": "bodies",
                "batch_ms": { "min": MIN_BATCH_MS, "max": MAX_BATCH_MS },
            },
            // Whether this bridge can carry files on a task (spec: Tasks →
            // Attachments). Stated outright rather than left to the minor, because
            // a composer that offers a paperclip against a bridge with no
            // `tasks.attach` offers a press that cannot work.
            // `watching`: the watch verbs, the read marks, and `tracker_task`
            // rows on the inbox. Stated beside `attachments` rather than left to
            // the minor, for the same reason — an inbox that offers a Done press
            // against a bridge that cannot clear watched task rows offers one that cannot
            // work.
            "tasks": { "attachments": true, "watching": true },
            "thread_post_operations": {
                "version": 1,
                "status_method": "thread.operation",
                "states": ["queued", "claimed", "delivered", "uncertain"],
            },
            "message_context": { "version": 1 },
        })
    }
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
    create::create(state, params, timer)
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
        let mut s = timer.lock(state);
        let key = s.tab_key_of_wire_id(&term_id)?;
        s.check_terminal_scope(params, &key)?;
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
        let scope = resolve_terminal_scope(self, params)?;
        Ok(json!({ "terminals": self.shell_tabs_for(&scope) }))
    }

    /// The same rows, for a root already resolved: what `term.list` answers
    /// with, and what a `terminals` push item carries.
    pub(in crate::app) fn shell_tabs_for(&self, scope: &TerminalScope) -> Vec<Value> {
        shell_tab_rows(&self.session_registry.shell_tabs(), scope)
    }

    /// Close a user terminal: remove it, kill AND reap its shell (the existing
    /// zombie-prevention contract), and tell every attached client.
    pub(in crate::app) fn term_close(&mut self, params: &Value) -> Result<Value, String> {
        let term_id = require_str(params, "term_id")?;
        let key = self.tab_key_of_wire_id(&term_id)?;
        self.check_terminal_scope(params, &key)?;
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
        self.drop_ended_session_uploads(session_id);
        self.peers().end_session(session_id);
        for screen in self.session_registry.screen_handles() {
            screen.detach(session_id);
        }
    }

    /// Periodically close tabs whose worktree vanished out-of-band (nothing
    /// went through `finish_*_mutation` — e.g. the user deleted a worktree by
    /// hand). Runs beside `spawn_idle_monitor`.
    pub fn spawn_terminal_reaper(state: Arc<Mutex<AppState>>, interval: Duration) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let reaping = Arc::clone(&state);
                let reaped = off_the_workers(move || Self::reap_orphaned_terminals(&reaping)).await;
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
            .filter(|(id, a)| !self.stopped_at_usage_limit(id, &a.agents))
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
            .filter(|(id, a)| !self.stopped_at_usage_limit(id, &a.agents))
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

    /// Whether an entity's agent went quiet because its harness ran out of
    /// usage (task #58). That is not an agent walking off without reporting:
    /// it is idle with the limit as its reason, which the wire already says,
    /// and it is started again when the limit lifts.
    fn stopped_at_usage_limit(&self, entity_id: &str, agents: &crate::agent::AgentRoster) -> bool {
        agents
            .primary()
            .is_some_and(|agent| self.usage_limits.stopped(entity_id, &agent.id))
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
                let sweeping = Arc::clone(&state);
                off_the_workers(move || AppState::sweep_idle(&sweeping, quiet_threshold)).await;
            }
        });
    }

    /// One tick of the idle monitor: demote, reap finished routers, then ask
    /// the live agents their conversations' names.
    fn sweep_idle(state: &Arc<Mutex<AppState>>, quiet_threshold: Duration) {
        let (demoted, routers) = {
            let mut app = state.lock().unwrap();
            // A router process that died mid-decision told nobody, and its
            // capture would otherwise read as being routed forever.
            let demoted = app.mark_idle_tasks(quiet_threshold);
            let routers = app.reap_finished_router_sessions();
            for owner in &demoted {
                app.publish_owner_snapshot_projection(owner);
            }
            if !routers.is_empty() {
                app.publish_snapshot_projection();
            }
            (demoted, routers)
        };
        for task_id in demoted {
            eprintln!("idle monitor: {task_id} went idle without a done report");
        }
        for capture_id in routers {
            eprintln!("idle monitor: the router on {capture_id} stopped");
        }
        // Asked with the lock RELEASED: a terminal answers this off its
        // harness's transcript tree, which is a filesystem read.
        capture_conversation_names(state);
    }
}

/// One rendering for live registry facts and independently published facts.
/// Screen handles preserve the latest resize without retaining a session.
pub(in crate::app) fn shell_tab_rows(
    tabs: &[crate::app::runtime::sessions::ShellTabFact],
    scope: &TerminalScope,
) -> Vec<Value> {
    let mut terminals: Vec<(u64, Value)> = tabs
        .iter()
        .filter(|tab| tab.key.root == scope.root)
        .filter(|tab| scope.accepts(tab.project_source.as_ref()))
        .filter_map(|tab| {
            let (cols, rows) = tab.screen.as_ref()?.size();
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
    terminals.into_iter().map(|(_, entry)| entry).collect()
}

#[cfg(test)]
mod greeting_tests {
    use super::*;
    use crate::execution::session::{
        ClientSessions, DeliveryError, EncodedEvent, EventSink, Opening, ReplySink, RequestContext,
        StreamClass, VerifiedCaller,
    };
    use crate::execution::{ExecutionService, Invocation};

    struct Reply;
    impl ReplySink for Reply {
        fn try_send(&self, _event: EncodedEvent) -> Result<(), DeliveryError> {
            Ok(())
        }
    }
    impl EventSink for Reply {
        fn try_send(&self, _event: EncodedEvent) -> Result<(), DeliveryError> {
            Ok(())
        }
    }

    fn producer_state(root: &std::path::Path) -> Arc<Mutex<AppState>> {
        AppState::new_unrooted(root, "main", true, "unused.sock")
            .with_update_service(Arc::new(crate::update::UpdateService::disabled(
                "0.2.0".into(),
                "test".into(),
            )))
            .shared()
    }

    fn producer_tasks(
        app: &AppState,
        session_id: &str,
        scope: (u64, Option<u64>),
    ) -> [tokio::task::AbortHandle; 3] {
        let subscriptions = [
            &app.update_subscriptions,
            &app.models_subscriptions,
            &app.harnesses_subscriptions,
        ];
        std::array::from_fn(|index| {
            assert_eq!(subscriptions[index].len(), 1);
            let (context, task) = subscriptions[index].get(session_id).unwrap();
            assert_eq!(context.read_scope(), scope);
            assert!(context.events_are_live());
            assert!(!task.is_finished());
            task.abort_handle()
        })
    }

    async fn shared_hello(service: &ExecutionService, request: RequestContext) -> Value {
        let mut call = tokio::time::timeout(
            Duration::from_secs(5),
            service.submit(
                request,
                Invocation::decode("session.hello", &json!({})).unwrap(),
            ),
        )
        .await
        .expect("hello admission must settle")
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), call.reply())
            .await
            .expect("hello execution must settle")
            .unwrap()
    }

    async fn retired(tasks: &[tokio::task::AbortHandle]) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while tasks.iter().any(|task| !task.is_finished()) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("replaced producer tasks must finish after cancellation");
    }

    #[tokio::test]
    async fn execution_greeting_before_application_binding_registers_no_producers() {
        let temp = tempfile::tempdir().unwrap();
        let state = producer_state(temp.path());
        let service = ExecutionService::new(AppState::handler(state.clone()));
        let session = service
            .sessions()
            .open(VerifiedCaller::verified_relay("device", Opening::new()))
            .unwrap();
        for _ in 0..2 {
            let greeting = service.greeting(&session).unwrap();
            assert_eq!(greeting["bridge_epoch"], crate::bridge_epoch::current());
            assert_eq!(greeting["api_version"], API_VERSION);
            let app = state.lock().unwrap();
            assert!(app.update_subscriptions.is_empty());
            assert!(app.models_subscriptions.is_empty());
            assert!(app.harnesses_subscriptions.is_empty());
        }
    }

    #[tokio::test]
    async fn application_hello_upserts_and_ignores_retired_binding_requests_and_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let state = producer_state(temp.path());
        let handler = AppState::handler(state.clone());
        let service = ExecutionService::new(handler.clone());
        let sessions = service.sessions();
        let session = sessions
            .open(VerifiedCaller::verified_relay("device", Opening::new()))
            .unwrap();
        let old = sessions
            .bind_events(&session, StreamClass::Application, Arc::new(Reply))
            .unwrap();
        let request = sessions
            .request(&session, Arc::new(Reply), Some(&old))
            .unwrap();
        assert_eq!(shared_hello(&service, request).await["ok"], true);
        let first = producer_tasks(
            &state.lock().unwrap(),
            session.id(),
            old.events().read_scope(),
        );

        let request = sessions
            .request(&session, Arc::new(Reply), Some(&old))
            .unwrap();
        assert_eq!(shared_hello(&service, request).await["ok"], true);
        retired(&first).await;
        let repeated = producer_tasks(
            &state.lock().unwrap(),
            session.id(),
            old.events().read_scope(),
        );

        let delayed = sessions
            .request(&session, Arc::new(Reply), Some(&old))
            .unwrap();
        let successor = sessions
            .replace_events(
                &session,
                StreamClass::Application,
                Some(&old),
                Arc::new(Reply),
            )
            .unwrap();
        let request = sessions
            .request(&session, Arc::new(Reply), Some(&successor))
            .unwrap();
        assert_eq!(shared_hello(&service, request).await["ok"], true);
        retired(&repeated).await;
        let current = producer_tasks(
            &state.lock().unwrap(),
            session.id(),
            successor.events().read_scope(),
        );

        let _stale_reply = shared_hello(&service, delayed).await;
        old.close();
        handler.cleanup_outputs();
        let retained = producer_tasks(
            &state.lock().unwrap(),
            session.id(),
            successor.events().read_scope(),
        );
        assert!(current.iter().all(|task| !task.is_finished()));
        assert!(retained.iter().all(|task| !task.is_finished()));
        assert!(successor.is_live());

        successor.close();
        handler.cleanup_outputs();
        retired(&retained).await;
        let app = state.lock().unwrap();
        assert!(app.update_subscriptions.is_empty());
        assert!(app.models_subscriptions.is_empty());
        assert!(app.harnesses_subscriptions.is_empty());
    }

    #[tokio::test]
    async fn greeting_without_events_registers_no_producers() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::new_unrooted(temp.path(), "main", true, "unused.sock").shared();
        let sessions = ClientSessions::new(crate::bridge_epoch::current());
        let session = sessions
            .open(VerifiedCaller::verified_relay("device", Opening::new()))
            .unwrap();
        let request = sessions.request(&session, Arc::new(Reply), None).unwrap();
        let sender = SessionSender::from_request(&request);
        assert!(!sender.has_events());
        let timer = crate::timing::FrameClock::new().frame("session.hello");
        session_hello(&state, &sender, &json!({}), &timer).unwrap();
        let app = state.lock().unwrap();
        assert!(app.update_subscriptions.is_empty());
        assert!(app.models_subscriptions.is_empty());
        assert!(app.harnesses_subscriptions.is_empty());
    }

    #[tokio::test]
    async fn greeting_with_events_registers_application_producers() {
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::new_unrooted(temp.path(), "main", true, "unused.sock").shared();
        let sender = SessionSender::legacy(
            "events-client",
            crate::execution::session::Opening::new(),
            Arc::new(|_| true),
        );
        assert!(sender.has_events());
        let timer = crate::timing::FrameClock::new().frame("session.hello");
        session_hello(&state, &sender, &json!({}), &timer).unwrap();
        let mut app = state.lock().unwrap();
        assert!(app.models_subscriptions.contains_key(sender.session_id()));
        assert!(app
            .harnesses_subscriptions
            .contains_key(sender.session_id()));
        app.unsubscribe_models_changed(sender.session_id());
        app.unsubscribe_harnesses_changed(sender.session_id());
    }

    #[tokio::test]
    async fn replacement_producer_cleanup_preserves_successor_binding() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let temp = tempfile::tempdir().unwrap();
        let state = AppState::new_unrooted(temp.path(), "main", true, "unused.sock").shared();
        let opening = Opening::new();
        let old_live = Arc::new(AtomicBool::new(true));
        let check = old_live.clone();
        let old = SessionSender::legacy("replaced-client", opening.clone(), Arc::new(|_| true))
            .bound(
                1,
                Arc::new(move || check.load(Ordering::Acquire)),
                Arc::new(Mutex::new(())),
            );
        let new = SessionSender::legacy("replaced-client", opening, Arc::new(|_| true)).bound(
            2,
            Arc::new(|| true),
            Arc::new(Mutex::new(())),
        );
        let timer = crate::timing::FrameClock::new().frame("session.hello");
        session_hello(&state, &old, &json!({}), &timer).unwrap();
        let old_task = state
            .lock()
            .unwrap()
            .models_subscriptions
            .get(old.session_id())
            .unwrap()
            .1
            .abort_handle();
        old_live.store(false, Ordering::Release);
        session_hello(&state, &new, &json!({}), &timer).unwrap();
        let new_task = state
            .lock()
            .unwrap()
            .models_subscriptions
            .get(new.session_id())
            .unwrap()
            .1
            .abort_handle();
        let removed = state.lock().unwrap().prune_output_resources();
        assert!(removed.is_empty());
        tokio::task::yield_now().await;
        assert!(
            old_task.is_finished(),
            "replacement retires the old producer"
        );
        assert!(
            !new_task.is_finished(),
            "old cleanup cannot abort its successor"
        );
        let mut app = state.lock().unwrap();
        assert_eq!(
            app.models_subscriptions
                .get(new.session_id())
                .unwrap()
                .0
                .read_scope(),
            new.read_scope()
        );
        app.unsubscribe_models_changed(new.session_id());
        app.unsubscribe_harnesses_changed(new.session_id());
    }

    #[test]
    fn authoritative_greeting_matches_legacy_hello_without_a_sink() {
        let temp = tempfile::tempdir().unwrap();
        let app = AppState::new_unrooted(temp.path(), "main", true, "unused.sock");
        let sender = SessionSender::detached("greeting-client");
        let expected = app.session_greeting(sender.bridge_epoch());
        let state = app.shared();
        let timer = crate::timing::FrameClock::new().frame("session.hello");
        let actual = session_hello(&state, &sender, &json!({}), &timer).unwrap();
        assert_eq!(actual, expected);
        assert_eq!(expected["bridge_epoch"], sender.bridge_epoch());
        assert_eq!(
            expected["ack_confirm_methods"],
            json!(crate::api::writes::ACK_CONFIRM_METHODS)
        );
    }
}
