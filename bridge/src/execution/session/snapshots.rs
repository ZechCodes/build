//! Joint reservations for one Opening and the shared bridge snapshot budget.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use tokio::{
    runtime::Handle,
    sync::{watch, Notify},
    time::{sleep_until, Instant},
};

use super::TerminalInterest;

// Two 80 MiB slots plus sixteen 4 MiB sockets reserve 224 MiB.
const BRIDGE_SLOTS: usize = 2;
const SLOT_WAIT: Duration = Duration::from_secs(10);
const WRITE_FLOOR: u64 = 30;
const WRITE_CEILING: u64 = 192;
const WRITE_BYTES_PER_SECOND: usize = 65_536;

pub(crate) fn snapshot_write_budget(bytes: usize) -> Duration {
    let seconds = bytes.div_ceil(WRITE_BYTES_PER_SECOND) as u64;
    Duration::from_secs(seconds.clamp(WRITE_FLOOR, WRITE_CEILING))
}

#[derive(Default)]
pub(crate) struct SnapshotPool {
    state: Mutex<PoolState>,
    changed: Notify,
}

#[derive(Default)]
struct PoolState {
    held: HashSet<u64>,
    waiting: VecDeque<QueueEntry>,
    refusals: HashMap<u64, usize>,
    next: u64,
}

#[derive(Clone, Copy)]
struct QueueEntry {
    id: u64,
    opening: u64,
    expires: Instant,
}

impl SnapshotPool {
    fn join(self: &Arc<Self>, opening: u64) -> SnapshotWaiter {
        let mut state = self.state.lock().unwrap();
        let entry = QueueEntry {
            id: state.next,
            opening,
            expires: Instant::now() + SLOT_WAIT,
        };
        state.next += 1;
        state.waiting.push_back(entry);
        SnapshotWaiter {
            pool: self.clone(),
            entry,
        }
    }

    fn try_grant(self: &Arc<Self>, entry: QueueEntry) -> Option<SnapshotSlot> {
        let mut state = self.state.lock().unwrap();
        if state.held.len() == BRIDGE_SLOTS {
            return None;
        }
        let now = Instant::now();
        let mut seen = HashSet::new();
        // An expired waiter stays first for its Opening until its refusal
        // is queued. Other Openings can use a free bridge slot meanwhile.
        let index = state.waiting.iter().position(|waiter| {
            seen.insert(waiter.opening)
                && !state.held.contains(&waiter.opening)
                && !state.refusals.contains_key(&waiter.opening)
                && now < waiter.expires
        })?;
        if state.waiting[index].id != entry.id {
            return None;
        }
        state.waiting.remove(index);
        state.held.insert(entry.opening);
        self.changed.notify_waiters();
        Some(SnapshotSlot {
            pool: self.clone(),
            entry,
            granted: now,
        })
    }

    #[cfg(test)]
    pub(crate) fn available_permits(&self) -> usize {
        BRIDGE_SLOTS - self.state.lock().unwrap().held.len()
    }

    #[cfg(test)]
    pub(crate) fn hold_for_test(self: &Arc<Self>) -> SnapshotSlot {
        let waiter = self.join(super::Opening::new().diagnostic_id());
        self.try_grant(waiter.entry).expect("test bridge slot")
    }

    #[cfg(test)]
    pub(crate) fn holds(&self, opening: u64) -> bool {
        self.state.lock().unwrap().held.contains(&opening)
    }

    #[cfg(test)]
    pub(crate) fn waiting(&self, opening: u64) -> usize {
        self.state
            .lock()
            .unwrap()
            .waiting
            .iter()
            .filter(|entry| entry.opening == opening)
            .count()
    }
}

pub(crate) struct SnapshotWaiter {
    pool: Arc<SnapshotPool>,
    entry: QueueEntry,
}

impl SnapshotWaiter {
    fn expire(&self) -> SnapshotBusy {
        let opening = self.entry.opening;
        let mut state = self.pool.state.lock().unwrap();
        state.waiting.retain(|entry| entry.id != self.entry.id);
        *state.refusals.entry(opening).or_default() += 1;
        self.pool.changed.notify_waiters();
        SnapshotBusy {
            pool: self.pool.clone(),
            opening,
        }
    }

