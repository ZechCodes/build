//! Production HTTP mount over the daemon's one shared execution service.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{
        header::{ALLOW, AUTHORIZATION, CONTENT_ENCODING, CONTENT_TYPE},
        HeaderMap, HeaderValue, Request, StatusCode,
    },
    response::{IntoResponse, Response},
    routing::{delete, post},
    Router,
};
use serde::Deserialize;
use serde_json::json;
use tokio::time::Instant;

use super::auth::{AuthError, HttpAuthVerifier, VerifiedHttpGrant};
use crate::execution::{
    session::{
        DeliveryError, EncodedEvent, HttpIdentity, ReplySink, RequestAuthorization, SessionLease,
        VerifiedCaller,
    },
    ExecutionService, Invocation, RequestMetadata,
};

const LIFECYCLE_BYTES: usize = 64 * 1024;
const MAX_FAMILY_OPENINGS: usize = 16;
const MAX_OPENINGS: usize = 1024;

#[derive(Clone)]
struct VerifiedAuthority {
    identity: HttpIdentity,
    deadline: Instant,
    live: Arc<dyn Fn() -> bool + Send + Sync>,
    scopes: Arc<[String]>,
}

impl VerifiedAuthority {
    fn from_grant(grant: VerifiedHttpGrant) -> Self {
        let binding = grant.binding();
        let identity = HttpIdentity {
            issuer: binding.issuer().into(),
            account_id: binding.account_id().into(),
            device_id: binding.device_id().into(),
            family_id: binding.family_id().into(),
            origin: binding.origin().into(),
            endpoint_epoch: binding.endpoint_epoch().into(),
            authorization_epoch: binding.authorization_epoch(),
        };
        let scopes = binding.scopes().to_vec().into();
        let deadline = grant.deadline();
        let retained = grant.clone();
        let live = Arc::new(move || retained.check().is_ok());
        Self {
            identity,
            deadline,
            live,
            scopes,
        }
    }

    fn ensure_scope(&self, required: &str) -> Result<(), AuthError> {
        if !(self.live)() || Instant::now() >= self.deadline {
            return Err(AuthError::Expired);
        }
        self.scopes
            .iter()
            .any(|scope| scope == required)
            .then_some(())
            .ok_or(AuthError::ForbiddenScope)
    }
}

#[async_trait]
trait GrantVerifier: Send + Sync {
    async fn verify(&self, bearer: &str) -> Result<VerifiedAuthority, AuthError>;
    fn close(&self) {}
}

#[async_trait]
impl GrantVerifier for HttpAuthVerifier {
    async fn verify(&self, bearer: &str) -> Result<VerifiedAuthority, AuthError> {
        self.verify(bearer).await.map(VerifiedAuthority::from_grant)
    }

    fn close(&self) {
        HttpAuthVerifier::close(self);
    }
}

#[derive(Clone)]
struct HttpState {
    execution: Arc<ExecutionService>,
    verifier: Arc<dyn GrantVerifier>,
    openings: Arc<Mutex<HashMap<String, MountedOpening>>>,
    buffers: Arc<super::limits::BufferBudget>,
    #[cfg(test)]
    output_observer: Arc<Mutex<Option<OutputObserver>>>,
}

#[cfg(test)]
type OutputObserver = Arc<dyn Fn(super::listener::OutputAuthority) + Send + Sync>;

#[derive(Clone)]
pub struct HttpRuntime {
    state: HttpState,
}

struct MountedOpening {
    session: SessionLease,
    _generation: String,
    authority: VerifiedAuthority,
    budget: Arc<super::limits::OpeningBudget>,
}

struct ResourceReplySink {
    sender: Mutex<Option<tokio::sync::oneshot::Sender<EncodedEvent>>>,
    _reservation: Arc<super::limits::BufferReservation>,
}

impl ReplySink for ResourceReplySink {
    fn try_send(&self, event: EncodedEvent) -> Result<(), DeliveryError> {
        self.sender
            .lock()
            .unwrap()
            .take()
            .ok_or(DeliveryError::Closed)?
            .send(event)
            .map_err(|_| DeliveryError::Closed)
    }
}

impl HttpState {
    fn reap_ended(&self) -> usize {
        let ended = {
            let openings = self.openings.lock().unwrap();
            openings
                .iter()
                .filter(|(_, opening)| !opening.session.is_live())
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>()
        };
        let removed = {
            let mut openings = self.openings.lock().unwrap();
            ended
                .into_iter()
                .filter_map(|id| openings.remove(&id))
                .collect::<Vec<_>>()
        };
        let count = removed.len();
        for opening in removed {
            self.execution.sessions().close(&opening.session);
        }
        count
    }

    fn retire_all(&self) -> usize {
        let removed = {
            let mut openings = self.openings.lock().unwrap();
            openings
                .drain()
                .map(|(_, opening)| opening)
                .collect::<Vec<_>>()
        };
        let count = removed.len();
        for opening in removed {
            self.execution.sessions().close(&opening.session);
        }
        count
    }
}

/// Mount lifecycle routes before the explicit resource resolver.
pub fn router(execution: Arc<ExecutionService>, verifier: HttpAuthVerifier) -> Router {
    mount(execution, verifier).router()
}

pub fn mount(execution: Arc<ExecutionService>, verifier: HttpAuthVerifier) -> HttpRuntime {
    runtime_with(execution, Arc::new(verifier))
}

impl HttpRuntime {
    pub fn router(&self) -> Router {
        Router::new()
            .route("/v1/sessions", post(open_session))
            .route("/v1/sessions/{session_id}", delete(close_session))
            .fallback(resource_request)
            .with_state(self.state.clone())
    }

    pub fn shutdown(&self) -> usize {
        self.state.verifier.close();
        self.state.retire_all()
    }

    #[cfg(test)]
    fn opening_count(&self) -> usize {
        self.state.openings.lock().unwrap().len()
    }
}

fn runtime_with(execution: Arc<ExecutionService>, verifier: Arc<dyn GrantVerifier>) -> HttpRuntime {
    HttpRuntime {
        state: HttpState {
            execution,
            verifier,
            openings: Arc::new(Mutex::new(HashMap::new())),
            buffers: Arc::new(super::limits::BufferBudget::default()),
            #[cfg(test)]
            output_observer: Arc::new(Mutex::new(None)),
        },
    }
}

async fn resource_request(State(state): State<HttpState>, request: Request<Body>) -> Response {
    match resource_response(&state, request).await {
        Ok(response) | Err(response) => response,
    }
}

type HttpResult<T> = Result<T, Response>;

