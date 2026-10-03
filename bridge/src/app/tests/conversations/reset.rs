use super::*;

fn reset_params(state: &AppState, owner: &str) -> Value {
    let agent = state.entity_agents(owner).unwrap().primary().unwrap();
    json!({
        "project_id": state.projects.project_id_of(owner).unwrap(),
        "entity_id": owner,
        "agent_id": agent.id,
        "conversation_id": agent.conversation_id(),
        "expected_thread_id": agent.thread.id,
    })
}

#[test]
fn conversation_reset_keeps_identity_and_settings_but_replaces_every_history_field() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "clear conversation");
    let id = primary_agent_id(&state, &owner);
    state.edit_agent_record("test", &owner, &id, |agent| {
        agent.name = Some("Useful name".into());
        agent.topic = Some("Old subject".into());
        agent.resume_session_id = Some("old-harness-history".into());
        agent.last_context_tokens = Some(1200);
        agent.session_cache_read_tokens = Some(300);
        agent.max_context_tokens = Some(150_000);
    });
    let old = state
        .entity_agents(&owner)
        .unwrap()
        .by_id(&id)
        .unwrap()
        .clone();
    let reply = state.handle(req("conversation.reset", reset_params(&state, &owner)));
    assert_eq!(reply["ok"], true, "{reply:?}");
    let agent = state.entity_agents(&owner).unwrap().by_id(&id).unwrap();
    assert_retained_identity_and_settings(agent, &old);
    assert_cleared_history_and_process_fields(agent, &old);
    assert_eq!(reply["result"]["thread"]["items"], json!([]));
    assert_eq!(reply["result"]["agent"]["thread_id"], agent.thread.id);
}

fn assert_retained_identity_and_settings(agent: &crate::agent::Agent, old: &crate::agent::Agent) {
    assert_eq!(agent.id, old.id);
    assert_eq!(agent.name, old.name);
    assert_eq!(agent.choice, old.choice);
    assert_eq!(agent.conversation_id(), old.conversation_id());
    assert_eq!(agent.max_context_tokens, Some(150_000));
    assert_eq!(agent.choice_revision, old.choice_revision + 1);
}

fn assert_cleared_history_and_process_fields(
    agent: &crate::agent::Agent,
    old: &crate::agent::Agent,
) {
    assert_ne!(agent.thread.id, old.thread.id);
    assert!(agent.thread.items.is_empty());
    assert!(agent.thread.sessions.is_empty());
    assert!(agent.thread.revisions.is_empty());
    assert_eq!(agent.thread.last_completion, None);
    assert_eq!(agent.topic, None);
    assert_eq!(agent.resume_session_id, None);
    assert_eq!(agent.last_context_tokens, None);
    assert_eq!(agent.session_cache_read_tokens, None);
}

#[test]
fn conversation_reset_rejects_a_stale_generation_and_old_posts() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "clear once");
    let params = reset_params(&state, &owner);
    let old_id = params["expected_thread_id"].clone();
    let reset = state.handle(req("conversation.reset", params.clone()));
    assert_eq!(reset["ok"], true, "{reset:?}");
    let stale = state.handle(req("conversation.reset", params));
    assert_eq!(stale["error_code"], "conflict", "{stale:?}");
    let stale_post = state.handle(req(
        "thread.post",
        json!({
            "entity_id": owner, "thread_id": old_id, "body": "old queued post"
        }),
    ));
    assert_eq!(stale_post["error_code"], "conflict", "{stale_post:?}");
    assert!(state
        .agent_conversation(&owner, None)
        .unwrap()
        .items
        .is_empty());
}

#[test]
fn conversation_reset_removes_persisted_history_and_operations_on_restart() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "durable clear");
    let posted = state.handle(req(
        "thread.post",
        json!({
            "entity_id": owner, "operation_id": "erase-this-operation", "body": "erase this payload"
        }),
    ));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let reset = state.handle(req("conversation.reset", reset_params(&state, &owner)));
    assert_eq!(reset["ok"], true, "{reset:?}");
    assert_eq!(
        state
            .require_store()
            .unwrap()
            .operation("erase-this-operation")
            .unwrap(),
        None
    );
    let restored = qa_state(&repo, dir.path());
    let thread = restored.agent_conversation(&owner, None).unwrap();
    assert!(thread.items.is_empty());
    assert_eq!(thread.id, reset["result"]["thread_id"]);
    assert_eq!(
        restored
            .require_store()
            .unwrap()
            .thread_item_count(&primary_agent_id(&restored, &owner))
            .unwrap(),
        0
    );
}

