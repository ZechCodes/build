//! Real producer paths must pair immediate capabilities with committed identities.
//! This draft registers after core guard-free RED; it is not run evidence.
use super::*;
use crate::app::runtime::sessions::McpIssuer;

fn issuer(fixture: &Fixture, agent: &str, token: &str) -> McpIssuer {
    fixture
        .state
        .lock()
        .unwrap()
        .session_registry
        .mcp_read_authority()
        .load()
        .authenticate_token(agent, token)
        .expect("fixture token is valid")
}

fn saved_issuer(fixture: &Fixture, agent: &str) -> Result<McpIssuer, String> {
    fixture
        .state
        .lock()
        .unwrap()
        .read_service()
        .mcp_addresses()
        .issued_address(agent)
        .map(McpIssuer::Coding)
}

#[tokio::test]
async fn issued_new_binding_cannot_authorize_old_metadata_or_failed_candidate() {
    let fixture = Fixture::new();
    let canonical = {
        let mut app = fixture.state.lock().unwrap();
        let mut owner = app.take_run(&fixture.owner).unwrap();
        let id = owner
            .agents
            .add(fixture.owner.as_str(), owner.model_choice.clone(), "now")
            .id
            .clone();
        app.finish_run_mutation(fixture.owner.clone(), owner)
            .unwrap();
        app.delivery_queue.clear_queued();
        id
    };
    let old = saved_issuer(&fixture, &fixture.agent).unwrap();
    {
        let mut app = fixture.state.lock().unwrap();
        let mut candidate = app.take_run(&fixture.owner).unwrap();
        candidate
            .agents
            .by_id_mut(&fixture.agent)
            .unwrap()
            .bind_conversation(canonical.clone());
        app.runs.insert(fixture.owner.clone(), candidate);
        // Capture the actual live reservation identity, not the stale saved slot.
        app.test_install_mcp_token(fixture.agent.clone(), TOKEN.into());
    }
    assert!(issuer(&fixture, &fixture.agent, TOKEN) != old);
    assert!(saved_issuer(&fixture, &fixture.agent).unwrap() == old);
    assert_eq!(
        fixture.reply(fixture.frame(fixture.get())).await,
        json!({"ok":false,"error":"unauthorized MCP session"})
    );
    {
        let mut app = fixture.state.lock().unwrap();
        let candidate = app.take_run(&fixture.owner).unwrap();
        app.store.as_ref().unwrap().fail_next_write();
        assert!(app
            .finish_run_mutation(fixture.owner.clone(), candidate)
            .is_err());
    }
    assert!(saved_issuer(&fixture, &fixture.agent).unwrap() == old);
    assert_eq!(
        fixture.reply(fixture.frame(fixture.get())).await,
        json!({"ok":false,"error":"unauthorized MCP session"})
    );
    {
        let mut app = fixture.state.lock().unwrap();
        let mut candidate = app.take_run(&fixture.owner).unwrap();
        candidate
            .agents
            .by_id_mut(&fixture.agent)
            .unwrap()
            .bind_conversation(canonical);
        app.finish_run_mutation(fixture.owner.clone(), candidate)
            .unwrap();
    }
    assert!(
        saved_issuer(&fixture, &fixture.agent).unwrap() == issuer(&fixture, &fixture.agent, TOKEN)
    );
    assert_eq!(
        fixture.reply(fixture.frame(fixture.get())).await["ok"],
        true
    );
}

#[tokio::test]
async fn token_issued_for_an_unsaved_new_agent_waits_for_its_actual_accepted_save() {
    let fixture = Fixture::new();
    let added = {
        let mut app = fixture.state.lock().unwrap();
        let mut candidate = app.take_run(&fixture.owner).unwrap();
        let added = candidate
            .agents
            .add(
                fixture.owner.as_str(),
                candidate.model_choice.clone(),
                "now",
            )
            .clone();
        app.runs.insert(fixture.owner.clone(), candidate);
        app.test_install_mcp_token(added.id.clone(), "new-address-token".into());
        added
    };
    let frame =
        json!({"task_id":added.id,"session_token":"new-address-token","request":fixture.get()});
    assert!(saved_issuer(&fixture, &added.id).is_err());
    assert_eq!(
        fixture.reply(frame.clone()).await,
        json!({"ok":false,"error":"unauthorized MCP session"})
    );
    {
        let mut app = fixture.state.lock().unwrap();
        let candidate = app.take_run(&fixture.owner).unwrap();
        app.store.as_ref().unwrap().fail_next_write();
        assert!(app
            .finish_run_mutation(fixture.owner.clone(), candidate)
            .is_err());
    }
    assert!(saved_issuer(&fixture, &added.id).is_err());
    assert_eq!(
        fixture.reply(frame.clone()).await,
        json!({"ok":false,"error":"unauthorized MCP session"})
    );
    {
        let mut app = fixture.state.lock().unwrap();
        let mut candidate = app.take_run(&fixture.owner).unwrap();
        let mut agents = candidate.agents.agents().to_vec();
        agents.push(added.clone());
        candidate.agents = crate::agent::AgentRoster::restore(
            &fixture.owner,
            agents,
            crate::thread::Thread::default(),
            candidate.model_choice.clone(),
            "now",
        );
        app.finish_run_mutation(fixture.owner.clone(), candidate)
            .unwrap();
    }
    assert!(
        saved_issuer(&fixture, &added.id).unwrap()
            == issuer(&fixture, &added.id, "new-address-token")
    );
    assert_eq!(fixture.reply(frame).await["ok"], true);
}

