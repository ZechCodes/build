//! The relay answers plain-HTTP `GET /health` probes on its WebSocket port so
//! Kubernetes liveness/readiness checks work without a WS client. The probe
//! handler peeks (never consumes), so real WebSocket upgrades pass through
//! untouched.

use build_bridge::relay_server;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

/// Serve one connection the way `bin/relay.rs` does: health probe first, then WS.
async fn serve_one(listener: TcpListener) {
    let (mut tcp, _) = listener.accept().await.expect("accept");
    if relay_server::handle_health_probe(&mut tcp)
        .await
        .expect("probe check")
    {
        return; // probe answered and connection dropped
    }
    let ws = tokio_tungstenite::accept_async(tcp).await.expect("ws");
    let (mut sink, mut source) = ws.split();
    if let Some(Ok(Message::Text(text))) = source.next().await {
        sink.send(Message::Text(format!("echo:{text}")))
            .await
            .expect("echo");
    }
}

#[tokio::test]
async fn get_health_returns_http_200() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(serve_one(listener));

    let mut tcp = TcpStream::connect(addr).await.unwrap();
    tcp.write_all(b"GET /health HTTP/1.1\r\nHost: relay\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    tcp.read_to_string(&mut response).await.unwrap();

    assert!(
        response.starts_with("HTTP/1.1 200"),
        "expected 200, got: {response}"
    );
    assert!(response.contains("ok"), "expected body 'ok': {response}");
    drop(tcp); // EOF ends the server's drain immediately
    server.await.unwrap();
}

#[tokio::test]
async fn websocket_upgrade_still_works_after_probe_check() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(serve_one(listener));

    // Loopback test socket; TLS terminates at the edge in production. nosemgrep
    let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws/client")) // nosemgrep
        .await
        .expect("ws handshake must survive the probe peek");
    let (mut sink, mut source) = ws.split();
    sink.send(Message::Text("ping".into())).await.unwrap();
    let reply = source.next().await.expect("reply").expect("frame");
    assert_eq!(reply, Message::Text("echo:ping".into()));
    server.await.unwrap();
}
