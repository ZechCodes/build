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

    /// Attribute one physical off-lock RPC computation to every real caller
    /// retained by its folded work item. This changes no timer or write root.
    pub(crate) fn capture_completed_rpc_ids(&mut self, ids: impl IntoIterator<Item = Value>) {
        if let Some(capture) = &mut self.completed_capture {
            capture.original_ids = ids
                .into_iter()
                .map(|id| match id {
                    Value::String(id) => id,
                    id => id.to_string(),
                })
                .collect();
            capture.protocol = FrameProtocol::Rpc;
        }
    }

    /// Native MCP reads carry adapter identity in the request, never an echoed
    /// response ID. Binding it here does not create a semantic WriteOrigin.
    pub(crate) fn capture_completed_mcp_request(&mut self, frame: &Value) {
        if let Some(capture) = &mut self.completed_capture {
            capture.original_ids = vec![write_origin::original_id(frame)];
            capture.protocol = FrameProtocol::Mcp;
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

#[test]
fn read_identity_binding_keeps_one_physical_frame_and_creates_no_write_roots() {
    let clock = FrameClock::new();
    let registry = clock.capture_write_waits();
    let rows = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&rows);
    clock.observe_completed_frames(Arc::new(move |frame| captured.lock().unwrap().push(frame)));
    {
        let mut timer = clock.frame("tasks.get");
        timer.capture_completed_rpc_ids([json!("folded/first"), json!(7)]);
    }
    {
        let mut timer = clock.frame("mcp.control");
        timer.capture_completed_mcp_request(&json!({"id":"mcp/read/original"}));
    }
    let rows = rows.lock().unwrap();
    assert_eq!(rows.len(), 2, "one row per actual timer, not per folded caller");
    assert_eq!(rows[0].original_ids, ["folded/first", "7"]);
    assert_eq!(rows[0].protocol, FrameProtocol::Rpc);
    assert_eq!(rows[1].original_ids, ["mcp/read/original"]);
    assert_eq!(rows[1].protocol, FrameProtocol::Mcp);
    assert!(registry.snapshot().is_empty(), "read binding must not mint write roots");
}
