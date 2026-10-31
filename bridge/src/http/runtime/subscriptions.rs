//! Mounted SSE subscriptions: one application event stream per HTTP opening.
//!
//! `POST /v1/subscriptions` issues a handle, `GET .../events` binds the
//! opening's application `EventBinding` to a bounded SSE body, and `DELETE`
//! retires it. Resource requests name the handle in `X-Build-Subscription`.
//! There is no event ID or replay: reconnecting refreshes current state.
use std::sync::{Arc, Mutex};

use axum::{
    body::Body,
    extract::{Path, State},
    http::{
        header::{CACHE_CONTROL, CONTENT_TYPE},
        request::Parts,
        HeaderMap, HeaderValue, Request, StatusCode,
    },
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::json;

use super::{
    auth_refusal, bearer, body_refusal, exact_header, refusal, resource_opening,
    resource_session_id, HttpResult, HttpState, LIFECYCLE_BYTES,
};
use crate::execution::session::{
    DeliveryError, EncodedEvent, EventBinding, EventSink, RequestAuthorization, SessionError,
    SessionLease, StreamClass,
};
use crate::http::{
    listener::OutputAuthority,
    sse::{self, SseEvent, SseReceiver, SseSender},
    sse_stream,
    stream_slot::{RetireBinding, StreamError, StreamSlot},
};

pub(super) type Streams = StreamSlot<HttpStream>;

/// The installed application binding and the SSE queue it feeds.
#[derive(Clone)]
pub(super) struct HttpStream {
    binding: EventBinding,
    sender: SseSender,
}

impl RetireBinding for HttpStream {
    fn retire(&self) {
        self.binding.close();
        self.sender.close();
    }
}

/// Application pushes become `event` frames; the queue never waits on a socket.
struct SseSink(SseSender);

impl EventSink for SseSink {
    fn try_send(&self, event: EncodedEvent) -> Result<(), DeliveryError> {
        let event = SseEvent::json("event", event.payload()).map_err(delivery_error)?;
        self.0.try_send(event).map_err(delivery_error)
    }
}

fn delivery_error(error: sse::DeliveryError) -> DeliveryError {
    match error {
        sse::DeliveryError::Closed => DeliveryError::Closed,
        sse::DeliveryError::Full => DeliveryError::Full,
        sse::DeliveryError::TooLarge => DeliveryError::TooLarge,
        sse::DeliveryError::Unsupported => DeliveryError::InvalidPayload,
    }
}

/// A verified caller's own opening, resumed under this request's grant.
struct StreamOpening {
    session: SessionLease,
    authorization: RequestAuthorization,
    deadline: tokio::time::Instant,
    streams: Arc<Streams>,
    generation: String,
    output: OutputAuthority,
}

async fn stream_opening(state: &HttpState, request: &Parts) -> HttpResult<StreamOpening> {
    let bearer = bearer(&request.headers)
        .ok_or_else(|| refusal(StatusCode::UNAUTHORIZED, "invalid_credential"))?;
    let authority = state.verifier.verify(bearer).await.map_err(auth_refusal)?;
    authority
        .ensure_scope("bridge:read")
        .map_err(auth_refusal)?;
    let session_id = resource_session_id(&request.headers)?;
    let output = request
        .extensions
        .get::<OutputAuthority>()
        .cloned()
        .ok_or_else(|| refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    let opening = resource_opening(state, session_id, &authority)?;
    let deadline = opening
        .authorization
        .deadline()
        .ok_or_else(|| refusal(StatusCode::UNAUTHORIZED, "invalid_credential"))?;
    Ok(StreamOpening {
        session: opening.session,
        authorization: opening.authorization,
        deadline,
        streams: opening.streams,
        generation: opening.generation,
        output,
    })
}

impl StreamOpening {
    fn check(&self) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let authorization = self.authorization.clone();
        Arc::new(move || authorization.is_valid())
    }

    fn bind_output(
        &self,
        check: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> crate::http::listener::OutputTicket {
        let deadline = self.deadline;
        self.output.bind(
            check,
            Arc::new(move || deadline),
            self.session.opening().clone(),
        )
    }

    fn json(&self, status: StatusCode, value: serde_json::Value) -> Response {
        let encoded = serde_json::to_vec(&value).expect("lifecycle replies are plain JSON");
        let bytes = crate::http::body::authority_bytes(encoded, self.bind_output(self.check()));
        let mut response = Response::new(Body::from(bytes));
        *response.status_mut() = status;
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        response
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRequest {
    kind: SubscriptionKind,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum SubscriptionKind {
    Application,
}

pub(super) async fn create(State(state): State<HttpState>, request: Request<Body>) -> Response {
    created(&state, request)
        .await
        .unwrap_or_else(|error| *error)
}

async fn created(state: &HttpState, request: Request<Body>) -> HttpResult<Response> {
    let (parts, body) = request.into_parts();
    let opening = stream_opening(state, &parts).await?;
    let predecessor = subscription_header(&parts.headers)?.map(str::to_owned);
    let check = opening.check();
    let body =
        crate::http::body::read_json_authorized(body, LIFECYCLE_BYTES, opening.deadline, || {
            check()
        })
        .await
        .map_err(body_refusal)?;
    let CreateRequest {
        kind: SubscriptionKind::Application,
    } = serde_json::from_value(body)
        .map_err(|_| refusal(StatusCode::BAD_REQUEST, "invalid_request"))?;
    let subscription_id = opening
        .streams
        .replace(predecessor.as_deref())
        .map_err(stream_refusal)?;
    Ok(opening.json(
        StatusCode::CREATED,
        json!({
            "subscription_id": subscription_id,
            "opening_generation": opening.generation,
            "events_path": format!("/v1/subscriptions/{subscription_id}/events"),
            "durable": false,
        }),
    ))
}

pub(super) async fn events(
    Path(subscription_id): Path<String>,
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response {
    streamed(&state, subscription_id, request)
        .await
        .unwrap_or_else(|error| *error)
}

async fn streamed(
    state: &HttpState,
    subscription_id: String,
    request: Request<Body>,
) -> HttpResult<Response> {
    let (parts, _) = request.into_parts();
    let opening = stream_opening(state, &parts).await?;
    if parts.headers.contains_key("Last-Event-ID") {
        return Err(refusal(StatusCode::BAD_REQUEST, "cursor_unsupported").into());
    }
    let reservation = state
        .buffers
        .reserve_stream()
        .map_err(|_| refusal(StatusCode::TOO_MANY_REQUESTS, "overloaded"))?;
    let (sender, receiver) = sse::channel();
    // Queued before the binding exists, so no application push precedes it.
    sender
        .try_send(hello(&subscription_id, &opening.generation))
        .map_err(|_| refusal(StatusCode::INTERNAL_SERVER_ERROR, "internal"))?;
    let stream = bind_stream(state, &opening, &subscription_id, sender)?;
    Ok(event_response(
        opening,
        subscription_id,
        stream,
        receiver,
        reservation,
    ))
}

fn hello(subscription_id: &str, generation: &str) -> SseEvent {
    SseEvent::json(
        "hello",
        &json!({
            "durable": false,
            "recovery": "current_state_refresh",
            "subscription_id": subscription_id,
            "opening_generation": generation,
            "bridge_epoch": crate::bridge_epoch::current(),
        }),
    )
    .expect("the hello frame is small and named")
}

/// Why a GET could not install its binding.
enum BindRefusal {
    Slot(StreamError),
    Session(SessionError),
}

impl From<StreamError> for BindRefusal {
    fn from(error: StreamError) -> Self {
        Self::Slot(error)
    }
}

/// Installs the binding while this handle is current and unconsumed.
fn bind_stream(
    state: &HttpState,
    opening: &StreamOpening,
    subscription_id: &str,
    sender: SseSender,
) -> HttpResult<HttpStream> {
    let sessions = state.execution.sessions();
    let bound = opening.streams.bind(subscription_id, |predecessor| {
        sessions
            .replace_events(
                &opening.session,
                StreamClass::Application,
                predecessor.map(|stream| &stream.binding),
                Arc::new(SseSink(sender.clone())),
            )
            .map(|binding| HttpStream {
                binding,
                sender: sender.clone(),
            })
            .map_err(BindRefusal::Session)
    });
    bound.map_err(|refused| match refused {
        BindRefusal::Slot(error) => stream_refusal(error),
        BindRefusal::Session(SessionError::StaleBinding) => stream_refusal(StreamError::Replaced),
        BindRefusal::Session(_) => refusal(StatusCode::UNAUTHORIZED, "invalid_credential").into(),
    })
}

fn event_response(
    opening: StreamOpening,
    subscription_id: String,
    stream: HttpStream,
    receiver: SseReceiver,
    reservation: crate::http::limits::BufferReservation,
) -> Response {
    let authorized = opening.check();
    let binding = stream.binding.clone();
    let check: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::new(move || authorized() && binding.is_live());
    let ticket = Mutex::new(Some(opening.bind_output(check.clone())));
    let streams = opening.streams.clone();
    let close = Arc::new(move || {
        streams.release(&subscription_id);
        if let Some(ticket) = ticket.lock().unwrap().take() {
            ticket.release();
        }
    });
    let deadline = opening.deadline;
    let body = sse_stream::application_body(
        stream.sender,
        receiver,
        Arc::new(reservation),
        check,
        Arc::new(move || deadline),
        close,
    );
    let mut response = Response::new(body);
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub(super) async fn delete(
    Path(subscription_id): Path<String>,
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response {
    deleted(&state, &subscription_id, request)
        .await
        .unwrap_or_else(|error| *error)
}

async fn deleted(
    state: &HttpState,
    subscription_id: &str,
    request: Request<Body>,
) -> HttpResult<Response> {
    let (parts, _) = request.into_parts();
    let opening = stream_opening(state, &parts).await?;
    opening
        .streams
        .delete(subscription_id)
        .map_err(stream_refusal)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// The binding a resource request captures; required methods need a live one.
pub(super) fn resource_events(
    streams: &Streams,
    headers: &HeaderMap,
    requires_events: bool,
) -> HttpResult<Option<EventBinding>> {
    match subscription_header(headers)? {
        None if requires_events => Err(stream_refusal(StreamError::Required)),
        None => Ok(None),
        Some(handle) => streams
            .binding(Some(handle))
            .ok()
            .filter(|stream| stream.binding.is_live())
            .map(|stream| Some(stream.binding))
            .ok_or_else(|| stream_refusal(StreamError::Replaced)),
    }
}

fn subscription_header(headers: &HeaderMap) -> HttpResult<Option<&str>> {
    match exact_header(headers, "X-Build-Subscription") {
        Ok(None) => Ok(None),
        Ok(Some(handle)) if !handle.is_empty() && handle.len() <= 128 => Ok(Some(handle)),
        _ => Err(refusal(StatusCode::BAD_REQUEST, "invalid_request").into()),
    }
}

fn stream_refusal(error: StreamError) -> Box<Response> {
    let code = match error {
        StreamError::Required => "stream_required",
        StreamError::Replaced | StreamError::Consumed => "stream_replaced",
    };
    refusal(StatusCode::CONFLICT, code).into()
}
