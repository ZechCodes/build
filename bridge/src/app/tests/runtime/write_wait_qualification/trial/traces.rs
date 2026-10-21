//! Off-source split of the frozen coordinator; no execution evidence.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FrameTrace {
    // Harness-assigned monotonic ID for this collector observation. Product
    // CompletedFrame has NO frame identifier; never call this a product ID.
    pub observer_id: u64,
    pub original_ids: Vec<String>,
    pub protocol: String,
    pub method: String,
    pub sampled_at_us: u64,
    // Exact durations from the SAME CompletedFrame/Spent record. No inferred
    // per-acquisition begin/end timestamps or service/lock correlation.
    pub queued_us: u64,
    pub permit_wait_us: Option<u64>,
    pub pool_wait_us: Option<u64>,
    pub lock_wait_us: u64,
    pub held_us: u64,
    pub total_us: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Interval {
    pub begin_us: u64,
    pub end_us: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServiceTrace {
    pub original_id: String,
    pub registry_interval_id: String,
    pub registry_origins: Vec<String>,
    pub kind: String,
    pub target_us: u64,
    pub ceiling_us: u64,
    // Exact fields copied from the real FiniteServiceSample.
    pub actual_us: u64,
    pub succeeded: bool,
    pub abandoned: bool,
    // Supplied by the approved actual-operation-before-padding helper.
    pub operation_before_padding_us: Option<u64>,
    // Actual matching CompletedInterval(FiniteService), joined by original
    // through registry origins. Exactly one planned sample AND interval/root.
    pub finite_service: Interval,
    // No invented outer OwnExternal span: registry aggregate stays in waits.
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UnionTrace {
    pub write_ids: Vec<String>,
    pub sampled_at_us: u64,
    // Copied directly from registry WriteSnapshot aggregates. These are not
    // synthesized by summing frames, service samples or independent maxima.
    pub known_wait_union_us: u64,
    pub sql_service_union_us: u64,
    pub blocked_or_sql_union_us: u64,
    pub conservative_upper_us: u64,
    pub own_external_union_us: u64,
    pub exclusive_active_external_us: u64,
    pub max_chargeable_interval_us: u64,
    pub complete: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Row {
    pub slot: Slot,
    pub recipe: Recipe,
    pub milestones_us: BTreeMap<Stage, u64>,
    pub proof: Option<ExecutionProof>,
    pub error: Option<String>,
    pub frames: Vec<FrameTrace>,
    pub services: Vec<ServiceTrace>,
    pub waits: Option<UnionTrace>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Violation {
    pub original_id: Option<String>,
    pub kind: String,
    pub observed_at_us: u64,
    pub measured_us: Option<u64>,
    pub limit_us: Option<u64>,
    pub detail: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpenRegistryRow {
    pub original_id: String,
    pub write_id: String,
    pub known_wait_union_us: u64,
    pub wait_plus_sql_union_us: u64,
    pub conservative_upper_us: u64,
    pub active_finite_services: Vec<OpenService>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpenService {
    pub registry_interval_id: String,
    pub elapsed_us: u64,
    pub ceiling_us: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProbeSnapshot {
    pub registry: Vec<OpenRegistryRow>,
    pub producer_active: Vec<String>,
    pub deferred_active: Vec<String>,
    pub receiver_pending: Vec<String>,
    pub producer_failures: Vec<String>,
    pub producer_counts: BTreeMap<String, ProducerCounts>,
    // Supplied by the independent atomic/deadline caller watcher. No access
    // to this coordinator's mutable row ledger on the watchdog thread.
    pub caller_breaches: Vec<Violation>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProducerCounts {
    pub period_us: u64,
    pub planned: u64,
    pub counters_coherent: bool,
    pub joined: bool,
    pub body_released: bool,
    pub intended: u64,
    pub started: u64,
    pub completed: u64,
    pub failed: u64,
    pub pending_not_started: u64,
    pub missed_at_stop: u64,
    pub active_ordinal: Option<u64>,
    pub maximum_start_lag_us: u64,
    pub maximum_return_lag_us: u64,
    pub oldest_owed_ordinal: Option<u64>,
    pub oldest_owed_age_us: Option<u64>,
}
