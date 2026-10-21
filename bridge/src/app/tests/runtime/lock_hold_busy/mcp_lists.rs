//! Focused task/work list latency through real local MCP socket pairs.
use super::*;

const LIST_ROUNDS: usize = 3;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "release-only 700-task/200-conversation busy MCP list latency regression"]
async fn busy_mcp_lists_finish_within_100ms() {
    let busy = Busy::with_task_body(&"Stored task context and acceptance evidence. ".repeat(48));
    let router = {
        let mut app = busy.state.lock().unwrap();
        let (_, router) =
            crate::app::tests::routing::router::captured(&mut app, "list work observer");
        app.test_install_mcp_token(router.clone(), format!("busy-token-{router}"));
        app.delivery_queue.clear_queued();
        router
    };
    let plane = ControlPlane::new(&busy.state.lock().unwrap());
    busy.samples.clear();
    let start = Arc::new(tokio::sync::Barrier::new(AGENTS * 3 + 1));
    let mut calls = status_observers(&busy, &start);
    let completed = Arc::new(AtomicUsize::new(0));
    let progress = Arc::new(ListProgress {
        writers: Arc::clone(&completed),
        reads: Mutex::new(std::collections::BTreeMap::new()),
    });
    for (i, (owner, agent)) in busy.owners.iter().take(AGENTS).enumerate() {
        calls.push(tokio::spawn(control_agent(
            Arc::clone(&busy.state),
            Arc::clone(&plane),
            ControlIdentity {
                owner: owner.clone(),
                agent: agent.clone(),
                task: busy.tasks[i].clone(),
                recipient: busy.owners[(i + 1) % AGENTS].1.clone(),
                view: busy.view.clone(),
            },
            Arc::clone(&start),
            Arc::clone(&completed),
            busy.samples.clone(),
        )));
        calls.push(tokio::spawn(reader(
            Arc::clone(&busy.state),
            Arc::clone(&plane),
            Arc::clone(&start),
            agent.clone(),
            router.clone(),
            busy.samples.clone(),
            Arc::clone(&progress),
        )));
    }
    start.wait().await;
    let mut failures = Vec::new();
    for call in calls {
        if let Err(error) = call.await {
            failures.push(error.to_string());
        }
    }
    let measurements = wait_for_fixture_idle(&busy).await;
    println!(
        "TASK738_MCP_LISTS {}",
        json!({
            "tasks":TASKS,"conversations":CONVERSATIONS,"worktrees":WORKTREES,
            "readers":AGENTS,"rounds":LIST_ROUNDS,"body_bytes_per_task":48*44,
            "measurements":measurements,"failures":failures,
        "completed_writer_calls":completed.load(Ordering::Relaxed),
        "read_progress":*progress.reads.lock().unwrap()
        })
    );
    assert!(failures.is_empty(), "{failures:?}");
    assert_eq!(
        completed.load(Ordering::Relaxed),
        AGENTS * ROUNDS * CONTROL_TOOLS.len()
    );
    assert_eq!(
        measurements["agent.poll_status"]["client.total"]["count"],
        AGENTS * STATUS_OBSERVATIONS_PER_AGENT
    );
    for counts in progress.reads.lock().unwrap().values() {
        assert!(
            counts.0 > 0,
            "each list must actually send while writers remain outstanding"
        );
        assert!(
            counts.1 > 0,
            "actual serialized reply byte size is measured"
        );
    }
    for tool in ["list_tasks", "list_work"] {
        let row = &measurements[format!("mcp.control:{tool}")]["client.total"];
        assert_eq!(row["count"], AGENTS * LIST_ROUNDS, "{row}");
        assert!(row["max_ms"].as_f64().unwrap() <= 100.0, "{tool}: {row}");
        assert!(row["p99_ms"].as_f64().unwrap() <= 100.0, "{tool}: {row}");
    }
}

struct ListProgress {
    writers: Arc<AtomicUsize>,
    // Per tool: sends with outstanding original writer calls, maximum reply bytes.
    reads: Mutex<std::collections::BTreeMap<String, (usize, usize)>>,
}

async fn reader(
    state: Arc<Mutex<AppState>>,
    plane: Arc<ControlPlane>,
    start: Arc<tokio::sync::Barrier>,
    agent: String,
    router: String,
    samples: PhaseSamples,
    progress: Arc<ListProgress>,
) {
    let (client, daemon) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(handle_done_stream(state, daemon, plane));
    let mut client = tokio::io::BufReader::new(client);
    start.wait().await;
    for _ in 0..LIST_ROUNDS {
        for (endpoint, action) in [
            (
                &agent,
                BridgeAction::TrackerListTasks {
                    state: None,
                    status: None,
                    label: None,
                },
            ),
            (&router, BridgeAction::ListWork),
        ] {
            let frame = json!({"task_id":endpoint,"session_token":format!("busy-token-{endpoint}"),"request":action});
            let requested = Instant::now();
            let overlapping =
                progress.writers.load(Ordering::Relaxed) < AGENTS * ROUNDS * CONTROL_TOOLS.len();
            progress
                .reads
                .lock()
                .unwrap()
                .entry(action.tool_name().into())
                .or_default()
                .0 += usize::from(overlapping);
            client
                .get_mut()
                .write_all(format!("{frame}\n").as_bytes())
                .await
                .unwrap();
            let mut line = String::new();
            tokio::time::timeout(REQUEST_PATIENCE, client.read_line(&mut line))
                .await
                .unwrap()
                .unwrap();
            let reply: Value = serde_json::from_str(&line).unwrap();
            let mut counts = progress.reads.lock().unwrap();
            let count = counts.entry(action.tool_name().into()).or_default();
            count.1 = count.1.max(line.len());
            drop(counts);
            samples.record_client_total(
                &format!("mcp.control:{}", action.tool_name()),
                requested.elapsed(),
            );
            assert_eq!(reply["ok"], true, "{reply}");
            let rows = if action.tool_name() == "list_tasks" {
                "tasks"
            } else {
                "work"
            };
            assert!(reply["result"][rows].as_array().unwrap().len() >= TASKS);
        }
    }
    drop(client);
    server.await.unwrap();
}

#[test]
fn task_list_summary_queries_are_bounded_by_batches() {
    let root = tempfile::tempdir().unwrap();
    let (_home, mut app, project) = tracked(root.path());
    for i in 0..24 {
        filed(&mut app, &project, &format!("summary query witness {i}"));
    }
    let clock = crate::timing::FrameClock::reporting_to(Arc::new(|_| {}));
    let samples = clock.capture_phases();
    let timer = clock.frame("mcp.control");
    timer.sample_tool("list_tasks");
    let answer = timer
        .sample_read_work(|| {
            app.task_read_context()
                .tasks_list(app.store.as_ref().unwrap(), &json!({"project_id":project}))
        })
        .unwrap();
    assert_eq!(answer["tasks"].as_array().unwrap().len(), 24);
    let captured = samples.snapshot();
    let queries: u64 = captured["mcp.control:list_tasks"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(phase, _)| phase.starts_with("entered.sql."))
        .map(|(_, metric)| metric["count"].as_u64().unwrap())
        .sum();
    assert!(
        queries <= 16,
        "24 task summaries must use bounded batch queries, observed {queries}: {captured}"
    );
}
