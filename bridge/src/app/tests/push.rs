use super::*;
use crate::api::API_VERSION;

// ==== Push invalidation ====================================================

/// A window a test can wait out. Production coalesces over
/// [`DEFAULT_COALESCE_WINDOW`]; nothing here depends on the number, only on
/// there being one.
const TEST_CHANGE_WINDOW: Duration = Duration::from_millis(60);

/// A QA daemon plus one browser session that greeted it — the shape every
/// push-invalidation test starts from. The sender comes back because a test
/// that also opens a terminal has to attach on the SAME session.
pub(in crate::app::tests) fn greeted_push_session(
    repo: &std::path::Path,
    dir: &std::path::Path,
) -> (
    Arc<Mutex<AppState>>,
    FrameHandler,
    SessionSender,
    tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
    String,
) {
    let mut app = qa_state(repo, dir).with_change_window(TEST_CHANGE_WINDOW);
    app.term_shell = "/bin/bash".into();
    let state = app.shared();
    let handler = AppState::handler(Arc::clone(&state));
    let (sender, rx, key) = SessionSender::observable("browser");
    let hello = handler.call(sender.clone(), req("session.hello", json!({})));
    assert_eq!(hello["ok"], true, "{hello:?}");
    (state, handler, sender, rx, key)
}

/// Give the flusher several windows, then take everything it sent.
pub(in crate::app::tests) async fn settled_pushes(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
    session_key: &str,
) -> Vec<Value> {
    tokio::time::sleep(TEST_CHANGE_WINDOW * 5).await;
    let mut seen = Vec::new();
    while let Ok(message) = rx.try_recv() {
        seen.push(SessionSender::decrypt_push(session_key, &message));
    }
    seen
}

/// Collect pushes until the assertion's actual condition arrives. A file
/// watcher and a git scan can publish separate frames in either order.
async fn pushes_until(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
    session_key: &str,
    mut ready: impl FnMut(&[Value]) -> bool,
) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut pushes = Vec::new();
    while !ready(&pushes) {
        let message = tokio::time::timeout_at(deadline, rx.recv())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for matching pushes: {pushes:?}"))
            .expect("the push session remains open");
        pushes.push(SessionSender::decrypt_push(session_key, &message));
    }
    pushes
}

/// Only the invalidation events out of a push history.
pub(in crate::app::tests) fn change_events(pushes: &[Value]) -> Vec<Value> {
    pushes
        .iter()
        .filter(|push| push["type"] == "board.changed" || push["type"] == "entity.changed")
        .cloned()
        .collect()
}

/// Install a readable legacy Issue without exercising its retired mutation
/// surface, then publish the same invalidations a completed mutation tail
/// would publish. These tests cover the push fanout, not Issue creation.
fn publish_legacy_issue(state: &Arc<Mutex<AppState>>, goal: &str) -> String {
    let mut state = state.lock().unwrap();
    let issue = state
        .plan_create(&json!({ "goal": goal, "dispatch": false }))
        .expect("the legacy issue fixture is filed through the domain seam");
    let issue_id = issue["plan_id"]
        .as_str()
        .expect("the legacy issue has an id")
        .to_string();
    state.note_board_changed();
    state.note_entity_changed(&issue_id);
    issue_id
}

/// The capability announcement, in both places a client can find it: the
/// greeting it opens with, and the probe it already sends. An old bridge has
/// neither, so absence is the answer for a new client too.
#[tokio::test]
async fn the_greeting_announces_push_events() {
    let (dir, repo) = init_repo();
    let (_state, handler) = shared_qa_state_and_handler(&repo, dir.path());

    let hello = call(&handler, "session.hello", json!({}));
    assert_eq!(hello["ok"], true, "{hello:?}");
    assert_eq!(hello["result"]["push_events"], true, "{hello:?}");
    assert_eq!(
        hello["result"]["message_context"]["version"], 1,
        "{hello:?}"
    );
    assert_eq!(
        hello["result"]["thread_post_operations"],
        json!({
            "version": 1,
            "status_method": "thread.operation",
            "states": ["queued", "claimed", "delivered", "uncertain"],
        }),
        "{hello:?}"
    );
    assert_eq!(
        hello["result"]["events"],
        json!(["board.changed", "entity.changed", "changes"]),
        "{hello:?}"
    );
    // Step 1.5: what a Part 1 adapter reads instead of probing — now
    // including whether a `changes` item carries the body of what moved or
    // only its name, which is the difference between a client that paints
    // from a push and one that refetches after it.
    assert_eq!(
        hello["result"]["changes"],
        json!({
            "subscriptions": true,
            "mode": "legacy",
            "kinds": ["state", "thread", "git", "files", "terminals", "issues"],
            "items": "bodies",
            "batch_ms": { "min": 1000, "max": 600_000 },
        }),
        "{hello:?}"
    );
    // The minor that announced them. A client picks its adapter off this
    // number, so the number moving with the announcement is the contract —
    // which is why it is a literal here and an edit every time it moves. 1.3.0
    // is the issue tracker: ten `issues.*` verbs and an `issues` change kind.
    assert_eq!(hello["result"]["api_version"], "1.16.0", "{hello:?}");
    assert!(
        hello["result"]["coalesce_window_ms"]
            .as_u64()
            .is_some_and(|ms| ms > 0),
        "{hello:?}"
    );

    let ping = call(&handler, "ping", json!({}));
    assert_eq!(ping["result"]["pong"], true, "{ping:?}");
    assert_eq!(ping["result"]["push_events"], true, "{ping:?}");
    assert_eq!(ping["result"]["message_context"]["version"], 1, "{ping:?}");
}

/// Step 2.0 of the wire spec: the greeting and the probe both say which API
/// this bridge speaks, so a client can pick an adapter without guessing.
#[tokio::test]
async fn the_greeting_and_the_probe_report_the_api_version() {
    let (dir, repo) = init_repo();
    let (_state, handler) = shared_qa_state_and_handler(&repo, dir.path());

    let hello = call(&handler, "session.hello", json!({}));
    assert_eq!(hello["result"]["api_version"], API_VERSION, "{hello:?}");

    let ping = call(&handler, "ping", json!({}));
    assert_eq!(ping["result"]["api_version"], API_VERSION, "{ping:?}");
}

