//! Opt-in attribution carried by real frame scheduling, separate from frame totals.
use super::*;
use completed_frames::FrameProtocol;
use write_wait::{IntervalCategory, LogicalWrite, WaitGuard, WaitKind, WriteCohort, WriteContext};

// Section 8's audited protocol writes; suffixes inherit no exemption.
const PROTOCOL_WRITES: &[&str] = &[
    "session.hello",
    "changes.subscribe",
    "changes.unsubscribe",
    "term.attach",
    "agent.attach",
    "term.input",
    "term.resize",
    "term.ack",
    "stream.start",
];

/// Shared attribution does not create descendants. The outer reply path owns
/// completion; dropping its last handle without completion preserves abandonment.
pub(crate) struct WriteOrigin {
    root: Mutex<Option<LogicalWrite>>,
    context: WriteContext,
    protocol: FrameProtocol,
    queued: Mutex<Option<WaitGuard>>,
}

impl WriteOrigin {
    fn new(root: LogicalWrite, protocol: FrameProtocol) -> Arc<Self> {
        let context = root.context();
        Arc::new(Self {
            root: Mutex::new(Some(root)),
            context,
            protocol,
            queued: Mutex::new(None),
        })
    }

    pub(crate) fn frame_protocol(&self) -> FrameProtocol {
        self.protocol
    }

    pub(crate) fn context(&self) -> WriteContext {
        self.context.clone()
    }

    pub(crate) fn classify(&self, frame: &crate::transport::Frame) -> bool {
        let method = frame.payload["method"]
            .as_str()
            .unwrap_or(&frame.frame_type);
        use crate::api::effects::{effect, Effect};
        let cohort = match effect(method) {
            Some(Effect::Write) if PROTOCOL_WRITES.contains(&method) => WriteCohort::ExemptProtocol,
            Some(Effect::Write) => WriteCohort::SemanticWrite,
            Some(Effect::Control) => WriteCohort::ExemptProtocol,
            Some(_) => WriteCohort::ExemptRead,
            None if frame.payload.get("method").is_none() => WriteCohort::ExemptProtocol,
            None => WriteCohort::SemanticWrite,
        };
        if let Some(root) = self.root.lock().unwrap().as_ref() {
            root.classify(original_id(&frame.payload), method, cohort);
            root.admitted();
        }
        if cohort != WriteCohort::SemanticWrite {
            // Only provisional admission is charged to an exempt request.
            self.settled();
            return false;
        }
        *self.queued.lock().unwrap() = Some(self.wait(WaitKind::Dispatch));
        true
    }

    pub(crate) fn wait(&self, kind: WaitKind) -> WaitGuard {
        self.context.interval(IntervalCategory::KnownWait(kind))
    }

    pub(crate) fn execution_started(&self) {
        self.queued.lock().unwrap().take();
    }

    pub(crate) fn cancel(&self) {
        self.execution_started();
        if let Some(root) = self.root.lock().unwrap().take() {
            root.cancel();
        }
    }

    pub(crate) fn settled(&self) {
        self.execution_started();
        if let Some(root) = self.root.lock().unwrap().take() {
            root.complete();
        }
    }

    pub(crate) fn complete(&self) {
        self.execution_started();
        if let Some(root) = self.root.lock().unwrap().take() {
            root.acknowledged();
            root.complete();
        }
    }
}

impl Drop for WriteOrigin {
    fn drop(&mut self) {
        // Close the queue interval while its root obligation is still live.
        self.queued.get_mut().unwrap().take();
    }
}

pub(crate) fn original_id(frame: &Value) -> String {
    match frame.get("id") {
        Some(Value::String(id)) => id.clone(),
        Some(id) => id.to_string(),
        None => String::new(),
    }
}

impl FrameClock {
    pub(crate) fn background_write(&self, method: &str) -> Option<write_wait::LogicalWrite> {
        let registry = self.write_wait_registry.read().unwrap().clone()?;
        let original = format!("background/{method}/{}", uuid::Uuid::new_v4());
        let write = registry.begin_background(original, method, registry.now());
        write.admitted();
        Some(write)
    }

    pub(crate) fn set_write_wait_registry(&self, registry: Arc<write_wait::WriteWaitRegistry>) {
        *self.write_wait_registry.write().unwrap() = Some(registry);
    }

