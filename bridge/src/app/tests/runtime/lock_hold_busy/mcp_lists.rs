//! Focused task/work list latency through real local MCP socket pairs.
use super::*;

const LIST_ROUNDS: usize = 3;

struct FilteredTaskCase {
    name: &'static str,
    arguments: Value,
    rows: usize,
}

fn filtered_task_cases() -> Vec<FilteredTaskCase> {
    vec![
        FilteredTaskCase {
            name: "state",
            arguments: json!({"state":"open"}),
            rows: 20,
        },
        FilteredTaskCase {
            name: "status",
            arguments: json!({"status":"ready"}),
            rows: 20,
        },
        FilteredTaskCase {
            name: "label_short",
            arguments: json!({"status":"ready","label":"busy-short"}),
            rows: 5,
        },
        FilteredTaskCase {
            name: "label_empty",
            arguments: json!({"status":"ready","label":"busy-missing"}),
            rows: 0,
        },
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "release-only 700-task/200-conversation busy MCP list latency regression"]
async fn busy_mcp_lists_finish_within_100ms() {
    let body = "Stored task context and acceptance evidence. ".repeat(48);
    let busy = Busy::with_task_body(&body);
    let router = prepare_list_fixture(&busy);
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
            "readers":AGENTS,"rounds":LIST_ROUNDS,"body_bytes_per_task":body.trim().len(),
            "measurements":measurements,"failures":failures,
        "completed_writer_calls":completed.load(Ordering::Relaxed),
        "read_progress":*progress.reads.lock().unwrap()
        })
    );
    print_list_summary(&measurements);
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
    for (tool, expected) in [
        (
            "list_tasks".to_string(),
            AGENTS * (LIST_ROUNDS + filtered_task_cases().len()),
        ),
        ("list_work".to_string(), AGENTS * LIST_ROUNDS),
        ("list_tasks.unfiltered".to_string(), AGENTS * LIST_ROUNDS),
    ]
    .into_iter()
    .chain(
        ["state", "status", "label_short", "label_empty"]
            .into_iter()
            .map(|case| (format!("list_tasks.{case}"), AGENTS)),
    ) {
        let row = &measurements[format!("mcp.control:{tool}")]["client.total"];
        assert_eq!(row["count"], expected, "{tool}: {row}");
        assert!(row["max_ms"].as_f64().unwrap() <= 100.0, "{tool}: {row}");
        assert!(row["p99_ms"].as_f64().unwrap() <= 100.0, "{tool}: {row}");
        assert!(row["p95_ms"].as_f64().unwrap() <= 100.0, "{tool}: {row}");
    }
}

