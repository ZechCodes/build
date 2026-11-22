use super::*;
use std::fmt::Write as _;

#[tokio::test]
async fn mounted_tls_slow_reader_preserves_snapshot_bytes_under_backpressure() {
    const SNAPSHOT_BYTES: usize = 6 * 1024 * 1024;
    let mut snapshot = String::with_capacity(SNAPSHOT_BYTES);
    for offset in (0..SNAPSHOT_BYTES).step_by(16) {
        write!(snapshot, "{offset:016x}").unwrap();
    }
    let result = json!({"term_id":"large","snapshot":snapshot,"cursor":0});
    let expected = serde_json::to_vec(&api::reply(json!("ordered"), Ok(result.clone()))).unwrap();
    let server = Server::start(greeting(FrameHandler::new(
        FrameClock::new(),
        move |_, frame, _| success(&frame, result.clone()),
    )))
    .await;
    let (observe, mut observed) = mpsc::unbounded_channel();
    *server.state.output_observer.lock().unwrap() = Some(Arc::new(move |authority| {
        let _ = observe.send(authority);
    }));
    let opening = server.opening().await;
    let mut socket = server.socket(&server.ticket(&opening).await).await;
    socket2::SockRef::from(socket.get_ref().get_ref().0)
        .set_recv_buffer_size(16 * 1024)
        .unwrap();
    let authority = timeout(TIMEOUT, observed.recv()).await.unwrap().unwrap();
    send(
        &mut socket,
        "ordered",
        "term.attach",
        json!({"term_id":"large"}),
    )
    .await;
    // Hold all reads until the mounted listener encounters a pending write.
    timeout(TIMEOUT, async {
        while !authority.has_pending_write() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the small receive buffer must back up the physical TLS writer");

    let (producer, consumer) = tokio::io::duplex(16 * 1024);
    let reader = tokio::spawn(forward_slow_reads(socket, producer));
    let mut websocket = WebSocketStream::from_raw_socket(consumer, Role::Client, None).await;
    let message = timeout(Duration::from_secs(30), websocket.next())
        .await
        .expect("the slow TLS reader must finish")
        .expect("snapshot WebSocket message")
        .unwrap();
    reader.abort();
    assert!(message.is_binary());
    let received = message.into_data();
    assert_eq!(received.len(), expected.len());
    let mismatch = received
        .iter()
        .zip(&expected)
        .position(|(left, right)| left != right);
    assert_eq!(mismatch, None, "first reordered or corrupted byte");
    eprintln!(
        "mounted TLS delivered all {} ordered bytes after backpressure",
        received.len()
    );
    server.stop().await;
}

async fn forward_slow_reads(mut socket: Client, mut receiver: tokio::io::DuplexStream) {
    let mut chunk = [0; 4 * 1024];
    loop {
        let len = socket.get_mut().read(&mut chunk).await.unwrap();
        if len == 0 || receiver.write_all(&chunk[..len]).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

#[tokio::test]
async fn mounted_tls_near_limit_snapshots_complete_at_full_speed() {
    let server = paced_snapshot_server().await;
    let (_, first) = start_paced_snapshot(&server, "first").await;
    let (_, second) = start_paced_snapshot(&server, "second").await;
    let mut first = WebSocketStream::from_raw_socket(first, Role::Client, None).await;
    let mut second = WebSocketStream::from_raw_socket(second, Role::Client, None).await;
    let started = Instant::now();
    let (first, second) = tokio::join!(read_complete(&mut first), read_complete(&mut second));
    assert!(first > MAX_SNAPSHOT_BYTES - 512);
    assert!(second > MAX_SNAPSHOT_BYTES - 512);
    eprintln!(
        "full-speed mounted TLS delivered {first} and {second} bytes in {:?}",
        started.elapsed()
    );
    server.stop().await;
}

async fn read_complete(io: &mut Client) -> usize {
    let message = next(io).await;
    assert!(message.is_binary());
    message.len()
}

#[tokio::test]
#[ignore = "plain loopback TCP control for the physical write with slow readers"]
async fn bare_tcp_trickle_observes_the_same_physical_writer() {
    let (mut first, a) = bare_writer().await;
    let (mut second, b) = bare_writer().await;
    let started = Instant::now();
    let readers = async {
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        let mut chunk = vec![0; TRICKLE_BYTES];
        for count in 1..=15 {
            interval.tick().await;
            for socket in [&mut first, &mut second] {
                socket.read_exact(&mut chunk).await.unwrap();
            }
            eprintln!(
                "both bare TCP readers consumed {} bytes at {:?}",
                count * TRICKLE_BYTES,
                started.elapsed()
            );
        }
    };
    timeout(Duration::from_secs(20), readers).await.unwrap();
    let finished = (a.is_finished(), b.is_finished());
    a.abort();
    b.abort();
    assert_eq!(
        finished,
        (false, false),
        "slow readers should keep both bare TCP writers in progress"
    );
}

#[tokio::test]
#[ignore = "plain loopback TCP control at 100 KiB/s"]
async fn bare_tcp_near_limit_snapshot_at_100_kib_per_second() {
    let (socket, writer) = bare_writer().await;
    let (producer, consumer) = tokio::io::duplex(128 * 1024);
    let progress = Arc::new(AtomicUsize::new(0));
    let started = Instant::now();
    let reader = tokio::spawn(forward_paced(socket, producer, progress.clone(), started));
    let mut websocket = WebSocketStream::from_raw_socket(consumer, Role::Client, None).await;
    let message = timeout(Duration::from_secs(150), websocket.next())
        .await
        .unwrap()
        .unwrap();
    let written = writer.await.unwrap();
    eprintln!(
        "bare TCP writer result {written:?}, {} bytes in {:?}",
        progress.load(Ordering::Relaxed),
        started.elapsed()
    );
    reader.abort();
    assert!(written.is_ok());
    assert_eq!(message.unwrap().len(), MAX_SNAPSHOT_BYTES);
}

async fn bare_writer() -> (
    TcpStream,
    JoinHandle<Result<(), super::super::super::writer::WriteFailed>>,
) {
    use crate::http::listener::{BoundedIo, TcpWriteProgress};

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    let writer = tokio::spawn(async move {
        let authority = OutputAuthority::default();
        authority.track_write_progress();
        let output = BoundedIo::new(TcpWriteProgress::new(server, authority.clone()), authority);
        let mut websocket = WebSocketStream::from_raw_socket(output, Role::Server, None).await;
        WriterFence::default()
            .write_message(&mut websocket, &vec![b'x'; MAX_SNAPSHOT_BYTES])
            .await
    });
    (client, writer)
}
