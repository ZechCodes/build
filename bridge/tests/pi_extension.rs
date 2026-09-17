use std::io::{BufRead, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::Value;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn extension_copy(temp: &tempfile::TempDir) -> PathBuf {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/harness/build-tools.ts");
    let installed = temp.path().join("build-tools.mjs");
    std::fs::copy(source, &installed).unwrap();
    installed
}

fn fake_child(temp: &tempfile::TempDir) -> PathBuf {
    let child = temp.path().join("fake-build-bridge");
    std::fs::copy(fixture("pi-mcp-child.mjs"), &child).unwrap();
    std::fs::set_permissions(&child, std::fs::Permissions::from_mode(0o700)).unwrap();
    child
}

fn run_driver(mode: &str, scenario: &str) -> (Output, tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let extension = extension_copy(&temp);
    let child = fake_child(&temp);
    let log = temp.path().join("mcp.log");
    let pid = temp.path().join("mcp.pid");
    let session_started = temp.path().join("session.started");
    let output = Command::new("node")
        .arg(fixture("pi-extension-driver.mjs"))
        .env("BUILD_PI_EXTENSION_PATH", extension)
        .env("BUILD_PI_MCP_COMMAND", child)
        .env("BUILD_PI_MCP_OWNER", "agent-01J")
        .env("BRIDGE_MCP_SOCKET", "/tmp/build-mcp.sock")
        .env("BRIDGE_MCP_TOKEN", "rotated-token")
        .env("BUILD_PI_MCP_TIMEOUT_MS", "5000")
        .env("FAKE_MCP_MODE", mode)
        .env("FAKE_MCP_LOG", &log)
        .env("FAKE_MCP_PID", &pid)
        .env("PI_DRIVER_SCENARIO", scenario)
        .env("PI_DRIVER_SESSION_STARTED", session_started)
        .output()
        .unwrap();
    (output, temp, log, pid)
}

#[test]
fn registration_failures_abort_pi_and_close_and_reap_the_mcp_child() {
    for (scenario, expected_error) in [
        (
            "register_tool_failure",
            "Build Pi extension initialization failed: fixture registerTool failure",
        ),
        (
            "register_hook_failure",
            "Build Pi extension initialization failed: fixture hook registration failure",
        ),
    ] {
        let started = Instant::now();
        let (output, temp, log, pid) = run_driver("stubborn_close", scenario);
        assert_eq!(output.status.code(), Some(2), "{scenario}: {output:?}");
        assert!(started.elapsed() < Duration::from_secs(4));
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected_error),
            "{scenario}: {output:?}"
        );
        assert!(
            !temp.path().join("session.started").exists(),
            "{scenario} allowed a tool-less Pi session to continue"
        );
        assert!(
            std::fs::read_to_string(log).unwrap().contains("stdin_end"),
            "{scenario} did not close MCP stdin"
        );
        let child_pid: i32 = std::fs::read_to_string(pid).unwrap().parse().unwrap();
        assert_eq!(
            unsafe { libc::kill(child_pid, 0) },
            -1,
            "{scenario} did not reap the MCP child"
        );
    }
}

#[test]
fn empty_discovered_tool_set_aborts_pi_and_closes_and_reaps_the_mcp_child() {
    let (output, temp, log, pid) = run_driver("empty_tools", "empty_tools");
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Build Pi extension initialization failed: tools/list returned no tools"),
        "{output:?}"
    );
    assert!(
        !temp.path().join("session.started").exists(),
        "empty discovery allowed a tool-less Pi session to continue"
    );
    assert!(
        std::fs::read_to_string(log).unwrap().contains("stdin_end"),
        "empty discovery did not close MCP stdin"
    );
    let child_pid: i32 = std::fs::read_to_string(pid).unwrap().parse().unwrap();
    assert_eq!(
        unsafe { libc::kill(child_pid, 0) },
        -1,
        "empty discovery did not reap the MCP child"
    );
}

