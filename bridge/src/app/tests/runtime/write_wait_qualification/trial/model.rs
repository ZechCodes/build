//! Off-source split of the frozen coordinator; no execution evidence.
use super::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Envelope {
    Small,
    Large,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrialName {
    pub envelope: Envelope,
    pub number: u8,
}
impl TrialName {
    pub fn label(self) -> String {
        format!(
            "{}_trial_{}",
            match self.envelope {
                Envelope::Small => "small",
                Envelope::Large => "large",
            },
            self.number
        )
    }
}
pub const ORIGINAL_TRIALS: [TrialName; 6] = [
    TrialName {
        envelope: Envelope::Small,
        number: 1,
    },
    TrialName {
        envelope: Envelope::Small,
        number: 2,
    },
    TrialName {
        envelope: Envelope::Small,
        number: 3,
    },
    TrialName {
        envelope: Envelope::Large,
        number: 1,
    },
    TrialName {
        envelope: Envelope::Large,
        number: 2,
    },
    TrialName {
        envelope: Envelope::Large,
        number: 3,
    },
];

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Traffic {
    Write,
    Read,
    Exempt,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Route {
    SpaRpc,
    AgentMcp,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum WriteClass {
    State,
    FilesystemProcess,
    GitProvider,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Slot {
    pub original_id: String,
    pub sequence: usize,
    pub traffic: Traffic,
    pub route: Route,
    pub caller: usize,
    pub owner: usize,
    pub due_us: u64,
    pub class: Option<WriteClass>,
    pub finite_target_us: Option<u64>,
    pub finite_ceiling_us: Option<u64>,
    pub method: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ReadCatalog {
    // Generated from deployed effects registry, not a hand-written subset.
    pub rpc: Vec<String>,
    // The fourteen actual MCP read action names; recipes come from Global.
    pub mcp: Vec<String>,
    // ListProjects, ListWork, ReadConversation. Concrete labels match mcp_cases.
    pub router_only_mcp: BTreeSet<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recipe {
    pub method: String,
    // Actual read-case/scenario label; required and stable for the1000-sample
    // per-method/scenario loaded p99 gate. Writes/exempts may use their recipe.
    pub scenario: String,
    // Fixture-owned key for real params/action/effect verifier. Never echo-only.
    pub recipe_key: String,
    pub predecessor: Option<String>,
    pub needs_admission: bool,
    pub needs_durable_ack: bool,
    pub needs_registry_root: bool,
    pub expected_read_cohort: Option<ReadCohort>,
    // Distinct durable-mutation wire ID when planned, NEVER a replacement for
    // original RPC/client/registry identity. MCP may allocate one in its reply.
    pub durable_operation_id: Option<String>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Offered,
    DispatchStarted,
    WireSent,
    ServerIntake,
    Admitted,
    DurableAck,
    EncodedFinalReply,
    ParsedFinalReply,
    /// Actual validated parser timestamp, sent before transport/proof tails.
    DecodedReplyObserved,
    /// Coordinator observation time when caller/socket credit was returned.
    CallerCreditReleased,
    EffectVerified,
    Completed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReadCohort {
    ReadyBounded,
    ExplicitPending,
    LegacyUnbounded,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecutionProof {
    pub original_id: String,
    pub method: String,
    pub durable_operation_id: Option<String>,
    pub response_evidence: ResponseEvidence,
    pub effect: Option<EffectProof>,
    pub read_cohort: Option<ReadCohort>,
    // Populated from actual registry completion, not merely an accepted receipt.
    pub registry_root: Option<String>,
    pub mandatory_children_complete: bool,
}

/// Control-only outcome: a real decoded response can finish without inventing
/// a proof for an unreleased accepted-version/completion observer.
pub enum ControlExecution {
    Verified(ExecutionProof),
    ProofGap(ControlProofGap),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlProofGap {
    pub original_id: String,
    pub method: String,
    pub actual_effect_verified: bool,
    pub response_evidence: Option<ResponseEvidence>,
    pub detail: String,
}
