//! Process-local required descendants of committed ready assignment attempts.
//!
//! Nothing here is serialized: recovery without a live entry is background work.
use super::DeliveryIntent;
use crate::timing::write_wait::{
    IntervalCategory, Obligation, SyncContextGuard, WaitGuard, WaitKind, WriteContext,
};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

thread_local! {
    static AUTHORS: RefCell<Vec<Option<WriteContext>>> = const { RefCell::new(Vec::new()) };
}

/// A nonowning descriptor captured at the semantic AppState mutation boundary.
/// Explicit None never falls back to a joined resource context.
pub(crate) struct ActivationAuthor;
impl ActivationAuthor {
    pub(crate) fn captured(context: Option<WriteContext>) -> Self {
        AUTHORS.with(|authors| authors.borrow_mut().push(context));
        Self
    }
}
impl Drop for ActivationAuthor {
    fn drop(&mut self) {
        AUTHORS.with(|authors| {
            authors.borrow_mut().pop();
        });
    }
}

#[derive(Clone, Default)]
pub(crate) struct DeliveryOrigins(Arc<Mutex<BTreeMap<String, Arc<Attempt>>>>);

pub(crate) struct Attempt {
    id: String,
    owned: Mutex<Owned>,
}
struct Owned {
    child: Option<Obligation>,
    queued: Option<WaitGuard>,
}

impl DeliveryOrigins {
    /// Called by the typed transaction after COMMIT and before its wakeup.
    pub(super) fn committed(&self, intent: &DeliveryIntent) {
        if !matches!(intent.state.as_str(), "queued" | "provisioning") {
            return;
        }
        let author = AUTHORS.with(|authors| authors.borrow().last().cloned().flatten());
        let Some(author) = author else {
            return;
        };
        let child = author.child();
        let queued = Some(child.interval(IntervalCategory::KnownWait(WaitKind::Dispatch)));
        let attempt = Arc::new(Attempt {
            id: intent.id.clone(),
            owned: Mutex::new(Owned {
                child: Some(child),
                queued,
            }),
        });
        // A retry is a new activation even though SQL reuses id/generation/op.
        // A selected old run retains its own Arc and cannot remove this entry.
        self.0.lock().unwrap().insert(intent.id.clone(), attempt);
    }

    pub(crate) fn attempt(&self, id: &str) -> Option<Arc<Attempt>> {
        self.0.lock().unwrap().get(id).cloned()
    }

    pub(crate) fn scan(&self) -> Run {
        let attempts: Vec<_> = self.0.lock().unwrap().values().cloned().collect();
        let owned: Vec<_> = attempts
            .iter()
            .map(|attempt| attempt.owned.lock().unwrap())
            .collect();
        let contexts: Vec<_> = owned
            .iter()
            .filter_map(|owned| owned.child.as_ref().map(Obligation::context))
            .collect();
        if contexts
            .first()
            .is_some_and(|first| contexts.iter().any(|context| !first.same_registry(context)))
        {
            drop(owned);
            panic!("one saved-delivery scan must use one write-wait registry");
        }
        let child = if contexts.is_empty() {
            None
        } else {
            Some(WriteContext::coalesced(&contexts))
        };
        Run { child }
    }

    pub(crate) fn finish(&self, attempt: &Arc<Attempt>, cancelled: bool) {
        let mut owned = attempt.owned.lock().unwrap();
        owned.queued.take();
        if let Some(child) = owned.child.take() {
            if cancelled {
                child.cancel();
            } else {
                child.complete();
            }
        }
        drop(owned);
        let mut entries = self.0.lock().unwrap();
        if entries
            .get(&attempt.id)
            .is_some_and(|current| Arc::ptr_eq(current, attempt))
        {
            entries.remove(&attempt.id);
        }
    }
}

impl Attempt {
    pub(crate) fn start(&self) -> Run {
        let mut owned = self.owned.lock().unwrap();
        let child = owned.child.as_ref().map(Obligation::child);
        owned.queued.take();
        Run { child }
    }

    pub(crate) fn waiting(&self) {
        let mut owned = self.owned.lock().unwrap();
        if owned.queued.is_none() {
            owned.queued = owned
                .child
                .as_ref()
                .map(|child| child.interval(IntervalCategory::KnownWait(WaitKind::Dispatch)));
        }
    }
}

pub(crate) struct Run {
    child: Option<Obligation>,
}
impl Run {
    pub(crate) fn joined(contexts: impl IntoIterator<Item = WriteContext>) -> Self {
        let contexts: Vec<_> = contexts.into_iter().collect();
        Self {
            child: (!contexts.is_empty()).then(|| WriteContext::coalesced(&contexts)),
        }
    }
    pub(crate) fn context(&self) -> Option<WriteContext> {
        self.child.as_ref().map(Obligation::context)
    }
    pub(crate) fn bind(&self) -> Option<SyncContextGuard> {
        self.context().map(|context| context.bind_sync())
    }
    pub(crate) fn complete(mut self) {
        if let Some(child) = self.child.take() {
            child.complete();
        }
    }
}
impl Drop for Run {
    fn drop(&mut self) {
        // This is an execution lease. The independently owned attempt remains
        // required until explicit native settlement; unwinding stays abandoned.
        if !std::thread::panicking() {
            if let Some(child) = self.child.take() {
                child.complete();
            }
        }
    }
}
