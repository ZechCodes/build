use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use semver::Version;
use serde_json::Value;

use super::limits::AppServerLimits;
use super::protocol::{ConnectionEvent, PendingOperation, RpcError};
use crate::harness::{ActivityReport, AgentActivity, AgentStatus};

const CLIENT_NAME: &str = "build_bridge";
const MINIMUM_VERSION: &str = "0.153.0";

#[derive(Debug, Clone, PartialEq)]
pub enum SessionEffect {
    Request(PendingOperation),
    NotifyInitialized,
    ThreadReady(String),
    CloseTurn(String),
    Report(ActivityReport),
    RequireVersionEvidence,
    Close,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    Start,
    SendTurn(String),
    Interrupt,
    Connection(ConnectionEvent),
    ThreadStarted(String),
    TurnStarted(String),
    TurnCompleted {
        turn_id: String,
        error: Option<String>,
    },
    VersionEvidence(Result<String, String>),
    CheckTimeouts,
    FailTurn(String),
    FailSession(String),
    Eof,
}

#[derive(Debug, thiserror::Error)]
#[error("Codex app-server protocol error: {0}")]
pub struct StateError(pub String);

#[derive(Debug, Clone)]
pub struct StateTransition {
    pub state: CodexSessionState,
    pub effects: Vec<SessionEffect>,
}

#[derive(Debug, Clone)]
pub struct CodexSessionState {
    root: PathBuf,
    selected_model: Option<String>,
    selected_effort: Option<String>,
    resume_id: Option<String>,
    phase: Phase,
    thread_id: Option<String>,
    active_model: Option<String>,
    active_effort: Option<String>,
    queued_turns: VecDeque<String>,
    queued_bytes: usize,
    last_completed_turn: Option<String>,
    reported_error: Option<String>,
}

#[derive(Debug, Clone)]
enum Phase {
    Starting,
    Initializing,
    AwaitingVersion,
    OpeningThread {
        observed_id: Option<String>,
        reconcile_since: Option<Duration>,
    },
    Waiting,
    StartingTurn {
        input: String,
        observed_id: Option<String>,
        completion: Option<TurnCompletion>,
        interrupt_after_start: bool,
        reconcile_since: Option<Duration>,
    },
    Working(WorkingTurn),
    Ending,
    Ended,
}

#[derive(Debug, Clone)]
struct WorkingTurn {
    id: String,
    steer: Option<PendingSteer>,
    interrupt: Option<PendingInterrupt>,
    completion: Option<TurnCompletion>,
}

#[derive(Debug, Clone)]
enum PendingSteer {
    Response { input: String },
    NoActiveTurn { input: String, since: Duration },
    NotSteerable { input: String },
}

#[derive(Debug, Clone)]
enum PendingInterrupt {
    Response,
    AwaitingCompletion,
}

#[derive(Debug, Clone)]
struct TurnCompletion {
    id: String,
    error: Option<String>,
}

impl CodexSessionState {
    pub fn new(
        root: PathBuf,
        selected_model: Option<String>,
        selected_effort: Option<String>,
        resume_id: Option<String>,
    ) -> CodexSessionState {
        CodexSessionState {
            root,
            selected_model,
            selected_effort,
            resume_id,
            phase: Phase::Starting,
            thread_id: None,
            active_model: None,
            active_effort: None,
            queued_turns: VecDeque::new(),
            queued_bytes: 0,
            last_completed_turn: None,
            reported_error: None,
        }
    }

    pub fn transition(
        &self,
        event: SessionEvent,
        now: Duration,
        limits: &AppServerLimits,
    ) -> Result<StateTransition, StateError> {
        let mut state = self.clone();
        let effects = state.apply(event, now, limits)?;
        Ok(StateTransition { state, effects })
    }