#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: extension_discovers_tools_forwards_calls_and_converts_only_text is at 30, threshold 15 — bring it under, then remove
fn extension_discovers_tools_forwards_calls_and_converts_only_text() {
    let (output, _temp, log, _pid) = run_driver("normal", "happy");
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["tools"][0]["name"], "done");
    assert_eq!(value["tools"][0]["label"], "done");
    assert_eq!(value["tools"][0]["description"], "canonical done");
    assert_eq!(value["tools"][0]["parameters"]["required"][0], "kind");
    assert_eq!(value["one"]["content"][0]["text"], "one");
    assert_eq!(value["many"]["content"][0]["text"], "first\nsecond");
    assert_eq!(value["empty"]["content"][0]["text"], "");
    assert_eq!(value["error"], "failed clearly");
    assert_eq!(
        value["emptyError"],
        "Build tool failed without an error message"
    );
    assert_eq!(
        value["malformed"],
        "Build tool returned an unsupported content block"
    );
    assert_eq!(
        value["unsupported"],
        "Build tool returned an unsupported content block"
    );
    assert_eq!(value["rpcError"], "canonical rpc error");
    assert_eq!(value["afterMalformed"]["content"][0]["text"], "one");
    assert_eq!(value["afterRpcError"]["content"][0]["text"], "one");
    assert_eq!(value["concurrent"][0]["content"][0]["text"], "first-call");
    assert_eq!(value["concurrent"][1]["content"][0]["text"], "second-call");
    assert_eq!(value["switchRefused"], true);
    assert_eq!(value["forkRefused"], true);

    let requests: Vec<Value> = std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        requests[0]["startup"]["argv"],
        serde_json::json!(["mcp", "--task", "agent-01J"])
    );
    assert_eq!(requests[0]["startup"]["socket"], "/tmp/build-mcp.sock");
    assert_eq!(requests[0]["startup"]["token"], "rotated-token");
    assert_eq!(requests[1]["method"], "initialize");
    assert_eq!(requests[2]["method"], "notifications/initialized");
    assert!(requests[2].get("id").is_none());
    assert_eq!(requests[3]["method"], "tools/list");
    assert_eq!(requests[4]["method"], "tools/call");
    assert_eq!(requests[4]["params"]["name"], "done");
    assert_eq!(requests[4]["params"]["arguments"]["kind"], "concurrent");
}

#[test]
fn extension_drains_saturated_stderr_and_closes_normally() {
    let (output, _temp, _log, pid) = run_driver("stderr_saturation", "happy");
    assert!(output.status.success(), "{output:?}");
    let child_pid: i32 = std::fs::read_to_string(pid).unwrap().parse().unwrap();
    assert_eq!(
        unsafe { libc::kill(child_pid, 0) },
        -1,
        "MCP child was not reaped"
    );
}

#[test]
fn extension_kills_a_child_that_refuses_normal_close() {
    let started = Instant::now();
    let (output, _temp, _log, pid) = run_driver("stubborn_close", "happy");
    assert!(output.status.success(), "{output:?}");
    assert!(started.elapsed() < Duration::from_secs(4));
    let child_pid: i32 = std::fs::read_to_string(pid).unwrap().parse().unwrap();
    assert_eq!(
        unsafe { libc::kill(child_pid, 0) },
        -1,
        "MCP child survived close"
    );
}

#[test]
fn terminal_protocol_failures_terminate_pi() {
    for mode in [
        "malformed_json",
        "malformed_frame",
        "invalid_envelope",
        "both_result_error",
        "unknown_id",
        "duplicate_id",
    ] {
        let (output, _temp, _log, _pid) = run_driver(mode, "terminal");
        assert_eq!(
            output.status.signal(),
            Some(libc::SIGTERM),
            "{mode}: {output:?}"
        );
    }
}