/// What a client declares about itself is counted per live session, by the
/// range it asked for, so dropping a major is a decision made from numbers.
/// A session that declared nothing — or nonsense — is a live client too, and
/// counts as `unknown` rather than being refused.
#[tokio::test]
async fn bridge_stats_counts_live_clients_by_declared_range() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    let greet = |session: &str, params: Value| {
        let reply = handler.call(
            SessionSender::detached(session),
            req("session.hello", params),
        );
        assert_eq!(reply["ok"], true, "{reply:?}");
    };

    greet(
        "spa-1",
        json!({ "client": { "name": "spa", "version": "abc123", "api_range": ">=1.0.0 <2.0.0" } }),
    );
    greet(
        "spa-2",
        json!({ "client": { "api_range": ">=1.0.0 <2.0.0" } }),
    );
    greet("old", json!({}));
    greet("odd", json!({ "client": "not an object" }));
    greet("odder", json!({ "client": { "api_range": 7 } }));
    // A reconnect greets again on the same session: still one client.
    greet(
        "spa-1",
        json!({ "client": { "api_range": ">=1.0.0 <2.0.0" } }),
    );

    let stats = call(&handler, "bridge.stats", json!({}));
    assert_eq!(
        stats["result"]["clients"],
        json!({ ">=1.0.0 <2.0.0": 2, "unknown": 3 }),
        "{stats:?}"
    );

    let close = Frame {
        session_id: "spa-1".into(),
        message_id: String::new(),
        frame_type: transport::CLOSE_FRAME_TYPE.into(),
        sender: transport::SENDER_DEVICE.into(),
        created_at: String::new(),
        payload: Value::Null,
    };
    handler.call(SessionSender::detached("spa-1"), close);
    drop(state);

    let stats = call(&handler, "bridge.stats", json!({}));
    assert_eq!(
        stats["result"]["clients"],
        json!({ ">=1.0.0 <2.0.0": 1, "unknown": 3 }),
        "{stats:?}"
    );
}

/// Greeting twice — a browser that reconnected — leaves one subscription,
/// so a change is one event and not two.
#[tokio::test]
async fn greeting_twice_leaves_one_subscription() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    handler.call(sender, req("session.hello", json!({})));
    settled_pushes(&mut rx, &key).await;

    state.lock().unwrap().note_board_changed();

    assert_eq!(
        change_events(&settled_pushes(&mut rx, &key).await),
        vec![json!({ "type": "board.changed" })]
    );
}

