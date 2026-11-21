//! Native authority/CORS/header policy, independent of bearer authentication.
use axum::{
    extract::{Request, State},
    http::{header, HeaderName, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Json, Router,
};
use tower_http::cors::CorsLayer;

use super::limits::MAX_HEADER_BYTES;

const REQUEST_HEADERS: [&str; 10] = [
    "authorization",
    "content-type",
    "accept",
    "x-build-session",
    "x-build-subscription",
    "idempotency-key",
    "x-build-after",
    "x-build-request-id",
    "x-build-priority",
    "last-event-id",
];

#[derive(Clone)]
pub struct HttpPolicy {
    authority: axum::http::uri::Authority,
    origins: Vec<HeaderValue>,
}

impl HttpPolicy {
    /// Inputs have already passed the identity module's strict CanonicalOrigin.
    pub fn new(origin: &str, allowed: &[String]) -> Result<Self, String> {
        let authority = origin
            .strip_prefix("https://")
            .ok_or("HTTP origin must be HTTPS")?
            .parse()
            .map_err(|_| "invalid canonical HTTP authority")?;
        let origins = allowed
            .iter()
            .map(|origin| {
                HeaderValue::from_str(origin)
                    .map_err(|_| "invalid configured HTTP allowed origin".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { authority, origins })
    }

    pub fn apply(self, router: Router) -> Router {
        let cors = CorsLayer::new()
            .allow_origin(self.origins.clone())
            .allow_methods([
                Method::GET,
                Method::HEAD,
                Method::POST,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers(REQUEST_HEADERS.map(HeaderName::from_static))
            .expose_headers([
                HeaderName::from_static("x-request-id"),
                header::LOCATION,
                header::RETRY_AFTER,
            ]);
        router
            .layer(cors)
            .layer(middleware::from_fn_with_state(self, guard))
    }

    fn check(&self, request: &Request) -> Result<(), (StatusCode, &'static str)> {
        let bytes: usize = request
            .headers()
            .iter()
            .map(|(name, value)| name.as_str().len() + value.as_bytes().len() + 4)
            .sum();
        if bytes > MAX_HEADER_BYTES {
            return Err((
                StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
                "headers_too_large",
            ));
        }
        let host = request
            .headers()
            .get(header::HOST)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<axum::http::uri::Authority>().ok());
        if !host
            .as_ref()
            .is_some_and(|host| self.matches_authority(host))
        {
            return Err((StatusCode::MISDIRECTED_REQUEST, "authority_mismatch"));
        }
        if request
            .uri()
            .authority()
            .is_some_and(|authority| !self.matches_authority(authority))
            || request
                .uri()
                .scheme_str()
                .is_some_and(|scheme| scheme != "https")
        {
            return Err((StatusCode::MISDIRECTED_REQUEST, "authority_mismatch"));
        }
        if request.headers().contains_key(header::CONTENT_ENCODING) {
            return Err((
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "content_encoding_unsupported",
            ));
        }
        if request
            .headers()
            .get(header::ORIGIN)
            .is_some_and(|origin| !self.origins.contains(origin))
        {
            return Err((StatusCode::FORBIDDEN, "origin_forbidden"));
        }
        if request.method() == Method::OPTIONS {
            self.preflight(request)?;
        }
        Ok(())
    }

    fn matches_authority(&self, authority: &axum::http::uri::Authority) -> bool {
        !authority.as_str().contains('@')
            && authority.host().eq_ignore_ascii_case(self.authority.host())
            && authority.port_u16().unwrap_or(443) == self.authority.port_u16().unwrap_or(443)
    }

    fn preflight(&self, request: &Request) -> Result<(), (StatusCode, &'static str)> {
        let method = request
            .headers()
            .get(header::ACCESS_CONTROL_REQUEST_METHOD)
            .and_then(|value| value.to_str().ok());
        if !request.headers().contains_key(header::ORIGIN)
            || !matches!(
                method,
                Some("GET" | "HEAD" | "POST" | "PATCH" | "DELETE" | "OPTIONS")
            )
        {
            return Err((StatusCode::FORBIDDEN, "preflight_forbidden"));
        }
        if let Some(headers) = request
            .headers()
            .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
        {
            let names = headers
                .to_str()
                .map_err(|_| (StatusCode::FORBIDDEN, "preflight_forbidden"))?;
            if names.split(',').any(|name| {
                !REQUEST_HEADERS
                    .iter()
                    .any(|allowed| name.trim().eq_ignore_ascii_case(allowed))
            }) {
                return Err((StatusCode::FORBIDDEN, "preflight_forbidden"));
            }
        }
        Ok(())
    }
}

/// Terminal credentials require one explicit configured caller Origin in
/// addition to the listener's verified bridge authority.
pub(super) struct TerminalOrigin<'a> {
    caller: &'a str,
    policy: &'a HttpPolicy,
}

impl TerminalOrigin<'_> {
    pub(super) fn caller(&self) -> &str {
        self.caller
    }

    pub(super) fn matches_bridge(&self, origin: &str) -> bool {
        origin
            .strip_prefix("https://")
            .and_then(|authority| authority.parse().ok())
            .is_some_and(|authority| self.policy.matches_authority(&authority))
    }
}

pub(super) fn terminal_origin(request: &Request) -> Result<TerminalOrigin<'_>, Box<Response>> {
    let forbidden = || Box::new(origin_forbidden());
    let policy = request
        .extensions()
        .get::<HttpPolicy>()
        .ok_or_else(forbidden)?;
    let headers = request.headers();
    if headers.get_all(header::HOST).iter().count() != 1 {
        return Err(authority_mismatch().into());
    }
    let mut origins = headers.get_all(header::ORIGIN).iter();
    let value = origins.next().ok_or_else(forbidden)?;
    let caller = value.to_str().map_err(|_| forbidden())?;
    if origins.next().is_some()
        || !caller.starts_with("https://")
        || !policy.origins.contains(value)
    {
        return Err(forbidden());
    }
    Ok(TerminalOrigin { caller, policy })
}

pub(super) fn origin_forbidden() -> Response {
    policy_refusal(StatusCode::FORBIDDEN, "origin_forbidden")
}

pub(super) fn authority_mismatch() -> Response {
    policy_refusal(StatusCode::MISDIRECTED_REQUEST, "authority_mismatch")
}

fn policy_refusal(status: StatusCode, code: &'static str) -> Response {
    (
        status,
        Json(serde_json::json!({
            "error": "HTTP request policy refused",
            "error_code": code,
            "retryable": false,
            "request_id": uuid::Uuid::new_v4().to_string(),
        })),
    )
        .into_response()
}

async fn guard(State(policy): State<HttpPolicy>, mut request: Request, next: Next) -> Response {
    let mut response = match policy.check(&request) {
        Ok(()) => {
            request.extensions_mut().insert(policy);
            next.run(request).await
        }
        Err((status, code)) => policy_refusal(status, code),
    };
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if !response.headers().contains_key(header::VARY) {
        response
            .headers_mut()
            .insert(header::VARY, HeaderValue::from_static("Origin"));
    }
    response
}
