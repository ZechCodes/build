//! Authenticated client openings and allocation-fenced application output.
//!
//! Sinks perform bounded, nonblocking enqueue only. Adapters recheck
//! `EncodedEvent::is_live` immediately before writing queued output.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use serde_json::Value;
use tokio::sync::Notify;
use tokio::time::Instant;

const MAX_REQUESTS: usize = 32;

/// Allocation identity, shared with the trusted rendezvous adapter.
#[derive(Clone)]
pub struct Opening(Arc<OpeningState>);

struct OpeningState {
    is_open: AtomicBool,
    client_hint: OnceLock<uuid::Uuid>,
    ordinal: u64,
    diagnostic_key: std::collections::hash_map::RandomState,
    closed: Notify,
    output_gate: Mutex<()>,
}

impl Opening {
    pub(crate) fn new() -> Self {
        static OPENED: AtomicU64 = AtomicU64::new(0);
        Self(Arc::new(OpeningState {
            is_open: AtomicBool::new(true),
            client_hint: OnceLock::new(),
            ordinal: OPENED.fetch_add(1, Ordering::Relaxed),
            diagnostic_key: std::collections::hash_map::RandomState::new(),
            closed: Notify::new(),
            output_gate: Mutex::new(()),
        }))
    }

    pub fn is_open(&self) -> bool {
        self.0.is_open.load(Ordering::SeqCst)
    }
    pub fn is(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    /// Serialize legacy adapter emission with exact opening invalidation.
    /// The callback must not reenter session methods taking this output gate.
    pub(crate) fn while_open<T>(&self, f: impl FnOnce() -> T) -> Option<T> {
        let _output = self.0.output_gate.lock().unwrap();
        self.is_open().then(f)
    }
    pub(crate) fn invalidate(&self) {
        let _output = self.0.output_gate.lock().unwrap();
        self.invalidate_unlocked();
    }
    fn invalidate_unlocked(&self) {
        self.0.is_open.store(false, Ordering::SeqCst);
        self.0.closed.notify_waiters();
    }
    pub(crate) fn diagnostic_id(&self) -> u64 {
        self.0.ordinal
    }
    pub(crate) fn diagnostic_key(&self) -> std::collections::hash_map::RandomState {
        self.0.diagnostic_key.clone()
    }
    pub(crate) fn bind_client_hint(&self, offered: Option<uuid::Uuid>) -> Option<uuid::Uuid> {
        if let Some(hint) = offered {
            let _ = self.0.client_hint.set(hint);
        }
        self.client_hint()
    }
    pub(crate) fn client_hint(&self) -> Option<uuid::Uuid> {
        self.0.client_hint.get().copied()
    }
    pub(crate) fn opened_after(&self, other: &Self) -> bool {
        self.0.ordinal > other.0.ordinal
    }
}

/// Verified binding metadata; possession of this value grants no authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpIdentity {
    pub issuer: String,
    pub account_id: String,
    pub device_id: String,
    pub family_id: String,
    pub origin: String,
    pub endpoint_epoch: String,
    pub authorization_epoch: u64,
}

type Validity = Arc<dyn Fn() -> bool + Send + Sync>;

/// Only trusted adapters may mint caller authority. Never deserialized.
pub struct VerifiedCaller {
    identity: Identity,
    deadline: Option<Instant>,
    live: Validity,
    opening: Opening,
}

#[derive(Clone, PartialEq, Eq)]
enum Identity {
    Http(HttpIdentity),
    Relay(String),
}

impl VerifiedCaller {
    /// Called only after the HTTP account verifier has validated the grant.
    pub(crate) fn verified_http(identity: HttpIdentity, deadline: Instant, live: Validity) -> Self {
        Self {
            identity: Identity::Http(identity),
            deadline: Some(deadline),
            live,
            opening: Opening::new(),
        }
    }
    /// Called only at authenticated, owner-authorized relay intake.
    pub(crate) fn verified_relay(device_id: impl Into<String>, opening: Opening) -> Self {
        Self {
            identity: Identity::Relay(device_id.into()),
            deadline: None,
            live: Arc::new(|| true),
            opening,
        }
    }
    pub fn device_id(&self) -> &str {
        match &self.identity {
            Identity::Http(identity) => &identity.device_id,
            Identity::Relay(device) => device,
        }
    }
}

