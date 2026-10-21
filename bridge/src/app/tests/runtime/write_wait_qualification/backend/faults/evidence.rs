use super::*;
use serde::Serialize;
#[derive(Serialize)]
pub(in crate::app::tests::runtime::write_wait_qualification) struct FaultReport {
    pub schema: &'static str,
    pub qualification: bool,
    pub case: String,
    pub original_id: String,
    pub durable_operation_id: Option<String>,
    pub physical_hold_us: u64,
    pub observed_wait_before_release: Option<Value>,
    pub completed_original: Option<Value>,
    pub actual_registry_boundary: Value,
    pub client_rows: Vec<Value>,
    pub client_watch_report: Value,
    pub registry_watch_reports: Vec<Value>,
    pub actual_decoded_reply: Option<Value>,
    pub effect_checks: Value,
    pub errors: Vec<String>,
    pub cleanup_joined: bool,
}
pub(in crate::app::tests::runtime::write_wait_qualification) struct FaultArtifacts {
    root: PathBuf,
}
impl FaultArtifacts {
    pub fn new(root: impl AsRef<Path>) -> Result<Self, String> {
        let root = root.as_ref().to_owned();
        std::fs::create_dir(&root).map_err(|e| e.to_string())?;
        Ok(Self { root })
    }
    pub(super) fn engine_path(&self) -> PathBuf {
        self.root.join("overload400-engine")
    }
    pub fn write(&self, name: &str, report: &impl Serialize) -> Result<PathBuf, String> {
        if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err("fault artifact name must be a plain label".into());
        }
        let path = self.root.join(format!("{name}.json"));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| e.to_string())?;
        serde_json::to_writer_pretty(file, report).map_err(|e| e.to_string())?;
        Ok(path)
    }
}
pub(in crate::app::tests::runtime::write_wait_qualification) struct FaultRun {
    pub report: FaultReport,
    pub artifact: PathBuf,
    pub unfinished: Option<RetainedFault>,
}
/// Every timed-out owned caller/fixture is returned, never aborted or replaced.
pub(in crate::app::tests::runtime::write_wait_qualification) struct RetainedFault {
    pub fixture: Option<Arc<QualificationFixture>>,
    pub caller: Option<tokio::task::JoinHandle<ReplyProof>>,
    pub raw_state: Option<Arc<Mutex<AppState>>>,
    pub raw_rpc: Option<Arc<RpcHarness>>,
    pub private_roots: Vec<tempfile::TempDir>,
    pub(super) holders: Vec<super::hold::Hold>,
}
pub(super) fn us(value: Duration) -> u64 {
    value.as_micros().min(i64::MAX as u128) as u64
}
pub(super) fn at(value: Instant, anchor: Instant) -> i64 {
    if value >= anchor {
        us(value.duration_since(anchor)) as i64
    } else {
        -(us(anchor.duration_since(value)) as i64)
    }
}
pub(super) fn native(row: &WriteSnapshot, anchor: Instant) -> Value {
    json!({"write_id":format!("{:?}",row.id),"original_id":row.original_id,"method":row.method,"cohort":format!("{:?}",row.cohort),
        "intake_us":at(row.intake,anchor),"admitted_us":row.admitted_at.map(|v|at(v,anchor)),"acknowledged_us":row.acknowledged_at.map(|v|at(v,anchor)),"completed_us":row.completed_at.map(|v|at(v,anchor)),
        "root_finished":row.root_finished,"outstanding":row.outstanding.iter().map(|v|format!("{v:?}")).collect::<Vec<_>>(),
        "active":row.active.iter().map(|v|json!({"id":format!("{:?}",v.id),"category":format!("{:?}",v.category),"started_us":at(v.started,anchor)})).collect::<Vec<_>>(),
        "known_wait_us":us(row.known_wait_union),"blocked_or_sql_us":us(row.blocked_or_sql_union),"conservative_upper_us":us(row.conservative_upper),"max_interval_us":us(row.max_chargeable_interval),
        "interval_breached":row.interval_breached,"cumulative_breached":row.cumulative_breached,"cancelled":row.cancelled,"abandoned":row.abandoned})
}
pub(super) fn watch(report: &WatchReport, anchor: Instant) -> Value {
    json!({"failed":report.failed,"panicked":report.panicked,"polls":report.polls,"indexed":report.still_indexed,
        "breaches":report.breaches.iter().map(|v|json!({"original_id":v.original_id,"intended_us":at(v.intended_at,anchor),"observed_us":at(v.observed_at,anchor),"elapsed_us":us(v.observed_at.saturating_duration_since(v.intended_at))})).collect::<Vec<_>>()})
}

impl Drop for RetainedFault {
    fn drop(&mut self) {
        // An unfinished control never deletes roots still used by accepted
        // work. The supervisor must retain/report this case and end its own
        // isolated process, not replace the failed original.
        if let Some(fixture) = self.fixture.take() {
            std::mem::forget(fixture);
        }
        if let Some(state) = self.raw_state.take() {
            std::mem::forget(state);
        }
        if let Some(rpc) = self.raw_rpc.take() {
            std::mem::forget(rpc);
        }
        for root in self.private_roots.drain(..) {
            let _ = root.keep();
        }
        for holder in self.holders.drain(..) {
            std::mem::forget(holder);
        }
    }
}

/// Final cleanup (or explicitly unfinished boundary) export, never a poller.
pub(super) fn boundary_archive(
    registry: &WriteWaitRegistry,
    anchor: Instant,
    original: &str,
    errors: &mut Vec<String>,
) -> (Option<WriteSnapshot>, Value) {
    let export_started = Instant::now();
    let rows = registry.snapshot();
    let intervals = registry.completed_intervals();
    let matching = rows
        .iter()
        .filter(|row| row.original_id == original)
        .collect::<Vec<_>>();
    let original_row = if matching.len() == 1 {
        Some(matching[0].clone())
    } else {
        errors.push(format!(
            "actual original has {} native roots; expected exactly one",
            matching.len()
        ));
        None
    };
    let archive = json!({"export_started_us":at(export_started,anchor),"rows":rows.iter().map(|row|native(row,anchor)).collect::<Vec<_>>(),
        "completed_intervals":intervals.iter().map(|span|json!({"wait_id":format!("{:?}",span.id),"origins":span.origins.iter().map(|id|format!("{id:?}")).collect::<Vec<_>>(),"category":format!("{:?}",span.category),"started_us":at(span.started,anchor),"ended_us":at(span.ended,anchor)})).collect::<Vec<_>>()});
    (original_row, archive)
}
