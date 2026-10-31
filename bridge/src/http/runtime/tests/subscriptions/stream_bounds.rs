//! Stream bounds end to end on the production TLS listener: a client that
//! stops reading is cut off and its memory returned, without costing another
//! client a push or the bridge its admission bound.
use super::*;
use crate::http::{
    limits::{BufferBudget, STREAM_BUFFER_BYTES},
    sse::{MAX_EVENT_BYTES, MAX_QUEUED_BYTES, STALLED_WRITE_TIMEOUT},
};
use std::collections::HashMap;
use std::time::Duration;

/// Room past each bound for a loaded gate machine; never the expected time.
const SETTLE: Duration = Duration::from_secs(30);
/// Streams the stream pool admits at once.
const STREAMS: usize = 16;
/// Small enough that a draining socket never lets the queue back up.
const PAD: usize = 64 * 1024;

/// What each push request queues: `count` numbered events of `pad` bytes.
#[derive(Default)]
struct Pushes {
    pad: AtomicUsize,
    count: AtomicUsize,
    seq: AtomicUsize,
}

impl Pushes {
    fn set(&self, pad: usize, count: usize) {
        self.pad.store(pad, Ordering::SeqCst);
        self.count.store(count, Ordering::SeqCst);
    }
}

fn padded_execution(pushes: Arc<Pushes>) -> Arc<ExecutionService> {
    ExecutionService::new(
        FrameHandler::new(FrameClock::new(), move |context, frame, _| {
            let pad = "x".repeat(pushes.pad.load(Ordering::SeqCst));
            let count = pushes.count.load(Ordering::SeqCst);
            let first = pushes.seq.fetch_add(count, Ordering::SeqCst);
            for seq in first..first + count {
                context.push(json!({"type": "probe.padded", "seq": seq, "pad": pad}));
            }
            json!({"id": frame.payload["id"], "ok": true, "result": {"seq": first}})
        })
        .with_greeting_reads(
            |epoch| json!({"api_version": crate::api::API_VERSION, "bridge_epoch": epoch}),
        ),
    )
}

/// Each bearer is its own family, so one test can hold more openings than
/// the per-family bound allows.
struct BearerFamilies;

#[async_trait]
impl GrantVerifier for BearerFamilies {
    async fn verify(&self, bearer: &str) -> Result<VerifiedAuthority, AuthError> {
        let mut authority = authority();
        authority.identity.family_id = bearer.into();
        Ok(authority)
    }

    async fn revalidate(&self, bearer: &str) -> Result<VerifiedAuthority, AuthError> {
        self.verify(bearer).await
    }
}

/// The mounted listener, with the runtime state the tests watch.
struct Bounds {
    mounted: MountedTls,
    buffers: Arc<BufferBudget>,
    openings: Arc<Mutex<HashMap<String, super::super::super::MountedOpening>>>,
    pushes: Arc<Pushes>,
    idle: usize,
}

/// One client's opening and the subscription it streams.
struct Client {
    bearer: String,
    session_id: String,
    subscription: String,
    events_path: String,
}

async fn bounds() -> Bounds {
    let pushes = Arc::new(Pushes::default());
    let runtime = runtime_with(padded_execution(pushes.clone()), Arc::new(BearerFamilies));
    let buffers = runtime.state.buffers.clone();
    let openings = runtime.state.openings.clone();
    Bounds {
        idle: buffers.available(),
        mounted: mounted_tls(runtime).await,
        buffers,
        openings,
        pushes,
    }
}

impl Bounds {
    fn request_as(
        &self,
        bearer: &str,
        method: reqwest::Method,
        path: &str,
    ) -> reqwest::RequestBuilder {
        self.mounted
            .client
            .request(method, format!("{}{path}", self.mounted.base))
            .header("origin", "https://app.example")
            .bearer_auth(bearer)
    }