#[test]
fn real_router_creation_and_abandonment_update_preexisting_authority_handle() {
    let fixture = Fixture::new();
    let authority = fixture
        .state
        .lock()
        .unwrap()
        .session_registry
        .mcp_read_authority();
    let (capture, agent) = {
        let mut app = fixture.state.lock().unwrap();
        let pair = super::super::routing::router::captured(&mut app, "immediate router authority");
        app.test_install_mcp_token(pair.1.clone(), TOKEN.into());
        pair
    };
    assert!(matches!(
        authority.load().authenticate_token(&agent, TOKEN),
        Some(McpIssuer::Router(_))
    ));
    let root = fixture.state.lock().unwrap().read_view.load();
    {
        let mut app = fixture.state.lock().unwrap();
        app.abandon_router_session(&capture);
        assert!(
            Arc::ptr_eq(&root, &app.read_view.load()),
            "capability lifecycle must not need a board publication"
        );
    }
    assert!(authority.load().authenticate_token(&agent, TOKEN).is_none());
}

#[test]
fn runtime_reset_overlay_outlives_the_atomic_committed_reset_metadata() {
    let fixture = Fixture::new();
    let authority = fixture
        .state
        .lock()
        .unwrap()
        .session_registry
        .mcp_read_authority();
    let conversation = fixture
        .state
        .lock()
        .unwrap()
        .resolve_conversation_address(&fixture.owner, Some(&fixture.agent))
        .unwrap()
        .conversation_id;
    let observed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = Arc::clone(&observed);
    let check = authority.clone();
    let target = conversation.clone();
    let _observer = AppState::observe_reset_publications(move |_| {
        assert!(
            check.load().is_resetting(&target),
            "overlay cleared before committed reset publication"
        );
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    let reply = {
        let mut app = fixture.state.lock().unwrap();
        let generation = app
            .mcp_conversation_generation(&fixture.owner, &fixture.agent)
            .unwrap();
        app.handle(req(
            "conversation.reset",
            json!({"project_id":fixture.project,
            "entity_id":fixture.owner,"agent_id":fixture.agent,"conversation_id":conversation,
            "expected_thread_id":generation.thread_id}),
        ))
    };
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(
        observed.load(std::sync::atomic::Ordering::SeqCst) > 0,
        "actual committed reset observer did not run"
    );
    assert!(!authority.load().is_resetting(&conversation));
    assert!(authority
        .load()
        .authenticate_token(&fixture.agent, TOKEN)
        .is_none());
}

#[test]
fn refused_reset_attempt_cannot_clear_the_existing_operations_runtime_reservation() {
    let fixture = Fixture::new();
    let mut app = fixture.state.lock().unwrap();
    let authority = app.session_registry.mcp_read_authority();
    let address = app
        .resolve_conversation_address(&fixture.owner, Some(&fixture.agent))
        .unwrap();
    let agents = app.entity_agents(&fixture.owner).unwrap().agents().to_vec();
    let _held = app.reserve_reset_turns(&address, &agents).unwrap();
    assert!(authority.load().is_resetting(&address.conversation_id));
    let refused = app
        .reserve_reset_turns(&address, &agents)
        .err()
        .expect("second reservation must refuse");
    assert_eq!(
        refused,
        "conversation.reset: conversation is being cleared; try again shortly"
    );
    assert!(authority.load().is_resetting(&address.conversation_id));
    // This isolated producer control deliberately owns the first reservation.
    app.resetting_conversations.remove(&address.conversation_id);
    app.session_registry
        .set_mcp_resetting(&address.conversation_id, false);
}
