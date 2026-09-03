use std::io::{Cursor, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use super::connection::{read_jsonl_frame, AppServerConnection};
use super::limits::AppServerLimits;
use super::policy::{AfterResponse, ServerRequestPolicy};
use super::protocol::{
    ClientNotification, ConnectionEvent, ItemType, PendingOperation, RpcError, ServerNotification,
    ServerRequest, ServerResponse, TurnCompletion,
};
use super::state::{CodexSessionState, SessionEffect, SessionEvent};
use super::translator::CodexActivityTranslator;
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
        .transition(SessionEvent::Start, Duration::ZERO, &limits())
        .unwrap()
        .state;
    state = state
        .transition(
            correlated(
                PendingOperation::Initialize,
                Ok(json!({"userAgent":"build_bridge/0.153.0 (fixture)"})),
            ),
            Duration::ZERO,
            &limits(),
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
            &limits(),
        )
        .unwrap()
        .state
}

#[test]
fn correlation_resolves_out_of_order_to_typed_operations() {
    let connection = AppServerConnection::memory(limits());
    let first = connection.request(PendingOperation::Initialize).unwrap();
    let second = connection.request(start_thread()).unwrap();

    let second_event = connection
        .decode(json!({"id":second,"result":{"thread":{"id":"t"},"model":"m","reasoningEffort":null,"cwd":"/tmp","approvalPolicy":"never","sandbox":{"type":"dangerFullAccess"}}}))
        .unwrap();
    let first_event = connection
        .decode(json!({"id":first,"result":{"userAgent":"build_bridge/0.153.0"}}))
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
        let connection = AppServerConnection::memory(limits());
        connection.request(PendingOperation::Initialize).unwrap();
        assert!(connection.decode(response).is_err());
    }
}

#[test]
fn pending_overflow_and_failed_write_leave_correlation_unchanged() {
    let mut small = limits();
    small.pending_requests = 1;
    let connection = AppServerConnection::memory(small);
    connection.request(PendingOperation::Initialize).unwrap();
    assert!(connection.request(start_thread()).is_err());
    assert_eq!(connection.pending_count(), 1);

    let failed = AppServerConnection::failing_writer(limits());
    assert!(failed.request(PendingOperation::Initialize).is_err());
    assert_eq!(failed.pending_count(), 0);
    assert!(failed
        .decode(json!({"id":1,"result":{"userAgent":"build_bridge/0.153.0"}}))
        .unwrap_err()
        .to_string()
        .contains("unknown response id 1"));

    let exhausted = AppServerConnection::memory(limits());
    exhausted.set_next_id(u64::MAX);
    assert!(exhausted.request(PendingOperation::Initialize).is_err());
    assert_eq!(exhausted.pending_count(), 0);
}

#[test]
fn bound_turn_request_needs_no_mutable_connection_context() {
    let connection = AppServerConnection::memory(limits());

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
        AppServerLimits::default(),
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
    let connection = AppServerConnection::memory(tiny);
    assert!(connection.request(PendingOperation::Initialize).is_err());
    assert_eq!(connection.pending_count(), 0);
    assert!(connection.close().is_ok());
    assert!(connection.close().is_ok());
    assert!(connection.notify(ClientNotification::Initialized).is_err());
}

#[test]
fn jsonl_decoder_accepts_crlf_and_exact_limit_but_rejects_bad_frames() {
    let valid = b"{\"method\":\"initialized\"}\r\n";
    assert_eq!(
        read_jsonl_frame(&mut Cursor::new(valid), valid.len(),).unwrap(),
        Some(json!({"method":"initialized"}))
    );

    for bad in [
        b"\n".to_vec(),
        b"{bad}\n".to_vec(),
        b"{} trailing\n".to_vec(),
        vec![0xff, b'\n'],
        b"{}".to_vec(),
    ] {
        assert!(
            read_jsonl_frame(&mut Cursor::new(bad.clone()), bad.len() + 2).is_err(),
            "{bad:?}"
        );
    }
    assert!(read_jsonl_frame(&mut Cursor::new(b"12345\n"), 4).is_err());
    let exact_crlf = b"1234\r\n";
    assert_eq!(
        read_jsonl_frame(&mut Cursor::new(exact_crlf), 4).unwrap(),
        Some(json!(1234))
    );
}