async fn resource_response(state: &HttpState, request: Request<Body>) -> HttpResult<Response> {
    let bearer = bearer(request.headers())
        .ok_or_else(|| refusal(StatusCode::UNAUTHORIZED, "invalid_credential"))?;
    let authority = state.verifier.verify(bearer).await.map_err(auth_refusal)?;
    authority
        .ensure_scope("bridge:read")
        .map_err(auth_refusal)?;
    let operation = super::routes::operation(
        request.method().as_str(),
        request.uri().path(),
        request.uri().query(),
    )
    .map_err(route_refusal)?;
    authorize_resource(&authority, operation, request.headers())?;
    let session_id = resource_session_id(request.headers())?;
    let output = request
        .extensions()
        .get::<super::listener::OutputAuthority>()
        .cloned()
        .ok_or_else(|| refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    observe_output(state, &output);
    let (opening_budget, session, request_authorization) =
        resource_opening(state, session_id, &authority)?;
    let reservation = Arc::new(
        state
            .buffers
            .reserve(
                operation.request_max_bytes,
                operation.response_max_bytes,
                &opening_budget,
            )
            .map_err(|_| refusal(StatusCode::TOO_MANY_REQUESTS, "overloaded"))?,
    );
    let verb = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    let query = request.uri().query().map(str::to_owned);
    let metadata = request_metadata(request.headers())?;
    let body = super::body::read_json_authorized(
        request.into_body(),
        operation.request_max_bytes,
        authority.deadline,
        || (authority.live)(),
    )
    .await
    .map_err(body_refusal)?;
    let resolved =
        super::routes::resolve(&verb, &path, query.as_deref(), body).map_err(route_refusal)?;
    let invocation = Invocation::decode(resolved.method, &resolved.params)
        .and_then(|invocation| invocation.with_metadata(metadata))
        .map_err(api_refusal)?;
    let (event, call) = execute_resource(
        state,
        &session,
        request_authorization,
        invocation,
        reservation.clone(),
    )
    .await?;
    if event.bytes().len() > resolved.operation.response_max_bytes {
        return Err(refusal(
            StatusCode::INTERNAL_SERVER_ERROR,
            "response_too_large",
        ));
    }
    Ok(resource_output(
        event,
        call,
        reservation,
        output,
        session,
        &authority,
    ))
}

fn authorize_resource(
    authority: &VerifiedAuthority,
    operation: &super::routes::ResourceOperation,
    headers: &HeaderMap,
) -> HttpResult<()> {
    if headers.contains_key(CONTENT_ENCODING) {
        return Err(refusal(StatusCode::BAD_REQUEST, "invalid_request"));
    }
    match crate::api::effects::requires_write_scope(&operation.method) {
        Some(false) => {}
        Some(true) => authority
            .ensure_scope("bridge:write")
            .map_err(auth_refusal)?,
        None => return Err(refusal(StatusCode::INTERNAL_SERVER_ERROR, "internal")),
    }
    if operation.requires_events {
        return Err(refusal(StatusCode::CONFLICT, "stream_required"));
    }
    match exact_header(headers, "X-Build-Subscription") {
        Ok(None) => {}
        Ok(Some(_)) => return Err(refusal(StatusCode::CONFLICT, "stream_replaced")),
        Err(()) => return Err(refusal(StatusCode::BAD_REQUEST, "invalid_request")),
    };
    Ok(())
}

fn resource_session_id(headers: &HeaderMap) -> HttpResult<&str> {
    match exact_header(headers, "X-Build-Session") {
        Ok(Some(session_id)) if !session_id.is_empty() && session_id.len() <= 128 => Ok(session_id),
        _ => Err(refusal(StatusCode::UNAUTHORIZED, "invalid_credential")),
    }
}

fn observe_output(state: &HttpState, output: &super::listener::OutputAuthority) {
    #[cfg(not(test))]
    let _ = (state, output);
    #[cfg(test)]
    if let Some(observer) = state.output_observer.lock().unwrap().clone() {
        observer(output.clone());
    }
}

fn resource_opening(
    state: &HttpState,
    session_id: &str,
    authority: &VerifiedAuthority,
) -> HttpResult<(
    Arc<super::limits::OpeningBudget>,
    SessionLease,
    RequestAuthorization,
)> {
    let opening_budget = {
        let mut openings = state.openings.lock().unwrap();
        let opening = openings
            .get_mut(session_id)
            .ok_or_else(|| refusal(StatusCode::UNAUTHORIZED, "invalid_credential"))?;
        if opening.authority.identity != authority.identity || !opening.session.is_live() {
            return Err(refusal(StatusCode::UNAUTHORIZED, "invalid_credential"));
        }
        opening.authority = authority.clone();
        opening.budget.clone()
    };
    let caller = VerifiedCaller::verified_http(
        authority.identity.clone(),
        authority.deadline,
        authority.live.clone(),
    );
    let (session, request_authorization) = state
        .execution
        .sessions()
        .resume_with_request_authority(caller, session_id)
        .map_err(|_| refusal(StatusCode::UNAUTHORIZED, "invalid_credential"))?;
    Ok((opening_budget, session, request_authorization))
}

async fn execute_resource(
    state: &HttpState,
    session: &SessionLease,
    request_authorization: RequestAuthorization,
    invocation: Invocation,
    reservation: Arc<super::limits::BufferReservation>,
) -> HttpResult<(EncodedEvent, crate::execution::Call)> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let sink = Arc::new(ResourceReplySink {
        sender: Mutex::new(Some(sender)),
        _reservation: reservation.clone(),
    });
    let context = state
        .execution
        .sessions()
        .request_authorized(session, sink, None, request_authorization)
        .map_err(|_| refusal(StatusCode::UNAUTHORIZED, "invalid_credential"))?;
    let mut call = state
        .execution
        .submit(context, invocation)
        .await
        .map_err(|_| refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    call.reply()
        .await
        .map_err(|_| refusal(StatusCode::UNAUTHORIZED, "invalid_credential"))?;
    let event = receiver
        .await
        .ok()
        .filter(EncodedEvent::is_live)
        .ok_or_else(|| refusal(StatusCode::UNAUTHORIZED, "invalid_credential"))?;
    Ok((event, call))
}

fn resource_output(
    event: EncodedEvent,
    call: crate::execution::Call,
    reservation: Arc<super::limits::BufferReservation>,
    output: super::listener::OutputAuthority,
    session: SessionLease,
    authority: &VerifiedAuthority,
) -> Response {
    let output_session = session;
    let deadline = authority.deadline;
    let request_live = authority.live.clone();
    let ticket = output.bind(
        Arc::new(move || output_session.is_live() && request_live()),
        Arc::new(move || deadline),
    );
    let bytes = bytes::Bytes::from_owner(ResourceBytes {
        event,
        call: Some(call),
        reservation: Some(reservation),
        ticket: Some(ticket),
    });
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

/// Retain the execution call and reservation until the listener confirms that
/// Hyper's accepted bytes have crossed the production socket flush boundary.
struct ResourceBytes {
    event: crate::execution::session::EncodedEvent,
    call: Option<crate::execution::Call>,
    reservation: Option<Arc<super::limits::BufferReservation>>,
    ticket: Option<super::listener::OutputTicket>,
}

impl AsRef<[u8]> for ResourceBytes {
    fn as_ref(&self) -> &[u8] {
        self.event.bytes()
    }
}

impl Drop for ResourceBytes {
    fn drop(&mut self) {
        if let (Some(ticket), Some(call), Some(reservation)) = (
            self.ticket.take(),
            self.call.take(),
            self.reservation.take(),
        ) {
            ticket.release_with((call, reservation));
        }
    }
}

#[cfg(test)]
fn router_with(execution: Arc<ExecutionService>, verifier: Arc<dyn GrantVerifier>) -> Router {
    runtime_with(execution, verifier).router()
}

pub async fn serve_mounted(
    listener: tokio::net::TcpListener,
    tls: Arc<rustls::ServerConfig>,
    policy: super::policy::HttpPolicy,
    runtime: HttpRuntime,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> std::io::Result<()> {
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let serving = super::listener::serve(listener, tls, policy.apply(runtime.router()), stopped);
    tokio::pin!(serving);
    if *shutdown.borrow() {
        runtime.shutdown();
        let _ = stop.send(true);
        return serving.await;
    }
    let mut reaper = tokio::time::interval(std::time::Duration::from_secs(1));
    reaper.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            result = &mut serving => {
                runtime.shutdown();
                return result;
            }
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    runtime.shutdown();
                    let _ = stop.send(true);
                    return serving.await;
                }
            }
            _ = reaper.tick() => {
                runtime.state.reap_ended();
            }
        }
    }
}