#[test]
fn conversation_reset_rejects_foreign_project_and_foreign_agent_without_mutation() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "address guard");
    let params = reset_params(&state, &owner);
    let original = state.agent_conversation(&owner, None).unwrap().clone();
    let mut foreign = params.clone();
    foreign["project_id"] = json!("another-project");
    let reply = state.handle(req("conversation.reset", foreign));
    assert_eq!(reply["error_code"], "invalid_params", "{reply:?}");
    let mut foreign = params;
    foreign["agent_id"] = json!("agent-other-project");
    let reply = state.handle(req("conversation.reset", foreign));
    assert_eq!(reply["ok"], false, "{reply:?}");
    assert_eq!(*state.agent_conversation(&owner, None).unwrap(), original);
}

#[test]
fn conversation_reset_can_change_harness_and_preserves_a_sibling() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "change harness");
    let added = state.handle(req(
        "agent.add",
        json!({ "entity_id": owner, "name": "Sibling" }),
    ));
    assert_eq!(added["ok"], true, "{added:?}");
    let sibling = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    let before = state
        .entity_agents(&owner)
        .unwrap()
        .by_id(&sibling)
        .unwrap()
        .clone();
    let mut params = reset_params(&state, &owner);
    params["provider"] = json!("codex_app_server");
    params["model"] = json!("gpt-5.4");
    params["effort"] = json!("high");
    params["max_context_tokens"] = json!(0);
    let reply = state.handle(req("conversation.reset", params));
    assert_eq!(reply["ok"], true, "{reply:?}");
    let agent = state.entity_agents(&owner).unwrap().primary().unwrap();
    assert_eq!(agent.choice.provider, AgentProvider::CodexAppServer);
    assert_eq!(agent.choice.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(agent.max_context_tokens, Some(0));
    assert_eq!(
        *state
            .entity_agents(&owner)
            .unwrap()
            .by_id(&sibling)
            .unwrap(),
        before
    );
}

#[test]
fn conversation_reset_deletes_draft_uploads_and_preserves_shared_sibling_bytes() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "draft attachments");
    let added = state.handle(req("agent.add", json!({ "entity_id": owner })));
    let sibling = added["result"]["agent"]["id"].clone();
    let upload = |state: &mut AppState, agent_id: Value, name: &str, content: &[u8]| {
        let reply = state.handle(req("thread.attach", json!({ "entity_id": owner, "agent_id": agent_id, "filename": name, "content_b64": b64encode(content) })));
        assert_eq!(reply["ok"], true, "{reply:?}");
        reply["result"].clone()
    };
    let primary = json!(primary_agent_id(&state, &owner));
    let exclusive = upload(
        &mut state,
        primary.clone(),
        "exclusive.txt",
        b"erase exclusive draft",
    );
    let shared = upload(&mut state, primary, "shared.txt", b"keep shared draft");
    upload(&mut state, sibling, "shared.txt", b"keep shared draft");
    let root = state.runs[&owner].worktree.path.clone();
    let reply = state.handle(req("conversation.reset", reset_params(&state, &owner)));
    assert_eq!(reply["ok"], true, "{reply:?}");
    assert!(!root.join(exclusive["path"].as_str().unwrap()).exists());
    assert!(root.join(shared["path"].as_str().unwrap()).exists());
    let leaf = std::path::Path::new(exclusive["path"].as_str().unwrap())
        .file_name()
        .unwrap();
    assert!(!state.local_attachments_dir().join(leaf).exists());
}

#[test]
fn conversation_reset_store_refusal_restores_files_history_and_pending_delivery() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "rollback clear");
    let upload = state.handle(req("thread.attach", json!({ "entity_id": owner, "filename": "draft.txt", "content_b64": b64encode(b"old draft") })));
    assert_eq!(upload["ok"], true, "{upload:?}");
    let old = state.agent_conversation(&owner, None).unwrap().clone();
    let queued = state.delivery_queue.queued_len();
    state.require_store().unwrap().fail_next_write();
    let reply = state.handle(req("conversation.reset", reset_params(&state, &owner)));
    assert_eq!(reply["ok"], false, "{reply:?}");
    assert_eq!(*state.agent_conversation(&owner, None).unwrap(), old);
    assert_eq!(state.delivery_queue.queued_len(), queued);
    assert!(state.runs[&owner]
        .worktree
        .path
        .join(upload["result"]["path"].as_str().unwrap())
        .exists());
}

