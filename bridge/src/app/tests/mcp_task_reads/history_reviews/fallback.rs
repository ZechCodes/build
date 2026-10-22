//! A projection delayed across reset cannot supply old-generation fallback rows.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_history_router_message_failure_rejects_old_generation_accepted_history() {
    let fixture = Fixture::new();
    history(&fixture);
    let (router, old_thread) = {
        let mut app = fixture.state.lock().unwrap();
        let old = app
            .read_view
            .load()
            .history(&fixture.agent)
            .unwrap()
            .clone();
        let thread = app
            .agent_conversation(&fixture.owner, Some(&fixture.agent))
            .unwrap();
        let old_thread = thread.id.clone();
        let params = json!({"project_id":fixture.project,"entity_id":fixture.owner,
            "agent_id":fixture.agent,"conversation_id":fixture.agent,"expected_thread_id":thread.id});
        let reply = app.handle(req("conversation.reset", params));
        assert_eq!(reply["ok"], true, "{reply}");
        let mut active = app.take_run(&fixture.owner).unwrap();
        active
            .agents
            .by_id_mut(&fixture.agent)
            .unwrap()
            .thread
            .post_user("new generation message", None, "2026-10-20T12:05:00Z");
        app.finish_run_mutation(fixture.owner.clone(), active)
            .unwrap();
        assert_ne!(
            app.agent_conversation(&fixture.owner, Some(&fixture.agent))
                .unwrap()
                .id,
            old_thread
        );
        let (_, router) = crate::app::tests::routing::router::captured(
            &mut app,
            "new generation fallback router",
        );
        app.test_install_mcp_token(router.clone(), TOKEN.into());
        // Model a previously accepted root whose history replacement lagged
        // or was rejected, while canonical Store/live state already reset.
        let delta = crate::app::read_view::SnapshotDelta::new(app.read_view.next_version())
            .put_history(fixture.agent.clone(), old);
        assert!(app.read_view.publish_delta(delta));
        (router, old_thread)
    };
    let connection =
        rusqlite::Connection::open(fixture._root.path().join("store/build.db")).unwrap();
    assert_eq!(
        connection
            .execute(
                "UPDATE thread_items SET item = '{' WHERE agent_id = ?1",
                [&fixture.agent],
            )
            .unwrap(),
        1
    );
    drop(connection);
    sentry(&fixture);
    let before = fixture.state.lock().unwrap().read_view.load();
    let turns = fixture.held_turns().await;
    let action = BridgeAction::ReadConversation {
        entity_id: fixture.owner.clone(),
        agent_id: Some(fixture.agent.clone()),
        limit: 300,
    };
    let mut frame = fixture.frame(action);
    frame["task_id"] = json!(router);
    let answer = fixture.reply(frame).await;
    assert_eq!(
        answer["ok"], false,
        "old generation {old_thread} escaped: {answer}"
    );
    assert!(
        answer["error"]
            .as_str()
            .unwrap()
            .contains("conversation store:"),
        "{answer}"
    );
    assert_no_drain(&fixture, &before);
    assert_eq!(turns.len(), 2);
}
