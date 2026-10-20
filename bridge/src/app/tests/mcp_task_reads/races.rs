use super::*;

async fn paused_read(fixture: &Fixture) -> (OffLockGateHandle, tokio::task::JoinHandle<Value>) {
    let (gate, held) = OffLockGate::new();
    fixture.state.lock().unwrap().off_lock_gate = Some(gate);
    let reading = tokio::spawn(socket_reply(
        Arc::clone(&fixture.state),
        Arc::clone(&fixture.plane),
        fixture.frame(fixture.get()),
    ));
    tokio::time::timeout(WATCHDOG, async {
        while !held.has_pending_arrival() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("task SQL read never reached its admitted lock-free phase");
    // Acquiring this guard while the read is paused proves it retained no app guard.
    assert!(fixture.state.try_lock().is_ok());
    (held, reading)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_task_read_rechecks_revoked_token_after_query_admission() {
    let fixture = Fixture::new();
    let (held, reading) = paused_read(&fixture).await;
    fixture
        .state
        .lock()
        .unwrap()
        .session_registry
        .test_install_token(fixture.agent.clone(), "replacement-capability".into());
    held.release();
    assert_eq!(
        reading.await.unwrap(),
        json!({"ok":false,"error":"unauthorized MCP session"})
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_task_read_rechecks_generation_even_when_capability_is_reinstalled() {
    let fixture = Fixture::new();
    let generation = fixture
        .state
        .lock()
        .unwrap()
        .mcp_conversation_generation(&fixture.owner, &fixture.agent)
        .unwrap();
    let (held, reading) = paused_read(&fixture).await;
    let expected = {
        let mut app = fixture.state.lock().unwrap();
        let reply = app.handle(req(
            "conversation.reset",
            json!({"project_id":fixture.project,
            "entity_id":fixture.owner,"agent_id":fixture.agent,"conversation_id":fixture.agent,
            "expected_thread_id":generation.thread_id}),
        ));
        assert_eq!(reply["ok"], true, "{reply}");
        app.session_registry
            .test_install_token(fixture.agent.clone(), TOKEN.into());
        assert_eq!(
            crate::app::mcp::authenticated_mcp_owner(
                &fixture.frame(fixture.get()),
                &app.session_registry
            ),
            Some(fixture.agent.as_str())
        );
        generation
            .guard(&app)
            .expect_err("the captured generation must now be stale")
    };
    held.release();
    assert_eq!(reading.await.unwrap(), json!({"ok":false,"error":expected}));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_task_read_rechecks_project_binding_after_query_admission() {
    let fixture = Fixture::new();
    let (_other_home, other_repo) = init_repo();
    let other =
        super::super::project_agent::added_project(&mut fixture.state.lock().unwrap(), &other_repo);
    let (held, reading) = paused_read(&fixture).await;
    fixture
        .state
        .lock()
        .unwrap()
        .projects
        .bind_entity(fixture.owner.clone(), other);
    held.release();
    assert_eq!(
        reading.await.unwrap(),
        json!({"ok":false,"error":format!("unknown task_id: {}",fixture.task)})
    );
}
