use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tokio::sync::{broadcast, watch};

use super::connection::{AppServerConnection, SharedConnection};
use super::diagnostics::SessionDiagnostics;
use super::limits::{AppServerLimits, StateLimits};
use super::observation::{CodexSurfaces, GoalReadFence};
use super::policy::{AfterResponse, ServerRequestDecision, ServerRequestPolicy};
use super::process::{
    AppServerProcess, ProcessOutcome, StderrOutcome, TerminalEventSink, TerminalSource,
    TerminalSourceEvent,
};
use super::protocol::{
    ClientNotification, ConnectionEvent, InboundNotification, InboundServerRequest,
    OperationResult, ParentThreadFilter, ParentThreadRoute, PendingOperation, RoutedServerRequest,
    ServerNotification, ThreadMetadataNotification,
};
use super::state::{CodexSessionState, SessionEffect, SessionEvent};
use super::translator::CodexActivityTranslator;
use crate::harness::surfaces::{AgentSurfaces, SurfaceRevision};
use crate::harness::{
    publish_context, ActivityReport, AgentSession, AgentStatus, Harness, HarnessError,
    SessionStatusSnapshot, Turn, TurnChoiceSupport, TurnContext, TurnReceiptSnapshot,
    TurnReceiptSupport,
};
use crate::models::ModelChoice;
use crate::pty::HarnessSpec;

mod observations;
#[cfg(test)]
use observations::next_goal_retry;
use observations::observed_at;

const ACTIVITY_BACKLOG: usize = 1024;
const STATUS_WHILE_TERMINAL_SOURCES_SETTLE: AgentStatus = AgentStatus::Working;

#[derive(Default)]
struct PendingTurnReceipts {
    next_token: u64,
    turns: VecDeque<PendingTurnReceipt>,
    observed_item_ids: HashSet<String>,
}

struct PendingTurnReceipt {
    token: u64,
    text: String,
    operation_id: Option<String>,
}

impl PendingTurnReceipts {
    fn register(&mut self, turn: &Turn) -> u64 {
        let token = self.next_token;
        self.next_token = self.next_token.wrapping_add(1);
        self.turns.push_back(PendingTurnReceipt {
            token,
            text: turn.text.clone(),
            operation_id: turn.operation_id.clone(),
        });
        token
    }

    fn cancel(&mut self, token: u64) {
        if let Some(index) = self.turns.iter().position(|turn| turn.token == token) {
            self.turns.remove(index);
        }
    }

    fn take_operation_ids(&mut self) -> Vec<String> {
        let operation_ids = self
            .turns
            .drain(..)
            .filter_map(|turn| turn.operation_id)
            .collect();
        self.observed_item_ids.clear();
        operation_ids
    }

    fn observe(&mut self, item: &super::protocol::ItemNotification, text: &str) -> Option<String> {
        self.observe_matching(item, |pending| pending == text)
    }

    /// The pending turn `item` shows Codex consumed: the first whose text
    /// `consumed` accepts, once per item.
    fn observe_matching(
        &mut self,
        item: &super::protocol::ItemNotification,
        consumed: impl Fn(&str) -> bool,
    ) -> Option<String> {
        if let Some(item_id) = item.item["id"].as_str() {
            if !self.observed_item_ids.insert(item_id.to_string()) {
                return None;
            }
        } else if item.lifecycle != super::protocol::ItemLifecycle::Started {
            return None;
        }
        let index = self.turns.iter().position(|turn| consumed(&turn.text))?;
        self.turns.remove(index)?.operation_id
    }
}

fn user_message_text(item: &Value) -> Option<String> {
    if let Some(text) = item.get("text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    let content = item.get("content")?.as_array()?;
    let text: String = content
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect();
    (!text.is_empty()).then_some(text)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoordinatorTerminalEvent {
    SourceSettled(TerminalSourceEvent),
    TerminalError(String),
}

impl CoordinatorTerminalEvent {
    fn demands_shutdown(&self) -> bool {
        match self {
            CoordinatorTerminalEvent::TerminalError(_)
            | CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::StdoutSettled {
                ..
            })
            | CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::ProcessSettled(_))
            | CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::SourceExpired {
                ..
            }) => true,
            CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::StderrSettled(
                outcome,
            )) => outcome.drainer_error.is_some(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalOutcome {
    pub exit_code: Option<i32>,
    pub epitaph: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalSnapshot {
    stdout_settled: bool,
    process: Option<ProcessOutcome>,
    stderr: Option<StderrOutcome>,
    terminal_error: Option<String>,
    expiry: Option<String>,
}

impl TerminalSnapshot {
    pub fn with_terminal_event(&self, event: CoordinatorTerminalEvent) -> TerminalSnapshot {
        let mut next = self.clone();
        match event {
            CoordinatorTerminalEvent::TerminalError(reason) => next.retain_terminal_error(reason),
            CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::StdoutSettled {
                reader_error,
            }) => {
                next.settle_stdout(reader_error);
            }
            CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::ProcessSettled(
                outcome,
            )) => {
                next.process.get_or_insert(outcome);
            }
            CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::StderrSettled(
                outcome,
            )) => {
                next.settle_stderr(outcome);
            }
            CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::SourceExpired {
                source,
                reason,
            }) => {
                if next.settle_expired_source(source) {
                    next.expiry.get_or_insert(reason);
                }
            }
        }
        next
    }

    fn settle_expired_source(&mut self, source: TerminalSource) -> bool {
        match source {
            TerminalSource::Stdout => self.settle_stdout(None),
            TerminalSource::Stderr => self.settle_stderr(StderrOutcome::default()),
        }
    }

    fn settle_stdout(&mut self, reader_error: Option<String>) -> bool {
        if self.stdout_settled {
            return false;
        }
        self.stdout_settled = true;
        if let Some(reason) = reader_error {
            self.retain_terminal_error(reason);
        }
        true
    }

    fn settle_stderr(&mut self, outcome: StderrOutcome) -> bool {
        let was_pending = self.stderr.is_none();
        self.stderr.get_or_insert(outcome);
        was_pending
    }

    pub fn outcome(&self) -> Option<TerminalOutcome> {
        let process = self.process.as_ref()?;
        let stderr = self.stderr.as_ref()?;
        if !self.stdout_settled {
            return None;
        }
        Some(TerminalOutcome {
            exit_code: process.exit_code,
            epitaph: self
                .terminal_error
                .clone()
                .or_else(|| process.monitor_error.clone())
                .or_else(|| stderr.drainer_error.clone())
                .or_else(|| stderr.retained_tail.clone())
                .or_else(|| self.expiry.clone()),
        })
    }

    fn retain_terminal_error(&mut self, reason: String) {
        self.terminal_error.get_or_insert(reason);
    }
}

pub struct CodexAppServerSession {
    core: Arc<SessionCore>,
}

struct SessionCore {
    connection: SharedConnection,
    process: Arc<AppServerProcess>,
    state: Mutex<CodexSessionState>,
    #[cfg(test)]
    state_changed: Condvar,
    translator: Mutex<CodexActivityTranslator>,
    surfaces: Mutex<CodexSurfaces>,
    surfaces_revision: SurfaceRevision,
    activity: Mutex<Option<broadcast::Sender<ActivityReport>>>,
    status: watch::Sender<SessionStatusSnapshot>,
    receipts: watch::Sender<TurnReceiptSnapshot>,
    pending_receipts: Mutex<PendingTurnReceipts>,
    terminal: Mutex<TerminalSnapshot>,
    #[cfg(test)]
    terminal_processed: Mutex<TerminalSnapshot>,
    #[cfg(test)]
    terminal_processed_changed: Condvar,
    published: Mutex<Option<TerminalOutcome>>,
    last_message: Mutex<Instant>,
    started: Instant,
    state_limits: StateLimits,
    binary: PathBuf,
    shutting_down: AtomicBool,
    reconciliation_timer: ReconciliationTimer,
    diagnostics: SessionDiagnostics,
    shutdown_origin: Mutex<Option<&'static str>>,
    last_protocol_event: Mutex<&'static str>,
}

impl CodexAppServerSession {
    pub fn spawn(
        spec: &HarnessSpec,
        root: PathBuf,
        choice: ModelChoice,
        resume_id: Option<String>,
        limits: AppServerLimits,
    ) -> Result<(CodexAppServerSession, broadcast::Receiver<ActivityReport>), HarnessError> {
        Self::spawn_with_grace_wait(spec, root, choice, resume_id, limits, std::thread::sleep)
    }

    fn spawn_with_grace_wait(
        spec: &HarnessSpec,
        root: PathBuf,
        choice: ModelChoice,
        resume_id: Option<String>,
        limits: AppServerLimits,
        wait_for_grace: impl FnOnce(Duration) + Send + 'static,
    ) -> Result<(CodexAppServerSession, broadcast::Receiver<ActivityReport>), HarnessError> {
        let binary = crate::pty::resolve_binary(spec)?;
        let (terminal_sender, terminal_events) = mpsc::channel();
        let events: TerminalEventSink = Arc::new(move |event| {
            let _ = terminal_sender.send(event);
        });
        let (process, pipes) = AppServerProcess::spawn_with_grace_wait(
            spec,
            root.clone(),
            limits.process(),
            Arc::clone(&events),
            wait_for_grace,
        )?;
        let connection = Arc::new(AppServerConnection::new(
            Box::new(pipes.stdin),
            limits.connection(),
        ));
        let (sender, receiver) = broadcast::channel(ACTIVITY_BACKLOG);
        let (status, _) = watch::channel(SessionStatusSnapshot::new(AgentStatus::Starting));
        let (receipts, _) = watch::channel(TurnReceiptSnapshot::default());
        let core = Arc::new(SessionCore {
            connection,
            process: Arc::new(process),
            state: Mutex::new(CodexSessionState::new(
                root,
                choice.model,
                choice.effort,
                resume_id,
            )),
            #[cfg(test)]
            state_changed: Condvar::new(),
            translator: Mutex::new(CodexActivityTranslator::new(limits.translator())),
            surfaces: Mutex::new(CodexSurfaces::default()),
            surfaces_revision: SurfaceRevision::default(),
            activity: Mutex::new(Some(sender)),
            status,
            receipts,
            pending_receipts: Mutex::new(PendingTurnReceipts::default()),
            terminal: Mutex::new(TerminalSnapshot::default()),
            #[cfg(test)]
            terminal_processed: Mutex::new(TerminalSnapshot::default()),
            #[cfg(test)]
            terminal_processed_changed: Condvar::new(),
            published: Mutex::new(None),
            last_message: Mutex::new(Instant::now()),
            started: Instant::now(),
            state_limits: limits.state(),
            binary,
            shutting_down: AtomicBool::new(false),
            reconciliation_timer: ReconciliationTimer::new(),
            diagnostics: SessionDiagnostics::new(),
            shutdown_origin: Mutex::new(None),
            last_protocol_event: Mutex::new("session_created"),
        });
        core.log("session_created", []);
        core.reconciliation_timer.start(Arc::downgrade(&core));
        start_terminal_pump(Arc::downgrade(&core), terminal_events);
        start_reader(Arc::downgrade(&core), pipes.stdout, events);
        core.apply_state(SessionEvent::Start)?;
        Ok((CodexAppServerSession { core }, receiver))
    }
}

