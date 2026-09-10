use super::*;

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
        hello["result"]["events"],
        json!(["board.changed", "entity.changed"]),
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
    let (_state, handler, _sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    settled_pushes(&mut rx, &key).await; // boot noise

    let plan = call(&handler, "plan.create", json!({ "goal": "push me" }));
    assert_eq!(plan["ok"], true, "{plan:?}");
    let plan_id = plan_id_of(&plan);

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
    let project_id = state.lock().unwrap().projects[0].id.clone();
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
    let project_id = state.lock().unwrap().projects[0].id.clone();
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
    let project_id = state.lock().unwrap().projects[0].id.clone();

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
    let (_state, handler, _sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let plan = call(&handler, "plan.create", json!({ "goal": "coalesce me" }));
    let plan_id = plan_id_of(&plan);
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
    let (state, handler, _sender, mut rx, key) = greeted_push_session(&repo, dir.path());
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

    let plan = call(
        &handler,
        "plan.create",
        json!({ "goal": "nobody hears this" }),
    );
    assert_eq!(plan["ok"], true, "{plan:?}");
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
    let (state, handler, _sender, mut rx, key) = greeted_push_session(&repo, dir.path());
    let plan = call(&handler, "plan.create", json!({ "goal": "agent moved me" }));
    let plan_id = plan_id_of(&plan);
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
