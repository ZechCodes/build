use crate::api::{self, ApiError, API_VERSION};
use crate::app::{
    agent_attach, agent_interrupt, agent_start, rtc_close, rtc_ice, rtc_offer, session_hello,
    stream_start, term_ack, term_attach, term_create, term_input, term_resize, AppState,
    DeferredNext, DeliveryRunner, PeersSlot,
};
use crate::carrier::{FrameHandler, SessionSender};
use crate::orchestrator::OrchestratorError;
use crate::timing::FrameTimer;
use crate::transport;
use crate::transport::Frame;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// The verbs that count as the human acting on an entity, and the param naming
/// it. Deliberately asymmetric: opening a stage doc counts, because a task is
/// a queue you triage by reading and reading one IS engaging with it — while a
/// worktree needs an action, since looking at a diff is not the same as doing
/// something about it. An agent's own work never appears here; if it did, the
/// rail would reorder itself while you watched.
pub(in crate::app) const INTERACTION_VERBS: &[(&str, &str)] = &[
    ("run.request_changes", "run_id"),
    ("run.message", "run_id"),
    ("run.git_action", "run_id"),
    ("run.abandon", "run_id"),
    ("run.adopt", "worktree_id"),
    ("workspace.ensure_conversation", "workspace_id"),
    ("project.ensure_conversation", "project_id"),
    ("thread.post", "entity_id"),
];

/// The verbs that are the user acting, for the user's session (spec: Tasks
/// dashboard → Done since you left). Broader than [`INTERACTION_VERBS`]: that
/// table decides which entity rises in the rail, this one only whether the
/// user was here. Reading counts, so the read marks are in it.
///
/// A list and not "every write", because some writes a client makes on its
/// own — acks, resizes, subscriptions, re-attaching after a reconnect — and a
/// tab left open overnight must not keep a session alive. `term.input` stays
/// out for the same reason: a terminal answers a program's queries by itself.
///
/// `agent.start`, `agent.interrupt` and `term.create` are answered outside
/// `dispatch` ([`session_scoped`]), so [`dispatch_frame`] stamps those itself.
/// `user.present` is not listed: it records the arrival and answers the
/// session in one step.
pub(in crate::app) const USER_ACTIVITY_VERBS: &[&str] = &[
    "agent.add",
    "agent.choose",
    "agent.interrupt",
    "agent.remove",
    "agent.start",
    "branch.dispatch",
    "branch.finish",
    "bridge.install_update",
    "capture.answer",
    "capture.cancel",
    "capture.create",
    "capture.reroute",
    "conversation.settings",
    "conversation.unwatch",
    "conversation.watch",
    "entity.dismiss",
    "entity.mute",
    "entity.seen",
    "fs.mkdir",
    "fs.write",
    "git.checkout_ref",
    "git.commit",
    "git.discard",
    "git.fetch",
    "git.merge_abort",
    "git.pull",
    "git.push",
    "git.stage",
    "git.stash",
    "git.stash_pop",
    "tasks.assign",
    "tasks.attach",
    "tasks.close",
    "tasks.comment",
    "tasks.create",
    "tasks.read_through",
    "tasks.reopen",
    "tasks.unwatch",
    "tasks.update",
    "tasks.watch",
    "project.add_source",
    "project.create",
    "project.delete",
    "project.init_git",
    "project.remove_source",
    "project.set_isolation",
    "project.set_remote",
    "project.update_source",
    "run.abandon",
    "run.adopt",
    "run.git_action",
    "run.message",
    "run.request_changes",
    "settings.set",
    "term.create",
    "thread.attach",
    "thread.post",
    "workspace.add_directory",
    "workspace.create",
    "workspace.delete",
    "workspace.finish",
    "workspace.init_git",
    "workspace.reclaim",
    "workspace.remove_directory",
    "workspace.rename",
    "workspace.retry",
];

