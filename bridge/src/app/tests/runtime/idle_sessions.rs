use super::*;

/// An approved single-doc plan on a side orchestrator, played by hand (write
/// the doc, report `done(phase=plan)`, approve) — the app-level twin of the
/// orchestrator's own `approved_plan` fixture. Runs only ever implement a
/// plan, so a test that needs a run driven by a PARTICULAR harness builds a
/// plan on that harness's orchestrator first.
/// A run of `plan` on a side orchestrator, prepared and opened the way
/// `run.create` prepares and opens one — the fixture's twin of the two
/// halves the verb runs on either side of the app mutex.
pub(in crate::app::tests) fn dispatch_side_run(
    orch: &Orchestrator,
    store: &Store,
    plan: &ActivePlan,
    run_id: &str,
) -> (ActiveRun, AgentTurn) {
    let issue = ImplementableIssue::judge(RunSource {
        plan,
        has_active_run: false,
    })
    .unwrap();
    let prepared = orch
        .prepare_run_checkout(&issue, "main", run_id, Isolation::Worktree, store)
        .unwrap();
    orch.open_prepared_run(RunId::new(run_id), plan, prepared, Default::default())
        .unwrap()
}

pub(in crate::app::tests) fn approved_side_plan(
    orch: &Orchestrator,
    store: &Store,
    id: &str,
) -> ActivePlan {
    let mut plan = orch.create_plan(PlanId::new(id), "side goal", "main", Default::default());
    let workspace = orch.prepare_plan_workspace(id, store).unwrap();
    orch.open_plan_drafting(&mut plan, workspace).unwrap();
    let docs_dir = plan
        .workspace
        .as_ref()
        .expect("the plan has a workspace")
        .docs_dir
        .clone();
    std::fs::create_dir_all(docs_dir.join(".build")).unwrap();
    std::fs::write(docs_dir.join(".build/plan.md"), "# Plan\n").unwrap();
    orch.on_plan_done(
        &mut plan,
        store,
        DoneReport {
            phase: DonePhase::Plan,
            status: DoneStatus::Completed,
            summary: "planned".to_string(),
            outputs: DoneOutputs {
                plan_path: Some(".build/plan.md".to_string()),
                ..DoneOutputs::default()
            },
        },
    )
    .unwrap();
    orch.approve_plan(&mut plan).unwrap();
    plan
}

/// A run built on a side orchestrator (its plan lives there, not in
/// `state`), installed under `state`'s first project with `run_state`
/// stamped on. No process: a worktree's agent lives on the tab registry, so
/// a test that needs one asks for [`insert_run_with_agent_tab`].
pub(in crate::app::tests) fn insert_run(
    state: &mut AppState,
    repo: &std::path::Path,
    side_root: &std::path::Path,
    run_id: &str,
    run_state: RunState,
) -> std::path::PathBuf {
    let store = crate::store::Store::new(side_root.join("store")).expect("store opens");
    let side = Orchestrator::new(
        repo.to_path_buf(),
        side_root.join("wt"),
        Agent::Warm(HarnessSpec::new("true")),
        Templates::default(),
        test_bridge_exe(),
    );
    let plan = approved_side_plan(&side, &store, &format!("plan-of-{run_id}"));
    let (mut active, _turn) = dispatch_side_run(&side, &store, &plan, run_id);
    active.run.state = run_state;
    let root = AppState::canonical_root(&active.worktree.path);
    let project_id = state.project_at(0).id.clone();
    state.projects.bind_entity(run_id.to_string(), project_id);
    state.runs.insert(run_id.to_string(), active);
    root
}

/// Such a run PLUS the live agent tab its worktree owns, running `spec`.
/// Returns the tab's key. This is the shape the daemon actually holds: the
/// run carries lifecycle and thread, the worktree carries the process.
pub(in crate::app::tests) fn insert_run_with_agent_tab(
    state: &mut AppState,
    repo: &std::path::Path,
    side_root: &std::path::Path,
    run_id: &str,
    run_state: RunState,
    spec: HarnessSpec,
) -> TabKey {
    let (key, mut painted) =
        spawn_run_with_agent_tab(state, repo, side_root, run_id, run_state, spec);
    drain_pty_into_screen(state, &key, &mut painted, ScreenDrain::WhateverHasArrived);
    key
}

/// The same run and tab for a harness that ENDS ON ITS OWN: the screen is
/// fed until the PTY closes, so the test speaks about the harness's
/// complete last words instead of whatever a short deadline caught. Under a
/// loaded suite a child can still be starting when that deadline is up,
/// which is how a crash's epitaph went missing. A warm harness never
/// reaches EOF and must use the call above.
fn insert_run_with_dying_agent_tab(
    state: &mut AppState,
    repo: &std::path::Path,
    side_root: &std::path::Path,
    run_id: &str,
    run_state: RunState,
    spec: HarnessSpec,
) -> TabKey {
    let (key, mut painted) =
        spawn_run_with_agent_tab(state, repo, side_root, run_id, run_state, spec);
    drain_pty_into_screen(state, &key, &mut painted, ScreenDrain::EverythingUpToEof);
    key
}

