//! Fake CLI applies the observed 2.1.284 environment/settings precedence.
use super::tests::choice;
use crate::compaction::{CompactionSource, ResolvedCompactionPolicy};
use crate::harness::compaction::CompactionLaunchIdentity;
use crate::harness::{harness_for, HarnessContext};
use crate::models::AgentProvider;
use crate::orchestrator::SpawnOptions;
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn claude_compaction_fake_children_override_stale_settings_and_preserve_manual_compact() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("fake-claude.py");
    std::fs::write(&script, r#"
import json,os,sys
args=sys.argv[1:]
settings=json.loads(args[args.index('--settings')+1])
user=json.load(open(os.environ['FAKE_USER_SETTINGS']))
effective=dict(os.environ)
effective.update(user['env'])
effective.update(settings['env'])
command=sys.stdin.readline().strip()
print(json.dumps({'args':args,'env':{k:effective.get(k) for k in ['CLAUDE_CODE_AUTO_COMPACT_WINDOW','CLAUDE_AUTOCOMPACT_PCT_OVERRIDE','DISABLE_COMPACT','DISABLE_AUTO_COMPACT','CLAUDE_CODE_MAX_OUTPUT_TOKENS']},'hooks':settings['hooks'],'manual':command=='/compact' and effective['DISABLE_COMPACT']=='0'}))
"#).unwrap();
    let user_path = dir.path().join("settings.json");
    let original = json!({"env": {
        "CLAUDE_CODE_AUTO_COMPACT_WINDOW":"999000",
        "CLAUDE_AUTOCOMPACT_PCT_OVERRIDE":"1",
        "DISABLE_COMPACT":"1", "DISABLE_AUTO_COMPACT":"0",
        "CLAUDE_CODE_MAX_OUTPUT_TOKENS":"8192"
    }}).to_string();
    std::fs::write(&user_path, &original).unwrap();
    let context = HarnessContext { bridge_exe: "/fake/bridge".into(), mcp_socket: dir.path().join("socket"), state_root: dir.path().join("state") };
    for provider in [AgentProvider::ClaudeAdk, AgentProvider::Claude] {
        for resume in [None, Some("exact"), Some("continue")] {
            let choice = choice(provider);
            let harness = harness_for(provider);
            let policy = harness.prepare_compaction_policy(ResolvedCompactionPolicy { tokens: 150_000, source: CompactionSource::Device }, choice.clone(), Some("2.1.284".into()), CompactionLaunchIdentity::new("thread")).unwrap();
            let options = SpawnOptions { compaction_policy: Some(policy), resume_session_id: resume.filter(|r| *r=="exact").map(str::to_owned), continue_session: resume.is_some(), ..SpawnOptions::default() };
            let spec = harness.spec(&choice, &options, &context).unwrap();
            let mut command = Command::new("python3");
            command.arg(&script).args(&spec.args)
                .env("FAKE_USER_SETTINGS", &user_path)
                .env("DISABLE_AUTO_COMPACT", "0")
                .env("DISABLE_COMPACT", "1")
                .env("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "777000")
                .env("CLAUDE_AUTOCOMPACT_PCT_OVERRIDE", "0.1")
                .stdin(Stdio::piped()).stdout(Stdio::piped());
            for key in &spec.unset { command.env_remove(key); }
            command.envs(spec.env);
            let mut child = command.spawn().unwrap();
            child.stdin.take().unwrap().write_all(b"/compact\n").unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success());
            let result: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(result["env"]["DISABLE_AUTO_COMPACT"], "1");
            assert_eq!(result["env"]["CLAUDE_CODE_AUTO_COMPACT_WINDOW"], "");
            assert_eq!(result["env"]["CLAUDE_AUTOCOMPACT_PCT_OVERRIDE"], "");
            assert_eq!(result["env"]["CLAUDE_CODE_MAX_OUTPUT_TOKENS"], "8192");
            assert_eq!(result["manual"], true);
            assert!(result["hooks"]["PreCompact"].is_array());
            let args = result["args"].as_array().unwrap();
            assert_eq!(args.contains(&json!("--resume")), resume==Some("exact"));
            assert_eq!(args.contains(&json!("--continue")), resume==Some("continue"));
        }
    }
    assert_eq!(std::fs::read_to_string(user_path).unwrap(), original);
    assert!(!context.state_root.exists());
}
