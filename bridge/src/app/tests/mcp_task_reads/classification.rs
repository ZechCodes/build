use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn valid_done_report_takes_precedence_over_a_task_read_request() {
    let fixture = Fixture::new();
    let held = fixture.held_turns().await;
    let (client, daemon) = tokio::net::UnixStream::pair().unwrap();
    let server = tokio::spawn(handle_done_stream(
        Arc::clone(&fixture.state),
        daemon,
        Arc::clone(&fixture.plane),
    ));
    let mut client = tokio::io::BufReader::new(client);
    let mut report = fixture.frame(fixture.get());
    report["report"] = json!({"status":"blocked","summary":"done remains a notification"});
    let mut invalid = fixture.frame(fixture.get());
    invalid["request"] = json!({"action":"not-a-real-action"});
    client
        .get_mut()
        .write_all(format!("{report}\n{invalid}\n").as_bytes())
        .await
        .unwrap();
    let mut line = String::new();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), client.read_line(&mut line))
            .await
            .is_err(),
        "a report frame was answered as a task read before a writer turn"
    );
    drop(held);
    tokio::time::timeout(WATCHDOG, client.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&line).unwrap(),
        json!({"ok":false,"error":"invalid MCP action request"}),
        "the report must emit no reply; the next frame supplies this response"
    );
    drop(client);
    server.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_and_wrapped_task_reads_remain_on_the_authenticated_writer_lane() {
    let fixture = Fixture::new();
    let wrapped = BridgeAction::TrackerWithWaitingOn {
        wrapped: Box::new(fixture.get()),
        waiting_on: Vec::new(),
    };
    let wrapped_expected = fixture.legacy(wrapped.clone());
    let mut malformed = fixture.frame(fixture.get());
    malformed["request"] = json!({"action":"tracker_get_task","task_id":12});
    for (frame, expected) in [
        (
            malformed,
            json!({"ok":false,"error":"invalid MCP action request"}),
        ),
        (fixture.frame(wrapped), wrapped_expected),
    ] {
        let held = fixture.held_turns().await;
        let mut answer = tokio::spawn(socket_reply(
            Arc::clone(&fixture.state),
            Arc::clone(&fixture.plane),
            frame,
        ));
        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut answer)
                .await
                .is_err(),
            "an unsupported frame bypassed the authenticated writer path"
        );
        drop(held);
        assert_eq!(answer.await.unwrap(), expected);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_read_router_and_unbound_owner_refusals_keep_their_existing_envelopes() {
    let fixture = Fixture::new();
    let router = {
        let mut app = fixture.state.lock().unwrap();
        let (_, router) = crate::app::tests::routing::router::captured(&mut app, "read refusal");
        app.delivery_queue.clear_queued();
        app.test_install_mcp_token(router.clone(), TOKEN.into());
        router
    };
    let held = fixture.held_turns().await;
    let mut frame = fixture.frame(fixture.get());
    frame["task_id"] = json!(router);
    assert_eq!(
        fixture.reply(frame).await,
        json!({"ok":false,"error":"get_task is a coding tool; this session routes captures"})
    );
    {
        let mut app = fixture.state.lock().unwrap();
        app.projects.unbind_entity(&fixture.owner);
        app.publish_read_metadata();
    }
    assert_eq!(
        fixture.reply(fixture.frame(fixture.get())).await,
        json!({"ok":false,"error":format!("{} belongs to no project",fixture.owner)})
    );
    assert_eq!(held.len(), 2);
}
