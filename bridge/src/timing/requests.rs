//! Opt-in per-request timestamps for paired, test-only execution measurements.
use super::*;
use crate::transport::Frame;

pub(crate) type RequestObserver = dyn Fn(&Frame, &'static str, Instant) + Send + Sync;

impl FrameClock {
    pub(crate) fn observe_requests(&self, observer: Arc<RequestObserver>) {
        *self.request_observer.write().unwrap() = Some(observer);
        self.request_observed.store(true, Ordering::Release);
    }

    pub(crate) fn observes_requests(&self) -> bool {
        self.request_observed.load(Ordering::Relaxed)
    }

    pub(crate) fn request_frame(&self, frame: &Frame) -> Option<Frame> {
        self.request_observed
            .load(Ordering::Relaxed)
            .then(|| frame.clone())
    }

    pub(crate) fn mark_observed(&self, frame: &Option<Frame>, phase: &'static str, at: Instant) {
        if let Some(frame) = frame {
            self.mark_request(frame, phase, at);
        }
    }

    pub(crate) fn mark_request(&self, frame: &Frame, phase: &'static str, at: Instant) {
        if !self.request_observed.load(Ordering::Relaxed) {
            return;
        }
        let observer = self.request_observer.read().unwrap().clone();
        if let Some(observer) = observer {
            observer(frame, phase, at);
        }
    }
}

impl WaitTimer {
    pub(crate) fn started_at(&self) -> Instant {
        self.0
    }
}

impl FrameTimer {
    pub(crate) fn bind_request(&mut self, frame: &Frame) {
        if self.clock.request_observed.load(Ordering::Relaxed) {
            self.request = Some(frame.clone());
        }
    }

    pub(crate) fn mark_request(&self, phase: &'static str) {
        if let Some(frame) = &self.request {
            self.clock.mark_request(frame, phase, Instant::now());
        }
    }
}

#[test]
fn request_marks_preserve_saved_intake_and_bound_frame_identity() {
    let clock = FrameClock::new();
    let received = Arc::new(Mutex::new(Vec::new()));
    let records = received.clone();
    clock.observe_requests(Arc::new(move |frame, phase, at| {
        records
            .lock()
            .unwrap()
            .push((frame.payload["id"].clone(), phase, at));
    }));
    let frame: Frame = serde_json::from_value(json!({"session_id":"s","message_id":"m","frame_type":"data","sender":"client","created_at":"","payload":{"id":17}})).unwrap();
    let arrival = WaitTimer::start().started_at();
    clock.mark_request(&frame, "intake", arrival);
    let mut timer = clock.frame("ping");
    timer.bind_request(&frame);
    timer.mark_request("worker_start");
    let records = received.lock().unwrap();
    assert_eq!(records[0], (json!(17), "intake", arrival));
    assert_eq!(records[1].0, json!(17));
    assert_eq!(records[1].1, "worker_start");
    assert!(records[1].2 >= arrival);
}
