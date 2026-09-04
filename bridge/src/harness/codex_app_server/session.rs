use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::broadcast;

use super::connection::{read_jsonl_frame, AppServerConnection, SharedConnection};
use super::limits::{AppServerLimits, ConnectionLimits, StateLimits};
use super::policy::{AfterResponse, ServerRequestDecision, ServerRequestPolicy};
use super::process::{AppServerProcess, TerminalEventSink, TerminalSourceEvent};
use super::protocol::{
    ClientNotification, ConnectionEvent, InboundNotification, InboundServerRequest,
    ParentThreadFilter, ParentThreadRoute, RoutedServerRequest, ServerNotification,
};
use super::state::{CodexSessionState, SessionEffect, SessionEvent};
use super::translator::CodexActivityTranslator;
use crate::harness::{ActivityReport, AgentSession, AgentStatus, HarnessError, Turn};
use crate::models::ModelChoice;
use crate::pty::HarnessSpec;

const ACTIVITY_BACKLOG: usize = 1024;

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
            | CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::ProcessSettled {
                ..
            }) => true,
            CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::StderrSettled {
                drainer_error,
                ..
            }) => drainer_error.is_some(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalOutcome {
    pub exit_code: Option<i32>,
    pub epitaph: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessOutcome {
    exit_code: Option<i32>,
    monitor_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StderrOutcome {
    retained_tail: Option<String>,
    drainer_error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerminalSnapshot {
    stdout_settled: bool,
    process: Option<ProcessOutcome>,
    stderr: Option<StderrOutcome>,
    terminal_error: Option<String>,
}

impl TerminalSnapshot {
    pub fn with_terminal_event(&self, event: CoordinatorTerminalEvent) -> TerminalSnapshot {
        let mut next = self.clone();
        match event {
            CoordinatorTerminalEvent::TerminalError(reason) => next.retain_terminal_error(reason),
            CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::StdoutSettled {
                reader_error,
            }) => {
                if !next.stdout_settled {
                    next.stdout_settled = true;
                    if let Some(reason) = reader_error {
                        next.retain_terminal_error(reason);
                    }
                }
            }
            CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::ProcessSettled {
                exit_code,
                monitor_error,
            }) => {
                next.process.get_or_insert(ProcessOutcome {
                    exit_code,
                    monitor_error,
                });
            }
            CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::StderrSettled {
                retained_tail,
                drainer_error,
            }) => {
                next.stderr.get_or_insert(StderrOutcome {
                    retained_tail,
                    drainer_error,
                });
            }
        }
        next
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
                .or_else(|| stderr.retained_tail.clone()),
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
    translator: Mutex<CodexActivityTranslator>,
    activity: Mutex<Option<broadcast::Sender<ActivityReport>>>,
    terminal: Mutex<TerminalSnapshot>,
    published: Mutex<Option<TerminalOutcome>>,
    last_message: Mutex<Instant>,
    started: Instant,
    state_limits: StateLimits,
    connection_limits: ConnectionLimits,
    binary: PathBuf,
    shutting_down: AtomicBool,
    reconciliation_timer: ReconciliationTimer,
}

impl CodexAppServerSession {
    pub fn spawn(
        spec: &HarnessSpec,
        root: PathBuf,
        choice: ModelChoice,
        resume_id: Option<String>,
        limits: AppServerLimits,
    ) -> Result<(CodexAppServerSession, broadcast::Receiver<ActivityReport>), HarnessError> {
        let binary = crate::pty::resolve_binary(spec)?;
        let (terminal_sender, terminal_events) = mpsc::channel();
        let events: TerminalEventSink = Arc::new(move |event| {
            let _ = terminal_sender.send(event);
        });
        let (process, pipes) =
            AppServerProcess::spawn(spec, root.clone(), limits.process(), Arc::clone(&events))?;
        let connection = Arc::new(AppServerConnection::new(
            Box::new(pipes.stdin),
            limits.connection(),
        ));
        let (sender, receiver) = broadcast::channel(ACTIVITY_BACKLOG);
        let core = Arc::new(SessionCore {
            connection,
            process: Arc::new(process),
            state: Mutex::new(CodexSessionState::new(
                root,
                choice.model,
                choice.effort,
                resume_id,
            )),
            translator: Mutex::new(CodexActivityTranslator::new(limits.translator())),
            activity: Mutex::new(Some(sender)),
            terminal: Mutex::new(TerminalSnapshot::default()),
            published: Mutex::new(None),
            last_message: Mutex::new(Instant::now()),
            started: Instant::now(),
            state_limits: limits.state(),
            connection_limits: limits.connection(),
            binary,
            shutting_down: AtomicBool::new(false),
            reconciliation_timer: ReconciliationTimer::new(),
        });
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

    fn apply_state(self: &Arc<Self>, event: SessionEvent) -> Result<(), HarnessError> {
        let (require_version, should_close, reconciliation_pending) = {
            let mut state = self.state.lock().unwrap();
            let transition = match state.transition(event, self.elapsed(), self.state_limits) {
                Ok(transition) => transition,
                Err(error) => {
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
            self.begin_shutdown();
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

    fn apply_effect(&self, effect: &SessionEffect) -> Result<bool, HarnessError> {
        match effect {
            SessionEffect::Request(operation) => {
                self.connection
                    .request(operation.clone())
                    .map_err(|error| HarnessError::Session(error.to_string()))?;
            }
            SessionEffect::NotifyInitialized => self
                .connection
                .notify(ClientNotification::Initialized)
                .map_err(|error| HarnessError::Session(error.to_string()))?,
            SessionEffect::ThreadReady(_) => {}
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
            response @ ConnectionEvent::Response { .. } => {
                self.accept_parent_message();
                self.apply_state(SessionEvent::Connection(response))
            }
            ConnectionEvent::Notification(notification) => self.handle_notification(notification),
            ConnectionEvent::Request(request) => self.handle_server_request(request),
        }
    }

    fn handle_notification(
        self: &Arc<Self>,
        inbound: InboundNotification,
    ) -> Result<(), HarnessError> {
        let expected_parent = self.expected_parent_thread();
        if ParentThreadFilter::notification(
            &inbound.method,
            &inbound.params,
            expected_parent.as_deref(),
        ) == ParentThreadRoute::Child
        {
            return Ok(());
        }
        self.accept_parent_message();
        let notification = ServerNotification::decode(&inbound.method, inbound.params)
            .map_err(HarnessError::Session)?;
        self.dispatch_notification(notification)
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
                self.apply_state(SessionEvent::ThreadStarted(thread_id.clone()))
            }
            ServerNotification::TurnStarted { turn_id, .. } => {
                self.apply_state(SessionEvent::TurnStarted(turn_id.clone()))
            }
            ServerNotification::TurnCompleted { completion, .. } => {
                self.apply_state(SessionEvent::ObservedCompletion(completion.clone()))
            }
            ServerNotification::Item(item) => {
                self.apply_state(SessionEvent::TurnStarted(item.turn_id.clone()))?;
                self.translate(&notification)
            }
            ServerNotification::Error(params) => {
                self.translate(&notification)?;
                if params.will_retry {
                    return Ok(());
                }
                self.apply_state(SessionEvent::FailSession(params.error.message.clone()))
            }
            ServerNotification::Delta => Ok(()),
            ServerNotification::Unknown => self.translate(&notification),
        }
    }

    fn translate(&self, notification: &ServerNotification) -> Result<(), HarnessError> {
        let reports = self
            .translator
            .lock()
            .unwrap()
            .translate_notification(notification)
            .map_err(|error| HarnessError::Session(error.to_string()))?;
        self.report_all(reports);
        Ok(())
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
        {
            let mut snapshot = self.terminal.lock().unwrap();
            *snapshot = snapshot.with_terminal_event(event);
        }
        if demands_shutdown {
            self.begin_shutdown();
        }
        let outcome = self.terminal.lock().unwrap().outcome();
        if let Some(outcome) = outcome {
            self.publish_terminal(outcome);
        }
    }

    fn publish_terminal(&self, outcome: TerminalOutcome) {
        let mut published = self.published.lock().unwrap();
        if published.is_some() {
            return;
        }
        *published = Some(outcome);
        drop(published);
        self.report_all(self.translator.lock().unwrap().close_all());
        self.close_activity();
    }

    fn record_terminal_error(&self, reason: String) {
        self.apply_terminal(CoordinatorTerminalEvent::TerminalError(reason));
    }

    fn fail(&self, reason: String) {
        self.record_terminal_error(reason);
    }

    fn begin_shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::AcqRel) {
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
}

impl AgentSession for CodexAppServerSession {
    fn send_turn(&self, turn: &Turn) -> Result<(), HarnessError> {
        self.core
            .apply_state(SessionEvent::SendTurn(turn.text.clone()))
    }

    fn status(&self) -> AgentStatus {
        if let Some(outcome) = self.core.published.lock().unwrap().as_ref() {
            return AgentStatus::Ended {
                code: outcome.exit_code,
            };
        }
        self.core.state.lock().unwrap().status()
    }

    fn quiet_for(&self) -> Duration {
        Instant::now().saturating_duration_since(*self.core.last_message.lock().unwrap())
    }

    fn exited_within(&self, timeout: Duration) -> bool {
        self.core.process.exited_within(timeout)
    }

    fn end(&self) {
        self.core.begin_shutdown();
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
        self.core.apply_state(SessionEvent::Interrupt)
    }

    fn session_id(&self) -> Option<String> {
        self.core.state.lock().unwrap().session_id()
    }

    fn active_model(&self) -> Option<String> {
        self.core.state.lock().unwrap().active_model()
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
        self.core.begin_shutdown();
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
        let frame = match read_jsonl_frame(stdout, core.connection_limits.inbound_frame_bytes) {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                let _ = core.apply_state(SessionEvent::Eof);
                return None;
            }
            Err(error) => return Some(error.to_string()),
        };
        let event = match core.connection.decode(frame) {
            Ok(event) => event,
            Err(error) => return Some(error.to_string()),
        };
        if let Err(error) = core.handle_connection(event) {
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
    use crate::harness::codex_app_server::policy::AfterResponse;
    use crate::harness::codex_app_server::protocol::ServerResponse;
    use crate::harness::AgentSession;
    use crate::models::{AgentProvider, ModelChoice};
    use crate::pty::HarnessSpec;

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
        let child_start = json!({"thread":{"id":"thread-child","parentThreadId":"thread-parent"}});
        assert_eq!(
            ParentThreadFilter::notification("thread/started", &child_start, None),
            ParentThreadRoute::Child
        );
        assert_eq!(
            ParentThreadFilter::notification(
                "turn/started",
                &json!({"threadId":"thread-child"}),
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
        let spec = HarnessSpec::new("sh").arg("-c").arg(script);
        CodexAppServerSession::spawn(
            &spec,
            root.to_path_buf(),
            ModelChoice {
                provider: AgentProvider::CodexAppServer,
                model: Some("gpt-5.6-sol".to_string()),
                effort: Some("high".to_string()),
            },
            None,
            AppServerLimits::default(),
        )
        .unwrap()
    }

    fn opened_thread_script(root: &Path, thread_traffic: &str) -> String {
        let initialize_response = r#"{"id":1,"result":{"userAgent":"build_bridge/0.153.0"}}"#;
        let thread_response = format!(
            r#"{{"id":2,"result":{{"thread":{{"id":"thread-1"}},"model":"gpt-5.6-sol","reasoningEffort":"high","cwd":"{}","approvalPolicy":"never","sandbox":{{"type":"dangerFullAccess"}}}}}}"#,
            root.display()
        );
        format!(
            "read initialize; printf '%s\\n' '{initialize_response}'; read initialized; read thread; printf '%s\\n' '{thread_response}'; {thread_traffic}"
        )
    }

    fn drain_reports(
        activity: &mut broadcast::Receiver<ActivityReport>,
        until: impl Fn(&[ActivityReport]) -> bool,
    ) -> Vec<ActivityReport> {
        let mut reports = Vec::new();
        for _ in 0..200 {
            match activity.try_recv() {
                Ok(report) => reports.push(report),
                Err(broadcast::error::TryRecvError::Closed) => break,
                Err(broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("activity receive failed: {error}"),
            }
            if until(&reports) {
                break;
            }
        }
        reports
    }

    #[test]
    fn terminal_publication_waits_for_stdout_process_and_stderr_in_every_order() {
        let events = [
            TerminalSourceEvent::StdoutSettled { reader_error: None },
            TerminalSourceEvent::ProcessSettled {
                exit_code: Some(17),
                monitor_error: None,
            },
            TerminalSourceEvent::StderrSettled {
                retained_tail: Some("stderr tail".to_string()),
                drainer_error: None,
            },
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
            TerminalSourceEvent::ProcessSettled {
                exit_code: None,
                monitor_error: Some("later process error".to_string()),
            },
        );
        assert!(snapshot.outcome().is_none());
        snapshot = settled(
            &snapshot,
            TerminalSourceEvent::StderrSettled {
                retained_tail: Some("stderr fallback".to_string()),
                drainer_error: Some("later drainer error".to_string()),
            },
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
            TerminalSourceEvent::StderrSettled {
                retained_tail: Some("tail".to_string()),
                drainer_error: Some("drainer".to_string()),
            },
        );
        process_first = settled(
            &process_first,
            TerminalSourceEvent::ProcessSettled {
                exit_code: None,
                monitor_error: Some("process".to_string()),
            },
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
                TerminalSourceEvent::ProcessSettled {
                    exit_code: Some(0),
                    monitor_error: None,
                },
            ),
            TerminalSourceEvent::StderrSettled {
                retained_tail: Some("tail".to_string()),
                drainer_error: Some("drainer".to_string()),
            },
        );
        assert_eq!(
            drainer_first.outcome().unwrap().epitaph.as_deref(),
            Some("drainer")
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
            TerminalSourceEvent::ProcessSettled {
                exit_code: Some(1),
                monitor_error: Some("process error".to_string()),
            },
        );
        snapshot = settled(
            &snapshot,
            TerminalSourceEvent::StderrSettled {
                retained_tail: None,
                drainer_error: None,
            },
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
            TerminalSourceEvent::ProcessSettled {
                exit_code: Some(0),
                monitor_error: None,
            },
            TerminalSourceEvent::StderrSettled {
                retained_tail: None,
                drainer_error: None,
            },
        ];
        let mut snapshot = TerminalSnapshot::default();
        for event in &events[..2] {
            snapshot = settled(&snapshot, event.clone());
            snapshot = settled(&snapshot, event.clone());
            assert!(snapshot.outcome().is_none());
        }
        let resettled = settled(
            &settled(&snapshot, events[2].clone()),
            TerminalSourceEvent::StderrSettled {
                retained_tail: Some("late tail".to_string()),
                drainer_error: Some("late drainer".to_string()),
            },
        );
        assert_eq!(resettled.outcome().unwrap().epitaph, None);
    }

    #[test]
    fn stdout_eof_closes_activity_and_end_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let (session, mut activity) = scripted_session(root.path(), "read line");
        for _ in 0..100 {
            if matches!(
                activity.try_recv(),
                Err(broadcast::error::TryRecvError::Closed)
            ) {
                session.end();
                session.end();
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("activity did not close after stdout EOF");
    }

    #[test]
    fn stdout_eof_reports_unanswered_tools_before_closing_activity() {
        let root = tempfile::tempdir().unwrap();
        let script = opened_thread_script(
            root.path(),
            &format!(
                "read turn; printf '%s\\n' '{}'; printf '%s\\n' '{}'",
                r#"{"id":3,"result":{"turn":{"id":"turn-1"}}}"#,
                r#"{"method":"item/started","params":{"threadId":"thread-1","turnId":"turn-1","item":{"id":"tool-1","type":"webSearch"}}}"#,
            ),
        );
        let (session, mut activity) = scripted_session(root.path(), &script);
        for _ in 0..100 {
            if session.session_id().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        session
            .send_turn(&Turn {
                text: "go".to_string(),
            })
            .unwrap();

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
    fn child_thread_traffic_never_contaminates_the_parent_session() {
        let root = tempfile::tempdir().unwrap();
        let script = opened_thread_script(
            root.path(),
            &format!(
                "printf '%s\n' '{}' '{}' '{}' '{}' '{}' '{}' '{}'; read child_response; read hold",
                r#"{"method":"thread/started","params":{"thread":{"id":"thread-child","parentThreadId":"thread-1"}}}"#,
                r#"{"method":"error","params":{"threadId":"thread-child","malformed":true}}"#,
                r#"{"method":"item/started","params":{"threadId":"thread-child","item":{}}}"#,
                r#"{"method":"item/agentMessage/delta","params":{"threadId":"thread-child"}}"#,
                r#"{"method":"future/notification","params":{"threadId":"thread-child"}}"#,
                r#"{"id":9,"method":"item/tool/requestUserInput","params":{"threadId":"thread-child"}}"#,
                r#"{"method":"error","params":{"threadId":"thread-1","turnId":"turn-1","error":{"message":"parent stays alive"},"willRetry":true}}"#,
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
        assert_eq!(session.session_id().as_deref(), Some("thread-1"));
        session.end();
    }

    #[test]
    fn child_traffic_never_advances_the_parent_quiet_clock() {
        let root = tempfile::tempdir().unwrap();
        let script = opened_thread_script(
            root.path(),
            &format!(
                "printf '%s\n' '{}'; read turn; printf '%s\n' '{}' '{}' '{}'; read child_response",
                r#"{"method":"error","params":{"threadId":"thread-1","turnId":"turn-1","error":{"message":"parent stays alive"},"willRetry":true}}"#,
                r#"{"method":"item/started","params":{"threadId":"thread-child","turnId":"turn-child","item":{"id":"tool-child","type":"webSearch"}}}"#,
                r#"{"method":"turn/completed","params":{"threadId":"thread-child","turn":{"id":"turn-child","status":"completed"}}}"#,
                r#"{"id":9,"method":"item/tool/requestUserInput","params":{"threadId":"thread-child"}}"#,
            ),
        );
        let (session, mut activity) = scripted_session(root.path(), &script);

        let mut reports = drain_reports(&mut activity, |reports| !reports.is_empty());
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert!(session.quiet_for() < Duration::from_secs(60));

        session.backdate_last_output(Duration::from_secs(60));
        session
            .send_turn(&Turn {
                text: "go".to_string(),
            })
            .unwrap();

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
    fn protocol_failure_epitaph_precedes_stderr_fallback() {
        let root = tempfile::tempdir().unwrap();
        let (session, _activity) = scripted_session(
            root.path(),
            "read line; echo stderr-fallback >&2; echo '{bad}'",
        );
        for _ in 0..100 {
            if let Some(epitaph) = session.epitaph() {
                assert!(epitaph.contains("invalid JSON"), "{epitaph}");
                assert!(!epitaph.contains("stderr-fallback"), "{epitaph}");
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("protocol failure produced no epitaph");
    }
}