async fn close_session(
    Path(session_id): Path<String>,
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response {
    if session_id.is_empty() || session_id.len() > 128 {
        return refusal(StatusCode::NOT_FOUND, "not_found");
    }
    let bearer = match bearer(request.headers()) {
        Some(bearer) => bearer,
        None => return refusal(StatusCode::UNAUTHORIZED, "invalid_credential"),
    };
    let authority = match state.verifier.verify(bearer).await {
        Ok(authority) => authority,
        Err(error) => return auth_refusal(error),
    };
    if let Err(error) = authority.ensure_scope("bridge:read") {
        return auth_refusal(error);
    }
    let removed = {
        let mut openings = state.openings.lock().unwrap();
        match openings.get(&session_id) {
            Some(opening) if opening.authority.identity == authority.identity => {
                openings.remove(&session_id)
            }
            _ => None,
        }
    };
    let Some(opening) = removed else {
        return refusal(StatusCode::NOT_FOUND, "not_found");
    };
    state.execution.sessions().close(&opening.session);
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenSessionRequest {
    api_range: String,
}

async fn open_session(
    State(state): State<HttpState>,
    request: Request<axum::body::Body>,
) -> Response {
    let bearer = match bearer(request.headers()) {
        Some(bearer) => bearer,
        None => return refusal(StatusCode::UNAUTHORIZED, "invalid_credential"),
    };
    let authority = match state.verifier.verify(bearer).await {
        Ok(authority) => authority,
        Err(error) => return auth_refusal(error),
    };
    if let Err(error) = authority.ensure_scope("bridge:read") {
        return auth_refusal(error);
    }
    let output = match request
        .extensions()
        .get::<super::listener::OutputAuthority>()
    {
        Some(output) => output.clone(),
        None => return refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
    };
    let body = match super::body::read_json_authorized(
        request.into_body(),
        LIFECYCLE_BYTES,
        authority.deadline,
        || (authority.live)(),
    )
    .await
    {
        Ok(body) => body,
        Err(error) => return body_refusal(error),
    };
    let request: OpenSessionRequest = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(_) => return refusal(StatusCode::BAD_REQUEST, "invalid_request"),
    };
    let range = match api_range(&request.api_range) {
        Some(range) => range,
        None => return refusal(StatusCode::BAD_REQUEST, "invalid_request"),
    };
    let version = semver::Version::parse(crate::api::API_VERSION)
        .expect("the compiled API version is valid semver");
    if !range.matches(&version) {
        return refusal(StatusCode::UPGRADE_REQUIRED, "unsupported_version");
    }

    state.reap_ended();
    let mut openings = state.openings.lock().unwrap();
    if openings.len() >= MAX_OPENINGS {
        return refusal(StatusCode::TOO_MANY_REQUESTS, "overloaded");
    }
    if openings
        .values()
        .filter(|opening| opening.authority.identity.family_id == authority.identity.family_id)
        .count()
        >= MAX_FAMILY_OPENINGS
    {
        return refusal(StatusCode::TOO_MANY_REQUESTS, "overloaded");
    }
    let caller = VerifiedCaller::verified_http(
        authority.identity.clone(),
        authority.deadline,
        authority.live.clone(),
    );
    let session = match state.execution.sessions().open(caller) {
        Ok(session) => session,
        Err(_) => return refusal(StatusCode::UNAUTHORIZED, "invalid_credential"),
    };
    let greeting = match state.execution.greeting(&session) {
        Ok(greeting) => greeting,
        Err(_) => {
            state.execution.sessions().close(&session);
            return refusal(StatusCode::UNAUTHORIZED, "invalid_credential");
        }
    };
    let generation = uuid::Uuid::new_v4().to_string();
    let session_id = session.id().to_owned();
    let expires_at = grant_expiry(authority.deadline);
    let operation_ids = super::routes::manifest()
        .operations
        .iter()
        .filter(|operation| !operation.requires_events)
        .map(|operation| operation.operation_id.as_str())
        .collect::<Vec<_>>();
    let response = json!({
        "session_id": session_id.clone(),
        "opening_generation": generation.clone(),
        "greeting": greeting,
        "grant_expires_at": expires_at,
        "transport": {
            "resource_manifest_version": super::routes::manifest().version,
            "operation_ids": operation_ids,
            "terminal": "webrtc",
            "durable_events": false,
        },
    });
    openings.insert(
        session_id,
        MountedOpening {
            session: session.clone(),
            _generation: generation,
            authority: authority.clone(),
            budget: Arc::new(super::limits::OpeningBudget::default()),
        },
    );
    drop(openings);
    let output_session = session;
    let deadline = authority.deadline;
    let ticket = output.bind(
        Arc::new(move || output_session.is_live()),
        Arc::new(move || deadline),
    );
    let encoded = match serde_json::to_vec(&response) {
        Ok(encoded) => encoded,
        Err(_) => return refusal(StatusCode::INTERNAL_SERVER_ERROR, "invalid_configuration"),
    };
    let bytes = super::body::authority_bytes(encoded, ticket);
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = StatusCode::CREATED;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

fn api_range(raw: &str) -> Option<semver::VersionReq> {
    if raw.len() > 128 || raw.chars().any(char::is_control) {
        return None;
    }
    let comparators: Vec<_> = raw.split_ascii_whitespace().collect();
    let normalized = if comparators.len() > 1 && !raw.contains(',') {
        comparators.join(", ")
    } else {
        raw.to_owned()
    };
    semver::VersionReq::parse(&normalized).ok()
}

fn grant_expiry(deadline: Instant) -> String {
    let remaining = deadline.saturating_duration_since(Instant::now());
    let expiry = time::OffsetDateTime::now_utc()
        + time::Duration::try_from(remaining).unwrap_or(time::Duration::MAX);
    expiry
        .format(&time::format_description::well_known::Rfc3339)
        .expect("UTC grant deadline formats as RFC3339")
}

fn body_refusal(error: super::body::BodyError) -> Response {
    match error {
        super::body::BodyError::TooLarge => {
            refusal(StatusCode::PAYLOAD_TOO_LARGE, "request_too_large")
        }
        super::body::BodyError::Timeout => refusal(StatusCode::REQUEST_TIMEOUT, "timeout"),
        super::body::BodyError::Unauthorized => {
            refusal(StatusCode::UNAUTHORIZED, "invalid_credential")
        }
        super::body::BodyError::InvalidJson
        | super::body::BodyError::Disconnected
        | super::body::BodyError::Encoding => refusal(StatusCode::BAD_REQUEST, "invalid_request"),
    }
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    let token = value.strip_prefix("Bearer ")?;
    (!token.is_empty()).then_some(token)
}

fn exact_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ()> {
    let mut values = headers.get_all(name).iter();
    let Some(value) = values.next() else {
        return Ok(None);
    };
    let value = value.to_str().map_err(|_| ())?;
    if values.next().is_some() {
        return Err(());
    }
    Ok(Some(value))
}

fn request_metadata(headers: &HeaderMap) -> Result<RequestMetadata, Response> {
    let request_id = exact_header(headers, "X-Build-Request-Id")
        .map_err(|_| refusal(StatusCode::BAD_REQUEST, "invalid_request"))?
        .map(str::to_owned)
        .filter(|value| !value.is_empty() && value.len() <= 128);
    if headers.contains_key("X-Build-Request-Id") && request_id.is_none() {
        return Err(refusal(StatusCode::BAD_REQUEST, "invalid_request"));
    }
    let priority = match exact_header(headers, "X-Build-Priority")
        .map_err(|_| refusal(StatusCode::BAD_REQUEST, "invalid_request"))?
    {
        None | Some("user") => crate::timing::Priority::Foreground,
        Some("background") => crate::timing::Priority::Background,
        Some(_) => return Err(refusal(StatusCode::BAD_REQUEST, "invalid_request")),
    };
    let operation_id = exact_header(headers, "Idempotency-Key")
        .map_err(|_| refusal(StatusCode::BAD_REQUEST, "invalid_request"))?;
    let after = exact_header(headers, "X-Build-After")
        .map_err(|_| refusal(StatusCode::BAD_REQUEST, "invalid_request"))?;
    let write = match (operation_id, after) {
        (None, None) => None,
        (None, Some(_)) => return Err(refusal(StatusCode::BAD_REQUEST, "invalid_request")),
        (Some(operation_id), after) => {
            let mut value = json!({"operation_id": operation_id});
            if let Some(after) = after {
                value["after"] = serde_json::from_str(after)
                    .map_err(|_| refusal(StatusCode::BAD_REQUEST, "invalid_request"))?;
            }
            Some(
                serde_json::from_value(value)
                    .map_err(|_| refusal(StatusCode::BAD_REQUEST, "invalid_request"))?,
            )
        }
    };
    Ok(RequestMetadata {
        request_id,
        priority,
        write,
    })
}

fn route_refusal(error: super::routes::RouteError) -> Response {
    match error {
        super::routes::RouteError::NotFound => refusal(StatusCode::NOT_FOUND, "not_found"),
        super::routes::RouteError::MethodNotAllowed { allow } => {
            let mut response = refusal(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed");
            if let Ok(value) = HeaderValue::from_str(&allow.join(", ")) {
                response.headers_mut().insert(ALLOW, value);
            }
            response
        }
        super::routes::RouteError::InvalidParams { .. } => {
            refusal(StatusCode::BAD_REQUEST, "invalid_params")
        }
    }
}

fn api_refusal(error: crate::api::ApiError) -> Response {
    let status = match error.code() {
        "not_found" => StatusCode::NOT_FOUND,
        "already_exists" | "locked" | "conflict" | "stale_version" | "stale_body" => {
            StatusCode::CONFLICT
        }
        "busy" => StatusCode::TOO_MANY_REQUESTS,
        "unavailable" => StatusCode::SERVICE_UNAVAILABLE,
        "internal" => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    };
    refusal(status, error.code())
}

fn auth_refusal(error: AuthError) -> Response {
    match error {
        AuthError::InvalidCredential
        | AuthError::Expired
        | AuthError::Closed
        | AuthError::BindingMismatch => refusal(StatusCode::UNAUTHORIZED, "invalid_credential"),
        AuthError::ForbiddenScope => refusal(StatusCode::FORBIDDEN, "forbidden_scope"),
        AuthError::Overloaded => refusal(StatusCode::TOO_MANY_REQUESTS, "overloaded"),
        AuthError::Unavailable => refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable"),
        AuthError::InvalidConfiguration => {
            refusal(StatusCode::INTERNAL_SERVER_ERROR, "invalid_configuration")
        }
    }
}

fn refusal(status: StatusCode, code: &'static str) -> Response {
    let retryable = matches!(code, "overloaded" | "unavailable" | "timeout");
    (
        status,
        axum::Json(json!({
            "error": code,
            "error_code": code,
            "retryable": retryable,
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{execution::FrameHandler, timing::FrameClock};
    use axum::{body::Body, http::header::CONTENT_TYPE};
    use futures_util::stream;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tower::ServiceExt;

    struct WitnessVerifier {
        calls: Arc<AtomicUsize>,
        authority: Option<VerifiedAuthority>,
    }

    #[async_trait]
    impl GrantVerifier for WitnessVerifier {
        async fn verify(&self, _bearer: &str) -> Result<VerifiedAuthority, AuthError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.authority.clone().ok_or(AuthError::InvalidCredential)
        }
    }

    struct PairVerifier {
        a_live: Arc<AtomicBool>,
        b_live: Arc<AtomicBool>,
        a_lifetime: std::time::Duration,
    }

    #[async_trait]
    impl GrantVerifier for PairVerifier {
        async fn verify(&self, bearer: &str) -> Result<VerifiedAuthority, AuthError> {
            match bearer {
                "grant-a" => Ok(authority_with(self.a_live.clone(), self.a_lifetime)),
                "grant-b" => Ok(authority_with(
                    self.b_live.clone(),
                    std::time::Duration::from_secs(60),
                )),
                _ => Err(AuthError::InvalidCredential),
            }
        }
    }

    fn authority() -> VerifiedAuthority {
        authority_with(
            Arc::new(AtomicBool::new(true)),
            std::time::Duration::from_secs(60),
        )
    }

    fn authority_with(live: Arc<AtomicBool>, lifetime: std::time::Duration) -> VerifiedAuthority {
        VerifiedAuthority {
            identity: HttpIdentity {
                issuer: "https://account.example".into(),
                account_id: "account-1".into(),
                device_id: "device-1".into(),
                family_id: "family-1".into(),
                origin: "https://device.example".into(),
                endpoint_epoch: "endpoint-1".into(),
                authorization_epoch: 1,
            },
            deadline: Instant::now() + lifetime,
            live: Arc::new(move || live.load(Ordering::SeqCst)),
            scopes: vec!["bridge:read".into(), "bridge:write".into()].into(),
        }
    }

    fn execution() -> Arc<ExecutionService> {
        ExecutionService::new(
            FrameHandler::new(
                FrameClock::new(),
                |_, frame, _| json!({"id": frame.payload["id"], "ok": true, "result": {}}),
            )
            .with_greeting_reads(
                |epoch| json!({"api_version": crate::api::API_VERSION, "bridge_epoch": epoch}),
            ),
        )
    }

    async fn resource_json(
        app: &Router,
        method: &str,
        uri: impl AsRef<str>,
        session_id: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        resource_json_with_token(app, method, uri, session_id, body, "account-bound-token").await
    }

    async fn resource_json_with_token(
        app: &Router,
        method: &str,
        uri: impl AsRef<str>,
        session_id: &str,
        body: Option<serde_json::Value>,
        token: &str,
    ) -> (StatusCode, serde_json::Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri.as_ref())
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header("X-Build-Session", session_id);
        let body = match body {
            Some(body) => {
                builder = builder.header(CONTENT_TYPE, "application/json");
                Body::from(serde_json::to_vec(&body).unwrap())
            }
            None => Body::empty(),
        };
        let mut request = builder.body(body).unwrap();
        request
            .extensions_mut()
            .insert(super::super::listener::OutputAuthority::default());
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 9 * 1024 * 1024)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn session_open_rejects_missing_bearer_before_body_or_verifier() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = router_with(
            execution(),
            Arc::new(WitnessVerifier {
                calls: calls.clone(),
                authority: None,
            }),
        );
        let response = app
            .oneshot(
                Request::post("/v1/sessions")
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from("{not-json"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn verified_session_open_returns_shared_execution_greeting() {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = router_with(
            execution(),
            Arc::new(WitnessVerifier {
                calls: calls.clone(),
                authority: Some(authority()),
            }),
        );
        let response = app
            .oneshot(open_request(Body::from(
                r#"{"api_range":">=3.0.0 <4.0.0"}"#,
            )))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let opening: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(opening["greeting"]["api_version"], crate::api::API_VERSION);
        assert!(opening["session_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()));
        assert!(opening["opening_generation"]
            .as_str()
            .is_some_and(|generation| !generation.is_empty()));
        assert_eq!(opening["transport"]["resource_manifest_version"], 1);
        assert_eq!(opening["transport"]["terminal"], "webrtc");
        assert_eq!(opening["transport"]["durable_events"], false);
    }

    #[tokio::test]
    async fn expired_openings_are_reclaimed_before_family_quota_is_counted() {
        struct ReplacingVerifier {
            calls: AtomicUsize,
            old_live: Arc<AtomicBool>,
        }
        #[async_trait]
        impl GrantVerifier for ReplacingVerifier {
            async fn verify(&self, _bearer: &str) -> Result<VerifiedAuthority, AuthError> {
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                let live = if call < MAX_FAMILY_OPENINGS {
                    self.old_live.clone()
                } else {
                    Arc::new(AtomicBool::new(true))
                };
                Ok(authority_with(live, std::time::Duration::from_secs(60)))
            }
        }

        let old_live = Arc::new(AtomicBool::new(true));
        let app = router_with(
            execution(),
            Arc::new(ReplacingVerifier {
                calls: AtomicUsize::new(0),
                old_live: old_live.clone(),
            }),
        );
        for _ in 0..MAX_FAMILY_OPENINGS {
            let response = app
                .clone()
                .oneshot(open_request(Body::from(r#"{"api_range":">=3 <4"}"#)))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CREATED);
        }
        old_live.store(false, Ordering::SeqCst);
        let replacement = app
            .oneshot(open_request(Body::from(r#"{"api_range":">=3 <4"}"#)))
            .await
            .unwrap();
        assert_eq!(replacement.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn opening_advertises_exactly_the_mounted_non_event_resources() {
        let app = router_with(
            execution(),
            Arc::new(WitnessVerifier {
                calls: Arc::new(AtomicUsize::new(0)),
                authority: Some(authority()),
            }),
        );
        let response = app
            .oneshot(open_request(Body::from(r#"{"api_range":">=3 <4"}"#)))
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let opening: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let expected = super::super::routes::manifest()
            .operations
            .iter()
            .filter(|operation| !operation.requires_events)
            .map(|operation| operation.operation_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(opening["transport"]["operation_ids"], json!(expected));
    }

    #[tokio::test]
    async fn authenticated_health_resource_uses_the_shared_execution_service() {
        let app = router_with(
            execution(),
            Arc::new(WitnessVerifier {
                calls: Arc::new(AtomicUsize::new(0)),
                authority: Some(authority()),
            }),
        );
        let opened = app
            .clone()
            .oneshot(open_request(Body::from(r#"{"api_range":">=3 <4"}"#)))
            .await
            .unwrap();
        assert_eq!(opened.status(), StatusCode::CREATED);
        let session_id = opening_id(opened).await;

        let mut request = Request::get("/v1/health")
            .header(AUTHORIZATION, "Bearer account-bound-token")
            .header("X-Build-Session", session_id)
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(super::super::listener::OutputAuthority::default());
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let reply: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(reply["ok"], true);
    }

    #[tokio::test]
    async fn mounted_resources_apply_real_task_file_and_agent_effects() {
        let dir = tempfile::tempdir().unwrap();
        let state_root = std::fs::canonicalize(dir.path()).unwrap();
        let repo = crate::git_fixture::init_repo_named(&state_root, "repo");
        std::fs::write(repo.join("mounted.txt"), "mounted resource\n").unwrap();
        let context = crate::harness::HarnessContext::resolved(
            state_root.join("mcp.sock"),
            state_root.clone(),
        )
        .unwrap();
        let mut app_state = crate::app::AppState::new_unrooted_configured(
            state_root.join("worktrees"),
            "main",
            true,
            context,
        )
        .with_task_store(state_root.join("store"))
        .unwrap();
        let project_id = app_state.add_project(repo, "main".into());
        let execution = ExecutionService::new(crate::app::AppState::handler(Arc::new(Mutex::new(
            app_state,
        ))));
        let runtime = runtime_with(
            execution,
            Arc::new(WitnessVerifier {
                calls: Arc::new(AtomicUsize::new(0)),
                authority: Some(authority()),
            }),
        );
        let app = runtime.router();
        let opened = app
            .clone()
            .oneshot(open_request(Body::from(r#"{"api_range":">=3 <4"}"#)))
            .await
            .unwrap();
        let session_id = opening_id(opened).await;

        let (status, created) = resource_json(
            &app,
            "POST",
            format!("/v1/projects/{project_id}/tasks"),
            &session_id,
            Some(json!({"title":"Mounted resource task"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{created:?}");
        assert_eq!(created["ok"], true, "{created:?}");
        let task_id = created["result"]["task"]["id"].as_str().unwrap().to_owned();
        let (status, read_task) = resource_json(
            &app,
            "GET",
            format!("/v1/tasks/{task_id}"),
            &session_id,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{read_task:?}");
        assert_eq!(
            read_task["result"]["task"]["title"],
            "Mounted resource task"
        );

        let (status, file) = resource_json(
            &app,
            "POST",
            "/v1/files/content-queries",
            &session_id,
            Some(json!({"project_id":project_id,"path":"mounted.txt"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{file:?}");
        assert_eq!(file["ok"], true, "{file:?}");
        assert_eq!(file["result"]["content_b64"], "bW91bnRlZCByZXNvdXJjZQo=");

        let (status, workspace) = resource_json(
            &app,
            "POST",
            "/v1/workspaces",
            &session_id,
            Some(json!({"project_id":project_id,"name":"mounted-agent","isolation":"worktree"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{workspace:?}");
        let workspace_id = workspace["result"]["workspace_id"].as_str().unwrap();
        let (status, conversation) = resource_json(
            &app,
            "POST",
            format!("/v1/workspaces/{workspace_id}/conversation"),
            &session_id,
            Some(json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{conversation:?}");
        let run_id = conversation["result"]["run_id"].as_str().unwrap();
        let (status, started) = resource_json(
            &app,
            "POST",
            "/v1/agent-starts",
            &session_id,
            Some(json!({"id":run_id})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{started:?}");
        assert_eq!(started["ok"], true, "{started:?}");
        assert!(started["result"]["term_id"]
            .as_str()
            .is_some_and(|term_id| term_id.starts_with("agent:")));
        runtime.shutdown();
    }

    #[tokio::test]
    async fn read_only_grant_can_read_but_cannot_dispatch_a_mutation() {
        let mut read_only = authority();
        read_only.scopes = vec!["bridge:read".into()].into();
        let app = router_with(
            execution(),
            Arc::new(WitnessVerifier {
                calls: Arc::new(AtomicUsize::new(0)),
                authority: Some(read_only),
            }),
        );
        let opened = app
            .clone()
            .oneshot(open_request(Body::from(r#"{"api_range":">=3 <4"}"#)))
            .await
            .unwrap();
        let session_id = opening_id(opened).await;

        let mut read = Request::get("/v1/health")
            .header(AUTHORIZATION, "Bearer account-bound-token")
            .header("X-Build-Session", &session_id)
            .body(Body::empty())
            .unwrap();
        read.extensions_mut()
            .insert(super::super::listener::OutputAuthority::default());
        assert_eq!(
            app.clone().oneshot(read).await.unwrap().status(),
            StatusCode::OK
        );

        let mut write = Request::patch("/v1/settings")
            .header(AUTHORIZATION, "Bearer account-bound-token")
            .header(CONTENT_TYPE, "application/json")
            .header("X-Build-Session", session_id)
            .body(Body::from("{}"))
            .unwrap();
        write
            .extensions_mut()
            .insert(super::super::listener::OutputAuthority::default());
        assert_eq!(
            app.oneshot(write).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn duplicate_resource_metadata_headers_are_rejected() {
        let app = router_with(
            execution(),
            Arc::new(WitnessVerifier {
                calls: Arc::new(AtomicUsize::new(0)),
                authority: Some(authority()),
            }),
        );
        let opened = app
            .clone()
            .oneshot(open_request(Body::from(r#"{"api_range":">=3 <4"}"#)))
            .await
            .unwrap();
        let session_id = opening_id(opened).await;

        let mut request = Request::get("/v1/health")
            .header(AUTHORIZATION, "Bearer account-bound-token")
            .header("X-Build-Session", session_id)
            .header("X-Build-Priority", "user")
            .header("X-Build-Priority", "background")
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(super::super::listener::OutputAuthority::default());

        assert_eq!(
            app.oneshot(request).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn resource_requests_reject_content_encoding() {
        let app = router_with(
            execution(),
            Arc::new(WitnessVerifier {
                calls: Arc::new(AtomicUsize::new(0)),
                authority: Some(authority()),
            }),
        );
        let opened = app
            .clone()
            .oneshot(open_request(Body::from(r#"{"api_range":">=3 <4"}"#)))
            .await
            .unwrap();
        let session_id = opening_id(opened).await;

        let mut request = Request::get("/v1/health")
            .header(AUTHORIZATION, "Bearer account-bound-token")
            .header("X-Build-Session", session_id)
            .header("Content-Encoding", "identity")
            .body(Body::empty())
            .unwrap();
        request
            .extensions_mut()
            .insert(super::super::listener::OutputAuthority::default());

        assert_eq!(
            app.oneshot(request).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn cancelled_http_request_retains_reservation_until_executor_settlement() {
        let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let release_rx = Arc::new(Mutex::new(release_rx));
        let handler = FrameHandler::new(FrameClock::new(), move |_, frame, _| {
            entered_tx.send(()).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
            json!({"id": frame.payload["id"], "ok": true, "result": {}})
        })
        .with_greeting_reads(
            |epoch| json!({"api_version": crate::api::API_VERSION, "bridge_epoch": epoch}),
        );
        let runtime = runtime_with(
            ExecutionService::new(handler),
            Arc::new(WitnessVerifier {
                calls: Arc::new(AtomicUsize::new(0)),
                authority: Some(authority()),
            }),
        );
        let budget = runtime.state.buffers.clone();
        let initially_available = budget.available();
        let app = runtime.router();
        let opened = app
            .clone()
            .oneshot(open_request(Body::from(r#"{"api_range":">=3 <4"}"#)))
            .await
            .unwrap();
        let session_id = opening_id(opened).await;

        let mut request = Request::patch("/v1/settings")
            .header(AUTHORIZATION, "Bearer account-bound-token")
            .header(CONTENT_TYPE, "application/json")
            .header("X-Build-Session", session_id)
            .body(Body::from("{}"))
            .unwrap();
        request
            .extensions_mut()
            .insert(super::super::listener::OutputAuthority::default());
        let in_flight = tokio::spawn(app.oneshot(request));
        entered_rx.recv().await.unwrap();
        let reserved = budget.available();
        assert!(reserved < initially_available);

        in_flight.abort();
        assert!(
            in_flight.await.unwrap_err().is_cancelled(),
            "the HTTP future must be gone before inspecting executor ownership"
        );
        assert_eq!(
            budget.available(),
            reserved,
            "HTTP cancellation cannot release executor-owned capacity"
        );

        release_tx.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while budget.available() != initially_available {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("settled executor releases the reservation");
    }

    async fn pending_request_is_not_renewed_by_an_overlapping_grant(expire: bool) {
        let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let release_rx = Arc::new(Mutex::new(release_rx));
        let calls = Arc::new(AtomicUsize::new(0));
        let call_count = calls.clone();
        let handler = FrameHandler::new(FrameClock::new(), move |_, frame, _| {
            if call_count.fetch_add(1, Ordering::SeqCst) == 0 {
                entered_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
            }
            json!({"id": frame.payload["id"], "ok": true, "result": {}})
        })
        .with_greeting_reads(
            |epoch| json!({"api_version": crate::api::API_VERSION, "bridge_epoch": epoch}),
        );
        let a_live = Arc::new(AtomicBool::new(true));
        let b_live = Arc::new(AtomicBool::new(true));
        let lifetime = if expire {
            std::time::Duration::from_millis(40)
        } else {
            std::time::Duration::from_secs(60)
        };
        let app = router_with(
            ExecutionService::new(handler),
            Arc::new(PairVerifier {
                a_live: a_live.clone(),
                b_live: b_live.clone(),
                a_lifetime: lifetime,
            }),
        );
        let opened = app
            .clone()
            .oneshot(open_request_with_token(
                Body::from(r#"{"api_range":">=3 <4"}"#),
                "grant-a",
            ))
            .await
            .unwrap();
        let session_id = opening_id(opened).await;

        let mut request_a = Request::patch("/v1/settings")
            .header(AUTHORIZATION, "Bearer grant-a")
            .header(CONTENT_TYPE, "application/json")
            .header("X-Build-Session", &session_id)
            .body(Body::from("{}"))
            .unwrap();
        request_a
            .extensions_mut()
            .insert(super::super::listener::OutputAuthority::default());
        let request_a = tokio::spawn(app.clone().oneshot(request_a));
        entered_rx.recv().await.unwrap();

        // This request renews the opening to B, then fails after renewal.
        let mut request_b = Request::patch("/v1/settings")
            .header(AUTHORIZATION, "Bearer grant-b")
            .header(CONTENT_TYPE, "application/json")
            .header("X-Build-Session", &session_id)
            .body(Body::from("{not-json"))
            .unwrap();
        request_b
            .extensions_mut()
            .insert(super::super::listener::OutputAuthority::default());
        assert_eq!(
            app.clone().oneshot(request_b).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
        if expire {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        } else {
            a_live.store(false, Ordering::SeqCst);
        }
        release_tx.send(()).unwrap();
        assert_eq!(
            request_a.await.unwrap().unwrap().status(),
            StatusCode::UNAUTHORIZED,
            "grant B must not replace queued request A's authority"
        );

        let (status, reply) =
            resource_json_with_token(&app, "GET", "/v1/health", &session_id, None, "grant-b").await;
        assert_eq!(status, StatusCode::OK, "grant B remains live: {reply:?}");
    }

    #[tokio::test]
    async fn queued_request_keeps_its_exact_grant_after_overlapping_renewal() {
        pending_request_is_not_renewed_by_an_overlapping_grant(false).await;
    }

    #[tokio::test]
    async fn queued_request_keeps_its_exact_expiry_after_overlapping_renewal() {
        pending_request_is_not_renewed_by_an_overlapping_grant(true).await;
    }

    #[tokio::test]
    async fn queued_before_handler_request_keeps_its_exact_grant() {
        let clock = FrameClock::new();
        let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let release_rx = Arc::new(Mutex::new(release_rx));
        let calls = Arc::new(AtomicUsize::new(0));
        let call_count = calls.clone();
        let handler = FrameHandler::new(clock.clone(), move |_, frame, _| {
            let call = call_count.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                entered_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
            }
            json!({"id": frame.payload["id"], "ok": true, "result": {}})
        })
        .with_greeting_reads(
            |epoch| json!({"api_version": crate::api::API_VERSION, "bridge_epoch": epoch}),
        );
        let execution = ExecutionService::with_capacity(handler, 8, 1);
        let a_live = Arc::new(AtomicBool::new(true));
        let app = router_with(
            execution,
            Arc::new(PairVerifier {
                a_live: a_live.clone(),
                b_live: Arc::new(AtomicBool::new(true)),
                a_lifetime: std::time::Duration::from_secs(60),
            }),
        );
        let opened = app
            .clone()
            .oneshot(open_request_with_token(
                Body::from(r#"{"api_range":">=3 <4"}"#),
                "grant-a",
            ))
            .await
            .unwrap();
        let session_id = opening_id(opened).await;
        let request = |token: &'static str| {
            let mut request = Request::patch("/v1/settings")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .header(CONTENT_TYPE, "application/json")
                .header("X-Build-Session", &session_id)
                .body(Body::from("{}"))
                .unwrap();
            request
                .extensions_mut()
                .insert(super::super::listener::OutputAuthority::default());
            request
        };
        let blocker = tokio::spawn(app.clone().oneshot(request("grant-a")));
        entered_rx.recv().await.unwrap();
        let queued = tokio::spawn(app.clone().oneshot(request("grant-a")));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while clock.stats()["queue_depth"] != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("request A is queued before grant B renews the opening");

        let mut renewal = request("grant-b");
        *renewal.body_mut() = Body::from("{not-json");
        assert_eq!(
            app.oneshot(renewal).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
        a_live.store(false, Ordering::SeqCst);
        release_tx.send(()).unwrap();
        assert_eq!(
            queued.await.unwrap().unwrap().status(),
            StatusCode::UNAUTHORIZED,
            "queued A cannot execute under grant B"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "queued A never reaches the handler"
        );
        assert_eq!(
            blocker.await.unwrap().unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn encoded_pending_output_keeps_exact_grant_through_deferred_flush_release() {
        let a_live = Arc::new(AtomicBool::new(true));
        let b_live = Arc::new(AtomicBool::new(true));
        let app = router_with(
            execution(),
            Arc::new(PairVerifier {
                a_live: a_live.clone(),
                b_live: b_live.clone(),
                a_lifetime: std::time::Duration::from_secs(60),
            }),
        );
        let opened = app
            .clone()
            .oneshot(open_request_with_token(
                Body::from(r#"{"api_range":">=3 <4"}"#),
                "grant-a",
            ))
            .await
            .unwrap();
        let session_id = opening_id(opened).await;

        let output_a = super::super::listener::OutputAuthority::default();
        let mut request_a = Request::get("/v1/health")
            .header(AUTHORIZATION, "Bearer grant-a")
            .header("X-Build-Session", &session_id)
            .body(Body::empty())
            .unwrap();
        request_a.extensions_mut().insert(output_a.clone());
        let response_a = app.clone().oneshot(request_a).await.unwrap();
        assert_eq!(response_a.status(), StatusCode::OK);

        let mut renewal = Request::patch("/v1/settings")
            .header(AUTHORIZATION, "Bearer grant-b")
            .header(CONTENT_TYPE, "application/json")
            .header("X-Build-Session", &session_id)
            .body(Body::from("{not-json"))
            .unwrap();
        renewal
            .extensions_mut()
            .insert(super::super::listener::OutputAuthority::default());
        assert_eq!(
            app.clone().oneshot(renewal).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );

        let bytes = axum::body::to_bytes(response_a.into_body(), 64 * 1024)
            .await
            .unwrap();
        let (socket, mut peer) = tokio::io::duplex(1);
        let mut io = super::super::listener::BoundedIo::new(socket, output_a.clone());
        let writing = tokio::spawn(async move {
            io.write_all(&bytes).await?;
            io.flush().await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !output_a.has_pending_write() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("encoded A output is parked at the socket boundary");
        a_live.store(false, Ordering::SeqCst);
        let draining = tokio::spawn(async move {
            let mut drained = Vec::new();
            peer.read_to_end(&mut drained).await.unwrap();
            drained
        });
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), writing)
            .await
            .expect("revoked A output settles rather than remaining socket-blocked")
            .unwrap();
        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied,
            "grant B must not flush encoded output admitted under revoked grant A"
        );
        let _ = draining.await.unwrap();

        let output_b = super::super::listener::OutputAuthority::default();
        let mut request_b = Request::get("/v1/health")
            .header(AUTHORIZATION, "Bearer grant-b")
            .header("X-Build-Session", &session_id)
            .body(Body::empty())
            .unwrap();
        request_b.extensions_mut().insert(output_b.clone());
        let response_b = app.clone().oneshot(request_b).await.unwrap();
        let bytes_b = axum::body::to_bytes(response_b.into_body(), 64 * 1024)
            .await
            .unwrap();
        let mut live_socket = super::super::listener::BoundedIo::new(tokio::io::sink(), output_b);
        live_socket.write_all(&bytes_b).await.unwrap();
        live_socket.flush().await.unwrap();

        let closed_output = super::super::listener::OutputAuthority::default();
        let mut before_close = Request::get("/v1/health")
            .header(AUTHORIZATION, "Bearer grant-b")
            .header("X-Build-Session", &session_id)
            .body(Body::empty())
            .unwrap();
        before_close.extensions_mut().insert(closed_output.clone());
        let response = app.clone().oneshot(before_close).await.unwrap();
        let closed_bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let mut close = close_request_with_token(&session_id, "grant-b");
        close
            .extensions_mut()
            .insert(super::super::listener::OutputAuthority::default());
        assert_eq!(
            app.oneshot(close).await.unwrap().status(),
            StatusCode::NO_CONTENT
        );
        let mut closed_socket =
            super::super::listener::BoundedIo::new(tokio::io::sink(), closed_output);
        assert_eq!(
            closed_socket
                .write_all(&closed_bytes)
                .await
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::PermissionDenied,
            "session close remains an independent fence"
        );
    }

    #[tokio::test]
    async fn authenticated_trickle_body_cannot_outlive_grant() {
        let app = router_with(
            execution(),
            Arc::new(WitnessVerifier {
                calls: Arc::new(AtomicUsize::new(0)),
                authority: Some(authority_with(
                    Arc::new(AtomicBool::new(true)),
                    std::time::Duration::from_millis(40),
                )),
            }),
        );
        let chunks = stream::unfold((), |_| async {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            Some((Ok::<_, std::io::Error>(bytes::Bytes::from_static(b" ")), ()))
        });
        let response = tokio::time::timeout(
            std::time::Duration::from_millis(150),
            app.oneshot(open_request(Body::from_stream(chunks))),
        )
        .await
        .expect("grant deadline ends a body that keeps beating the idle timeout")
        .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn opening_output_is_fenced_at_the_socket_after_revocation() {
        let live = Arc::new(AtomicBool::new(true));
        let app = router_with(
            execution(),
            Arc::new(WitnessVerifier {
                calls: Arc::new(AtomicUsize::new(0)),
                authority: Some(authority_with(
                    live.clone(),
                    std::time::Duration::from_secs(60),
                )),
            }),
        );
        let output = super::super::listener::OutputAuthority::default();
        let mut request = open_request(Body::from(r#"{"api_range":">=3 <4"}"#));
        request.extensions_mut().insert(output.clone());
        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);

        live.store(false, Ordering::SeqCst);
        let mut socket = super::super::listener::BoundedIo::new(tokio::io::sink(), output);
        let error = socket.write_all(b"buffered opening").await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        drop(response);
    }

    #[tokio::test]
    async fn production_tls_socket_cuts_off_buffered_resource_after_revocation() {
        let dir = tempfile::tempdir().unwrap();
        let rcgen::CertifiedKey { cert, key_pair } =
            rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let cert_path = dir.path().join("cert.pem");
        let key_path = dir.path().join("key.pem");
        std::fs::write(&cert_path, cert.pem()).unwrap();
        std::fs::write(&key_path, key_pair.serialize_pem()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let tls = super::super::tls::load(&cert_path, &key_path).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let policy = super::super::policy::HttpPolicy::new(
            &format!("https://localhost:{port}"),
            &["https://app.example".to_owned()],
        )
        .unwrap();
        let payload = "x".repeat(7 * 1024 * 1024);
        let execution = ExecutionService::new(
            FrameHandler::new(FrameClock::new(), move |_, frame, _| {
                json!({"id": frame.payload["id"], "ok": true, "result": {"payload": payload.clone()}})
            })
            .with_greeting_reads(
                |epoch| json!({"api_version": crate::api::API_VERSION, "bridge_epoch": epoch}),
            ),
        );
        let a_live = Arc::new(AtomicBool::new(true));
        let b_live = Arc::new(AtomicBool::new(true));
        let runtime = runtime_with(
            execution,
            Arc::new(PairVerifier {
                a_live: a_live.clone(),
                b_live: b_live.clone(),
                a_lifetime: std::time::Duration::from_secs(60),
            }),
        );
        let (authority_tx, mut authority_rx) = tokio::sync::mpsc::unbounded_channel();
        *runtime.state.output_observer.lock().unwrap() = Some(Arc::new(move |authority| {
            authority_tx.send(authority).unwrap();
        }));
        let (stop, stopped) = tokio::sync::watch::channel(false);
        let server_runtime = runtime.clone();
        let server = tokio::spawn(async move {
            serve_mounted(listener, tls, policy, server_runtime, stopped).await
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .http1_only()
            .add_root_certificate(reqwest::Certificate::from_pem(cert.pem().as_bytes()).unwrap())
            .build()
            .unwrap();
        let base = format!("https://localhost:{port}");
        let opening: serde_json::Value = client
            .post(format!("{base}/v1/sessions"))
            .header("origin", "https://app.example")
            .bearer_auth("grant-a")
            .json(&json!({"api_range":">=3 <4"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let session_id = opening["session_id"].as_str().unwrap().to_owned();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert.der().clone()).unwrap();
        let client_config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));

        for revoke in [false, true] {
            let socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .unwrap();
            let mut socket = connector
                .connect(
                    rustls::pki_types::ServerName::try_from("localhost").unwrap(),
                    socket,
                )
                .await
                .unwrap();
            let request = format!(
                "PATCH /v1/settings HTTP/1.1\r\nHost: localhost:{port}\r\nOrigin: https://app.example\r\nAuthorization: Bearer grant-a\r\nX-Build-Session: {session_id}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
            );
            socket.write_all(request.as_bytes()).await.unwrap();
            let authority = authority_rx.recv().await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    if authority.has_pending_write() {
                        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                        if authority.has_pending_write() {
                            break;
                        }
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("the unread production TLS response must park a socket write");
            if revoke {
                let renewal = client
                    .patch(format!("{base}/v1/settings"))
                    .header("origin", "https://app.example")
                    .header("X-Build-Session", &session_id)
                    .header(CONTENT_TYPE.as_str(), "application/json")
                    .bearer_auth("grant-b")
                    .body("{not-json")
                    .send()
                    .await
                    .unwrap();
                assert_eq!(renewal.status(), StatusCode::BAD_REQUEST);
                a_live.store(false, Ordering::SeqCst);
            }
            let mut response = Vec::new();
            let read = tokio::time::timeout(
                std::time::Duration::from_secs(4),
                socket.read_to_end(&mut response),
            )
            .await
            .expect("the socket response must settle");
            let split = response
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| position + 4)
                .expect("HTTP response has headers");
            let headers = std::str::from_utf8(&response[..split]).unwrap();
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|value| value.parse::<usize>().ok())
                })
                .expect("bounded resource response has a content length");
            let received = response.len() - split;
            if revoke {
                assert!(
                    read.is_err() || received < content_length,
                    "revocation must truncate the parked production socket response"
                );
            } else {
                read.unwrap();
                assert_eq!(
                    received, content_length,
                    "the identical live authority completes the parked response"
                );
            }
        }

        let live_b = client
            .get(format!("{base}/v1/health"))
            .header("origin", "https://app.example")
            .header("X-Build-Session", &session_id)
            .bearer_auth("grant-b")
            .send()
            .await
            .unwrap();
        assert_eq!(live_b.status(), StatusCode::OK);
        assert!(
            !live_b.bytes().await.unwrap().is_empty(),
            "grant B remains live and completes through the production TLS socket"
        );

        stop.send(true).unwrap();
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn explicit_close_recovers_quota_and_stale_close_cannot_harm_successor() {
        let runtime = runtime_with(
            execution(),
            Arc::new(WitnessVerifier {
                calls: Arc::new(AtomicUsize::new(0)),
                authority: Some(authority()),
            }),
        );
        let app = runtime.router();
        let mut first = None;
        for _ in 0..MAX_FAMILY_OPENINGS {
            let response = app
                .clone()
                .oneshot(open_request(Body::from(r#"{"api_range":">=3 <4"}"#)))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CREATED);
            let id = opening_id(response).await;
            first.get_or_insert(id);
        }
        assert_eq!(runtime.opening_count(), MAX_FAMILY_OPENINGS);
        let first = first.unwrap();
        let closed = app.clone().oneshot(close_request(&first)).await.unwrap();
        assert_eq!(closed.status(), StatusCode::NO_CONTENT);

        let replacement = app
            .clone()
            .oneshot(open_request(Body::from(r#"{"api_range":">=3 <4"}"#)))
            .await
            .unwrap();
        assert_eq!(replacement.status(), StatusCode::CREATED);
        let replacement_id = opening_id(replacement).await;
        let stale = app.oneshot(close_request(&first)).await.unwrap();
        assert_eq!(stale.status(), StatusCode::NOT_FOUND);
        assert_ne!(first, replacement_id);
        assert_eq!(runtime.opening_count(), MAX_FAMILY_OPENINGS);
    }

    #[tokio::test]
    async fn shutdown_retires_every_exact_opening() {
        let runtime = runtime_with(
            execution(),
            Arc::new(WitnessVerifier {
                calls: Arc::new(AtomicUsize::new(0)),
                authority: Some(authority()),
            }),
        );
        for _ in 0..3 {
            let response = runtime
                .router()
                .oneshot(open_request(Body::from(r#"{"api_range":">=3 <4"}"#)))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CREATED);
        }
        assert_eq!(runtime.opening_count(), 3);
        assert_eq!(runtime.shutdown(), 3);
        assert_eq!(runtime.opening_count(), 0);
        assert_eq!(runtime.shutdown(), 0);
    }

    #[tokio::test]
    async fn global_opening_bound_recovers_after_revocation() {
        struct UniqueFamilyVerifier {
            calls: AtomicUsize,
            old_live: Arc<AtomicBool>,
        }
        #[async_trait]
        impl GrantVerifier for UniqueFamilyVerifier {
            async fn verify(&self, _bearer: &str) -> Result<VerifiedAuthority, AuthError> {
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                let live = if call <= MAX_OPENINGS {
                    self.old_live.clone()
                } else {
                    Arc::new(AtomicBool::new(true))
                };
                let mut authority = authority_with(live, std::time::Duration::from_secs(60));
                authority.identity.family_id = format!("family-{call}");
                Ok(authority)
            }
        }

        let old_live = Arc::new(AtomicBool::new(true));
        let runtime = runtime_with(
            execution(),
            Arc::new(UniqueFamilyVerifier {
                calls: AtomicUsize::new(0),
                old_live: old_live.clone(),
            }),
        );
        for _ in 0..MAX_OPENINGS {
            let response = runtime
                .router()
                .oneshot(open_request(Body::from(r#"{"api_range":">=3 <4"}"#)))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CREATED);
        }
        let full = runtime
            .router()
            .oneshot(open_request(Body::from(r#"{"api_range":">=3 <4"}"#)))
            .await
            .unwrap();
        assert_eq!(full.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(runtime.opening_count(), MAX_OPENINGS);

        old_live.store(false, Ordering::SeqCst);
        let recovered = runtime
            .router()
            .oneshot(open_request(Body::from(r#"{"api_range":">=3 <4"}"#)))
            .await
            .unwrap();
        assert_eq!(recovered.status(), StatusCode::CREATED);
        assert_eq!(runtime.opening_count(), 1);
    }

    #[tokio::test]
    async fn real_listener_applies_policy_and_shutdown_fences_openings() {
        let dir = tempfile::tempdir().unwrap();
        let rcgen::CertifiedKey { cert, key_pair } =
            rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let cert_path = dir.path().join("cert.pem");
        let key_path = dir.path().join("key.pem");
        std::fs::write(&cert_path, cert.pem()).unwrap();
        std::fs::write(&key_path, key_pair.serialize_pem()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let tls = super::super::tls::load(&cert_path, &key_path).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let policy = super::super::policy::HttpPolicy::new(
            &format!("https://localhost:{port}"),
            &["https://app.example".to_owned()],
        )
        .unwrap();
        let runtime = runtime_with(
            execution(),
            Arc::new(WitnessVerifier {
                calls: Arc::new(AtomicUsize::new(0)),
                authority: Some(authority_with(
                    Arc::new(AtomicBool::new(true)),
                    std::time::Duration::from_millis(300),
                )),
            }),
        );
        let (stop, stopped) = tokio::sync::watch::channel(false);
        let server_runtime = runtime.clone();
        let server = tokio::spawn(async move {
            serve_mounted(listener, tls, policy, server_runtime, stopped).await
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .http1_only()
            .add_root_certificate(reqwest::Certificate::from_pem(cert.pem().as_bytes()).unwrap())
            .build()
            .unwrap();
        let url = format!("https://localhost:{port}/v1/sessions");

        let denied = client
            .post(&url)
            .header("origin", "https://wrong.example")
            .bearer_auth("account-bound-token")
            .json(&json!({"api_range":">=3 <4"}))
            .send()
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let opened = client
            .post(&url)
            .header("origin", "https://app.example")
            .bearer_auth("account-bound-token")
            .json(&json!({"api_range":">=3 <4"}))
            .send()
            .await
            .unwrap();
        assert_eq!(opened.status(), StatusCode::CREATED);
        assert_eq!(runtime.opening_count(), 1);

        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        assert_eq!(runtime.opening_count(), 0);

        stop.send(true).unwrap();
        server.await.unwrap().unwrap();
        assert_eq!(runtime.opening_count(), 0);
    }

    async fn opening_id(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["session_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn close_request(session_id: &str) -> Request<Body> {
        close_request_with_token(session_id, "account-bound-token")
    }

    fn close_request_with_token(session_id: &str, token: &str) -> Request<Body> {
        Request::delete(format!("/v1/sessions/{session_id}"))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    }

    fn open_request(body: Body) -> Request<Body> {
        open_request_with_token(body, "account-bound-token")
    }

    fn open_request_with_token(body: Body, token: &str) -> Request<Body> {
        let mut request = Request::post("/v1/sessions")
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .body(body)
            .unwrap();
        request
            .extensions_mut()
            .insert(super::super::listener::OutputAuthority::default());
        request
    }
}
