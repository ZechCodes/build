use super::activity::{spawn_shell_tail_poller, ActivitySlot};
use super::protocol::{
    publish_status, publish_usage_limit, ProtocolState, RecordedCall, BUILD_MCP_TOOL_PREFIX,
    SURFACE_TASK_SUBTYPES,
};
use super::translation::{
    bounded_activity_text, ended_summary, result_error_text, spoken, task_description,
    task_status_is_terminal, tool_call_summary, tool_result_text, unix_millis_now, Voice,
};
use crate::harness::surfaces::SurfaceRevision;
use crate::harness::{
    publish_context, ActivityReport, AgentActivity, AgentStatus, SessionStatusSnapshot,
    ToolOutcome, TurnContext,
};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;
use tokio::sync::watch;

pub(super) struct ProtocolReader {
    pub(super) state: Arc<Mutex<ProtocolState>>,
    status_updates: watch::Sender<SessionStatusSnapshot>,
    pub(super) activity: ActivitySlot,
    pub(super) calls: HashMap<String, RecordedCall>,
    pub(super) revision: SurfaceRevision,
    pub(super) shell_poller: Arc<Mutex<Option<JoinHandle<()>>>>,
    /// The context the newest top-level request held, and the cache reads of
    /// every turn so far — published whole when a turn ends.
    context: TurnContext,
    /// The child this reader is reading, for the one thing a reader may do
    /// to it: end it when its `init` line says it is running a model Build
    /// did not ask for. `None` for a reader driven from a recording.
    child: Option<Arc<Mutex<Child>>>,
}

impl ProtocolReader {
    pub(super) fn new(
        state: Arc<Mutex<ProtocolState>>,
        activity: ActivitySlot,
        revision: SurfaceRevision,
        status_updates: watch::Sender<SessionStatusSnapshot>,
    ) -> ProtocolReader {
        ProtocolReader {
            state,
            status_updates,
            activity,
            calls: HashMap::new(),
            revision,
            shell_poller: Arc::new(Mutex::new(None)),
            context: TurnContext::default(),
            child: None,
        }
    }

    pub(super) fn ending(mut self, child: Arc<Mutex<Child>>) -> ProtocolReader {
        self.child = Some(child);
        self
    }

