//! OFFSOURCE app/tests/mcp_task_reads child; register only after root chooses TDD build split.
use super::*;
use crate::app::read_context::HarnessSettings;

fn addresses(app: &AppState) -> Arc<crate::app::read_context::McpAddresses> {
    app.read_service().mcp_addresses()
}

#[test]
fn mcp_saved_address_changes_only_after_successful_owner_save() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let owner = file_legacy_task(&mut app, "addressed owner");
    let canonical_owner = file_legacy_task(&mut app, "canonical owner");
    let agent = app.ensure_primary_agent(&owner).unwrap();
    let canonical = app.ensure_primary_agent(&canonical_owner).unwrap();
    app.initialize_read_context();
    let old = addresses(&app);
    let before = old.resolve(&owner, Some(&agent)).unwrap();
    let mut candidate = app.take_plan(&owner).unwrap();
    let selected = candidate.agents.by_id_mut(&agent).unwrap();
    selected.bind_conversation(&canonical);
    selected.name = Some("rejected name".into());
    app.store.as_ref().unwrap().fail_next_write();
    assert!(app.finish_plan_mutation(owner.clone(), candidate).is_err());
    app.publish_read_metadata_only();
    assert!(addresses(&app).resolve(&owner, Some(&agent)).unwrap() == before);
    assert_eq!(addresses(&app).display_name(&agent), None);
    let mut candidate = app.take_plan(&owner).unwrap();
    let selected = candidate.agents.by_id_mut(&agent).unwrap();
    selected.bind_conversation(&canonical);
    selected.name = Some("committed name".into());
    app.finish_plan_mutation(owner.clone(), candidate).unwrap();
    let current = addresses(&app);
    let accepted = current.resolve(&owner, Some(&agent)).unwrap();
    assert_eq!(accepted.issued.conversation, canonical);
    assert_eq!(accepted.issued.canonical_owner, canonical_owner);
    assert_eq!(current.display_name(&agent), Some("committed name"));
    assert!(current.validate(&before.issued).is_err());
    assert!(old.resolve(&owner, Some(&agent)).unwrap() == before);
    assert_eq!(old.display_name(&agent), None);
}

#[test]
fn mcp_failed_live_candidate_cannot_change_pin_binding_or_admit_addition() {
    let fixture = Fixture::new();
    let mut app = fixture.state.lock().unwrap();
    let old = addresses(&app);
    let before = old.resolve(&fixture.owner, Some(&fixture.agent)).unwrap();
    assert_eq!(
        old.surface(&fixture.owner, &fixture.agent).unwrap(),
        crate::mcp::McpSurface::Project
    );
    let mut candidate = app.take_run(&fixture.owner).unwrap();
    let choice = candidate
        .agents
        .by_id(&fixture.agent)
        .unwrap()
        .choice
        .clone();
    let selected = candidate.agents.by_id_mut(&fixture.agent).unwrap();
    selected.name = Some("not committed".into());
    selected.bind_conversation("missing-candidate-binding");
    selected.project_role = Some(crate::project_roster::ProjectRole::Worker);
    selected.thread.id = "not-a-committed-head".into();
    let added = candidate
        .agents
        .add(fixture.owner.as_str(), choice, "2026-10-20T00:00:00Z")
        .id
        .clone();
    app.store.as_ref().unwrap().fail_next_write();
    assert!(app.persist_run_record(&fixture.owner, &candidate).is_err());
    // Intentionally retain the failed live candidate, exercising the existing
    // removal-only publisher independently of canonical finisher rollback.
    app.runs.insert(fixture.owner.clone(), candidate);
    app.publish_read_owner(&fixture.owner, false);
    app.publish_read_metadata_only();
    app.publish_read_operations();
    let current = addresses(&app);
    assert!(
        current
            .resolve(&fixture.owner, Some(&fixture.agent))
            .unwrap()
            == before
    );
    assert_eq!(
        current.display_name(&fixture.agent),
        old.display_name(&fixture.agent)
    );
    assert_eq!(
        current.surface(&fixture.owner, &fixture.agent).unwrap(),
        crate::mcp::McpSurface::Project
    );
    assert!(current.issued_address(&added).is_err());
    app.runs
        .get_mut(&fixture.owner)
        .unwrap()
        .agents
        .remove(&fixture.agent)
        .unwrap();
    app.publish_read_owner(&fixture.owner, false);
    assert!(addresses(&app).issued_address(&fixture.agent).is_err());
    assert!(old.issued_address(&fixture.agent).is_ok());
}

#[test]
fn mcp_borrowed_peer_and_nested_save_keep_committed_address_until_settlement() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let owner = file_legacy_task(&mut app, "published owner");
    let peer = file_legacy_task(&mut app, "borrowed peer");
    let agent = app.ensure_primary_agent(&owner).unwrap();
    let peer_agent = app.ensure_primary_agent(&peer).unwrap();
    app.initialize_read_context();
    let before = addresses(&app);
    let peer_before = before.issued_address(&peer_agent).unwrap();
    let borrowed = app.take_plan(&peer).unwrap();
    let mut changed = app.take_plan(&owner).unwrap();
    changed.agents.by_id_mut(&agent).unwrap().name = Some("settled name".into());
    app.finish_plan_mutation(owner.clone(), changed).unwrap();
    app.publish_read_metadata_only();
    app.publish_read_operations();
    assert!(addresses(&app).issued_address(&peer_agent).unwrap() == peer_before);
    assert_eq!(
        addresses(&app).display_name(&agent),
        None,
        "inner saved notice is deferred by borrowed barrier"
    );
    app.finish_plan_mutation(peer.clone(), borrowed).unwrap();
    assert_eq!(addresses(&app).display_name(&agent), Some("settled name"));
    assert!(addresses(&app).issued_address(&peer_agent).unwrap() == peer_before);
    assert_eq!(before.display_name(&agent), None);
}