/// The run, the tab, and the byte stream the tab's screen is fed from —
/// how far to drain it is the caller's to say.
fn spawn_run_with_agent_tab(
    state: &mut AppState,
    repo: &std::path::Path,
    side_root: &std::path::Path,
    run_id: &str,
    run_state: RunState,
    spec: HarnessSpec,
) -> (TabKey, broadcast::Receiver<Vec<u8>>) {
    let root = insert_run(state, repo, side_root, run_id, run_state);
    let (mut tab, rx) = Tab::spawn_agent(
        run_id.to_string(),
        crate::agent::derived_agent_id(run_id),
        test_agent_session_request(
            AgentProvider::default(),
            spec,
            root.clone(),
            terminal_size(120, 40),
        ),
    )
    .expect("the agent tab spawns");
    let key = derived_agent_key(&root, run_id);
    tab.session_instance = state.record_agent_session_start(
        run_id,
        &crate::agent::derived_agent_id(run_id),
        &root,
        &ModelChoice::default(),
        "build",
    );
    state.session_registry.test_insert_tab(key.clone(), tab);
    (key, rx.bytes.expect("a PTY session paints"))
}

/// How far [`drain_pty_into_screen`] feeds the screen.
enum ScreenDrain {
    /// Whatever has arrived by a short deadline — the only answer a warm
    /// harness, which never exits, can give.
    WhateverHasArrived,
    /// Every byte the harness ever painted, up to the EOF one that ends on
    /// its own reaches. A harness still open at the deadline fails the test.
    EverythingUpToEof,
}

/// Feed what the harness painted into its tab's screen — what
/// `spawn_tab_pump` does in the daemon, done synchronously here so a test
/// without a runtime can still speak about the retained screen.
fn drain_pty_into_screen(
    state: &mut AppState,
    key: &TabKey,
    rx: &mut broadcast::Receiver<Vec<u8>>,
    drain: ScreenDrain,
) {
    let budget = match drain {
        ScreenDrain::WhateverHasArrived => Duration::from_secs(2),
        ScreenDrain::EverythingUpToEof => Duration::from_secs(30),
    };
    let deadline = std::time::Instant::now() + budget;
    while std::time::Instant::now() < deadline {
        match rx.try_recv() {
            Ok(chunk) => {
                if let Some(screen) = state
                    .session_registry
                    .test_tab(key)
                    .and_then(|tab| tab.screen.as_ref())
                {
                    screen.feed(&chunk);
                }
            }
            Err(broadcast::error::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            // Closed: the reader thread hit EOF and dropped both senders,
            // so every byte the harness ever painted has already been
            // handed over. An exited child is NOT the same signal — the
            // wait can reap it a scheduling slice before the reader has
            // forwarded its last words, and a loaded machine is where that
            // slice gets long: a crash's epitaph would go missing exactly
            // when the whole suite is running.
            Err(_) => return,
        }
    }
    assert!(
        matches!(drain, ScreenDrain::WhateverHasArrived),
        "the harness was still running after {budget:?}; a test that waits for its \
         last words has none to read"
    );
}

/// The warm stand-in for a real TUI: it enables bracketed-paste mode (so a
/// prompt write's readiness wait resolves) and drains stdin forever.
pub(in crate::app::tests) fn warm_tui_spec() -> HarnessSpec {
    HarnessSpec::new("sh")
        .arg("-c")
        .arg("printf '\\033[?2004h'; cat >/dev/null")
}

pub(in crate::app::tests) fn test_agent_session_request(
    provider: AgentProvider,
    spec: HarnessSpec,
    root: std::path::PathBuf,
    size: PtySize,
) -> SessionOpenRequest {
    SessionOpenRequest {
        spec,
        root,
        choice: ModelChoice {
            provider,
            ..ModelChoice::default()
        },
        terminal: TerminalOpenOptions {
            size,
            turn_ready_grace: Some(crate::orchestrator::HARNESS_READY_GRACE),
            identity: None,
        },
        resume_session_id: None,
    }
}

/// The sweep threshold these tests speak in — the shape of the real one
/// (`BRIDGE_IDLE_SECONDS`, 300s by default): minutes, and so comfortably
/// past the 30s window inside which a PTY's paint makes it `Working`. A
/// threshold shorter than that window would be asking whether an agent that
/// painted a moment ago is quiet, which is a question no deployment asks.
pub(in crate::app::tests) const QUIET_THRESHOLD: Duration = Duration::from_secs(300);

/// Silence is an anomaly only when measured from the last thing Build
/// asked. A tab's agent outlives every phase and idles at a prompt between
/// them, so raw PTY silence would demote a run the instant it is
/// re-dispatched after a long quiet review — the agent has said nothing for
/// an hour because nobody spoke to it.
#[test]
fn an_agent_is_only_quiet_if_it_has_been_silent_since_the_last_turn() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let key = insert_run_with_agent_tab(
        &mut state,
        &repo,
        dir.path(),
        "run-quiet",
        RunState::Building,
        warm_tui_spec(),
    );
    // The PTY has painted nothing for ten minutes: silent by the paint
    // clock, and — since that is minutes past the 30s window — not claiming
    // to be working either. Aged rather than waited out, so the test reads
    // the behaviour instead of a wall clock.
    state
        .session_registry
        .test_tab(&key)
        .unwrap()
        .session
        .backdate_last_output(Duration::from_secs(600));

    state
        .session_registry
        .test_tab_mut(&key)
        .unwrap()
        .last_delivered_at = Some(std::time::Instant::now());
    assert!(
        state.mark_idle_tasks(QUIET_THRESHOLD).is_empty(),
        "an agent that was just given a turn is working, not quiet"
    );
    assert_eq!(state.runs["run-quiet"].run.state, RunState::Building);

    state
        .session_registry
        .test_tab_mut(&key)
        .unwrap()
        .last_delivered_at = Some(std::time::Instant::now() - QUIET_THRESHOLD);
    assert_eq!(
        state.mark_idle_tasks(QUIET_THRESHOLD),
        vec!["run-quiet".to_string()],
        "silence that outlasts the turn that provoked it is an anomaly"
    );
}

