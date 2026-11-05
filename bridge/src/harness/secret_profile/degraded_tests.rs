use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::harness::{harness_for, AgentSession, HarnessContext, SecretLaunchEnvironment};
use crate::models::{AgentProvider, ModelChoice};
use crate::pty::HarnessSpec;

const CLI: &str = r#"#!/usr/bin/env python3
import json, os, pathlib, sys, time
if '--fixture-agent' in sys.argv:
    marker = 'unsafe' if any(key.startswith('BRIDGE_SECRET_') for key in os.environ) else 'safe'
    if os.environ.get('BRIDGE_SECRET_TOKEN') == 'private-fixture-capability':
        marker = 'enabled'
    pathlib.Path(os.environ['FIXTURE_ENV_REPORT']).write_text(marker + '\ncomplete\n')
    sys.stdin.read()
    sys.exit(0)
home = pathlib.Path(os.environ['CODEX_HOME'])
mode = (home.parent / 'fixture-mode').read_text()
for line in sys.stdin:
    request = json.loads(line)
    method = request.get('method')
    if mode == 'config_timeout':
        time.sleep(30)
        continue
    if mode == 'legacy_profile':
        sys.stderr.write('legacy profile is unsupported: private-fixture-diagnostic')
        sys.exit(1)
    if method == 'initialize':
        result = {'userAgent': 'fixture/0.166.0'}
    elif method == 'config/read' and mode == 'ready':
        result = {'config': {'shell_environment_policy': {}}}
    elif method == 'config/read':
        code = -32601 if mode == 'unsupported' else -32600
        print(json.dumps({'id':request['id'],'error':{'code':code,'message':'private-fixture-diagnostic'}}), flush=True)
        continue
    else:
        continue
    print(json.dumps({'id':request['id'],'result':result}), flush=True)
"#;

fn fixture(root: &Path, mode: &str, provider: AgentProvider) -> (HarnessSpec, PathBuf) {
    let bin = root.join("bin");
    std::fs::create_dir(&bin).unwrap();
    crate::isolation::test_fixture::write_executable(&bin.join("codex"), CLI);
    let bridge = bin.join("build-bridge");
    crate::runtime_launcher::install_sibling(&bridge).unwrap();
    let target = root.join("user-home");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(root.join("fixture-mode"), mode).unwrap();
    let untouched = target.join(format!("build-secret-{}.config.toml", uuid::Uuid::new_v4()));
    std::fs::write(&untouched, "user-owned file must remain").unwrap();
    let home = match mode {
        "home_symlink" => {
            let link = root.join("home-link");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            link
        }
        "ancestor_symlink" => {
            let link = root.join("parent-link");
            std::os::unix::fs::symlink(root, &link).unwrap();
            link.join("user-home")
        }
        _ => target,
    };
    let context = HarnessContext {
        channel: crate::config::Channel::Dev,
        bridge_exe: bridge.clone(),
        mcp_socket: root.join("mcp.sock"),
        state_root: root.join("state"),
    };
    let options = crate::orchestrator::SpawnOptions {
        owner_id: "agent-fixture".into(),
        mcp_session_token: "ordinary-mcp-authority".into(),
        cwd: root.to_owned(),
        ..crate::orchestrator::SpawnOptions::default()
    };
    let spec = harness_for(provider)
        .spec(
            &ModelChoice {
                provider,
                ..ModelChoice::default()
            },
            &options,
            &context,
        )
        .unwrap()
        .arg("--fixture-agent")
        .env("CODEX_HOME", home.to_str().unwrap())
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env(
            "FIXTURE_ENV_REPORT",
            root.join("agent-report").to_str().unwrap(),
        );
    let spec = crate::harness::secret_launch::runtime_path(spec, &bridge).unwrap();
    let spec = crate::harness::secret_launch::apply(
        spec,
        Some(SecretLaunchEnvironment::new(
            "/fixture.sock".into(),
            "dev".into(),
            "agent-fixture".into(),
            "owner-fixture".into(),
            "private-fixture-capability".into(),
            bridge.to_str().unwrap().into(),
        )),
    );
    (
        spec.env("BRIDGE_SECRET_FUTURE", "private-future-capability"),
        untouched,
    )
}

