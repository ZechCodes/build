//! Public endpoint identity proof; no application or account authority is issued.

use std::time::Duration;

use super::*;
use crate::http::{
    discovery::DeviceProofSigner, discovery_limits::DiscoveryLimiter, listener::PeerAddress,
};

const REQUEST_BYTES: usize = 4 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
// Hosted challenges live for 60 seconds. Keep local spends beyond that window.
const REPLAY_WINDOW: Duration = Duration::from_secs(65);
const MAX_REPLAYS: usize = 128;

pub(super) struct Discovery {
    signer: Arc<DeviceProofSigner>,
    limiter: DiscoveryLimiter,
    seen: Mutex<HashMap<String, Instant>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Challenge {
    challenge_id: String,
    nonce: String,
}

impl Discovery {
    pub(super) fn new(signer: Arc<DeviceProofSigner>) -> Self {
        Self {
            signer,
            limiter: DiscoveryLimiter::default(),
            seen: Mutex::new(HashMap::new()),
        }
    }

    fn proof(&self, challenge: Challenge) -> HttpResult<String> {
        let now = Instant::now();
        let mut seen = self.seen.lock().unwrap();
        seen.retain(|_, created| now.duration_since(*created) < REPLAY_WINDOW);
        if seen.contains_key(&challenge.challenge_id) {
            return Err(refusal(StatusCode::CONFLICT, "replayed_challenge").into());
        }
        if seen.len() >= MAX_REPLAYS {
            return Err(refusal(StatusCode::TOO_MANY_REQUESTS, "overloaded").into());
        }
        let proof = self
            .signer
            .discovery_proof(
                &challenge.challenge_id,
                &challenge.nonce,
                crate::http::discovery::unix_now()
                    .map_err(|_| refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?,
            )
            .map_err(|_| refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
        seen.insert(challenge.challenge_id, now);
        Ok(proof)
    }
}

pub(super) async fn discover(State(state): State<HttpState>, request: Request<Body>) -> Response {
    match respond(&state, request).await {
        Ok(response) => response,
        Err(response) => *response,
    }
}

async fn respond(state: &HttpState, request: Request<Body>) -> HttpResult<Response> {
    ensure_accepting(state)?;
    let discovery = state
        .discovery
        .as_ref()
        .ok_or_else(|| refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    let peer = request
        .extensions()
        .get::<PeerAddress>()
        .ok_or_else(|| refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    if !discovery.limiter.admit(peer.0) {
        return Err(refusal(StatusCode::TOO_MANY_REQUESTS, "overloaded").into());
    }
    validate_headers(request.headers())?;
    let body = tokio::time::timeout(
        REQUEST_TIMEOUT,
        crate::http::body::read_json(request.into_body(), REQUEST_BYTES),
    )
    .await
    .map_err(|_| refusal(StatusCode::REQUEST_TIMEOUT, "timeout"))?
    .map_err(body_refusal)?;
    let challenge = parse_challenge(body)?;
    ensure_accepting(state)?;
    let proof = discovery.proof(challenge)?;
    ensure_accepting(state)?;
    let mut response = axum::Json(json!({"proof": proof})).into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

fn ensure_accepting(state: &HttpState) -> HttpResult<()> {
    if !state.accepting.load(Ordering::SeqCst) {
        return Err(refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable").into());
    }
    Ok(())
}

fn validate_headers(headers: &HeaderMap) -> HttpResult<()> {
    let json = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/json")
        });
    if !json || headers.contains_key(CONTENT_ENCODING) {
        return Err(refusal(StatusCode::BAD_REQUEST, "invalid_request").into());
    }
    Ok(())
}

fn parse_challenge(body: serde_json::Value) -> HttpResult<Challenge> {
    let challenge: Challenge = serde_json::from_value(body)
        .map_err(|_| refusal(StatusCode::BAD_REQUEST, "invalid_request"))?;
    let valid = |value: &str, minimum| {
        (minimum..=128).contains(&value.len()) && !value.chars().any(char::is_control)
    };
    if !valid(&challenge.challenge_id, 1) || !valid(&challenge.nonce, 32) {
        return Err(refusal(StatusCode::BAD_REQUEST, "invalid_request").into());
    }
    Ok(challenge)
}