#[test]
fn conversation_reset_refuses_in_flight_delivery_until_it_can_cancel_every_turn() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "delivery reservation");
    let ready = state.delivery_queue.take_ready(|_| false);
    let turn = ready.into_iter().find(|turn| turn.owner == owner).unwrap();
    let ticket = state.delivery_queue.start(&turn);
    let params = reset_params(&state, &owner);
    let reply = state.handle(req("conversation.reset", params.clone()));
    assert_eq!(reply["error_code"], "busy", "{reply:?}");
    assert_eq!(reply["retryable"], true);
    state.delivery_queue.settle(ticket);
    state.delivery_queue.enqueue(turn);
    let reply = state.handle(req("conversation.reset", params));
    assert_eq!(reply["ok"], true, "{reply:?}");
    assert!(!state
        .delivery_queue
        .queued()
        .any(|turn| turn.owner == owner));
}

#[test]
fn conversation_reset_preserves_assigned_and_tracked_task_and_clears_bound_aliases() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "shared conversation");
    let canonical = primary_agent_id(&state, &owner);
    let added = state.handle(req(
        "agent.add",
        json!({ "entity_id": owner, "name": "Alias" }),
    ));
    let alias = added["result"]["agent"]["id"].as_str().unwrap().to_string();
    state.edit_agent_record("test alias", &owner, &alias, |agent| {
        agent.bind_conversation(&canonical);
        agent.resume_session_id = Some("old-alias-session".into());
        agent.topic = Some("Old alias topic".into());
        agent.choice_revision = 100;
    });
    let project = state.projects.project_id_of(&owner).unwrap().to_string();
    let task = crate::app::tests::tracker::filed(&mut state, &project, "keep assignment");
    let task_id = task["id"].as_str().unwrap();
    let store = state.require_store().unwrap().clone();
    let mut task = store.load_tracker_task(task_id).unwrap().unwrap();
    task.assignee = Some(crate::tracker::Assignee::Agent {
        agent_id: canonical.clone(),
    });
    task.trackers = vec![canonical.clone()];
    task.links.conversation_ids = vec![owner.clone()];
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    let reply = state.handle(req("conversation.reset", reset_params(&state, &owner)));
    assert_eq!(reply["ok"], true, "{reply:?}");
    assert_eq!(store.load_tracker_task(&task.id).unwrap().unwrap(), task);
    let alias = state.entity_agents(&owner).unwrap().by_id(&alias).unwrap();
    assert_eq!(alias.name.as_deref(), Some("Alias"));
    assert_eq!(alias.conversation_id(), canonical);
    assert_eq!(alias.resume_session_id, None);
    assert_eq!(alias.topic, None);
    let tips = state.thread_tips(&owner, &[]);
    assert_eq!(tips[0].thread_generation_revision, Some(1));
    assert_eq!(tips[1].thread_generation_revision, Some(1));
    assert_eq!(tips[0].thread_id, tips[1].thread_id);
}

#[test]
fn conversation_reset_waits_with_lock_free_and_reserves_its_aliases() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "off lock reset");
    let upload = state.handle(req("thread.attach", json!({ "entity_id": owner, "filename": "read-during-reset.txt", "content_b64": b64encode(b"still readable") })));
    assert_eq!(upload["ok"], true, "{upload:?}");
    let posted = state.handle(req("thread.post", json!({ "entity_id": owner, "operation_id": "reset-readable-operation", "body": "read this while reset waits" })));
    assert_eq!(posted["ok"], true, "{posted:?}");
    let params = reset_params(&state, &owner);
    let agent_id = primary_agent_id(&state, &owner);
    let (gate, gate_handle) = OffLockGate::new();
    state.off_lock_gate = Some(gate);
    let state = state.shared();
    let reset = frame_on_a_thread(&state, "reset-client", "conversation.reset", params.clone());
    gate_handle.wait_for_arrival();
    let board = frame_on_a_thread(&state, "board-client", "board.list", json!({}))
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert_eq!(board["ok"], true, "{board:?}");
    assert_reset_reads_available(
        &state,
        &owner,
        &agent_id,
        &params["expected_thread_id"],
        &upload["result"]["path"],
    );
    let refused = frame_on_a_thread(
        &state,
        "watch-client",
        "conversation.unwatch",
        json!({ "entity_id": owner, "agent_id": agent_id }),
    )
    .recv_timeout(Duration::from_secs(10))
    .unwrap();
    assert_eq!(refused["error_code"], "busy", "{refused:?}");
    state
        .lock()
        .unwrap()
        .edit_agent_record("rename during reset", &owner, &agent_id, |agent| {
            agent.name = Some("Current name".into());
            agent.watched = false;
            agent.max_context_tokens = Some(130_000);
        });
    gate_handle.release();
    let reply = reset.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(reply["ok"], true, "{reply:?}");
    let locked = state.lock().unwrap();
    let agent = locked
        .entity_agents(&owner)
        .unwrap()
        .by_id(&agent_id)
        .unwrap();
    assert_eq!(agent.name.as_deref(), Some("Current name"));
    assert!(!agent.watched);
    assert_eq!(agent.max_context_tokens, Some(130_000));
    drop(locked);
    let stale_seen = state.lock().unwrap().handle(req("entity.seen", json!({ "entity_id": owner, "agent_id": agent_id, "thread_id": params["expected_thread_id"], "read_through_sequence": 999 })));
    assert_eq!(stale_seen["error_code"], "conflict", "{stale_seen:?}");
}