    async fn acquire(&self) -> SnapshotSlot {
        loop {
            let changed = self.pool.changed.notified();
            if let Some(claim) = self.pool.try_grant(self.entry) {
                return claim;
            }
            changed.await;
        }
    }
}

impl Drop for SnapshotWaiter {
    fn drop(&mut self) {
        let mut state = self.pool.state.lock().unwrap();
        if let Some(index) = state
            .waiting
            .iter()
            .position(|entry| entry.id == self.entry.id)
        {
            state.waiting.remove(index);
            self.pool.changed.notify_waiters();
        }
    }
}

/// One claim accounts for both slots. Waiting tasks hold only a queue entry.
pub(crate) struct SnapshotSlot {
    pool: Arc<SnapshotPool>,
    entry: QueueEntry,
    granted: Instant,
}

impl Drop for SnapshotSlot {
    fn drop(&mut self) {
        self.pool
            .state
            .lock()
            .unwrap()
            .held
            .remove(&self.entry.opening);
        self.pool.changed.notify_waiters();
    }
}

/// A refusal ahead of the next snapshot for this Opening. Holds no slot.
pub(crate) struct SnapshotBusy {
    pool: Arc<SnapshotPool>,
    opening: u64,
}

impl std::fmt::Debug for SnapshotBusy {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SnapshotBusy")
            .field("opening", &self.opening)
            .finish_non_exhaustive()
    }
}

impl Drop for SnapshotBusy {
    fn drop(&mut self) {
        let mut state = self.pool.state.lock().unwrap();
        let pending = state
            .refusals
            .get_mut(&self.opening)
            .expect("queued refusal");
        *pending -= 1;
        if *pending == 0 {
            state.refusals.remove(&self.opening);
            self.pool.changed.notify_waiters();
        }
    }
}

#[derive(Debug)]
pub(crate) enum SnapshotWaitError {
    Cancelled,
    Busy(SnapshotBusy),
}

#[derive(Clone)]
pub(crate) struct TerminalSnapshotLane {
    opening: u64,
    bridge: Arc<SnapshotPool>,
    pub(crate) runtime: Handle,
    stopped: watch::Receiver<bool>,
    active: watch::Sender<Option<ActiveSnapshot>>,
}

#[derive(Clone)]
struct ActiveSnapshot {
    id: u64,
    deadline: Instant,
    reservation: Weak<TerminalSnapshotReservation>,
}

pub(crate) struct TerminalSnapshotReservation {
    id: u64,
    granted: Instant,
    slot: Mutex<Option<SnapshotSlot>>,
    active: watch::Sender<Option<ActiveSnapshot>>,
}

impl TerminalSnapshotLane {
    pub(crate) fn new(
        opening: u64,
        bridge: Arc<SnapshotPool>,
        stopped: watch::Receiver<bool>,
    ) -> Self {
        Self {
            opening,
            bridge,
            runtime: Handle::current(),
            stopped,
            active: watch::channel(None).0,
        }
    }

    pub(crate) fn is_stopped(&self) -> bool {
        *self.stopped.borrow()
    }

    #[cfg(test)]
    pub(crate) fn pool(&self) -> Arc<SnapshotPool> {
        self.bridge.clone()
    }

    pub(crate) fn queue(&self) -> SnapshotWaiter {
        self.bridge.join(self.opening)
    }

    pub(crate) async fn reserve(
        &self,
        interest: &TerminalInterest,
    ) -> Result<Arc<TerminalSnapshotReservation>, SnapshotWaitError> {
        self.reserve_queued(interest, self.queue()).await
    }