#[test]
fn mcp_reset_first_publication_updates_all_own_heads_in_one_metadata_root() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let (_, canonical_owner) = planned_run_in_review(&mut app, "canonical");
    let (_, alias_owner) = planned_run_in_review(&mut app, "alias");
    let (_, indirect_owner) = planned_run_in_review(&mut app, "one hop witness");
    let canonical = primary_agent_id(&app, &canonical_owner);
    let alias = primary_agent_id(&app, &alias_owner);
    let indirect = primary_agent_id(&app, &indirect_owner);
    app.edit_agent_record("retained alias own head", &alias_owner, &alias, |agent| {
        // Valid retained data after an earlier local reset, before binding.
        agent.thread.id = format!("thread:{}:retained-reset", agent.id);
        agent.thread.generation_revision = 3;
        agent.bind_conversation(&canonical);
    });
    app.edit_agent_record(
        "one-hop alias witness",
        &indirect_owner,
        &indirect,
        |agent| {
            agent.bind_conversation(&alias);
        },
    );
    app.delivery_queue.clear_queued();
    app.initialize_read_context();
    app.initialize_snapshot_projection();
    // Passive real producers: no background delivery/waiting services.
    let state = Arc::new(Mutex::new(app));
    let _handler = AppState::handler(state.clone());
    let mut app = state.lock().unwrap();
    let old = addresses(&app);
    let old_canonical = old.issued_address(&canonical).unwrap();
    let old_indirect = old.issued_address(&indirect).unwrap();
    let service = app.read_service();
    let runtime = app.session_registry.mcp_read_authority();
    let reset_id = canonical.clone();
    let seen = std::rc::Rc::new(std::cell::Cell::new(0));
    let count = seen.clone();
    let _observer = AppState::observe_reset_publications(move |_| {
        let current = service.mcp_addresses();
        // Whichever reset owner HashSet visits first, both own heads must
        // already have changed. One-hop indirect binding detects old alias
        // OWN head even if canonical happened to publish first.
        assert!(current.validate(&old_canonical).is_err());
        assert!(current.validate(&old_indirect).is_err());
        assert!(runtime.load().is_resetting(&reset_id));
        count.set(count.get() + 1);
    });
    let params = json!({
        "project_id":app.projects.project_id_of(&canonical_owner),
        "entity_id":canonical_owner,"agent_id":canonical,
        "conversation_id":canonical,"expected_thread_id":old.issued_address(&canonical).unwrap().thread_id
    });
    let reply = app.handle(req("conversation.reset", params));
    assert_eq!(reply["ok"], true, "{reply}");
    assert!(seen.get() >= 2);
    assert!(!app
        .session_registry
        .mcp_read_authority()
        .load()
        .is_resetting(&canonical));
}

#[test]
fn mcp_harness_settings_come_from_accepted_config_and_survive_failed_writes() {
    let fixture = Fixture::new();
    let mut app = fixture.state.lock().unwrap();
    // Fixture startup intentionally has no config path. Enable a real target
    // so the Write/Rename failures exercise persistence instead of a no-op.
    app.config_path = Some(fixture._root.path().join("metadata-settings.json"));
    let old = app.read_view.load();
    let before = HarnessSettings::published(&old).unwrap().unwrap();
    assert_eq!(before.default_harness, app.default_harness);
    assert_eq!(
        serde_json::to_value(&before.role_models).unwrap(),
        serde_json::to_value(&app.role_models).unwrap()
    );
    for step in [
        crate::app::ConfigPersistStep::Write,
        crate::app::ConfigPersistStep::Rename,
    ] {
        app.config_persist_failure = Some(step);
        assert!(app.settings_set(&json!({"default_harness":"pi"})).is_err());
        app.config_persist_failure = None;
        let current = HarnessSettings::published(&app.read_view.load())
            .unwrap()
            .unwrap();
        assert_eq!(current.default_harness, before.default_harness);
        assert_eq!(
            serde_json::to_value(current.role_models).unwrap(),
            serde_json::to_value(&before.role_models).unwrap()
        );
        assert!(Arc::ptr_eq(&old, &app.read_view.load()));
    }
    app.settings_set(&json!({"default_harness":"pi"})).unwrap();
    let current = HarnessSettings::published(&app.read_view.load())
        .unwrap()
        .unwrap();
    assert_eq!(current.default_harness, crate::models::AgentProvider::Pi);
    assert_eq!(
        HarnessSettings::published(&old)
            .unwrap()
            .unwrap()
            .default_harness,
        before.default_harness
    );
}