struct AuthorizationState {
    deadline: Option<Instant>,
    live: Validity,
    retired: bool,
}

/// A live proof fence, separate from update or executor admission.
#[derive(Clone)]
pub struct AuthorizationLease {
    opening: Opening,
    state: Arc<Mutex<AuthorizationState>>,
}

impl AuthorizationLease {
    pub fn is_valid(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if !self.opening.is_open() || state.retired {
            return false;
        }
        let valid = state
            .deadline
            .is_none_or(|deadline| Instant::now() < deadline)
            && (state.live)();
        if !valid {
            state.retired = true;
        }
        valid
    }
    pub fn deadline(&self) -> Option<Instant> {
        self.state.lock().unwrap().deadline
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    #[error("session is closed")]
    Closed,
    #[error("caller authorization expired or was revoked")]
    Unauthorized,
    #[error("verified caller binding differs")]
    WrongCaller,
    #[error("binding belongs to another opening")]
    WrongOpening,
    #[error("opening request limit reached")]
    Full,
    #[error("this opening already has a shared session")]
    AlreadyOpen,
    #[error("event replacement does not name the last binding")]
    StaleBinding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DeliveryError {
    #[error("output binding is closed")]
    Closed,
    #[error("output queue is full")]
    Full,
    #[error("output caller is unauthorized")]
    Unauthorized,
    #[error("output exceeds the adapter byte budget")]
    TooLarge,
    #[error("application output must be an object")]
    InvalidPayload,
}

/// Implementations must not block or reenter ClientSessions during enqueue.
pub trait EventSink: Send + Sync {
    fn try_send(&self, event: EncodedEvent) -> Result<(), DeliveryError>;
}
pub trait ReplySink: Send + Sync {
    fn try_send(&self, event: EncodedEvent) -> Result<(), DeliveryError>;
}

/// A stamped application object, encoded once, with its exact delivery fence.
#[derive(Clone)]
pub struct EncodedEvent {
    payload: Value,
    encoded: Arc<[u8]>,
    fence: OutputFence,
}
impl EncodedEvent {
    fn new(mut payload: Value, fence: OutputFence) -> Result<Self, DeliveryError> {
        let object = payload
            .as_object_mut()
            .ok_or(DeliveryError::InvalidPayload)?;
        object.insert(
            "bridge_epoch".into(),
            Value::String(
                fence
                    .session
                    .upgrade()
                    .ok_or(DeliveryError::Closed)?
                    .epoch
                    .to_string(),
            ),
        );
        let encoded = serde_json::to_vec(&payload)
            .map_err(|_| DeliveryError::InvalidPayload)?
            .into();
        Ok(Self {
            payload,
            encoded,
            fence,
        })
    }
    pub fn payload(&self) -> &Value {
        &self.payload
    }
    pub fn bytes(&self) -> &[u8] {
        &self.encoded
    }
    pub fn is_live(&self) -> bool {
        self.fence.is_live()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamClass {
    Application,
    Terminal,
}
impl StreamClass {
    fn index(self) -> usize {
        match self {
            Self::Application => 0,
            Self::Terminal => 1,
        }
    }
}

struct SessionState {
    bindings: [Option<Arc<EventState>>; 2],
    last_bindings: [Option<Arc<AtomicBool>>; 2],
    inflight: usize,
}
struct Session {
    id: String,
    epoch: Arc<str>,
    opening: Opening,
    identity: Identity,
    authorization: AuthorizationLease,
    state: Mutex<SessionState>,
}

#[derive(Clone)]
pub struct SessionLease(Arc<Session>);
impl SessionLease {
    pub fn id(&self) -> &str {
        &self.0.id
    }
    pub fn opening(&self) -> &Opening {
        &self.0.opening
    }
    pub fn principal(&self) -> &'static str {
        "device_user"
    }
    pub fn authorization(&self) -> AuthorizationLease {
        self.0.authorization.clone()
    }
    pub fn is_live(&self) -> bool {
        self.0.authorization.is_valid()
    }
}

/// One registry is shared by every transport adapter in the daemon.
pub struct ClientSessions {
    epoch: Arc<str>,
    sessions: Mutex<HashMap<String, SessionLease>>,
    request_order: AtomicU64,
}

impl ClientSessions {
    pub fn new(epoch: impl Into<Arc<str>>) -> Self {
        Self {
            epoch: epoch.into(),
            sessions: Mutex::new(HashMap::new()),
            request_order: AtomicU64::new(1),
        }
    }
    pub fn open(&self, caller: VerifiedCaller) -> Result<SessionLease, SessionError> {
        let authorization = AuthorizationLease {
            opening: caller.opening.clone(),
            state: Arc::new(Mutex::new(AuthorizationState {
                deadline: caller.deadline,
                live: caller.live,
                retired: false,
            })),
        };
        if !authorization.is_valid() {
            return Err(SessionError::Unauthorized);
        }
        let session = SessionLease(Arc::new(Session {
            id: uuid::Uuid::new_v4().to_string(),
            epoch: self.epoch.clone(),
            opening: caller.opening,
            identity: caller.identity,
            authorization,
            state: Mutex::new(SessionState {
                bindings: [None, None],
                last_bindings: [None, None],
                inflight: 0,
            }),
        }));
        let mut sessions = self.sessions.lock().unwrap();
        if sessions
            .values()
            .any(|existing| existing.opening().is(session.opening()))
        {
            return Err(SessionError::AlreadyOpen);
        }
        sessions.insert(session.id().into(), session.clone());
        Ok(session)
    }
    /// Renewal requires a newly verified proof of the exact HTTP family binding.
    pub fn resume(&self, caller: VerifiedCaller, id: &str) -> Result<SessionLease, SessionError> {
        let session = self
            .sessions
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or(SessionError::Closed)?;
        let _output = session.opening().0.output_gate.lock().unwrap();
        let _state = session.0.state.lock().unwrap();
        if !session.opening().is_open() {
            return Err(SessionError::Closed);
        }
        if !session.is_live() {
            return Err(SessionError::Unauthorized);
        }
        if !matches!(caller.identity, Identity::Http(_)) || caller.identity != session.0.identity {
            return Err(SessionError::WrongCaller);
        }
        if caller
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
            || !(caller.live)()
        {
            return Err(SessionError::Unauthorized);
        }
        let mut authorization = session.0.authorization.state.lock().unwrap();
        authorization.deadline = caller.deadline;
        authorization.live = caller.live;
        drop(authorization);
        drop(_state);
        drop(_output);
        Ok(session)
    }
    pub fn bind_events(
        &self,
        session: &SessionLease,
        class: StreamClass,
        sink: Arc<dyn EventSink>,
    ) -> Result<EventBinding, SessionError> {
        self.replace_events(session, class, None, sink)
    }
    /// Compare against the last allocation, including its closed tombstone.
    pub fn replace_events(
        &self,
        session: &SessionLease,
        class: StreamClass,
        expected: Option<&EventBinding>,
        sink: Arc<dyn EventSink>,
    ) -> Result<EventBinding, SessionError> {
        let _output = session.opening().0.output_gate.lock().unwrap();
        let mut state = session.0.state.lock().unwrap();
        Self::authorize(session)?;
        if let Some(expected) = expected {
            if !expected.session.opening().is(session.opening()) || expected.class != class {
                return Err(SessionError::WrongOpening);
            }
        }
        let matches_last = match (&state.last_bindings[class.index()], expected) {
            (None, None) => true,
            (Some(last), Some(expected)) => Arc::ptr_eq(last, &expected.binding.live),
            _ => false,
        };
        if !matches_last {
            return Err(SessionError::StaleBinding);
        }
        // Legacy carrier IDs occupy the low half; application bindings have a
        // disjoint namespace while both output paths coexist during extraction.
        static BINDINGS: AtomicU64 = AtomicU64::new(1);
        let binding = Arc::new(EventState {
            ordinal: BINDINGS.fetch_add(1, Ordering::Relaxed) | (1 << 63),
            live: Arc::new(AtomicBool::new(true)),
            sink,
        });
        if let Some(old) = state.bindings[class.index()].replace(binding.clone()) {
            old.live.store(false, Ordering::SeqCst);
        }
        state.last_bindings[class.index()] = Some(binding.live.clone());
        Ok(EventBinding {
            session: session.clone(),
            class,
            binding,
        })
    }
    pub fn request(
        &self,
        session: &SessionLease,
        sink: Arc<dyn ReplySink>,
        events: Option<&EventBinding>,
    ) -> Result<RequestContext, SessionError> {
        let _output = session.opening().0.output_gate.lock().unwrap();
        let mut state = session.0.state.lock().unwrap();
        Self::authorize(session)?;
        if let Some(events) = events {
            if !events.session.opening().is(session.opening()) {
                return Err(SessionError::WrongOpening);
            }
            if !events.binding.live.load(Ordering::SeqCst) {
                return Err(SessionError::Closed);
            }
        }
        if state.inflight >= MAX_REQUESTS {
            return Err(SessionError::Full);
        }
        state.inflight += 1;
        let cancellation = RequestCancellation {
            state: Arc::new(CancellationState {
                cancelled: AtomicBool::new(false),
                notify: Notify::new(),
            }),
            authorization: session.authorization(),
        };
        Ok(RequestContext {
            session: session.clone(),
            reply: ReplyLease {
                session: session.clone(),
                sink,
                cancellation,
            },
            events: events.map(EventBinding::events),
            request_order: self.request_order.fetch_add(1, Ordering::Relaxed),
            intake_time: Instant::now(),
        })
    }
    pub fn close(&self, session: &SessionLease) {
        let _output = session.opening().0.output_gate.lock().unwrap();
        let mut state = session.0.state.lock().unwrap();
        session.opening().invalidate_unlocked();
        for binding in &mut state.bindings {
            if let Some(binding) = binding.take() {
                binding.live.store(false, Ordering::SeqCst);
            }
        }
        self.sessions.lock().unwrap().remove(session.id());
    }
    fn authorize(session: &SessionLease) -> Result<(), SessionError> {
        if !session.opening().is_open() {
            return Err(SessionError::Closed);
        }
        if !session.is_live() {
            return Err(SessionError::Unauthorized);
        }
        Ok(())
    }
}

struct EventState {
    ordinal: u64,
    live: Arc<AtomicBool>,
    sink: Arc<dyn EventSink>,
}

/// Replacing this handle retires only its stream class, never request replies.
#[derive(Clone)]
pub struct EventBinding {
    session: SessionLease,
    class: StreamClass,
    binding: Arc<EventState>,
}
impl EventBinding {
    pub fn events(&self) -> SessionEvents {
        SessionEvents(self.clone())
    }
    pub fn is_live(&self) -> bool {
        self.binding.live.load(Ordering::SeqCst) && self.session.is_live()
    }
    pub fn close(&self) {
        let _output = self.session.opening().0.output_gate.lock().unwrap();
        let mut state = self.session.0.state.lock().unwrap();
        self.binding.live.store(false, Ordering::SeqCst);
        let slot = &mut state.bindings[self.class.index()];
        if slot
            .as_ref()
            .is_some_and(|binding| Arc::ptr_eq(binding, &self.binding))
        {
            *slot = None;
        }
    }
}

#[derive(Clone)]
pub struct SessionEvents(EventBinding);
impl SessionEvents {
    pub fn session(&self) -> &SessionLease {
        &self.0.session
    }
    pub fn opening(&self) -> &Opening {
        self.session().opening()
    }
    pub fn read_scope(&self) -> (u64, Option<u64>) {
        (self.opening().diagnostic_id(), Some(self.0.binding.ordinal))
    }

    pub fn push(&self, payload: Value) -> Result<(), DeliveryError> {
        let binding = &self.0;
        let _output = binding.session.opening().0.output_gate.lock().unwrap();
        let mut state = binding.session.0.state.lock().unwrap();
        let fence = OutputFence {
            session: Arc::downgrade(&binding.session.0),
            target: OutputTarget::Event(binding.binding.live.clone()),
        };
        fence.check()?;
        let result = binding
            .binding
            .sink
            .try_send(EncodedEvent::new(payload, fence)?);
        if matches!(
            result,
            Err(DeliveryError::Full | DeliveryError::TooLarge | DeliveryError::Closed)
        ) {
            binding.binding.live.store(false, Ordering::SeqCst);
            state.bindings[binding.class.index()] = None;
        }
        result
    }
    pub fn is_live(&self) -> bool {
        self.0.is_live()
    }
}

struct CancellationState {
    cancelled: AtomicBool,
    notify: Notify,
}
#[derive(Clone)]
pub struct RequestCancellation {
    state: Arc<CancellationState>,
    authorization: AuthorizationLease,
}
impl RequestCancellation {
    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::SeqCst) || !self.authorization.is_valid()
    }
    pub fn cancel(&self) {
        self.state.cancelled.store(true, Ordering::SeqCst);
        self.state.notify.notify_waiters();
    }
    pub async fn cancelled(&self) {
        loop {
            let notified = self.state.notify.notified();
            let closed = self.authorization.opening.0.closed.notified();
            tokio::pin!(notified, closed);
            notified.as_mut().enable();
            closed.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            let deadline = self.authorization.deadline();
            tokio::select! {
                _ = notified => {},
                _ = closed => {},
                _ = async {
                    match deadline {
                        Some(deadline) => tokio::time::sleep_until(deadline).await,
                        None => std::future::pending().await,
                    }
                } => {},
            }
        }
    }
}

#[derive(Clone)]
pub struct ReplyLease {
    session: SessionLease,
    sink: Arc<dyn ReplySink>,
    cancellation: RequestCancellation,
}
impl ReplyLease {
    pub fn send(&self, payload: Value) -> Result<(), DeliveryError> {
        let _output = self.session.opening().0.output_gate.lock().unwrap();
        let _state = self.session.0.state.lock().unwrap();
        let fence = OutputFence {
            session: Arc::downgrade(&self.session.0),
            target: OutputTarget::Reply(self.cancellation.clone()),
        };
        fence.check()?;
        self.sink.try_send(EncodedEvent::new(payload, fence)?)
    }
    pub fn is_live(&self) -> bool {
        !self.cancellation.is_cancelled() && self.session.is_live()
    }
}

pub struct RequestContext {
    session: SessionLease,
    reply: ReplyLease,
    events: Option<SessionEvents>,
    request_order: u64,
    intake_time: Instant,
}
impl RequestContext {
    pub fn session(&self) -> &SessionLease {
        &self.session
    }
    pub fn authorization(&self) -> AuthorizationLease {
        self.session.authorization()
    }
    pub fn reply(&self) -> ReplyLease {
        self.reply.clone()
    }
    pub fn events(&self) -> Option<SessionEvents> {
        self.events.clone()
    }
    pub fn request_order(&self) -> u64 {
        self.request_order
    }
    pub fn intake_time(&self) -> Instant {
        self.intake_time
    }
    pub fn cancellation(&self) -> RequestCancellation {
        self.reply.cancellation.clone()
    }
    pub fn is_live(&self) -> bool {
        self.reply.is_live()
    }
    pub async fn cancelled(&self) {
        self.reply.cancellation.cancelled().await
    }
}
impl Drop for RequestContext {
    fn drop(&mut self) {
        let mut state = self.session.0.state.lock().unwrap();
        self.reply.cancellation.cancel();
        state.inflight -= 1;
    }
}

#[derive(Clone)]
struct OutputFence {
    session: Weak<Session>,
    target: OutputTarget,
}
#[derive(Clone)]
enum OutputTarget {
    Event(Arc<AtomicBool>),
    Reply(RequestCancellation),
}
impl OutputFence {
    /// Caller holds the session output gate during the synchronous enqueue.
    fn check(&self) -> Result<(), DeliveryError> {
        let session = self.session.upgrade().ok_or(DeliveryError::Closed)?;
        let live = match &self.target {
            OutputTarget::Event(live) => live.load(Ordering::SeqCst),
            OutputTarget::Reply(cancellation) => !cancellation.is_cancelled(),
        };
        if !live || !session.opening.is_open() {
            return Err(DeliveryError::Closed);
        }
        if !session.authorization.is_valid() {
            return Err(DeliveryError::Unauthorized);
        }
        Ok(())
    }
    fn is_live(&self) -> bool {
        let Some(session) = self.session.upgrade() else {
            return false;
        };
        let _output = session.opening.0.output_gate.lock().unwrap();
        let _state = session.state.lock().unwrap();
        self.check().is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    #[derive(Default)]
    struct Sink(Mutex<Vec<EncodedEvent>>);
    impl EventSink for Sink {
        fn try_send(&self, event: EncodedEvent) -> Result<(), DeliveryError> {
            self.0.lock().unwrap().push(event);
            Ok(())
        }
    }
    impl ReplySink for Sink {
        fn try_send(&self, event: EncodedEvent) -> Result<(), DeliveryError> {
            EventSink::try_send(self, event)
        }
    }

    fn identity() -> HttpIdentity {
        HttpIdentity {
            issuer: "issuer".into(),
            account_id: "owner".into(),
            device_id: "device".into(),
            family_id: "family".into(),
            origin: "https://device.example".into(),
            endpoint_epoch: "endpoint-1".into(),
            authorization_epoch: 1,
        }
    }
    fn caller() -> VerifiedCaller {
        VerifiedCaller::verified_http(
            identity(),
            Instant::now() + Duration::from_secs(20),
            Arc::new(|| true),
        )
    }

    #[test]
    fn replaced_application_stream_does_not_retire_terminal_or_independent_reply() {
        let sessions = ClientSessions::new("epoch");
        let session = sessions.open(caller()).unwrap();
        let old_sink = Arc::new(Sink::default());
        let old = sessions
            .bind_events(&session, StreamClass::Application, old_sink.clone())
            .unwrap();
        let term = sessions
            .bind_events(&session, StreamClass::Terminal, Arc::new(Sink::default()))
            .unwrap();
        let reply_sink = Arc::new(Sink::default());
        let request = sessions
            .request(&session, reply_sink.clone(), Some(&old))
            .unwrap();
        let fresh = sessions
            .replace_events(
                &session,
                StreamClass::Application,
                Some(&old),
                Arc::new(Sink::default()),
            )
            .unwrap();
        assert_eq!(
            old.events().push(json!({"type":"old"})),
            Err(DeliveryError::Closed)
        );
        old.close();
        fresh.events().push(json!({"type":"fresh"})).unwrap();
        term.events().push(json!({"type":"terminal"})).unwrap();
        request
            .reply()
            .send(json!({"ok":true,"bridge_epoch":"spoof"}))
            .unwrap();
        assert_eq!(
            reply_sink.0.lock().unwrap()[0].payload()["bridge_epoch"],
            "epoch"
        );
        assert!(old_sink.0.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn deadline_and_shared_revocation_fence_admission_and_output() {
        let live = Arc::new(AtomicBool::new(true));
        let captured = live.clone();
        let caller = VerifiedCaller::verified_http(
            identity(),
            Instant::now() + Duration::from_secs(20),
            Arc::new(move || captured.load(Ordering::SeqCst)),
        );
        let sessions = ClientSessions::new("epoch");
        let session = sessions.open(caller).unwrap();
        let request = sessions
            .request(&session, Arc::new(Sink::default()), None)
            .unwrap();
        live.store(false, Ordering::SeqCst);
        assert!(!request.is_live());
        assert_eq!(
            request.reply().send(json!({"ok":true})),
            Err(DeliveryError::Closed)
        );
        assert!(matches!(
            sessions.request(&session, Arc::new(Sink::default()), None),
            Err(SessionError::Unauthorized)
        ));
        let other = sessions.open(super::tests::caller()).unwrap();
        tokio::time::advance(Duration::from_secs(21)).await;
        assert!(!other.authorization().is_valid());
        assert!(matches!(
            sessions.resume(super::tests::caller(), other.id()),
            Err(SessionError::Unauthorized)
        ));
    }

    #[test]
    fn renewal_requires_every_binding_field_and_preserves_principal() {
        let sessions = ClientSessions::new("epoch");
        let session = sessions.open(caller()).unwrap();
        let mut changes: Vec<HttpIdentity> = Vec::new();
        for field in 0..8 {
            let mut changed = identity();
            match field {
                0 => changed.issuer.push('x'),
                1 => changed.account_id.push('x'),
                2 => changed.device_id.push('x'),
                3 => changed.family_id.push('x'),
                4 => changed.origin.push('x'),
                5 => changed.endpoint_epoch.push('x'),
                6 => changed.authorization_epoch += 1,
                _ => changed.account_id.clear(),
            }
            changes.push(changed);
        }
        for changed in changes {
            let different = VerifiedCaller::verified_http(
                changed,
                Instant::now() + Duration::from_secs(25),
                Arc::new(|| true),
            );
            assert!(matches!(
                sessions.resume(different, session.id()),
                Err(SessionError::WrongCaller)
            ));
        }
        let renewed = sessions.resume(caller(), session.id()).unwrap();
        assert!(session.opening().is(renewed.opening()));
        assert_eq!(renewed.principal(), "device_user");
        sessions.close(&session);
        assert!(matches!(
            sessions.resume(caller(), session.id()),
            Err(SessionError::Closed)
        ));
    }

    #[tokio::test]
    async fn shared_request_limit_drop_cancellation_and_foreign_binding() {
        let sessions = ClientSessions::new("epoch");
        let session = sessions.open(caller()).unwrap();
        let other = sessions.open(caller()).unwrap();
        let foreign = sessions
            .bind_events(&other, StreamClass::Application, Arc::new(Sink::default()))
            .unwrap();
        assert!(matches!(
            sessions.request(&session, Arc::new(Sink::default()), Some(&foreign)),
            Err(SessionError::WrongOpening)
        ));
        let mut requests = Vec::new();
        for _ in 0..32 {
            requests.push(
                sessions
                    .request(&session, Arc::new(Sink::default()), None)
                    .unwrap(),
            );
        }
        assert!(matches!(
            sessions.request(&session, Arc::new(Sink::default()), None),
            Err(SessionError::Full)
        ));
        let request = requests.pop().unwrap();
        let reply = request.reply();
        let cancellation = request.cancellation();
        drop(request);
        cancellation.cancelled().await;
        assert_eq!(reply.send(json!({"ok":true})), Err(DeliveryError::Closed));
        assert!(sessions
            .request(&session, Arc::new(Sink::default()), None)
            .is_ok());
    }

    #[test]
    fn close_is_exact_and_relay_authority_does_not_expire() {
        let sessions = ClientSessions::new("epoch");
        let opening = Opening::new();
        let relay = sessions
            .open(VerifiedCaller::verified_relay("device", opening.clone()))
            .unwrap();
        assert!(opening.is(relay.opening()));
        assert_eq!(relay.principal(), "device_user");
        let event = sessions
            .bind_events(&relay, StreamClass::Application, Arc::new(Sink::default()))
            .unwrap();
        sessions.close(&relay);
        assert!(!opening.is_open());
        assert_eq!(
            event.events().push(json!({"type":"event"})),
            Err(DeliveryError::Closed)
        );
        let fresh = sessions
            .open(VerifiedCaller::verified_relay("device", Opening::new()))
            .unwrap();
        sessions.close(&relay);
        assert!(fresh.is_live());
    }

    struct FullSink(AtomicUsize);
    impl EventSink for FullSink {
        fn try_send(&self, _: EncodedEvent) -> Result<(), DeliveryError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(DeliveryError::Full)
        }
    }
    #[test]
    fn overflow_retires_only_exact_event_binding() {
        let sessions = ClientSessions::new("epoch");
        let session = sessions.open(caller()).unwrap();
        let sink = Arc::new(FullSink(AtomicUsize::new(0)));
        let binding = sessions
            .bind_events(&session, StreamClass::Application, sink.clone())
            .unwrap();
        assert_eq!(
            binding.events().push(json!({"type":"event"})),
            Err(DeliveryError::Full)
        );
        assert_eq!(
            binding.events().push(json!({"type":"event"})),
            Err(DeliveryError::Closed)
        );
        assert_eq!(sink.0.load(Ordering::SeqCst), 1);
        assert!(session.is_live());
    }
    #[test]
    fn one_opening_cannot_allocate_two_independent_admission_budgets() {
        let sessions = ClientSessions::new("epoch");
        let opening = Opening::new();
        let first = sessions
            .open(VerifiedCaller::verified_relay("device", opening.clone()))
            .unwrap();
        let duplicate = sessions.open(VerifiedCaller::verified_relay("device", opening));
        assert!(matches!(duplicate, Err(SessionError::AlreadyOpen)));
        assert!(first.is_live());
    }

    #[tokio::test(start_paused = true)]
    async fn expiry_and_opening_close_wake_queued_request_waiters() {
        let sessions = ClientSessions::new("epoch");
        let session = sessions.open(caller()).unwrap();
        let request = sessions
            .request(&session, Arc::new(Sink::default()), None)
            .unwrap();
        let cancellation = request.cancellation();
        tokio::time::advance(Duration::from_secs(21)).await;
        cancellation.cancelled().await;
        assert!(!request.is_live());
        let other = sessions.open(caller()).unwrap();
        let request = sessions
            .request(&other, Arc::new(Sink::default()), None)
            .unwrap();
        let cancellation = request.cancellation();
        sessions.close(&other);
        cancellation.cancelled().await;
    }

    #[test]
    fn queued_payloads_are_fenced_at_adapter_write_and_released_without_cycles() {
        let sessions = ClientSessions::new("epoch");
        let session = sessions.open(caller()).unwrap();
        let weak_session = Arc::downgrade(&session.0);
        let sink = Arc::new(Sink::default());
        let binding = sessions
            .bind_events(&session, StreamClass::Application, sink.clone())
            .unwrap();
        binding.events().push(json!({"type":"update"})).unwrap();
        assert!(sink.0.lock().unwrap()[0].is_live());
        sessions.close(&session);
        assert!(!sink.0.lock().unwrap()[0].is_live());
        drop(binding);
        drop(session);
        assert!(weak_session.upgrade().is_none());
    }

    #[test]
    fn event_replacement_requires_last_allocation_even_after_close_or_overflow() {
        let sessions = ClientSessions::new("epoch");
        let session = sessions.open(caller()).unwrap();
        let first = sessions
            .bind_events(
                &session,
                StreamClass::Application,
                Arc::new(Sink::default()),
            )
            .unwrap();
        assert!(matches!(
            sessions.bind_events(
                &session,
                StreamClass::Application,
                Arc::new(Sink::default())
            ),
            Err(SessionError::StaleBinding)
        ));
        first.close();
        assert!(matches!(
            sessions.bind_events(
                &session,
                StreamClass::Application,
                Arc::new(Sink::default())
            ),
            Err(SessionError::StaleBinding)
        ));
        let second = sessions
            .replace_events(
                &session,
                StreamClass::Application,
                Some(&first),
                Arc::new(FullSink(AtomicUsize::new(0))),
            )
            .unwrap();
        assert_eq!(
            second.events().push(json!({"type":"event"})),
            Err(DeliveryError::Full)
        );
        assert!(matches!(
            sessions.replace_events(
                &session,
                StreamClass::Application,
                Some(&first),
                Arc::new(Sink::default())
            ),
            Err(SessionError::StaleBinding)
        ));
        let third = sessions
            .replace_events(
                &session,
                StreamClass::Application,
                Some(&second),
                Arc::new(Sink::default()),
            )
            .unwrap();
        second.close();
        third.events().push(json!({"type":"current"})).unwrap();
    }
    #[test]
    fn event_scope_and_legacy_output_gate_preserve_exact_opening_identity() {
        let sessions = ClientSessions::new("epoch");
        let session = sessions.open(caller()).unwrap();
        let app = sessions
            .bind_events(
                &session,
                StreamClass::Application,
                Arc::new(Sink::default()),
            )
            .unwrap();
        let events = app.events();
        assert!(events.opening().is(session.opening()));
        assert_eq!(events.session().id(), session.id());
        let (opening, binding) = events.read_scope();
        assert_eq!(opening, session.opening().diagnostic_id());
        assert!(binding.unwrap() & (1 << 63) != 0);
        assert_eq!(session.opening().while_open(|| 7), Some(7));
        let fresh = sessions
            .replace_events(
                &session,
                StreamClass::Application,
                Some(&app),
                Arc::new(Sink::default()),
            )
            .unwrap();
        assert_ne!(events.read_scope(), fresh.events().read_scope());
        session.opening().invalidate();
        assert_eq!(session.opening().while_open(|| 9), None);
    }
}
