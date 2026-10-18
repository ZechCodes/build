//! Pi's extension owns idle boundaries; terminal silence cannot compact tools.
use super::compaction::{CompactingAgent, RUN};
use super::*;
use crate::harness::compaction::{CompactionEnforcement, CompactionLaunchIdentity};
use crate::harness::{
    harness_for, HarnessContext, SessionIdentitySource, SessionOpenRequest, TerminalOpenOptions,
};
use crate::orchestrator::SpawnOptions;

struct EndingSession(Arc<dyn AgentSession>);

impl Drop for EndingSession {
    fn drop(&mut self) {
        self.0.end();
    }
}

// The fake replaces only the executable. It consumes the policy and sidecar
// selected by the public Pi spec, with its original exact-session argv intact.
// Stage files release real extension boundaries without timing the fake work.
const CHILD: &str = r#"
import json, os, pathlib, sys, time
root, outcome = pathlib.Path(sys.argv[1]), sys.argv[2]
policy = json.loads(os.environ['BUILD_PI_COMPACTION_POLICY'])
log = pathlib.Path(os.environ['BUILD_PI_TELEMETRY_LOG'])
sequence = 0
def stage(name):
    (root / name).touch()
def wait(name):
    while not (root / name).exists():
        time.sleep(0.01)
def emit(kind, **fields):
    global sequence
    sequence += 1
    value = {key: policy[key] for key in ['launch_id', 'thread_id', 'owner']}
    value.update(sequence=sequence, type=kind, **fields)
    with log.open('a') as out:
        out.write(json.dumps(value) + '\n')
def capture():
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)
    with (root / 'stdin.txt').open('a') as out:
        out.write(line)
    return line
assert sys.argv[sys.argv.index('--session-id') + 1] == policy['owner']
emit('ready')
stage('ready')
capture()
emit('working')
emit('context', tokens=200000)
stage('working')
wait('early-end')
emit('turn_end')
emit('agent_end')
stage('early-ended')
if outcome == 'overflow':
    wait('overflow')
    for failed in ['failed', 'aborted', 'cancelled']:
        emit('compaction', completed=False, reason='overflow')
        emit('compaction', completed=False, failed=True, reason='overflow', outcome=failed)
    print('Compacted (terminal text only)', flush=True)
    stage('failures')
    wait('success')
    emit('compaction', completed=False, reason='overflow')
    emit('compaction', completed=True, reason='overflow')
    emit('context', tokens=None)
    stage('compacted')
wait('settle')
emit('settled')
stage('settled')
line = capture()
if outcome != 'overflow':
    assert line.strip() == '/compact', repr(line)
    emit('compaction', completed=False, reason='manual')
    stage('compact-requested')
    wait('complete')
    if outcome == 'success':
        emit('compaction', completed=True, reason='manual')
    else:
        emit('compaction', completed=False, failed=True, reason='manual', outcome=outcome)
    emit('context', tokens=None if outcome == 'success' else 200000)
    emit('settled')
    line = capture()
assert line.strip() == 'queued follow-up', repr(line)
emit('working')
stage('follow-up')
if outcome == 'overflow':
    wait('new-context')
    emit('context', tokens=200000)
    stage('new-context-recorded')
while True:
    capture()
"#;

