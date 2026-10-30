//! Application caller/output compatibility while producers migrate to events.
//! No carrier framing, session key or transport codec enters this boundary.

use super::session::{Opening, RequestContext};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

type Output = Arc<dyn Fn(Value) -> bool + Send + Sync>;
type Liveness = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Clone)]
pub struct CallContext {
    session_id: String,
    bridge_epoch: Arc<str>,
    opening: Opening,
    events: Output,
    reply: Output,
    session_live: Liveness,
    reply_live: Liveness,
    read_live: Liveness,
    request_scoped: bool,
    event_live: Liveness,
    scope: Option<u64>,
    request_order: u64,
    has_events: bool,
    subscription_limit: Option<usize>,
    gate: Arc<Mutex<()>>,
}

/// Application object prepared outside producer locks; framing belongs to sinks.
pub(crate) struct PreparedOutput(Value);

impl CallContext {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub(crate) fn bridge_epoch(&self) -> &str {
        &self.bridge_epoch
    }
    pub(crate) fn request_order(&self) -> u64 {
        self.request_order
    }
    pub(crate) fn set_request_order(&mut self, order: u64) {
        self.request_order = order;
    }
    pub(crate) fn read_scope(&self) -> (u64, Option<u64>) {
        (self.opening.diagnostic_id(), self.scope)
    }
    pub(crate) fn opening(&self) -> Opening {
        self.opening.clone()
    }
    pub(crate) fn session_is_open(&self) -> bool {
        self.opening.is_open() && (self.session_live)()
    }
    pub(crate) fn read_is_live(&self) -> bool {
        self.session_is_open() && (self.reply_live)() && (self.read_live)()
    }
    /// Legacy admitted work survives one carrier closing while its opening lives.
    /// Per-request work additionally retains the initiating request's cancellation.
    pub(crate) fn execution_is_live(&self) -> bool {
        self.session_is_open() && (!self.request_scoped || (self.reply_live)())
    }
    pub(crate) fn method_is_live(&self, method: &str) -> bool {
        self.execution_is_live() && (!super::requires_read_interest(method) || self.read_is_live())
    }
    pub fn has_events(&self) -> bool {
        self.has_events
    }
    pub(crate) fn events_are_live(&self) -> bool {
        self.has_events && self.session_is_open() && (self.event_live)()
    }
    pub(crate) fn logical_subscription_limit(&self) -> Option<usize> {
        self.subscription_limit
    }
    /// A pure folded read retains all live interests through worker waits.
    pub(crate) fn for_fold(mut self, callers: Vec<Self>) -> Self {
        self.request_scoped = true;
        self.reply_live = Arc::new(move || callers.iter().any(Self::read_is_live));
        self.read_live = Arc::new(|| true);
        self
    }

    /// Producer state must never retain one request's response/budget resources.
    pub(crate) fn event_context(&self) -> Self {
        let mut producer = self.clone();
        producer.reply = Arc::new(|_| false);
        producer.reply_live = producer.event_live.clone();
        producer.read_live = Arc::new(|| true);
        producer
    }

