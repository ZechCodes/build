use super::*;
use crate::timing::write_wait::{current_context, WriteWaitRegistry};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Default)]
struct Seen {
    polls: Vec<Vec<String>>,
    dropped: Vec<String>,
}

struct InspectFuture {
    seen: Arc<Mutex<Seen>>,
    polled: bool,
}

fn current_ids() -> Vec<String> {
    current_context().map_or_else(Vec::new, |context| context.original_ids())
}

impl Future for InspectFuture {
    type Output = u32;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.seen.lock().unwrap().polls.push(current_ids());
        if std::mem::replace(&mut self.polled, true) {
            Poll::Ready(17)
        } else {
            Poll::Pending
        }
    }
}

impl Drop for InspectFuture {
    fn drop(&mut self) {
        self.seen.lock().unwrap().dropped = current_ids();
    }
}

#[test]
fn pending_service_poll_keeps_child_and_restores_callers_tls() {
    let registry = WriteWaitRegistry::new();
    let root = registry.begin("service-poll", "harnesses.refresh", Instant::now());
    let id = root.id();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut future = {
        let _bound = root.context().bind_sync();
        ServiceFuture::queued(InspectFuture {
            seen: Arc::clone(&seen),
            polled: false,
        })
    };
    root.acknowledged();
    root.complete();
    let caller = registry.begin("polling-caller", "different-write", Instant::now());
    let caller_binding = caller.context().bind_sync();
    let mut cx = Context::from_waker(std::task::Waker::noop());
    let first = Pin::new(&mut future).poll(&mut cx);
    let restored = current_ids();
    let retained = registry
        .active_snapshot()
        .into_iter()
        .find(|row| row.id == id);
    let final_poll = Pin::new(&mut future).poll(&mut cx);
    drop(future);
    drop(caller_binding);
    caller.complete();

    assert_eq!(first, Poll::Pending);
    assert_eq!(final_poll, Poll::Ready(17));
    assert_eq!(restored, ["polling-caller"]);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.polls, [vec!["service-poll"], vec!["service-poll"]]);
    assert_eq!(seen.dropped, ["service-poll"]);
    assert!(retained.is_some_and(|row| row.root_finished && !row.outstanding.is_empty()));
    let terminal = registry
        .snapshot()
        .into_iter()
        .find(|row| row.id == id)
        .unwrap();
    assert!(
        terminal.root_finished && terminal.outstanding.is_empty() && terminal.active.is_empty()
    );
    assert!(!terminal.cancelled && !terminal.abandoned);
    assert!(current_context().is_none());
}

#[test]
fn cancelled_service_binds_cleanup_and_records_explicit_cancellation() {
    let registry = WriteWaitRegistry::new();
    let root = registry.begin("cancel-service", "bridge.check_update", Instant::now());
    let id = root.id();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let mut future = {
        let _bound = root.context().bind_sync();
        ServiceFuture::queued(InspectFuture {
            seen: Arc::clone(&seen),
            polled: false,
        })
    };
    root.acknowledged();
    root.complete();
    let mut cx = Context::from_waker(std::task::Waker::noop());
    let pending = Pin::new(&mut future).poll(&mut cx);
    let retained = registry
        .active_snapshot()
        .into_iter()
        .find(|row| row.id == id);
    drop(future);

    assert_eq!(pending, Poll::Pending);
    assert_eq!(seen.lock().unwrap().dropped, ["cancel-service"]);
    assert!(retained.is_some_and(|row| row.root_finished && !row.outstanding.is_empty()));
    let terminal = registry
        .snapshot()
        .into_iter()
        .find(|row| row.id == id)
        .unwrap();
    assert!(
        terminal.root_finished && terminal.outstanding.is_empty() && terminal.active.is_empty()
    );
    assert!(terminal.cancelled && !terminal.abandoned);
    assert!(current_context().is_none());
}