#[test]
fn child_death_with_a_pending_call_terminates_pi() {
    let (output, _temp, _log, _pid) = run_driver("child_exit", "happy");
    assert_eq!(output.status.signal(), Some(libc::SIGTERM), "{output:?}");
}

#[test]
fn child_death_latches_one_failure_for_pending_and_future_calls() {
    let (output, _temp, _log, _pid) = run_driver("child_exit", "latched");
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["pendingFailure"], value["futureFailure"]);
    assert!(value["pendingFailure"]
        .as_str()
        .unwrap()
        .contains("Build MCP"));
    assert_eq!(value["terminationSignals"][0]["signal"], "SIGTERM");
}

#[test]
fn invalid_error_rejects_every_pending_and_future_call_with_one_latched_failure() {
    let (output, _temp, _log, _pid) = run_driver("invalid_error", "latched_multiple");
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let expected = "Build MCP response has an invalid JSON-RPC error";
    assert_eq!(
        value["pendingFailures"],
        serde_json::json!([expected, expected])
    );
    assert_eq!(value["futureFailure"], expected);
    assert_eq!(value["terminationSignals"][0]["signal"], "SIGTERM");
}

#[test]
fn invalid_initialize_shapes_abort_initialization() {
    for mode in [
        "invalid_initialize_object",
        "invalid_initialize_protocol",
        "invalid_initialize_capabilities",
        "invalid_initialize_server",
    ] {
        let (output, _temp, _log, _pid) = run_driver(mode, "happy");
        assert!(!output.status.success(), "{mode}: {output:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("initialize result"),
            "{mode}: {output:?}"
        );
    }
}

#[test]
fn malformed_tool_list_fields_abort_initialization() {
    for mode in [
        "tool_entry_array",
        "empty_tool_name",
        "missing_tool_description",
        "array_tool_schema",
    ] {
        let (output, _temp, _log, _pid) = run_driver(mode, "happy");
        assert!(!output.status.success(), "{mode}: {output:?}");
    }
}

#[test]
fn framing_and_utf8_failures_are_terminal() {
    for mode in ["oversized_frame", "invalid_utf8"] {
        let (output, _temp, _log, _pid) = run_driver(mode, "terminal");
        assert_eq!(
            output.status.signal(),
            Some(libc::SIGTERM),
            "{mode}: {output:?}"
        );
    }
}

#[test]
fn write_failure_is_latched_and_terminates_pi() {
    let (output, _temp, _log, _pid) = run_driver("write_failure", "latched_write");
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(
        value["failure"]
            .as_str()
            .unwrap()
            .contains("Build MCP write failed"),
        "{value}"
    );
    assert_eq!(value["terminationSignals"][0]["signal"], "SIGTERM");
}

#[test]
fn malformed_discovered_tools_abort_initialization_and_reap_the_child() {
    let (output, _temp, _log, pid) = run_driver("duplicate_tools", "happy");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unique non-empty"),
        "{output:?}"
    );
    let child_pid: i32 = std::fs::read_to_string(pid).unwrap().parse().unwrap();
    assert_eq!(unsafe { libc::kill(child_pid, 0) }, -1);
}

#[test]
fn request_timeout_terminates_pi() {
    let temp = tempfile::tempdir().unwrap();
    let extension = extension_copy(&temp);
    let child = fake_child(&temp);
    let output = Command::new("node")
        .arg(fixture("pi-extension-driver.mjs"))
        .env("BUILD_PI_EXTENSION_PATH", extension)
        .env("BUILD_PI_MCP_COMMAND", child)
        .env("BUILD_PI_MCP_OWNER", "agent-timeout")
        .env("BRIDGE_MCP_SOCKET", "/tmp/build-mcp.sock")
        .env("BRIDGE_MCP_TOKEN", "rotated-token")
        .env("BUILD_PI_MCP_TIMEOUT_MS", "30")
        .env("FAKE_MCP_MODE", "timeout")
        .env("PI_DRIVER_SCENARIO", "timeout")
        .output()
        .unwrap();
    assert_eq!(output.status.signal(), Some(libc::SIGTERM), "{output:?}");
}

