//! OFF-SOURCE actual MCP socket adapter. No compile/run/qualification evidence.
//! One request per fresh socket. Reads and writes share two sockets per issued
//! Coding identity; exactly one separately reported Router is passive/read-only.
use super::client_watch::{ClientWatch, FinalOutcome, TrafficCohort};
use super::rpc::ReplyProof;
use crate::app::mcp::{
    authenticated_mcp_owner, handle_done_stream, is_independent_read, AddressedSession,
    ControlPlane,
};
use crate::app::AppState;
use crate::mcp::BridgeAction;
use crate::timing::write_wait::{WriteId, WriteSnapshot, WriteWaitRegistry};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, Semaphore};
use tokio::task::JoinHandle;

mod archive;
mod driver;
mod socket;

pub(super) const CODING_ENDPOINTS: usize = 8;
pub(super) const ROUTER_ENDPOINT: usize = 8;
const SOCKETS_PER_IDENTITY: usize = 2;
const INFRASTRUCTURE_BOUND: Duration = Duration::from_secs(30);

// Deliberately no Debug: issued tokens never belong in artifact evidence.
pub(super) struct McpIdentity {
    pub agent: String,
    pub token: String,
    pub read_only: bool,
}

struct Endpoint {
    identity: McpIdentity,
    sockets: Arc<Semaphore>,
    active: AtomicUsize,
    maximum_active: AtomicUsize,
}

#[derive(Clone, Debug)]
pub(super) struct SocketEvidence {
    pub endpoint: usize,
    pub agent: String,
    pub passive_router: bool,
    pub active: usize,
    pub maximum_active: usize,
    pub limit: usize,
}

