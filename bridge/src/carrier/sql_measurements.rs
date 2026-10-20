//! Test-only measurements around actual SQL scheduling boundaries.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::FrameHandler;
use crate::transport::Frame;

pub(super) type SqlObserver = Arc<dyn Fn(&Frame, Duration, Duration) + Send + Sync>;

#[derive(Default)]
pub(super) struct SqlSchedule {
    checkout: Duration,
    handed_off: Option<Instant>,
}

impl SqlSchedule {
    pub(super) fn new(checkout: Duration) -> Self {
        Self {
            checkout,
            handed_off: Some(Instant::now()),
        }
    }

    pub(super) fn observe(&self, handler: &FrameHandler, frame: &Frame) {
        if let (Some(observer), Some(handed_off)) = (&handler.sql_observer, self.handed_off) {
            observer(frame, self.checkout, handed_off.elapsed());
        }
    }
}