/// The point of the whole thing: a state change reaches a browser that
/// never asked, naming the feed and the entity that moved.
#[tokio::test]
async fn a_state_change_reaches_the_browser_unasked() {
    let (dir, repo) = init_repo();
    let (state, _handler, _sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    settled_pushes(&mut rx, &key).await; // boot noise

    let plan_id = publish_legacy_issue(&state, "push me");

    let events = change_events(&settled_pushes(&mut rx, &key).await);
    assert!(
        events.contains(&json!({ "type": "board.changed" })),
        "{events:?}"
    );
    assert!(
        events.contains(&json!({ "type": "entity.changed", "id": plan_id })),
        "{events:?}"
    );
}

/// A verb whose git ran with the mutex released announces itself from the
/// APPLY half — after the write-back, never before it. Emitting at the
/// decide half would tell a browser to refetch state the drain was still
/// about to replace.
#[tokio::test]
async fn a_deferred_verb_announces_from_its_apply_half() {
    let (dir, repo) = init_repo();
    let (state, handler, _sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    settled_pushes(&mut rx, &key).await;

    std::fs::write(repo.join("pushed.txt"), "committed off-lock\n").unwrap();
    let staged = call(
        &handler,
        "git.stage",
        json!({ "project_id": project_id, "paths": ["pushed.txt"] }),
    );
    assert_eq!(staged["ok"], true, "{staged:?}");
    let committed = call(
        &handler,
        "git.commit",
        json!({ "project_id": project_id, "message": "off-lock commit" }),
    );
    assert_eq!(committed["ok"], true, "{committed:?}");

    let events = change_events(&settled_pushes(&mut rx, &key).await);
    assert!(
        events.contains(&json!({ "type": "board.changed" })),
        "a commit moved the tree the board summarises: {events:?}"
    );
}

/// A read that deferred its git purely to keep the mutex free wrote nothing
/// back, so it invalidates nothing. Polls run through this path constantly:
/// announcing them would make the SPA's own reads the event storm.
#[tokio::test]
async fn a_deferred_read_announces_nothing() {
    let (dir, repo) = init_repo();
    let (state, handler, _sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    settled_pushes(&mut rx, &key).await;

    for _ in 0..5 {
        let log = call(&handler, "git.log", json!({ "project_id": project_id }));
        assert_eq!(log["ok"], true, "{log:?}");
        let status = call(&handler, "git.status", json!({ "project_id": project_id }));
        assert_eq!(status["ok"], true, "{status:?}");
    }

    assert_eq!(
        change_events(&settled_pushes(&mut rx, &key).await),
        Vec::<Value>::new()
    );
}

/// Terminal output has its own push path and is NOT a change. A repainting
/// TUI writes megabytes; routing that through invalidation would hand the
/// SPA a refetch per frame.
#[tokio::test]
async fn a_terminal_byte_storm_is_not_a_change_event() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();

    let created = call(&handler, "term.create", json!({ "project_id": project_id }));
    assert_eq!(created["ok"], true, "{created:?}");
    let attached = handler.call(sender, req("term.attach", json!({ "term_id": "term-1" })));
    assert_eq!(attached["ok"], true, "{attached:?}");
    settled_pushes(&mut rx, &key).await; // everything the setup itself moved

    let input = b64encode(b"for i in $(seq 1 400); do echo storm-$i; done\r");
    let wrote = call(
        &handler,
        "term.input",
        json!({ "term_id": "term-1", "data": input }),
    );
    assert_eq!(wrote["ok"], true, "{wrote:?}");
    let seen = wait_for_pushes(&mut rx, &key, |seen| {
        output_text(seen, "term-1").contains("storm-400")
    })
    .await;

    assert!(
        seen.iter().any(|push| push["type"] == "term.output"),
        "the storm did reach the browser: {}",
        seen.len()
    );
    assert_eq!(
        change_events(&seen),
        Vec::<Value>::new(),
        "bytes on a screen are not a change to the board"
    );
}

/// A burst of real mutations costs one event per key per window, not one
/// per mutation.
#[tokio::test]
async fn rapid_mutations_cost_one_event_per_window() {
    let (dir, repo) = init_repo();
    let (state, handler, _sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let plan_id = publish_legacy_issue(&state, "coalesce me");
    settled_pushes(&mut rx, &key).await;

    let mutations = 60;
    let started = std::time::Instant::now();
    for _ in 0..mutations {
        let seen = call(&handler, "entity.seen", json!({ "entity_id": plan_id }));
        assert_eq!(seen["ok"], true, "{seen:?}");
    }
    let elapsed = started.elapsed();

    let events = change_events(&settled_pushes(&mut rx, &key).await);
    assert!(!events.is_empty(), "the browser did hear about them");
    // One leading event, one per window the burst spanned, one trailing.
    let ceiling = (elapsed.as_millis() / TEST_CHANGE_WINDOW.as_millis()) as usize + 2;
    assert!(
        events.len() <= ceiling,
        "{mutations} mutations in {elapsed:?} became {} events; at most one per \
         {TEST_CHANGE_WINDOW:?} window was expected ({ceiling}): {events:?}",
        events.len()
    );
}

/// The session ended; nothing is encrypted into it again.
#[tokio::test]
async fn a_closed_session_hears_no_more_changes() {
    let (dir, repo) = init_repo();
    let (state, _handler, _sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    settled_pushes(&mut rx, &key).await;

    let close = Frame {
        session_id: "browser".into(),
        message_id: String::new(),
        frame_type: transport::CLOSE_FRAME_TYPE.into(),
        sender: transport::SENDER_DEVICE.into(),
        created_at: String::new(),
        payload: Value::Null,
    };
    assert_eq!(
        dispatch_frame(
            &state,
            SessionSender::detached("browser"),
            close,
            FrameClock::new().frame("close")
        )["ok"],
        true
    );
    assert_eq!(state.lock().unwrap().changes().subscriber_count(), 0);

    publish_legacy_issue(&state, "nobody hears this");
    assert_eq!(
        change_events(&settled_pushes(&mut rx, &key).await),
        Vec::<Value>::new()
    );
}

/// An agent's own work is a change too: it reaches the browser through the
/// same mutation tails, with no verb from that browser behind it.
#[tokio::test]
async fn an_entity_change_names_the_entity_that_moved() {
    let (dir, repo) = init_repo();
    let (state, _handler, _sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let plan_id = publish_legacy_issue(&state, "agent moved me");
    settled_pushes(&mut rx, &key).await;

    state.lock().unwrap().note_entity_changed(&plan_id);

    assert_eq!(
        change_events(&settled_pushes(&mut rx, &key).await),
        vec![
            json!({ "type": "board.changed" }),
            json!({ "type": "entity.changed", "id": plan_id }),
        ]
    );
}

// ==== Subscriptions and the per-worktree watcher =============================

/// `changes.subscribe` covering a worktree with `git` or `files` puts a
/// watcher on it and answers `watch: "live"`; the last unsubscribe covering it
/// drops the watcher. The reconcile runs on the off-lock drain, so the reply
/// already knows whether the start succeeded.
#[tokio::test]
async fn a_subscription_covering_a_worktree_starts_its_watcher() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, _rx, _key) = greeted_push_session(&repo, dir.path());
    let board = handler.call(sender.clone(), req("board.list", json!({})));
    let project_id = board["result"]["projects"][0]["project_id"]
        .as_str()
        .expect("the QA daemon lists its repo as a project")
        .to_string();

    let subscribed = handler.call(
        sender.clone(),
        req(
            "changes.subscribe",
            json!({
                "subscription_id": "s-focus",
                "scope": { "kind": "entity", "id": project_id },
                "kinds": ["git", "files"],
            }),
        ),
    );
    assert_eq!(subscribed["ok"], true, "{subscribed:?}");
    assert_eq!(subscribed["result"]["watch"], "live", "{subscribed:?}");
    assert!(state.lock().unwrap().watchers().is_watching(&project_id));

    let listed = handler.call(sender.clone(), req("changes.list", json!({})));
    assert_eq!(
        listed["result"]["subscriptions"][0]["subscription_id"],
        "s-focus"
    );

    let unsubscribed = handler.call(
        sender.clone(),
        req(
            "changes.unsubscribe",
            json!({ "subscription_id": "s-focus" }),
        ),
    );
    assert_eq!(unsubscribed["result"]["ok"], true, "{unsubscribed:?}");
    assert!(!state.lock().unwrap().watchers().is_watching(&project_id));
}

/// A write in a watched worktree reaches the subscription as a `changes`
/// frame naming the path — the producer, the bus and the flusher wired end to
/// end, with the facts source filling the git keys.
#[tokio::test]
async fn a_write_in_a_watched_worktree_is_pushed_with_its_path() {
    let (dir, repo) = init_repo();
    let (_state, handler, sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let board = handler.call(sender.clone(), req("board.list", json!({})));
    let project_id = board["result"]["projects"][0]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let subscribed = handler.call(
        sender.clone(),
        req(
            "changes.subscribe",
            json!({
                "subscription_id": "s-focus",
                "scope": { "kind": "entity", "id": project_id },
                "kinds": ["git", "files"],
            }),
        ),
    );
    assert_eq!(subscribed["result"]["watch"], "live", "{subscribed:?}");
    settled_pushes(&mut rx, &key).await;

    std::fs::write(repo.join("noted.txt"), "hello").unwrap();
    let items = |frames: &[Value]| {
        frames
            .iter()
            .filter(|frame| frame["type"] == "changes")
            .flat_map(|frame| frame["items"].as_array().cloned().unwrap_or_default())
            .filter(|item| item["entity_id"] == project_id)
            .collect::<Vec<_>>()
    };
    let frames = pushes_until(&mut rx, &key, |frames| {
        let items = items(frames);
        items
            .iter()
            .any(|item| item["files"]["paths"] == json!(["noted.txt"]))
            && items
                .iter()
                .any(|item| item["git"]["status_key"].is_string())
    })
    .await;
    let items = items(&frames);
    let file_item = items
        .iter()
        .find(|item| item["files"]["paths"] == json!(["noted.txt"]))
        .expect("the changed path arrived");
    assert_eq!(file_item["files"]["paths"], json!(["noted.txt"]));
    let git_item = items
        .iter()
        .find(|item| item["git"]["status_key"].is_string())
        .expect("the git scan arrived");
    assert!(git_item["git"]["status_key"].is_string());
    assert!(
        frames
            .iter()
            .filter(|frame| frame["type"] == "changes")
            .all(|frame| frame["subscription_id"] == "s-focus"),
        "{frames:?}"
    );
}

/// The `state` item IS the feed row: exactly what `board.list` carries for
/// this entity, so a client writes it into its cache and repaints the inbox
/// without asking the bridge anything.
#[tokio::test]
async fn a_state_item_carries_the_row_the_board_would_paint() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    // A run, not an issue: legacy issues no longer appear on the board, and
    // the point of this test is that the pushed `state` says what the board
    // row says. Minted through the domain seam because the workflow RPCs that
    // used to mint one are retired.
    let plan_id = {
        let mut app = state.lock().unwrap();
        let (_, run_id) = planned_run_in_review(&mut app, "state rides the item");
        app.note_board_changed();
        app.note_entity_changed(&run_id);
        run_id
    };
    let subscribed = handler.call(
        sender.clone(),
        req(
            "changes.subscribe",
            json!({
                "subscription_id": "s-focus",
                "scope": { "kind": "entity", "id": plan_id },
                "kinds": ["state"],
            }),
        ),
    );
    assert_eq!(subscribed["ok"], true, "{subscribed:?}");
    // Read the board once first: the row a push carries is the row the board
    // has already worked out, and this is what a client's own first sync is.
    assert_eq!(call(&handler, "board.list", json!({}))["ok"], true);
    settled_pushes(&mut rx, &key).await;

    state.lock().unwrap().note_entity_changed(&plan_id);

    let pushes = settled_pushes(&mut rx, &key).await;
    let item = pushes
        .iter()
        .filter(|push| push["type"] == "changes")
        .flat_map(|frame| frame["items"].as_array().cloned().unwrap_or_default())
        .find(|item| item["entity_id"] == plan_id)
        .unwrap_or_else(|| panic!("no changes item for the issue: {pushes:?}"));
    let board = call(&handler, "board.list", json!({}));
    let row = board["result"]["items"]
        .as_array()
        .expect("the board lists items")
        .iter()
        .find(|row| row["run_id"] == plan_id.as_str())
        .unwrap_or_else(|| panic!("no board row for the run: {board:?}"));
    // Everything but the clock that runs while the test does: how long an
    // agent has been working is counted from now, so the push and the board
    // read a moment later disagree about it by design.
    let steady = |row: &Value| {
        let mut row = row.clone();
        assert!(row["working_time"].is_object(), "{row:?}");
        row["working_time"] = Value::Null;
        row
    };
    assert_eq!(
        steady(&item["state"]),
        steady(row),
        "{item:?} against {row:?}"
    );
}

/// An external checkout's row is found in whatever project holds it. A git
/// project sitting earlier in the list whose checkout walk has not landed —
/// a fresh daemon, or a repository the scan failed on — says nothing about
/// the projects after it, and the checkouts in those still push their rows.
#[tokio::test]
async fn a_state_item_finds_a_checkout_past_a_project_nothing_has_scanned() {
    let (dir, repo) = init_repo();
    let (state, handler) = shared_qa_state_and_handler(&repo, dir.path());
    // The first project is deliberately left unscanned: this is the boot
    // where its walk has not run yet.
    let second = init_repo_named(dir.path(), "second-repo");
    let added = call(
        &handler,
        "project.add",
        json!({ "path": second.display().to_string(), "base_branch": "main" }),
    );
    assert_eq!(added["ok"], true, "{added:?}");
    let second_id = added["result"]["project_id"]
        .as_str()
        .expect("the second project has an id")
        .to_string();
    super::filesystem::add_external_worktree(&second, dir.path(), "loose", "feature-loose");

    let mut app = state.lock().unwrap();
    let worktree_id = app
        .scan_external_worktrees_now(&second_id)
        .expect("the second project's checkouts are scanned")
        .first()
        .expect("the checkout was found")
        .id
        .clone();
    let first_id = app.project_at(0).id.clone();
    assert!(
        app.external_scan_of(&first_id).is_none(),
        "the fixture is a project nothing has scanned"
    );

    let item = app
        .entity_state_item(&worktree_id)
        .unwrap_or_else(|| panic!("no state row for {worktree_id}"));
    assert_eq!(item["worktree_id"], worktree_id.as_str(), "{item:?}");
    assert_eq!(item["project_id"], second_id.as_str(), "{item:?}");
    assert_eq!(item["branch"], "feature-loose", "{item:?}");
}

/// The board item carries the project list whole when a project was added,
/// and nothing but the revision when a row moved. A client caches the list
/// and is told when it changed, rather than re-reading it on every tick.
#[tokio::test]
async fn a_board_item_carries_the_project_list_when_a_project_arrives() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let subscribed = handler.call(
        sender.clone(),
        req(
            "changes.subscribe",
            json!({
                "subscription_id": "s-inbox",
                "scope": { "kind": "all" },
                "kinds": ["state"],
            }),
        ),
    );
    assert_eq!(subscribed["ok"], true, "{subscribed:?}");

    let before_move = state.lock().unwrap().changes.board_revision();
    state.lock().unwrap().note_board_changed();
    let moved_item = |pushes: &[Value]| {
        pushes
            .iter()
            .filter(|push| push["type"] == "changes")
            .flat_map(|frame| frame["items"].as_array().cloned().unwrap_or_default())
            .find(|item| {
                item["entity_id"] == crate::changes::BOARD_ITEM_ID
                    && item["state"]["revision"]
                        .as_u64()
                        .is_some_and(|revision| revision > before_move)
            })
            .map(|item| item["state"].clone())
    };
    let pushes = pushes_until(&mut rx, &key, |pushes| moved_item(pushes).is_some()).await;
    let moved = moved_item(&pushes).expect("the board move reaches the subscription");
    assert!(moved["revision"].is_u64(), "{moved:?}");
    assert!(
        moved.get("projects").is_none(),
        "a row moved, not the list: {moved:?}"
    );

    let second = crate::git_fixture::init_repo_named(dir.path(), "second-repo");
    let added = call(
        &handler,
        "project.add",
        json!({ "path": second.display().to_string(), "base_branch": "main" }),
    );
    assert_eq!(added["ok"], true, "{added:?}");

    let project_item = |pushes: &[Value]| {
        pushes
            .iter()
            .filter(|push| push["type"] == "changes")
            .flat_map(|frame| frame["items"].as_array().cloned().unwrap_or_default())
            .find(|item| {
                item["entity_id"] == crate::changes::BOARD_ITEM_ID
                    && item["state"]["projects"]
                        .as_array()
                        .is_some_and(|projects| {
                            projects.iter().any(|project| {
                                project["project_id"] == added["result"]["project_id"]
                            })
                        })
            })
            .map(|item| item["state"].clone())
    };
    let pushes = pushes_until(&mut rx, &key, |pushes| project_item(pushes).is_some()).await;
    let item = project_item(&pushes).expect("the board push carries the added project");
    let listed = item["projects"]
        .as_array()
        .unwrap_or_else(|| panic!("the board item carries the list: {item:?}"));
    assert_eq!(listed.len(), 2, "{item:?}");
    assert!(
        listed
            .iter()
            .any(|project| project["project_id"] == added["result"]["project_id"]),
        "{item:?}"
    );
}

/// The project list a board item carries is captured under the app mutex
/// and RENDERED with it released. A render opens each project's repository
/// and asks git for its origin remote — a subprocess with no timeout, on
/// whatever the repository sits on — and the daemon has one lock for every
/// RPC and every push.
#[tokio::test]
async fn a_board_items_project_list_renders_with_the_app_lock_released() {
    let (dir, repo) = init_repo();
    let (state, _handler) = shared_qa_state_and_handler(&repo, dir.path());
    let captured = state
        .lock()
        .unwrap()
        .board_lists(crate::changes::BoardLists::PROJECTS);

    // Rendered with the mutex in this test's own hand: whatever the capture
    // left to do may not need it back.
    let held = state.lock().unwrap();
    let rendered = captured.render();
    drop(held);

    assert_eq!(
        rendered["projects"],
        state.lock().unwrap().project_list()["projects"],
        "the same list `project.list` answers"
    );
}

/// A `files` item carries the worktree's root listing beside the paths that
/// moved — the same body `fs.tree` answers for `path: ""` — so a client
/// repaints its file tree's top level off the push and re-lists only the
/// deeper directories it is holding open.
#[tokio::test]
async fn a_files_item_carries_the_root_listing() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    let subscribed = handler.call(
        sender.clone(),
        req(
            "changes.subscribe",
            json!({
                "subscription_id": "s-files",
                "scope": { "kind": "entity", "id": project_id },
                "kinds": ["files"],
            }),
        ),
    );
    assert_eq!(subscribed["ok"], true, "{subscribed:?}");
    settled_pushes(&mut rx, &key).await;

    std::fs::write(repo.join("listed.txt"), "in the root\n").unwrap();
    state
        .lock()
        .unwrap()
        .changes()
        .note_files(&project_id, &["listed.txt".to_string()]);

    let pushes = settled_pushes(&mut rx, &key).await;
    let files = pushes
        .iter()
        .filter(|push| push["type"] == "changes")
        .flat_map(|frame| frame["items"].as_array().cloned().unwrap_or_default())
        .find(|item| item["entity_id"] == project_id && item.get("files").is_some())
        .map(|item| item["files"].clone())
        .unwrap_or_else(|| panic!("no files item: {pushes:?}"));

    assert_eq!(files["paths"], json!(["listed.txt"]), "{files:?}");
    let listed = call(
        &handler,
        "fs.tree",
        json!({ "project_id": project_id, "path": "" }),
    );
    assert_eq!(files["root"], listed["result"], "{files:?}");
    assert!(
        files["root"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "listed.txt"),
        "{files:?}"
    );
}

/// A `git` item carries the surfaces themselves: the status shape, the
/// latest commits and what is unpublished — the same bodies `git.status`,
/// `git.log` and `git.unpushed` answer with, so a client writes them into
/// its cache and asks nothing.
///
/// The one thing it does not carry is the working tree's hunks. Those are
/// the largest thing a checkout has and the surface that shows them is one
/// a reader has to open, so the item names the diff and its size and the
/// body is read on demand.
#[tokio::test]
async fn a_git_item_carries_the_shapes_the_client_would_have_pulled() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    let subscribed = handler.call(
        sender.clone(),
        req(
            "changes.subscribe",
            json!({
                "subscription_id": "s-git",
                "scope": { "kind": "entity", "id": project_id },
                "kinds": ["git"],
            }),
        ),
    );
    assert_eq!(subscribed["ok"], true, "{subscribed:?}");
    settled_pushes(&mut rx, &key).await;

    std::fs::write(repo.join("pushed.txt"), "a small change\n").unwrap();
    let git = pushed_git_item(&state, &mut rx, &key, &project_id, |git| {
        git["status"]["files"][0]["path"] == "pushed.txt"
    })
    .await;

    // The `git.status` answer, whole: the walk AND the line census it joins
    // on. The counts cannot be read off the patch riding beside it — a run's
    // diff is against its baseline and an external checkout's against the
    // branch it was cut from, neither of which is the working tree — so a
    // client that reconstructed them would paint branch-wide numbers on a
    // working-tree pane.
    let status = call(&handler, "git.status", json!({ "project_id": project_id }));
    assert_eq!(
        git["status"], status["result"],
        "the item carries what `git.status` answers: {git:?}"
    );
    assert_eq!(git["status"]["files"][0]["path"], "pushed.txt", "{git:?}");
    assert_eq!(git["status"]["files"][0]["added"], 1, "{git:?}");
    assert_eq!(git["status"]["files"][0]["deleted"], 0, "{git:?}");
    assert_eq!(git["status"]["stat"]["insertions"], 1, "{git:?}");
    assert_eq!(
        git["status_key"], status["result"]["status_key"],
        "the key still names the shape beside it"
    );
    let log = call(&handler, "git.log", json!({ "project_id": project_id }));
    assert_eq!(git["log"]["newest"], log["result"]["newest"], "{git:?}");
    assert!(git["log"]["commits"][0]["hash"].is_string(), "{git:?}");
    let unpushed = call(
        &handler,
        "git.unpushed",
        json!({ "project_id": project_id }),
    );
    assert_eq!(
        git["unpushed"]["diff_key"], unpushed["result"]["diff_key"],
        "{git:?}"
    );
    assert_eq!(
        git["unpushed"]["base"], unpushed["result"]["base"],
        "{git:?}"
    );
    assert!(
        git["unpushed"].get("patch").is_none(),
        "a patch is what the item deliberately leaves for the review surface: {git:?}"
    );
    let diff = call(
        &handler,
        "project.diff",
        json!({ "project_id": project_id }),
    );
    assert_eq!(
        git["diff"],
        Value::Null,
        "the item names the diff and carries none of it: {git:?}"
    );
    assert_eq!(
        git["diff_bytes"].as_u64(),
        Some(diff["result"]["patch"].as_str().unwrap().len() as u64),
        "the size it names is the body `project.diff` answers with: {git:?}"
    );
    assert!(
        diff["result"]["patch"]
            .as_str()
            .is_some_and(|patch| patch.contains("a small change")),
        "and that body is still there for the surface that asks: {diff:?}"
    );
}

