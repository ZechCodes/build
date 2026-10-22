//! Reservation policy differs for coding callers, roster targets and router history.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_real_reset_retirement_keeps_router_history_but_refuses_coding_and_roster() {
    let fixture = Fixture::new();
    let (owner, agent) = {
        let mut app = fixture.state.lock().unwrap();
        let ensured = app.handle(req(
            "workspace.ensure_conversation",
            json!({"workspace_id":fixture.workspace}),
        ));
        assert_eq!(ensured["ok"], true, "{ensured}");
        let owner = ensured["result"]["run_id"].as_str().unwrap().to_owned();
        let added = app.handle(req("agent.add", json!({"entity_id":owner})));
        assert_eq!(added["ok"], true, "{added}");
        let agent = added["result"]["agent"]["id"].as_str().unwrap().to_owned();
        let mut active = app.take_run(&owner).unwrap();
        let thread = &mut active.agents.by_id_mut(&agent).unwrap().thread;
        thread.post_user("retirement buried witness", None, "2026-10-20T12:00:00Z");
        for index in 0..260 {
            thread.post_user(
                format!("retirement newer {index}"),
                None,
                "2026-10-20T12:01:00Z",
            );
        }
        app.finish_run_mutation(owner.clone(), active).unwrap();
        let record = app
            .require_store()
            .unwrap()
            .load_all_runs()
            .unwrap()
            .into_iter()
            .find(|run| run.id == owner)
            .unwrap();
        let active = crate::orchestrator::ActiveRun::reattach(&record, ".build/plan.md".into());
        let thread = &active.agents.by_id(&agent).unwrap().thread;
        assert!(thread.items.len() < thread.total_item_count() as usize);
        assert!(!thread.items.iter().any(|item| serde_json::to_string(item)
            .unwrap()
            .contains("retirement buried witness")));
        app.runs.insert(owner.clone(), active);
        app.publish_read_owner(&owner, true);
        app.test_install_mcp_token(agent.clone(), TOKEN.into());
        app.delivery_queue.clear_queued();
        (owner, agent)
    };
    let (_, router) = snapshot_reads::router(&fixture);
    let action = BridgeAction::ReadConversation {
        entity_id: owner.clone(),
        agent_id: Some(agent.clone()),
        limit: 300,
    };
    let expected = fixture
        .reply(snapshot_reads::router_frame(&router, action.clone()))
        .await;
    assert_eq!(expected["ok"], true, "{expected}");
    assert!(expected["result"]["transcript"]
        .as_str()
        .unwrap()
        .contains("retirement buried witness"));
    let (authority, issuer, before) = {
        let app = fixture.state.lock().unwrap();
        let authority = app.session_registry.mcp_read_authority();
        let issuer = authority.load().authenticate_token(&agent, TOKEN).unwrap();
        let before = app
            .read_service()
            .mcp_addresses()
            .issued_address(&agent)
            .unwrap();
        (authority, issuer, before)
    };
    let params = json!({"project_id":fixture.project,"entity_id":owner,
        "agent_id":agent,"conversation_id":before.conversation,
        "expected_thread_id":before.thread_id});
    let (staged, job, reserved, retired) = {
        let mut app = fixture.state.lock().unwrap();
        app.delivery_queue.clear_queued();
        let (staged, job) = app.dispatch_deferring("conversation.reset", &params);
        let reserved = authority.load().is_resetting(&before.conversation);
        let retired = !app.session_registry.token_matches(&agent, TOKEN);
        // Use the retained pre-reset identity to isolate reservation refusal
        // from retirement's independent immediate token revocation.
        app.session_registry
            .install_mcp_token(agent.clone(), TOKEN.into(), issuer);
        (
            staged,
            job.expect("actual reset must defer after reserving and retiring"),
            reserved,
            retired,
        )
    };
    let turns = fixture.held_turns().await;
    // A shorter outer deadline can release the held turns and join every
    // socket/server before the socket helper's own watchdog expires.
    let mut readings = [
        tokio::spawn(socket_reply(
            fixture.state.clone(),
            fixture.plane.clone(),
            snapshot_reads::router_frame(&router, action),
        )),
        tokio::spawn(socket_reply(
            fixture.state.clone(),
            fixture.plane.clone(),
            fixture.frame(BridgeAction::ListWorkspaceAgents {
                workspace_id: fixture.workspace.clone(),
            }),
        )),
        tokio::spawn(socket_reply(
            fixture.state.clone(),
            fixture.plane.clone(),
            snapshot_reads::router_frame(
                &agent,
                BridgeAction::SearchConversation {
                    query: crate::thread::ConversationQuery {
                        text: Some("retirement".into()),
                        ..Default::default()
                    },
                },
            ),
        )),
    ];
    let mut replies = Vec::new();
    let completed_with_turns_held = tokio::time::timeout(Duration::from_secs(1), async {
        for reading in &mut readings {
            replies.push(reading.await);
        }
    })
    .await
    .is_ok();
    drop(turns);
    // A timeout leaves the current and later handles unconsumed. Release the
    // writer permits, join them, and only then run the reserved reset job.
    for reading in readings.iter_mut().skip(replies.len()) {
        replies.push(reading.await);
    }
    let done = tokio::task::spawn_blocking(move || job.run())
        .await
        .unwrap();
    let (settled, after) = {
        let mut app = fixture.state.lock().unwrap();
        let settled = app.apply_deferred("conversation.reset", &params, done);
        let after = app.read_service().mcp_addresses().issued_address(&agent);
        (settled, after)
    };
    let mut replies = replies.into_iter();
    let history = replies.next().unwrap();
    let roster = replies.next().unwrap();
    let coding = replies.next().unwrap();
    assert!(completed_with_turns_held, "reads waited for writer permits");
    assert!(staged.is_ok(), "{staged:?}");
    assert!(
        reserved,
        "actual reset must publish its runtime reservation"
    );
    assert!(retired, "actual retirement must revoke the target token");
    assert_eq!(
        history.unwrap(),
        expected,
        "router reads the complete old committed history while reset work is pending"
    );
    let busy = json!({"ok":false,"error":"conversation.reset: conversation is being cleared; try again shortly"});
    assert_eq!(
        roster.unwrap(),
        busy,
        "unaffected caller must refuse the resetting roster target"
    );
    assert_eq!(
        coding.unwrap(),
        busy,
        "same-token reinstall must still respect the coding reservation"
    );
    assert!(settled.is_ok(), "{settled:?}");
    assert!(!authority.load().is_resetting(&before.conversation));
    let after = after.unwrap();
    assert_ne!(after.thread_id, before.thread_id);
    assert!(after.generation > before.generation);
}
