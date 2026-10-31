//! Compacting a session because an agent asked: `compact_self` for its own,
//! `compact_agent` for another of its project's. Between turns the compaction
//! goes at once; mid-turn it waits for the turn to end and then goes ahead of
//! whatever turn is queued. Either way its row says what it was focused on and
//! how big the context was either side of it.

use super::compaction::{CompactingAgent, RUN};
use super::*;
use crate::app::tests::project_agent::{added_project, items, project_agent, rooted, workspace};
use crate::harness::{AgentActivity, TurnContext};
use crate::mcp::BridgeAction;

fn capture_policy(
    app: &mut AppState,
    root: &Path,
    owner: &str,
    agent_id: &str,
    version: Option<&str>,
) {
    let choice = app
        .entity_agents(owner)
        .unwrap()
        .by_id(agent_id)
        .unwrap()
        .choice
        .clone();
    let policy = crate::harness::harness_for(choice.provider)
        .prepare_compaction_policy(
            app.resolve_compaction_policy(owner, agent_id).unwrap(),
            choice,
            version.map(str::to_owned),
            crate::harness::compaction::CompactionLaunchIdentity::new(
                app.agent_conversation(owner, Some(agent_id))
                    .unwrap()
                    .id
                    .clone(),
            ),
        )
        .unwrap();
    let key = TabKey::agent(&AppState::canonical_root(root), agent_id);
    let tab = app.session_registry.test_tab_mut(&key).unwrap();
    tab.session =
        crate::harness::compaction::PolicySession::attach(Arc::clone(&tab.session), policy);
}

fn context(context_tokens: u64) -> TurnContext {
    TurnContext {
        context_tokens,
        cache_read_tokens: 0,
    }
}

/// The drain the MCP socket runs after every tool call.
fn drain(state: &Arc<Mutex<AppState>>) {
    DeliveryRunner::drain(state, &a_frame(state));
}

/// The one `Compaction` row an agent's conversation carries, as the client
/// reads it.
fn compaction_row(state: &Arc<Mutex<AppState>>, owner: &str, agent_id: &str) -> Value {
    let items = items(&mut state.lock().unwrap(), owner, agent_id);
    let rows: Vec<Value> = items
        .into_iter()
        .filter(|item| item["data"]["event"] == "compaction")
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    rows[0]["data"].clone()
}

/// The harness says it started compacting, then reports what it left.
fn the_harness_compacts(state: &Arc<Mutex<AppState>>, owner: &str, agent_id: &str, after: u64) {
    let mut app = state.lock().unwrap();
    app.record_agent_activity(
        owner,
        agent_id,
        &AgentActivity::Compaction { completed: false },
        None,
    );
    app.record_agent_turn_context(owner, agent_id, context(after));
}

impl CompactingAgent {
    fn with_captured_policy(self, version: Option<&str>) -> Self {
        capture_policy(
            &mut self.state.lock().unwrap(),
            &self.root,
            RUN,
            &self.agent_id,
            version,
        );
        self
    }

    fn on(self, provider: AgentProvider) -> CompactingAgent {
        self.state
            .lock()
            .unwrap()
            .set_agent_model_choice(
                RUN,
                &self.agent_id,
                ModelChoice {
                    provider,
                    ..ModelChoice::default()
                },
            )
            .unwrap();
        self
    }

    fn compact_self(&self, instructions: &str) -> Result<Value, String> {
        self.state.lock().unwrap().agent_action(
            RUN,
            &self.agent_id,
            BridgeAction::CompactSelf {
                instructions: instructions.to_string(),
            },
        )
    }

    fn pump(&self) {
        let key = TabKey::agent(&AppState::canonical_root(&self.root), &self.agent_id);
        let (session, instance) = {
            let app = self.state.lock().unwrap();
            let tab = app.session_registry.test_tab(&key).unwrap();
            (Arc::clone(&tab.session), tab.session_instance.clone())
        };
        spawn_status_pump(
            &self.state,
            key,
            session,
            instance,
            Some(self.status.subscribe()),
        );
    }

    /// Move the session to `status` and wait for the pump to see it.
    async fn becomes(&self, status: AgentStatus) {
        let working = matches!(status, AgentStatus::Working);
        let next = self.status.borrow().transition(status).unwrap();
        self.status.send(next).unwrap();
        wait_for(Duration::from_secs(2), || {
            (self.agent().working_since.is_some() == working).then_some(())
        })
        .await
        .expect("the pump records the status");
    }