/// A `git` item names one checkout's diff and the diff verb for that
/// checkout answers the body — whatever kind of checkout it is, so the read
/// a client makes on opening the changes fills the slot the push marked.
///
/// An external checkout's `worktree.diff` carries more than the stat, the
/// files and the patch: the checkout's id, the branch the diff is anchored
/// on so a surface can name it instead of saying "the base branch", and
/// whether it can be adopted. A read routed to a run's verb instead would
/// leave every one of those undefined the first time a reader opened the
/// changes.
#[tokio::test]
async fn a_checkouts_diff_is_named_by_the_push_and_answered_by_its_own_verb() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    let checkout =
        super::filesystem::add_external_worktree(&repo, dir.path(), "loose", "feature-loose");
    let worktree_id = state
        .lock()
        .unwrap()
        .scan_external_worktrees_now(&project_id)
        .expect("the project's checkouts are scanned")
        .first()
        .expect("the checkout was found")
        .id
        .clone();
    let subscribed = handler.call(
        sender.clone(),
        req(
            "changes.subscribe",
            json!({
                "subscription_id": "s-git",
                "scope": { "kind": "entity", "id": worktree_id },
                "kinds": ["git"],
            }),
        ),
    );
    assert_eq!(subscribed["ok"], true, "{subscribed:?}");
    settled_pushes(&mut rx, &key).await;

    std::fs::write(checkout.join("pushed.txt"), "a small change\n").unwrap();
    let git = pushed_git_item(&state, &mut rx, &key, &worktree_id, |git| {
        git.get("diff_bytes").is_some()
    })
    .await;

    let read = call(
        &handler,
        "worktree.diff",
        json!({ "project_id": project_id, "worktree_id": worktree_id }),
    );
    assert_eq!(git["diff"], Value::Null, "{git:?}");
    assert_eq!(
        git["diff_bytes"].as_u64(),
        Some(read["result"]["patch"].as_str().unwrap().len() as u64),
        "the item sizes the body this checkout's own verb answers: {git:?} against {read:?}"
    );
    assert!(
        read["result"]["patch"]
            .as_str()
            .is_some_and(|patch| patch.contains("a small change")),
        "{read:?}"
    );
    assert_eq!(
        read["result"]["worktree_id"],
        worktree_id.as_str(),
        "{read:?}"
    );
    assert_eq!(read["result"]["base_branch"], "main", "{read:?}");
}

