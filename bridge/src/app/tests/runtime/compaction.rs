//! Compacting a warm session before the turn that would grow it past its
//! agent's threshold, and delivering that turn once the compaction is over.

use super::*;
use crate::harness::{SessionStatusSnapshot, TurnContext};

const RUN: &str = "run-compact";

/// A run whose agent talks to the Codex app server — a harness that compacts
/// on `/compact` — through a live session waiting at its prompt.
struct CompactingAgent {
    _dir: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    root: PathBuf,
    agent_id: String,
    log: SessionLog,
    status: tokio::sync::watch::Sender<SessionStatusSnapshot>,
}

impl CompactingAgent {
    fn new() -> CompactingAgent {
        let (dir, repo) = init_repo();
        let mut app = qa_state(&repo, dir.path());
        let root = insert_run(&mut app, &repo, dir.path(), RUN, RunState::Building);
        let agent_id = crate::agent::derived_agent_id(RUN);
        app.set_agent_model_choice(
            RUN,
            &agent_id,
            ModelChoice {
                provider: AgentProvider::CodexAppServer,
                ..ModelChoice::default()
            },
        )
        .unwrap();
        let (status, watched) =
            tokio::sync::watch::channel(SessionStatusSnapshot::new(AgentStatus::Waiting));
        let log = SessionLog::default();
        insert_agent_tab(
            &mut app,
            &root,
            RUN,
            &agent_id,
            DictatedSession::reporting(AgentStatus::Waiting)
                .recording_into(&log)
                .watching_status(watched),
        );
        CompactingAgent {
            _dir: dir,
            state: app.shared(),
            root,
            agent_id,
            log,
            status,
        }
    }

    fn with_context(self, context_tokens: u64) -> CompactingAgent {
        self.state.lock().unwrap().record_agent_turn_context(
            RUN,
            &self.agent_id,
            TurnContext {
                context_tokens,
                cache_read_tokens: 7,
            },
        );
        self
    }

    fn edit(&self, edit: impl FnOnce(&mut crate::agent::Agent)) {
        self.state
            .lock()
            .unwrap()
            .edit_agent_record("test", RUN, &self.agent_id, edit);
    }

    fn agent(&self) -> crate::agent::Agent {
        self.state.lock().unwrap().runs[RUN]
            .agents
            .by_id(&self.agent_id)
            .unwrap()
            .clone()
    }

    fn turn(&self, interrupt: bool) -> PendingAgentTurn {
        let agent = self.agent();
        PendingAgentTurn {
            operation_id: None,
            root: AppState::canonical_root(&self.root),
            owner: RUN.to_string(),
            agent_id: self.agent_id.clone(),
            conversation_id: agent.conversation_id().to_string(),
            model_choice: agent.choice.clone(),
            choice_revision: agent.choice_revision,
            interrupt,
            say: Some(TurnText {
                cold: "cold".into(),
                warm: "warm".into(),
            }),
            phase: "build",
            wants_catch_up: false,
            survives_refusal: false,
        }
    }

    /// Queue one turn and run the drain, the way a verb that queued it does.
    fn say(&self, interrupt: bool) {
        let turn = self.turn(interrupt);
        self.state.lock().unwrap().delivery_queue.enqueue(turn);
        deliver_pending_agent_turns(&self.state);
    }

    fn still_queued(&self) -> usize {
        self.state.lock().unwrap().delivery_queue.queued().count()
    }
}

#[test]
fn a_warm_session_at_its_threshold_compacts_first_and_hears_the_turn_on_the_next_drain() {
    let agent = CompactingAgent::new().with_context(200_000);

    agent.say(false);

    assert_eq!(agent.log.turns(), vec!["/compact".to_string()]);
    assert_eq!(agent.still_queued(), 1, "the turn waits for the compaction");
    assert_eq!(
        agent.agent().last_context_tokens,
        None,
        "the reading that asked for it is spent"
    );

    deliver_pending_agent_turns(&agent.state);

    assert_eq!(
        agent.log.turns(),
        vec!["/compact".to_string(), "warm".to_string()]
    );
    assert_eq!(agent.still_queued(), 0);
}

#[test]
fn below_the_threshold_the_turn_goes_straight_through() {
    let agent = CompactingAgent::new().with_context(199_999);

    agent.say(false);

    assert_eq!(agent.log.turns(), vec!["warm".to_string()]);
    assert_eq!(agent.agent().last_context_tokens, Some(199_999));
}

#[test]
fn the_conversations_own_limit_beats_the_devices() {
    let agent = CompactingAgent::new().with_context(60_000);
    agent.edit(|record| record.max_context_tokens = Some(50_000));

    agent.say(false);

    assert_eq!(agent.log.turns(), vec!["/compact".to_string()]);
}