    async fn heard(&self, turns: usize) -> Vec<String> {
        wait_for(Duration::from_secs(2), || {
            (self.log.turns().len() >= turns).then_some(())
        })
        .await
        .unwrap_or_else(|| panic!("{turns} turns, heard {:?}", self.log.turns()));
        self.log.turns()
    }
}

/// `compact_self` is called from inside the caller's own turn, so it never
/// goes then: it waits for the turn to end, and then goes ahead of the turn
/// that was queued behind it. A Codex session compacts with no focus.
#[tokio::test]
async fn compact_self_waits_for_the_turn_and_goes_ahead_of_the_queued_one() {
    let agent = CompactingAgent::without_background_services();
    agent.pump();
    agent.becomes(AgentStatus::Working).await;

    let answered = agent.compact_self("keep the rail notes").unwrap();
    assert_eq!(
        answered["message"],
        "Build will compact your session when this turn ends."
    );
    drain(&agent.state);
    assert!(agent.log.turns().is_empty(), "nothing interrupts the turn");

    let turn = agent.turn(false);
    agent.state.lock().unwrap().delivery_queue.enqueue(turn);
    agent.becomes(AgentStatus::Waiting).await;

    assert_eq!(agent.heard(1).await, vec!["/compact".to_string()]);
    // The compaction is heard from inside the pump's drain, before that drain
    // puts the deferred turn back; wait for it to land rather than race it.
    wait_for(Duration::from_secs(2), || {
        (agent.still_queued() == 1).then_some(())
    })
    .await
    .expect("the turn waits for the compaction");
    assert_eq!(agent.log.turns(), vec!["/compact".to_string()]);

    agent.becomes(AgentStatus::Working).await;
    agent.becomes(AgentStatus::Waiting).await;
    assert_eq!(
        agent.heard(2).await,
        vec!["/compact".to_string(), "warm".to_string()]
    );
}

/// Queue a turn for the agent, interrupting or not.
fn queue_a_turn(agent: &CompactingAgent, interrupt: bool) {
    let turn = agent.turn(interrupt);
    agent.state.lock().unwrap().delivery_queue.enqueue(turn);
}

/// Any drain may run while the turn that called `compact_self` is still going
/// — the one after every tool call, the task delivery service's — and none of
/// them sends the turn queued behind it: that turn waits for the compaction.
#[tokio::test]
async fn a_drain_mid_turn_holds_the_turn_queued_behind_compact_self() {
    let agent = CompactingAgent::without_background_services();
    agent.pump();
    agent.becomes(AgentStatus::Working).await;
    agent.compact_self("keep the rail notes").unwrap();
    queue_a_turn(&agent, false);

    drain(&agent.state);

    assert_eq!(agent.still_queued(), 1, "the turn waits for the compaction");
    assert!(agent.log.turns().is_empty(), "{:?}", agent.log.turns());
}

/// The agent's report closes its thread's turn while the provider's turn is
/// still running — `compact_self`, then the closing `post_thread_message`, is
/// the usual order — and the drain after that tool call still holds the turn.
#[tokio::test]
async fn a_report_mid_turn_still_holds_the_turn_queued_behind_compact_self() {
    let agent = CompactingAgent::without_background_services();
    agent.pump();
    agent.becomes(AgentStatus::Working).await;
    agent.compact_self("keep the rail notes").unwrap();
    agent
        .state
        .lock()
        .unwrap()
        .agent_action(
            RUN,
            &agent.agent_id,
            BridgeAction::PostThreadMessage {
                still_working: false,
                body: "done".to_string(),
                options: Vec::new(),
                attachments: Vec::new(),
            },
        )
        .unwrap();
    queue_a_turn(&agent, false);

    drain(&agent.state);

    assert_eq!(agent.still_queued(), 1, "the turn waits for the compaction");
    assert!(agent.log.turns().is_empty(), "{:?}", agent.log.turns());
}