impl SessionCore {
    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    fn log<'a>(&self, event: &str, fields: impl IntoIterator<Item = (&'a str, Value)>) {
        self.diagnostics.emit(event, self.elapsed(), fields);
    }

    fn remember_protocol(&self, event: &'static str) {
        *self.last_protocol_event.lock().unwrap() = event;
    }

    fn apply_state(self: &Arc<Self>, event: SessionEvent) -> Result<(), HarnessError> {
        let event_kind = session_event_kind(&event);
        if matches!(
            &event,
            SessionEvent::FailTurn(_) | SessionEvent::FailSession(_)
        ) {
            self.log(
                "policy_failure",
                [
                    ("scope", json!(event_kind)),
                    ("reason_present", json!(true)),
                ],
            );
        }
        let (require_version, should_close, reconciliation_pending) = {
            let mut state = self.state.lock().unwrap();
            let phase = state.diagnostic_phase();
            let transition = match state.transition(event, self.elapsed(), self.state_limits) {
                Ok(transition) => transition,
                Err(error) => {
                    self.log(
                        "state_failure",
                        [
                            ("error_kind", json!("state_transition")),
                            ("trigger", json!(event_kind)),
                            ("phase", json!(phase)),
                        ],
                    );
                    let error = HarnessError::Session(error.to_string());
                    drop(state);
                    self.fail(error.to_string());
                    return Err(error);
                }
            };
            let mut require_version = false;
            let mut should_close = false;
            for effect in &transition.effects {
                if matches!(effect, SessionEffect::Close) {
                    should_close = true;
                    continue;
                }
                match self.apply_effect(effect) {
                    Ok(needed) => require_version |= needed,
                    Err(error) => {
                        drop(state);
                        self.fail(error.to_string());
                        return Err(error);
                    }
                }
            }
            *state = transition.state;
            #[cfg(test)]
            self.state_changed.notify_all();
            if let Some(status) = state.live_status() {
                let previous = self.status.borrow().clone();
                if let Some(next) = previous.transition(status) {
                    self.status.send_replace(next);
                }
            }
            (
                require_version,
                should_close.then(|| state.epitaph()),
                state.reconciliation_pending(),
            )
        };
        if let Some(reason) = should_close {
            if let Some(reason) = reason {
                self.record_terminal_error(reason);
            }
            self.begin_shutdown(if event_kind == "eof" {
                "stdout_eof"
            } else {
                "state_close"
            });
        }
        if require_version {
            self.apply_state(SessionEvent::VersionEvidence(
                super::CodexAppServerHarness::probe_version(&self.binary),
            ))?;
        }
        self.reconciliation_timer
            .set(reconciliation_pending, self.state_limits.reconciliation);
        Ok(())
    }

    fn apply_effect(self: &Arc<Self>, effect: &SessionEffect) -> Result<bool, HarnessError> {
        match effect {
            SessionEffect::Request(operation) => {
                let (thread_id, turn_id) = operation_ids(operation);
                self.log(
                    if operation.method() == "turn/interrupt" {
                        "interrupt_outgoing"
                    } else {
                        "request_outgoing"
                    },
                    [
                        ("method", json!(operation.method())),
                        (
                            "origin",
                            json!(if operation.method() == "turn/interrupt" {
                                "state_generated"
                            } else {
                                "state_effect"
                            }),
                        ),
                        ("provider_thread_id", json!(thread_id)),
                        ("turn_id", json!(turn_id)),
                    ],
                );
                self.remember_protocol(operation_event(operation));
                self.connection
                    .request(operation.clone())
                    .map_err(|error| HarnessError::Session(error.to_string()))?;
            }
            SessionEffect::NotifyInitialized => self
                .connection
                .notify(ClientNotification::Initialized)
                .map_err(|error| HarnessError::Session(error.to_string()))?,
            SessionEffect::ThreadReady(thread_id) => {
                let fence = self
                    .surfaces
                    .lock()
                    .unwrap()
                    .observation
                    .open_thread(thread_id.clone(), observed_at());
                self.surfaces_revision.bump();
                self.request_goal(fence, 0);
            }
            SessionEffect::CloseTurn(turn_id) => {
                let reports = self
                    .translator
                    .lock()
                    .unwrap()
                    .close_turn(turn_id)
                    .map_err(|error| HarnessError::Session(error.to_string()))?;
                self.report_all(reports);
            }
            SessionEffect::Report(report) => self.report(report.clone()),
            SessionEffect::RequireVersionEvidence => return Ok(true),
            SessionEffect::Close => unreachable!("close effects are applied after releasing state"),
        }
        Ok(false)
    }

    fn handle_connection(self: &Arc<Self>, event: ConnectionEvent) -> Result<(), HarnessError> {
        match event {
            ConnectionEvent::Response { operation, result } => {
                if let PendingOperation::ReadGoal {
                    thread_id,
                    generation,
                    revision,
                    attempt,
                } = operation
                {
                    return self.handle_goal_response(
                        GoalReadFence {
                            thread_id,
                            generation,
                            revision,
                        },
                        attempt,
                        result,
                    );
                }
                if let PendingOperation::ReadThread { thread_id } = operation {
                    if let Ok(OperationResult::ThreadRead(result)) = result {
                        if result.thread.id == thread_id {
                            if let Some(parent) = self.expected_parent_thread() {
                                if self
                                    .surfaces
                                    .lock()
                                    .unwrap()
                                    .subagents
                                    .apply_thread_read(&result, &parent)
                                {
                                    self.surfaces_revision.bump();
                                }
                            }
                        }
                    }
                    self.surfaces
                        .lock()
                        .unwrap()
                        .subagents
                        .hydration_finished(&thread_id, false);
                    self.request_next_subagent_hydration();
                    return Ok(());
                }
                self.log(
                    "response_received",
                    [
                        ("method", json!(operation.method())),
                        ("success", json!(result.is_ok())),
                        (
                            "error_code",
                            json!(result.as_ref().err().map(|error| error.code)),
                        ),
                    ],
                );
                self.remember_protocol("response_received");
                self.accept_parent_message();
                self.apply_state(SessionEvent::Connection(ConnectionEvent::Response {
                    operation,
                    result,
                }))
            }
            ConnectionEvent::Notification(notification) => self.handle_notification(notification),
            ConnectionEvent::Request(request) => self.handle_server_request(request),
        }
    }

    fn handle_notification(
        self: &Arc<Self>,
        inbound: InboundNotification,
    ) -> Result<(), HarnessError> {
        if matches!(
            inbound.method.as_str(),
            "thread/goal/updated" | "thread/goal/cleared" | "turn/plan/updated"
        ) {
            return self.handle_observation_notification(inbound);
        }
        let expected_parent = self.expected_parent_thread();
        if ParentThreadFilter::notification(
            &inbound.method,
            &inbound.params,
            expected_parent.as_deref(),
        ) == ParentThreadRoute::Child
        {
            self.observe_child_thread_metadata(&inbound, expected_parent.as_deref());
            return Ok(());
        }
        self.accept_parent_message();
        let notification = ServerNotification::decode(&inbound.method, inbound.params)
            .map_err(HarnessError::Session)?;
        self.dispatch_notification(notification)
    }

    fn observe_child_thread_metadata(
        &self,
        inbound: &InboundNotification,
        expected_parent: Option<&str>,
    ) {
        let Some(expected_parent) = expected_parent else {
            return;
        };
        let Ok(Some(metadata)) =
            ThreadMetadataNotification::decode(&inbound.method, &inbound.params)
        else {
            return;
        };
        if self
            .surfaces
            .lock()
            .unwrap()
            .subagents
            .apply_thread_metadata(&metadata, expected_parent)
        {
            self.surfaces_revision.bump();
        }
    }

    fn handle_server_request(
        self: &Arc<Self>,
        inbound: InboundServerRequest,
    ) -> Result<(), HarnessError> {
        let expected_parent = self.expected_parent_thread();
        let routed = RoutedServerRequest::decode(&inbound, expected_parent.as_deref())
            .map_err(HarnessError::Session)?;
        let decision =
            ServerRequestPolicy::decide(routed.request, routed.route, unix_seconds_now());
        let decision = write_server_response(&self.connection, decision)?;
        if routed.route == ParentThreadRoute::Child {
            return Ok(());
        }
        self.accept_parent_message();
        if let Some(report) = decision.report {
            self.report(report);
        }
        match decision.after_response {
            AfterResponse::Continue => Ok(()),
            AfterResponse::FailTurn(reason) => self.apply_state(SessionEvent::FailTurn(reason)),
            AfterResponse::FailSession(reason) => {
                self.apply_state(SessionEvent::FailSession(reason))
            }
        }
    }

    fn dispatch_notification(
        self: &Arc<Self>,
        notification: ServerNotification,
    ) -> Result<(), HarnessError> {
        match &notification {
            ServerNotification::ThreadStarted { thread_id, .. } => {
                self.log(
                    "thread_correlated",
                    [("provider_thread_id", json!(thread_id))],
                );
                self.remember_protocol("thread_started");
                self.apply_state(SessionEvent::ThreadStarted(thread_id.clone()))
            }
            ServerNotification::TurnStarted { turn_id, .. } => {
                if self
                    .surfaces
                    .lock()
                    .unwrap()
                    .observation
                    .observe_turn(turn_id)
                {
                    self.surfaces_revision.bump();
                }
                self.log("turn_started", [("turn_id", json!(turn_id))]);
                self.remember_protocol("turn_started");
                self.apply_state(SessionEvent::TurnStarted(turn_id.clone()))
            }
            ServerNotification::TurnCompleted {
                thread_id,
                completion,
            } => {
                self.log(
                    "turn_completed",
                    [
                        ("turn_id", json!(completion.turn_id)),
                        ("provider_thread_id", json!(thread_id)),
                        ("status", json!(completion.status)),
                        ("error_present", json!(completion.error.is_some())),
                    ],
                );
                self.remember_protocol("turn_completed");
                self.apply_state(SessionEvent::ObservedCompletion(completion.clone()))
            }
            ServerNotification::Item(item) => {
                if self
                    .surfaces
                    .lock()
                    .unwrap()
                    .observation
                    .observe_turn(&item.turn_id)
                {
                    self.surfaces_revision.bump();
                }
                self.apply_state(SessionEvent::TurnStarted(item.turn_id.clone()))?;
                self.observe_user_message(item);
                self.observe_compaction(item);
                self.translate(&notification)
            }
            ServerNotification::Error(error) => {
                self.log(
                    "error_notification",
                    [
                        ("provider_thread_id", json!(error.thread_id)),
                        ("turn_id", json!(error.turn_id)),
                        ("will_retry", json!(error.will_retry)),
                        ("error_present", json!(!error.error.message.is_empty())),
                    ],
                );
                self.remember_protocol("error_notification");
                self.apply_state(SessionEvent::ObservedError(error.clone()))
            }
            ServerNotification::TokenUsage(context) => {
                publish_context(&self.status, *context);
                Ok(())
            }
            ServerNotification::ContextCompacted => {
                self.forget_compacted_context();
                Ok(())
            }
            ServerNotification::Delta => Ok(()),
            ServerNotification::Unknown => self.translate(&notification),
        }
    }

    fn observe_user_message(&self, item: &super::protocol::ItemNotification) {
        if item.item["type"].as_str() != Some("userMessage") {
            return;
        }
        let Some(text) = user_message_text(&item.item) else {
            return;
        };
        let operation_id = self.pending_receipts.lock().unwrap().observe(item, &text);
        self.mark_seen(operation_id);
    }

    /// A `contextCompaction` item is how a `/compact` shows it ran — no user
    /// message echoes one — and whatever the context held before is gone.
    fn observe_compaction(&self, item: &super::protocol::ItemNotification) {
        if item.item["type"].as_str() != Some("contextCompaction")
            || item.lifecycle != super::protocol::ItemLifecycle::Completed
        {
            return;
        }
        self.forget_compacted_context();
        let operation_id = self
            .pending_receipts
            .lock()
            .unwrap()
            .observe_matching(item, |pending| {
                super::CodexAppServerHarness.starts_compaction(pending)
            });
        self.mark_seen(operation_id);
    }

    /// Keep the thread's cache reads, but none of the context a compaction
    /// just replaced: a reading from before it would overstate what is left.
    fn forget_compacted_context(&self) {
        let cache_read_tokens = self
            .status
            .borrow()
            .context
            .map_or(0, |context| context.cache_read_tokens);
        publish_context(
            &self.status,
            TurnContext {
                context_tokens: 0,
                cache_read_tokens,
            },
        );
    }

    fn mark_seen(&self, operation_id: Option<String>) {
        let Some(operation_id) = operation_id else {
            return;
        };
        self.receipts.send_modify(|snapshot| {
            if !snapshot.seen_operation_ids.contains(&operation_id) {
                snapshot.seen_operation_ids.push(operation_id.clone());
            }
        });
    }

    fn translate(&self, notification: &ServerNotification) -> Result<(), HarnessError> {
        let changed = { self.surfaces.lock().unwrap().subagents.apply(notification) };
        if changed {
            self.surfaces_revision.bump();
        }
        self.request_next_subagent_hydration();
        let reports = self
            .translator
            .lock()
            .unwrap()
            .translate_notification(notification)
            .map_err(|error| HarnessError::Session(error.to_string()))?;
        self.report_all(reports);
        Ok(())
    }

    fn request_next_subagent_hydration(&self) {
        let thread_id = self
            .surfaces
            .lock()
            .unwrap()
            .subagents
            .hydration_candidate();
        let Some(thread_id) = thread_id else { return };
        // This is descriptive enrichment. A refused read must not fail the
        // parent turn or hide the lifecycle data already observed.
        if self
            .connection
            .request(PendingOperation::ReadThread {
                thread_id: thread_id.clone(),
            })
            .is_err()
        {
            self.surfaces
                .lock()
                .unwrap()
                .subagents
                .hydration_finished(&thread_id, true);
        }
    }

    fn expected_parent_thread(&self) -> Option<String> {
        self.state.lock().unwrap().expected_parent_thread()
    }

    fn accept_parent_message(&self) {
        *self.last_message.lock().unwrap() = Instant::now();
    }

    fn report(&self, report: ActivityReport) {
        if let Some(sender) = self.activity.lock().unwrap().as_ref() {
            let _ = sender.send(report);
        }
    }

    fn report_all(&self, reports: Vec<ActivityReport>) {
        for report in reports {
            self.report(report);
        }
    }

    fn close_activity(&self) {
        self.activity.lock().unwrap().take();
    }

    fn apply_terminal(&self, event: CoordinatorTerminalEvent) {
        let demands_shutdown = event.demands_shutdown();
        self.log_terminal_event(&event);
        {
            let mut snapshot = self.terminal.lock().unwrap();
            *snapshot = snapshot.with_terminal_event(event.clone());
        }
        if demands_shutdown {
            self.begin_shutdown(terminal_shutdown_origin(&event));
        }
        let outcome = self.terminal.lock().unwrap().outcome();
        if let Some(outcome) = outcome {
            self.publish_terminal(outcome);
        }
        #[cfg(test)]
        {
            *self.terminal_processed.lock().unwrap() = self.terminal.lock().unwrap().clone();
            self.terminal_processed_changed.notify_all();
        }
    }

    fn publish_terminal(&self, outcome: TerminalOutcome) {
        let mut published = self.published.lock().unwrap();
        if published.is_some() {
            return;
        }
        let ended = AgentStatus::Ended {
            code: outcome.exit_code,
        };
        *published = Some(outcome.clone());
        drop(published);
        let last = *self.last_protocol_event.lock().unwrap();
        let origin = *self.shutdown_origin.lock().unwrap();
        self.log(
            "terminal_settled",
            [
                ("exit_code", json!(outcome.exit_code)),
                ("epitaph_present", json!(outcome.epitaph.is_some())),
                ("shutdown_origin", json!(origin)),
                ("last_protocol_event", json!(last)),
            ],
        );
        let previous = self.status.borrow().clone();
        if let Some(next) = previous.transition(ended) {
            self.status.send_replace(next);
        }
        if self.surfaces.lock().unwrap().subagents.settle_running() {
            self.surfaces_revision.bump();
        }
        if self
            .surfaces
            .lock()
            .unwrap()
            .observation
            .mark_terminal_stale()
        {
            self.surfaces_revision.bump();
        }
        self.report_all(self.translator.lock().unwrap().close_all());
        let uncertain = self.pending_receipts.lock().unwrap().take_operation_ids();
        if !uncertain.is_empty() {
            self.receipts.send_modify(|snapshot| {
                for operation_id in &uncertain {
                    if !snapshot.uncertain_operation_ids.contains(operation_id) {
                        snapshot.uncertain_operation_ids.push(operation_id.clone());
                    }
                }
            });
        }
        self.close_activity();
    }

    fn record_terminal_error(&self, reason: String) {
        self.apply_terminal(CoordinatorTerminalEvent::TerminalError(reason));
    }

    fn fail(&self, reason: String) {
        self.record_terminal_error(reason);
    }

    fn begin_shutdown(&self, origin: &'static str) {
        let mut retained_origin = self.shutdown_origin.lock().unwrap();
        let first = !self.shutting_down.swap(true, Ordering::AcqRel);
        if first {
            *retained_origin = Some(origin);
        }
        drop(retained_origin);
        if first {
            let last = *self.last_protocol_event.lock().unwrap();
            self.log(
                "shutdown_trigger",
                [
                    ("origin", json!(origin)),
                    ("last_protocol_event", json!(last)),
                ],
            );
        } else {
            self.log("shutdown_consequence", [("origin", json!(origin))]);
            return;
        }
        self.reconciliation_timer.stop();
        if let Err(error) = self.connection.close() {
            self.record_terminal_error(error.to_string());
        }
        if let Err(error) = self.process.shutdown() {
            self.record_terminal_error(error.to_string());
        }
    }

    fn log_terminal_event(&self, event: &CoordinatorTerminalEvent) {
        let (source, error_present, exit_code) = match event {
            CoordinatorTerminalEvent::TerminalError(_) => ("terminal_error", true, None),
            CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::StdoutSettled {
                reader_error,
            }) => (
                if reader_error.is_some() {
                    "stdout_reader_failure"
                } else {
                    "stdout_eof"
                },
                reader_error.is_some(),
                None,
            ),
            CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::ProcessSettled(
                outcome,
            )) => (
                "process_exit",
                outcome.monitor_error.is_some(),
                outcome.exit_code,
            ),
            CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::StderrSettled(
                outcome,
            )) => ("stderr_settled", outcome.drainer_error.is_some(), None),
            CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::SourceExpired {
                source,
                ..
            }) => (
                match source {
                    TerminalSource::Stdout => "stdout_expired",
                    TerminalSource::Stderr => "stderr_expired",
                },
                true,
                None,
            ),
        };
        self.log(
            source,
            [
                ("error_present", json!(error_present)),
                ("exit_code", json!(exit_code)),
            ],
        );
    }
}

