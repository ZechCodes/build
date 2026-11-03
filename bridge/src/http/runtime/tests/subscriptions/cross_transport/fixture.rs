use super::*;
use crate::carrier::{testing, CarrierHandle, FrameIntake, OutboundEnvelope, SessionSender};
use crate::http::{limits::TOTAL_BUFFER_BYTES, listener::OutputAuthority, sse::MAX_QUEUED_BYTES};
use crate::transport;
use serde_json::Value;
use tokio::sync::mpsc;

pub(super) const RTC_SESSIONS: usize = 4;
pub(super) const POOL_STREAMS: usize = STREAM_BUFFER_BYTES / MAX_QUEUED_BYTES;
pub(super) const WATCHDOG: Duration = Duration::from_secs(10);

pub(super) struct RtcClient {
    pub carrier: CarrierHandle,
    pub outbound: mpsc::UnboundedReceiver<OutboundEnvelope>,
    pub id: String,
    pub key: String,
}

impl RtcClient {
    pub fn open(intake: &FrameIntake, index: usize) -> Self {
        let (carrier, outbound) = CarrierHandle::open_channel();
        let id = format!("acceptance-rtc-{index}");
        let key = transport::generate_session_key();
        intake
            .open(
                &id,
                &testing::session_init(&id, intake.transport_public_key(), &key),
                &carrier,
            )
            .unwrap();
        Self {
            carrier,
            outbound,
            id,
            key,
        }
    }

    pub async fn send(
        &self,
        intake: &FrameIntake,
        id: usize,
    ) -> Result<(), crate::carrier::CarrierError> {
        intake
            .accept(
                testing::client_request(
                    &self.key,
                    &self.id,
                    "data",
                    json!({"id": id, "method": "session.hello", "params": {}}),
                ),
                &self.carrier,
            )
            .await
    }

    pub async fn reply(&mut self, id: usize) {
        tokio::time::timeout(WATCHDOG, async {
            loop {
                let output = self
                    .outbound
                    .recv()
                    .await
                    .expect("the channel remains open");
                if matches!(output, OutboundEnvelope::SessionAccept { .. }) {
                    continue;
                }
                let value = SessionSender::decrypt_push(&self.key, &output);
                if value["id"] == id && value.get("ok").is_some() {
                    assert_eq!(value["ok"], true, "{value}");
                    return;
                }
            }
        })
        .await
        .expect("WebRTC makes progress beside the full SSE pool");
    }
}

pub(super) struct HttpClient {
    pub family: String,
    pub session: String,
    pub subscription: String,
    pub events: Frames,
    pub output: OutputAuthority,
}

pub(super) struct Fixture {
    pub intake: Arc<FrameIntake>,
    pub runtime: HttpRuntime,
    pub app: Router,
    pub live: Arc<AtomicBool>,
    pub rtc: Vec<RtcClient>,
    pub http: Vec<HttpClient>,
}

impl Fixture {
    pub fn new(handler: FrameHandler) -> Self {
        let intake = FrameIntake::new(handler, transport::generate_transport_keypair());
        let live = Arc::new(AtomicBool::new(true));
        let runtime = runtime_with(
            intake.execution(),
            Arc::new(Families { live: live.clone() }),
        );
        let app = runtime.router();
        let rtc = (0..RTC_SESSIONS)
            .map(|i| RtcClient::open(&intake, i))
            .collect();
        Self {
            intake,
            runtime,
            app,
            live,
            rtc,
            http: Vec::new(),
        }
    }

    pub async fn fill_stream_pool(&mut self) {
        for i in self.http.len()..POOL_STREAMS {
            let family = format!("family-{}", i / MAX_FAMILY_STREAMS);
            self.http.push(self.http_client(&family).await);
        }
        assert_eq!(self.runtime.state.buffers.streams_available(), 0);
        assert_eq!(self.runtime.state.buffers.available(), TOTAL_BUFFER_BYTES);
    }

