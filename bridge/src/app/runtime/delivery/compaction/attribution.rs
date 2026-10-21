//! OFFSOURCE cfg(test) required compaction ownership. No AppState/session resource ownership.
use crate::app::TabKey;
use crate::harness::AgentSession;
use crate::thread::SessionInstance;
use crate::timing::write_wait::{
    current_context, IntervalCategory, Obligation, SyncContextGuard, WaitGuard, WaitKind,
    WriteContext,
};
use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak};

type Key = (String, String);
const UNSET: u8 = 0;
const COMPLETE: u8 = 1;
const CANCEL: u8 = 2;

#[derive(Clone)]
pub(in crate::app) struct SessionIdentity {
    pub key: TabKey,
    pub instance: SessionInstance,
    pub thread: String,
    pub session: Weak<dyn AgentSession>,
}
impl SessionIdentity {
    fn same(&self, other: &Self) -> bool {
        self.key == other.key
            && self.instance == other.instance
            && self.thread == other.thread
            && self.session.ptr_eq(&other.session)
    }
    fn owner_key(&self) -> Key {
        (
            self.instance.entity_id.clone(),
            self.instance.agent_id.clone(),
        )
    }
}

/// Holding an immutable Entry Arc keeps its owning obligation live. Readers
/// can clone context/create admission children without acquiring a leaf mutex
/// and cannot race a writer completing the principal token underneath them.
pub(in crate::app) struct Entry {
    pub identity: SessionIdentity,
    table: std::sync::Weak<Inner>,
    context: WriteContext,
    owned: Option<Obligation>,
    interval: Mutex<Option<WaitGuard>>,
    outcome: AtomicU8,
}
impl Entry {
    fn new(
        table: CompactionAttribution,
        identity: SessionIdentity,
        contexts: &[WriteContext],
    ) -> Arc<Self> {
        let child = WriteContext::coalesced(contexts);
        let context = child.context();
        let waiting = context.interval(IntervalCategory::KnownWait(WaitKind::Dispatch));
        Arc::new(Self {
            identity,
            table: Arc::downgrade(&table.0),
            context,
            owned: Some(child),
            interval: Mutex::new(Some(waiting)),
            outcome: AtomicU8::new(UNSET),
        })
    }
    pub(in crate::app) fn cancel(self: &Arc<Self>) {
        if let Some(table) = self.table.upgrade() {
            CompactionAttribution(table).cancel(self);
        } else {
            self.finish(CANCEL);
        }
    }
    pub(in crate::app) fn context(&self) -> WriteContext {
        self.context.clone()
    }
    pub(in crate::app) fn bind(&self) -> SyncContextGuard {
        self.context.bind_sync()
    }
    pub(in crate::app) fn app_wait(&self) -> WaitGuard {
        self.context
            .interval(IntervalCategory::KnownWait(WaitKind::AppLock))
    }
    pub(in crate::app) fn start_service(&self) {
        let _binding = self.bind();
        let mut interval = crate::timing::resource_lock!(&self.interval).unwrap();
        if self.outcome.load(Ordering::Acquire) != UNSET {
            return;
        }
        interval.take();
        *interval = Some(self.context.interval(IntervalCategory::OwnExternal));
    }
    fn finish(&self, outcome: u8) {
        let _binding = self.bind();
        let mut interval = crate::timing::resource_lock!(&self.interval).unwrap();
        interval.take();
        self.outcome.store(outcome, Ordering::Release);
    }
}
impl Drop for Entry {
    fn drop(&mut self) {
        self.interval.get_mut().unwrap().take();
        if let Some(child) = self.owned.take() {
            match self.outcome.load(Ordering::Acquire) {
                COMPLETE => child.complete(),
                CANCEL => child.cancel(),
                _ => drop(child), // unexpected disappearance remains abandoned evidence
            }
        }
    }
}

