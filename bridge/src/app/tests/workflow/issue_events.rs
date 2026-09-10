use super::*;

/// Connect to a unix socket a spawned worker is still binding.
async fn connect_when_bound(path: &std::path::Path) -> tokio::net::UnixStream {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match tokio::net::UnixStream::connect(path).await {
            Ok(stream) => return stream,
            Err(error) if std::time::Instant::now() >= deadline => {
                panic!("done socket never came up at {}: {error}", path.display())
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
}

/// The hand-off has to survive the path it actually travels: a `done` line
/// on the daemon's control socket, where the turn is queued while the socket
/// worker holds the state lock. Only draining that queue after the lock is
/// free gets the validation prompt written — and it must be written to the
/// SAME process that just reported the stage built, not a replacement.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_done_over_the_socket_delivers_the_validation_turn_to_the_same_agent() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    on_the_terminal_provider(&state);
    let (run_id, root) = {
        // The fixture only needs state; the frame handler below is what
        // delivered the dispatch turn that opened the agent.
        let mut s = state.lock().unwrap();
        run_awaiting_a_real_stage_build(&mut s, "hand off over the socket")
    };
    // `run.create` above ran on the shared state directly, so its dispatch
    // turn is still queued; the handler is the thing that delivers.
    let opened = call(&handler, "run.get", json!({ "run_id": run_id }));
    assert_eq!(opened["ok"], true, "{opened:?}");
    let key = derived_agent_key(&root, &run_id);
    wait_for_agent_tab(&state, &key).await;
    let (build_pid, session_token) = {
        let s = state.lock().unwrap();
        let tab = s
            .session_registry
            .test_tab(&key)
            .expect("dispatching a stage opens the worktree's agent");
        let agent_id = &s.runs[&run_id].agents.primary().unwrap().id;
        (
            agent_pid(tab).expect("a live harness has a pid"),
            // The capability is minted per AGENT: that is who reports.
            s.session_registry.test_token(agent_id).unwrap().to_string(),
        )
    };

    let socket_path = dir.path().join("done.sock");
    AppState::spawn_done_socket(
        Arc::clone(&state),
        socket_path.to_string_lossy().into_owned(),
    );
    let mut socket = connect_when_bound(&socket_path).await;
    let reporting_agent = state.lock().unwrap().runs[&run_id]
        .agents
        .primary()
        .unwrap()
        .id
        .clone();
    let report = json!({
        "task_id": reporting_agent,
        "session_token": session_token,
        "report": {
            "phase": "build",
            "status": "completed",
            "summary": "stage one is built",
            "outputs": {},
        },
    });
    socket
        .write_all(format!("{report}\n").as_bytes())
        .await
        .unwrap();
    socket.flush().await.unwrap();

    let screen = wait_for_agent_screen(&state, &root, "VALIDATION agent").await;
    assert!(
        screen.contains("VALIDATION agent"),
        "the validation turn must reach the agent's PTY: {screen:?}"
    );
    let s = state.lock().unwrap();
    assert_eq!(
        s.runs[&run_id].stages[0].state,
        StageProgressState::Validating
    );
    assert_eq!(
        s.session_registry.test_tab(&key).and_then(agent_pid),
        Some(build_pid),
        "the agent that built the stage is the one asked to validate it"
    );
    assert_eq!(
        s.session_registry.test_counts().tabs,
        1,
        "one worktree, one agent"
    );
}

/// A `done` off the control socket is a frame like any other: it takes the
/// same mutex a browser frame does and its delivery spawns the same
/// harnesses, so it is timed and counted — under a method of its own, since
/// nothing on the wire named it. Untimed, a daemon wedged by a harness's
/// report would report that wedge as nobody's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_done_over_the_socket_is_timed_under_its_own_method() {
    let (dir, repo) = init_repo();
    let (clock, lines) = recording_clock();
    let state = qa_state_timed_by(Arc::clone(&clock), &repo, dir.path());
    on_the_terminal_provider(&state);
    let (agent_id, session_token) = {
        let mut s = state.lock().unwrap();
        let (run_id, _root) = run_awaiting_a_real_stage_build(&mut s, "report over the socket");
        // The capability is minted at spawn. Handing one out without a
        // spawn is what leaves the run's agent cold, so the report's
        // delivery is the one that opens a harness.
        let agent_id = s.runs[&run_id].agents.primary().unwrap().id.clone();
        let session_token = uuid::Uuid::new_v4().to_string();
        s.session_registry
            .test_install_token(agent_id.clone(), session_token.clone());
        (agent_id, session_token)
    };
    // A spawn builds its session locator with the app mutex released, so a
    // factory that takes its time is time the delivery spends and the frame
    // that queued it does not. Seconds, not a slow frame's worth: the
    // reporting frame renders the run's whole diff under the mutex and is
    // hundreds of milliseconds on a loaded machine by itself, so only a
    // spawn an order of magnitude longer tells a frame that waited for it
    // apart from one that was merely slow.
    const SPAWN_HOLD: Duration = Duration::from_secs(3);
    state.lock().unwrap().session_locator_factory = Arc::new(move |_, _| {
        std::thread::sleep(SPAWN_HOLD);
        None
    });

    let socket_path = dir.path().join("done.sock");
    AppState::spawn_done_socket(
        Arc::clone(&state),
        socket_path.to_string_lossy().into_owned(),
    );
    let mut socket = connect_when_bound(&socket_path).await;
    let report = json!({
        "task_id": agent_id,
        "session_token": session_token,
        "report": {
            "phase": "build",
            "status": "completed",
            "summary": "stage one is built",
            "outputs": {},
        },
    });
    socket
        .write_all(format!("{report}\n").as_bytes())
        .await
        .unwrap();
    socket.flush().await.unwrap();

    let line = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let logged = lines
                .lock()
                .unwrap()
                .iter()
                .find(|line| line.starts_with("slow frame agent.deliver "))
                .cloned();
            if let Some(line) = logged {
                return line;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "the report's delivery logged nothing of its own: {:?}",
            lines.lock().unwrap()
        )
    });

    assert!(
        slow_frame_millis(&line, "total=") >= SLOW_FRAME.as_secs_f64() * 1000.0,
        "the spawn's seconds are the delivery's own: {line}"
    );
    let stats = clock.stats();
    assert!(
        stats["methods"]["mcp.control"]["served"]
            .as_u64()
            .is_some_and(|served| served >= 1),
        "the socket's frames are counted since boot: {stats}"
    );
    assert!(
        stats["methods"]["mcp.control"]["max_ms"]
            .as_f64()
            .is_some_and(|held| held < SPAWN_HOLD.as_millis() as f64),
        "the reporting frame answered only once the harness it triggered was up: {stats}"
    );
}