    async fn client(&self, bearer: &str) -> Client {
        let opened: serde_json::Value = self
            .request_as(bearer, reqwest::Method::POST, "/v1/sessions")
            .json(&json!({"api_range": ">=3 <4"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let session_id = opened["session_id"].as_str().unwrap().to_owned();
        let created: serde_json::Value = self
            .request_as(bearer, reqwest::Method::POST, "/v1/subscriptions")
            .header("X-Build-Session", &session_id)
            .json(&json!({"kind": "application"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        Client {
            bearer: bearer.to_owned(),
            session_id,
            subscription: created["subscription_id"].as_str().unwrap().to_owned(),
            events_path: created["events_path"].as_str().unwrap().to_owned(),
        }
    }

    async fn events(&self, client: &Client) -> reqwest::Response {
        self.request_as(&client.bearer, reqwest::Method::GET, &client.events_path)
            .header("X-Build-Session", &client.session_id)
            .send()
            .await
            .unwrap()
    }

    /// The client's stream, past its hello.
    async fn stream(&self, client: &Client) -> SocketEvents {
        let response = self.events(client).await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut events = SocketEvents {
            response,
            buffered: String::new(),
        };
        assert_eq!(events.next().await.expect("hello").0, "hello");
        events
    }

    async fn push(&self, client: &Client) -> (StatusCode, serde_json::Value) {
        let response = self
            .request_as(
                &client.bearer,
                reqwest::Method::POST,
                "/v1/session-greetings",
            )
            .header("X-Build-Session", &client.session_id)
            .header("X-Build-Subscription", &client.subscription)
            .json(&json!({}))
            .send()
            .await
            .unwrap();
        (response.status(), response.json().await.unwrap())
    }

    /// Events and bytes waiting in the client's queue while its stream lives.
    fn queued(&self, client: &Client) -> Option<(usize, usize)> {
        let openings = self.openings.lock().unwrap();
        let stream = openings
            .get(&client.session_id)?
            .streams
            .binding(Some(&client.subscription))
            .ok()?;
        Some(stream.queued())
    }

    /// Pushes padded events at a client that reads nothing until half its
    /// queue waits: more than hyper holds, so the socket write is pending.
    async fn stall(&self, client: &Client) {
        self.pushes.set(PAD, 2);
        for _ in 0..1024 {
            let (status, reply) = self.push(client).await;
            assert_eq!(status, StatusCode::OK, "{}", reply["error_code"]);
            let (_, bytes) = self.queued(client).expect("the stream lives until stalled");
            if bytes >= MAX_QUEUED_BYTES / 2 {
                return;
            }
        }
        panic!("the unread socket never backed up its queue");
    }

    /// Polls runtime state until `condition`, within the stalled-write
    /// timeout plus a loaded machine's margin.
    async fn settled(&self, what: &str, condition: impl Fn(&Self) -> bool) {
        let give_up = tokio::time::Instant::now() + STALLED_WRITE_TIMEOUT + SETTLE;
        while !condition(self) {
            assert!(tokio::time::Instant::now() < give_up, "{what}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Every stream's queue is back in the pool, and RPCs never paid for one.
    fn released(&self) -> bool {
        self.buffers.streams_available() == STREAM_BUFFER_BYTES
            && self.buffers.available() == self.idle
    }
}

/// Reads a stalled client's socket to its end: it must end, not trickle.
async fn drained(response: reqwest::Response) {
    let mut response = response;
    tokio::time::timeout(SETTLE, async {
        while let Ok(Some(_)) = response.chunk().await {}
    })
    .await
    .expect("the cut socket ends once its kernel buffers drain");
}

#[tokio::test]
async fn a_client_that_stops_reading_is_retired_when_its_queue_fills() {
    let bounds = bounds().await;
    let client = bounds.client("family-a").await;
    let stalled = bounds.stream(&client).await;
    assert!(
        bounds.buffers.streams_available() <= STREAM_BUFFER_BYTES - MAX_QUEUED_BYTES,
        "the stream holds its queue's reservation"
    );
    bounds.stall(&client).await;

    let mut refused = None;
    for _ in 0..64 {
        let (status, reply) = bounds.push(&client).await;
        if status != StatusCode::OK {
            refused = Some((status, reply));
            break;
        }
    }

    let (status, reply) = refused.expect("a full queue retires the binding");
    assert_eq!(status, StatusCode::CONFLICT, "{}", reply["error_code"]);
    assert_eq!(reply["error_code"], "stream_replaced");
    bounds
        .settled(
            "the retired stream returns its reservation",
            Bounds::released,
        )
        .await;
    drained(stalled.response).await;
    bounds.mounted.shutdown().await;
}

#[tokio::test]
async fn a_stalled_socket_write_is_cut_and_its_reservation_returned() {
    let bounds = bounds().await;
    let client = bounds.client("family-a").await;
    let stalled = bounds.stream(&client).await;
    bounds.stall(&client).await;

    bounds
        .settled("the stalled write is cut", Bounds::released)
        .await;

    let (status, reply) = bounds.push(&client).await;
    assert_eq!(status, StatusCode::CONFLICT, "{}", reply["error_code"]);
    assert_eq!(reply["error_code"], "stream_replaced");
    drained(stalled.response).await;
    bounds.mounted.shutdown().await;
}

#[tokio::test]
async fn another_client_gets_every_push_in_order_while_one_is_stalled() {
    let bounds = bounds().await;
    let stalled = bounds.client("family-a").await;
    let _stalled_events = bounds.stream(&stalled).await;
    bounds.stall(&stalled).await;
    let reader = bounds.client("family-b").await;
    let mut events = bounds.stream(&reader).await;

    bounds.pushes.set(0, 1);
    let mut sent = Vec::new();
    for _ in 0..50 {
        let (status, reply) = bounds.push(&reader).await;
        assert_eq!(status, StatusCode::OK, "{}", reply["error_code"]);
        sent.push(reply["result"]["seq"].clone());
    }

    let mut received = Vec::new();
    while received.len() < sent.len() {
        let (name, event) = events.next().await.expect("every push arrives");
        assert_eq!(
            (name.as_str(), &event["type"]),
            ("event", &json!("probe.padded"))
        );
        received.push(event["seq"].clone());
    }
    assert_eq!(received, sent, "every push, in order, none lost");
    bounds.mounted.shutdown().await;
}

#[tokio::test]
async fn stream_admission_stops_at_its_own_pool_and_recovers_after_a_cut() {
    let bounds = bounds().await;
    let mut healthy = Vec::new();
    for family in 0..STREAMS - 1 {
        let client = bounds.client(&format!("family-{family}")).await;
        let events = bounds.stream(&client).await;
        healthy.push((client, events));
    }
    let stalled = bounds.client("family-stalled").await;
    let _stalled_events = bounds.stream(&stalled).await;
    bounds.stall(&stalled).await;
    let waiting = bounds.client("family-waiting").await;

    let refused = bounds.events(&waiting).await;
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    let refused: serde_json::Value = refused.json().await.unwrap();
    assert_eq!(refused["error_code"], "overloaded");
    assert_eq!(refused["retryable"], true);
    assert_eq!(
        bounds.buffers.available(),
        bounds.idle,
        "streams never draw on the RPC budget"
    );
    bounds.pushes.set(0, 1);
    for (reader, events) in &mut healthy {
        let (status, reply) = bounds.push(reader).await;
        assert_eq!(status, StatusCode::OK, "{}", reply["error_code"]);
        let (_, pushed) = events
            .next()
            .await
            .expect("a healthy stream still delivers");
        assert_eq!(pushed["seq"], reply["result"]["seq"]);
    }

    bounds
        .settled("the cut stream returns its share of the pool", |bounds| {
            bounds.buffers.streams_available() >= MAX_QUEUED_BYTES
        })
        .await;
    let _admitted = bounds.stream(&waiting).await;
    bounds.mounted.shutdown().await;
}

#[tokio::test]
async fn an_oversized_push_retires_only_its_own_stream() {
    let bounds = bounds().await;
    let client = bounds.client("family-a").await;
    let mut events = bounds.stream(&client).await;

    bounds.pushes.set(MAX_EVENT_BYTES, 1);
    let (status, reply) = bounds.push(&client).await;
    assert_eq!(status, StatusCode::OK, "{}", reply["error_code"]);

    assert!(
        events.next().await.is_none(),
        "the oversized event ends the stream"
    );
    bounds
        .settled("the ended stream returns its reservation", Bounds::released)
        .await;
    let (status, reply) = bounds.push(&client).await;
    assert_eq!(status, StatusCode::CONFLICT, "{}", reply["error_code"]);
    assert_eq!(reply["error_code"], "stream_replaced");

    let replaced: serde_json::Value = bounds
        .request_as(&client.bearer, reqwest::Method::POST, "/v1/subscriptions")
        .header("X-Build-Session", &client.session_id)
        .header("X-Build-Subscription", &client.subscription)
        .json(&json!({"kind": "application"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let successor = Client {
        subscription: replaced["subscription_id"].as_str().unwrap().to_owned(),
        events_path: replaced["events_path"].as_str().unwrap().to_owned(),
        ..client
    };
    let mut events = bounds.stream(&successor).await;
    bounds.pushes.set(0, 1);
    let (status, reply) = bounds.push(&successor).await;
    assert_eq!(status, StatusCode::OK, "{}", reply["error_code"]);
    assert_eq!(
        events.next().await.expect("the opening streams on").1["seq"],
        reply["result"]["seq"]
    );
    bounds.mounted.shutdown().await;
}

/// Pushes per latency sample set; together they stay well inside the stalled
/// client's `STALLED_WRITE_TIMEOUT`, so it is still stalled at the end.
const LATENCY_SAMPLES: usize = 200;

/// Push-to-event latency on `client`'s open stream, sorted.
async fn push_latencies(
    bounds: &Bounds,
    client: &Client,
    events: &mut SocketEvents,
) -> Vec<Duration> {
    let mut latencies = Vec::with_capacity(LATENCY_SAMPLES);
    for _ in 0..LATENCY_SAMPLES {
        let started = std::time::Instant::now();
        let (status, reply) = bounds.push(client).await;
        assert_eq!(status, StatusCode::OK, "{}", reply["error_code"]);
        let (_, event) = events.next().await.expect("every push arrives");
        latencies.push(started.elapsed());
        assert_eq!(event["seq"], reply["result"]["seq"]);
    }
    latencies.sort();
    latencies
}

fn percentile(sorted: &[Duration], percent: usize) -> Duration {
    sorted[(sorted.len() * percent / 100).min(sorted.len() - 1)]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "timing: release-profile isolated latency gate; see timing.yml"]
async fn stalled_stream_leaves_push_latency_p50_p95() {
    let bounds = bounds().await;
    let reader = bounds.client("family-reader").await;
    let mut events = bounds.stream(&reader).await;
    bounds.pushes.set(0, 1);
    let alone = push_latencies(&bounds, &reader, &mut events).await;

    let stalled = bounds.client("family-stalled").await;
    let _stalled_events = bounds.stream(&stalled).await;
    bounds.stall(&stalled).await;
    bounds.pushes.set(0, 1);
    let beside = push_latencies(&bounds, &reader, &mut events).await;
    assert!(
        bounds.queued(&stalled).is_some(),
        "the stalled stream was still open for every sample"
    );

    let (alone_p50, alone_p95) = (percentile(&alone, 50), percentile(&alone, 95));
    let (beside_p50, beside_p95) = (percentile(&beside, 50), percentile(&beside, 95));
    println!("push->event without a stalled stream: p50 {alone_p50:?} p95 {alone_p95:?}");
    println!("push->event beside a stalled stream:  p50 {beside_p50:?} p95 {beside_p95:?}");
    let ceiling = alone_p95 * 2 + Duration::from_millis(20);
    assert!(
        beside_p95 <= ceiling,
        "a stalled stream slowed another's p95 to {beside_p95:?}, ceiling {ceiling:?}"
    );
    bounds.mounted.shutdown().await;
}
