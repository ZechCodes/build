//! The relay client — the bridge's link to the world.
//!
//! The device connects to the relay over a WebSocket, authenticates with an
//! Ed25519-signed challenge, uploads its X25519 transport public key, and then
//! relays end-to-end encrypted sessions with browser clients. The relay only ever
//! sees the opaque outer envelope (`{version, session_id, route_to, nonce,
//! ciphertext}`) — it routes by `session_id` and forwards JSON it cannot read.
//!
//! Frame flow, per the relay's `/ws/device` protocol:
//! - relay → `{"type":"authenticated","heartbeat_interval_s":N}`
//! - device → `{"type":"transport_key","transport_public_key":"<b64>"}`
//! - device → `{"type":"heartbeat"}` every N seconds
//! - relay → `{"type":"session_init","session_id":S,"session_init":{...}}`
//! - device → `{"type":"session_accept","session_id":S,"envelope":{...}}`
//! - both → `{"type":"e2ee_envelope","session_id":S,"envelope":{...}}`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderValue, Request};
use tokio_tungstenite::tungstenite::Message;

use crate::transport::{self, Envelope, Frame, KeyPairB64, OuterFields, SessionInit};

/// The signed-challenge path the relay expects (`{ts}.GET./ws/device`).
const AUTH_PATH: &str = "/ws/device";

#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error("websocket error: {0}")]
    Ws(Box<tokio_tungstenite::tungstenite::Error>),
    #[error("transport error: {0}")]
    Transport(#[from] transport::TransportError),
    #[error("protocol error: {0}")]
    Protocol(String),
}

impl From<tokio_tungstenite::tungstenite::Error> for RelayError {
    fn from(err: tokio_tungstenite::tungstenite::Error) -> Self {
        // Boxed: tungstenite's error type is large, and a large `Err` variant
        // bloats every `Result` in this module.
        RelayError::Ws(Box::new(err))
    }
}

/// The device's stable identity: who it is and the keys that prove it.
#[derive(Debug, Clone)]
pub struct DeviceIdentity {
    pub device_id: String,
    /// Ed25519 seed (base64) — signs the auth challenge.
    pub identity_private_key_b64: String,
    /// The durable X25519 transport keypair clients wrap session keys to.
    pub transport: KeyPairB64,
}

/// A handle the app uses to push encrypted frames to a specific client session —
/// the channel for server-initiated output (live terminal bytes, updates), not
/// just request replies. Cheap to clone; store one per attached client.
#[derive(Clone)]
pub struct SessionSender {
    session_id: String,
    session_key: String,
    out: mpsc::UnboundedSender<Message>,
}

impl SessionSender {
    /// The session this sender targets.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// A sender not bound to a live connection — for tests and request/response
    /// callers that never push. `push` succeeds-into-the-void.
    pub fn detached(session_id: impl Into<String>) -> Self {
        let (out, _rx) = mpsc::unbounded_channel();
        SessionSender {
            session_id: session_id.into(),
            session_key: String::new(),
            out,
        }
    }

    /// Test-only: a sender with a real session key and a captured channel, so
    /// tests can decrypt every pushed frame (`term.output`, `term.closed`, …)
    /// with [`decrypt_push`](Self::decrypt_push).
    #[cfg(test)]
    pub fn observable(
        session_id: impl Into<String>,
    ) -> (Self, mpsc::UnboundedReceiver<Message>, String) {
        let (out, rx) = mpsc::unbounded_channel();
        let session_key = transport::generate_session_key();
        (
            SessionSender {
                session_id: session_id.into(),
                session_key: session_key.clone(),
                out,
            },
            rx,
            session_key,
        )
    }

    /// Test-only: decode one captured [`Self::observable`] message back to the
    /// pushed inner payload.
    #[cfg(test)]
    pub fn decrypt_push(session_key: &str, message: &Message) -> Value {
        let Message::Text(text) = message else {
            panic!("pushes are text frames, got {message:?}");
        };
        let outer: Value = serde_json::from_str(text).expect("push is JSON");
        let envelope: Envelope =
            serde_json::from_value(outer["envelope"].clone()).expect("push carries an envelope");
        transport::decrypt_envelope(session_key, &envelope)
            .expect("push decrypts with the session key")
            .payload
    }