#[test]
fn a_conversation_limit_of_zero_never_compacts() {
    let agent = CompactingAgent::new().with_context(900_000);
    agent.edit(|record| record.max_context_tokens = Some(0));

    agent.say(false);

    assert_eq!(agent.log.turns(), vec!["warm".to_string()]);
}

#[test]
fn a_device_threshold_of_zero_never_compacts() {
    let agent = CompactingAgent::new().with_context(900_000);
    agent.state.lock().unwrap().compact_above_tokens = 0;

    agent.say(false);

    assert_eq!(agent.log.turns(), vec!["warm".to_string()]);
}

/// A turn for an agent that is still working reaches its session as it
/// always has — the session queues or steers it — and no compaction is pushed
/// into the middle of the turn in flight, interrupting or not.
#[test]
fn a_working_agent_is_never_compacted_mid_turn() {
    let agent = CompactingAgent::new().with_context(900_000);
    agent.state.lock().unwrap().record_agent_working_since(
        RUN,
        &agent.agent_id,
        Some(now_rfc3339()),
    );

    agent.say(false);
    agent.say(true);

    assert_eq!(
        agent.log.turns(),
        vec!["warm".to_string(), "warm".to_string()]
    );
    assert_eq!(
        agent.agent().last_context_tokens,
        Some(900_000),
        "the reading waits for a turn between turns"
    );
}

#[test]
fn only_a_warm_session_is_compacted() {
    let agent = CompactingAgent::new().with_context(900_000);
    let turn = agent.turn(false);
    let app = agent.state.lock().unwrap();

    assert!(app.compaction_due_before(&turn, Spawned::Warm, "warm"));
    assert!(
        !app.compaction_due_before(&turn, Spawned::Fresh, "cold"),
        "a fresh session has no measured context to compact"
    );
    assert!(
        !app.compaction_due_before(&turn, Spawned::Warm, "/compact"),
        "a turn that is itself a compaction is not preceded by another"
    );
}

#[test]
fn a_harness_that_cannot_compact_is_never_asked_to() {
    let agent = CompactingAgent::new().with_context(900_000);
    let mut turn = agent.turn(false);
    turn.model_choice.provider = AgentProvider::Pi;

    assert!(!agent
        .state
        .lock()
        .unwrap()
        .compaction_due_before(&turn, Spawned::Warm, "warm"));
}

/// The whole loop through the status pump: the reading that asks for the
/// compaction is recorded, the compaction runs, and the stale reading every
/// later snapshot still carries never asks for a second one.
#[tokio::test]
async fn one_compaction_per_reading_and_the_turn_follows_it() {
    let agent = CompactingAgent::new();
    let key = TabKey::agent(&AppState::canonical_root(&agent.root), &agent.agent_id);
    let (session, instance) = {
        let app = agent.state.lock().unwrap();
        let tab = app.session_registry.test_tab(&key).unwrap();
        (Arc::clone(&tab.session), tab.session_instance.clone())
    };
    spawn_status_pump(
        &agent.state,
        key,
        session,
        instance,
        Some(agent.status.subscribe()),
    );
    let stale = TurnContext {
        context_tokens: 250_000,
        cache_read_tokens: 40_000,
    };
    agent
        .status
        .send_modify(|snapshot| snapshot.context = Some(stale));
    wait_for(Duration::from_secs(2), || {
        (agent.agent().last_context_tokens == Some(250_000)).then_some(())
    })
    .await
    .expect("the pump records the reading");
    assert_eq!(agent.agent().session_cache_read_tokens, Some(40_000));

    agent.say(false);
    assert_eq!(agent.log.turns(), vec!["/compact".to_string()]);

    let working = agent
        .status
        .borrow()
        .transition(AgentStatus::Working)
        .unwrap();
    agent.status.send(working.clone()).unwrap();
    wait_for(Duration::from_secs(2), || {
        agent.agent().working_since.is_some().then_some(())
    })
    .await
    .expect("the compaction is a turn of its own");
    agent
        .status
        .send(working.transition(AgentStatus::Waiting).unwrap())
        .unwrap();

    wait_for(Duration::from_secs(2), || {
        (agent.log.turns().len() == 2).then_some(())
    })
    .await
    .expect("the waiting session hears the deferred turn");
    assert_eq!(
        agent.log.turns(),
        vec!["/compact".to_string(), "warm".to_string()]
    );
    assert_eq!(
        agent.agent().last_context_tokens,
        None,
        "the stale reading is never written back"
    );
}