/// A session that knows when its turn began is never demoted mid-turn.
///
/// The sweep's whole instrument used to be silence, and silence is exactly
/// what a model reasoning for forty minutes produces. A PTY could only
/// guess at the difference; a session that reports its own turn boundaries
/// can say it, so `Working` short-circuits the demotion and the anomaly
/// clock is only consulted for a session that is not in a turn.
#[test]
fn a_session_that_reports_a_turn_in_flight_is_never_demoted_for_silence() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let root = insert_run(
        &mut state,
        &repo,
        dir.path(),
        "run-mid-turn",
        RunState::Building,
    );
    let agent_id = crate::agent::derived_agent_id("run-mid-turn");
    let key = derived_agent_key(&root, "run-mid-turn");
    let quiet = Duration::from_secs(2400);
    state.session_registry.test_insert_tab(
        key.clone(),
        dictated_agent_tab(
            &root,
            "run-mid-turn",
            &agent_id,
            DictatedSession::reporting(AgentStatus::Working).silent_for(quiet),
        ),
    );
    // Build spoke long ago and has heard nothing since: every other
    // instrument the sweep owns reads this as an anomaly.
    state
        .session_registry
        .test_tab_mut(&key)
        .unwrap()
        .last_delivered_at = Some(std::time::Instant::now() - quiet);

    assert!(
        state.mark_idle_tasks(Duration::from_secs(300)).is_empty(),
        "a model mid-turn is working, however long it has been thinking"
    );
    assert_eq!(state.runs["run-mid-turn"].run.state, RunState::Building);

    // Control: the same silence, one status later. The turn ended without a
    // `done`, and THAT is the anomaly the sweep exists for.
    state.session_registry.test_insert_tab(
        key,
        dictated_agent_tab(
            &root,
            "run-mid-turn",
            &agent_id,
            DictatedSession::reporting(AgentStatus::Waiting).silent_for(quiet),
        ),
    );
    assert_eq!(
        state.mark_idle_tasks(Duration::from_secs(300)),
        vec!["run-mid-turn".to_string()],
        "a turn that ended in silence rather than a report is still demoted"
    );
}

/// And for a PTY the short-circuit is a no-op, by construction: paint
/// inside thirty seconds is the only thing that makes one `Working`, so a
/// tab quiet past a threshold minutes long can never be. The new conjunct
/// cannot spare a single agent the sweep used to demote.
#[test]
fn a_pty_quiet_past_the_threshold_is_never_working() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let key = insert_run_with_agent_tab(
        &mut state,
        &repo,
        dir.path(),
        "run-painting",
        RunState::Building,
        warm_tui_spec(),
    );
    let tab = &state.session_registry.test_tab(&key).unwrap();
    assert_eq!(
        tab.session.status(),
        AgentStatus::Working,
        "a freshly spawned harness has just painted"
    );

    for quiet in [
        AGENT_WORKING_WINDOW + Duration::from_secs(1),
        Duration::from_secs(300),
        Duration::from_secs(2400),
    ] {
        state
            .session_registry
            .test_tab(&key)
            .unwrap()
            .session
            .backdate_last_output(quiet);
        assert_ne!(
            state
                .session_registry
                .test_tab(&key)
                .unwrap()
                .session
                .status(),
            AgentStatus::Working,
            "a PTY silent for {quiet:?} cannot claim to be working"
        );
    }
    state
        .session_registry
        .test_tab_mut(&key)
        .unwrap()
        .last_delivered_at = Some(std::time::Instant::now() - Duration::from_secs(600));
    assert_eq!(
        state.mark_idle_tasks(Duration::from_secs(300)),
        vec!["run-painting".to_string()],
        "so the demotion the sweep has always made is unmoved"
    );
}

/// Reading is what STARTS the reviewer's Working indicator (it stamps
/// seen_at), and only a posted reply stops it. An agent that is never told
/// that leaves the line running after it has finished — so the read itself
/// has to say so, in the one place the agent is guaranteed to look.
#[test]
fn reading_messages_says_it_started_the_working_indicator() {
    let mut thread = crate::thread::Thread::new("run-read");
    thread.post_user("do the thing", None, "2026-08-09T18:00:00Z");

    let read = apply_thread_action(
        &mut thread,
        crate::thread::ArtifactKind::Diff,
        BridgeAction::ReadUnreadMessages,
        "2026-08-09T18:00:01Z",
    )
    .expect("reading succeeds");
    let notice = read["working"]
        .as_str()
        .expect("a read that returned messages explains the indicator it started");
    assert!(
        notice.contains("post_thread_message"),
        "it must name the tool that stops it: {notice}"
    );

    // Nothing unread: nothing was marked seen, so nothing was started and
    // there is nothing to explain.
    let empty = apply_thread_action(
        &mut thread,
        crate::thread::ArtifactKind::Diff,
        BridgeAction::ReadUnreadMessages,
        "2026-08-09T18:00:02Z",
    )
    .expect("an empty read succeeds");
    assert!(empty["messages"].as_array().unwrap().is_empty());
    assert!(
        empty["working"].is_null(),
        "an empty read starts no indicator: {empty}"
    );
}

