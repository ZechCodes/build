//! Test-only required service descendants, retained after the request replies.
use super::{
    current_context, current_interval, IntervalCategory, Obligation, SyncContextGuard, WaitGuard,
    WaitKind, WriteContext,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

#[cfg(test)]
mod tests;

#[derive(Clone, Default)]
pub(crate) struct ServiceOrigins(Arc<Mutex<Vec<Arc<Origin>>>>);

struct Origin(Mutex<Owned>);
struct Owned {
    child: Option<Obligation>,
    waiting: Option<WaitGuard>,
}

impl std::fmt::Debug for ServiceOrigins {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ServiceOrigins")
    }
}

impl ServiceOrigins {
    pub(crate) fn capture() -> Self {
        let origins = Self::default();
        origins.push_current();
        origins
    }

    pub(crate) fn push_current(&self) {
        let Some(context) = current_context() else {
            return;
        };
        let child = context.child();
        let waiting = Some(child.interval(IntervalCategory::KnownWait(WaitKind::Dispatch)));
        self.0
            .lock()
            .unwrap()
            .push(Arc::new(Origin(Mutex::new(Owned {
                child: Some(child),
                waiting,
            }))));
    }

    pub(crate) fn combined(groups: impl IntoIterator<Item = Self>) -> Self {
        let origins = Self::default();
        for group in groups {
            origins
                .0
                .lock()
                .unwrap()
                .extend(group.0.lock().unwrap().iter().cloned());
        }
        origins
    }

    pub(crate) fn start(&self) -> ServiceRun {
        // Hold each original's ownership stable until the joined owner exists.
        // A concurrent sweep can settle an aliased generation on another thread.
        let mut originals = self.0.lock().unwrap().clone();
        originals.sort_by_key(|origin| Arc::as_ptr(origin) as usize);
        originals.dedup_by(|left, right| Arc::ptr_eq(left, right));
        let mut owned: Vec<_> = originals
            .iter()
            .map(|origin| origin.0.lock().unwrap())
            .collect();
        let contexts: Vec<_> = owned
            .iter()
            .filter_map(|owned| owned.child.as_ref().map(Obligation::context))
            .collect();
        // Mixed registries reject before even closing a queued interval.
        if contexts
            .first()
            .is_some_and(|first| contexts.iter().any(|context| !first.same_registry(context)))
        {
            // Release ownership guards before rejecting instrumentation misuse,
            // so the unchanged queues remain usable rather than poisoned.
            drop(owned);
            panic!("one service run cannot combine different write-wait registries");
        }
        let child = (!contexts.is_empty()).then(|| WriteContext::coalesced(&contexts));
        for origin in &mut owned {
            origin.waiting.take();
        }
        ServiceRun { child }
    }

    pub(crate) fn waiting(&self) {
        for origin in self.0.lock().unwrap().iter() {
            let mut owned = origin.0.lock().unwrap();
            if owned.waiting.is_none() {
                owned.waiting = owned
                    .child
                    .as_ref()
                    .map(|child| child.interval(IntervalCategory::KnownWait(WaitKind::Dispatch)));
            }
        }
    }

    pub(crate) fn complete(&self) {
        self.finish(false);
    }
    pub(crate) fn cancel(&self) {
        self.finish(true);
    }

    fn finish(&self, cancel: bool) {
        for origin in self.0.lock().unwrap().drain(..) {
            let mut owned = origin.0.lock().unwrap();
            owned.waiting.take();
            if let Some(child) = owned.child.take() {
                if cancel {
                    child.cancel();
                } else {
                    child.complete();
                }
            }
        }
    }
}

pub(crate) struct ServiceRun {
    child: Option<Obligation>,
}
impl ServiceRun {
    pub(crate) fn bind(&self) -> Option<SyncContextGuard> {
        self.child.as_ref().map(|child| child.context().bind_sync())
    }
    pub(crate) fn complete(self) {
        if let Some(child) = self.child {
            child.complete();
        }
    }
    fn cancel(self) {
        if let Some(child) = self.child {
            child.cancel();
        }
    }
}

/// Own the required child until the future settles. TLS is bound only during
/// one poll (and explicit cancellation cleanup), never across Pending.
pub(crate) struct ServiceFuture<F> {
    future: Option<Pin<Box<F>>>,
    origins: ServiceOrigins,
    running: Option<ServiceRun>,
    finished: bool,
}
impl<F: Future> ServiceFuture<F> {
    pub(crate) fn queued(future: F) -> Self {
        Self {
            future: Some(Box::pin(future)),
            origins: ServiceOrigins::capture(),
            running: None,
            finished: false,
        }
    }
}
impl<F: Future> Future for ServiceFuture<F> {
    type Output = F::Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.running.is_none() {
            this.running = Some(this.origins.start());
        }
        let binding = this.running.as_ref().and_then(ServiceRun::bind);
        let result = this
            .future
            .as_mut()
            .expect("service future polled after completion")
            .as_mut()
            .poll(cx);
        if result.is_ready() {
            // Destroy the completed future while attribution and its owner live.
            this.future.take();
            drop(binding);
            this.origins.complete();
            this.running.take().unwrap().complete();
            this.finished = true;
        }
        result
    }
}
impl<F> Drop for ServiceFuture<F> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        // Dropping a blocked task may itself drop application/resource guards.
        // Keep explicit ownership and TLS throughout that synchronous cleanup.
        if self.running.is_none() {
            self.running = Some(self.origins.start());
        }
        let binding = self.running.as_ref().and_then(ServiceRun::bind);
        self.future.take();
        drop(binding);
        self.origins.cancel();
        if let Some(running) = self.running.take() {
            running.cancel();
        }
    }
}

pub(crate) async fn external<F: Future>(future: F) -> F::Output {
    let _service = current_interval(IntervalCategory::OwnExternal);
    future.await
}