    fn apply(
        &mut self,
        event: SessionEvent,
        now: Duration,
        limits: &AppServerLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        match event {
            SessionEvent::Start => self.start(),
            SessionEvent::SendTurn(input) => self.send_turn(input, limits),
            SessionEvent::Interrupt => self.interrupt(),
            SessionEvent::Connection(ConnectionEvent::Response { operation, result }) => {
                self.response(operation, result, now, limits)
            }
            SessionEvent::Connection(_) => Err(StateError(
                "state received a non-response connection event".to_string(),
            )),
            SessionEvent::ThreadStarted(id) => self.thread_started(id, now),
            SessionEvent::TurnStarted(id) => self.turn_started(id, now),
            SessionEvent::TurnCompleted { turn_id, error } => {
                self.turn_completed(turn_id, error, now, limits)
            }
            SessionEvent::VersionEvidence(version) => self.version_evidence(version),
            SessionEvent::CheckTimeouts => self.check_timeouts(now, limits),
            SessionEvent::FailTurn(reason) => self.fail_turn(reason),
            SessionEvent::FailSession(reason) => {
                self.reported_error = Some(reason.clone());
                self.phase = Phase::Ending;
                Ok(vec![operational_report(reason), SessionEffect::Close])
            }
            SessionEvent::Eof => {
                self.phase = Phase::Ended;
                Ok(vec![SessionEffect::Close])
            }
        }
    }

    fn start(&mut self) -> Result<Vec<SessionEffect>, StateError> {
        if !matches!(self.phase, Phase::Starting) {
            return Err(StateError(
                "initialize was started more than once".to_string(),
            ));
        }
        self.phase = Phase::Initializing;
        Ok(vec![SessionEffect::Request(PendingOperation::Initialize)])
    }

    fn send_turn(
        &mut self,
        input: String,
        limits: &AppServerLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        match &mut self.phase {
            Phase::Waiting => Ok(vec![self.begin_start_turn(input)]),
            Phase::Working(working) if working.interrupt.is_none() && working.steer.is_none() => {
                working.steer = Some(PendingSteer::Response {
                    input: input.clone(),
                });
                Ok(vec![SessionEffect::Request(PendingOperation::SteerTurn {
                    turn_id: working.id.clone(),
                    input,
                })])
            }
            Phase::Starting
            | Phase::Initializing
            | Phase::AwaitingVersion
            | Phase::OpeningThread { .. }
            | Phase::StartingTurn { .. }
            | Phase::Working(_) => {
                self.queue(input, limits)?;
                Ok(Vec::new())
            }
            Phase::Ending | Phase::Ended => Err(StateError(
                "session has ended and accepts no more turns".to_string(),
            )),
        }
    }

    fn queue(&mut self, input: String, limits: &AppServerLimits) -> Result<(), StateError> {
        let bytes = input.len();
        if self.queued_turns.len() >= limits.queued_turns {
            return Err(StateError(format!(
                "queued turn count exceeds {}",
                limits.queued_turns
            )));
        }
        if self.queued_bytes.saturating_add(bytes) > limits.queued_turn_bytes {
            return Err(StateError(format!(
                "queued turn bytes exceed {}",
                limits.queued_turn_bytes
            )));
        }
        self.queued_turns.push_back(input);
        self.queued_bytes += bytes;
        Ok(())
    }

    fn interrupt(&mut self) -> Result<Vec<SessionEffect>, StateError> {
        let Phase::Working(working) = &mut self.phase else {
            return Ok(Vec::new());
        };
        if working.completion.is_some() || working.interrupt.is_some() {
            return Ok(Vec::new());
        }
        working.interrupt = Some(PendingInterrupt::Response);
        Ok(vec![SessionEffect::Request(
            PendingOperation::InterruptTurn {
                turn_id: working.id.clone(),
            },
        )])
    }

    fn response(
        &mut self,
        operation: PendingOperation,
        result: Result<Value, RpcError>,
        now: Duration,
        limits: &AppServerLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        match operation {
            PendingOperation::Initialize => self.initialize_response(result),
            PendingOperation::StartThread | PendingOperation::ResumeThread => {
                self.thread_response(operation, result, limits)
            }
            PendingOperation::StartTurn { input } => {
                self.start_turn_response(input, result, limits)
            }
            PendingOperation::SteerTurn { turn_id, input } => {
                self.steer_response(turn_id, input, result, now, limits)
            }
            PendingOperation::InterruptTurn { turn_id } => {
                self.interrupt_response(turn_id, result, limits)
            }
        }
    }

