//! Compacting a warm session before the turn that would grow it past its
//! agent's threshold, and delivering that turn once the compaction is over.

use super::*;
use crate::harness::{SessionStatusSnapshot, TurnContext};

pub(super) const RUN: &str = "run-compact";

/// A run whose agent talks to the Codex app server — a harness that compacts
/// on `/compact` — through a live session waiting at its prompt.
pub(super) struct CompactingAgent {
    _dir: tempfile::TempDir,
    pub(super) state: Arc<Mutex<AppState>>,
    pub(super) root: PathBuf,
    pub(super) agent_id: String,
    pub(super) log: SessionLog,
    pub(super) status: tokio::sync::watch::Sender<SessionStatusSnapshot>,
}

impl CompactingAgent {
    pub(super) fn new() -> CompactingAgent {
        let (dir, repo) = init_repo();
        let mut app = qa_state(&repo, dir.path());
        let root = insert_run(&mut app, &repo, dir.path(), RUN, RunState::Building);
        let agent_id = crate::agent::derived_agent_id(RUN);
        app.edit_agent_record("test", RUN, &agent_id, |agent| {
            agent.name = Some("Compaction".to_string());
            agent.name_asked = true;
        });
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

    pub(super) fn with_context(self, context_tokens: u64) -> CompactingAgent {
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

    pub(super) fn edit(&self, edit: impl FnOnce(&mut crate::agent::Agent)) {
        self.state
            .lock()
            .unwrap()
            .edit_agent_record("test", RUN, &self.agent_id, edit);
    }

    pub(super) fn agent(&self) -> crate::agent::Agent {
        self.state.lock().unwrap().runs[RUN]
            .agents
            .by_id(&self.agent_id)
            .unwrap()
            .clone()
    }

    pub(super) fn turn(&self, interrupt: bool) -> PendingAgentTurn {
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
    pub(super) fn say(&self, interrupt: bool) {
        let turn = self.turn(interrupt);
        self.state.lock().unwrap().delivery_queue.enqueue(turn);
        deliver_pending_agent_turns(&self.state);
    }

    pub(super) fn still_queued(&self) -> usize {
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
    assert_eq!(agent.agent().last_context_at, None, "and so is its time");

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

/// Claude reports its compaction's start through a hook rather than through
/// `starts_compaction`, and still compacts on `/compact`.
#[test]
fn a_claude_session_past_its_threshold_is_compacted() {
    let agent = CompactingAgent::new().with_context(900_000);
    for provider in [AgentProvider::ClaudeAdk, AgentProvider::Claude] {
        let mut turn = agent.turn(false);
        turn.model_choice.provider = provider;
        let app = agent.state.lock().unwrap();

        assert!(
            app.compaction_due_before(&turn, Spawned::Warm, "warm"),
            "{provider:?} compacts on /compact"
        );
        assert!(
            !app.compaction_due_before(&turn, Spawned::Warm, "/compact focus on the API"),
            "{provider:?}: a turn that is itself a compaction is not preceded by another"
        );
    }
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

impl CompactingAgent {
    fn handle(&self, method: &str, params: Value) -> Value {
        self.state.lock().unwrap().handle(req(method, params))
    }

    /// The agent as `agent.list` shows it — the same list the project agent
    /// reads through `list_workspace_agents`.
    fn listed(&self) -> Value {
        let listed = self.handle("agent.list", json!({ "entity_id": RUN }));
        assert_eq!(listed["ok"], true, "{listed:?}");
        listed["result"]["agents"][0].clone()
    }

    fn set_limit(&self, max_context_tokens: Value) -> Value {
        self.handle(
            "conversation.settings",
            json!({
                "entity_id": RUN,
                "agent_id": self.agent_id,
                "max_context_tokens": max_context_tokens,
            }),
        )
    }
}

#[test]
fn the_digest_carries_the_context_and_the_threshold_in_effect() {
    let agent = CompactingAgent::new();
    let unmeasured = agent.listed();
    assert_eq!(unmeasured["last_context_tokens"], Value::Null);
    assert_eq!(unmeasured["last_context_at"], Value::Null);
    assert_eq!(unmeasured["session_cache_read_tokens"], Value::Null);
    assert_eq!(unmeasured["max_context_tokens"], Value::Null);
    assert_eq!(unmeasured["compact_at_tokens"], 200_000);

    let agent = agent.with_context(123_456);
    let measured = agent.listed();
    assert_eq!(measured["last_context_tokens"], 123_456);
    assert_eq!(measured["session_cache_read_tokens"], 7);
    assert!(
        measured["last_context_at"]
            .as_str()
            .is_some_and(|at| at.contains('T')),
        "the reading says when it was taken: {measured:?}"
    );
}

#[test]
fn conversation_settings_sets_and_clears_the_conversations_own_limit() {
    let agent = CompactingAgent::new();

    let set = agent.set_limit(json!(50_000));
    assert_eq!(set["ok"], true, "{set:?}");
    assert_eq!(set["result"]["agent_id"], json!(agent.agent_id));
    assert_eq!(set["result"]["max_context_tokens"], 50_000);
    assert_eq!(set["result"]["compact_at_tokens"], 50_000);
    assert_eq!(agent.agent().max_context_tokens, Some(50_000));
    assert_eq!(agent.listed()["compact_at_tokens"], 50_000);

    let never = agent.set_limit(json!(0));
    assert_eq!(never["result"]["compact_at_tokens"], 0, "{never:?}");

    let cleared = agent.set_limit(Value::Null);
    assert_eq!(cleared["ok"], true, "{cleared:?}");
    assert_eq!(cleared["result"]["max_context_tokens"], Value::Null);
    assert_eq!(cleared["result"]["compact_at_tokens"], 200_000);
    assert_eq!(agent.agent().max_context_tokens, None);
}

#[test]
fn conversation_settings_refuses_what_it_cannot_honour() {
    let agent = CompactingAgent::new();

    let unnamed = agent.handle(
        "conversation.settings",
        json!({ "entity_id": RUN, "agent_id": agent.agent_id }),
    );
    assert_eq!(unnamed["error_code"], "invalid_params", "{unnamed:?}");

    let negative = agent.set_limit(json!(-1));
    assert_eq!(negative["error_code"], "invalid_params", "{negative:?}");

    let nobody = agent.handle(
        "conversation.settings",
        json!({ "entity_id": RUN, "agent_id": "agent-nobody", "max_context_tokens": 1 }),
    );
    assert_eq!(nobody["ok"], false, "{nobody:?}");
    assert_eq!(agent.agent().max_context_tokens, None);
}
