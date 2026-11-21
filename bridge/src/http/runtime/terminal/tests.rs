use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};

use axum::http::{header, HeaderMap};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{watch, Notify},
    task::JoinHandle,
    time::{timeout, Instant},
};
use tokio_rustls::{client::TlsStream, TlsConnector};
use tokio_tungstenite::{
    tungstenite::protocol::{
        frame::{
            coding::{Data, OpCode},
            Frame,
        },
        Role,
    },
    WebSocketStream,
};

use crate::{
    execution::{
        session::{DeliveryError, EncodedEvent, EventSink, StreamClass},
        FrameHandler,
    },
    http::{
        listener::terminal_upgrade::{
            tests::{read_head, status, tls_configs, write_request},
            PROTOCOL,
        },
        policy::HttpPolicy,
        runtime::{runtime_with, serve_mounted, GrantVerifier, VerifiedAuthority},
    },
    timing::FrameClock,
};

use super::*;

type ClientIo = TlsStream<TcpStream>;
type Client = WebSocketStream<ClientIo>;
const TIMEOUT: Duration = Duration::from_secs(10);

fn fixture() -> &'static Value {
    static FIXTURE: OnceLock<Value> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../../../fixtures/api/http/terminal-websocket-v1.json"
        ))
        .unwrap()
    })
}

fn case_text(value: &Value) -> &str {
    value.as_str().unwrap()
}

fn origin() -> &'static str {
    case_text(&fixture()["endpoint"]["caller_origin"])
}

fn origins() -> Vec<String> {
    serde_json::from_value(fixture()["endpoint"]["allowed_caller_origins"].clone()).unwrap()
}

struct Verifier {
    bridge: String,
    revoked: Arc<AtomicBool>,
    changed: Arc<Notify>,
    proof_lifetime: Duration,
}

#[async_trait::async_trait]
impl GrantVerifier for Verifier {
    async fn verify(
        &self,
        bearer: &str,
    ) -> Result<VerifiedAuthority, crate::http::auth::AuthError> {
        let writer = bearer != "opening";
        let revoked = self.revoked.clone();
        Ok(VerifiedAuthority {
            identity: crate::execution::session::HttpIdentity {
                issuer: "https://issuer.test".into(),
                account_id: "account".into(),
                device_id: "device".into(),
                family_id: "family".into(),
                origin: self.bridge.clone(),
                endpoint_epoch: "endpoint".into(),
                authorization_epoch: 1,
            },
            deadline: Instant::now()
                + if writer {
                    self.proof_lifetime
                } else {
                    TIMEOUT * 6
                },
            live: Arc::new(move || !writer || !revoked.load(Ordering::Acquire)),
            scopes: vec!["bridge:read".into(), "bridge:write".into()].into(),
            credential_digest: if writer { [1; 32] } else { [2; 32] },
            changed: self.changed.clone(),
        })
    }

    async fn revalidate(
        &self,
        bearer: &str,
    ) -> Result<VerifiedAuthority, crate::http::auth::AuthError> {
        self.verify(bearer).await
    }
}

struct Server {
    port: u16,
    state: HttpState,
    client: TlsConnector,
    stop: watch::Sender<bool>,
    serving: JoinHandle<std::io::Result<()>>,
    verifier: Arc<Verifier>,
}

impl Server {
    async fn start(handler: FrameHandler) -> Self {
        Self::start_with(handler, TIMEOUT * 6).await
    }