    fn initialize_response(
        &mut self,
        result: Result<Value, RpcError>,
    ) -> Result<Vec<SessionEffect>, StateError> {
        if !matches!(self.phase, Phase::Initializing) {
            return Err(StateError(
                "initialize response arrived out of order".to_string(),
            ));
        }
        let result = result.map_err(rpc_failure)?;
        let Some(user_agent) = result["userAgent"].as_str() else {
            self.phase = Phase::AwaitingVersion;
            return Ok(vec![SessionEffect::RequireVersionEvidence]);
        };
        match check_user_agent(user_agent) {
            VersionCheck::Supported => self.finish_initialize(),
            VersionCheck::TooOld(observed) => Err(StateError(format!(
                "Codex {observed} is unsupported; version 0.153.0 or newer is required"
            ))),
            VersionCheck::Unavailable => {
                self.phase = Phase::AwaitingVersion;
                Ok(vec![SessionEffect::RequireVersionEvidence])
            }
        }
    }

    fn version_evidence(
        &mut self,
        evidence: Result<String, String>,
    ) -> Result<Vec<SessionEffect>, StateError> {
        if !matches!(self.phase, Phase::AwaitingVersion) {
            return Err(StateError(
                "version evidence arrived out of order".to_string(),
            ));
        }
        let observed = evidence.map_err(StateError)?;
        let version = observed
            .strip_prefix("codex-cli ")
            .and_then(parse_version)
            .ok_or_else(|| {
                StateError(format!("could not parse Codex version from {observed:?}"))
            })?;
        if version < minimum_version() {
            return Err(StateError(format!(
                "Codex {version} is unsupported; version 0.153.0 or newer is required"
            )));
        }
        self.finish_initialize()
    }

    fn finish_initialize(&mut self) -> Result<Vec<SessionEffect>, StateError> {
        self.phase = Phase::OpeningThread {
            observed_id: None,
            reconcile_since: None,
        };
        let open = match self.resume_id {
            Some(_) => PendingOperation::ResumeThread,
            None => PendingOperation::StartThread,
        };
        Ok(vec![
            SessionEffect::NotifyInitialized,
            SessionEffect::Request(open),
        ])
    }

    fn thread_started(
        &mut self,
        id: String,
        now: Duration,
    ) -> Result<Vec<SessionEffect>, StateError> {
        match &mut self.phase {
            Phase::OpeningThread {
                observed_id,
                reconcile_since,
            } => {
                establish_id(observed_id, &id, "thread")?;
                reconcile_since.get_or_insert(now);
                Ok(Vec::new())
            }
            Phase::Waiting | Phase::StartingTurn { .. } | Phase::Working(_)
                if self.thread_id.as_deref() == Some(id.as_str()) =>
            {
                Ok(Vec::new())
            }
            _ => Err(StateError(format!(
                "thread/started named unexpected thread {id}"
            ))),
        }
    }

    fn thread_response(
        &mut self,
        operation: PendingOperation,
        result: Result<Value, RpcError>,
        limits: &AppServerLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let expected_resume = matches!(operation, PendingOperation::ResumeThread);
        if expected_resume != self.resume_id.is_some() {
            return Err(StateError(
                "wrong thread-open response operation".to_string(),
            ));
        }
        let Phase::OpeningThread { observed_id, .. } = &self.phase else {
            return Err(StateError(
                "thread-open response arrived out of order".to_string(),
            ));
        };
        let observed = observed_id.clone();
        let result = result.map_err(|error| {
            if observed.is_some() {
                StateError(format!(
                    "thread was announced and then open failed: {}",
                    error.message
                ))
            } else {
                rpc_failure(error)
            }
        })?;
        self.verify_thread_settings(&result)?;
        let id = required_string(&result, "/thread/id", "thread response id")?;
        if observed.as_deref().is_some_and(|candidate| candidate != id) {
            return Err(StateError(format!(
                "thread id mismatch: notification named {:?}, response named {id:?}",
                observed.unwrap()
            )));
        }
        self.thread_id = Some(id.to_string());
        self.active_model = result["model"].as_str().map(str::to_string);
        self.active_effort = result["reasoningEffort"].as_str().map(str::to_string);
        self.phase = Phase::Waiting;
        let mut effects = vec![SessionEffect::ThreadReady(id.to_string())];
        effects.extend(self.start_next_queued(limits));
        Ok(effects)
    }

