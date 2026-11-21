//! Pending terminal upgrade credentials and exact socket admission.
//!
//! The registry lock serializes mint, redeem and PATCH for each mounted runtime.
//! Binding publication and live proof replacement also take the Opening output
//! gate, which the listener holds across physical output.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
    time::Duration,
};

use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderValue, Request, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::{sync::Notify, time::Instant};

use super::{
    auth_refusal, bearer, body_refusal, grant_expiry, refusal, resource_opening,
    resource_session_id, AuthError, HttpResult, HttpState, ResourceOpening, VerifiedAuthority,
    LIFECYCLE_BYTES,
};
use crate::execution::session::{
    AuthorizationLease, DeliveryError, EncodedEvent, EventBinding, EventSink, SessionLease,
    StreamClass, VerifiedCaller,
};
use crate::http::{
    limits::TerminalSocketReservation,
    listener::{terminal_upgrade::TerminalUpgrade, OutputAuthority},
    policy::{self, TerminalOrigin},
};

pub(super) const PATH: &str = "/v1/terminal-tickets";
const TOKEN_BYTES: usize = 32;
const MAX_AGE: Duration = Duration::from_millis(15_000);
const MAX_PENDING_FAMILY: usize = 8;
const MAX_PENDING_GLOBAL: usize = 128;
const ISSUE_BURST: u32 = 8;
const ISSUE_INTERVAL: Duration = Duration::from_millis(1_000);
type TicketDigest = [u8; 32];

impl VerifiedAuthority {
    fn ensure_terminal_scope(&self) -> Result<(), AuthError> {
        self.ensure_scope("bridge:read")?;
        self.ensure_scope("bridge:write")
    }

    fn same_terminal_credential(&self, other: &Self) -> bool {
        self.identity == other.identity
            && self.credential_digest == other.credential_digest
            && self.scopes == other.scopes
    }
}

#[derive(Default)]
pub(super) struct TerminalTickets {
    registry: Mutex<Registry>,
}

#[derive(Default)]
struct Registry {
    pending: HashMap<TicketDigest, PendingTicket>,
    last: HashMap<String, LastBinding>,
    /// Time at which this family's issue bucket is full again.
    full_at: HashMap<String, Instant>,
}

#[derive(Clone)]
struct PendingTicket {
    authority: VerifiedAuthority,
    session: SessionLease,
    generation: String,
    caller_origin: String,
    expires: Instant,
    predecessor: Option<EventBinding>,
}

struct LastBinding {
    binding: EventBinding,
    proof: Weak<TerminalProof>,
}

impl PendingTicket {
    fn new(
        session: SessionLease,
        generation: String,
        authority: VerifiedAuthority,
        caller_origin: String,
        now: Instant,
    ) -> Self {
        Self {
            expires: ticket_deadline(authority.deadline, now),
            authority,
            session,
            generation,
            caller_origin,
            predecessor: None,
        }
    }

    fn is_live(&self, now: Instant) -> bool {
        now < self.expires && self.session.authorization().is_valid() && self.authority.is_live()
    }
}

fn ticket_deadline(proof_deadline: Instant, now: Instant) -> Instant {
    proof_deadline.min(now + MAX_AGE)
}

impl Registry {
    fn reap(&mut self, now: Instant) {
        self.pending.retain(|_, ticket| ticket.is_live(now));
        self.full_at.retain(|_, full_at| *full_at > now);
    }

