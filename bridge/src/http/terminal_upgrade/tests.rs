use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use axum::{
    extract::{Request, State},
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
    routing::any,
    Extension, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{watch, Notify, Semaphore},
    task::JoinHandle,
    time::timeout,
};
use tokio_rustls::{client::TlsStream, TlsConnector};
use tokio_tungstenite::{
    tungstenite::{
        error::{CapacityError, ProtocolError},
        protocol::{
            frame::{
                coding::{Data, OpCode},
                Frame,
            },
            Role,
        },
        Error, Message,
    },
    WebSocketStream,
};

use super::{Handshake, TerminalSocket, TerminalUpgrade, UpgradeRefusal, PATH, PROTOCOL};
use crate::http::{
    limits::{self, TerminalSocketBudget},
    listener::{serve_connections, OutputAuthority},
    policy::HttpPolicy,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(10);
type ClientIo = TlsStream<TcpStream>;
type ClientSocket = WebSocketStream<ClientIo>;

fn contract() -> Value {
    serde_json::from_str(include_str!(
        "../../../../fixtures/api/http/terminal-websocket-v1.json"
    ))
    .unwrap()
}

fn number(value: &Value) -> usize {
    usize::try_from(value.as_u64().unwrap()).unwrap()
}

#[derive(Default)]
enum Mode {
    #[default]
    Echo,
    BlockUntilStop,
    FailHandshakeWrite,
    HoldBeforeUpgrade,
    StallWriter,
}

#[derive(Default)]
struct Witness {
    mode: Mode,
    reserved: Notify,
    started: AtomicUsize,
    ended: AtomicUsize,
    oversized: Notify,
    unmasked: Notify,
    stall_authority: Mutex<Option<OutputAuthority>>,
    writing: Notify,
    cut_stall: Arc<Notify>,
}

struct SessionGuard(Arc<Witness>);

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.0.ended.fetch_add(1, Ordering::Relaxed);
    }
}

async fn test_admission(
    State(witness): State<Arc<Witness>>,
    Extension(upgrade): Extension<TerminalUpgrade>,
    Extension(authority): Extension<OutputAuthority>,
    mut request: Request,
) -> Response {
    let handshake = match Handshake::parse(&request) {
        Ok(handshake) => handshake,
        Err(refusal) => return refusal.into_response(),
    };
    let family = match handshake.ticket() {
        ticket if *ticket == [0; 32] => "first",
        ticket if *ticket == [1; 32] => "second",
        ticket if *ticket == [2; 32] => "third",
        _ => return UpgradeRefusal::InvalidCredential.into_response(),
    };
    let reservation = match upgrade.reserve(family) {
        Ok(reservation) => reservation,
        Err(refusal) => return refusal.into_response(),
    };
    witness.reserved.notify_one();
    match witness.mode {
        Mode::BlockUntilStop => upgrade.stopped().await,
        Mode::HoldBeforeUpgrade => std::future::pending().await,
        Mode::FailHandshakeWrite => authority.set(Some(Arc::new(|| false))),
        Mode::Echo | Mode::StallWriter => {}
    }
    upgrade
        .accept(&mut request, handshake, reservation, move |socket| {
            Box::pin(run_test_socket(socket, witness, authority))
        })
        .into_response()
}

async fn run_test_socket(
    socket: &mut TerminalSocket,
    witness: Arc<Witness>,
    authority: OutputAuthority,
) {
    witness.started.fetch_add(1, Ordering::Relaxed);
    let _cleanup = SessionGuard(witness.clone());
    if matches!(witness.mode, Mode::StallWriter) {
        authority.cut_stalled_writes_on(witness.cut_stall.clone());
        *witness.stall_authority.lock().unwrap() = Some(authority);
        witness.writing.notify_one();
        while socket
            .send(Message::Binary(vec![0; 128 * 1024]))
            .await
            .is_ok()
        {}
        return;
    }
    while let Some(message) = socket.next().await {
        match message {
            Ok(Message::Binary(bytes)) => {
                if socket.send(Message::Binary(bytes)).await.is_err() {
                    break;
                }
            }
            Err(Error::Capacity(CapacityError::MessageTooLong { .. })) => {
                witness.oversized.notify_one();
                break;
            }
            Err(Error::Protocol(ProtocolError::UnmaskedFrameFromClient)) => {
                witness.unmasked.notify_one();
                break;
            }
            _ => break,
        }
    }
}