/// A working tree with more diff than a push may carry says how big it is and
/// carries none of it. The client reads it when a reviewer opens the changes.
#[tokio::test]
async fn a_git_item_past_the_diff_cap_names_the_size_and_carries_no_diff() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    let subscribed = handler.call(
        sender.clone(),
        req(
            "changes.subscribe",
            json!({
                "subscription_id": "s-git",
                "scope": { "kind": "entity", "id": project_id },
                "kinds": ["git"],
            }),
        ),
    );
    assert_eq!(subscribed["ok"], true, "{subscribed:?}");
    settled_pushes(&mut rx, &key).await;

    let huge: String = std::iter::repeat_n(
        "a line of a very large change\n",
        crate::changes::WORKING_TREE_DIFF_MAX_BYTES / 10,
    )
    .collect();
    std::fs::write(repo.join("huge.txt"), huge).unwrap();
    let git = pushed_git_item(&state, &mut rx, &key, &project_id, |git| {
        git.get("diff_bytes").is_some()
    })
    .await;

    assert_eq!(git["diff"], Value::Null, "{git:?}");
    assert!(
        git["diff_bytes"].as_u64().unwrap() > crate::changes::WORKING_TREE_DIFF_MAX_BYTES as u64,
        "{git:?}"
    );
    assert!(
        git["status"]["files"][0]["path"].is_string(),
        "the rest of the item is unaffected: {git:?}"
    );
    // With no patch on the item there is nothing to count from, so the
    // census the status carries is the only thing a client has.
    assert!(
        git["status"]["files"][0]["added"].as_u64().unwrap() > 0,
        "{git:?}"
    );
    assert!(
        git["status"]["stat"]["insertions"].as_u64().unwrap() > 0,
        "{git:?}"
    );
}