fn launch(agent: &CompactingAgent, directory: &Path, outcome: &str) -> (EndingSession, TabKey) {
    let choice = ModelChoice {
        provider: AgentProvider::Pi,
        ..ModelChoice::default()
    };
    let harness = harness_for(AgentProvider::Pi);
    let mut app = agent.state.lock().unwrap();
    app.set_agent_model_choice(RUN, &agent.agent_id, choice.clone())
        .unwrap();
    let policy = harness
        .prepare_compaction_policy(
            app.resolve_compaction_policy(RUN, &agent.agent_id).unwrap(),
            choice.clone(),
            Some("0.86.1".into()),
            CompactionLaunchIdentity::new(
                app.agent_conversation(RUN, Some(&agent.agent_id))
                    .unwrap()
                    .id
                    .clone(),
            ),
        )
        .unwrap();
    let options = SpawnOptions {
        cwd: agent.root.clone(),
        owner_id: agent.agent_id.clone(),
        compaction_policy: Some(policy.clone()),
        ..SpawnOptions::default()
    };
    let context = HarnessContext {
        state_root: directory.join("state"),
        bridge_exe: directory.join("unused-bridge"),
        mcp_socket: directory.join("unused-socket"),
    };
    std::fs::create_dir_all(&context.state_root).unwrap();
    let mut spec = harness.spec(&choice, &options, &context).unwrap();
    std::fs::write(directory.join("fake-pi.py"), CHILD).unwrap();
    spec.binary = "python3".into();
    spec.args.splice(
        0..0,
        [
            "-u".into(),
            directory.join("fake-pi.py").to_string_lossy().into_owned(),
            directory.to_string_lossy().into_owned(),
            outcome.into(),
        ],
    );
    spec.env
        .push(("HOME".into(), directory.to_string_lossy().into_owned()));
    let session = harness
        .open_session(SessionOpenRequest {
            spec,
            root: agent.root.clone(),
            choice,
            compaction_policy: Some(policy),
            terminal: TerminalOpenOptions {
                size: portable_pty::PtySize::default(),
                turn_ready_grace: None,
                identity: Some(SessionIdentitySource::Known(agent.agent_id.clone())),
                activity_locator: None,
            },
            resume_session_id: None,
            recovery_context: None,
        })
        .unwrap()
        .session;
    let activity = session.activity();
    let key = insert_agent_tab(
        &mut app,
        &agent.root,
        RUN,
        &agent.agent_id,
        DictatedSession::reporting(AgentStatus::Starting),
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
    .unwrap_or_else(|| panic!("fake Pi never reached {stage}"));
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

async fn start_and_queue(agent: &CompactingAgent, session: &dyn AgentSession, directory: &Path) {
    reached(directory, "ready").await;
    wait_for(Duration::from_secs(5), || {
        (session.status() == AgentStatus::Waiting).then_some(())
    })
    .await
    .expect("extension readiness reaches status watch");
    assert_eq!(
        session.compaction_policy().unwrap().enforcement,
        CompactionEnforcement::BetweenTurn
    );
    session.send_turn(&Turn::new("long tool loop")).unwrap();
    reached(directory, "working").await;
    wait_for(Duration::from_secs(5), || {
        (agent.agent().last_context_tokens == Some(200_000)).then_some(())
    })
    .await
    .expect("real status pump persists inclusive threshold crossing");
    let mut turn = agent.turn(false);
    turn.say = Some(TurnText {
        cold: "queued follow-up".into(),
        warm: "queued follow-up".into(),
    });
    agent.state.lock().unwrap().delivery_queue.enqueue(turn);
    release(directory, "early-end");
    reached(directory, "early-ended").await;
}

fn assert_open(agent: &CompactingAgent, session: &dyn AgentSession, key: &TabKey) {
    assert_eq!(session.status(), AgentStatus::Working);
    assert_eq!(
        session.session_id().as_deref(),
        Some(agent.agent_id.as_str())
    );
    assert!(agent.agent().working_since.is_some());
    assert_eq!(agent.still_queued(), 1);
    assert!(!session.automatic_compaction_due(Some(200_000)));
    assert!(
        agent
            .state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(key)
            .unwrap()
            .live
    );
}

async fn assert_no_early_compaction(
    agent: &CompactingAgent,
    session: &dyn AgentSession,
    key: &TabKey,
    directory: &Path,
) {
    session.backdate_last_output(AGENT_WORKING_WINDOW + Duration::from_secs(1));
    deliver_pending_agent_turns(&agent.state);
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_open(agent, session, key);
    assert!(
        session.send_turn(&Turn::new("/compact")).is_err(),
        "manual compaction must not abort active tools"
    );
    assert_eq!(
        std::fs::read_to_string(directory.join("stdin.txt")).unwrap(),
        "long tool loop\n"
    );
}

async fn assert_delivered_once(
    agent: &CompactingAgent,
    session: &dyn AgentSession,
    directory: &Path,
    compacted: bool,
) {
    reached(directory, "follow-up").await;
    deliver_pending_agent_turns(&agent.state);
    assert_eq!(agent.still_queued(), 0);
    let expected = if compacted {
        "long tool loop\n/compact\nqueued follow-up\n"
    } else {
        "long tool loop\nqueued follow-up\n"
    };
    assert_eq!(
        std::fs::read_to_string(directory.join("stdin.txt")).unwrap(),
        expected
    );
    assert_eq!(
        agent.agent().last_context_tokens,
        None,
        "null post-compaction context invalidates the persisted reading"
    );
    assert!(
        !session.automatic_compaction_due(Some(200_000)),
        "old persisted usage cannot trigger again"
    );
    assert_eq!(session.epitaph(), None);
}

async fn fallback(outcome: &str) {
    let directory = tempfile::tempdir().unwrap();
    let agent = CompactingAgent::without_background_services().with_context(199_999);
    let (session, key) = launch(&agent, directory.path(), outcome);
    start_and_queue(&agent, session.0.as_ref(), directory.path()).await;
    assert_no_early_compaction(&agent, session.0.as_ref(), &key, directory.path()).await;
    release(directory.path(), "settle");
    reached(directory.path(), "compact-requested").await;
    assert_open(&agent, session.0.as_ref(), &key);
    assert_eq!(completed_compactions(&agent), 0);
    release(directory.path(), "complete");
    assert_delivered_once(&agent, session.0.as_ref(), directory.path(), true).await;
    wait_for(Duration::from_secs(5), || {
        (completed_compactions(&agent) == usize::from(outcome == "success")).then_some(())
    })
    .await
    .expect("only successful summary produces a completion");
}

#[tokio::test]
async fn pi_fallback_waits_for_settled_then_compacts_and_delivers_follow_up_once() {
    fallback("success").await;
}

#[tokio::test]
async fn pi_summary_failure_abort_and_cancel_deliver_follow_up_without_success() {
    for outcome in ["failed", "aborted", "cancelled"] {
        fallback(outcome).await;
    }
}

#[tokio::test]
async fn pi_overflow_recovery_preserves_the_open_turn_and_clears_stale_context() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CompactingAgent::without_background_services().with_context(199_999);
    let (session, key) = launch(&agent, directory.path(), "overflow");
    start_and_queue(&agent, session.0.as_ref(), directory.path()).await;
    assert_no_early_compaction(&agent, session.0.as_ref(), &key, directory.path()).await;
    release(directory.path(), "overflow");
    reached(directory.path(), "failures").await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert_eq!(completed_compactions(&agent), 0);
    assert_open(&agent, session.0.as_ref(), &key);
    release(directory.path(), "success");
    reached(directory.path(), "compacted").await;
    wait_for(Duration::from_secs(5), || {
        (completed_compactions(&agent) == 1 && agent.agent().last_context_tokens.is_none())
            .then_some(())
    })
    .await
    .expect("overflow completion clears persisted context through both real pumps");
    assert_open(&agent, session.0.as_ref(), &key);
    release(directory.path(), "settle");
    assert_delivered_once(&agent, session.0.as_ref(), directory.path(), false).await;
    release(directory.path(), "new-context");
    reached(directory.path(), "new-context-recorded").await;
    wait_for(Duration::from_secs(5), || {
        (agent.agent().last_context_tokens == Some(200_000)).then_some(())
    })
    .await
    .expect("an identical reading after reset is new context, not a duplicate");
}
