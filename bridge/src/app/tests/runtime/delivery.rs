use super::*;

/// The feed row for a run, off `board.list` — the one place `working` is
/// actually read from, so a test about a stuck working state asks there.
fn feed_row_for_run(handler: &FrameHandler, run_id: &str) -> Value {
    let board = call(handler, "board.list", json!({}));
    board["result"]["items"]
        .as_array()
        .expect("the feed is a list of rows")
        .iter()
        .find(|row| row["run_id"] == json!(run_id))
        .unwrap_or_else(|| panic!("no feed row for {run_id}: {board:?}"))
        .clone()
}

/// The conversation an entity's primary agent owns — what a test that
/// predates the agent rail means by "the entity's thread".
pub(in crate::app::tests) fn primary_thread(
    roster: &crate::agent::AgentRoster,
) -> &crate::thread::Thread {
    &roster.primary().expect(THE_TEST_PUT_AN_AGENT_HERE).thread
}

/// The same conversation, to write on: a test that says what an agent
/// heard or answered before the assertion it is really about.
pub(in crate::app::tests) fn primary_thread_mut(
    roster: &mut crate::agent::AgentRoster,
) -> &mut crate::thread::Thread {
    &mut roster
        .primary_mut()
        .expect(THE_TEST_PUT_AN_AGENT_HERE)
        .thread
}

const THE_TEST_PUT_AN_AGENT_HERE: &str = "the entity holds the agent this test put there";

/// The interruption a dead session leaves on a conversation, if it left
/// one.
fn interruption_in(thread: &crate::thread::Thread) -> Option<&crate::thread::ThreadEvent> {
    thread.items.iter().find_map(|item| match item {
        crate::thread::ThreadItem::Event(event)
            if event.event == crate::thread::ThreadEventKind::Interrupted =>
        {
            Some(event)
        }
        _ => None,
    })
}

/// Hand `run_id`'s first agent a turn: the human asks, the agent reads it,
/// nothing comes back. That read is what starts the working clock.
pub(in crate::app::tests) fn open_a_turn(state: &Arc<Mutex<AppState>>, run_id: &str) {
    let mut s = state.lock().unwrap();
    let agent_id = {
        let run = s.runs.get_mut(run_id).expect("the run is on the board");
        primary_thread_mut(&mut run.agents).post_user("do the thing", None, "2026-08-15T10:00:00Z");
        primary_thread_mut(&mut run.agents).read_unread("2026-08-15T10:00:01Z");
        assert!(
            primary_thread(&run.agents).working_since().is_some(),
            "the agent read the message, so it holds the turn"
        );
        run.agents.primary().unwrap().id.clone()
    };
    s.record_agent_working_since(run_id, &agent_id, Some("2026-08-15T10:00:01Z".to_string()));
}

/// A killed agent process must not leave its row working.
///
/// The run sits at its review gate, so its STATE claims nothing — the row's
/// working flag comes entirely from the open turn, which is the half the
/// idle sweep cannot reach (it only demotes entities whose state is
/// working). The pump's EOF is where the death is noticed, so that is where
/// the turn closes.
#[tokio::test]
async fn a_killed_agent_process_leaves_its_row_inactive() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let (tab_key, _wire_id) = insert_live_run(&state, &repo, dir.path().join("side"), "run-killed");
    state
        .lock()
        .unwrap()
        .runs
        .get_mut("run-killed")
        .unwrap()
        .run
        .state = RunState::Review;
    open_a_turn(&state, "run-killed");
    assert_eq!(
        feed_row_for_run(&handler, "run-killed")["working"],
        json!(true),
        "a turn in flight is the row working"
    );

    state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&tab_key)
        .unwrap()
        .session
        .end();

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let row = loop {
        let row = feed_row_for_run(&handler, "run-killed");
        if row["working"] == json!(false) && row["unread_reason"] == "interrupted" {
            break row;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the agent's process was killed and the row still claims it is working: {row:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(
        row["unread_reason"], "interrupted",
        "the row says why the work stopped: {row:?}"
    );

    let s = state.lock().unwrap();
    let thread = primary_thread(&s.runs["run-killed"].agents);
    assert_eq!(thread.working_since(), None, "the turn is closed");
    let interruption = interruption_in(thread).expect("the conversation records the death");
    assert!(
        interruption
            .summary
            .as_deref()
            .is_some_and(|said| said.contains("without reporting")),
        "the entry says the session ended without reporting: {interruption:?}"
    );
}

/// A second agent on a run, with its own PTY tab and its own conversation —
/// the shape of a branch carrying more than one agent. Returns its id and
/// the key of the tab it runs in.
fn add_second_agent(state: &Arc<Mutex<AppState>>, run_id: &str) -> (String, TabKey) {
    let (agent_id, root) = {
        let mut s = state.lock().unwrap();
        let active = s.runs.get_mut(run_id).expect("the run is on the board");
        let agent_id = active
            .agents
            .add(run_id, ModelChoice::default(), "2026-08-15T10:00:00Z")
            .id
            .clone();
        let root = AppState::canonical_root(&active.worktree.path);
        (agent_id, root)
    };
    let (mut tab, rx) = Tab::spawn_agent(
        run_id.to_string(),
        agent_id.clone(),
        test_agent_session_request(
            AgentProvider::default(),
            warm_tui_spec(),
            root.clone(),
            terminal_size(120, 40),
        ),
    )
    .expect("the second agent's tab spawns");
    let key = TabKey::agent(&root, &agent_id);
    {
        let mut app = state.lock().unwrap();
        tab.session_instance = app.record_agent_session_start(
            run_id,
            &agent_id,
            &root,
            &ModelChoice::default(),
            "build",
        );
        app.session_registry.test_insert_tab(key.clone(), tab);
    }
    spawn_tab_pumps(state, key.clone(), rx);
    (agent_id, key)
}

