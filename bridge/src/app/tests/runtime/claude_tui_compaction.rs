//! The Claude TUI's hooks are activity inside a turn, through the real sidecar.
use super::compaction::{CompactingAgent, RUN};
use super::*;
use crate::harness::compaction::{
    CompactionEnforcement, CompactionLaunchIdentity, NativeCompactionSettings, PolicySession,
};
use crate::harness::{harness_for, HarnessContext, SessionIdentitySource};
use crate::orchestrator::SpawnOptions;
use crate::pty::HarnessSpec;

struct EndingSession(Arc<dyn AgentSession>);

impl Drop for EndingSession {
    fn drop(&mut self) {
        self.0.end();
    }
}

/// The fake CLI runs the actual commands from Claude's --settings. Files release
/// stages; no terminal banner or elapsed time stands in for a hook boundary.
fn fake_tui(directory: &Path, settings: String, sidecar: PathBuf) -> HarnessSpec {
    let script = directory.join("fake-claude.py");
    std::fs::write(
        &script,
        r#"
import json, pathlib, subprocess, sys, time
root = pathlib.Path(sys.argv[1])
settings = json.loads(sys.argv[2])
def stage(name):
    (root / name).touch()
def wait(name):
    while not (root / name).exists():
        time.sleep(0.01)
def capture():
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)
    with (root / 'stdin.txt').open('a') as out:
        out.write(line)
def hook(event):
    for group in settings['hooks'][event]:
        for command in group['hooks']:
            subprocess.run(['sh', '-c', command['command']], check=True)
capture()
stage('started')
wait('compact')
hook('PreCompact')
# A failed attempt / misleading terminal banner is not a PostCompact boundary.
print('Compacted (terminal banner only)', flush=True)
stage('pre-hook')
wait('post')
hook('PostCompact')
stage('post-hook')
wait('finish')
stage('turn-ended')
capture()
stage('follow-up')
while True:
    capture()
"#,
    )
    .unwrap();
    HarnessSpec::new("python3")
        .arg("-u")
        .arg(script.to_string_lossy())
        .arg(directory.to_string_lossy())
        .arg(settings)
        .compaction_sidecar(sidecar.clone())
        .env("BUILD_COMPACTION_LOG", sidecar.to_string_lossy())
}

fn native_tui(agent: &CompactingAgent, directory: &Path) -> (EndingSession, TabKey) {
    let choice = ModelChoice {
        provider: AgentProvider::Claude,
        ..ModelChoice::default()
    };
    let mut app = agent.state.lock().unwrap();
    app.set_agent_model_choice(RUN, &agent.agent_id, choice.clone())
        .unwrap();
    let harness = harness_for(AgentProvider::Claude);
    let mut policy = harness
        .prepare_compaction_policy(
            app.resolve_compaction_policy(RUN, &agent.agent_id).unwrap(),
            choice.clone(),
            Some("2.1.284".into()),
            CompactionLaunchIdentity::new(
                app.agent_conversation(RUN, Some(&agent.agent_id))
                    .unwrap()
                    .id
                    .clone(),
            ),
        )
        .unwrap();
    // Exercise qualified lifecycle without promoting any production model.
    let translated = crate::harness::claude_compaction::translate(
        "2.1.284",
        policy.requested.tokens,
        1_000_000,
        20_000,
    )
    .unwrap();
    policy.translated_trigger = Some(translated.trigger);
    policy.native_settings = Some(NativeCompactionSettings::Claude(translated));
    policy.enforcement = CompactionEnforcement::Native;
    let options = SpawnOptions {
        compaction_policy: Some(policy.clone()),
        ..SpawnOptions::default()
    };
    let context = HarnessContext {
        channel: crate::config::Channel::Prod,
        bridge_exe: directory.join("unused-bridge"),
        mcp_socket: directory.join("unused-socket"),
        state_root: directory.join("state"),
    };
    let configured = harness.spec(&choice, &options, &context).unwrap();
    let settings_at = configured
        .args
        .iter()
        .position(|s| s == "--settings")
        .unwrap();
    let spec = fake_tui(
        directory,
        configured.args[settings_at + 1].clone(),
        configured.compaction_sidecar.unwrap(),
    );
    let pty = PtySession::spawn(&spec, None, portable_pty::PtySize::default())
        .unwrap()
        .with_session_identity(Some(SessionIdentitySource::Known("tui-native-test".into())));
    let activity = pty.activity().unwrap();
    let key = insert_agent_tab(&mut app, &agent.root, RUN, &agent.agent_id, pty);
    let tab = app.session_registry.test_tab_mut(&key).unwrap();
    tab.session = PolicySession::attach(Arc::clone(&tab.session), policy);
    let session = EndingSession(Arc::clone(&tab.session));
    drop(app);
    spawn_activity_pump(&agent.state, key.clone(), Some(activity));
    (session, key)
}

