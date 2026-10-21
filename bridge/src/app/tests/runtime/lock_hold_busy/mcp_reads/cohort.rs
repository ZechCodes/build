//! Same seed and writers in each comparison; only task-list competition varies.
use super::*;

#[derive(Default)]
struct Overlap {
    task_lists: AtomicUsize,
    reads_during_task_lists: [AtomicUsize; TOOLS.len()],
    reads_before_writers_finished: [AtomicUsize; TOOLS.len()],
}

pub(super) async fn measure(load: Load) {
    let (busy, fixture) = Busy::with_seed(|app, owners, tasks| {
        Arc::new(Fixture::seed(
            app,
            &owners[1].0,
            &tasks[AGENTS],
            &tasks[AGENTS + 1],
        ))
    });
    // Drain untimed startup activity; every subsequent request is measured.
    let _ = wait_for_fixture_idle(&busy).await;
    busy.samples.clear();
    let plane = ControlPlane::new(&busy.state.lock().unwrap());
    let completed = Arc::new(AtomicUsize::new(0));
    let overlap = Arc::new(Overlap::default());
    let extra = if load.busy() {
        AGENTS * 2 + RPC_METHODS.len()
    } else {
        0
    };
    let list_streams = if load.task_lists() { AGENTS } else { 0 };
    let start = Arc::new(tokio::sync::Barrier::new(AGENTS + extra + list_streams + 1));
    let mut calls = if load.busy() {
        writers(&busy, &plane, &start, &completed)
    } else {
        Vec::new()
    };
    for (i, (_, agent)) in busy.owners.iter().take(AGENTS).enumerate() {
        let reader = Reader {
            state: busy.state.clone(),
            plane: plane.clone(),
            fixture: fixture.clone(),
            agent: agent.clone(),
            samples: busy.samples.clone(),
            start: start.clone(),
            overlap: overlap.clone(),
            completed: completed.clone(),
            offset: i,
            load,
        };
        calls.push(tokio::spawn(reader.run(false)));
        if load.task_lists() {
            let lister = Reader {
                state: busy.state.clone(),
                plane: plane.clone(),
                fixture: fixture.clone(),
                agent: agent.clone(),
                samples: busy.samples.clone(),
                start: start.clone(),
                overlap: overlap.clone(),
                completed: completed.clone(),
                offset: 0,
                load,
            };
            calls.push(tokio::spawn(lister.run(true)));
        }
    }
    start.wait().await;
    let failures = join_calls(calls).await;
    let measurements = wait_for_fixture_idle(&busy).await;
    println!(
        "TASK741_MCP_READS {}",
        json!({
            "load":load.name(),"conversations":CONVERSATIONS,"worktrees":WORKTREES,
            "initial_tasks":TASKS,"simulated_running_agents":AGENTS,"rounds":ROUNDS,
            "six_read_streams":AGENTS,"list_task_streams":list_streams,
            "reads_during_task_lists":counts(&overlap.reads_during_task_lists),
            "reads_before_writers_finished":counts(&overlap.reads_before_writers_finished),
            "completed_writer_calls":completed.load(Ordering::Relaxed),
            "failures":failures,"measurements":measurements,
        })
    );
    assert!(
        failures.is_empty(),
        "all socket callers complete: {failures:?}"
    );
    assert_load(
        load,
        completed.load(Ordering::Relaxed),
        &overlap,
        &measurements,
    );
    assert_measurements(&measurements, load.task_lists());
}

async fn join_calls(calls: Vec<tokio::task::JoinHandle<()>>) -> Vec<String> {
    let mut failures = Vec::new();
    for call in calls {
        if let Err(error) = call.await {
            failures.push(error.to_string());
        }
    }
    failures
}

fn assert_load(load: Load, completed: usize, overlap: &Overlap, measurements: &Value) {
    if load.busy() {
        assert_eq!(completed, AGENTS * ROUNDS * CONTROL_TOOLS.len());
        for (method, expected) in expected_request_samples() {
            assert_eq!(
                measurements[&method]["client.total"]["count"], expected,
                "{method}"
            );
        }
        assert!(
            overlap
                .reads_before_writers_finished
                .iter()
                .all(|count| count.load(Ordering::Relaxed) > 0),
            "all six reads overlap active writers"
        );
    }
    if load.task_lists() {
        assert!(
            overlap
                .reads_during_task_lists
                .iter()
                .all(|count| count.load(Ordering::Relaxed) > 0),
            "all six reads overlap task-list requests"
        );
    }
}

struct Reader {
    state: Arc<Mutex<AppState>>,
    plane: Arc<ControlPlane>,
    fixture: Arc<Fixture>,
    agent: String,
    samples: PhaseSamples,
    start: Arc<tokio::sync::Barrier>,
    overlap: Arc<Overlap>,
    completed: Arc<AtomicUsize>,
    offset: usize,
    load: Load,
}