    /// Encrypt `payload` as an inner frame and send it to the client as an
    /// `e2ee_envelope`. Returns false once the connection is gone (so the app can
    /// drop the stale sender).
    pub fn push(&self, payload: Value) -> bool {
        let envelope = match transport::encrypt_frame(
            &self.session_key,
            &OuterFields {
                session_id: self.session_id.clone(),
                route_to: format!("session:{}", self.session_id),
            },
            &transport::FrameFields {
                frame_type: "data".into(),
                sender: "device".into(),
                payload,
                message_id: None,
                created_at: None,
            },
            None,
        ) {
            Ok(env) => env,
            Err(_) => return false,
        };
        self.out
            .send(Message::Text(
                json!({ "type": "e2ee_envelope", "session_id": self.session_id, "envelope": envelope })
                    .to_string(),
            ))
            .is_ok()
    }
}

/// Handles a decrypted request frame. Receives a [`SessionSender`] (so it can
/// register the session for server-initiated pushes) and returns the response
/// payload to send back.
pub type FrameHandler = Arc<dyn Fn(SessionSender, Frame) -> Value + Send + Sync>;

/// How many handlers may run at once. Handlers are blocking (they take the app
/// mutex, and some of them walk a worktree with libgit2), so they run on the
/// blocking pool and this is the width of the useful work, not of the CPU: wide
/// enough that a couple of slow diffs still leave room for the cheap calls a
/// browser makes beside them, narrow enough that a flood of expensive frames
/// cannot put dozens of concurrent worktree scans on the disk at once.
const DISPATCH_WORKERS: usize = 8;

/// How many frames may wait for a worker. The socket is read as fast as frames
/// arrive and each waiting frame is a small JSON payload, so this is deep enough
/// to absorb a multi-second stall of every poller a few browser tabs can run
/// (the SPA polls a couple of times a second per tab) without the memory ever
/// mattering. Past it, `dispatch` waits: the flood backs up in a queue whose size
/// is known instead of growing until the process dies.
const DISPATCH_QUEUE_DEPTH: usize = 256;

/// How many frames may wait on one ordered lane. A lane carries one terminal's
/// input, acks and resizes — frames whose handlers are a write to a pty fd — so a
/// lane this deep holds far more than the largest paste burst a client can make
/// before the caller is asked to wait.
const LANE_QUEUE_DEPTH: usize = 64;

/// The method that ends a terminal — and with it the terminal's ordered lane.
const TERMINAL_CLOSE_METHOD: &str = "term.close";

/// One decrypted frame waiting for a handler.
struct Job {
    sender: SessionSender,
    frame: Frame,
}

/// What an ordered lane can be asked to do.
enum LaneMessage {
    /// Run this frame's handler; the lane runs one at a time, in arrival order.
    Run(Job),
    /// Tell me when everything queued before you has run. Used to make a session
    /// close the last thing that happens to that session.
    Fence(tokio::sync::oneshot::Sender<()>),
}

/// Runs frame handlers off the read loop.
///
/// The read loop reads, decrypts and enqueues; nothing else. Handlers run on a
/// bounded pool of workers, each of which calls the handler on a blocking thread
/// (handlers take the app mutex and can walk a worktree), so no handler — however
/// slow — can keep the socket from being drained. That was the wedge: one
/// `board.list` diff on a churning 56k-file worktree took seconds, and the frames
/// behind it piled up in the kernel until the relay called the device dead.
///
/// Frames that mean something as a stream keep their order: anything naming a
/// `term_id` goes to a serial lane keyed by that terminal and that session, so a
/// client's input and acks are handled in the order it sent them. Everything else
/// is an independent request and runs concurrently.
struct Dispatcher {
    handler: FrameHandler,
    /// The shared pool queue. Bounded: a full queue makes the read loop wait.
    jobs: mpsc::Sender<Job>,
    /// (session_id, term_id) → its serial lane.
    lanes: HashMap<(String, String), mpsc::Sender<LaneMessage>>,
}