    pub(super) fn read_line(&mut self, line: &str) {
        self.state.lock().unwrap().last_line = Instant::now();
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            // Not protocol. A harness can print a warning to stdout before the
            // stream starts; it is evidence the child is alive (stamped above)
            // and nothing more.
            return;
        };
        match event["type"].as_str() {
            Some("system") => self.read_system(&event),
            Some("assistant") => self.read_message(&event, Voice::Assistant),
            Some("user") => self.read_message(&event, Voice::User),
            Some("result") => self.read_result(&event),
            Some("control_response") => self.read_control_response(&event),
            Some("rate_limit_event") => self.read_rate_limit_event(&event),
            _ => {}
        }
        self.publish_live_status();
    }

    fn publish_live_status(&self) {
        let state = self.state.lock().unwrap();
        publish_status(&self.status_updates, state.live_status());
    }

    pub(super) fn publish_status(&self, status: AgentStatus) {
        publish_status(&self.status_updates, status);
    }

    pub(super) fn end_stream(&self) {
        // A child that goes before its turn's result still ended that turn at
        // the limit, when the limit was what it last reported (issue #58).
        self.conclude_usage_limit();
        let changed = self
            .state
            .lock()
            .unwrap()
            .surfaces
            .mark_retained_checklist_stale();
        self.bump_revision_when(changed);
    }

    /// The turn is over: if the harness marked a message of it as a usage
    /// limit, that is why, and the session says so on its status snapshot —
    /// idle because limited, rather than idle and unexplained, which is how an
    /// agent sat on an uncommitted tree for fifty-five minutes on 2026-09-20.
    ///
    /// The reset is the `rate_limit_event`'s instant when one came, and the
    /// clock read from the sentence otherwise.
    fn conclude_usage_limit(&self) {
        let concluded = {
            let mut state = self.state.lock().unwrap();
            state.limit_said_last.take().map(|said| {
                let mut resolved = said.resolved(time::OffsetDateTime::now_utc());
                if let Some(instant) = state.rate_limit_resets_at {
                    resolved.resets_at = Some(instant);
                }
                state.usage_limited = Some(said.clone());
                (
                    state.agent_id.clone(),
                    state.session_id.clone(),
                    said,
                    resolved,
                )
            })
        };
        // Written with the lock released: `eprintln!` can block on a full pipe,
        // and nothing else should wait on this session's state behind a log line.
        if let Some((agent, session, said, resolved)) = concluded {
            eprintln!(
                "harness usage_limited: agent={agent:?} session={session:?} said={:?} reset_clock={:?} resets_at={:?}",
                said.said, said.reset_clock, resolved.resets_at
            );
            publish_usage_limit(&self.status_updates, resolved);
        }
    }

    /// A top-level assistant message's `error`, which is how the CLI marks a
    /// message it wrote itself about a failed request rather than one the model
    /// wrote. `rate_limit` is a usage limit (issue #58): the message's text is
    /// what the harness said, kept for the turn's end to conclude on. Any
    /// other assistant message means the model answered, so a limit reported
    /// earlier in the turn is not what ended it.
    ///
    /// Every error is logged with the agent it happened to, so the next one is
    /// diagnosable from bridge.log whatever it turns out to be.
    fn read_assistant_error(&mut self, event: &Value) {
        let Some(error) = event["error"].as_str() else {
            self.state.lock().unwrap().limit_said_last = None;
            return;
        };
        let said = assistant_text(&event["message"]);
        let agent = {
            let mut state = self.state.lock().unwrap();
            state.limit_said_last = (error == "rate_limit")
                .then(|| crate::harness::usage_limit::usage_limit_said(&said));
            state.agent_id.clone()
        };
        eprintln!("harness assistant_error: agent={agent:?} error={error:?} said={said:?}");
    }

    /// The CLI's account of a usage window: whether requests are being refused
    /// and when that ends. A `rejected` event's `resetsAt` is the reset
    /// instant — epoch SECONDS, as the CLI's own formatter reads it
    /// (`new Date(resetsAt * 1000)`); any other status means the window is open,
    /// and no reset is owed.
    fn read_rate_limit_event(&mut self, event: &Value) {
        let info = &event["rate_limit_info"];
        let status = info["status"].as_str();
        let resets_at = info["resetsAt"]
            .as_i64()
            .and_then(|seconds| time::OffsetDateTime::from_unix_timestamp(seconds).ok());
        let agent = {
            let mut state = self.state.lock().unwrap();
            state.rate_limit_resets_at = resets_at.filter(|_| status == Some("rejected"));
            state.agent_id.clone()
        };
        eprintln!(
            "harness rate_limit_event: agent={agent:?} status={status:?} type={:?} resets_at={resets_at:?}",
            info["rateLimitType"].as_str()
        );
    }

    /// The lifecycle line, and the background-task lines that ride the same
    /// subtype. Anything else on `system` is not this session's business.
    fn read_system(&mut self, event: &Value) {
        let Some(subtype) = event["subtype"].as_str() else {
            return;
        };
        match subtype {
            "init" => self.read_init(event),
            "hook_started" if event["hook_event"].as_str() == Some("PreCompact") => {
                self.send_report(ActivityReport::own_work(AgentActivity::Compaction {
                    completed: false,
                }));
            }
            "compact_boundary" => self.read_compact_boundary(event),
            "background_tasks_changed" => self.read_task_roster(event),
            "task_started" => self.read_task_started(event),
            "task_updated" => self.read_task_updated(event),
            "task_notification" => self.read_task_notification(event),
            _ => {}
        }
        if SURFACE_TASK_SUBTYPES.contains(&subtype) {
            self.read_surface_task_event(subtype, event);
        }
    }

    /// A compaction finished. What the context held before it is gone, so the
    /// reading is replaced by what survived — nothing, when the boundary does
    /// not say — rather than left standing over a context that shrank.
    fn read_compact_boundary(&mut self, event: &Value) {
        self.send_report(ActivityReport::own_work(AgentActivity::Compaction {
            completed: true,
        }));
        self.context.context_tokens = event["compact_metadata"]["post_tokens"]
            .as_u64()
            .unwrap_or(0);
        publish_context(&self.status_updates, self.context);
    }

    fn read_surface_task_event(&mut self, subtype: &str, event: &Value) {
        let moved = self
            .state
            .lock()
            .unwrap()
            .surfaces
            .read_task_event(subtype, event);
        self.note_surfaces_moved(moved);
    }

    fn bump_revision_when(&self, moved: bool) {
        if moved {
            self.revision.bump();
        }
    }

    fn note_surfaces_moved(&self, moved: bool) {
        self.bump_revision_when(moved);
        if moved {
            self.ensure_shell_tail_poller();
        }
    }

    pub(super) fn ensure_shell_tail_poller(&self) {
        let state = self.state.lock().unwrap();
        if state.surfaces.running_shell_outputs().is_empty() {
            return;
        }
        let mut poller = self.shell_poller.lock().unwrap();
        if poller.is_some() {
            return;
        }
        *poller = Some(spawn_shell_tail_poller(
            Arc::clone(&self.state),
            Arc::clone(&self.activity),
            self.revision.clone(),
            Arc::clone(&self.shell_poller),
        ));
    }

    /// `init` is when the child can take a turn, and it carries the session id a
    /// respawn resumes by.
    ///
    /// It also says what model the child is running, and that is checked
    /// against what Build asked for the way the codex carrier checks its
    /// opened thread: a child running something else is ended with that as
    /// its last words, because an agent that quietly ran on another model
    /// would report every turn as the model the human chose. The init line
    /// echoes the `--model` argument verbatim (probed: `claude-haiku-4-5`
    /// asked, `claude-haiku-4-5` announced, while the messages carry the
    /// dated id), so an exact comparison is the right one.
    fn read_init(&mut self, event: &Value) {
        let mismatch = {
            let mut state = self.state.lock().unwrap();
            state.announced = true;
            if let Some(id) = event["session_id"].as_str() {
                state.session_id = Some(id.to_string());
            }
            if let Some(model) = event["model"].as_str() {
                state.model = Some(model.to_string());
            }
            if let Some(announced) = event["capabilities"].as_array() {
                state.capabilities = announced
                    .iter()
                    .filter_map(|entry| entry.as_str().map(str::to_string))
                    .collect();
            }
            let mismatch = match (&state.requested_model, &state.model) {
                (Some(asked), Some(running)) if !runs_the_model_asked(asked, running) => Some(
                    format!(
                        "Build stopped this agent's Claude Code session because it opened {running}, and the agent asks for {asked}."
                    ),
                ),
                _ => None,
            };
            if let Some(reason) = &mismatch {
                state.reported_error = Some(reason.clone());
                state.start_refused = Some(reason.clone());
                // Build is ending this child over what it just said, so the
                // session is closed from here: the words it was ended over
                // are its last, and nothing it had already written clears
                // them.
                state.closed = true;
            }
            publish_status(&self.status_updates, state.live_status());
            mismatch
        };
        if let Some(reason) = &mismatch {
            // Logged, because a child ended here dies before it writes a
            // transcript: two agents asked for `opus` sat unstarted for a day
            // with nothing in bridge.log to say why (issue #72).
            let agent = self.state.lock().unwrap().agent_id.clone();
            eprintln!("harness model_mismatch: agent={agent:?} {reason}");
            if let Some(child) = &self.child {
                let _ = child.lock().unwrap().kill();
            }
        }
    }

    /// The child's own statement of what background work is live, which
    /// REPLACES the set rather than merging into it.
    ///
    /// A reconciled set cannot drift from the harness: a task Build somehow
    /// never saw start is inserted here, and a task whose end never got its own
    /// event is removed here. Both are membership transitions, so both mint.
    fn read_task_roster(&mut self, event: &Value) {
        let listed: Vec<(String, String)> = event["tasks"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|task| {
                let id = task["task_id"].as_str()?;
                Some((id.to_string(), task_description(task, id)))
            })
            .collect();
        let minted = {
            let mut state = self.state.lock().unwrap();
            let mut minted = Vec::new();
            for (id, description) in &listed {
                if !state.tasks.contains_key(id) {
                    minted.push(format!("{description} — started"));
                }
            }
            for (id, description) in &state.tasks {
                if !listed.iter().any(|(listed, _)| listed == id) {
                    minted.push(format!("{description} — finished"));
                }
            }
            state.tasks = listed.into_iter().collect();
            publish_status(&self.status_updates, state.live_status());
            minted
        };
        self.mint_task_updates(minted);
    }

    /// A task announcing itself. The minting trigger for a start, and the reason
    /// status flips to `Working` without waiting for the next roster — but only
    /// when it actually inserts, because a roster that already listed this task
    /// has said the same thing once.
    fn read_task_started(&mut self, event: &Value) {
        let Some(id) = event["task_id"].as_str() else {
            return;
        };
        let description = task_description(event, id);
        let minted = {
            let mut state = self.state.lock().unwrap();
            let minted = match state.tasks.insert(id.to_string(), description.clone()) {
                Some(_) => Vec::new(),
                None => vec![format!("{description} — started")],
            };
            publish_status(&self.status_updates, state.live_status());
            minted
        };
        self.mint_task_updates(minted);
    }

    /// A patch against one task. A terminal status ends it; anything else is
    /// progress and touches membership not at all.
    ///
    /// A progress patch mints nothing. The pinned payload carries only a status
    /// and an end time — no human-readable line of its own — so a patch that
    /// moves no membership has nothing to say that the task's own name did not
    /// already say. A `description` it does carry renames the task for the rows
    /// still to come rather than minting a row about the rename.
    fn read_task_updated(&mut self, event: &Value) {
        let Some(id) = event["task_id"].as_str() else {
            return;
        };
        let patch = &event["patch"];
        let status = patch["status"]
            .as_str()
            .or_else(|| event["status"].as_str())
            .unwrap_or_default();
        let minted = {
            let mut state = self.state.lock().unwrap();
            let minted = if !task_status_is_terminal(status) {
                if let Some(renamed) = patch["description"].as_str() {
                    if let Some(held) = state.tasks.get_mut(id) {
                        *held = renamed.to_string();
                    }
                }
                Vec::new()
            } else {
                match state.tasks.remove(id) {
                    Some(description) => vec![ended_summary(status, &description, patch)],
                    None => Vec::new(),
                }
            };
            publish_status(&self.status_updates, state.live_status());
            minted
        };
        self.mint_task_updates(minted);
    }

    /// The task saying something worth reading — and, when it carries a
    /// terminal status, the only word some tasks ever get that the work is over.
    ///
    /// A FOREGROUND Bash command is a task too, and the child closes it with a
    /// notification alone: no `task_updated`, no roster, ever (probe,
    /// 2026-08-30). A reader that took every notification for chatter would
    /// hold that task for the life of the session and report `Working` over an
    /// agent idle for hours — the inverse of the failure this step closes. So a
    /// terminal status here IS a membership removal, and mints the ending row
    /// the way the roster and the terminal patch do.
    ///
    /// Its text is minted under the task's own name while the set still holds
    /// it, and on its own after that: the child empties the roster before it
    /// delivers a background task's notification, and a name the set no longer
    /// holds is not a name to speak with — the text says which task it is
    /// either way. Text that only repeats the task's own name mints nothing,
    /// because a foreground notification's summary IS the description, and a
    /// row reading `X: X` says nothing the ending row did not.
    fn read_task_notification(&mut self, event: &Value) {
        let said = event["summary"]
            .as_str()
            .or_else(|| event["message"].as_str())
            .unwrap_or_default()
            .trim();
        let status = event["status"].as_str().unwrap_or_default();
        let ends = task_status_is_terminal(status);
        let minted = {
            let mut state = self.state.lock().unwrap();
            let held = event["task_id"].as_str().and_then(|id| match ends {
                true => state.tasks.remove(id),
                false => state.tasks.get(id).cloned(),
            });
            let mut minted = Vec::new();
            if !said.is_empty() && held.as_deref() != Some(said) {
                minted.push(match &held {
                    Some(description) => format!("{description}: {said}"),
                    None => said.to_string(),
                });
            }
            if ends {
                if let Some(description) = &held {
                    minted.push(ended_summary(status, description, event));
                }
            }
            publish_status(&self.status_updates, state.live_status());
            minted
        };
        self.mint_task_updates(minted);
    }

    /// Send one row per transition, in the order the transitions happened, each
    /// clipped the way a tool summary is: this is operational text about the
    /// work, not the agent speaking.
    fn mint_task_updates(&self, summaries: Vec<String>) {
        for summary in summaries {
            self.send_report(ActivityReport::bounded_task_update(&summary));
        }
    }

    /// The child's answer to a `control_request`. Only a request this session
    /// made counts: a response naming another request is noise, and a session
    /// that took it as its own would swallow a real crash.
    ///
    /// An interrupt's ack is recorded for the result that follows it to read.
    /// A `set_model`'s answer is the whole of what says whether the model
    /// moved: a success moves what the session reports as its active model,
    /// and an error is the session's last words, the way a codex thread that
    /// opened on the wrong model is.
    fn read_control_response(&mut self, event: &Value) {
        let response = &event["response"];
        let Some(answered) = response["request_id"]
            .as_str()
            .or_else(|| event["request_id"].as_str())
        else {
            return;
        };
        let mut state = self.state.lock().unwrap();
        if let Some(pending) = state.pending_interrupt.as_mut() {
            if pending.request_id == answered {
                pending.acked = true;
            }
        }
        if let Some(model) = state.pending_model_changes.remove(answered) {
            if response["subtype"].as_str() == Some("error") {
                let error = response["error"]
                    .as_str()
                    .unwrap_or("the child refused without saying why");
                state.reported_error = Some(format!("claude refused model {model:?}: {error}"));
            } else {
                state.model = Some(model);
            }
        }
    }

    /// The turn boundary. A result is never a completion — `done` is still the
    /// only completion contract — so this closes the turn and, when it carried
    /// an error, records the session's last words.
    ///
    /// Unless the human stopped it. An interrupted turn ends in an
    /// `error_during_execution` result, and reporting that as a crash would end
    /// the human's own stop with a crash notice quoting it. The ACK is what
    /// makes the clearing safe rather than a blanket amnesty: the child answers
    /// the control request before it emits the result, so an interrupt still
    /// unanswered here is one the child never acted on, and the failure the
    /// result reports is the turn's own.
    fn read_result(&mut self, event: &Value) {
        let failed = event["is_error"].as_bool().unwrap_or(false)
            || event["subtype"]
                .as_str()
                .is_some_and(|kind| kind != "success");
        // Logged whether or not it failed, because the whole difficulty in #58
        // was that nobody could say afterwards WHETHER a result had arrived.
        // The ids are copied out and the lock released BEFORE the write:
        // `eprintln!` can block on a full pipe, and blocking on I/O while holding
        // the protocol state would stall every reader of this session behind a
        // log line.
        let (agent, session) = {
            let state = self.state.lock().unwrap();
            (state.agent_id.clone(), state.session_id.clone())
        };
        eprintln!(
            "harness turn_result: agent={:?} session={:?} subtype={:?} is_error={:?} duration_ms={:?}",
            agent,
            session,
            event["subtype"].as_str(),
            event["is_error"].as_bool(),
            event["duration_ms"].as_u64(),
        );
        // Before the status says the turn is over, so no reader ever sees this
        // session idle without the reason: a queue drained in that gap would
        // hand the harness a turn it can only refuse.
        self.conclude_usage_limit();
        {
            let mut state = self.state.lock().unwrap();
            // Taken, acked or not, so an interrupt can never leak into the turn
            // after the one it ended.
            let stopped = state.pending_interrupt.take();
            // The turn queued behind an interrupt is running the moment this
            // result lands, so the flag is handed to it rather than cleared.
            state.turn_open = stopped.as_ref().is_some_and(|pending| pending.steered);
            // A result that succeeded clears the error a turn reported —
            // unless Build has already ended this child over one. A killed
            // child's last write is still in the pipe after the blow lands,
            // and a session explained by the last thing that went right is a
            // crash notice with no crash in it.
            if failed && !stopped.as_ref().is_some_and(|pending| pending.acked) {
                state.reported_error = Some(result_error_text(event));
            } else if !state.closed {
                state.reported_error = None;
            }
            // The context first, so the snapshot that says the turn is over
            // already says what it cost.
            self.context.cache_read_tokens +=
                usage_tokens(&event["usage"], "cache_read_input_tokens");
            publish_context(&self.status_updates, self.context);
            publish_status(&self.status_updates, state.live_status());
        }
        // Outside the lock, because emitting is the broadcast channel's
        // business and not this session's state. A turn the protocol answered
        // in full leaves nothing to close.
        self.close_open_calls();
    }

    /// One message's content blocks, minted in the order the child reported
    /// them.
    ///
    /// The voice decides what a block can be: text and thinking are the agent
    /// speaking, so they are only read off an `assistant` message — a `user`
    /// message carrying text is Build's own turn echoed back, and minting that
    /// would put the human's words in the timeline a second time as narration.
    fn read_message(&mut self, event: &Value, voice: Voice) {
        let parent_call_id = event["parent_tool_use_id"].as_str();
        if voice == Voice::Assistant {
            if parent_call_id.is_none() {
                self.remember_context(&event["message"]["usage"]);
                // A SUBAGENT's error is that subagent's own trouble to report;
                // the session's usage is the parent's.
                self.read_assistant_error(event);
            }
            if let Some(call_id) = parent_call_id {
                let moved = self
                    .state
                    .lock()
                    .unwrap()
                    .surfaces
                    .read_subagent_message(call_id, &event["message"]);
                self.bump_revision_when(moved);
            }
        }
        let Some(blocks) = event["message"]["content"].as_array() else {
            return;
        };
        for block in blocks {
            match (voice, block["type"].as_str()) {
                (Voice::Assistant, Some("thinking")) => {
                    if let Some(summary) = spoken(block["thinking"].as_str()) {
                        self.report(AgentActivity::Reasoning { summary }, parent_call_id);
                    }
                }
                (Voice::Assistant, Some("text")) => {
                    if let Some(summary) = spoken(block["text"].as_str()) {
                        self.report(AgentActivity::Narration { summary }, parent_call_id);
                    }
                }
                (Voice::Assistant, Some("tool_use")) => self.read_tool_use(block, parent_call_id),
                (Voice::User, Some("tool_result")) => {
                    self.read_tool_result(event, block, parent_call_id)
                }
                _ => {}
            }
        }
    }

    /// The context one top-level request held: everything it sent, fresh or
    /// cached. Every event of one message repeats the same usage, and the
    /// turn's last request is the one that counts, so the newest wins.
    fn remember_context(&mut self, usage: &Value) {
        if usage.is_object() {
            self.context.context_tokens = CONTEXT_USAGE_FIELDS
                .iter()
                .map(|field| usage_tokens(usage, field))
                .sum();
        }
    }

    fn read_tool_use(&mut self, block: &Value, parent_call_id: Option<&str>) {
        let tool = block["name"].as_str().unwrap_or_default().to_string();
        let call_id = block["id"].as_str().unwrap_or_default().to_string();
        if tool.starts_with(BUILD_MCP_TOOL_PREFIX) {
            self.calls.insert(call_id, RecordedCall::BuildsOwn);
            return;
        }
        let summary = tool_call_summary(&tool, &block["input"]);
        if parent_call_id.is_none() {
            let moved = self
                .state
                .lock()
                .unwrap()
                .surfaces
                .read_tool_call(&tool, block);
            self.bump_revision_when(moved);
        }
        self.calls.insert(
            call_id.clone(),
            RecordedCall::Minted {
                tool,
                parent_call_id: parent_call_id.map(str::to_string),
            },
        );
        self.report(AgentActivity::ToolUse { call_id, summary }, parent_call_id);
    }

    /// One call's answer, reported as the completion of the call it names
    /// rather than as an event of its own — the pairing this reader has always
    /// computed, carried outward instead of thrown away.
    ///
    /// The answer travels alone, without the tool's name in front of it: the row
    /// it lands on is the call, which said what tool this was when it was
    /// minted.
    fn read_tool_result(&mut self, event: &Value, block: &Value, parent_call_id: Option<&str>) {
        let call_id = block["tool_use_id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        // Taken, not read: a call is answered once, and a session that runs for
        // hours must not accumulate one entry per tool call it ever made.
        let answered = self.calls.remove(&call_id);
        let answered_text = tool_result_text(block);
        match answered {
            Some(RecordedCall::BuildsOwn) => return,
            Some(RecordedCall::Minted {
                tool,
                parent_call_id: None,
            }) => {
                let now_ms = unix_millis_now();
                let moved = self.state.lock().unwrap().surfaces.read_tool_answer(
                    &tool,
                    &call_id,
                    event,
                    &answered_text,
                    now_ms,
                );
                self.note_surfaces_moved(moved);
            }
            Some(RecordedCall::Minted { .. }) | None => {}
        }
        let outcome = match block["is_error"].as_bool().unwrap_or(false) {
            true => ToolOutcome::Error,
            false => ToolOutcome::Ok,
        };
        self.report(
            AgentActivity::ToolResult {
                call_id,
                outcome,
                summary: bounded_activity_text(&answered_text),
            },
            parent_call_id,
        );
    }

    fn close_open_calls(&mut self) {
        let mut still_open_under_a_spawned_agent = HashMap::new();
        for (call_id, recorded) in std::mem::take(&mut self.calls) {
            match recorded {
                RecordedCall::BuildsOwn => {}
                RecordedCall::Minted {
                    parent_call_id: Some(_),
                    ..
                } => {
                    still_open_under_a_spawned_agent.insert(call_id, recorded);
                }
                RecordedCall::Minted { .. } => {
                    self.report(
                        AgentActivity::ToolResult {
                            call_id,
                            outcome: ToolOutcome::Unanswered,
                            summary: String::new(),
                        },
                        None,
                    );
                }
            }
        }
        self.calls = still_open_under_a_spawned_agent;
        self.state.lock().unwrap().surfaces.close_pending_creates();
    }

    fn report(&self, activity: AgentActivity, parent_call_id: Option<&str>) {
        self.send_report(match parent_call_id {
            None => ActivityReport::own_work(activity),
            Some(spawning_call_id) => ActivityReport {
                activity,
                parent_call_id: Some(spawning_call_id.to_string()),
            },
        });
    }

    /// Every activity this session reports passes here.
    fn send_report(&self, report: ActivityReport) {
        if let Some(sender) = self.activity.lock().unwrap().as_ref() {
            let _ = sender.send(report);
        }
    }
}