impl AgentSession for CodexAppServerSession {
    fn send_turn(&self, turn: &Turn) -> Result<(), HarnessError> {
        self.core.log(
            "turn_submitted",
            [("choice_present", json!(turn.choice.is_some()))],
        );
        if let Some(frozen) = &turn.choice {
            frozen
                .model_choice
                .validate()
                .map_err(HarnessError::Unsupported)?;
            if self.turn_choice_support(&frozen.model_choice) == TurnChoiceSupport::RestartRequired
            {
                return Err(HarnessError::Unsupported(
                    "this Codex session cannot clear its sticky model settings; start a fresh session with the requested choice"
                        .to_string(),
                ));
            }
        }
        let receipt_token = self.core.pending_receipts.lock().unwrap().register(turn);
        let event = if turn.choice.is_some() {
            SessionEvent::SendChosenTurn(turn.clone())
        } else {
            SessionEvent::SendTurn(turn.text.clone())
        };
        let result = self.core.apply_state(event);
        if result.is_err() {
            self.core
                .pending_receipts
                .lock()
                .unwrap()
                .cancel(receipt_token);
        }
        result
    }

    fn accepts_turn_choice(&self) -> bool {
        true
    }

    fn turn_choice_support(&self, choice: &ModelChoice) -> TurnChoiceSupport {
        self.core.state.lock().unwrap().turn_choice_support(choice)
    }

    fn status(&self) -> AgentStatus {
        if let Some(outcome) = self.core.published.lock().unwrap().as_ref() {
            return AgentStatus::Ended {
                code: outcome.exit_code,
            };
        }
        self.core
            .state
            .lock()
            .unwrap()
            .live_status()
            .unwrap_or(STATUS_WHILE_TERMINAL_SOURCES_SETTLE)
    }

    fn status_changed(&self) -> Option<watch::Receiver<SessionStatusSnapshot>> {
        Some(self.core.status.subscribe())
    }

    fn turn_receipt_support(&self) -> TurnReceiptSupport {
        TurnReceiptSupport::Seen
    }

    fn turn_receipts(&self) -> Option<watch::Receiver<TurnReceiptSnapshot>> {
        Some(self.core.receipts.subscribe())
    }

    fn quiet_for(&self) -> Duration {
        Instant::now().saturating_duration_since(*self.core.last_message.lock().unwrap())
    }

