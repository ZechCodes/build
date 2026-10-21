//! Concrete OFFSOURCE assembly. Prepared resources are real; readiness is
//! fail-closed until named test-first prerequisites have reviewed receipts.
mod archive;
mod completed_origins;
mod control;
mod exempt;
mod final_transport;
mod git_support;
mod native_archive;
#[cfg(test)]
#[path = "native_read_tests.rs"]
mod native_read_tests;
#[cfg(test)]
#[path = "normal_tests.rs"]
mod normal_tests;
#[cfg(test)]
#[path = "preparation_tests.rs"]
mod preparation_tests;
mod probe;
mod producer_bodies;
#[path = "saved_effects/mod.rs"]
mod saved_effects;
mod saved_evidence;
mod watch_owner;
pub(super) use control::ControlBackend;
#[path = "setup.rs"]
mod setup;
#[path = "shutdown.rs"]
mod shutdown;
#[path = "transport.rs"]
mod transport;

use super::client_watch::ClientWatch;
use super::common_requests::{CommonRequest, CommonTargets};
use super::mcp_adapter::McpHarness;
use super::producer_adapter::ConcreteProducers;
use super::trial_complete::{Envelope, PipelineControl, Provenance, ReadCatalog, Slot, TrialName};
use crate::app::tests::runtime::write_wait_qualification::{
    fixture::QualificationFixture,
    read_recipes::{
        FullRpcCase, McpExpected, McpReadInputs, ReadyReadInputs, RpcCase, SeededReadInputs,
    },
};
use crate::test_support::finite_service::FixtureServices;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PipelineMode {
    Qualification,
    TwoSecondControl,
}
impl PipelineMode {
    pub fn plan(self, name: TrialName, catalog: &ReadCatalog) -> Result<Vec<Slot>, String> {
        match self {
            Self::Qualification => super::trial_complete::make_plan(name, catalog),
            Self::TwoSecondControl => PipelineControl::plan(name, catalog),
        }
    }
}

/// This concrete type is deliberately NOT TrialBackend while the lifecycle,
/// form and accepted-version prerequisites below have no reviewed integration.
/// It owns every real setup resource; a blocked setup never becomes Ready.
pub(super) struct PreparedBackend {
    name: TrialName,
    mode: PipelineMode,
    provenance: Provenance,
    plan: Vec<Slot>,
    full_plan: Vec<Slot>,
    // One final failed export only; not a second lifecycle ledger.
    transport_archive: Option<serde_json::Value>,
    native_archive: Option<serde_json::Value>,
    mcp_shutdown: Option<super::mcp_adapter::McpShutdownReport>,
    catalog: ReadCatalog,
    common: BTreeMap<String, CommonRequest>,
    saved_evidence: Option<Arc<saved_evidence::SavedEvidence>>,
    targets: Option<CommonTargets>,
    reads: Option<Arc<ReadyReadInputs>>,
    rpc_cases: Vec<FullRpcCase>,
    bounded_reads: Option<SeededReadInputs>,
    bounded_rpc_cases: Vec<RpcCase>,
    mcp_inputs: Vec<(McpReadInputs, McpExpected)>,
    fixture: Option<QualificationFixture>,
    terminal: Option<exempt::ExemptTerminal>,
    services: Option<FixtureServices>,
    frames: Arc<archive::FrameArchive>,
    producers: Option<Arc<ConcreteProducers>>,
    watch: Option<ClientWatch>,
    lifecycle: Option<Arc<watch_owner::WatchOwner>>,
    mcp: Option<Arc<McpHarness>>,
    probe: Option<Arc<probe::Probe>>,
}

pub(super) const READINESS_PREREQUISITES: &[&str] = &[
    "clean completed-bootstrap rpc.attach_client_watch implementation/control reviewed and GREEN",
    "quiet watch detachment and nonclosing actual registry timestamp export need reviewed APIs/control",
    "WriteWaitRegistry completed_snapshots_since/completed_intervals_since cursor readers need test-first release",
    "AcceptedRecords original-specific successful save/install observer must pass concurrent/noop/failed-save controls",
    "actual original-specific completion inbox/drain and native saved-effect verifier wiring still required",
];

impl PreparedBackend {
    fn bind_native_fixture_counts(&mut self) -> Result<(), String> {
        // TESTFIRST: native measurements exist, but normal binding is not wired.
        Err("native pre-T0 fixture binding is not implemented".into())
    }
    pub fn blocked_reason(&self) -> String {
        let mut reasons = READINESS_PREREQUISITES.to_vec();
        if self.mode == PipelineMode::Qualification {
            reasons.push("#726 genuine ReadyReadInputs Router bootstrap and full45RPC+14MCP case metadata must pass; no substitution");
        }
        format!("concrete pipeline not Ready: {}", reasons.join("; "))
    }
    async fn unassembled(name: TrialName, mode: PipelineMode, provenance: Provenance) -> Self {
        let envelope = match name.envelope {
            Envelope::Small => super::fixture::Envelope::Small,
            Envelope::Large => super::fixture::Envelope::Large,
        };
        // ready() owns native failed-bootstrap cleanup and preserves #726 panic.
        let (fixture, bounded_reads) = match mode {
            PipelineMode::Qualification => (QualificationFixture::ready(envelope).await, None),
            PipelineMode::TwoSecondControl => {
                let (fixture, seed) = QualificationFixture::pipeline_control().await;
                (fixture, Some(seed))
            }
        };
        Self {
            name,
            mode,
            provenance,
            plan: Vec::new(),
            full_plan: Vec::new(),
            transport_archive: None,
            native_archive: None,
            mcp_shutdown: None,
            catalog: ReadCatalog {
                rpc: Vec::new(),
                mcp: Vec::new(),
                router_only_mcp: Default::default(),
            },
            common: BTreeMap::new(),
            saved_evidence: None,
            targets: None,
            reads: None,
            rpc_cases: Vec::new(),
            bounded_reads,
            bounded_rpc_cases: Vec::new(),
            mcp_inputs: Vec::new(),
            fixture: Some(fixture),
            terminal: None,
            services: None,
            frames: archive::FrameArchive::new(),
            producers: None,
            watch: None,
            lifecycle: None,
            mcp: None,
            probe: None,
        }
    }
    pub async fn prepare(
        name: TrialName,
        mode: PipelineMode,
        provenance: Provenance,
    ) -> Result<Self, PreparationFailure> {
        let mut prepared = Self::unassembled(name, mode, provenance).await;
        use futures_util::FutureExt;
        let assembled = std::panic::AssertUnwindSafe(prepared.assemble())
            .catch_unwind()
            .await;
        match assembled {
            Ok(Ok(())) => Ok(prepared),
            Ok(Err(error)) => Err(PreparationFailure {
                reason: error,
                retained: prepared,
            }),
            Err(_) => Err(PreparationFailure {
                reason: "actual concrete setup panicked; real owned cleanup required".into(),
                retained: prepared,
            }),
        }
    }
}
pub(super) struct PreparationFailure {
    pub reason: String,
    pub retained: PreparedBackend,
}
pub(super) struct CleanupEvidence {
    pub producers_joined: bool,
    pub mcp_quiet: bool,
    pub client_watch_joined: bool,
    pub native_cleanup_completed: bool,
}
pub(super) struct RetainedSetup {
    pub reason: String,
    pub retained: PreparedBackend,
}