/// One agent dying is not every agent dying. The interruption lands on the
/// conversation of the agent whose process ended, and the agent beside it
/// — still running, still mid-turn — keeps its turn and hears nothing.
#[tokio::test]
async fn a_dead_agents_interruption_lands_on_its_own_conversation() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let (_first_key, _wire_id) =
        insert_live_run(&state, &repo, dir.path().join("side"), "run-two-agents");
    open_a_turn(&state, "run-two-agents");
    let (second_id, second_key) = add_second_agent(&state, "run-two-agents");
    {
        let mut s = state.lock().unwrap();
        {
            let second = s.runs.get_mut("run-two-agents").unwrap();
            let second = second.agents.by_id_mut(&second_id).expect("just added");
            second
                .thread
                .post_user("and this one too", None, "2026-08-15T10:00:02Z");
            second.thread.read_unread("2026-08-15T10:00:03Z");
        }
        s.record_agent_working_since(
            "run-two-agents",
            &second_id,
            Some("2026-08-15T10:00:03Z".to_string()),
        );
    }

    state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&second_key)
        .unwrap()
        .session
        .end();

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        {
            let s = state.lock().unwrap();
            let roster = &s.runs["run-two-agents"].agents;
            let second = roster.by_id(&second_id).expect("still on the roster");
            if second.thread.working_since().is_none() {
                assert!(
                    interruption_in(&second.thread).is_some(),
                    "the dead agent's own conversation records it: {:?}",
                    second.thread.items
                );
                assert!(
                    roster.primary().unwrap().thread.working_since().is_some(),
                    "the agent still running keeps its turn"
                );
                assert!(
                    interruption_in(&roster.primary().unwrap().thread).is_none(),
                    "and is told nothing: {:?}",
                    roster.primary().unwrap().thread.items
                );
                break;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the second agent's process was killed and its turn never closed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// A process that exits after the agent handed back is an ordinary ending,
/// not an interruption. The marker exists to close a turn nobody else will
/// ever close, so a conversation with no turn in flight gains nothing.
#[tokio::test]
async fn a_process_that_exits_after_handing_back_records_no_interruption() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let (tab_key, _wire_id) =
        insert_live_run(&state, &repo, dir.path().join("side"), "run-handback");
    state
        .lock()
        .unwrap()
        .runs
        .get_mut("run-handback")
        .unwrap()
        .run
        .state = RunState::Review;
    open_a_turn(&state, "run-handback");
    {
        let mut s = state.lock().unwrap();
        let agent_id = primary_agent_id(&s, "run-handback");
        s.on_agent_mcp_action(
            "run-handback",
            &agent_id,
            BridgeAction::PostThreadMessage {
                body: "here is what I did".to_string(),
                still_working: false,
                options: Vec::new(),
            },
        )
        .unwrap();
        let run = s.runs.get("run-handback").unwrap();
        assert_eq!(
            primary_thread(&run.agents).working_since(),
            None,
            "the reply hands back"
        );
    }

    state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&tab_key)
        .unwrap()
        .session
        .end();

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&tab_key)
        .unwrap()
        .live
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the pump never noticed the process end"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let row = feed_row_for_run(&handler, "run-handback");
    assert_eq!(row["working"], json!(false), "{row:?}");
    let s = state.lock().unwrap();
    assert!(
        interruption_in(primary_thread(&s.runs["run-handback"].agents)).is_none(),
        "a handed-back turn is not interrupted: {:?}",
        primary_thread(&s.runs["run-handback"].agents).items
    );
}

/// A worktree deleted out-of-band (`rm -rf` by hand — nothing went through
/// a Build verb) takes its agent tab through `reap_orphaned_terminals`,
/// which removes the tab from the registry BEFORE killing the process. The
/// pump's EOF handler looks the tab up and finds nothing, so the reaper is
/// the only place left that knows this session ended — and the run stays on
/// the board. If it forgot to close the turn, the row would read working
/// forever: the exact bug the pump path already fixes, through a different
/// exit.
///
/// The run is Merged — a run kept with `cleanup=keep`, its directory later
/// deleted by hand while a follow-up question had its agent mid-turn. That
/// is the shape where nothing else ever cleans up: `board.list`'s archive
/// sweep skips terminal runs, so this row is on the board for good, and the
/// reaper's close is the only one it will ever get.
#[tokio::test]
async fn the_reaper_closes_the_turn_of_an_agent_whose_worktree_vanished() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let (tab_key, wire_id) =
        insert_live_run(&state, &repo, dir.path().join("side"), "run-vanished");
    state
        .lock()
        .unwrap()
        .runs
        .get_mut("run-vanished")
        .unwrap()
        .run
        .state = RunState::Merged;
    open_a_turn(&state, "run-vanished");
    assert_eq!(
        feed_row_for_run(&handler, "run-vanished")["working"],
        json!(true),
        "a turn in flight is the row working"
    );

    std::fs::remove_dir_all(&tab_key.root).expect("the user rm -rfs the checkout");
    let reaped = state.lock().unwrap().reap_orphaned_terminals();
    assert!(
        reaped.contains(&wire_id),
        "the reaper closes the vanished worktree's tab: {reaped:?}"
    );

    let row = feed_row_for_run(&handler, "run-vanished");
    assert_eq!(
        row["working"],
        json!(false),
        "the run stays on the board and its row leaves working: {row:?}"
    );
    assert_eq!(
        row["unread_reason"], "interrupted",
        "the row says why the work stopped: {row:?}"
    );
    let s = state.lock().unwrap();
    let thread = primary_thread(&s.runs["run-vanished"].agents);
    assert_eq!(thread.working_since(), None, "the turn is closed");
    let interruption = interruption_in(thread).expect("the conversation records the death");
    assert!(
        interruption
            .summary
            .as_deref()
            .is_some_and(|said| said.contains("without reporting")),
        "the entry says the session ended without reporting: {interruption:?}"
    );
}

