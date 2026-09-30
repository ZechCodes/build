use super::*;

/// Shared state + handler for the keyed-terminal tests: the handler drives
/// the RPC surface while the state handle lets tests inspect internals.
/// Point a fixture's account at the provider with a terminal. A test that
/// reaches for an agent's pid, or spawns the warm TUI spec, is a test about
/// a PTY session — so it says which provider it means instead of riding
/// whatever the account's Claude Code mode happens to be.
pub(in crate::app::tests) fn on_the_terminal_provider(state: &Arc<Mutex<AppState>>) {
    state.lock().unwrap().default_harness = AgentProvider::Claude;
}

pub(in crate::app::tests) fn shared_state_and_handler(
    repo: &std::path::Path,
    dir: &std::path::Path,
) -> (Arc<Mutex<AppState>>, FrameHandler) {
    state_and_handler_timed_by(FrameClock::new(), repo, dir)
}

/// The same, on a clock the test chose — the one whose slow-frame lines it
/// means to read back.
fn state_and_handler_timed_by(
    clock: Arc<FrameClock>,
    repo: &std::path::Path,
    dir: &std::path::Path,
) -> (Arc<Mutex<AppState>>, FrameHandler) {
    let mut app = AppState::new(
        repo.to_path_buf(),
        dir.join("wt"),
        "main",
        true,
        "/tmp/test-mcp.sock",
    );
    // Deterministic terminals for tests: plain bash regardless of the dev
    // machine's login shell (production resolves the user's own shell).
    app.term_shell = "/bin/bash".into();
    app.frame_clock = clock;
    let state = app.shared();
    let handler = AppState::handler(Arc::clone(&state));
    (state, handler)
}

/// The verb that says what the daemon is doing must not wait on the daemon
/// doing it. `bridge.stats` reads the frame clock and never the state, so
/// the moment it is needed — a frame parked on the app mutex — is the
/// moment it still answers, naming the method that is holding it.
#[test]
fn bridge_stats_answers_while_another_frame_holds_the_app_mutex() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let clock = Arc::clone(&state.lock().unwrap().frame_clock);

    let (held, is_held) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let timer = clock.frame("board.list");
        let _guard = timer.lock(&state);
        held.send(()).expect("the test is watching");
        released.recv().expect("the test releases the lock");
    });
    is_held
        .recv_timeout(Duration::from_secs(5))
        .expect("the frame took the app mutex");

    let (answered, answers) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let stats = handler.call(
            SessionSender::detached("s-stats"),
            req("bridge.stats", json!({})),
        );
        let _ = answered.send(stats);
    });
    let stats = answers
        .recv_timeout(Duration::from_secs(5))
        .expect("bridge.stats answers with the app mutex held by another frame");

    assert_eq!(stats["ok"], true, "{stats:?}");
    assert_eq!(
        stats["result"]["lock_holder"], "board.list",
        "the stats name the frame that is holding the lock: {stats:?}"
    );
    release.send(()).expect("the holder is still waiting");
    holder.join().expect("the holding frame ends");
}

/// Frames are counted under the method they answered, so the stats say which
/// verb is slow rather than only that something is.
#[test]
fn bridge_stats_count_every_frame_under_its_own_method() {
    let (dir, repo) = init_repo();
    let (_state, handler) = shared_state_and_handler(&repo, dir.path());
    let sender = SessionSender::detached("s-counting");
    handler.call(sender.clone(), req("project.list", json!({})));
    handler.call(sender.clone(), req("board.list", json!({})));
    handler.call(sender.clone(), req("board.list", json!({})));

    let stats = handler.call(sender, req("bridge.stats", json!({})))["result"].clone();
    assert_eq!(stats["methods"]["board.list"]["served"], 2, "{stats:?}");
    assert_eq!(stats["methods"]["project.list"]["served"], 1, "{stats:?}");
    assert_eq!(stats["queue_depth"], 0);
    assert_eq!(stats["lock_holder"], Value::Null);
    // Three, not four: a frame is published when it ends, and the frame
    // asking is still running.
    assert_eq!(stats["frames_served"], 3, "{stats:?}");
}

/// One of the four durations a slow-frame line reports, in milliseconds.
pub(in crate::app::tests) fn slow_frame_millis(line: &str, field: &str) -> f64 {
    line.split_whitespace()
        .find_map(|entry| entry.strip_prefix(field))
        .and_then(|duration| duration.strip_suffix("ms"))
        .and_then(|duration| duration.parse().ok())
        .unwrap_or_else(|| panic!("no {field} in {line}"))
}

/// A delivery is not the frame that asked for it.
///
/// Its spawn probes a transcript tree and waits out a harness's readiness,
/// and charging that to `agent.start` would make every verb that speaks to
/// an agent read as the daemon's slowest while saying nothing about where
/// the time actually went. The delivery is timed under a method of its own,
/// and the frame that queued it answers before any of it has happened.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delivery_is_timed_under_its_own_method_and_never_the_frames() {
    let (dir, repo) = init_repo();
    let (clock, lines) = recording_clock();
    let (state, handler) = state_and_handler_timed_by(Arc::clone(&clock), &repo, dir.path());
    insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-timed");

    // The spawn's session locator runs with the app mutex released, so a
    // factory that takes its time is time the delivery spends and the frame
    // that queued it does not.
    state.lock().unwrap().session_locator_factory = Arc::new(move |_, _| {
        std::thread::sleep(SLOW_FRAME + Duration::from_millis(50));
        None
    });

    let started = call(&handler, "agent.start", json!({ "id": "run-timed" }));
    assert_eq!(started["ok"], true, "{started:?}");

    let line = wait_for(Duration::from_secs(20), || {
        lines
            .lock()
            .unwrap()
            .iter()
            .find(|line| line.starts_with("slow frame agent.deliver "))
            .cloned()
    })
    .await
    .unwrap_or_else(|| {
        panic!(
            "the delivery logged no slow frame of its own: {:?}",
            lines.lock().unwrap()
        )
    });
    assert!(
        slow_frame_millis(&line, "total=") >= SLOW_FRAME.as_secs_f64() * 1000.0,
        "the spawn's seconds are the delivery's own: {line}"
    );
    let lines = lines.lock().unwrap();
    assert!(
        !lines
            .iter()
            .any(|line| line.starts_with("slow frame agent.start ")),
        "the start answered before the harness it asked for was up: {lines:?}"
    );
}

