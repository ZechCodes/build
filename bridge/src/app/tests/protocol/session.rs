use super::*;

/// Every tool-call row on this conversation — since step 12 that is the
/// whole of a call: the summary carries the answer when one arrived, and
/// the outcome says how it ended.
pub(in crate::app::tests) fn tool_call_rows(
    thread: &crate::thread::Thread,
) -> Vec<crate::thread::ThreadEvent> {
    thread
        .items
        .iter()
        .filter_map(|item| match item {
            crate::thread::ThreadItem::Event(event)
                if event.event == crate::thread::ThreadEventKind::ToolUse =>
            {
                Some(event.clone())
            }
            _ => None,
        })
        .collect()
}

/// The same conversation's tool-call rows as (summary, outcome), which is
/// what most of the assertions below are about.
pub(in crate::app::tests) fn tool_calls_of(
    thread: &crate::thread::Thread,
) -> Vec<(String, Option<crate::thread::ToolCallOutcome>)> {
    tool_call_rows(thread)
        .into_iter()
        .map(|event| (event.summary.unwrap_or_default(), event.outcome))
        .collect()
}

/// How many times this agent has reasoned out loud — the evidence a turn
/// was delivered and started, one per turn for the fakes below.
fn reasoning_count(state: &Arc<Mutex<AppState>>, run_id: &str) -> usize {
    let s = state.lock().unwrap();
    activity_of(primary_thread(&s.runs[run_id].agents))
        .iter()
        .filter(|(kind, _)| *kind == crate::thread::ThreadEventKind::Reasoning)
        .count()
}

/// What the conversation says about this agent's background work, in the
/// order it was said — the human's only window onto a headless agent's
/// tasks.
fn background_rows(state: &Arc<Mutex<AppState>>, run_id: &str) -> Vec<String> {
    let s = state.lock().unwrap();
    activity_of(primary_thread(&s.runs[run_id].agents))
        .into_iter()
        .filter(|(kind, _)| *kind == crate::thread::ThreadEventKind::TaskUpdate)
        .map(|(_, summary)| summary)
        .collect()
}

/// Age an agent's tab past the sweep's threshold on BOTH clocks the sweep
/// reads — how long since the child said anything, and how long since Build
/// handed it a turn — so the only thing left that can spare it is what its
/// own status says.
fn age_past_the_idle_threshold(state: &Arc<Mutex<AppState>>, key: &TabKey) {
    let mut s = state.lock().unwrap();
    s.tabs[key]
        .session
        .backdate_last_output(Duration::from_secs(600));
    s.tabs.get_mut(key).unwrap().last_delivered_at =
        Some(std::time::Instant::now() - Duration::from_secs(600));
}

/// The failure this step ends, from the sweep's side: a headless agent whose
/// turn closed while background work ran reported `Waiting`, so five quiet
/// minutes later the sweep demoted it to `idle_unreported` — an agent
/// mid-work explained as an anomaly.
///
/// No new sweep code: §11 q4's demotion rule already short-circuits on
/// `Working`, and a session holding a live task now IS working. The second
/// half is what proves the set clears rather than pinning `Working` forever
/// — the same session, quiet just as long, is demoted once its roster
/// empties.
#[tokio::test]
async fn the_idle_sweep_spares_an_agent_holding_a_background_task_and_no_other() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-background");
    let key = derived_agent_key(&root, "run-background");
    use crate::harness::adk::fake;
    // Turn one starts work that outlives it; turn two is answered by the
    // roster saying the work is over.
    run_on_a_headless_provider(
        &state,
        &repo,
        "run-background",
        fake::stream_json_harness_turn_by_turn(&[
            &[fake::TASK_STARTED, fake::RESULT],
            &[fake::TASK_ROSTER_EMPTY, fake::RESULT],
        ]),
    );

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-background", "body": "kick off the reindex" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    wait_for(Duration::from_secs(10), || {
        (background_rows(&state, "run-background").len() == 1).then_some(())
    })
    .await
    .expect("the task the turn started reaches the conversation");
    assert_eq!(
        background_rows(&state, "run-background"),
        vec![format!("{} — started", fake::TASK_DESCRIPTION)],
    );

    age_past_the_idle_threshold(&state, &key);
    assert!(
        state
            .lock()
            .unwrap()
            .mark_idle_tasks(Duration::from_secs(300))
            .is_empty(),
        "the turn is over and the work is not: silence here is not an anomaly"
    );
    let got = call(&handler, "run.get", json!({ "run_id": "run-background" }));
    assert_eq!(got["result"]["state"], "building", "{got:?}");

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-background", "body": "and now stop" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    wait_for(Duration::from_secs(10), || {
        (background_rows(&state, "run-background").len() == 2).then_some(())
    })
    .await
    .expect("the roster that drops the task closes it in the conversation");
    assert_eq!(
        background_rows(&state, "run-background")[1],
        format!("{} — finished", fake::TASK_DESCRIPTION),
    );

    age_past_the_idle_threshold(&state, &key);
    assert_eq!(
        state
            .lock()
            .unwrap()
            .mark_idle_tasks(Duration::from_secs(300)),
        vec!["run-background".to_string()],
        "with nothing live and nothing said, the sweep demotes as it always did"
    );
    let got = call(&handler, "run.get", json!({ "run_id": "run-background" }));
    assert_eq!(got["result"]["state"], "idle_unreported", "{got:?}");
}