fn assert_reset_reads_available(
    state: &Arc<Mutex<AppState>>,
    owner: &str,
    agent_id: &str,
    thread_id: &Value,
    attachment: &Value,
) {
    let params = json!({ "entity_id": owner, "agent_id": agent_id, "thread_id": thread_id });
    let mut activity = params.clone();
    activity["from_sequence"] = json!(1);
    activity["through_sequence"] = json!(1);
    let mut operation = params.clone();
    operation["operation_id"] = json!("reset-readable-operation");
    let mut attachment_params = params.clone();
    attachment_params["path"] = attachment.clone();
    for (method, params) in [
        ("thread.page", params),
        ("thread.activity", activity),
        ("thread.operation", operation),
        ("thread.attachment", attachment_params),
    ] {
        let reply = frame_on_a_thread(state, method, method, params)
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert_eq!(reply["ok"], true, "{reply:?}");
    }
}

#[test]
fn conversation_reset_rechecks_dispatch_handoff_after_reserving() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "reservation handoff race");
    let address = state.resolve_conversation_address(&owner, None).unwrap();
    let agents = state
        .entity_agents(&owner)
        .unwrap()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let ready = state.delivery_queue.take_ready(|_| false);
    let turn = ready.into_iter().find(|turn| turn.owner == owner).unwrap();
    let old = state.conversation_at(&address).unwrap().clone();
    // Admission passed, but a previously drained turn claims execution before
    // the lifecycle closure reserves the conversation.
    state.refuse_reset_in_flight(&address, &agents).unwrap();
    let ticket = state.delivery_queue.start(&turn);
    let held = state.reserve_reset_turns(&address, &agents);
    assert!(held.is_err());
    assert!(!state
        .resetting_conversations
        .contains(&address.conversation_id));
    assert_eq!(*state.conversation_at(&address).unwrap(), old);
    state.delivery_queue.settle(ticket);
}

#[test]
fn conversation_reset_invalid_context_limits_are_client_errors() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "invalid reset settings");
    let old = state.agent_conversation(&owner, None).unwrap().clone();
    for invalid in [json!(-1), json!(1.5), json!("bad"), json!({})] {
        let mut params = reset_params(&state, &owner);
        params["max_context_tokens"] = invalid;
        let reply = state.handle(req("conversation.reset", params));
        assert_eq!(reply["error_code"], "invalid_params", "{reply:?}");
    }
    assert_eq!(*state.agent_conversation(&owner, None).unwrap(), old);
}