    fn verify_thread_settings(&self, result: &Value) -> Result<(), StateError> {
        let expected_cwd = self.root.to_string_lossy();
        let actual_cwd = required_string(result, "/cwd", "thread cwd")?;
        if actual_cwd != expected_cwd {
            return Err(StateError(format!(
                "thread cwd mismatch: expected {expected_cwd:?}, got {actual_cwd:?}"
            )));
        }
        if result["approvalPolicy"].as_str() != Some("never") {
            return Err(StateError(
                "Codex did not apply approvalPolicy=never".to_string(),
            ));
        }
        if result.pointer("/sandbox/type").and_then(Value::as_str) != Some("dangerFullAccess") {
            return Err(StateError(
                "Codex did not apply danger-full-access sandbox".to_string(),
            ));
        }
        if let Some(selected) = &self.selected_model {
            if result["model"].as_str() != Some(selected) {
                return Err(StateError(format!(
                    "Codex opened model {:?}, expected {selected:?}",
                    result["model"]
                )));
            }
        }
        Ok(())
    }

    fn begin_start_turn(&mut self, input: String) -> SessionEffect {
        self.phase = Phase::StartingTurn {
            input: input.clone(),
            observed_id: None,
            completion: None,
            interrupt_after_start: false,
            reconcile_since: None,
        };
        SessionEffect::Request(PendingOperation::StartTurn { input })
    }

    fn start_turn_response(
        &mut self,
        input: String,
        result: Result<Value, RpcError>,
        limits: &AppServerLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let Phase::StartingTurn {
            input: retained,
            observed_id,
            completion,
            interrupt_after_start,
            ..
        } = &self.phase
        else {
            return Err(StateError(
                "turn/start response arrived out of order".to_string(),
            ));
        };
        if retained != &input {
            return Err(StateError(
                "turn/start response input did not match".to_string(),
            ));
        }
        let observed = observed_id.clone();
        let completed = completion.clone();
        let interrupt_after_start = *interrupt_after_start;
        let result = result.map_err(|error| {
            if observed.is_some() || completed.is_some() {
                StateError(format!(
                    "turn was observed and then turn/start failed: {}",
                    error.message
                ))
            } else {
                rpc_failure(error)
            }
        })?;
        let id = required_string(&result, "/turn/id", "turn/start response id")?;
        ensure_optional_id(&observed, id, "turn/start")?;
        if let Some(completion) = completed {
            ensure_id(&completion.id, id, "completed turn")?;
            self.finish_turn(completion, limits)
        } else {
            self.active_model = self.selected_model.clone().or(self.active_model.clone());
            self.active_effort = self.selected_effort.clone().or(self.active_effort.clone());
            self.phase = Phase::Working(WorkingTurn {
                id: id.to_string(),
                steer: None,
                interrupt: interrupt_after_start.then_some(PendingInterrupt::Response),
                completion: None,
            });
            Ok(interrupt_after_start
                .then(|| {
                    SessionEffect::Request(PendingOperation::InterruptTurn {
                        turn_id: id.to_string(),
                    })
                })
                .into_iter()
                .collect())
        }
    }

    fn turn_started(
        &mut self,
        id: String,
        now: Duration,
    ) -> Result<Vec<SessionEffect>, StateError> {
        match &mut self.phase {
            Phase::StartingTurn {
                observed_id,
                reconcile_since,
                ..
            } => {
                establish_id(observed_id, &id, "turn")?;
                reconcile_since.get_or_insert(now);
                Ok(Vec::new())
            }
            Phase::Working(working) if working.id == id => Ok(Vec::new()),
            Phase::Waiting if self.last_completed_turn.as_deref() == Some(id.as_str()) => {
                Ok(Vec::new())
            }
            _ => Err(StateError(format!(
                "turn/started named unexpected turn {id}"
            ))),
        }
    }

