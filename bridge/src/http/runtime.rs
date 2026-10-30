//! Production HTTP mount over the daemon's one shared execution service.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use axum::{
    extract::State,
    http::{header::AUTHORIZATION, HeaderMap, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
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
}

#[async_trait]
impl GrantVerifier for HttpAuthVerifier {
    async fn verify(&self, bearer: &str) -> Result<VerifiedAuthority, AuthError> {
        self.verify(bearer).await.map(VerifiedAuthority::from_grant)
    }
}

#[derive(Clone)]
struct HttpState {
    execution: Arc<ExecutionService>,
    verifier: Arc<dyn GrantVerifier>,
    openings: Arc<Mutex<HashMap<String, MountedOpening>>>,
}

struct MountedOpening {
    _session: SessionLease,
    _generation: String,
    _authority: VerifiedAuthority,
}

/// Mount lifecycle routes before the explicit resource resolver.
pub fn router(execution: Arc<ExecutionService>, verifier: HttpAuthVerifier) -> Router {
    router_with(execution, Arc::new(verifier))
}

fn router_with(execution: Arc<ExecutionService>, verifier: Arc<dyn GrantVerifier>) -> Router {
    Router::new()
        .route("/v1/sessions", post(open_session))
        .with_state(HttpState {
            execution,
            verifier,
            openings: Arc::new(Mutex::new(HashMap::new())),
        })
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
    let body = match super::body::read_json(request.into_body(), LIFECYCLE_BYTES).await {
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

    let mut openings = state.openings.lock().unwrap();
    if openings
        .values()
        .filter(|opening| opening._authority.identity.family_id == authority.identity.family_id)
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
    let operation_ids: Vec<_> = super::routes::manifest()
        .operations
        .iter()
        .map(|operation| operation.operation_id.as_str())
        .collect();
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
            _session: session,
            _generation: generation,
            _authority: authority,
        },
    );
    (StatusCode::CREATED, axum::Json(response)).into_response()
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
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
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
            deadline: Instant::now() + std::time::Duration::from_secs(60),
            live: Arc::new(|| true),
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
            .oneshot(
                Request::post("/v1/sessions")
                    .header(CONTENT_TYPE, "application/json")
                    .header(AUTHORIZATION, "Bearer account-bound-token")
                    .body(Body::from(r#"{"api_range":">=3.0.0 <4.0.0"}"#))
                    .unwrap(),
            )
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
}
