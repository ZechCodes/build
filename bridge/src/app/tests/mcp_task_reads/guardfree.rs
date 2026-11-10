//! Actual control sockets must make progress without either AppState guard.
use super::*;

pub(super) mod held;

fn legacy_request(fixture: &Fixture, action: BridgeAction) -> held::Request {
    let expected = fixture.legacy(action.clone());
    assert_eq!(expected["ok"], true, "{}", action.tool_name());
    held::Request::new(fixture.frame(action), expected)
}

async fn successful_group(fixture: &Fixture, requests: Vec<held::Request>) {
    history_reviews::sentry(fixture);
    let before = fixture.state.lock().unwrap().read_view.load();
    held::group(fixture, requests, || {}).await;
    history_reviews::assert_no_drain(fixture, &before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_guardfree_sql_reads_complete_with_app_and_both_writer_turns_held() {
    let fixture = Fixture::new();
    let requests = [
        fixture.get(),
        BridgeAction::TrackerListTasks {
            state: None,
            status: None,
            label: None,
            limit: None,
            cursor: None,
        },
        BridgeAction::TrackerTaskDependents {
            task_id: fixture.task.clone(),
            limit: Some(1),
            cursor: None,
        },
    ]
    .into_iter()
    .map(|action| legacy_request(&fixture, action))
    .collect();
    successful_group(&fixture, requests).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_guardfree_catalog_rosters_complete_with_app_and_both_writer_turns_held() {
    let fixture = Fixture::new();
    let requests = [
        BridgeAction::ListHarnesses,
        BridgeAction::ListWorkspaceAgents {
            workspace_id: fixture.workspace.clone(),
        },
        BridgeAction::ListProjectAgents,
    ]
    .into_iter()
    .map(|action| legacy_request(&fixture, action))
    .collect();
    successful_group(&fixture, requests).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_guardfree_snapshot_lists_complete_with_app_and_both_writer_turns_held() {
    let fixture = Fixture::new();
    let (capture, router) = snapshot_reads::router(&fixture);
    let mut requests = Vec::new();
    for action in [BridgeAction::ListProjects, BridgeAction::ListWork] {
        let expected = fixture
            .state
            .lock()
            .unwrap()
            .router_action(&capture, action.clone())
            .unwrap();
        requests.push(held::Request::new(
            snapshot_reads::router_frame(&router, action),
            json!({"ok":true,"result":expected}),
        ));
    }
    fixture
        .state
        .lock()
        .unwrap()
        .publish_all_snapshot_projection();
    requests.push(legacy_request(&fixture, BridgeAction::ListWorkspaces));
    successful_group(&fixture, requests).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_guardfree_history_reads_complete_with_app_and_both_writer_turns_held() {
    let fixture = Fixture::new();
    history_reviews::history(&fixture);
    let comment = history_reviews::comment(&fixture);
    let (capture, router) = snapshot_reads::router(&fixture);
    let search = legacy_request(
        &fixture,
        BridgeAction::SearchConversation {
            query: crate::thread::ConversationQuery {
                text: Some("buried history witness".into()),
                ..Default::default()
            },
        },
    );
    assert_eq!(
        search.expected["result"]["hits"].as_array().unwrap().len(),
        1
    );
    let action = BridgeAction::ReadConversation {
        entity_id: fixture.owner.clone(),
        agent_id: Some(fixture.agent.clone()),
        limit: 300,
    };
    let expected = fixture
        .state
        .lock()
        .unwrap()
        .router_action(&capture, action.clone())
        .unwrap();
    assert!(expected["transcript"]
        .as_str()
        .unwrap()
        .contains("buried history witness"));
    let read = held::Request::new(
        snapshot_reads::router_frame(&router, action),
        json!({"ok":true,"result":expected}),
    );
    let comment = legacy_request(
        &fixture,
        BridgeAction::TrackerReadComment {
            comment_id: comment,
        },
    );
    fixture
        .state
        .lock()
        .unwrap()
        .publish_all_snapshot_projection();
    successful_group(&fixture, vec![search, read, comment]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_guardfree_review_reads_complete_with_app_and_both_writer_turns_held() {
    let fixture = Fixture::new();
    let mut requests = Vec::new();
    for action in history_reviews::review_actions(&fixture) {
        let expected = history_reviews::legacy_review(&fixture, action.clone()).await;
        assert_eq!(expected["ok"], true, "{}", action.tool_name());
        requests.push(held::Request::new(fixture.frame(action), expected));
    }
    successful_group(&fixture, requests).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_guardfree_final_validation_completes_while_app_is_held() {
    let mut fixture = Fixture::new();
    let expected = fixture.legacy(fixture.get());
    let frame = fixture.frame(fixture.get());
    let (gate, reading) = held::pause(&mut fixture, frame).await;
    held::finish(&fixture, gate, reading, expected).await;
}

async fn sql_capacity(fixture: &Fixture) -> Vec<crate::store::ReadConnection> {
    let pool = fixture
        .state
        .lock()
        .unwrap()
        .store
        .as_ref()
        .unwrap()
        .read_pool();
    let mut connections = Vec::new();
    for _ in 0..SQL_READERS {
        connections.push(pool.acquire().await);
    }
    connections
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_guardfree_auth_and_surface_refusals_precede_sql_and_app_wait() {
    let fixture = Fixture::new();
    let (_, router) = snapshot_reads::router(&fixture);
    let mut requests = Vec::new();
    for token in [Value::Null, json!("invalid-test-capability")] {
        let mut frame = fixture.frame(fixture.get());
        frame["session_token"] = token;
        requests.push(held::Request::new(
            frame,
            json!({"ok":false,"error":"unauthorized MCP session"}),
        ));
    }
    let router_frame = snapshot_reads::router_frame(&router, fixture.get());
    requests.push(held::Request::new(
        router_frame,
        json!({"ok":false,"error":"get_task is a coding tool; this session routes captures"}),
    ));
    let before = fixture.state.lock().unwrap().read_view.load();
    fixture
        .state
        .lock()
        .unwrap()
        .session_registry
        .revoke_mcp_token(&fixture.agent);
    requests.push(held::Request::new(
        fixture.frame(fixture.get()),
        json!({"ok":false,"error":"unauthorized MCP session"}),
    ));
    let connections = sql_capacity(&fixture).await;
    held::group(&fixture, requests, || drop(connections)).await;
    assert!(Arc::ptr_eq(
        &before,
        &fixture.state.lock().unwrap().read_view.load()
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_guardfree_final_revocation_rejects_without_app() {
    for missing in [false, true] {
        let mut fixture = Fixture::new();
        let action = if missing {
            BridgeAction::TrackerGetTask {
                task_id: "task-missing-final-auth".into(),
            }
        } else {
            fixture.get()
        };
        let frame = fixture.frame(action);
        let (gate, reading) = held::pause(&mut fixture, frame).await;
        let before = fixture.state.lock().unwrap().read_view.load();
        fixture
            .state
            .lock()
            .unwrap()
            .session_registry
            .revoke_mcp_token(&fixture.agent);
        held::finish(
            &fixture,
            gate,
            reading,
            json!({"ok":false,"error":"unauthorized MCP session"}),
        )
        .await;
        assert!(Arc::ptr_eq(
            &before,
            &fixture.state.lock().unwrap().read_view.load()
        ));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_guardfree_old_token_lease_cannot_revoke_replacement() {
    let fixture = Fixture::new();
    let expected = fixture.legacy(fixture.get());
    let before = fixture.state.lock().unwrap().read_view.load();
    let current = {
        let mut app = fixture.state.lock().unwrap();
        let issuer = app.live_mcp_issuer(&fixture.owner, &fixture.agent).unwrap();
        let old = app.session_registry.install_mcp_token(
            fixture.agent.clone(),
            TOKEN.into(),
            issuer.clone(),
        );
        let current = app.session_registry.install_mcp_token(
            fixture.agent.clone(),
            "replacement-test-capability".into(),
            issuer,
        );
        app.session_registry.revoke_mcp_token_if_current(&old);
        current
    };
    let mut replacement = fixture.frame(fixture.get());
    replacement["session_token"] = json!("replacement-test-capability");
    held::group(
        &fixture,
        vec![
            held::Request::new(replacement.clone(), expected),
            held::Request::new(
                fixture.frame(fixture.get()),
                json!({"ok":false,"error":"unauthorized MCP session"}),
            ),
        ],
        || {},
    )
    .await;
    fixture
        .state
        .lock()
        .unwrap()
        .session_registry
        .revoke_mcp_token_if_current(&current);
    held::group(
        &fixture,
        vec![held::Request::new(
            replacement,
            json!({"ok":false,"error":"unauthorized MCP session"}),
        )],
        || {},
    )
    .await;
    assert!(Arc::ptr_eq(
        &before,
        &fixture.state.lock().unwrap().read_view.load()
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_guardfree_update_refusal_precedes_auth_and_app_wait() {
    let mut fixture = Fixture::new();
    let updates = cancellation::update_service(
        fixture._root.path(),
        Arc::new(cancellation::Backend::default()),
    );
    fixture.state.lock().unwrap().updates = Some(updates.clone());
    held::recapture_plane(&mut fixture);
    updates.check().await.unwrap();
    updates
        .install(crate::update::InstallWhen::Idle, true)
        .await
        .unwrap();
    assert_eq!(
        updates.tick(false).await.unwrap().state,
        crate::update::UpdateState::Installing
    );
    assert!(updates.admission().try_enter().is_none());
    let mut unauthorized = fixture.frame(fixture.get());
    unauthorized["session_token"] = Value::Null;
    let requests = [fixture.frame(fixture.get()), unauthorized]
        .into_iter()
        .map(|frame| {
            held::Request::new(
                frame,
                json!({"ok":false,"error":"bridge update handoff in progress"}),
            )
        })
        .collect();
    let connections = sql_capacity(&fixture).await;
    held::group(&fixture, requests, || drop(connections)).await;
}

#[test]
fn mcp_guardfree_harness_catalogue_does_not_probe_executables_under_either_guard() {
    use crate::harness::installed::executable::probe_counts;
    let fixture = Fixture::new();
    let action = BridgeAction::ListHarnesses;
    let (expected, legacy_probes) = probe_counts::measure(|| fixture.legacy(action.clone()));
    assert_eq!(expected["ok"], true);
    assert!(
        legacy_probes.total() > 0,
        "legacy catalogue must witness real executable resolution"
    );
    let before = fixture.state.lock().unwrap().read_view.load();
    let (answer, guarded_probes) =
        crate::app::mcp::probe_guarded_mcp_executables(&fixture.state, &fixture.frame(action));
    assert_eq!(answer, expected);
    assert!(Arc::ptr_eq(
        &before,
        &fixture.state.lock().unwrap().read_view.load()
    ));
    assert_eq!(
        guarded_probes, 0,
        "MCP catalogue resolved executable metadata or canonical paths while holding AppState"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_guardfree_project_sources_complete_with_app_and_both_writer_turns_held() {
    let fixture = Fixture::new();
    successful_group(
        &fixture,
        vec![legacy_request(&fixture, BridgeAction::ListProjectSources)],
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_guardfree_project_sources_enforce_surface_token_and_bound_project() {
    let fixture = Fixture::new();
    let (_other_root, other_repo) = init_repo();
    let (foreign, worker) = {
        let mut app = fixture.state.lock().unwrap();
        let foreign = super::super::project_agent::added_project(&mut app, &other_repo);
        let worker =
            super::super::project_coordinator::add_worker(&mut app, &fixture.owner, json!({}));
        app.test_install_mcp_token(worker.clone(), TOKEN.into());
        (foreign, worker)
    };
    let (_, router) = snapshot_reads::router(&fixture);
    let expected = fixture.legacy(BridgeAction::ListProjectSources);
    let mut scoped = fixture.frame(BridgeAction::ListProjectSources);
    scoped["request"]["project_id"] = json!(foreign);
    scoped["request"]["environment_id"] = json!("foreign-environment");
    let mut worker_frame = fixture.frame(BridgeAction::ListProjectSources);
    worker_frame["task_id"] = json!(worker);
    let mut unknown = fixture.frame(BridgeAction::ListProjectSources);
    unknown["session_token"] = json!("wrong-source-token");
    held::group(&fixture, vec![
        held::Request::new(scoped, expected),
        held::Request::new(worker_frame, json!({"ok":false,"error":"list_project_sources is a project tool; this session is on the coding surface"})),
        held::Request::new(snapshot_reads::router_frame(&router, BridgeAction::ListProjectSources),
            json!({"ok":false,"error":"list_project_sources is a project tool; this session routes captures"})),
        held::Request::new(unknown, json!({"ok":false,"error":"unauthorized MCP session"})),
    ], || {}).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_guardfree_project_sources_follow_source_add_remove_commits() {
    let fixture = Fixture::new();
    let environment_path = fixture._root.path().join("environment-source");
    let project_path = fixture._root.path().join("project-source");
    std::fs::create_dir(&environment_path).unwrap();
    std::fs::create_dir(&project_path).unwrap();
    let environment = source_mutation(
        &fixture,
        BridgeAction::AddEnvironmentSource {
            path: Some(environment_path.display().to_string()),
            remote: None,
            name: None,
            base_branch: None,
        },
    );
    let env_source = environment["sources"][1]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let added = fixture.legacy(BridgeAction::ListProjectSources);
    assert_eq!(
        added["result"]["sources"][1],
        json!({"id":env_source,"path":environment_path,"scope":"environment"})
    );
    source_read_while_held(&fixture, added).await;

    let project = source_mutation(
        &fixture,
        BridgeAction::AddProjectSource {
            path: Some(project_path.display().to_string()),
            remote: None,
            name: None,
            base_branch: None,
        },
    );
    let project_source = project["sources"][2]["id"].as_str().unwrap().to_string();
    let added = fixture.legacy(BridgeAction::ListProjectSources);
    assert_eq!(
        added["result"]["sources"][2],
        json!({"id":project_source,"path":project_path,"scope":"project"})
    );
    source_read_while_held(&fixture, added).await;

    source_mutation(
        &fixture,
        BridgeAction::RemoveEnvironmentSource {
            source_id: env_source,
        },
    );
    let removed = fixture.legacy(BridgeAction::ListProjectSources);
    assert_eq!(removed["result"]["sources"].as_array().unwrap().len(), 2);
    assert_eq!(removed["result"]["sources"][1]["scope"], "project");
    source_read_while_held(&fixture, removed).await;

    source_mutation(
        &fixture,
        BridgeAction::RemoveProjectSource {
            source_id: project_source,
        },
    );
    let removed = fixture.legacy(BridgeAction::ListProjectSources);
    assert_eq!(removed["result"]["sources"].as_array().unwrap().len(), 1);
    source_read_while_held(&fixture, removed).await;
}

fn source_mutation(fixture: &Fixture, action: BridgeAction) -> Value {
    fixture
        .state
        .lock()
        .unwrap()
        .agent_action(&fixture.owner, &fixture.agent, action)
        .unwrap()
}

async fn source_read_while_held(fixture: &Fixture, expected: Value) {
    held::group(
        fixture,
        vec![held::Request::new(
            fixture.frame(BridgeAction::ListProjectSources),
            expected,
        )],
        || {},
    )
    .await;
}