/// The digest pair this step makes reachable on a headless provider, pinned:
/// `working: true` with `can_interrupt: false`.
///
/// It is legal and always was — the PTY has reported it since the field
/// landed — because an interrupt stops a TURN and background work is not
/// one. The shipped SPA gates the control on `working && can_interrupt`, so
/// what the composer offers here is the plain Send.
#[tokio::test]
async fn an_agent_working_only_a_background_task_offers_no_interrupt() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-tasks-only");
    use crate::harness::adk::fake;
    run_on_a_headless_provider(
        &state,
        &repo,
        "run-tasks-only",
        fake::stream_json_harness(&[fake::TASK_STARTED, fake::RESULT]),
    );

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-tasks-only", "body": "kick off the reindex" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    wait_for(Duration::from_secs(10), || {
        (background_rows(&state, "run-tasks-only").len() == 1).then_some(())
    })
    .await
    .expect("the task reaches the conversation");

    // The result closed the turn behind the task line, so what the digest
    // reports as working can only be the task.
    let bubble = wait_for(Duration::from_secs(10), || {
        let listed = call(
            &handler,
            "agent.list",
            json!({ "entity_id": "run-tasks-only" }),
        );
        let bubble = listed["result"]["agents"][0].clone();
        (bubble["can_interrupt"] == false).then_some(bubble)
    })
    .await
    .expect("the turn closes and the control goes with it");
    assert_eq!(
        bubble["working"], true,
        "the work outlived the turn, and the rail dot keeps pulsing: {bubble:?}"
    );
    assert_eq!(
        bubble["has_terminal"], false,
        "this session has no basement to fall back on: {bubble:?}"
    );
}

/// `Ended` wins over a live roster: `status()` reads the exit code before
/// the live state, so a child that dies with work outstanding is over.
///
/// And the death rites are untouched — the tab stops reading as live and the
/// conversation's session lineage closes. Nothing waits on, drains or mourns
/// the tasks: they died with the child that was running them.
#[tokio::test]
async fn a_child_that_exits_with_a_task_live_is_ended_and_gets_the_usual_rites() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-outlived");
    let key = derived_agent_key(&root, "run-outlived");
    use crate::harness::adk::fake;
    // It starts the work, never answers the turn, and leaves.
    run_on_a_headless_provider(
        &state,
        &repo,
        "run-outlived",
        fake::stream_json_harness_that_leaves(&[fake::TASK_STARTED]),
    );

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-outlived", "body": "kick off the reindex" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    // The delivery is made off the frame that queued it, so the session
    // opens after the reply: wait for the work it started to reach the
    // timeline before asking whether the session that started it is over.
    wait_for(Duration::from_secs(10), || {
        (background_rows(&state, "run-outlived").len() == 1).then_some(())
    })
    .await
    .expect("the agent started the work it was sent");
    wait_for(Duration::from_secs(10), || {
        (open_session_count(&state, "run-outlived") == 0).then_some(())
    })
    .await
    .expect("the stream closing ends the session Build opened");

    assert_eq!(
        background_rows(&state, "run-outlived"),
        vec![format!("{} — started", fake::TASK_DESCRIPTION)],
        "the work it started is in the timeline, and nothing closes it for it"
    );
    let s = state.lock().unwrap();
    assert!(
        matches!(
            s.tabs[&key].session.status(),
            AgentStatus::Ended { code: Some(_) }
        ),
        "a roster outstanding does not keep a dead child working"
    );
    assert!(!s.tabs[&key].live, "the tab stops reading as live");
}

/// Everything the conversation says happened, so a test can assert what did
/// NOT happen over the whole vocabulary rather than a sample of it.
fn event_kinds(state: &Arc<Mutex<AppState>>, run_id: &str) -> Vec<crate::thread::ThreadEventKind> {
    let s = state.lock().unwrap();
    primary_thread(&s.runs[run_id].agents)
        .items
        .iter()
        .filter_map(|item| match item {
            crate::thread::ThreadItem::Event(event) => Some(event.event),
            _ => None,
        })
        .collect()
}