#[derive(Clone, Debug)]
pub(super) struct McpCallRow {
    pub original_id: String,
    pub endpoint: usize,
    pub method: String,
    pub read: bool,
    pub queued_at: Instant,
    pub socket_wait_started_at: Option<Instant>,
    pub socket_acquired_at: Option<Instant>,
    pub sent_at: Option<Instant>,
    pub final_reply_at: Option<Instant>,
    pub response_ok: Option<bool>,
    pub response_bytes: Option<usize>,
    pub response_sha256: Option<String>,
    /// Current native replies have no id. Correlation is the dedicated socket
    /// with exactly one sent request; this field never fabricates an echoed id.
    pub actual_wire_id: Option<String>,
    pub client_closed_at: Option<Instant>,
    pub server_joined_at: Option<Instant>,
    pub worker_finished_at: Option<Instant>,
    pub worker_reaped_at: Option<Instant>,
    pub abandoned: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) struct UnimportedOrigin {
    pub row: WriteSnapshot,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub(super) struct McpRegistryArchive {
    /// Export completion only; never used to re-age capped scalar intervals.
    pub collected_at: Instant,
    pub rows: Vec<WriteSnapshot>,
    pub unimported: Vec<UnimportedOrigin>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct McpEvidence {
    pub calls: BTreeMap<String, McpCallRow>,
    pub sockets: Vec<SocketEvidence>,
    pub errors: Vec<String>,
    pub registry_archive: Option<McpRegistryArchive>,
}

#[derive(Clone)]
struct ImportedOrigin {
    id: WriteId,
    intake: Instant,
    admitted_at: Option<Instant>,
}

struct Jobs {
    accepting: bool,
    owned: BTreeMap<String, JoinHandle<()>>,
    imported: BTreeMap<String, ImportedOrigin>,
    evidence: McpEvidence,
}

struct Inner {
    state: Arc<Mutex<AppState>>,
    registry: Arc<WriteWaitRegistry>,
    plane: Arc<ControlPlane>,
    endpoints: Vec<Arc<Endpoint>>,
    watch: ClientWatch,
    jobs: Mutex<Jobs>,
    active: Arc<arc_swap::ArcSwap<imbl::OrdMap<String, ()>>>,
    failed: Arc<AtomicBool>,
}

impl Inner {
    fn update(&self, id: &str, change: impl FnOnce(&mut McpCallRow)) {
        let mut jobs = self.jobs.lock().unwrap();
        change(
            jobs.evidence
                .calls
                .get_mut(id)
                .expect("owned original exists"),
        );
    }
    fn error(&self, id: &str, error: String) {
        self.failed.store(true, Ordering::Release);
        let mut jobs = self.jobs.lock().unwrap();
        jobs.evidence.errors.push(format!("{id}: {error}"));
        if let Some(row) = jobs.evidence.calls.get_mut(id) {
            row.error = Some(error);
        }
    }
    fn identity_error(&self, id: &str, error: String) {
        self.error(id, error);
        self.watch.record_unmatched_reply(id);
    }

    fn finish_worker(&self, id: &str) {
        let mut jobs = self.jobs.lock().unwrap();
        jobs.evidence.calls.get_mut(id).unwrap().worker_finished_at = Some(Instant::now());
        let mut active = (**self.active.load()).clone();
        active.remove(id);
        self.active.store(Arc::new(active));
    }
}

/// IndependentProbe reads this immutable active transport index and atomics.
/// It never locks jobs, accesses credentials, or scans completed call evidence.
pub(super) struct McpObservation {
    active: Arc<arc_swap::ArcSwap<imbl::OrdMap<String, ()>>>,
    failed: Arc<AtomicBool>,
}

impl McpObservation {
    pub fn pending(&self) -> Arc<imbl::OrdMap<String, ()>> {
        self.active.load_full()
    }
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
}

pub(super) struct McpHarness {
    inner: Arc<Inner>,
}

#[derive(Clone, Debug)]
pub(super) struct McpShutdownReport {
    pub quiet: bool,
    pub unfinished: Vec<String>,
    pub evidence: McpEvidence,
}

impl McpShutdownReport {
    pub fn is_clean(&self) -> bool {
        self.quiet
            && self.unfinished.is_empty()
            && self.evidence.errors.is_empty()
            && self.evidence.calls.values().all(|row| {
                row.final_reply_at.is_some()
                    && row.server_joined_at.is_some()
                    && row.worker_reaped_at.is_some()
                    && !row.abandoned
                    && row.error.is_none()
                    && row.response_ok == Some(true)
            })
    }
}

impl McpHarness {
    pub fn new(
        state: Arc<Mutex<AppState>>,
        registry: Arc<WriteWaitRegistry>,
        identities: Vec<McpIdentity>,
        watch: ClientWatch,
    ) -> Result<Arc<Self>, String> {
        Self::with_identities(state, registry, identities, watch, CODING_ENDPOINTS + 1)
    }

    /// Explicit short pipeline control: exactly eight real Coding identities,
    /// with no Router/read cohort and no fabricated ninth endpoint. This is not
    /// a full qualification constructor; new() retains its nine-identity rule.
    pub fn pipeline_control(
        state: Arc<Mutex<AppState>>,
        registry: Arc<WriteWaitRegistry>,
        identities: Vec<McpIdentity>,
        watch: ClientWatch,
    ) -> Result<Arc<Self>, String> {
        Self::with_identities(state, registry, identities, watch, CODING_ENDPOINTS)
    }

    fn with_identities(
        state: Arc<Mutex<AppState>>,
        registry: Arc<WriteWaitRegistry>,
        identities: Vec<McpIdentity>,
        watch: ClientWatch,
        expected: usize,
    ) -> Result<Arc<Self>, String> {
        if identities.len() != expected {
            return Err(format!(
                "expected {expected} issued MCP identities; full qualification requires eight Coding plus one passive Router, pipeline_control requires exactly eight Coding"
            ));
        }
        let mut distinct = BTreeSet::new();
        let plane = {
            let app = state.lock().map_err(|_| "fixture app lock poisoned")?;
            for (index, identity) in identities.iter().enumerate() {
                if !distinct.insert(identity.agent.clone()) {
                    return Err("issued MCP identities must be distinct".into());
                }
                validate_identity(&app, index, identity)?;
            }
            app.frame_clock
                .set_write_wait_registry(Arc::clone(&registry));
            ControlPlane::new(&app)
        };
        let endpoints = identities
            .into_iter()
            .map(|identity| {
                Arc::new(Endpoint {
                    identity,
                    sockets: Arc::new(Semaphore::new(SOCKETS_PER_IDENTITY)),
                    active: AtomicUsize::new(0),
                    maximum_active: AtomicUsize::new(0),
                })
            })
            .collect();
        Ok(Arc::new(Self {
            inner: Arc::new(Inner {
                state,
                registry,
                plane,
                endpoints,
                watch,
                jobs: Mutex::new(Jobs {
                    accepting: true,
                    owned: BTreeMap::new(),
                    imported: BTreeMap::new(),
                    evidence: McpEvidence::default(),
                }),
                active: Arc::new(arc_swap::ArcSwap::from_pointee(imbl::OrdMap::new())),
                failed: Arc::new(AtomicBool::new(false)),
            }),
        }))
    }

    /// Global trial callers 4..11 are Coding; caller 12 is the passive Router.
    pub fn endpoint_for_caller(caller: usize) -> Result<usize, String> {
        caller
            .checked_sub(4)
            .filter(|endpoint| *endpoint <= ROUTER_ENDPOINT)
            .ok_or_else(|| "MCP caller must be in 4..=12".into())
    }
}

fn validate_identity(app: &AppState, index: usize, identity: &McpIdentity) -> Result<(), String> {
    let frame = json!({"task_id":identity.agent,"session_token":identity.token});
    if identity.agent.is_empty()
        || identity.token.is_empty()
        || authenticated_mcp_owner(&frame, &app.session_registry) != Some(identity.agent.as_str())
    {
        return Err(format!(
            "endpoint {index} lacks an actual issued session token"
        ));
    }
    let correct = match app.addressed_session(identity.agent.clone()) {
        Some(AddressedSession::Coding(_)) => index < CODING_ENDPOINTS && !identity.read_only,
        Some(AddressedSession::Router { .. }) => index == ROUTER_ENDPOINT && identity.read_only,
        None => false,
    };
    if !correct {
        return Err(format!(
            "endpoint {index} does not have the declared actual Coding/Router role"
        ));
    }
    Ok(())
}

fn validate_call(
    inner: &Inner,
    endpoint: &Endpoint,
    id: &str,
    frame: &Value,
    read: bool,
) -> Result<(), String> {
    let actual_read = is_independent_read(frame);
    if actual_read != read {
        return Err("declared MCP cohort differs from actual read-lane classifier".into());
    }
    if endpoint.identity.read_only && !actual_read {
        return Err("passive Router cannot consume a semantic write slot".into());
    }
    let schedule = inner
        .watch
        .schedule_lookup(id)
        .ok_or("MCP original must be preregistered")?;
    if schedule.intended_at.is_none() {
        return Err("trial T0 must be released before MCP execution".into());
    }
    let expected = if actual_read {
        TrafficCohort::Read
    } else {
        TrafficCohort::SemanticWrite
    };
    if schedule.cohort != expected {
        return Err("MCP schedule cohort differs from actual request".into());
    }
    Ok(())
}