#[derive(Clone, Default)]
struct Slot {
    requested: Option<Arc<Entry>>,
    pending: Vec<Arc<Entry>>,
    sent: Vec<Arc<Entry>>,
}
struct Inner {
    writer: Mutex<()>,
    current: ArcSwap<HashMap<Key, Slot>>,
}
#[derive(Clone)]
pub(in crate::app) struct CompactionAttribution(Arc<Inner>);
impl Default for CompactionAttribution {
    fn default() -> Self {
        Self(Arc::new(Inner {
            writer: Mutex::new(()),
            current: ArcSwap::from_pointee(HashMap::new()),
        }))
    }
}
impl CompactionAttribution {
    fn mutate(&self, change: impl FnOnce(&mut HashMap<Key, Slot>)) {
        let _writer = crate::timing::resource_lock!(&self.0.writer).unwrap();
        let mut next = (**self.0.current.load()).clone();
        change(&mut next);
        next.retain(|_, slot| {
            slot.requested.is_some() || !slot.pending.is_empty() || !slot.sent.is_empty()
        });
        self.0.current.store(Arc::new(next));
    }
    pub(in crate::app) fn request(&self, identity: SessionIdentity) {
        let Some(current) = current_context() else {
            return;
        };
        let old = self
            .0
            .current
            .load()
            .get(&identity.owner_key())
            .and_then(|slot| slot.requested.clone());
        let mut contexts = vec![current];
        if let Some(old) = old.as_ref().filter(|old| old.identity.same(&identity)) {
            contexts.push(old.context());
        }
        let entry = Entry::new(self.clone(), identity, &contexts);
        self.mutate(|next| {
            next.entry(entry.identity.owner_key())
                .or_default()
                .requested = Some(Arc::clone(&entry));
        });
        if let Some(old) = old {
            old.finish(if old.identity.same(&entry.identity) {
                COMPLETE
            } else {
                CANCEL
            });
        }
    }
    /// Called under the native preparing AppState guard. The exact Entry is
    /// retained by both the table and CompactionSend before its frame ACKs.
    pub(in crate::app) fn prepare_send(
        &self,
        identity: SessionIdentity,
        causal_turn: Option<WriteContext>,
    ) -> Option<Arc<Entry>> {
        let old = self
            .0
            .current
            .load()
            .get(&identity.owner_key())
            .and_then(|slot| slot.requested.clone());
        let mut contexts: Vec<_> = causal_turn.into_iter().collect();
        if let Some(old) = old.as_ref().filter(|old| old.identity.same(&identity)) {
            contexts.push(old.context());
        }
        if contexts.is_empty() {
            return None;
        }
        let entry = Entry::new(self.clone(), identity, &contexts);
        self.mutate(|next| {
            let slot = next.entry(entry.identity.owner_key()).or_default();
            slot.requested = None;
            slot.pending.push(Arc::clone(&entry));
        });
        if let Some(old) = old {
            old.finish(if old.identity.same(&entry.identity) {
                COMPLETE
            } else {
                CANCEL
            });
        }
        Some(entry)
    }
    pub(in crate::app) fn sent(&self, entry: &Arc<Entry>) {
        self.mutate(|next| {
            let Some(slot) = next.get_mut(&entry.identity.owner_key()) else {
                return;
            };
            if entry.outcome.load(Ordering::Acquire) != UNSET
                || !slot
                    .pending
                    .iter()
                    .any(|pending| Arc::ptr_eq(pending, entry))
            {
                return;
            }
            slot.pending.retain(|pending| !Arc::ptr_eq(pending, entry));
            slot.sent.push(Arc::clone(entry));
        });
    }
    pub(in crate::app) fn cancel(&self, entry: &Arc<Entry>) {
        self.finish(std::slice::from_ref(entry), CANCEL);
    }
    fn finish(&self, entries: &[Arc<Entry>], outcome: u8) {
        self.mutate(|next| {
            for entry in entries {
                if let Some(slot) = next.get_mut(&entry.identity.owner_key()) {
                    if slot
                        .requested
                        .as_ref()
                        .is_some_and(|candidate| Arc::ptr_eq(candidate, entry))
                    {
                        slot.requested = None;
                    }
                    slot.pending
                        .retain(|candidate| !Arc::ptr_eq(candidate, entry));
                    slot.sent.retain(|candidate| !Arc::ptr_eq(candidate, entry));
                }
                entry.finish(outcome);
            }
        });
    }
    pub(in crate::app) fn forget_session(&self, key: &TabKey, session: &Arc<dyn AgentSession>) {
        let entries: Vec<_> = self
            .0
            .current
            .load()
            .values()
            .flat_map(|slot| slot.requested.iter().chain(&slot.pending).chain(&slot.sent))
            .filter(|entry| {
                entry.identity.key == *key
                    && entry.identity.session.ptr_eq(&Arc::downgrade(session))
            })
            .cloned()
            .collect();
        if entries.is_empty() {
            return;
        }
        let mut contexts: Vec<_> = entries.iter().map(|entry| entry.context()).collect();
        contexts.extend(current_context());
        let child = (!contexts.is_empty()).then(|| WriteContext::coalesced(&contexts));
        let binding = child.as_ref().map(|child| child.context().bind_sync());
        self.finish(&entries, CANCEL);
        drop(binding);
        if let Some(child) = child {
            child.complete();
        }
    }
    pub(in crate::app) fn has_sent(&self, owner: &str, agent: &str) -> bool {
        self.0
            .current
            .load()
            .get(&(owner.to_owned(), agent.to_owned()))
            .is_some_and(|slot| !slot.sent.is_empty())
    }
    pub(in crate::app) fn forget(&self, owner: &str, agent: &str) {
        let entries: Vec<_> = self
            .0
            .current
            .load()
            .get(&(owner.to_owned(), agent.to_owned()))
            .into_iter()
            .flat_map(|slot| slot.requested.iter().chain(&slot.pending).chain(&slot.sent))
            .cloned()
            .collect();
        if entries.is_empty() {
            return;
        }
        let mut contexts: Vec<_> = entries.iter().map(|entry| entry.context()).collect();
        contexts.extend(current_context());
        let child = WriteContext::coalesced(&contexts);
        let binding = child.context().bind_sync();
        self.finish(&entries, CANCEL);
        drop(binding);
        child.complete();
    }
    pub(in crate::app) fn stamp(&self, identity: &SessionIdentity) -> Option<StampScope> {
        let entries: Vec<_> = self
            .0
            .current
            .load()
            .get(&identity.owner_key())
            .into_iter()
            .flat_map(|slot| &slot.sent)
            .filter(|entry| entry.identity.same(identity))
            .cloned()
            .collect();
        if entries.is_empty() {
            return None;
        }
        let mut contexts: Vec<_> = entries.iter().map(|entry| entry.context()).collect();
        contexts.extend(current_context());
        let child = WriteContext::coalesced(&contexts);
        let binding = child.context().bind_sync();
        Some(StampScope {
            table: self.clone(),
            entries,
            child: Some(child),
            binding: Some(binding),
            stamped: None,
        })
    }
    /// Explicit TESTFIRST frontier: lookup/ownership before status app acquire
    /// remains absent until the actual held-AppState completion control RED.
    pub(in crate::app) fn status_admission(
        &self,
        _key: &TabKey,
        _instance: &SessionInstance,
        _session: &Weak<dyn AgentSession>,
        _context: Option<crate::harness::TurnContext>,
    ) -> Option<StatusAdmission> {
        None
    }
}