/// Dispatch one decrypted request frame. [`session_scoped`] answers the verbs
/// that need the shared `Arc` (background producer/pump) or the caller's own
/// `SessionSender` (somewhere to push live output to); [`routed`] answers
/// everything else, under a short-held lock, through `api::v1` first and the
/// legacy table second. Both outcomes leave here as one reply envelope
/// ([`crate::api::reply`]), so the success shape and the refusal shape — code,
/// retryability, details — are written in exactly one place.
pub(in crate::app) fn dispatch_frame(
    state: &Arc<Mutex<AppState>>,
    peers: &PeersSlot,
    sender: SessionSender,
    frame: Frame,
    timer: FrameTimer,
) -> Value {
    // A session ended (client `close` frame, or the relay's session_closed on
    // browser disconnect): release its attachments so the bridge stops encrypting
    // terminal output into a session nobody will ever read.
    if frame.frame_type == transport::CLOSE_FRAME_TYPE {
        let (changes, watchers) = {
            let mut app = timer.lock(state);
            app.drop_session(sender.session_id());
            app.unsubscribe_update_status(sender.session_id());
            app.unsubscribe_models_changed(sender.session_id());
            (app.changes(), app.watchers())
        };
        changes.unsubscribe(sender.session_id());
        // Its subscriptions went with it; the watchers they covered follow.
        watchers.wake();
        timer.clock().clients().forget(sender.session_id());
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

    // Signaling first, and off the app mutex entirely: an offer or a candidate
    // touches the peers and nothing else, and an ICE restart that waits behind
    // a busy app is a phone that gives up on the device (task #128). Admission
    // is not consulted either — a peer negotiating across an update handoff
    // reads and writes no state the handoff protects.
    if let Some(outcome) = signaling(peers, &sender, &method, &params) {
        return api::reply(id, outcome.map_err(ApiError::from));
    }

    // Keep a request admitted through its off-lock git work and queue drain.
    // The idle updater can claim the bridge only between complete requests.
    let admission = if matches!(
        method.as_str(),
        "ping" | "bridge.stats" | "bridge.update_status"
    ) {
        None
    } else {
        state.lock().unwrap().update_admission()
    };
    let _lease = match admission {
        Some(gate) => match gate.try_enter() {
            Some(lease) => Some(lease),
            None => {
                return api::reply(id, Err(ApiError::busy("bridge update handoff in progress")))
            }
        },
        None => None,
    };

    let result = match session_scoped(state, &sender, &method, &params, &timer) {
        Some(outcome) => noted_as_user_activity(state, &method, outcome, &timer).map_err(|error| {
            if error == PROJECT_DELETION_IN_PROGRESS {
                ApiError::unavailable(error)
            } else if error == crate::reclaim::RESERVED {
                // A terminal opening in a workspace the reclaim service has
                // reserved: try again in a moment.
                ApiError::busy(error)
            } else {
                conversation_session_refusal(error)
            }
        }),
        // The caller is named for the frame: the `changes.*` verbs push to
        // it, and `routed` carries only the state.
        None => crate::api::v1::changes::with_session(&sender, || {
            routed(state, &method, &params, &timer)
        }),
    };
    api::reply(id, result)
}

fn conversation_session_refusal(error: String) -> ApiError {
    if error.starts_with("stale thread_id")
        || error.starts_with("stale unversioned mutation")
        || error.contains("conversation is being cleared")
    {
        ApiError::classify(error)
    } else {
        ApiError::from(error)
    }
}

/// Resume, Stop and a new terminal are answered by [`session_scoped`] and
/// never reach `dispatch`, whose stamp is the only other one; so a
/// successful one is noted here. A refusal is not the user being here.
fn noted_as_user_activity(
    state: &Arc<Mutex<AppState>>,
    method: &str,
    outcome: Result<Value, String>,
    timer: &FrameTimer,
) -> Result<Value, String> {
    if outcome.is_ok() && USER_ACTIVITY_VERBS.contains(&method) {
        let now = i64::try_from(crate::agent::now_ms()).unwrap_or(i64::MAX);
        timer.lock(state).note_user_activity(now);
    }
    outcome
}

/// The three signaling verbs, answered from the peers slot alone. `None` is
/// every other verb.
fn signaling(
    peers: &PeersSlot,
    sender: &SessionSender,
    method: &str,
    params: &Value,
) -> Option<Result<Value, String>> {
    let peers = peers.get();
    Some(match method {
        "rtc.offer" => rtc_offer(&peers, sender, params),
        "rtc.ice" => rtc_ice(&peers, sender, params),
        "rtc.close" => rtc_close(&peers, sender),
        _ => return None,
    })
}

/// What a `ping` answers, wherever it is answered from.
///
/// Two places answer it and they must answer identically: the carrier's fast
/// path ([`crate::carrier::FrameIntake::accept`]), which is the one a browser's
/// liveness probe reaches, and [`AppState::route_legacy`] for the callers that
/// dispatch a frame directly.
///
/// `push_events` rides the probe as well as the greeting: a client that only
/// ever pings can still tell whether this bridge will invalidate for it, and an
/// old client ignores the extra field.
pub(crate) fn pong() -> Value {
    json!({
        "pong": true,
        "api_version": API_VERSION,
        "push_events": true,
        "message_context": { "version": 1 },
    })
}

/// The verbs that cannot go through [`AppState::route`]: each needs the
/// caller's own [`SessionSender`] (somewhere to push to) or the shared `Arc`
/// (a background producer or pump to spawn). `None` means "not one of mine",
/// which is every verb `route` — and so `api::v1` — answers.
fn session_scoped(
    state: &Arc<Mutex<AppState>>,
    sender: &SessionSender,
    method: &str,
    params: &Value,
    timer: &FrameTimer,
) -> Option<Result<Value, String>> {
    if !allowed_during_project_deletion(method) && timer.lock(state).project_deletion_in_progress {
        return Some(Err(PROJECT_DELETION_IN_PROGRESS.to_string()));
    }
    Some(match method {
        // The greeting: what this bridge can do for the session, and — for the
        // capabilities that need somewhere to send to — the subscription
        // itself. Needs the caller's own `SessionSender`, which is why it is
        // here and not in `route`.
        "session.hello" => session_hello(state, sender, params, timer),
        // Answered from the frame clock alone, never from `AppState`: the frame
        // that asks what is wedging the daemon must not queue behind the wedge.
        "bridge.stats" => Ok(timer.clock().stats()),
        // QA-only (`BRIDGE_QA_AGENT=1`); unknown to everyone else.
        "stream.start" if timer.lock(state).qa_agent => stream_start(state, params, timer),
        // Opening a terminal or an agent in a worktree IS interacting with it in
        // Build — it is the reason a hand-made worktree graduates out of the
        // Worktrees row. This arm bypasses `dispatch`, so it stamps for itself.
        "term.create" => {
            let created = term_create(state, params, timer);
            if created.is_ok() {
                if let Some(scope_id) = params
                    .get("workspace_id")
                    .or_else(|| params.get("run_id"))
                    .or_else(|| params.get("worktree_id"))
                    .and_then(Value::as_str)
                {
                    timer.lock(state).touch_attention(scope_id);
                }
            }
            created
        }
        "term.attach" => term_attach(state, sender, params, timer),
        // A write to a child's pty blocks while the child is not draining, so
        // both of these take the handle under the lock and write with it
        // released.
        "term.input" => term_input(state, params, timer),
        "term.resize" => term_resize(state, params, timer),
        // Needs the caller's own session: an ack speaks for one client's
        // receive queue, not for the screen.
        "term.ack" => term_ack(state, sender, params, timer),
        "agent.attach" => agent_attach(state, sender, params, timer),
        // Bypasses `dispatch` because it hands its queued turn to
        // `DeliveryRunner`, which needs the shared handle `dispatch` does not
        // have.
        "agent.start" => agent_start(state, params, timer),
        // Upstream's stop control, on the same footing as `agent.start`: it
        // reaches the harness through the shared handle, which `dispatch`
        // does not hold.
        "agent.interrupt" => agent_interrupt(state, params, timer),
        _ => return None,
    })
}

/// Everything else: `route` (v1 first, legacy second) under a short-held lock,
/// with the git work it defers run with the mutex released.
fn routed(
    state: &Arc<Mutex<AppState>>,
    method: &str,
    params: &Value,
    timer: &FrameTimer,
) -> Result<Value, ApiError> {
    // A verb whose git work must not run under the lock hands that work back
    // rather than doing it here; the drain below runs it with the mutex
    // released. See `AppState::deferred_work`.
    let (dispatched, deferred) = timer.lock(state).dispatch_deferring(method, params);
    let dispatched = match deferred {
        // THE POINT OF ALL THIS: seconds to minutes of git — a status walk, a
        // fetch, a merge, a `git worktree remove` of a six-gigabyte checkout —
        // with every other frame, every terminal pump and the relay's own read
        // loop free to make progress meanwhile.
        Some(mut deferred) => loop {
            let done = deferred.run();
            match timer.lock(state).apply_deferred_stage(method, params, done) {
                DeferredNext::Answered(answer) => break answer.map_err(ApiError::classify),
                DeferredNext::Again(next) => deferred = next,
            }
        },
        None => dispatched,
    };
    // A verb speaks to a worktree's agent by queuing a turn, and the frame's
    // own answer never waits for it to arrive: the mutation is durable, and a
    // cold spawn blocks for seconds on the harness's readiness wait while the
    // browser gives up at twelve.
    if dispatched.is_ok() {
        DeliveryRunner::drain(state, timer);
    }
    dispatched
}

/// The entity ids a frame names — in the params it was called with, and in the
/// result it produced. Order-preserving and deduped.
///
/// One list rather than a per-verb table, because the surface addresses an
/// entity three ways: by a bare `id` where the caller holds one entity and
/// knows nothing else about it, by its kind (`task_id` / `run_id` /
/// `worktree_id`) where the verb is that kind's, and out of the result where
/// the call is what minted it. `project_id` is deliberately absent: a project
/// is not an entity a browser holds a detail view of.
pub(in crate::app) fn entity_ids_of(params: &Value, result: &Value) -> Vec<String> {
    const ENTITY_KEYS: [&str; 7] = [
        "id",
        "entity_id",
        "task_id",
        "plan_id",
        "run_id",
        "worktree_id",
        "workspace_id",
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

pub(in crate::app) fn err(e: OrchestratorError) -> String {
    e.to_string()
}

/// How this bridge tells a client a param it needed was not there — written
/// once, so every required param reads the same to the client.
pub(in crate::app) fn missing_param(key: &str) -> String {
    format!("missing required param: {key}")
}

const PROJECT_DELETION_IN_PROGRESS: &str =
    "a project is being deleted; retry after deletion completes";

/// Keep observations and session subscriptions available during filesystem cleanup.
/// New verbs default to blocked until their read-only behavior is established.
fn allowed_during_project_deletion(method: &str) -> bool {
    matches!(
        method,
        "ping"
            | "session.hello"
            | "bridge.stats"
            | "bridge.update_status"
            | "bridge.check_update"
            | "changes.subscribe"
            | "changes.unsubscribe"
            | "rtc.offer"
            | "rtc.ice"
            | "rtc.close"
            | "term.list"
            | "term.attach"
            | "term.ack"
            | "term.resize"
            | "agent.attach"
            | "stream.events"
            | "stream.state"
            | "archived.list"
            | "board.list"
            | "capture.get"
            | "models.list"
            | "project.list"
            | "settings.get"
            | "fs.list"
            | "fs.read"
            | "fs.tree"
            | "git.changeset_diff"
            | "git.diff"
            | "git.log"
            | "git.refs"
            | "git.show"
            | "git.status"
            | "git.unpushed"
            | "task.stage_diff"
            | "task.doc"
            | "task.get"
            | "task.stage_doc"
            | "task.stages"
            | "run.diff"
            | "worktree.diff"
            | "thread.activity"
            | "thread.attachment"
            | "thread.operation"
            | "thread.page"
            | "thread.revision"
            | "workspace.get"
            | "workspace.list"
            | "workspace.git_init_options"
    )
}

pub(in crate::app) fn optional_nonempty_string<'a>(
    params: &'a Value,
    field: &str,
) -> Result<Option<&'a str>, String> {
    match params.get(field) {
        None => Ok(None),
        Some(Value::String(value)) if value.is_empty() => Err(format!("{field} cannot be empty")),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(format!("{field} must be a string")),
    }
}

pub(in crate::app) fn require_array(params: &Value, key: &str) -> Result<Vec<Value>, String> {
    require_value(params, key)?
        .as_array()
        .cloned()
        .ok_or_else(|| missing_param(key))
}

pub(in crate::app) fn require_str(params: &Value, key: &str) -> Result<String, String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| missing_param(key))
}

