//! Concrete OFFSOURCE assembly. Prepared resources are real; readiness is
//! fail-closed until named test-first prerequisites have reviewed receipts.
mod archive;
mod control;
mod exempt;
mod git_support;
mod native_archive;
mod probe;
mod producer_bodies;
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
    catalog: ReadCatalog,
    common: BTreeMap<String, CommonRequest>,
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
    pub fn blocked_reason(&self) -> String {
        let mut reasons = READINESS_PREREQUISITES.to_vec();
        if self.mode == PipelineMode::Qualification {
            reasons.push("#726 genuine ReadyReadInputs Router bootstrap and full45RPC+14MCP case metadata must pass; no substitution");
        }
        format!("concrete pipeline not Ready: {}", reasons.join("; "))
    }
    pub async fn prepare(
        name: TrialName,
        mode: PipelineMode,
        provenance: Provenance,
    ) -> Result<Self, PreparationFailure> {
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
        let mut prepared = Self {
            name,
            mode,
            provenance,
            plan: Vec::new(),
            full_plan: Vec::new(),
            catalog: ReadCatalog {
                rpc: Vec::new(),
                mcp: Vec::new(),
                router_only_mcp: Default::default(),
            },
            common: BTreeMap::new(),
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
        };
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