fn await_report(root: &Path, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(report) = std::fs::read_to_string(root.join("agent-report")) {
            if let Some(report) = report.strip_suffix("\ncomplete\n") {
                assert_eq!(report, expected);
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("ordinary agent did not start");
}

fn spawn_ordinary(spec: &HarnessSpec, root: &Path, provider: AgentProvider) {
    spawn_reported(spec, root, provider, "safe");
}

fn spawn_reported(spec: &HarnessSpec, root: &Path, provider: AgentProvider, expected: &str) {
    if provider == AgentProvider::CodexAppServer {
        crate::harness::codex_app_server::with_spawned_process_for_test(spec, root, || {
            await_report(root, expected)
        });
    } else {
        let session = crate::pty::PtySession::spawn(
            spec,
            None,
            portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            },
        )
        .unwrap();
        await_report(root, expected);
        session.end();
    }
}

#[tokio::test]
async fn setup_failures_disable_secrets_and_allow_both_ordinary_codex_carriers_to_start() {
    const CHILD: &str = "BUILD_DEGRADED_SECRET_CHILD";
    if std::env::var_os(CHILD).is_none() {
        assert_failure_child(CHILD);
        return;
    }
    for mode in [
        "home_symlink",
        "ancestor_symlink",
        "config_error",
        "config_timeout",
        "unsupported",
        "legacy_profile",
    ] {
        for provider in [AgentProvider::Codex, AgentProvider::CodexAppServer] {
            let root = tempfile::tempdir().unwrap();
            let (spec, untouched) = fixture(root.path(), mode, provider);
            let original_args = spec.args.clone();
            let original_home = spec
                .env
                .iter()
                .find(|(key, _)| key == "CODEX_HOME")
                .unwrap()
                .1
                .clone();
            let ready = super::prepare(spec, root.path());
            assert_eq!(ready.args, original_args);
            assert!(ready
                .env
                .iter()
                .all(|(key, _)| !key.starts_with("BRIDGE_SECRET_")));
            assert!(ready.codex_shell_policy.is_none());
            assert!(ready.secret_profile.is_none());
            let path = ready
                .env
                .iter()
                .find(|(key, _)| key == "PATH")
                .unwrap()
                .1
                .clone();
            assert!(!std::env::split_paths(&path)
                .any(|entry| entry == root.path().join("bin/.build-runtime")));
            assert!(ready
                .env
                .iter()
                .any(|(key, value)| key == "CODEX_HOME" && value == &original_home));
            assert_eq!(
                std::fs::read_to_string(&untouched).unwrap(),
                "user-owned file must remain"
            );
            spawn_ordinary(&ready, root.path(), provider);
        }
    }
}

fn assert_failure_child(marker: &str) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "harness::secret_profile::degraded_tests::setup_failures_disable_secrets_and_allow_both_ordinary_codex_carriers_to_start", "--nocapture"])
        .env(marker, "1").output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        stderr
    );
    assert!(!stderr.contains("private-fixture"));
    assert!(!stderr.contains("private-future"));
    let warnings: Vec<_> = stderr
        .lines()
        .filter(|line| line.starts_with("secret runtime unavailable: "))
        .collect();
    assert_eq!(warnings.len(), 12, "{stderr}");
    for warning in warnings {
        assert!(
            [
                "home_symlink",
                "config_error",
                "config_timeout",
                "unsupported",
                "legacy_profile"
            ]
            .iter()
            .any(|reason| warning == format!("secret runtime unavailable: {reason}")),
            "{warning}"
        );
    }
}

#[tokio::test]
async fn registry_failure_disables_secrets_without_blocking_the_ordinary_tui() {
    let root = tempfile::tempdir().unwrap();
    let (spec, untouched) = fixture(root.path(), "ready", AgentProvider::Codex);
    let original_args = spec.args.clone();
    // The trusted state parent is absent, so registry creation cannot proceed.
    let ready = super::prepare(spec, root.path());
    assert_eq!(ready.args, original_args);
    assert!(ready
        .env
        .iter()
        .all(|(name, _)| !name.starts_with("BRIDGE_SECRET_")));
    assert!(ready.codex_shell_policy.is_none());
    assert!(ready.secret_profile.is_none());
    assert!(!root.path().join("state").exists());
    assert_eq!(
        std::fs::read_to_string(untouched).unwrap(),
        "user-owned file must remain"
    );
    spawn_ordinary(&ready, root.path(), AgentProvider::Codex);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_codex_spawns_retain_every_secret_capability() {
    let root = tempfile::tempdir().unwrap();
    let registry = root.path().join("shared-registry");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let mut workers = Vec::new();
    for index in 0..8 {
        let work = root.path().join(index.to_string());
        std::fs::create_dir(&work).unwrap();
        let registry = registry.clone();
        let barrier = barrier.clone();
        workers.push(tokio::task::spawn_blocking(move || {
            let (mut spec, _) = fixture(&work, "ready", AgentProvider::Codex);
            spec.secret_profile_registry_root = Some(registry);
            barrier.wait();
            let ready = super::prepare(spec, &work);
            assert!(ready.secret_profile.is_some(), "concurrent launch degraded");
            assert!(ready.codex_shell_policy.is_some());
            spawn_reported(&ready, &work, AgentProvider::Codex, "enabled");
        }));
    }
    for worker in workers {
        worker.await.unwrap();
    }
}