    fn issue(&mut self, mut ticket: PendingTicket, now: Instant) -> HttpResult<String> {
        self.reap(now);
        let family = &ticket.authority.identity.family_id;
        let mut pending_family = 0;
        let mut previous = None;
        for (digest, pending) in &self.pending {
            pending_family += usize::from(&pending.authority.identity.family_id == family);
            if pending.session.opening().is(ticket.session.opening()) {
                previous = Some(*digest);
            }
        }
        if previous.is_none()
            && (pending_family >= MAX_PENDING_FAMILY || self.pending.len() >= MAX_PENDING_GLOBAL)
        {
            return Err(overloaded());
        }
        self.charge_issue(family, now)?;
        let mut raw = [0; TOKEN_BYTES];
        SystemRandom::new()
            .fill(&mut raw)
            .map_err(|_| refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
        let digest = Sha256::digest(raw).into();
        if self.pending.contains_key(&digest) {
            return Err(refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable").into());
        }
        if let Some(previous) = previous {
            self.pending.remove(&previous);
        }
        ticket.predecessor = self
            .last
            .get(ticket.session.id())
            .map(|last| last.binding.clone());
        self.pending.insert(digest, ticket);
        Ok(URL_SAFE_NO_PAD.encode(raw))
    }

    fn charge_issue(&mut self, family: &str, now: Instant) -> HttpResult<()> {
        // Families can churn through Openings. Bound rate state independently
        // of pending tickets and keep family debt even after its last DELETE.
        let next = match self.full_at.get(family) {
            Some(&full_at) => full_at.max(now) + ISSUE_INTERVAL,
            None if self.full_at.len() >= super::MAX_OPENINGS => return Err(overloaded()),
            None => now + ISSUE_INTERVAL,
        };
        if next > now + ISSUE_INTERVAL * ISSUE_BURST {
            return Err(overloaded());
        }
        self.full_at.insert(family.to_owned(), next);
        Ok(())
    }
}

impl TerminalTickets {
    pub(super) fn reap(&self) {
        self.registry.lock().unwrap().reap(Instant::now());
    }

    pub(super) fn retire(&self, session: &SessionLease) {
        let mut registry = self.registry.lock().unwrap();
        registry.last.remove(session.id());
        registry
            .pending
            .retain(|_, ticket| !ticket.session.opening().is(session.opening()));
    }

    pub(super) fn clear(&self) {
        *self.registry.lock().unwrap() = Registry::default();
    }

    /// Run a successful lifecycle PATCH and its terminal-authority update in
    /// the same order as mint and redemption. The previous proof must still
    /// be live at the output gate for an exact renewal to replace it.
    pub(super) fn renew(
        &self,
        fresh: &VerifiedAuthority,
        resume: impl FnOnce() -> HttpResult<SessionLease>,
    ) -> HttpResult<SessionLease> {
        let mut registry = self.registry.lock().unwrap();
        let session = resume()?;
        let proof = registry
            .last
            .get(session.id())
            .and_then(|last| last.proof.upgrade());
        session
            .opening()
            .while_open(|| {
                let terminal_scope = fresh.ensure_terminal_scope().is_ok();
                registry.pending.retain(|_, ticket| {
                    !ticket.session.opening().is(session.opening())
                        || (terminal_scope && ticket.authority.same_terminal_credential(fresh))
                });
                if let Some(proof) = proof {
                    proof.renew(fresh);
                }
            })
            .ok_or_else(invalid)?;
        Ok(session)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IssueRequest {
    opening_generation: String,
}

pub(super) async fn create(State(state): State<HttpState>, request: Request<Body>) -> Response {
    issue_response(&state, request)
        .await
        .unwrap_or_else(|error| *error)
}

async fn issue_response(state: &HttpState, request: Request<Body>) -> HttpResult<Response> {
    if !state.accepting.load(Ordering::SeqCst) {
        return Err(invalid());
    }
    if request.uri().query().is_some() {
        return Err(refusal(StatusCode::BAD_REQUEST, "invalid_request").into());
    }
    let origin = policy::terminal_origin(&request)?;
    let bearer = bearer(request.headers()).ok_or_else(invalid)?;
    let session_id = resource_session_id(request.headers())?;
    let authority = state.verifier.verify(bearer).await.map_err(auth_refusal)?;
    authority.ensure_terminal_scope().map_err(auth_refusal)?;
    if !origin.matches_bridge(&authority.identity.origin) {
        return Err(policy::authority_mismatch().into());
    }
    let opening = resource_opening(state, session_id, &authority)?;
    let caller_origin = origin.caller().to_owned();
    let output = request
        .extensions()
        .get::<OutputAuthority>()
        .cloned()
        .ok_or_else(|| refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    let upgrade = request
        .extensions()
        .get::<TerminalUpgrade>()
        .cloned()
        .ok_or_else(|| refusal(StatusCode::SERVICE_UNAVAILABLE, "unavailable"))?;
    let deadline = authority.deadline;
    let body = crate::http::body::read_json_authorized(
        request.into_body(),
        LIFECYCLE_BYTES,
        deadline,
        || opening.authorization.is_valid(),
    )
    .await
    .map_err(body_refusal)?;
    let body: IssueRequest = serde_json::from_value(body)
        .map_err(|_| refusal(StatusCode::BAD_REQUEST, "invalid_request"))?;
    if opening.generation != body.opening_generation {
        return Err(invalid());
    }
    mint(state, opening, authority, caller_origin, output, upgrade)
}

fn mint(
    state: &HttpState,
    opening: ResourceOpening,
    authority: VerifiedAuthority,
    caller_origin: String,
    output: OutputAuthority,
    upgrade: TerminalUpgrade,
) -> HttpResult<Response> {
    let tickets = state.terminal_tickets.as_ref().ok_or_else(invalid)?;
    let mut registry = tickets.registry.lock().unwrap();
    let (token, expires) = opening
        .session
        .opening()
        .while_open(|| {
            let now = Instant::now();
            let ticket = PendingTicket::new(
                opening.session.clone(),
                opening.generation.clone(),
                authority,
                caller_origin,
                now,
            );
            if !state.accepting.load(Ordering::SeqCst) || !ticket.is_live(now) {
                return Err(invalid());
            }
            upgrade
                .check_capacity(&ticket.authority.identity.family_id)
                .map_err(|error| Box::new(error.into_response()))?;
            let expires = ticket.expires;
            Ok::<_, Box<Response>>((registry.issue(ticket, now)?, expires))
        })
        .ok_or_else(invalid)??;
    drop(registry);
    Ok(issue_output(opening, output, token, expires))
}

fn issue_output(
    opening: ResourceOpening,
    output: OutputAuthority,
    token: String,
    expires: Instant,
) -> Response {
    let body = json!({
        "ticket": token,
        "expires_at": grant_expiry(expires),
        "opening_generation": opening.generation,
    });
    let lifecycle = opening.session.authorization();
    let fence = output.bind(
        Arc::new(move || lifecycle.is_valid() && opening.authorization.is_valid()),
        Arc::new(move || expires),
        opening.session.opening().clone(),
    );
    let bytes = crate::http::body::authority_bytes(
        serde_json::to_vec(&body).expect("the credential reply is plain JSON"),
        fence,
    );
    let mut response = (StatusCode::CREATED, Body::from(bytes)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// The handshake supplies decoded ticket material and the request policy
/// supplies the verified origin. Only the digest reaches the registry.
pub(super) fn redeem(
    state: &HttpState,
    token: &[u8; TOKEN_BYTES],
    origin: TerminalOrigin<'_>,
    sink: Arc<dyn EventSink>,
    reserve: impl FnOnce(&str) -> HttpResult<TerminalSocketReservation>,
) -> HttpResult<TerminalAdmission> {
    let digest: TicketDigest = Sha256::digest(token).into();
    let tickets = state.terminal_tickets.as_ref().ok_or_else(invalid)?;
    let mut registry = tickets.registry.lock().unwrap();
    registry.reap(Instant::now());
    let ticket = registry.pending.get(&digest).ok_or_else(invalid)?;
    validate_redemption(state, ticket, origin)?;
    let held = reserve(&ticket.authority.identity.family_id)?;
    let ticket = registry
        .pending
        .remove(&digest)
        .expect("ticket held under registry lock");
    let prior = registry
        .last
        .get(ticket.session.id())
        .and_then(|last| last.proof.upgrade());
    let admission = bind(state, &ticket, prior.as_deref(), sink, held)?;
    registry.last.insert(
        ticket.session.id().into(),
        LastBinding {
            binding: admission.binding.clone(),
            proof: Arc::downgrade(&admission.proof),
        },
    );
    Ok(admission)
}

fn validate_redemption(
    state: &HttpState,
    ticket: &PendingTicket,
    origin: TerminalOrigin<'_>,
) -> HttpResult<()> {
    if !state.accepting.load(Ordering::SeqCst) {
        return Err(invalid());
    }
    if !origin.matches_bridge(&ticket.authority.identity.origin) {
        return Err(policy::authority_mismatch().into());
    }
    if ticket.caller_origin != origin.caller() {
        return Err(policy::origin_forbidden().into());
    }
    let openings = state.openings.lock().unwrap();
    let mounted = openings.get(ticket.session.id()).ok_or_else(invalid)?;
    if !mounted.session.opening().is(ticket.session.opening())
        || mounted.generation != ticket.generation
        || mounted.identity != ticket.authority.identity
    {
        return Err(invalid());
    }
    Ok(())
}

fn bind(
    state: &HttpState,
    ticket: &PendingTicket,
    prior: Option<&TerminalProof>,
    sink: Arc<dyn EventSink>,
    held: TerminalSocketReservation,
) -> HttpResult<TerminalAdmission> {
    let proof = Arc::new(TerminalProof {
        authority: Mutex::new(Some(ticket.authority.clone())),
        changed: Notify::new(),
    });
    let lifecycle = ticket.session.authorization();
    let binding = state
        .execution
        .sessions()
        .replace_events_if(
            &ticket.session,
            StreamClass::Terminal,
            ticket.predecessor.as_ref(),
            Arc::new(ProofSink {
                lifecycle: lifecycle.clone(),
                accepting: state.accepting.clone(),
                proof: Arc::downgrade(&proof),
                sink: Arc::downgrade(&sink),
            }),
            || {
                if !state.accepting.load(Ordering::SeqCst) || !ticket.is_live(Instant::now()) {
                    return false;
                }
                if let Some(prior) = prior {
                    prior.retire();
                }
                true
            },
        )
        .map_err(|_| invalid())?;
    Ok(TerminalAdmission {
        session: ticket.session.clone(),
        binding,
        lifecycle,
        proof,
        _sink: sink,
        accepting: state.accepting.clone(),
        held: Arc::new(held),
    })
}

fn invalid() -> Box<Response> {
    refusal(StatusCode::UNAUTHORIZED, "invalid_credential").into()
}

fn overloaded() -> Box<Response> {
    refusal(StatusCode::TOO_MANY_REQUESTS, "overloaded").into()
}

struct TerminalProof {
    authority: Mutex<Option<VerifiedAuthority>>,
    changed: Notify,
}

impl TerminalProof {
    fn current(&self) -> Option<VerifiedAuthority> {
        self.authority
            .lock()
            .unwrap()
            .as_ref()
            .filter(|proof| proof.is_live())
            .cloned()
    }

    fn is_live(&self) -> bool {
        self.authority
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(VerifiedAuthority::is_live)
    }

    fn deadline(&self) -> Option<Instant> {
        self.authority
            .lock()
            .unwrap()
            .as_ref()
            .map(|proof| proof.deadline)
    }

    fn retire(&self) {
        *self.authority.lock().unwrap() = None;
        self.changed.notify_waiters();
    }

    fn renew(&self, fresh: &VerifiedAuthority) {
        let mut proof = self.authority.lock().unwrap();
        match proof.as_mut() {
            Some(old)
                if old.is_live()
                    && old.same_terminal_credential(fresh)
                    && fresh.ensure_terminal_scope().is_ok() =>
            {
                if fresh.deadline > old.deadline {
                    *old = fresh.clone();
                }
            }
            _ => *proof = None,
        }
        self.changed.notify_waiters();
    }
}

/// A stored predecessor retains only weak references to socket resources.
struct ProofSink {
    lifecycle: AuthorizationLease,
    accepting: Arc<AtomicBool>,
    proof: Weak<TerminalProof>,
    sink: Weak<dyn EventSink>,
}

impl EventSink for ProofSink {
    fn is_live(&self) -> bool {
        self.accepting.load(Ordering::SeqCst)
            && self.proof.upgrade().is_some_and(|proof| proof.is_live())
            && self.lifecycle.is_valid()
    }

    fn try_send(&self, event: EncodedEvent) -> Result<(), DeliveryError> {
        if !self.is_live() {
            return Err(DeliveryError::Closed);
        }
        self.sink
            .upgrade()
            .ok_or(DeliveryError::Closed)?
            .try_send(event)
    }
}

/// Keep through the physical lifetime. A failed upgrade drops the binding and
/// releases the socket reservation, leaving the spent ticket unusable.
pub(super) struct TerminalAdmission {
    session: SessionLease,
    binding: EventBinding,
    lifecycle: AuthorizationLease,
    proof: Arc<TerminalProof>,
    _sink: Arc<dyn EventSink>,
    held: Arc<TerminalSocketReservation>,
    accepting: Arc<AtomicBool>,
}

impl TerminalAdmission {
    pub(super) fn session(&self) -> &SessionLease {
        &self.session
    }

    pub(super) fn retire(&self) {
        self.binding.close();
        self.proof.retire();
    }

    pub(super) fn reservation(&self) -> Arc<TerminalSocketReservation> {
        self.held.clone()
    }

    pub(super) async fn ended(&self) {
        let opening = self.session.opening();
        loop {
            let changed = self.proof.changed.notified();
            let Some(proof) = self.proof.current() else {
                return;
            };
            let revoked = proof.changed.notified();
            if !self.is_live() {
                return;
            }
            let deadline = self.deadline();
            tokio::select! {
                _ = changed => {},
                _ = opening.closed() => return,
                _ = tokio::time::sleep_until(deadline) => {},
                _ = revoked => {},
            }
        }
    }

    pub(super) fn binding(&self) -> &EventBinding {
        &self.binding
    }

    pub(super) fn is_live(&self) -> bool {
        self.accepting.load(Ordering::SeqCst)
            && self.binding.is_current()
            && self.lifecycle.is_valid()
            && self.proof.is_live()
    }

    pub(super) fn deadline(&self) -> Instant {
        let terminal = self.proof.deadline().unwrap_or_else(Instant::now);
        terminal.min(self.lifecycle.deadline().unwrap_or_else(Instant::now))
    }

    /// Capture one request's exact proof. A subsequent PATCH can extend the
    /// socket while the recorded authorization for this request stays fixed.
    pub(super) fn capture_caller(&self) -> Option<VerifiedCaller> {
        if !self.is_live() {
            return None;
        }
        let authority = self.proof.current()?;
        let binding = self.binding.clone();
        let lifecycle = self.lifecycle.clone();
        let proof = self.proof.clone();
        let accepting = self.accepting.clone();
        let deadline = authority.deadline.min(lifecycle.deadline()?);
        Some(VerifiedCaller::verified_http(
            authority.identity,
            deadline,
            Arc::new(move || {
                accepting.load(Ordering::SeqCst)
                    && binding.is_current()
                    && lifecycle.is_valid()
                    && proof.is_live()
                    && (authority.live)()
            }),
        ))
    }
}

impl Drop for TerminalAdmission {
    fn drop(&mut self) {
        self.retire();
    }
}

#[cfg(test)]
mod tests;
