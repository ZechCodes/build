use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::broadcast;

use super::connection::{read_jsonl_frame, AppServerConnection, SharedConnection};
use super::limits::AppServerLimits;
use super::policy::{AfterResponse, ServerRequestDecision, ServerRequestPolicy};
use super::process::AppServerProcess;
use super::protocol::{ClientNotification, ConnectionEvent, ServerNotification};
use super::state::{CodexSessionState, SessionEffect, SessionEvent};
use super::translator::CodexActivityTranslator;
use crate::harness::{ActivityReport, AgentSession, AgentStatus, HarnessError, Turn};
use crate::models::ModelChoice;
use crate::pty::HarnessSpec;

const ACTIVITY_BACKLOG: usize = 1024;

pub struct CodexAppServerSession {
    core: Arc<SessionCore>,
}

struct SessionCore {
    connection: SharedConnection,
    process: Arc<AppServerProcess>,
    state: Mutex<CodexSessionState>,
    translator: Mutex<CodexActivityTranslator>,
    activity: Mutex<Option<broadcast::Sender<ActivityReport>>>,
    protocol_error: Mutex<Option<String>>,
    last_message: Mutex<Instant>,
    started: Instant,
    limits: AppServerLimits,
    binary: PathBuf,
    ended: AtomicBool,
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
        let (process, pipes) = AppServerProcess::spawn(spec, root.clone(), &limits)?;
        let connection = Arc::new(AppServerConnection::new(
            Box::new(pipes.stdin),
            limits.clone(),
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
            translator: Mutex::new(CodexActivityTranslator::new(limits.clone())),
            activity: Mutex::new(Some(sender)),
            protocol_error: Mutex::new(None),
            last_message: Mutex::new(Instant::now()),
            started: Instant::now(),
            limits,
            binary,
            ended: AtomicBool::new(false),
            reconciliation_timer: ReconciliationTimer::new(),
        });
        core.reconciliation_timer.start(Arc::downgrade(&core));
        start_reader(Arc::downgrade(&core), pipes.stdout);
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
            let transition = match state.transition(event, self.elapsed(), &self.limits) {
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
                should_close,
                state.reconciliation_pending(),
            )
        };
        if should_close {
            self.terminate();
        }
        if require_version {
            self.apply_state(SessionEvent::VersionEvidence(
                super::CodexAppServerHarness::probe_version(&self.binary),
            ))?;
        }
        self.reconciliation_timer
            .set(reconciliation_pending, self.limits.reconciliation);
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
                let reports = self.translator.lock().unwrap().close_turn(turn_id);
                self.report_all(reports);
            }
            SessionEffect::Report(report) => self.report(report.clone()),
            SessionEffect::RequireVersionEvidence => return Ok(true),
            SessionEffect::Close => unreachable!("close effects are applied after releasing state"),
        }
        Ok(false)
    }

    fn handle_connection(self: &Arc<Self>, event: ConnectionEvent) -> Result<(), HarnessError> {
        *self.last_message.lock().unwrap() = Instant::now();
        match event {
            response @ ConnectionEvent::Response { .. } => {
                self.apply_state(SessionEvent::Connection(response))
            }
            ConnectionEvent::Notification(notification) => self.handle_notification(notification),
            ConnectionEvent::Request(request) => {
                let decision = ServerRequestPolicy::decide(request, unix_seconds_now());
                let decision = write_server_response(&self.connection, decision)?;
                if let Some(report) = decision.report {
                    self.report(report);
                }
                match decision.after_response {
                    AfterResponse::Continue => Ok(()),
                    AfterResponse::FailTurn(reason) => {
                        self.apply_state(SessionEvent::FailTurn(reason))
                    }
                    AfterResponse::FailSession(reason) => {
                        self.apply_state(SessionEvent::FailSession(reason))
                    }
                }
            }
        }
    }

    fn handle_notification(
        self: &Arc<Self>,
        notification: ServerNotification,
    ) -> Result<(), HarnessError> {
        match &notification {
            ServerNotification::ThreadStarted {
                thread_id,
                parent_thread_id,
            } => {
                let active_parent = self.state.lock().unwrap().session_id();
                if is_child_thread_notification(
                    thread_id,
                    parent_thread_id.as_deref(),
                    active_parent.as_deref(),
                ) {
                    return Ok(());
                }
                self.apply_state(SessionEvent::ThreadStarted(thread_id.clone()))
            }
            ServerNotification::TurnStarted { thread_id, turn_id } => {
                if !self.parent_thread_matches(thread_id) {
                    return Ok(());
                }
                self.apply_state(SessionEvent::TurnStarted(turn_id.clone()))
            }
            ServerNotification::TurnCompleted {
                thread_id,
                completion,
            } => {
                if !self.parent_thread_matches(thread_id) {
                    return Ok(());
                }
                self.apply_state(SessionEvent::ObservedCompletion(completion.clone()))
            }
            ServerNotification::Item(item) => {
                if !self.parent_thread_matches(&item.thread_id) {
                    return Ok(());
                }
                self.apply_state(SessionEvent::TurnStarted(item.turn_id.clone()))?;
                let reports = self
                    .translator
                    .lock()
                    .unwrap()
                    .translate_notification(&notification)
                    .map_err(|error| HarnessError::Session(error.to_string()))?;
                self.report_all(reports);
                Ok(())
            }
            ServerNotification::Error(params) => {
                let reports = self
                    .translator
                    .lock()
                    .unwrap()
                    .translate_notification(&notification)
                    .map_err(|error| HarnessError::Session(error.to_string()))?;
                self.report_all(reports);
                if !params.will_retry {
                    self.protocol_error
                        .lock()
                        .unwrap()
                        .get_or_insert_with(|| params.error.message.clone());
                }
                Ok(())
            }
            ServerNotification::Delta => Ok(()),
            ServerNotification::Unknown => {
                self.translator
                    .lock()
                    .unwrap()
                    .translate_notification(&notification)
                    .map_err(|error| HarnessError::Session(error.to_string()))?;
                Ok(())
            }
        }
    }

    fn parent_thread_matches(&self, thread_id: &str) -> bool {
        self.state.lock().unwrap().parent_thread_matches(thread_id)
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

    fn fail(&self, reason: String) {
        self.protocol_error.lock().unwrap().get_or_insert(reason);
        self.terminate();
    }

    fn end(&self) {
        self.terminate();
    }

    fn terminate(&self) {
        if self.ended.swap(true, Ordering::AcqRel) {
            return;
        }
        self.reconciliation_timer.stop();
        self.report_all(self.translator.lock().unwrap().close_all());
        self.close_activity();
        if let Err(error) = self.connection.close() {
            self.protocol_error
                .lock()
                .unwrap()
                .get_or_insert_with(|| error.to_string());
        }
        if let Err(error) = self.process.shutdown() {
            self.protocol_error
                .lock()
                .unwrap()
                .get_or_insert_with(|| error.to_string());
        }
    }

    fn eof(self: &Arc<Self>) {
        let _ = self.apply_state(SessionEvent::Eof);
        self.terminate();
    }
}