    fn turn_completed(
        &mut self,
        turn_id: String,
        error: Option<String>,
        now: Duration,
        limits: &AppServerLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let completion = TurnCompletion { id: turn_id, error };
        match &mut self.phase {
            Phase::StartingTurn {
                observed_id,
                completion: held,
                reconcile_since,
                ..
            } => {
                establish_id(observed_id, &completion.id, "turn")?;
                if let Some(existing) = held {
                    ensure_id(&existing.id, &completion.id, "duplicate completion")?;
                    return Ok(Vec::new());
                }
                *held = Some(completion);
                reconcile_since.get_or_insert(now);
                Ok(vec![SessionEffect::CloseTurn(
                    observed_id.clone().expect("completion established an id"),
                )])
            }
            Phase::Working(working) => {
                ensure_id(&working.id, &completion.id, "turn/completed")?;
                if working.completion.is_some() {
                    return Ok(Vec::new());
                }
                let mut effects = vec![SessionEffect::CloseTurn(completion.id.clone())];
                working.completion = Some(completion.clone());
                let waits_for_response =
                    matches!(working.steer, Some(PendingSteer::Response { .. }))
                        || matches!(working.interrupt, Some(PendingInterrupt::Response));
                if waits_for_response {
                    return Ok(effects);
                }
                if let Some(steer) = working.steer.take() {
                    let input = retained_steer_input(steer);
                    effects.push(self.begin_start_turn(input));
                    return Ok(effects);
                }
                effects.extend(self.finish_turn(completion, limits)?);
                Ok(effects)
            }
            Phase::Waiting
                if self.last_completed_turn.as_deref() == Some(completion.id.as_str()) =>
            {
                Ok(Vec::new())
            }
            _ => Err(StateError(format!(
                "turn/completed named unexpected turn {}",
                completion.id
            ))),
        }
    }

    fn steer_response(
        &mut self,
        operation_turn_id: String,
        operation_input: String,
        result: Result<Value, RpcError>,
        now: Duration,
        limits: &AppServerLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let Phase::Working(working) = &mut self.phase else {
            return Err(StateError(
                "turn/steer response arrived out of order".to_string(),
            ));
        };
        ensure_id(&working.id, &operation_turn_id, "steer operation")?;
        let Some(PendingSteer::Response { input }) = working.steer.take() else {
            return Err(StateError(
                "turn/steer response had no pending steer".to_string(),
            ));
        };
        if input != operation_input {
            return Err(StateError(
                "turn/steer response input did not match".to_string(),
            ));
        }
        let completion = working.completion.clone();
        match result {
            Ok(result) => {
                let returned = required_string(&result, "/turnId", "turn/steer turn id")?;
                ensure_id(&working.id, returned, "turn/steer response")?;
                if let Some(completion) = completion {
                    if working.interrupt.is_some() {
                        return Ok(Vec::new());
                    }
                    return self.finish_turn(completion, limits);
                }
                self.release_next_steer(limits)
            }
            Err(error) if error.is_no_active_turn() => {
                if let Some(completion) = completion {
                    if working.interrupt.is_some() {
                        working.steer = Some(PendingSteer::NotSteerable {
                            input: operation_input,
                        });
                        Ok(Vec::new())
                    } else {
                        self.last_completed_turn = Some(completion.id);
                        Ok(vec![self.begin_start_turn(operation_input)])
                    }
                } else {
                    working.steer = Some(PendingSteer::NoActiveTurn {
                        input: operation_input,
                        since: now,
                    });
                    Ok(Vec::new())
                }
            }
            Err(error) if error.is_active_turn_not_steerable() => {
                if let Some(completion) = completion {
                    if working.interrupt.is_some() {
                        working.steer = Some(PendingSteer::NotSteerable {
                            input: operation_input,
                        });
                        Ok(Vec::new())
                    } else {
                        self.last_completed_turn = Some(completion.id);
                        Ok(vec![self.begin_start_turn(operation_input)])
                    }
                } else {
                    working.steer = Some(PendingSteer::NotSteerable {
                        input: operation_input,
                    });
                    Ok(Vec::new())
                }
            }
            Err(error) => Err(StateError(format!(
                "Codex did not deliver steer input: {}",
                error.message
            ))),
        }
    }

    fn release_next_steer(
        &mut self,
        _limits: &AppServerLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let Some(input) = self.pop_queue() else {
            return Ok(Vec::new());
        };
        let Phase::Working(working) = &mut self.phase else {
            return Err(StateError(
                "cannot release steer without active turn".to_string(),
            ));
        };
        if working.interrupt.is_some() {
            self.queued_bytes += input.len();
            self.queued_turns.push_front(input);
            return Ok(Vec::new());
        }
        working.steer = Some(PendingSteer::Response {
            input: input.clone(),
        });
        Ok(vec![SessionEffect::Request(PendingOperation::SteerTurn {
            turn_id: working.id.clone(),
            input,
        })])
    }