pub(in crate::app) fn require_value(params: &Value, key: &str) -> Result<Value, String> {
    params.get(key).cloned().ok_or_else(|| missing_param(key))
}

impl AppState {
    /// The relay's frame handler over a shared state. `stream.start`/`term.attach`
    /// need the shared handle (background producers/pumps), so it dispatches
    /// through [`dispatch_frame`].
    ///
    /// The clock comes off the state, not the handler: the MCP done socket takes
    /// this mutex with no handler behind it, and both must record against one
    /// set of since-boot counters.
    pub fn handler(state: Arc<Mutex<AppState>>) -> FrameHandler {
        let (clock, peers) = {
            let app = state.lock().unwrap();
            (Arc::clone(&app.frame_clock), app.peers_slot())
        };
        FrameHandler::new(clock, move |sender, frame, timer| {
            dispatch_frame(&state, &peers, sender, frame, timer)
        })
    }

    /// Convenience for tests: own the state and build a handler in one step.
    pub fn into_handler(self) -> FrameHandler {
        Self::handler(self.shared())
    }

    /// Synchronous dispatch used by the unit tests (no background producer). The
    /// running handler goes through [`dispatch_frame`].
    #[cfg(test)]
    pub(in crate::app) fn handle(&mut self, frame: Frame) -> Value {
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

        api::reply(id, self.dispatch_api(&method, &params))
    }

