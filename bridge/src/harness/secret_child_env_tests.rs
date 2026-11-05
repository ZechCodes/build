//! Exercise the actual process carriers with shell children, without a provider login.
use std::path::Path;
use std::time::{Duration, Instant};

use super::{secret_launch, AgentSession, HarnessContext, SecretLaunchEnvironment};
use crate::models::{AgentProvider, ModelChoice};
use crate::pty::HarnessSpec;

fn reported(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(path) {
            return text;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("shell child never reported its launch environment");
}

fn shell_child(spec: &mut HarnessSpec, out: &Path) {
    spec.binary = "/bin/sh".into();
    // The second shell models a harness's shell tool. No capability is in argv.
    spec.args = vec![
        "-c".into(),
        format!(
            "/bin/sh -c 'printf \"%s\\n\" \"${{BRIDGE_SECRET_TOKEN-unset}}\" \"${{BRIDGE_SECRET_BRIDGE-unset}}\" \"${{BRIDGE_SECRET_FUTURE-unset}}\" \"$PATH\"' > '{}'; exec cat >/dev/null",
            out.display()
        ),
    ];
}

fn run_carrier(provider: AgentProvider, spec: &HarnessSpec, root: &Path, out: &Path) -> String {
    match provider {
        AgentProvider::ClaudeAdk => {
            let choice = ModelChoice {
                provider,
                ..ModelChoice::default()
            };
            let (session, _) = super::adk::AdkSession::spawn(spec, None, &choice).unwrap();
            let result = reported(out);
            session.end();
            result
        }
        AgentProvider::CodexAppServer => {
            let mut result = None;
            super::codex_app_server::with_spawned_process_for_test(spec, root, || {
                result = Some(reported(out));
            });
            result.unwrap()
        }
        _ => {
            let size = portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            };
            let session = crate::pty::PtySession::spawn(spec, None, size).unwrap();
            let result = reported(out);
            session.end();
            result
        }
    }
}

#[tokio::test]
async fn all_five_harness_shell_children_receive_their_own_capability_and_runtime_path() {
    let root = tempfile::tempdir().unwrap();
    let context = HarnessContext {
        channel: crate::config::Channel::Dev,
        bridge_exe: "/installed-build/bin/build-bridge".into(),
        mcp_socket: root.path().join("mcp.sock"),
        state_root: root.path().to_owned(),
    };
    for provider in AgentProvider::ALL {
        let choice = ModelChoice {
            provider,
            ..ModelChoice::default()
        };
        let options = crate::orchestrator::SpawnOptions {
            owner_id: "agent-fixture".into(),
            mcp_session_token: "separate-mcp-token".into(),
            cwd: root.path().to_owned(),
            ..crate::orchestrator::SpawnOptions::default()
        };
        let spec = super::harness_for(provider)
            .spec(&choice, &options, &context)
            .unwrap();
        if matches!(
            provider,
            AgentProvider::Codex | AgentProvider::CodexAppServer
        ) {
            assert!(
                spec.args
                    .windows(2)
                    .any(|args| args == ["--config", super::codex::SECRET_SHELL_ENVIRONMENT]),
                "{provider:?}"
            );
        }
        let spec = spec.env("BRIDGE_SECRET_FUTURE", "parent-capability");
        let launch = SecretLaunchEnvironment::new(
            "/fixture.sock".into(),
            "dev".into(),
            "agent-fixture".into(),
            "owner-fixture".into(),
            "own-secret-token".into(),
            context.bridge_exe.to_str().unwrap().into(),
        );
        let mut spec = secret_launch::runtime_path(
            secret_launch::apply(spec, Some(launch)),
            &context.bridge_exe,
        )
        .unwrap();
        let out = root.path().join(format!("{provider:?}.env"));
        shell_child(&mut spec, &out);
        let result = run_carrier(provider, &spec, root.path(), &out);
        let lines: Vec<_> = result.lines().collect();
        assert_eq!(
            &lines[..3],
            &[
                "own-secret-token",
                "/installed-build/bin/build-bridge",
                "unset"
            ],
            "{provider:?}"
        );
        assert!(
            lines[3].starts_with("/installed-build/bin:"),
            "{provider:?}"
        );
    }
}

#[tokio::test]
async fn inherited_capabilities_are_removed_from_new_agents_terminals_and_probes() {
    const CHILD: &str = "BUILD_SECRET_ENV_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "harness::secret_child_env_tests::inherited_capabilities_are_removed_from_new_agents_terminals_and_probes", "--nocapture"])
            .env(CHILD, "1")
            .env("BRIDGE_SECRET_TOKEN", "parent-token")
            .env("BRIDGE_SECRET_BRIDGE", "/parent/bin/build-bridge")
            .env("BRIDGE_SECRET_FUTURE", "parent-future")
            .output().unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let root = tempfile::tempdir().unwrap();
    for provider in AgentProvider::ALL {
        let out = root.path().join(format!("{provider:?}.env"));
        let mut spec = secret_launch::apply(HarnessSpec::new("fixture"), None);
        shell_child(&mut spec, &out);
        assert!(
            run_carrier(provider, &spec, root.path(), &out).starts_with("unset\nunset\nunset\n")
        );
    }
    let out = root.path().join("terminal.env");
    let mut terminal = crate::app::shell_harness_spec_for_test("/bin/sh");
    shell_child(&mut terminal, &out);
    assert!(
        run_carrier(AgentProvider::Claude, &terminal, root.path(), &out)
            .starts_with("unset\nunset\nunset\n")
    );
    let output = super::installed::probe::command_for_test(
        "/bin/sh",
        &[
            "-c",
            "printf '%s,%s' \"${BRIDGE_SECRET_TOKEN-unset}\" \"${BRIDGE_SECRET_FUTURE-unset}\"",
        ],
    )
    .output()
    .unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"unset,unset");
}