/// A router reaching a branch is a whole checkout of the repository, and it
/// arrives over the control socket rather than over a frame. It runs the
/// same way every other dispatch does: the socket takes the job out under
/// the guard and runs it with the guard released, so a routed capture no
/// longer serializes the daemon for the length of a `git worktree add`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_router_dispatch_over_the_socket_cuts_its_branch_with_the_state_lock_free() {
    let (dir, repo) = init_repo();
    let state = qa_state_timed_by(FrameClock::new(), &repo, dir.path());
    let (gate, gate_handle) = OffLockGate::new();
    let (capture_id, project_id, agent_id, session_token) = {
        let mut app = state.lock().unwrap();
        let project_id = app.project_at(0).id.clone();
        let (capture_id, agent_id) = captured(&mut app, "finish the toast on the login branch");
        app.pending_agent_turns.clear();
        let session_token = uuid::Uuid::new_v4().to_string();
        app.session_registry
            .test_install_token(agent_id.clone(), session_token.clone());
        app.off_lock_gate = Some(gate);
        (capture_id, project_id, agent_id, session_token)
    };

    let socket_path = dir.path().join("done.sock");
    AppState::spawn_done_socket(
        Arc::clone(&state),
        socket_path.to_string_lossy().into_owned(),
    );
    let mut socket = connect_when_bound(&socket_path).await;
    let request = json!({
        "task_id": agent_id,
        "session_token": session_token,
        "request": {
            "action": "dispatch_branch",
            "project_id": project_id,
            "instruction": "finish the toast",
            "rationale": "continues the login work",
        },
    });
    socket
        .write_all(format!("{request}\n").as_bytes())
        .await
        .unwrap();
    socket.flush().await.unwrap();

    let gate_handle = tokio::task::spawn_blocking(move || {
        gate_handle.wait_for_arrival();
        gate_handle
    })
    .await
    .unwrap();
    assert!(
        state.try_lock().is_ok(),
        "the router's dispatch is holding the app mutex through its git"
    );
    gate_handle.release();

    let mut lines = tokio::io::BufReader::new(socket).lines();
    let answered = tokio::time::timeout(Duration::from_secs(30), lines.next_line())
        .await
        .expect("the socket answers the router")
        .unwrap()
        .expect("the socket answers the router");
    let answered: Value = serde_json::from_str(&answered).unwrap();
    assert_eq!(answered["ok"], true, "{answered:?}");
    assert_eq!(
        answered["result"]["branch"], "build/finish-the-toast",
        "the router is told where the work went: {answered:?}"
    );
    let app = state.lock().unwrap();
    let routing = app.captures[&capture_id]
        .routing
        .as_ref()
        .expect("the route is written down once the branch is real");
    assert_eq!(routing.target_id, "build/finish-the-toast");
    assert_eq!(
        routing.rationale.as_deref(),
        Some("continues the login work")
    );
}

/// A second daemon pointed at a live control socket must not steal it: the
/// running bridge's harnesses dial this path for every `done`, and an
/// unlink would silently break every one of them. The newcomer refuses and
/// says so; the incumbent keeps accepting.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_done_socket_is_never_stolen_by_a_second_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("done.sock");
    let incumbent = bind_done_listener(&path).expect("the first daemon binds");

    let state = AppState::new_unrooted(dir.path(), "main", true, "unused").shared();
    AppState::spawn_done_socket(Arc::clone(&state), path.to_string_lossy().into_owned());
    // Give the would-be thief time to run its bind attempt to completion.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // The path still belongs to the incumbent: a fresh client's connect is
    // answered by the ORIGINAL listener, not a replacement.
    let (accepted, connected) =
        tokio::join!(incumbent.accept(), tokio::net::UnixStream::connect(&path),);
    accepted.expect("the incumbent still owns its socket");
    connected.expect("harnesses can still dial the path");
}