    fn exited_within(&self, timeout: Duration) -> bool {
        self.core.process.exited_within(timeout)
    }

    fn end(&self) {
        self.core.begin_shutdown("explicit_end");
    }

    fn epitaph(&self) -> Option<String> {
        self.core
            .published
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|outcome| outcome.epitaph.clone())
    }

    fn activity(&self) -> Option<broadcast::Receiver<ActivityReport>> {
        Some(match self.core.activity.lock().unwrap().as_ref() {
            Some(sender) => sender.subscribe(),
            None => {
                let (sender, receiver) = broadcast::channel(1);
                drop(sender);
                receiver
            }
        })
    }

    fn can_interrupt(&self) -> bool {
        self.core.state.lock().unwrap().can_interrupt()
    }

    fn interrupt(&self) -> Result<(), HarnessError> {
        self.core
            .log("interrupt_requested", [("origin", json!("explicit"))]);
        self.core.apply_state(SessionEvent::Interrupt)
    }

    fn session_id(&self) -> Option<String> {
        self.core.state.lock().unwrap().session_id()
    }

    fn active_model(&self) -> Option<String> {
        self.core.state.lock().unwrap().active_model()
    }

    fn active_effort(&self) -> Option<String> {
        self.core.state.lock().unwrap().active_effort()
    }

    fn active_choice(&self) -> (Option<String>, Option<String>) {
        self.core.state.lock().unwrap().active_choice()
    }

    fn surfaces(&self) -> Option<AgentSurfaces> {
        self.core.surfaces.lock().unwrap().snapshot()
    }

    fn surfaces_changed(&self) -> Option<watch::Receiver<u64>> {
        Some(self.core.surfaces_revision.subscribe())
    }

    #[cfg(test)]
    fn backdate_last_output(&self, ago: Duration) {
        *self.core.last_message.lock().unwrap() = Instant::now()
            .checked_sub(ago)
            .expect("a stamp old enough to age");
    }
}

impl Drop for CodexAppServerSession {
    fn drop(&mut self) {
        self.core.begin_shutdown("session_drop");
    }
}

fn start_terminal_pump(core: Weak<SessionCore>, events: mpsc::Receiver<TerminalSourceEvent>) {
    std::thread::spawn(move || {
        for event in events {
            let Some(core) = core.upgrade() else {
                return;
            };
            core.apply_terminal(CoordinatorTerminalEvent::SourceSettled(event));
        }
    });
}

fn start_reader(
    core: Weak<SessionCore>,
    mut stdout: std::process::ChildStdout,
    events: TerminalEventSink,
) {
    std::thread::spawn(move || {
        let reader_error = read_until_settled(&core, &mut stdout);
        events(TerminalSourceEvent::StdoutSettled { reader_error });
    });
}

fn read_until_settled(
    core: &Weak<SessionCore>,
    stdout: &mut std::process::ChildStdout,
) -> Option<String> {
    loop {
        let core = core.upgrade()?;
        let event = match core.connection.read_event(stdout) {
            Ok(Some(event)) => event,
            Ok(None) => {
                let _ = core.apply_state(SessionEvent::Eof);
                return None;
            }
            Err(error) => {
                let mut fields = vec![("error_kind", json!(connection_error_kind(&error)))];
                if let super::connection::ConnectionError::FrameTooLarge {
                    limit,
                    observed_at_least,
                } = &error
                {
                    fields.push(("limit_bytes", json!(limit)));
                    fields.push(("observed_at_least_bytes", json!(observed_at_least)));
                }
                core.log("protocol_failure", fields);
                return Some(error.to_string());
            }
        };
        if let Err(error) = core.handle_connection(event) {
            core.log(
                "protocol_failure",
                [("error_kind", json!("event_handling"))],
            );
            core.fail(error.to_string());
            return None;
        }
    }
}

fn unix_seconds_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs() as i64
}

fn operation_ids(operation: &PendingOperation) -> (Option<&str>, Option<&str>) {
    match operation {
        PendingOperation::StartTurn { thread_id, .. } => (Some(thread_id), None),
        PendingOperation::SteerTurn {
            thread_id, turn_id, ..
        }
        | PendingOperation::InterruptTurn { thread_id, turn_id } => {
            (Some(thread_id), Some(turn_id))
        }
        PendingOperation::ResumeThread { thread_id, .. } => (Some(thread_id), None),
        PendingOperation::ReadThread { thread_id }
        | PendingOperation::ReadGoal { thread_id, .. }
        | PendingOperation::CompactThread { thread_id } => (Some(thread_id), None),
        PendingOperation::Initialize | PendingOperation::StartThread { .. } => (None, None),
    }
}

fn session_event_kind(event: &SessionEvent) -> &'static str {
    match event {
        SessionEvent::Start => "start",
        SessionEvent::SendTurn(_) | SessionEvent::SendChosenTurn(_) => "send_turn",
        SessionEvent::Interrupt => "interrupt",
        SessionEvent::Connection(_) => "connection",
        SessionEvent::ThreadStarted(_) => "thread_started",
        SessionEvent::TurnStarted(_) => "turn_started",
        SessionEvent::ObservedCompletion(_) => "turn_completed",
        SessionEvent::ObservedError(_) => "error_notification",
        SessionEvent::VersionEvidence(_) => "version_evidence",
        SessionEvent::CheckTimeouts => "check_timeouts",
        SessionEvent::FailTurn(_) => "fail_turn",
        SessionEvent::FailSession(_) => "fail_session",
        SessionEvent::Eof => "eof",
    }
}

fn operation_event(operation: &PendingOperation) -> &'static str {
    match operation {
        PendingOperation::Initialize => "initialize_sent",
        PendingOperation::StartThread { .. } => "thread_start_sent",
        PendingOperation::ResumeThread { .. } => "thread_resume_sent",
        PendingOperation::StartTurn { .. } => "turn_start_sent",
        PendingOperation::SteerTurn { .. } => "turn_steer_sent",
        PendingOperation::InterruptTurn { .. } => "turn_interrupt_sent",
        PendingOperation::ReadThread { .. } => "thread_read_sent",
        PendingOperation::ReadGoal { .. } => "goal_read_sent",
        PendingOperation::CompactThread { .. } => "thread_compact_sent",
    }
}

fn terminal_shutdown_origin(event: &CoordinatorTerminalEvent) -> &'static str {
    match event {
        CoordinatorTerminalEvent::TerminalError(_) => "terminal_error",
        CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::StdoutSettled {
            reader_error: Some(_),
        }) => "stdout_reader_failure",
        CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::StdoutSettled {
            reader_error: None,
        }) => "stdout_eof",
        CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::ProcessSettled(_)) => {
            "process_exit"
        }
        CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::SourceExpired { .. }) => {
            "source_expiry"
        }
        CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::StderrSettled(_)) => {
            "stderr_reader_failure"
        }
    }
}

fn connection_error_kind(error: &super::connection::ConnectionError) -> &'static str {
    use super::connection::ConnectionError;
    match error {
        ConnectionError::Closed => "closed",
        ConnectionError::RequestIdExhausted => "request_id_exhausted",
        ConnectionError::PendingLimit(_) => "pending_limit",
        ConnectionError::FrameTooLarge { .. } => "frame_too_large",
        ConnectionError::UnterminatedFrame => "unterminated_frame",
        ConnectionError::InvalidUtf8(_) => "invalid_utf8",
        ConnectionError::InvalidJson(_) => "invalid_json",
        ConnectionError::Protocol(_) => "protocol",
        ConnectionError::Io(_) => "io",
    }
}

fn write_server_response(
    connection: &AppServerConnection,
    decision: ServerRequestDecision,
) -> Result<ServerRequestDecision, HarnessError> {
    connection
        .respond(decision.response.clone())
        .map_err(|error| HarnessError::Session(error.to_string()))?;
    Ok(decision)
}

struct ReconciliationTimer {
    shared: Arc<(Mutex<TimerState>, Condvar)>,
}

struct TimerState {
    generation: u64,
    deadline: Option<Instant>,
    stopped: bool,
}

impl ReconciliationTimer {
    fn new() -> ReconciliationTimer {
        ReconciliationTimer {
            shared: Arc::new((
                Mutex::new(TimerState {
                    generation: 0,
                    deadline: None,
                    stopped: false,
                }),
                Condvar::new(),
            )),
        }
    }

    fn start(&self, core: Weak<SessionCore>) {
        let shared = Arc::clone(&self.shared);
        std::thread::spawn(move || reconciliation_timer_loop(shared, core));
    }

    fn set(&self, pending: bool, delay: Duration) {
        let (state, wake) = &*self.shared;
        let mut state = state.lock().unwrap();
        if state.stopped {
            return;
        }
        if pending == state.deadline.is_some() {
            return;
        }
        state.generation = state.generation.wrapping_add(1);
        state.deadline = pending.then(|| Instant::now() + delay);
        wake.notify_one();
    }

    fn stop(&self) {
        let (state, wake) = &*self.shared;
        let mut state = state.lock().unwrap();
        state.stopped = true;
        state.deadline = None;
        wake.notify_one();
    }

    #[cfg(test)]
    fn generation(&self) -> u64 {
        self.shared.0.lock().unwrap().generation
    }
}