/// A session locator that stops where the test says. It is the first thing
/// a spawn's lock-free phase does, so a spawn parked here is a delivery in
/// flight and nothing else about the daemon is holding still.
pub(in crate::app::tests) fn spawns_parked_at(state: &Arc<Mutex<AppState>>) -> OffLockGateHandle {
    let (gate, handle) = OffLockGate::new();
    state.lock().unwrap().session_locator_factory = Arc::new(move |_, _| {
        gate.arrive();
        None
    });
    handle
}

/// Call `method` and wait for its answer with the spawn it triggers parked
/// at `spawning`. Nothing opens that gate until the answer is in, so an
/// answer at all is one that did not wait for the spawn, however slow the
/// machine. The timeout only turns a verb that does wait into a failure
/// instead of a hung suite.
///
/// The call runs on a thread of the test's runtime: with no runtime under
/// it a delivery runs on the caller's time by design
/// (`DeliveryRunner::spawn`).
async fn answered_with_the_spawn_parked(
    handler: &FrameHandler,
    spawning: &OffLockGateHandle,
    method: &'static str,
    params: Value,
) -> Value {
    let handler = handler.clone();
    let answering = tokio::task::spawn_blocking(move || call(&handler, method, params));
    match tokio::time::timeout(Duration::from_secs(30), answering).await {
        Ok(answer) => answer.expect("the call did not panic"),
        Err(_) => {
            spawning.release();
            panic!("{method} waited for the spawn it triggered")
        }
    }
}

/// The whole of spec step 2, in one frame: a message is answered when the
/// message is durable, and never when the agent is up.
///
/// A cold spawn waits on the harness's readiness for up to
/// `HARNESS_READY_GRACE` and the browser gives up at twelve seconds, so a
/// reply that waited for the spawn was the reply the human never saw. The
/// rest of the daemon is free while it happens.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_is_answered_before_its_agent_has_spawned() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-answered");
    let spawning = spawns_parked_at(&state);

    let posted = answered_with_the_spawn_parked(
        &handler,
        &spawning,
        "thread.post",
        json!({ "entity_id": "run-answered", "body": "start on this" }),
    )
    .await;
    assert_eq!(posted["ok"], true, "{posted:?}");

    spawning.wait_for_arrival();
    let board = call(&handler, "board.list", json!({}));
    assert_eq!(
        board["ok"], true,
        "the daemon reads while a cold spawn is in flight: {board:?}"
    );
    spawning.release();

    wait_for_agent_tab(&state, &derived_agent_key(&root, "run-answered")).await;
}

/// The same rule for the verb that exists only to open an agent. Its reply
/// carries the tab id the agent's own identity mints — reserved under the
/// lock, addressable before the harness behind it exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_start_answers_with_the_reserved_tab_before_the_harness_is_up() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-reserved");
    let spawning = spawns_parked_at(&state);

    let started = answered_with_the_spawn_parked(
        &handler,
        &spawning,
        "agent.start",
        json!({ "id": "run-reserved" }),
    )
    .await;
    assert_eq!(started["ok"], true, "{started:?}");
    let key = derived_agent_key(&root, "run-reserved");
    assert_eq!(
        started["result"]["term_id"], key.tab_id,
        "the reply addresses the tab the spawn is about to fill: {started:?}"
    );
    assert!(
        !state.lock().unwrap().session_registry.contains(&key),
        "and it answered before that tab existed"
    );

    spawning.wait_for_arrival();
    spawning.release();
    wait_for_agent_tab(&state, &key).await;
}

/// A delivery that never reached an agent is made on a thread of its own,
/// so the reply cannot carry the failure. It reaches the browser the way
/// every background outcome does: written onto the entity, and announced.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delivery_that_fails_in_the_background_lands_on_its_entity() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    watch_everything(&handler, &sender);
    insert_run_without_agent(&state, &repo, dir.path().join("side"), "run-unreachable");
    {
        // The checkout its agent would work in is not a directory, so the
        // scaffold every spawn makes fails and no agent can be reached.
        let mut s = state.lock().unwrap();
        s.runs
            .get_mut("run-unreachable")
            .expect("the run")
            .worktree
            .path = std::path::PathBuf::from("/dev/null/there-is-no-worktree-here");
    }
    settled_pushes(&mut rx, &key).await;

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": "run-unreachable", "body": "are you there" }),
    );
    assert_eq!(
        posted["ok"], true,
        "the message is durable whatever the delivery does: {posted:?}"
    );
    wait_for_deliveries(&state).await;

    let got = run_detail(
        &mut state.lock().unwrap(),
        json!({ "run_id": "run-unreachable" }),
    );
    assert!(
        got["result"]["last_error"]
            .as_str()
            .unwrap_or_default()
            .contains("could not reach the agent"),
        "the failure is legible on the run: {got:?}"
    );
    let moved = changed_entities(&settled_pushes(&mut rx, &key).await);
    assert!(
        moved.iter().any(|entity| entity == "run-unreachable"),
        "and the browser is told to look: {moved:?}"
    );
}

/// A caller that lost the spawn race waits on the winner, and waits holding
/// nothing.
///
/// The winner needs the app mutex to publish its tab, so a loser that
/// polled for it — the 25 ms sleep loop this replaces — was taking the
/// mutex away from the spawn it was waiting for. One harness comes out of
/// it either way; a daemon that answers meanwhile does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_callers_of_one_tab_spawn_one_harness_without_spinning() {
    let (dir, repo) = init_repo();
    let (state, handler, root) = agent_tab_fixture(&repo, dir.path(), "run-queued");
    let spawning = spawns_parked_at(&state);
    let asking = || {
        let state = Arc::clone(&state);
        let root = root.clone();
        tokio::task::spawn_blocking(move || {
            ensure_agent_tab(
                &state,
                &root,
                "run-queued",
                &crate::agent::derived_agent_id("run-queued"),
                &ModelChoice::default(),
                "test",
            )
        })
    };

    let winner = asking();
    spawning.wait_for_arrival();
    let loser = asking();
    // Long enough for the second caller to have reached the wait, so the
    // read below is answered from behind it and not in front of it.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let board = call(&handler, "board.list", json!({}));
    assert_eq!(
        board["ok"], true,
        "the daemon reads while a caller waits out another's spawn: {board:?}"
    );
    spawning.release();

    let (won_id, won) = winner.await.unwrap().expect("the winner spawns");
    let (lost_id, lost) = loser.await.unwrap().expect("the loser gets the tab");
    assert_eq!(won, Spawned::Fresh);
    assert_eq!(
        lost,
        Spawned::Warm,
        "the loser is handed the winner's tab, never a second harness"
    );
    assert_eq!(won_id, lost_id, "both callers address one tab");
    let s = state.lock().unwrap();
    assert_eq!(
        s.session_registry.test_counts().tabs,
        1,
        "one worktree, one agent"
    );
    assert!(
        s.session_registry.test_counts().claims == 0,
        "the claim went back"
    );
}

