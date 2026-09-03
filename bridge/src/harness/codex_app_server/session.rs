use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use tokio::sync::broadcast;

use super::connection::{read_jsonl_frame, AppServerConnection, RequestContext, SharedConnection};
use super::limits::AppServerLimits;
use super::policy::{AfterResponse, ServerRequestPolicy};
use super::process::AppServerProcess;
use super::protocol::{ConnectionEvent, ServerNotification};
use super::state::{CodexSessionState, SessionEffect, SessionEvent};
use super::translator::CodexActivityTranslator;
use crate::harness::{
    ActivityReport, AgentActivity, AgentSession, AgentStatus, HarnessError, Turn,
};
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
        let context = RequestContext {
            root: root.clone(),
            model: choice.model.clone(),
            effort: choice.effort.clone(),
            resume_id: resume_id.clone(),
            thread_id: None,
        };
        let connection = Arc::new(AppServerConnection::new(
            Box::new(pipes.stdin),
            context,
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
        });
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
        let (require_version, schedule_timeout) = {
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
            for effect in &transition.effects {
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
            (require_version, state.reconciliation_pending())
        };
        if require_version {
            self.apply_state(SessionEvent::VersionEvidence(CodexVersionProbe::probe(
                &self.binary,
            )))?;
        }
        if schedule_timeout {
            schedule_reconciliation_check(Arc::downgrade(self), self.limits.reconciliation);
        }
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
                .notify("initialized", None)
                .map_err(|error| HarnessError::Session(error.to_string()))?,
            SessionEffect::ThreadReady(thread_id) => {
                self.connection.set_thread_id(thread_id.clone());
            }
            SessionEffect::CloseTurn(turn_id) => {
                let reports = self.translator.lock().unwrap().close_turn(turn_id);
                self.report_all(reports);
            }
            SessionEffect::Report(report) => self.report(report.clone()),
            SessionEffect::RequireVersionEvidence => return Ok(true),
            SessionEffect::Close => self.close_activity(),
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
                let approval_request = matches!(
                    request.method.as_str(),
                    "item/commandExecution/requestApproval"
                        | "item/fileChange/requestApproval"
                        | "execCommandApproval"
                        | "applyPatchApproval"
                );
                let decision = ServerRequestPolicy::decide(request, unix_seconds_now());
                self.connection
                    .respond(decision.response)
                    .map_err(|error| HarnessError::Session(error.to_string()))?;
                if approval_request {
                    self.report(ActivityReport::own_work(AgentActivity::TaskUpdate {
                        summary: "Codex approval request declined".to_string(),
                    }));
                }
                match decision.after_response {
                    AfterResponse::Continue => Ok(()),
                    AfterResponse::FailTurn(reason) => {
                        self.apply_state(SessionEvent::FailTurn(reason))
                    }
                    AfterResponse::FailSession(reason) => {
                        self.apply_state(SessionEvent::FailSession(reason))?;
                        self.end();
                        Ok(())
                    }
                }
            }
        }
    }

    fn handle_notification(
        self: &Arc<Self>,
        notification: ServerNotification,
    ) -> Result<(), HarnessError> {
        let method = notification.method.as_str();
        let params = &notification.params;
        match method {
            "thread/started" => self.apply_state(SessionEvent::ThreadStarted(required_id(
                params,
                "/thread/id",
                method,
            )?)),
            "turn/started" => self.apply_state(SessionEvent::TurnStarted(required_id(
                params, "/turn/id", method,
            )?)),
            "turn/completed" => self.apply_state(SessionEvent::TurnCompleted {
                turn_id: required_id(params, "/turn/id", method)?,
                error: params
                    .pointer("/turn/error/message")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }),
            "item/started" | "item/completed" => {
                let turn_id = required_id(params, "/turnId", method)?;
                self.apply_state(SessionEvent::TurnStarted(turn_id))?;
                let reports = self
                    .translator
                    .lock()
                    .unwrap()
                    .translate(method, params)
                    .map_err(|error| HarnessError::Session(error.to_string()))?;
                self.report_all(reports);
                Ok(())
            }
            "error" => {
                let reports = self
                    .translator
                    .lock()
                    .unwrap()
                    .translate(method, params)
                    .map_err(|error| HarnessError::Session(error.to_string()))?;
                self.report_all(reports);
                if params["willRetry"].as_bool() == Some(false) {
                    if let Some(message) = params.pointer("/error/message").and_then(Value::as_str)
                    {
                        self.protocol_error
                            .lock()
                            .unwrap()
                            .get_or_insert_with(|| message.to_string());
                    }
                }
                Ok(())
            }
            method if method.contains("delta") => Ok(()),
            _ => {
                self.translator
                    .lock()
                    .unwrap()
                    .translate(method, params)
                    .map_err(|error| HarnessError::Session(error.to_string()))?;
                Ok(())
            }
        }
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
        self.report_all(self.translator.lock().unwrap().close_all());
        self.close_activity();
        let _ = self.connection.close();
        let _ = self.process.shutdown();
        self.ended.store(true, Ordering::Release);
    }

    fn end(&self) {
        if self.ended.swap(true, Ordering::AcqRel) {
            return;
        }
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
        self.report_all(self.translator.lock().unwrap().close_all());
        let _ = self.apply_state(SessionEvent::Eof);
        self.end();
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

fn schedule_reconciliation_check(core: Weak<SessionCore>, delay: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        if let Some(core) = core.upgrade() {
            if let Err(error) = core.apply_state(SessionEvent::CheckTimeouts) {
                core.fail(error.to_string());
            }
        }
    });
}

fn required_id(params: &Value, pointer: &str, method: &str) -> Result<String, HarnessError> {
    params
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| HarnessError::Session(format!("malformed {method}: missing {pointer}")))
}

fn unix_seconds_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs() as i64
}

struct CodexVersionProbe;

impl CodexVersionProbe {
    fn probe(binary: &Path) -> Result<String, String> {
        static CACHE: OnceLock<Mutex<HashMap<PathBuf, Result<String, String>>>> = OnceLock::new();
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some(cached) = cache.lock().unwrap().get(binary).cloned() {
            return cached;
        }
        let observed = Command::new(binary)
            .arg("--version")
            .output()
            .map_err(|error| format!("cannot run {} --version: {error}", binary.display()))
            .and_then(|output| {
                if !output.status.success() {
                    return Err(format!(
                        "{} --version exited with {}",
                        binary.display(),
                        output.status
                    ));
                }
                String::from_utf8(output.stdout)
                    .map(|text| text.trim().to_string())
                    .map_err(|error| format!("Codex version output is not UTF-8: {error}"))
            });
        cache
            .lock()
            .unwrap()
            .insert(binary.to_path_buf(), observed.clone());
        observed
    }
}