#[test]
fn conversation_reset_preserves_native_history_referenced_by_equivalent_surviving_cwd() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "shared provider history");
    let primary = primary_agent_id(&state, &owner);
    let sibling = state.handle(req("agent.add", json!({ "entity_id": owner })))["result"]["agent"]
        ["id"]
        .as_str()
        .unwrap()
        .to_string();
    let make_lineage = |agent_id: &str, cwd: &std::path::Path, provider: &str| {
        let mut thread = crate::thread::Thread::for_agent(agent_id);
        thread.start_agent_session(crate::thread::SessionStart {
            entity_id: &owner,
            agent_id,
            checkout: cwd.to_str().unwrap(),
            cwd: "",
            provider,
            model: None,
            effort: None,
            phase: "test",
            now: "2026-10-03T00:00:00Z",
        });
        let mut lineage = thread.sessions.remove(0);
        lineage.resume_session_id = Some("shared-native-id".into());
        lineage
    };
    let native = make_lineage(&primary, &repo, "codex");
    let surviving = make_lineage(&sibling, &repo.join("."), "codex_app_server");
    state.edit_agent_record("surviving native", &owner, &sibling, |agent| {
        agent.thread.sessions.push(surviving)
    });
    let address = state
        .resolve_conversation_address(&owner, Some(&primary))
        .unwrap();
    assert!(state.native_history_is_shared(&address, &native));
    state.edit_agent_record("unresolved historical cwd", &owner, &sibling, |agent| {
        agent.thread.sessions.last_mut().unwrap().cwd =
            Some(dir.path().join("vanished-checkout").display().to_string());
    });
    assert!(state.native_history_is_shared(&address, &native));
    state.edit_agent_record("current native", &owner, &sibling, |agent| {
        agent.thread.sessions.clear();
        agent.choose(ModelChoice {
            provider: AgentProvider::CodexAppServer,
            model: None,
            effort: None,
        });
        agent.resume_session_id = Some("shared-native-id".into());
    });
    assert!(state.native_history_is_shared(&address, &native));
}

#[test]
fn conversation_reset_refuses_native_history_that_becomes_shared_during_cleanup() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "late shared provider");
    let primary = primary_agent_id(&state, &owner);
    let sibling = state.handle(req("agent.add", json!({ "entity_id": owner })))["result"]["agent"]
        ["id"]
        .as_str()
        .unwrap()
        .to_string();
    state.edit_agent_record("known native", &owner, &primary, |agent| {
        agent
            .thread
            .start_agent_session(crate::thread::SessionStart {
                entity_id: &owner,
                agent_id: &primary,
                checkout: repo.to_str().unwrap(),
                cwd: "",
                provider: "codex",
                model: None,
                effort: None,
                phase: "test",
                now: "2026-10-03T00:00:00Z",
            });
        agent.thread.sessions.last_mut().unwrap().resume_session_id =
            Some("late-shared-native-id".into());
    });
    let params = reset_params(&state, &owner);
    let original = state.agent_conversation(&owner, None).unwrap().clone();
    let (gate, gate_handle) = OffLockGate::new();
    state.off_lock_gate = Some(gate);
    let state = state.shared();
    let reset = frame_on_a_thread(&state, "reset-client", "conversation.reset", params);
    gate_handle.wait_for_arrival();
    state
        .lock()
        .unwrap()
        .edit_agent_record("new native sharing", &owner, &sibling, |agent| {
            agent.choose(ModelChoice {
                provider: AgentProvider::CodexAppServer,
                model: None,
                effort: None,
            });
            agent.resume_session_id = Some("late-shared-native-id".into());
        });
    gate_handle.release();
    let reply = reset.recv_timeout(Duration::from_secs(30)).unwrap();
    assert_eq!(reply["error_code"], "busy", "{reply:?}");
    assert_eq!(
        *state
            .lock()
            .unwrap()
            .agent_conversation(&owner, None)
            .unwrap(),
        original
    );
}

#[cfg(unix)]
#[test]
fn conversation_reset_filesystem_refusal_keeps_history_and_never_follows_attachment_symlink() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "unsafe attachment");
    let upload = state.handle(req("thread.attach", json!({ "entity_id": owner, "filename": "draft.txt", "content_b64": b64encode(b"old draft") })));
    let path = state.runs[&owner]
        .worktree
        .path
        .join(upload["result"]["path"].as_str().unwrap());
    let other = dir.path().join("other-owned-file");
    std::fs::write(&other, b"retain these bytes").unwrap();
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(&other, &path).unwrap();
    let original = state.agent_conversation(&owner, None).unwrap().clone();
    let reply = state.handle(req("conversation.reset", reset_params(&state, &owner)));
    assert_eq!(reply["ok"], false, "{reply:?}");
    assert_eq!(*state.agent_conversation(&owner, None).unwrap(), original);
    assert_eq!(std::fs::read(other).unwrap(), b"retain these bytes");
}