/// A live headless agent mid-turn, on `spec`, with one turn already
/// delivered and running.
///
/// The fakes below never emit a `result`, so the turn stays open and the
/// session keeps reporting `Working` — which is the state an interrupt is
/// for and the only one the composer offers it in.
async fn a_headless_agent_mid_turn(
    state: &Arc<Mutex<AppState>>,
    handler: &FrameHandler,
    run_id: &str,
    spec: HarnessSpec,
    repo: &std::path::Path,
) {
    run_on_a_headless_provider(state, repo, run_id, spec);
    let posted = call(
        handler,
        "thread.post",
        json!({ "entity_id": run_id, "body": "start on the index" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    wait_for(Duration::from_secs(10), || {
        (reasoning_count(state, run_id) == 1).then_some(())
    })
    .await
    .expect("the first turn reaches the agent and it starts working");
}

/// The steering flow, end to end: a message that stops the turn it lands in.
///
/// The wire is `thread.post` with `interrupt: true` and there is no
/// `agent.interrupt` verb — Build never interrupts without a turn to follow,
/// so a verb of its own would always be followed by this post a moment
/// later, with a window between them in which the child starts a fresh turn
/// or the agent calls `done`.
///
/// And status moves by exactly one step: the human's message. Nothing else
/// is minted — an interrupted turn's `error_during_execution` result is a
/// turn boundary, never a report, and the only path by which its text could
/// have reached a human was the epitaph the session clears.
#[tokio::test]
async fn a_post_that_interrupts_stops_the_turn_and_hands_over_the_message() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-steer");
    use crate::harness::adk::fake;
    // Everything Build says to this agent is kept, because the steering
    // flow IS two writes in one order and what comes back cannot tell them
    // apart from an ordinary send.
    let heard = dir.path().join("heard.jsonl");
    a_headless_agent_mid_turn(
        &state,
        &handler,
        "run-steer",
        fake::stream_json_harness_recording_stdin(&[fake::THINKING], &heard),
        &repo,
    )
    .await;

    let bubble = call(&handler, "agent.list", json!({ "entity_id": "run-steer" }))["result"]
        ["agents"][0]
        .clone();
    assert_eq!(bubble["working"], true, "{bubble:?}");
    assert_eq!(
        bubble["can_interrupt"], true,
        "the child announced an interrupt in its init line: {bubble:?}"
    );

    let steered = call(
        &handler,
        "thread.post",
        json!({
            "entity_id": "run-steer",
            "body": "stop — just the trigger",
            "interrupt": true,
        }),
    );
    assert_eq!(steered["ok"], true, "{steered:?}");

    // The child acks, closes the stopped turn with `error_during_execution`,
    // and only then reads the steering turn — so a second reasoning event
    // can only mean the message was handed over behind the interrupt.
    wait_for(Duration::from_secs(10), || {
        (reasoning_count(&state, "run-steer") == 2).then_some(())
    })
    .await
    .expect("the message the interrupt cleared the way for is delivered");

    // The wire, in order: the child heard the first turn, then a
    // `control_request` to stop it, and only then the turn that replaces it.
    let said: Vec<Value> = std::fs::read_to_string(&heard)
        .expect("the child kept what it heard")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a protocol line"))
        .collect();
    let kinds: Vec<&str> = said
        .iter()
        .map(|line| line["type"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        kinds,
        vec!["user", "control_request", "user"],
        "stop first, then hand over — the order the flag on the message exists to hold: {said:?}"
    );
    assert_eq!(said[1]["request"]["subtype"], "interrupt", "{said:?}");

    let s = state.lock().unwrap();
    let said: Vec<String> = primary_thread(&s.runs["run-steer"].agents)
        .items
        .iter()
        .filter_map(|item| match item {
            crate::thread::ThreadItem::Message(message) => {
                assert_eq!(
                    message.outcome, None,
                    "nothing here is an outcome: {message:?}"
                );
                assert!(
                    !message.done,
                    "and nothing here is a completion: {message:?}"
                );
                Some(message.body.clone())
            }
            _ => None,
        })
        .collect();
    assert!(
        said.iter()
            .any(|body| body.contains("stop — just the trigger")),
        "the human's own message is the record of why the turn stopped: {said:?}"
    );
    assert_eq!(
        s.runs["run-steer"].run.state,
        RunState::Building,
        "an interrupt moves no state of any kind"
    );
    drop(s);

    use crate::thread::ThreadEventKind;
    let happened = event_kinds(&state, "run-steer");
    for never in [
        ThreadEventKind::Blocked,
        ThreadEventKind::RunFailed,
        // `Interrupted` means a session that is GONE. This one is the same
        // session, holding the same conversation, and it is about to answer.
        ThreadEventKind::Interrupted,
        ThreadEventKind::SessionEnded,
    ] {
        assert!(
            !happened.contains(&never),
            "a stopped turn is not a failure: {never:?} in {happened:?}"
        );
    }
}

/// A refused interrupt does not fail the post.
///
/// Where the session cannot stop a turn — a CLI built before the capability
/// landed, or one lost between the digest the client read and the post it
/// sent — the message is delivered as an ordinary queued turn, which the
/// probes verified reaches the running turn at its next step boundary
/// anyway. The alternative is an error the human must read for a difference
/// they cannot act on and did not cause.
#[tokio::test]
async fn an_interrupt_the_io_refuses_still_hands_over_the_message() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-refuses");
    use crate::harness::adk::fake;
    a_headless_agent_mid_turn(
        &state,
        &handler,
        "run-refuses",
        fake::stream_json_harness_without_interrupt(&[fake::THINKING]),
        &repo,
    )
    .await;

    let bubble = call(
        &handler,
        "agent.list",
        json!({ "entity_id": "run-refuses" }),
    )["result"]["agents"][0]
        .clone();
    assert_eq!(
        bubble["can_interrupt"], false,
        "this child announced none, so the composer never offers the control: {bubble:?}"
    );

    let steered = call(
        &handler,
        "thread.post",
        json!({
            "entity_id": "run-refuses",
            "body": "stop — just the trigger",
            "interrupt": true,
        }),
    );
    assert_eq!(
        steered["ok"], true,
        "a capability the harness lacks is not the human's mistake: {steered:?}"
    );
    wait_for(Duration::from_secs(10), || {
        (reasoning_count(&state, "run-refuses") == 2).then_some(())
    })
    .await
    .expect("the message is delivered as an ordinary queued turn");
}

/// Put `run_id` on the headless provider running `spec`, and keep every
/// [`SpawnOptions`] the daemon built a spawn from.
///
/// What a respawn picks back up is decided there and nowhere else, so the
/// recording is where a test reads the daemon's answer without a real
/// claude on the other end of it.
fn run_on_a_headless_provider_recording_spawns(
    state: &Arc<Mutex<AppState>>,
    repo: &std::path::Path,
    run_id: &str,
    spec: HarnessSpec,
) -> Arc<Mutex<Vec<SpawnOptions>>> {
    let spawns: Arc<Mutex<Vec<SpawnOptions>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&spawns);
    let choice = {
        let mut s = state.lock().unwrap();
        let worktrees = s.worktrees_root.clone();
        let agent = Agent::WarmBuilder(Arc::new(
            move |_prompt: &str, _choice: &ModelChoice, options: &SpawnOptions| {
                recorded.lock().unwrap().push(options.clone());
                Ok(spec.clone())
            },
        ));
        s.project_at_mut(0).orch = Orchestrator::new(
            repo.to_path_buf(),
            worktrees,
            agent,
            Templates::default(),
            test_bridge_exe(),
        );
        ModelChoice {
            provider: AgentProvider::ClaudeAdk,
            ..ModelChoice::default()
        }
    };
    let mut s = state.lock().unwrap();
    // These tests are about what the pump does with a name AFTER the spawn
    // spends it, so the reservation's verification says yes: the tree holds
    // what the record claims, and the session still ends the way it ends.
    s.resume_id_probe = Arc::new(|_, _, _| true);
    let run = s.runs.get_mut(run_id).expect("the run");
    run.model_choice = choice.clone();
    run.agents.resolve_mut(None).expect("its agent").choice = choice;
    spawns
}

/// The argv the headless provider builds from one recorded spawn.
fn headless_argv(options: &SpawnOptions) -> String {
    use crate::harness::Harness;
    crate::harness::adk::AdkHarness
        .spec(
            &ModelChoice {
                provider: AgentProvider::ClaudeAdk,
                ..ModelChoice::default()
            },
            options,
            &crate::harness::HarnessContext {
                bridge_exe: std::path::PathBuf::from("/usr/local/bin/build-bridge"),
                mcp_socket: std::path::PathBuf::from("/tmp/build-mcp.sock"),
                state_root: std::path::PathBuf::from("/tmp/build-state"),
            },
        )
        .unwrap()
        .args
        .join(" ")
}

fn seed_exact_resumable_session(
    state: &Arc<Mutex<AppState>>,
    owner: &str,
    root: &std::path::Path,
    choice: &ModelChoice,
    resume_id: &str,
) {
    let mut app = state.lock().unwrap();
    let agent_id = app
        .entity_agents(owner)
        .unwrap()
        .primary()
        .expect("the resumable agent")
        .id
        .clone();
    let instance = app
        .record_agent_session_start(owner, &agent_id, root, choice, "build")
        .expect("the exact prior session");
    app.note_self_report(
        owner,
        &agent_id,
        &instance,
        SelfReport {
            named: Some(resume_id.to_string()),
            model: None,
        },
    );
    app.record_agent_session_end(owner, &agent_id, &instance);
}

/// A respawn resumes the conversation the last session NAMED.
///
/// The child announces its session id in its own `init` line, the activity
/// pump writes it onto the agent's record, and the next spawn carries
/// `--resume <id>` instead of the transcript probe's `--continue` — which
/// names the newest conversation in the checkout and not necessarily the
/// one Build was speaking to.
#[tokio::test]
async fn a_respawn_resumes_the_conversation_the_last_session_named() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-resume");
    let agent_id = crate::agent::derived_agent_id("run-resume");
    use crate::harness::adk::fake;
    // It answers its one turn and leaves, the way a real one does when its
    // work is over — which is what makes the next message a respawn.
    let spawns = run_on_a_headless_provider_recording_spawns(
        &state,
        &repo,
        "run-resume",
        fake::stream_json_harness_that_leaves(&[fake::NARRATION, fake::RESULT]),
    );

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-resume", "body": "drop the index" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    let named = wait_for(Duration::from_secs(10), || {
        let s = state.lock().unwrap();
        s.recorded_resume_id("run-resume", &agent_id)
    })
    .await
    .expect("the name the child gave its conversation reaches the agent's record");
    assert_eq!(named, "sess-adk");

    let again = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-resume", "body": "and the trigger" }),
    );
    assert_eq!(again["ok"], true, "{again:?}");
    let spawned = wait_for(Duration::from_secs(10), || {
        let spawns = spawns.lock().unwrap();
        (spawns.len() == 2).then(|| spawns.clone())
    })
    .await
    .expect("a message to an agent that has left starts it again");

    assert_eq!(
        spawned[0].resume_session_id, None,
        "the first spawn had no conversation to name: {:?}",
        spawned[0]
    );
    assert_eq!(
        spawned[1].resume_session_id.as_deref(),
        Some("sess-adk"),
        "and the second carries the one the first announced: {:?}",
        spawned[1]
    );
    let argv = headless_argv(&spawned[1]);
    assert!(argv.contains("--resume sess-adk"), "{argv}");
    assert!(
        !argv.contains("--continue"),
        "the name and the cwd guess are alternatives, never both: {argv}"
    );
}

