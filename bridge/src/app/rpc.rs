use crate::api::{self, ApiError, API_VERSION};
use crate::app::{
    agent_attach, agent_interrupt, agent_start, rtc_close, rtc_ice, rtc_offer, session_hello,
    stream_start, term_ack, term_attach, term_create, term_input, term_resize, AppState,
    DeliveryRunner,
};
use crate::carrier::{FrameHandler, SessionSender};
use crate::orchestrator::OrchestratorError;
use crate::timing::FrameTimer;
use crate::transport;
use crate::transport::Frame;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// The verbs that count as the human acting on an entity, and the param naming
/// it. Deliberately asymmetric: opening a stage doc counts, because an issue is
/// a queue you triage by reading and reading one IS engaging with it — while a
/// worktree needs an action, since looking at a diff is not the same as doing
/// something about it. An agent's own work never appears here; if it did, the
/// rail would reorder itself while you watched.
pub(in crate::app) const INTERACTION_VERBS: &[(&str, &str)] = &[
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
    ("workspace.ensure_conversation", "workspace_id"),
    ("project.ensure_conversation", "project_id"),
    ("thread.post", "entity_id"),
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

    let result = match session_scoped(state, &sender, &method, &params, &timer) {
        Some(outcome) => outcome.map_err(|error| {
            if error == PROJECT_DELETION_IN_PROGRESS {
                ApiError::unavailable(error)
            } else {
                ApiError::from(error)
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
        "rtc.offer" => rtc_offer(state, sender, params, timer),
        "rtc.ice" => rtc_ice(state, sender.session_id(), params, timer),
        "rtc.close" => rtc_close(state, sender.session_id(), timer),
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
        Some(deferred) => {
            // THE POINT OF ALL THIS: seconds to minutes of git — a status
            // walk, a fetch, a merge, a `git worktree remove` of a
            // six-gigabyte checkout — with every other frame, every terminal
            // pump and the relay's own read loop free to make progress
            // meanwhile.
            let done = deferred.run();
            timer
                .lock(state)
                .apply_deferred(method, params, done)
                .map_err(ApiError::classify)
        }
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
/// knows nothing else about it, by its kind (`issue_id` / `run_id` /
/// `worktree_id`) where the verb is that kind's, and out of the result where
/// the call is what minted it. `project_id` is deliberately absent: a project
/// is not an entity a browser holds a detail view of.
pub(in crate::app) fn entity_ids_of(params: &Value, result: &Value) -> Vec<String> {
    const ENTITY_KEYS: [&str; 7] = [
        "id",
        "entity_id",
        "issue_id",
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
            | "changes.list"
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
            | "agent.list"
            | "stream.events"
            | "stream.state"
            | "archive.list"
            | "archived.list"
            | "board.list"
            | "capture.get"
            | "capture.list"
            | "models.list"
            | "project.list"
            | "project.diff"
            | "settings.get"
            | "fs.list"
            | "fs.read"
            | "fs.tree"
            | "git.branches"
            | "git.diff"
            | "git.log"
            | "git.refs"
            | "git.show"
            | "git.status"
            | "git.unpushed"
            | "issue.diff"
            | "issue.stage_diff"
            | "issue.doc"
            | "issue.get"
            | "issue.list"
            | "issue.stage_doc"
            | "issue.stages"
            | "plan.doc"
            | "plan.get"
            | "plan.list"
            | "plan.stage_doc"
            | "plan.stages"
            | "run.diff"
            | "run.stage_diff"
            | "run.get"
            | "branch.get"
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

/// Legacy documents remain readable, but workspaces no longer launch or
/// mutate the retired issue/planning workflow.
fn retired_planning_operation(method: &str) -> bool {
    if method.starts_with("issue.") || method.starts_with("plan.") {
        let action = method.split_once('.').map(|(_, action)| action);
        return !matches!(
            action,
            Some("get" | "list" | "doc" | "stages" | "stage_doc" | "stage_diff" | "diff")
        );
    }
    matches!(
        method,
        "run.create" | "run.stage_dispatch" | "run.stage_send_notes" | "run.set_auto_advance"
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
        let clock = Arc::clone(&state.lock().unwrap().frame_clock);
        FrameHandler::new(clock, move |sender, frame, timer| {
            dispatch_frame(&state, sender, frame, timer)
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
        match deferred {
            Some(deferred) => {
                let done = deferred.run();
                self.apply_deferred(method, params, done)
                    .map_err(ApiError::classify)
            }
            None => outcome,
        }
    }

    /// v1 first, legacy second (wire spec Part 2, step 2.2). A verb
    /// [`api::v1`] registers is answered from its typed handler; everything
    /// else falls through to [`AppState::route_legacy`], whose bare
    /// `Err(String)` has no code of its own and so reads as `internal`.
    ///
    /// The retirement guard runs before BOTH. Planning was retired upstream by
    /// keeping its verbs served and making the mutating ones refuse, so the
    /// check has to precede the facade that would otherwise run them: a
    /// retired verb answers [`crate::app::issues::ISSUES_RETIRED_ERROR`], not
    /// `unknown_method`, and its reads (`get`, `list`, `doc`, the stage and
    /// diff reads) go on through v1 untouched.
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
        if retired_planning_operation(method) {
            // `unavailable`, not `internal`: the verb is served and its
            // refusal is understood — the capability behind it is gone.
            return Err(ApiError::unavailable(
                crate::app::issues::ISSUES_RETIRED_ERROR,
            ));
        }
        if let Some(answered) = crate::api::v1::dispatch(self, method, params) {
            return answered;
        }
        match self.route_legacy(method, params) {
            Some(outcome) => outcome.map_err(ApiError::from),
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
    pub(in crate::app) fn stamp_interaction_for(
        &mut self,
        method: &str,
        params: &Value,
        result: &Value,
    ) {
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
}
