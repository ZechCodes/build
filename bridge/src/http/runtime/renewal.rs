//! `PATCH /v1/sessions/{id}`: a bearer hosted verifies afresh, of the opening's
//! exact binding, renews the opening's lifecycle authority. Its live stream and
//! event pushes follow that authority, so renewal is the only way past the
//! first grant; a refused or late renewal leaves the existing deadline alone.
use std::sync::{atomic::Ordering, Arc};

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header::CONTENT_TYPE, HeaderValue, Request, StatusCode},
    response::Response,
};
use tokio::time::Instant;

use super::{auth_refusal, bearer, opening_body, refusal, HttpResult, HttpState};
use crate::execution::session::{SessionLease, VerifiedCaller};
use crate::http::listener::OutputAuthority;

type Check = Arc<dyn Fn() -> bool + Send + Sync>;
type Deadline = Arc<dyn Fn() -> Instant + Send + Sync>;

/// The opening's lifecycle fences, re-read on every poll so renewal moves them.
pub(super) fn lifecycle_fences(session: &SessionLease) -> (Check, Deadline) {
    let lifecycle = session.authorization();
    let deadline = lifecycle.clone();
    (
        Arc::new(move || lifecycle.is_valid()),
        Arc::new(move || deadline.deadline().unwrap_or_else(Instant::now)),
    )
}

pub(super) async fn renew(
    Path(session_id): Path<String>,
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response {
    renewed(&state, &session_id, request)
        .await
        .unwrap_or_else(|error| *error)
}

async fn renewed(
    state: &HttpState,
    session_id: &str,
    request: Request<Body>,
) -> HttpResult<Response> {
    let unauthorized = || refusal(StatusCode::UNAUTHORIZED, "invalid_credential");
    if !state.accepting.load(Ordering::SeqCst) {
        return Err(unauthorized().into());
    }
    let bearer = bearer(request.headers()).ok_or_else(unauthorized)?;
    // An unverified bearer changes nothing: hosted revocation reaches the
    // opening through the revoked token's own grants, not through this reply.
    let authority = state
        .verifier
        .revalidate(bearer)
        .await
        .map_err(auth_refusal)?;
    authority
        .ensure_scope("bridge:read")
        .map_err(auth_refusal)?;
    let output = request
        .extensions()
        .get::<OutputAuthority>()
        .cloned()
        .ok_or_else(|| refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    let (mounted, generation) = {
        let openings = state.openings.lock().unwrap();
        match openings.get(session_id) {
            Some(opening) if opening.identity == authority.identity => {
                (opening.session.clone(), opening.generation.clone())
            }
            _ => return Err(refusal(StatusCode::NOT_FOUND, "not_found").into()),
        }
    };
    let caller = VerifiedCaller::verified_http(
        authority.identity.clone(),
        authority.deadline,
        authority.live.clone(),
    );
    let session = state
        .execution
        .sessions()
        .resume(caller, session_id)
        .ok()
        .filter(|session| session.opening().is(mounted.opening()))
        .ok_or_else(unauthorized)?;
    let greeting = state
        .execution
        .greeting(&session)
        .map_err(|_| unauthorized())?;
    let body = opening_body(session_id, &generation, greeting, authority.deadline);
    let encoded = serde_json::to_vec(&body).expect("the opening is plain JSON");
    let (check, deadline) = lifecycle_fences(&session);
    let ticket = output.bind(check, deadline, session.opening().clone());
    let mut response = Response::new(Body::from(crate::http::body::authority_bytes(
        encoded, ticket,
    )));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    Ok(response)
}