/// A spawn that unwinds gives its claim back too.
///
/// `agent_spawns_in_flight` is removed from in exactly two places — the
/// settle a published tab makes and the settle an abandoned reservation
/// makes — so a claim a panic walked past would be held for the life of the
/// daemon: every later delivery to that tab waits out `AGENT_SPAWN_WAIT`
/// and then fails, the entity reads as permanently starting, and
/// `agent.remove` refuses the agent forever. `SpawnClaim`'s `Drop` is the
/// only thing between a panicking probe and that.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawn_that_panics_gives_its_claim_back() {
    let (dir, repo) = init_repo();
    let (state, _handler, root) = agent_tab_fixture(&repo, dir.path(), "run-panicking-spawn");
    let agent_id = crate::agent::derived_agent_id("run-panicking-spawn");
    let asking = || {
        let state = Arc::clone(&state);
        let root = root.clone();
        let agent_id = agent_id.clone();
        tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ensure_agent_tab(
                    &state,
                    &root,
                    "run-panicking-spawn",
                    &agent_id,
                    &ModelChoice::default(),
                    "test",
                )
            }))
        })
    };

    // The first thing the lock-free phase touches, so the unwind happens
    // with the claim taken and the app mutex NOT held.
    state.lock().unwrap().session_locator_factory =
        Arc::new(|_, _| panic!("the transcript tree the spawn was reading blew up"));
    assert!(
        asking().await.unwrap().is_err(),
        "the probe's panic unwinds the spawn"
    );
    assert!(
        state.lock().unwrap().session_registry.test_counts().claims == 0,
        "the claim went back with the unwinding spawn"
    );

    // What a leak would actually cost: the next caller waits out
    // `AGENT_SPAWN_WAIT` behind a claim nobody holds and then fails.
    state.lock().unwrap().session_locator_factory = Arc::new(|_, _| None);
    let asked_at = std::time::Instant::now();
    let (_wire_id, spawned) = asking()
        .await
        .unwrap()
        .expect("the next spawn does not panic")
        .expect("it opens the tab the panicking one did not");
    assert_eq!(spawned, Spawned::Fresh);
    assert!(
        asked_at.elapsed() < AGENT_SPAWN_WAIT,
        "the spawn queued behind a leaked claim: {:?}",
        asked_at.elapsed()
    );
}

/// And it gives it back from inside the acquisition that publishes.
///
/// `publish_agent_tab` holds the app mutex and the claim at once. The claim
/// is declared outside the block the guard lives in and locals drop in
/// reverse, so an unwind drops the guard first and `Drop` finds the mutex
/// free. Get that order wrong and the daemon does not leak, it deadlocks on
/// itself — which is why this is asserted against a deadline rather than by
/// joining.
#[test]
fn a_claim_dropped_by_a_panic_under_the_app_mutex_does_not_deadlock() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new_unrooted(dir.path(), "main", true, "unused").shared();
    let (unwound, settled) = std::sync::mpsc::channel();
    let publishing = Arc::clone(&state);
    let published = derived_agent_key(dir.path(), "run-published");
    std::thread::spawn(move || {
        let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _claim = SpawnClaim::take(&mut publishing.lock().unwrap(), &published);
            let _guard = publishing.lock().unwrap();
            panic!("the frame publishing the tab died holding the app mutex");
        }));
        let _ = unwound.send(died.is_err());
    });
    assert_eq!(
        settled.recv_timeout(Duration::from_secs(5)),
        Ok(true),
        "the claim's Drop re-entered the mutex it was unwinding out of"
    );

    let s = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(
        s.session_registry.test_counts().claims == 0,
        "a panic under the publishing lock kept the claim"
    );
}

/// A frame that inserts a tab starts its pumps one statement after its own
/// lock block releases, on the frame's thread. An acquisition there is the
/// frame's — charged to nothing and named as nobody's if it is bare — so
/// the pumps take what they need from the tab they are handed and touch no
/// lock at all: they start while another frame holds it.
#[tokio::test]
async fn a_tabs_pumps_start_while_another_frame_holds_the_app_mutex() {
    let dir = tempfile::tempdir().unwrap();
    let state = AppState::new_unrooted(dir.path(), "main", true, "unused").shared();
    let key = derived_agent_key(dir.path(), "run-pumped");
    let (tab, output) = Tab::spawn_shell(
        &HarnessSpec::new("cat"),
        key.tab_id.clone(),
        dir.path().to_path_buf(),
        terminal_size(80, 24),
    )
    .expect("the tab spawns");
    let pumps = tab.pumps(output);
    let held = state.lock().unwrap();

    let runtime = tokio::runtime::Handle::current();
    let starting = Arc::clone(&state);
    let (started, start) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _in_runtime = runtime.enter();
        crate::app::spawn_tab_pumps(&starting, key, pumps);
        let _ = started.send(());
    });
    start
        .recv_timeout(Duration::from_secs(2))
        .expect("the pumps started with the app mutex held by another frame");
    drop(held);
    tab.session.end();
}

/// The convoy this step exists to prevent, from the pump's side: an agent
/// painting a full-speed TUI parses every chunk under its OWN screen lock,
/// so a frame holding the app mutex — a board read, a commit, anything —
/// does not stop the paint, and the paint does not stop it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_streaming_pty_never_takes_the_app_mutex() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let root = AppState::canonical_root(&repo);
    let key = TabKey {
        root: root.clone(),
        tab_id: "term-1".to_string(),
    };
    let (tab, output) = Tab::spawn_shell(
        &HarnessSpec::new("yes"),
        key.tab_id.clone(),
        root,
        terminal_size(80, 24),
    )
    .expect("the flooding tab spawns");
    let screen = screen_of(&tab).clone();
    let session = Arc::clone(&tab.session);
    let pumps = tab.pumps(output);
    state
        .lock()
        .unwrap()
        .session_registry
        .test_insert_tab(key.clone(), tab);
    crate::app::spawn_tab_pumps(&state, key, pumps);

    // The app mutex is held for the whole of this, the way a slow frame
    // holds it. The screen must keep filling underneath.
    let held = state.lock().unwrap();
    let painted = painted_bytes_within(&screen, Duration::from_secs(10));
    drop(held);

    assert!(
        painted > 0,
        "a flooding PTY painted nothing while a frame held the app mutex"
    );
    session.end();
}