/// Point a QA state's only project at a different agent — the seam every
/// test that cares about what actually gets spawned goes through.
fn use_agent(state: &Arc<Mutex<AppState>>, repo: &std::path::Path, wt: PathBuf, agent: Agent) {
    state.lock().unwrap().project_at_mut(0).orch = Orchestrator::new(
        repo.to_path_buf(),
        wt,
        agent,
        Templates::default(),
        test_bridge_exe(),
    );
}

fn capture_turns(
    state: &Arc<Mutex<AppState>>,
    repo: &std::path::Path,
    dir: &std::path::Path,
) -> PathBuf {
    let capture = dir.join("named-agent-stdin.txt");
    let path = capture.clone();
    use_agent(
        state,
        repo,
        dir.join("wt-capture"),
        Agent::WarmBuilder(std::sync::Arc::new(move |_, _, _| {
            Ok(HarnessSpec::new("sh")
                .arg("-c")
                .arg("cat > \"$1\"")
                .arg("build-agent-capture")
                .arg(path.to_string_lossy()))
        })),
    );
    capture
}

async fn capture_containing(path: &std::path::Path, needle: &str) -> String {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(contents) = std::fs::read_to_string(path) {
                if contents.contains(needle) {
                    return contents;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the agent receives the turn")
}

#[tokio::test]
async fn legacy_post_asks_an_unnamed_agent_once_when_its_turn_is_sent() {
    let (dir, repo) = init_repo();
    let owner = "run-legacy-name";
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), owner);
    let capture = capture_turns(&state, &repo, dir.path());
    let agent_id = crate::agent::derived_agent_id(owner);
    let posted = state.lock().unwrap().handle(req(
        "thread.post",
        json!({ "entity_id": owner, "agent_id": agent_id, "body": "first legacy message" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    deliver_pending_agent_turns(&state);
    let first = capture_containing(&capture, "first legacy message").await;
    assert!(first.contains("call set_name"), "{first}");
    assert!(
        state
            .lock()
            .unwrap()
            .entity_agents(owner)
            .unwrap()
            .by_id(&agent_id)
            .unwrap()
            .name_asked
    );

    deliver(
        &state,
        &root,
        owner,
        &agent_id,
        &ModelChoice::default(),
        "followup",
        ["second turn", "second turn"],
    )
    .unwrap();
    let second = capture_containing(&capture, "second turn").await;
    assert_eq!(
        second.matches("You have no name yet").count(),
        1,
        "{second}"
    );
}

/// An agent needs a folder, not a repository (#297): a project on a folder
/// with no git spawns its project agent and delivers to it, and nothing runs
/// `git init` on the user's behalf.
#[tokio::test]
async fn a_project_agent_is_delivered_to_in_a_folder_with_no_git() {
    let dir = tempfile::tempdir().unwrap();
    let plain = std::fs::canonicalize(dir.path()).unwrap().join("draft");
    std::fs::create_dir(&plain).unwrap();
    std::fs::write(plain.join("notes.txt"), "keep me\n").unwrap();
    let (state, _handler) = shared_state_and_handler(&plain, dir.path());
    let capture = capture_turns(&state, &plain, dir.path());
    let project_id = {
        let s = state.lock().unwrap();
        assert!(!s.project_at(0).is_git);
        s.project_at(0).id.clone()
    };
    let (owner, agent_id) =
        crate::app::tests::project_agent::project_agent(&mut state.lock().unwrap(), &project_id);

    let posted = state.lock().unwrap().handle(req(
        "thread.post",
        json!({ "entity_id": owner, "agent_id": agent_id, "body": "hello from a plain folder" }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    deliver_pending_agent_turns(&state);

    capture_containing(&capture, "hello from a plain folder").await;
    assert!(!plain.join(".git").exists());
}

#[tokio::test]
async fn direct_task_notice_asks_an_unnamed_agent_on_its_first_turn() {
    let (dir, repo) = init_repo();
    let owner = "run-notice-name";
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), owner);
    let capture = capture_turns(&state, &repo, dir.path());
    let agent_id = crate::agent::derived_agent_id(owner);
    {
        let mut app = state.lock().unwrap();
        let agent = app
            .runs
            .get_mut(owner)
            .unwrap()
            .agents
            .primary_mut()
            .unwrap();
        agent
            .thread
            .post_user_from_build("Task moved", crate::store::now_rfc3339());
        let conversation_id = agent.conversation_id().to_string();
        app.delivery_queue.enqueue(PendingAgentTurn {
            operation_id: None,
            root: AppState::canonical_root(&root),
            owner: owner.to_string(),
            agent_id: agent_id.clone(),
            conversation_id,
            model_choice: ModelChoice::default(),
            choice_revision: 0,
            interrupt: false,
            say: Some(TurnText {
                cold: crate::orchestrator::conversation_prompt(NEW_THREAD_MESSAGES_PROMPT),
                warm: NEW_THREAD_MESSAGES_PROMPT.to_string(),
            }),
            phase: "task_notice",
            wants_catch_up: true,
            survives_refusal: false,
        });
    }
    deliver_pending_agent_turns(&state);
    let heard = capture_containing(&capture, "Task moved").await;
    assert!(heard.contains("call set_name"), "{heard}");
}

/// The rendered turn is always multi-line (the conversation protocol block
/// is appended), so through a real TUI it must arrive as ONE bracketed
/// paste. The capture harness never paints, so this also proves the
/// readiness grace expires into a write rather than a silently lost prompt.
#[tokio::test]
async fn a_delivered_turn_reaches_a_silent_harness_as_one_bracketed_paste() {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-paste");
    let capture = dir.path().join("agent-stdin.txt");
    let capture_for_builder = capture.clone();
    use_agent(
        &state,
        &repo,
        dir.path().join("wt2"),
        Agent::WarmBuilder(std::sync::Arc::new(
            move |_prompt: &str, _choice: &ModelChoice, _options: &SpawnOptions| {
                Ok(HarnessSpec::new("sh")
                    .arg("-c")
                    .arg("cat > \"$1\"")
                    .arg("build-agent-capture")
                    .arg(capture_for_builder.to_string_lossy()))
            },
        )),
    );

    deliver(
        &state,
        &root,
        "run-paste",
        &crate::agent::derived_agent_id("run-paste"),
        &ModelChoice::default(),
        "build",
        ["Paste framing marker\nsecond line", "warm"],
    )
    .expect("the delivery reaches a silent harness");

    let captured = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(contents) = std::fs::read_to_string(&capture) {
                if contents.contains("\u{1b}[201~") {
                    return contents;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the framed turn should reach the harness despite its silence");

    assert!(
        captured.starts_with("\u{1b}[200~"),
        "the turn opens as a bracketed paste: {captured:?}"
    );
    assert!(
        captured.contains("Paste framing marker"),
        "the rendered turn rides inside the frame: {captured:?}"
    );
}

/// A fresh session is recorded before its output pump can observe EOF. An
/// immediately exiting child used to let the pump record the end first and
/// the delivery record a stale open session afterward.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_immediately_exiting_harness_leaves_one_closed_session() {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-exits");
    use_agent(&state, &repo, dir.path().join("wt2"), instant_exit_agent());
    let agent_id = crate::agent::derived_agent_id("run-exits");
    {
        let mut app = state.lock().unwrap();
        app.delivery_queue.enqueue(PendingAgentTurn {
            operation_id: None,
            root: AppState::canonical_root(&root),
            owner: "run-exits".into(),
            agent_id: agent_id.clone(),
            conversation_id: agent_id.clone(),
            model_choice: ModelChoice::default(),
            choice_revision: 0,
            interrupt: false,
            say: Some(TurnText {
                cold: "cold".into(),
                warm: "warm".into(),
            }),
            phase: "build",
            wants_catch_up: false,
            survives_refusal: false,
        });
    }

    deliver_pending_agent_turns(&state);

    let key = TabKey::agent(&AppState::canonical_root(&root), &agent_id);
    wait_for(Duration::from_secs(5), || {
        let s = state.lock().unwrap();
        let thread = primary_thread(&s.runs["run-exits"].agents);
        (!s.session_registry.test_tab(&key).unwrap().live
            && thread.items.iter().any(|item| {
                matches!(
                    item,
                    crate::thread::ThreadItem::Event(event)
                        if event.event == crate::thread::ThreadEventKind::SessionEnded
                )
            }))
        .then_some(())
    })
    .await
    .expect("the pump records the child exit in the conversation");
    let s = state.lock().unwrap();
    let thread = primary_thread(&s.runs["run-exits"].agents);
    assert_eq!(
        thread
            .sessions
            .iter()
            .filter(|session| session.ended_at.is_none())
            .count(),
        0,
        "EOF must not be followed by a stale session start: {:?}",
        thread.sessions
    );
    let session_events = thread
        .items
        .iter()
        .filter_map(|item| match item {
            crate::thread::ThreadItem::Event(event)
                if matches!(
                    event.event,
                    crate::thread::ThreadEventKind::RunStarted
                        | crate::thread::ThreadEventKind::SessionEnded
                ) =>
            {
                Some((event.event, event.sequence))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        session_events
            .iter()
            .filter(|(kind, _)| *kind == crate::thread::ThreadEventKind::SessionEnded)
            .count(),
        1,
        "the process produces exactly one session end: {session_events:?}"
    );
    assert_eq!(
        session_events.len(),
        2,
        "one start and one end are the whole session timeline: {session_events:?}"
    );
    assert_eq!(
        session_events[0].0,
        crate::thread::ThreadEventKind::RunStarted,
        "the fresh session starts before it can end: {session_events:?}"
    );
    assert_eq!(
        session_events[1].0,
        crate::thread::ThreadEventKind::SessionEnded,
        "no stale start may appear after the end: {session_events:?}"
    );
    assert!(
        session_events[0].1 < session_events[1].1,
        "the recorded start precedes the recorded end: {session_events:?}"
    );
    assert!(
        !s.session_registry.test_tab(&key).unwrap().live,
        "the retained tab is non-live"
    );
}

/// An agent that is gone before it reads a byte.
fn instant_exit_agent() -> Agent {
    Agent::WarmBuilder(std::sync::Arc::new(
        |_prompt: &str, _choice: &ModelChoice, _options: &SpawnOptions| {
            Ok(HarnessSpec::new("sh").arg("-c").arg("exit 0"))
        },
    ))
}

/// The daemon-wide terminal cap counts the human's own shells WHEREVER they
/// are held — including the tab registry — and never counts an agent tab.
///
/// Both halves have teeth. A shell that escapes the cap by living on the
/// new registry is the cap quietly doubling; an agent that sixteen open
/// shells could crowd out is not "always reachable", which is the
/// invariant's direct negation. So the fixture holds one agent tab and one
/// shell tab: exactly fifteen more shells must fit, and the sixteenth must
/// not.
#[tokio::test]
async fn the_terminal_cap_counts_shell_tabs_and_never_the_agent() {
    let (dir, repo) = init_repo();
    let (state, handler, root) = agent_tab_fixture(&repo, dir.path(), "run-cap");
    let project_id = state.lock().unwrap().project_at(0).id.clone();

    ensure_agent_tab(
        &state,
        &root,
        "run-cap",
        &crate::agent::derived_agent_id("run-cap"),
        &ModelChoice::default(),
        "start",
    )
    .unwrap();
    // One of the human's own shells, held as a tab rather than in `terms`.
    let shell_root = AppState::canonical_root(&repo);
    let shell_key = TabKey {
        root: shell_root.clone(),
        tab_id: "term-99".to_string(),
    };
    let (shell_tab, _shell_rx) = Tab::spawn_shell(
        &shell_harness_spec("/bin/bash"),
        "term-99".to_string(),
        shell_root,
        terminal_size(80, 24),
    )
    .expect("a shell tab spawns");
    state
        .lock()
        .unwrap()
        .session_registry
        .test_insert_tab(shell_key, shell_tab);

    // The agent takes none of the sixteen, so fifteen more shells fit
    // beside the one shell tab...
    for n in 1..MAX_USER_TERMINALS {
        let created = handler.call(
            SessionSender::detached("s1"),
            req("term.create", json!({ "project_id": project_id })),
        );
        assert_eq!(created["ok"], true, "shell {n} of the cap: {created:?}");
    }
    // ...and the sixteenth does not: the shell tab is one of them.
    let over = handler.call(
        SessionSender::detached("s1"),
        req("term.create", json!({ "project_id": project_id })),
    );
    assert_eq!(over["ok"], false, "the shell tab is one of the sixteen");
    assert!(
        over["error"]
            .as_str()
            .unwrap_or_default()
            .contains("terminal limit reached"),
        "{over:?}"
    );
}

#[test]
fn mcp_control_frames_require_the_current_session_token() {
    let mut sessions = SessionRegistry::new();
    sessions.test_install_token("run-1".to_string(), "secret-current".to_string());
    let valid = json!({
        "task_id": "run-1",
        "session_token": "secret-current",
        "report": { "phase": "build", "status": "completed", "summary": "done", "outputs": {} }
    });
    assert_eq!(authenticated_mcp_owner(&valid, &sessions), Some("run-1"));
    assert_eq!(
        authenticated_mcp_owner(
            &json!({ "task_id": "run-1", "session_token": "stale" }),
            &sessions
        ),
        None
    );
    assert_eq!(
        authenticated_mcp_owner(&json!({ "task_id": "run-1" }), &sessions),
        None
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_listener_rejects_rotated_and_wrong_tokens_and_keeps_canonical_errors() {
    async fn request(path: &Path, frame: Value) -> Value {
        let mut stream = tokio::net::UnixStream::connect(path).await.unwrap();
        stream
            .write_all(format!("{frame}\n").as_bytes())
            .await
            .unwrap();
        let mut response = String::new();
        tokio::io::BufReader::new(stream)
            .read_line(&mut response)
            .await
            .unwrap();
        serde_json::from_str(&response).unwrap()
    }

    let (directory, repo) = init_repo();
    let mut app = qa_state(&repo, directory.path());
    let entity_id = adopted_run(&mut app, &repo, directory.path(), "authenticate-mcp");
    let agent_id = primary_agent_id(&app, &entity_id);
    let state = app.shared();
    let (stale_reservation, current_reservation, stale_token, current_token) = {
        let mut app = state.lock().unwrap();
        let root = app.entity_agent_root(&entity_id).unwrap();
        let key = TabKey::agent(&root, &agent_id);
        let conversation_id = app
            .resolve_conversation_address(&entity_id, Some(&agent_id))
            .unwrap()
            .conversation_id;
        let choice = ModelChoice::default();
        let stale = reserve_agent_spawn(
            &mut app,
            &key,
            &AgentSpawnRequest {
                owner: &entity_id,
                agent_id: &agent_id,
                conversation_id: &conversation_id,
                model_choice: &choice,
                force_fresh: false,
                phase: "test",
            },
        )
        .unwrap()
        .holding;
        let stale_token = stale.test_session_token().to_string();
        let current = reserve_agent_spawn(
            &mut app,
            &key,
            &AgentSpawnRequest {
                owner: &entity_id,
                agent_id: &agent_id,
                conversation_id: &conversation_id,
                model_choice: &choice,
                force_fresh: false,
                phase: "test",
            },
        )
        .unwrap()
        .holding;
        let current_token = current.test_session_token().to_string();
        (stale, current, stale_token, current_token)
    };
    let socket = directory.path().join("authenticated-mcp.sock");
    let listener = bind_done_listener(&socket).unwrap();
    let server = tokio::spawn(serve_done_listener(Arc::clone(&state), listener));

    for token in [stale_token.as_str(), "wrong-token"] {
        let response = request(
            &socket,
            json!({
                "task_id": agent_id,
                "session_token": token,
                "request": { "action": "set_topic", "topic": "authentication" }
            }),
        )
        .await;
        assert_eq!(
            response,
            json!({ "ok": false, "error": "unauthorized MCP session" })
        );
    }

    let accepted = request(
        &socket,
        json!({
            "task_id": agent_id,
            "session_token": current_token,
            "request": { "action": "set_topic", "topic": "authentication" }
        }),
    )
    .await;
    assert_eq!(accepted["ok"], true, "{accepted}");

    let done = crate::mcp::DoneServer::for_owner(&agent_id).handle_message(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"done","arguments":{"phase":"build"}}}"#,
    );
    let done_error: Value = serde_json::from_str(done.reply.as_deref().unwrap()).unwrap();
    assert_eq!(done_error["result"]["isError"], true);
    assert_eq!(
        done_error["result"]["content"][0]["text"],
        "unknown tool: done"
    );

    let post = crate::mcp::DoneServer::for_owner(&agent_id).handle_message(
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"post_thread_message","arguments":{"status":"Waiting"}}}"#,
    );
    let post_error: Value = serde_json::from_str(post.reply.as_deref().unwrap()).unwrap();
    assert_eq!(post_error["result"]["isError"], true);
    assert_eq!(
        post_error["result"]["content"][0]["text"],
        "body is required"
    );

    server.abort();
    drop(current_reservation);
    drop(stale_reservation);
}

#[cfg(unix)]
#[tokio::test]
async fn done_socket_is_explicitly_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mcp.sock");
    let _listener = bind_done_listener(&path).expect("bind private MCP socket");
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
}

/// An agent tab is a MANAGED agent: the spec it spawns from carries Build's
/// `done` MCP server and the owner id that routes reports back through the
/// owner lookup. The socket lives inside the harness builder's closure, so
/// launch preparation is the only way the app layer can reach it — and a
/// prepared spec that dropped the config or the owner would open an agent
/// Build cannot talk to, in a tab that looks entirely healthy.
#[test]
fn prepared_agent_spec_carries_the_done_mcp_server_and_the_owner_id() {
    // claude's pre-trust writes a registry; keep it off the developer's own.
    let config_dir = tempfile::tempdir().unwrap();
    std::env::set_var("CLAUDE_CONFIG_DIR", config_dir.path());
    let (directory, repo) = init_repo();
    let cwd = directory.path().join("wt-1");
    std::fs::create_dir(&cwd).unwrap();
    let orch = Orchestrator::new(
        repo,
        directory.path().join("worktrees"),
        test_build_agent("/tmp/build mcp.sock"),
        Templates::default(),
        test_bridge_exe(),
    );
    let claude = ModelChoice {
        provider: AgentProvider::Claude,
        model: None,
        effort: None,
    };
    let codex = ModelChoice {
        provider: AgentProvider::Codex,
        model: None,
        effort: None,
    };

    let spec = orch
        .agent_launch()
        .prepare(
            "agent-42",
            crate::orchestrator::LaunchDirs::at(&cwd),
            &claude,
            false,
            None,
            "token-42",
        )
        .unwrap();
    assert_eq!(spec.spec.binary, "claude");
    let args = spec.spec.args.join(" ");
    assert!(
        args.contains("--mcp-config .build/mcp-agent-42.json --strict-mcp-config"),
        "the harness reads the config written for THIS agent: {args}"
    );
    assert!(!args.contains("--continue"), "{args}");
    assert!(
        spec.spec
            .env
            .iter()
            .any(|(key, value)| key == "BRIDGE_MCP_SOCKET" && value == "/tmp/build mcp.sock"),
        "{:?}",
        spec.spec.env
    );
    // A replaced tab picks its own conversation back up.
    let resumed = orch
        .agent_launch()
        .prepare(
            "run-42",
            crate::orchestrator::LaunchDirs::at(&cwd),
            &claude,
            true,
            None,
            "token-43",
        )
        .unwrap();
    assert!(resumed.spec.args.join(" ").contains("--continue"));

    let spec = orch
        .agent_launch()
        .prepare(
            "run-42",
            crate::orchestrator::LaunchDirs::at(&cwd),
            &codex,
            false,
            None,
            "token-42",
        )
        .unwrap();
    assert_eq!(spec.spec.binary, "codex");
    let args = spec.spec.args.join(" ");
    assert!(
        args.contains(r#"mcp_servers.build.args=["mcp","--task","run-42"]"#),
        "{args}"
    );
    assert!(
        args.contains(r#"mcp_servers.build.env.BRIDGE_MCP_SOCKET="/tmp/build mcp.sock""#),
        "{args}"
    );
    assert!(
        args.contains(&format!(
            r#"projects."{}".trust_level="trusted""#,
            cwd.display()
        )),
        "{args}"
    );
    assert!(!args.ends_with("resume --last"), "{args}");
    let resumed = orch
        .agent_launch()
        .prepare(
            "run-42",
            crate::orchestrator::LaunchDirs::at(&cwd),
            &codex,
            true,
            None,
            "token-43",
        )
        .unwrap();
    assert!(resumed.spec.args.join(" ").ends_with("resume --last"));

    std::env::remove_var("CLAUDE_CONFIG_DIR");
}

#[test]
fn pi_launch_identity_reaches_the_session_through_tab_spawn() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().unwrap();
    let repo = init_repo_named(directory.path(), "repo");
    let worktree = directory.path().join("pi-worktree");
    std::fs::create_dir(&worktree).unwrap();
    let state_root = directory.path().join("state");
    let context = HarnessContext::resolved(directory.path().join("mcp.sock"), state_root)
        .expect("resolve isolated Pi context");
    let orchestrator = Orchestrator::new(
        repo,
        directory.path().join("worktrees"),
        build_agent(false, context),
        Templates::default(),
        test_bridge_exe(),
    );
    let agent_id = "agent-pi-tab-spawn";
    let mut prepared = orchestrator
        .agent_launch()
        .prepare(
            agent_id,
            crate::orchestrator::LaunchDirs::at(&worktree),
            &ModelChoice {
                provider: AgentProvider::Pi,
                model: None,
                effort: None,
            },
            false,
            None,
            "pi-token",
        )
        .expect("prepare the production Pi launch");
    let fake_pi = directory.path().join("pi");
    std::fs::write(
        &fake_pi,
        "#!/bin/sh\nprintf '\\033[?2004h'\ncat >/dev/null\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake_pi, std::fs::Permissions::from_mode(0o700)).unwrap();
    prepared.spec.binary = fake_pi.to_string_lossy().into_owned();

    let (tab, _output) = Tab::spawn_agent(
        "run-pi-identity".to_string(),
        agent_id.to_string(),
        agent_open_request(
            prepared,
            AppState::canonical_root(&worktree),
            &ModelChoice {
                provider: AgentProvider::Pi,
                model: None,
                effort: None,
            },
            None,
            None,
        ),
    )
    .expect("the Pi-shaped tab spawns");

    assert_eq!(
        tab.session.session_id().as_deref(),
        Some(agent_id),
        "Tab::spawn must carry Pi's launch-known identity into AgentSession"
    );
    tab.session.end();
}

#[test]
fn concurrent_fresh_sessions_never_trust_the_same_first_writer_locator() {
    struct RacedLocator;
    impl crate::harness::SessionLocator for RacedLocator {
        fn session_id(&self) -> Option<String> {
            Some("session-written-first-by-the-sibling".to_string())
        }
    }

    let request = |_agent_id: &str| {
        agent_open_request(
            PreparedAgentLaunch {
                spec: HarnessSpec::new("true"),
                pty_size: terminal_size(80, 24),
            },
            PathBuf::from("/tmp/shared-checkout"),
            &ModelChoice::default(),
            None,
            Some(Box::new(RacedLocator)),
        )
    };
    let first = request("agent-a");
    let second = request("agent-b");
    assert!(first.terminal.identity.is_none());
    assert!(second.terminal.identity.is_none());
}

fn pi_extension_child_death_spec(directory: &Path, agent_id: &str) -> HarnessSpec {
    use std::os::unix::fs::PermissionsExt;

    let extension = directory.join("build-tools.mjs");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/harness/build-tools.ts"),
        &extension,
    )
    .unwrap();
    let child = directory.join("fake-build-bridge");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pi-mcp-child.mjs"),
        &child,
    )
    .unwrap();
    std::fs::set_permissions(&child, std::fs::Permissions::from_mode(0o700)).unwrap();

    HarnessSpec::new("node")
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/pi-extension-driver.mjs")
                .to_string_lossy()
                .into_owned(),
        )
        .known_session_id(agent_id)
        .env("BUILD_PI_EXTENSION_PATH", extension.to_string_lossy())
        .env("BUILD_PI_MCP_COMMAND", child.to_string_lossy())
        .env("BUILD_PI_MCP_OWNER", agent_id)
        .env("BRIDGE_MCP_SOCKET", "/tmp/build-pi-death.sock")
        .env("BRIDGE_MCP_TOKEN", "pi-death-token")
        .env("BUILD_PI_MCP_TIMEOUT_MS", "5000")
        .env("FAKE_MCP_MODE", "child_exit")
        .env("PI_DRIVER_SCENARIO", "happy")
}

#[tokio::test]
async fn pi_mcp_child_death_runs_the_normal_tab_exit_path() {
    let (directory, repo) = init_repo();
    let mut app = qa_state(&repo, directory.path());
    let run_id = "run-pi-mcp-death";
    let root = insert_run(
        &mut app,
        &repo,
        directory.path(),
        run_id,
        RunState::Building,
    );
    let agent_id = crate::agent::derived_agent_id(run_id);
    let key = TabKey::agent(&root, &agent_id);
    let choice = ModelChoice {
        provider: AgentProvider::Pi,
        model: None,
        effort: None,
    };
    app.runs.get_mut(run_id).unwrap().model_choice = choice.clone();
    app.runs
        .get_mut(run_id)
        .unwrap()
        .agents
        .resolve_mut(None)
        .unwrap()
        .choice = choice.clone();
    let instance = app.record_agent_session_start(run_id, &agent_id, &root, &choice, "build");
    let (tab, output) = Tab::spawn_agent(
        run_id.to_string(),
        agent_id.clone(),
        agent_open_request(
            PreparedAgentLaunch {
                spec: pi_extension_child_death_spec(directory.path(), &agent_id),
                pty_size: terminal_size(120, 40),
            },
            root,
            &choice,
            None,
            None,
        ),
    )
    .expect("the Pi extension fixture starts through a PTY");
    assert_eq!(tab.session.session_id().as_deref(), Some(agent_id.as_str()));
    let (sender, mut pushes, session_key) = SessionSender::observable("pi-death-observer");
    screen_of(&tab).attach(&sender, None);
    app.session_registry.test_insert_tab(key.clone(), tab);
    app.session_registry
        .test_tab_mut(&key)
        .unwrap()
        .session_instance = instance;
    let state = app.shared();
    spawn_tab_pumps(&state, key.clone(), output);

    wait_for_push(&mut pushes, &session_key, |push| {
        push["type"] == "term.closed"
            && push["term_id"] == key.tab_id
            && push["reason"] == "agent_session_ended"
    })
    .await;
    wait_for(Duration::from_secs(5), || {
        (open_session_count(&state, run_id) == 0).then_some(())
    })
    .await
    .expect("normal exit handling closes the conversation session");

    let app = state.lock().unwrap();
    let retained = &app.session_registry.test_tab(&key).unwrap();
    assert!(!retained.live, "the retained Pi tab must be non-live");
    assert!(
        matches!(retained.session.status(), AgentStatus::Ended { .. }),
        "the Pi process must be reaped as ended"
    );
    assert!(primary_thread(&app.runs[run_id].agents)
        .items
        .iter()
        .any(|item| matches!(
            item,
            crate::thread::ThreadItem::Event(event)
                if event.event == crate::thread::ThreadEventKind::SessionEnded
        )));
}

/// The tab spawns the spec the orchestrator built FOR IT: the run as
/// `owner_id`, the canonical worktree as cwd, and — for an owner holding no
/// record of a conversation, the shape a router has — no resume of any
/// kind, however much the checkout already holds. The empty prompt is the
/// contract too: a turn never rides in argv, it travels through the PTY.
#[tokio::test]
async fn an_agent_tab_spawns_the_harness_the_orchestrator_built() {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-wired");
    let specs_built: Arc<Mutex<Vec<SpawnOptions>>> = Arc::new(Mutex::new(Vec::new()));
    {
        let recorder = Arc::clone(&specs_built);
        let agent = Agent::WarmBuilder(Arc::new(
            move |prompt: &str, _choice: &ModelChoice, options: &SpawnOptions| {
                assert!(prompt.is_empty(), "a turn never rides in argv: {prompt:?}");
                recorder.lock().unwrap().push(options.clone());
                Ok(HarnessSpec::new("sh").arg("-c").arg(
                    "printf 'SPEC-FROM-THE-ORCHESTRATOR'; printf '\\033[?2004h'; cat >/dev/null",
                ))
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

    ensure_agent_tab(
        &state,
        &root,
        "run-wired",
        &crate::agent::derived_agent_id("run-wired"),
        &ModelChoice::default(),
        "start",
    )
    .expect("the agent spawns");

    let built = specs_built.lock().unwrap().clone();
    assert_eq!(built.len(), 1, "one spawn, one spec: {built:?}");
    assert_eq!(
        built[0].owner_id,
        crate::agent::derived_agent_id("run-wired"),
        "`done` names the AGENT that sent it; the entity is a lookup away"
    );
    assert_eq!(
        built[0].cwd,
        AppState::canonical_root(&root),
        "the spec is built for the canonical root"
    );
    assert!(
        !built[0].continue_session,
        "an owner with no record of a conversation inherits nobody else's"
    );
    let screen = wait_for_agent_screen(&state, &root, "SPEC-FROM-THE-ORCHESTRATOR").await;
    assert!(
        screen.contains("SPEC-FROM-THE-ORCHESTRATOR"),
        "the tab runs the orchestrator's spec: {screen:?}"
    );
}

#[test]
fn a_harness_spec_error_releases_the_reservation_and_never_spawns() {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-spec-fail");
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let attempts = Arc::clone(&attempts);
        let agent = Agent::WarmBuilder(Arc::new(move |_, _, _| {
            attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(HarnessError::Setup("injected Pi setup failure".to_string()))
        }));
        let mut app = state.lock().unwrap();
        let worktrees = app.worktrees_root.clone();
        app.project_at_mut(0).orch = Orchestrator::new(
            repo.clone(),
            worktrees,
            agent,
            Templates::default(),
            test_bridge_exe(),
        );
    }
    let agent_id = crate::agent::derived_agent_id("run-spec-fail");
    let error = ensure_agent_tab(
        &state,
        &root,
        "run-spec-fail",
        &agent_id,
        &ModelChoice::default(),
        "start",
    )
    .unwrap_err();
    assert!(error.contains("injected Pi setup failure"), "{error}");
    assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
    let app = state.lock().unwrap();
    let key = derived_agent_key(&root, "run-spec-fail");
    assert!(!app.session_registry.claim_is_held(&key));
    assert!(app.session_registry.test_token(&agent_id).is_none());
    assert!(!app.session_registry.contains(&key));
}

/// The registry key is the CANONICAL worktree path, so the same worktree
/// reaching the daemon by a different spelling — a run scope hands back
/// `worktrees_root/<name>` uncanonicalized while an external worktree is
/// already canonical, and on macOS `/tmp` is `/private/tmp` — is one tab,
/// not two agents in one directory.
#[tokio::test]
async fn the_agent_tab_key_survives_the_same_root_by_another_path() {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-alias");
    let alias = dir.path().join("alias-root");
    std::os::unix::fs::symlink(&root, &alias).unwrap();

    let (direct_id, direct) = ensure_agent_tab(
        &state,
        &root,
        "run-alias",
        &crate::agent::derived_agent_id("run-alias"),
        &ModelChoice::default(),
        "start",
    )
    .unwrap();
    let (aliased_id, aliased) = ensure_agent_tab(
        &state,
        &alias,
        "run-alias",
        &crate::agent::derived_agent_id("run-alias"),
        &ModelChoice::default(),
        "start",
    )
    .unwrap();

    assert_eq!(direct, Spawned::Fresh);
    assert_eq!(aliased, Spawned::Warm, "the alias finds the same tab");
    assert_eq!(direct_id, aliased_id);
    assert_eq!(state.lock().unwrap().session_registry.test_counts().tabs, 1);
}

/// One registry, one detach loop: a closed relay session must come off
/// EVERY tab's screen — the agent's as much as a shell's. A pump still
/// encrypting output into a session the relay has dropped fails silently
/// and shows up only as CPU.
#[tokio::test]
async fn a_close_frame_detaches_the_sessions_terminal_sender() {
    let (dir, repo) = init_repo();
    let (state, handler, root) = agent_tab_fixture(&repo, dir.path(), "run-detach");
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    let (agent_wire_id, _) = ensure_agent_tab(
        &state,
        &root,
        "run-detach",
        &crate::agent::derived_agent_id("run-detach"),
        &ModelChoice::default(),
        "start",
    )
    .unwrap();
    handler.call(
        SessionSender::detached("s-live"),
        req("term.create", json!({ "project_id": project_id })),
    );
    for session_id in ["s-live", "s-dead"] {
        handler.call(
            SessionSender::detached(session_id),
            req("term.attach", json!({ "term_id": "term-1" })),
        );
        handler.call(
            SessionSender::detached(session_id),
            req("term.attach", json!({ "term_id": agent_wire_id.clone() })),
        );
    }

    let close = Frame {
        session_id: "s-dead".into(),
        message_id: String::new(),
        frame_type: transport::CLOSE_FRAME_TYPE.into(),
        sender: transport::SENDER_DEVICE.into(),
        created_at: String::new(),
        payload: Value::Null,
    };
    let peers = state.lock().unwrap().peers_slot();
    let response = dispatch_frame(
        &state,
        &peers,
        SessionSender::detached("s-dead"),
        close,
        FrameClock::new().frame("close"),
    );
    assert_eq!(response["ok"], true);

    let s = state.lock().unwrap();
    let attached_to = |wire_id: &str| -> Vec<String> {
        s.session_registry
            .test_tabs()
            .map(|(_, tab)| tab)
            .find(|tab| tab.wire_id() == wire_id)
            .map(screen_of)
            .expect("the tab is still registered")
            .attached_sessions()
    };
    assert_eq!(
        attached_to("term-1"),
        vec!["s-live".to_string()],
        "only the closed session's sender is dropped from a shell"
    );
    assert_eq!(
        attached_to(&agent_wire_id),
        vec!["s-live".to_string()],
        "…and from the agent tab too"
    );
}