/// An issue's agent runs in the primary checkout, so every plan verb is a
/// turn addressed there — never to a worktree, never to a fresh process —
/// and it splits cold/warm exactly as the run verbs do: the reviewer's
/// words are already durable on the plan's thread, so a warm agent is only
/// told to read them, while a cold one gets the same instruction wrapped in
/// the plan context it has no way to reconstruct.
#[test]
#[allow(clippy::cognitive_complexity)] // ratchet: plan_verbs_are_turns_addressed_to_the_primary_checkout is at 26, threshold 15 — bring it under, then remove
fn plan_verbs_are_turns_addressed_to_the_primary_checkout() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let notes_plan = plan_id_of(&state.handle(req("plan.create", json!({ "goal": "notes" }))));
    let stage_plan = plan_id_of(&state.handle(req("plan.create", json!({ "goal": "stages" }))));
    let comment = state.handle(req(
        "plan.comment_add",
        json!({ "plan_id": stage_plan, "stage_id": "first-half", "body": "split further" }),
    ));
    assert_eq!(comment["ok"], true, "{comment:?}");
    let planning_root = |state: &AppState, plan_id: &str| {
        AppState::canonical_root(
            &state.plans[plan_id]
                .workspace
                .as_ref()
                .expect("a drafting issue has a planning workspace")
                .checkout,
        )
    };
    let notes_root = planning_root(&state, &notes_plan);
    let stage_root = planning_root(&state, &stage_plan);
    assert_eq!(
        notes_root,
        std::fs::canonicalize(&repo).unwrap(),
        "an issue drafts in the project's primary checkout"
    );
    assert_eq!(
        notes_root, stage_root,
        "every issue of a project drafts in that one checkout"
    );
    // The scripted agent answers every verb itself and drives the plan back
    // to its gate; from here each plan must stay where its verb puts it.
    state.qa_agent = false;

    state.pending_agent_turns.clear();
    let sent = state.handle(req(
        "plan.send_notes",
        json!({ "plan_id": notes_plan, "comments": "make stage two smaller" }),
    ));
    assert_eq!(sent["ok"], true, "{sent:?}");
    assert!(
        state.session_registry.test_counts().tabs == 0,
        "a verb queues a turn; only delivery — off the state lock — spawns"
    );
    let queued = state
        .pending_agent_turns
        .last()
        .expect("plan notes are a turn for the issue's agent");
    assert_eq!(queued.owner, notes_plan);
    assert_eq!(
        queued.root, notes_root,
        "a plan's turn goes to the primary checkout"
    );
    assert_eq!(queued.phase, "revise");
    assert_eq!(
        queued.said().warm,
        NEW_THREAD_MESSAGES_PROMPT,
        "an agent already drafting is only told to read the thread"
    );
    assert!(
        queued.said().cold.contains(NEW_THREAD_MESSAGES_PROMPT)
            && queued.said().cold.contains("Build conversation protocol"),
        "a cold agent gets the plan context AND the instruction: {}",
        queued.said().cold
    );
    let durable = primary_thread(&state.plans[&notes_plan].agents)
        .items
        .iter()
        .any(|item| {
            matches!(item, crate::thread::ThreadItem::Message(m)
            if m.body == "make stage two smaller")
        });
    assert!(durable, "the notes stay durable on the plan's thread");

    // A freeform message reaches the same agent while the plan drafts.
    state.pending_agent_turns.clear();
    let messaged = state.handle(req(
        "plan.message",
        json!({ "plan_id": notes_plan, "message": "prefer smaller stages" }),
    ));
    assert_eq!(messaged["ok"], true, "{messaged:?}");
    let queued = state
        .pending_agent_turns
        .last()
        .expect("a plan message is a turn for the issue's agent");
    assert_eq!(queued.owner, notes_plan);
    assert_eq!(queued.root, notes_root);
    assert_eq!(queued.phase, "message");
    assert_eq!(queued.said().warm, NEW_THREAD_MESSAGES_PROMPT);
    assert!(
        queued.said().cold.contains(NEW_THREAD_MESSAGES_PROMPT)
            && queued.said().cold.contains("Build conversation protocol"),
        "{}",
        queued.said().cold
    );
    let durable = primary_thread(&state.plans[&notes_plan].agents)
        .items
        .iter()
        .any(|item| {
            matches!(item, crate::thread::ThreadItem::Message(m)
            if m.body == "prefer smaller stages")
        });
    assert!(durable, "the message stays durable on the plan's thread");

    // A stage's open comments are the payload of a per-stage revision.
    state.pending_agent_turns.clear();
    let stage_notes = state.handle(req(
        "plan.stage_send_notes",
        json!({ "plan_id": stage_plan, "stage_id": "first-half" }),
    ));
    assert_eq!(stage_notes["ok"], true, "{stage_notes:?}");
    let queued = state
        .pending_agent_turns
        .last()
        .expect("stage notes are a turn for the issue's agent");
    assert_eq!(queued.owner, stage_plan);
    assert_eq!(queued.root, stage_root);
    assert_eq!(queued.phase, "revise");
    assert!(
        queued.said().warm.contains("read_unread_messages"),
        "the comments travel through MCP; the turn only points at them: {}",
        queued.said().warm
    );
    assert!(
        !queued.said().warm.contains("Build conversation protocol"),
        "an agent already drafting is not re-taught the protocol: {}",
        queued.said().warm
    );
    assert!(
        queued.said().cold.contains(".build/plan/01-first-half.md")
            && queued.said().cold.contains("Build conversation protocol"),
        "a cold agent is pointed at the stage doc it must revise: {}",
        queued.said().cold
    );
}

/// The plan half of "one checkout, one agent": authoring a plan opens the
/// issue's agent in the primary checkout, and every later plan verb reaches
/// THAT process. Same pid, one tab — a plan revision is a turn, not a
/// replacement.
#[tokio::test]
async fn every_plan_verb_reaches_the_issues_one_agent() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    on_the_terminal_provider(&state);
    let plan = call(&handler, "plan.create", json!({ "goal": "one plan agent" }));
    assert_eq!(plan["ok"], true, "{plan:?}");
    let plan_id = plan_id_of(&plan);
    let key = {
        let s = state.lock().unwrap();
        derived_agent_key(
            &AppState::canonical_root(
                &s.plans[&plan_id]
                    .workspace
                    .as_ref()
                    .expect("a plan at its gate keeps its workspace")
                    .checkout,
            ),
            &plan_id,
        )
    };
    wait_for_deliveries(&state).await;
    let drafting_pid = {
        let s = state.lock().unwrap();
        let tab = s
            .session_registry
            .test_tab(&key)
            .expect("authoring a plan opens the primary checkout's agent");
        assert!(tab.session_is_live(), "the plan's agent is running");
        agent_pid(tab).expect("a live harness has a pid")
    };
    // The scripted agent would answer each verb itself and drive the plan
    // straight back to its gate; from here it must stay where a verb puts it.
    state.lock().unwrap().qa_agent = false;

    for (method, params) in [
        (
            "plan.send_notes",
            json!({ "plan_id": plan_id, "comments": "make stage two smaller" }),
        ),
        (
            "plan.message",
            json!({ "plan_id": plan_id, "message": "prefer smaller stages" }),
        ),
    ] {
        let done = call(&handler, method, params);
        assert_eq!(done["ok"], true, "{method}: {done:?}");
        wait_for_deliveries(&state).await;
        let s = state.lock().unwrap();
        assert_eq!(
            s.session_registry.test_tab(&key).and_then(agent_pid),
            Some(drafting_pid),
            "{method} must reach the process that authored the plan"
        );
        assert_eq!(
            s.session_registry.test_counts().tabs,
            1,
            "{method}: one worktree, one agent"
        );
    }
}