fn prepare_list_fixture(busy: &Busy) -> String {
    let mut app = busy.state.lock().unwrap();
    // Writers create backlog tasks and move the oldest tasks. This distinct
    // forty-row SQL range keeps five matches in a twenty-candidate label page.
    let store = app.store.as_ref().unwrap();
    for (index, id) in busy.tasks.iter().rev().take(40).enumerate() {
        let mut task = store.load_tracker_task(id).unwrap().unwrap();
        task.status = "ready".into();
        if index < 5 {
            task.labels.push("busy-short".into());
        }
        store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    }
    let (_, router) = crate::app::tests::routing::router::captured(&mut app, "list work observer");
    app.test_install_mcp_token(router.clone(), format!("busy-token-{router}"));
    app.delivery_queue.clear_queued();
    app.publish_all_snapshot_projection();
    let board = app
        .snapshot_read("board.list", &json!({}))
        .unwrap()
        .unwrap();
    assert!(
        board["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["kind"] == "capture"),
        "capture exclusion must start with a capture in the accepted board"
    );
    router
}

fn print_list_summary(measurements: &Value) {
    for (method, phases) in measurements.as_object().unwrap() {
        if !method.starts_with("mcp.control:list_") {
            continue;
        }
        let row = &phases["client.total"];
        println!(
            "TASK738_MCP_LIST_SUMMARY {}",
            json!({
                "method":method,"count":row["count"],"p50_ms":row["p50_ms"],
                "p95_ms":row["p95_ms"],"p99_ms":row["p99_ms"],"max_ms":row["max_ms"],
            })
        );
    }
}

struct ListProgress {
    writers: Arc<AtomicUsize>,
    // Per tool: sends with outstanding original writer calls, maximum reply bytes.
    reads: Mutex<std::collections::BTreeMap<String, (usize, usize)>>,
}

struct ListReader {
    client: tokio::io::BufReader<tokio::net::UnixStream>,
    samples: PhaseSamples,
    progress: Arc<ListProgress>,
}

impl ListReader {
    async fn request(
        &mut self,
        endpoint: &str,
        action: &BridgeAction,
        case: Option<&str>,
    ) -> Value {
        let frame = json!({"task_id":endpoint,"session_token":format!("busy-token-{endpoint}"),"request":action});
        let requested = Instant::now();
        let overlapping =
            self.progress.writers.load(Ordering::Relaxed) < AGENTS * ROUNDS * CONTROL_TOOLS.len();
        let mut keys = vec![action.tool_name().to_string()];
        if let Some(case) = case {
            keys.push(format!("{}.{}", action.tool_name(), case));
        }
        {
            let mut counts = self.progress.reads.lock().unwrap();
            for key in &keys {
                counts.entry(key.clone()).or_default().0 += usize::from(overlapping);
            }
        }
        self.client
            .get_mut()
            .write_all(format!("{frame}\n").as_bytes())
            .await
            .unwrap();
        let mut line = String::new();
        tokio::time::timeout(REQUEST_PATIENCE, self.client.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        let reply = serde_json::from_str(&line).unwrap();
        {
            let mut counts = self.progress.reads.lock().unwrap();
            for key in &keys {
                let count = counts.entry(key.clone()).or_default();
                count.1 = count.1.max(line.len());
            }
        }
        let elapsed = requested.elapsed();
        for key in keys {
            self.samples
                .record_client_total(&format!("mcp.control:{key}"), elapsed);
        }
        reply
    }

    async fn filtered_tasks(&mut self, agent: &str) {
        for case in filtered_task_cases() {
            let mut arguments = case.arguments.clone();
            arguments["action"] = json!("tracker_list_tasks");
            let action = serde_json::from_value(arguments).unwrap();
            let reply = self.request(agent, &action, Some(case.name)).await;
            assert_eq!(reply["ok"], true, "{}: {reply}", case.name);
            let rows = reply["result"]["tasks"].as_array().unwrap();
            assert_eq!(rows.len(), case.rows, "{}: {reply}", case.name);
            assert!(reply["result"]["next_cursor"].is_string());
            for row in rows {
                for (key, value) in case.arguments.as_object().unwrap() {
                    assert!(matches_filter(row, key, value), "{}: {row}", case.name);
                }
            }
        }
    }
}

fn matches_filter(row: &Value, key: &str, expected: &Value) -> bool {
    match key {
        "label" => row["labels"].as_array().unwrap().contains(expected),
        _ => row[key] == *expected,
    }
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
    let mut reader = ListReader {
        client: tokio::io::BufReader::new(client),
        samples,
        progress,
    };
    start.wait().await;
    let mut cursor = None::<String>;
    let mut last_number = u64::MAX;
    for round in 0..LIST_ROUNDS {
        let mut task_page = json!({"action":"tracker_list_tasks"});
        if round > 0 {
            task_page["limit"] = json!(20);
            task_page["cursor"] = json!(cursor);
        }
        for (endpoint, action) in [
            (&agent, serde_json::from_value(task_page).unwrap()),
            (&router, BridgeAction::ListWork),
        ] {
            let case = (action.tool_name() == "list_tasks").then_some("unfiltered");
            let reply = reader.request(endpoint, &action, case).await;
            assert_list_page(&action, &reply, &mut cursor, &mut last_number);
        }
        if round == 0 {
            reader.filtered_tasks(&agent).await;
        }
    }
    drop(reader);
    server.await.unwrap();
}

fn assert_list_page(
    action: &BridgeAction,
    reply: &Value,
    cursor: &mut Option<String>,
    last_number: &mut u64,
) {
    assert_eq!(reply["ok"], true, "{reply}");
    let rows = if action.tool_name() == "list_tasks" {
        "tasks"
    } else {
        "work"
    };
    let count = reply["result"][rows].as_array().unwrap().len();
    if rows == "tasks" {
        assert_eq!(count, 20);
        assert!(reply["result"]["next_cursor"].is_string());
        *cursor = reply["result"]["next_cursor"].as_str().map(str::to_owned);
        for task in reply["result"]["tasks"].as_array().unwrap() {
            let number = task["number"].as_u64().unwrap();
            assert!(
                number < *last_number,
                "cursor pages preserve descending key order"
            );
            *last_number = number;
        }
    } else {
        assert!(count >= TASKS);
        assert!(reply["result"]["work"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["kind"] != "capture"));
    }
}
