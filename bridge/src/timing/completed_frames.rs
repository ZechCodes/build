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
        self.completed_observed.store(true, Ordering::Release);
    }
}

pub(super) struct Capture {
    observer: Arc<CompletedObserver>,
    original_ids: Vec<String>,
    protocol: FrameProtocol,
}

pub(super) fn start(
    clock: &FrameClock,
    origin: Option<&write_origin::WriteOrigin>,
    inherited: Option<&write_wait::WriteContext>,
) -> Option<Capture> {
    if !clock.completed_observed.load(Ordering::Relaxed) {
        return None;
    }
    let observer = clock.completed_observer.read().unwrap().clone()?;
    let (original_ids, protocol) = origin.map_or_else(
        || {
            inherited.map_or_else(
                || (Vec::new(), FrameProtocol::Background),
                |context| (context.original_ids(), FrameProtocol::RequiredWork),
            )
        },
        |origin| (origin.context().original_ids(), origin.frame_protocol()),
    );
    Some(Capture {
        observer,
        original_ids,
        protocol,
    })
}

impl FrameTimer {
    pub(crate) fn capture_completed_request(&mut self, frame: &crate::transport::Frame) {
        if let Some(capture) = &mut self.completed_capture {
            capture.original_ids = vec![write_origin::original_id(&frame.payload)];
            capture.protocol = FrameProtocol::Rpc;
        }
    }

    pub(crate) fn capture_completed_context(&mut self, context: Option<&write_wait::WriteContext>) {
        let (Some(capture), Some(context)) = (&mut self.completed_capture, context) else {
            return;
        };
        // A single actual delivery frame may drain several owned turns. Preserve
        // every original from that one timer; never add duplicate frame samples.
        for original in context.original_ids() {
            if !capture.original_ids.contains(&original) {
                capture.original_ids.push(original);
            }
        }
        capture.protocol = FrameProtocol::RequiredWork;
    }
}

pub(super) fn record(timer: &FrameTimer, spent: &Spent) {
    let Some(capture) = &timer.completed_capture else {
        return;
    };
    (capture.observer)(CompletedFrame {
        original_ids: capture.original_ids.clone(),
        protocol: capture.protocol,
        method: timer.sample_label(),
        queued: spent.queued,
        permit_wait: spent.permit_wait,
        pool_wait: spent.pool_wait,
        lock_wait: spent.lock_wait,
        held: spent.held,
        total: spent.total,
    });
}