fn reconciliation_timer_loop(shared: Arc<(Mutex<TimerState>, Condvar)>, core: Weak<SessionCore>) {
    let (state, wake) = &*shared;
    let mut state = state.lock().unwrap();
    loop {
        if state.stopped {
            return;
        }
        let Some(deadline) = state.deadline else {
            state = wake.wait(state).unwrap();
            continue;
        };
        let generation = state.generation;
        let delay = deadline.saturating_duration_since(Instant::now());
        let (next_state, timeout) = wake.wait_timeout(state, delay).unwrap();
        state = next_state;
        if !timeout.timed_out() || state.generation != generation || state.deadline.is_none() {
            continue;
        }
        state.deadline = None;
        drop(state);
        let Some(core) = core.upgrade() else {
            return;
        };
        if let Err(error) = core.apply_state(SessionEvent::CheckTimeouts) {
            core.fail(error.to_string());
            return;
        }
        state = shared.0.lock().unwrap();
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::harness::codex_app_server::fixtures::{
        initialize_result, selected_choice, supported_user_agent, thread_opened_at,
        CHILD_THREAD_ID, CHILD_TURN_ID, SELECTED_EFFORT, THREAD_ID, TURN_ID,
    };
    use crate::harness::codex_app_server::policy::AfterResponse;
    use crate::harness::codex_app_server::protocol::{
        ItemLifecycle, ItemNotification, RpcError, ServerResponse, ThreadReadResult, ThreadSummary,
    };
    use crate::harness::AgentSession;
    use crate::pty::HarnessSpec;

    fn user_message(id: &str, text: &str, lifecycle: ItemLifecycle) -> ItemNotification {
        ItemNotification {
            lifecycle,
            thread_id: THREAD_ID.to_string(),
            turn_id: TURN_ID.to_string(),
            item: json!({
                "id": id,
                "type": "userMessage",
                "content": [{"type": "text", "text": text}]
            }),
        }
    }

    #[test]
    fn goal_recovery_attempts_are_bounded_per_episode() {
        assert_eq!(next_goal_retry(0), Some(1));
        assert_eq!(next_goal_retry(1), Some(2));
        assert_eq!(next_goal_retry(2), None);
        assert_eq!(next_goal_retry(u8::MAX), None);
    }

    #[test]
    fn native_receipts_wait_for_a_matching_user_message_echo() {
        let mut receipts = PendingTurnReceipts::default();
        receipts.register(&Turn::new("start input").with_operation_id("start-operation"));

        assert_eq!(
            receipts.observe(
                &user_message("echo-start", "different input", ItemLifecycle::Started),
                "different input"
            ),
            None
        );
        assert_eq!(
            receipts.observe(
                &user_message("echo-start", "start input", ItemLifecycle::Started),
                "start input"
            ),
            None,
            "an already observed item id cannot be reused to manufacture a receipt"
        );
        assert_eq!(
            receipts.observe(
                &user_message("matching-echo", "start input", ItemLifecycle::Started),
                "start input"
            ),
            Some("start-operation".to_string())
        );
    }

    #[test]
    fn queued_and_steered_inputs_stay_correlated_and_item_duplicates_are_inert() {
        let mut receipts = PendingTurnReceipts::default();
        receipts.register(&Turn::new("first").with_operation_id("operation-first"));
        receipts.register(&Turn::new("steer").with_operation_id("operation-steer"));
        receipts.register(&Turn::new("queued").with_operation_id("operation-queued"));

        let first = user_message("echo-first", "first", ItemLifecycle::Started);
        assert_eq!(
            receipts.observe(&first, "first").as_deref(),
            Some("operation-first")
        );
        let duplicate = user_message("echo-first", "first", ItemLifecycle::Completed);
        assert_eq!(receipts.observe(&duplicate, "first"), None);
        assert_eq!(
            receipts
                .observe(
                    &user_message("echo-steer", "steer", ItemLifecycle::Completed),
                    "steer"
                )
                .as_deref(),
            Some("operation-steer")
        );
        assert_eq!(
            receipts
                .observe(
                    &user_message("echo-queued", "queued", ItemLifecycle::Started),
                    "queued"
                )
                .as_deref(),
            Some("operation-queued")
        );
    }

    #[test]
    fn turn_start_response_does_not_publish_seen_without_the_user_message_item() {
        let root = tempfile::tempdir().unwrap();
        let turn_response = json!({"id":4,"result":{"turn":{"id":TURN_ID}}});
        let script = opened_thread_script(
            root.path(),
            &format!(
                "read turn; printf '%s\\n' '{}'; cat >/dev/null",
                turn_response
            ),
        );
        let (session, _activity) = scripted_session(root.path(), &script);
        let state = session.core.state.lock().unwrap();
        let (state, _) = session
            .core
            .state_changed
            .wait_timeout_while(state, Duration::from_secs(2), |state| {
                state.session_id().is_none()
            })
            .unwrap();
        assert!(
            state.session_id().is_some(),
            "the session never opened its thread"
        );
        drop(state);
        let receipts = session.turn_receipts().unwrap();
        assert_eq!(session.turn_receipt_support(), TurnReceiptSupport::Seen);
        session
            .send_turn(&Turn::new("go").with_operation_id("operation-go"))
            .unwrap();
        let state = session.core.state.lock().unwrap();
        let (state, _) = session
            .core
            .state_changed
            .wait_timeout_while(state, Duration::from_secs(2), |state| {
                state.diagnostic_phase() != "working"
            })
            .unwrap();
        assert_eq!(state.diagnostic_phase(), "working");
        drop(state);
        assert!(receipts.borrow().seen_operation_ids.is_empty());

        session
            .core
            .dispatch_notification(ServerNotification::Item(user_message(
                "echo-go",
                "go",
                ItemLifecycle::Started,
            )))
            .unwrap();
        assert_eq!(
            receipts.borrow().seen_operation_ids,
            vec!["operation-go".to_string()]
        );

        session
            .send_turn(&Turn::new("steer").with_operation_id("operation-steer"))
            .unwrap();
        assert_eq!(receipts.borrow().seen_operation_ids.len(), 1);
        session
            .core
            .handle_connection(ConnectionEvent::Response {
                operation: PendingOperation::SteerTurn {
                    thread_id: THREAD_ID.to_string(),
                    turn_id: TURN_ID.to_string(),
                    input: "steer".to_string(),
                },
                result: Ok(OperationResult::TurnSteered(
                    super::super::protocol::TurnSteerResult {
                        turn_id: TURN_ID.to_string(),
                    },
                )),
            })
            .unwrap();
        assert_eq!(
            receipts.borrow().seen_operation_ids.len(),
            1,
            "a successful steer response is not model-consumption evidence"
        );
        session
            .core
            .dispatch_notification(ServerNotification::Item(user_message(
                "echo-steer",
                "steer",
                ItemLifecycle::Started,
            )))
            .unwrap();
        assert_eq!(
            receipts.borrow().seen_operation_ids,
            vec!["operation-go".to_string(), "operation-steer".to_string()]
        );
        session.end();
    }

    #[test]
    fn accepted_turn_becomes_uncertain_when_the_session_dies_before_its_echo() {
        let root = tempfile::tempdir().unwrap();
        let script = opened_thread_script(root.path(), "read turn");
        let (session, _activity) = scripted_session(root.path(), &script);
        wait_until("opened its thread", || session.session_id().is_some());
        let receipts = session.turn_receipts().unwrap();
        session
            .send_turn(&Turn::new("unseen").with_operation_id("operation-unseen"))
            .unwrap();

        wait_until("published terminal uncertainty", || {
            receipts
                .borrow()
                .uncertain_operation_ids
                .iter()
                .any(|id| id == "operation-unseen")
        });
        let snapshot = receipts.borrow().clone();
        assert!(snapshot.seen_operation_ids.is_empty());
        assert_eq!(
            snapshot.uncertain_operation_ids,
            vec!["operation-unseen".to_string()]
        );
    }

    #[test]
    fn one_reconciliation_timer_generation_serves_repeated_pending_events() {
        let timer = ReconciliationTimer::new();
        timer.set(true, Duration::from_secs(5));
        let generation = timer.generation();
        timer.set(true, Duration::from_secs(5));
        assert_eq!(timer.generation(), generation);
        timer.set(false, Duration::from_secs(5));
        assert_eq!(timer.generation(), generation + 1);
        timer.set(false, Duration::from_secs(5));
        assert_eq!(timer.generation(), generation + 1);
    }

    #[test]
    fn child_thread_start_is_routed_away_before_and_after_parent_readiness() {
        let child_start = json!({"thread":{"id":CHILD_THREAD_ID,"parentThreadId":"thread-parent"}});
        assert_eq!(
            ParentThreadFilter::notification("thread/started", &child_start, None),
            ParentThreadRoute::Child
        );
        assert_eq!(
            ParentThreadFilter::notification(
                "turn/started",
                &json!({"threadId":CHILD_THREAD_ID}),
                Some("thread-parent"),
            ),
            ParentThreadRoute::Child
        );
        assert_eq!(
            ParentThreadFilter::notification(
                "thread/started",
                &json!({"thread":{"id":"thread-parent"}}),
                None,
            ),
            ParentThreadRoute::Parent
        );
    }

    #[test]
    fn child_thread_metadata_updates_surfaces_without_touching_parent_lifecycle() {
        let root = tempfile::tempdir().unwrap();
        let script = opened_thread_script(root.path(), "read hold");
        let (session, _activity) = scripted_session(root.path(), &script);
        wait_until("opened its parent thread", || {
            session.session_id().is_some()
        });
        session.backdate_last_output(Duration::from_secs(60));
        let status_before = session.status();
        let revision = session.surfaces_changed().unwrap();

        session
            .core
            .handle_notification(InboundNotification {
                method: "thread/started".to_string(),
                params: json!({
                    "thread": {
                        "id":CHILD_THREAD_ID, "parentThreadId":THREAD_ID,
                        "preview":"Shorten the notice", "agentNickname":"Hegel",
                        "model":"gpt-6-astra", "reasoningEffort":"high"
                    }
                }),
            })
            .unwrap();
        session
            .core
            .handle_notification(InboundNotification {
                method: "thread/settings/updated".to_string(),
                params: json!({
                    "threadId":CHILD_THREAD_ID,
                    "threadSettings":{"model":"gpt-5.6-sol", "effort":null}
                }),
            })
            .unwrap();

        let agent = &session.surfaces().unwrap().subagents[0];
        assert_eq!(agent.label, "Hegel");
        assert_eq!(agent.description.as_deref(), Some("Shorten the notice"));
        assert_eq!(agent.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(agent.reasoning_effort, None);
        assert!(*revision.borrow() >= 2);
        assert_eq!(session.status(), status_before);
        assert!(session.quiet_for() >= Duration::from_secs(60));
        assert_eq!(session.epitaph(), None);

        for malformed_or_unknown in [
            InboundNotification {
                method: "thread/started".to_string(),
                params: json!({"thread":{"parentThreadId":THREAD_ID}}),
            },
            InboundNotification {
                method: "future/notification".to_string(),
                params: json!({"threadId":CHILD_THREAD_ID}),
            },
        ] {
            session
                .core
                .handle_notification(malformed_or_unknown)
                .unwrap();
        }
        assert_eq!(session.status(), status_before);
        assert!(session.quiet_for() >= Duration::from_secs(60));
        assert_eq!(session.surfaces().unwrap().subagents.len(), 1);
        assert_eq!(session.epitaph(), None);
        session.end();
    }

    #[test]
    fn thread_read_enrichment_never_touches_parent_liveness() {
        let root = tempfile::tempdir().unwrap();
        let script = opened_thread_script(root.path(), "read hold");
        let (session, _activity) = scripted_session(root.path(), &script);
        wait_until("opened its parent thread", || {
            session.session_id().is_some()
        });
        let activity = ServerNotification::Item(ItemNotification {
            lifecycle: ItemLifecycle::Completed,
            thread_id: THREAD_ID.to_string(),
            turn_id: TURN_ID.to_string(),
            item: json!({
                "id":"activity", "type":"subAgentActivity", "agentPath":"/root/reviewer",
                "agentThreadId":CHILD_THREAD_ID, "kind":"started"
            }),
        });
        {
            let mut surfaces = session.core.surfaces.lock().unwrap();
            surfaces.subagents.apply(&activity);
            assert_eq!(
                surfaces.subagents.hydration_candidate().as_deref(),
                Some(CHILD_THREAD_ID)
            );
        }
        session.backdate_last_output(Duration::from_secs(60));
        let status_before = session.status();
        let result = ThreadReadResult {
            thread: ThreadSummary {
                id: CHILD_THREAD_ID.to_string(),
                parent_thread_id: Some(THREAD_ID.to_string()),
                preview: None,
                agent_role: None,
                agent_nickname: Some("Schrodinger".to_string()),
                name: None,
                model: Some(Some("gpt-5.6-sol".to_string())),
                reasoning_effort: Some(Some("low".to_string())),
            },
        };
        session
            .core
            .handle_connection(ConnectionEvent::Response {
                operation: PendingOperation::ReadThread {
                    thread_id: CHILD_THREAD_ID.to_string(),
                },
                result: Ok(OperationResult::ThreadRead(result.clone())),
            })
            .unwrap();
        let agent = &session.surfaces().unwrap().subagents[0];
        assert_eq!(agent.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(agent.reasoning_effort.as_deref(), Some("low"));

        for response in [
            Ok(OperationResult::ThreadRead(ThreadReadResult {
                thread: ThreadSummary {
                    id: "wrong-child".to_string(),
                    ..result.thread.clone()
                },
            })),
            Err(RpcError::new(-32603, "malformed optional metadata")),
        ] {
            session
                .core
                .handle_connection(ConnectionEvent::Response {
                    operation: PendingOperation::ReadThread {
                        thread_id: CHILD_THREAD_ID.to_string(),
                    },
                    result: response,
                })
                .unwrap();
        }
        assert_eq!(session.status(), status_before);
        assert!(session.quiet_for() >= Duration::from_secs(60));
        assert_eq!(session.epitaph(), None);
        session.end();
    }

    #[test]
    fn parent_subagent_items_publish_surfaces_while_child_items_stay_isolated() {
        let root = tempfile::tempdir().unwrap();
        let child = serde_json::to_string(&json!({
            "method":"item/completed",
            "params":{
                "threadId":CHILD_THREAD_ID,
                "turnId":CHILD_TURN_ID,
                "item":{"id":"child-own","type":"subAgentActivity","agentPath":"hidden","agentThreadId":"grandchild","kind":"started"}
            }
        })).unwrap();
        let parent = serde_json::to_string(&json!({
            "method":"item/completed",
            "params":{
                "threadId":THREAD_ID,
                "turnId":TURN_ID,
                "item":{
                    "id":"spawn-call","type":"collabAgentToolCall","tool":"spawnAgent",
                    "status":"inProgress","prompt":"Inspect parser","model":"gpt-5.6-sol",
                    "reasoningEffort":"high","receiverThreadIds":[CHILD_THREAD_ID],
                    "senderThreadId":THREAD_ID,
                    "agentsStates":{CHILD_THREAD_ID:{"status":"running","message":null}}
                }
            }
        }))
        .unwrap();
        let turn_response = serde_json::to_string(&json!({
            "id":4,"result":{"turn":{"id":TURN_ID}}
        }))
        .unwrap();
        let traffic =
            format!("read turn; printf '%s\\n' '{turn_response}' '{child}' '{parent}'; sleep 1");
        let script = opened_thread_script(root.path(), &traffic);
        let (session, _activity) = scripted_session(root.path(), &script);
        let revision = session.surfaces_changed().unwrap();
        wait_until("opened its parent thread", || {
            session.session_id().is_some()
        });
        wait_until("completed its initial goal read", || {
            session.surfaces().is_some_and(|surfaces| {
                surfaces
                    .observations
                    .goal
                    .as_ref()
                    .and_then(crate::harness::surfaces::SurfaceObservation::freshness)
                    == Some(crate::harness::surfaces::SurfaceFreshness::Current)
            })
        });
        session.send_turn(&Turn::new("delegate")).unwrap();

        wait_until("published its subagent surface", || {
            session
                .surfaces()
                .is_some_and(|surfaces| surfaces.subagents.len() == 1)
        });
        assert!(*revision.borrow() > 0);
        let agents = session.surfaces().unwrap().subagents;
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, CHILD_THREAD_ID);
        assert_eq!(agents[0].description.as_deref(), Some("Inspect parser"));
        assert_eq!(agents[0].reasoning_effort.as_deref(), Some("high"));
        assert_eq!(agents[0].spawning_call_id.as_deref(), Some("spawn-call"));
        session.end();
    }

    #[test]
    fn server_response_write_failure_prevents_after_response_and_report_actions() {
        let connection =
            AppServerConnection::failing_writer(AppServerLimits::default().connection());
        let decision = ServerRequestDecision {
            response: ServerResponse::result(json!(1), json!({"decision":"decline"})),
            after_response: AfterResponse::FailTurn("must not run".to_string()),
            report: Some(ActivityReport::own_work(
                crate::harness::AgentActivity::TaskUpdate {
                    summary: "must not report".to_string(),
                },
            )),
        };
        assert!(write_server_response(&connection, decision).is_err());
    }

    fn settled(snapshot: &TerminalSnapshot, event: TerminalSourceEvent) -> TerminalSnapshot {
        snapshot.with_terminal_event(CoordinatorTerminalEvent::SourceSettled(event))
    }

    fn scripted_session(
        root: &Path,
        script: &str,
    ) -> (CodexAppServerSession, broadcast::Receiver<ActivityReport>) {
        scripted_session_under(root, script, AppServerLimits::default())
    }

    fn scripted_session_under(
        root: &Path,
        script: &str,
        limits: AppServerLimits,
    ) -> (CodexAppServerSession, broadcast::Receiver<ActivityReport>) {
        let spec = HarnessSpec::new("sh").arg("-c").arg(script);
        CodexAppServerSession::spawn(&spec, root.to_path_buf(), selected_choice(), None, limits)
            .unwrap()
    }

    fn scripted_session_with_gated_grace(
        root: &Path,
        script: &str,
    ) -> (
        CodexAppServerSession,
        broadcast::Receiver<ActivityReport>,
        mpsc::Sender<()>,
    ) {
        let spec = HarnessSpec::new("sh").arg("-c").arg(script);
        let (release, wait) = mpsc::channel();
        let (session, activity) = CodexAppServerSession::spawn_with_grace_wait(
            &spec,
            root.to_path_buf(),
            selected_choice(),
            None,
            AppServerLimits {
                source_settle_grace: Duration::from_millis(100),
                ..AppServerLimits::default()
            },
            move |_| {
                // Sender drop releases the monitor too if an assertion panics.
                let _ = wait.recv();
            },
        )
        .unwrap();
        (session, activity, release)
    }

    fn wait_for_terminal_sources(
        session: &CodexAppServerSession,
        expectation: &str,
        ready: impl Fn(&TerminalSnapshot) -> bool,
    ) {
        let snapshot = session.core.terminal_processed.lock().unwrap();
        let (snapshot, _) = session
            .core
            .terminal_processed_changed
            .wait_timeout_while(snapshot, Duration::from_secs(2), |snapshot| {
                !ready(snapshot)
            })
            .unwrap();
        assert!(ready(&snapshot), "the session never {expectation}");
    }

    fn opened_thread_script(root: &Path, thread_traffic: &str) -> String {
        let initialize_response = serde_json::to_string(
            &json!({"id":1,"result":initialize_result(&supported_user_agent())}),
        )
        .unwrap();
        let thread_response = serde_json::to_string(&json!({
            "id": 2,
            "result": thread_opened_at(
                &root.display().to_string(),
                THREAD_ID,
                Some(SELECTED_EFFORT),
            )
        }))
        .unwrap();
        let goal_response = serde_json::to_string(&json!({"id":3,"result":{"goal":null}})).unwrap();
        format!(
            "read initialize; printf '%s\\n' '{initialize_response}'; read initialized; read thread; printf '%s\\n' '{thread_response}'; read goal; printf '%s\\n' '{goal_response}'; {thread_traffic}"
        )
    }

    fn poll(mut condition: impl FnMut() -> bool) -> bool {
        for _ in 0..400 {
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    fn wait_until(expectation: &str, condition: impl FnMut() -> bool) {
        assert!(poll(condition), "the session never {expectation}");
    }

    fn wait_for_activity_closed(activity: &mut broadcast::Receiver<ActivityReport>) {
        wait_until("closed its activity channel", || {
            match activity.try_recv() {
                Err(broadcast::error::TryRecvError::Empty) => false,
                Err(broadcast::error::TryRecvError::Closed) => true,
                other => panic!("unexpected activity before channel closed: {other:?}"),
            }
        });
    }

    fn drain_reports(
        activity: &mut broadcast::Receiver<ActivityReport>,
        until: impl Fn(&[ActivityReport]) -> bool,
    ) -> Vec<ActivityReport> {
        let mut reports = Vec::new();
        poll(|| loop {
            match activity.try_recv() {
                Ok(report) => reports.push(report),
                Err(broadcast::error::TryRecvError::Empty) => return until(&reports),
                Err(broadcast::error::TryRecvError::Closed) => return true,
                Err(error) => panic!("activity receive failed: {error}"),
            }
        });
        reports
    }

    #[test]
    fn terminal_publication_waits_for_stdout_process_and_stderr_in_every_order() {
        let events = [
            TerminalSourceEvent::StdoutSettled { reader_error: None },
            TerminalSourceEvent::ProcessSettled(ProcessOutcome {
                exit_code: Some(17),
                monitor_error: None,
            }),
            TerminalSourceEvent::StderrSettled(StderrOutcome {
                retained_tail: Some("stderr tail".to_string()),
                drainer_error: None,
            }),
        ];
        for order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let mut snapshot = TerminalSnapshot::default();
            for index in &order[..2] {
                snapshot = settled(&snapshot, events[*index].clone());
                assert!(snapshot.outcome().is_none(), "{order:?}");
            }
            snapshot = settled(&snapshot, events[order[2]].clone());
            let outcome = snapshot.outcome().unwrap();
            assert_eq!(outcome.exit_code, Some(17));
            assert_eq!(outcome.epitaph.as_deref(), Some("stderr tail"));
        }
    }

    #[test]
    fn terminal_error_precedence_is_stable_when_failures_follow_reader_finished() {
        let mut snapshot = settled(
            &TerminalSnapshot::default(),
            TerminalSourceEvent::StdoutSettled {
                reader_error: Some("buffered protocol error".to_string()),
            },
        );
        assert!(snapshot.outcome().is_none());
        snapshot = settled(
            &snapshot,
            TerminalSourceEvent::ProcessSettled(ProcessOutcome {
                exit_code: None,
                monitor_error: Some("later process error".to_string()),
            }),
        );
        assert!(snapshot.outcome().is_none());
        snapshot = settled(
            &snapshot,
            TerminalSourceEvent::StderrSettled(StderrOutcome {
                retained_tail: Some("stderr fallback".to_string()),
                drainer_error: Some("later drainer error".to_string()),
            }),
        );
        assert_eq!(
            snapshot.outcome().unwrap().epitaph.as_deref(),
            Some("buffered protocol error")
        );

        let mut process_first = settled(
            &TerminalSnapshot::default(),
            TerminalSourceEvent::StdoutSettled { reader_error: None },
        );
        process_first = settled(
            &process_first,
            TerminalSourceEvent::StderrSettled(StderrOutcome {
                retained_tail: Some("tail".to_string()),
                drainer_error: Some("drainer".to_string()),
            }),
        );
        process_first = settled(
            &process_first,
            TerminalSourceEvent::ProcessSettled(ProcessOutcome {
                exit_code: None,
                monitor_error: Some("process".to_string()),
            }),
        );
        assert_eq!(
            process_first.outcome().unwrap().epitaph.as_deref(),
            Some("process")
        );

        let drainer_first = settled(
            &settled(
                &settled(
                    &TerminalSnapshot::default(),
                    TerminalSourceEvent::StdoutSettled { reader_error: None },
                ),
                TerminalSourceEvent::ProcessSettled(ProcessOutcome {
                    exit_code: Some(0),
                    monitor_error: None,
                }),
            ),
            TerminalSourceEvent::StderrSettled(StderrOutcome {
                retained_tail: Some("tail".to_string()),
                drainer_error: Some("drainer".to_string()),
            }),
        );
        assert_eq!(
            drainer_first.outcome().unwrap().epitaph.as_deref(),
            Some("drainer")
        );
    }

    #[test]
    fn source_expiry_settles_the_source_but_ranks_below_every_real_epitaph() {
        let stdout_expired = TerminalSourceEvent::SourceExpired {
            source: TerminalSource::Stdout,
            reason: "stdout expired".to_string(),
        };
        let stderr_expired = TerminalSourceEvent::SourceExpired {
            source: TerminalSource::Stderr,
            reason: "stderr expired".to_string(),
        };
        let process_ok = TerminalSourceEvent::ProcessSettled(ProcessOutcome {
            exit_code: Some(3),
            monitor_error: None,
        });

        let stdout_expiry_alone = settled(
            &settled(
                &settled(&TerminalSnapshot::default(), stdout_expired.clone()),
                process_ok.clone(),
            ),
            TerminalSourceEvent::StderrSettled(StderrOutcome {
                retained_tail: None,
                drainer_error: None,
            }),
        );
        assert_eq!(
            stdout_expiry_alone.outcome(),
            Some(TerminalOutcome {
                exit_code: Some(3),
                epitaph: Some("stdout expired".to_string()),
            })
        );

        let tail_outranks_expiry = settled(
            &settled(
                &settled(&TerminalSnapshot::default(), stdout_expired.clone()),
                process_ok.clone(),
            ),
            TerminalSourceEvent::StderrSettled(StderrOutcome {
                retained_tail: Some("boom".to_string()),
                drainer_error: None,
            }),
        );
        assert_eq!(
            tail_outranks_expiry.outcome().unwrap().epitaph.as_deref(),
            Some("boom")
        );

        let monitor_outranks_expiry = settled(
            &settled(
                &settled(&TerminalSnapshot::default(), stderr_expired.clone()),
                TerminalSourceEvent::ProcessSettled(ProcessOutcome {
                    exit_code: None,
                    monitor_error: Some("monitor".to_string()),
                }),
            ),
            TerminalSourceEvent::StdoutSettled { reader_error: None },
        );
        assert_eq!(
            monitor_outranks_expiry
                .outcome()
                .unwrap()
                .epitaph
                .as_deref(),
            Some("monitor")
        );

        let expiry_after_settlement_is_inert = settled(
            &settled(
                &settled(
                    &settled(
                        &TerminalSnapshot::default(),
                        TerminalSourceEvent::StdoutSettled { reader_error: None },
                    ),
                    TerminalSourceEvent::StderrSettled(StderrOutcome {
                        retained_tail: None,
                        drainer_error: None,
                    }),
                ),
                stdout_expired,
            ),
            stderr_expired,
        );
        assert_eq!(expiry_after_settlement_is_inert.outcome(), None);
        assert_eq!(
            settled(&expiry_after_settlement_is_inert, process_ok)
                .outcome()
                .unwrap()
                .epitaph,
            None
        );
    }

    #[test]
    fn first_protocol_error_wins_even_when_recorded_after_reader_settlement() {
        let mut snapshot = settled(
            &TerminalSnapshot::default(),
            TerminalSourceEvent::StdoutSettled { reader_error: None },
        );
        for reason in ["first protocol error", "second protocol error"] {
            snapshot = snapshot
                .with_terminal_event(CoordinatorTerminalEvent::TerminalError(reason.to_string()));
        }
        snapshot = settled(
            &snapshot,
            TerminalSourceEvent::ProcessSettled(ProcessOutcome {
                exit_code: Some(1),
                monitor_error: Some("process error".to_string()),
            }),
        );
        snapshot = settled(
            &snapshot,
            TerminalSourceEvent::StderrSettled(StderrOutcome {
                retained_tail: None,
                drainer_error: None,
            }),
        );
        assert_eq!(
            snapshot.outcome().unwrap().epitaph.as_deref(),
            Some("first protocol error")
        );
    }

    #[test]
    fn each_terminal_source_settles_exactly_once() {
        let events = [
            TerminalSourceEvent::StdoutSettled { reader_error: None },
            TerminalSourceEvent::ProcessSettled(ProcessOutcome {
                exit_code: Some(0),
                monitor_error: None,
            }),
            TerminalSourceEvent::StderrSettled(StderrOutcome {
                retained_tail: None,
                drainer_error: None,
            }),
        ];
        let mut snapshot = TerminalSnapshot::default();
        for event in &events[..2] {
            snapshot = settled(&snapshot, event.clone());
            snapshot = settled(&snapshot, event.clone());
            assert!(snapshot.outcome().is_none());
        }
        let resettled = settled(
            &settled(&snapshot, events[2].clone()),
            TerminalSourceEvent::StderrSettled(StderrOutcome {
                retained_tail: Some("late tail".to_string()),
                drainer_error: Some("late drainer".to_string()),
            }),
        );
        assert_eq!(resettled.outcome().unwrap().epitaph, None);
    }

    #[test]
    fn stdout_eof_closes_activity_and_end_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let (session, mut activity) =
            scripted_session(root.path(), "read line; exec 1>&-; exec cat >/dev/null");
        wait_until("closed activity after stdout EOF", || {
            matches!(
                activity.try_recv(),
                Err(broadcast::error::TryRecvError::Closed)
            )
        });
        assert_eq!(
            *session.core.shutdown_origin.lock().unwrap(),
            Some("stdout_eof")
        );
        assert!(session.core.diagnostics.captured().iter().any(|event| {
            event["event"] == "shutdown_trigger" && event["origin"] == "stdout_eof"
        }));
        session.end();
        session.end();
    }

    #[test]
    fn stdout_eof_reports_unanswered_tools_before_closing_activity() {
        let root = tempfile::tempdir().unwrap();
        let script = opened_thread_script(
            root.path(),
            &format!(
                "read turn; printf '%s\\n' '{}'; printf '%s\\n' '{}'",
                json!({"id":4,"result":{"turn":{"id":TURN_ID}}}),
                json!({"method":"item/started","params":{"threadId":THREAD_ID,"turnId":TURN_ID,"item":{"id":"tool-1","type":"webSearch"}}}),
            ),
        );
        let (session, mut activity) = scripted_session(root.path(), &script);
        wait_until("opened its thread", || session.session_id().is_some());
        session.send_turn(&Turn::new("go")).unwrap();

        let reports = drain_reports(&mut activity, |_| false);
        assert!(reports.iter().any(|report| matches!(
            report.activity,
            crate::harness::AgentActivity::ToolResult {
                outcome: crate::harness::ToolOutcome::Unanswered,
                ..
            }
        )));
    }

    #[test]
    fn token_usage_and_a_compaction_publish_the_threads_context() {
        let root = tempfile::tempdir().unwrap();
        let compact_capture = root.path().join("compact-frame.json");
        let usage = |context: u64| {
            let breakdown = json!({"inputTokens":context,"cachedInputTokens":context / 2,
                "cacheWriteInputTokens":0,"outputTokens":1,"reasoningOutputTokens":0,
                "totalTokens":context + 1});
            json!({"method":"thread/tokenUsage/updated","params":{"threadId":THREAD_ID,
                "turnId":TURN_ID,"tokenUsage":{"last":breakdown,"total":breakdown,
                "modelContextWindow":null}}})
        };
        let script = opened_thread_script(
            root.path(),
            &format!(
                "printf '%s\\n' '{usage}'; read compact; printf '%s\\n' \"$compact\" > {staged}; mv {staged} {capture}; printf '%s\\n' '{answer}' '{started}' '{compacted}' '{completed}'; read hold",
                usage = usage(90_000),
                staged = compact_capture.with_extension("part").display(),
                capture = compact_capture.display(),
                answer = json!({"id":4,"result":{}}),
                started = json!({"method":"turn/started","params":{"threadId":THREAD_ID,"turn":{"id":TURN_ID}}}),
                compacted = json!({"method":"item/completed","params":{"threadId":THREAD_ID,"turnId":TURN_ID,"item":{"id":"compaction-1","type":"contextCompaction"}}}),
                completed = json!({"method":"turn/completed","params":{"threadId":THREAD_ID,"turn":{"id":TURN_ID,"status":"completed"}}}),
            ),
        );
        let (session, _activity) = scripted_session(root.path(), &script);
        let watched = session.status_changed().unwrap();
        let receipts = session.turn_receipts().unwrap();
        wait_until("published the token usage", || {
            watched.borrow().context.is_some()
        });
        let reported = watched.borrow().clone();
        assert_eq!(reported.status, AgentStatus::Waiting);
        assert_eq!(
            reported.context,
            Some(TurnContext {
                context_tokens: 90_000,
                cache_read_tokens: 45_000,
            })
        );

        let mut compact = Turn::new("/compact");
        compact.operation_id = Some("op-compact".to_string());
        session.send_turn(&compact).unwrap();
        wait_until("asked codex to compact", || compact_capture.exists());
        let frame: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&compact_capture).unwrap()).unwrap();
        assert_eq!(frame["method"], "thread/compact/start");
        assert_eq!(frame["params"], json!({"threadId": THREAD_ID}));

        wait_until("finished the compaction", || {
            let snapshot = watched.borrow();
            snapshot.status == AgentStatus::Waiting && snapshot.last_worked_at.is_some()
        });
        assert_eq!(
            watched.borrow().context,
            Some(TurnContext {
                context_tokens: 0,
                cache_read_tokens: 45_000,
            })
        );
        assert_eq!(receipts.borrow().seen_operation_ids, ["op-compact"]);
        session.end();
    }

    #[test]
    fn child_thread_traffic_never_contaminates_the_parent_session() {
        let root = tempfile::tempdir().unwrap();
        let script = opened_thread_script(
            root.path(),
            &format!(
                "printf '%s\n' '{}' '{}' '{}' '{}' '{}' '{}' '{}'; read child_response; read hold",
                json!({"method":"thread/started","params":{"thread":{"id":CHILD_THREAD_ID,"parentThreadId":THREAD_ID}}}),
                json!({"method":"error","params":{"threadId":CHILD_THREAD_ID,"malformed":true}}),
                json!({"method":"item/started","params":{"threadId":CHILD_THREAD_ID,"item":{}}}),
                json!({"method":"item/agentMessage/delta","params":{"threadId":CHILD_THREAD_ID}}),
                json!({"method":"future/notification","params":{"threadId":CHILD_THREAD_ID}}),
                json!({"id":9,"method":"item/tool/requestUserInput","params":{"threadId":CHILD_THREAD_ID}}),
                json!({"method":"error","params":{"threadId":THREAD_ID,"turnId":TURN_ID,"error":{"message":"parent stays alive"},"willRetry":true}}),
            ),
        );
        let (session, mut activity) = scripted_session(root.path(), &script);

        let reports = drain_reports(&mut activity, |reports| !reports.is_empty());
        assert!(matches!(
            reports.as_slice(),
            [ActivityReport {
                activity: crate::harness::AgentActivity::TaskUpdate { summary },
                ..
            }] if summary == "parent stays alive"
        ));
        assert_eq!(session.epitaph(), None);
        assert_eq!(session.status(), AgentStatus::Waiting);
        assert_eq!(session.session_id().as_deref(), Some(THREAD_ID));
        session.end();
    }

    #[test]
    fn child_traffic_never_advances_the_parent_quiet_clock() {
        let root = tempfile::tempdir().unwrap();
        let script = opened_thread_script(
            root.path(),
            &format!(
                "printf '%s\n' '{}'; read turn; printf '%s\n' '{}' '{}' '{}'; read child_response",
                json!({"method":"error","params":{"threadId":THREAD_ID,"turnId":TURN_ID,"error":{"message":"parent stays alive"},"willRetry":true}}),
                json!({"method":"item/started","params":{"threadId":CHILD_THREAD_ID,"turnId":CHILD_TURN_ID,"item":{"id":"tool-child","type":"webSearch"}}}),
                json!({"method":"turn/completed","params":{"threadId":CHILD_THREAD_ID,"turn":{"id":CHILD_TURN_ID,"status":"completed"}}}),
                json!({"id":9,"method":"item/tool/requestUserInput","params":{"threadId":CHILD_THREAD_ID}}),
            ),
        );
        let (session, mut activity) = scripted_session(root.path(), &script);

        let mut reports = drain_reports(&mut activity, |reports| !reports.is_empty());
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert!(session.quiet_for() < Duration::from_secs(60));

        session.backdate_last_output(Duration::from_secs(60));
        session.send_turn(&Turn::new("go")).unwrap();

        reports.extend(drain_reports(&mut activity, |_| false));
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert!(
            session.quiet_for() >= Duration::from_secs(60),
            "{:?}",
            session.quiet_for()
        );
        assert_eq!(session.epitaph(), None);
        session.end();
    }

    #[test]
    fn interrupt_asks_codex_to_stop_and_never_kills_the_process() {
        let root = tempfile::tempdir().unwrap();
        let interrupt_capture = root.path().join("interrupt-frame.json");
        let script = opened_thread_script(
            root.path(),
            &format!(
                "read turn; printf '%s\\n' '{turn_response}'; read interrupt; printf '%s\\n' \"$interrupt\" > {staged_capture}; mv {staged_capture} {interrupt_capture}; read hold",
                turn_response = json!({"id":4,"result":{"turn":{"id":TURN_ID}}}),
                staged_capture = interrupt_capture.with_extension("part").display(),
                interrupt_capture = interrupt_capture.display(),
            ),
        );
        let (session, _activity) = scripted_session(root.path(), &script);
        wait_until("opened its thread", || session.session_id().is_some());
        session.send_turn(&Turn::new("go")).unwrap();
        wait_until("started a turn to interrupt", || session.can_interrupt());
        session.interrupt().unwrap();

        wait_until("asked codex to interrupt the turn", || {
            interrupt_capture.exists()
        });
        let frame: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&interrupt_capture).unwrap()).unwrap();
        assert_eq!(frame["method"], "turn/interrupt");
        assert_eq!(frame["params"]["threadId"], THREAD_ID);
        assert_eq!(frame["params"]["turnId"], TURN_ID);
        assert!(!session.exited_within(Duration::ZERO));
        assert!(!matches!(session.status(), AgentStatus::Ended { .. }));
        session.end();
        assert_eq!(
            *session.core.shutdown_origin.lock().unwrap(),
            Some("explicit_end")
        );
        let diagnostics = session.core.diagnostics.captured();
        assert!(diagnostics.iter().any(|event| {
            event["event"] == "interrupt_requested" && event["origin"] == "explicit"
        }));
        assert!(diagnostics.iter().any(|event| {
            event["event"] == "shutdown_trigger" && event["origin"] == "explicit_end"
        }));
    }

    #[test]
    fn ended_is_published_only_after_stdout_process_and_stderr_settle() {
        let root = tempfile::tempdir().unwrap();
        let stderr_release = root.path().join("release-stderr");
        struct ReleaseStderrOnDrop(PathBuf);
        impl Drop for ReleaseStderrOnDrop {
            fn drop(&mut self) {
                let _ = std::fs::write(&self.0, b"");
            }
        }
        let _release_on_drop = ReleaseStderrOnDrop(stderr_release.clone());
        let script = format!(
            "(while [ -d '{}' ] && [ ! -e '{}' ]; do sleep 0.01; done; echo late >&2) >/dev/null & exec 1>&-; exec cat >/dev/null",
            root.path().display(),
            stderr_release.display()
        );
        let (session, mut activity, release_grace) = scripted_session_with_gated_grace(
            root.path(),
            &opened_thread_script(root.path(), &script),
        );
        wait_for_terminal_sources(
            &session,
            "settled stdout and reaped its process",
            |snapshot| snapshot.stdout_settled && snapshot.process.is_some(),
        );
        assert!(
            !matches!(session.status(), AgentStatus::Ended { .. }),
            "{:?}",
            session.status()
        );
        assert_eq!(session.epitaph(), None);
        assert!(!matches!(
            activity.try_recv(),
            Err(broadcast::error::TryRecvError::Closed)
        ));

        std::fs::write(&stderr_release, b"").unwrap();
        wait_for_terminal_sources(&session, "settled stderr", |snapshot| {
            snapshot.stderr.is_some()
        });
        release_grace.send(()).unwrap();
        wait_until("published its terminal outcome", || {
            matches!(session.status(), AgentStatus::Ended { .. })
        });
        let settled_code = session
            .core
            .terminal
            .lock()
            .unwrap()
            .process
            .as_ref()
            .unwrap()
            .exit_code;
        assert!(settled_code.is_some());
        assert_eq!(session.status(), AgentStatus::Ended { code: settled_code });
        assert_eq!(session.epitaph().as_deref(), Some("late"));
        wait_for_activity_closed(&mut activity);
    }

    #[test]
    fn stderr_held_open_past_the_grace_still_publishes_ended() {
        let root = tempfile::tempdir().unwrap();
        let (session, mut activity, release_grace) = scripted_session_with_gated_grace(
            root.path(),
            &opened_thread_script(
                root.path(),
                "(sleep 30; echo late >&2) >/dev/null & exec 1>&-; exec cat >/dev/null",
            ),
        );

        wait_for_terminal_sources(
            &session,
            "settled stdout and reaped its process",
            |snapshot| snapshot.stdout_settled && snapshot.process.is_some(),
        );
        assert!(!matches!(session.status(), AgentStatus::Ended { .. }));
        release_grace.send(()).unwrap();

        wait_until("published its terminal outcome", || {
            matches!(session.status(), AgentStatus::Ended { .. })
        });
        assert!(
            matches!(session.status(), AgentStatus::Ended { code: Some(_) }),
            "{:?}",
            session.status()
        );
        assert_eq!(
            session.epitaph().as_deref(),
            Some("Codex stderr did not settle within 100ms")
        );
        wait_for_activity_closed(&mut activity);
    }

    #[test]
    fn stdout_held_open_past_the_grace_still_publishes_ended() {
        let root = tempfile::tempdir().unwrap();
        let (session, mut activity, release_grace) = scripted_session_with_gated_grace(
            root.path(),
            &opened_thread_script(root.path(), "exec 2>&-; (sleep 30) & exit 0"),
        );

        wait_for_terminal_sources(
            &session,
            "settled stderr and reaped its process",
            |snapshot| snapshot.stderr.is_some() && snapshot.process.is_some(),
        );
        assert!(!matches!(session.status(), AgentStatus::Ended { .. }));
        release_grace.send(()).unwrap();

        wait_until("published its terminal outcome", || {
            matches!(session.status(), AgentStatus::Ended { .. })
        });
        assert!(
            matches!(session.status(), AgentStatus::Ended { code: Some(_) }),
            "{:?}",
            session.status()
        );
        assert_eq!(
            session.epitaph().as_deref(),
            Some("Codex stdout did not settle within 100ms")
        );
        wait_for_activity_closed(&mut activity);
    }

    #[test]
    fn stdout_expiry_does_not_mask_the_retained_stderr_tail() {
        let root = tempfile::tempdir().unwrap();
        let (session, _activity, release_grace) = scripted_session_with_gated_grace(
            root.path(),
            &opened_thread_script(root.path(), "echo boom >&2; exec 2>&-; (sleep 30) & exit 0"),
        );

        wait_for_terminal_sources(
            &session,
            "settled stderr and reaped its process",
            |snapshot| snapshot.stderr.is_some() && snapshot.process.is_some(),
        );
        assert!(!matches!(session.status(), AgentStatus::Ended { .. }));
        release_grace.send(()).unwrap();

        wait_until("published its terminal outcome", || {
            matches!(session.status(), AgentStatus::Ended { .. })
        });
        assert!(
            session.core.terminal.lock().unwrap().expiry.is_some(),
            "the stdout expiry never fired"
        );
        assert_eq!(session.epitaph().as_deref(), Some("boom"));
    }

    #[test]
    fn protocol_failure_epitaph_precedes_stderr_fallback() {
        let root = tempfile::tempdir().unwrap();
        let (session, _activity) = scripted_session(
            root.path(),
            "read line; echo stderr-fallback >&2; echo '{bad}'",
        );
        wait_until("produced an epitaph", || session.epitaph().is_some());
        let epitaph = session.epitaph().unwrap();
        assert!(epitaph.contains("invalid JSON"), "{epitaph}");
        assert!(!epitaph.contains("stderr-fallback"), "{epitaph}");
    }
}