impl AgentSession for CodexAppServerSession {
    fn send_turn(&self, turn: &Turn) -> Result<(), HarnessError> {
        self.core
            .apply_state(SessionEvent::SendTurn(turn.text.clone()))
    }

    fn status(&self) -> AgentStatus {
        if let Some(code) = self.core.process.exit_code() {
            return AgentStatus::Ended { code: Some(code) };
        }
        if self.core.process.liveness_failed() {
            if let Some(error) = self.core.process.error_epitaph() {
                self.core.fail(error);
            }
            return AgentStatus::Ended { code: None };
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
        self.core.end();
    }

    fn epitaph(&self) -> Option<String> {
        self.core
            .protocol_error
            .lock()
            .unwrap()
            .clone()
            .or_else(|| self.core.state.lock().unwrap().epitaph())
            .or_else(|| self.core.process.error_epitaph())
            .or_else(|| self.core.process.stderr_epitaph())
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
        self.core.end();
    }
}

fn start_reader(core: Weak<SessionCore>, mut stdout: std::process::ChildStdout) {
    std::thread::spawn(move || loop {
        let Some(core) = core.upgrade() else {
            return;
        };
        match read_jsonl_frame(&mut stdout, core.limits.inbound_frame_bytes) {
            Ok(Some(value)) => match core.connection.decode(value) {
                Ok(event) => {
                    if let Err(error) = core.handle_connection(event) {
                        core.fail(error.to_string());
                        return;
                    }
                }
                Err(error) => {
                    core.fail(error.to_string());
                    return;
                }
            },
            Ok(None) => {
                core.eof();
                return;
            }
            Err(error) => {
                core.fail(error.to_string());
                return;
            }
        }
    });
}

fn unix_seconds_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs() as i64
}

