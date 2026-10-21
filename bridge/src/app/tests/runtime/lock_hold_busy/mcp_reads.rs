//! Focused #741 comparisons on main's unchanged realistic busy-state seed.
use super::*;

mod cohort;
mod fixture;
use fixture::Fixture;

const TOOLS: [&str; 6] = [
    "list_task_dependents",
    "search_conversation",
    "read_review",
    "read_comment",
    "list_work",
    "list_workspaces",
];

#[derive(Clone, Copy)]
enum Load {
    Readers,
    Busy,
    BusyWithTaskLists,
}

impl Load {
    fn busy(self) -> bool {
        !matches!(self, Self::Readers)
    }

    fn task_lists(self) -> bool {
        matches!(self, Self::BusyWithTaskLists)
    }

    fn name(self) -> &'static str {
        match self {
            Self::Readers => "six_reads_only",
            Self::Busy => "busy_without_list_tasks",
            Self::BusyWithTaskLists => "busy_with_list_tasks",
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#741 release timing comparison: realistic busy seed, six read streams only"]
async fn six_mcp_reads_without_writers() {
    cohort::measure(Load::Readers).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#741 release timing regression: busy writers, status and RPC, no list_tasks"]
async fn six_mcp_reads_with_busy_writers() {
    cohort::measure(Load::Busy).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "#741 release timing regression: same busy cohort plus heavy list_tasks"]
async fn six_mcp_reads_with_busy_writers_and_task_lists() {
    cohort::measure(Load::BusyWithTaskLists).await;
}

fn assert_measurements(measurements: &Value, lists: bool) {
    for tool in TOOLS.into_iter().chain(lists.then_some("list_tasks")) {
        let row = &measurements[format!("mcp.control:{tool}")];
        for phase in [
            "client.total",
            "frame.total",
            "frame.pool_wait",
            "read.admission.total",
            "read.query.total",
            "read.run.total",
            "read.finish.total",
            "reply.serialization",
        ] {
            assert_eq!(row[phase]["count"], AGENTS * ROUNDS, "{tool} {phase}");
        }
        if !["list_work", "list_workspaces"].contains(&tool) {
            assert_eq!(
                row["read.class_wait"]["count"],
                AGENTS * ROUNDS,
                "{tool} per-type admission"
            );
            assert_eq!(
                row["read.sql_wait"]["count"],
                AGENTS * ROUNDS,
                "{tool} SQL acquisition"
            );
        }
        for phase in ["frame.held", "frame.lock_wait", "frame.permit_wait"] {
            assert_eq!(row[phase]["count"], AGENTS * ROUNDS, "{tool} {phase}");
            assert_eq!(row[phase]["max_ms"], 0.0, "{tool} bypasses {phase}");
        }
    }
    for tool in TOOLS {
        let latency = &measurements[format!("mcp.control:{tool}")]["client.total"];
        assert!(
            latency["max_ms"].as_f64().unwrap() <= MAX_BUSY_MCP_READ_RESPONSE_MS,
            "{tool} exceeds {MAX_BUSY_MCP_READ_RESPONSE_MS}ms: {latency}"
        );
    }
}

async fn reply(client: &mut tokio::io::BufReader<tokio::net::UnixStream>, frame: Value) -> Value {
    client
        .get_mut()
        .write_all(format!("{frame}\n").as_bytes())
        .await
        .unwrap();
    let mut response = String::new();
    let bytes = tokio::time::timeout(REQUEST_PATIENCE, client.read_line(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(bytes > 0, "real socket reply must complete");
    serde_json::from_str(&response).unwrap()
}

#[test]
fn focused_read_recipes_use_the_exact_independent_lane() {
    let fixture = Fixture::example();
    let cases = fixture.cases();
    assert_eq!(
        cases
            .iter()
            .map(BridgeAction::tool_name)
            .collect::<Vec<_>>(),
        TOOLS
    );
    for action in cases.into_iter().chain([fixture.task_list()]) {
        let frame = fixture.frame("coding-agent", &action);
        assert!(crate::app::mcp::is_independent_read(&frame));
        let mut reported = frame;
        reported["report"] = json!({"status":"blocked","summary":"report precedence"});
        assert!(!crate::app::mcp::is_independent_read(&reported));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn focused_read_recipes_return_saved_witnesses() {
    let root = tempfile::tempdir().unwrap();
    let (_home, mut app, project) = tracked(root.path());
    let task = filed(&mut app, &project, "focused read task")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let dependent = filed(&mut app, &project, "focused dependent")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let workspace =
        crate::app::tests::project_agent::workspace(&mut app, &project, "focused-read-recipes");
    let selected = app.handle(req(
        "workspace.ensure_conversation",
        json!({"workspace_id":workspace}),
    ));
    assert_eq!(selected["ok"], true, "{selected}");
    let owner = selected["result"]["entity_id"].as_str().unwrap().to_owned();
    let added = app.handle(req("agent.add", json!({"entity_id":owner})));
    assert_eq!(added["ok"], true, "{added}");
    let agent = added["result"]["agent"]["id"].as_str().unwrap().to_owned();
    assert_eq!(primary_agent_id(&app, &owner), agent);
    let mut active = app.take_run(&owner).unwrap();
    seed_history(&mut active.agents.by_id_mut(&agent).unwrap().thread);
    app.finish_run_mutation(owner, active).unwrap();
    let fixture = Fixture::seed(&mut app, &workspace, &task, &dependent);
    app.test_install_mcp_token(agent.clone(), format!("busy-token-{agent}"));
    app.delivery_queue.clear_queued();
    app.initialize_read_context();
    app.initialize_snapshot_projection();
    let state = Arc::new(Mutex::new(app));
    let plane = ControlPlane::new(&state.lock().unwrap());
    let (client, daemon) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(handle_done_stream(state, daemon, plane));
    let mut client = tokio::io::BufReader::new(client);
    for action in fixture.cases().into_iter().chain([fixture.task_list()]) {
        let response = reply(&mut client, fixture.frame(&agent, &action)).await;
        fixture.assert_reply(&action, &response);
    }
    drop(client);
    server.await.unwrap();
}