impl Dispatcher {
    fn new(handler: FrameHandler) -> Self {
        Self::with_capacity(handler, DISPATCH_QUEUE_DEPTH, DISPATCH_WORKERS)
    }

    fn with_capacity(handler: FrameHandler, queue_depth: usize, workers: usize) -> Self {
        let (jobs, rx) = mpsc::channel::<Job>(queue_depth.max(1));
        // One queue, many workers: whoever is free takes the next frame.
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        for _ in 0..workers.max(1) {
            let rx = rx.clone();
            let handler = handler.clone();
            tokio::spawn(async move {
                loop {
                    let job = rx.lock().await.recv().await;
                    let Some(job) = job else { break };
                    run_job(&handler, job).await;
                }
            });
        }
        Dispatcher {
            handler,
            jobs,
            lanes: HashMap::new(),
        }
    }

    /// Hand one decrypted request frame to a worker. Waits only when every
    /// worker is busy and the queue is full.
    async fn dispatch(&mut self, sender: SessionSender, frame: Frame) {
        match ordered_lane(&sender, &frame) {
            Some(key) => {
                let closing = frame.payload.get("method").and_then(Value::as_str)
                    == Some(TERMINAL_CLOSE_METHOD);
                let lane = self.lane(key.clone());
                let _ = lane.send(LaneMessage::Run(Job { sender, frame })).await;
                if closing {
                    // A terminal id is minted once and never reused, so its close
                    // is the last frame its lane can carry. Letting the sender go
                    // ends the lane as soon as it has run that close.
                    self.lanes.remove(&key);
                }
            }
            None => {
                let _ = self.jobs.send(Job { sender, frame }).await;
            }
        }
    }

    /// The lane for a terminal, started on first use.
    fn lane(&mut self, key: (String, String)) -> &mpsc::Sender<LaneMessage> {
        self.lanes.entry(key).or_insert_with(|| {
            let (tx, mut rx) = mpsc::channel::<LaneMessage>(LANE_QUEUE_DEPTH);
            let handler = self.handler.clone();
            tokio::spawn(async move {
                while let Some(message) = rx.recv().await {
                    match message {
                        LaneMessage::Run(job) => run_job(&handler, job).await,
                        LaneMessage::Fence(reply) => {
                            let _ = reply.send(());
                        }
                    }
                }
            });
            tx
        })
    }

    /// A session ended: tell the app so it releases the session's attachments.
    ///
    /// This is the session's last frame and it is treated as one — it runs only
    /// after every frame that arrived before it, or an attach still sitting on a
    /// lane would register a sender into a session already closed and push
    /// terminal output at a browser that is gone.
    async fn close_session(&mut self, session_id: &str) {
        let mut fences = Vec::new();
        let session_lanes: Vec<(String, String)> = self
            .lanes
            .keys()
            .filter(|(session, _)| session == session_id)
            .cloned()
            .collect();
        for key in session_lanes {
            let Some(lane) = self.lanes.remove(&key) else {
                continue;
            };
            let (reply, wait) = tokio::sync::oneshot::channel();
            if lane.send(LaneMessage::Fence(reply)).await.is_ok() {
                fences.push(wait);
            }
            // Dropping the lane sender ends the lane once it has drained.
        }

        let handler = self.handler.clone();
        let session_id = session_id.to_string();
        tokio::spawn(async move {
            for fence in fences {
                let _ = fence.await;
            }
            let closed = Frame {
                session_id: session_id.clone(),
                message_id: String::new(),
                frame_type: "close".into(),
                sender: "relay".into(),
                created_at: String::new(),
                payload: Value::Null,
            };
            let sender = SessionSender::detached(&session_id);
            // No response: nobody is left to read one.
            let _ = tokio::task::spawn_blocking(move || handler(sender, closed)).await;
        });
    }
}