    fn interrupt_response(
        &mut self,
        operation_turn_id: String,
        result: Result<Value, RpcError>,
        limits: &AppServerLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let Phase::Working(working) = &mut self.phase else {
            return Err(StateError(
                "turn/interrupt response arrived out of order".to_string(),
            ));
        };
        ensure_id(&working.id, &operation_turn_id, "interrupt operation")?;
        if !matches!(working.interrupt, Some(PendingInterrupt::Response)) {
            return Err(StateError(
                "turn/interrupt response had no pending interrupt".to_string(),
            ));
        }
        let accepted = result.is_ok()
            || result
                .as_ref()
                .is_err_and(|error| error.is_no_active_turn());
        if let Some(completion) = working.completion.clone() {
            if matches!(working.steer, Some(PendingSteer::Response { .. })) {
                working.interrupt = None;
                return Ok(Vec::new());
            }
            if let Some(steer) = working.steer.take() {
                let input = retained_steer_input(steer);
                self.last_completed_turn = Some(completion.id);
                return Ok(vec![self.begin_start_turn(input)]);
            }
            if let Err(error) = result {
                if !error.is_no_active_turn() {
                    self.reported_error = Some(error.message.clone());
                    let mut effects = self.finish_turn(completion, limits)?;
                    effects.insert(0, operational_report(error.message));
                    return Ok(effects);
                }
            }
            return self.finish_turn(completion, limits);
        }
        if accepted {
            working.interrupt = Some(PendingInterrupt::AwaitingCompletion);
            return Ok(Vec::new());
        }
        let error = result.expect_err("unaccepted result is an error");
        working.interrupt = None;
        self.reported_error = Some(error.message.clone());
        Ok(vec![operational_report(error.message)])
    }

    fn finish_turn(
        &mut self,
        completion: TurnCompletion,
        limits: &AppServerLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        self.last_completed_turn = Some(completion.id);
        self.reported_error = completion.error;
        self.phase = Phase::Waiting;
        Ok(self.start_next_queued(limits))
    }

    fn start_next_queued(&mut self, _limits: &AppServerLimits) -> Vec<SessionEffect> {
        self.pop_queue()
            .map(|input| vec![self.begin_start_turn(input)])
            .unwrap_or_default()
    }

    fn pop_queue(&mut self) -> Option<String> {
        let input = self.queued_turns.pop_front()?;
        self.queued_bytes -= input.len();
        Some(input)
    }

    fn check_timeouts(
        &mut self,
        now: Duration,
        limits: &AppServerLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let since = match &self.phase {
            Phase::OpeningThread {
                reconcile_since, ..
            }
            | Phase::StartingTurn {
                reconcile_since, ..
            } => *reconcile_since,
            Phase::Working(WorkingTurn {
                steer: Some(PendingSteer::NoActiveTurn { since, .. }),
                ..
            }) => Some(*since),
            _ => None,
        };
        if since.is_some_and(|started| now.saturating_sub(started) >= limits.reconciliation) {
            return Err(StateError(
                "notification/response reconciliation timed out".to_string(),
            ));
        }
        Ok(Vec::new())
    }

    fn fail_turn(&mut self, reason: String) -> Result<Vec<SessionEffect>, StateError> {
        self.reported_error = Some(reason.clone());
        let mut effects = vec![operational_report(reason)];
        if let Phase::StartingTurn {
            completion,
            interrupt_after_start,
            ..
        } = &mut self.phase
        {
            if completion.is_none() {
                *interrupt_after_start = true;
            }
            return Ok(effects);
        }
        effects.extend(self.interrupt()?);
        Ok(effects)
    }

    pub fn reconciliation_pending(&self) -> bool {
        matches!(
            &self.phase,
            Phase::OpeningThread {
                reconcile_since: Some(_),
                ..
            } | Phase::StartingTurn {
                reconcile_since: Some(_),
                ..
            } | Phase::Working(WorkingTurn {
                steer: Some(PendingSteer::NoActiveTurn { .. }),
                ..
            })
        )
    }