/// A crash's only explanation is usually the thing the harness printed
/// before it died — codex refusing to start its required MCP server, a
/// provider saying the account is out of quota. Reporting a bare exit code
/// throws that away and leaves the human staring at a retained screen that
/// still looks like a session.
#[test]
fn a_crashed_agent_reports_what_it_printed_before_it_died() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    // Fed to EOF rather than paused for a fixed 300ms: the pause read an
    // empty screen whenever a loaded machine was slow to start the child.
    insert_run_with_dying_agent_tab(
        &mut state,
        &repo,
        dir.path(),
        "run-crashed",
        RunState::Building,
        HarnessSpec::new("sh")
            .arg("-c")
            .arg("printf 'MCP server build failed to start\n'; exit 1"),
    );

    assert_eq!(
        state.mark_idle_tasks(Duration::from_millis(50)),
        vec!["run-crashed".to_string()]
    );
    let reported = state.runs["run-crashed"]
        .last_error
        .clone()
        .expect("a crash is an error worth naming");
    assert!(
        reported.contains("exit code 1"),
        "the code still rides along: {reported}"
    );
    assert!(
        reported.contains("MCP server build failed to start"),
        "the harness's own last words are the diagnosis: {reported}"
    );
}

/// Wait until a tab's PTY has been silent for `quiet` — the harness has
/// finished echoing whatever it was just handed. A test that then backdates
/// the stamp is not racing the reader thread for the last word on when this
/// terminal painted.
async fn wait_for_pty_quiet(state: &Arc<Mutex<AppState>>, key: &TabKey, quiet: Duration) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let idle = state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(key)
            .unwrap()
            .session
            .quiet_for();
        if idle >= quiet {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the agent's PTY never settled"
        );
        tokio::time::sleep(quiet - idle).await;
    }
}

/// And the clock the rule reads is started by DELIVERY itself.
///
/// Nothing else can start it: the agent tab outlives every phase, so the
/// only moment that means "you now owe an answer" is the moment Build
/// submitted a turn. If a delivery left the stamp alone, an agent handed a
/// long job would be demoted the first time it thought quietly for longer
/// than the threshold — silence read as an anomaly when it is the work.
#[tokio::test]
async fn delivering_a_turn_starts_the_quiescence_clock() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_qa_state_and_handler(&repo, dir.path());
    let root = {
        let mut s = state.lock().unwrap();
        insert_run(
            &mut s,
            &repo,
            dir.path(),
            "run-spoken-to",
            RunState::Building,
        )
    };

    let (_, spawned) = deliver(
        &state,
        &root,
        "run-spoken-to",
        &crate::agent::derived_agent_id("run-spoken-to"),
        &ModelChoice::default(),
        "build",
        ["get to work", "there is more"],
    )
    .expect("the turn reaches an agent");
    assert_eq!(spawned, Spawned::Fresh, "the tab did not exist yet");

    let key = derived_agent_key(&root, "run-spoken-to");
    // The tty echoes a written prompt back through the reader thread, so
    // wait for the PTY to go quiet before speaking about its silence.
    wait_for_pty_quiet(&state, &key, Duration::from_millis(200)).await;

    let mut s = state.lock().unwrap();
    // It has painted nothing since — it is chewing on what it was asked.
    s.session_registry
        .test_tab_mut(&key)
        .unwrap()
        .session
        .backdate_last_output(Duration::from_secs(600));
    assert!(
        s.mark_idle_tasks(Duration::from_secs(60)).is_empty(),
        "an agent Build has just spoken to is working, however quiet it is"
    );
    assert_eq!(s.runs["run-spoken-to"].run.state, RunState::Building);

    // Control: the run was demotable all along — it is the turn's stamp,
    // and only that, holding it up.
    s.session_registry
        .test_tab_mut(&key)
        .unwrap()
        .last_delivered_at = Some(std::time::Instant::now() - Duration::from_secs(600));
    assert_eq!(
        s.mark_idle_tasks(Duration::from_secs(60)),
        vec!["run-spoken-to".to_string()],
        "silence that outlasts the turn that provoked it is an anomaly"
    );
}

/// A quiet agent is still an agent. The idle sweep records WHY an entity
/// went quiet, and that record must not also claim the session ended: the
/// process is sitting at its prompt, and the next turn continues the very
/// session the thread would have closed.
#[test]
fn an_idle_demotion_leaves_the_live_agents_session_open() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let key = insert_run_with_agent_tab(
        &mut state,
        &repo,
        dir.path(),
        "run-still-there",
        RunState::Building,
        warm_tui_spec(),
    );
    primary_thread_mut(&mut state.runs.get_mut("run-still-there").unwrap().agents).start_session(
        "claude",
        None,
        None,
        "build",
        &now_rfc3339(),
    );
    // Silent past the threshold, aged rather than waited out.
    state
        .session_registry
        .test_tab(&key)
        .unwrap()
        .session
        .backdate_last_output(Duration::from_secs(600));
    state
        .session_registry
        .test_tab_mut(&key)
        .unwrap()
        .last_delivered_at = Some(std::time::Instant::now() - QUIET_THRESHOLD);

    assert_eq!(
        state.mark_idle_tasks(QUIET_THRESHOLD),
        vec!["run-still-there".to_string()],
        "the quiet agent's entity is demoted"
    );
    assert!(
        state
            .session_registry
            .test_tab(&key)
            .unwrap()
            .session_is_live(),
        "this test is only meaningful while the agent is still alive"
    );
    let thread = primary_thread(&state.runs["run-still-there"].agents);
    assert!(
        thread.sessions.last().unwrap().ended_at.is_none(),
        "a quiet agent is still in its session: {:?}",
        thread.sessions
    );
}