#[test]
fn plan_message_rejects_the_review_gate_and_unknown_plans() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "at the gate" })));
    let plan_id = plan_id_of(&plan);
    let gated = state.handle(req(
        "plan.message",
        json!({ "plan_id": plan_id, "message": "hi" }),
    ));
    assert!(
        gated["error"].as_str().unwrap().contains("review gate"),
        "{gated:?}"
    );
    let unknown = state.handle(req(
        "plan.message",
        json!({ "plan_id": "plan-nope", "message": "hi" }),
    ));
    assert_eq!(unknown["ok"], false);
}

#[test]
fn plan_delete_is_abandoned_only() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "kill me" })));
    let plan_id = plan_id_of(&plan);
    let live = state.handle(req("plan.delete", json!({ "plan_id": plan_id })));
    assert!(
        live["error"].as_str().unwrap().contains("abandoned"),
        "{live:?}"
    );
    state.handle(req("plan.abandon", json!({ "plan_id": plan_id })));
    let gone = state.handle(req("plan.delete", json!({ "plan_id": plan_id })));
    assert_eq!(gone["ok"], true, "{gone:?}");
    assert_eq!(
        state.handle(req("plan.get", json!({ "plan_id": plan_id })))["ok"],
        false
    );
}

#[test]
fn on_agent_done_routes_by_owner_lookup() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    // A run parked at Building with no live session (owner-lookup target).
    let active = crate::orchestrator::ActiveRun::reattach(
        &fake_run_record("run-route"),
        ".build/plan.md".into(),
    );
    let project_id = state.project_at(0).id.clone();
    state.projects.bind_entity("run-route".into(), project_id);
    state.runs.insert("run-route".into(), active);
    // A completed build report routes to the runs map and opens review.
    state.on_agent_done(
        "run-route",
        DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Completed,
            summary: "built".into(),
            outputs: DoneOutputs::default(),
        },
    );
    let got = state.handle(req("run.get", json!({ "run_id": "run-route" })));
    assert_eq!(got["result"]["state"], "review", "{got:?}");
    // An unknown owner id is a quiet no-op, never a panic.
    state.on_agent_done(
        "run-nope",
        DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Completed,
            summary: "x".into(),
            outputs: DoneOutputs::default(),
        },
    );
}

/// The record of a reported completion is the agent's own message: the
/// summary it wrote, the outcome it reported, and the structured report
/// riding the message that carries them.
#[test]
fn a_completed_build_report_is_one_agent_message_carrying_the_completion_report() {
    let mut thread = crate::thread::Thread::new("run-completion");
    record_report_in_thread(
        &mut thread,
        &DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Completed,
            summary: "Fixed and deployed the renderer.".into(),
            outputs: DoneOutputs {
                completion_report: Some(crate::thread::CompletionReport {
                    critical_files: vec!["src/render.rs — the new draw path".into()],
                    risk_notes: vec!["untested on the legacy screen".into()],
                    decisions: vec!["kept the old entry point".into()],
                    skips: vec!["no perf pass".into()],
                }),
                ..DoneOutputs::default()
            },
        },
        None,
    );

    assert_eq!(thread.items.len(), 1, "{:?}", thread.items);
    let crate::thread::ThreadItem::Message(completion) = &thread.items[0] else {
        panic!("the completion is a message: {:?}", thread.items);
    };
    assert_eq!(completion.role, crate::thread::MessageRole::Agent);
    assert_eq!(
        completion.outcome,
        Some(crate::thread::MessageOutcome::Completed)
    );
    assert!(
        completion.done,
        "the flag an older client reads keeps its meaning"
    );
    assert_eq!(completion.body, "Fixed and deployed the renderer.");
    let carried = completion
        .completion_report
        .as_deref()
        .expect("the report rides the message");
    assert_eq!(
        carried.critical_files,
        vec!["src/render.rs — the new draw path"]
    );
    assert_eq!(carried.skips, vec!["no perf pass"]);
    assert_eq!(
        thread.last_completion.as_ref(),
        Some(carried),
        "a cold session still finds the newest report on the thread"
    );
    assert_eq!(thread.items[0].attention_reason(), Some("done"));
}

