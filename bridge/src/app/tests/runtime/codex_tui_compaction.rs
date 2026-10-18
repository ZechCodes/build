//! Both Codex carriers keep native compaction inside the ongoing work turn.
use super::compaction::{CompactingAgent, RUN};
use super::*;
use crate::harness::compaction::{CompactionEnforcement, CompactionLaunchIdentity, PolicySession};
use crate::harness::{harness_for, SessionIdentitySource, SessionOpenRequest, TerminalOpenOptions};
use crate::pty::HarnessSpec;

const SESSION: &str = "codex-native-session";

struct EndingSession(Arc<dyn AgentSession>);

impl Drop for EndingSession {
    fn drop(&mut self) {
        self.0.end();
    }
}

// Stage files control provider boundaries. No sleep or terminal banner is used
// to claim a successful compaction or a finished protocol turn.
const CHILD: &str = r#"
import json, pathlib, sys, time
root, cwd, carrier = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
thread = 'codex-native-session'
rollout = root / '.codex/sessions/2026/10/18/ours.jsonl'
foreign = rollout.with_name('foreign.jsonl')
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
    return line
def send(value):
    print(json.dumps(value), flush=True)
def notify(method, params):
    send({'method': method, 'params': params})
def item(lifecycle, value, owner=thread):
    notify('item/' + lifecycle, {'threadId': owner, 'turnId': 'work-turn', 'item': value})
def append(path, value):
    with path.open('a') as out:
        out.write(json.dumps(value) + '\n')
def usage():
    tokens = {'inputTokens':900000, 'cachedInputTokens':7, 'cacheWriteInputTokens':0,
              'outputTokens':1, 'reasoningOutputTokens':0, 'totalTokens':900001}
    notify('thread/tokenUsage/updated', {'threadId':thread, 'turnId':'work-turn',
           'tokenUsage':{'last':tokens, 'total':tokens, 'modelContextWindow':1000000}})
if carrier == 'app-server':
    request = json.loads(sys.stdin.readline())
    send({'id':request['id'], 'result':{'userAgent':'build_bridge/0.153.0 (fixture)'}})
    sys.stdin.readline()
    request = json.loads(sys.stdin.readline())
    send({'id':request['id'], 'result':{'thread':{'id':thread}, 'model':'gpt-5.6-sol',
         'reasoningEffort':'high', 'cwd':cwd, 'approvalPolicy':'never',
         'sandbox':{'type':'dangerFullAccess'}}})
    request = json.loads(sys.stdin.readline())
    send({'id':request['id'], 'result':{'goal':None}})
    request = json.loads(capture())
    send({'id':request['id'], 'result':{'turn':{'id':'work-turn'}}})
    item('started', {'id':'input-1', 'type':'userMessage',
                    'content':[{'type':'text', 'text':'long tool loop'}]})
    usage()
else:
    capture()
    print('working', flush=True)
stage('started')
wait('compact')
if carrier == 'app-server':
    item('started', {'id':'compact-failed', 'type':'contextCompaction'})
    for status in ['failed', 'cancelled', 'interrupted']:
        item('completed', {'id':'compact-' + status, 'type':'contextCompaction', 'status':status})
    item('completed', {'id':'foreign-compact', 'type':'contextCompaction'}, 'another-thread')
else:
    append(rollout, {'type':'event_msg', 'payload':{'type':'error', 'message':'compaction failed'}})
    append(foreign, {'type':'compacted', 'payload':{'message':'another session'}})
    print('Compacted (terminal banner only)', flush=True)
stage('attempted')
wait('success')
if carrier == 'app-server':
    item('started', {'id':'compact-success', 'type':'contextCompaction'})
    item('completed', {'id':'compact-success', 'type':'contextCompaction', 'status':'completed'})
else:
    append(rollout, {'type':'compacted', 'payload':{'message':'summary'}})
    print('still working', flush=True)
stage('compacted')
wait('finish')
if carrier == 'app-server':
    notify('turn/completed', {'threadId':thread, 'turn':{'id':'work-turn', 'status':'completed'}})
stage('turn-ended')
line = capture()
if carrier == 'app-server':
    request = json.loads(line)
    send({'id':request['id'], 'result':{'turn':{'id':'follow-up-turn'}}})
    notify('item/started', {'threadId':thread, 'turnId':'follow-up-turn',
           'item':{'id':'input-2', 'type':'userMessage',
                   'content':[{'type':'text', 'text':'queued follow-up'}]}})
stage('follow-up')
while True:
    capture()
"#;