/// One dead id costs one restart, not every restart.
///
/// A session spawned with a `--resume` id that no longer resolves exits
/// without ever announcing itself. So a session that ends having announced
/// nothing clears the record: the next spawn falls back to the transcript
/// probe, which is the path that shipped before any of this and still
/// answers. The same clearing covers a child that died at startup for an
/// unrelated reason, where the fallback is what would have run anyway.
#[tokio::test]
async fn a_session_that_never_announced_clears_the_name_it_was_spawned_with() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-stale");
    let agent_id = crate::agent::derived_agent_id("run-stale");
    // A child that leaves without a word — the shape of one handed an id
    // its harness cannot find.
    let spawns = run_on_a_headless_provider_recording_spawns(
        &state,
        &repo,
        "run-stale",
        HarnessSpec::new("sh").arg("-c").arg("exit 1"),
    );
    seed_exact_resumable_session(
        &state,
        "run-stale",
        &root,
        &ModelChoice {
            provider: AgentProvider::ClaudeAdk,
            ..ModelChoice::default()
        },
        "sess-gone",
    );

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-stale", "body": "are you still there" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    let spawned = wait_for(Duration::from_secs(10), || {
        spawns.lock().unwrap().first().cloned()
    })
    .await
    .expect("the message starts the agent");
    assert_eq!(
        spawned.resume_session_id.as_deref(),
        Some("sess-gone"),
        "the spawn carried the recorded name — which is how it becomes a dead spawn"
    );

    wait_for(Duration::from_secs(10), || {
        let s = state.lock().unwrap();
        s.recorded_resume_id("run-stale", &agent_id)
            .is_none()
            .then_some(())
    })
    .await
    .expect("a session that announced nothing takes the name it was spawned with with it");
}