/// How far a screen's cursor gets inside `budget`, polled without ever
/// awaiting — the caller is holding a lock the runtime must not park.
fn painted_bytes_within(screen: &ScreenHandle, budget: Duration) -> u64 {
    let deadline = std::time::Instant::now() + budget;
    while std::time::Instant::now() < deadline {
        let painted = screen.cursor();
        if painted > 0 {
            return painted;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    0
}

/// The other half of the same rule: a screen that is busy — parsing a
/// flood, serializing a snapshot, pushing to a slow client — holds nothing
/// but itself, so every other frame in the daemon answers straight through
/// it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_frame_answers_while_a_screen_lock_is_held() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    let created = handler.call(
        SessionSender::detached("s1"),
        req("term.create", json!({ "project_id": project_id })),
    );
    assert_eq!(created["ok"], true, "{created:?}");
    let key = state
        .lock()
        .unwrap()
        .tab_key_of_wire_id("term-1")
        .expect("the shell is registered");
    let screen = screen_of(
        state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(&key)
            .unwrap(),
    )
    .clone();

    let held = screen.hold();
    assert!(
        state.try_lock().is_ok(),
        "a screen lock is not the app mutex"
    );
    let read = frame_on_a_thread(&state, "s-read", "project.list", json!({}));
    let answered = read
        .recv_timeout(Duration::from_secs(5))
        .expect("an unrelated read is answered while a screen is busy");
    assert_eq!(answered["ok"], true, "{answered:?}");
    drop(held);

    state
        .lock()
        .unwrap()
        .session_registry
        .test_tab(&key)
        .unwrap()
        .session
        .end();
}

/// A harness that has stopped draining its pty blocks the write to it for
/// as long as it likes. Under the app mutex that one child wedged the whole
/// daemon; off it, it costs one worker and nothing else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn term_input_to_a_pty_that_is_not_draining_leaves_the_app_mutex_free() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let root = AppState::canonical_root(&repo);
    let agent_id = "agent-wedged";
    let key = TabKey::agent(&root, agent_id);
    let (gate, gate_handle) = OffLockGate::new();
    state.lock().unwrap().session_registry.test_insert_tab(
        key.clone(),
        gated_tab(
            &root,
            gated_agent_role(agent_id),
            GatedHarness::new().refusing_input_until(gate),
        ),
    );

    let typed = frame_on_a_thread(
        &state,
        "s-typed",
        "term.input",
        json!({ "term_id": agent_tab_id(agent_id), "data": b64encode(b"ls\r") }),
    );
    gate_handle.wait_for_arrival();

    assert!(
        state.try_lock().is_ok(),
        "the pty write is holding the app mutex"
    );
    let read = frame_on_a_thread(&state, "s-read", "project.list", json!({}));
    let answered = read
        .recv_timeout(Duration::from_secs(5))
        .expect("an unrelated read is answered while a pty write is stuck");
    assert_eq!(answered["ok"], true, "{answered:?}");

    gate_handle.release();
    let typed = typed
        .recv_timeout(Duration::from_secs(5))
        .expect("the write answers once the child takes it");
    assert_eq!(typed["ok"], true, "{typed:?}");
}

/// A tab in `root` carrying `harness`, with a grid of its own — the shape
/// a test needs to hold one step of a tab's life open and watch the rest of
/// the daemon carry on.
pub(in crate::app::tests) fn gated_tab(
    root: &std::path::Path,
    role: TabRole,
    harness: GatedHarness,
) -> Tab {
    let tab_id = match &role {
        TabRole::Agent { agent_id, .. } => agent_tab_id(agent_id),
        TabRole::Shell => "term-1".to_string(),
    };
    let session_instance = role.agent().map(|(owner, agent_id)| SessionInstance {
        id: format!("session-test-{agent_id}"),
        entity_id: owner.to_string(),
        agent_id: agent_id.to_string(),
        conversation_id: agent_id.to_string(),
        checkout: root.display().to_string(),
    });
    Tab {
        screen: Some(ScreenHandle::new(&tab_id, 80, 24)),
        tab_id,
        root: root.to_path_buf(),
        role,
        created_at: now_rfc3339(),
        session: Arc::new(harness),
        session_instance,
        live: true,
        call_sequences: HashMap::new(),
        last_delivered_at: None,
    }
}

pub(in crate::app::tests) fn gated_agent_role(agent_id: &str) -> TabRole {
    TabRole::Agent {
        owner: "run-wedged".to_string(),
        agent_id: agent_id.to_string(),
        provider: AgentProvider::default(),
    }
}

/// A session with a terminal whose every blocking step the test decides
/// when to release: the write to its pty, and its own death.
pub(in crate::app::tests) struct GatedHarness {
    output: broadcast::Sender<Vec<u8>>,
    on_write: Option<OffLockGate>,
    on_resize: Option<OffLockGate>,
    on_end: Option<OffLockGate>,
    on_self_report: Option<OffLockGate>,
    named: Option<String>,
}

impl GatedHarness {
    pub(in crate::app::tests) fn new() -> GatedHarness {
        let (output, _) = broadcast::channel(4);
        GatedHarness {
            output,
            on_write: None,
            on_resize: None,
            on_end: None,
            on_self_report: None,
            named: None,
        }
    }

    /// A harness that has stopped draining its pty.
    pub(in crate::app::tests) fn refusing_input_until(mut self, gate: OffLockGate) -> GatedHarness {
        self.on_write = Some(gate);
        self
    }

    /// A harness that has stopped answering the window-change ioctl.
    pub(in crate::app::tests) fn refusing_resize_until(
        mut self,
        gate: OffLockGate,
    ) -> GatedHarness {
        self.on_resize = Some(gate);
        self
    }

    /// A harness wedged in uninterruptible I/O: SIGKILL lands, the reap
    /// does not return.
    pub(in crate::app::tests) fn refusing_to_die_until(
        mut self,
        gate: OffLockGate,
    ) -> GatedHarness {
        self.on_end = Some(gate);
        self
    }

    /// A harness that answers `named` when asked what conversation it is
    /// having — slowly, the way a locator listing a transcript tree does.
    pub(in crate::app::tests) fn naming_its_conversation_through(
        mut self,
        gate: OffLockGate,
        named: &str,
    ) -> GatedHarness {
        self.on_self_report = Some(gate);
        self.named = Some(named.to_string());
        self
    }
}

