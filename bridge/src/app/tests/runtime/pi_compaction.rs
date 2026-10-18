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
generation = 1
def stage(name):
    (root / name).touch()
def wait(name):
    while not (root / name).exists():
        time.sleep(0.01)
def emit(kind, **fields):
    global sequence
    sequence += 1
    value = {key: policy[key] for key in ['launch_id', 'thread_id', 'owner']}
    value.update(sequence=sequence, generation=generation, type=kind, **fields)
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
if outcome == 'health-failure':
    emit('context', tokens=200000)
    emit('settled')
    stage('failure-context')
    wait('fail')
    emit('unsupported')
    stage('failed')
    wait('recover')
    generation = 2
    sequence = 0
    emit('starting')
    emit('ready')
    emit('context', tokens=200000)
    emit('settled')
    stage('recovered')
    wait('fail-again')
    emit('unsupported')
    stage('failed-again')
    while True:
        capture()
if outcome in ['manual-command', 'manual-observation']:
    if outcome == 'manual-command':
        assert capture().strip() == '/compact'
    else:
        wait('manual-request')
    emit('working')
    emit('compaction', completed=False, reason='manual')
    stage('compact-requested')
    wait('complete')
    emit('compaction', completed=True, reason='manual')
    emit('context', tokens=None)
    emit('settled')
    stage('manual-completed')
    while True:
        capture()
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
    emit('compaction', completed=False, reason='overflow')
    print('Compacted (terminal text only)', flush=True)
    stage('compacting')
    wait('success')
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
    let opened = harness
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
        .unwrap();
    let session = opened.session;
    let activity = opened.output.activity;
    let key = insert_agent_tab(
        &mut app,
        &agent.root,
        RUN,
        &agent.agent_id,
        DictatedSession::reporting(AgentStatus::Starting),
    );
    let tab = app.session_registry.test_tab_mut(&key).unwrap();
    tab.role = TabRole::Agent {
        owner: RUN.into(),
        agent_id: agent.agent_id.clone(),
        provider: AgentProvider::Pi,
    };
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