    /// Route a verb, record it if it counts as the human touching something,
    /// and run the deferred git work inline — the synchronous entry
    /// point ([`AppState::handle`] and the unit tests), which owns the state
    /// directly and has no mutex to release. Everything running is
    /// [`dispatch_frame`], which runs the same work with the lock free.
    #[cfg(test)]
    pub(in crate::app) fn dispatch(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<Value, String> {
        self.dispatch_api(method, params)
            .map_err(|error| error.message().to_string())
    }

    /// [`AppState::dispatch`] with the refusal's code kept — what
    /// [`AppState::handle`] answers from, and what the tests that assert on
    /// `error_code` read.
    #[cfg(test)]
    pub(in crate::app) fn dispatch_api(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<Value, ApiError> {
        let (outcome, deferred) = self.dispatch_deferring(method, params);
        // No `Arc` to release the mutex through — the synchronous entry point.
        // The git work runs right here, exactly as it did before the split;
        // [`dispatch_frame`] is the caller that runs it with the lock free.
        let Some(mut deferred) = deferred else {
            return outcome;
        };
        loop {
            let done = deferred.run();
            match self.apply_deferred_stage(method, params, done) {
                DeferredNext::Answered(answer) => return answer.map_err(ApiError::classify),
                DeferredNext::Again(next) => deferred = next,
            }
        }
    }

    /// v1 first, legacy second (wire spec Part 2, step 2.2). A verb
    /// [`api::v1`] registers is answered from its typed handler; everything
    /// else falls through to [`AppState::route_legacy`], whose bare
    /// `Err(String)` has no code of its own and so reads as `internal`. A verb
    /// neither answers is `unknown_method` — which is what every verb cut
    /// before release (#207) now is.
    ///
    /// [`api::v1`]: crate::api::v1
    pub(in crate::app) fn route(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Result<Value, ApiError> {
        if self.project_deletion_in_progress && !allowed_during_project_deletion(method) {
            return Err(ApiError::unavailable(PROJECT_DELETION_IN_PROGRESS));
        }
        if let Some(answered) = crate::api::v1::dispatch(self, method, params) {
            return answered;
        }
        match self.route_legacy(method, params) {
            Some(outcome) => outcome.map_err(conversation_session_refusal),
            None => Err(ApiError::unknown_method(method)),
        }
    }

    /// The verbs answered by hand rather than through [`api::v1`]: the probe,
    /// the QA stream fixtures, and the two terminal reads that need no
    /// session. `None` is "no such verb here", which [`AppState::route`]
    /// turns into `unknown_method`.
    ///
    /// Every name here is also named in `tests/api_contract.rs`'s
    /// `LEGACY_METHODS`, which is what keeps "answered outside v1" a decision
    /// rather than an oversight.
    ///
    /// [`api::v1`]: crate::api::v1
    pub(in crate::app) fn route_legacy(
        &mut self,
        method: &str,
        params: &Value,
    ) -> Option<Result<Value, String>> {
        Some(match method {
            // `push_events` rides the probe as well as the greeting: a client
            // that only ever pings can still tell whether this bridge will
            // invalidate for it, and an old client ignores the extra field.
            "ping" => Ok(pong()),
            // QA-only (`BRIDGE_QA_AGENT=1`), and unknown to everyone else: the
            // scripted stream is a test fixture, not part of `api/v1`.
            "stream.events" if self.qa_agent => self.stream_events(params),
            "stream.state" if self.qa_agent => self.stream_state(params),
            "term.list" => self.term_list(params),
            "term.close" => self.term_close(params),
            _ => return None,
        })
    }

    /// Stamp the entity a successful verb acted on, if that verb counts as an
    /// interaction. One table rather than fifteen call sites: the policy is the
    /// kind of thing that drifts when it lives next to the code it describes.
    pub(in crate::app) fn stamp_interaction_for(&mut self, method: &str, params: &Value) {
        let param = |key: &str| params.get(key).and_then(Value::as_str).map(str::to_string);
        let touched: Vec<String> = INTERACTION_VERBS
            .iter()
            .filter(|(verb, _)| *verb == method)
            .filter_map(|(_, key)| param(key))
            .collect();
        for id in touched {
            self.touch_attention(&id);
        }
        if USER_ACTIVITY_VERBS.contains(&method) {
            let now = i64::try_from(crate::agent::now_ms()).unwrap_or(i64::MAX);
            self.note_user_activity(now);
        }
    }
}
