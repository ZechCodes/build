//! OFF-SOURCE coordinator modules. Same external API as trial_complete.rs.
//! The immutable pre-split source is preserved under trial/archive/.
//! No build, lint, test, fixture or normal timing execution has been performed.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const SECONDS: u64 = 600;
const WRITES: usize = 24_000;
const EXEMPTS: usize = 60_000;
const READS: usize = 59_000;
const WRITE_CALLERS: usize = 12;
const ROUTER_CALLER: usize = 12;
const CALLERS: usize = 13;
const WRITE_CREDITS: usize = 2;
// Separate auxiliary credits; these do not expand semantic-write concurrency.
const SPA_READ_CREDITS: usize = 8;
const MCP_SOCKET_CREDITS: usize = 2;
const EXEMPT_CREDITS_PER_SPA: usize = 8;
const CLEANUP_BOUND: Duration = Duration::from_secs(30);
const WRITE_BOUND_US: u64 = 5_000_000;
const READ_BOUND_US: u64 = 100_000;
type Work<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

#[path = "trial/model.rs"]
mod model;
pub use model::{
    ControlExecution, ControlProofGap, Envelope, ExecutionProof, ReadCatalog, ReadCohort, Recipe,
    Route, Slot, Stage, Traffic, TrialName, WriteClass, ORIGINAL_TRIALS,
};

#[path = "trial/schedule.rs"]
mod schedule;
pub use schedule::{make_pipeline_control_plan, make_plan, make_semantic_slots};

#[path = "trial/evidence.rs"]
mod evidence;
pub use evidence::{
    verify_and_summarize_response, EffectProof, OracleAssertion, OracleWitness, ResponseEvidence,
    WitnessScalar,
};

#[path = "trial/traces.rs"]
mod traces;
pub use traces::{
    FrameTrace, Interval, OpenRegistryRow, OpenService, ProbeSnapshot, ProducerCounts, Row,
    ServiceTrace, UnionTrace, Violation,
};

#[path = "trial/backend.rs"]
mod backend;
pub use backend::{
    FixtureFactory, IndependentProbe, ProducerHandle, ProducerSet, Provenance, TrialBackend,
};

#[path = "trial/observation.rs"]
mod observation;
pub use observation::Observation;

#[path = "trial/proof.rs"]
mod proof;

#[path = "trial/watchdog.rs"]
mod watchdog;

#[path = "trial/report.rs"]
mod report;
pub use report::{
    ArtifactSink, Distribution, FailureSnapshot, MethodMetrics, TrafficCounts, TrialReport,
};

#[path = "trial/metrics.rs"]
mod metrics;

#[path = "trial/runner.rs"]
mod runner;
pub use runner::{
    run_pipeline_control, run_trial, PipelineControl, PipelineControlReport, PipelineControlRun,
    TrialRun, UnfinishedCleanup,
};

#[path = "trial/gates.rs"]
mod gates;

#[path = "trial/suite.rs"]
mod suite;
pub use suite::{run_all_six, SuiteReport, SuiteRun};

#[path = "trial/artifacts.rs"]
mod artifacts;
pub use artifacts::JsonArtifacts;

use evidence::validate_witness;
use gates::{sql_only_budget_review_required, validate_final_rows, validate_loaded_ready_reads};
use metrics::{counts, distribution, method_metrics};
use observation::Ledger;
use proof::validate_proof;
use watchdog::WatchThread;