/// The serial lane a frame belongs to, if its order is part of its meaning.
/// Terminal traffic — input, acks, resizes, attach and close — is a stream per
/// terminal per client; everything else is an independent request.
fn ordered_lane(sender: &SessionSender, frame: &Frame) -> Option<(String, String)> {
    let term_id = frame
        .payload
        .get("params")
        .and_then(|params| params.get("term_id"))
        .and_then(Value::as_str)?;
    Some((sender.session_id().to_string(), term_id.to_string()))
}

/// Run one handler on a blocking thread and send its answer back.
///
/// Blocking, not async: a handler takes the app mutex and may sit in libgit2 for
/// seconds. On a runtime worker that would block the read loop and the writer
/// with it — the very thing this queue exists to prevent.
async fn run_job(handler: &FrameHandler, job: Job) {
    let Job { sender, frame } = job;
    let id = frame.payload.get("id").cloned().unwrap_or(Value::Null);
    let handler = handler.clone();
    let answering = sender.clone();
    match tokio::task::spawn_blocking(move || handler(answering, frame)).await {
        Ok(payload) => {
            sender.push(payload);
        }
        // The handler panicked (or the runtime is shutting down). Answer anyway:
        // a client that never hears back waits forever.
        Err(_) => {
            sender.push(json!({ "id": id, "ok": false, "error": "handler failed" }));
        }
    }
}

/// Build the authenticated WebSocket upgrade request: the relay verifies an
/// Ed25519 signature over `{timestamp}.GET./ws/device`.
fn auth_request(url: &str, identity: &DeviceIdentity) -> Result<Request<()>, RelayError> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs()
        .to_string();
    let challenge = format!("{timestamp}.GET.{AUTH_PATH}");
    let signature =
        transport::sign_message_b64(&identity.identity_private_key_b64, challenge.as_bytes())?;

    let mut request = url.into_client_request().map_err(RelayError::from)?;
    let headers = request.headers_mut();
    headers.insert(
        "X-Device-Id",
        HeaderValue::from_str(&identity.device_id)
            .map_err(|e| RelayError::Protocol(e.to_string()))?,
    );
    headers.insert(
        "X-Timestamp",
        HeaderValue::from_str(&timestamp).map_err(|e| RelayError::Protocol(e.to_string()))?,
    );
    headers.insert(
        "X-Signature",
        HeaderValue::from_str(&signature).map_err(|e| RelayError::Protocol(e.to_string()))?,
    );
    Ok(request)
}

/// Connect to the relay and run the device session until the socket closes.
///
/// Handles both plain-`ws` URLs (local/dev) and `wss://` (production, e.g.
/// `wss://relay.getbuild.ing/ws/device`) — TLS is rustls with bundled webpki
/// roots (the crate's only TLS feature, so the default connector below can never
/// silently pick native-tls). Decrypted client request frames are passed to
/// `handler`; its returned payload is encrypted and sent back as an
/// `e2ee_envelope`.
pub async fn run(
    url: &str,
    identity: &DeviceIdentity,
    handler: FrameHandler,
) -> Result<(), RelayError> {
    run_with_connector(url, identity, handler, None).await
}