impl AgentSession for GatedHarness {
    fn send_turn(&self, _turn: &Turn) -> Result<(), HarnessError> {
        Ok(())
    }
    fn status(&self) -> AgentStatus {
        AgentStatus::Waiting
    }
    fn quiet_for(&self) -> Duration {
        Duration::ZERO
    }
    fn exited_within(&self, _timeout: Duration) -> bool {
        false
    }
    fn end(&self) {
        if let Some(gate) = &self.on_end {
            gate.arrive();
        }
    }
    fn backdate_last_output(&self, _ago: Duration) {}
    fn session_id(&self) -> Option<String> {
        if let Some(gate) = &self.on_self_report {
            gate.arrive();
        }
        self.named.clone()
    }
    fn terminal(&self) -> Option<&dyn crate::harness::TerminalView> {
        Some(self)
    }
}

impl crate::harness::TerminalView for GatedHarness {
    fn subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.output.subscribe()
    }
    fn write_input(&self, _bytes: &[u8]) -> Result<(), HarnessError> {
        if let Some(gate) = &self.on_write {
            gate.arrive();
        }
        Ok(())
    }
    fn resize(&self, _size: PtySize) -> Result<(), HarnessError> {
        if let Some(gate) = &self.on_resize {
            gate.arrive();
        }
        Ok(())
    }
    fn pid(&self) -> Option<u32> {
        None
    }
}

/// `AgentSession::end` is kill THEN reap, and SIGKILL does not land on a
/// child wedged in uninterruptible I/O until that I/O returns. Every verb
/// that closes a tab used to wait for that under the app mutex, so one
/// stuck harness stopped the daemon. The wait goes to a thread of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killing_a_wedged_harness_never_holds_the_app_mutex() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let root = AppState::canonical_root(&repo);
    let (gate, gate_handle) = OffLockGate::new();
    let key = TabKey {
        root: root.clone(),
        tab_id: "term-1".to_string(),
    };
    state.lock().unwrap().session_registry.test_insert_tab(
        key.clone(),
        gated_tab(
            &root,
            TabRole::Shell,
            GatedHarness::new().refusing_to_die_until(gate),
        ),
    );

    let closed = frame_on_a_thread(
        &state,
        "s-close",
        "term.close",
        json!({ "term_id": "term-1" }),
    );
    gate_handle.wait_for_arrival();
    let closed = closed
        .recv_timeout(Duration::from_secs(5))
        .expect("the close answers without waiting for the reap");
    assert_eq!(closed["ok"], true, "{closed:?}");

    assert!(
        state.try_lock().is_ok(),
        "the reap is holding the app mutex"
    );
    let read = frame_on_a_thread(&state, "s-read", "project.list", json!({}));
    let answered = read
        .recv_timeout(Duration::from_secs(5))
        .expect("an unrelated read is answered while a harness will not die");
    assert_eq!(answered["ok"], true, "{answered:?}");
    assert!(
        !state.lock().unwrap().session_registry.contains(&key),
        "the tab is out of the registry the moment the verb answers"
    );

    gate_handle.release();
}

/// The tab's clients are told it is gone whatever the process does about
/// it: the close push is bounded work on the screen's own lock and stays
/// where it always was, while only the kill leaves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_close_after_a_wedged_kill_still_reaches_its_clients() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = AppState::canonical_root(&repo);
    let (gate, gate_handle) = OffLockGate::new();
    let key = TabKey {
        root: root.clone(),
        tab_id: "term-1".to_string(),
    };
    state.lock().unwrap().session_registry.test_insert_tab(
        key.clone(),
        gated_tab(
            &root,
            TabRole::Shell,
            GatedHarness::new().refusing_to_die_until(gate),
        ),
    );
    let (sender, mut pushes, session_key) = SessionSender::observable("watching");
    let attached = handler.call(sender, req("term.attach", json!({ "term_id": "term-1" })));
    assert_eq!(attached["ok"], true, "{attached:?}");

    let closed = frame_on_a_thread(
        &state,
        "s-close",
        "term.close",
        json!({ "term_id": "term-1" }),
    );
    gate_handle.wait_for_arrival();
    let closed = closed
        .recv_timeout(Duration::from_secs(5))
        .expect("the close answers without waiting for the reap");
    assert_eq!(closed["ok"], true, "{closed:?}");

    let seen = wait_for_push(&mut pushes, &session_key, |push| {
        push["type"] == "term.closed" && push["term_id"] == "term-1"
    })
    .await;
    assert!(!seen.is_empty(), "{seen:?}");

    gate_handle.release();
}

/// A closed screen means nobody is watching and nothing is painting. The
/// kill is asynchronous, so between the close reply and the child's death
/// the PTY can keep producing — for a harness wedged in uninterruptible
/// I/O, without end. The pump used to stop the instant its tab left the
/// registry; now that it never consults the registry, the screen's own
/// closed state is what stops it, or it would parse and push `term.output`
/// forever to the clients it just told `term.closed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_closed_tab_stops_painting_before_its_harness_dies() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = AppState::canonical_root(&repo);
    let (gate, gate_handle) = OffLockGate::new();
    let key = TabKey {
        root: root.clone(),
        tab_id: "term-1".to_string(),
    };
    let harness = GatedHarness::new().refusing_to_die_until(gate);
    let pty = harness.output.clone();
    state
        .lock()
        .unwrap()
        .session_registry
        .test_insert_tab(key.clone(), gated_tab(&root, TabRole::Shell, harness));
    spawn_tab_pumps(
        &state,
        key.clone(),
        SessionOutput::painting(pty.subscribe()),
    );
    let (sender, mut pushes, session_key) = SessionSender::observable("watching");
    let attached = handler.call(sender, req("term.attach", json!({ "term_id": "term-1" })));
    assert_eq!(attached["ok"], true, "{attached:?}");

    let closed = frame_on_a_thread(
        &state,
        "s-close",
        "term.close",
        json!({ "term_id": "term-1" }),
    );
    gate_handle.wait_for_arrival();
    let closed = closed
        .recv_timeout(Duration::from_secs(5))
        .expect("the close answers without waiting for the reap");
    assert_eq!(closed["ok"], true, "{closed:?}");
    wait_for_push(&mut pushes, &session_key, |push| {
        push["type"] == "term.closed" && push["term_id"] == "term-1"
    })
    .await;

    // The child, still alive, keeps painting into a tab nothing addresses.
    let _ = pty.send(b"AFTER-CLOSE".to_vec());
    wait_for(Duration::from_secs(5), || {
        (pty.receiver_count() == 0).then_some(())
    })
    .await
    .expect("the pump ends the moment its tab is retired, not when the child dies");
    tokio::time::sleep(Duration::from_millis(TERM_FLUSH_MS * 5)).await;
    let after_close = wait_for_pushes(&mut pushes, &session_key, |_| true).await;
    assert!(
        after_close.iter().all(|push| push["type"] != "term.output"),
        "a client told its terminal closed was painted to afterwards: {after_close:?}"
    );

    gate_handle.release();
}

