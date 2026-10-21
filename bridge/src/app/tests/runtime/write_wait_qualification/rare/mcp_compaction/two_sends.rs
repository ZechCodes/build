//! Two real native sends cannot both inherit one context callback.
use super::*;

async fn wait_for_native_sent(
    agent: &CompactingAgent,
    count: usize,
    context_before: Option<u64>,
) -> bool {
    let deadline = Instant::now() + WAIT_BUDGET;
    loop {
        let accepted = {
            let app = agent.state.lock().unwrap();
            app.compactions
                .sent_context_before_for_test(RUN, &agent.agent_id)
                == Some(context_before)
        };
        if agent.log.turns().len() == count && accepted {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::task::yield_now().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn logical_write_two_accepted_compaction_sends_are_not_both_completed_by_one_context() {
    let agent = CompactingAgent::without_background_services().with_context(200_000);
    let caller = {
        let mut app = agent.state.lock().unwrap();
        let project = app.projects.project_id_of(RUN).unwrap().to_owned();
        let caller = crate::app::tests::project_agent::project_agent(&mut app, &project).1;
        app.test_install_mcp_token(caller.clone(), TOKEN.into());
        caller
    };
    let fixture = RareFixture::shared("compaction-two-sends", Arc::clone(&agent.state));
    // Both commands use the real project caller/socket/admission/drain. The
    // session has reported Waiting and no newer status snapshot has arrived;
    // neither native policy nor accepted authority is edited between calls.
    let first = fixture
        .call_mcp(
            &caller,
            TOKEN,
            BridgeAction::CompactAgent {
                agent_id: agent.agent_id.clone(),
                instructions: "retain the first request evidence".into(),
            },
        )
        .await;
    let first_sent = wait_for_native_sent(&agent, 1, Some(200_000)).await;
    let first_retained = fixture.origin_active(&first).await;
    let waiting_between_sends = {
        let app = agent.state.lock().unwrap();
        app.entity_agents(RUN)
            .unwrap()
            .by_id(&agent.agent_id)
            .unwrap()
            .working_since
            .is_none()
    };
    let second = fixture
        .call_mcp(
            &caller,
            TOKEN,
            BridgeAction::CompactAgent {
                agent_id: agent.agent_id.clone(),
                instructions: "retain the second request evidence".into(),
            },
        )
        .await;
    // First send clears context. Some(None) proves the SECOND guarded native
    // ledger installation completed, after send_turn accepted it.
    let second_sent = wait_for_native_sent(&agent, 2, None).await;
    let second_retained = fixture.origin_active(&second).await;
    let before_callback = fixture.rpc.registry.snapshot();
    let retained = agent.state.lock().unwrap().read_view.load();
    let old_board = retained.get_owned("board.list", "");
    if first_sent && second_sent {
        let mut app = agent.state.lock().unwrap();
        app.record_agent_activity(
            RUN,
            &agent.agent_id,
            &AgentActivity::Compaction { completed: false },
            None,
        );
        app.record_agent_turn_context(
            RUN,
            &agent.agent_id,
            TurnContext {
                context_tokens: 17_000,
                cache_read_tokens: 0,
            },
        );
    }
    let callback_deadline = Instant::now() + WAIT_BUDGET;
    let after_callback = loop {
        let rows = fixture.rpc.registry.snapshot();
        let latest_terminal = rows
            .iter()
            .find(|row| row.original_id == second.original_id)
            .is_some_and(|row| {
                row.root_finished && row.outstanding.is_empty() && row.active.is_empty()
            });
        if latest_terminal || Instant::now() >= callback_deadline {
            break rows;
        }
        tokio::task::yield_now().await;
    };
    let published = {
        let mut app = agent.state.lock().unwrap();
        crate::app::tests::project_agent::items(&mut app, RUN, &agent.agent_id)
            .into_iter()
            .filter(|row| row["data"]["event"] == "compaction")
            .collect::<Vec<_>>()
    };
    let sends = agent.log.turns();
    // Real reset is cleanup, after both snapshots: it explicitly retires any
    // still-required attempt without synthesizing collector completion.
    let reset_params = {
        let app = agent.state.lock().unwrap();
        let thread = app.agent_conversation(RUN, Some(&agent.agent_id)).unwrap();
        json!({"project_id": app.projects.project_id_of(RUN).unwrap(),
            "entity_id": RUN, "agent_id": agent.agent_id,
            "conversation_id": agent.agent_id, "expected_thread_id":thread.id})
    };
    let reset = fixture.call("conversation.reset", reset_params).await;
    drop(fixture._watchdog);
    fixture.rpc.finish().await;
    // Complete real send/publication/cleanup positives precede the RED.
    assert!(first_sent && second_sent && waiting_between_sends);
    assert!(first_retained && second_retained);
    assert_eq!(sends, ["/compact", "/compact"]);
    assert_eq!(reset.reply["ok"], true);
    assert_eq!(published.len(), 1);
    assert_eq!(published[0]["data"]["compaction"]["context_after"], 17_000);
    assert!(old_board.is_some());
    assert_eq!(retained.get_owned("board.list", ""), old_board);
    let first_before = before_callback
        .iter()
        .find(|row| row.original_id == first.original_id)
        .unwrap();
    let latest_before = before_callback
        .iter()
        .find(|row| row.original_id == second.original_id)
        .unwrap();
    assert!(
        latest_before.root_finished
            && !latest_before.outstanding.is_empty()
            && !latest_before.cancelled
            && !latest_before.abandoned,
        "{latest_before:?}"
    );
    assert!(
        first_before.root_finished
            && !first_before.abandoned
            && (first_before.cancelled || !first_before.outstanding.is_empty()),
        "{first_before:?}"
    );
    let latest = after_callback
        .iter()
        .find(|row| row.original_id == second.original_id)
        .unwrap();
    assert!(
        latest.root_finished
            && latest.outstanding.is_empty()
            && latest.active.is_empty()
            && !latest.cancelled
            && !latest.abandoned,
        "the actual stamped latest request must finish successfully: {latest:?}"
    );
    let prior = after_callback
        .iter()
        .find(|row| row.original_id == first.original_id)
        .unwrap();
    // The native ledger retained only the latest receipt. The first attempt
    // may be explicitly superseded, or remain required until its own proof;
    // it cannot silently abandon or inherit success from the latest stamp.
    assert!(
        !prior.abandoned && (prior.cancelled || !prior.outstanding.is_empty()),
        "a single latest callback cannot certify the prior attempt: {prior:?}"
    );
}