#[test]
fn initialize_is_first_and_a_turn_waits_for_readiness() {
    let state = initialized_state();
    let started = state
        .transition(SessionEvent::Start, Duration::ZERO, &limits())
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
            &limits(),
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
        .transition(SessionEvent::Start, Duration::ZERO, &limits())
        .unwrap()
        .state
        .transition(
            correlated(
                PendingOperation::Initialize,
                Ok(json!({"userAgent":"unparseable"})),
            ),
            Duration::ZERO,
            &limits(),
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
            &limits(),
        )
        .is_ok());

    let failure = initialized_state()
        .transition(SessionEvent::Start, Duration::ZERO, &limits())
        .unwrap()
        .state
        .transition(
            correlated(PendingOperation::Initialize, Ok(json!({}))),
            Duration::ZERO,
            &limits(),
        )
        .unwrap()
        .state
        .transition(
            SessionEvent::VersionEvidence(Err("exact probe failure".to_string())),
            Duration::ZERO,
            &limits(),
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
        .transition(SessionEvent::Start, Duration::ZERO, &limits())
        .unwrap()
        .state
        .transition(
            correlated(
                PendingOperation::Initialize,
                Ok(json!({"userAgent":"build_bridge/0.153.0"})),
            ),
            Duration::ZERO,
            &limits(),
        )
        .unwrap()
        .state;
    let notified = opening
        .transition(
            SessionEvent::ThreadStarted("thread-1".to_string()),
            Duration::ZERO,
            &limits(),
        )
        .unwrap()
        .state;
    let matching = notified.transition(correlated(start_thread(), Ok(json!({"thread":{"id":"thread-1"},"model":"gpt-5.6-sol","reasoningEffort":null,"cwd":"/tmp/worktree","approvalPolicy":"never","sandbox":{"type":"dangerFullAccess"}}))), Duration::ZERO, &limits()).unwrap();
    assert_eq!(matching.state.status(), AgentStatus::Waiting);

    let mismatch = opening.transition(SessionEvent::ThreadStarted("other".to_string()), Duration::ZERO, &limits()).unwrap().state
        .transition(correlated(start_thread(), Ok(json!({"thread":{"id":"thread-1"},"model":"gpt-5.6-sol","reasoningEffort":null,"cwd":"/tmp/worktree","approvalPolicy":"never","sandbox":{"type":"dangerFullAccess"}}))), Duration::ZERO, &limits());
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
            &limits(),
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
            &limits(),
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
            &limits(),
        )
        .unwrap()
        .state;
    assert_eq!(completed.status(), AgentStatus::Working);
    let settled = completed
        .transition(
            correlated(start_turn("go"), Ok(json!({"turn":{"id":"turn-1"}}))),
            Duration::from_secs(2),
            &limits(),
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
            &limits(),
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
            &limits(),
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
            &limits(),
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
            &limits(),
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
            &limits(),
        )
        .is_err());
}