    pub async fn http_client(&self, family: &str) -> HttpClient {
        let request = open_request_with_token(Body::from(r#"{"api_range":">=3 <4"}"#), family);
        let (status, opening) =
            json_response(self.app.clone().oneshot(request).await.unwrap()).await;
        assert_eq!(status, StatusCode::CREATED, "{opening}");
        let session = opening["session_id"].as_str().unwrap().to_owned();
        let request = request_as(
            family,
            "POST",
            "/v1/subscriptions",
            &session,
            Some(r#"{"kind":"application"}"#),
        );
        let (status, created) =
            json_response(self.app.clone().oneshot(request).await.unwrap()).await;
        assert_eq!(status, StatusCode::CREATED, "{created}");
        let subscription = created["subscription_id"].as_str().unwrap().to_owned();
        let request = request_as(
            family,
            "GET",
            created["events_path"].as_str().unwrap(),
            &session,
            None,
        );
        let output = request
            .extensions()
            .get::<OutputAuthority>()
            .unwrap()
            .clone();
        let response = self.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut events: Frames = Box::pin(response.into_body().into_data_stream());
        assert_eq!(next_event(&mut events).await.0, "hello");
        HttpClient {
            family: family.to_owned(),
            session,
            subscription,
            events,
            output,
        }
    }

    pub fn greeting(&self, client: &HttpClient) -> Request<Body> {
        let mut request = request_as(
            &client.family,
            "POST",
            "/v1/session-greetings",
            &client.session,
            Some("{}"),
        );
        request
            .headers_mut()
            .insert("X-Build-Subscription", client.subscription.parse().unwrap());
        request
    }

    pub async fn refused_stream(&self, family: &str) {
        let request = open_request_with_token(Body::from(r#"{"api_range":">=3 <4"}"#), family);
        let (status, opening) =
            json_response(self.app.clone().oneshot(request).await.unwrap()).await;
        assert_eq!(status, StatusCode::CREATED);
        let session = opening["session_id"].as_str().unwrap();
        let request = request_as(
            family,
            "POST",
            "/v1/subscriptions",
            session,
            Some(r#"{"kind":"application"}"#),
        );
        let (_, created) = json_response(self.app.clone().oneshot(request).await.unwrap()).await;
        let request = request_as(
            family,
            "GET",
            created["events_path"].as_str().unwrap(),
            session,
            None,
        );
        let response = self.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let (status, error) = json_response(response).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{error}");
        assert_eq!(error["error_code"], "overloaded");
        assert_eq!(error["retryable"], true);
    }

    pub fn close_rtc(&self) {
        for client in &self.rtc {
            self.intake.close_carrier(&client.carrier);
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.close_rtc();
        self.runtime.shutdown();
    }
}

pub(super) fn request_as(
    family: &str,
    method: &str,
    path: &str,
    session: &str,
    body: Option<&str>,
) -> Request<Body> {
    let mut request = lifecycle(method, path, session, body);
    request
        .headers_mut()
        .insert(AUTHORIZATION, format!("Bearer {family}").parse().unwrap());
    request
}

pub(super) async fn json_response(response: Response) -> (StatusCode, Value) {
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

/// A successful socket flush releases the exact response's retained call and
/// buffer reservation, just as Hyper does after handing its bytes to TLS.
pub(super) async fn flush(output: OutputAuthority) {
    use tokio::io::AsyncWriteExt;
    let mut io = crate::http::listener::BoundedIo::new(tokio::io::sink(), output);
    io.flush().await.unwrap();
}

pub(super) fn handler() -> FrameHandler {
    FrameHandler::new(FrameClock::new(), |sender, frame, _| {
        sender.push(json!({"type": "probe.cross_transport", "seq": frame.payload["id"]}));
        json!({"id": frame.payload["id"], "ok": true, "result": {}})
    })
    .with_greeting_reads(
        |epoch| json!({"api_version": crate::api::API_VERSION, "bridge_epoch": epoch}),
    )
}
