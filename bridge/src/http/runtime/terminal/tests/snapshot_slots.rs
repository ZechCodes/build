use super::*;

#[tokio::test]
async fn mounted_tls_snapshot_slots_wait_for_flush_or_drop_and_drain() {
    let screen = crate::screen::ScreenHandle::new("fast", 80, 24);
    let pool = Arc::new(OnceLock::new());
    let server = Server::start(snapshot_handler(screen.clone(), pool.clone())).await;
    let (observe, mut observed) = mpsc::unbounded_channel();
    *server.state.output_observer.lock().unwrap() = Some(Arc::new(move |authority| {
        let _ = observe.send(authority);
    }));
    let first = stall_snapshot(&server, &mut observed, "first").await;
    let second = stall_snapshot(&server, &mut observed, "second").await;
    let pool = pool.get().unwrap();
    assert_eq!(pool.available_permits(), 0);

    let opening = server.opening().await;
    let mut fast = server.socket(&server.ticket(&opening).await).await;
    drop(observed.recv().await.unwrap());
    send(
        &mut fast,
        "waiting",
        "term.attach",
        json!({"term_id":"fast"}),
    )
    .await;
    assert_ping(&mut fast).await;
    assert_eq!(
        screen.attached(),
        0,
        "a waiter holds a marker, not a capture"
    );
    // A flood lands while the attach waits, without a flush. The attach
    // covers it. The next tick must send only the post-attach tail.
    let flood = vec![b'f'; 128 * 1024 + 1];
    let baseline = flood.len();
    screen.feed(&flood);

    // Release one physical writer before the independent admission timeout.
    first.cut.notify_one();
    let reply = object(&mut fast).await;
    assert_eq!(reply["id"], "waiting");
    assert_eq!(reply["ok"], true);
    assert_eq!(
        reply["result"]["cursor"], baseline,
        "capture happens after waiting"
    );
    assert_eq!(pool.available_permits(), 1);

    screen.feed(b"latest");
    screen.flush();
    let tail = object(&mut fast).await;
    let cursor = baseline + b"latest".len();
    assert_eq!(tail["type"], "term.output");
    assert_eq!(tail["cursor"], cursor);
    assert_eq!(
        STANDARD.decode(case_text(&tail["data"])).unwrap(),
        b"latest"
    );
    let ack = call(
        &mut fast,
        "baseline",
        "term.ack",
        json!({"term_id":"fast","cursor":cursor}),
    )
    .await;
    assert_eq!(ack["ok"], true);
    let third = stall_snapshot(&server, &mut observed, "third").await;
    assert_eq!(pool.available_permits(), 0);
    screen.feed(&flood);
    screen.flush();
    screen.feed(b"coalesced");
    screen.flush();
    assert_ping(&mut fast).await;
    second.cut.notify_one();
    let reset = object(&mut fast).await;
    assert_eq!(reset["type"], "term.reset");
    assert_eq!(reset["cursor"], cursor + flood.len() + b"coalesced".len());
    assert_ping(&mut fast).await;
    assert_eq!(pool.available_permits(), 1);

    let fourth = stall_snapshot(&server, &mut observed, "fourth").await;
    assert_eq!(pool.available_permits(), 0);
    send(
        &mut fast,
        "drain-waiter",
        "term.attach",
        json!({"term_id":"fast"}),
    )
    .await;
    assert_ping(&mut fast).await;
    assert_eq!(screen.attached(), 1);
    server.stop().await;
    assert_eq!(pool.available_permits(), 2);
    assert_eq!(
        screen.attached(),
        1,
        "shutdown did not run the pending capture"
    );
    assert_closed(&mut fast, None).await;
    drop((first, second, third, fourth));
}