/// [`run`], with an explicit TLS connector. `None` uses the default (rustls +
/// webpki roots for `wss://`, plain TCP for `ws` URLs); tests inject
/// `Connector::Rustls` trusting a self-signed root to exercise real TLS locally.
pub async fn run_with_connector(
    url: &str,
    identity: &DeviceIdentity,
    handler: FrameHandler,
    tls_connector: Option<tokio_tungstenite::Connector>,
) -> Result<(), RelayError> {
    let request = auth_request(url, identity)?;
    let (stream, _resp) =
        tokio_tungstenite::connect_async_tls_with_config(request, None, false, tls_connector)
            .await?;
    let (mut sink, mut source) = stream.split();

    // One writer owns the sink; everything else queues messages to it. Unbounded
    // so pushes (terminal output bursts) never block the app under a lock.
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    // session_id → session_key (base64). One reader task, so no lock needed.
    let mut sessions: HashMap<String, String> = HashMap::new();
    let mut heartbeat: Option<tokio::task::JoinHandle<()>> = None;
    // Handlers run here, not on this task: below, the loop only reads, decrypts
    // and enqueues, so no handler can stop the socket from being drained.
    let mut dispatcher = Dispatcher::new(handler);

    while let Some(message) = source.next().await {
        let message = message?;
        let text = match message {
            Message::Text(t) => t,
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => break,
            _ => continue,
        };
        let Ok(msg) = serde_json::from_str::<Value>(&text) else {
            continue;
        };

        match msg.get("type").and_then(Value::as_str).unwrap_or("") {
            "authenticated" => {
                let interval = msg
                    .get("heartbeat_interval_s")
                    .and_then(Value::as_u64)
                    .unwrap_or(30);
                // Upload our transport public key so clients can wrap to it.
                send(
                    &out_tx,
                    json!({
                        "type": "transport_key",
                        "transport_public_key": identity.transport.public_key_b64,
                    }),
                );
                heartbeat = Some(spawn_heartbeat(out_tx.clone(), interval));
            }
            "session_init" => {
                if let Err(e) = handle_session_init(&out_tx, identity, &mut sessions, &msg) {
                    tracing_protocol_error(&e);
                }
            }
            "e2ee_envelope" => {
                if let Err(e) = handle_envelope(&out_tx, &mut sessions, &msg, &mut dispatcher).await
                {
                    tracing_protocol_error(&e);
                }
            }
            // The relay says the browser behind this session is gone: forget the
            // session key and let the app stop pushing into it.
            "session_closed" => {
                if let Some(session_id) = msg.get("session_id").and_then(Value::as_str) {
                    end_session(&mut sessions, session_id, &mut dispatcher).await;
                }
            }
            // "response"/"error"/unknown: nothing for the device to do here.
            _ => {}
        }
    }

    if let Some(h) = heartbeat {
        h.abort();
    }
    drop(out_tx);
    let _ = writer.await;
    Ok(())
}

/// A client opened a session: unwrap its session key and prove receipt with an
/// encrypted `session_accept`.
fn handle_session_init(
    out_tx: &mpsc::UnboundedSender<Message>,
    identity: &DeviceIdentity,
    sessions: &mut HashMap<String, String>,
    msg: &Value,
) -> Result<(), RelayError> {
    let session_id = field_str(msg, "session_id")?;
    let init: SessionInit = serde_json::from_value(
        msg.get("session_init")
            .cloned()
            .ok_or_else(|| RelayError::Protocol("session_init payload missing".into()))?,
    )
    .map_err(|e| RelayError::Protocol(format!("bad session_init: {e}")))?;

    let opened = transport::open_session_init(&identity.transport.private_key_b64, &init)?;
    sessions.insert(session_id.clone(), opened.session_key_b64.clone());

    let accept = transport::build_session_accept(
        &opened.session_key_b64,
        &session_id,
        &format!("session:{session_id}"),
        None,
    )?;
    send(
        out_tx,
        json!({
            "type": "session_accept",
            "session_id": session_id,
            "envelope": accept,
        }),
    );
    Ok(())
}

