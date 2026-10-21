//! Explicit short pipeline control; never an original qualification attempt.
use super::*;

#[derive(Clone, Copy, Debug)]
pub struct PipelineControl;
impl PipelineControl {
    pub const WINDOW: Duration = Duration::from_secs(2);
    /// Explicit native8 RPC subset, no Router/MCP reads. Shared immutable
    /// write/exempt constructors retain normal semantics and finite targets.
    pub fn plan(name: TrialName, catalog: &ReadCatalog) -> Result<Vec<Slot>, String> {
        let plan = make_pipeline_control_plan(name, catalog)?;
        let number = |traffic| plan.iter().filter(|slot| slot.traffic == traffic).count();
        if (
            number(Traffic::Write),
            number(Traffic::Exempt),
            number(Traffic::Read),
        ) != (80, 200, 32)
        {
            return Err(
                "two-second immutable prefix changed; expected80 writes/200 exempts/32 reads"
                    .into(),
            );
        }
        Ok(plan)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PipelineControlReport {
    pub schema: String,
    pub qualification: bool,
    pub pipeline_succeeded: bool,
    pub transport_schedule_completed: bool,
    pub explicit_proof_gaps: Vec<ControlProofGap>,
    pub fault_overload: Option<FaultOverloadState>,
    pub trial_seed: TrialName,
    pub provenance: Provenance,
    pub started_unix_us: u128,
    pub offer_window_us: u64,
    pub observed_end_us: u64,
    pub counts_at_offer_end: Option<BTreeMap<Traffic, TrafficCounts>>,
    pub final_counts: BTreeMap<Traffic, TrafficCounts>,
    pub methods: Vec<MethodMetrics>,
    pub endpoint_observations: BTreeMap<Stage, usize>,
    pub violations: Vec<Violation>,
    pub failures: Vec<FailureSnapshot>,
    pub last_probe: ProbeSnapshot,
    pub cleanup_drained: bool,
    pub backend_closed: bool,
    pub invalid_infrastructure: Option<String>,
}
pub struct PipelineControlRun {
    pub report: PipelineControlReport,
    pub unfinished: Option<UnfinishedCleanup>,
}

pub async fn run_pipeline_control(
    name: TrialName,
    backend: Arc<dyn TrialBackend>,
    sink: &dyn ArtifactSink,
) -> Result<PipelineControlRun, String> {
    let plan = PipelineControl::plan(name, &backend.catalog())?;
    let mut run = RunState::with_plan(name, backend, sink, plan, PipelineControl::WINDOW)?;
    // Same offers, credits, transport calls, oracles, deadline watcher and drain
    // as normal runs. Final475ms work retains its actual post-window tail.
    run.drive().await;
    run.finish_control()
}
impl RunState<'_> {
    pub(super) fn finish_control(mut self) -> Result<PipelineControlRun, String> {
        if self.failed || self.infrastructure.is_some() {
            self.producers.stop();
        }
        if let Some(watchdog) = self.watchdog.take() {
            watchdog.finish();
        }
        self.backend
            .archive_into(&self.observe)
            .unwrap_or_else(|error| self.observe.fail(None, "final_archive", error));
        let last_probe = self.probe.snapshot();
        let cleanup_drained = self.obligations_quiet(&last_probe);
        let rows = self
            .observe
            .ledger
            .lock()
            .unwrap()
            .rows
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for gap in &self.control_gaps {
            self.observe.fail(
                Some(gap.original_id.clone()),
                "missing_control_proof",
                gap.detail.clone(),
            );
        }
        let accepted = self.accepted_fault_rows(&rows);
        validate_final_rows(&accepted, &self.observe);
        // No1000-sample loaded-read qualification gate is weakened or called
        // for this4-round control. All per-ID5s/registry/service gates remain.
        let report = self.control_report(&rows, last_probe, cleanup_drained);
        self.persist_control(report, rows, cleanup_drained)
    }
    fn control_report(
        &self,
        rows: &[Row],
        last_probe: ProbeSnapshot,
        cleanup_drained: bool,
    ) -> PipelineControlReport {
        let mut final_counts = counts(rows.iter(), None);
        control_rates(&mut final_counts, self.offer_window.as_secs_f64());
        let mut at_end = self.at_600.clone();
        if let Some(counts) = &mut at_end {
            control_rates(counts, self.offer_window.as_secs_f64());
        }
        let mut methods = method_metrics(rows);
        for method in &mut methods {
            rate(&mut method.counts, self.offer_window.as_secs_f64());
        }
        let ledger = self.observe.ledger.lock().unwrap();
        let complete = finish::complete_plan(at_end.is_some(), &final_counts);
        PipelineControlReport {
            schema: "build.pipeline-control.NoQualification.v1-draft".into(),
            qualification: false,
            pipeline_succeeded: ledger.violations.is_empty()
                && complete
                && cleanup_drained
                && self.backend.closed()
                && self.infrastructure.is_none(),
            transport_schedule_completed: final_counts.values().all(|count| {
                count.offered == count.intended
                    && count.sent == count.intended
                    && count.completed == count.intended
            }),
            explicit_proof_gaps: self.control_gaps.clone(),
            fault_overload: self.fault_overload.clone(),
            trial_seed: self.name,
            provenance: self.provenance.clone(),
            started_unix_us: self.started_unix_us,
            offer_window_us: self.offer_window.as_micros() as u64,
            observed_end_us: self.observe.now_us(),
            counts_at_offer_end: at_end,
            final_counts,
            methods,
            endpoint_observations: finish::endpoint_counts(rows),
            violations: ledger.violations.clone(),
            failures: self.failures.clone(),
            last_probe,
            cleanup_drained,
            backend_closed: self.backend.closed(),
            invalid_infrastructure: self.infrastructure.clone(),
        }
    }
    fn persist_control(
        self,
        mut report: PipelineControlReport,
        rows: Vec<Row>,
        cleanup_drained: bool,
    ) -> Result<PipelineControlRun, String> {
        let other_frames = self.observe.ledger.lock().unwrap().other_frames.clone();
        if let Err(error) = self.sink.pipeline_control(&report, &rows, &other_frames) {
            report.pipeline_succeeded = false;
            report.invalid_infrastructure =
                Some(format!("control artifact persistence failed: {error}"));
        }
        let unfinished = (!cleanup_drained || !self.backend.closed()).then(|| UnfinishedCleanup {
            original_ids: self.running.keys().cloned().collect(),
            tasks: self
                .running
                .into_values()
                .map(|execution| execution.task)
                .collect(),
            _retained_backend: self.backend,
            _retained_producers: self.producers,
        });
        Ok(PipelineControlRun { report, unfinished })
    }
}
fn control_rates(counts: &mut BTreeMap<Traffic, TrafficCounts>, seconds: f64) {
    for count in counts.values_mut() {
        rate(count, seconds);
    }
}
fn rate(count: &mut TrafficCounts, seconds: f64) {
    count.offered_per_second = count.offered as f64 / seconds;
    count.sent_per_second = count.sent as f64 / seconds;
    count.admitted_per_second = count.admitted as f64 / seconds;
    count.completed_per_second = count.completed as f64 / seconds;
}