#[test]
fn mark_idle_demotes_a_quiet_plan_and_run() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    // A Building run whose agent exits immediately → demoted, with the exit
    // code recorded (the quiescence rule: silence is never completion). The
    // spawn returns at the PTY's EOF, so the agent has provably died before
    // the sweep asks — no pause is being raced.
    insert_run_with_dying_agent_tab(
        &mut state,
        &repo,
        dir.path(),
        "run-idle",
        RunState::Building,
        HarnessSpec::new("sh").arg("-c").arg("exit 7"),
    );
    let demoted = state.mark_idle_tasks(Duration::from_secs(3600));
    assert!(demoted.contains(&"run-idle".to_string()), "{demoted:?}");
    let got = state.handle(req("run.get", json!({ "run_id": "run-idle" })));
    assert_eq!(got["result"]["state"], "idle_unreported", "{got:?}");
    assert!(got["result"]["last_error"]
        .as_str()
        .unwrap()
        .contains("exit code 7"));
}

/// A turn addressed to a worktree that cannot host an agent — the scaffold
/// step fails on a path that is not a directory.
fn unreachable_turn(state: &mut AppState, owner: &str) -> PendingAgentTurn {
    let root = std::path::PathBuf::from("/dev/null/there-is-no-worktree-here");
    if let Some(active) = state.runs.get_mut(owner) {
        active.worktree.path = root.clone();
    } else if let Some(workspace) = state
        .plans
        .get_mut(owner)
        .and_then(|active| active.workspace.as_mut())
    {
        workspace.checkout = root.clone();
    }
    let agent = state
        .entity_agents(owner)
        .expect("the unreachable fixture owns an agent")
        .primary()
        .expect("the unreachable fixture has a primary agent");
    PendingAgentTurn {
        operation_id: None,
        root,
        owner: owner.to_string(),
        agent_id: agent.id.clone(),
        conversation_id: agent.conversation_id().to_string(),
        model_choice: agent.choice.clone(),
        choice_revision: agent.choice_revision,
        interrupt: false,
        say: Some(TurnText {
            cold: "cold turn".into(),
            warm: "warm turn".into(),
        }),
        phase: "build",
        wants_catch_up: false,
        survives_refusal: false,
    }
}

/// Install the stored Issue shape these runtime tests need without going
/// through the retired Issue/planning mutation surface. Runtime delivery and
/// idle recovery still support legacy records loaded from disk, which is the
/// behavior under test here.
fn insert_legacy_drafting_issue(state: &mut AppState, issue_id: &str, goal: &str) {
    let project_id = state.project_at(0).id.clone();
    let project = state
        .orch_for(&project_id)
        .expect("the test project has an orchestrator")
        .clone();
    let store = state
        .require_store()
        .expect("the test daemon has a store")
        .clone();
    let mut active = project.create_plan(PlanId::new(issue_id), goal, "main", Default::default());
    let workspace = project
        .prepare_plan_workspace(issue_id, &store)
        .expect("the legacy Issue workspace is prepared");
    project
        .open_plan_drafting(&mut active, workspace)
        .expect("the legacy Issue is drafting");
    state.projects.bind_entity(issue_id.to_string(), project_id);
    state
        .finish_plan_mutation(issue_id.to_string(), active)
        .expect("the legacy Issue is persisted");
}

/// A delivery that never reached an agent used to be a silent `eprintln!`:
/// the run had already transitioned to `Building` and been persisted, so it
/// sat there working with nobody working, forever. The failure has to land
/// on the entity where a surface can read it.
#[test]
fn a_delivery_that_never_reaches_an_agent_is_visible_on_its_entity() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    insert_run(
        &mut app,
        &repo,
        dir.path(),
        "run-unreachable",
        RunState::Building,
    );
    let turn = unreachable_turn(&mut app, "run-unreachable");
    app.delivery_queue.enqueue(turn);
    let state = app.shared();

    deliver_pending_agent_turns(&state);

    let got = state
        .lock()
        .unwrap()
        .handle(req("run.get", json!({ "run_id": "run-unreachable" })));
    let last_error = got["result"]["last_error"].as_str().unwrap_or_default();
    assert!(
        last_error.contains("could not reach the agent"),
        "the failure must be legible on the run: {got:?}"
    );
    // And it is durable: the reason survives a re-read from the store.
    let record = state
        .lock()
        .unwrap()
        .store
        .as_ref()
        .unwrap()
        .load_all_runs()
        .unwrap()
        .into_iter()
        .find(|r| r.id == "run-unreachable")
        .expect("the run is persisted");
    assert!(
        record.last_error.unwrap_or_default().contains("agent"),
        "the failure must be persisted, not just held in memory"
    );
}

/// The agent's own record has to carry it too.
///
/// The client lays a "starting" state over the row it pressed Resume on and
/// waits for the entity's next word about the session. A start that never
/// came up says nothing about the SESSION, so without this the row wears
/// the ring until the overlay's grace runs out and then goes quietly idle,
/// with the reason nowhere.
#[test]
fn a_start_that_never_reached_a_harness_says_so_on_its_agent() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    insert_run(
        &mut app,
        &repo,
        dir.path(),
        "run-no-start",
        RunState::Building,
    );
    let turn = unreachable_turn(&mut app, "run-no-start");
    app.delivery_queue.enqueue(turn);
    let state = app.shared();

    deliver_pending_agent_turns(&state);

    let got = state
        .lock()
        .unwrap()
        .handle(req("run.get", json!({ "run_id": "run-no-start" })));
    let agent = &got["result"]["agents"][0];
    assert!(
        agent["start_error"]
            .as_str()
            .unwrap_or_default()
            .contains("could not reach the agent"),
        "the agent says why its session never opened: {got:?}"
    );
}