struct Server {
    port: u16,
    client: TlsConnector,
    stop: watch::Sender<bool>,
    joined: JoinHandle<std::io::Result<()>>,
    witness: Arc<Witness>,
    sockets: Arc<Semaphore>,
    terminal_sockets: TerminalSocketBudget,
}

impl Server {
    async fn start(mode: Mode) -> Self {
        let fixture = contract();
        let total = number(&fixture["limits"]["sockets"]["http_listener_total"]);
        assert_eq!(limits::MAX_SOCKETS, total);
        let sockets = Arc::new(Semaphore::new(total));
        let terminal_sockets = TerminalSocketBudget::default();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tls, client) = tls_configs();
        let witness = Arc::new(Witness {
            mode,
            ..Witness::default()
        });
        let router = Router::new()
            .route(PATH, any(test_admission))
            .route("/ordinary", axum::routing::get(|| async { "ordinary" }))
            .with_state(witness.clone());
        let origin = format!("https://localhost:{port}");
        let policy = HttpPolicy::new(&origin, &["https://app.test".to_owned()]).unwrap();
        let (stop, stopped) = watch::channel(false);
        let joined = tokio::spawn(serve_connections(
            listener,
            tls,
            policy.apply(router),
            stopped,
            sockets.clone(),
            terminal_sockets.clone(),
        ));
        Self {
            port,
            client,
            stop,
            joined,
            witness,
            sockets,
            terminal_sockets,
        }
    }

    async fn connect(&self) -> ClientIo {
        let tcp = TcpStream::connect(("127.0.0.1", self.port)).await.unwrap();
        if matches!(self.witness.mode, Mode::StallWriter) {
            socket2::SockRef::from(&tcp)
                .set_recv_buffer_size(4096)
                .unwrap();
        }
        self.client
            .connect("localhost".try_into().unwrap(), tcp)
            .await
            .unwrap()
    }

    fn handshake(&self) -> HeaderMap {
        let fixture = contract();
        let mut headers = HeaderMap::new();
        for (name, value) in fixture["handshake"]["request"]["headers"]
            .as_object()
            .unwrap()
        {
            headers.insert(
                axum::http::HeaderName::try_from(name.as_str()).unwrap(),
                value.as_str().unwrap().parse().unwrap(),
            );
        }
        headers.insert(
            header::HOST,
            format!("localhost:{}", self.port).parse().unwrap(),
        );
        headers
    }

    async fn request(&self, path: &str, headers: &HeaderMap) -> Vec<u8> {
        let mut stream = self.connect().await;
        write_request(&mut stream, path, headers).await;
        read_head(&mut stream).await
    }

    async fn open(&self, ticket: u8) -> ClientSocket {
        let mut stream = self.connect().await;
        write_request(&mut stream, PATH, &self.family_headers(ticket)).await;
        assert_eq!(status(&read_head(&mut stream).await), 101);
        WebSocketStream::from_raw_socket(stream, Role::Client, None).await
    }

    fn family_headers(&self, ticket: u8) -> HeaderMap {
        let mut headers = self.handshake();
        headers.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            format!(
                "{PROTOCOL}, ticket.{}",
                URL_SAFE_NO_PAD.encode([ticket; 32])
            )
            .parse()
            .unwrap(),
        );
        headers
    }

    async fn wait_for_available(&self, available: usize) {
        let permits = timeout(
            TEST_TIMEOUT,
            self.sockets.acquire_many(u32::try_from(available).unwrap()),
        )
        .await
        .unwrap()
        .unwrap();
        drop(permits);
    }

    async fn wait_for_stalled_write(&self) {
        timeout(TEST_TIMEOUT, self.witness.writing.notified())
            .await
            .unwrap();
        let authority = self
            .witness
            .stall_authority
            .lock()
            .unwrap()
            .clone()
            .unwrap();
        timeout(TEST_TIMEOUT, async {
            while !authority.has_pending_write() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    async fn join(self) {
        self.stop.send(true).unwrap();
        timeout(
            limits::SOCKET_TEARDOWN_TIMEOUT + Duration::from_secs(1),
            self.joined,
        )
        .await
        .expect("listener teardown deadline")
        .unwrap()
        .unwrap();
        assert_eq!(self.sockets.available_permits(), limits::MAX_SOCKETS);
        assert_eq!(
            self.terminal_sockets.available(),
            limits::MAX_TERMINAL_SOCKETS
        );
    }
}

fn tls_configs() -> (Arc<rustls::ServerConfig>, TlsConnector) {
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
    let certificate = cert.der().clone();
    let key = rustls::pki_types::PrivatePkcs8KeyDer::from(key_pair.serialize_der());
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let server = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![certificate.clone()], key.into())
        .unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(certificate).unwrap();
    let client = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    (Arc::new(server), TlsConnector::from(Arc::new(client)))
}

async fn write_request(stream: &mut ClientIo, path: &str, headers: &HeaderMap) {
    let mut request = format!("GET {path} HTTP/1.1\r\n").into_bytes();
    for (name, value) in headers {
        request.extend_from_slice(name.as_str().as_bytes());
        request.extend_from_slice(b": ");
        request.extend_from_slice(value.as_bytes());
        request.extend_from_slice(b"\r\n");
    }
    request.extend_from_slice(b"\r\n");
    stream.write_all(&request).await.unwrap();
    stream.flush().await.unwrap();
}

async fn read_head(stream: &mut ClientIo) -> Vec<u8> {
    timeout(TEST_TIMEOUT, async {
        let mut head = Vec::new();
        let mut byte = [0];
        while let Ok(1) = stream.read(&mut byte).await {
            head.push(byte[0]);
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        head
    })
    .await
    .expect("HTTP response or socket drop")
}

fn status(head: &[u8]) -> u64 {
    std::str::from_utf8(head)
        .unwrap()
        .split_ascii_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}

fn assert_upgrade_head(head: &[u8], handshake: &Value) {
    let expected = &handshake["response"];
    assert_eq!(status(head), expected["status"].as_u64().unwrap());
    let head = std::str::from_utf8(head).unwrap();
    for (name, value) in expected["headers"].as_object().unwrap() {
        let actual = head.lines().filter_map(|line| line.split_once(": "));
        let values: Vec<_> = actual
            .filter_map(|(field, value)| field.eq_ignore_ascii_case(name).then_some(value))
            .collect();
        assert_eq!(values, [value.as_str().unwrap()]);
    }
    assert!(!head.contains("ticket."));
    assert!(!head
        .to_ascii_lowercase()
        .contains("sec-websocket-extensions"));
}

#[tokio::test]
async fn real_tls_negotiates_only_v1_and_retains_http_permit_until_peer_drops() {
    let server = Server::start(Mode::Echo).await;
    let mut stream = server.connect().await;
    let mut headers = server.handshake();
    headers.append(header::CONNECTION, "keep-alive".parse().unwrap());
    headers.insert(
        header::SEC_WEBSOCKET_EXTENSIONS,
        "permessage-deflate".parse().unwrap(),
    );
    write_request(&mut stream, PATH, &headers).await;
    assert_upgrade_head(&read_head(&mut stream).await, &contract()["handshake"]);
    let mut socket = WebSocketStream::from_raw_socket(stream, Role::Client, None).await;
    assert_echo(&mut socket, vec![0, 1, 255]).await;
    assert_eq!(server.sockets.available_permits(), limits::MAX_SOCKETS - 1);
    assert_eq!(
        server.terminal_sockets.available(),
        limits::MAX_TERMINAL_SOCKETS - 1
    );
    drop(socket);
    server.wait_for_available(limits::MAX_SOCKETS).await;
    assert_eq!(server.witness.ended.load(Ordering::Relaxed), 1);
    server.join().await;
}

#[tokio::test]
async fn tls_subprotocol_offers_match_the_frozen_vectors() {
    let server = Server::start(Mode::Echo).await;
    let fixture = contract();
    for case in fixture["handshake"]["offers"].as_array().unwrap() {
        let tokens: Vec<_> = case["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .map(|token| token.as_str().unwrap())
            .collect();
        let mut headers = server.handshake();
        headers.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            tokens.join(" ,\t").parse().unwrap(),
        );
        let head = server.request(PATH, &headers).await;
        assert_eq!(
            status(&head),
            case["status"].as_u64().unwrap(),
            "{}",
            case["name"]
        );
        assert!(!std::str::from_utf8(&head).unwrap().contains("ticket."));
    }
    server.join().await;
}

#[tokio::test]
async fn tls_malformed_upgrades_and_wrong_host_never_reach_admission() {
    let server = Server::start(Mode::Echo).await;
    let fixture = contract();
    for case in fixture["handshake"]["invalid_requests"].as_array().unwrap() {
        if case["header"] == "origin" || case.get("ticket_state").is_some() {
            continue;
        }
        let mut headers = server.handshake();
        if let Some(field) = case["header"].as_str() {
            headers.remove(field);
            for value in case["values"].as_array().unwrap() {
                headers.append(
                    axum::http::HeaderName::try_from(field).unwrap(),
                    value.as_str().unwrap().parse().unwrap(),
                );
            }
        }
        let head = server
            .request(case["path"].as_str().unwrap_or(PATH), &headers)
            .await;
        assert_eq!(
            status(&head),
            case["status"].as_u64().unwrap(),
            "{}",
            case["name"]
        );
    }
    let mut headers = server.handshake();
    headers.insert(header::HOST, "localhost".parse().unwrap());
    assert_eq!(status(&server.request(PATH, &headers).await), 421);
    let mut headers = server.handshake();
    headers.append(
        header::HOST,
        format!("localhost:{}", server.port).parse().unwrap(),
    );
    assert_eq!(status(&server.request(PATH, &headers).await), 421);
    assert_eq!(server.witness.started.load(Ordering::Relaxed), 0);
    assert_eq!(
        status(&server.request(PATH, &server.family_headers(3)).await),
        401
    );
    server.join().await;
}

#[tokio::test]
async fn held_connections_refuse_family_and_listener_overflow_but_serve_http() {
    let server = Server::start(Mode::Echo).await;
    let fixture = contract();
    let caps = &fixture["limits"]["sockets"];
    assert_eq!(limits::MAX_TERMINAL_SOCKETS, number(&caps["held_listener"]));
    assert_eq!(
        limits::MAX_FAMILY_TERMINAL_SOCKETS,
        number(&caps["held_family"])
    );
    let mut first = Vec::new();
    for _ in 0..number(&caps["held_family"]) {
        first.push(server.open(0).await);
    }
    assert_eq!(
        status(&server.request(PATH, &server.family_headers(0)).await),
        429
    );
    let mut second = Vec::new();
    for _ in first.len()..number(&caps["held_listener"]) {
        second.push(server.open(1).await);
    }
    assert_eq!(
        status(&server.request(PATH, &server.family_headers(2)).await),
        429
    );
    assert_eq!(
        status(&server.request("/ordinary", &server.handshake()).await),
        200
    );
    drop(first.pop());
    let held = number(&caps["held_listener"]);
    server
        .wait_for_available(limits::MAX_SOCKETS - held + 1)
        .await;
    let mut next = server.open(0).await;
    assert_echo(&mut next, Vec::new()).await;
    first.push(next);
    let witness = server.witness.clone();
    server.join().await;
    assert_eq!(witness.started.load(Ordering::Relaxed), held + 1);
    assert_eq!(witness.ended.load(Ordering::Relaxed), held + 1);
    drop((first, second));
}

#[test]
fn terminal_quota_matches_socket_vectors_including_superseded_io() {
    let fixture = contract();
    for case in fixture["limits"]["socket_cases"].as_array().unwrap() {
        let budget = TerminalSocketBudget::default();
        let superseded = |key: &str| case[key].as_u64().unwrap_or(0) as usize;
        let family = number(&case["held_family"]) + superseded("superseded_held_family");
        let listener = number(&case["held_listener"]) + superseded("superseded_held_listener");
        let mut held: Vec<_> = (0..family)
            .map(|_| budget.reserve("family").unwrap())
            .collect();
        held.extend((family..listener).map(|i| budget.reserve(&format!("other-{i}")).unwrap()));
        let admit = case["admit"].as_bool().unwrap();
        assert_eq!(budget.has_capacity("family"), admit, "{}", case["name"]);
        assert_eq!(budget.reserve("family").is_ok(), admit, "{}", case["name"]);
        drop(held);
        assert!(budget.has_capacity("family"));
    }
}

#[tokio::test]
async fn overload_and_credential_refusals_match_the_frozen_bodies() {
    let fixture = contract();
    let errors = &fixture["ticket"]["refusals"];
    for (refusal, code) in [
        (UpgradeRefusal::Overloaded, "overloaded"),
        (UpgradeRefusal::InvalidCredential, "invalid_credential"),
    ] {
        let response = refusal.into_response();
        assert_eq!(
            u64::from(response.status().as_u16()),
            errors[code]["status"]
        );
        let bytes = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body, errors[code]["body"]);
    }
}

async fn assert_echo(socket: &mut ClientSocket, bytes: Vec<u8>) {
    timeout(TEST_TIMEOUT, async {
        socket.send(Message::Binary(bytes.clone())).await.unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::Binary(bytes)
        );
    })
    .await
    .expect("terminal echo");
}

