use super::*;
use crate::execution::session::SnapshotPool;

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

const MAX_SNAPSHOT_BYTES: usize = 12 * 1024 * 1024;
const TRICKLE_BYTES: usize = 16 * 1024;
const TRICKLE_READS: usize = 192;

#[tokio::test]
#[ignore = "paces two real TLS readers through the 192-second snapshot budget"]
async fn mounted_tls_trickling_readers_recover_both_snapshot_slots() {
    let server = paced_snapshot_server().await;
    let pool = server.state.execution.sessions().terminal_snapshot_pool();
    let (first_opening, first) = start_paced_snapshot(&server, "first").await;
    let first_id = opening_diagnostic_id(&server, &first_opening);
    let first_grant = observe_grant(&pool, first_id).await;
    let (second_opening, second) = start_paced_snapshot(&server, "second").await;
    let second_id = opening_diagnostic_id(&server, &second_opening);
    let second_grant = observe_grant(&pool, second_id).await;
    let started = Instant::now();
    let progress = Arc::new([AtomicUsize::new(0), AtomicUsize::new(0)]);
    let reading = tokio::spawn({
        let progress = progress.clone();
        async move {
            tokio::join!(
                trickle(first, &progress[0], "first", started),
                trickle(second, &progress[1], "second", started)
            )
        }
    });
    assert_eq!(pool.available_permits(), 0);
    let waiting = server.opening().await;
    let mut sibling = server.socket(&server.ticket(&waiting).await).await;
    let wait_started = Instant::now();
    send(
        &mut sibling,
        "waiting",
        "term.attach",
        json!({"term_id":"small"}),
    )
    .await;
    let refusal = object_with_timeout(&mut sibling, TIMEOUT + Duration::from_secs(3)).await;
    let released = pool.available_permits();
    let read_counts = progress
        .each_ref()
        .map(|count| count.load(Ordering::Relaxed));
    if refusal["error_code"] != "snapshot_busy" || released != 0 {
        // Keep both readers alive for the diagnostic run after an early release.
        let _ = timeout(Duration::from_secs(15), async {
            while pool.holds(first_id) || pool.holds(second_id) {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await;
    }
    assert!(
        read_counts.iter().all(|count| *count >= 8),
        "both TLS readers must consume 16 KiB per second into the first 10-second window: {read_counts:?}"
    );
    assert_eq!(refusal["id"], "waiting");
    assert_eq!(
        refusal["error_code"],
        "snapshot_busy",
        "the waiter completed after {:?} while both TLS clients were still reading: {read_counts:?}. Reply ok={}",
        wait_started.elapsed(),
        refusal["ok"]
    );
    assert!(wait_started.elapsed() >= TIMEOUT);
    assert_ping(&mut sibling).await;
    assert_eq!(
        released, 0,
        "both readers still make progress after the independent 10-second wait"
    );
    tokio::join!(
        wait_for_grant_release(&pool, first_id, first_grant),
        wait_for_grant_release(&pool, second_id, second_grant),
    );
    let (first, second) = tokio::time::timeout_at(started + Duration::from_secs(195), reading)
        .await
        .expect("both readers must finish their paced reads")
        .unwrap();
    for (read_count, _) in [&first, &second] {
        assert!(
            *read_count >= TRICKLE_READS,
            "reader lost the connection after only {read_count} trickles"
        );
    }
    assert_eq!(pool.available_permits(), 2);
    for opening in [first_opening, second_opening] {
        assert_eq!(
            server.issue(&opening).await.0,
            409,
            "timeout latches on its exact Opening"
        );
    }
    let next = call(
        &mut sibling,
        "fresh",
        "term.attach",
        json!({"term_id":"small"}),
    )
    .await;
    assert_eq!(next["ok"], true);
    drop((first, second));
    server.stop().await;
}

async fn observe_grant(pool: &SnapshotPool, opening: u64) -> Instant {
    timeout(TIMEOUT, async {
        while !pool.holds(opening) {
            tokio::task::yield_now().await;
        }
        Instant::now()
    })
    .await
    .expect("snapshot slot granted")
}

async fn wait_for_grant_release(pool: &SnapshotPool, opening: u64, granted: Instant) {
    // Allow timer wakeup and observation to run on the mounted test runtime.
    const SCHEDULING_TOLERANCE: Duration = Duration::from_millis(250);
    let budget = Duration::from_secs(192) + SCHEDULING_TOLERANCE;
    tokio::time::timeout_at(granted + budget, async {
        while pool.holds(opening) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("snapshot slot must return by its own observed grant deadline");
    let elapsed = granted.elapsed();
    assert!(
        elapsed <= budget,
        "Opening {opening} held a slot for {elapsed:?}"
    );
    eprintln!("Opening {opening} returned its snapshot slot after {elapsed:?}");
}

fn opening_diagnostic_id(server: &Server, opening: &Value) -> u64 {
    server.state.openings.lock().unwrap()[case_text(&opening["session_id"])]
        .session
        .opening()
        .diagnostic_id()
}

#[tokio::test]
#[ignore = "paces a 12 MiB snapshot at 100 KiB/s over mounted TLS"]
async fn mounted_tls_maximum_snapshot_completes_at_100_kib_per_second() {
    let server = paced_snapshot_server().await;
    let (_, socket) = start_paced_snapshot(&server, "paced").await;
    let (producer, consumer) = tokio::io::duplex(128 * 1024);
    let progress = Arc::new(AtomicUsize::new(0));
    let started = Instant::now();
    let paced = tokio::spawn(forward_paced(socket, producer, progress.clone(), started));
    let mut websocket = WebSocketStream::from_raw_socket(consumer, Role::Client, None).await;
    let message = timeout(Duration::from_secs(150), websocket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap_or_else(|error| {
            panic!(
                "12 MiB snapshot incomplete after {:?} and {} bytes at 100 KiB/s: {error}",
                started.elapsed(),
                progress.load(Ordering::Relaxed)
            )
        });
    assert!(started.elapsed() >= Duration::from_secs(120));
    assert!((MAX_SNAPSHOT_BYTES - 512..=MAX_SNAPSHOT_BYTES).contains(&message.len()));
    let reply: Value = serde_json::from_slice(&message.into_data()).unwrap();
    assert_eq!(reply["id"], "paced");
    assert_eq!(reply["ok"], true);
    assert_eq!(
        server
            .state
            .execution
            .sessions()
            .terminal_snapshot_pool()
            .available_permits(),
        2
    );
    paced.abort();
    server.stop().await;
}

async fn paced_snapshot_server() -> Server {
    let handler = greeting(FrameHandler::new(FrameClock::new(), |_, frame, _| {
        let term_id = case_text(&frame.payload["params"]["term_id"]);
        let snapshot = if term_id == "large" {
            "A".repeat(MAX_SNAPSHOT_BYTES - 512)
        } else {
            String::new()
        };
        success(
            &frame,
            json!({"term_id":term_id,"snapshot":snapshot,"cursor":0}),
        )
    }));
    Server::start_with(handler, Duration::from_secs(360)).await
}

async fn start_paced_snapshot(server: &Server, id: &str) -> (Value, ClientIo) {
    let opening = server.opening().await;
    let headers = server.handshake(&server.ticket(&opening).await);
    let (head, mut io) = server.upgrade(&headers).await;
    assert_eq!(status(&head), 101);
    let call = json!({"id":id,"method":"term.attach","params":{"term_id":"large"}});
    let mut frame = Frame::message(
        serde_json::to_vec(&call).unwrap(),
        OpCode::Data(Data::Binary),
        true,
    );
    frame.header_mut().mask = Some([0x21, 0x73, 0xf9, 0x44]);
    let mut encoded = Vec::new();
    frame.format(&mut encoded).unwrap();
    io.write_all(&encoded).await.unwrap();
    io.flush().await.unwrap();
    (opening, io)
}

async fn trickle(
    mut socket: ClientIo,
    progress: &AtomicUsize,
    name: &str,
    started: Instant,
) -> (usize, ClientIo) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut buf = vec![0; TRICKLE_BYTES];
    for reads in 0..TRICKLE_READS {
        interval.tick().await;
        if !matches!(
            timeout(TIMEOUT, socket.read_exact(&mut buf)).await,
            Ok(Ok(_))
        ) {
            return (reads, socket);
        }
        progress.store(reads + 1, Ordering::Relaxed);
        eprintln!(
            "{name} reader received {} bytes at {:?}",
            (reads + 1) * TRICKLE_BYTES,
            started.elapsed()
        );
    }
    // Hold the transport through the last deadline tick.
    interval.tick().await;
    (TRICKLE_READS, socket)
}

async fn forward_paced(
    mut socket: impl tokio::io::AsyncRead + Unpin,
    mut receiver: tokio::io::DuplexStream,
    progress: Arc<AtomicUsize>,
    started: Instant,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    let mut buf = vec![0; 100 * 1024];
    loop {
        interval.tick().await;
        let mut remaining = buf.len();
        while remaining != 0 {
            let bytes = match socket.read(&mut buf[..remaining]).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    eprintln!(
                        "paced reader stopped after {:?} and {} bytes: {error}",
                        started.elapsed(),
                        progress.load(Ordering::Relaxed)
                    );
                    return;
                }
            };
            if bytes == 0 || receiver.write_all(&buf[..bytes]).await.is_err() {
                return;
            }
            progress.fetch_add(bytes, Ordering::Relaxed);
            remaining -= bytes;
        }
    }
}

#[path = "paced_controls.rs"]
mod paced_controls;

async fn object_with_timeout(socket: &mut Client, bound: Duration) -> Value {
    let message = timeout(bound, socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_slice(&message.into_data()).unwrap()
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
