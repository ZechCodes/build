use std::io::{Cursor, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::connection::{AppServerConnection, ConnectionError};
use super::fixtures::{
    checked_in_fixture, corpus_file_names, harness_context, initialize_result, item_envelope,
    item_envelope_at, selected_choice, spawn_options, supported_user_agent, thread_opened,
    thread_opened_at, thread_opened_with, CHECKED_IN_FIXTURES, CHILD_THREAD_ID, CHILD_TURN_ID,
    EXACT_THREAD_ID, SELECTED_EFFORT, SELECTED_MODEL, THREAD_ID, TURN_ID, WORKTREE_ROOT,
};
use super::limits::AppServerLimits;
use super::policy::{AfterResponse, ServerRequestPolicy};
use super::protocol::{
    ClientNotification, ConnectionEvent, InboundServerRequest, ParentThreadFilter,
    ParentThreadRoute, PendingOperation, RequestId, RoutedServerRequest, RpcError,
    ServerNotification, ServerRequest, ServerResponse, TurnCompletion, CLIENT_NAME,
};
use super::session::CodexAppServerSession;
use super::state::{CodexSessionState, SessionEffect, SessionEvent, StateError, StateTransition};
use super::translator::{
    classify_item, CodexActivityTranslator, ItemClassification, ItemReportKind, SuppressionReason,
    ToolSummaryCategory,
};
use crate::harness::Harness;
use crate::harness::{
    AgentActivity, AgentSession, AgentStatus, ToolOutcome, Turn, TurnChoiceSupport,
};
use crate::models::{AgentProvider, ModelChoice};
use crate::pty::HarnessSpec;

fn limits() -> AppServerLimits {
    AppServerLimits {
        inbound_frame_bytes: 256,
        outbound_frame_bytes: 256,
        pending_requests: 4,
        queued_turns: 2,
        queued_turn_bytes: 12,
        open_items: 2,
        open_item_bytes: 64,
        reconciliation: Duration::from_secs(5),
        ..AppServerLimits::default()
    }
}

fn session_state(resume_id: Option<&str>) -> CodexSessionState {
    CodexSessionState::new(
        PathBuf::from(WORKTREE_ROOT),
        Some(SELECTED_MODEL.to_string()),
        Some(SELECTED_EFFORT.to_string()),
        resume_id.map(str::to_string),
    )
}

fn correlated(operation: PendingOperation, result: Result<Value, RpcError>) -> SessionEvent {
    let result = match result {
        Ok(value) => Ok(operation.decode_result(&value).unwrap()),
        Err(error) => Err(error),
    };
    SessionEvent::Connection(ConnectionEvent::Response { operation, result })
}

fn start_thread() -> PendingOperation {
    PendingOperation::StartThread {
        cwd: WORKTREE_ROOT.to_string(),
        model: Some(SELECTED_MODEL.to_string()),
    }
}

fn initialized_then_opens_thread() -> Vec<SessionEffect> {
    vec![
        SessionEffect::NotifyInitialized,
        SessionEffect::Request(start_thread()),
    ]
}

fn resume_thread() -> PendingOperation {
    PendingOperation::ResumeThread {
        thread_id: EXACT_THREAD_ID.to_string(),
        cwd: WORKTREE_ROOT.to_string(),
        model: Some(SELECTED_MODEL.to_string()),
    }
}

fn start_turn(input: &str) -> PendingOperation {
    PendingOperation::StartTurn {
        thread_id: THREAD_ID.to_string(),
        input: input.to_string(),
        model: Some(SELECTED_MODEL.to_string()),
        effort: Some(SELECTED_EFFORT.to_string()),
    }
}

fn start_turn_with(input: &str, model: Option<&str>, effort: Option<&str>) -> PendingOperation {
    PendingOperation::StartTurn {
        thread_id: THREAD_ID.to_string(),
        input: input.to_string(),
        model: model.map(str::to_string),
        effort: effort.map(str::to_string),
    }
}

fn chosen_turn(input: &str, model: Option<&str>, effort: Option<&str>, revision: u64) -> Turn {
    Turn::with_choice(
        input,
        ModelChoice {
            provider: AgentProvider::CodexAppServer,
            model: model.map(str::to_string),
            effort: effort.map(str::to_string),
        },
        revision,
    )
}

fn steer_turn(input: &str) -> PendingOperation {
    PendingOperation::SteerTurn {
        thread_id: THREAD_ID.to_string(),
        turn_id: TURN_ID.to_string(),
        input: input.to_string(),
    }
}

fn interrupt_turn() -> PendingOperation {
    PendingOperation::InterruptTurn {
        thread_id: THREAD_ID.to_string(),
        turn_id: TURN_ID.to_string(),
    }
}

fn state_awaiting_initialize_response() -> CodexSessionState {
    session_state(None)
        .transition(SessionEvent::Start, Duration::ZERO, limits().state())
        .unwrap()
        .state
}

fn initialize_response(user_agent: &str) -> SessionEvent {
    correlated(
        PendingOperation::Initialize,
        Ok(initialize_result(user_agent)),
    )
}

fn initialize_transition_for(
    resume_id: Option<&str>,
    user_agent: &str,
) -> Result<StateTransition, StateError> {
    session_state(resume_id)
        .transition(SessionEvent::Start, Duration::ZERO, limits().state())
        .unwrap()
        .state
        .transition(
            initialize_response(user_agent),
            Duration::ZERO,
            limits().state(),
        )
}

fn initialize_transition(user_agent: &str) -> Result<StateTransition, StateError> {
    initialize_transition_for(None, user_agent)
}

fn turn_completed(turn_id: &str, error: Option<&str>) -> SessionEvent {
    SessionEvent::ObservedCompletion(TurnCompletion::new(
        turn_id.to_string(),
        error.map(str::to_string),
    ))
}

fn advance_to_waiting() -> CodexSessionState {
    initialize_transition(&supported_user_agent())
        .unwrap()
        .state
        .transition(
            correlated(
                start_thread(),
                Ok(thread_opened(THREAD_ID, Some(SELECTED_EFFORT))),
            ),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state
}

fn decode(
    connection: &AppServerConnection,
    message: Value,
) -> Result<ConnectionEvent, ConnectionError> {
    let mut line = serde_json::to_vec(&message).unwrap();
    line.push(b'\n');
    connection
        .read_event(&mut Cursor::new(line))
        .map(|event| event.expect("a complete frame yields one event"))
}

fn request_one(connection: &AppServerConnection, operation: PendingOperation) -> RequestId {
    connection.request(operation).unwrap();
    let [request_id] = connection.pending_ids()[..] else {
        panic!("one correlated request is pending");
    };
    request_id
}

#[test]
fn correlation_resolves_out_of_order_to_typed_operations() {
    let connection = AppServerConnection::memory(limits().connection());
    connection.request(PendingOperation::Initialize).unwrap();
    connection.request(start_thread()).unwrap();
    let [first, second] = connection.pending_ids()[..] else {
        panic!("two correlated requests are pending");
    };

    let second_event = decode(
        &connection,
        json!({"id":second,"result":thread_opened("t", None)}),
    )
    .unwrap();
    let first_event = decode(
        &connection,
        json!({"id":first,"result":initialize_result(&supported_user_agent())}),
    )
    .unwrap();

    assert!(matches!(
        second_event,
        ConnectionEvent::Response {
            operation: PendingOperation::StartThread { .. },
            ..
        }
    ));
    assert!(matches!(
        first_event,
        ConnectionEvent::Response {
            operation: PendingOperation::Initialize,
            ..
        }
    ));
}

#[test]
fn malformed_and_unknown_responses_fail_without_stealing_another_request() {
    for response in [
        json!({"id":99,"result":{}}),
        json!({"id":1,"result":{},"error":{"code":-1,"message":"both"}}),
        json!({"id":1}),
    ] {
        let connection = AppServerConnection::memory(limits().connection());
        connection.request(PendingOperation::Initialize).unwrap();
        assert!(decode(&connection, response).is_err());
    }
}

#[test]
fn a_response_body_that_does_not_match_its_operation_fails_the_connection() {
    for (operation, mistyped_result) in [
        (start_thread(), json!({"turn":{"id":"t"}})),
        (start_turn("go"), json!({"thread":{"id":"t"}})),
        (steer_turn("more"), json!({})),
        (interrupt_turn(), json!([])),
        (PendingOperation::Initialize, json!([])),
    ] {
        let connection = AppServerConnection::memory(limits().connection());
        let request_id = request_one(&connection, operation.clone());

        let error = decode(
            &connection,
            json!({"id":request_id,"result":mistyped_result}),
        )
        .expect_err("a mistyped body cannot resolve the operation");

        let ConnectionError::Protocol(message) = &error else {
            panic!("expected a protocol violation, got {error:?}");
        };
        assert!(message.contains(operation.method()), "{message}");
        assert!(message.contains("response has the wrong body"), "{message}");
        assert_eq!(connection.pending_count(), 0);
    }
}

#[test]
fn a_second_response_on_a_resolved_id_is_unknown() {
    let connection = AppServerConnection::memory(limits().connection());
    let request_id = request_one(&connection, PendingOperation::Initialize);
    let response = json!({"id":request_id,"result":initialize_result(&supported_user_agent())});

    decode(&connection, response.clone()).unwrap();
    let error = decode(&connection, response)
        .expect_err("a resolved id no longer correlates to an operation");

    assert!(error.to_string().contains("unknown response id"), "{error}");
    assert_eq!(connection.pending_count(), 0);
}

#[test]
fn pending_overflow_and_failed_write_leave_correlation_unchanged() {
    let mut small = limits();
    small.pending_requests = 1;
    let connection = AppServerConnection::memory(small.connection());
    connection.request(PendingOperation::Initialize).unwrap();
    assert!(connection.request(start_thread()).is_err());
    assert_eq!(connection.pending_count(), 1);

    let failed = AppServerConnection::failing_writer(limits().connection());
    assert!(failed.request(PendingOperation::Initialize).is_err());
    assert_eq!(failed.pending_count(), 0);
    assert!(decode(
        &failed,
        json!({"id":1,"result":initialize_result(&supported_user_agent())})
    )
    .unwrap_err()
    .to_string()
    .contains("unknown response id 1"));

    let exhausted = AppServerConnection::memory(limits().connection());
    exhausted.set_next_id(u64::MAX);
    assert!(exhausted.request(PendingOperation::Initialize).is_err());
    assert_eq!(exhausted.pending_count(), 0);
}

#[test]
fn bound_turn_request_needs_no_mutable_connection_context() {
    let connection = AppServerConnection::memory(limits().connection());

    assert!(connection.request(start_turn("too early")).is_ok());
    assert_eq!(connection.pending_count(), 1);
}

#[derive(Clone)]
struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

impl Write for CapturedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: request_shapes_put_model_and_effort_only_where_the_protocol_accepts_them is at 20, threshold 15 — bring it under, then remove
fn request_shapes_put_model_and_effort_only_where_the_protocol_accepts_them() {
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let connection = AppServerConnection::new(
        Box::new(CapturedWriter(Arc::clone(&bytes))),
        AppServerLimits::default().connection(),
    );
    connection.request(PendingOperation::Initialize).unwrap();
    connection.request(resume_thread()).unwrap();
    connection.request(start_turn("start")).unwrap();
    connection.request(steer_turn("steer")).unwrap();
    connection.request(interrupt_turn()).unwrap();
    connection.notify(ClientNotification::Initialized).unwrap();

    let written = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    let frames = written
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(frames[0]["method"], "initialize");
    assert_eq!(frames[0]["params"]["clientInfo"]["name"], CLIENT_NAME);
    assert!(frames[0].get("jsonrpc").is_none());
    assert!(frames[0]["params"]["capabilities"]
        .as_object()
        .unwrap()
        .is_empty());
    assert_eq!(frames[1]["method"], "thread/resume");
    assert_eq!(frames[1]["params"]["threadId"], EXACT_THREAD_ID);
    assert_eq!(frames[1]["params"]["model"], SELECTED_MODEL);
    assert!(frames[1]["params"].get("effort").is_none());
    assert_eq!(frames[1]["params"]["approvalPolicy"], "never");
    assert_eq!(frames[1]["params"]["sandbox"], "danger-full-access");
    assert_eq!(frames[1]["params"]["excludeTurns"], true);
    assert_eq!(frames[2]["params"]["model"], SELECTED_MODEL);
    assert_eq!(frames[2]["params"]["effort"], SELECTED_EFFORT);
    assert!(frames[3]["params"].get("model").is_none());
    assert!(frames[3]["params"].get("effort").is_none());
    assert_eq!(frames[3]["params"]["expectedTurnId"], TURN_ID);
    assert_eq!(frames[4]["method"], "turn/interrupt");
    assert_eq!(frames[4]["params"]["threadId"], THREAD_ID);
    assert_eq!(frames[4]["params"]["turnId"], TURN_ID);
    assert_eq!(frames[5], json!({"method":"initialized"}));
}

#[test]
fn thread_resume_excludes_turn_history_without_changing_thread_start() {
    let encode = |operation: PendingOperation| {
        let mut bytes = Vec::new();
        operation.serialize_request(7, &mut bytes).unwrap();
        serde_json::from_slice::<Value>(&bytes).unwrap()
    };

    assert_eq!(
        encode(start_thread()),
        json!({
            "id": 7,
            "method": "thread/start",
            "params": {
                "cwd": WORKTREE_ROOT,
                "model": SELECTED_MODEL,
                "approvalPolicy": "never",
                "sandbox": "danger-full-access"
            }
        })
    );
    assert_eq!(
        encode(resume_thread()),
        json!({
            "id": 7,
            "method": "thread/resume",
            "params": {
                "threadId": EXACT_THREAD_ID,
                "cwd": WORKTREE_ROOT,
                "model": SELECTED_MODEL,
                "approvalPolicy": "never",
                "sandbox": "danger-full-access",
                "excludeTurns": true
            }
        })
    );
}

#[test]
fn active_session_completes_from_a_frame_larger_than_one_megabyte() {
    let connection = AppServerConnection::memory(AppServerLimits::default().connection());
    let oversized_history = "x".repeat(1024 * 1024);
    let mut input = serde_json::to_vec(&json!({
        "method": "turn/completed",
        "params": {
            "threadId": THREAD_ID,
            "turn": {
                "id": TURN_ID,
                "items": [{
                    "type": "agentMessage",
                    "id": "large-agent-message",
                    "text": oversized_history,
                    "phase": "final_answer"
                }],
                "itemsView": "summary",
                "status": "completed",
                "error": null,
                "startedAt": 0,
                "completedAt": 1,
                "durationMs": 1
            }
        }
    }))
    .unwrap();
    assert!(input.len() > 1024 * 1024);
    input.push(b'\n');

    let event = connection
        .read_event(&mut Cursor::new(input))
        .unwrap()
        .expect("the large completion frame is read");
    let ConnectionEvent::Notification(inbound) = event else {
        panic!("expected a completion notification");
    };
    let ServerNotification::TurnCompleted { completion, .. } =
        ServerNotification::decode(&inbound.method, inbound.params).unwrap()
    else {
        panic!("expected a decoded turn completion");
    };
    let completed = working_state()
        .transition(
            SessionEvent::ObservedCompletion(completion),
            Duration::from_secs(1),
            AppServerLimits::default().state(),
        )
        .unwrap();

    assert_eq!(completed.state.live_status(), Some(AgentStatus::Waiting));
}

#[test]
fn spawned_session_stays_live_after_a_large_completion_and_completes_the_next_turn() {
    let root = tempfile::tempdir().unwrap();
    let large_completion_path = root.path().join("large-completion.jsonl");
    let large_completion = json!({
        "method": "turn/completed",
        "params": {
            "threadId": THREAD_ID,
            "turn": {
                "id": TURN_ID,
                "items": [{
                    "type": "agentMessage",
                    "id": "large-agent-message",
                    "text": "x".repeat(1024 * 1024),
                    "phase": "final_answer"
                }],
                "itemsView": "summary",
                "status": "completed",
                "error": null,
                "startedAt": 0,
                "completedAt": 1,
                "durationMs": 1
            }
        }
    });
    let mut large_completion_bytes = serde_json::to_vec(&large_completion).unwrap();
    assert!(large_completion_bytes.len() > 1024 * 1024);
    large_completion_bytes.push(b'\n');
    std::fs::write(&large_completion_path, large_completion_bytes).unwrap();

    let cwd = root.path().display().to_string();
    let initialize = json!({"id":1,"result":initialize_result(&supported_user_agent())});
    let opened = json!({"id":2,"result":thread_opened_at(&cwd, THREAD_ID, Some(SELECTED_EFFORT))});
    let first_started = json!({"id":3,"result":{"turn":{"id":TURN_ID}}});
    let second_started = json!({"id":4,"result":{"turn":{"id":"turn-next"}}});
    let second_completed = json!({
        "method":"turn/completed",
        "params":{
            "threadId":THREAD_ID,
            "turn":{"id":"turn-next","items":[],"itemsView":"summary","status":"completed","error":null}
        }
    });
    let script = format!(
        "read initialize; printf '%s\\n' '{initialize}'; read initialized; read thread; printf '%s\\n' '{opened}'; read first; printf '%s\\n' '{first_started}'; sleep 0.05; sed -n '1p' '{}'; read second; printf '%s\\n' '{second_started}'; sleep 0.05; printf '%s\\n' '{second_completed}'; read hold",
        large_completion_path.display()
    );
    let spec = HarnessSpec::new("sh").arg("-c").arg(script);
    let (session, _activity) = CodexAppServerSession::spawn(
        &spec,
        root.path().to_path_buf(),
        selected_choice(),
        None,
        AppServerLimits::default(),
    )
    .unwrap();
    let wait_until = |expectation: &str, condition: &mut dyn FnMut() -> bool| {
        for _ in 0..400 {
            if condition() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the session never {expectation}");
    };

    wait_until("opened its thread", &mut || session.session_id().is_some());
    session.send_turn(&Turn::new("first")).unwrap();
    wait_until("started its first turn", &mut || {
        session.status() == AgentStatus::Working
    });
    wait_until("completed its large first turn", &mut || {
        session.status() == AgentStatus::Waiting
    });
    session.send_turn(&Turn::new("second")).unwrap();
    wait_until("started its second turn", &mut || {
        session.status() == AgentStatus::Working
    });
    wait_until("completed its second turn", &mut || {
        session.status() == AgentStatus::Waiting
    });
    assert!(!session.exited_within(Duration::from_millis(20)));
    session.end();
}

#[test]
fn frame_larger_than_the_default_inbound_limit_reports_the_observed_lower_bound() {
    let configured = AppServerLimits::default();
    let connection = AppServerConnection::memory(configured.connection());
    let mut input = vec![b'x'; configured.inbound_frame_bytes + 1];
    input.push(b'\n');

    assert!(matches!(
        connection.read_event(&mut Cursor::new(input)).unwrap_err(),
        ConnectionError::FrameTooLarge { limit, observed_at_least }
            if limit == configured.inbound_frame_bytes
                && observed_at_least == configured.inbound_frame_bytes + 1
    ));
}

#[test]
fn close_is_idempotent_and_writes_after_close_fail() {
    let connection = AppServerConnection::memory(limits().connection());
    assert!(connection.close().is_ok());
    assert!(connection.close().is_ok());
    assert!(matches!(
        connection
            .notify(ClientNotification::Initialized)
            .unwrap_err(),
        ConnectionError::Closed
    ));
    assert!(matches!(
        connection
            .request(PendingOperation::Initialize)
            .unwrap_err(),
        ConnectionError::Closed
    ));
    assert_eq!(connection.pending_count(), 0);
    assert!(matches!(
        connection
            .respond(ServerResponse::method_not_found(json!(1), "unsupported"))
            .unwrap_err(),
        ConnectionError::Closed
    ));
}

#[test]
fn method_not_found_response_carries_the_json_rpc_code_and_message() {
    let response = ServerResponse::method_not_found(json!(3), "unsupported");
    assert_eq!(response.error_code(), Some(-32601));
    assert_eq!(
        response.to_value(),
        json!({"id":3,"error":{"code":-32601,"message":"unsupported"}})
    );
}

#[test]
fn every_write_path_enforces_the_outbound_frame_limit() {
    let mut tiny = limits();
    tiny.outbound_frame_bytes = 16;
    let capped = AppServerConnection::memory(tiny.connection());
    let oversized_response =
        || ServerResponse::method_not_found(json!("x".repeat(64)), "method not found");
    assert!(matches!(
        capped.request(PendingOperation::Initialize).unwrap_err(),
        ConnectionError::FrameTooLarge { limit: 16, .. }
    ));
    assert_eq!(capped.pending_count(), 0);
    assert!(matches!(
        capped.notify(ClientNotification::Initialized).unwrap_err(),
        ConnectionError::FrameTooLarge { limit: 16, .. }
    ));
    assert!(matches!(
        capped.respond(oversized_response()).unwrap_err(),
        ConnectionError::FrameTooLarge { limit: 16, .. }
    ));
    let roomy = AppServerConnection::memory(AppServerLimits::default().connection());
    assert!(roomy.respond(oversized_response()).is_ok());
}

#[test]
fn app_server_eof_ends_the_session_with_a_close_effect() {
    let transition = advance_to_waiting()
        .transition(SessionEvent::Eof, Duration::ZERO, limits().state())
        .unwrap();
    assert_eq!(transition.effects, vec![SessionEffect::Close]);
    assert_eq!(transition.state.live_status(), None);
}

#[test]
fn initialize_is_first_and_a_turn_waits_for_readiness() {
    let state = session_state(None);
    let started = state
        .transition(SessionEvent::Start, Duration::ZERO, limits().state())
        .unwrap();
    assert_eq!(
        started.effects,
        vec![SessionEffect::Request(PendingOperation::Initialize)]
    );
    let queued = started
        .state
        .transition(
            SessionEvent::SendTurn("first".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap();
    assert!(queued.effects.is_empty());
    assert_eq!(queued.state.live_status(), Some(AgentStatus::Starting));
}

#[test]
fn initialize_success_sends_initialized_once_then_opens_the_thread() {
    let initialized = initialize_transition(&supported_user_agent()).unwrap();
    assert_eq!(initialized.effects, initialized_then_opens_thread());
    let repeated = initialized
        .state
        .transition(
            initialize_response(&supported_user_agent()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap_err();
    assert!(repeated.to_string().contains("out of order"));
}

#[test]
fn initialize_error_fails_the_session() {
    let error = state_awaiting_initialize_response()
        .transition(
            correlated(
                PendingOperation::Initialize,
                Err(RpcError::new(-32603, "initialize refused")),
            ),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap_err()
        .to_string();
    assert!(error.contains("-32603"), "{error}");
    assert!(error.contains("initialize refused"), "{error}");
}

#[test]
fn a_below_floor_user_agent_fails_without_asking_the_probe() {
    let error = initialize_transition(&format!("{CLIENT_NAME}/0.152.9"))
        .expect_err("a below-floor user agent fails startup instead of asking the probe")
        .to_string();
    assert!(error.contains("0.152.9"), "{error}");
    assert!(error.contains("0.153.0"), "{error}");
}

fn initialize_effects_for_user_agent(user_agent: &str) -> Vec<SessionEffect> {
    initialize_transition(user_agent)
        .unwrap_or_else(|error| panic!("{user_agent:?}: {error}"))
        .effects
}

#[test]
fn initialize_version_floor_uses_only_the_leading_matching_component() {
    for passing in [
        format!("{CLIENT_NAME}/0.153.0"),
        format!("{CLIENT_NAME}/0.154.1 (0.1.0)"),
    ] {
        assert_eq!(
            initialize_effects_for_user_agent(&passing),
            initialized_then_opens_thread(),
            "{passing}"
        );
    }
    for probe_needed in [
        format!("other/0.153.0 {CLIENT_NAME}/9.0.0"),
        format!("{CLIENT_NAME}/not-a-version 0.200.0"),
        format!("0.153.0 {CLIENT_NAME}/0.153.0"),
        String::new(),
    ] {
        assert_eq!(
            initialize_effects_for_user_agent(&probe_needed),
            vec![SessionEffect::RequireVersionEvidence],
            "{probe_needed}"
        );
    }
}

#[test]
fn version_probe_evidence_accepts_the_floor_and_preserves_every_failure() {
    let awaiting = initialize_transition("unparseable").unwrap();
    assert_eq!(
        awaiting.effects,
        vec![SessionEffect::RequireVersionEvidence]
    );
    let accepted = awaiting.state.clone().transition(
        SessionEvent::VersionEvidence(Ok("codex-cli 0.153.0".to_string())),
        Duration::ZERO,
        limits().state(),
    );
    assert_eq!(accepted.unwrap().effects, initialized_then_opens_thread());

    let below_floor = awaiting
        .state
        .clone()
        .transition(
            SessionEvent::VersionEvidence(Ok("codex-cli 0.152.9".to_string())),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap_err()
        .to_string();
    assert!(below_floor.contains("0.152.9"), "{below_floor}");
    assert!(below_floor.contains("0.153.0"), "{below_floor}");

    let unparsable = awaiting
        .state
        .transition(
            SessionEvent::VersionEvidence(Ok("not-codex 1.0".to_string())),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap_err()
        .to_string();
    assert!(unparsable.contains("not-codex 1.0"), "{unparsable}");
    assert!(unparsable.contains("0.153.0"), "{unparsable}");

    let failure = state_awaiting_initialize_response()
        .transition(
            correlated(PendingOperation::Initialize, Ok(json!({}))),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state
        .transition(
            SessionEvent::VersionEvidence(Err("exact probe failure".to_string())),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap_err()
        .to_string();
    assert!(failure.contains("exact probe failure"), "{failure}");
    assert!(failure.contains("0.153.0"), "{failure}");
}

#[test]
fn concurrent_version_probes_share_one_cached_result() {
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("codex-probe");
    let calls = root.path().join("calls");
    std::fs::write(
        &binary,
        format!(
            "#!/bin/sh\nprintf 'called\\n' >> '{}'\nsleep 0.05\nprintf 'codex-cli 0.153.0\\n'\n",
            calls.display()
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&binary).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&binary, permissions).unwrap();

    let barrier = Arc::new(Barrier::new(8));
    let probes = (0..8)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            let binary = binary.clone();
            std::thread::spawn(move || {
                barrier.wait();
                super::CodexAppServerHarness::probe_version(&binary)
            })
        })
        .collect::<Vec<_>>();
    for probe in probes {
        assert_eq!(probe.join().unwrap().unwrap(), "codex-cli 0.153.0");
    }
    assert_eq!(std::fs::read_to_string(calls).unwrap().lines().count(), 1);

    let missing = root.path().join("missing-codex");
    let first = super::CodexAppServerHarness::probe_version(&missing).unwrap_err();
    let second = super::CodexAppServerHarness::probe_version(&missing).unwrap_err();
    assert_eq!(first, second);
}

#[test]
fn exact_resume_id_selects_resume_and_fresh_never_guesses() {
    let fresh = session_state(None);
    assert!(fresh.resume_id().is_none());
    let fresh_open = initialize_transition(&supported_user_agent()).unwrap();
    assert_eq!(fresh_open.effects, initialized_then_opens_thread());

    let resumed = session_state(Some(EXACT_THREAD_ID));
    assert_eq!(resumed.resume_id(), Some(EXACT_THREAD_ID));
    let resumed_open =
        initialize_transition_for(Some(EXACT_THREAD_ID), &supported_user_agent()).unwrap();
    assert_eq!(
        resumed_open.effects,
        vec![
            SessionEffect::NotifyInitialized,
            SessionEffect::Request(resume_thread()),
        ]
    );
}

#[test]
fn thread_open_response_operation_must_match_the_persisted_resume_id() {
    let opened = thread_opened(THREAD_ID, None);

    let resumed = initialize_transition_for(Some(EXACT_THREAD_ID), &supported_user_agent())
        .unwrap()
        .state;
    let start_answered_a_resume = resumed
        .transition(
            correlated(start_thread(), Ok(opened.clone())),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap_err();
    assert!(
        start_answered_a_resume
            .to_string()
            .contains("wrong thread-open response operation"),
        "{start_answered_a_resume}"
    );

    let fresh = initialize_transition(&supported_user_agent())
        .unwrap()
        .state;
    let resume_answered_a_start = fresh
        .transition(
            correlated(resume_thread(), Ok(opened)),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap_err();
    assert!(
        resume_answered_a_start
            .to_string()
            .contains("wrong thread-open response operation"),
        "{resume_answered_a_start}"
    );
}

#[test]
fn thread_open_error_after_started_notification_fails() {
    let announced = initialize_transition(&supported_user_agent())
        .unwrap()
        .state
        .transition(
            SessionEvent::ThreadStarted(THREAD_ID.to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let error = announced
        .transition(
            correlated(start_thread(), Err(RpcError::new(-32000, "open refused"))),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("thread was announced and then open failed"),
        "{error}"
    );
    assert!(error.to_string().contains("open refused"), "{error}");
}

#[test]
fn thread_open_error_without_notification_fails() {
    let opening = initialize_transition(&supported_user_agent())
        .unwrap()
        .state;
    let error = opening
        .transition(
            correlated(start_thread(), Err(RpcError::new(-32000, "open refused"))),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("open refused"), "{error}");
    assert!(
        !error.to_string().contains("thread was announced"),
        "{error}"
    );
}

#[test]
fn thread_open_settings_must_match_the_requested_session() {
    let mismatches = [
        (json!({"cwd":"/tmp/elsewhere"}), "thread cwd mismatch"),
        (
            json!({"approvalPolicy":"onRequest"}),
            "approvalPolicy=never",
        ),
        (
            json!({"sandbox":{"type":"workspaceWrite"}}),
            "danger-full-access sandbox",
        ),
        (json!({"model":"gpt-5.6-mini"}), "Codex opened model"),
    ];
    for (overrides, complaint) in mismatches {
        let error = initialize_transition(&supported_user_agent())
            .unwrap()
            .state
            .transition(
                correlated(
                    start_thread(),
                    Ok(thread_opened_with(THREAD_ID, None, overrides.clone())),
                ),
                Duration::ZERO,
                limits().state(),
            )
            .expect_err("a thread opened with the wrong settings fails startup")
            .to_string();
        assert!(error.contains(complaint), "{overrides}: {error}");
    }
}

#[test]
fn thread_notification_and_response_orders_converge_and_ids_must_match() {
    let opening = initialize_transition(&supported_user_agent())
        .unwrap()
        .state;
    let notified = opening
        .transition(
            SessionEvent::ThreadStarted(THREAD_ID.to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let matching = notified
        .transition(
            correlated(start_thread(), Ok(thread_opened(THREAD_ID, None))),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap();
    assert_eq!(matching.state.live_status(), Some(AgentStatus::Waiting));

    let mismatch = opening
        .transition(
            SessionEvent::ThreadStarted("other".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state
        .transition(
            correlated(start_thread(), Ok(thread_opened(THREAD_ID, None))),
            Duration::ZERO,
            limits().state(),
        );
    assert!(mismatch.is_err());
}

#[test]
fn thread_response_before_notification_is_ready_and_the_duplicate_is_inert() {
    let waiting = advance_to_waiting();
    assert_eq!(waiting.live_status(), Some(AgentStatus::Waiting));
    let duplicate = waiting
        .transition(
            SessionEvent::ThreadStarted(THREAD_ID.to_string()),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap();
    assert!(duplicate.effects.is_empty());
    assert_eq!(duplicate.state.session_id().as_deref(), Some(THREAD_ID));
    assert_eq!(
        duplicate.state.active_model().as_deref(),
        Some(SELECTED_MODEL)
    );
}

#[test]
fn starting_turn_completion_before_response_never_resurrects_working() {
    let state = advance_to_waiting();
    let starting = state
        .transition(
            SessionEvent::SendTurn("go".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let completed = starting
        .transition(
            turn_completed(TURN_ID, None),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap()
        .state;
    assert_eq!(completed.live_status(), Some(AgentStatus::Working));
    let settled = completed
        .transition(
            correlated(start_turn("go"), Ok(json!({"turn":{"id":TURN_ID}}))),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap();
    assert_eq!(settled.state.live_status(), Some(AgentStatus::Waiting));
}

#[test]
fn conflicting_duplicate_completion_is_rejected() {
    let starting = advance_to_waiting()
        .transition(
            SessionEvent::SendTurn("go".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let completed = starting
        .transition(
            turn_completed(TURN_ID, Some("first failure")),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap()
        .state;

    let normalized_duplicate = completed
        .transition(
            turn_completed(TURN_ID, Some("  first   failure\n")),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap();
    assert!(normalized_duplicate.effects.is_empty());

    assert!(normalized_duplicate
        .state
        .transition(
            SessionEvent::ObservedCompletion(TurnCompletion::observed(
                TURN_ID.to_string(),
                "interrupted".to_string(),
                Some("first failure".to_string()),
            )),
            Duration::from_secs(3),
            limits().state(),
        )
        .is_err());

    assert!(normalized_duplicate
        .state
        .transition(
            turn_completed(TURN_ID, Some("different failure")),
            Duration::from_secs(4),
            limits().state(),
        )
        .is_err());
}

#[test]
fn completion_before_start_response_applies_accepted_turn_facts() {
    let waiting = initialize_transition(&supported_user_agent())
        .unwrap()
        .state
        .transition(
            correlated(start_thread(), Ok(thread_opened(THREAD_ID, Some("low")))),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    assert_eq!(waiting.active_effort().as_deref(), Some("low"));
    let starting = waiting
        .transition(
            SessionEvent::SendTurn("go".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let completed = starting
        .transition(
            turn_completed(TURN_ID, None),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap()
        .state;
    let settled = completed
        .transition(
            correlated(start_turn("go"), Ok(json!({"turn":{"id":TURN_ID}}))),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap()
        .state;

    assert_eq!(settled.active_effort().as_deref(), Some(SELECTED_EFFORT));
}

#[test]
fn observed_turn_then_start_error_and_wrong_ids_fail() {
    let starting = advance_to_waiting()
        .transition(
            SessionEvent::SendTurn("go".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state
        .transition(
            SessionEvent::TurnStarted(TURN_ID.to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    assert!(starting
        .transition(
            correlated(start_turn("go"), Err(RpcError::new(-32000, "rejected"))),
            Duration::ZERO,
            limits().state()
        )
        .is_err());

    let starting = advance_to_waiting()
        .transition(
            SessionEvent::SendTurn("go".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    assert!(starting
        .transition(
            correlated(start_turn("go"), Ok(json!({"turn":{"id":"turn-2"}}))),
            Duration::ZERO,
            limits().state()
        )
        .unwrap()
        .state
        .transition(
            SessionEvent::TurnStarted(TURN_ID.to_string()),
            Duration::ZERO,
            limits().state()
        )
        .is_err());
}

#[test]
fn two_sends_during_start_issue_only_one_turn_start() {
    let state = advance_to_waiting();
    let first = state
        .transition(
            SessionEvent::SendTurn("first".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap();
    assert_eq!(first.effects.len(), 1);
    let second = first
        .state
        .transition(
            SessionEvent::SendTurn("second".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap();
    assert!(second.effects.is_empty());
}

#[test]
fn successive_frozen_choices_are_applied_to_their_own_turn_starts() {
    let first_choice = chosen_turn("first", Some("gpt-5.6-terra"), Some("low"), 1);
    let first = advance_to_waiting()
        .transition(
            SessionEvent::SendChosenTurn(first_choice),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap();
    assert!(matches!(
        first.effects.as_slice(),
        [SessionEffect::Request(PendingOperation::StartTurn { input, model, effort, .. })]
            if input == "first"
                && model.as_deref() == Some("gpt-5.6-terra")
                && effort.as_deref() == Some("low")
    ));

    let waiting = first
        .state
        .transition(
            correlated(
                start_turn_with("first", Some("gpt-5.6-terra"), Some("low")),
                Ok(json!({"turn":{"id":TURN_ID}})),
            ),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap()
        .state
        .transition(
            turn_completed(TURN_ID, None),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap()
        .state;
    assert_eq!(waiting.active_model().as_deref(), Some("gpt-5.6-terra"));
    assert_eq!(waiting.active_effort().as_deref(), Some("low"));

    let second = waiting
        .transition(
            SessionEvent::SendChosenTurn(chosen_turn(
                "second",
                Some("gpt-6-astra"),
                Some("xhigh"),
                2,
            )),
            Duration::from_secs(3),
            limits().state(),
        )
        .unwrap();
    assert!(matches!(
        second.effects.as_slice(),
        [SessionEffect::Request(PendingOperation::StartTurn { input, model, effort, .. })]
            if input == "second"
                && model.as_deref() == Some("gpt-6-astra")
                && effort.as_deref() == Some("xhigh")
    ));
    assert_eq!(
        second.state.active_model().as_deref(),
        Some("gpt-5.6-terra"),
        "requested settings do not masquerade as active before Codex accepts turn/start"
    );
    let accepted = second
        .state
        .transition(
            correlated(
                start_turn_with("second", Some("gpt-6-astra"), Some("xhigh")),
                Ok(json!({"turn":{"id":"turn-2"}})),
            ),
            Duration::from_secs(4),
            limits().state(),
        )
        .unwrap()
        .state;
    assert_eq!(accepted.active_model().as_deref(), Some("gpt-6-astra"));
    assert_eq!(accepted.active_effort().as_deref(), Some("xhigh"));
}

#[test]
fn clearing_a_sticky_choice_requires_a_safe_default_session_restart() {
    let explicit_b = ModelChoice {
        provider: AgentProvider::CodexAppServer,
        model: Some("gpt-5.6-terra".to_string()),
        effort: Some("low".to_string()),
    };
    let state = advance_to_waiting();
    assert_eq!(
        state.turn_choice_support(&explicit_b),
        TurnChoiceSupport::Native
    );
    let changed = state
        .transition(
            SessionEvent::SendChosenTurn(Turn::with_choice("use b", explicit_b, 1)),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    assert_eq!(
        changed.turn_choice_support(&ModelChoice {
            provider: AgentProvider::CodexAppServer,
            model: None,
            effort: None,
        }),
        TurnChoiceSupport::RestartRequired,
        "a session started at explicit A and moved to B cannot discover configured Default"
    );
}

#[test]
fn fresh_default_session_projects_the_configured_model_and_effort() {
    let default_choice = ModelChoice {
        provider: AgentProvider::CodexAppServer,
        model: None,
        effort: None,
    };
    let opening = CodexSessionState::new(PathBuf::from(WORKTREE_ROOT), None, None, None)
        .transition(SessionEvent::Start, Duration::ZERO, limits().state())
        .unwrap()
        .state
        .transition(
            initialize_response(&supported_user_agent()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let waiting = opening
        .transition(
            correlated(
                PendingOperation::StartThread {
                    cwd: WORKTREE_ROOT.to_string(),
                    model: None,
                },
                Ok(thread_opened_with(
                    THREAD_ID,
                    Some("medium"),
                    json!({"model":"configured-default"}),
                )),
            ),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    assert_eq!(
        waiting.active_model().as_deref(),
        Some("configured-default")
    );
    assert_eq!(waiting.active_effort().as_deref(), Some("medium"));
    assert_eq!(
        waiting.turn_choice_support(&default_choice),
        TurnChoiceSupport::Native
    );

    let starting = waiting
        .transition(
            SessionEvent::SendChosenTurn(Turn::with_choice(
                "use configured default",
                default_choice,
                3,
            )),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap();
    assert!(matches!(
        starting.effects.as_slice(),
        [SessionEffect::Request(PendingOperation::StartTurn {
            model: None,
            effort: None,
            ..
        })]
    ));
    let accepted = starting
        .state
        .transition(
            correlated(
                start_turn_with("use configured default", None, None),
                Ok(json!({"turn":{"id":TURN_ID}})),
            ),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap()
        .state;
    assert_eq!(
        accepted.active_model().as_deref(),
        Some("configured-default")
    );
    assert_eq!(accepted.active_effort().as_deref(), Some("medium"));
}

#[test]
fn failed_starting_turn_is_interrupted_after_its_id_is_confirmed() {
    let starting = advance_to_waiting()
        .transition(
            SessionEvent::SendTurn("go".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let failed = starting
        .transition(
            SessionEvent::FailTurn("unsupported callback".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap();
    assert!(!failed.effects.iter().any(|effect| matches!(
        effect,
        SessionEffect::Request(PendingOperation::InterruptTurn { .. })
    )));

    let confirmed = failed
        .state
        .transition(
            correlated(start_turn("go"), Ok(json!({"turn":{"id":TURN_ID}}))),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap();
    assert!(confirmed.effects.iter().any(|effect| matches!(
        effect,
        SessionEffect::Request(PendingOperation::InterruptTurn { turn_id, .. })
            if turn_id == TURN_ID
    )));
    assert!(!confirmed.state.can_interrupt());
}

fn working_state() -> CodexSessionState {
    let starting = advance_to_waiting()
        .transition(
            SessionEvent::SendTurn("go".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    starting
        .transition(
            correlated(start_turn("go"), Ok(json!({"turn":{"id":TURN_ID}}))),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state
}

#[test]
fn steers_are_serialized_and_success_releases_input_in_order() {
    let first = working_state()
        .transition(
            SessionEvent::SendTurn("one".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap();
    assert!(
        matches!(first.effects.as_slice(), [SessionEffect::Request(PendingOperation::SteerTurn { input, .. })] if input == "one")
    );
    let queued = first
        .state
        .transition(
            SessionEvent::SendTurn("two".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap();
    assert!(queued.effects.is_empty());
    let released = queued
        .state
        .transition(
            correlated(steer_turn("one"), Ok(json!({"turnId":TURN_ID}))),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap();
    assert!(
        matches!(released.effects.as_slice(), [SessionEffect::Request(PendingOperation::SteerTurn { input, .. })] if input == "two")
    );
}

#[test]
fn queued_frozen_turns_keep_the_choice_snapshot_they_arrived_with() {
    let first = chosen_turn("one", Some("gpt-5.6-terra"), Some("low"), 10);
    let second = chosen_turn("two", Some("gpt-6-astra"), Some("xhigh"), 11);
    let queued = working_state()
        .transition(
            SessionEvent::SendChosenTurn(first),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap();
    assert!(queued.effects.is_empty(), "a chosen turn is never a steer");
    let queued = queued
        .state
        .transition(
            SessionEvent::SendChosenTurn(second),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap();
    assert!(queued.effects.is_empty());

    let released = queued
        .state
        .transition(
            turn_completed(TURN_ID, None),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap();
    assert!(matches!(
        released.effects.as_slice(),
        [SessionEffect::CloseTurn(turn_id), SessionEffect::Request(PendingOperation::StartTurn { input, model, effort, .. })]
            if turn_id == TURN_ID
                && input == "one"
                && model.as_deref() == Some("gpt-5.6-terra")
                && effort.as_deref() == Some("low")
    ));
}

#[test]
fn a_steer_success_naming_another_turn_fails_the_session() {
    let steered = working_state()
        .transition(
            SessionEvent::SendTurn("one".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let mismatched_steer = || correlated(steer_turn("one"), Ok(json!({"turnId":"turn-9"})));
    let mismatch = steered
        .transition(mismatched_steer(), Duration::ZERO, limits().state())
        .expect_err("a steer success naming another turn desynchronizes the session");
    let message = mismatch.to_string();
    assert!(
        message.contains("turn/steer response id mismatch"),
        "{message}"
    );
    assert!(message.contains(TURN_ID), "{message}");
    assert!(message.contains("turn-9"), "{message}");

    let queued = steered
        .transition(
            SessionEvent::SendTurn("two".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap();
    assert!(queued.effects.is_empty());
    let queued_message = queued
        .state
        .transition(mismatched_steer(), Duration::ZERO, limits().state())
        .expect_err("a failed steer never releases the queued input")
        .to_string();
    assert!(
        queued_message.contains("turn/steer response id mismatch"),
        "{queued_message}"
    );
}

#[test]
fn steer_completion_race_replays_retained_input_exactly_once() {
    let pending = working_state()
        .transition(
            SessionEvent::SendTurn("next".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let completed = pending
        .transition(
            turn_completed(TURN_ID, None),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap()
        .state;
    let replayed = completed
        .transition(
            correlated(
                steer_turn("next"),
                Err(RpcError::new(-32600, "no active turn")),
            ),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap();
    assert!(
        matches!(replayed.effects.as_slice(), [SessionEffect::Request(PendingOperation::StartTurn { input, .. })] if input == "next")
    );

    let pending = working_state()
        .transition(
            SessionEvent::SendTurn("next".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let provisional = pending
        .transition(
            correlated(
                steer_turn("next"),
                Err(RpcError::new(-32600, "no active turn")),
            ),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap();
    assert!(provisional.effects.is_empty());
    let replayed = provisional
        .state
        .transition(
            turn_completed(TURN_ID, None),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap();
    assert!(replayed.effects.iter().any(|effect| matches!(effect, SessionEffect::Request(PendingOperation::StartTurn { input, .. }) if input == "next")));
}

#[test]
fn successful_steer_after_completion_never_replays_input() {
    let pending = working_state()
        .transition(
            SessionEvent::SendTurn("accepted".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let completed = pending
        .transition(
            turn_completed(TURN_ID, None),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap()
        .state;
    let settled = completed
        .transition(
            correlated(steer_turn("accepted"), Ok(json!({"turnId":TURN_ID}))),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap();
    assert_eq!(settled.state.live_status(), Some(AgentStatus::Waiting));
    assert!(!settled.effects.iter().any(|effect| matches!(
        effect,
        SessionEffect::Request(PendingOperation::StartTurn { .. })
    )));
}

#[test]
fn completed_turn_cannot_be_interrupted_while_a_steer_response_is_pending() {
    let pending = working_state()
        .transition(
            SessionEvent::SendTurn("accepted".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let completed = pending
        .transition(
            turn_completed(TURN_ID, None),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap()
        .state;

    assert!(!completed.can_interrupt());
    assert!(completed
        .transition(
            SessionEvent::Interrupt,
            Duration::from_secs(1),
            limits().state()
        )
        .unwrap()
        .effects
        .is_empty());
}

#[test]
fn steer_and_interrupt_completion_wait_for_both_correlated_responses() {
    let steering = working_state()
        .transition(
            SessionEvent::SendTurn("steer".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let interrupted = steering
        .transition(SessionEvent::Interrupt, Duration::ZERO, limits().state())
        .unwrap()
        .state;
    let completed = interrupted
        .transition(
            turn_completed(TURN_ID, None),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap()
        .state;
    let steer_settled = completed
        .transition(
            correlated(steer_turn("steer"), Ok(json!({"turnId":TURN_ID}))),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap();
    assert_eq!(
        steer_settled.state.live_status(),
        Some(AgentStatus::Working)
    );
    let interrupt_settled = steer_settled
        .state
        .transition(
            correlated(interrupt_turn(), Ok(json!({}))),
            Duration::from_secs(3),
            limits().state(),
        )
        .unwrap();
    assert_eq!(
        interrupt_settled.state.live_status(),
        Some(AgentStatus::Waiting)
    );
}

#[test]
fn completion_before_interrupt_error_preserves_the_error_during_steer_reconciliation() {
    let steering = working_state()
        .transition(
            SessionEvent::SendTurn("steer".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let interrupted = steering
        .transition(SessionEvent::Interrupt, Duration::ZERO, limits().state())
        .unwrap()
        .state;
    let completed = interrupted
        .transition(
            turn_completed(TURN_ID, None),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap()
        .state;
    let interrupt_failed = completed
        .transition(
            correlated(
                interrupt_turn(),
                Err(RpcError::new(-32000, "interrupt transport rejected")),
            ),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap();
    assert!(interrupt_failed.effects.iter().any(|effect| matches!(
        effect,
        SessionEffect::Report(report)
            if matches!(&report.activity, AgentActivity::TaskUpdate { summary } if summary.contains("interrupt transport rejected"))
    )));
    let settled = interrupt_failed
        .state
        .transition(
            correlated(steer_turn("steer"), Ok(json!({"turnId":TURN_ID}))),
            Duration::from_secs(3),
            limits().state(),
        )
        .unwrap()
        .state;
    assert_eq!(
        settled.epitaph().as_deref(),
        Some("interrupt transport rejected")
    );
}

#[test]
fn interrupt_answered_with_no_active_turn_after_completion_settles_quietly() {
    let interrupted = working_state()
        .transition(SessionEvent::Interrupt, Duration::ZERO, limits().state())
        .unwrap()
        .state;
    let completed = interrupted
        .transition(
            turn_completed(TURN_ID, None),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap()
        .state;
    let settled = completed
        .transition(
            correlated(
                interrupt_turn(),
                Err(RpcError::new(-32600, "no active turn")),
            ),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap();

    assert!(!settled
        .effects
        .iter()
        .any(|effect| matches!(effect, SessionEffect::Report(_))));
    assert_eq!(settled.state.live_status(), Some(AgentStatus::Waiting));
    assert_eq!(settled.state.epitaph(), None);
}

fn active_turn_not_steerable_error() -> RpcError {
    RpcError::with_data(
        -32600,
        "cannot steer",
        json!({"codexErrorInfo":{"activeTurnNotSteerable":{"turnKind":"review"}}}),
    )
}

#[test]
fn active_turn_not_steerable_waits_for_completion_then_replays_once() {
    let pending = working_state()
        .transition(
            SessionEvent::SendTurn("next".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let waiting = pending
        .transition(
            correlated(steer_turn("next"), Err(active_turn_not_steerable_error())),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap();
    assert!(waiting.effects.is_empty());
    assert_eq!(waiting.state.live_status(), Some(AgentStatus::Working));

    let replayed = waiting
        .state
        .transition(
            turn_completed(TURN_ID, None),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap();
    assert!(matches!(
        replayed.effects.as_slice(),
        [SessionEffect::CloseTurn(turn_id), SessionEffect::Request(PendingOperation::StartTurn { input, .. })]
            if turn_id == TURN_ID && input == "next"
    ));
}

#[test]
fn active_turn_not_steerable_after_completion_replays_immediately() {
    let completed = working_state()
        .transition(
            SessionEvent::SendTurn("next".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state
        .transition(
            turn_completed(TURN_ID, None),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap()
        .state;

    let replayed = completed
        .transition(
            correlated(steer_turn("next"), Err(active_turn_not_steerable_error())),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap();
    assert!(matches!(
        replayed.effects.as_slice(),
        [SessionEffect::Request(PendingOperation::StartTurn { input, .. })] if input == "next"
    ));
}

#[test]
fn queue_limits_fail_without_partial_insertion() {
    let state = working_state()
        .transition(
            SessionEvent::SendTurn("pending".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state
        .transition(
            SessionEvent::SendTurn("123456".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state
        .transition(
            SessionEvent::SendTurn("abcdef".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    assert!(state
        .transition(
            SessionEvent::SendTurn("overflow".to_string()),
            Duration::ZERO,
            limits().state()
        )
        .is_err());
    assert_eq!(state.queued_turn_count(), 2);
}

#[test]
fn interrupt_is_idempotent_and_holds_replacement_until_completion() {
    let interrupted = working_state()
        .transition(SessionEvent::Interrupt, Duration::ZERO, limits().state())
        .unwrap();
    assert_eq!(interrupted.effects.len(), 1);
    let duplicate = interrupted
        .state
        .transition(SessionEvent::Interrupt, Duration::ZERO, limits().state())
        .unwrap();
    assert!(duplicate.effects.is_empty());
    let queued = duplicate
        .state
        .transition(
            SessionEvent::SendTurn("replacement".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap();
    assert!(queued.effects.is_empty());
    let acked = queued
        .state
        .transition(
            correlated(interrupt_turn(), Ok(json!({}))),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap();
    assert!(acked.effects.is_empty());
    let completed = acked
        .state
        .transition(
            turn_completed(TURN_ID, None),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap();
    assert!(completed.effects.iter().any(|effect| matches!(effect, SessionEffect::Request(PendingOperation::StartTurn { input, .. }) if input == "replacement")));
}

#[test]
fn reconciliation_timeout_fails_instead_of_guessing() {
    let pending = working_state()
        .transition(
            SessionEvent::SendTurn("next".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let provisional = pending
        .transition(
            correlated(
                steer_turn("next"),
                Err(RpcError::new(-32600, "no active turn")),
            ),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    assert!(provisional
        .transition(
            SessionEvent::CheckTimeouts,
            Duration::from_secs(6),
            limits().state()
        )
        .is_err());
}

fn routed_request(id: Value, method: &str, params: Value) -> RoutedServerRequest {
    RoutedServerRequest::decode(
        &InboundServerRequest {
            id,
            method: method.to_string(),
            params,
        },
        Some("thread-parent"),
    )
    .expect("a well formed server request routes")
}

fn parent_decision(
    id: Value,
    method: &str,
    current_unix_seconds: i64,
) -> super::policy::ServerRequestDecision {
    let routed = routed_request(
        id,
        method,
        json!({"threadId":"thread-parent","conversationId":"thread-parent"}),
    );
    assert_ne!(routed.route, ParentThreadRoute::Child, "{method}");
    ServerRequestPolicy::decide(routed.request, routed.route, current_unix_seconds)
}

#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: every_server_request_has_a_refusing_or_read_only_policy is at 17, threshold 15 — bring it under, then remove
fn every_server_request_has_a_refusing_or_read_only_policy() {
    let cases = [
        (
            "item/commandExecution/requestApproval",
            json!({"decision":"decline"}),
            AfterResponse::Continue,
        ),
        (
            "item/fileChange/requestApproval",
            json!({"decision":"decline"}),
            AfterResponse::Continue,
        ),
        (
            "execCommandApproval",
            json!({"decision":{"denied":{"rejection":"Build does not approve commands"}}}),
            AfterResponse::Continue,
        ),
        (
            "applyPatchApproval",
            json!({"decision":{"denied":{"rejection":"Build does not approve file changes"}}}),
            AfterResponse::Continue,
        ),
        (
            "mcpServer/elicitation/request",
            json!({"action":"decline"}),
            AfterResponse::Continue,
        ),
        (
            "currentTime/read",
            json!({"currentTimeAt":1234}),
            AfterResponse::Continue,
        ),
    ];
    for (method, expected, after_response) in cases {
        let decision = parent_decision(json!(7), method, 1234);
        assert_eq!(
            decision.response.to_value(),
            json!({"id":7,"result":expected})
        );
        assert_eq!(
            serde_json::to_string(&decision.response).unwrap(),
            serde_json::to_string(&json!({"id":7,"result":expected})).unwrap()
        );
        assert_eq!(decision.after_response, after_response);
        assert_eq!(
            decision.report.is_some(),
            method.contains("Approval"),
            "{method}"
        );
        assert!(!decision.response.to_value().to_string().contains("accept"));
    }
    for method in [
        "item/tool/requestUserInput",
        "item/permissions/requestApproval",
        "item/tool/call",
    ] {
        let decision = parent_decision(json!(8), method, 0);
        assert_eq!(decision.response.error_code(), Some(-32601));
        assert_eq!(
            decision.response.to_value(),
            json!({"id":8,"error":{"code":-32601,"message":"method not supported"}})
        );
        assert!(matches!(
            decision.after_response,
            AfterResponse::FailTurn(_)
        ));
    }
    for method in [
        "account/chatgptAuthTokens/refresh",
        "attestation/generate",
        "future/request",
    ] {
        let decision = parent_decision(json!(9), method, 0);
        assert_eq!(decision.response.error_code(), Some(-32601));
        assert_eq!(
            decision.response.to_value(),
            json!({"id":9,"error":{"code":-32601,"message":"method not found"}})
        );
        assert!(matches!(
            decision.after_response,
            AfterResponse::FailSession(_)
        ));
    }
}

#[test]
fn server_request_decoder_types_known_requests_and_retains_only_unknown_methods() {
    let approval = routed_request(
        json!(1),
        "item/commandExecution/requestApproval",
        json!({"threadId":"thread-parent"}),
    );
    assert_eq!(approval.route, ParentThreadRoute::Parent);
    assert!(matches!(
        approval.request,
        ServerRequest::CommandApproval { id } if id == json!(1)
    ));

    let unknown = routed_request(json!(2), "future/request", json!({}));
    assert_eq!(unknown.route, ParentThreadRoute::Unscoped);
    assert!(matches!(
        unknown.request,
        ServerRequest::Unknown { id, method }
            if id == json!(2) && method == "future/request"
    ));

    assert!(RoutedServerRequest::decode(
        &InboundServerRequest {
            id: json!(3),
            method: "currentTime/read".to_string(),
            params: json!([]),
        },
        Some("thread-parent"),
    )
    .is_err());
}

#[test]
fn known_thread_scoped_requests_without_a_routing_id_fail_before_policy() {
    for expected_parent in [None, Some("thread-parent")] {
        for params in [json!({}), json!({"threadId":7}), json!([]), Value::Null] {
            let failure = RoutedServerRequest::decode(
                &InboundServerRequest {
                    id: json!(3),
                    method: "currentTime/read".to_string(),
                    params: params.clone(),
                },
                expected_parent,
            )
            .expect_err("a thread-scoped request needs its routing id");
            assert_eq!(
                failure, "currentTime/read routing id is missing",
                "{params}"
            );
        }
    }
}

#[test]
fn error_notifications_require_the_typed_liveness_shape() {
    assert!(matches!(
        ServerNotification::decode(
            "error",
            json!({
                "threadId":THREAD_ID,
                "turnId":TURN_ID,
                "error":{"message":"retry failed"},
                "willRetry":false
            })
        )
        .unwrap(),
        ServerNotification::Error(error)
            if error.thread_id == THREAD_ID
                && error.turn_id == TURN_ID
                && error.error.message == "retry failed"
                && !error.will_retry
    ));
    assert!(ServerNotification::decode(
        "error",
        json!({"error":{"message":"missing lifecycle fields"}})
    )
    .is_err());
}

#[test]
fn unknown_server_requests_ignore_arbitrary_params_before_policy() {
    for params in [
        None,
        Some(json!({"anything":true})),
        Some(json!([1, 2, 3])),
        Some(json!("scalar")),
        Some(json!(null)),
    ] {
        let routed = routed_request(json!(44), "future/request", params.unwrap_or(Value::Null));
        assert_eq!(routed.route, ParentThreadRoute::Unscoped);
        let decision = ServerRequestPolicy::decide(routed.request, routed.route, 1234);
        assert_eq!(
            decision.response.to_value(),
            json!({"id":44,"error":{"code":-32601,"message":"method not found"}})
        );
        assert!(matches!(
            decision.after_response,
            AfterResponse::FailSession(_)
        ));
    }
}

#[test]
fn parent_thread_filter_isolates_every_child_notification_before_decoding() {
    let child_thread = CHILD_THREAD_ID;
    let cases = [
        (
            "thread/started",
            json!({"thread":{"id":child_thread,"parentThreadId":"thread-parent"}}),
        ),
        (
            "turn/started",
            json!({"threadId":child_thread,"malformed":true}),
        ),
        (
            "turn/completed",
            json!({"threadId":child_thread,"malformed":true}),
        ),
        (
            "item/started",
            json!({"threadId":child_thread,"malformed":true}),
        ),
        (
            "item/completed",
            json!({"threadId":child_thread,"malformed":true}),
        ),
        (
            "item/agentMessage/delta",
            json!({"threadId":child_thread,"malformed":true}),
        ),
        ("error", json!({"threadId":child_thread,"malformed":true})),
        (
            "future/notification",
            json!({"threadId":child_thread,"malformed":true}),
        ),
    ];
    for (method, params) in cases {
        assert_eq!(
            ParentThreadFilter::notification(method, &params, Some("thread-parent")),
            ParentThreadRoute::Child,
            "{method}"
        );
    }
}

#[test]
fn every_known_child_thread_request_receives_a_safe_continue_response() {
    let cases = [
        (
            "item/commandExecution/requestApproval",
            json!({"threadId":CHILD_THREAD_ID}),
            json!({"id":7,"result":{"decision":"decline"}}),
        ),
        (
            "item/fileChange/requestApproval",
            json!({"threadId":CHILD_THREAD_ID}),
            json!({"id":7,"result":{"decision":"decline"}}),
        ),
        (
            "execCommandApproval",
            json!({"conversationId":CHILD_THREAD_ID}),
            json!({"id":7,"result":{"decision":{"denied":{"rejection":"Build does not approve commands"}}}}),
        ),
        (
            "applyPatchApproval",
            json!({"conversationId":CHILD_THREAD_ID}),
            json!({"id":7,"result":{"decision":{"denied":{"rejection":"Build does not approve file changes"}}}}),
        ),
        (
            "mcpServer/elicitation/request",
            json!({"threadId":CHILD_THREAD_ID}),
            json!({"id":7,"result":{"action":"decline"}}),
        ),
        (
            "item/tool/requestUserInput",
            json!({"threadId":CHILD_THREAD_ID}),
            json!({"id":7,"error":{"code":-32601,"message":"method not supported"}}),
        ),
        (
            "item/permissions/requestApproval",
            json!({"threadId":CHILD_THREAD_ID}),
            json!({"id":7,"error":{"code":-32601,"message":"method not supported"}}),
        ),
        (
            "item/tool/call",
            json!({"threadId":CHILD_THREAD_ID}),
            json!({"id":7,"error":{"code":-32601,"message":"method not supported"}}),
        ),
        (
            "currentTime/read",
            json!({"threadId":CHILD_THREAD_ID}),
            json!({"id":7,"result":{"currentTimeAt":1234}}),
        ),
    ];
    for (method, params, expected_response) in cases {
        let routed = routed_request(json!(7), method, params);
        assert_eq!(routed.route, ParentThreadRoute::Child, "{method}");
        let decision = ServerRequestPolicy::decide(routed.request, routed.route, 1234);
        assert_eq!(decision.response.to_value(), expected_response, "{method}");
        assert_eq!(decision.after_response, AfterResponse::Continue, "{method}");
        assert!(decision.report.is_none(), "{method}");
    }
}

#[test]
fn child_requests_with_malformed_non_routing_params_still_receive_the_safe_response() {
    let routed = routed_request(
        json!(11),
        "item/commandExecution/requestApproval",
        json!({"threadId":CHILD_THREAD_ID,"command":42,"cwd":[]}),
    );
    assert_eq!(routed.route, ParentThreadRoute::Child);
    let decision = ServerRequestPolicy::decide(routed.request, routed.route, 0);
    assert_eq!(
        decision.response.to_value(),
        json!({"id":11,"result":{"decision":"decline"}})
    );
    assert_eq!(decision.after_response, AfterResponse::Continue);
    assert!(decision.report.is_none());
}

#[test]
fn unscoped_requests_keep_their_tabled_session_failure_on_every_route() {
    for method in ["account/chatgptAuthTokens/refresh", "attestation/generate"] {
        for params in [json!({"threadId":CHILD_THREAD_ID}), Value::Null, json!([])] {
            let routed = routed_request(json!(12), method, params.clone());
            assert_eq!(
                routed.route,
                ParentThreadRoute::Unscoped,
                "{method} {params}"
            );
            let decision = ServerRequestPolicy::decide(routed.request, routed.route, 0);
            assert_eq!(
                decision.response.to_value(),
                json!({"id":12,"error":{"code":-32601,"message":"method not found"}}),
                "{method} {params}"
            );
            assert!(
                matches!(decision.after_response, AfterResponse::FailSession(_)),
                "{method} {params}"
            );
        }
    }
}

#[test]
fn every_schema_known_item_has_one_explicit_classification() {
    let cases = [
        (
            json!({"type":"agentMessage"}),
            ItemClassification::Emitting {
                report: ItemReportKind::Narration,
            },
        ),
        (
            json!({"type":"reasoning"}),
            ItemClassification::Emitting {
                report: ItemReportKind::Reasoning,
            },
        ),
        (
            json!({"type":"commandExecution"}),
            ItemClassification::TrackedTool {
                summary: ToolSummaryCategory::Command,
            },
        ),
        (
            json!({"type":"fileChange"}),
            ItemClassification::TrackedTool {
                summary: ToolSummaryCategory::FileChange,
            },
        ),
        (
            json!({"type":"mcpToolCall","server":"other"}),
            ItemClassification::TrackedTool {
                summary: ToolSummaryCategory::Mcp,
            },
        ),
        (
            json!({"type":"collabAgentToolCall"}),
            ItemClassification::TrackedTool {
                summary: ToolSummaryCategory::Collaboration,
            },
        ),
        (
            json!({"type":"subAgentActivity"}),
            ItemClassification::Emitting {
                report: ItemReportKind::SubAgentActivity,
            },
        ),
        (
            json!({"type":"webSearch"}),
            ItemClassification::TrackedTool {
                summary: ToolSummaryCategory::WebSearch,
            },
        ),
        (
            json!({"type":"imageView"}),
            ItemClassification::TrackedTool {
                summary: ToolSummaryCategory::ImageView,
            },
        ),
        (
            json!({"type":"sleep"}),
            ItemClassification::TrackedTool {
                summary: ToolSummaryCategory::Sleep,
            },
        ),
        (
            json!({"type":"imageGeneration"}),
            ItemClassification::TrackedTool {
                summary: ToolSummaryCategory::ImageGeneration,
            },
        ),
        (
            json!({"type":"contextCompaction"}),
            ItemClassification::Emitting {
                report: ItemReportKind::ContextCompaction,
            },
        ),
        (
            json!({"type":"userMessage"}),
            ItemClassification::Suppressed {
                reason: SuppressionReason::UserMessageEcho,
            },
        ),
        (
            json!({"type":"hookPrompt"}),
            ItemClassification::Suppressed {
                reason: SuppressionReason::HookPrompt,
            },
        ),
        (
            json!({"type":"functionCallOutput"}),
            ItemClassification::Suppressed {
                reason: SuppressionReason::FunctionCallOutput,
            },
        ),
        (
            json!({"type":"plan"}),
            ItemClassification::Suppressed {
                reason: SuppressionReason::ExperimentalPlan,
            },
        ),
        (
            json!({"type":"enteredReviewMode"}),
            ItemClassification::Emitting {
                report: ItemReportKind::EnteredReviewMode,
            },
        ),
        (
            json!({"type":"exitedReviewMode"}),
            ItemClassification::Emitting {
                report: ItemReportKind::ExitedReviewMode,
            },
        ),
        (
            json!({"type":"dynamicToolCall"}),
            ItemClassification::Suppressed {
                reason: SuppressionReason::DeferredDynamicTool,
            },
        ),
    ];
    for (item, expected) in cases {
        assert_eq!(classify_item(&item), expected, "{item:?}");
    }
    assert_eq!(
        classify_item(&json!({"type":"futureItem"})),
        ItemClassification::Suppressed {
            reason: SuppressionReason::UnknownItem,
        }
    );
    assert_eq!(
        classify_item(&json!({"type":"mcpToolCall","server":"build"})),
        ItemClassification::Suppressed {
            reason: SuppressionReason::BuildMcp,
        }
    );
}

#[test]
fn suppressed_items_need_no_id_and_consume_no_ledgers() {
    let mut bounded = limits();
    bounded.open_items = 1;
    bounded.open_item_bytes = 1;
    bounded.completed_items = 1;
    bounded.completed_item_bytes = 1;
    let mut translator = CodexActivityTranslator::new(bounded.translator());
    for item in [
        json!({"type":"mcpToolCall","server":"build"}),
        json!({"type":"dynamicToolCall"}),
        json!({"type":"plan"}),
        json!({"type":"futureItem"}),
        json!({"type":"userMessage"}),
        json!({"type":"hookPrompt"}),
        json!({"type":"functionCallOutput"}),
    ] {
        for method in ["item/started", "item/completed"] {
            assert!(translator
                .translate(method, &item_envelope(item.clone()))
                .unwrap()
                .is_empty());
        }
    }
    assert_eq!(translator.open_item_count(), 0);
    assert_eq!(translator.completed_item_count(), 0);
}

#[test]
fn completed_speech_and_tools_translate_with_bounded_readable_details() {
    let mut translator = CodexActivityTranslator::new(limits().translator());
    let reasoning = translator
        .translate(
            "item/completed",
            &item_envelope(json!({"id":"r","type":"reasoning","summary":["final summary"]})),
        )
        .unwrap();
    assert_eq!(
        reasoning[0].activity,
        AgentActivity::Reasoning {
            summary: "final summary".to_string()
        }
    );
    let narration = translator
        .translate(
            "item/completed",
            &item_envelope(json!({"id":"n","type":"agentMessage","text":"finished"})),
        )
        .unwrap();
    assert_eq!(
        narration[0].activity,
        AgentActivity::Narration {
            summary: "finished".to_string()
        }
    );

    let started = translator
        .translate(
            "item/started",
            &item_envelope(
                json!({"id":"c","type":"commandExecution","command":"shell -lc wrapped","commandActions":[{"type":"unknown","command":"cargo test --lib"}],"status":"inProgress"}),
            ),
        )
        .unwrap();
    assert_eq!(
        started[0].activity,
        AgentActivity::ToolUse {
            call_id: "c".to_string(),
            summary: "cargo test --lib".to_string(),
        }
    );
    let completed = translator
        .translate(
            "item/completed",
            &item_envelope(
                json!({"id":"c","type":"commandExecution","command":"shell -lc wrapped","aggregatedOutput":"test result: ok\n4 passed","status":"completed","exitCode":0}),
            ),
        )
        .unwrap();
    assert_eq!(
        completed[0].activity,
        AgentActivity::ToolResult {
            call_id: "c".to_string(),
            outcome: ToolOutcome::Ok,
            summary: "exit 0: test result: ok\n4 passed".to_string(),
        }
    );
}

#[test]
fn tool_summaries_select_safe_fields_and_never_dump_objects_or_diffs() {
    let mut translator = CodexActivityTranslator::new(AppServerLimits::default().translator());
    let cases = [
        (
            json!({"id":"files","type":"fileChange","changes":[
                {"path":"src/new.rs","kind":{"type":"add"},"diff":"PRIVATE PATCH"},
                {"path":"src/old.rs","kind":{"type":"delete"},"diff":"PRIVATE PATCH"}
            ],"status":"inProgress"}),
            "File change add src/new.rs, delete src/old.rs",
        ),
        (
            json!({"id":"mcp","type":"mcpToolCall","server":"github","tool":"search","arguments":{"query":"rust parser","token":"PRIVATE TOKEN"},"status":"inProgress"}),
            "MCP github.search rust parser",
        ),
        (
            json!({"id":"web","type":"webSearch","query":"Codex app server","action":{"type":"search","query":"PRIVATE ACTION"}}),
            "Web search Codex app server",
        ),
        (
            json!({"id":"image","type":"imageView","path":"/tmp/screenshot.png"}),
            "Image view /tmp/screenshot.png",
        ),
    ];
    for (item, expected) in cases {
        let reports = translator
            .translate("item/started", &item_envelope(item))
            .unwrap();
        let AgentActivity::ToolUse { summary, .. } = &reports[0].activity else {
            panic!("expected tool use");
        };
        assert_eq!(summary, expected);
        assert!(!summary.contains("PRIVATE"));
        assert!(!summary.contains('{'));
    }
}

#[test]
fn tool_results_report_errors_exit_codes_and_text_without_dumping_objects() {
    let mut translator = CodexActivityTranslator::new(limits().translator());
    for (started, completed, expected_outcome, expected_detail) in [
        (
            json!({"id":"failed","type":"commandExecution","command":"false","status":"inProgress"}),
            json!({"id":"failed","type":"commandExecution","command":"false","status":"failed","exitCode":1,"aggregatedOutput":null}),
            ToolOutcome::Error,
            "exit 1",
        ),
        (
            json!({"id":"mcp-result","type":"mcpToolCall","server":"docs","tool":"lookup","arguments":{"query":"limits"},"status":"inProgress"}),
            json!({"id":"mcp-result","type":"mcpToolCall","server":"docs","tool":"lookup","status":"completed","result":{"content":[{"type":"text","text":"Found the limit"}],"structuredContent":{"private":"DO NOT DUMP"}}}),
            ToolOutcome::Ok,
            "Found the limit",
        ),
        (
            json!({"id":"mcp-error","type":"mcpToolCall","server":"docs","tool":"lookup","status":"inProgress"}),
            json!({"id":"mcp-error","type":"mcpToolCall","server":"docs","tool":"lookup","status":"failed","error":{"message":"permission denied","private":"DO NOT DUMP"}}),
            ToolOutcome::Error,
            "permission denied",
        ),
    ] {
        translator
            .translate("item/started", &item_envelope(started))
            .unwrap();
        let reports = translator
            .translate("item/completed", &item_envelope(completed))
            .unwrap();
        let AgentActivity::ToolResult {
            outcome, summary, ..
        } = &reports[0].activity
        else {
            panic!("expected tool result");
        };
        assert_eq!(*outcome, expected_outcome);
        assert!(summary.contains(expected_detail), "{summary}");
        assert!(!summary.contains("DO NOT DUMP"));
        assert!(!summary.contains('{'));
    }
}

#[test]
fn expanded_tool_rows_keep_text_until_the_activity_bound() {
    let mut translator = CodexActivityTranslator::new(AppServerLimits::default().translator());
    let long = "λ\n".repeat(2_000);
    let started = translator
        .translate(
            "item/started",
            &item_envelope(
                json!({"id":"long","type":"commandExecution","command":long,"status":"inProgress"}),
            ),
        )
        .unwrap();
    let AgentActivity::ToolUse { summary, .. } = &started[0].activity else {
        panic!("expected tool use");
    };
    assert!(summary.chars().count() > crate::harness::adk::TOOL_SUMMARY_LIMIT);
    assert!(summary.chars().count() <= crate::harness::adk::ACTIVITY_TEXT_LIMIT + 1);
    let reports = translator
        .translate("item/completed", &item_envelope(json!({"id":"long","type":"commandExecution","status":"completed","exitCode":0,"aggregatedOutput":long})))
        .unwrap();
    let AgentActivity::ToolResult { summary, .. } = &reports[0].activity else {
        panic!("expected tool result");
    };
    assert!(summary.chars().count() > crate::harness::adk::TOOL_SUMMARY_LIMIT);
    assert!(summary.chars().count() <= crate::harness::adk::ACTIVITY_TEXT_LIMIT + 1);
    assert!(summary.lines().count() > 1);
}

#[test]
fn duplicate_completed_items_emit_once_and_cannot_reopen_tools() {
    let mut translator = CodexActivityTranslator::new(limits().translator());
    let completed_speech =
        item_envelope(json!({"id":"speech","type":"agentMessage","text":"once"}));
    assert_eq!(
        translator
            .translate("item/completed", &completed_speech)
            .unwrap()
            .len(),
        1
    );
    assert!(translator
        .translate("item/completed", &completed_speech)
        .unwrap()
        .is_empty());

    let started_tool = item_envelope(json!({"id":"tool","type":"webSearch"}));
    let completed_tool =
        item_envelope(json!({"id":"tool","type":"webSearch","status":"completed"}));
    assert_eq!(
        translator
            .translate("item/started", &started_tool)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        translator
            .translate("item/completed", &completed_tool)
            .unwrap()
            .len(),
        1
    );
    assert!(translator
        .translate("item/completed", &completed_tool)
        .unwrap()
        .is_empty());
    assert!(translator
        .translate("item/started", &started_tool)
        .unwrap()
        .is_empty());
}

#[test]
fn completed_item_deduplication_is_bounded_and_only_turn_close_clears_keys() {
    let mut bounded = limits();
    bounded.completed_items = 2;
    bounded.completed_item_bytes = 16;
    let mut translator = CodexActivityTranslator::new(bounded.translator());
    let completed = |turn: &str, id: &str| {
        item_envelope_at(
            THREAD_ID,
            turn,
            json!({"id":id,"type":"agentMessage","text":id}),
        )
    };

    for id in ["a", "b"] {
        assert_eq!(
            translator
                .translate("item/completed", &completed(TURN_ID, id))
                .unwrap()
                .len(),
            1
        );
    }
    assert_eq!(
        translator
            .translate("item/completed", &completed(TURN_ID, "c"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(translator.completed_item_count(), 2);
    assert_eq!(
        translator
            .translate("item/completed", &completed("turn-2", "c"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(translator.completed_item_count(), 2);
    translator.close_turn(TURN_ID).unwrap();
    assert_eq!(translator.completed_item_count(), 1);
}

#[test]
fn completed_item_lru_evicts_non_fatally_and_refreshes_duplicates() {
    let mut bounded = limits();
    bounded.completed_items = 2;
    bounded.completed_item_bytes = 64;
    let mut translator = CodexActivityTranslator::new(bounded.translator());
    let completed = |id: &str| item_envelope(json!({"id":id,"type":"agentMessage","text":id}));

    for id in ["a", "b"] {
        assert_eq!(
            translator
                .translate("item/completed", &completed(id))
                .unwrap()
                .len(),
            1
        );
    }
    assert!(translator
        .translate("item/completed", &completed("a"))
        .unwrap()
        .is_empty());
    assert_eq!(
        translator
            .translate("item/completed", &completed("c"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        translator
            .translate("item/completed", &completed("b"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(translator.completed_item_count(), 2);
}

#[test]
fn an_individually_oversized_completion_key_is_processed_but_not_retained() {
    let mut bounded = limits();
    bounded.completed_items = 4;
    bounded.completed_item_bytes = 4;
    let mut translator = CodexActivityTranslator::new(bounded.translator());
    let retainable = item_envelope_at(
        THREAD_ID,
        "t",
        json!({"id":"a","type":"agentMessage","text":"fits"}),
    );
    let oversized = item_envelope(json!({"id":"a","type":"agentMessage","text":"once"}));

    assert_eq!(
        translator
            .translate("item/completed", &retainable)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(translator.completed_item_count(), 1);
    assert_eq!(
        translator
            .translate("item/completed", &oversized)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(translator.completed_item_count(), 1);
    assert_eq!(
        translator
            .translate("item/completed", &oversized)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(translator.completed_item_count(), 1);
}

#[test]
fn more_than_256_valid_completions_remain_live_and_turn_close_clears_keys() {
    let mut translator = CodexActivityTranslator::new(AppServerLimits::default().translator());
    for index in 0..300 {
        let reports = translator
            .translate(
                "item/completed",
                &item_envelope(json!({
                    "id":format!("message-{index}"),
                    "type":"agentMessage",
                    "text":format!("message {index}")
                })),
            )
            .unwrap();
        assert_eq!(reports.len(), 1);
    }
    assert_eq!(translator.completed_item_count(), 256);
    translator.close_turn(TURN_ID).unwrap();
    assert_eq!(translator.completed_item_count(), 0);
}

#[test]
fn only_exact_delta_notification_methods_are_classified_as_deltas() {
    assert_eq!(
        ServerNotification::decode("item/agentMessage/delta", json!({})).unwrap(),
        ServerNotification::Delta
    );
    assert_eq!(
        ServerNotification::decode("future/deltaEvent", json!({})).unwrap(),
        ServerNotification::Unknown
    );
}

#[test]
fn child_thread_events_are_isolated_while_parent_subagent_activity_is_retained() {
    let state = working_state();
    let child_thread = ServerNotification::decode(
        "thread/started",
        json!({
            "thread":{
                "id":CHILD_THREAD_ID,
                "parentThreadId":THREAD_ID
            }
        }),
    )
    .unwrap();
    assert!(matches!(
        child_thread,
        ServerNotification::ThreadStarted {
            parent_thread_id: Some(parent),
            ..
        } if parent == THREAD_ID
    ));
    let child = ServerNotification::decode(
        "item/started",
        item_envelope_at(
            CHILD_THREAD_ID,
            CHILD_TURN_ID,
            json!({"id":"child-command","type":"commandExecution"}),
        ),
    )
    .unwrap();
    let ServerNotification::Item(child_item) = child else {
        panic!("expected a typed child item");
    };
    assert!(!state.parent_thread_matches(&child_item.thread_id));

    let parent = ServerNotification::decode(
        "item/completed",
        item_envelope(json!({
            "id":"subagent",
            "type":"subAgentActivity",
            "agentPath":"worker",
            "agentThreadId":CHILD_THREAD_ID,
            "kind":"completed"
        })),
    )
    .unwrap();
    let ServerNotification::Item(parent_item) = &parent else {
        panic!("expected a typed parent item");
    };
    assert!(state.parent_thread_matches(&parent_item.thread_id));
    assert_eq!(parent_item.item["type"], "subAgentActivity");
    assert!(state
        .transition(
            SessionEvent::TurnStarted(parent_item.turn_id.clone()),
            Duration::ZERO,
            limits().state(),
        )
        .is_ok());
    let reports = CodexActivityTranslator::new(limits().translator())
        .translate_notification(&parent)
        .unwrap();
    assert!(matches!(
        reports.as_slice(),
        [crate::harness::ActivityReport {
            activity: AgentActivity::TaskUpdate { .. },
            ..
        }]
    ));
}

#[test]
fn speech_keeps_full_text_until_its_separate_large_bound() {
    let mut translator = CodexActivityTranslator::new(AppServerLimits::default().translator());
    for item in [
        json!({"id":"r","type":"reasoning","summary":["x".repeat(1000)]}),
        json!({"id":"n","type":"agentMessage","text":"y".repeat(1000)}),
    ] {
        let reports = translator
            .translate("item/completed", &item_envelope(item))
            .unwrap();
        let summary = match &reports[0].activity {
            AgentActivity::Reasoning { summary } | AgentActivity::Narration { summary } => summary,
            other => panic!("expected speech report, got {other:?}"),
        };
        assert_eq!(summary.chars().count(), 1000);
    }

    let exact = "e".repeat(crate::harness::adk::ACTIVITY_TEXT_LIMIT);
    let reports = translator
        .translate(
            "item/completed",
            &item_envelope(json!({"id":"exact","type":"agentMessage","text":exact})),
        )
        .unwrap();
    let AgentActivity::Narration { summary } = &reports[0].activity else {
        panic!("expected narration report");
    };
    assert_eq!(summary, &exact, "the exact boundary remains unchanged");

    let reports = translator
        .translate(
            "item/completed",
            &item_envelope(json!({
                "id":"large",
                "type":"agentMessage",
                "text":"z".repeat(crate::harness::adk::ACTIVITY_TEXT_LIMIT + 100)
            })),
        )
        .unwrap();
    let AgentActivity::Narration { summary } = &reports[0].activity else {
        panic!("expected narration report");
    };
    assert_eq!(
        summary.chars().count(),
        crate::harness::adk::ACTIVITY_TEXT_LIMIT + 1
    );
    assert!(summary.ends_with('…'));
}

#[test]
fn every_required_tool_kind_emits_one_paired_call() {
    let mut translator = CodexActivityTranslator::new(AppServerLimits::default().translator());
    for (index, item) in [
        json!({"type":"commandExecution","status":"inProgress"}),
        json!({"type":"fileChange","status":"inProgress"}),
        json!({"type":"mcpToolCall","server":"other","tool":"lookup","status":"inProgress"}),
        json!({"type":"webSearch"}),
        json!({"type":"imageView"}),
        json!({"type":"sleep"}),
        json!({"type":"imageGeneration","status":"inProgress"}),
        json!({"type":"collabAgentToolCall","tool":"spawnAgent","status":"inProgress"}),
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("tool-{index}");
        let mut started_item = item.clone();
        started_item["id"] = json!(id);
        let started = translator
            .translate("item/started", &item_envelope(started_item))
            .unwrap();
        assert_eq!(started.len(), 1, "{item:?}");
        let mut completed_item = item;
        completed_item["id"] = json!(id);
        completed_item["status"] = json!("completed");
        let completed = translator
            .translate("item/completed", &item_envelope(completed_item))
            .unwrap();
        assert_eq!(completed.len(), 1);
        assert!(matches!(
            completed[0].activity,
            AgentActivity::ToolResult {
                outcome: ToolOutcome::Ok,
                ..
            }
        ));
    }
}

#[test]
fn subagent_activity_emits_only_on_completion() {
    let mut translator = CodexActivityTranslator::new(limits().translator());
    let item = item_envelope(json!({
        "id":"subagent-once",
        "type":"subAgentActivity",
        "agentPath":"/root/worker",
        "kind":"interacted"
    }));
    assert!(translator
        .translate("item/started", &item)
        .unwrap()
        .is_empty());
    let completed = translator.translate("item/completed", &item).unwrap();
    assert_eq!(completed.len(), 1);
    assert!(matches!(
        &completed[0].activity,
        AgentActivity::TaskUpdate { summary } if summary == "/root/worker - interacted"
    ));
}

#[test]
fn build_mcp_dynamic_and_unknown_items_are_suppressed() {
    let mut translator = CodexActivityTranslator::new(limits().translator());
    for item in [
        json!({"id":"build","type":"mcpToolCall","server":"build","tool":"done","status":"inProgress","arguments":{"secret":true}}),
        json!({"id":"dynamic","type":"dynamicToolCall","tool":"later","status":"inProgress","arguments":{}}),
        json!({"id":"future","type":"newItem"}),
    ] {
        assert!(translator
            .translate("item/started", &item_envelope(item))
            .unwrap()
            .is_empty());
    }
}

#[test]
fn open_tools_close_unanswered_and_release_limits() {
    let mut translator = CodexActivityTranslator::new(limits().translator());
    for id in ["a", "b"] {
        translator
            .translate(
                "item/started",
                &item_envelope(json!({"id":id,"type":"webSearch","query":"not retained"})),
            )
            .unwrap();
    }
    let closed = translator.close_turn(TURN_ID).unwrap();
    assert_eq!(closed.len(), 2);
    assert!(closed.iter().all(|report| matches!(
        report.activity,
        AgentActivity::ToolResult {
            outcome: ToolOutcome::Unanswered,
            ..
        }
    )));
    assert_eq!(translator.open_item_count(), 0);
}

#[test]
fn completing_an_item_releases_its_retained_key_byte_charge() {
    let mut bounded = limits();
    bounded.open_item_bytes = TURN_ID.len() + "a".len();
    let mut translator = CodexActivityTranslator::new(bounded.translator());
    let envelope = |id: &str, status: &str| {
        item_envelope(json!({"id":id,"type":"webSearch","query":"not retained","status":status}))
    };
    translator
        .translate("item/started", &envelope("a", "inProgress"))
        .unwrap();
    assert!(translator
        .translate("item/started", &envelope("b", "inProgress"))
        .is_err());
    translator
        .translate("item/completed", &envelope("a", "completed"))
        .unwrap();
    assert!(translator
        .translate("item/started", &envelope("b", "inProgress"))
        .is_ok());
}

#[test]
fn app_server_spec_reuses_codex_mcp_config_without_experimental_flags() {
    let options = spawn_options();
    let context = harness_context();
    let choice = selected_choice();
    let app = super::CodexAppServerHarness
        .spec(&choice, &options, &context)
        .expect("the app-server spec builds");
    let tui = crate::harness::codex::CodexHarness
        .spec(&choice, &options, &context)
        .expect("the TUI spec builds");
    let configs = |args: &[String]| {
        args.windows(2)
            .filter(|pair| pair[0] == "--config" && pair[1].starts_with("mcp_servers.build"))
            .map(|pair| pair[1].clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(configs(&app.args), configs(&tui.args));
    assert_eq!(&app.args[..2], ["app-server", "--stdio"]);
    assert!(app
        .args
        .iter()
        .any(|arg| arg == "model_reasoning_effort=\"high\""));
    assert!(!app
        .args
        .iter()
        .any(|arg| arg.contains("experimental") || arg.contains("multi_agent")));
}

#[derive(Debug, PartialEq, Eq)]
enum FixtureActivity {
    Reasoning,
    Narration,
    ToolUse(String),
    ToolResult(String, ToolOutcome),
}

struct FixtureReplay {
    activities: Vec<FixtureActivity>,
    correlated_methods: Vec<&'static str>,
    completed_turns: Vec<String>,
    reports: Vec<crate::harness::ActivityReport>,
}

fn replay_observed_fixture(fixture: &str) -> FixtureReplay {
    let connection = AppServerConnection::memory(AppServerLimits::default().connection());
    let mut translator = CodexActivityTranslator::new(AppServerLimits::default().translator());
    let mut correlated_methods = Vec::new();
    let mut completed_turns = Vec::new();
    let mut reports = Vec::new();
    for line in fixture.lines() {
        let envelope: Value = serde_json::from_str(line).unwrap();
        let message = &envelope["message"];
        if envelope["direction"] == "client" {
            if message.get("id").is_some() {
                connection.request(fixture_operation(message)).unwrap();
            }
            continue;
        }
        let event = decode(&connection, message.clone()).unwrap();
        match event {
            ConnectionEvent::Response { operation, .. } => {
                correlated_methods.push(operation.method())
            }
            ConnectionEvent::Notification(inbound) => {
                let notification =
                    ServerNotification::decode(&inbound.method, inbound.params).unwrap();
                if let ServerNotification::TurnCompleted { completion, .. } = &notification {
                    completed_turns.push(completion.turn_id.clone());
                    reports.extend(translator.close_turn(&completion.turn_id).unwrap());
                } else {
                    reports.extend(translator.translate_notification(&notification).unwrap());
                }
            }
            ConnectionEvent::Request(_) => panic!("observed fixture contains no server requests"),
        }
    }
    let activities = reports
        .iter()
        .filter_map(|report| match &report.activity {
            AgentActivity::Reasoning { .. } => Some(FixtureActivity::Reasoning),
            AgentActivity::Narration { .. } => Some(FixtureActivity::Narration),
            AgentActivity::ToolUse { call_id, .. } => {
                Some(FixtureActivity::ToolUse(call_id.clone()))
            }
            AgentActivity::ToolResult {
                call_id, outcome, ..
            } => Some(FixtureActivity::ToolResult(call_id.clone(), *outcome)),
            AgentActivity::TaskUpdate { .. } => None,
        })
        .collect();
    FixtureReplay {
        activities,
        correlated_methods,
        completed_turns,
        reports,
    }
}

fn fixture_operation(message: &Value) -> PendingOperation {
    let params = &message["params"];
    let optional = |field: &str| params[field].as_str().map(str::to_string);
    match message["method"].as_str().unwrap() {
        "initialize" => PendingOperation::Initialize,
        "thread/start" => PendingOperation::StartThread {
            cwd: params["cwd"].as_str().unwrap().to_string(),
            model: optional("model"),
        },
        "thread/resume" => PendingOperation::ResumeThread {
            thread_id: params["threadId"].as_str().unwrap().to_string(),
            cwd: params["cwd"].as_str().unwrap().to_string(),
            model: optional("model"),
        },
        "turn/start" => PendingOperation::StartTurn {
            thread_id: params["threadId"].as_str().unwrap().to_string(),
            input: params["input"][0]["text"].as_str().unwrap().to_string(),
            model: optional("model"),
            effort: optional("effort"),
        },
        "turn/steer" => PendingOperation::SteerTurn {
            thread_id: params["threadId"].as_str().unwrap().to_string(),
            turn_id: params["expectedTurnId"].as_str().unwrap().to_string(),
            input: params["input"][0]["text"].as_str().unwrap().to_string(),
        },
        "turn/interrupt" => PendingOperation::InterruptTurn {
            thread_id: params["threadId"].as_str().unwrap().to_string(),
            turn_id: params["turnId"].as_str().unwrap().to_string(),
        },
        method => panic!("unsupported fixture client method {method}"),
    }
}

#[test]
fn observed_start_fixture_replays_as_one_correlated_stream() {
    let replay =
        replay_observed_fixture(checked_in_fixture("0.153.0/observed-session-start.jsonl"));
    assert_eq!(
        replay.correlated_methods,
        [
            "initialize",
            "thread/start",
            "turn/start",
            "thread/resume",
            "turn/start",
            "turn/steer",
            "turn/interrupt"
        ]
    );
    assert_eq!(replay.completed_turns, ["turn-1", "turn-2"]);
    assert_eq!(
        replay.activities,
        [
            FixtureActivity::Reasoning,
            FixtureActivity::Narration,
            FixtureActivity::ToolUse("command-1".to_string()),
            FixtureActivity::ToolResult("command-1".to_string(), ToolOutcome::Ok),
            FixtureActivity::Narration,
            FixtureActivity::Narration,
            FixtureActivity::ToolUse("command-2".to_string()),
            FixtureActivity::ToolResult("command-2".to_string(), ToolOutcome::Unanswered),
        ]
    );
}

#[test]
fn observed_resume_fixture_replays_as_one_correlated_stream() {
    let replay =
        replay_observed_fixture(checked_in_fixture("0.153.0/observed-session-resume.jsonl"));
    assert_eq!(
        replay.correlated_methods,
        ["initialize", "thread/resume", "turn/start"]
    );
    assert_eq!(replay.completed_turns, ["turn-3"]);
    assert_eq!(
        replay.activities,
        [
            FixtureActivity::Reasoning,
            FixtureActivity::Narration,
            FixtureActivity::ToolUse("file-change-1".to_string()),
            FixtureActivity::ToolResult("file-change-1".to_string(), ToolOutcome::Ok),
            FixtureActivity::ToolUse("command-3".to_string()),
            FixtureActivity::ToolResult("command-3".to_string(), ToolOutcome::Error),
            FixtureActivity::Narration,
        ]
    );
}

#[test]
fn observed_mcp_fixture_suppresses_build_and_pairs_non_build_activity() {
    let replay = replay_observed_fixture(checked_in_fixture("0.153.0/observed-session-mcp.jsonl"));
    assert_eq!(
        replay.correlated_methods,
        ["initialize", "thread/start", "turn/start"]
    );
    assert_eq!(replay.completed_turns, ["turn-mcp-1"]);
    assert_eq!(
        replay.activities,
        [
            FixtureActivity::Reasoning,
            FixtureActivity::ToolUse("mcp-fixture-1".to_string()),
            FixtureActivity::ToolResult("mcp-fixture-1".to_string(), ToolOutcome::Ok),
            FixtureActivity::Narration,
        ]
    );
    assert!(!replay.reports.iter().any(|report| match &report.activity {
        AgentActivity::ToolUse { call_id, .. } | AgentActivity::ToolResult { call_id, .. } =>
            call_id == "mcp-build-1",
        _ => false,
    }));
}

#[test]
fn synthetic_retry_and_terminal_errors_emit_separate_reports() {
    let fixture = checked_in_fixture("0.153.0/synthetic-model-events.jsonl");
    let mut errors = fixture.lines().filter_map(|line| {
        let envelope: Value = serde_json::from_str(line).unwrap();
        (envelope["method"] == "error").then(|| {
            match ServerNotification::decode("error", envelope["params"].clone()).unwrap() {
                ServerNotification::Error(error) => error,
                other => panic!("expected an error notification, got {other:?}"),
            }
        })
    });

    let retried = working_state()
        .transition(
            SessionEvent::ObservedError(errors.next().unwrap()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap();
    assert!(
        matches!(
            retried.effects.as_slice(),
            [SessionEffect::Report(report)]
                if matches!(&report.activity, AgentActivity::TaskUpdate { summary } if summary.contains("Temporary"))
        ),
        "{:?}",
        retried.effects
    );
    assert_eq!(retried.state.live_status(), Some(AgentStatus::Working));
    assert_eq!(retried.state.epitaph(), None);

    let terminal = retried
        .state
        .transition(
            SessionEvent::ObservedError(errors.next().unwrap()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap();
    assert!(
        matches!(
            terminal.effects.as_slice(),
            [SessionEffect::Report(report), SessionEffect::Close]
                if matches!(&report.activity, AgentActivity::TaskUpdate { summary } if summary.contains("Terminal"))
        ),
        "{:?}",
        terminal.effects
    );
    assert_eq!(terminal.state.live_status(), None);
    assert_eq!(
        terminal.state.epitaph().as_deref(),
        Some("Terminal synthetic fixture failure.")
    );
}

#[test]
fn context_compaction_and_review_mode_transitions_stay_visible() {
    let mut translator = CodexActivityTranslator::new(AppServerLimits::default().translator());
    let summaries = |reports: Vec<crate::harness::ActivityReport>| {
        reports
            .into_iter()
            .map(|report| match report.activity {
                AgentActivity::TaskUpdate { summary } => summary,
                other => panic!("expected a task update, got {other:?}"),
            })
            .collect::<Vec<_>>()
    };

    let compaction = json!({"id":"compaction-1","type":"contextCompaction"});
    let compaction_started = summaries(
        translator
            .translate("item/started", &item_envelope(compaction.clone()))
            .unwrap(),
    );
    let compaction_completed = summaries(
        translator
            .translate("item/completed", &item_envelope(compaction))
            .unwrap(),
    );
    assert_eq!(compaction_started.len(), 1);
    assert!(compaction_started[0].contains("Context compaction"));
    assert!(compaction_started[0].contains("started"));
    assert_eq!(compaction_completed.len(), 1);
    assert!(compaction_completed[0].contains("Context compaction"));
    assert!(compaction_completed[0].contains("completed"));

    let mut transitions = Vec::new();
    for (item, expected) in [
        (
            json!({"id":"review-1","type":"enteredReviewMode"}),
            "Entered review mode",
        ),
        (
            json!({"id":"review-2","type":"exitedReviewMode"}),
            "Exited review mode",
        ),
    ] {
        assert!(translator
            .translate("item/started", &item_envelope(item.clone()))
            .unwrap()
            .is_empty());
        let completed = summaries(
            translator
                .translate("item/completed", &item_envelope(item))
                .unwrap(),
        );
        assert_eq!(completed, vec![expected.to_string()]);
        transitions.extend(completed);
    }

    for summary in compaction_started
        .into_iter()
        .chain(compaction_completed)
        .chain(transitions)
    {
        assert!(summary.chars().count() <= crate::harness::adk::TOOL_SUMMARY_LIMIT);
    }
}

const HOSTILE_PARAM_TEXT: &str = "sk-fixture-secret; rm -rf /";

fn hostile_request_params(routing_id: &str) -> Value {
    json!({
        "threadId": routing_id,
        "conversationId": routing_id,
        "command": [HOSTILE_PARAM_TEXT],
        "reason": HOSTILE_PARAM_TEXT,
        "decision": "approve",
        "action": "accept",
        "currentTimeAt": 9_999_999_999i64,
        "nested": {"deeper": HOSTILE_PARAM_TEXT}
    })
}

#[test]
fn untrusted_request_params_never_reach_the_response_or_the_report() {
    let methods = [
        "item/commandExecution/requestApproval",
        "item/fileChange/requestApproval",
        "execCommandApproval",
        "applyPatchApproval",
        "mcpServer/elicitation/request",
        "item/tool/requestUserInput",
        "item/permissions/requestApproval",
        "item/tool/call",
        "account/chatgptAuthTokens/refresh",
        "attestation/generate",
        "currentTime/read",
        "future/request",
    ];
    for method in methods {
        for routing_id in ["thread-parent", CHILD_THREAD_ID] {
            let routed = routed_request(json!(5), method, hostile_request_params(routing_id));
            let decision = ServerRequestPolicy::decide(routed.request, routed.route, 4242);
            let written = serde_json::to_string(&decision.response).unwrap();
            for granted in [
                HOSTILE_PARAM_TEXT,
                "\"decision\":\"approve\"",
                "\"action\":\"accept\"",
                "approved",
                "allowed",
                "9999999999",
            ] {
                assert!(
                    !written.contains(granted),
                    "{method} {routing_id}: {written}"
                );
            }
            if let Some(report) = decision.report {
                assert!(
                    !report.activity.summary().contains(HOSTILE_PARAM_TEXT),
                    "{method} {routing_id}"
                );
            }
        }
    }
}

#[test]
fn a_hostile_error_message_reports_within_the_activity_bound() {
    let hostile = format!("secret\n{}", "A".repeat(8192));
    for will_retry in [true, false] {
        let notification = match ServerNotification::decode(
            "error",
            json!({
                "threadId": THREAD_ID,
                "turnId": TURN_ID,
                "error": {"message": hostile},
                "willRetry": will_retry
            }),
        )
        .unwrap()
        {
            ServerNotification::Error(error) => error,
            other => panic!("expected an error notification, got {other:?}"),
        };
        let transition = working_state()
            .transition(
                SessionEvent::ObservedError(notification),
                Duration::ZERO,
                limits().state(),
            )
            .unwrap();
        let summary = match transition.effects.first() {
            Some(SessionEffect::Report(report)) => report.activity.summary().to_string(),
            other => panic!("expected a report first, got {other:?}"),
        };
        assert_eq!(
            summary.chars().count(),
            crate::harness::adk::ACTIVITY_TEXT_LIMIT + 1,
            "{summary}"
        );
        assert!(summary.ends_with('…'), "{summary}");
        assert!(summary.contains('\n'), "{summary}");
    }
}

#[test]
fn the_app_server_child_inherits_no_agent_identity_and_scopes_its_mcp_token() {
    let options = spawn_options();
    let spec = super::CodexAppServerHarness
        .spec(&selected_choice(), &options, &harness_context())
        .expect("the app-server spec builds");

    for marker in crate::harness::INHERITED_AGENT_MARKERS {
        assert!(spec.unset.iter().any(|key| key == marker), "{marker}");
    }
    assert!(spec.env.is_empty(), "{:?}", spec.env);
    assert_eq!(
        spec.args
            .iter()
            .filter(|argument| argument.contains(&options.mcp_session_token))
            .collect::<Vec<_>>(),
        vec!["mcp_servers.build.env.BRIDGE_MCP_TOKEN=\"fixture-token\""]
    );
    assert!(spec
        .args
        .iter()
        .any(|argument| argument == "mcp_servers.build.required=true"));
    let enabled_tools = spec
        .args
        .iter()
        .find(|argument| argument.starts_with("mcp_servers.build.enabled_tools="))
        .expect("the Build MCP server enables only named tools");
    assert!(enabled_tools.contains("done"), "{enabled_tools}");
    assert!(
        !enabled_tools.contains("create_issue"),
        "{enabled_tools}: a coding owner gets no router tools"
    );
}

#[test]
fn checked_in_fixtures_retain_no_account_or_machine_material() {
    let home = std::env::var("HOME").expect("a home directory names this machine");
    let mut scanned_files: Vec<String> = CHECKED_IN_FIXTURES
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();
    scanned_files.sort();
    assert_eq!(
        corpus_file_names(),
        scanned_files,
        "every checked-in fixture under every version directory is scanned for account or machine material"
    );

    for (name, body) in CHECKED_IN_FIXTURES {
        let lowercased = body.to_lowercase();
        for secret_shape in [
            "/users/",
            "sk-",
            "bearer ",
            "eyj",
            "authorization",
            "access_token",
            "api_key",
            "accountid",
            "workspaceid",
            "@openai.com",
            "http://",
            "https://",
        ] {
            assert!(!lowercased.contains(secret_shape), "{name}: {secret_shape}");
        }
        assert!(!body.contains(&home), "{name} names this machine's home");
        for home_path in body.match_indices("/home/") {
            assert!(
                body[home_path.0..].starts_with("/home/fixture"),
                "{name} retains a real home path"
            );
        }
        for line in body.lines() {
            let envelope: Value =
                serde_json::from_str(line).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert!(
                envelope.get("direction").is_some() || envelope.get("method").is_some(),
                "{name}: every fixture line is a labelled protocol record"
            );
        }
    }
}