/// History alone is not exact lineage and cannot authorize a resume.
#[tokio::test]
async fn agent_history_without_an_exact_name_and_checkout_starts_fresh() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("lived"), "run-lived");
    let mut s = state.lock().unwrap();
    let agent_id = s.runs["run-lived"].agents.primary().unwrap().id.clone();
    primary_thread_mut(&mut s.runs.get_mut("run-lived").expect("the run").agents).start_session(
        "claude",
        None,
        None,
        "implementation",
        "2026-08-29T00:00:00Z",
    );
    assert!(
        s.resumable_session_id("run-lived", &agent_id, &root, AgentProvider::Claude)
            .is_none(),
        "a session count cannot substitute for an exact provider id and checkout"
    );
}

/// The spawn rule has two safe outcomes: exact persisted lineage resumes;
/// every missing-lineage case starts fresh.
#[tokio::test]
async fn a_spawn_resumes_exact_lineage_and_never_guesses_from_history() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let named_root = insert_run_without_agent(&state, &repo, dir.path().join("named"), "run-named");
    let history_root =
        insert_run_without_agent(&state, &repo, dir.path().join("history"), "run-history");
    let fresh_root = insert_run_without_agent(&state, &repo, dir.path().join("fresh"), "run-fresh");

    let specs_built: Arc<Mutex<Vec<SpawnOptions>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let recorder = Arc::clone(&specs_built);
        let agent = Agent::WarmBuilder(Arc::new(
            move |_prompt: &str, _choice: &ModelChoice, options: &SpawnOptions| {
                recorder.lock().unwrap().push(options.clone());
                Ok(warm_tui_spec())
            },
        ));
        let mut s = state.lock().unwrap();
        let worktrees = s.worktrees_root.clone();
        s.resume_id_probe = Arc::new(|_, _, id| id == "sess-named");
        s.project_at_mut(0).orch = Orchestrator::new(
            repo.clone(),
            worktrees,
            agent,
            Templates::default(),
            test_bridge_exe(),
        );
        // 1. A name Build wrote on this agent's exact checkout lineage.
        let named_agent = s.runs["run-named"].agents.primary().unwrap().id.clone();
        let instance = s
            .record_agent_session_start(
                "run-named",
                &named_agent,
                &named_root,
                &ModelChoice::default(),
                "build",
            )
            .unwrap();
        s.edit_agent_conversation("run-named", &named_agent, |thread, _artifact| {
            assert!(thread.name_session_instance(&instance, "sess-named"));
            Ok(())
        })
        .unwrap();
        s.record_agent_resume_id("run-named", &named_agent, Some("sess-named".to_string()));
        s.record_agent_session_end("run-named", &named_agent, &instance);
        // 2. No name, but a session of its own has opened before.
        primary_thread_mut(&mut s.runs.get_mut("run-history").expect("the run").agents)
            .start_session(
                "claude",
                None,
                None,
                "implementation",
                "2026-08-29T00:00:00Z",
            );
        // 3. run-fresh is left exactly as it was minted.
    }

    for run_id in ["run-named", "run-history", "run-fresh"] {
        let posted = call(
            &handler,
            "thread.post",
            json!({ "entity_id": run_id, "body": "pick this up" }),
        );
        assert_eq!(posted["ok"], true, "{posted:?}");
    }

    wait_for_deliveries(&state).await;
    let built = specs_built.lock().unwrap().clone();
    let spawned_in = |root: &std::path::Path| {
        let root = AppState::canonical_root(root);
        built
            .iter()
            .find(|options| options.cwd == root)
            .unwrap_or_else(|| panic!("the message spawned an agent in {}", root.display()))
            .clone()
    };

    let named = spawned_in(&named_root);
    assert_eq!(
        named.resume_session_id.as_deref(),
        Some("sess-named"),
        "a verified name is resumed exactly: {named:?}"
    );
    assert!(
        !named.continue_session,
        "and never with the cwd guess beside it: {named:?}"
    );

    let history = spawned_in(&history_root);
    assert_eq!(
        history.resume_session_id, None,
        "there was no name to spend: {history:?}"
    );
    assert!(
        !history.continue_session,
        "history without exact lineage starts fresh: {history:?}"
    );

    let fresh = spawned_in(&fresh_root);
    assert_eq!(
        fresh.resume_session_id, None,
        "a brand-new agent record names nothing: {fresh:?}"
    );
    assert!(
        !fresh.continue_session,
        "and inherits nothing — the checkout's old conversation belongs to \
         whoever had it: {fresh:?}"
    );
}