/// A client sent an encrypted frame: decrypt it, hand the inner request to the
/// application along with a [`SessionSender`], and send the response back.
async fn handle_envelope(
    out_tx: &mpsc::UnboundedSender<Message>,
    sessions: &mut HashMap<String, String>,
    msg: &Value,
    dispatcher: &mut Dispatcher,
) -> Result<(), RelayError> {
    let session_id = field_str(msg, "session_id")?;
    let session_key = sessions
        .get(&session_id)
        .cloned()
        .ok_or_else(|| RelayError::Protocol(format!("no session key for {session_id}")))?;
    let envelope: Envelope = serde_json::from_value(
        msg.get("envelope")
            .cloned()
            .ok_or_else(|| RelayError::Protocol("envelope missing".into()))?,
    )
    .map_err(|e| RelayError::Protocol(format!("bad envelope: {e}")))?;

    let frame = transport::decrypt_envelope(&session_key, &envelope)?;
    // `close` frames end the conversation: forget the key and tell the app.
    if frame.frame_type == "close" {
        end_session(sessions, &session_id, dispatcher).await;
        return Ok(());
    }

    let sender = SessionSender {
        session_id: session_id.clone(),
        session_key,
        out: out_tx.clone(),
    };
    // The handler runs on a worker and pushes its own answer down this same
    // channel; the app may also push server-initiated frames during the call
    // (e.g. an initial terminal flush).
    dispatcher.dispatch(sender, frame).await;
    Ok(())
}

/// Drop a finished session: forget its key — old session keys must not stay
/// decryptable for the connection's lifetime — and hand the app a synthetic
/// `close` frame so it releases the session's attachments (e.g. terminal
/// senders) instead of encrypting into a session nobody will read again.
async fn end_session(
    sessions: &mut HashMap<String, String>,
    session_id: &str,
    dispatcher: &mut Dispatcher,
) {
    if sessions.remove(session_id).is_none() {
        return; // unknown/already-closed session: nothing to release
    }
    dispatcher.close_session(session_id).await;
}

fn spawn_heartbeat(
    out_tx: mpsc::UnboundedSender<Message>,
    interval_s: u64,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(interval_s.max(1)));
        loop {
            ticker.tick().await;
            if out_tx
                .send(Message::Text(json!({"type": "heartbeat"}).to_string()))
                .is_err()
            {
                break;
            }
        }
    })
}

fn send(out_tx: &mpsc::UnboundedSender<Message>, value: Value) {
    let _ = out_tx.send(Message::Text(value.to_string()));
}

fn field_str(msg: &Value, key: &str) -> Result<String, RelayError> {
    msg.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| RelayError::Protocol(format!("{key} missing")))
}

fn tracing_protocol_error(err: &RelayError) {
    // Protocol errors on a single frame must not kill the connection; a real
    // build wires this to `tracing`. Kept minimal here.
    let _ = err;
}