/// And it is the LAST start's failure, not a permanent mark: a fresh turn
/// on its way to the agent makes it history, so the next press wears its
/// own ring instead of being answered by the failure before it.
#[test]
fn a_fresh_turn_forgets_the_last_start_failure() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    insert_run(&mut app, &repo, dir.path(), "run-retry", RunState::Building);
    let turn = unreachable_turn(&mut app, "run-retry");
    app.delivery_queue.enqueue(turn);
    let state = app.shared();
    deliver_pending_agent_turns(&state);

    {
        let mut app = state.lock().unwrap();
        let turn = unreachable_turn(&mut app, "run-retry");
        app.delivery_queue.enqueue(turn);
    }
    // Taken, not delivered: the point is that reaching for the agent is
    // what forgets the last failure, before anything is known about how
    // this one ends. The marks it holds are released when it drops.
    let _taken = state.lock().unwrap().take_pending_turns();

    let got = state
        .lock()
        .unwrap()
        .handle(req("run.get", json!({ "run_id": "run-retry" })));
    assert!(
        got["result"]["agents"][0]["start_error"].is_null(),
        "the turn now on its way answers for the session, not the one before it: {got:?}"
    );
}

/// The third answer to a start. An issue whose session is over holds no
/// workspace, so a turn for it opens nothing — and used to say so only on
/// stderr, leaving the client's "starting" ring on until its grace ran out.
/// The agent carries the reason; the issue's own `last_error` does not,
/// because nothing about the work failed.
#[test]
fn a_start_for_an_entity_whose_session_is_over_says_so_on_its_agent() {
    let (dir, repo) = init_repo();
    let app = qa_state(&repo, dir.path());
    let state = app.shared();
    let plan_id = "plan-session-over";
    {
        let side = Orchestrator::new(
            repo.clone(),
            dir.path().join("side-wt"),
            Agent::Warm(HarnessSpec::new("true")),
            Templates::default(),
            test_bridge_exe(),
        );
        let active = side.create_plan(
            PlanId::new(plan_id),
            "a goal whose session is over",
            "main",
            Default::default(),
        );
        let mut s = state.lock().unwrap();
        let project_id = s.project_at(0).id.clone();
        s.projects.bind_entity(plan_id.to_string(), project_id);
        s.plans.insert(plan_id.to_string(), active);
        let turn = unreachable_turn(&mut s, plan_id);
        s.delivery_queue.enqueue(turn);
    }

    deliver_pending_agent_turns(&state);

    let got = state
        .lock()
        .unwrap()
        .handle(req("issue.get", json!({ "issue_id": plan_id })));
    let agent = &got["result"]["agents"][0];
    assert_eq!(
        agent["start_error"],
        json!(AGENT_START_DECLINED_SESSION_OVER),
        "the agent says why no session opened: {got:?}"
    );
    assert!(
        got["result"]["last_error"].is_null(),
        "a session that is over is not a failure of the work: {got:?}"
    );
}

/// The idle sweep used to skip an entity with no agent tab (`let tab =
/// tab?`), which is exactly the entity a failed delivery leaves behind.
/// Build owns every agent and always keeps it as a tab, so a working entity
/// with none is an anomaly, not an absence of evidence.
#[test]
fn a_working_run_with_no_agent_tab_is_an_anomaly_not_a_skip() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    insert_run(
        &mut state,
        &repo,
        dir.path(),
        "run-tabless",
        RunState::Building,
    );
    assert_eq!(
        state.mark_idle_tasks(Duration::from_secs(3600)),
        vec!["run-tabless".to_string()],
        "a working run with no agent at all must be demoted, not skipped"
    );
    let got = state.handle(req("run.get", json!({ "run_id": "run-tabless" })));
    assert_eq!(got["result"]["state"], "idle_unreported", "{got:?}");
    // No harness exited here, so no exit-code claim is invented.
    assert!(got["result"]["last_error"].is_null(), "{got:?}");
}

/// The legacy Issue half of the same hole. Its turns are queued and delivered
/// by exactly the same path as a run's, so an agent that never starts must
/// land on the Issue the same way, and the reason must survive a restart.
#[test]
fn a_delivery_that_never_reaches_an_agent_is_visible_on_its_legacy_issue() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let issue_id = "issue-unreachable";
    insert_legacy_drafting_issue(&mut app, issue_id, "unreachable");
    let state = app.shared();
    {
        let mut app = state.lock().unwrap();
        app.delivery_queue.clear_queued();
        let turn = unreachable_turn(&mut app, issue_id);
        app.delivery_queue.enqueue(turn);
    }

    deliver_pending_agent_turns(&state);

    let got = state
        .lock()
        .unwrap()
        .handle(req("issue.get", json!({ "issue_id": issue_id })));
    let last_error = got["result"]["last_error"].as_str().unwrap_or_default();
    assert!(
        last_error.contains("could not reach the agent"),
        "the failure must be legible on the legacy Issue: {got:?}"
    );
    let record = state
        .lock()
        .unwrap()
        .store
        .as_ref()
        .unwrap()
        .load_all_plans()
        .unwrap()
        .into_iter()
        .find(|p| p.id == issue_id)
        .expect("the legacy Issue is persisted");
    assert!(
        record.last_error.unwrap_or_default().contains("agent"),
        "the failure must be persisted, not just held in memory"
    );
}