/// With the kill asynchronous, a replaced session's EOF can arrive after
/// its replacement is already in the registry. The pump ends the tab it was
/// started for and no other: it carries the session it pumps, and a tab
/// holding a different one is somebody else's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replaced_sessions_late_eof_leaves_the_replacement_tab_alone() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let root = AppState::canonical_root(&repo);
    let key = derived_agent_key(&root, "run-replaced");
    let agent_id = crate::agent::derived_agent_id("run-replaced");

    let spawn_one = || {
        Tab::spawn_agent(
            "run-replaced".to_string(),
            agent_id.clone(),
            agent_open_request(
                PreparedAgentLaunch {
                    spec: HarnessSpec::new("cat"),
                    pty_size: terminal_size(80, 24),
                },
                root.clone(),
                &ModelChoice::default(),
                None,
                None,
            ),
        )
        .expect("the agent tab spawns")
    };

    let (replaced, output) = spawn_one();
    let dying = Arc::clone(&replaced.session);
    let pumps = replaced.pumps(output);
    state
        .lock()
        .unwrap()
        .session_registry
        .test_insert_tab(key.clone(), replaced);
    crate::app::spawn_tab_pumps(&state, key.clone(), pumps);

    // The replacement takes the tab over while the first session is still
    // being reaped, which is what an asynchronous kill allows.
    let (replacement, output) = spawn_one();
    let living = Arc::clone(&replacement.session);
    let pumps = replacement.pumps(output);
    state
        .lock()
        .unwrap()
        .session_registry
        .test_insert_tab(key.clone(), replacement);
    crate::app::spawn_tab_pumps(&state, key.clone(), pumps);

    dying.end();
    wait_for(Duration::from_secs(10), || {
        matches!(dying.status(), AgentStatus::Ended { .. }).then_some(())
    })
    .await
    .expect("the replaced session dies");
    tokio::time::sleep(Duration::from_millis(200)).await;

    assert!(
        state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(&key)
            .unwrap()
            .live,
        "a dead session's EOF closed the tab that replaced it"
    );
    living.end();
}

/// The rites of a dying session take the app mutex twice, with a
/// filesystem walk between them, so the tab can turn over mid-rite: a post
/// arrives, the dead tab is replaced, and the replacement is already
/// working. What the dead session then reports is its own — writing it down
/// against the live agent closes the turn in flight and records the wrong
/// conversation to resume.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_self_report_never_lands_on_the_session_that_replaced_it() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let root = AppState::canonical_root(&repo);
    let run_id = adopted_run(
        &mut state.lock().unwrap(),
        &repo,
        dir.path(),
        "feature-late-report",
    );
    let agent_id = primary_agent_id(&state.lock().unwrap(), &run_id);
    let role = TabRole::Agent {
        owner: run_id.clone(),
        agent_id: agent_id.clone(),
        provider: AgentProvider::default(),
    };
    let key = TabKey::agent(&root, &agent_id);
    let (gate, gate_handle) = OffLockGate::new();
    let dying = gated_tab(
        &root,
        role.clone(),
        GatedHarness::new().naming_its_conversation_through(gate, "the-dead-conversation"),
    );
    let session = Arc::clone(&dying.session);
    let instance = dying.session_instance.clone();
    let screen = screen_of(&dying).clone();
    state
        .lock()
        .unwrap()
        .session_registry
        .test_insert_tab(key.clone(), dying);

    let rites = {
        let state = Arc::clone(&state);
        let key = key.clone();
        let session = Arc::clone(&session);
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            end_of_session(&state, &key, &session, instance.as_ref(), &screen);
            let _ = done.send(());
        });
        finished
    };
    gate_handle.wait_for_arrival();

    // A post lands in the window: the dead tab is replaced, the replacement
    // names its own conversation, and it is holding a turn.
    {
        let mut s = state.lock().unwrap();
        s.session_registry
            .test_insert_tab(key.clone(), gated_tab(&root, role, GatedHarness::new()));
        s.record_agent_resume_id(
            &run_id,
            &agent_id,
            Some("the-live-conversation".to_string()),
        );
    }
    open_a_turn(&state, &run_id);

    gate_handle.release();
    rites
        .recv_timeout(Duration::from_secs(5))
        .expect("the dying session finishes its rites");

    let s = state.lock().unwrap();
    assert!(
        s.session_registry.test_tab(&key).unwrap().live,
        "the dead session's rites marked its replacement dead"
    );
    assert_eq!(
        s.recorded_resume_id(&run_id, &agent_id).as_deref(),
        Some("the-live-conversation"),
        "the dead session's name was written over the live one's"
    );
    assert!(
        primary_thread(&s.runs[&run_id].agents)
            .working_since()
            .is_some(),
        "the dead session's rites closed the turn its replacement is holding"
    );
}
/// The close a dying session owes its clients and the `live = false` that
/// makes its tab replaceable are ONE acquisition.
///
/// A tab reads as replaceable the moment `live` goes false, and
/// [`ensure_agent_tab`] replaces it by taking its screen — clients and all
/// — over to the new session without a word. So a close pushed after that
/// acquisition released would reach browsers that are watching the LIVE
/// replacement, interleaved with its opening reset; and the post that
/// triggers the replacement is the ordinary case, a human answering an
/// agent that just exited. The close is bounded — the screen's own lock
/// and one send per client — so it belongs inside the acquisition that
/// marks the tab, where nothing can come between them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replacement_cannot_slip_between_a_session_ending_and_its_close() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let root = AppState::canonical_root(&repo);
    let run_id = adopted_run(
        &mut state.lock().unwrap(),
        &repo,
        dir.path(),
        "feature-atomic-close",
    );
    let agent_id = primary_agent_id(&state.lock().unwrap(), &run_id);
    let key = TabKey::agent(&root, &agent_id);
    let dying = gated_tab(
        &root,
        TabRole::Agent {
            owner: run_id.clone(),
            agent_id: agent_id.clone(),
            provider: AgentProvider::default(),
        },
        GatedHarness::new(),
    );
    let session = Arc::clone(&dying.session);
    let instance = dying.session_instance.clone();
    let screen = screen_of(&dying).clone();
    let (sender, mut pushes, session_key) = SessionSender::observable("watching");
    screen.attach(&sender, None);
    state
        .lock()
        .unwrap()
        .session_registry
        .test_insert_tab(key.clone(), dying);

    // The screen is busy the way a flooding pump makes it busy, so the
    // rites park inside the close they owe.
    let held = screen.hold();
    let rites = {
        let state = Arc::clone(&state);
        let key = key.clone();
        let session = Arc::clone(&session);
        let screen = screen.clone();
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            end_of_session(&state, &key, &session, instance.as_ref(), &screen);
            let _ = done.send(());
        });
        finished
    };

    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    let mut replaceable = false;
    while !replaceable && std::time::Instant::now() < deadline {
        if let Ok(s) = state.try_lock() {
            replaceable = !s.session_registry.test_tab(&key).unwrap().live;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        !replaceable,
        "a spawn could have read this tab as dead and carried its screen \
         onto a new session before the old one's clients heard it end"
    );

    drop(held);
    rites
        .recv_timeout(Duration::from_secs(5))
        .expect("the rites finish once the screen frees up");
    let seen = wait_for_pushes(&mut pushes, &session_key, |seen| {
        seen.iter().any(|push| push["type"] == "term.closed")
    })
    .await;
    let closed: Vec<&Value> = seen
        .iter()
        .filter(|push| push["type"] == "term.closed")
        .collect();
    assert_eq!(closed.len(), 1, "told once, and once only: {seen:?}");
    assert_eq!(closed[0]["reason"], "agent_session_ended", "{closed:?}");
    assert_eq!(
        screen.attached(),
        1,
        "and the client stays on the retained grid, for the session that \
         paints here next"
    );
}