#[test]
fn transport_limits_match_the_frozen_contract() {
    let fixture = contract();
    let limits = &fixture["limits"];
    assert_eq!(
        limits::MAX_TERMINAL_CLIENT_BYTES,
        number(&limits["message_bytes"]["client"])
    );
    assert_eq!(
        limits::TERMINAL_CODEC_BYTES,
        number(&limits["bridge_reservations_bytes"]["ordinary_codec_per_socket"]),
    );
    assert_eq!(
        limits::SOCKET_TEARDOWN_TIMEOUT.as_millis() as u64,
        limits["sockets"]["teardown_ms"].as_u64().unwrap(),
    );
    assert_eq!(
        crate::http::sse::STALLED_WRITE_TIMEOUT.as_millis() as u64,
        limits["sockets"]["physical_stall_ms"].as_u64().unwrap(),
    );
}

async fn send_fragments(socket: &mut ClientSocket, lengths: &[Value]) {
    for (index, length) in lengths.iter().enumerate() {
        let opcode = if index == 0 {
            Data::Binary
        } else {
            Data::Continue
        };
        let frame = Frame::message(
            vec![7; number(length)],
            OpCode::Data(opcode),
            index + 1 == lengths.len(),
        );
        socket.send(Message::Frame(frame)).await.unwrap();
    }
}