/// Whether the model `init` announced is the one Build asked for.
///
/// Exact, except for the CLI's aliases: `--model opus` announces the model the
/// alias stands for today (`claude-opus-5`), not `opus`, and an agent asked for
/// the alias is running exactly what it asked for. An alias is a bare family
/// name, so it matches any model of that family and no other; a full id still
/// has to match itself. A `[1m]` context suffix names the window, not the model.
pub(super) fn runs_the_model_asked(asked: &str, running: &str) -> bool {
    let model = |id: &str| id.split('[').next().unwrap_or(id).to_string();
    let (asked, running) = (model(asked), model(running));
    if asked == running {
        return true;
    }
    let is_alias = !asked.is_empty() && asked.chars().all(|c| c.is_ascii_alphabetic());
    is_alias
        && running
            .strip_prefix("claude-")
            .and_then(|rest| rest.strip_prefix(asked.as_str()))
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
}

/// Every text block of one message, in order: what a message the CLI wrote
/// itself says, whole.
fn assistant_text(message: &Value) -> String {
    message["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|block| block["type"] == "text")
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The usage fields that together make up what one request put in context.
const CONTEXT_USAGE_FIELDS: [&str; 3] = [
    "input_tokens",
    "cache_read_input_tokens",
    "cache_creation_input_tokens",
];

fn usage_tokens(usage: &Value, field: &str) -> u64 {
    usage[field].as_u64().unwrap_or(0)
}