    async fn start_with(handler: FrameHandler, proof_lifetime: Duration) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tls, client) = tls_configs();
        let bridge = format!("https://localhost:{port}");
        let policy = HttpPolicy::new(&bridge, &origins()).unwrap();
        let verifier = Arc::new(Verifier {
            bridge,
            revoked: Arc::default(),
            changed: Arc::default(),
            proof_lifetime,
        });
        let mut runtime = runtime_with(ExecutionService::new(handler), verifier.clone());
        runtime.state.terminal_tickets = Some(Arc::default());
        let state = runtime.state.clone();
        let (stop, shutdown) = watch::channel(false);
        let serving = tokio::spawn(serve_mounted(listener, tls, policy, runtime, shutdown));
        Self {
            port,
            state,
            client,
            stop,
            serving,
            verifier,
        }
    }

    async fn connect(&self) -> ClientIo {
        let tcp = TcpStream::connect(("127.0.0.1", self.port)).await.unwrap();
        self.client
            .connect("localhost".try_into().unwrap(), tcp)
            .await
            .unwrap()
    }

    async fn json(
        &self,
        method: &str,
        path: &str,
        bearer: &str,
        session: &str,
        body: Value,
    ) -> (u64, Value) {
        let mut stream = self.connect().await;
        let body = body.to_string();
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost:{}\r\nOrigin: {}\r\n\
             Authorization: Bearer {bearer}\r\nX-Build-Session: {session}\r\n\
             Connection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            self.port, origin(), body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();
        let head = read_head(&mut stream).await;
        let body = read_json_body(&mut stream, &head).await;
        (status(&head), body)
    }

    async fn opening(&self) -> Value {
        let (status, opening) = self
            .json(
                "POST",
                "/v1/sessions",
                "opening",
                "",
                json!({"api_range":">=3 <4"}),
            )
            .await;
        assert_eq!(status, 201, "{opening:?}");
        opening
    }

    async fn issue(&self, opening: &Value) -> (u64, Value) {
        self.json(
            "POST",
            terminal_tickets::PATH,
            "writer",
            case_text(&opening["session_id"]),
            json!({"opening_generation": opening["opening_generation"]}),
        )
        .await
    }

    async fn ticket(&self, opening: &Value) -> String {
        let (status, body) = self.issue(opening).await;
        assert_eq!(status, 201, "{body:?}");
        case_text(&body["ticket"]).to_owned()
    }

    fn handshake(&self, token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (key, value) in fixture()["handshake"]["request"]["headers"]
            .as_object()
            .unwrap()
        {
            headers.insert(
                axum::http::HeaderName::try_from(key).unwrap(),
                case_text(value).parse().unwrap(),
            );
        }
        headers.insert(
            header::HOST,
            format!("localhost:{}", self.port).parse().unwrap(),
        );
        headers.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            format!("{PROTOCOL}, ticket.{token}").parse().unwrap(),
        );
        headers
    }

    async fn upgrade(&self, headers: &HeaderMap) -> (Vec<u8>, ClientIo) {
        let mut client = self.connect().await;
        write_request(&mut client, PATH, headers).await;
        let head = read_head(&mut client).await;
        (head, client)
    }

    async fn socket(&self, ticket: &str) -> Client {
        let (head, io) = self.upgrade(&self.handshake(ticket)).await;
        assert_eq!(status(&head), 101, "{}", String::from_utf8_lossy(&head));
        assert!(!String::from_utf8_lossy(&head).contains(ticket));
        WebSocketStream::from_raw_socket(io, Role::Client, None).await
    }

    async fn stop(self) {
        self.stop.send(true).unwrap();
        timeout(TIMEOUT, self.serving)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}

async fn read_json_body(stream: &mut ClientIo, head: &[u8]) -> Value {
    let length: usize = String::from_utf8_lossy(head)
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .expect("JSON response has a content length");
    let mut bytes = vec![0; length];
    timeout(TIMEOUT, stream.read_exact(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn fake_handler() -> FrameHandler {
    FrameHandler::new(FrameClock::new(), |_, frame, _| {
        panic!("unexpected terminal dispatch {}", frame.payload["method"]);
    })
    .with_greeting_reads(|epoch| json!({"api_version": api::API_VERSION, "bridge_epoch":epoch}))
}

async fn send(socket: &mut Client, id: &str, method: &str, params: Value) {
    socket
        .send(Message::Binary(
            json!({"id":id,"method":method,"params":params})
                .to_string()
                .into_bytes(),
        ))
        .await
        .unwrap();
}

async fn next(socket: &mut Client) -> Message {
    timeout(TIMEOUT, socket.next())
        .await
        .unwrap()
        .expect("terminal frame")
        .unwrap()
}

async fn object(socket: &mut Client) -> Value {
    let Message::Binary(bytes) = next(socket).await else {
        panic!("expected binary terminal JSON")
    };
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["bridge_epoch"], crate::bridge_epoch::current());
    value
}

async fn call(socket: &mut Client, id: &str, method: &str, params: Value) -> Value {
    send(socket, id, method, params).await;
    loop {
        let value = object(socket).await;
        if value["id"] == id {
            return value;
        }
    }
}

async fn assert_closed(socket: &mut Client, expected: Option<u16>) {
    let answer = timeout(TIMEOUT, socket.next()).await.unwrap();
    match expected {
        Some(code) => assert!(
            matches!(&answer, Some(Ok(Message::Close(Some(frame)))) if u16::from(frame.code) == code),
            "{answer:?}"
        ),
        None => assert!(
            answer.as_ref().is_none_or(|result| result.is_err()),
            "{answer:?}"
        ),
    }
}

struct ApplicationSink(Arc<AtomicUsize>);

impl EventSink for ApplicationSink {
    fn try_send(&self, _event: EncodedEvent) -> Result<(), DeliveryError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

fn application_binding(
    server: &Server,
    opening: &Value,
) -> (crate::execution::session::EventBinding, Arc<AtomicUsize>) {
    let session = server.state.openings.lock().unwrap()[case_text(&opening["session_id"])]
        .session
        .clone();
    let count = Arc::new(AtomicUsize::new(0));
    let binding = server
        .state
        .execution
        .sessions()
        .bind_events(
            &session,
            StreamClass::Application,
            Arc::new(ApplicationSink(count.clone())),
        )
        .unwrap();
    (binding, count)
}

fn greeting(handler: FrameHandler) -> FrameHandler {
    handler
        .with_greeting_reads(|epoch| json!({"api_version": api::API_VERSION, "bridge_epoch":epoch}))
}

fn success(frame: &crate::transport::Frame, result: Value) -> Value {
    api::reply(frame.payload["id"].clone(), Ok(result))
}

mod admission;
mod dispatch;
mod wire;