/// A session that ends with no last snapshot takes the compaction waiting
/// for its turn with it, so nothing holds a turn for an agent that is gone.
#[tokio::test]
async fn a_session_ending_mid_turn_withdraws_the_compaction_waiting_for_it() {
    let agent = CompactingAgent::without_background_services();
    agent.pump();
    agent.becomes(AgentStatus::Working).await;
    agent.compact_self("keep the rail notes").unwrap();
    let key = TabKey::agent(&AppState::canonical_root(&agent.root), &agent.agent_id);
    let mut app = agent.state.lock().unwrap();
    let instance = app
        .session_registry
        .test_tab(&key)
        .unwrap()
        .session_instance
        .clone()
        .unwrap();

    app.record_agent_session_end(RUN, &agent.agent_id, &instance);

    assert!(app.compactions.waiting_for_turn().is_empty());
    assert!(app.compactions.withdraw(RUN, &agent.agent_id).is_none());
}

/// The held turn goes once the compaction it waited for has: the turn ends,
/// the compaction goes ahead of it, and when that is done the turn follows.
#[tokio::test]
async fn the_held_turn_goes_once_the_compaction_is_done() {
    let agent = CompactingAgent::without_background_services();
    agent.pump();
    agent.becomes(AgentStatus::Working).await;
    agent.compact_self("keep the rail notes").unwrap();
    queue_a_turn(&agent, false);
    drain(&agent.state);

    agent.becomes(AgentStatus::Waiting).await;
    assert_eq!(agent.heard(1).await, vec!["/compact".to_string()]);
    wait_for(Duration::from_secs(2), || {
        (agent.still_queued() == 1).then_some(())
    })
    .await
    .expect("the turn waits for the compaction");

    agent.becomes(AgentStatus::Working).await;
    agent.becomes(AgentStatus::Waiting).await;
    assert_eq!(
        agent.heard(2).await,
        vec!["/compact".to_string(), "warm".to_string()]
    );
    assert_eq!(agent.still_queued(), 0);
}

/// A session that reports no turn ends has no status pump to release a held
/// turn, so nothing is held for it: the turn queued behind `compact_self` goes
/// on the next drain, as it did before the hold.
#[tokio::test]
async fn a_session_without_turn_ends_does_not_hold_the_turn_behind_compact_self() {
    let agent = CompactingAgent::without_turn_boundaries();
    agent.state.lock().unwrap().record_agent_working_since(
        RUN,
        &agent.agent_id,
        Some("2026-10-31T21:30:00Z".to_string()),
    );
    agent.compact_self("keep the rail notes").unwrap();
    queue_a_turn(&agent, false);

    drain(&agent.state);

    assert_eq!(agent.heard(1).await, vec!["warm".to_string()]);
    assert_eq!(agent.still_queued(), 0);
}

/// With no compaction waiting, a turn queued mid-turn goes on the next drain,
/// as it always has: only a pending compaction holds it.
#[tokio::test]
async fn with_no_compaction_waiting_a_drain_mid_turn_sends_the_queued_turn() {
    let agent = CompactingAgent::without_background_services();
    agent.pump();
    agent.becomes(AgentStatus::Working).await;
    queue_a_turn(&agent, false);

    drain(&agent.state);

    assert_eq!(agent.heard(1).await, vec!["warm".to_string()]);
    assert_eq!(agent.still_queued(), 0);
}

/// An interrupting turn is not held behind a waiting compaction: it goes on
/// the next drain, as it always has.
#[tokio::test]
async fn an_interrupting_turn_is_not_held_behind_compact_self() {
    let agent = CompactingAgent::without_background_services();
    agent.pump();
    agent.becomes(AgentStatus::Working).await;
    agent.compact_self("keep the rail notes").unwrap();
    queue_a_turn(&agent, true);

    drain(&agent.state);

    assert_eq!(agent.heard(1).await, vec!["warm".to_string()]);
    assert_eq!(agent.still_queued(), 0);
}

/// With nothing queued behind it, the pump sends it itself when the turn ends,
/// with the focus a Claude session takes.
#[tokio::test]
async fn compact_self_with_nothing_queued_goes_when_the_turn_ends() {
    let agent = CompactingAgent::new().on(AgentProvider::Claude);
    agent.pump();
    agent.becomes(AgentStatus::Working).await;
    agent.compact_self("keep the rail notes").unwrap();

    agent.becomes(AgentStatus::Waiting).await;

    assert_eq!(
        agent.heard(1).await,
        vec!["/compact keep the rail notes".to_string()]
    );
}