/// Every outcome an agent reports lands on its own message. What stays an
/// event is what Build read for itself — a triage pass nobody has to
/// answer, and a validation report Build judged.
#[test]
fn conversation_records_every_reported_outcome_on_the_agents_message() {
    let mut thread = crate::thread::Thread::new("run-activity");
    record_report_in_thread(
        &mut thread,
        &DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Blocked,
            summary: "Needs production credentials".into(),
            outputs: DoneOutputs::default(),
        },
        None,
    );
    assert!(
        thread.items.iter().any(|item| matches!(
            item,
            crate::thread::ThreadItem::Message(message)
                if message.outcome == Some(crate::thread::MessageOutcome::Blocked)
                    && message.body == "Needs production credentials"
                    && !message.done
        )),
        "{:?}",
        thread.items
    );

    record_report_in_thread(
        &mut thread,
        &DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Failed,
            summary: "The migration will not run".into(),
            outputs: DoneOutputs::default(),
        },
        None,
    );
    assert!(
        thread.items.iter().any(|item| matches!(
            item,
            crate::thread::ThreadItem::Message(message)
                if message.outcome == Some(crate::thread::MessageOutcome::Failed)
                    && message.body == "The migration will not run"
        )),
        "{:?}",
        thread.items
    );

    record_report_in_thread(
        &mut thread,
        &DoneReport {
            phase: DonePhase::Validate,
            status: DoneStatus::Completed,
            summary: "Validation completed".into(),
            outputs: DoneOutputs {
                validation: Some(crate::run::ValidationReport {
                    passed: false,
                    findings: "The migration is not reversible".into(),
                    notes_for_next_stage: String::new(),
                }),
                completion_report: Some(crate::thread::CompletionReport {
                    critical_files: vec!["src/app.rs".into()],
                    ..crate::thread::CompletionReport::default()
                }),
                ..DoneOutputs::default()
            },
        },
        None,
    );
    assert!(
        thread.items.iter().any(|item| matches!(
            item,
            crate::thread::ThreadItem::Event(event)
                if event.event == crate::thread::ThreadEventKind::ReviewBlocked
                    && event.summary.as_deref() == Some("The migration is not reversible")
        )),
        "Build's own reading of a validation report stays an event: {:?}",
        thread.items
    );
    assert_eq!(
        thread.last_completion.as_ref().unwrap().critical_files,
        vec!["src/app.rs"]
    );

    record_report_in_thread(
        &mut thread,
        &DoneReport {
            phase: DonePhase::Triage,
            status: DoneStatus::Completed,
            summary: "Classified 12 hunks".into(),
            outputs: DoneOutputs::default(),
        },
        None,
    );
    assert!(
        thread.items.iter().any(|item| matches!(
            item,
            crate::thread::ThreadItem::Event(event)
                if event.event == crate::thread::ThreadEventKind::Triaged
        )),
        "a triage pass asks nothing of anyone and stays quiet: {:?}",
        thread.items
    );
    assert!(
        !thread.items.iter().any(|item| matches!(
            item,
            crate::thread::ThreadItem::Event(event)
                if matches!(
                    event.event,
                    crate::thread::ThreadEventKind::Done
                        | crate::thread::ThreadEventKind::Blocked
                        | crate::thread::ThreadEventKind::RunFailed
                )
        )),
        "nothing emits the outcome events any more: {:?}",
        thread.items
    );
}

/// A report Build could not apply is still the agent's report: the outcome
/// is a failure on its message, and Build's note rides the same body.
#[test]
fn a_report_build_cannot_apply_fails_on_the_agents_message() {
    let mut thread = crate::thread::Thread::new("run-unapplied");
    record_report_in_thread(
        &mut thread,
        &DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Completed,
            summary: "Implemented the change".into(),
            outputs: DoneOutputs::default(),
        },
        Some("the worktree is gone"),
    );

    assert_eq!(thread.items.len(), 1, "{:?}", thread.items);
    let crate::thread::ThreadItem::Message(failed) = &thread.items[0] else {
        panic!("the outcome is a message: {:?}", thread.items);
    };
    assert_eq!(
        failed.outcome,
        Some(crate::thread::MessageOutcome::Failed),
        "{failed:?}"
    );
    assert!(
        failed.body.starts_with("Implemented the change"),
        "{failed:?}"
    );
    assert!(
        failed
            .body
            .contains("Build could not apply the report: the worktree is gone"),
        "{failed:?}"
    );
    assert_eq!(thread.items[0].attention_reason(), Some("run_failed"));
}

/// What Build observed for itself has no agent message to hang on, so it
/// stays an event: an agent that went quiet, and one whose process died.
#[test]
fn builds_own_observations_about_a_silent_agent_stay_events() {
    let mut quiet = crate::thread::Thread::new("run-quiet");
    record_idle_in_thread(&mut quiet, None);
    let mut crashed = crate::thread::Thread::new("run-crashed");
    record_idle_in_thread(
        &mut crashed,
        Some(&HarnessExit {
            code: 1,
            epitaph: Some("out of quota".into()),
        }),
    );
    let mut killed = crate::thread::Thread::new("run-killed");
    record_session_death_in_thread(&mut killed, &now_rfc3339());

    for (thread, kind) in [
        (&quiet, crate::thread::ThreadEventKind::IdleUnreported),
        (&crashed, crate::thread::ThreadEventKind::RunFailed),
        (&killed, crate::thread::ThreadEventKind::Interrupted),
    ] {
        assert!(
            thread.items.iter().any(|item| matches!(
                item,
                crate::thread::ThreadItem::Event(event) if event.event == kind
            )),
            "{kind:?}: {:?}",
            thread.items
        );
    }
}