#[tokio::test]
async fn the_codec_checks_aggregate_fragment_length() {
    let server = Server::start(Mode::Echo).await;
    let fixture = contract();
    for case in fixture["limits"]["message_cases"].as_array().unwrap() {
        let Some(lengths) = case["fragments"].as_array() else {
            continue;
        };
        let mut socket = server.open(0).await;
        send_fragments(&mut socket, lengths).await;
        if case["accepted"] == true {
            let expected = Message::Binary(vec![7; number(&case["bytes"])]);
            assert_eq!(
                timeout(TEST_TIMEOUT, socket.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap(),
                expected
            );
        } else {
            timeout(TEST_TIMEOUT, server.witness.oversized.notified())
                .await
                .unwrap();
        }
    }
    server.join().await;
}

#[tokio::test]
async fn the_codec_refuses_unmasked_client_frames() {
    let server = Server::start(Mode::Echo).await;
    let mut stream = server.connect().await;
    write_request(&mut stream, PATH, &server.handshake()).await;
    assert_eq!(status(&read_head(&mut stream).await), 101);
    stream.write_all(&[0x82, 1, 17]).await.unwrap();
    stream.flush().await.unwrap();
    timeout(TEST_TIMEOUT, server.witness.unmasked.notified())
        .await
        .unwrap();
    server.join().await;
}

#[tokio::test]
async fn failed_101_write_releases_pending_terminal_and_http_permits() {
    let server = Server::start(Mode::FailHandshakeWrite).await;
    for _ in 0..=limits::MAX_FAMILY_TERMINAL_SOCKETS {
        let head = server.request(PATH, &server.handshake()).await;
        assert!(head.is_empty(), "unexpected HTTP response: {head:?}");
    }
    assert_eq!(server.witness.started.load(Ordering::Relaxed), 0);
    server.join().await;
}

#[tokio::test]
async fn dropping_the_peer_before_101_returns_the_reservation() {
    let server = Server::start(Mode::HoldBeforeUpgrade).await;
    let mut stream = server.connect().await;
    write_request(&mut stream, PATH, &server.handshake()).await;
    timeout(TEST_TIMEOUT, server.witness.reserved.notified())
        .await
        .unwrap();
    socket2::SockRef::from(stream.get_ref().0)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(stream);
    server.wait_for_available(limits::MAX_SOCKETS).await;
    assert_eq!(
        server.terminal_sockets.available(),
        limits::MAX_TERMINAL_SOCKETS
    );
    assert_eq!(server.witness.started.load(Ordering::Relaxed), 0);
    server.join().await;
}

#[tokio::test]
async fn stop_after_reservation_prevents_101_and_joins_the_connection() {
    let server = Server::start(Mode::BlockUntilStop).await;
    let mut stream = server.connect().await;
    write_request(&mut stream, PATH, &server.handshake()).await;
    timeout(TEST_TIMEOUT, server.witness.reserved.notified())
        .await
        .unwrap();
    let witness = server.witness.clone();
    server.join().await;
    assert!(read_head(&mut stream).await.is_empty());
    assert_eq!(witness.started.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn shutdown_bounds_the_join_of_a_pending_admission_that_never_resumes() {
    let server = Server::start(Mode::HoldBeforeUpgrade).await;
    let mut stream = server.connect().await;
    write_request(&mut stream, PATH, &server.handshake()).await;
    timeout(TEST_TIMEOUT, server.witness.reserved.notified())
        .await
        .unwrap();
    tokio::time::pause();
    let start = tokio::time::Instant::now();
    server.join().await;
    assert!(start.elapsed() <= limits::SOCKET_TEARDOWN_TIMEOUT);
    assert!(read_head(&mut stream).await.is_empty());
}

#[tokio::test]
async fn the_physical_stall_cut_still_applies_after_upgrade() {
    let server = Server::start(Mode::StallWriter).await;
    let socket = server.open(0).await;
    server.wait_for_stalled_write().await;
    server.witness.cut_stall.notify_one();
    server.wait_for_available(limits::MAX_SOCKETS).await;
    assert_eq!(server.witness.ended.load(Ordering::Relaxed), 1);
    drop(socket);
    server.join().await;
}

#[tokio::test]
async fn listener_shutdown_joins_a_stalled_websocket_writer() {
    let server = Server::start(Mode::StallWriter).await;
    let socket = server.open(0).await;
    server.wait_for_stalled_write().await;
    let witness = server.witness.clone();
    server.join().await;
    assert_eq!(witness.ended.load(Ordering::Relaxed), 1);
    drop(socket);
}
