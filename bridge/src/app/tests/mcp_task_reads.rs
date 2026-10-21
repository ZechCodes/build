//! Independent MCP task reads must not queue behind mutation turns.
use super::*;
use crate::app::mcp::{handle_done_stream, ControlPlane, Turn as ControlTurn, MCP_CONTROL_METHOD};
use crate::mcp::BridgeAction;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

const TOKEN: &str = "task-read-lane-test-capability";
const WATCHDOG: Duration = Duration::from_secs(5);
// ReadPool keeps eight fixed connections; capacity is intentionally private.
const SQL_READERS: usize = 8;

mod cancellation;
mod classification;
mod cold_catalogue;
mod guardfree;
mod history_reviews;
mod immediate_authority;
mod metadata;
mod races;
mod reset_retirement;
mod roster_observations;
mod snapshot_reads;
mod write_wait;

struct Fixture {
    _root: tempfile::TempDir,
    _repo: tempfile::TempDir,
    state: Arc<Mutex<AppState>>,
    plane: Arc<ControlPlane>,
    project: String,
    owner: String,
    agent: String,
    task: String,
    workspace: String,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let (repo, mut app, project) = super::tracker::tracked(root.path());
        let (owner, agent) = super::project_agent::project_agent(&mut app, &project);
        let workspace = super::project_agent::workspace(&mut app, &project, "read-lane");
        let task = super::tracker::filed(&mut app, &project, &format!("Read @agent:{agent}"));
        app.delivery_queue.clear_queued();
        app.test_install_mcp_token(agent.clone(), TOKEN.into());
        app.initialize_read_context();
        app.initialize_snapshot_projection();
        // Exercise the real initialized producers and control socket without
        // app.shared()'s background delivery services. Those services would
        // independently consume the no-drain sentinel and retire fake routers.
        let state = Arc::new(Mutex::new(app));
        let plane = ControlPlane::new(&state.lock().unwrap());
        Self {
            _root: root,
            _repo: repo,
            state,
            plane,
            project,
            owner,
            agent,
            task: task["id"].as_str().unwrap().into(),
            workspace,
        }
    }

    fn frame(&self, action: BridgeAction) -> Value {
        json!({"task_id":self.agent,"session_token":TOKEN,"request":action})
    }

    fn get(&self) -> BridgeAction {
        BridgeAction::TrackerGetTask {
            task_id: self.task.clone(),
        }
    }

    fn legacy(&self, action: BridgeAction) -> Value {
        let answer =
            self.state
                .lock()
                .unwrap()
                .on_agent_mcp_action(&self.owner, &self.agent, action);
        crate::app::mcp::mcp_action_response(answer)
    }

    async fn held_turns(&self) -> Vec<ControlTurn> {
        let clock = Arc::clone(&self.state.lock().unwrap().frame_clock);
        let mut held = Vec::new();
        for _ in 0..crate::app::mcp::CONTROL_FRAMES_AT_THE_LOCK {
            let timer = clock.frame(MCP_CONTROL_METHOD);
            held.push(ControlTurn::take(&self.plane, &timer).await);
        }
        held
    }

    async fn reply(&self, frame: Value) -> Value {
        socket_reply(Arc::clone(&self.state), Arc::clone(&self.plane), frame).await
    }
}

async fn socket_reply(
    state: Arc<Mutex<AppState>>,
    plane: Arc<ControlPlane>,
    frame: Value,
) -> Value {
    let (client, daemon) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(handle_done_stream(state, daemon, plane));
    let mut client = tokio::io::BufReader::new(client);
    client
        .get_mut()
        .write_all(format!("{frame}\n").as_bytes())
        .await
        .unwrap();
    let mut response = String::new();
    tokio::time::timeout(WATCHDOG, client.read_line(&mut response))
        .await
        .expect("MCP read queued behind held mutation turns")
        .unwrap();
    assert!(
        !response.is_empty(),
        "the control socket closed without a reply"
    );
    drop(client);
    server.await.unwrap();
    serde_json::from_str(&response).unwrap()
}