/// A newer request replaces the instructions of the one still waiting.
#[tokio::test]
async fn a_newer_request_replaces_the_waiting_one() {
    let agent = CompactingAgent::new().on(AgentProvider::Claude);
    agent.pump();
    agent.becomes(AgentStatus::Working).await;
    agent.compact_self("the old focus").unwrap();
    agent.compact_self("the new focus").unwrap();

    agent.becomes(AgentStatus::Waiting).await;

    assert_eq!(
        agent.heard(1).await,
        vec!["/compact the new focus".to_string()]
    );
}

#[test]
fn compact_self_refuses_a_harness_that_cannot_compact() {
    let agent = CompactingAgent::new()
        .on(AgentProvider::Pi)
        .with_captured_policy(None);

    let refused = agent.compact_self("anything").unwrap_err();

    assert_eq!(refused, "Build cannot compact your session: its running session has no qualified compaction command.");
    assert!(agent
        .state
        .lock()
        .unwrap()
        .compactions
        .withdraw(RUN, &agent.agent_id)
        .is_none());
    assert!(agent.log.turns().is_empty());
}

#[test]
fn compact_self_uses_the_captured_capability_after_a_model_choice_edit() {
    let agent = CompactingAgent::new()
        .on(AgentProvider::Pi)
        .with_captured_policy(None)
        .on(AgentProvider::CodexAppServer);
    assert!(agent.compact_self("anything").is_err());
    assert!(agent
        .state
        .lock()
        .unwrap()
        .compactions
        .withdraw(RUN, &agent.agent_id)
        .is_none());
}

#[test]
fn compact_self_accepts_a_qualified_pi_command() {
    let agent = CompactingAgent::new()
        .on(AgentProvider::Pi)
        .with_captured_policy(Some("0.86.1"));
    assert!(agent.compact_self("keep the notes").is_ok());
    assert_eq!(
        agent
            .state
            .lock()
            .unwrap()
            .compactions
            .withdraw(RUN, &agent.agent_id)
            .as_deref(),
        Some("keep the notes")
    );
}

/// An automatic compaction's row carries no focus, and the context either
/// side of it.
#[test]
fn an_automatic_compaction_row_carries_the_context_either_side_of_it() {
    let agent = CompactingAgent::new().with_context(200_000);
    agent.say(false);
    assert_eq!(agent.log.turns(), vec!["/compact".to_string()]);

    the_harness_compacts(&agent.state, RUN, &agent.agent_id, 12_000);

    assert_eq!(
        compaction_row(&agent.state, RUN, &agent.agent_id)["compaction"],
        json!({ "context_before": 200_000, "context_after": 12_000 })
    );
}

/// A project agent, and a Claude agent on one of its workspaces with a live
/// session called "Rail scroll".
struct ProjectWithWorker {
    _home: tempfile::TempDir,
    _dir: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    project_owner: String,
    project_agent: String,
    owner: String,
    worker: String,
    log: SessionLog,
}

impl ProjectWithWorker {
    fn with_captured_policy(self, version: Option<&str>) -> Self {
        capture_policy(
            &mut self.state.lock().unwrap(),
            self._dir.path(),
            &self.owner,
            &self.worker,
            version,
        );
        self
    }

    fn new(provider: AgentProvider) -> ProjectWithWorker {
        let (home, repo) = init_repo();
        let repo = std::fs::canonicalize(&repo).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let state_root = std::fs::canonicalize(dir.path()).unwrap();
        let mut state = rooted(&state_root);
        let project_id = added_project(&mut state, &repo);
        let (project_owner, project_agent) = project_agent(&mut state, &project_id);
        let workspace_id = workspace(&mut state, &project_id, "rail");
        let ensured = state.handle(req(
            "workspace.ensure_conversation",
            json!({ "workspace_id": workspace_id }),
        ));
        let owner = ensured["result"]["run_id"].as_str().unwrap().to_string();
        let added = state.handle(req("agent.add", json!({ "entity_id": owner })));
        let worker = added["result"]["agent"]["id"].as_str().unwrap().to_string();
        state
            .set_agent_name(&owner, &worker, "Rail scroll")
            .unwrap();
        state
            .set_agent_model_choice(
                &owner,
                &worker,
                ModelChoice {
                    provider,
                    ..ModelChoice::default()
                },
            )
            .unwrap();
        ProjectWithWorker {
            _home: home,
            _dir: dir,
            state: state.shared(),
            project_owner,
            project_agent,
            owner,
            worker,
            log: SessionLog::default(),
        }
    }

