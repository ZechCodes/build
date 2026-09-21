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
    let agent = CompactingAgent::new();
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
    assert_eq!(agent.still_queued(), 1, "the turn waits for the compaction");

    agent.becomes(AgentStatus::Working).await;
    agent.becomes(AgentStatus::Waiting).await;
    assert_eq!(
        agent.heard(2).await,
        vec!["/compact".to_string(), "warm".to_string()]
    );
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
    let agent = CompactingAgent::new().on(AgentProvider::Pi);

    let refused = agent.compact_self("anything").unwrap_err();

    assert!(
        refused.starts_with("Build cannot compact your session:"),
        "{refused}"
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
    let project = ProjectWithWorker::new(AgentProvider::Pi).with_session(AgentStatus::Waiting);

    let refused = project.compact("keep the API notes").unwrap_err();

    assert_eq!(
        refused,
        "Build cannot compact Rail scroll: its harness cannot compact."
    );
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