/// The `git` half of one entity's items, waited for until it says what the
/// test is about. The worktree's own notes are paced by the settle window,
/// so a git surface may take a second to arrive.
async fn pushed_git_item(
    state: &Arc<Mutex<AppState>>,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
    key: &str,
    entity_id: &str,
    ready: impl Fn(&Value) -> bool,
) -> Value {
    let mut seen = Vec::new();
    for _ in 0..40 {
        state.lock().unwrap().note_entity_settled(entity_id);
        for push in settled_pushes(rx, key).await {
            if push["type"] != "changes" {
                continue;
            }
            for item in push["items"].as_array().cloned().unwrap_or_default() {
                if item["entity_id"] != entity_id {
                    continue;
                }
                if ready(&item["git"]) {
                    return item["git"].clone();
                }
                seen.push(item);
            }
        }
    }
    panic!("no git item for {entity_id} saying so: {seen:?}");
}

/// A `thread` item carries the conversation, not a hint about it: the first
/// flush says where the conversation stands, and the one after it carries
/// what was said in between — until a burst wider than the push cap, which
/// goes back to the tip alone and leaves the client to page forward.
#[tokio::test]
async fn a_thread_item_carries_what_was_said_since_the_last_flush() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let run_id = {
        let mut app = state.lock().unwrap();
        planned_run_in_review(&mut app, "thread rides the item").1
    };
    let subscribed = handler.call(
        sender.clone(),
        req(
            "changes.subscribe",
            json!({
                "subscription_id": "s-thread",
                "scope": { "kind": "entity", "id": run_id },
                "kinds": ["thread"],
            }),
        ),
    );
    assert_eq!(subscribed["ok"], true, "{subscribed:?}");
    settled_pushes(&mut rx, &key).await;

    // A flush with nothing said since the last one: the tip, and no items to
    // place against it.
    state.lock().unwrap().note_entity_changed(&run_id);
    let first = thread_tip(&settled_pushes(&mut rx, &key).await, &run_id);
    assert_eq!(first["items"], json!([]), "{first:?}");
    let tip = first["last_sequence"]
        .as_u64()
        .expect("a tip is a sequence");

    // The next one: what was said after it, in full.
    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": run_id, "body": "the item carries this" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    let second = thread_tip(&settled_pushes(&mut rx, &key).await, &run_id);
    assert_eq!(second["since_sequence"], json!(tip), "{second:?}");
    let bodies: Vec<&str> = second["items"]
        .as_array()
        .expect("the item carries items")
        .iter()
        .filter_map(|item| item["data"]["body"].as_str())
        .collect();
    assert!(bodies.contains(&"the item carries this"), "{second:?}");

    // A burst wider than the cap: the tip alone, and the client pages.
    {
        let mut app = state.lock().unwrap();
        let active = app.runs.get_mut(&run_id).expect("the run");
        for turn in 0..(crate::changes::THREAD_PUSH_MAX_ITEMS + 5) {
            primary_thread_mut(&mut active.agents).post_user(
                format!("burst {turn}"),
                None,
                crate::store::now_rfc3339(),
            );
        }
        app.note_entity_changed(&run_id);
    }
    let third = thread_tip(&settled_pushes(&mut rx, &key).await, &run_id);
    assert_eq!(third["items"], json!([]), "{third:?}");
    assert_eq!(third["since_sequence"], Value::Null, "{third:?}");
    assert!(
        third["last_sequence"].as_u64().unwrap() > tip,
        "the tip still says where the conversation got to: {third:?}"
    );
}

/// A message the client sent under an operation says so on the item itself,
/// on the push and on the page alike.
///
/// This is what lets a client draw a message the moment it presses send and
/// have the conversation take that stand-in away when it arrives: the item is
/// the reader's own words coming back, and the operation is the only thing
/// that says so — the sequence belongs to the bridge and the client had none
/// to wait under.
#[tokio::test]
async fn a_message_posted_under_an_operation_carries_it_on_the_item() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let run_id = {
        let mut app = state.lock().unwrap();
        planned_run_in_review(&mut app, "the operation rides the item").1
    };
    let subscribed = handler.call(
        sender.clone(),
        req(
            "changes.subscribe",
            json!({
                "subscription_id": "s-operation",
                "scope": { "kind": "entity", "id": run_id },
                "kinds": ["thread"],
            }),
        ),
    );
    assert_eq!(subscribed["ok"], true, "{subscribed:?}");
    settled_pushes(&mut rx, &key).await;
    // The first flush is the tip alone; the cursor it leaves is what the next
    // one carries items against.
    state.lock().unwrap().note_entity_changed(&run_id);
    settled_pushes(&mut rx, &key).await;

    let posted = call(
        &handler,
        "thread.post",
        json!({
            "entity_id": run_id,
            "operation_id": "op-1",
            "body": "ship it"
        }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");

    let pushed = thread_tip(&settled_pushes(&mut rx, &key).await, &run_id);
    let sent = pushed["items"]
        .as_array()
        .expect("the item carries items")
        .iter()
        .find(|item| item["data"]["body"] == json!("ship it"))
        .unwrap_or_else(|| panic!("no message on the push: {pushed:?}"));
    assert_eq!(sent["data"]["operation_id"], json!("op-1"), "{sent:?}");

    let page = call(
        &handler,
        "thread.page",
        json!({ "entity_id": run_id, "limit": 20 }),
    );
    assert_eq!(page["ok"], true, "{page:?}");
    let paged = page["result"]["items"]
        .as_array()
        .expect("a page carries items")
        .iter()
        .find(|item| item["data"]["body"] == json!("ship it"))
        .unwrap_or_else(|| panic!("no message on the page: {page:?}"));
    assert_eq!(paged["data"]["operation_id"], json!("op-1"), "{paged:?}");
}

/// A message whose delivery status moved — handed to the agent, then seen by
/// it — is a message that CHANGED, and the reader is watching that word on it.
///
/// The push has to carry the message itself, not just a tip saying the
/// conversation moved: a client that is only told "something moved" has to
/// read the conversation again to find out what, and until it does the reader
/// watches their own message sit on the word it arrived with.
#[tokio::test]
async fn a_delivery_status_moving_pushes_the_message_it_moved_on() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let run_id = {
        let mut app = state.lock().unwrap();
        planned_run_in_review(&mut app, "delivery status rides the item").1
    };
    let subscribed = handler.call(
        sender.clone(),
        req(
            "changes.subscribe",
            json!({
                "subscription_id": "s-delivery",
                "scope": { "kind": "entity", "id": run_id },
                "kinds": ["thread"],
            }),
        ),
    );
    assert_eq!(subscribed["ok"], true, "{subscribed:?}");
    settled_pushes(&mut rx, &key).await;

    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": run_id, "operation_id": "op-watched", "body": "take a look" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");
    let arrival = thread_tip(&settled_pushes(&mut rx, &key).await, &run_id);
    let agent_id = arrival["agent_id"]
        .as_str()
        .expect("a tip names its agent")
        .to_string();
    let first = arrival["items"]
        .as_array()
        .expect("the item carries items")
        .iter()
        .find(|item| item["data"]["body"] == json!("take a look"))
        .unwrap_or_else(|| panic!("no message on the push: {arrival:?}"));
    let arrived_at = first["data"]["updated_sequence"]
        .as_u64()
        .max(first["data"]["sequence"].as_u64())
        .expect("an item stands at a sequence");
    let cursor = arrival["last_sequence"]
        .as_u64()
        .expect("a tip is a sequence");

    // The agent takes the turn. Nothing new is SAID: the only thing that moved
    // is the word on the message the reader is watching.
    state
        .lock()
        .unwrap()
        .record_native_operation_seen(&run_id, &agent_id, "op-watched")
        .expect("the operation is this conversation's");

    let delivered = thread_tip(&settled_pushes(&mut rx, &key).await, &run_id);
    assert!(
        delivered["last_sequence"].as_u64().unwrap() > cursor,
        "the conversation moved, so its tip moved: {delivered:?}"
    );
    let moved = delivered["items"]
        .as_array()
        .expect("the item carries items")
        .iter()
        .find(|item| item["data"]["body"] == json!("take a look"))
        .unwrap_or_else(|| panic!("the message that moved is not on the push: {delivered:?}"));
    assert_eq!(moved["data"]["delivery_status"], json!("seen"), "{moved:?}");
    assert!(
        moved["data"]["updated_sequence"].as_u64().unwrap() > arrived_at,
        "the item carries the sequence it moved at, which is how a client merges it \
         over the copy it holds: {moved:?}"
    );
}

