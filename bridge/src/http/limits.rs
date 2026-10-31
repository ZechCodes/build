//! Adapter buffers are reserved before reading bodies, never under app locks.
//! This bounds wire allowances; parsed values, normalization and domain decoded
//! copies consume additional memory. Large-work permits survive cancellation in
//! the shared execution reply target until actual work has settled.
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const TOTAL_BUFFER_BYTES: usize = 32 * 1024 * 1024;
const BULK_BUFFER_BYTES: usize = 22 * 1024 * 1024;
pub const DEFAULT_BODY_BYTES: usize = 1024 * 1024;
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
pub const MAX_LIFECYCLE_BYTES: usize = 64 * 1024;
pub const MAX_DISCOVERY_BYTES: usize = 16 * 1024;
pub const MAX_SOCKETS: usize = 128;
pub const MAX_FAMILY_OPENINGS: usize = 16;
pub const HEADER_TIMEOUT: Duration = Duration::from_secs(5);
pub const BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(10);
pub const SOCKET_TEARDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaExceeded;

pub struct BufferBudget {
    bytes: Arc<Semaphore>,
    bulk_bytes: Arc<Semaphore>,
    large_calls: Arc<Semaphore>,
}

pub struct OpeningBudget(Arc<Semaphore>);

/// The reply target owns this guard, so closing HTTP interest cannot release
/// bulk concurrency while the shared execution worker still owns body copies.
pub struct BufferReservation {
    _permits: Vec<OwnedSemaphorePermit>,
}

impl Default for BufferBudget {
    fn default() -> Self {
        Self {
            bytes: Arc::new(Semaphore::new(TOTAL_BUFFER_BYTES)),
            bulk_bytes: Arc::new(Semaphore::new(BULK_BUFFER_BYTES)),
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
        Ok(BufferReservation { _permits: permits })
    }

    pub fn reserve_stream(&self) -> Result<BufferReservation, QuotaExceeded> {
        Ok(BufferReservation {
            _permits: vec![acquire(&self.bytes, super::sse::MAX_QUEUED_BYTES)?],
        })
    }

    pub fn available(&self) -> usize {
        self.bytes.available_permits()
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