/// The legacy Issue half of the tabless anomaly: a drafting Issue whose agent
/// never arrived is demoted by the sweep, and one whose turn is still queued
/// is left alone.
#[test]
fn a_working_legacy_issue_with_no_agent_tab_is_an_anomaly_not_a_skip() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let issue_id = "issue-tabless";
    insert_legacy_drafting_issue(&mut state, issue_id, "tabless");
    state.delivery_queue.clear_queued();
    let turn = unreachable_turn(&mut state, issue_id);
    state.delivery_queue.enqueue(turn);
    assert!(
        state.mark_idle_tasks(Duration::from_secs(3600)).is_empty(),
        "a queued turn means the planning agent is coming, not missing"
    );

    state.delivery_queue.clear_queued();
    assert_eq!(
        state.mark_idle_tasks(Duration::from_secs(3600)),
        vec![issue_id.to_string()],
        "a drafting legacy Issue with no agent at all must be demoted, not skipped"
    );
    let got = state.handle(req("issue.get", json!({ "issue_id": issue_id })));
    assert_eq!(got["result"]["state"], "idle_unreported", "{got:?}");
    // No harness exited here, so no exit-code claim is invented.
    assert!(got["result"]["last_error"].is_null(), "{got:?}");
}

/// A delivery that unwinds has to give back what it took.
///
/// `take_pending_turns` marks every owner in flight, and an owner marked in
/// flight is spared by the idle sweep for as long as the mark stands. A
/// batch that panicked used to keep those marks forever: the run sat in
/// Working with no agent tab and nothing left in the daemon could ever
/// demote it.
#[test]
fn a_delivery_that_panics_gives_its_in_flight_marks_back() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    insert_run(
        &mut app,
        &repo,
        dir.path(),
        "run-panicked",
        RunState::Building,
    );
    let turn = unreachable_turn(&mut app, "run-panicked");
    app.delivery_queue.enqueue(turn);
    let state = app.shared();
    let turns = state.lock().unwrap().take_pending_turns();
    assert!(
        state
            .lock()
            .unwrap()
            .agent_turn_is_undelivered("run-panicked"),
        "the batch holds the mark while it delivers"
    );

    // The panic no delivery can catch: another frame died holding the app
    // mutex, so the delivery's first acquisition unwraps a poisoned lock —
    // after the turn and its mark have already left the batch.
    let poisoner = Arc::clone(&state);
    std::thread::spawn(move || {
        let _held = poisoner.lock().unwrap();
        panic!("the frame holding the app mutex died");
    })
    .join()
    .expect_err("the poisoning thread panics");

    let delivered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        DeliveryRunner::run(&state, turns)
    }));
    assert!(delivered.is_err(), "a poisoned lock unwinds the delivery");

    let mut s = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(
        s.delivery_queue.is_idle(),
        "an unwinding delivery gives its in-flight marks back"
    );
    assert_eq!(
        s.mark_idle_tasks(Duration::from_secs(3600)),
        vec!["run-panicked".to_string()],
        "the entity it stranded is demotable again"
    );
}

/// A harness already on its way is the one that reads the next message,
/// and the guard that says so has to see a turn in every state a turn can
/// be in.
///
/// The queue is emptied under the frame's own acquisition and the spawn
/// claim is taken on a thread of the delivery's, so between the two there
/// is a stretch — a thread-pool handoff for the first turn, the whole of
/// every earlier turn's cold spawn for the rest — in which a turn on its
/// way is in neither the queue nor the claim set. A second message landing
/// there queues a duplicate turn: the claim still stops a second harness,
/// but nothing stops the duplicate `read_unread_messages` nudge.
#[test]
fn a_second_message_queues_nothing_while_the_first_is_mid_delivery() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    insert_run(
        &mut app,
        &repo,
        dir.path(),
        "run-nudged",
        RunState::Building,
    );
    let state = app.shared();
    let post = |body: &str| {
        state.lock().unwrap().handle(req(
            "thread.post",
            json!({ "entity_id": "run-nudged", "body": body }),
        ))
    };

    let first = post("start on this");
    assert_eq!(first["ok"], true, "{first:?}");
    assert_eq!(
        state.lock().unwrap().delivery_queue.queued_len(),
        1,
        "the first message wakes the agent"
    );

    // Off the queue, no claim yet: the delivery is between its two halves.
    let _delivering = state.lock().unwrap().take_pending_turns();
    {
        let s = state.lock().unwrap();
        assert!(s.delivery_queue.queued_is_empty());
        assert!(s.session_registry.test_counts().claims == 0);
    }

    let second = post("and this");
    assert_eq!(
        second["ok"], true,
        "the message is durable on the thread either way: {second:?}"
    );
    assert!(
        state.lock().unwrap().delivery_queue.queued_is_empty(),
        "a second turn was queued behind the one already coming"
    );
}

