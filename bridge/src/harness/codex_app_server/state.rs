use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use super::limits::StateLimits;
use super::protocol::{
    ConnectionEvent, ErrorNotification, InitializeResult, OperationResult, PendingOperation,
    RpcError, ThreadOpenResult, TurnCompletion, TurnStartResult, TurnSteerResult, CLIENT_NAME,
};
use crate::harness::{ActivityReport, AgentActivity, AgentStatus};
use semver::Version;

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
    ObservedCompletion(TurnCompletion),
    ObservedError(ErrorNotification),
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
    last_completion: Option<TurnCompletion>,
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
    ActiveTurnNotSteerable { input: String },
    ReplayAfterInterrupt { input: String },
}

#[derive(Debug, Clone)]
enum PendingInterrupt {
    Response,
    AwaitingCompletion,
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
            last_completion: None,
            reported_error: None,
        }
    }

    pub fn transition(
        &self,
        event: SessionEvent,
        now: Duration,
        limits: StateLimits,
    ) -> Result<StateTransition, StateError> {
        let mut state = self.clone();
        let effects = state.apply(event, now, limits)?;
        Ok(StateTransition { state, effects })
    }

    fn apply(
        &mut self,
        event: SessionEvent,
        now: Duration,
        limits: StateLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        match event {
            command @ (SessionEvent::Start
            | SessionEvent::SendTurn(_)
            | SessionEvent::Interrupt) => self.apply_command(command, limits),
            SessionEvent::Connection(event) => self.apply_connection(event, now, limits),
            lifecycle @ (SessionEvent::ThreadStarted(_)
            | SessionEvent::TurnStarted(_)
            | SessionEvent::ObservedCompletion(_)
            | SessionEvent::ObservedError(_)
            | SessionEvent::Eof) => self.apply_lifecycle(lifecycle, now, limits),
            reconciliation @ (SessionEvent::VersionEvidence(_) | SessionEvent::CheckTimeouts) => {
                self.apply_reconciliation(reconciliation, now, limits)
            }
            failure @ (SessionEvent::FailTurn(_) | SessionEvent::FailSession(_)) => {
                self.apply_failure(failure)
            }
        }
    }

    fn apply_command(
        &mut self,
        command: SessionEvent,
        limits: StateLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        match command {
            SessionEvent::Start => self.start(),
            SessionEvent::SendTurn(input) => self.send_turn(input, limits),
            SessionEvent::Interrupt => self.interrupt(),
            _ => unreachable!(),
        }
    }

    fn apply_lifecycle(
        &mut self,
        event: SessionEvent,
        now: Duration,
        limits: StateLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        match event {
            SessionEvent::ThreadStarted(id) => self.thread_started(id, now),
            SessionEvent::TurnStarted(id) => self.turn_started(id, now),
            SessionEvent::ObservedCompletion(completion) => {
                self.complete_turn(completion, now, limits)
            }
            SessionEvent::ObservedError(notification) => self.observe_error(notification),
            SessionEvent::Eof => {
                self.phase = Phase::Ended;
                Ok(vec![SessionEffect::Close])
            }
            _ => unreachable!(),
        }
    }

    fn apply_connection(
        &mut self,
        event: ConnectionEvent,
        now: Duration,
        limits: StateLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        match event {
            ConnectionEvent::Response { operation, result } => {
                self.response(operation, result, now, limits)
            }
            _ => Err(StateError(
                "state received a non-response connection event".to_string(),
            )),
        }
    }

    fn apply_reconciliation(
        &mut self,
        event: SessionEvent,
        now: Duration,
        limits: StateLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        match event {
            SessionEvent::VersionEvidence(version) => self.version_evidence(version),
            SessionEvent::CheckTimeouts => self.check_timeouts(now, limits),
            _ => unreachable!(),
        }
    }

    fn apply_failure(&mut self, event: SessionEvent) -> Result<Vec<SessionEffect>, StateError> {
        match event {
            SessionEvent::FailTurn(reason) => self.fail_turn(reason),
            SessionEvent::FailSession(reason) => self.fail_session(reason),
            _ => unreachable!(),
        }
    }

    fn observe_error(
        &mut self,
        notification: ErrorNotification,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let message = notification.error.message;
        if notification.will_retry {
            Ok(vec![operational_report(message)])
        } else {
            self.fail_session(message)
        }
    }

    fn fail_session(&mut self, reason: String) -> Result<Vec<SessionEffect>, StateError> {
        self.reported_error = Some(reason.clone());
        self.phase = Phase::Ending;
        Ok(vec![operational_report(reason), SessionEffect::Close])
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
        limits: StateLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let thread_id = self.thread_id.clone();
        match &mut self.phase {
            Phase::Waiting => Ok(vec![self.begin_start_turn(input)]),
            Phase::Working(working) if working.interrupt.is_none() && working.steer.is_none() => {
                working.steer = Some(PendingSteer::Response {
                    input: input.clone(),
                });
                Ok(vec![SessionEffect::Request(PendingOperation::SteerTurn {
                    thread_id: thread_id.expect("an active turn has a thread"),
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

    fn queue(&mut self, input: String, limits: StateLimits) -> Result<(), StateError> {
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
        let thread_id = self.thread_id.clone();
        let Phase::Working(working) = &mut self.phase else {
            return Ok(Vec::new());
        };
        if working.completion.is_some() || working.interrupt.is_some() {
            return Ok(Vec::new());
        }
        working.interrupt = Some(PendingInterrupt::Response);
        Ok(vec![SessionEffect::Request(
            PendingOperation::InterruptTurn {
                thread_id: thread_id.expect("an active turn has a thread"),
                turn_id: working.id.clone(),
            },
        )])
    }

    fn response(
        &mut self,
        operation: PendingOperation,
        result: Result<OperationResult, RpcError>,
        now: Duration,
        limits: StateLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        match operation {
            PendingOperation::Initialize => self.initialize_response(expect_initialize(result)?),
            PendingOperation::StartThread { .. } | PendingOperation::ResumeThread { .. } => {
                self.thread_response(operation, result, limits)
            }
            PendingOperation::StartTurn { input, .. } => {
                self.start_turn_response(input, result, limits)
            }
            PendingOperation::SteerTurn { turn_id, input, .. } => {
                self.steer_response(turn_id, input, result, now, limits)
            }
            PendingOperation::InterruptTurn { turn_id, .. } => {
                self.interrupt_response(turn_id, expect_interrupted(result)?, limits)
            }
        }
    }

    fn initialize_response(
        &mut self,
        result: Result<InitializeResult, RpcError>,
    ) -> Result<Vec<SessionEffect>, StateError> {
        if !matches!(self.phase, Phase::Initializing) {
            return Err(StateError(
                "initialize response arrived out of order".to_string(),
            ));
        }
        let result = result.map_err(rpc_failure)?;
        let Some(user_agent) = result.user_agent else {
            self.phase = Phase::AwaitingVersion;
            return Ok(vec![SessionEffect::RequireVersionEvidence]);
        };
        match check_user_agent(&user_agent) {
            VersionCheck::Supported => self.finish_initialize(),
            VersionCheck::TooOld(observed) => Err(version_rejection(format!(
                "Codex {observed} is unsupported"
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
        let observed = evidence.map_err(|failure| {
            version_rejection(format!(
                "could not obtain Codex version evidence: {failure}"
            ))
        })?;
        let version = observed
            .strip_prefix("codex-cli ")
            .and_then(parse_version)
            .ok_or_else(|| {
                version_rejection(format!("could not parse Codex version from {observed:?}"))
            })?;
        if version < minimum_version() {
            return Err(version_rejection(format!("Codex {version} is unsupported")));
        }
        self.finish_initialize()
    }

    fn finish_initialize(&mut self) -> Result<Vec<SessionEffect>, StateError> {
        self.phase = Phase::OpeningThread {
            observed_id: None,
            reconcile_since: None,
        };
        let cwd = self.root.to_string_lossy().into_owned();
        let open = match &self.resume_id {
            Some(thread_id) => PendingOperation::ResumeThread {
                thread_id: thread_id.clone(),
                cwd,
                model: self.selected_model.clone(),
            },
            None => PendingOperation::StartThread {
                cwd,
                model: self.selected_model.clone(),
            },
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
        result: Result<OperationResult, RpcError>,
        limits: StateLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let expected_resume = matches!(operation, PendingOperation::ResumeThread { .. });
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
        let typed_result = expect_thread_opened(result)?;
        let result = typed_result.map_err(|error| {
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
        let id = result.thread.id;
        if observed.as_deref().is_some_and(|candidate| candidate != id) {
            return Err(StateError(format!(
                "thread id mismatch: notification named {:?}, response named {id:?}",
                observed.unwrap()
            )));
        }
        self.thread_id = Some(id.clone());
        self.active_model = Some(result.model);
        self.active_effort = result.reasoning_effort;
        self.phase = Phase::Waiting;
        let mut effects = vec![SessionEffect::ThreadReady(id)];
        effects.extend(self.start_next_queued(limits));
        Ok(effects)
    }

    fn verify_thread_settings(&self, result: &ThreadOpenResult) -> Result<(), StateError> {
        let expected_cwd = self.root.to_string_lossy();
        let actual_cwd = &result.cwd;
        if actual_cwd != expected_cwd.as_ref() {
            return Err(StateError(format!(
                "thread cwd mismatch: expected {expected_cwd:?}, got {actual_cwd:?}"
            )));
        }
        if result.approval_policy != "never" {
            return Err(StateError(
                "Codex did not apply approvalPolicy=never".to_string(),
            ));
        }
        if result.sandbox.kind != "dangerFullAccess" {
            return Err(StateError(
                "Codex did not apply danger-full-access sandbox".to_string(),
            ));
        }
        if let Some(selected) = &self.selected_model {
            if result.model != *selected {
                return Err(StateError(format!(
                    "Codex opened model {:?}, expected {selected:?}",
                    result.model
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
        SessionEffect::Request(PendingOperation::StartTurn {
            thread_id: self
                .thread_id
                .clone()
                .expect("a turn starts only after its thread is ready"),
            input,
            model: self.selected_model.clone(),
            effort: self.selected_effort.clone(),
        })
    }

    fn start_turn_response(
        &mut self,
        input: String,
        result: Result<OperationResult, RpcError>,
        limits: StateLimits,
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
        let typed_result = expect_turn_started(result)?;
        let result = typed_result.map_err(|error| {
            if observed.is_some() || completed.is_some() {
                StateError(format!(
                    "turn was observed and then turn/start failed: {}",
                    error.message
                ))
            } else {
                rpc_failure(error)
            }
        })?;
        let id = result.turn.id;
        ensure_optional_id(&observed, &id, "turn/start")?;
        self.active_model = self.selected_model.clone().or(self.active_model.clone());
        self.active_effort = self.selected_effort.clone().or(self.active_effort.clone());
        if let Some(completion) = completed {
            ensure_id(&completion.turn_id, &id, "completed turn")?;
            self.finish_turn(completion, limits)
        } else {
            self.phase = Phase::Working(WorkingTurn {
                id: id.clone(),
                steer: None,
                interrupt: interrupt_after_start.then_some(PendingInterrupt::Response),
                completion: None,
            });
            Ok(interrupt_after_start
                .then(|| {
                    SessionEffect::Request(PendingOperation::InterruptTurn {
                        thread_id: self.thread_id.clone().expect("an active turn has a thread"),
                        turn_id: id,
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
            Phase::Waiting
                if self
                    .last_completion
                    .as_ref()
                    .is_some_and(|completion| completion.turn_id == id) =>
            {
                Ok(Vec::new())
            }
            _ => Err(StateError(format!(
                "turn/started named unexpected turn {id}"
            ))),
        }
    }

    fn complete_turn(
        &mut self,
        completion: TurnCompletion,
        now: Duration,
        limits: StateLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        if matches!(self.phase, Phase::StartingTurn { .. }) {
            return self.complete_starting_turn(completion, now);
        }
        if matches!(self.phase, Phase::Working(_)) {
            return self.complete_working_turn(completion, limits);
        }
        if matches!(self.phase, Phase::Waiting) {
            return self.check_waiting_duplicate(completion);
        }
        Err(unexpected_completion(&completion))
    }

    fn complete_starting_turn(
        &mut self,
        completion: TurnCompletion,
        now: Duration,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let Phase::StartingTurn {
            observed_id,
            completion: held,
            reconcile_since,
            ..
        } = &mut self.phase
        else {
            unreachable!()
        };
        establish_id(observed_id, &completion.turn_id, "turn")?;
        if let Some(existing) = held {
            return duplicate_completion(existing, &completion);
        }
        *held = Some(completion);
        reconcile_since.get_or_insert(now);
        Ok(vec![SessionEffect::CloseTurn(
            observed_id.clone().expect("completion established an id"),
        )])
    }

    fn complete_working_turn(
        &mut self,
        completion: TurnCompletion,
        limits: StateLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let Phase::Working(working) = &mut self.phase else {
            unreachable!()
        };
        ensure_id(&working.id, &completion.turn_id, "turn/completed")?;
        if let Some(existing) = &working.completion {
            return duplicate_completion(existing, &completion);
        }
        let mut effects = vec![SessionEffect::CloseTurn(completion.turn_id.clone())];
        working.completion = Some(completion.clone());
        if pending_response(working) {
            return Ok(effects);
        }
        if let Some(steer) = working.steer.take() {
            let input = retained_steer_input(steer);
            self.remember_completion(completion);
            effects.push(self.begin_start_turn(input));
            return Ok(effects);
        }
        effects.extend(self.finish_turn(completion, limits)?);
        Ok(effects)
    }

    fn check_waiting_duplicate(
        &self,
        completion: TurnCompletion,
    ) -> Result<Vec<SessionEffect>, StateError> {
        match &self.last_completion {
            Some(existing) if existing.turn_id == completion.turn_id => {
                duplicate_completion(existing, &completion)
            }
            None => Err(unexpected_completion(&completion)),
            Some(_) => Err(unexpected_completion(&completion)),
        }
    }

    fn steer_response(
        &mut self,
        operation_turn_id: String,
        operation_input: String,
        result: Result<OperationResult, RpcError>,
        now: Duration,
        limits: StateLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let completion = self.take_pending_steer(&operation_turn_id, &operation_input)?;
        match result {
            Ok(OperationResult::TurnSteered(TurnSteerResult { turn_id })) => {
                self.accept_steer(turn_id, completion, limits)
            }
            Err(error) if error.is_no_active_turn() => {
                self.reconcile_no_active_turn(operation_input, completion, now)
            }
            Err(error) if error.is_active_turn_not_steerable() => {
                self.reconcile_non_steerable(operation_input, completion)
            }
            Ok(_) => Err(StateError(
                "turn/steer response body was mistyped".to_string(),
            )),
            Err(error) => Err(StateError(format!(
                "Codex did not deliver steer input: {}",
                error.message
            ))),
        }
    }

    fn take_pending_steer(
        &mut self,
        operation_turn_id: &str,
        operation_input: &str,
    ) -> Result<Option<TurnCompletion>, StateError> {
        let Phase::Working(working) = &mut self.phase else {
            return Err(StateError(
                "turn/steer response arrived out of order".to_string(),
            ));
        };
        ensure_id(&working.id, operation_turn_id, "steer operation")?;
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
        Ok(working.completion.clone())
    }

    fn accept_steer(
        &mut self,
        returned_turn_id: String,
        completion: Option<TurnCompletion>,
        limits: StateLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let Phase::Working(working) = &self.phase else {
            unreachable!()
        };
        ensure_id(&working.id, &returned_turn_id, "turn/steer response")?;
        if completion.is_some() && working.interrupt.is_some() {
            return Ok(Vec::new());
        }
        match completion {
            Some(completion) => self.finish_turn(completion, limits),
            None => self.release_next_steer(limits),
        }
    }

    fn reconcile_no_active_turn(
        &mut self,
        input: String,
        completion: Option<TurnCompletion>,
        now: Duration,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let Phase::Working(working) = &mut self.phase else {
            unreachable!()
        };
        let Some(completion) = completion else {
            working.steer = Some(PendingSteer::NoActiveTurn { input, since: now });
            return Ok(Vec::new());
        };
        if working.interrupt.is_some() {
            working.steer = Some(PendingSteer::ReplayAfterInterrupt { input });
            return Ok(Vec::new());
        }
        self.remember_completion(completion);
        Ok(vec![self.begin_start_turn(input)])
    }

    fn reconcile_non_steerable(
        &mut self,
        input: String,
        completion: Option<TurnCompletion>,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let Phase::Working(working) = &mut self.phase else {
            unreachable!()
        };
        let Some(completion) = completion else {
            working.steer = Some(PendingSteer::ActiveTurnNotSteerable { input });
            return Ok(Vec::new());
        };
        if working.interrupt.is_some() {
            working.steer = Some(PendingSteer::ReplayAfterInterrupt { input });
            return Ok(Vec::new());
        }
        self.remember_completion(completion);
        Ok(vec![self.begin_start_turn(input)])
    }

    fn release_next_steer(
        &mut self,
        _limits: StateLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let Some(input) = self.pop_queue() else {
            return Ok(Vec::new());
        };
        let thread_id = self.thread_id.clone().expect("an active turn has a thread");
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
            thread_id,
            turn_id: working.id.clone(),
            input,
        })])
    }

    fn interrupt_response(
        &mut self,
        operation_turn_id: String,
        result: Result<(), RpcError>,
        limits: StateLimits,
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
        let completion = working.completion.clone();
        if let Some(completion) = completion {
            return self.interrupt_after_completion(completion, result, limits);
        }
        self.interrupt_before_completion(result)
    }

    fn interrupt_before_completion(
        &mut self,
        result: Result<(), RpcError>,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let Phase::Working(working) = &mut self.phase else {
            unreachable!()
        };
        let accepted = result.is_ok()
            || result
                .as_ref()
                .is_err_and(|error| error.is_no_active_turn());
        if accepted {
            working.interrupt = Some(PendingInterrupt::AwaitingCompletion);
            return Ok(Vec::new());
        }
        let error = result.expect_err("unaccepted result is an error");
        working.interrupt = None;
        self.reported_error = Some(error.message.clone());
        Ok(vec![operational_report(error.message)])
    }

    fn interrupt_after_completion(
        &mut self,
        completion: TurnCompletion,
        result: Result<(), RpcError>,
        limits: StateLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        let Phase::Working(working) = &mut self.phase else {
            unreachable!()
        };
        if matches!(working.steer, Some(PendingSteer::Response { .. })) {
            working.interrupt = None;
            return self.report_late_interrupt_error(result);
        }
        if let Some(steer) = working.steer.take() {
            let input = retained_steer_input(steer);
            self.remember_completion(completion);
            return Ok(vec![self.begin_start_turn(input)]);
        }
        let report = match result {
            Err(error) if !error.is_no_active_turn() => {
                self.reported_error = Some(error.message.clone());
                Some(operational_report(error.message))
            }
            _ => None,
        };
        let mut effects = self.finish_turn(completion, limits)?;
        effects.splice(0..0, report);
        Ok(effects)
    }

    fn report_late_interrupt_error(
        &mut self,
        result: Result<(), RpcError>,
    ) -> Result<Vec<SessionEffect>, StateError> {
        match result {
            Err(error) if !error.is_no_active_turn() => {
                self.reported_error = Some(error.message.clone());
                Ok(vec![operational_report(error.message)])
            }
            _ => Ok(Vec::new()),
        }
    }

    fn finish_turn(
        &mut self,
        completion: TurnCompletion,
        limits: StateLimits,
    ) -> Result<Vec<SessionEffect>, StateError> {
        self.remember_completion(completion.clone());
        if completion.error.is_some() {
            self.reported_error = completion.error;
        }
        self.phase = Phase::Waiting;
        Ok(self.start_next_queued(limits))
    }

    fn remember_completion(&mut self, completion: TurnCompletion) {
        self.last_completion = Some(completion);
    }

    fn start_next_queued(&mut self, _limits: StateLimits) -> Vec<SessionEffect> {
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
        limits: StateLimits,
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

    pub fn expected_parent_thread(&self) -> Option<String> {
        self.thread_id.clone().or_else(|| match &self.phase {
            Phase::OpeningThread {
                observed_id: Some(observed),
                ..
            } => Some(observed.clone()),
            _ => None,
        })
    }

    #[cfg(test)]
    pub fn parent_thread_matches(&self, thread_id: &str) -> bool {
        self.expected_parent_thread().as_deref() == Some(thread_id)
    }

    pub fn active_model(&self) -> Option<String> {
        self.active_model.clone()
    }

    #[cfg(test)]
    pub fn active_effort(&self) -> Option<String> {
        self.active_effort.clone()
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

fn duplicate_completion(
    existing: &TurnCompletion,
    duplicate: &TurnCompletion,
) -> Result<Vec<SessionEffect>, StateError> {
    if existing == duplicate {
        Ok(Vec::new())
    } else {
        Err(StateError(format!(
            "conflicting duplicate completion for turn {:?}",
            duplicate.turn_id
        )))
    }
}

fn unexpected_completion(completion: &TurnCompletion) -> StateError {
    StateError(format!(
        "turn/completed named unexpected turn {}",
        completion.turn_id
    ))
}

fn pending_response(working: &WorkingTurn) -> bool {
    matches!(working.steer, Some(PendingSteer::Response { .. }))
        || matches!(working.interrupt, Some(PendingInterrupt::Response))
}

fn rpc_failure(error: RpcError) -> StateError {
    StateError(format!("JSON-RPC {}: {}", error.code, error.message))
}

fn expect_initialize(
    result: Result<OperationResult, RpcError>,
) -> Result<Result<InitializeResult, RpcError>, StateError> {
    match result {
        Ok(OperationResult::Initialize(result)) => Ok(Ok(result)),
        Ok(_) => Err(StateError(
            "initialize response body was mistyped".to_string(),
        )),
        Err(error) => Ok(Err(error)),
    }
}

fn expect_thread_opened(
    result: Result<OperationResult, RpcError>,
) -> Result<Result<ThreadOpenResult, RpcError>, StateError> {
    match result {
        Ok(OperationResult::ThreadOpened(result)) => Ok(Ok(result)),
        Ok(_) => Err(StateError(
            "thread-open response body was mistyped".to_string(),
        )),
        Err(error) => Ok(Err(error)),
    }
}

fn expect_turn_started(
    result: Result<OperationResult, RpcError>,
) -> Result<Result<TurnStartResult, RpcError>, StateError> {
    match result {
        Ok(OperationResult::TurnStarted(result)) => Ok(Ok(result)),
        Ok(_) => Err(StateError(
            "turn/start response body was mistyped".to_string(),
        )),
        Err(error) => Ok(Err(error)),
    }
}

fn expect_interrupted(
    result: Result<OperationResult, RpcError>,
) -> Result<Result<(), RpcError>, StateError> {
    match result {
        Ok(OperationResult::TurnInterrupted) => Ok(Ok(())),
        Ok(_) => Err(StateError(
            "turn/interrupt response body was mistyped".to_string(),
        )),
        Err(error) => Ok(Err(error)),
    }
}

fn retained_steer_input(steer: PendingSteer) -> String {
    match steer {
        PendingSteer::Response { input }
        | PendingSteer::NoActiveTurn { input, .. }
        | PendingSteer::ActiveTurnNotSteerable { input }
        | PendingSteer::ReplayAfterInterrupt { input } => input,
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

fn version_rejection(problem: String) -> StateError {
    StateError(format!(
        "{problem}; version {MINIMUM_VERSION} or newer is required"
    ))
}

fn minimum_version() -> Version {
    Version::parse(MINIMUM_VERSION).expect("the supported Codex version is valid")
}