fn transcript(directory: &Path, cwd: &Path, name: &str, id: &str) {
    let dated = directory.join(".codex/sessions/2026/10/18");
    std::fs::create_dir_all(&dated).unwrap();
    let header = json!({"type":"session_meta", "payload":{"id":id,"cwd":cwd}});
    std::fs::write(
        dated.join(name),
        format!("{header}\n{{\"type\":\"compacted\"}}\n"),
    )
    .unwrap();
}

fn launch(
    agent: &CompactingAgent,
    directory: &Path,
    provider: AgentProvider,
) -> (EndingSession, TabKey) {
    let choice = ModelChoice {
        provider,
        model: Some("gpt-5.6-sol".into()),
        effort: Some("high".into()),
    };
    let harness = harness_for(provider);
    let mut app = agent.state.lock().unwrap();
    app.set_agent_model_choice(RUN, &agent.agent_id, choice.clone())
        .unwrap();
    let mut policy = harness
        .prepare_compaction_policy(
            app.resolve_compaction_policy(RUN, &agent.agent_id).unwrap(),
            choice.clone(),
            None,
            CompactionLaunchIdentity::new(agent.agent_id.clone()),
        )
        .unwrap();
    // Lifecycle coverage does not promote an unqualified production model.
    policy.enforcement = CompactionEnforcement::Native;
    std::fs::write(directory.join("child.py"), CHILD).unwrap();
    let spec = HarnessSpec::new("python3")
        .arg("-u")
        .arg(directory.join("child.py").to_string_lossy())
        .arg(directory.to_string_lossy())
        .arg(agent.root.to_string_lossy())
        .arg(if provider == AgentProvider::Codex {
            "tui"
        } else {
            "app-server"
        });
    transcript(directory, &agent.root, "ours.jsonl", SESSION);
    transcript(directory, &agent.root, "foreign.jsonl", "another-session");
    let session = if provider == AgentProvider::Codex {
        let locator = harness.session_locator(directory, &agent.root).unwrap();
        Arc::new(
            PtySession::spawn(&spec, None, portable_pty::PtySize::default())
                .unwrap()
                .with_session_identity(Some(SessionIdentitySource::Known(SESSION.into())))
                .with_activity_locator(Some((locator, Some(SESSION.into())))),
        ) as Arc<dyn AgentSession>
    } else {
        harness
            .open_session(SessionOpenRequest {
                compaction_policy: None,
                spec,
                root: agent.root.clone(),
                choice,
                terminal: TerminalOpenOptions {
                    size: portable_pty::PtySize::default(),
                    turn_ready_grace: None,
                    identity: None,
                    activity_locator: None,
                },
                resume_session_id: None,
                recovery_context: None,
            })
            .unwrap()
            .session
    };
    let session = PolicySession::attach(session, policy);
    let activity = session.activity();
    let key = insert_agent_tab(
        &mut app,
        &agent.root,
        RUN,
        &agent.agent_id,
        DictatedSession::reporting(AgentStatus::Working),
    );
    let tab = app.session_registry.test_tab_mut(&key).unwrap();
    tab.session = Arc::clone(&session);
    let instance = tab.session_instance.clone();
    drop(app);
    spawn_status_pump(
        &agent.state,
        key.clone(),
        Arc::clone(&session),
        instance,
        session.status_changed(),
    );
    spawn_activity_pump(&agent.state, key.clone(), activity);
    (EndingSession(session), key)
}

async fn reached(directory: &Path, stage: &str) {
    wait_for(Duration::from_secs(5), || {
        directory.join(stage).exists().then_some(())
    })
    .await
    .unwrap_or_else(|| panic!("fake Codex never reached {stage}"));
}

fn release(directory: &Path, stage: &str) {
    std::fs::write(directory.join(stage), "").unwrap();
}

fn completed_compactions(agent: &CompactingAgent) -> usize {
    agent
        .agent()
        .thread
        .items
        .iter()
        .filter(|item| {
            matches!(item,
                crate::thread::ThreadItem::Event(event)
                if event.event == crate::thread::ThreadEventKind::Compaction
                && event.summary.as_deref() == Some("Compacted")
            )
        })
        .count()
}

fn assert_open(agent: &CompactingAgent, session: &dyn AgentSession, key: &TabKey) {
    assert_eq!(session.status(), AgentStatus::Working);
    assert_eq!(session.session_id().as_deref(), Some(SESSION));
    assert_eq!(session.epitaph(), None);
    assert_eq!(
        session.compaction_policy().unwrap().enforcement,
        CompactionEnforcement::Native
    );
    assert!(!session.automatic_compaction_due(Some(900_000)));
    assert!(agent.agent().working_since.is_some());
    assert_eq!(agent.still_queued(), 1);
    let state = agent.state.lock().unwrap();
    assert!(state.session_registry.test_tab(key).unwrap().live);
    assert!(state.compactions.ready_agents().is_empty());
}

