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
    let first = stall_snapshot(&server, &mut observed, "first", false).await;
    let second = stall_snapshot(&server, &mut observed, "second", true).await;
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

    // The first socket uses the production ten-second BoundedIo timer. The
    // second uses the manual stall cut so the test can inspect recovery of
    // exactly one physical writer before draining the listener.
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(11)).await;
    tokio::time::resume();
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
    let third = stall_snapshot(&server, &mut observed, "third", true).await;
    assert_eq!(pool.available_permits(), 0);
    screen.feed(&flood);
    screen.flush();
    screen.feed(b"coalesced");
    screen.flush();
    assert_ping(&mut fast).await;
    second.cut.as_ref().unwrap().notify_one();
    let reset = object(&mut fast).await;
    assert_eq!(reset["type"], "term.reset");
    assert_eq!(reset["cursor"], cursor + flood.len() + b"coalesced".len());
    assert_ping(&mut fast).await;
    assert_eq!(pool.available_permits(), 1);

    let fourth = stall_snapshot(&server, &mut observed, "fourth", true).await;
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

struct StalledSnapshot {
    _socket: Client,
    cut: Option<Arc<Notify>>,
}

async fn stall_snapshot(
    server: &Server,
    observed: &mut mpsc::UnboundedReceiver<OutputAuthority>,
    id: &str,
    manual_stall: bool,
) -> StalledSnapshot {
    let cut = manual_stall.then(|| Arc::new(Notify::new()));
    *server.state.stall_cut.lock().unwrap() = cut.clone();
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
        _socket: socket,
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
    pool: Arc<OnceLock<Arc<tokio::sync::Semaphore>>>,
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
            pool.get_or_init(|| lane.bridge.clone());
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