#[tokio::test]
async fn mounted_tls_snapshot_timeout_without_close_latches_until_explicit_retry() {
    let fast = crate::screen::ScreenHandle::new("fast", 80, 24);
    let pool = Arc::new(OnceLock::new());
    let server = Server::start_with(
        snapshot_handler(fast, pool.clone()),
        Duration::from_secs(360),
    )
    .await;
    let (observe, mut observed) = mpsc::unbounded_channel();
    *server.state.output_observer.lock().unwrap() = Some(Arc::new(move |authority| {
        let _ = observe.send(authority);
    }));
    let StalledSnapshot {
        mut socket,
        opening,
        cut: _stall_cut,
    } = stall_snapshot(&server, &mut observed, "lost-close").await;

    let grant_pool = pool.get().unwrap();
    assert_eq!(grant_pool.available_permits(), 1);
    // stall_snapshot has observed a real blocked TLS write and controls the
    // independent no-progress cut. The whole-message deadline wins mid-frame.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(193)).await;
    tokio::time::resume();
    let received = timeout(TIMEOUT, socket.next()).await.unwrap();
    assert_eq!(grant_pool.available_permits(), 2);
    assert!(
        matches!(
            received,
            Some(Err(Error::Protocol(
                tokio_tungstenite::tungstenite::error::ProtocolError::ResetWithoutClosingHandshake
            )))
        ) || matches!(received, Some(Err(Error::Io(ref error))) if error.kind() == std::io::ErrorKind::UnexpectedEof),
        "peer should see a transport loss without a Close frame: {received:?}"
    );

    let expected = &fixture()["ticket"]["snapshot_timeout"]["automatic_refusal"];
    assert_eq!(
        server.issue(&opening).await,
        (
            expected["status"].as_u64().unwrap(),
            expected["body"].clone()
        )
    );
    let mut retry_body = fixture()["ticket"]["snapshot_timeout"]["explicit_retry_body"].clone();
    retry_body["opening_generation"] = opening["opening_generation"].clone();
    let (status, issued) = server
        .json(
            "POST",
            terminal_tickets::PATH,
            "writer",
            case_text(&opening["session_id"]),
            retry_body,
        )
        .await;
    assert_eq!(status, 201, "{issued:?}");
    let mut restored = server.socket(case_text(&issued["ticket"])).await;
    let reply = call(
        &mut restored,
        "restored",
        "term.attach",
        json!({"term_id":"fast"}),
    )
    .await;
    assert_eq!(reply["ok"], true, "{reply:?}");
    assert_eq!(server.ticket_requests.load(Ordering::Relaxed), 2);
    assert_eq!(server.upgrades.load(Ordering::Relaxed), 2);
    server.stop().await;
}

struct StalledSnapshot {
    socket: Client,
    opening: Value,
    cut: Arc<Notify>,
}

async fn stall_snapshot(
    server: &Server,
    observed: &mut mpsc::UnboundedReceiver<OutputAuthority>,
    id: &str,
) -> StalledSnapshot {
    let cut = Arc::new(Notify::new());
    *server.state.stall_cut.lock().unwrap() = Some(cut.clone());
    let opening = server.opening().await;
    let mut socket = server
        .socket_with_reader(&server.ticket(&opening).await, true)
        .await;
    let authority = timeout(TIMEOUT, observed.recv()).await.unwrap().unwrap();
    send(&mut socket, id, "term.attach", json!({"term_id":"slow"})).await;
    timeout(TIMEOUT, async {
        while !authority.has_pending_write() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("physical TLS writer must actually stall");
    StalledSnapshot {
        socket,
        opening,
        cut,
    }
}

async fn assert_ping(socket: &mut Client) {
    send(socket, "ping", "ping", json!({})).await;
    let reply = object(socket).await;
    assert_eq!(reply["id"], "ping");
    assert_eq!(reply["ok"], true);
}

fn snapshot_handler(
    screen: crate::screen::ScreenHandle,
    pool: Arc<OnceLock<Arc<crate::execution::session::SnapshotPool>>>,
) -> FrameHandler {
    greeting(FrameHandler::new(
        FrameClock::new(),
        move |sender, frame, _| {
            let term = case_text(&frame.payload["params"]["term_id"]);
            let lane = sender
                .terminal_interest()
                .unwrap()
                .snapshots
                .as_ref()
                .unwrap();
            pool.get_or_init(|| lane.pool());
            if frame.payload["method"] == "term.ack" {
                screen.ack(
                    sender.terminal_client_id(),
                    frame.payload["params"]["cursor"].as_u64().unwrap(),
                );
                return success(&frame, json!({}));
            }
            let snapshot = if term == "fast" {
                let reading = screen.attach(&sender.for_terminal_screen(), None);
                json!({"term_id":term,"snapshot":reading.snapshot,"cursor":reading.cursor})
            } else {
                json!({"term_id":term,"snapshot":"A".repeat(8 * 1024 * 1024),"cursor":0})
            };
            success(&frame, snapshot)
        },
    ))
}
