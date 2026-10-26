use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::connection::AppServerConnection;
use super::fixtures::{harness_context, selected_choice, spawn_options};
use super::limits::AppServerLimits;
use super::protocol::PendingOperation;
use crate::compaction::{CompactionSource, ResolvedCompactionPolicy};
use crate::harness::compaction::{
    AppliedCompactionPolicy, CompactionEnforcement, CompactionLaunchIdentity,
    NativeCompactionSettings,
};
use crate::harness::Harness;

fn native_policy(tokens: u64) -> AppliedCompactionPolicy {
    let mut policy = super::CodexAppServerHarness
        .prepare_compaction_policy(
            ResolvedCompactionPolicy {
                tokens,
                source: CompactionSource::Device,
            },
            selected_choice(),
            Some("0.160.0".into()),
            CompactionLaunchIdentity::new("build-thread"),
        )
        .unwrap();
    policy.enforcement = CompactionEnforcement::Native;
    policy.translated_trigger = Some(tokens);
    policy.native_settings = Some(NativeCompactionSettings::Codex(
        crate::harness::codex_compaction::Translation {
            token_limit: tokens,
            model_window: 1_000_000,
            trigger: tokens,
        },
    ));
    policy
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn compaction_config_is_scoped_to_thread_open_requests_only() {
    let capture = Capture::default();
    let config = json!({"model_auto_compact_token_limit": 150000,
        "model_auto_compact_token_limit_scope": "total"});
    let connection = AppServerConnection::new(
        Box::new(capture.clone()),
        AppServerLimits::default().connection(),
    )
    .with_thread_config(Some(config.clone()));
    for operation in [
        PendingOperation::Initialize,
        PendingOperation::StartThread {
            cwd: "/tmp/test".into(),
            model: None,
        },
        PendingOperation::ResumeThread {
            thread_id: "resume".into(),
            cwd: "/tmp/test".into(),
            model: None,
        },
        PendingOperation::CompactThread {
            thread_id: "resume".into(),
        },
        PendingOperation::StartTurn {
            thread_id: "resume".into(),
            input: "continue".into(),
            model: None,
            effort: None,
        },
    ] {
        connection.request(operation).unwrap();
    }
    let bytes = capture.0.lock().unwrap();
    let frames: Vec<Value> = bytes
        .split(|b| *b == b'\n')
        .filter(|b| !b.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    for frame in frames {
        if matches!(
            frame["method"].as_str(),
            Some("thread/start" | "thread/resume")
        ) {
            assert_eq!(frame["params"]["config"], config);
        } else {
            assert!(frame["params"].get("config").is_none(), "{frame}");
        }
    }
}

const CHILD: &str = r#"
import json, pathlib, sys, tomllib
root = pathlib.Path.cwd()
launch = {}
if any(arg == '--profile' or arg.startswith('--profile=') for arg in sys.argv[1:]):
    raise RuntimeError('fixture cannot qualify file profile precedence')
args = iter(sys.argv[1:])
for arg in args:
    if arg == '--config':
        key, value = next(args).split('=', 1)
        if key.startswith('model_auto_compact_'):
            launch[key] = json.loads(value)
for line in sys.stdin:
    request = json.loads(line)
    method = request.get('method')
    if method == 'initialize':
        result = {'userAgent':'build_bridge/0.160.0 (fixture)'}
    elif method == 'config/read':
        settings = tomllib.loads((root / 'fake-home/.codex/config.toml').read_text())
        if 'profile' in settings:
            print(json.dumps({'id':request['id'], 'error':{'code':-32603, 'message':'legacy profile selection is unsupported'}}), flush=True)
            continue
        result = {'config':settings}
    elif method in ('thread/start', 'thread/resume'):
        params = request['params']
        stale = tomllib.loads((root / 'fake-home/.codex/config.toml').read_text())
        stale.update(tomllib.loads((root / '.codex/config.toml').read_text()))
        # Fixture safety guard, not a claim about real thread/start behavior.
        if 'profile' in stale:
            raise RuntimeError('fixture cannot qualify legacy profile precedence')
        effective = dict(stale, **launch)
        effective.update(params.get('config', {}))
        (root / 'captured.json').write_text(json.dumps({'method':method, 'params':params, 'launch':launch, 'effective':effective}))
        result = {'thread':{'id':params.get('threadId','fresh')}, 'model':params['model'], 'reasoningEffort':'high', 'cwd':str(root), 'approvalPolicy':'never', 'sandbox':{'type':'dangerFullAccess'}}
    elif method == 'thread/goal/get':
        result = {'goal':None}
    else:
        continue
    print(json.dumps({'id':request['id'], 'result':result}), flush=True)
"#;

#[test]
fn compaction_fake_children_keep_ordinary_file_start_resume_snapshots_distinct() {
    let mut children = Vec::new();
    for (tokens, resume) in [(150_000, None), (200_000, Some("existing-thread"))] {
        let root = tempfile::tempdir().unwrap();
        write_stale_configs(root.path());
        let policy = native_policy(tokens);
        let mut options = spawn_options();
        options.cwd = root.path().to_path_buf();
        options.compaction_policy = Some(policy.clone());
        let mut spec = super::CodexAppServerHarness
            .spec(&selected_choice(), &options, &harness_context())
            .unwrap();
        spec.binary = "python3".into();
        spec.args.splice(0..0, ["-c".into(), CHILD.into()]);
        let opened = super::CodexAppServerHarness
            .open_session(open_request(root.path(), spec, policy, resume))
            .unwrap();
        let session = opened.session;
        children.push((root, session, tokens, resume));
    }
    for (root, session, tokens, resume) in children {
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.session_id().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            session.session_id().as_deref(),
            Some(resume.unwrap_or("fresh"))
        );
        let captured: Value =
            serde_json::from_slice(&std::fs::read(root.path().join("captured.json")).unwrap())
                .unwrap();
        assert_eq!(
            captured["method"],
            if resume.is_some() {
                "thread/resume"
            } else {
                "thread/start"
            }
        );
        assert_eq!(captured["params"]["config"], captured["launch"]);
        assert_eq!(
            captured["effective"]["model_auto_compact_token_limit"],
            tokens
        );
        assert_eq!(
            captured["effective"]["model_auto_compact_token_limit_scope"],
            "total"
        );
        assert!(captured["params"]["config"]
            .get("model_context_window")
            .is_none());
        assert_eq!(captured["effective"]["model_context_window"], 1_000_000);
        assert_eq!(captured["effective"]["personality"], "friendly");
        assert_stale_configs_unchanged(root.path());
        session.end();
    }
}

#[test]
fn compaction_open_rejects_another_launch_choice_before_spawning() {
    let mut policy = native_policy(150_000);
    policy.choice.model = Some("other-model".into());
    let root = tempfile::tempdir().unwrap();
    let request = open_request(
        root.path(),
        crate::pty::HarnessSpec::new("/does-not-exist-codex-config-test"),
        policy,
        None,
    );
    let error = match super::CodexAppServerHarness.open_session(request) {
        Ok(_) => panic!("mismatched launch policy must not start a child"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("compaction"), "{error}");
    assert!(
        !error.contains("does-not-exist"),
        "validation must happen before process spawn: {error}"
    );
}

fn open_request(
    root: &std::path::Path,
    spec: crate::pty::HarnessSpec,
    policy: AppliedCompactionPolicy,
    resume: Option<&str>,
) -> crate::harness::SessionOpenRequest {
    crate::harness::SessionOpenRequest {
        compaction_policy: Some(policy),
        spec,
        root: root.to_path_buf(),
        choice: selected_choice(),
        resume_session_id: resume.map(str::to_owned),
        recovery_context: None,
        terminal: crate::harness::TerminalOpenOptions {
            size: portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            },
            turn_ready_grace: None,
            identity: None,
            activity_locator: None,
        },
    }
}

const STALE_GLOBAL: &str = "model_auto_compact_token_limit=17\nmodel_auto_compact_token_limit_scope=\"input\"\nmodel_context_window=1000000\npersonality=\"friendly\"\n";
const STALE_PROJECT: &str =
    "model_auto_compact_token_limit=23\nmodel_auto_compact_token_limit_scope=\"input\"\n";

fn stale_configs() -> [(&'static str, &'static str); 2] {
    [
        ("fake-home/.codex/config.toml", STALE_GLOBAL),
        (".codex/config.toml", STALE_PROJECT),
    ]
}

fn write_stale_configs(root: &std::path::Path) {
    for (path, bytes) in stale_configs() {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
}

fn assert_stale_configs_unchanged(root: &std::path::Path) {
    for (path, bytes) in stale_configs() {
        assert_eq!(std::fs::read(root.join(path)).unwrap(), bytes.as_bytes());
    }
}

#[test]
fn compaction_tui_fake_child_uses_ordinary_root_overrides_for_fresh_exact_and_continue() {
    let script = r#"
import json, pathlib, sys, tomllib
root = pathlib.Path.cwd()
settings = tomllib.loads((root / 'fake-home/.codex/config.toml').read_text())
settings.update(tomllib.loads((root / '.codex/config.toml').read_text()))
# Fixture safety guard; real TUI profile precedence has not been qualified.
if 'profile' in settings:
    raise RuntimeError('fixture cannot qualify legacy profile precedence')
launch = {}
if any(arg == '--profile' or arg.startswith('--profile=') for arg in sys.argv[1:]):
    raise RuntimeError('fixture cannot qualify file profile precedence')
args = iter(sys.argv[1:])
tail = []
for arg in args:
    if arg == 'resume':
        tail = [arg, *args]
        break
    if arg == '--config':
        key, value = next(args).split('=', 1)
        if key.startswith('model_auto_compact_'):
            launch[key] = json.loads(value)
settings.update(launch)
print(json.dumps({'tail':tail, 'effective':settings, 'launch':launch}))
"#;
    for resume in [None, Some("existing-thread"), Some("--last")] {
        let root = tempfile::tempdir().unwrap();
        write_stale_configs(root.path());
        let mut policy = native_policy(150_000);
        policy.choice.provider = crate::models::AgentProvider::Codex;
        let mut options = spawn_options();
        options.cwd = root.path().to_path_buf();
        options.resume_session_id = resume.filter(|id| *id != "--last").map(str::to_owned);
        options.continue_session = resume == Some("--last");
        options.compaction_policy = Some(policy.clone());
        let spec = crate::harness::codex::CodexHarness
            .spec(&policy.choice, &options, &harness_context())
            .unwrap();
        let output = std::process::Command::new("python3")
            .args(["-c", script])
            .args(&spec.args)
            .current_dir(root.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let captured: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            captured["tail"],
            resume.map_or_else(|| json!([]), |id| json!(["resume", id]))
        );
        assert_eq!(
            captured["launch"]["model_auto_compact_token_limit"],
            150_000
        );
        assert_eq!(
            captured["launch"]["model_auto_compact_token_limit_scope"],
            "total"
        );
        assert!(captured["launch"].get("model_context_window").is_none());
        assert_eq!(captured["effective"]["model_context_window"], 1_000_000);
        assert_eq!(captured["effective"]["personality"], "friendly");
        assert_stale_configs_unchanged(root.path());
    }
}

// Selected-profile cases exercise preparation only. The pinned config/read probe rejects
// legacy profile selection; no fake native merge may establish its precedence.
#[test]
fn compaction_selected_profile_files_keep_both_public_carriers_on_native_defaults() {
    for legacy in [true, false] {
        for harness in [
            &crate::harness::codex::CodexHarness as &dyn Harness,
            &super::CodexAppServerHarness as &dyn Harness,
        ] {
            assert_profile_fallback(harness, legacy);
        }
    }
}

fn assert_profile_fallback(harness: &dyn Harness, legacy: bool) {
    let root = tempfile::tempdir().unwrap();
    let files = profile_configs(legacy);
    assert!(
        files
            .iter()
            .any(|(_, bytes)| bytes.contains("profile-sentinel")),
        "missing selected-profile fixture"
    );
    for (path, bytes) in &files {
        let path = root.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    let mut choice = selected_choice();
    choice.provider = harness.provider();
    let policy = harness
        .prepare_compaction_policy(
            ResolvedCompactionPolicy {
                tokens: 150_000,
                source: CompactionSource::Device,
            },
            choice.clone(),
            Some("0.160.0".into()),
            CompactionLaunchIdentity::new("profile-thread"),
        )
        .unwrap();
    assert_profile_policy(&policy, &choice);
    let mut options = spawn_options();
    options.cwd = root.path().to_path_buf();
    let mut original = harness.spec(&choice, &options, &harness_context()).unwrap();
    options.compaction_policy = Some(policy);
    let fallback_spec = harness.spec(&choice, &options, &harness_context()).unwrap();
    assert_eq!(fallback_spec.args, original.args);
    assert_eq!(fallback_spec.env, original.env);
    // Represent an existing runtime profile selection without opening a child.
    // App-server rejects this option; TUI precedence remains unqualified.
    if !legacy {
        original.args.extend(["--profile".into(), "x".into()]);
    }
    let prepared = crate::harness::codex_compaction::apply(
        original.clone(),
        &choice,
        options.compaction_policy.as_ref(),
    )
    .unwrap();
    assert_eq!(prepared.args, original.args);
    assert_eq!(prepared.env, original.env);
    for (path, bytes) in files {
        assert_eq!(
            std::fs::read(root.path().join(path)).unwrap(),
            bytes.as_bytes()
        );
    }
}

fn assert_profile_policy(policy: &AppliedCompactionPolicy, choice: &crate::models::ModelChoice) {
    assert_eq!(policy.enforcement, CompactionEnforcement::Native);
    assert_eq!(policy.requested.tokens, 150_000);
    assert_eq!(
        policy.reason.as_deref(),
        Some("codex_model_window_buffer_and_profile_unqualified")
    );
    assert!(policy.native_settings.is_none());
    assert!(policy.translated_trigger.is_none());
    assert!(!policy.capabilities.native_threshold);
    assert_eq!(
        crate::harness::codex_compaction::thread_config(choice, Some(policy)).unwrap(),
        None
    );
}

fn profile_configs(legacy: bool) -> Vec<(&'static str, String)> {
    let profile = "# profile-sentinel: stale compaction preferences\nmodel_auto_compact_token_limit=31\nmodel_auto_compact_token_limit_scope=\"body_after_prefix\"\nmodel_post_turn_compact_threshold_percent=70\nmodel_context_window=500000\npersonality=\"pragmatic\"\n";
    let mut files: Vec<_> = stale_configs()
        .into_iter()
        .map(|(path, bytes)| (path, bytes.to_owned()))
        .collect();
    if legacy {
        files[0].1 = format!("profile=\"x\"\n{}[profiles.x]\n{profile}", STALE_GLOBAL);
    } else {
        files.push(("fake-home/.codex/x.config.toml", profile.to_owned()));
    }
    files
}

#[test]
fn compaction_config_read_fixture_refuses_legacy_profile_precedence_claims() {
    use std::io::Write;
    let root = tempfile::tempdir().unwrap();
    write_stale_configs(root.path());
    std::fs::write(
        root.path().join("fake-home/.codex/config.toml"),
        "profile=\"x\"\n[profiles.x]\nmodel_auto_compact_token_limit=31\n",
    )
    .unwrap();
    let mut child = std::process::Command::new("python3")
        .args(["-c", CHILD])
        .current_dir(root.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"id\":1,\"method\":\"config/read\"}\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout)
        .expect("config/read must refuse unsupported legacy profiles explicitly");
    assert_eq!(response["error"]["code"], -32603);
    assert!(response["error"]["message"]
        .as_str()
        .unwrap()
        .contains("legacy profile"));
}