    pub(crate) fn event_liveness(&self) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let opening = self.opening.clone();
        let session = self.session_live.clone();
        let event = self.event_live.clone();
        let has_events = self.has_events;
        Arc::new(move || has_events && opening.is_open() && session() && event())
    }
    pub(crate) fn with_reply_observer(
        mut self,
        observe: impl Fn(&Value) + Send + Sync + 'static,
    ) -> Self {
        let send = self.reply.clone();
        self.reply = Arc::new(move |value| {
            let accepted = send(value.clone());
            if accepted {
                observe(&value);
            }
            accepted
        });
        self
    }

    pub(crate) fn legacy(session_id: &str, opening: Opening, send: Output) -> Self {
        Self {
            session_id: session_id.into(),
            bridge_epoch: crate::bridge_epoch::current().into(),
            opening,
            events: send.clone(),
            reply: send,
            session_live: Arc::new(|| true),
            reply_live: Arc::new(|| true),
            read_live: Arc::new(|| true),
            request_scoped: false,
            event_live: Arc::new(|| true),
            scope: None,
            request_order: 0,
            has_events: true,
            subscription_limit: None,
            gate: Arc::new(Mutex::new(())),
        }
    }

    pub(crate) fn from_request(request: &RequestContext) -> Self {
        let session = request.session().clone();
        let reply = request.reply();
        let reply_live = reply.clone();
        let events = request.events();
        let event_live = events.clone();
        let scope = events.as_ref().and_then(|events| events.read_scope().1);
        Self {
            session_id: session.id().into(),
            bridge_epoch: crate::bridge_epoch::current().into(),
            opening: session.opening().clone(),
            has_events: events.is_some(),
            subscription_limit: Some(8),
            scope,
            request_order: request.request_order(),
            events: Arc::new(move |payload| {
                events
                    .as_ref()
                    .is_some_and(|events| events.push(payload).is_ok())
            }),
            reply: Arc::new(move |payload| reply.send(payload).is_ok()),
            session_live: Arc::new(move || session.is_live()),
            reply_live: Arc::new(move || reply_live.is_live()),
            read_live: request.read_interest(),
            request_scoped: true,
            event_live: Arc::new(move || {
                event_live.as_ref().is_some_and(|events| events.is_live())
            }),
            gate: Arc::new(Mutex::new(())),
        }
    }

    pub(crate) fn bound(mut self, scope: u64, live: Liveness, gate: Arc<Mutex<()>>) -> Self {
        self.scope = Some(scope);
        self.reply_live = live.clone();
        self.event_live = live;
        self.gate = gate;
        self
    }

    pub fn detached(session_id: impl Into<String>) -> Self {
        Self::legacy(&session_id.into(), Opening::new(), Arc::new(|_| false))
    }

    /// Producer events remain attached to the captured event generation.
    pub fn push(&self, payload: Value) -> bool {
        self.seal(payload)
            .is_some_and(|prepared| self.send_sealed(prepared))
    }
    pub(crate) fn push_scoped(&self, payload: Value) -> bool {
        self.push(payload)
    }
    pub(crate) fn seal(&self, mut payload: Value) -> Option<PreparedOutput> {
        payload
            .as_object_mut()?
            .insert("bridge_epoch".into(), json!(self.bridge_epoch()));
        Some(PreparedOutput(payload))
    }
    pub(crate) fn send_sealed(&self, prepared: PreparedOutput) -> bool {
        let _gate = self.gate.lock().unwrap();
        if !self.session_is_open() || !(self.event_live)() {
            return false;
        }
        (self.events)(prepared.0)
    }
    pub(crate) fn push_reply(&self, payload: Value) -> bool {
        let Some(prepared) = self.seal(payload) else {
            return false;
        };
        let _gate = self.gate.lock().unwrap();
        self.session_is_open() && (self.reply_live)() && (self.reply)(prepared.0)
    }
    pub(crate) fn push_read_reply(&self, payload: Value) {
        if self.read_is_live() {
            self.push_reply(payload);
        }
    }

    #[cfg(test)]
    pub(crate) fn push_observed(
        &self,
        payload: Value,
        clock: &crate::timing::FrameClock,
        observed: &Option<crate::transport::Frame>,
        read_fenced: bool,
    ) {
        if let Some(frame) = observed {
            clock.mark_request(frame, "reply_sealed", std::time::Instant::now());
        }
        if read_fenced {
            self.push_read_reply(payload);
        } else {
            self.push_reply(payload);
        }
    }
    #[cfg(test)]
    pub(crate) fn set_epoch_for_test(&mut self, epoch: Arc<str>) {
        self.bridge_epoch = epoch;
    }

    #[cfg(test)]
    pub(crate) fn opening_ended(&self) {
        self.opening.invalidate();
    }
}