/// A locator that has found the name the harness wrote down, or has not
/// yet — the two answers a real one gives, without a real transcript tree.
struct LocatorThatFound(Option<&'static str>);

impl crate::harness::SessionLocator for LocatorThatFound {
    fn session_id(&self) -> Option<String> {
        self.0.map(str::to_string)
    }
}

/// Give every terminal this daemon opens a locator with this answer.
fn every_terminal_names_its_conversation(
    state: &Arc<Mutex<AppState>>,
    named: Option<&'static str>,
) {
    state.lock().unwrap().session_locator_factory =
        Arc::new(move |_, _| Some(Box::new(LocatorThatFound(named))));
}

/// A fresh terminal locator is not identity. Two sessions starting in one
/// checkout can both observe the transcript written first and would then
/// alias one provider conversation if the sweep trusted that observation.
#[tokio::test]
async fn a_terminal_locator_never_authorizes_a_fresh_sessions_resume_identity() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-pty");
    let agent_id = crate::agent::derived_agent_id("run-pty");
    every_terminal_names_its_conversation(&state, Some("sess-pty"));

    let specs_built: Arc<Mutex<Vec<SpawnOptions>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let recorder = Arc::clone(&specs_built);
        let agent = Agent::WarmBuilder(Arc::new(
            move |_prompt: &str, _choice: &ModelChoice, options: &SpawnOptions| {
                recorder.lock().unwrap().push(options.clone());
                Ok(warm_tui_spec())
            },
        ));
        let mut s = state.lock().unwrap();
        let worktrees = s.worktrees_root.clone();
        s.resume_id_probe = Arc::new(|_, _, id| id == "sess-pty");
        s.project_at_mut(0).orch = Orchestrator::new(
            repo.clone(),
            worktrees,
            agent,
            Templates::default(),
            test_bridge_exe(),
        );
    }

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-pty", "body": "start something" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    wait_for_deliveries(&state).await;
    assert_eq!(
        state
            .lock()
            .unwrap()
            .recorded_resume_id("run-pty", &agent_id),
        None,
        "nothing has looked yet"
    );

    capture_conversation_names(&state);
    assert_eq!(
        state
            .lock()
            .unwrap()
            .recorded_resume_id("run-pty", &agent_id),
        None,
        "a cwd/new-file locator cannot identify one of concurrent sessions"
    );

    // The harness dies. The next message is a respawn, and it opens on the
    // conversation by name.
    let key = derived_agent_key(&root, "run-pty");
    state
        .lock()
        .unwrap()
        .tabs
        .get(&key)
        .expect("the agent tab")
        .session
        .end();
    wait_for(Duration::from_secs(10), || {
        let s = state.lock().unwrap();
        (!s.tabs.get(&key).expect("the agent tab").live).then_some(())
    })
    .await
    .expect("the pump notices the harness left");

    let again = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-pty", "body": "and again" }),
    );
    assert_eq!(again["ok"], true, "{again:?}");
    let spawned = wait_for(Duration::from_secs(10), || {
        let built = specs_built.lock().unwrap();
        (built.len() == 2).then(|| built[1].clone())
    })
    .await
    .expect("the message starts the agent again");
    assert_eq!(
        spawned.resume_session_id, None,
        "without an exact identity the respawn starts fresh: {spawned:?}"
    );
    assert!(
        !spawned.continue_session,
        "and never the cwd guess beside it: {spawned:?}"
    );
    assert!(!spawned.continue_session, "{spawned:?}");
}

/// A short-lived fresh terminal cannot turn a locator observation into an
/// exact identity on its close path either.
#[tokio::test]
async fn a_fresh_terminal_close_does_not_persist_a_locator_guess() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-brief");
    let agent_id = crate::agent::derived_agent_id("run-brief");
    every_terminal_names_its_conversation(&state, Some("sess-brief"));
    {
        let agent = Agent::WarmBuilder(Arc::new(
            |_prompt: &str, _choice: &ModelChoice, _options: &SpawnOptions| {
                // Announces its line editor, takes the turn, and leaves.
                Ok(HarnessSpec::new("sh")
                    .arg("-c")
                    .arg("printf '\\033[?2004h'; exit 0"))
            },
        ));
        let mut s = state.lock().unwrap();
        let worktrees = s.worktrees_root.clone();
        s.project_at_mut(0).orch = Orchestrator::new(
            repo.clone(),
            worktrees,
            agent,
            Templates::default(),
            test_bridge_exe(),
        );
    }

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-brief", "body": "one quick thing" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    let key = derived_agent_key(&root, "run-brief");
    wait_for(Duration::from_secs(10), || {
        let s = state.lock().unwrap();
        (!s.tabs.get(&key)?.live).then_some(())
    })
    .await
    .expect("the harness leaves on its own");

    assert_eq!(
        state
            .lock()
            .unwrap()
            .recorded_resume_id("run-brief", &agent_id),
        None,
        "the close arm cannot promote a locator race into resume identity"
    );
}

#[tokio::test]
async fn a_spawn_writes_down_the_model_it_spent() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    on_the_terminal_provider(&state);
    insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-spent");
    insert_run_without_agent(
        &state,
        &repo,
        dir.path().join("other"),
        "run-harness-default",
    );
    {
        let agent = Agent::WarmBuilder(Arc::new(
            |_prompt: &str, _choice: &ModelChoice, _options: &SpawnOptions| Ok(warm_tui_spec()),
        ));
        let mut s = state.lock().unwrap();
        let worktrees = s.worktrees_root.clone();
        s.project_at_mut(0).orch = Orchestrator::new(
            repo.clone(),
            worktrees,
            agent,
            Templates::default(),
            test_bridge_exe(),
        );
    }

    let chosen = call(
        &handler,
        "agent.choose",
        json!({ "entity_id": "run-spent", "model": "claude-opus-5" }),
    );
    assert_eq!(chosen["ok"], true, "{chosen:?}");
    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-spent", "body": "start something" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    assert_eq!(
        state
            .lock()
            .unwrap()
            .agent_digests("run-spent", DigestScope::List)[0]["active_model"],
        "claude-opus-5",
        "a session that announces nothing still says what Build handed it"
    );

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-harness-default", "body": "start something" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    assert_eq!(
        state
            .lock()
            .unwrap()
            .agent_digests("run-harness-default", DigestScope::List)[0]["active_model"],
        "",
        "a spawn left at the harness default knows nothing rather than lying"
    );
}

