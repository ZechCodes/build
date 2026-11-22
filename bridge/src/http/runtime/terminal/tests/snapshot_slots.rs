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

struct StalledSnapshot {
    _socket: Client,
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

// Run with Node 22 and the SPA and desktop lockfile installs. This opt-in
// witness uses the real browser endpoint and the desktop's locked WS client.
#[tokio::test]
#[ignore = "runs the SPA pane test against the mounted TLS fixture"]
async fn mounted_tls_lost_close_stops_spa_ticket_requests() {
    let entered = Arc::new(Notify::new());
    let (release, parked) = std::sync::mpsc::channel();
    let handler = lost_close_handler(entered.clone(), parked);
    let server = Server::start_with(handler, Duration::from_secs(360)).await;
    let (observe, mut observed) = mpsc::unbounded_channel();
    *server.state.output_observer.lock().unwrap() = Some(Arc::new(move |authority| {
        let _ = observe.send(authority);
    }));

    let certificate = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(certificate.path(), &server.certificate).unwrap();
    let mut spa = start_spa_lost_close_test(&server, certificate.path());
    let result = tokio::select! {
        _ = entered.notified() => {
            // The attach was dispatched after the roster request and upgrade.
            // Refuse the physical Close write while the Opening stays valid.
            let output = std::iter::from_fn(|| observed.try_recv().ok()).last().unwrap();
            output.set(Some(Arc::new(|| false)));
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(193)).await;
            tokio::time::resume();
            let pool = server.state.execution.sessions().terminal_snapshot_pool();
            timeout(TIMEOUT, async {
                while pool.available_permits() != 2 {
                    tokio::task::yield_now().await;
                }
            }).await.unwrap();
            release.send(()).unwrap();
            spa.await
        }
        result = &mut spa => result,
    };
    drop(release);
    server.stop().await;
    let output = result.unwrap().unwrap();
    assert!(
        output.status.success(),
        "{}
{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn lost_close_handler(entered: Arc<Notify>, parked: std::sync::mpsc::Receiver<()>) -> FrameHandler {
    let parked = Mutex::new(Some(parked));
    greeting(FrameHandler::new(FrameClock::new(), move |_, frame, _| {
        let result = match case_text(&frame.payload["method"]) {
            "term.list" => json!({"terminals": [{"term_id": "term-1"}]}),
            "term.attach" => {
                if let Some(parked) = parked.lock().unwrap().take() {
                    entered.notify_one();
                    let _ = parked.recv();
                }
                json!({"term_id": "term-1", "snapshot": "b2s=", "cursor": 2, "cols": 80, "rows": 24})
            }
            _ => json!({}),
        };
        success(&frame, result)
    }))
}

fn start_spa_lost_close_test(
    server: &Server,
    certificate: &std::path::Path,
) -> JoinHandle<std::io::Result<std::process::Output>> {
    let origin = format!("https://localhost:{}", server.port);
    let certificate = certificate.to_owned();
    tokio::task::spawn_blocking(move || {
        std::process::Command::new("node")
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../spa"))
            .env("NODE_EXTRA_CA_CERTS", certificate)
            .env("TERMINAL_WSS_TEST_ORIGIN", origin)
            .args([
                "node_modules/vitest/vitest.mjs",
                "run",
                "test/terminalWebSocketPanes.test.js",
                "-t",
                "stops ticket requests against the mounted bridge",
            ])
            .output()
    })
}
