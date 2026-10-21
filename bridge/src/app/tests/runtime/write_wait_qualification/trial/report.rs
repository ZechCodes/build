//! Off-source split of the frozen coordinator; no execution evidence.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FailureSnapshot {
    pub at_us: u64,
    pub trigger: Violation,
    pub pending_unsubmitted: Vec<String>,
    pub running_originals: Vec<String>,
    pub not_yet_offered: Vec<String>,
    pub probe: ProbeSnapshot,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TrafficCounts {
    pub intended: usize,
    pub offered: usize,
    pub dispatch_started: usize,
    pub sent: usize,
    pub server_intake: usize,
    pub admitted: usize,
    pub durable_acknowledged: usize,
    pub completed: usize,
    pub verified_success: usize,
    pub failed: usize,
    pub pending_unsubmitted: usize,
    pub sent_unfinished: usize,
    pub admitted_unfinished: usize,
    pub not_offered: usize,
    // Always divided by fixed600s. No post-drain denominator shrink.
    pub offered_per_second: f64,
    pub sent_per_second: f64,
    pub admitted_per_second: f64,
    pub completed_per_second: f64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Distribution {
    pub samples: usize,
    pub p50_us: Option<u64>,
    pub p95_us: Option<u64>,
    pub p99_us: Option<u64>,
    pub max_us: Option<u64>,
    pub violations: Vec<(String, u64)>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MethodMetrics {
    pub traffic: Traffic,
    pub method: String,
    pub scenario: String,
    pub read_cohort: Option<String>,
    pub counts: TrafficCounts,
    pub metrics: BTreeMap<String, Distribution>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrialReport {
    pub schema: String,
    pub trial: TrialName,
    pub provenance: Provenance,
    pub started_unix_us: u128,
    pub offer_window_us: u64,
    pub observed_end_us: u64,
    pub counts_at_600s: Option<BTreeMap<Traffic, TrafficCounts>>,
    pub final_counts: BTreeMap<Traffic, TrafficCounts>,
    pub endpoint_observations: BTreeMap<Stage, usize>,
    pub methods: Vec<MethodMetrics>,
    pub violations: Vec<Violation>,
    pub failures: Vec<FailureSnapshot>,
    pub last_probe: ProbeSnapshot,
    pub cleanup_drained: bool,
    pub backend_closed: bool,
    pub qualified: bool,
    pub invalid_infrastructure: Option<String>,
    pub coordinator_sql_review_required: bool,
    pub row_file: String,
    pub other_frame_count: usize,
    pub other_frames_file: String,
}

pub trait ArtifactSink: Send + Sync {
    /// Final-only unfiltered native roots/intervals; includes bootstrap and autonomous service roots.
    fn registry_archive(&self, name: TrialName, archive: &Value) -> Result<(), String>;
    fn progress(
        &self,
        name: TrialName,
        at_us: u64,
        counts: &BTreeMap<Traffic, TrafficCounts>,
    ) -> Result<(), String>;
    fn failure(&self, name: TrialName, snapshot: &FailureSnapshot) -> Result<(), String>;
    fn finish(
        &self,
        report: &TrialReport,
        rows: &[Row],
        other_frames: &[FrameTrace],
    ) -> Result<(), String>;
    /// Stream full decoded failures and at most one verified example/case to
    /// separate files. Does not retain the full response in ledger memory.
    fn raw_response(
        &self,
        trial: TrialName,
        original_id: &str,
        case: &str,
        failure: bool,
        decoded: &Value,
    ) -> Result<Option<String>, String>;
    fn suite(&self, report: &SuiteReport) -> Result<(), String>;
    fn pipeline_control(
        &self,
        report: &PipelineControlReport,
        rows: &[Row],
        other_frames: &[FrameTrace],
    ) -> Result<(), String>;
}
