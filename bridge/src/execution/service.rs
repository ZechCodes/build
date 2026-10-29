//! Shared intake orchestration over the existing scheduler and read services.

use super::session::{ClientSessions, RequestContext, SessionError, SessionLease};
use super::{dispatch::Dispatcher, CallContext, FrameHandler, Invocation};
use crate::timing::WaitTimer;
use crate::transport::Frame;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

pub struct ExecutionService {
    pub(crate) dispatcher: Arc<Dispatcher>,
    sessions: Arc<ClientSessions>,
    cleanup_active: Arc<AtomicBool>,
}

/// The caller keeps this handle through writing its response. Dropping it
/// cancels queued interest/delivery, while admitted external work settles.
#[must_use = "retain the call through response completion; dropping cancels delivery"]
pub struct Call {
    request: RequestContext,
    outcome: oneshot::Receiver<Value>,
}

#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("request delivery ended before a final reply")]
    Closed,
}

impl Call {
    pub async fn reply(&mut self) -> Result<Value, CallError> {
        (&mut self.outcome).await.map_err(|_| CallError::Closed)
    }
    pub fn cancel(&self) {
        self.request.cancellation().cancel();
    }
    pub fn context(&self) -> &RequestContext {
        &self.request
    }
}

impl ExecutionService {
    pub fn new(handler: FrameHandler) -> Arc<Self> {
        Self::from_dispatcher(Arc::new(Dispatcher::new(handler)))
    }
    pub(crate) fn from_dispatcher(dispatcher: Arc<Dispatcher>) -> Arc<Self> {
        let sessions = Arc::new(ClientSessions::new(crate::bridge_epoch::current()));
        let cleanup = Arc::downgrade(&dispatcher);
        sessions.set_cleanup_hook(Arc::new(move |session| {
            if let Some(dispatcher) = cleanup.upgrade() {
                dispatcher.retire_opening(session.id());
            }
        }));
        Arc::new(Self {
            dispatcher,
            sessions,
            cleanup_active: Arc::new(AtomicBool::new(false)),
        })
    }
    pub fn sessions(&self) -> Arc<ClientSessions> {
        self.sessions.clone()
    }
    pub fn greeting(&self, session: &SessionLease) -> Result<Value, SessionError> {
        if !session.is_live() {
            return Err(SessionError::Unauthorized);
        }
        let handler = self.dispatcher.handler();
        let greeting = handler.greeting(crate::bridge_epoch::current());
        if !session.is_live() {
            return Err(SessionError::Unauthorized);
        }
        Ok(greeting)
    }
    /// No durable acceptance is implied. SQL/I/O capacity still parks here on
    /// the same per-opening queues as WebRTC; adapters never choose a lane.
    pub async fn submit(
        &self,
        context: RequestContext,
        invocation: Invocation,
    ) -> Result<Call, SessionError> {
        if !context.is_live() {
            return Err(SessionError::Unauthorized);
        }
        let frame = invocation.into_frame(context.session().id(), context.request_order());
        let (done, outcome) = oneshot::channel();
        let done = Arc::new(Mutex::new(Some(done)));
        let sender = CallContext::from_request(&context).with_reply_observer(move |value| {
            if let Some(done) = done.lock().unwrap().take() {
                let _ = done.send(value.clone());
            }
        });
        let call = Call {
            request: context,
            outcome,
        };
        self.submit_frame(sender, frame, WaitTimer::start()).await;
        Ok(call)
    }
    pub(crate) async fn submit_frame(&self, sender: CallContext, frame: Frame, arrived: WaitTimer) {
        if !sender.read_is_live() {
            return;
        }
        let handler = self.dispatcher.handler();
        let arrived = match handler.snapshot_since(&frame, arrived) {
            Ok(reply) => {
                sender.push_read_reply(reply);
                return;
            }
            Err(arrived) => arrived,
        };
        if frame.payload["method"] == "bridge.stats" {
            if sender.read_is_live() {
                sender.push_reply(handler.call_since(sender.clone(), frame, arrived));
            }
            return;
        }
        if frame.payload["method"] == "ping" {
            sender.push_reply(crate::api::reply(
                frame.payload["id"].clone(),
                Ok(crate::app::pong()),
            ));
            return;
        }
        self.dispatcher.dispatch_since(sender, frame, arrived).await;
    }
    /// Called by the daemon's liveness tick; returns ended exact allocations
    /// after their cleanup hook has been handed off once.
    pub fn reap_expired(&self) -> usize {
        let ended = self.sessions.reap_expired().len();
        if !self.cleanup_active.swap(true, Ordering::AcqRel) {
            let handler = self.dispatcher.handler();
            let active = self.cleanup_active.clone();
            tokio::task::spawn_blocking(move || {
                struct Reset(Arc<AtomicBool>);
                impl Drop for Reset {
                    fn drop(&mut self) {
                        self.0.store(false, Ordering::Release);
                    }
                }
                let _reset = Reset(active);
                handler.cleanup_outputs();
            });
        }
        ended
    }
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