fn compaction_history(agent: &CompactingAgent) -> Vec<String> {
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

fn completed_compactions(agent: &CompactingAgent) -> usize {
    compaction_history(agent)
        .iter()
        .filter(|summary| summary.as_str() == "Compacted")
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
    // A queued follow-up is released by the status pump. Calling the generic
    // explicit drain here would intentionally steer a same-model live turn.
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
        "the consumed threshold reading cannot recursively schedule compaction"
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
    let telemetry = session.0.status_changed().unwrap();
    assert_eq!(
        telemetry
            .borrow()
            .context
            .map(|context| context.context_tokens),
        (outcome != "success").then_some(200_000),
        "failed summaries retain actual usage without renewing the consumed threshold reading"
    );
    wait_for(Duration::from_secs(5), || {
        (completed_compactions(&agent) == usize::from(outcome == "success")).then_some(())
    })
    .await
    .expect("only successful summary produces a completion");
    if outcome == "success" {
        assert_eq!(
            compaction_history(&agent),
            ["Compacted"],
            "one command and its hooks describe one completed attempt"
        );
    }
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
    reached(directory.path(), "compacting").await;
    wait_for(Duration::from_secs(5), || {
        (compaction_history(&agent) == ["Compacting"]).then_some(())
    })
    .await
    .expect("unsolicited overflow still publishes its start");
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
    assert_eq!(compaction_history(&agent), ["Compacted"]);
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

async fn manual_compaction_history(outcome: &str) {
    let directory = tempfile::tempdir().unwrap();
    let agent = CompactingAgent::without_background_services().with_context(199_999);
    let (session, _) = launch(&agent, directory.path(), outcome);
    wait_for(Duration::from_secs(5), || {
        (session.0.status() == AgentStatus::Waiting).then_some(())
    })
    .await
    .expect("manual compaction starts from the extension's idle boundary");
    if outcome == "manual-command" {
        let mut turn = agent.turn(false);
        turn.say = Some(TurnText {
            cold: "/compact".into(),
            warm: "/compact".into(),
        });
        agent.state.lock().unwrap().delivery_queue.enqueue(turn);
        deliver_pending_agent_turns(&agent.state);
    } else {
        release(directory.path(), "manual-request");
    }
    reached(directory.path(), "compact-requested").await;
    wait_for(Duration::from_secs(5), || {
        (!compaction_history(&agent).is_empty()).then_some(())
    })
    .await
    .expect("manual start reaches the real activity pump");
    release(directory.path(), "complete");
    reached(directory.path(), "manual-completed").await;
    wait_for(Duration::from_secs(5), || {
        (completed_compactions(&agent) == 1).then_some(())
    })
    .await
    .expect("manual completion reaches the real activity pump");
    assert_eq!(
        compaction_history(&agent),
        ["Compacted"],
        "no duplicate active row remains after completion"
    );
}

#[tokio::test]
async fn pi_manual_command_has_one_completed_history_row() {
    manual_compaction_history("manual-command").await;
}

#[tokio::test]
async fn pi_unsolicited_manual_compaction_keeps_start_and_completion_observations() {
    manual_compaction_history("manual-observation").await;
}

/// Audit actual durable roster writes in this fixture's throwaway database.
/// No production counter is added: the one-agent fixture's normal persistence
/// path hits this trigger even when an UPDATE merely rewrites unchanged JSON.
fn audit_agent_writes(agent: &CompactingAgent) -> rusqlite::Connection {
    let database = agent
        .state
        .lock()
        .unwrap()
        .state_root
        .join("store/build.db");
    let connection = rusqlite::Connection::open(database).unwrap();
    connection.busy_timeout(Duration::from_secs(5)).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE pi_agent_write_audit (writes INTEGER NOT NULL);
         CREATE TRIGGER pi_agent_write_capture AFTER UPDATE OF record ON agents
         BEGIN INSERT INTO pi_agent_write_audit VALUES (1); END;",
        )
        .unwrap();
    connection
}

fn agent_write_count(connection: &rusqlite::Connection) -> u64 {
    connection
        .query_row("SELECT COUNT(*) FROM pi_agent_write_audit", [], |row| {
            row.get(0)
        })
        .unwrap()
}

fn invalidation_count(agent: &CompactingAgent) -> u64 {
    agent.state.lock().unwrap().changes.board_revision()
}

async fn failure_reached(agent: &CompactingAgent, session: &dyn AgentSession) {
    wait_for(Duration::from_secs(5), || {
        (agent.agent().last_context_tokens.is_none()
            && session.compaction_policy().unwrap().enforcement
                == CompactionEnforcement::Unsupported)
            .then_some(())
    })
    .await
    .expect("the extension failure clears persisted context through the real status pump");
}

#[tokio::test]
async fn pi_failed_generation_invalidates_persistence_once_across_repeated_health_polls() {
    let directory = tempfile::tempdir().unwrap();
    let agent = CompactingAgent::without_background_services().with_context(199_999);
    let (session, _) = launch(&agent, directory.path(), "health-failure");
    reached(directory.path(), "failure-context").await;
    wait_for(Duration::from_secs(5), || {
        (agent.agent().last_context_tokens == Some(200_000)
            && session.0.status() == AgentStatus::Waiting)
            .then_some(())
    })
    .await
    .expect("the initial generation's trustworthy context is persisted");
    // Keep legacy terminal fallback idle as well, so its working clock cannot
    // add unrelated roster writes during the persistence observation window.
    session
        .0
        .backdate_last_output(AGENT_WORKING_WINDOW + Duration::from_secs(1));
    let pid = session.0.terminal().unwrap().pid();
    let connection = audit_agent_writes(&agent);
    let before = invalidation_count(&agent);
    release(directory.path(), "fail");
    reached(directory.path(), "failed").await;
    failure_reached(&agent, session.0.as_ref()).await;
    assert_eq!(agent_write_count(&connection), 1);
    assert_eq!(invalidation_count(&agent), before + 1);

    // The follower's stale budget is ten seconds, and its real health loop
    // polls every 25 ms. This spans another fifty polls after that budget.
    tokio::time::sleep(Duration::from_millis(11_250)).await;
    assert_eq!(
        agent_write_count(&connection),
        1,
        "failed health polls must not keep rewriting the agent row"
    );
    assert_eq!(
        invalidation_count(&agent),
        before + 1,
        "failed health polls must not repeatedly invalidate clients"
    );
    assert_eq!(session.0.terminal().unwrap().pid(), pid);
    assert!(!session.0.exited_within(Duration::ZERO));

    release(directory.path(), "recover");
    reached(directory.path(), "recovered").await;
    wait_for(Duration::from_secs(5), || {
        (agent.agent().last_context_tokens == Some(200_000)
            && session.0.compaction_policy().unwrap().enforcement
                == CompactionEnforcement::BetweenTurn)
            .then_some(())
    })
    .await
    .expect("a fresh generation recovers and persists fresh context");
    let recovered_writes = agent_write_count(&connection);
    let recovered_invalidations = invalidation_count(&agent);
    release(directory.path(), "fail-again");
    reached(directory.path(), "failed-again").await;
    failure_reached(&agent, session.0.as_ref()).await;
    assert_eq!(agent_write_count(&connection), recovered_writes + 1);
    assert_eq!(invalidation_count(&agent), recovered_invalidations + 1);
}