#[cfg(test)]
mod dispatcher_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    fn request(id: u64, method: &str, params: Value) -> Frame {
        Frame {
            session_id: "s".into(),
            message_id: format!("m-{id}"),
            frame_type: "data".into(),
            sender: "client".into(),
            created_at: String::new(),
            payload: json!({ "id": id, "method": method, "params": params }),
        }
    }

    /// Wait for the next push on an observable session, decoded.
    async fn next_push(
        rx: &mut mpsc::UnboundedReceiver<Message>,
        key: &str,
        within: Duration,
    ) -> Value {
        let message = tokio::time::timeout(within, rx.recv())
            .await
            .expect("a response arrives in time")
            .expect("the session channel is open");
        SessionSender::decrypt_push(key, &message)
    }

    /// The incident in one test: a handler that takes seconds must not keep the
    /// next frame from being handled. Frames are dispatched from one task (as the
    /// read loop does), so a stall here is a stall of the socket.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_slow_handler_does_not_hold_up_the_next_frame() {
        let handler: FrameHandler = Arc::new(|_sender, frame| {
            if frame.payload["method"] == "slow" {
                std::thread::sleep(Duration::from_millis(750));
            }
            json!({ "id": frame.payload["id"], "ok": true })
        });
        let mut dispatcher = Dispatcher::with_capacity(handler, 8, 4);

        let (slow_sender, mut slow_rx, slow_key) = SessionSender::observable("s-slow");
        let (fast_sender, mut fast_rx, fast_key) = SessionSender::observable("s-fast");

        dispatcher
            .dispatch(slow_sender, request(1, "slow", json!({})))
            .await;
        dispatcher
            .dispatch(fast_sender, request(2, "fast", json!({})))
            .await;

        let fast = next_push(&mut fast_rx, &fast_key, Duration::from_millis(250)).await;
        assert_eq!(
            fast["id"], 2,
            "the fast frame answered while the slow one ran"
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(10), slow_rx.recv())
                .await
                .is_err(),
            "the slow handler is still running — this is what the fast frame overtook"
        );

        let slow = next_push(&mut slow_rx, &slow_key, Duration::from_secs(5)).await;
        assert_eq!(slow["id"], 1, "the slow frame still gets its answer");
    }

    /// Terminal input and acks are a stream: their order is the client's meaning.
    /// Frames carrying the same `term_id` run one at a time, in arrival order,
    /// even while independent frames run beside them.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn frames_for_one_terminal_keep_their_order() {
        let seen: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = seen.clone();
        let handler: FrameHandler = Arc::new(move |_sender, frame| {
            let id = frame.payload["id"].as_u64().unwrap_or_default();
            // The first frame is the slow one: without a serial lane the ones
            // behind it would finish first and record out of order.
            if id == 0 {
                std::thread::sleep(Duration::from_millis(300));
            }
            recorder.lock().unwrap().push(id);
            json!({ "id": id, "ok": true })
        });
        let mut dispatcher = Dispatcher::with_capacity(handler, 64, 8);

        let (sender, mut rx, key) = SessionSender::observable("s-1");
        for id in 0..12u64 {
            dispatcher
                .dispatch(
                    sender.clone(),
                    request(id, "term.input", json!({ "term_id": "term-1" })),
                )
                .await;
        }
        // A second terminal is a second lane: it answers without waiting for the
        // slow frame ahead of it on the first.
        let (other, mut other_rx, other_key) = SessionSender::observable("s-1");
        dispatcher
            .dispatch(
                other,
                request(99, "term.input", json!({ "term_id": "term-2" })),
            )
            .await;
        let overtaking = next_push(&mut other_rx, &other_key, Duration::from_millis(200)).await;
        assert_eq!(
            overtaking["id"], 99,
            "a different terminal runs concurrently"
        );

        for id in 0..12u64 {
            let answered = next_push(&mut rx, &key, Duration::from_secs(5)).await;
            assert_eq!(answered["id"], id, "responses come back in order");
        }
        let on_the_lane: Vec<u64> = seen
            .lock()
            .unwrap()
            .iter()
            .copied()
            .filter(|id| *id != 99)
            .collect();
        assert_eq!(
            on_the_lane,
            (0..12).collect::<Vec<u64>>(),
            "handlers ran in arrival order"
        );
    }

    /// A flood parks in a bounded queue: once it is full the dispatch call waits
    /// instead of growing memory without end.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_full_queue_makes_the_caller_wait() {
        let blocked = Arc::new(AtomicBool::new(true));
        let gate = blocked.clone();
        let handler: FrameHandler = Arc::new(move |_sender, frame| {
            while gate.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            json!({ "id": frame.payload["id"], "ok": true })
        });
        // One worker, two waiting slots: the fourth frame has nowhere to go.
        let mut dispatcher = Dispatcher::with_capacity(handler, 2, 1);
        let (sender, mut rx, key) = SessionSender::observable("s-flood");

        for id in 0..3u64 {
            tokio::time::timeout(
                Duration::from_millis(500),
                dispatcher.dispatch(sender.clone(), request(id, "board.list", json!({}))),
            )
            .await
            .expect("the worker and the queue take the first three");
        }
        assert!(
            tokio::time::timeout(
                Duration::from_millis(200),
                dispatcher.dispatch(sender.clone(), request(3, "board.list", json!({})))
            )
            .await
            .is_err(),
            "a full queue holds the caller instead of accepting without limit"
        );

        blocked.store(false, Ordering::SeqCst);
        tokio::time::timeout(
            Duration::from_secs(5),
            dispatcher.dispatch(sender, request(4, "board.list", json!({}))),
        )
        .await
        .expect("a drained queue takes frames again");
        let answered = next_push(&mut rx, &key, Duration::from_secs(5)).await;
        assert_eq!(answered["id"], 0, "the queued work still ran");
    }

    /// A terminal's lane lives as long as the terminal: the close still runs, and
    /// nothing is left behind for an id that will never be minted again.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_closed_terminal_leaves_no_lane_behind() {
        let (ran, mut ran_rx) = mpsc::unbounded_channel::<u64>();
        let handler: FrameHandler = Arc::new(move |_sender, frame| {
            let _ = ran.send(frame.payload["id"].as_u64().unwrap_or_default());
            json!({ "ok": true })
        });
        let mut dispatcher = Dispatcher::with_capacity(handler, 8, 2);
        let (sender, _rx, _key) = SessionSender::observable("s-term");

        dispatcher
            .dispatch(
                sender.clone(),
                request(1, "term.input", json!({ "term_id": "term-1" })),
            )
            .await;
        assert_eq!(dispatcher.lanes.len(), 1, "the terminal has a lane");

        dispatcher
            .dispatch(
                sender,
                request(2, "term.close", json!({ "term_id": "term-1" })),
            )
            .await;
        assert!(
            dispatcher.lanes.is_empty(),
            "the lane goes with the terminal"
        );

        let mut ran_ids = Vec::new();
        for _ in 0..2 {
            ran_ids.push(
                tokio::time::timeout(Duration::from_secs(5), ran_rx.recv())
                    .await
                    .expect("both frames ran")
                    .expect("the channel is open"),
            );
        }
        assert_eq!(ran_ids, vec![1, 2], "the close ran, after the input");
    }

    /// Handlers run on blocking threads now, and a handler that opens a terminal
    /// spawns its output pump onto the runtime (a pump that fails to start is
    /// silently a terminal with no output). The runtime must still be reachable
    /// from there.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_handler_can_still_spawn_onto_the_runtime() {
        let (spawned, mut spawned_rx) = mpsc::unbounded_channel::<&'static str>();
        let handler: FrameHandler = Arc::new(move |_sender, _frame| {
            let reachable = tokio::runtime::Handle::try_current().is_ok();
            if reachable {
                let spawned = spawned.clone();
                tokio::spawn(async move {
                    let _ = spawned.send("pump");
                });
            }
            json!({ "ok": reachable })
        });
        let mut dispatcher = Dispatcher::with_capacity(handler, 4, 2);

        let (sender, mut rx, key) = SessionSender::observable("s-pump");
        dispatcher
            .dispatch(sender, request(1, "term.create", json!({})))
            .await;

        let answered = next_push(&mut rx, &key, Duration::from_secs(5)).await;
        assert_eq!(
            answered["ok"], true,
            "the runtime is reachable from a handler"
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), spawned_rx.recv())
                .await
                .expect("the spawned task ran"),
            Some("pump")
        );
    }

    /// A session's close is its last word: it runs after the frames that arrived
    /// before it, so an attach can never register into an already-closed session.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_close_waits_for_the_frames_ahead_of_it() {
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = seen.clone();
        let handler: FrameHandler = Arc::new(move |_sender, frame| {
            if frame.frame_type == "close" {
                recorder.lock().unwrap().push("close".into());
                return json!({ "ok": true });
            }
            std::thread::sleep(Duration::from_millis(200));
            recorder
                .lock()
                .unwrap()
                .push(frame.payload["id"].to_string());
            json!({ "ok": true })
        });
        let mut dispatcher = Dispatcher::with_capacity(handler, 16, 4);

        let (sender, _rx, _key) = SessionSender::observable("s-closing");
        dispatcher
            .dispatch(
                sender.clone(),
                request(1, "term.attach", json!({ "term_id": "term-1" })),
            )
            .await;
        dispatcher.close_session("s-closing").await;

        for _ in 0..100 {
            if seen.lock().unwrap().len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["1".to_string(), "close".to_string()],
            "the close ran last"
        );
    }
}
