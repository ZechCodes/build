//! Drain actual completed frames; registry and service proof uses cursor owners.
use super::super::trial_complete::{FrameTrace, Observation};
use crate::timing::completed_frames::CompletedFrame;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(super) struct FrameArchive {
    rows: Mutex<Vec<(Instant, CompletedFrame)>>,
}
impl FrameArchive {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            rows: Mutex::new(Vec::new()),
        })
    }
    pub fn receive(&self, frame: CompletedFrame) {
        self.rows.lock().unwrap().push((Instant::now(), frame));
    }
    pub fn drain_into(&self, observe: &Observation) -> Result<(), String> {
        let rows = std::mem::take(&mut *self.rows.lock().unwrap());
        for (sampled, frame) in rows {
            observe.frame(FrameTrace {
                observer_id: 0,
                original_ids: frame.original_ids,
                protocol: format!("{:?}", frame.protocol),
                method: frame.method,
                sampled_at_us: observe.at_us(sampled),
                queued_us: us(frame.queued),
                permit_wait_us: frame.permit_wait.map(us),
                pool_wait_us: frame.pool_wait.map(us),
                lock_wait_us: us(frame.lock_wait),
                held_us: us(frame.held),
                total_us: us(frame.total),
            })?;
        }
        Ok(())
    }
}

fn us(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}