    pub(crate) async fn reserve_queued(
        &self,
        interest: &TerminalInterest,
        waiter: SnapshotWaiter,
    ) -> Result<Arc<TerminalSnapshotReservation>, SnapshotWaitError> {
        let mut stopped = self.stopped.clone();
        let slot = tokio::select! {
            biased;
            _ = stopped.wait_for(|stopped| *stopped) => return Err(SnapshotWaitError::Cancelled),
            _ = interest.ended() => return Err(SnapshotWaitError::Cancelled),
            _ = sleep_until(waiter.entry.expires) => return Err(SnapshotWaitError::Busy(waiter.expire())),
            slot = waiter.acquire() => slot,
        };
        let reservation = Arc::new(TerminalSnapshotReservation {
            id: slot.entry.id,
            granted: slot.granted,
            slot: Mutex::new(Some(slot)),
            active: self.active.clone(),
        });
        self.active.send_replace(Some(ActiveSnapshot {
            id: reservation.id,
            deadline: reservation.granted + Duration::from_secs(WRITE_CEILING),
            reservation: Arc::downgrade(&reservation),
        }));
        Ok(reservation)
    }

    pub(crate) fn expired_now(&self) -> Option<Arc<TerminalSnapshotReservation>> {
        let active = self.active.borrow();
        let snapshot = active
            .as_ref()
            .filter(|snapshot| Instant::now() >= snapshot.deadline)?;
        snapshot.reservation.upgrade()
    }

    pub(crate) async fn timed_out(&self) -> Arc<TerminalSnapshotReservation> {
        let mut active = self.active.subscribe();
        loop {
            let current = active.borrow_and_update().clone();
            match current {
                None => {
                    active.changed().await.expect("snapshot lane owns sender");
                }
                Some(snapshot) => {
                    tokio::select! {
                        biased;
                        _ = active.changed() => {}
                        _ = sleep_until(snapshot.deadline) => {
                            if let Some(reservation) = snapshot.reservation.upgrade() {
                                return reservation;
                            }
                        }
                    }
                }
            }
        }
    }
}

impl TerminalSnapshotReservation {
    /// Once encoded, charge elapsed capture time to the same whole-message budget.
    pub(crate) fn encoded(&self, bytes: usize) {
        let deadline = self.granted + snapshot_write_budget(bytes);
        self.active.send_if_modified(|active| {
            if let Some(snapshot) = active.as_mut().filter(|snapshot| snapshot.id == self.id) {
                if deadline < snapshot.deadline {
                    snapshot.deadline = deadline;
                    return true;
                }
            }
            false
        });
    }

    pub(crate) fn is_expired(&self) -> bool {
        self.active
            .borrow()
            .as_ref()
            .is_some_and(|snapshot| snapshot.id == self.id && Instant::now() >= snapshot.deadline)
    }

    pub(crate) fn release(&self) {
        let slot = self.slot.lock().unwrap().take();
        if slot.is_some() {
            self.active.send_if_modified(|active| {
                if active
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.id == self.id)
                {
                    *active = None;
                    return true;
                }
                false
            });
            drop(slot);
        }
    }
}

impl Drop for TerminalSnapshotReservation {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_budget_scales_from_floor_to_wire_limit() {
        assert_eq!(snapshot_write_budget(1), Duration::from_secs(30));
        assert_eq!(
            snapshot_write_budget(30 * WRITE_BYTES_PER_SECOND),
            Duration::from_secs(30)
        );
        assert_eq!(
            snapshot_write_budget(30 * WRITE_BYTES_PER_SECOND + 1),
            Duration::from_secs(31)
        );
        assert_eq!(
            snapshot_write_budget(12 * 1024 * 1024),
            Duration::from_secs(192)
        );
    }
    #[tokio::test(start_paused = true)]
    async fn expired_waiter_yields_slots_but_orders_its_refusal_before_a_same_opening_snapshot() {
        let pool = Arc::new(SnapshotPool::default());
        let waiting = pool.join(1);
        tokio::time::advance(SLOT_WAIT).await;
        let refusal = waiting.expire();
        let next = pool.join(1);
        let independent = pool.join(2);
        assert_eq!(pool.waiting(1), 1);
        assert_eq!(pool.available_permits(), 2);
        assert!(pool.try_grant(next.entry).is_none());
        let independent = pool.try_grant(independent.entry).unwrap();
        drop(refusal);
        let next = pool.try_grant(next.entry).unwrap();
        assert_eq!(pool.available_permits(), 0);
        drop((independent, next));
        assert_eq!(pool.available_permits(), 2);
    }
}