/// The same rite, on the carrier with no bytes. Its stream closing ends a
/// session too — not live, open tool calls harvested as unanswered, the
/// turn closed — and none of that belongs to the session that took the tab
/// over while it was reading.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replaced_sessions_late_activity_close_leaves_the_replacement_alone() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let root = AppState::canonical_root(&repo);
    let run_id = adopted_run(
        &mut state.lock().unwrap(),
        &repo,
        dir.path(),
        "feature-late-activity",
    );
    let agent_id = primary_agent_id(&state.lock().unwrap(), &run_id);
    let role = TabRole::Agent {
        owner: run_id.clone(),
        agent_id: agent_id.clone(),
        provider: AgentProvider::default(),
    };
    let key = TabKey::agent(&root, &agent_id);
    let (gate, gate_handle) = OffLockGate::new();
    let reporting = gated_tab(
        &root,
        role.clone(),
        GatedHarness::new().naming_its_conversation_through(gate, "the-dead-conversation"),
    );
    let session = Arc::clone(&reporting.session);
    let instance = reporting.session_instance.clone();
    state
        .lock()
        .unwrap()
        .session_registry
        .test_insert_tab(key.clone(), reporting);
    let (activity, subscribed) = broadcast::channel(4);
    crate::app::spawn_activity_pump(
        &state,
        key.clone(),
        session,
        instance,
        Some(subscribed),
        None,
    );
    // The stream closes: the session behind this tab is over.
    drop(activity);
    gate_handle.wait_for_arrival();

    {
        let mut s = state.lock().unwrap();
        s.session_registry
            .test_insert_tab(key.clone(), gated_tab(&root, role, GatedHarness::new()));
        s.record_agent_resume_id(
            &run_id,
            &agent_id,
            Some("the-live-conversation".to_string()),
        );
    }
    open_a_turn(&state, &run_id);

    gate_handle.release();
    tokio::time::sleep(Duration::from_millis(200)).await;

    let s = state.lock().unwrap();
    assert!(
        s.session_registry.test_tab(&key).unwrap().live,
        "a closed stream marked the tab that replaced it dead"
    );
    assert_eq!(
        s.recorded_resume_id(&run_id, &agent_id).as_deref(),
        Some("the-live-conversation"),
        "the dead session's name was written over the live one's"
    );
    assert!(
        primary_thread(&s.runs[&run_id].agents)
            .working_since()
            .is_some(),
        "the dead session's close ended the turn its replacement is holding"
    );
}

/// An attach clones the tab's handle under the app mutex and registers with
/// it released, so a close can take the tab out in between. The screen's own
/// lock decides which happened first, because the app mutex no longer can:
/// the late client hears `term.closed` rather than sitting on a live-looking
/// grid nothing will ever paint or close.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_attaching_to_a_tab_that_just_closed_is_told_so() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_state_and_handler(&repo, dir.path());
    let root = AppState::canonical_root(&repo);
    let key = TabKey {
        root: root.clone(),
        tab_id: "term-1".to_string(),
    };
    state.lock().unwrap().session_registry.test_insert_tab(
        key.clone(),
        gated_tab(&root, TabRole::Shell, GatedHarness::new()),
    );
    // What an attach already in flight is holding.
    let screen = screen_of(
        state
            .lock()
            .unwrap()
            .session_registry
            .test_tab(&key)
            .unwrap(),
    )
    .clone();

    let closed = handler.call(
        SessionSender::detached("s-close"),
        req("term.close", json!({ "term_id": "term-1" })),
    );
    assert_eq!(closed["ok"], true, "{closed:?}");

    let (sender, mut pushes, session_key) = SessionSender::observable("late");
    screen.attach(&sender, Some((80, 24)));

    let seen = wait_for_push(&mut pushes, &session_key, |push| {
        push["type"] == "term.closed" && push["term_id"] == "term-1"
    })
    .await;
    assert!(!seen.is_empty(), "{seen:?}");
    assert_eq!(
        screen.attached(),
        0,
        "a screen nothing will close again took a client anyway"
    );
}

