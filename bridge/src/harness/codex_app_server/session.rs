use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::broadcast;

use super::connection::{AppServerConnection, SharedConnection};
use super::limits::{AppServerLimits, StateLimits};
use super::policy::{AfterResponse, ServerRequestDecision, ServerRequestPolicy};
use super::process::{AppServerProcess, TerminalEventSink, TerminalSource, TerminalSourceEvent};
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
const STATUS_WHILE_TERMINAL_SOURCES_SETTLE: AgentStatus = AgentStatus::Working;

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
            })
            | CoordinatorTerminalEvent::SourceSettled(TerminalSourceEvent::SourceExpired {
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
                next.settle_stderr(StderrOutcome {
                    retained_tail,
                    drainer_error,
                });
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
            TerminalSource::Stderr => self.settle_stderr(StderrOutcome {
                retained_tail: None,
                drainer_error: None,
            }),
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
    translator: Mutex<CodexActivityTranslator>,
    activity: Mutex<Option<broadcast::Sender<ActivityReport>>>,
    terminal: Mutex<TerminalSnapshot>,
    published: Mutex<Option<TerminalOutcome>>,
    last_message: Mutex<Instant>,
    started: Instant,
    state_limits: StateLimits,
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
            ServerNotification::Error(error) => {
                self.apply_state(SessionEvent::ObservedError(error.clone()))
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
        self.core
            .state
            .lock()
            .unwrap()
            .live_status()
            .unwrap_or(STATUS_WHILE_TERMINAL_SOURCES_SETTLE)
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
        let event = match core.connection.read_event(stdout) {
            Ok(Some(event)) => event,
            Ok(None) => {
                let _ = core.apply_state(SessionEvent::Eof);
                return None;
            }
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
    use crate::harness::codex_app_server::fixtures::{
        initialize_result, selected_choice, supported_user_agent, thread_opened_at,
        CHILD_THREAD_ID, CHILD_TURN_ID, SELECTED_EFFORT, THREAD_ID, TURN_ID,
    };
    use crate::harness::codex_app_server::policy::AfterResponse;
    use crate::harness::codex_app_server::protocol::ServerResponse;
    use crate::harness::AgentSession;
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
        format!(
            "read initialize; printf '%s\\n' '{initialize_response}'; read initialized; read thread; printf '%s\\n' '{thread_response}'; {thread_traffic}"
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
    fn source_expiry_settles_the_source_but_ranks_below_every_real_epitaph() {
        let stdout_expired = TerminalSourceEvent::SourceExpired {
            source: TerminalSource::Stdout,
            reason: "stdout expired".to_string(),
        };
        let stderr_expired = TerminalSourceEvent::SourceExpired {
            source: TerminalSource::Stderr,
            reason: "stderr expired".to_string(),
        };
        let process_ok = TerminalSourceEvent::ProcessSettled {
            exit_code: Some(3),
            monitor_error: None,
        };

        let stdout_expiry_alone = settled(
            &settled(
                &settled(&TerminalSnapshot::default(), stdout_expired.clone()),
                process_ok.clone(),
            ),
            TerminalSourceEvent::StderrSettled {
                retained_tail: None,
                drainer_error: None,
            },
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
            TerminalSourceEvent::StderrSettled {
                retained_tail: Some("boom".to_string()),
                drainer_error: None,
            },
        );
        assert_eq!(
            tail_outranks_expiry.outcome().unwrap().epitaph.as_deref(),
            Some("boom")
        );

        let monitor_outranks_expiry = settled(
            &settled(
                &settled(&TerminalSnapshot::default(), stderr_expired.clone()),
                TerminalSourceEvent::ProcessSettled {
                    exit_code: None,
                    monitor_error: Some("monitor".to_string()),
                },
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
                    TerminalSourceEvent::StderrSettled {
                        retained_tail: None,
                        drainer_error: None,
                    },
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
        wait_until("closed activity after stdout EOF", || {
            matches!(
                activity.try_recv(),
                Err(broadcast::error::TryRecvError::Closed)
            )
        });
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
                json!({"id":3,"result":{"turn":{"id":TURN_ID}}}),
                json!({"method":"item/started","params":{"threadId":THREAD_ID,"turnId":TURN_ID,"item":{"id":"tool-1","type":"webSearch"}}}),
            ),
        );
        let (session, mut activity) = scripted_session(root.path(), &script);
        wait_until("opened its thread", || session.session_id().is_some());
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
    fn interrupt_asks_codex_to_stop_and_never_kills_the_process() {
        let root = tempfile::tempdir().unwrap();
        let interrupt_capture = root.path().join("interrupt-frame.json");
        let script = opened_thread_script(
            root.path(),
            &format!(
                "read turn; printf '%s\\n' '{turn_response}'; read interrupt; printf '%s\\n' \"$interrupt\" > {staged_capture}; mv {staged_capture} {interrupt_capture}; read hold",
                turn_response = json!({"id":3,"result":{"turn":{"id":TURN_ID}}}),
                staged_capture = interrupt_capture.with_extension("part").display(),
                interrupt_capture = interrupt_capture.display(),
            ),
        );
        let (session, _activity) = scripted_session(root.path(), &script);
        wait_until("opened its thread", || session.session_id().is_some());
        session
            .send_turn(&Turn {
                text: "go".to_string(),
            })
            .unwrap();
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
    }

    #[test]
    fn ended_is_published_only_after_stdout_process_and_stderr_settle() {
        let root = tempfile::tempdir().unwrap();
        let stderr_release = root.path().join("release-stderr");
        let script = format!(
            "exec 1>&-; (until [ -e '{}' ]; do sleep 0.01; done; echo late >&2) & exec cat >/dev/null",
            stderr_release.display()
        );
        let (session, mut activity) = scripted_session(root.path(), &script);
        wait_until("settled stdout and reaped its process", || {
            let snapshot = session.core.terminal.lock().unwrap();
            snapshot.stdout_settled && snapshot.process.is_some()
        });
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
        assert!(matches!(
            activity.try_recv(),
            Err(broadcast::error::TryRecvError::Closed)
        ));
    }

    #[test]
    fn stderr_held_open_past_the_grace_still_publishes_ended() {
        let root = tempfile::tempdir().unwrap();
        let (session, mut activity) = scripted_session_under(
            root.path(),
            "exec 1>&-; (sleep 30; echo late >&2) & exec cat >/dev/null",
            AppServerLimits {
                source_settle_grace: Duration::from_millis(100),
                ..AppServerLimits::default()
            },
        );

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
        assert!(matches!(
            activity.try_recv(),
            Err(broadcast::error::TryRecvError::Closed)
        ));
    }

    #[test]
    fn stdout_held_open_past_the_grace_still_publishes_ended() {
        let root = tempfile::tempdir().unwrap();
        let (session, mut activity) = scripted_session_under(
            root.path(),
            "exec 2>&-; (sleep 30) & sleep 0.2",
            AppServerLimits {
                source_settle_grace: Duration::from_millis(100),
                ..AppServerLimits::default()
            },
        );

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
        assert!(matches!(
            activity.try_recv(),
            Err(broadcast::error::TryRecvError::Closed)
        ));
    }

    #[test]
    fn stdout_expiry_does_not_mask_the_retained_stderr_tail() {
        let root = tempfile::tempdir().unwrap();
        let (session, _activity) = scripted_session_under(
            root.path(),
            "echo boom >&2; exec 2>&-; (sleep 30) & sleep 0.2",
            AppServerLimits {
                source_settle_grace: Duration::from_millis(100),
                ..AppServerLimits::default()
            },
        );

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
