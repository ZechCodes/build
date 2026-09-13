use crate::app::{
    agent_attach, agent_interrupt, agent_start, rtc_close, rtc_ice, rtc_offer, session_hello,
    stream_start, term_ack, term_attach, term_create, term_input, term_resize, AppState,
    DeliveryRunner,
};
use crate::carrier::{FrameHandler, SessionSender};
use crate::harness::harness_for;
use crate::orchestrator::OrchestratorError;
use crate::timing::FrameTimer;
use crate::transport::Frame;
use crate::{models, transport};
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
    ("thread.post", "entity_id"),
];

/// Dispatch one decrypted request frame. `stream.start` and `term.attach` are
/// handled here because they need the shared `Arc` (background producer/pump) and
/// the `SessionSender` (to push live output to this client); everything else runs
/// under a short-held lock.
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
        "agent.interrupt" => agent_interrupt(state, &params, &timer),
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

/// Copy a canonical opaque id into the legacy parameter name consumed by the
/// compatibility implementation. If an old client already sent the legacy
/// name it remains untouched.
pub(in crate::app) fn alias_param(params: &Value, canonical: &str, legacy: &str) -> Value {
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
        "run.create"
            | "run.stage_dispatch"
            | "run.stage_fix"
            | "run.stage_send_notes"
            | "run.set_auto_advance"
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

        match self.dispatch(&method, &params) {
            Ok(result) => json!({ "id": id, "ok": true, "result": result }),
            Err(message) => json!({ "id": id, "ok": false, "error": message }),
        }
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

    pub(in crate::app) fn route(&mut self, method: &str, params: &Value) -> Result<Value, String> {
        if retired_planning_operation(method) {
            return Err(crate::app::issues::ISSUES_RETIRED_ERROR.into());
        }
        match method {
            // `push_events` rides the probe as well as the greeting: a client
            // that only ever pings can still tell whether this bridge will
            // invalidate for it, and an old client ignores the extra field.
            "ping" => Ok(json!({
                "pong": true,
                "push_events": true,
                "message_context": { "version": 1 },
            })),
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
            "fs.mkdir" => self.fs_mkdir(params),
            "fs.tree" => self.fs_tree(params),
            "fs.read" => self.fs_read(params),
            "fs.write" => self.fs_write(params),
            "project.diff" => self.project_diff(params),
            "git.log" => self.git_log(params),
            "git.show" => self.git_show(params),
            "git.status" => self.git_status(params),
            "git.unpushed" => self.git_unpushed(params),
            "git.diff" => self.git_diff(params),
            "git.stage" => self.git_stage(params),
            "git.unstage" => self.git_unstage(params),
            "git.commit" => self.git_commit(params),
            "git.fetch" => self.git_fetch(params),
            "git.pull" => self.git_pull(params),
            "git.push" => self.git_push(params),
            "git.branches" => self.git_branches(params),
            "git.refs" => self.git_refs(params),
            "git.checkout" => self.git_checkout(params),
            "git.checkout_ref" => self.git_checkout_ref(params),
            "git.branch_delete" => self.git_branch_delete(params),
            "git.stash" => self.git_stash(params),
            "git.stash_pop" => self.git_stash_pop(params),
            "git.discard" => self.git_discard(params),
            "git.merge_abort" => self.git_merge_abort(params),
            "settings.get" => Ok(self.settings_get()),
            "settings.set" => self.settings_set(params),
            "project.list" => Ok(self.defer_project_list()),
            "project.add" => self.project_add(params),
            "project.init_git" => self.project_init_git(params),
            "project.create" => self.project_create(params),
            "project.clone" => self.project_clone(params),
            "project.set_remote" => self.project_set_remote(params),
            "project.set_isolation" => self.project_set_isolation(params),
            "workspace.list" => self.workspace_list(params),
            "workspace.create" => self.workspace_create(params),
            "workspace.retry" => self.workspace_retry(params),
            "workspace.get" => self.workspace_get(params),
            "workspace.ensure_conversation" => self.workspace_ensure_conversation(params),
            "workspace.git_init_options" => self.workspace_git_init_options(params),
            "workspace.init_git" => self.workspace_init_git(params),
            "workspace.finish" => self.workspace_finish(params),
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
            "run.finish" => self.workspace_finish_legacy(params),
            // Branch surface: the work item the feed and the URLs speak, over
            // whichever of run / worktree / primary checkout stores it.
            "branch.get" => self.branch_get(params),
            "branch.dispatch" => self.branch_dispatch(params),
            "branch.finish" => self.workspace_finish_legacy(params),
            "worktree.create" => self.worktree_create(params),
            "worktree.finish" => self.workspace_finish_legacy(params),
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
