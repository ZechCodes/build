use std::io::{Cursor, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::connection::{AppServerConnection, ConnectionError};
use super::limits::AppServerLimits;
use super::policy::{AfterResponse, ServerRequestPolicy};
use super::protocol::{
    ClientNotification, ConnectionEvent, InboundServerRequest, ParentThreadFilter,
    ParentThreadRoute, PendingOperation, RoutedServerRequest, RpcError, ServerNotification,
    ServerRequest, TurnCompletion,
};
use super::state::{CodexSessionState, SessionEffect, SessionEvent};
use super::translator::{
    classify_item, CodexActivityTranslator, ItemClassification, ItemReportKind, SuppressionReason,
    ToolSummaryCategory,
};
use crate::harness::{AgentActivity, AgentStatus, ToolOutcome};
use crate::harness::{Harness, HarnessContext};
use crate::orchestrator::SpawnOptions;

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

fn initialized_state() -> CodexSessionState {
    CodexSessionState::new(
        PathBuf::from("/tmp/worktree"),
        Some("gpt-5.6-sol".to_string()),
        Some("high".to_string()),
        None,
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
        cwd: "/tmp/worktree".to_string(),
        model: Some("gpt-5.6-sol".to_string()),
    }
}

fn resume_thread() -> PendingOperation {
    PendingOperation::ResumeThread {
        thread_id: "thread-exact".to_string(),
        cwd: "/tmp/worktree".to_string(),
        model: Some("gpt-5.6-sol".to_string()),
    }
}

fn start_turn(input: &str) -> PendingOperation {
    PendingOperation::StartTurn {
        thread_id: "thread-1".to_string(),
        input: input.to_string(),
        model: Some("gpt-5.6-sol".to_string()),
        effort: Some("high".to_string()),
    }
}

fn steer_turn(input: &str) -> PendingOperation {
    PendingOperation::SteerTurn {
        thread_id: "thread-1".to_string(),
        turn_id: "turn-1".to_string(),
        input: input.to_string(),
    }
}

fn interrupt_turn() -> PendingOperation {
    PendingOperation::InterruptTurn {
        thread_id: "thread-1".to_string(),
        turn_id: "turn-1".to_string(),
    }
}

fn advance_to_waiting(mut state: CodexSessionState) -> CodexSessionState {
    state = state
        .transition(SessionEvent::Start, Duration::ZERO, limits().state())
        .unwrap()
        .state;
    state = state
        .transition(
            correlated(
                PendingOperation::Initialize,
                Ok(json!({"userAgent":"build_bridge/0.153.0 (fixture)"})),
            ),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    state
        .transition(
            correlated(
                start_thread(),
                Ok(json!({
                    "thread":{"id":"thread-1"},
                    "model":"gpt-5.6-sol",
                    "reasoningEffort":"high",
                    "cwd":"/tmp/worktree",
                    "approvalPolicy":"never",
                    "sandbox":{"type":"dangerFullAccess"}
                })),
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

#[test]
fn correlation_resolves_out_of_order_to_typed_operations() {
    let connection = AppServerConnection::memory(limits().connection());
    connection.request(PendingOperation::Initialize).unwrap();
    connection.request(start_thread()).unwrap();
    let [first, second] = connection.pending_ids()[..] else {
        panic!("two correlated requests are pending");
    };

    let second_event = decode(&connection, json!({"id":second,"result":{"thread":{"id":"t"},"model":"m","reasoningEffort":null,"cwd":"/tmp","approvalPolicy":"never","sandbox":{"type":"dangerFullAccess"}}}))
        .unwrap();
    let first_event = decode(
        &connection,
        json!({"id":first,"result":{"userAgent":"build_bridge/0.153.0"}}),
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
        connection.request(operation.clone()).unwrap();
        let [request_id] = connection.pending_ids()[..] else {
            panic!("one correlated request is pending");
        };

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
    connection.request(PendingOperation::Initialize).unwrap();
    let [request_id] = connection.pending_ids()[..] else {
        panic!("one correlated request is pending");
    };
    let response = json!({"id":request_id,"result":{"userAgent":"build_bridge/0.153.0"}});

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
        json!({"id":1,"result":{"userAgent":"build_bridge/0.153.0"}})
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
    assert!(frames[0].get("jsonrpc").is_none());
    assert!(frames[0]["params"]["capabilities"]
        .as_object()
        .unwrap()
        .is_empty());
    assert_eq!(frames[1]["method"], "thread/resume");
    assert_eq!(frames[1]["params"]["threadId"], "thread-exact");
    assert_eq!(frames[1]["params"]["model"], "gpt-5.6-sol");
    assert!(frames[1]["params"].get("effort").is_none());
    assert_eq!(frames[1]["params"]["approvalPolicy"], "never");
    assert_eq!(frames[1]["params"]["sandbox"], "danger-full-access");
    assert_eq!(frames[2]["params"]["model"], "gpt-5.6-sol");
    assert_eq!(frames[2]["params"]["effort"], "high");
    assert!(frames[3]["params"].get("model").is_none());
    assert!(frames[3]["params"].get("effort").is_none());
    assert_eq!(frames[3]["params"]["expectedTurnId"], "turn-1");
    assert_eq!(frames[4]["method"], "turn/interrupt");
    assert_eq!(frames[4]["params"]["threadId"], "thread-1");
    assert_eq!(frames[4]["params"]["turnId"], "turn-1");
    assert_eq!(frames[5], json!({"method":"initialized"}));
}

#[test]
fn outbound_limit_and_close_are_enforced_before_or_during_writes() {
    let mut tiny = limits();
    tiny.outbound_frame_bytes = 16;
    let connection = AppServerConnection::memory(tiny.connection());
    assert!(connection.request(PendingOperation::Initialize).is_err());
    assert_eq!(connection.pending_count(), 0);
    assert!(connection.close().is_ok());
    assert!(connection.close().is_ok());
    assert!(connection.notify(ClientNotification::Initialized).is_err());
}

#[test]
fn app_server_eof_ends_the_session_with_a_close_effect() {
    let transition = advance_to_waiting(initialized_state())
        .transition(SessionEvent::Eof, Duration::ZERO, limits().state())
        .unwrap();
    assert_eq!(transition.effects, vec![SessionEffect::Close]);
    assert_eq!(transition.state.status(), AgentStatus::Ended { code: None });
}

#[test]
fn initialize_is_first_and_a_turn_waits_for_readiness() {
    let state = initialized_state();
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
    assert_eq!(queued.state.status(), AgentStatus::Starting);
}

#[test]
fn initialize_version_floor_uses_only_the_leading_matching_component() {
    for passing in ["build_bridge/0.153.0", "build_bridge/0.154.1 (0.1.0)"] {
        assert!(
            CodexSessionState::validate_user_agent(passing).is_ok(),
            "{passing}"
        );
    }
    for probe_needed in [
        "other/0.153.0 build_bridge/9.0.0",
        "build_bridge/not-a-version 0.200.0",
        "0.153.0 build_bridge/0.153.0",
        "",
    ] {
        assert!(
            CodexSessionState::validate_user_agent(probe_needed).is_err(),
            "{probe_needed}"
        );
    }
    assert!(CodexSessionState::validate_user_agent("build_bridge/0.152.9").is_err());
}

#[test]
fn version_probe_fallback_accepts_success_and_preserves_failure() {
    let awaiting = initialized_state()
        .transition(SessionEvent::Start, Duration::ZERO, limits().state())
        .unwrap()
        .state
        .transition(
            correlated(
                PendingOperation::Initialize,
                Ok(json!({"userAgent":"unparseable"})),
            ),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap();
    assert_eq!(
        awaiting.effects,
        vec![SessionEffect::RequireVersionEvidence]
    );
    assert!(awaiting
        .state
        .transition(
            SessionEvent::VersionEvidence(Ok("codex-cli 0.153.0".to_string())),
            Duration::ZERO,
            limits().state(),
        )
        .is_ok());

    let failure = initialized_state()
        .transition(SessionEvent::Start, Duration::ZERO, limits().state())
        .unwrap()
        .state
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
        .unwrap_err();
    assert!(failure.to_string().contains("exact probe failure"));
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
    let fresh = initialized_state();
    let resumed = CodexSessionState::new(
        PathBuf::from("/tmp/worktree"),
        None,
        None,
        Some("thread-exact".to_string()),
    );
    assert!(fresh.resume_id().is_none());
    assert_eq!(resumed.resume_id(), Some("thread-exact"));
}

#[test]
fn thread_notification_and_response_orders_converge_and_ids_must_match() {
    let opening = initialized_state()
        .transition(SessionEvent::Start, Duration::ZERO, limits().state())
        .unwrap()
        .state
        .transition(
            correlated(
                PendingOperation::Initialize,
                Ok(json!({"userAgent":"build_bridge/0.153.0"})),
            ),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let notified = opening
        .transition(
            SessionEvent::ThreadStarted("thread-1".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let matching = notified.transition(correlated(start_thread(), Ok(json!({"thread":{"id":"thread-1"},"model":"gpt-5.6-sol","reasoningEffort":null,"cwd":"/tmp/worktree","approvalPolicy":"never","sandbox":{"type":"dangerFullAccess"}}))), Duration::ZERO, limits().state()).unwrap();
    assert_eq!(matching.state.status(), AgentStatus::Waiting);

    let mismatch = opening.transition(SessionEvent::ThreadStarted("other".to_string()), Duration::ZERO, limits().state()).unwrap().state
        .transition(correlated(start_thread(), Ok(json!({"thread":{"id":"thread-1"},"model":"gpt-5.6-sol","reasoningEffort":null,"cwd":"/tmp/worktree","approvalPolicy":"never","sandbox":{"type":"dangerFullAccess"}}))), Duration::ZERO, limits().state());
    assert!(mismatch.is_err());
}

#[test]
fn thread_response_before_notification_is_ready_and_the_duplicate_is_inert() {
    let waiting = advance_to_waiting(initialized_state());
    assert_eq!(waiting.status(), AgentStatus::Waiting);
    let duplicate = waiting
        .transition(
            SessionEvent::ThreadStarted("thread-1".to_string()),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap();
    assert!(duplicate.effects.is_empty());
    assert_eq!(duplicate.state.session_id().as_deref(), Some("thread-1"));
    assert_eq!(
        duplicate.state.active_model().as_deref(),
        Some("gpt-5.6-sol")
    );
}

#[test]
fn starting_turn_completion_before_response_never_resurrects_working() {
    let state = advance_to_waiting(initialized_state());
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
            SessionEvent::TurnCompleted {
                turn_id: "turn-1".to_string(),
                error: None,
            },
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap()
        .state;
    assert_eq!(completed.status(), AgentStatus::Working);
    let settled = completed
        .transition(
            correlated(start_turn("go"), Ok(json!({"turn":{"id":"turn-1"}}))),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap();
    assert_eq!(settled.state.status(), AgentStatus::Waiting);
}

#[test]
fn conflicting_duplicate_completion_is_rejected() {
    let starting = advance_to_waiting(initialized_state())
        .transition(
            SessionEvent::SendTurn("go".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    let completed = starting
        .transition(
            SessionEvent::TurnCompleted {
                turn_id: "turn-1".to_string(),
                error: Some("first failure".to_string()),
            },
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap()
        .state;

    let normalized_duplicate = completed
        .transition(
            SessionEvent::TurnCompleted {
                turn_id: "turn-1".to_string(),
                error: Some("  first   failure\n".to_string()),
            },
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap();
    assert!(normalized_duplicate.effects.is_empty());

    assert!(normalized_duplicate
        .state
        .transition(
            SessionEvent::ObservedCompletion(TurnCompletion::observed(
                "turn-1".to_string(),
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
            SessionEvent::TurnCompleted {
                turn_id: "turn-1".to_string(),
                error: Some("different failure".to_string()),
            },
            Duration::from_secs(4),
            limits().state(),
        )
        .is_err());
}

#[test]
fn completion_before_start_response_applies_accepted_turn_facts() {
    let waiting = initialized_state()
        .transition(SessionEvent::Start, Duration::ZERO, limits().state())
        .unwrap()
        .state
        .transition(
            correlated(
                PendingOperation::Initialize,
                Ok(json!({"userAgent":"build_bridge/0.153.0"})),
            ),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state
        .transition(
            correlated(
                start_thread(),
                Ok(json!({
                    "thread":{"id":"thread-1"},
                    "model":"gpt-5.6-sol",
                    "reasoningEffort":"low",
                    "cwd":"/tmp/worktree",
                    "approvalPolicy":"never",
                    "sandbox":{"type":"dangerFullAccess"}
                })),
            ),
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
            SessionEvent::TurnCompleted {
                turn_id: "turn-1".to_string(),
                error: None,
            },
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap()
        .state;
    let settled = completed
        .transition(
            correlated(start_turn("go"), Ok(json!({"turn":{"id":"turn-1"}}))),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap()
        .state;

    assert_eq!(settled.active_effort().as_deref(), Some("high"));
}

#[test]
fn observed_turn_then_start_error_and_wrong_ids_fail() {
    let starting = advance_to_waiting(initialized_state())
        .transition(
            SessionEvent::SendTurn("go".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state
        .transition(
            SessionEvent::TurnStarted("turn-1".to_string()),
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

    let starting = advance_to_waiting(initialized_state())
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
            SessionEvent::TurnStarted("turn-1".to_string()),
            Duration::ZERO,
            limits().state()
        )
        .is_err());
}

#[test]
fn two_sends_during_start_issue_only_one_turn_start() {
    let state = advance_to_waiting(initialized_state());
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
fn failed_starting_turn_is_interrupted_after_its_id_is_confirmed() {
    let starting = advance_to_waiting(initialized_state())
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
            correlated(start_turn("go"), Ok(json!({"turn":{"id":"turn-1"}}))),
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap();
    assert!(confirmed.effects.iter().any(|effect| matches!(
        effect,
        SessionEffect::Request(PendingOperation::InterruptTurn { turn_id, .. })
            if turn_id == "turn-1"
    )));
    assert!(!confirmed.state.can_interrupt());
}

fn working_state() -> CodexSessionState {
    let starting = advance_to_waiting(initialized_state())
        .transition(
            SessionEvent::SendTurn("go".to_string()),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap()
        .state;
    starting
        .transition(
            correlated(start_turn("go"), Ok(json!({"turn":{"id":"turn-1"}}))),
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
            correlated(steer_turn("one"), Ok(json!({"turnId":"turn-1"}))),
            Duration::ZERO,
            limits().state(),
        )
        .unwrap();
    assert!(
        matches!(released.effects.as_slice(), [SessionEffect::Request(PendingOperation::SteerTurn { input, .. })] if input == "two")
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
            SessionEvent::TurnCompleted {
                turn_id: "turn-1".to_string(),
                error: None,
            },
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
            SessionEvent::TurnCompleted {
                turn_id: "turn-1".to_string(),
                error: None,
            },
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
            SessionEvent::TurnCompleted {
                turn_id: "turn-1".to_string(),
                error: None,
            },
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap()
        .state;
    let settled = completed
        .transition(
            correlated(steer_turn("accepted"), Ok(json!({"turnId":"turn-1"}))),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap();
    assert_eq!(settled.state.status(), AgentStatus::Waiting);
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
            SessionEvent::TurnCompleted {
                turn_id: "turn-1".to_string(),
                error: None,
            },
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
            SessionEvent::TurnCompleted {
                turn_id: "turn-1".to_string(),
                error: None,
            },
            Duration::from_secs(1),
            limits().state(),
        )
        .unwrap()
        .state;
    let steer_settled = completed
        .transition(
            correlated(steer_turn("steer"), Ok(json!({"turnId":"turn-1"}))),
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap();
    assert_eq!(steer_settled.state.status(), AgentStatus::Working);
    let interrupt_settled = steer_settled
        .state
        .transition(
            correlated(interrupt_turn(), Ok(json!({}))),
            Duration::from_secs(3),
            limits().state(),
        )
        .unwrap();
    assert_eq!(interrupt_settled.state.status(), AgentStatus::Waiting);
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
            SessionEvent::TurnCompleted {
                turn_id: "turn-1".to_string(),
                error: None,
            },
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
            correlated(steer_turn("steer"), Ok(json!({"turnId":"turn-1"}))),
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
    assert_eq!(waiting.state.status(), AgentStatus::Working);

    let replayed = waiting
        .state
        .transition(
            SessionEvent::TurnCompleted {
                turn_id: "turn-1".to_string(),
                error: None,
            },
            Duration::from_secs(2),
            limits().state(),
        )
        .unwrap();
    assert!(matches!(
        replayed.effects.as_slice(),
        [SessionEffect::CloseTurn(turn_id), SessionEffect::Request(PendingOperation::StartTurn { input, .. })]
            if turn_id == "turn-1" && input == "next"
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
            SessionEvent::TurnCompleted {
                turn_id: "turn-1".to_string(),
                error: None,
            },
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
            SessionEvent::TurnCompleted {
                turn_id: "turn-1".to_string(),
                error: None,
            },
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
fn error_notifications_require_the_typed_liveness_shape() {
    assert!(matches!(
        ServerNotification::decode(
            "error",
            json!({
                "threadId":"thread-1",
                "turnId":"turn-1",
                "error":{"message":"retry failed"},
                "willRetry":false
            })
        )
        .unwrap(),
        ServerNotification::Error(error)
            if error.thread_id == "thread-1"
                && error.turn_id == "turn-1"
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
    let child_thread = "thread-child";
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
            json!({"threadId":"thread-child"}),
            json!({"id":7,"result":{"decision":"decline"}}),
        ),
        (
            "item/fileChange/requestApproval",
            json!({"threadId":"thread-child"}),
            json!({"id":7,"result":{"decision":"decline"}}),
        ),
        (
            "execCommandApproval",
            json!({"conversationId":"thread-child"}),
            json!({"id":7,"result":{"decision":{"denied":{"rejection":"Build does not approve commands"}}}}),
        ),
        (
            "applyPatchApproval",
            json!({"conversationId":"thread-child"}),
            json!({"id":7,"result":{"decision":{"denied":{"rejection":"Build does not approve file changes"}}}}),
        ),
        (
            "mcpServer/elicitation/request",
            json!({"threadId":"thread-child"}),
            json!({"id":7,"result":{"action":"decline"}}),
        ),
        (
            "item/tool/requestUserInput",
            json!({"threadId":"thread-child"}),
            json!({"id":7,"error":{"code":-32601,"message":"method not supported"}}),
        ),
        (
            "item/permissions/requestApproval",
            json!({"threadId":"thread-child"}),
            json!({"id":7,"error":{"code":-32601,"message":"method not supported"}}),
        ),
        (
            "item/tool/call",
            json!({"threadId":"thread-child"}),
            json!({"id":7,"error":{"code":-32601,"message":"method not supported"}}),
        ),
        (
            "currentTime/read",
            json!({"threadId":"thread-child"}),
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
        json!({"threadId":"thread-child","command":42,"cwd":[]}),
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
        for params in [json!({"threadId":"thread-child"}), Value::Null, json!([])] {
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
                .translate(
                    method,
                    &json!({"threadId":"thread-1","turnId":"turn-1","item":item})
                )
                .unwrap()
                .is_empty());
        }
    }
    assert_eq!(translator.open_item_count(), 0);
    assert_eq!(translator.completed_item_count(), 0);
}

#[test]
fn completed_speech_and_tools_translate_without_raw_payloads() {
    let mut translator = CodexActivityTranslator::new(limits().translator());
    let reasoning = translator.translate("item/completed", &json!({"threadId":"thread-1","turnId":"turn-1","item":{"id":"r","type":"reasoning","summary":["final summary"]}})).unwrap();
    assert_eq!(
        reasoning[0].activity,
        AgentActivity::Reasoning {
            summary: "final summary".to_string()
        }
    );
    let narration = translator.translate("item/completed", &json!({"threadId":"thread-1","turnId":"turn-1","item":{"id":"n","type":"agentMessage","text":"finished"}})).unwrap();
    assert_eq!(
        narration[0].activity,
        AgentActivity::Narration {
            summary: "finished".to_string()
        }
    );

    let started = translator.translate("item/started", &json!({"threadId":"thread-1","turnId":"turn-1","item":{"id":"c","type":"commandExecution","command":"secret command","status":"inProgress"}})).unwrap();
    assert!(
        matches!(&started[0].activity, AgentActivity::ToolUse { call_id, summary } if call_id == "c" && !summary.contains("secret"))
    );
    let completed = translator.translate("item/completed", &json!({"threadId":"thread-1","turnId":"turn-1","item":{"id":"c","type":"commandExecution","command":"secret command","aggregatedOutput":"secret output","status":"completed","exitCode":0}})).unwrap();
    assert!(
        matches!(&completed[0].activity, AgentActivity::ToolResult { call_id, outcome: ToolOutcome::Ok, summary } if call_id == "c" && !summary.contains("secret"))
    );
}

#[test]
fn duplicate_completed_items_emit_once_and_cannot_reopen_tools() {
    let mut translator = CodexActivityTranslator::new(limits().translator());
    let completed_speech = json!({
        "threadId":"thread-1",
        "turnId":"turn-1",
        "item":{"id":"speech","type":"agentMessage","text":"once"}
    });
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

    let started_tool = json!({
        "threadId":"thread-1",
        "turnId":"turn-1",
        "item":{"id":"tool","type":"webSearch"}
    });
    let completed_tool = json!({
        "threadId":"thread-1",
        "turnId":"turn-1",
        "item":{"id":"tool","type":"webSearch","status":"completed"}
    });
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
        json!({
            "threadId":"thread-1",
            "turnId":turn,
            "item":{"id":id,"type":"agentMessage","text":id}
        })
    };

    for id in ["a", "b"] {
        assert_eq!(
            translator
                .translate("item/completed", &completed("turn-1", id))
                .unwrap()
                .len(),
            1
        );
    }
    assert_eq!(
        translator
            .translate("item/completed", &completed("turn-1", "c"))
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
    translator.close_turn("turn-1").unwrap();
    assert_eq!(translator.completed_item_count(), 1);
}

#[test]
fn completed_item_lru_evicts_non_fatally_and_refreshes_duplicates() {
    let mut bounded = limits();
    bounded.completed_items = 2;
    bounded.completed_item_bytes = 64;
    let mut translator = CodexActivityTranslator::new(bounded.translator());
    let completed = |id: &str| {
        json!({
            "threadId":"thread-1",
            "turnId":"turn-1",
            "item":{"id":id,"type":"agentMessage","text":id}
        })
    };

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
fn more_than_256_valid_completions_remain_live_and_turn_close_clears_keys() {
    let mut translator = CodexActivityTranslator::new(AppServerLimits::default().translator());
    for index in 0..300 {
        let reports = translator
            .translate(
                "item/completed",
                &json!({
                    "threadId":"thread-1",
                    "turnId":"turn-1",
                    "item":{
                        "id":format!("message-{index}"),
                        "type":"agentMessage",
                        "text":format!("message {index}")
                    }
                }),
            )
            .unwrap();
        assert_eq!(reports.len(), 1);
    }
    assert_eq!(translator.completed_item_count(), 256);
    translator.close_turn("turn-1").unwrap();
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
                "id":"thread-child",
                "parentThreadId":"thread-1"
            }
        }),
    )
    .unwrap();
    assert!(matches!(
        child_thread,
        ServerNotification::ThreadStarted {
            parent_thread_id: Some(parent),
            ..
        } if parent == "thread-1"
    ));
    let child = ServerNotification::decode(
        "item/started",
        json!({
            "threadId":"thread-child",
            "turnId":"turn-child",
            "item":{"id":"child-command","type":"commandExecution"}
        }),
    )
    .unwrap();
    let ServerNotification::Item(child_item) = child else {
        panic!("expected a typed child item");
    };
    assert!(!state.parent_thread_matches(&child_item.thread_id));

    let parent = ServerNotification::decode(
        "item/completed",
        json!({
            "threadId":"thread-1",
            "turnId":"turn-1",
            "item":{
                "id":"subagent",
                "type":"subAgentActivity",
                "agentPath":"worker",
                "agentThreadId":"thread-child",
                "kind":"completed"
            }
        }),
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
fn speech_summaries_share_the_activity_summary_bound() {
    let mut translator = CodexActivityTranslator::new(AppServerLimits::default().translator());
    for item in [
        json!({"id":"r","type":"reasoning","summary":["x".repeat(1000)]}),
        json!({"id":"n","type":"agentMessage","text":"y".repeat(1000)}),
    ] {
        let reports = translator
            .translate(
                "item/completed",
                &json!({"threadId":"thread-1","turnId":"turn-1","item":item}),
            )
            .unwrap();
        let summary = match &reports[0].activity {
            AgentActivity::Reasoning { summary } | AgentActivity::Narration { summary } => summary,
            other => panic!("expected speech report, got {other:?}"),
        };
        assert!(summary.chars().count() <= crate::harness::adk::TOOL_SUMMARY_LIMIT + 1);
    }
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
            .translate(
                "item/started",
                &json!({"threadId":"thread-1","turnId":"turn-1","item":started_item}),
            )
            .unwrap();
        assert_eq!(started.len(), 1, "{item:?}");
        let mut completed_item = item;
        completed_item["id"] = json!(id);
        completed_item["status"] = json!("completed");
        let completed = translator
            .translate(
                "item/completed",
                &json!({"threadId":"thread-1","turnId":"turn-1","item":completed_item}),
            )
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
fn build_mcp_dynamic_and_unknown_items_are_suppressed() {
    let mut translator = CodexActivityTranslator::new(limits().translator());
    for item in [
        json!({"id":"build","type":"mcpToolCall","server":"build","tool":"done","status":"inProgress","arguments":{"secret":true}}),
        json!({"id":"dynamic","type":"dynamicToolCall","tool":"later","status":"inProgress","arguments":{}}),
        json!({"id":"future","type":"newItem"}),
    ] {
        assert!(translator
            .translate(
                "item/started",
                &json!({"threadId":"thread-1","turnId":"turn-1","item":item})
            )
            .unwrap()
            .is_empty());
    }
}

#[test]
fn open_tools_close_unanswered_and_release_limits() {
    let mut translator = CodexActivityTranslator::new(limits().translator());
    for id in ["a", "b"] {
        translator.translate("item/started", &json!({"threadId":"thread-1","turnId":"turn-1","item":{"id":id,"type":"webSearch","query":"not retained"}})).unwrap();
    }
    let closed = translator.close_turn("turn-1").unwrap();
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
fn completing_an_item_releases_its_aggregate_byte_charge() {
    let mut bounded = limits();
    bounded.open_item_bytes = 20;
    let mut translator = CodexActivityTranslator::new(bounded.translator());
    let envelope = |id: &str, status: &str| json!({"threadId":"thread-1","turnId":"turn-1","item":{"id":id,"type":"webSearch","query":"not retained","status":status}});
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
    let options = SpawnOptions {
        owner_id: "run-1".to_string(),
        cwd: PathBuf::from("/tmp/worktree"),
        mcp_session_token: "fixture-token".to_string(),
        ..SpawnOptions::default()
    };
    let context = HarnessContext {
        bridge_exe: "/usr/local/bin/build-bridge".to_string(),
        mcp_socket: "/tmp/build.sock".to_string(),
    };
    let choice = crate::models::ModelChoice {
        provider: crate::models::AgentProvider::CodexAppServer,
        model: Some("gpt-5.6-sol".to_string()),
        effort: Some("high".to_string()),
    };
    let app = super::CodexAppServerHarness.spec(&choice, &options, &context);
    let tui = crate::harness::codex::CodexHarness.spec(&choice, &options, &context);
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
    let replay = replay_observed_fixture(include_str!(
        "../../../tests/fixtures/codex-app-server/0.153.0/observed-session-start.jsonl"
    ));
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
    let replay = replay_observed_fixture(include_str!(
        "../../../tests/fixtures/codex-app-server/0.153.0/observed-session-resume.jsonl"
    ));
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
    let replay = replay_observed_fixture(include_str!(
        "../../../tests/fixtures/codex-app-server/0.153.0/observed-session-mcp.jsonl"
    ));
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
    let fixture = include_str!(
        "../../../tests/fixtures/codex-app-server/0.153.0/synthetic-model-events.jsonl"
    );
    let mut translator = CodexActivityTranslator::new(AppServerLimits::default().translator());
    let reports = fixture
        .lines()
        .filter_map(|line| {
            let envelope: Value = serde_json::from_str(line).unwrap();
            (envelope["method"] == "error").then_some(envelope)
        })
        .flat_map(|envelope| {
            translator
                .translate(envelope["method"].as_str().unwrap(), &envelope["params"])
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(reports.len(), 2);
    assert!(matches!(
        &reports[0].activity,
        AgentActivity::TaskUpdate { summary } if summary.contains("Temporary")
    ));
    assert!(matches!(
        &reports[1].activity,
        AgentActivity::TaskUpdate { summary } if summary.contains("Terminal")
    ));
}