#[test]
fn review_actions_are_recorded_in_the_plan_conversation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "review activity" })));
    let plan_id = plan_id_of(&plan);

    state.handle(req(
        "plan.stage_approve",
        json!({ "plan_id": plan_id, "stage_id": "first-half" }),
    ));
    state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
    state.handle(req("run.create", json!({ "plan_id": plan_id })));
    let view = state.handle(req("plan.get", json!({ "plan_id": plan_id })));
    let items = view["result"]["thread"]["items"].as_array().unwrap();
    assert!(items
        .iter()
        .any(|item| item["data"]["event"] == "stage_approved"));
    assert!(items.iter().any(|item| item["data"]["event"] == "approved"));
    let implementation_started = items
        .iter()
        .find(|item| item["data"]["event"] == "implementation_started")
        .expect("implementation start is journaled");
    assert!(implementation_started["data"]["links"]
        .as_array()
        .unwrap()
        .iter()
        .any(|link| link["kind"] == "implementation"));
    let worktree_created = items
        .iter()
        .find(|item| item["data"]["event"] == "worktree_created")
        .expect("initial checkout creation is journaled");
    assert!(worktree_created["data"]["links"]
        .as_array()
        .unwrap()
        .iter()
        .any(|link| link["kind"] == "implementation"));
    assert!(worktree_created["data"]["links"]
        .as_array()
        .unwrap()
        .iter()
        .any(|link| link["kind"] == "worktree"));
    assert!(
        !items
            .iter()
            .any(|item| item["data"]["event"] == "worktree_reused"),
        "a newly created checkout must not immediately be mislabeled as reused: {items:?}"
    );
}

#[test]
fn stage_failure_is_journaled_with_issue_stage_and_implementation_references() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue = state.handle(req("issue.create", json!({ "goal": "linked failure" })));
    let issue_id = issue["result"]["issue_id"].as_str().unwrap().to_string();
    for stage_id in ["first-half", "second-half"] {
        state.handle(req(
            "issue.stage_approve",
            json!({ "issue_id": issue_id, "stage_id": stage_id }),
        ));
    }
    state.handle(req("issue.approve", json!({ "issue_id": issue_id })));
    let run = state.handle(req("run.create", json!({ "plan_id": issue_id })));
    let run_id = run_id_of(&run);
    state.qa_agent = false;
    let dispatched = state.handle(req(
        "run.stage_dispatch",
        json!({ "run_id": run_id, "stage_id": "second-half" }),
    ));
    assert_eq!(dispatched["ok"], true, "{dispatched:?}");
    state.on_agent_done(
        &run_id,
        DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Failed,
            summary: "stage implementation failed".into(),
            outputs: DoneOutputs::default(),
        },
    );

    let issue = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
    let failed = issue["result"]["thread"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["data"]["event"] == "stage_failed")
        .expect("stage failure has a dedicated lifecycle event");
    let links = failed["data"]["links"].as_array().unwrap();
    assert!(links.iter().any(|link| {
        link["kind"] == "issue_stage"
            && link["issue_id"] == issue_id
            && link["stage_id"] == "second-half"
    }));
    assert!(links.iter().any(|link| {
        link["kind"] == "implementation"
            && link["issue_id"] == issue_id
            && link["implementation_id"] == run_id
    }));
}

#[test]
fn failed_stage_validation_is_journaled_with_canonical_references() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue = state.handle(req(
        "issue.create",
        json!({ "goal": "linked validation failure" }),
    ));
    let issue_id = issue["result"]["issue_id"].as_str().unwrap().to_string();
    for stage_id in ["first-half", "second-half"] {
        state.handle(req(
            "issue.stage_approve",
            json!({ "issue_id": issue_id, "stage_id": stage_id }),
        ));
    }
    state.handle(req("issue.approve", json!({ "issue_id": issue_id })));
    let run = state.handle(req("run.create", json!({ "plan_id": issue_id })));
    let run_id = run_id_of(&run);
    state.qa_agent = false;
    state.handle(req(
        "run.stage_dispatch",
        json!({ "run_id": run_id, "stage_id": "second-half" }),
    ));
    state.on_agent_done(
        &run_id,
        DoneReport {
            phase: DonePhase::Build,
            status: DoneStatus::Completed,
            summary: "built candidate".into(),
            outputs: DoneOutputs::default(),
        },
    );
    state.on_agent_done(
        &run_id,
        DoneReport {
            phase: DonePhase::Validate,
            status: DoneStatus::Completed,
            summary: "validation found defects".into(),
            outputs: DoneOutputs {
                validation: Some(crate::run::ValidationReport {
                    passed: false,
                    findings: "required regression test is failing".into(),
                    notes_for_next_stage: String::new(),
                }),
                ..DoneOutputs::default()
            },
        },
    );

    let issue = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
    let failed = issue["result"]["thread"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| {
            item["data"]["event"] == "stage_failed"
                && item["data"]["summary"] == "required regression test is failing"
        })
        .expect("failed validation has a stage lifecycle event");
    let links = failed["data"]["links"].as_array().unwrap();
    assert!(links
        .iter()
        .any(|link| { link["kind"] == "issue_stage" && link["stage_id"] == "second-half" }));
    assert!(links
        .iter()
        .any(|link| { link["kind"] == "implementation" && link["implementation_id"] == run_id }));
}

#[test]
fn planning_announces_each_stage_with_a_link_to_its_document() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "linked stages" })));
    let plan_id = plan_id_of(&plan);
    let view = state.handle(req("plan.get", json!({ "plan_id": plan_id })));
    let stage_messages: Vec<&Value> = view["result"]["thread"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| {
            item["type"] == "message"
                && item["data"]["role"] == "agent"
                && item["data"]["links"][0]["kind"] == "issue_stage"
        })
        .collect();

    assert_eq!(stage_messages.len(), 2, "{stage_messages:?}");
    assert_eq!(stage_messages[0]["data"]["links"][0]["issue_id"], plan_id);
    assert_eq!(
        stage_messages[0]["data"]["links"][0]["stage_id"],
        "first-half"
    );
    assert_eq!(
        stage_messages[1]["data"]["links"][0]["stage_id"],
        "second-half"
    );
}