#[test]
fn completion_before_start_response_applies_accepted_turn_facts() {
    let waiting = initialized_state()
        .transition(SessionEvent::Start, Duration::ZERO, &limits())
        .unwrap()
        .state
        .transition(
            correlated(
                PendingOperation::Initialize,
                Ok(json!({"userAgent":"build_bridge/0.153.0"})),
            ),
            Duration::ZERO,
            &limits(),
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
            &limits(),
        )
        .unwrap()
        .state;
    assert_eq!(waiting.active_effort().as_deref(), Some("low"));
    let starting = waiting
        .transition(
            SessionEvent::SendTurn("go".to_string()),
            Duration::ZERO,
            &limits(),
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
            &limits(),
        )
        .unwrap()
        .state;
    let settled = completed
        .transition(
            correlated(start_turn("go"), Ok(json!({"turn":{"id":"turn-1"}}))),
            Duration::from_secs(2),
            &limits(),
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
            &limits(),
        )
        .unwrap()
        .state
        .transition(
            SessionEvent::TurnStarted("turn-1".to_string()),
            Duration::ZERO,
            &limits(),
        )
        .unwrap()
        .state;
    assert!(starting
        .transition(
            correlated(start_turn("go"), Err(RpcError::new(-32000, "rejected"))),
            Duration::ZERO,
            &limits()
        )
        .is_err());

    let starting = advance_to_waiting(initialized_state())
        .transition(
            SessionEvent::SendTurn("go".to_string()),
            Duration::ZERO,
            &limits(),
        )
        .unwrap()
        .state;
    assert!(starting
        .transition(
            correlated(start_turn("go"), Ok(json!({"turn":{"id":"turn-2"}}))),
            Duration::ZERO,
            &limits()
        )
        .unwrap()
        .state
        .transition(
            SessionEvent::TurnStarted("turn-1".to_string()),
            Duration::ZERO,
            &limits()
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
            &limits(),
        )
        .unwrap();
    assert_eq!(first.effects.len(), 1);
    let second = first
        .state
        .transition(
            SessionEvent::SendTurn("second".to_string()),
            Duration::ZERO,
            &limits(),
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
            &limits(),
        )
        .unwrap()
        .state;
    let failed = starting
        .transition(
            SessionEvent::FailTurn("unsupported callback".to_string()),
            Duration::ZERO,
            &limits(),
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
            &limits(),
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
            &limits(),
        )
        .unwrap()
        .state;
    starting
        .transition(
            correlated(start_turn("go"), Ok(json!({"turn":{"id":"turn-1"}}))),
            Duration::ZERO,
            &limits(),
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
            &limits(),
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
            &limits(),
        )
        .unwrap();
    assert!(queued.effects.is_empty());
    let released = queued
        .state
        .transition(
            correlated(steer_turn("one"), Ok(json!({"turnId":"turn-1"}))),
            Duration::ZERO,
            &limits(),
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
            &limits(),
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
            &limits(),
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
            &limits(),
        )
        .unwrap();
    assert!(
        matches!(replayed.effects.as_slice(), [SessionEffect::Request(PendingOperation::StartTurn { input, .. })] if input == "next")
    );

    let pending = working_state()
        .transition(
            SessionEvent::SendTurn("next".to_string()),
            Duration::ZERO,
            &limits(),
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
            &limits(),
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
            &limits(),
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
            &limits(),
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
            &limits(),
        )
        .unwrap()
        .state;
    let settled = completed
        .transition(
            correlated(steer_turn("accepted"), Ok(json!({"turnId":"turn-1"}))),
            Duration::from_secs(2),
            &limits(),
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
            &limits(),
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
            &limits(),
        )
        .unwrap()
        .state;

    assert!(!completed.can_interrupt());
    assert!(completed
        .transition(SessionEvent::Interrupt, Duration::from_secs(1), &limits())
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
            &limits(),
        )
        .unwrap()
        .state;
    let interrupted = steering
        .transition(SessionEvent::Interrupt, Duration::ZERO, &limits())
        .unwrap()
        .state;
    let completed = interrupted
        .transition(
            SessionEvent::TurnCompleted {
                turn_id: "turn-1".to_string(),
                error: None,
            },
            Duration::from_secs(1),
            &limits(),
        )
        .unwrap()
        .state;
    let steer_settled = completed
        .transition(
            correlated(steer_turn("steer"), Ok(json!({"turnId":"turn-1"}))),
            Duration::from_secs(2),
            &limits(),
        )
        .unwrap();
    assert_eq!(steer_settled.state.status(), AgentStatus::Working);
    let interrupt_settled = steer_settled
        .state
        .transition(
            correlated(interrupt_turn(), Ok(json!({}))),
            Duration::from_secs(3),
            &limits(),
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
            &limits(),
        )
        .unwrap()
        .state;
    let interrupted = steering
        .transition(SessionEvent::Interrupt, Duration::ZERO, &limits())
        .unwrap()
        .state;
    let completed = interrupted
        .transition(
            SessionEvent::TurnCompleted {
                turn_id: "turn-1".to_string(),
                error: None,
            },
            Duration::from_secs(1),
            &limits(),
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
            &limits(),
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
            &limits(),
        )
        .unwrap()
        .state;
    assert_eq!(
        settled.epitaph().as_deref(),
        Some("interrupt transport rejected")
    );
}

#[test]
fn active_turn_not_steerable_remains_an_unsupported_steer_error() {
    let pending = working_state()
        .transition(
            SessionEvent::SendTurn("next".to_string()),
            Duration::ZERO,
            &limits(),
        )
        .unwrap()
        .state;
    let failure = pending
        .transition(
            correlated(
                steer_turn("next"),
                Err(RpcError::with_data(
                    -32600,
                    "cannot steer",
                    json!({"codexErrorInfo":{"activeTurnNotSteerable":{"turnKind":"review"}}}),
                )),
            ),
            Duration::from_secs(1),
            &limits(),
        )
        .unwrap_err();
    assert!(failure
        .to_string()
        .contains("Codex did not deliver steer input"));
}

#[test]
fn queue_limits_fail_without_partial_insertion() {
    let state = working_state()
        .transition(
            SessionEvent::SendTurn("pending".to_string()),
            Duration::ZERO,
            &limits(),
        )
        .unwrap()
        .state
        .transition(
            SessionEvent::SendTurn("123456".to_string()),
            Duration::ZERO,
            &limits(),
        )
        .unwrap()
        .state
        .transition(
            SessionEvent::SendTurn("abcdef".to_string()),
            Duration::ZERO,
            &limits(),
        )
        .unwrap()
        .state;
    assert!(state
        .transition(
            SessionEvent::SendTurn("overflow".to_string()),
            Duration::ZERO,
            &limits()
        )
        .is_err());
    assert_eq!(state.queued_turn_count(), 2);
}

#[test]
fn interrupt_is_idempotent_and_holds_replacement_until_completion() {
    let interrupted = working_state()
        .transition(SessionEvent::Interrupt, Duration::ZERO, &limits())
        .unwrap();
    assert_eq!(interrupted.effects.len(), 1);
    let duplicate = interrupted
        .state
        .transition(SessionEvent::Interrupt, Duration::ZERO, &limits())
        .unwrap();
    assert!(duplicate.effects.is_empty());
    let queued = duplicate
        .state
        .transition(
            SessionEvent::SendTurn("replacement".to_string()),
            Duration::ZERO,
            &limits(),
        )
        .unwrap();
    assert!(queued.effects.is_empty());
    let acked = queued
        .state
        .transition(
            correlated(interrupt_turn(), Ok(json!({}))),
            Duration::from_secs(1),
            &limits(),
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
            &limits(),
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
            &limits(),
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
            &limits(),
        )
        .unwrap()
        .state;
    assert!(provisional
        .transition(
            SessionEvent::CheckTimeouts,
            Duration::from_secs(6),
            &limits()
        )
        .is_err());
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
        let decision =
            ServerRequestPolicy::decide(ServerRequest::new(json!(7), method, json!({})), 1234);
        assert_eq!(
            decision.response,
            ServerResponse::result(json!(7), expected)
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
        let decision =
            ServerRequestPolicy::decide(ServerRequest::new(json!(8), method, json!({})), 0);
        assert_eq!(decision.response.error_code(), Some(-32601));
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
        let decision =
            ServerRequestPolicy::decide(ServerRequest::new(json!(9), method, json!({})), 0);
        assert_eq!(decision.response.error_code(), Some(-32601));
        assert!(matches!(
            decision.after_response,
            AfterResponse::FailSession(_)
        ));
    }
}

#[test]
fn completed_speech_and_tools_translate_without_raw_payloads() {
    let mut translator = CodexActivityTranslator::new(limits());
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
    assert_eq!(parent_item.item_type, ItemType::SubAgentActivity);
    assert!(state
        .transition(
            SessionEvent::TurnStarted(parent_item.turn_id.clone()),
            Duration::ZERO,
            &limits(),
        )
        .is_ok());
    let reports = CodexActivityTranslator::new(limits())
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
    let mut translator = CodexActivityTranslator::new(AppServerLimits::default());
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
    let mut translator = CodexActivityTranslator::new(AppServerLimits::default());
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
    let mut translator = CodexActivityTranslator::new(limits());
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
    let mut translator = CodexActivityTranslator::new(limits());
    for id in ["a", "b"] {
        translator.translate("item/started", &json!({"threadId":"thread-1","turnId":"turn-1","item":{"id":id,"type":"webSearch","query":"not retained"}})).unwrap();
    }
    let closed = translator.close_turn("turn-1");
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
    let mut translator = CodexActivityTranslator::new(bounded);
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

#[test]
fn synthetic_fixture_replays_all_required_item_kinds() {
    let fixture = include_str!(
        "../../../tests/fixtures/codex-app-server/0.153.0/synthetic-model-events.jsonl"
    );
    let mut translator = CodexActivityTranslator::new(AppServerLimits::default());
    let reports = fixture
        .lines()
        .flat_map(|line| {
            let envelope: Value = serde_json::from_str(line).unwrap();
            translator
                .translate(envelope["method"].as_str().unwrap(), &envelope["params"])
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert!(reports
        .iter()
        .any(|report| matches!(report.activity, AgentActivity::Reasoning { .. })));
    assert!(reports
        .iter()
        .any(|report| matches!(report.activity, AgentActivity::Narration { .. })));
    assert!(reports
        .iter()
        .any(|report| matches!(report.activity, AgentActivity::TaskUpdate { .. })));
    assert!(reports.iter().any(|report| matches!(
        report.activity,
        AgentActivity::ToolResult {
            outcome: ToolOutcome::Error,
            ..
        }
    )));
}