async fn native_compaction_lifecycle(provider: AgentProvider) {
    let directory = tempfile::tempdir().unwrap();
    let agent = CompactingAgent::without_background_services().with_context(
        if provider == AgentProvider::CodexAppServer {
            800_000
        } else {
            900_000
        },
    );
    let (session, key) = launch(&agent, directory.path(), provider);
    wait_for(Duration::from_secs(5), || {
        session.0.opening_succeeded().then_some(())
    })
    .await
    .expect("provider opened");
    let pid = session.0.terminal().and_then(|terminal| terminal.pid());
    session.0.send_turn(&Turn::new("long tool loop")).unwrap();
    reached(directory.path(), "started").await;
    if provider == AgentProvider::CodexAppServer {
        // The changed reading proves the status pump has consumed Working,
        // including any older startup Waiting snapshot, before input queues.
        wait_for(Duration::from_secs(5), || {
            (agent.agent().last_context_tokens == Some(900_000)).then_some(())
        })
        .await
        .expect("the status pump observed the ongoing work turn");
    }
    let mut turn = agent.turn(false);
    turn.say = Some(TurnText {
        cold: "queued follow-up".into(),
        warm: "queued follow-up".into(),
    });
    agent.state.lock().unwrap().delivery_queue.enqueue(turn);
    release(directory.path(), "compact");
    reached(directory.path(), "attempted").await;
    // Several follower polls while native work is deliberately held open.
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_open(&agent, session.0.as_ref(), &key);
    assert_eq!(
        completed_compactions(&agent),
        0,
        "failures, foreign records, old history and terminal banners cannot claim success"
    );
    assert_eq!(agent.agent().last_context_tokens, Some(900_000));
    if let Some(status) = session.0.status_changed() {
        assert_eq!(status.borrow().context.unwrap().context_tokens, 900_000);
    }
    assert_eq!(
        std::fs::read_to_string(directory.path().join("stdin.txt"))
            .unwrap()
            .lines()
            .count(),
        1
    );

    release(directory.path(), "success");
    reached(directory.path(), "compacted").await;
    wait_for(Duration::from_secs(5), || {
        (completed_compactions(&agent) == 1).then_some(())
    })
    .await
    .expect("native completion reached the real activity pump");
    assert_open(&agent, session.0.as_ref(), &key);
    assert_eq!(
        session.0.terminal().and_then(|terminal| terminal.pid()),
        pid
    );
    if let Some(status) = session.0.status_changed() {
        assert_eq!(status.borrow().context.unwrap().context_tokens, 0);
        assert_eq!(status.borrow().context.unwrap().cache_read_tokens, 7);
    }

    release(directory.path(), "finish");
    reached(directory.path(), "turn-ended").await;
    if provider == AgentProvider::Codex {
        session
            .0
            .backdate_last_output(AGENT_WORKING_WINDOW + Duration::from_secs(1));
    }
    if provider == AgentProvider::Codex {
        agent
            .state
            .lock()
            .unwrap()
            .record_agent_working_since(RUN, &agent.agent_id, None);
        deliver_pending_agent_turns(&agent.state);
    }
    // The app-server's real status pump releases its queued turn automatically.
    reached(directory.path(), "follow-up").await;
    deliver_pending_agent_turns(&agent.state);
    assert_eq!(agent.still_queued(), 0);
    let input = std::fs::read_to_string(directory.path().join("stdin.txt")).unwrap();
    assert_eq!(
        input.lines().count(),
        2,
        "one follow-up and no duplicate Build threshold command: {input}"
    );
    if provider == AgentProvider::Codex {
        assert_eq!(input, "long tool loop\nqueued follow-up\n");
    } else {
        let frames: Vec<Value> = input
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(frames.iter().all(|frame| frame["method"] == "turn/start"));
        assert_eq!(frames[1]["params"]["input"][0]["text"], "queued follow-up");
    }
    assert_eq!(session.0.session_id().as_deref(), Some(SESSION));
    assert_eq!(session.0.epitaph(), None);
}

#[tokio::test]
async fn codex_tui_native_compaction_preserves_work_and_delivers_follow_up_once() {
    native_compaction_lifecycle(AgentProvider::Codex).await;
}

#[tokio::test]
async fn codex_app_server_native_compaction_preserves_work_and_delivers_follow_up_once() {
    native_compaction_lifecycle(AgentProvider::CodexAppServer).await;
}