impl Reader {
    async fn run(self, task_lists: bool) {
        let (client, daemon) = tokio::net::UnixStream::pair().unwrap();
        let server = tokio::spawn(handle_done_stream(
            self.state.clone(),
            daemon,
            self.plane.clone(),
        ));
        let mut client = tokio::io::BufReader::new(client);
        self.start.wait().await;
        self.wait_for_load(task_lists).await;
        for _ in 0..ROUNDS {
            for action in self.actions(task_lists) {
                self.before_send(&action, task_lists);
                let requested = Instant::now();
                let response = reply(&mut client, self.fixture.frame(&self.agent, &action)).await;
                self.samples.record_client_total(
                    &format!("mcp.control:{}", action.tool_name()),
                    requested.elapsed(),
                );
                if task_lists {
                    self.overlap.task_lists.fetch_sub(1, Ordering::SeqCst);
                }
                self.fixture.assert_reply(&action, &response);
            }
        }
        drop(client);
        server.await.unwrap();
    }

    fn actions(&self, task_lists: bool) -> Vec<BridgeAction> {
        if task_lists {
            return vec![self.fixture.task_list()];
        }
        let mut actions = self.fixture.cases();
        actions.rotate_left(self.offset % TOOLS.len());
        actions
    }

    async fn wait_for_load(&self, task_lists: bool) {
        if !self.load.busy() {
            return;
        }
        tokio::time::timeout(REQUEST_PATIENCE, async {
            loop {
                let completed = self.completed.load(Ordering::Relaxed);
                assert!(
                    completed < AGENTS * ROUNDS * CONTROL_TOOLS.len(),
                    "read first wave must start while writers remain active"
                );
                let lists_ready = task_lists
                    || !self.load.task_lists()
                    || self.overlap.task_lists.load(Ordering::SeqCst) > 0;
                if completed > 0 && lists_ready {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("busy writer/task-list first wave starts");
    }

    fn before_send(&self, action: &BridgeAction, task_lists: bool) {
        if task_lists {
            self.overlap.task_lists.fetch_add(1, Ordering::SeqCst);
            return;
        }
        let index = TOOLS
            .iter()
            .position(|tool| *tool == action.tool_name())
            .unwrap();
        if self.overlap.task_lists.load(Ordering::SeqCst) > 0 {
            self.overlap.reads_during_task_lists[index].fetch_add(1, Ordering::Relaxed);
        }
        if self.load.busy()
            && self.completed.load(Ordering::Relaxed) < AGENTS * ROUNDS * CONTROL_TOOLS.len()
        {
            self.overlap.reads_before_writers_finished[index].fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn writers(
    busy: &Busy,
    plane: &Arc<ControlPlane>,
    start: &Arc<tokio::sync::Barrier>,
    completed: &Arc<AtomicUsize>,
) -> Vec<tokio::task::JoinHandle<()>> {
    let mut calls = status_observers(busy, start);
    for (i, ((owner, agent), task)) in busy.owners.iter().take(AGENTS).zip(&busy.tasks).enumerate()
    {
        calls.push(tokio::spawn(control_agent(
            busy.state.clone(),
            plane.clone(),
            ControlIdentity {
                owner: owner.clone(),
                agent: agent.clone(),
                task: task.clone(),
                recipient: busy.owners[(i + 1) % AGENTS].1.clone(),
                view: busy.view.clone(),
            },
            start.clone(),
            completed.clone(),
            busy.samples.clone(),
        )));
    }
    let intake = FrameIntake::with_pool(
        AppState::handler(busy.state.clone()),
        crate::transport::generate_transport_keypair(),
        128,
        8,
    );
    let (owner, agent) = &busy.owners[0];
    let thread = busy
        .state
        .lock()
        .unwrap()
        .agent_conversation(owner, Some(agent))
        .unwrap()
        .id
        .clone();
    let rpcs = [
        (
            "entity.seen",
            json!({"entity_id":owner,"agent_id":agent,"thread_id":thread}),
        ),
        (
            "thread.post",
            json!({"entity_id":owner,"agent_id":agent,"thread_id":thread,"body":"busy user turn"}),
        ),
        ("user.present", json!({})),
        (
            "tasks.review.get",
            json!({"task_ids":busy.tasks.iter().take(8).collect::<Vec<_>>()}),
        ),
        ("git.show", json!({"run_id":owner,"hash":busy.hash})),
    ];
    for (method, params) in rpcs {
        calls.push(busy_rpc(
            intake.clone(),
            start.clone(),
            method,
            params,
            busy.samples.clone(),
        ));
    }
    calls
}

fn counts(values: &[AtomicUsize; TOOLS.len()]) -> Value {
    Value::Object(
        TOOLS
            .into_iter()
            .zip(values.iter())
            .map(|(tool, count)| (tool.into(), json!(count.load(Ordering::Relaxed))))
            .collect(),
    )
}