#[test]
fn run_conversation_links_every_started_stage_back_to_the_plan() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let plan = state.handle(req("plan.create", json!({ "goal": "linked dispatch" })));
    let plan_id = plan_id_of(&plan);
    for stage_id in ["first-half", "second-half"] {
        state.handle(req(
            "plan.stage_approve",
            json!({ "plan_id": plan_id, "stage_id": stage_id }),
        ));
    }
    state.handle(req("plan.approve", json!({ "plan_id": plan_id })));
    let run = state.handle(req("run.create", json!({ "plan_id": plan_id })));
    let run_id = run_id_of(&run);
    state.handle(req(
        "run.stage_dispatch",
        json!({ "run_id": run_id, "stage_id": "second-half" }),
    ));
    let view = state.handle(req("run.get", json!({ "run_id": run_id })));
    let starts: Vec<&Value> = view["result"]["thread"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["data"]["event"] == "stage_started")
        .collect();

    assert_eq!(starts.len(), 2, "{starts:?}");
    assert_eq!(starts[0]["data"]["links"][0]["issue_id"], plan_id);
    assert_eq!(starts[0]["data"]["links"][0]["stage_id"], "first-half");
    assert_eq!(starts[1]["data"]["links"][0]["stage_id"], "second-half");
    for started in starts {
        assert!(started["data"]["links"]
            .as_array()
            .unwrap()
            .iter()
            .any(|link| link["kind"] == "implementation" && link["implementation_id"] == run_id));
    }
}

#[test]
fn mcp_thread_actions_are_owner_scoped_and_revision_snapshots_are_on_demand() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let mut active = crate::orchestrator::ActiveRun::reattach(
        &fake_run_record("run-thread"),
        ".build/plan.md".into(),
    );
    primary_thread_mut(&mut active.agents).post_user("rename it", None, now_rfc3339());
    let revision = primary_thread_mut(&mut active.agents).add_revision(
        crate::thread::ArtifactKind::Diff,
        "diff --git a/a b/a\n+new",
        &now_rfc3339(),
    );
    let project_id = state.project_at(0).id.clone();
    state.projects.bind_entity("run-thread".into(), project_id);
    state.runs.insert("run-thread".into(), active);

    let unread = state
        .on_mcp_action("run-thread", BridgeAction::ReadUnreadMessages)
        .unwrap();
    assert_eq!(unread["messages"][0]["body"], "rename it");
    assert!(state
        .on_mcp_action("run-thread", BridgeAction::ReadUnreadMessages)
        .unwrap()["messages"]
        .as_array()
        .unwrap()
        .is_empty());
    state
        .on_mcp_action(
            "run-thread",
            BridgeAction::PostThreadMessage {
                body: "Which name?".into(),
                anchor: None,
                links: Vec::new(),
                still_working: false,
                options: Vec::new(),
            },
        )
        .unwrap();
    let escaping_link = state.on_mcp_action(
        "run-thread",
        BridgeAction::PostThreadMessage {
            body: "Open this".into(),
            anchor: None,
            links: vec![crate::thread::ThreadLink::File {
                path: "../../etc/passwd".into(),
                line_start: None,
                line_end: None,
            }],
            still_working: false,
            options: Vec::new(),
        },
    );
    assert_eq!(
        escaping_link.unwrap_err(),
        "file link path escapes the worktree"
    );
    let view = state.handle(req("run.get", json!({ "run_id": "run-thread" })));
    assert!(view["result"]["thread"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["data"]["body"] == "Which name?"));
    assert!(view["result"]["thread"]["revisions"][0]
        .get("snapshot")
        .is_none());

    let historical = state.handle(req(
        "thread.revision",
        json!({ "entity_id": "run-thread", "revision_id": revision.id }),
    ));
    assert_eq!(historical["result"]["contents"], "diff --git a/a b/a\n+new");
    assert!(state
        .on_mcp_action("another-run", BridgeAction::ReadUnreadMessages)
        .is_err());
}

#[test]
fn mcp_typed_links_cannot_forge_another_issues_lineage() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_a, run_a) = planned_run_in_review(&mut state, "owner a");
    let (issue_b, run_b) = planned_run_in_review(&mut state, "owner b");
    let stage_b = state.plans[&issue_b].stages[0].clone();

    let forged_stage = state.on_mcp_action(
        &run_a,
        BridgeAction::PostThreadMessage {
            body: "look elsewhere".into(),
            anchor: None,
            links: vec![crate::thread::ThreadLink::PlanStage {
                plan_id: issue_b.clone(),
                stage_id: stage_b.id,
                path: stage_b.path,
            }],
            still_working: false,
            options: Vec::new(),
        },
    );
    assert_eq!(
        forged_stage.unwrap_err(),
        "plan stage link does not belong to this Issue"
    );
    let forged_run = state.on_mcp_action(
        &run_a,
        BridgeAction::PostThreadMessage {
            body: "open another implementation".into(),
            anchor: None,
            links: vec![crate::thread::ThreadLink::Run { run_id: run_b }],
            still_working: false,
            options: Vec::new(),
        },
    );
    assert_eq!(
        forged_run.unwrap_err(),
        "run link does not belong to this Issue"
    );
    assert!(state.plans.contains_key(&issue_a));
}

fn text_query(text: &str) -> crate::thread::ConversationQuery {
    crate::thread::ConversationQuery {
        text: Some(text.to_string()),
        ..crate::thread::ConversationQuery::default()
    }
}

fn agent_says(state: &mut AppState, entity_id: &str, body: &str) {
    state
        .on_mcp_action(
            entity_id,
            BridgeAction::PostThreadMessage {
                body: body.to_string(),
                anchor: None,
                links: Vec::new(),
                still_working: false,
                options: Vec::new(),
            },
        )
        .unwrap_or_else(|error| panic!("{entity_id} could not post: {error}"));
}