    fn with_session(self, status: AgentStatus) -> ProjectWithWorker {
        {
            let mut app = self.state.lock().unwrap();
            // A root that exists: a tab whose worktree is gone is reaped.
            let root = self._dir.path().to_path_buf();
            insert_agent_tab(
                &mut app,
                &root,
                &self.owner,
                &self.worker,
                DictatedSession::reporting(status).recording_into(&self.log),
            );
        }
        self
    }

    fn with_context(self, context_tokens: u64) -> ProjectWithWorker {
        self.state.lock().unwrap().record_agent_turn_context(
            &self.owner,
            &self.worker,
            context(context_tokens),
        );
        self
    }

    /// `compact_agent` as the socket runs it: the tool, then the drain.
    fn compact(&self, instructions: &str) -> Result<Value, String> {
        let answered = self.state.lock().unwrap().agent_action(
            &self.project_owner,
            &self.project_agent,
            BridgeAction::CompactAgent {
                agent_id: self.worker.clone(),
                instructions: instructions.to_string(),
            },
        );
        drain(&self.state);
        answered
    }
}

#[test]
fn compact_agent_between_turns_compacts_at_once_with_its_focus() {
    let project = ProjectWithWorker::new(AgentProvider::Claude)
        .with_session(AgentStatus::Waiting)
        .with_context(150_000);

    let answered = project.compact("keep the API notes").unwrap();

    assert_eq!(answered["message"], "Build is compacting Rail scroll now.");
    assert_eq!(
        project.log.turns(),
        vec!["/compact keep the API notes".to_string()]
    );

    the_harness_compacts(&project.state, &project.owner, &project.worker, 9_000);
    assert_eq!(
        compaction_row(&project.state, &project.owner, &project.worker)["compaction"],
        json!({
            "instructions": "keep the API notes",
            "context_before": 150_000,
            "context_after": 9_000,
        })
    );
}

#[test]
fn compact_agent_mid_turn_waits_and_says_so() {
    let project = ProjectWithWorker::new(AgentProvider::Claude).with_session(AgentStatus::Working);

    let answered = project.compact("keep the API notes").unwrap();

    assert_eq!(
        answered["message"],
        "Build will compact Rail scroll when its current turn ends."
    );
    assert!(
        project.log.turns().is_empty(),
        "the turn is not interrupted"
    );
}

#[test]
fn compact_agent_refuses_an_agent_with_no_running_session() {
    let project = ProjectWithWorker::new(AgentProvider::Claude);

    let refused = project.compact("keep the API notes").unwrap_err();

    assert_eq!(
        refused,
        "Build cannot compact Rail scroll: it has no running session."
    );
}

#[test]
fn compact_agent_refuses_a_harness_that_cannot_compact() {
    let project = ProjectWithWorker::new(AgentProvider::Pi)
        .with_session(AgentStatus::Waiting)
        .with_captured_policy(None);

    let refused = project
        .state
        .lock()
        .unwrap()
        .agent_action(
            &project.project_owner,
            &project.project_agent,
            BridgeAction::CompactAgent {
                agent_id: project.worker.clone(),
                instructions: "keep the API notes".into(),
            },
        )
        .unwrap_err();

    assert_eq!(
        refused,
        "Build cannot compact Rail scroll: its running session has no qualified compaction command."
    );
    assert!(project
        .state
        .lock()
        .unwrap()
        .compactions
        .withdraw(&project.owner, &project.worker)
        .is_none());
    assert!(project.log.turns().is_empty());
}

/// On a Codex agent the focus is dropped, and the row says none was kept.
#[test]
fn compact_agent_on_codex_sends_a_bare_compact() {
    let project =
        ProjectWithWorker::new(AgentProvider::CodexAppServer).with_session(AgentStatus::Waiting);

    project.compact("keep the API notes").unwrap();

    assert_eq!(project.log.turns(), vec!["/compact".to_string()]);
}
