//! Off-source split of the frozen coordinator; no execution evidence.
use super::*;

struct Finished {
    original_id: String,
    result: Result<ControlExecution, String>,
}
/// Detached only after a bounded cleanup FAILURE. Dropping JoinHandle does not
/// abort its task. Each task owns backend Arc and hence all fixture resources.
/// The supervisor must preserve artifacts and stop its own verified process if
/// these never finish; it must not start another qualification trial meanwhile.
pub struct UnfinishedCleanup {
    pub original_ids: Vec<String>,
    pub tasks: Vec<JoinHandle<()>>,
    pub backend: Arc<dyn TrialBackend>,
    pub producers: ProducerSet,
}
pub struct TrialRun {
    pub report: TrialReport,
    pub unfinished: Option<UnfinishedCleanup>,
}

type Running = BTreeMap<String, (usize, usize, JoinHandle<()>)>;
struct RunState<'a> {
    name: TrialName,
    backend: Arc<dyn TrialBackend>,
    sink: &'a dyn ArtifactSink,
    provenance: Provenance,
    plan: Vec<Slot>,
    recipes: BTreeMap<String, Recipe>,
    producers: ProducerSet,
    probe: Arc<dyn IndependentProbe>,
    observe: Observation,
    watchdog: Option<WatchThread>,
    watch_rx: mpsc::UnboundedReceiver<Violation>,
    done_tx: mpsc::UnboundedSender<Finished>,
    done_rx: mpsc::UnboundedReceiver<Finished>,
    pending: [VecDeque<Slot>; 3 * CALLERS],
    active: [usize; 3 * CALLERS],
    sockets: [usize; CALLERS],
    running: Running,
    start: Instant,
    end: Instant,
    offer_window: Duration,
    started_unix_us: u128,
    next: usize,
    failed: bool,
    infrastructure: Option<String>,
    failures: Vec<FailureSnapshot>,
    control: bool,
    control_effects: BTreeSet<String>,
    control_gaps: Vec<ControlProofGap>,
    at_600: Option<BTreeMap<Traffic, TrafficCounts>>,
    drain_deadline: Option<Instant>,
    next_progress: Instant,
}
#[path = "runner/control.rs"]
mod control;
#[path = "runner/credits.rs"]
mod credits;
#[path = "runner/dispatch.rs"]
mod dispatch;
#[path = "runner/drain.rs"]
mod drain;
#[path = "runner/finish.rs"]
mod finish;
#[path = "runner/setup.rs"]
mod setup;
pub use control::{
    run_pipeline_control, PipelineControl, PipelineControlReport, PipelineControlRun,
};

pub async fn run_trial(
    name: TrialName,
    backend: Arc<dyn TrialBackend>,
    sink: &dyn ArtifactSink,
) -> Result<TrialRun, String> {
    let mut run = RunState::new(name, backend, sink)?;
    run.drive().await;
    run.finish()
}
impl RunState<'_> {
    async fn drive(&mut self) {
        loop {
            let now = Instant::now();
            self.capture_failures();
            self.enter_drain(now);
            if !self.failed {
                self.offer_due(now);
                self.dispatch_ready();
            }
            self.collect_done().await;
            self.detect_crashed();
            if let Err(error) = self.backend.archive_into(&self.observe) {
                self.observe.fail(None, "archive", error);
            }
            if self.close_when_quiet() {
                break;
            }
            if self.cleanup_expired(now) {
                break;
            }
            self.progress(now);
            self.pause(now).await;
        }
    }
}
