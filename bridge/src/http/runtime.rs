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
        header::{AUTHORIZATION, CONTENT_TYPE},
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
    session::{HttpIdentity, SessionLease, VerifiedCaller},
    ExecutionService,
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
}

#[derive(Clone)]
pub struct HttpRuntime {
    state: HttpState,
}

struct MountedOpening {
    session: SessionLease,
    _generation: String,
    authority: VerifiedAuthority,
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
        },
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
    let operation_ids: [&str; 0] = [];
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
    use tokio::io::AsyncWriteExt;
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
    async fn opening_advertises_no_resource_operation_before_routes_are_mounted() {
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
        assert_eq!(opening["transport"]["operation_ids"], json!([]));
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
        Request::delete(format!("/v1/sessions/{session_id}"))
            .header(AUTHORIZATION, "Bearer account-bound-token")
            .body(Body::empty())
            .unwrap()
    }

    fn open_request(body: Body) -> Request<Body> {
        let mut request = Request::post("/v1/sessions")
            .header(CONTENT_TYPE, "application/json")
            .header(AUTHORIZATION, "Bearer account-bound-token")
            .body(body)
            .unwrap();
        request
            .extensions_mut()
            .insert(super::super::listener::OutputAuthority::default());
        request
    }
}