/// The scope rule: an agent's history is its own conversation plus the
/// Issue it is implementing. Another entity's conversation is not history
/// it lost — it is history it was never given.
#[test]
fn a_search_reads_this_agents_conversations_and_no_one_elses() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue_a, run_a) = planned_run_in_review(&mut state, "owner a");
    let (_issue_b, run_b) = planned_run_in_review(&mut state, "owner b");
    agent_says(&mut state, &run_a, "the ledger keeps its own clock");
    agent_says(&mut state, &run_b, "the cache is written twice");

    let own = state
        .on_mcp_action(
            &run_a,
            BridgeAction::SearchConversation {
                query: text_query("LEDGER"),
            },
        )
        .unwrap();
    assert_eq!(own["hits"].as_array().unwrap().len(), 1, "{own:?}");
    assert!(own["hits"][0]["excerpt"]
        .as_str()
        .unwrap()
        .contains("the ledger keeps its own clock"));
    assert_eq!(own["hits"][0]["role"], "agent");

    // The Issue's conversation is where a planned implementation's first
    // agent actually speaks, so it has to be in scope.
    let issue_thread = primary_thread(&state.plans[&issue_a].agents).id.clone();
    assert!(own["threads_searched"]
        .as_array()
        .unwrap()
        .contains(&json!(issue_thread)));

    let stranger = state
        .on_mcp_action(
            &run_a,
            BridgeAction::SearchConversation {
                query: text_query("cache"),
            },
        )
        .unwrap();
    assert!(
        stranger["hits"].as_array().unwrap().is_empty(),
        "another entity's conversation must not be readable: {stranger:?}"
    );
    assert!(state
        .on_mcp_action(
            "run-nobody-owns",
            BridgeAction::SearchConversation {
                query: text_query("cache"),
            },
        )
        .is_err());
}

/// A search is asked ABOUT a conversation, not asked to render it, so a
/// restart that loaded the tail of a long one must still answer out of the
/// whole of it. An agent that could only search what is resident has lost
/// exactly the old decisions the tool exists to look up.
#[test]
fn a_search_reaches_history_the_restart_never_loaded() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let held = run_with_long_conversation(&mut state, "run-history", 250);
    let active = state.runs.remove("run-history").expect("the run is there");
    state
        .persist_run_record("run-history", &active)
        .expect("the run saves");

    let mut restarted = qa_state(&repo, dir.path());
    assert!(
        primary_thread(&restarted.runs["run-history"].agents)
            .items
            .len()
            < held,
        "the restart loaded the conversation whole, so the search proves nothing"
    );

    // "turn 0" is the first thing ever said here, 249 items below the tail.
    let found = restarted
        .on_mcp_action(
            "run-history",
            BridgeAction::SearchConversation {
                query: text_query("turn 0"),
            },
        )
        .unwrap();
    assert_eq!(found["hits"].as_array().unwrap().len(), 1, "{found:?}");
    assert_eq!(found["hits"][0]["sequence"], 1, "{found:?}");
    assert!(found["hits"][0]["excerpt"]
        .as_str()
        .unwrap()
        .contains("turn 0"));
}

/// The whole point of the tool: a session with no memory asks what was
/// decided and gets back a pointer with the metadata that found it.
#[test]
fn a_search_finds_a_message_by_the_file_the_checkout_really_has() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (issue, run) = planned_run_in_review(&mut state, "index the notes");
    let checkout = state.runs[&run].worktree.path.clone();
    std::fs::write(checkout.join("notes.md"), "the notes").expect("a real file");

    let posted = state.handle(req(
        "thread.post",
        json!({
            "entity_id": issue,
            "body": "rewrite notes.md, and ignore imaginary.md and/or the rest",
        }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");

    let found = state
        .on_mcp_action(
            &run,
            BridgeAction::SearchConversation {
                query: crate::thread::ConversationQuery {
                    file: Some("notes.md".to_string()),
                    ..crate::thread::ConversationQuery::default()
                },
            },
        )
        .unwrap();
    let hits = found["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{found:?}");
    assert_eq!(hits[0]["role"], "user");
    assert_eq!(hits[0]["metadata"]["files"], json!(["notes.md"]));
}

#[test]
fn a_cursored_poll_reships_a_message_after_the_agent_marks_it_seen() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let mut active = crate::orchestrator::ActiveRun::reattach(
        &fake_run_record("run-seen"),
        ".build/plan.md".into(),
    );
    primary_thread_mut(&mut active.agents).post_user("rename it", None, now_rfc3339());
    let project_id = state.project_at(0).id.clone();
    state.projects.bind_entity("run-seen".into(), project_id);
    state.runs.insert("run-seen".into(), active);

    // The client holds the full thread: its cursor is the last sequence.
    let full = state.handle(req("run.get", json!({ "run_id": "run-seen" })));
    let full_items = full["result"]["thread"]["items"].as_array().unwrap();
    let cursor = full_items.last().unwrap()["data"]["sequence"]
        .as_u64()
        .unwrap();

    // The agent reads the message: an in-place mutation of an item the
    // client already holds. Pre-fix regression: the cursored poll skipped
    // it and the message rendered "Unread" forever.
    state
        .on_mcp_action("run-seen", BridgeAction::ReadUnreadMessages)
        .unwrap();
    let delta = state.handle(req(
        "run.get",
        json!({ "run_id": "run-seen", "thread_after_sequence": cursor }),
    ));
    let delta_thread = &delta["result"]["thread"];
    let reshipped = delta_thread["items"].as_array().unwrap();
    assert!(
        reshipped
            .iter()
            .any(|item| item["data"]["body"] == "rename it" && item["data"]["seen_at"].is_string()),
        "{delta_thread:?}"
    );
    // And the advanced high-water mark drains: the client does not loop.
    let advanced = delta_thread["thread_last_sequence"].as_u64().unwrap();
    assert!(advanced > cursor, "{delta_thread:?}");
    let drained = state.handle(req(
        "run.get",
        json!({ "run_id": "run-seen", "thread_after_sequence": advanced }),
    ));
    assert_eq!(
        drained["result"]["thread"]["items"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "{drained:?}"
    );
}