pub(in crate::app) struct StatusAdmission {
    child: Obligation,
}
impl StatusAdmission {
    pub(in crate::app) fn bind(&self) -> SyncContextGuard {
        self.child.context().bind_sync()
    }
}

/// Synchronous only (!Send through its binding), never stored across an await.
pub(in crate::app) struct StampScope {
    table: CompactionAttribution,
    entries: Vec<Arc<Entry>>,
    child: Option<Obligation>,
    binding: Option<SyncContextGuard>,
    stamped: Option<bool>,
}
impl StampScope {
    pub(in crate::app) fn native_stamp_result(&mut self, stamped: bool) {
        self.stamped = Some(stamped);
    }
}
impl Drop for StampScope {
    fn drop(&mut self) {
        let outcome = (!std::thread::panicking())
            .then_some(self.stamped)
            .flatten();
        if let Some(stamped) = outcome {
            self.table
                .finish(&self.entries, if stamped { COMPLETE } else { CANCEL });
        }
        self.entries.clear();
        self.binding.take();
        if let Some(child) = self.child.take() {
            match outcome {
                Some(true) => child.complete(),
                Some(false) => child.cancel(),
                None => drop(child), // unwind/unproved outcome remains abandoned
            }
        }
    }
}

impl crate::app::AppState {
    fn compaction_session_identity(
        &self,
        owner: &str,
        agent: &str,
        expected: Option<&Arc<dyn AgentSession>>,
    ) -> Option<SessionIdentity> {
        let tab = self
            .session_registry
            .live_agent_snapshots()
            .into_iter()
            .find(|tab| {
                tab.instance.as_ref().is_some_and(|instance| {
                    instance.entity_id == owner && instance.agent_id == agent
                }) && expected.is_none_or(|expected| Arc::ptr_eq(expected, &tab.session))
            })?;
        Some(SessionIdentity {
            key: tab.key,
            instance: tab.instance?,
            thread: self.agent_conversation(owner, Some(agent)).ok()?.id.clone(),
            session: Arc::downgrade(&tab.session),
        })
    }
    pub(in crate::app) fn capture_compaction_origin(&self, owner: &str, agent: &str) {
        if let Some(identity) = self.compaction_session_identity(owner, agent, None) {
            self.session_registry
                .compaction_attribution()
                .request(identity);
        }
    }
    pub(in crate::app) fn prepare_compaction_send_origin(
        &self,
        owner: &str,
        agent: &str,
        session: &Arc<dyn AgentSession>,
        causal_turn: bool,
    ) -> Option<Arc<Entry>> {
        self.session_registry.compaction_attribution().prepare_send(
            self.compaction_session_identity(owner, agent, Some(session))?,
            causal_turn.then(current_context).flatten(),
        )
    }
    pub(in crate::app) fn compaction_stamp_identity_for_pump(
        &self,
        key: &TabKey,
        instance: &SessionInstance,
        session: &Weak<dyn AgentSession>,
    ) -> Option<SessionIdentity> {
        if !self
            .session_registry
            .compaction_attribution()
            .has_sent(&instance.entity_id, &instance.agent_id)
        {
            return None;
        }
        let tab = self.session_registry.agent_snapshot(key)?;
        if tab.instance.as_ref() != Some(instance) || !session.ptr_eq(&Arc::downgrade(&tab.session))
        {
            return None;
        }
        Some(SessionIdentity {
            key: key.clone(),
            instance: instance.clone(),
            thread: self
                .agent_conversation(&instance.entity_id, Some(&instance.agent_id))
                .ok()?
                .id
                .clone(),
            session: session.clone(),
        })
    }
    pub(in crate::app) fn compaction_stamp_scope(
        &self,
        owner: &str,
        agent: &str,
        identity: Option<SessionIdentity>,
    ) -> Option<StampScope> {
        let table = self.session_registry.compaction_attribution();
        if !table.has_sent(owner, agent) {
            return None;
        }
        table.stamp(&identity.or_else(|| self.compaction_session_identity(owner, agent, None))?)
    }
}
