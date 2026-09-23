//! An agent removed by another client changes issue destinations, even when
//! its workspace stays and neither agent is watched.

use super::*;
use crate::tracker::{Assignee, Issue};

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

fn comment_as(app: &mut AppState, entity: &str, agent: &str, issue: &str) {
    app.on_agent_mcp_action(
        entity,
        agent,
        crate::mcp::BridgeAction::TrackerCommentIssue {
            issue_id: issue.into(),
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
        "issues.create",
        json!({
            "project_id": project, "title": "Remote agent removal",
            "body": format!("Ask @agent:{removed} or @agent:{survivor}."),
        }),
    ));
    let open = created["result"]["issue"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    comment_as(app, &entity, &removed, &open);
    // Seed an existing assignment without starting a harness for this fixture.
    let store = app.tracker_store().unwrap();
    let mut issue = store.load_tracker_issue(&open).unwrap().unwrap();
    issue.assignee = Some(Assignee::Agent {
        agent_id: removed.clone(),
    });
    store.save_tracker_issue_activity(&issue, &[], &[]).unwrap();

    let closed = super::tracker::filed(app, &project, "Closed historical reference")["id"]
        .as_str()
        .unwrap()
        .to_string();
    comment_as(app, &entity, &removed, &closed);
    assert_eq!(
        app.handle(req("issues.close", json!({ "issue_id": closed })))["ok"],
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

fn read_issue(app: &AppState, id: &str) -> Issue {
    app.tracker_store()
        .unwrap()
        .load_tracker_issue(id)
        .unwrap()
        .unwrap()
}

fn removing_an_agent(fail_persistence: bool) -> Value {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path()).with_change_window(Duration::ZERO);
    let fixture = seed(&mut app);
    let before_saved = read_issue(&app, &fixture.open);
    let state = app.shared();
    let changes = state.lock().unwrap().changes();
    let handler = AppState::handler(Arc::clone(&state));
    let (reader, mut rx, key) = SessionSender::observable("issue-reader");
    let greeting = handler.call(
        reader.clone(),
        req("session.hello", json!({ "changes": "subscriptions" })),
    );
    let read = || {
        json!({
            "get": handler.call(reader.clone(), req("issues.get", json!({ "issue_id": fixture.open })))["result"],
            "list": handler.call(reader.clone(), req("issues.list", json!({ "project_id": fixture.project, "state": "open" })))["result"],
        })
    };
    let before = read();
    let subscribed = handler.call(reader.clone(), req("changes.subscribe", json!({
        "subscription_id": "issue-identities", "scope": { "kind": "entity", "id": fixture.project },
        "kinds": ["issues"],
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
        .flat_map(|item| item["issues"]["issue_ids"].as_array().into_iter().flatten())
        .map(|id| id.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        ids,
        std::collections::BTreeSet::from([fixture.open.clone(), fixture.closed.clone()]),
        "only issues naming the removed agent must refresh; unrelated {}: {events:?}",
        fixture.unrelated
    );
    let app = state.lock().unwrap();
    assert!(app.workspaces.get(&fixture.workspace).is_some());
    assert!(app
        .entity_agents(&fixture.entity)
        .unwrap()
        .by_id(&fixture.survivor)
        .is_some());
    let saved = read_issue(&app, &fixture.open);
    assert_eq!(saved.updated_at, before_saved.updated_at);
    let identities = &after["get"]["issue"]["identities"];
    assert_eq!(identities[&fixture.removed]["available"], false);
    assert_eq!(identities[&fixture.removed]["name"], "Historian");
    assert_eq!(identities[&fixture.removed]["provider"], "pi");
    assert_eq!(identities[&fixture.survivor]["available"], true);
    trace
}

/// The browser wiring runner sets BUILD_ISSUE_AGENT_REMOVAL_TRACE to consume
/// these real wire responses and encrypted/decrypted pushes in Chromium.
#[test]
fn remote_agent_removal_invalidates_issue_identities() {
    let trace = removing_an_agent(false);
    if let Ok(path) = std::env::var("BUILD_ISSUE_AGENT_REMOVAL_TRACE") {
        std::fs::write(path, serde_json::to_vec_pretty(&trace).unwrap()).unwrap();
    }
}

#[test]
fn removed_live_agent_invalidates_issues_even_if_saving_roster_fails() {
    removing_an_agent(true);
}
