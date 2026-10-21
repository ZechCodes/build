//! Terminal creation records presence and attention without waiting for SQLite
//! while holding application state.
use super::*;

mod equivalence;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_activity_waits_for_store_without_holding_app_state() {
    let directory = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracker::tracked(directory.path());
    app.term_shell = "/bin/sh".into();
    app.initialize_snapshot_projection();
    let workspace = app.workspaces.list(Some(&project))[0].id.clone();
    let state = Arc::new(Mutex::new(app));
    state.lock().unwrap().self_handle = Some(Arc::downgrade(&state));
    let handler = AppState::handler(Arc::clone(&state));
    let (store, before_session) = {
        let app = state.lock().unwrap();
        (app.store.clone().unwrap(), app.user_session())
    };
    let writer = store.writer_for_test();
    let creating = handler.clone();
    let scope = workspace.clone();
    let worker =
        std::thread::spawn(move || call(&creating, "term.create", json!({"workspace_id": scope})));
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut observed = false;
    while std::time::Instant::now() < deadline {
        if state
            .try_lock()
            .is_ok_and(|app| app.shell_tab_count() == 1 && app.user_session() != before_session)
        {
            observed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // Release before joining/asserting so a red run cannot strand a child or
    // the creation thread behind the deliberately held writer.
    drop(writer);
    let created = worker.join().unwrap();
    assert_eq!(created["ok"], true, "{created}");
    let (session, child) = {
        let app = state.lock().unwrap();
        let key = app
            .tab_key_of_wire_id(created["result"]["term_id"].as_str().unwrap())
            .unwrap();
        (
            app.user_session(),
            Arc::clone(&app.session_registry.test_tab(&key).unwrap().session),
        )
    };
    let persisted = store.load_user_session().unwrap();
    let closed = call(
        &handler,
        "term.close",
        json!({"term_id": created["result"]["term_id"]}),
    );
    child.end();
    assert!(child.exited_within(Duration::from_secs(2)));
    assert_eq!(closed["ok"], true, "{closed}");
    assert!(
        observed,
        "terminal presence must publish while SQLite is unavailable, with the app mutex released"
    );
    assert_eq!(persisted, Some(session));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_attention_returns_before_store_and_preserves_newer_stamps() {
    assert_terminal_attention_does_not_wait(false);
    assert_terminal_attention_does_not_wait(true);
}

fn assert_terminal_attention_does_not_wait(run_scope: bool) {
    use crate::app::snapshot_projection::work_counts;
    const OLD: &str = "2001-01-01T00:00:00Z";
    let directory = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracker::tracked(directory.path());
    app.term_shell = "/bin/sh".into();
    app.initialize_snapshot_projection();
    let entity = if run_scope {
        let root = app.projects.get(&project).unwrap().repo_path.clone();
        adopted_run(&mut app, &root, directory.path(), "terminal-attention")
    } else {
        app.workspaces.list(Some(&project))[0].id.clone()
    };
    let mut old_attention = crate::attention::Attention::default();
    old_attention.interact(OLD);
    let mut entries = app
        .board
        .attention()
        .persistence()
        .into_entries()
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();
    entries.insert(entity.clone(), old_attention.clone());
    app.board.attention_mut().replace_entries(entries);
    // Isolate attention's writer wait. The separate presence test exercises
    // its save; this already-current session needs no new persistence.
    app.user_session = crate::session_summary::UserSession {
        session_started_ms: Some(0),
        last_activity_ms: Some(i64::MAX),
        previous_session_ended_ms: None,
    };
    app.initialize_snapshot_projection();
    let samples = app.frame_clock.capture_phases();
    let store = app.store.clone().unwrap();
    let state = Arc::new(Mutex::new(app));
    state.lock().unwrap().self_handle = Some(Arc::downgrade(&state));
    let handler = AppState::handler(Arc::clone(&state));
    let mut subscription = BoardSubscriber::new(&state);
    let before_revision = subscription.bus.board_revision();
    let writer = store.writer_for_test();
    let selector = if run_scope {
        json!({"run_id":entity})
    } else {
        json!({"workspace_id":entity})
    };
    let (sent, received) = std::sync::mpsc::channel();
    let worker = {
        let handler = handler.clone();
        std::thread::spawn(move || {
            sent.send(work_counts::measure(|| {
                call(&handler, "term.create", selector)
            }))
            .unwrap();
        })
    };
    let early = received.recv_timeout(Duration::from_secs(2));
    let replied_while_writer_busy = early.is_ok();
    // A concurrent durable seen/mute update must survive the delayed stamp.
    let mut newer = old_attention;
    newer.muted = true;
    newer.seen_state_at = Some("2026-10-21T00:00:00Z".into());
    writer
        .execute(
            "INSERT INTO attention (entity_id, record) VALUES (?1, ?2) \
             ON CONFLICT(entity_id) DO UPDATE SET record = excluded.record",
            rusqlite::params![entity, serde_json::to_string(&newer).unwrap()],
        )
        .unwrap();
    drop(writer);
    let (created, counts) = early.unwrap_or_else(|_| received.recv().unwrap());
    worker.join().unwrap();
    assert_eq!(created["ok"], true, "{created}");
    let (attention, revision, published, child) = {
        let app = state.lock().unwrap();
        let key = app
            .tab_key_of_wire_id(created["result"]["term_id"].as_str().unwrap())
            .unwrap();
        (
            app.board.attention().attention(&entity).cloned(),
            app.changes().board_revision(),
            app.read_view.load().get_owned("board.list", "").unwrap(),
            Arc::clone(&app.session_registry.test_tab(&key).unwrap().session),
        )
    };
    let phases = samples.snapshot();
    let pushes = subscription.flush();
    let closed = call(
        &handler,
        "term.close",
        json!({"term_id":created["result"]["term_id"]}),
    );
    child.end();
    let reaped = child.exited_within(Duration::from_secs(2));
    assert!(reaped);
    assert_eq!(closed["ok"], true, "{closed}");
    assert!(
        replied_while_writer_busy,
        "attention persistence delayed the terminal reply"
    );
    let attention = attention.expect("successful terminal creation records attention");
    assert_ne!(attention.last_interaction_at.as_deref(), Some(OLD));
    assert_eq!(
        revision,
        before_revision + 1,
        "terminal open must bump board recency once"
    );
    assert!(
        pushes.iter().any(|event| {
            event["items"].as_array().is_some_and(|items| {
                items
                    .iter()
                    .any(|item| item["entity_id"] == crate::changes::BOARD_ITEM_ID)
            })
        }),
        "workspace and run opens must publish their board recency bump: {pushes:?}"
    );
    if run_scope {
        let row = published["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["run_id"] == entity)
            .unwrap();
        assert_eq!(row["attention"]["resume_at"], json!(attention.resume_at));
    }
    assert_eq!(counts.workspace_details_full, 0, "{counts:?}");
    assert_eq!(counts.workspace_details_scoped, 0, "{counts:?}");
    assert!(
        phases["term.create"]
            .get("entered.publication.global")
            .is_none(),
        "{phases}"
    );
    assert_delayed_attention_merge(&store, &entity, &attention, &newer);
}

fn assert_delayed_attention_merge(
    store: &crate::store::Store,
    entity: &str,
    attention: &crate::attention::Attention,
    newer: &crate::attention::Attention,
) {
    assert!(
        settles(|| {
            store.load_attention().get(entity).is_some_and(|saved| {
                saved.last_interaction_at == attention.last_interaction_at
                    && saved.muted
                    && saved.seen_state_at == newer.seen_state_at
            })
        }),
        "delayed interaction must merge with current durable seen/mute state"
    );
}

#[tokio::test]
async fn terminal_attention_first_external_interaction_publishes_and_persists() {
    let directory = tempfile::tempdir().unwrap();
    let (_repository, mut app, project) = tracker::tracked(directory.path());
    app.term_shell = "/bin/sh".into();
    let root = app.projects.get(&project).unwrap().repo_path.clone();
    add_external_worktree(
        &root,
        directory.path(),
        "external-attention",
        "external-attention",
    );
    let owner = app
        .scan_external_worktrees_now(&project)
        .unwrap()
        .into_iter()
        .find(|worktree| worktree.branch.as_deref() == Some("external-attention"))
        .unwrap()
        .id;
    app.initialize_snapshot_projection();
    assert!(app.board.attention().attention(&owner).is_none());
    let store = app.store.clone().unwrap();
    let state = Arc::new(Mutex::new(app));
    state.lock().unwrap().self_handle = Some(Arc::downgrade(&state));
    let handler = AppState::handler(Arc::clone(&state));
    let created = call(
        &handler,
        "term.create",
        json!({"project_id":project, "worktree_id":owner}),
    );
    assert_eq!(created["ok"], true, "{created}");
    let (attention, published, child) = {
        let app = state.lock().unwrap();
        let key = app
            .tab_key_of_wire_id(created["result"]["term_id"].as_str().unwrap())
            .unwrap();
        (
            app.attention_json(&owner),
            app.read_view.load().get_owned("board.list", "").unwrap(),
            Arc::clone(&app.session_registry.test_tab(&key).unwrap().session),
        )
    };
    let closed = call(
        &handler,
        "term.close",
        json!({"term_id":created["result"]["term_id"]}),
    );
    child.end();
    assert!(child.exited_within(Duration::from_secs(2)));
    assert_eq!(closed["ok"], true, "{closed}");
    let row = published["external_worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["worktree_id"] == owner)
        .unwrap();
    assert_eq!(row["attention"]["resume_at"], attention["resume_at"]);
    assert_eq!(row["attention"]["interacted"], attention["interacted"]);
    assert_eq!(row["attention"]["seen"], attention["seen"]);
    assert!(row["attention"]["anchor"].is_string());
    assert_eq!(attention["interacted"], true);
    assert!(
        settles(|| store.load_attention().get(&owner).is_some_and(|saved| {
            json!(saved.resume_at) == attention["resume_at"] && saved.last_interaction_at.is_some()
        })),
        "first external interaction must retain its previous durable semantics"
    );
}

// Managed workspace summaries expose their selected run's attention, not an
// attention field for Workspace.id itself. Its existing recency signal is the
// board revision push; assert that signal without redirecting the owner stamp.
struct BoardSubscriber {
    bus: Arc<crate::changes::ChangeBus>,
    receiver: tokio::sync::mpsc::UnboundedReceiver<crate::carrier::OutboundEnvelope>,
    key: String,
}

impl BoardSubscriber {
    fn new(state: &Arc<Mutex<AppState>>) -> Self {
        let bus = {
            let mut app = state.lock().unwrap();
            assert!(app
                .facts_handle
                .set(crate::app::facts::FactsSource::new(
                    app.read_view.clone(),
                    std::sync::Weak::new(),
                ))
                .is_ok());
            app.changes = crate::app::facts::bus_with_sources(
                Duration::ZERO,
                &app.watchers,
                &app.facts_handle,
            );
            app.changes()
        };
        let (session, receiver, key) = SessionSender::observable("terminal-attention-subscriber");
        bus.subscribe(
            &session,
            crate::changes::SubscriptionSpec {
                id: "board".into(),
                scope: crate::changes::Scope::Board,
                kinds: [crate::changes::Kind::State].into_iter().collect(),
                mode: Default::default(),
                priority: Default::default(),
            },
        );
        let mut subscriber = Self { bus, receiver, key };
        subscriber.bus.note_board();
        assert!(!subscriber.flush().is_empty());
        subscriber
    }

    fn flush(&mut self) -> Vec<Value> {
        self.bus.flush();
        let mut events = Vec::new();
        while let Ok(envelope) = self.receiver.try_recv() {
            events.push(SessionSender::decrypt_push(&self.key, &envelope));
        }
        events
    }
}
