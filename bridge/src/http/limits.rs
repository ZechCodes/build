//! Adapter buffers are reserved before reading bodies, never under app locks.
//! This bounds wire allowances; parsed values, normalization and domain decoded
//! copies consume additional memory. Large-work permits survive cancellation in
//! the shared execution reply target until actual work has settled.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const TOTAL_BUFFER_BYTES: usize = 32 * 1024 * 1024;
/// Event streams draw only on this pool, never on `TOTAL_BUFFER_BYTES`: one
/// `MAX_QUEUED_BYTES` queue each, so thirty-two streams at once, and a full
/// pool refuses the next stream without costing an RPC its buffers. Queued
/// adapter bytes are therefore bounded by 32 MiB of RPC plus 32 MiB of stream
/// buffers, before parse and encode copies.
pub const STREAM_BUFFER_BYTES: usize = 32 * 1024 * 1024;
const BULK_BUFFER_BYTES: usize = 22 * 1024 * 1024;
pub const DEFAULT_BODY_BYTES: usize = 1024 * 1024;
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
pub const MAX_LIFECYCLE_BYTES: usize = 64 * 1024;
pub const MAX_DISCOVERY_BYTES: usize = 16 * 1024;
pub const MAX_SOCKETS: usize = 128;
pub const MAX_FAMILY_OPENINGS: usize = 16;
/// Live streams one family may hold. The pool holds 32 streams, so no family
/// can fill more than a quarter of it. A replaced stream counts until its
/// socket lets it go.
pub const MAX_FAMILY_STREAMS: usize = 8;
const _: () = assert!(STREAM_BUFFER_BYTES / super::sse::MAX_QUEUED_BYTES >= 4 * MAX_FAMILY_STREAMS);
pub const HEADER_TIMEOUT: Duration = Duration::from_secs(5);
pub const BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(10);
pub const SOCKET_TEARDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaExceeded;

pub struct BufferBudget {
    bytes: Arc<Semaphore>,
    bulk_bytes: Arc<Semaphore>,
    stream_bytes: Arc<Semaphore>,
    family_streams: Arc<Mutex<HashMap<String, usize>>>,
    large_calls: Arc<Semaphore>,
}

pub struct OpeningBudget(Arc<Semaphore>);

/// The reply target owns this guard, so closing HTTP interest cannot release
/// bulk concurrency while the shared execution worker still owns body copies.
pub struct BufferReservation {
    // Fields drop in order: the family count is released before the pool
    // permit, on purpose, so a returned permit never meets a stale count.
    _family: Option<FamilyStream>,
    _permits: Vec<OwnedSemaphorePermit>,
}

/// One of a family's live streams, counted until its reservation drops.
struct FamilyStream {
    families: Arc<Mutex<HashMap<String, usize>>>,
    family: String,
    #[cfg(test)]
    drop_started: Option<std::sync::mpsc::Sender<()>>,
}

impl Drop for FamilyStream {
    fn drop(&mut self) {
        #[cfg(test)]
        if let Some(started) = self.drop_started.take() {
            let _ = started.send(());
        }
        let mut families = self.families.lock().unwrap();
        if let Some(count) = families.get_mut(&self.family) {
            *count -= 1;
            if *count == 0 {
                families.remove(&self.family);
            }
        }
    }
}

impl Default for BufferBudget {
    fn default() -> Self {
        Self {
            bytes: Arc::new(Semaphore::new(TOTAL_BUFFER_BYTES)),
            bulk_bytes: Arc::new(Semaphore::new(BULK_BUFFER_BYTES)),
            stream_bytes: Arc::new(Semaphore::new(STREAM_BUFFER_BYTES)),
            family_streams: Arc::default(),
            large_calls: Arc::new(Semaphore::new(2)),
        }
    }
}

impl Default for OpeningBudget {
    fn default() -> Self {
        Self(Arc::new(Semaphore::new(1)))
    }
}

impl BufferBudget {
    pub fn reserve(
        &self,
        input: usize,
        output: usize,
        opening: &OpeningBudget,
    ) -> Result<BufferReservation, QuotaExceeded> {
        let total = input
            .checked_add(output)
            .filter(|value| *value <= TOTAL_BUFFER_BYTES)
            .ok_or(QuotaExceeded)?;
        let mut permits = Vec::with_capacity(4);
        if input > DEFAULT_BODY_BYTES {
            permits.push(acquire(&self.large_calls, 1)?);
            permits.push(acquire(&opening.0, 1)?);
            permits.push(acquire(&self.bulk_bytes, total)?);
        }
        // Full input+output reservation means no wait for output capacity while
        // retaining input. Bulk cannot consume small-RPC's 10MiB headroom.
        permits.push(acquire(&self.bytes, total)?);
        Ok(BufferReservation {
            _permits: permits,
            _family: None,
        })
    }

    /// Refuses, reserving nothing, when `family` already holds
    /// `MAX_FAMILY_STREAMS` streams or the pool is full.
    pub fn reserve_stream(&self, family: &str) -> Result<BufferReservation, QuotaExceeded> {
        let mut families = self.family_streams.lock().unwrap();
        let count = families.get(family).copied().unwrap_or(0);
        if count >= MAX_FAMILY_STREAMS {
            return Err(QuotaExceeded);
        }
        let permit = acquire(&self.stream_bytes, super::sse::MAX_QUEUED_BYTES)?;
        families.insert(family.to_owned(), count + 1);
        Ok(BufferReservation {
            _permits: vec![permit],
            _family: Some(FamilyStream {
                families: self.family_streams.clone(),
                family: family.to_owned(),
                #[cfg(test)]
                drop_started: None,
            }),
        })
    }

    pub fn available(&self) -> usize {
        self.bytes.available_permits()
    }

    pub fn streams_available(&self) -> usize {
        self.stream_bytes.available_permits()
    }
}

fn acquire(
    semaphore: &Arc<Semaphore>,
    bytes: usize,
) -> Result<OwnedSemaphorePermit, QuotaExceeded> {
    let count = u32::try_from(bytes).map_err(|_| QuotaExceeded)?;
    semaphore
        .clone()
        .try_acquire_many_owned(count)
        .map_err(|_| QuotaExceeded)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pass: even if family cleanup is blocked by another admission, a stream
    /// cannot advertise returned pool bytes while its family count is stale.
    /// The rendezvous makes the field drop order deterministic, without sleeps.
    #[test]
    fn buffer_reservation_releases_family_before_returning_pool_permits() {
        let budget = BufferBudget::default();
        let mut reservation = budget.reserve_stream("family").unwrap();
        let held = budget.streams_available();
        let (started, entered) = std::sync::mpsc::channel();
        reservation._family.as_mut().unwrap().drop_started = Some(started);
        let families = budget.family_streams.lock().unwrap();
        let worker = std::thread::spawn(move || drop(reservation));
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        let pool_still_held = budget.streams_available() == held;
        drop(families);
        worker.join().unwrap();
        assert!(pool_still_held, "pool bytes returned before family cleanup");
        assert_eq!(budget.streams_available(), STREAM_BUFFER_BYTES);
        assert!(budget.family_streams.lock().unwrap().is_empty());
    }
}