/// A textless start promises the agent nothing to read, so a message
/// posted while it is on its way has to queue its own turn.
///
/// The guard on a second turn assumes the harness already coming opens on
/// a cold prompt that tells it to call `read_unread_messages`. A start with
/// nothing unread carries no prompt at all: the harness opens and is sent
/// nothing, and a message posted between the button and the harness would
/// sit durable on the thread with nobody told about it. The spawn claim
/// still guarantees one harness; the turn queued here lands Warm on the tab
/// the start opened.
#[test]
fn a_message_posted_during_a_textless_start_queues_its_own_turn() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    insert_run(
        &mut app,
        &repo,
        dir.path(),
        "run-started-bare",
        RunState::Building,
    );
    let root = app.entity_agent_root("run-started-bare").unwrap();
    let agent_id = primary_agent_id(&app, "run-started-bare");
    app.delivery_queue.enqueue(PendingAgentTurn {
        operation_id: None,
        root: root.clone(),
        owner: "run-started-bare".into(),
        agent_id: agent_id.clone(),
        conversation_id: agent_id.clone(),
        model_choice: ModelChoice::default(),
        choice_revision: 0,
        interrupt: false,
        say: None,
        phase: "start",
        wants_catch_up: true,
        survives_refusal: false,
    });
    let state = app.shared();

    // Off the queue, mid-delivery: the harness is coming, with nothing to say.
    let _delivering = state.lock().unwrap().take_pending_turns();
    assert!(state.lock().unwrap().delivery_queue.queued_is_empty());

    let posted = state.lock().unwrap().handle(req(
        "thread.post",
        json!({ "entity_id": "run-started-bare", "body": "read this" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let s = state.lock().unwrap();
    assert_eq!(
        s.delivery_queue.queued_len(),
        1,
        "a start that says nothing is not the turn that reads this message"
    );
    let queued = &s.delivery_queue.queued_nth(0).unwrap();
    assert_eq!(queued.agent_id, agent_id);
    assert!(
        queued.says_something(),
        "the queued turn is the one that tells it to read"
    );
}

/// One batch, one owner, two agents: settling the delivered turn gives
/// back that turn's mark and nobody else's.
///
/// The mark travels with the turn it was taken for. Looked up by owner
/// alone, the first settle could hand back the OTHER agent's mark, and the
/// agent whose turn was still undelivered would read as absent — the window
/// the count exists to close.
#[test]
fn settling_one_agents_turn_leaves_the_other_agents_mark_in_flight() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    insert_run(
        &mut state,
        &repo,
        dir.path(),
        "run-two-agents",
        RunState::Building,
    );
    let first = unreachable_turn(&mut state, "run-two-agents");
    let mut second = unreachable_turn(&mut state, "run-two-agents");
    second.agent_id = "agent-second".into();
    let root = first.root.clone();
    let first_agent = first.agent_id.clone();
    state.delivery_queue.enqueue(first);
    state.delivery_queue.enqueue(second);

    let mut delivering = state.take_pending_turns();
    let (delivered, mark) = delivering.next_turn().expect("the first turn");
    assert_eq!(delivered.agent_id, first_agent);
    mark.settle(&mut state);

    assert!(
        !state.agent_is_on_its_way(&root, &first_agent),
        "the delivered turn's agent is settled"
    );
    assert!(
        state.agent_is_on_its_way(&root, "agent-second"),
        "the undelivered turn's agent is still on its way"
    );
    assert!(
        state.mark_idle_tasks(Duration::from_secs(3600)).is_empty(),
        "and so is the owner"
    );
    let (_, mark) = delivering.next_turn().expect("the second turn");
    mark.settle(&mut state);
    assert!(
        !state.agent_is_on_its_way(&root, "agent-second"),
        "both marks are back once both turns have landed"
    );
}

/// The tabless anomaly must not fire on the gap the queue opens: a verb
/// transitions the run under the state lock and the turn is delivered after
/// it, so for the seconds a cold spawn takes there is a working run whose
/// agent is legitimately still on its way.
#[test]
fn a_run_whose_turn_is_still_on_its_way_is_not_demoted() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    insert_run(
        &mut state,
        &repo,
        dir.path(),
        "run-dispatching",
        RunState::Building,
    );
    let turn = unreachable_turn(&mut state, "run-dispatching");
    state.delivery_queue.enqueue(turn);
    assert!(
        state.mark_idle_tasks(Duration::from_secs(3600)).is_empty(),
        "a queued turn means the agent is coming, not missing"
    );

    // Mid-delivery — off the queue, not yet a tab — is the same story.
    let mut delivering = state.take_pending_turns();
    assert!(state.delivery_queue.queued_is_empty());
    assert!(
        state.mark_idle_tasks(Duration::from_secs(3600)).is_empty(),
        "a turn mid-delivery means the agent is coming, not missing"
    );

    // Once the delivery is over and no tab appeared, it IS the anomaly.
    let (_, mark) = delivering.next_turn().expect("the one turn");
    mark.settle(&mut state);
    assert_eq!(
        state.mark_idle_tasks(Duration::from_secs(3600)),
        vec!["run-dispatching".to_string()]
    );
}

pub(in crate::app::tests) fn fake_run_record(id: &str) -> PersistedRun {
    PersistedRun {
        id: id.into(),
        plan_id: None,
        goal: "quiet".into(),
        project_path: String::new(),
        base_branch: "main".into(),
        state: RunState::Building,
        branch: "build/quiet".into(),
        worktree_name: "quiet".into(),
        worktree_path: "/tmp/nonexistent-run".into(),
        base_sha: None,
        stages: Vec::new(),
        current_stage_id: None,
        revising_stage_id: None,
        auto_advance: false,
        adopted: false,
        triage: None,
        recovery: None,
        publication_attempt: None,
        provider: AgentProvider::Claude,
        model: None,
        effort: None,
        agents: crate::agent::stored_agents(id),
        legacy_thread: crate::thread::Thread::default(),
        last_summary: None,
        last_error: None,
        created_at: "2026-07-01T10:00:00Z".into(),
        updated_at: "2026-07-01T10:00:00Z".into(),
        state_changed_at: None,
    }
}