#[test]
fn conversation_reset_requires_generation_for_delayed_legacy_mutations() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "unversioned stale payload");
    let reply = state.handle(req("conversation.reset", reset_params(&state, &owner)));
    assert_eq!(reply["ok"], true, "{reply:?}");
    let current = reply["result"]["thread_id"].clone();
    let post = state.handle(req(
        "thread.post",
        json!({ "entity_id": owner, "body": "delayed legacy words" }),
    ));
    assert_eq!(post["error_code"], "conflict", "{post:?}");
    let attach = state.handle(req("thread.attach", json!({ "entity_id": owner, "filename": "late.txt", "content_b64": b64encode(b"delayed legacy bytes") })));
    assert_eq!(attach["error_code"], "conflict", "{attach:?}");
    let state = state.shared();
    let handler = AppState::handler(Arc::clone(&state));
    let start = call(&handler, "agent.start", json!({ "id": owner }));
    assert_eq!(start["error_code"], "conflict", "{start:?}");
    let post = state.lock().unwrap().handle(req(
        "thread.post",
        json!({ "entity_id": owner, "thread_id": current, "body": "current words" }),
    ));
    assert_eq!(post["ok"], true, "{post:?}");
    assert!(state
        .lock()
        .unwrap()
        .agent_conversation(&owner, None)
        .unwrap()
        .items
        .iter()
        .all(|item| !serde_json::to_string(item)
            .unwrap()
            .contains("delayed legacy")));
}

#[test]
fn conversation_reset_refuses_a_reserved_spawn_and_a_drained_silent_start() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "starting process");
    let key = TabKey::agent(
        &state.entity_agent_root(&owner).unwrap(),
        &primary_agent_id(&state, &owner),
    );
    let claim = state.session_registry.take_spawn_claim(key);
    let params = reset_params(&state, &owner);
    let reply = state.handle(req("conversation.reset", params.clone()));
    assert_eq!(reply["error_code"], "busy", "{reply:?}");
    state.session_registry.settle_spawn_claim(claim);
    let mut ready = state.delivery_queue.take_ready(|_| false);
    let mut turn = ready.pop().unwrap();
    turn.say = None;
    let ticket = state.delivery_queue.start(&turn);
    let reply = state.handle(req("conversation.reset", params.clone()));
    assert_eq!(reply["error_code"], "busy", "{reply:?}");
    state.delivery_queue.settle(ticket);
    let reply = state.handle(req("conversation.reset", params));
    assert_eq!(reply["ok"], true, "{reply:?}");
}

#[test]
fn conversation_reset_rejects_an_existing_agent_from_another_project() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "actual project boundary");
    let other_repo = init_repo_named(dir.path(), "other");
    let other_project = crate::app::tests::project_agent::added_project(&mut state, &other_repo);
    let (other_owner, other_agent) =
        crate::app::tests::project_agent::project_agent(&mut state, &other_project);
    let original = state.agent_conversation(&owner, None).unwrap().clone();
    let other = state
        .agent_conversation(&other_owner, Some(&other_agent))
        .unwrap()
        .clone();
    let mut params = reset_params(&state, &owner);
    params["agent_id"] = json!(other_agent);
    let reply = state.handle(req("conversation.reset", params));
    assert_eq!(reply["ok"], false, "{reply:?}");
    assert_eq!(*state.agent_conversation(&owner, None).unwrap(), original);
    assert_eq!(
        *state
            .agent_conversation(&other_owner, Some(&other_agent))
            .unwrap(),
        other
    );
}

#[test]
fn conversation_reset_requires_current_generation_for_choice_watch_and_compaction_settings() {
    let (dir, repo) = init_repo();
    let mut state = qa_state(&repo, dir.path());
    let (_, owner) = planned_run_in_review(&mut state, "delayed configuration");
    let id = primary_agent_id(&state, &owner);
    let reset = state.handle(req("conversation.reset", reset_params(&state, &owner)));
    assert_eq!(reset["ok"], true, "{reset:?}");
    for (verb, mut params) in [
        (
            "agent.choose",
            json!({ "entity_id": owner, "agent_id": id, "effort": "high" }),
        ),
        (
            "conversation.watch",
            json!({ "entity_id": owner, "agent_id": id }),
        ),
        (
            "conversation.unwatch",
            json!({ "entity_id": owner, "agent_id": id }),
        ),
        (
            "conversation.settings",
            json!({ "entity_id": owner, "agent_id": id, "max_context_tokens": 130_000 }),
        ),
    ] {
        let refused = state.handle(req(verb, params.clone()));
        assert_eq!(refused["error_code"], "conflict", "{verb}: {refused:?}");
        params["thread_id"] = reset["result"]["thread_id"].clone();
        let accepted = state.handle(req(verb, params));
        assert_eq!(accepted["ok"], true, "{verb}: {accepted:?}");
    }
}