#[tokio::test]
async fn one_sweep_tick_writes_down_the_model_a_session_announced() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let agent_id = crate::agent::derived_agent_id("run-announced");
    {
        let mut s = state.lock().unwrap();
        let root = insert_run(
            &mut s,
            &repo,
            dir.path(),
            "run-announced",
            RunState::Building,
        );
        insert_dictated_agent_tab(
            &mut s,
            &root,
            "run-announced",
            DictatedSession::reporting(AgentStatus::Waiting).announcing_model("claude-fable-5-1"),
        );
    }

    capture_conversation_names(&state);
    assert_eq!(
        state
            .lock()
            .unwrap()
            .recorded_active_model("run-announced", &agent_id)
            .as_deref(),
        Some("claude-fable-5-1"),
        "one tick puts the model the child announced on the agent's record"
    );

    state.lock().unwrap().changes().flush();
    capture_conversation_names(&state);
    assert!(
        !state.lock().unwrap().changes().has_pending(),
        "a tick with nothing moved writes nothing"
    );
}

#[tokio::test]
async fn a_session_that_ends_keeps_the_model_it_last_ran() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-outlives");
    let agent_id = crate::agent::derived_agent_id("run-outlives");
    run_on_a_headless_provider(
        &state,
        &repo,
        "run-outlives",
        HarnessSpec::new("sh").arg("-c").arg("exit 0"),
    );
    {
        let mut s = state.lock().unwrap();
        s.resume_id_probe = Arc::new(|_, _, _| true);
        let run = s.runs.get_mut("run-outlives").expect("the run");
        let agent = run.agents.resolve_mut(None).expect("its agent");
        agent.choice.model = Some("claude-opus-5".to_string());
        agent.resume_session_id = Some("sess-gone".to_string());
    }

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-outlives", "body": "carry on" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    let key = derived_agent_key(&root, "run-outlives");
    wait_for(Duration::from_secs(10), || {
        let s = state.lock().unwrap();
        (!s.tabs.get(&key)?.live).then_some(())
    })
    .await
    .expect("the child leaves on its own");
    wait_for(Duration::from_secs(10), || {
        let s = state.lock().unwrap();
        s.recorded_resume_id("run-outlives", &agent_id)
            .is_none()
            .then_some(())
    })
    .await
    .expect("a session that announced nothing sends the next spawn back to the probe");

    assert_eq!(
        state
            .lock()
            .unwrap()
            .recorded_active_model("run-outlives", &agent_id)
            .as_deref(),
        Some("claude-opus-5"),
        "what an agent last ran on outlives the session that ran it"
    );
}

/// The close arm RECORDS; it never clears.
///
/// The headless pump clears on a session that ended having announced
/// nothing, because for a session protocol it means a dead `--resume` id. A
/// terminal resumed in place legitimately writes no new transcript, so its
/// locator finding nothing is the normal answer — and clearing on it would
/// throw a good name away at every restart. The dead-name problem is
/// answered at the reservation instead.
#[tokio::test]
async fn a_terminal_that_named_nothing_keeps_the_name_its_record_already_had() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-quiet");
    let agent_id = crate::agent::derived_agent_id("run-quiet");
    every_terminal_names_its_conversation(&state, None);
    {
        let agent = Agent::WarmBuilder(Arc::new(
            |_prompt: &str, _choice: &ModelChoice, _options: &SpawnOptions| {
                Ok(HarnessSpec::new("sh")
                    .arg("-c")
                    .arg("printf '\\033[?2004h'; exit 0"))
            },
        ));
        let mut s = state.lock().unwrap();
        let worktrees = s.worktrees_root.clone();
        s.resume_id_probe = Arc::new(|_, _, _| true);
        s.project_at_mut(0).orch = Orchestrator::new(
            repo.clone(),
            worktrees,
            agent,
            Templates::default(),
            test_bridge_exe(),
        );
    }
    seed_exact_resumable_session(
        &state,
        "run-quiet",
        &root,
        &ModelChoice::default(),
        "sess-resumed-in-place",
    );

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-quiet", "body": "carry on" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    let key = derived_agent_key(&root, "run-quiet");
    wait_for(Duration::from_secs(10), || {
        let s = state.lock().unwrap();
        (!s.tabs.get(&key)?.live).then_some(())
    })
    .await
    .expect("the harness leaves on its own");

    capture_conversation_names(&state);
    assert_eq!(
        state
            .lock()
            .unwrap()
            .recorded_resume_id("run-quiet", &agent_id)
            .as_deref(),
        Some("sess-resumed-in-place"),
        "a terminal that wrote no new transcript keeps the conversation it resumed"
    );
}