/// The first `thread` tip one entity's items carry, out of a push history.
fn thread_tip(pushes: &[Value], entity_id: &str) -> Value {
    pushes
        .iter()
        .filter(|push| push["type"] == "changes")
        .flat_map(|frame| frame["items"].as_array().cloned().unwrap_or_default())
        .find(|item| item["entity_id"] == entity_id && item["thread"][0].is_object())
        .map(|item| item["thread"][0].clone())
        .unwrap_or_else(|| panic!("no thread item for {entity_id}: {pushes:?}"))
}

/// A tab opening and a tab closing both move the `terminals` kind, and the
/// item carries the list `term.list` would answer — so a client repaints its
/// tab row off the push and asks nothing.
#[tokio::test]
async fn a_terminals_item_carries_the_tabs_a_checkout_holds() {
    let (dir, repo) = init_repo();
    let (state, handler, sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let project_id = state.lock().unwrap().project_at(0).id.clone();
    let subscribed = handler.call(
        sender.clone(),
        req(
            "changes.subscribe",
            json!({
                "subscription_id": "s-tabs",
                "scope": { "kind": "entity", "id": project_id },
                "kinds": ["terminals"],
            }),
        ),
    );
    assert_eq!(subscribed["ok"], true, "{subscribed:?}");
    settled_pushes(&mut rx, &key).await;

    let created = call(&handler, "term.create", json!({ "project_id": project_id }));
    assert_eq!(created["ok"], true, "{created:?}");
    let opened = terminals_item(&settled_pushes(&mut rx, &key).await, &project_id);
    let listed = call(&handler, "term.list", json!({ "project_id": project_id }));
    assert_eq!(opened["tabs"], listed["result"]["terminals"], "{opened:?}");
    assert_eq!(
        opened["tabs"].as_array().map(Vec::len),
        Some(1),
        "{opened:?}"
    );

    let closed = call(&handler, "term.close", json!({ "term_id": "term-1" }));
    assert_eq!(closed["ok"], true, "{closed:?}");
    let after = terminals_item(&settled_pushes(&mut rx, &key).await, &project_id);
    assert_eq!(after["tabs"], json!([]), "{after:?}");
}

/// The `terminals` half of one entity's items out of a push history.
fn terminals_item(pushes: &[Value], entity_id: &str) -> Value {
    pushes
        .iter()
        .filter(|push| push["type"] == "changes")
        .flat_map(|frame| frame["items"].as_array().cloned().unwrap_or_default())
        .find(|item| item["entity_id"] == entity_id && item.get("terminals").is_some())
        .map(|item| item["terminals"].clone())
        .unwrap_or_else(|| panic!("no terminals item for {entity_id}: {pushes:?}"))
}

/// Step 1.5, the legacy default: a client that greets with NO `changes`
/// param is subscribed to today's events and to nothing else. A real
/// mutation over the wire reaches it as `{"type":"board.changed"}` and
/// `{"type":"entity.changed","id":…}` — those keys and no others, the bytes
/// a pre-subscriptions client parses — and never as a `changes` frame. The
/// session beside it that greeted with `"changes": "subscriptions"` hears
/// nothing at all from the same mutation until it subscribes, and then hears
/// only its own subscription's frame.
#[tokio::test]
async fn a_legacy_greeting_hears_only_the_legacy_frames_for_a_real_mutation() {
    let (dir, repo) = init_repo();
    let (state, handler, _sender, mut legacy_rx, legacy_key) =
        greeted_push_session(&repo, dir.path());
    let (opted_in, mut opted_in_rx, opted_in_key) = SessionSender::observable("opted-in");
    let greeting = handler.call(
        opted_in.clone(),
        req("session.hello", json!({ "changes": "subscriptions" })),
    );
    assert_eq!(greeting["result"]["changes"]["mode"], "subscriptions");
    let run_id = {
        let mut app = state.lock().unwrap();
        planned_run_in_review(&mut app, "legacy hears this").1
    };
    settled_pushes(&mut legacy_rx, &legacy_key).await;
    settled_pushes(&mut opted_in_rx, &opted_in_key).await;

    // A real mutation over the wire, not a hand-published note.
    let posted = call(
        &handler,
        "thread.post",
        json!({ "entity_id": run_id, "body": "a real mutation" }),
    );
    assert_eq!(posted["ok"], true, "{posted:?}");

    let legacy = settled_pushes(&mut legacy_rx, &legacy_key).await;
    let board = json!({ "type": "board.changed" });
    let entity = json!({ "type": "entity.changed", "id": run_id });
    for frame in &legacy {
        assert!(
            *frame == board || *frame == entity,
            "a legacy session hears the two legacy frames and nothing else, \
             with no key beyond the ones it always carried: {frame:?}"
        );
    }
    assert!(legacy.contains(&board), "{legacy:?}");
    assert!(legacy.contains(&entity), "{legacy:?}");
    assert_eq!(
        settled_pushes(&mut opted_in_rx, &opted_in_key).await,
        Vec::<Value>::new(),
        "a session that opted into subscriptions hears nothing until it subscribes"
    );

    // ... and once it subscribes it hears its own frame, which is the one a
    // legacy session never sees.
    let subscribed = handler.call(
        opted_in.clone(),
        req(
            "changes.subscribe",
            json!({
                "subscription_id": "s-run",
                "scope": { "kind": "entity", "id": run_id },
                "kinds": ["state"],
            }),
        ),
    );
    assert_eq!(subscribed["ok"], true, "{subscribed:?}");
    settled_pushes(&mut opted_in_rx, &opted_in_key).await;
    state.lock().unwrap().note_entity_changed(&run_id);

    assert!(
        settled_pushes(&mut opted_in_rx, &opted_in_key)
            .await
            .iter()
            .any(|push| push["type"] == "changes"),
        "the subscribed session hears the new frame"
    );
    assert!(
        settled_pushes(&mut legacy_rx, &legacy_key)
            .await
            .iter()
            .all(|push| push["type"] != "changes"),
        "the legacy session never hears one"
    );
}

/// A write in a workspace moves its inbox row too: the row's stat is a fact
/// of the checkout, so a git flush for an entity re-sends its `state` — with
/// the stat re-read — to the subscriptions that carry rows, rather than
/// leaving the row as the last lifecycle change left it.
#[tokio::test]
async fn a_git_flush_re_sends_the_rows_of_the_entities_that_moved() {
    let (dir, repo) = init_repo();
    let (_state, handler, sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let board = handler.call(sender.clone(), req("board.list", json!({})));
    let project_id = board["result"]["projects"][0]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let workspace = handler.call(
        sender.clone(),
        req(
            "workspace.create",
            json!({"project_id": project_id, "name": "spoken", "isolation": "worktree"}),
        ),
    );
    let workspace_id = workspace["result"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    let git_dir = std::path::PathBuf::from(
        workspace["result"]["directories"][0]["path"]
            .as_str()
            .unwrap(),
    );
    let conversation = handler.call(
        sender.clone(),
        req(
            "workspace.ensure_conversation",
            json!({"workspace_id": workspace_id}),
        ),
    );
    let run_id = conversation["result"]["entity_id"]
        .as_str()
        .unwrap()
        .to_string();
    for (id, scope, kinds) in [
        ("s-inbox", json!({"kind": "all"}), json!(["state"])),
        (
            "s-active",
            json!({"kind": "entity", "id": run_id}),
            json!(["git", "files"]),
        ),
    ] {
        let subscribed = handler.call(
            sender.clone(),
            req(
                "changes.subscribe",
                json!({"subscription_id": id, "scope": scope, "kinds": kinds, "mode": "realtime"}),
            ),
        );
        assert_eq!(subscribed["result"]["watch"], "live", "{subscribed:?}");
    }
    settled_pushes(&mut rx, &key).await;

    std::fs::write(git_dir.join("noted.txt"), "hello").unwrap();
    let mut row = None;
    for _ in 0..60 {
        let pushes = settled_pushes(&mut rx, &key).await;
        row = pushes
            .iter()
            .filter(|push| push["type"] == "changes" && push["subscription_id"] == "s-inbox")
            .flat_map(|frame| frame["items"].as_array().cloned().unwrap_or_default())
            .find(|item| item["entity_id"] == run_id)
            .map(|item| item["state"].clone());
        if row
            .as_ref()
            .is_some_and(|row| row["stat"]["uncommitted"]["files_changed"] == json!(1))
        {
            break;
        }
    }
    let row = row.expect("the write re-sent the run's row on the state subscription");
    assert_eq!(row["run_id"], json!(run_id), "{row:?}");
    assert_eq!(
        row["stat"]["uncommitted"]["files_changed"],
        json!(1),
        "{row:?}"
    );
}

/// The inbox line under a workspace's row is its work summary, which rides
/// the workspace list. A write in the workspace moves that too: a git flush
/// for its conversation re-reads the summary and the board item that follows
/// carries the list with it.
#[tokio::test]
async fn a_git_flush_re_sends_the_workspace_list_with_its_summary() {
    let (dir, repo) = init_repo();
    let (_state, handler, sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let board = handler.call(sender.clone(), req("board.list", json!({})));
    let project_id = board["result"]["projects"][0]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let workspace = handler.call(
        sender.clone(),
        req(
            "workspace.create",
            json!({"project_id": project_id, "name": "spoken", "isolation": "worktree"}),
        ),
    );
    let workspace_id = workspace["result"]["workspace_id"]
        .as_str()
        .unwrap()
        .to_string();
    let git_dir = std::path::PathBuf::from(
        workspace["result"]["directories"][0]["path"]
            .as_str()
            .unwrap(),
    );
    let conversation = handler.call(
        sender.clone(),
        req(
            "workspace.ensure_conversation",
            json!({"workspace_id": workspace_id}),
        ),
    );
    let run_id = conversation["result"]["entity_id"]
        .as_str()
        .unwrap()
        .to_string();
    for (id, scope, kinds) in [
        ("s-inbox", json!({"kind": "all"}), json!(["state"])),
        (
            "s-active",
            json!({"kind": "entity", "id": run_id}),
            json!(["git", "files"]),
        ),
    ] {
        let subscribed = handler.call(
            sender.clone(),
            req(
                "changes.subscribe",
                json!({"subscription_id": id, "scope": scope, "kinds": kinds, "mode": "realtime"}),
            ),
        );
        assert_eq!(subscribed["result"]["watch"], "live", "{subscribed:?}");
    }
    settled_pushes(&mut rx, &key).await;

    std::fs::write(git_dir.join("noted.txt"), "hello\n").unwrap();
    let mut listed = None;
    let mut heard = Vec::new();
    for _ in 0..60 {
        let pushes = settled_pushes(&mut rx, &key).await;
        heard.extend(
            pushes
                .iter()
                .filter(|push| push["subscription_id"] == "s-inbox")
                .cloned(),
        );
        listed = pushes
            .iter()
            .filter(|push| push["type"] == "changes" && push["subscription_id"] == "s-inbox")
            .flat_map(|frame| frame["items"].as_array().cloned().unwrap_or_default())
            .filter(|item| item["entity_id"] == crate::changes::BOARD_ITEM_ID)
            .filter_map(|item| item["state"]["workspaces"].as_array().cloned())
            .flatten()
            .find(|row| row["workspace_id"] == json!(workspace_id));
        if listed
            .as_ref()
            .is_some_and(|row| row["work_summary"]["dirty"] == json!(true))
        {
            break;
        }
    }
    let listed = listed
        .unwrap_or_else(|| panic!("no workspace list on the board item; inbox heard {heard:?}"));
    // The summary counts the unpushed commit's lines beside the uncommitted
    // ones, so the write is read off the dirty flag it flipped and the
    // addition it brought, not off an exact total.
    assert_eq!(listed["work_summary"]["dirty"], json!(true), "{listed:?}");
    assert!(
        listed["work_summary"]["additions"].as_u64().unwrap_or(0) >= 1,
        "{listed:?}"
    );
}