async fn reached(directory: &Path, stage: &str) {
    wait_for(Duration::from_secs(5), || {
        directory.join(stage).exists().then_some(())
    })
    .await
    .unwrap_or_else(|| panic!("fake TUI never reached {stage}"));
}

fn release(directory: &Path, stage: &str) {
    std::fs::write(directory.join(stage), "").unwrap();
}

fn compactions(agent: &CompactingAgent) -> Vec<String> {
    agent
        .agent()
        .thread
        .items
        .into_iter()
        .filter_map(|item| match item {
            crate::thread::ThreadItem::Event(event)
                if event.event == crate::thread::ThreadEventKind::Compaction =>
            {
                event.summary
            }
            _ => None,
        })
        .collect()
}

fn assert_open_turn(agent: &CompactingAgent, session: &dyn AgentSession, key: &TabKey) {
    assert_eq!(session.status(), AgentStatus::Working);
    assert_eq!(session.session_id().as_deref(), Some("tui-native-test"));
    assert_eq!(session.epitaph(), None);
    assert!(
        !session.automatic_compaction_due(Some(900_000)),
        "native mode must not schedule a Build threshold command"
    );
    assert!(
        agent.agent().working_since.is_some(),
        "compaction must not end the turn"
    );
    assert_eq!(agent.agent().last_context_tokens, Some(900_000));
    assert_eq!(
        agent.still_queued(),
        1,
        "compaction must not consume queued input"
    );
    let state = agent.state.lock().unwrap();
    assert!(state.session_registry.test_tab(key).unwrap().live);
    assert!(state.compactions.ready_agents().is_empty());
}

#[tokio::test]
async fn claude_tui_native_hooks_preserve_open_turn_and_deliver_queued_follow_up_once() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CompactingAgent::without_background_services().with_context(900_000);
    let (session, key) = native_tui(&agent, directory.path());
    let pid = session.0.terminal().unwrap().pid();
    assert!(pid.is_some());
    session.0.send_turn(&Turn::new("long tool loop")).unwrap();
    reached(directory.path(), "started").await;
    let mut turn = agent.turn(false);
    turn.say = Some(TurnText {
        cold: "queued follow-up".into(),
        warm: "queued follow-up".into(),
    });
    agent.state.lock().unwrap().delivery_queue.enqueue(turn);

    release(directory.path(), "compact");
    reached(directory.path(), "pre-hook").await;
    wait_for(Duration::from_secs(5), || {
        (compactions(&agent) == ["Compacting"]).then_some(())
    })
    .await
    .expect("the generated PreCompact command reaches the sidecar and activity pump");
    assert_open_turn(&agent, session.0.as_ref(), &key);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(
        compactions(&agent),
        ["Compacting"],
        "PreCompact and a terminal banner cannot report success"
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("stdin.txt")).unwrap(),
        "long tool loop\n"
    );

    release(directory.path(), "post");
    reached(directory.path(), "post-hook").await;
    wait_for(Duration::from_secs(5), || {
        (compactions(&agent) == ["Compacted"]).then_some(())
    })
    .await
    .expect("only the generated PostCompact boundary completes the row");
    assert_open_turn(&agent, session.0.as_ref(), &key);
    assert_eq!(session.0.terminal().unwrap().pid(), pid);
    assert_eq!(
        session.0.compaction_policy().unwrap().enforcement,
        CompactionEnforcement::Native
    );

    release(directory.path(), "finish");
    reached(directory.path(), "turn-ended").await;
    agent
        .state
        .lock()
        .unwrap()
        .record_agent_working_since(RUN, &agent.agent_id, None);
    session
        .0
        .backdate_last_output(AGENT_WORKING_WINDOW + Duration::from_secs(1));
    deliver_pending_agent_turns(&agent.state);
    reached(directory.path(), "follow-up").await;
    deliver_pending_agent_turns(&agent.state);
    assert_eq!(agent.still_queued(), 0);
    assert_eq!(
        std::fs::read_to_string(directory.path().join("stdin.txt")).unwrap(),
        "long tool loop\nqueued follow-up\n",
        "one follow-up, no duplicate delivery or Build threshold /compact"
    );
    assert_eq!(session.0.session_id().as_deref(), Some("tui-native-test"));
    assert_eq!(session.0.epitaph(), None);
}
