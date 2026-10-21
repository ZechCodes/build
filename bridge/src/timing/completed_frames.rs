//! Opt-in completed frames, paired with exact original caller identities.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FrameProtocol {
    Rpc,
    Mcp,
    RequiredWork,
    Background,
}

#[derive(Clone, Debug)]
pub(crate) struct CompletedFrame {
    pub original_ids: Vec<String>,
    pub protocol: FrameProtocol,
    pub method: String,
    pub queued: Duration,
    pub permit_wait: Option<Duration>,
    pub pool_wait: Option<Duration>,
    pub lock_wait: Duration,
    pub held: Duration,
    pub total: Duration,
}

pub(crate) type CompletedObserver = dyn Fn(CompletedFrame) + Send + Sync;

impl FrameClock {
    pub(crate) fn observe_completed_frames(&self, observer: Arc<CompletedObserver>) {
        *self.completed_observer.write().unwrap() = Some(observer);
    }
}
