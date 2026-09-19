//! wss:// support for the bridge's outbound relay connection.
//!
//! The production relay lives behind TLS (`wss://relay.getbuild.ing/ws/device`),
//! so the relay client must speak TLS with rustls + webpki roots. These tests
//! prove it end-to-end against a local TLS WebSocket server with a self-signed
//! certificate injected as the trust root:
//!
//! - `wss_connects_to_tls_server_with_injected_root` runs the *real*
//!   `relay::run_with_connector` over a genuine TLS handshake and checks the
//!   signed device-auth headers arrive.
//! - `wss_scheme_is_supported_without_injected_connector` proves the default
//!   `relay::run` path (webpki roots) recognizes the `wss` scheme — the failure
//!   against an unreachable port is a network error, never tungstenite's
//!   "TLS support not compiled in".
//!
//! The mere existence of `tokio_tungstenite::Connector::Rustls` below is also a
//! compile-time assertion that the crate is built with a rustls TLS feature.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_rustls::rustls::pki_types::PrivatePkcs8KeyDer;
use tokio_rustls::rustls::{ClientConfig, RootCertStore, ServerConfig};
use tokio_rustls::TlsAcceptor;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::Connector;

use build_bridge::carrier::FrameHandler;
use build_bridge::reachability::Reachability;
use build_bridge::relay;
use common::{device_identity, test_intake};

mod common;

/// The provider the daemon installs at startup, installed the daemon's way:
/// a test that brought its own would pass while `serve` panicked on its first
/// wss:// connect, which is what happened once webrtc unified a second
/// provider into the build.
fn install_crypto_provider() {
    relay::install_crypto_provider();
}

/// A TLS WebSocket server that accepts one connection with a fresh self-signed
/// cert, greets the device as the relay would, waits for the first frame the
/// device sends back — its heartbeat — then closes. Returns the port, the cert
/// (DER) to trust, and a receiver that yields `(device_id_header,
/// signature_header_present)`.
async fn spawn_tls_ws_server() -> (
    u16,
    tokio_rustls::rustls::pki_types::CertificateDer<'static>,
    mpsc::Receiver<(String, bool)>,
) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let cert_der = cert.cert.der().clone();
    let key_der = PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der());

    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der.clone()], key_der.into())
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(server_config));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (seen_tx, seen_rx) = mpsc::channel(1);

    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.expect("device connects");
        let tls = acceptor.accept(tcp).await.expect("TLS handshake");

        // Capture the device-auth headers during the WebSocket upgrade.
        let mut device_id = String::new();
        let mut signed = false;
        // The Err size is tungstenite's `Callback` trait contract, not ours.
        #[allow(clippy::result_large_err)]
        let capture_auth_headers = |req: &Request, resp: Response| {
            device_id = req
                .headers()
                .get("X-Device-Id")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            signed = req.headers().contains_key("X-Signature")
                && req.headers().contains_key("X-Timestamp");
            Ok(resp)
        };
        let ws = tokio_tungstenite::accept_hdr_async(tls, capture_auth_headers)
            .await
            .expect("ws handshake over TLS");
        let (mut sink, mut source) = ws.split();

        sink.send(Message::Text(
            json!({"type": "authenticated", "heartbeat_interval_s": 30}).to_string(),
        ))
        .await
        .unwrap();

        // The device starts heartbeating at the advertised interval, the first beat
        // at once — that frame coming back over TLS is the round trip this test is
        // about. Then we hang up.
        while let Some(Ok(msg)) = source.next().await {
            if let Message::Text(text) = msg {
                let v: Value = serde_json::from_str(&text).unwrap();
                if v["type"] == "heartbeat" {
                    let _ = seen_tx.send((device_id.clone(), signed)).await;
                    break;
                }
            }
        }
        let _ = sink.send(Message::Close(None)).await;
    });

    (port, cert_der, seen_rx)
}

#[tokio::test]
async fn wss_connects_to_tls_server_with_injected_root() {
    install_crypto_provider();
    let (port, cert_der, mut seen_rx) = spawn_tls_ws_server().await;

    // Trust exactly the server's self-signed cert.
    let mut roots = RootCertStore::empty();
    roots.add(cert_der).unwrap();
    let client_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = Connector::Rustls(Arc::new(client_config));

    let identity = device_identity();
    let url = format!("wss://localhost:{port}/ws/device");
    let handler: FrameHandler = FrameHandler::new(
        build_bridge::timing::FrameClock::new(),
        |_sender, frame, _timer| json!({"echo": frame.payload}),
    );

    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        relay::run_with_connector(
            &url,
            &identity,
            test_intake(handler),
            Some(connector),
            &Reachability::unreachable(),
        ),
    )
    .await
    .expect("no timeout");
    outcome.expect("wss session runs to a clean close");

    let (device_id, signed) = seen_rx.recv().await.expect("server saw the device");
    assert_eq!(device_id, identity.device_id);
    assert!(signed, "auth headers rode the TLS upgrade");
}

#[tokio::test]
async fn wss_scheme_is_supported_without_injected_connector() {
    install_crypto_provider();
    // Bind-then-drop to get a port nothing listens on: the default `run` path
    // must get far enough to attempt the network, i.e. the wss scheme itself is
    // handled (with the TLS feature missing, tungstenite fails on the URL first).
    let unused_port = {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    };
    let identity = device_identity();
    let url = format!("wss://127.0.0.1:{unused_port}/ws/device");
    let handler: FrameHandler = FrameHandler::new(
        build_bridge::timing::FrameClock::new(),
        |_sender, frame, _timer| json!({"echo": frame.payload}),
    );

    let err = tokio::time::timeout(
        Duration::from_secs(10),
        relay::run(
            &url,
            &identity,
            test_intake(handler),
            &Reachability::unreachable(),
        ),
    )
    .await
    .expect("no timeout")
    .expect_err("nothing listens on the port");
    let message = err.to_string();
    assert!(
        !message.contains("support for TLS"),
        "wss must not be rejected as an unsupported scheme, got: {message}"
    );
}