/// A spawn that inherits waiting clients owes their child a window-change
/// ioctl, and an ioctl goes to a process that may not answer. The clients
/// move under the app mutex, beside the insert that publishes the tab,
/// because that half is bounded; the child is told with the lock down, so a
/// harness that will not take the resize wedges one worker rather than the
/// daemon.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn telling_an_inherited_child_its_size_never_holds_the_app_mutex() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let root = AppState::canonical_root(&repo);
    let agent_id = "agent-inheriting";
    let key = TabKey::agent(&root, agent_id);
    let (gate, gate_handle) = OffLockGate::new();
    let born = gated_tab(
        &root,
        gated_agent_role(agent_id),
        GatedHarness::new().refusing_resize_until(gate),
    );

    // A client mounted the Agent tab before this worktree had an agent.
    let (sender, _pushes, _session_key) = SessionSender::observable("waiting-client");
    let waiting = ScreenHandle::new(&agent_tab_id(agent_id), 90, 25);
    waiting.attach(&sender, Some((90, 25)));
    state
        .lock()
        .unwrap()
        .session_registry
        .test_remember_waiting_screen(key.clone(), waiting);

    let told = {
        let state = Arc::clone(&state);
        let key = key.clone();
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let inherited = {
                let mut s = state.lock().unwrap();
                let inherited = inherit_waiting_clients(&mut s, &key, &born);
                s.session_registry.test_insert_tab(key, born);
                inherited
            };
            inherited
                .expect("the waiting clients are carried onto the new screen")
                .fit_child_to_screen();
            let _ = done.send(());
        });
        finished
    };
    gate_handle.wait_for_arrival();

    assert!(
        state.try_lock().is_ok(),
        "the window-change ioctl is holding the app mutex"
    );
    let read = frame_on_a_thread(&state, "s-read", "project.list", json!({}));
    assert_eq!(
        read.recv_timeout(Duration::from_secs(5))
            .expect("an unrelated read is answered while a child will not resize")["ok"],
        true
    );
    assert_eq!(
        screen_of(
            state
                .lock()
                .unwrap()
                .session_registry
                .test_tab(&key)
                .unwrap()
        )
        .attached_sessions(),
        vec!["waiting-client".to_string()],
        "the client that was waiting is on the new screen the moment the tab is published"
    );

    gate_handle.release();
    told.recv_timeout(Duration::from_secs(5))
        .expect("the ioctl returns once the child takes it");
}

/// A terminal names its conversation by listing the harness's transcript
/// tree — a filesystem walk that grows with every conversation the human
/// has ever had. The sweep already took the session out of the registry to
/// ask, and then asked a SECOND time through the registry to write the
/// answer down, under the lock. There is one reading now, and it is off the
/// lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_agent_tabs_last_reading_leaves_the_app_mutex_free() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let root = AppState::canonical_root(&repo);
    let run_id = adopted_run(
        &mut state.lock().unwrap(),
        &repo,
        dir.path(),
        "feature-named",
    );
    let agent_id = primary_agent_id(&state.lock().unwrap(), &run_id);
    let (gate, gate_handle) = OffLockGate::new();
    let key = TabKey::agent(&root, &agent_id);
    {
        let mut app = state.lock().unwrap();
        let instance = app.record_agent_session_start(
            &run_id,
            &agent_id,
            &root,
            &ModelChoice::default(),
            "build",
        );
        let mut tab = gated_tab(
            &root,
            TabRole::Agent {
                owner: run_id.clone(),
                agent_id: agent_id.clone(),
                provider: AgentProvider::default(),
            },
            GatedHarness::new().naming_its_conversation_through(gate, "conversation-7"),
        );
        tab.session_instance = instance;
        app.session_registry.test_insert_tab(key.clone(), tab);
    }

    let captured = {
        let capturing = Arc::clone(&state);
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            capture_conversation_names(&capturing);
            let _ = done.send(());
        });
        finished
    };
    gate_handle.wait_for_arrival();

    assert!(
        state.try_lock().is_ok(),
        "the reading is holding the app mutex"
    );
    let read = frame_on_a_thread(&state, "s-read", "project.list", json!({}));
    assert_eq!(
        read.recv_timeout(Duration::from_secs(5))
            .expect("an unrelated read is answered while a session is being read")["ok"],
        true
    );

    gate_handle.release();
    captured
        .recv_timeout(Duration::from_secs(5))
        .expect("the sweep reads a session once, not twice");
    assert_eq!(
        state
            .lock()
            .unwrap()
            .recorded_resume_id(&run_id, &agent_id)
            .as_deref(),
        Some("conversation-7"),
        "and writes down what it read"
    );
}

/// A sweep reads outside the app lock. If S2 replaces S1 during that read,
/// S1's late provider name belongs only to S1 and cannot overwrite the
/// exact resume id S2 has already announced.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_name_capture_cannot_overwrite_its_replacement_session() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_state_and_handler(&repo, dir.path());
    let root = AppState::canonical_root(&repo);
    let run_id = adopted_run(
        &mut state.lock().unwrap(),
        &repo,
        dir.path(),
        "feature-stale-name",
    );
    let agent_id = primary_agent_id(&state.lock().unwrap(), &run_id);
    let key = TabKey::agent(&root, &agent_id);
    let (gate, gate_handle) = OffLockGate::new();
    {
        let mut app = state.lock().unwrap();
        let first = app
            .record_agent_session_start(&run_id, &agent_id, &root, &ModelChoice::default(), "build")
            .expect("the first exact session");
        let mut tab = gated_tab(
            &root,
            TabRole::Agent {
                owner: run_id.clone(),
                agent_id: agent_id.clone(),
                provider: AgentProvider::default(),
            },
            GatedHarness::new().naming_its_conversation_through(gate, "stale-S1-name"),
        );
        tab.session_instance = Some(first);
        app.session_registry.test_insert_tab(key.clone(), tab);
    }
    let captured = {
        let state = Arc::clone(&state);
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            capture_conversation_names(&state);
            let _ = done.send(());
        });
        finished
    };
    gate_handle.wait_for_arrival();

    let replacement = {
        let mut app = state.lock().unwrap();
        let replacement = app
            .record_agent_session_start(&run_id, &agent_id, &root, &ModelChoice::default(), "build")
            .expect("the replacement exact session");
        app.note_self_report(
            &run_id,
            &agent_id,
            &replacement,
            SelfReport {
                named: Some("live-S2-name".to_string()),
                model: None,
                effort: None,
            },
        );
        let mut tab = gated_tab(
            &root,
            TabRole::Agent {
                owner: run_id.clone(),
                agent_id: agent_id.clone(),
                provider: AgentProvider::default(),
            },
            GatedHarness::new(),
        );
        tab.session_instance = Some(replacement.clone());
        app.session_registry.test_insert_tab(key, tab);
        replacement
    };

    gate_handle.release();
    captured
        .recv_timeout(Duration::from_secs(5))
        .expect("the stale name read returns");

    let s = state.lock().unwrap();
    assert_eq!(
        s.recorded_resume_id(&run_id, &agent_id).as_deref(),
        Some("live-S2-name")
    );
    let current = s
        .agent_conversation(&run_id, Some(&agent_id))
        .unwrap()
        .open_session_instance(&agent_id)
        .expect("S2 stays current");
    assert_eq!(current, replacement);
}
