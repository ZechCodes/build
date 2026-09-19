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
    // Step 1.5: what a Part 1 adapter reads instead of probing.
    assert_eq!(
        hello["result"]["changes"],
        json!({
            "subscriptions": true,
            "mode": "legacy",
            "kinds": ["state", "thread", "git", "files", "terminals"],
            "batch_ms": { "min": 1000, "max": 600_000 },
        }),
        "{hello:?}"
    );
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
    let mut frames = Vec::new();
    for _ in 0..40 {
        frames.extend(
            settled_pushes(&mut rx, &key)
                .await
                .into_iter()
                .filter(|push| push["type"] == "changes"),
        );
        if !frames.is_empty() {
            break;
        }
    }
    let item = frames
        .iter()
        .flat_map(|frame| frame["items"].as_array().cloned().unwrap_or_default())
        .find(|item| item["entity_id"] == project_id)
        .unwrap_or_else(|| panic!("no changes item for the project: {frames:?}"));
    assert_eq!(item["files"]["paths"], json!(["noted.txt"]), "{item:?}");
    assert!(item["git"]["status_key"].is_string(), "{item:?}");
    assert_eq!(frames[0]["subscription_id"], "s-focus");
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