    pub fn status(&self) -> AgentStatus {
        match self.phase {
            Phase::Starting
            | Phase::Initializing
            | Phase::AwaitingVersion
            | Phase::OpeningThread { .. } => AgentStatus::Starting,
            Phase::Waiting => AgentStatus::Waiting,
            Phase::StartingTurn { .. } | Phase::Working(_) => AgentStatus::Working,
            Phase::Ending | Phase::Ended => AgentStatus::Ended { code: None },
        }
    }

    pub fn can_interrupt(&self) -> bool {
        matches!(
            &self.phase,
            Phase::Working(working)
                if working.completion.is_none() && working.interrupt.is_none()
        )
    }

    pub fn session_id(&self) -> Option<String> {
        self.thread_id.clone()
    }

    pub fn active_model(&self) -> Option<String> {
        self.active_model.clone()
    }

    pub fn epitaph(&self) -> Option<String> {
        self.reported_error.clone()
    }

    #[cfg(test)]
    pub fn resume_id(&self) -> Option<&str> {
        self.resume_id.as_deref()
    }

    #[cfg(test)]
    pub fn queued_turn_count(&self) -> usize {
        self.queued_turns.len()
    }

    #[cfg(test)]
    pub fn validate_user_agent(user_agent: &str) -> Result<(), String> {
        match check_user_agent(user_agent) {
            VersionCheck::Supported => Ok(()),
            VersionCheck::TooOld(version) => Err(format!("Codex {version} is below 0.153.0")),
            VersionCheck::Unavailable => Err("userAgent has no leading Codex version".to_string()),
        }
    }
}

fn establish_id(held: &mut Option<String>, id: &str, label: &str) -> Result<(), StateError> {
    match held {
        Some(existing) => ensure_id(existing, id, label),
        None => {
            *held = Some(id.to_string());
            Ok(())
        }
    }
}

fn ensure_optional_id(held: &Option<String>, id: &str, label: &str) -> Result<(), StateError> {
    match held {
        Some(existing) => ensure_id(existing, id, label),
        None => Ok(()),
    }
}

fn ensure_id(expected: &str, actual: &str, label: &str) -> Result<(), StateError> {
    if expected == actual {
        Ok(())
    } else {
        Err(StateError(format!(
            "{label} id mismatch: expected {expected:?}, got {actual:?}"
        )))
    }
}

fn required_string<'a>(
    value: &'a Value,
    pointer: &str,
    label: &str,
) -> Result<&'a str, StateError> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| StateError(format!("{label} is missing")))
}

fn rpc_failure(error: RpcError) -> StateError {
    StateError(format!("JSON-RPC {}: {}", error.code, error.message))
}

fn retained_steer_input(steer: PendingSteer) -> String {
    match steer {
        PendingSteer::Response { input }
        | PendingSteer::NoActiveTurn { input, .. }
        | PendingSteer::NotSteerable { input } => input,
    }
}

fn operational_report(summary: String) -> SessionEffect {
    SessionEffect::Report(ActivityReport::own_work(AgentActivity::TaskUpdate {
        summary: crate::harness::adk::one_line(&summary, crate::harness::adk::TOOL_SUMMARY_LIMIT),
    }))
}

enum VersionCheck {
    Supported,
    TooOld(Version),
    Unavailable,
}

fn check_user_agent(user_agent: &str) -> VersionCheck {
    let Some(leading) = user_agent.split_whitespace().next() else {
        return VersionCheck::Unavailable;
    };
    let Some((name, raw_version)) = leading.split_once('/') else {
        return VersionCheck::Unavailable;
    };
    if name != CLIENT_NAME || raw_version.contains('/') {
        return VersionCheck::Unavailable;
    }
    let Some(version) = parse_version(raw_version) else {
        return VersionCheck::Unavailable;
    };
    if version < minimum_version() {
        VersionCheck::TooOld(version)
    } else {
        VersionCheck::Supported
    }
}

fn parse_version(raw: &str) -> Option<Version> {
    Version::parse(raw).ok()
}

fn minimum_version() -> Version {
    Version::parse(MINIMUM_VERSION).expect("the supported Codex version is valid")
}
