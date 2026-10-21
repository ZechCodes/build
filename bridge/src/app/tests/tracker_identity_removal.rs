//! An agent removed by another client changes task destinations, even when
//! its workspace stays and neither agent is watched.

use super::*;
use crate::tracker::{Assignee, Task};

struct Fixture {
    project: String,
    workspace: String,
    entity: String,
    removed: String,
    survivor: String,
    open: String,
    closed: String,
    unrelated: String,
}

fn add_unwatched_agent(app: &mut AppState, entity: &str, name: &str) -> String {
    let added = app
        .agent_add(&json!({
            "entity_id": entity, "provider": "pi", "notify_user": false,
        }))
        .unwrap();
    let id = added["agent"]["id"].as_str().unwrap().to_string();
    app.set_agent_name(entity, &id, name).unwrap();
    assert!(
        !app.entity_agents(entity)
            .unwrap()
            .by_id(&id)
            .unwrap()
            .watched
    );
    id
}

fn comment_as(app: &mut AppState, entity: &str, agent: &str, task: &str) {
    app.on_agent_mcp_action(
        entity,
        agent,
        crate::mcp::BridgeAction::TrackerCommentTask {
            task_id: task.into(),
            body: "The fix is here.".into(),
            refs: Vec::new(),
            track: Some(false),
            attachments: Vec::new(),
            notify_user: None,
            mention_user: None,
        },
    )
    .unwrap();
}