    pub(crate) fn pending_write(
        &self,
        correlation: &str,
        intake: Instant,
    ) -> Option<Arc<WriteOrigin>> {
        let registry = self.write_wait_registry.read().unwrap().clone()?;
        Some(WriteOrigin::new(
            registry.begin_pending(correlation, intake),
            FrameProtocol::Rpc,
        ))
    }

    pub(crate) fn mcp_write(&self, frame: &Value, intake: Instant) -> Option<Arc<WriteOrigin>> {
        let registry = self.write_wait_registry.read().unwrap().clone()?;
        let tool =
            if serde_json::from_value::<crate::mcp::DoneReport>(frame["report"].clone()).is_ok() {
                "done"
            } else {
                serde_json::from_value::<crate::mcp::BridgeAction>(frame["request"].clone())
                    .map(|action| action.tool_name())
                    .unwrap_or("done")
            };
        let root = registry.begin(original_id(frame), format!("mcp.control:{tool}"), intake);
        root.admitted();
        Some(WriteOrigin::new(root, FrameProtocol::Mcp))
    }
}

impl WaitTimer {
    pub(crate) fn with_write_origin(mut self, origin: Option<Arc<WriteOrigin>>) -> Self {
        self.1 = origin;
        self
    }
    pub(crate) fn write_origin(&self) -> Option<Arc<WriteOrigin>> {
        self.1.clone()
    }
}
impl QueuedFrame {
    pub(crate) fn write_origin(&self) -> Option<Arc<WriteOrigin>> {
        self.write_origin.clone()
    }
}
impl FrameTimer {
    pub(crate) fn with_write_context(mut self, context: Option<WriteContext>) -> Self {
        self.set_write_context(context);
        self
    }
    pub(crate) fn set_write_context(&mut self, context: Option<WriteContext>) {
        self.capture_completed_context(context.as_ref());
        self.write_context = context;
    }

    pub(crate) fn write_context(&self) -> Option<WriteContext> {
        self.write_context
            .clone()
            .or_else(|| self.write_origin.as_ref().map(|origin| origin.context()))
    }
    pub(crate) fn write_wait(&self, kind: WaitKind) -> Option<WaitGuard> {
        self.write_context()
            .map(|context| context.interval(IntervalCategory::KnownWait(kind)))
    }
}

#[test]
fn logical_write_runtime_protocol_exemptions_use_audited_exact_names() {
    const EXEMPT: &[&str] = &[
        "session.hello",
        "changes.subscribe",
        "changes.unsubscribe",
        "term.attach",
        "agent.attach",
        "term.input",
        "term.resize",
        "term.ack",
        "stream.start",
    ];
    let registry = write_wait::WriteWaitRegistry::new();
    let clock = FrameClock::new();
    clock.set_write_wait_registry(Arc::clone(&registry));
    let classify = |method: &str| {
        let origin = clock
            .pending_write("encrypted-correlation", Instant::now())
            .unwrap();
        let frame = crate::transport::Frame {
            session_id: "logical-classifier".into(),
            message_id: method.into(),
            frame_type: "data".into(),
            sender: "client".into(),
            created_at: String::new(),
            payload: json!({"id":format!("raw/{method}"), "method":method, "params":{}}),
        };
        let semantic = origin.classify(&frame);
        origin.complete();
        let row = registry
            .snapshot()
            .into_iter()
            .find(|row| row.method == method)
            .unwrap();
        assert_eq!(row.original_id, format!("raw/{method}"));
        assert!(row.root_finished && row.outstanding.is_empty());
        (method.to_owned(), semantic, row.cohort)
    };
    let exemptions: Vec<_> = EXEMPT.iter().map(|method| classify(method)).collect();
    for method in [
        "settings.set",
        "term.input.extra",
        "changes.subscribe.extra",
    ] {
        let (_, semantic, cohort) = classify(method);
        assert!(
            semantic,
            "unknown suffixes cannot inherit protocol exemptions"
        );
        assert_eq!(cohort, WriteCohort::SemanticWrite);
    }
    for method in ["workspace.get", "tasks.get"] {
        let (_, semantic, cohort) = classify(method);
        assert!(!semantic);
        assert_eq!(cohort, WriteCohort::ExemptRead);
    }
    assert_eq!(exemptions.len(), 9);
    assert!(
        exemptions
            .iter()
            .all(|(_, semantic, cohort)| !semantic && *cohort == WriteCohort::ExemptProtocol),
        "exact audited runtime dispositions: {exemptions:?}"
    );
}
