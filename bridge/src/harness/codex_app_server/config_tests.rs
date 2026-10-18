use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::connection::AppServerConnection;
use super::fixtures::{harness_context, selected_choice, spawn_options};
use super::limits::AppServerLimits;
use super::protocol::PendingOperation;
use super::session::{CodexAppServerSession, SessionLaunchSettings};
use crate::compaction::{CompactionSource, ResolvedCompactionPolicy};
use crate::harness::compaction::{
    AppliedCompactionPolicy, CompactionEnforcement, CompactionLaunchIdentity,
    NativeCompactionSettings,
};
use crate::harness::{AgentSession, Harness};

fn native_policy(tokens: u64) -> AppliedCompactionPolicy {
    let mut policy = super::CodexAppServerHarness
        .prepare_compaction_policy(
            ResolvedCompactionPolicy { tokens, source: CompactionSource::Device },
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
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

#[test]
fn compaction_config_is_scoped_to_thread_open_requests_only() {
    let capture = Capture::default();
    let config = json!({"model_auto_compact_token_limit": 150000,
        "model_auto_compact_token_limit_scope": "total"});
    let connection = AppServerConnection::new(Box::new(capture.clone()), AppServerLimits::default().connection())
        .with_thread_config(Some(config.clone()));
    for operation in [
        PendingOperation::Initialize,
        PendingOperation::StartThread { cwd: "/tmp/test".into(), model: None },
        PendingOperation::ResumeThread { thread_id: "resume".into(), cwd: "/tmp/test".into(), model: None },
        PendingOperation::CompactThread { thread_id: "resume".into() },
        PendingOperation::StartTurn { thread_id: "resume".into(), input: "continue".into(), model: None, effort: None },
    ] { connection.request(operation).unwrap(); }
    let bytes = capture.0.lock().unwrap();
    let frames: Vec<Value> = bytes.split(|b| *b == b'\n').filter(|b| !b.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap()).collect();
    for frame in frames {
        if matches!(frame["method"].as_str(), Some("thread/start" | "thread/resume")) {
            assert_eq!(frame["params"]["config"], config);
        } else {
            assert!(frame["params"].get("config").is_none(), "{frame}");
        }
    }
}

const CHILD: &str = r#"
import json, pathlib, sys
root = pathlib.Path.cwd()
launch = {}
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
    elif method in ('thread/start', 'thread/resume'):
        params = request['params']
        stale = json.loads((root / 'stale-config.json').read_text())
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
fn compaction_fake_children_keep_distinct_start_resume_snapshots_and_stale_files_unchanged() {
    let mut children = Vec::new();
    for (tokens, resume) in [(150_000, None), (200_000, Some("existing-thread"))] {
        let root = tempfile::tempdir().unwrap();
        let stale = b"{\"model_auto_compact_token_limit\":17,\"model_auto_compact_token_limit_scope\":\"input\"}\n";
        std::fs::write(root.path().join("stale-config.json"), stale).unwrap();
        let policy = native_policy(tokens);
        let config = crate::harness::codex_compaction::thread_config(&selected_choice(), Some(&policy)).unwrap();
        let mut options = spawn_options();
        options.cwd = root.path().to_path_buf();
        options.compaction_policy = Some(policy);
        let mut spec = super::CodexAppServerHarness.spec(&selected_choice(), &options, &harness_context()).unwrap();
        spec.binary = "python3".into();
        spec.args.splice(0..0, ["-c".into(), CHILD.into()]);
        let (session, _activity) = CodexAppServerSession::spawn_with_settings(
            &spec, root.path().to_path_buf(), selected_choice(), resume.map(str::to_owned),
            AppServerLimits::default(), SessionLaunchSettings { recovery: None, thread_config: config },
        ).unwrap();
        children.push((root, session, tokens, resume, stale));
    }
    for (root, session, tokens, resume, stale) in children {
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.session_id().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(session.session_id().as_deref(), Some(resume.unwrap_or("fresh")));
        let captured: Value = serde_json::from_slice(&std::fs::read(root.path().join("captured.json")).unwrap()).unwrap();
        assert_eq!(captured["method"], if resume.is_some() { "thread/resume" } else { "thread/start" });
        assert_eq!(captured["params"]["config"], captured["launch"]);
        assert_eq!(captured["effective"]["model_auto_compact_token_limit"], tokens);
        assert_eq!(captured["effective"]["model_auto_compact_token_limit_scope"], "total");
        assert!(captured["params"]["config"].get("model_context_window").is_none());
        assert_eq!(std::fs::read(root.path().join("stale-config.json")).unwrap(), stale);
        session.stop();
    }
}

#[test]
fn compaction_open_rejects_another_launch_choice_before_spawning() {
    let mut policy = native_policy(150_000);
    policy.choice.model = Some("other-model".into());
    let root = tempfile::tempdir().unwrap();
    let request = crate::harness::SessionOpenRequest {
        compaction_policy: Some(policy),
        spec: crate::pty::HarnessSpec::new("/does-not-exist-codex-config-test"),
        root: root.path().to_path_buf(),
        choice: selected_choice(),
        resume_session_id: None,
        recovery_context: None,
        terminal: crate::harness::TerminalOpenOptions {
            size: portable_pty::PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 },
            turn_ready_grace: None,
            identity: None,
            activity_locator: None,
        },
    };
    let error = match super::CodexAppServerHarness.open_session(request) {
        Ok(_) => panic!("mismatched launch policy must not start a child"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("compaction"), "{error}");
    assert!(!error.contains("does-not-exist"), "validation must happen before process spawn: {error}");
}