fn seed(app: &mut AppState) -> Fixture {
    let board = app.handle(req("board.list", json!({})));
    let project = board["result"]["projects"][0]["project_id"]
        .as_str()
        .unwrap()
        .to_string();
    let workspace = super::project_agent::workspace(app, &project, "identity checkout");
    let ensured = app.handle(req(
        "workspace.ensure_conversation",
        json!({ "workspace_id": workspace }),
    ));
    let entity = ensured["result"]["entity_id"].as_str().unwrap().to_string();
    let removed = add_unwatched_agent(app, &entity, "Historian");
    let survivor = add_unwatched_agent(app, &entity, "Keeper");
    let created = app.handle(req(
        "tasks.create",
        json!({
            "project_id": project, "title": "Remote agent removal",
            "body": format!("Ask @agent:{removed} or @agent:{survivor}."),
        }),
    ));
    let open = created["result"]["task"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    comment_as(app, &entity, &removed, &open);
    // Seed an existing assignment without starting a harness for this fixture.
    let store = app.tracker_store().unwrap();
    let mut task = store.load_tracker_task(&open).unwrap().unwrap();
    task.assignee = Some(Assignee::Agent {
        agent_id: removed.clone(),
    });
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();

    let closed = super::tracker::filed(app, &project, "Closed historical reference")["id"]
        .as_str()
        .unwrap()
        .to_string();
    comment_as(app, &entity, &removed, &closed);
    assert_eq!(
        app.handle(req("tasks.close", json!({ "task_id": closed })))["ok"],
        true
    );
    let unrelated = super::tracker::filed(app, &project, "Unrelated")["id"]
        .as_str()
        .unwrap()
        .to_string();
    Fixture {
        project,
        workspace,
        entity,
        removed,
        survivor,
        open,
        closed,
        unrelated,
    }
}

fn read_task(app: &AppState, id: &str) -> Task {
    app.tracker_store()
        .unwrap()
        .load_tracker_task(id)
        .unwrap()
        .unwrap()
}

fn removing_an_agent(fail_persistence: bool) -> Value {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path()).with_change_window(Duration::ZERO);
    let fixture = seed(&mut app);
    let before_saved = read_task(&app, &fixture.open);
    let state = app.shared();
    let changes = state.lock().unwrap().changes();
    let handler = AppState::handler(Arc::clone(&state));
    let (reader, mut rx, key) = SessionSender::observable("task-reader");
    let greeting = handler.call(
        reader.clone(),
        req("session.hello", json!({ "changes": "subscriptions" })),
    );
    let read = || {
        json!({
            "get": handler.call(reader.clone(), req("tasks.get", json!({ "task_id": fixture.open })))["result"],
            "list": handler.call(reader.clone(), req("tasks.list", json!({ "project_id": fixture.project, "state": "open" })))["result"],
        })
    };
    let before = read();
    let subscribed = handler.call(reader.clone(), req("changes.subscribe", json!({
        "subscription_id": "task-identities", "scope": { "kind": "entity", "id": fixture.project },
        "kinds": ["tasks"],
    })));
    assert_eq!(subscribed["ok"], true, "{subscribed:?}");
    // Drive the real flusher synchronously; no timer or fixed-turn waits.
    changes.flush();
    while rx.try_recv().is_ok() {}

    if fail_persistence {
        rusqlite::Connection::open(dir.path().join("store/build.db"))
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER refuse_roster_save BEFORE UPDATE ON implementations
                BEGIN SELECT RAISE(FAIL, 'roster save refused'); END;",
            )
            .unwrap();
    }
    let removal = handler.call(
        SessionSender::detached("other-client"),
        req(
            "agent.remove",
            json!({
                "entity_id": fixture.entity, "agent_id": fixture.removed,
            }),
        ),
    );
    assert_eq!(removal["ok"], !fail_persistence, "{removal:?}");
    changes.flush();
    let mut events = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        events.push(SessionSender::decrypt_push(&key, &envelope));
    }
    let after = read();
    let trace = json!({
        "greeting": greeting["result"], "before": before, "after": after, "events": events,
        "workspace_id": fixture.workspace, "removed_agent_id": fixture.removed,
        "surviving_agent_id": fixture.survivor,
    });
    let ids: std::collections::BTreeSet<_> = events
        .iter()
        .filter(|event| event["type"] == "changes")
        .flat_map(|event| event["items"].as_array().unwrap())
        .filter(|item| item["entity_id"] == fixture.project)
        .flat_map(|item| item["tasks"]["task_ids"].as_array().into_iter().flatten())
        .map(|id| id.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        ids,
        std::collections::BTreeSet::from([fixture.open.clone(), fixture.closed.clone()]),
        "only tasks naming the removed agent must refresh; unrelated {}: {events:?}",
        fixture.unrelated
    );
    let app = state.lock().unwrap();
    assert!(app.workspaces.get(&fixture.workspace).is_some());
    assert!(app
        .entity_agents(&fixture.entity)
        .unwrap()
        .by_id(&fixture.survivor)
        .is_some());
    let saved = read_task(&app, &fixture.open);
    assert_eq!(saved.updated_at, before_saved.updated_at);
    let identities = &after["get"]["task"]["identities"];
    // Retirement is a live fact even when the durable roster save failed.
    assert_eq!(identities[&fixture.removed]["available"], false);
    assert_eq!(identities[&fixture.removed]["name"], "Historian");
    assert_eq!(identities[&fixture.removed]["provider"], "pi");
    assert_eq!(identities[&fixture.survivor]["available"], true);
    trace
}

/// The browser wiring runner sets BUILD_TASK_AGENT_REMOVAL_TRACE to consume
/// these real wire responses and encrypted/decrypted pushes in Chromium.
#[test]
fn remote_agent_removal_invalidates_task_identities() {
    let trace = removing_an_agent(false);
    if let Ok(path) = std::env::var("BUILD_TASK_AGENT_REMOVAL_TRACE") {
        std::fs::write(path, serde_json::to_vec_pretty(&trace).unwrap()).unwrap();
    }
}

#[test]
fn removed_live_agent_invalidates_tasks_even_if_saving_roster_fails() {
    removing_an_agent(true);
}