fn without_now(mut value: Value) -> Value {
    // The list formatter intentionally samples the current user-session clock.
    if let Some(session) = value["result"]["user_session"].as_object_mut() {
        session.remove("now_ms");
    }
    value
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_sql_reads_answer_while_both_writer_turns_are_held() {
    let fixture = Fixture::new();
    let actions = [
        fixture.get(),
        BridgeAction::TrackerListTasks {
            state: None,
            status: None,
            label: None,
        },
        BridgeAction::TrackerTaskDependents {
            task_id: fixture.task.clone(),
            limit: Some(1),
            cursor: None,
        },
    ];
    let expected = actions
        .iter()
        .cloned()
        .map(|action| fixture.legacy(action))
        .collect::<Vec<_>>();
    let before = fixture.state.lock().unwrap().read_view.load();
    let held = fixture.held_turns().await;
    let mutation = tokio::spawn(socket_reply(
        Arc::clone(&fixture.state),
        Arc::clone(&fixture.plane),
        fixture.frame(BridgeAction::SetTopic {
            topic: "writer remains queued".into(),
        }),
    ));
    for (action, expected) in actions.into_iter().zip(expected) {
        let tool = action.tool_name();
        let answer = fixture.reply(fixture.frame(action.clone())).await;
        assert_eq!(without_now(answer), without_now(expected), "{tool}");
        assert!(
            !mutation.is_finished(),
            "a writer consumed a turn held by this test"
        );
        assert!(
            Arc::ptr_eq(&before, &fixture.state.lock().unwrap().read_view.load()),
            "{tool} published state or drained unrelated delivery"
        );
    }
    assert_eq!(
        held.len(),
        2,
        "both turns remain held through every read reply"
    );
    drop(held);
    assert_eq!(mutation.await.unwrap()["ok"], true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_immutable_inventory_reads_answer_while_both_writer_turns_are_held() {
    let fixture = Fixture::new();
    let actions = [
        BridgeAction::ListHarnesses,
        BridgeAction::ListWorkspaceAgents {
            workspace_id: fixture.workspace.clone(),
        },
        BridgeAction::ListProjectAgents,
    ];
    let expected = actions
        .iter()
        .cloned()
        .map(|action| fixture.legacy(action))
        .collect::<Vec<_>>();
    let before = fixture.state.lock().unwrap().read_view.load();
    let held = fixture.held_turns().await;
    for (action, expected) in actions.into_iter().zip(expected) {
        let tool = action.tool_name();
        assert_eq!(
            fixture.reply(fixture.frame(action.clone())).await,
            expected,
            "{tool}"
        );
        assert!(
            Arc::ptr_eq(&before, &fixture.state.lock().unwrap().read_view.load()),
            "{tool}"
        );
    }
    assert_eq!(held.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_task_reads_authenticate_before_borrowing_sql_capacity() {
    let fixture = Fixture::new();
    let pool = fixture
        .state
        .lock()
        .unwrap()
        .store
        .as_ref()
        .unwrap()
        .read_pool();
    let mut connections = Vec::new();
    for _ in 0..SQL_READERS {
        connections.push(pool.acquire().await);
    }
    let held = fixture.held_turns().await;
    for token in [Value::Null, json!("wrong-capability")] {
        let mut frame = fixture.frame(fixture.get());
        frame["session_token"] = token;
        assert_eq!(
            fixture.reply(frame).await,
            json!({"ok":false,"error":"unauthorized MCP session"})
        );
    }
    fixture
        .state
        .lock()
        .unwrap()
        .test_install_mcp_token(fixture.agent.clone(), "rotated-capability".into());
    assert_eq!(
        fixture.reply(fixture.frame(fixture.get())).await,
        json!({"ok":false,"error":"unauthorized MCP session"})
    );
    assert_eq!(held.len(), 2);
    assert_eq!(connections.len(), SQL_READERS);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_task_read_scope_and_store_refusals_match_existing_tools() {
    let mut fixture = Fixture::new();
    let (_other_home, other_repo) = init_repo();
    let foreign = {
        let mut app = fixture.state.lock().unwrap();
        let other = super::project_agent::added_project(&mut app, &other_repo);
        super::tracker::filed(&mut app, &other, "foreign private title")
    };
    for task_id in [foreign["id"].as_str().unwrap(), "nonexistent-task"] {
        let action = BridgeAction::TrackerGetTask {
            task_id: task_id.into(),
        };
        let expected = fixture.legacy(action.clone());
        assert_eq!(
            expected,
            json!({"ok":false,"error":format!("unknown task_id: {task_id}")})
        );
        let mut frame = fixture.frame(action);
        frame["request"]["project_id"] = foreign["project_id"].clone();
        assert_eq!(fixture.reply(frame).await, expected);
    }
    fixture.state.lock().unwrap().store = None;
    guardfree::held::recapture_plane(&mut fixture);
    let action = fixture.get();
    let expected = fixture.legacy(action.clone());
    assert_eq!(
        expected,
        json!({"ok":false,"error":"tasks need a durable store; this bridge has none"})
    );
    assert_eq!(fixture.reply(fixture.frame(action)).await, expected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_task_read_preserves_committed_identity_after_failed_roster_save() {
    let fixture = Fixture::new();
    let expected = fixture.legacy(fixture.get());
    {
        let mut app = fixture.state.lock().unwrap();
        let mut candidate = app.take_run(&fixture.owner).unwrap();
        candidate.agents.by_id_mut(&fixture.agent).unwrap().name =
            Some("Rejected private name".into());
        app.store.as_ref().unwrap().fail_next_write();
        assert!(app.persist_run_record(&fixture.owner, &candidate).is_err());
        app.runs.insert(fixture.owner.clone(), candidate);
        app.publish_read_owner(&fixture.owner, false);
    }
    assert_eq!(fixture.reply(fixture.frame(fixture.get())).await, expected);
}