fn is_child_thread_notification(
    thread_id: &str,
    parent_thread_id: Option<&str>,
    active_parent: Option<&str>,
) -> bool {
    parent_thread_id.is_some() || active_parent.is_some_and(|parent| parent != thread_id)
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
    fn child_thread_start_is_ignored_before_and_after_parent_readiness() {
        assert!(is_child_thread_notification(
            "thread-child",
            Some("thread-parent"),
            None,
        ));
        assert!(is_child_thread_notification(
            "thread-child",
            None,
            Some("thread-parent"),
        ));
        assert!(!is_child_thread_notification(
            "thread-parent",
            None,
            Some("thread-parent"),
        ));
    }

    #[test]
    fn server_response_write_failure_prevents_after_response_and_report_actions() {
        let connection = AppServerConnection::failing_writer(AppServerLimits::default());
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

    #[test]
    fn stdout_eof_closes_activity_and_end_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let spec = HarnessSpec::new("sh").arg("-c").arg("read line");
        let (session, mut activity) = CodexAppServerSession::spawn(
            &spec,
            root.path().to_path_buf(),
            ModelChoice {
                provider: AgentProvider::CodexAppServer,
                model: None,
                effort: None,
            },
            None,
            AppServerLimits::default(),
        )
        .unwrap();
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
        let thread_response = format!(
            r#"{{"id":2,"result":{{"thread":{{"id":"thread-1"}},"model":"gpt-5.6-sol","reasoningEffort":"high","cwd":"{}","approvalPolicy":"never","sandbox":{{"type":"dangerFullAccess"}}}}}}"#,
            root.path().display()
        );
        let command = format!(
            "read initialize; printf '%s\\n' '{}'; read initialized; read thread; printf '%s\\n' '{}'; read turn; printf '%s\\n' '{}'; printf '%s\\n' '{}'",
            r#"{"id":1,"result":{"userAgent":"build_bridge/0.153.0"}}"#,
            thread_response,
            r#"{"id":3,"result":{"turn":{"id":"turn-1"}}}"#,
            r#"{"method":"item/started","params":{"threadId":"thread-1","turnId":"turn-1","item":{"id":"tool-1","type":"webSearch"}}}"#,
        );
        let spec = HarnessSpec::new("sh").arg("-c").arg(command);
        let (session, mut activity) = CodexAppServerSession::spawn(
            &spec,
            root.path().to_path_buf(),
            ModelChoice {
                provider: AgentProvider::CodexAppServer,
                model: Some("gpt-5.6-sol".to_string()),
                effort: Some("high".to_string()),
            },
            None,
            AppServerLimits::default(),
        )
        .unwrap();
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

        let mut reports = Vec::new();
        for _ in 0..100 {
            match activity.try_recv() {
                Ok(report) => reports.push(report),
                Err(broadcast::error::TryRecvError::Closed) => break,
                Err(broadcast::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("activity receive failed: {error}"),
            }
        }
        assert!(reports.iter().any(|report| matches!(
            report.activity,
            crate::harness::AgentActivity::ToolResult {
                outcome: crate::harness::ToolOutcome::Unanswered,
                ..
            }
        )));
    }

    #[test]
    fn protocol_failure_epitaph_precedes_stderr_fallback() {
        let root = tempfile::tempdir().unwrap();
        let spec = HarnessSpec::new("sh")
            .arg("-c")
            .arg("read line; echo stderr-fallback >&2; echo '{bad}'");
        let (session, _activity) = CodexAppServerSession::spawn(
            &spec,
            root.path().to_path_buf(),
            ModelChoice {
                provider: AgentProvider::CodexAppServer,
                model: None,
                effort: None,
            },
            None,
            AppServerLimits::default(),
        )
        .unwrap();
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
