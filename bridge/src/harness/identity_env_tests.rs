//! Every way the daemon starts an agent or probes a CLI leaves the daemon's
//! identity behind; the user's own terminal keeps it (#320). Each test runs
//! its spawn in a re-executed copy of this test binary that carries the
//! identity variables, so they never touch this runner's environment.

use std::path::Path;
use std::time::{Duration, Instant};

use super::{AgentSession, DAEMON_IDENTITY_VARS};
use crate::pty::HarnessSpec;

const CHILD: &str = "BUILD_DAEMON_IDENTITY_TEST_CHILD";
const SET: &str = "/nonexistent/daemon-identity";

/// True in the re-executed copy, where the test does its spawn. In the
/// runner, re-executes `test` with every identity variable set, asserts it
/// passed, and returns false.
fn in_child_with_daemon_identity(test: &str) -> bool {
    if std::env::var_os(CHILD).is_some() {
        return true;
    }
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", test, "--nocapture"])
        .env(CHILD, "1");
    for name in DAEMON_IDENTITY_VARS {
        command.env(name, SET);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{test}: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "{test} ran nothing: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    false
}

/// A shell line that writes each identity variable, or `unset`, to `out`.
fn report_identity(out: &Path) -> String {
    let names = DAEMON_IDENTITY_VARS
        .iter()
        .map(|name| format!("${{{name}-unset}}"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "printf '%s' \"{names}\" > '{}.tmp' && mv '{0}.tmp' '{0}'",
        out.display()
    )
}

fn none_set() -> String {
    vec!["unset"; DAEMON_IDENTITY_VARS.len()].join(",")
}

fn all_set() -> String {
    vec![SET; DAEMON_IDENTITY_VARS.len()].join(",")
}

fn reported(out: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(out) {
            return text;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the child never reported its environment");
}

/// A spec that reports what it inherited, then waits to be ended.
fn reporting_spec(out: &Path) -> HarnessSpec {
    HarnessSpec::new("sh")
        .arg("-c")
        .arg(format!("{}; exec cat >/dev/null", report_identity(out)))
}

/// Every provider's agent spec removes the identity, alongside the markers.
#[test]
fn every_agent_spec_unsets_the_daemon_identity() {
    use crate::models::{AgentProvider, ModelChoice};
    let worktree = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let options = crate::harness::SpawnOptions {
        owner_id: "agent-01J".to_string(),
        mcp_session_token: "token".to_string(),
        cwd: worktree.path().to_path_buf(),
        ..crate::harness::SpawnOptions::default()
    };
    let context = crate::harness::HarnessContext {
        bridge_exe: "/usr/local/bin/build-bridge".into(),
        mcp_socket: state.path().join("build-mcp.sock"),
        state_root: state.path().to_path_buf(),
    };
    for provider in AgentProvider::ALL {
        let choice = ModelChoice {
            provider,
            ..ModelChoice::default()
        };
        let spec = super::harness_for(provider)
            .spec(&choice, &options, &context)
            .unwrap_or_else(|error| panic!("{provider:?}: {error}"));
        for name in DAEMON_IDENTITY_VARS {
            assert!(
                spec.unset.iter().any(|key| key == name),
                "{provider:?} {name}"
            );
        }
    }
}

/// A PTY agent loses the identity; the user's terminal keeps it.
#[tokio::test]
async fn a_pty_agent_loses_the_identity_and_a_terminal_keeps_it() {
    if !in_child_with_daemon_identity(
        "harness::identity_env_tests::a_pty_agent_loses_the_identity_and_a_terminal_keeps_it",
    ) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let size = portable_pty::PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    };
    let agent_out = dir.path().join("agent");
    let agent = reporting_spec(&agent_out).unset_all(DAEMON_IDENTITY_VARS);
    let agent = crate::pty::PtySession::spawn(&agent, None, size).unwrap();
    assert_eq!(reported(&agent_out), none_set());
    agent.end();

    // The tab's own spec, with a "shell" that reports instead of prompting.
    let terminal_out = dir.path().join("terminal");
    let shell = dir.path().join("shell");
    std::fs::write(
        &shell,
        format!(
            "#!/bin/sh\n{}\nexec cat >/dev/null\n",
            report_identity(&terminal_out)
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shell, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let terminal = crate::app::shell_harness_spec_for_test(shell.to_str().unwrap());
    let terminal = crate::pty::PtySession::spawn(&terminal, None, size).unwrap();
    assert_eq!(reported(&terminal_out), all_set());
    terminal.end();
}

/// The ADK session's spawn applies the spec's unset list.
#[test]
fn an_adk_session_loses_the_identity() {
    if !in_child_with_daemon_identity(
        "harness::identity_env_tests::an_adk_session_loses_the_identity",
    ) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("adk");
    let spec = reporting_spec(&out).unset_all(DAEMON_IDENTITY_VARS);
    let choice = crate::models::ModelChoice {
        provider: crate::models::AgentProvider::ClaudeAdk,
        ..crate::models::ModelChoice::default()
    };
    let (session, _activity) = super::adk::AdkSession::spawn(&spec, None, &choice).unwrap();
    assert_eq!(reported(&out), none_set());
    session.end();
}

/// The codex app server's spawn applies the spec's unset list.
#[test]
fn a_codex_app_server_loses_the_identity() {
    if !in_child_with_daemon_identity(
        "harness::identity_env_tests::a_codex_app_server_loses_the_identity",
    ) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("app-server");
    let spec = reporting_spec(&out).unset_all(DAEMON_IDENTITY_VARS);
    super::codex_app_server::with_spawned_process_for_test(&spec, dir.path(), || {
        assert_eq!(reported(&out), none_set());
    });
}

/// A CLI probe runs without the identity.
#[test]
fn a_cli_probe_loses_the_identity() {
    if !in_child_with_daemon_identity("harness::identity_env_tests::a_cli_probe_loses_the_identity")
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("probe");
    super::installed::probe::command_for_test("sh", &["-c", &report_identity(&out)])
        .status()
        .unwrap();
    assert_eq!(reported(&out), none_set());
}

/// The codex app server's version probe runs without the identity.
#[test]
fn a_version_probe_loses_the_identity() {
    if !in_child_with_daemon_identity(
        "harness::identity_env_tests::a_version_probe_loses_the_identity",
    ) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("version");
    let cli = dir.path().join("codex");
    crate::isolation::test_fixture::write_executable(
        &cli,
        &format!(
            "#!/bin/sh\n{}\necho 'codex-cli 0.1.0'\n",
            report_identity(&out)
        ),
    );
    super::codex_app_server::version_probe_for_test(&cli).unwrap();
    assert_eq!(reported(&out), none_set());
}
