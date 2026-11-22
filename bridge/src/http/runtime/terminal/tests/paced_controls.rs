use super::*;

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
    use crate::http::listener::BoundedIo;

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    let writer = tokio::spawn(async move {
        let output = BoundedIo::new(server, OutputAuthority::default());
        let mut websocket = WebSocketStream::from_raw_socket(output, Role::Server, None).await;
        WriterFence::default()
            .write_message(&mut websocket, &vec![b'x'; MAX_SNAPSHOT_BYTES])
            .await
    });
    (client, writer)
}