fn assert_routed_availability(state: &Arc<Mutex<AppState>>, fixture: &Fixture, available: bool) {
    let (reads, pool) = {
        let app = state.lock().unwrap();
        (app.read_service(), app.tracker_store().unwrap().read_pool())
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let get = reads.call(
        req("tasks.get", json!({"task_id": fixture.open})),
        Some(runtime.block_on(pool.acquire())),
    );
    let list = reads.call(
        req("tasks.list", json!({"project_id": fixture.project})),
        Some(runtime.block_on(pool.acquire())),
    );
    assert_eq!(get["ok"], true, "{get}");
    assert_eq!(list["ok"], true, "{list}");
    let listed = list["result"]["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == fixture.open)
        .unwrap();
    for task in [&get["result"]["task"], listed] {
        let identities = &task["identities"];
        assert_eq!(identities[&fixture.removed]["available"], available);
        assert_eq!(identities[&fixture.removed]["name"], "Historian");
        assert_eq!(identities[&fixture.removed]["provider"], "pi");
        assert_eq!(identities[&fixture.survivor]["available"], true);
    }
}

#[test]
fn configured_idle_agents_remain_available_in_routed_task_reads() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let fixture = seed(&mut app);
    let root = app.entity_agent_root(&fixture.entity).unwrap();
    for agent in [&fixture.removed, &fixture.survivor] {
        assert!(!app.session_registry.contains(&TabKey::agent(&root, agent)));
    }
    let state = app.shared();
    let _handler = AppState::handler(Arc::clone(&state));
    assert_routed_availability(&state, &fixture, true);
}

fn queue_and_install_live_agent(app: &mut AppState, fixture: &Fixture) -> (TabKey, SessionLog) {
    let posted = app.handle(req(
        "thread.post",
        json!({"entity_id":fixture.entity, "agent_id":fixture.removed, "body":"queued work"}),
    ));
    assert_eq!(posted["ok"], true, "{posted}");
    assert!(app
        .delivery_queue
        .queued()
        .any(|turn| turn.agent_id == fixture.removed));
    let root = AppState::canonical_root(&app.entity_agent_root(&fixture.entity).unwrap());
    let key = TabKey::agent(&root, &fixture.removed);
    let log = SessionLog::default();
    let tab = dictated_agent_tab(
        &root,
        &fixture.entity,
        &fixture.removed,
        DictatedSession::reporting(crate::harness::AgentStatus::Working).recording_into(&log),
    );
    app.session_registry.test_insert_tab(key.clone(), tab);
    app.test_install_mcp_token(fixture.removed.clone(), "identity-test-token".into());
    (key, log)
}

#[test]
fn failed_removal_retires_live_binding_without_reactivating_task_identity() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let fixture = seed(&mut app);
    let state = app.shared();
    let handler = AppState::handler(Arc::clone(&state));
    assert_routed_availability(&state, &fixture, true);
    let (tab, session) = queue_and_install_live_agent(&mut state.lock().unwrap(), &fixture);
    rusqlite::Connection::open(dir.path().join("store/build.db"))
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER refuse_roster_save BEFORE UPDATE ON implementations
             BEGIN SELECT RAISE(FAIL, 'roster save refused'); END;",
        )
        .unwrap();
    let removal = handler.call(
        SessionSender::detached("other-client"),
        req(
            "agent.remove",
            json!({"entity_id":fixture.entity, "agent_id":fixture.removed}),
        ),
    );
    assert_eq!(removal["ok"], false, "{removal}");
    assert!(session.ended());
    {
        let app = state.lock().unwrap();
        assert!(!app.session_registry.contains(&tab));
        assert!(app.session_registry.test_token(&fixture.removed).is_none());
        assert!(app
            .delivery_queue
            .queued()
            .all(|turn| turn.agent_id != fixture.removed));
        assert!(app
            .entity_agents(&fixture.entity)
            .unwrap()
            .by_id(&fixture.removed)
            .is_some());
    }
    assert_routed_availability(&state, &fixture, false);
    let changed = handler.call(
        SessionSender::detached("other-client"),
        req(
            "tasks.update",
            json!({"task_id":fixture.open, "title":"Unrelated task metadata"}),
        ),
    );
    assert_eq!(changed["ok"], true, "{changed}");
    assert_eq!(
        changed["result"]["task"]["identities"][&fixture.removed]["available"],
        false
    );
    assert_routed_availability(&state, &fixture, false);
    {
        let mut app = state.lock().unwrap();
        let fresh = terminal_free_agent_tab(&tab.root, &fixture.entity, &fixture.removed);
        app.session_registry.insert_opened(tab, fresh);
        app.publish_read_metadata();
    }
    assert_routed_availability(&state, &fixture, true);
}