/// Record every spawn this daemon's orchestrator builds, over a checkout
/// that already holds somebody's transcript — the setup both adoption
/// tests need to read what the spawn rule decided.
fn spawns_over_an_old_transcript(
    state: &Arc<Mutex<AppState>>,
    repo: &std::path::Path,
) -> Arc<Mutex<Vec<SpawnOptions>>> {
    let specs_built: Arc<Mutex<Vec<SpawnOptions>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&specs_built);
    let agent = Agent::WarmBuilder(Arc::new(
        move |_prompt: &str, _choice: &ModelChoice, options: &SpawnOptions| {
            recorder.lock().unwrap().push(options.clone());
            Ok(warm_tui_spec())
        },
    ));
    let mut s = state.lock().unwrap();
    let worktrees = s.worktrees_root.clone();
    s.project_at_mut(0).orch = Orchestrator::new(
        repo.to_path_buf(),
        worktrees,
        agent,
        Templates::default(),
        test_bridge_exe(),
    );
    drop(s);
    specs_built
}

/// Adoption inherits nothing, walked end to end.
///
/// The conversation the human was having in the checkout they adopted is one
/// Build never heard: its conversation view for this agent starts at
/// sequence 1, so a session resumed under it would show an agent answering
/// messages that are nowhere on screen. A new agent is a new conversation,
/// on an adopted branch like anywhere else.
#[tokio::test]
async fn an_adopted_entitys_first_spawn_starts_a_conversation_of_its_own() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-adoptee");
    let specs_built = spawns_over_an_old_transcript(&state, &repo);
    // What both adoption mints write down.
    state
        .lock()
        .unwrap()
        .runs
        .get_mut("run-adoptee")
        .expect("the run")
        .adopted = true;

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-adoptee", "body": "have a look at this" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");

    wait_for_deliveries(&state).await;
    let spawned = specs_built
        .lock()
        .unwrap()
        .first()
        .cloned()
        .expect("the message starts the agent");
    assert_eq!(spawned.cwd, AppState::canonical_root(&root));
    assert!(
        !spawned.continue_session,
        "the first agent on an adopted branch inherits no conversation: {spawned:?}"
    );
    assert_eq!(
        spawned.resume_session_id, None,
        "and names none either: {spawned:?}"
    );
}

/// The live incoherence this rule ends: an adopted branch whose agents were
/// all removed and one added back inherited an entire prior session.
///
/// Removing the roster took the session lineage with it, so the branch read
/// as freshly adopted again and the pickup fired a second time. The agent
/// the human then talked to answered out of a history no view of Build's
/// holds. A record with no sessions of its own is a fresh conversation,
/// whatever the entity has been through.
#[tokio::test]
async fn an_agent_added_back_to_an_old_adopted_branch_starts_fresh() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-readopted");
    let specs_built = spawns_over_an_old_transcript(&state, &repo);
    let removable = {
        let mut s = state.lock().unwrap();
        let run = s.runs.get_mut("run-readopted").expect("the run");
        run.adopted = true;
        // The branch has been worked in: a session of Build's own opened on
        // it after the adoption.
        primary_thread_mut(&mut run.agents).start_session(
            "claude",
            None,
            None,
            "implementation",
            "2026-08-30T00:00:00Z",
        );
        run.agents
            .agents()
            .iter()
            .map(|agent| agent.id.clone())
            .collect::<Vec<_>>()
    };
    for agent_id in removable {
        let removed = call(
            &handler,
            "agent.remove",
            json!({ "entity_id": "run-readopted", "agent_id": agent_id }),
        );
        assert_eq!(removed["ok"], true, "{removed:?}");
    }

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-readopted", "body": "start over" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");

    wait_for_deliveries(&state).await;
    let spawned = specs_built
        .lock()
        .unwrap()
        .first()
        .cloned()
        .expect("the message creates the agent that hears it, and starts it");
    assert!(
        !spawned.continue_session,
        "the agent that replaced the roster has a conversation of its own to \
         have: {spawned:?}"
    );
    assert_eq!(
        spawned.resume_session_id, None,
        "and no name to spend: {spawned:?}"
    );
}

/// A dead name costs ZERO restarts, not one.
///
/// The recorded id is checked against the provider's own tree before it is
/// spent, so an id whose conversation is gone — deleted, or written by the
/// other provider an agent was switched away from — is cleared where it is
/// read and the spawn falls back to the rule below it, instead of burning a
/// session finding out.
#[tokio::test]
async fn a_recorded_name_the_provider_no_longer_holds_is_cleared_before_it_is_spent() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-poisoned");
    let agent_id = crate::agent::derived_agent_id("run-poisoned");

    let specs_built: Arc<Mutex<Vec<SpawnOptions>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let recorder = Arc::clone(&specs_built);
        let agent = Agent::WarmBuilder(Arc::new(
            move |_prompt: &str, _choice: &ModelChoice, options: &SpawnOptions| {
                recorder.lock().unwrap().push(options.clone());
                Ok(warm_tui_spec())
            },
        ));
        let mut s = state.lock().unwrap();
        let worktrees = s.worktrees_root.clone();
        s.resume_id_probe = Arc::new(|_, _, _| false);
        s.project_at_mut(0).orch = Orchestrator::new(
            repo.clone(),
            worktrees,
            agent,
            Templates::default(),
            test_bridge_exe(),
        );
    }
    seed_exact_resumable_session(
        &state,
        "run-poisoned",
        &root,
        &ModelChoice::default(),
        "sess-gone",
    );

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-poisoned", "body": "are you still there" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");

    wait_for_deliveries(&state).await;
    let spawned = specs_built
        .lock()
        .unwrap()
        .first()
        .cloned()
        .expect("the message starts the agent");
    assert_eq!(
        spawned.resume_session_id, None,
        "a name the provider does not hold is never spent: {spawned:?}"
    );
    assert_eq!(
        state
            .lock()
            .unwrap()
            .recorded_resume_id("run-poisoned", &agent_id),
        None,
        "and the apply phase forgets it, so no later spawn spends it either"
    );
}