#[test]
fn missing_runtime_environment_aborts_before_spawning_mcp() {
    for missing in [
        "BUILD_PI_MCP_COMMAND",
        "BUILD_PI_MCP_OWNER",
        "BRIDGE_MCP_SOCKET",
        "BRIDGE_MCP_TOKEN",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let extension = extension_copy(&temp);
        let child = fake_child(&temp);
        let pid = temp.path().join("mcp.pid");
        let output = Command::new("node")
            .arg(fixture("pi-extension-driver.mjs"))
            .env("BUILD_PI_EXTENSION_PATH", extension)
            .env("BUILD_PI_MCP_COMMAND", child)
            .env("BUILD_PI_MCP_OWNER", "agent-01J")
            .env("BRIDGE_MCP_SOCKET", "/tmp/build-mcp.sock")
            .env("BRIDGE_MCP_TOKEN", "rotated-token")
            .env("FAKE_MCP_PID", &pid)
            .env_remove(missing)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{missing}");
        assert!(String::from_utf8_lossy(&output.stderr).contains(missing));
        assert!(!pid.exists(), "{missing} spawned the MCP child");
    }
}

#[test]
fn extension_uses_real_mcp_stdio_token_and_canonical_tool_errors() {
    let temp = tempfile::tempdir().unwrap();
    let extension = extension_copy(&temp);
    let socket = temp.path().join("mcp.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let received = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut frames = Vec::new();
        while frames.len() < 2 {
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(accepted) => break accepted,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "real MCP child did not send both the message action and report"
                        );
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("real MCP socket accept failed: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut line = String::new();
            std::io::BufReader::new(&stream)
                .read_line(&mut line)
                .unwrap();
            let frame = serde_json::from_str::<Value>(&line).unwrap();
            if frame.get("request").is_some() {
                writeln!(
                    stream,
                    "{}",
                    serde_json::json!({
                        "ok": true,
                        "result": { "message_id": "message-real-mcp" }
                    })
                )
                .unwrap();
            }
            frames.push(frame);
        }
        frames
    });
    let output = Command::new("node")
        .arg(fixture("pi-extension-driver.mjs"))
        .env("BUILD_PI_EXTENSION_PATH", extension)
        .env("BUILD_PI_MCP_COMMAND", env!("CARGO_BIN_EXE_build-bridge"))
        .env("BUILD_PI_MCP_OWNER", "agent-real-mcp")
        .env("BRIDGE_MCP_SOCKET", &socket)
        .env("BRIDGE_MCP_TOKEN", "rotated-real-token")
        .env("BUILD_PI_MCP_TIMEOUT_MS", "5000")
        .env("PI_DRIVER_SCENARIO", "real_mcp")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let extension_output: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(extension_output["retiredDoneRegistered"], false);
    assert_eq!(extension_output["invalidPostError"], "body is required");
    assert_eq!(
        extension_output["valid"]["content"][0]["text"],
        r#"{"message_id":"message-real-mcp"}"#
    );
    let forwarded = received.join().unwrap();
    let request = &forwarded[0];
    assert_eq!(request["task_id"], "agent-real-mcp");
    assert_eq!(request["session_token"], "rotated-real-token");
    assert_eq!(request["request"]["action"], "post_thread_message");
    assert_eq!(request["request"]["still_working"], false);
    assert_eq!(
        request["request"]["body"],
        "waiting for deterministic input"
    );
    let report = &forwarded[1];
    assert_eq!(report["task_id"], "agent-real-mcp");
    assert_eq!(report["session_token"], "rotated-real-token");
    assert_eq!(report["report"]["status"], "blocked");
    assert_eq!(
        report["report"]["outputs"]["message_id"],
        "message-real-mcp"
    );
}
