//! One typed inventory drives scheduling, count and latency coverage checks.
use super::*;
use crate::mcp::McpSurface;

mod actor;
mod fixture;
mod progress;
pub(super) use actor::{run, Reader};
pub(super) use fixture::{seed_project_owner, Fixture};
pub(super) use progress::Progress;

pub(super) const TOOLS: [&str; 14] = [
    "get_task",
    "list_tasks",
    "list_task_dependents",
    "list_harnesses",
    "list_workspace_agents",
    "list_project_agents",
    "list_workspaces",
    "list_projects",
    "list_work",
    "search_conversation",
    "read_conversation",
    "read_comment",
    "get_review",
    "read_review",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Caller {
    Coding,
    Router,
}

pub(super) struct Ids {
    pub owner: String,
    pub agent: String,
    pub workspace: String,
    pub task: String,
    pub comment: String,
    pub snapshot: String,
    pub directory: String,
    pub path: String,
}

pub(super) fn cases(ids: &Ids) -> Vec<(Caller, BridgeAction)> {
    use Caller::{Coding, Router};
    vec![
        (
            Coding,
            BridgeAction::TrackerGetTask {
                task_id: ids.task.clone(),
            },
        ),
        (
            Coding,
            BridgeAction::TrackerListTasks {
                state: None,
                status: None,
                label: None,
            },
        ),
        (
            Coding,
            BridgeAction::TrackerTaskDependents {
                task_id: ids.task.clone(),
                limit: Some(1),
                cursor: None,
            },
        ),
        (Coding, BridgeAction::ListHarnesses),
        (
            Coding,
            BridgeAction::ListWorkspaceAgents {
                workspace_id: ids.workspace.clone(),
            },
        ),
        (Coding, BridgeAction::ListProjectAgents),
        (Coding, BridgeAction::ListWorkspaces),
        (Router, BridgeAction::ListProjects),
        (Router, BridgeAction::ListWork),
        (
            Coding,
            BridgeAction::SearchConversation {
                query: crate::thread::ConversationQuery {
                    text: Some("Busy request 0:".into()),
                    ..Default::default()
                },
            },
        ),
        (
            Router,
            BridgeAction::ReadConversation {
                entity_id: ids.owner.clone(),
                agent_id: Some(ids.agent.clone()),
                limit: 300,
            },
        ),
        (
            Coding,
            BridgeAction::TrackerReadComment {
                comment_id: ids.comment.clone(),
            },
        ),
        (
            Coding,
            BridgeAction::TrackerGetReview {
                task_id: ids.task.clone(),
            },
        ),
        (
            Coding,
            BridgeAction::TrackerReadReview {
                task_id: ids.task.clone(),
                snapshot_id: ids.snapshot.clone(),
                directory_id: ids.directory.clone(),
                mode: crate::reviews::read::ReviewReadMode::Blob,
                path: Some(ids.path.clone()),
                paths: Vec::new(),
                patch: false,
                range: Some(crate::body_page::FileRange {
                    offset: 0,
                    bytes: 4,
                    raw: None,
                }),
            },
        ),
    ]
}

fn fixture_ids() -> Ids {
    Ids {
        owner: "read-owner".into(),
        agent: "read-agent".into(),
        workspace: "read-workspace".into(),
        task: "read-task".into(),
        comment: "read-comment".into(),
        snapshot: "read-snapshot".into(),
        directory: "read-directory".into(),
        path: "README.md".into(),
    }
}

#[test]
fn typed_read_registry_matches_independent_scheduling_and_surfaces() {
    let ids = fixture_ids();
    let cases = cases(&ids);
    let tools = cases
        .iter()
        .map(|(_, action)| action.tool_name())
        .collect::<Vec<_>>();
    assert_eq!(tools, TOOLS);
    assert_eq!(
        tools
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        14
    );
    for (caller, action) in cases {
        let surface = match caller {
            Caller::Coding => McpSurface::Coding,
            Caller::Router => McpSurface::Router,
        };
        assert!(
            action.allowed_on(surface),
            "{} fixture surface",
            action.tool_name()
        );
        let frame = json!({"task_id":ids.agent,"session_token":"qualification-test-token","request":action});
        assert!(
            crate::app::mcp::is_independent_read(&frame),
            "{} missing classifier",
            action.tool_name()
        );
        let mut report = frame.clone();
        report["report"] = json!({"status":"blocked","summary":"report precedence"});
        assert!(!crate::app::mcp::is_independent_read(&report));
    }
    let wrapped = BridgeAction::TrackerWithWaitingOn {
        wrapped: Box::new(BridgeAction::TrackerGetTask { task_id: ids.task }),
        waiting_on: Vec::new(),
    };
    assert!(!crate::app::mcp::is_independent_read(
        &json!({"request":wrapped})
    ));
    assert!(!crate::app::mcp::is_independent_read(
        &json!({"request":{"action":"tracker_get_task","task_id":12}})
    ));
}

#[test]
fn busy_cohort_covers_all_fourteen_independent_mcp_reads() {
    let expected = expected_request_samples()
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>();
    let missing = TOOLS
        .into_iter()
        .filter(|tool| expected.get(&format!("mcp.control:{tool}")) != Some(&(AGENTS * ROUNDS)))
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "every read must have24actual callers in the busy cohort: {missing:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_busy_read_recipe_returns_its_real_saved_witness() {
    let root = tempfile::tempdir().unwrap();
    let (_home, mut app, project) = tracked(root.path());
    let task = filed(&mut app, &project, "busy read recipe task")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let dependent = filed(&mut app, &project, "busy read recipe dependent")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let workspace =
        crate::app::tests::project_agent::workspace(&mut app, &project, "busy-read-recipes");
    let selected = app.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id":workspace}),
    ));
    assert_eq!(selected["ok"], true, "{selected}");
    let owner = selected["result"]["entity_id"].as_str().unwrap().to_owned();
    let agent = primary_agent_id(&app, &owner);
    let mut active = app.take_run(&owner).unwrap();
    seed_history(&mut active.agents.by_id_mut(&agent).unwrap().thread);
    app.finish_run_mutation(owner.clone(), active).unwrap();
    let repo = app.project_at(0).repo_path.clone();
    let project_owner = adopted_run(&mut app, &repo, root.path(), "busy-read-project");
    let mut project_active = app.take_run(&project_owner).unwrap();
    seed_history(&mut project_active.agents.primary_mut().unwrap().thread);
    app.finish_run_mutation(project_owner.clone(), project_active)
        .unwrap();
    seed_project_owner(&mut app, &project_owner);
    let fixture = Fixture::seed(&mut app, &workspace, &agent, &task, &dependent);
    app.test_install_mcp_token(agent.clone(), format!("busy-token-{agent}"));
    app.delivery_queue.clear_queued();
    app.initialize_read_context();
    app.initialize_snapshot_projection();
    // No independent service may consume the intentionally passive router.
    let state = Arc::new(Mutex::new(app));
    let plane = ControlPlane::new(&state.lock().unwrap());
    let (client, daemon) = tokio::net::UnixStream::pair().unwrap();
    let mut server = tokio::spawn(handle_done_stream(state.clone(), daemon, plane));
    let mut client = tokio::io::BufReader::new(client);
    let mut replies = Vec::new();
    for (_, action) in cases(&fixture.ids(&owner, &agent)) {
        let reply = tokio::time::timeout(
            Duration::from_secs(5),
            control_reply(&mut client, fixture.frame(&agent, &action)),
        )
        .await;
        let complete = reply.is_ok();
        replies.push((action, reply));
        if !complete {
            break;
        }
    }
    drop(client);
    let joined = tokio::time::timeout(Duration::from_secs(5), &mut server).await;
    if joined.is_err() {
        server.abort();
        let _ = server.await;
    } else {
        joined.unwrap().unwrap();
    }
    assert_eq!(
        replies.len(),
        TOOLS.len(),
        "all real recipes must complete before inspecting their results"
    );
    for (action, reply) in replies {
        fixture.assert_reply(&action, &reply.expect("saved read recipe timed out"));
    }
    assert!(state
        .lock()
        .unwrap()
        .session_registry
        .token_matches(&fixture.router, &format!("busy-token-{}", fixture.router)));
}
