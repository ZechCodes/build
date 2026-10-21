//! Off-source split of the frozen coordinator; no execution evidence.
use super::*;

/// Must read immutable/atomic test observations only: never app, writer,
/// collector, service or publication locks. It is called from a std thread.
pub trait IndependentProbe: Send + Sync {
    fn snapshot(&self) -> ProbeSnapshot;
}
/// Handle must own a real producer task, a stop-offering signal and completion
/// observation. stop_offering does not cancel accepted service/publication work.
pub trait ProducerHandle: Send + Sync {
    fn kind(&self) -> &'static str;
    fn start_at(&self, start: Instant) -> Result<(), String>;
    fn stop_offering(&self);
    fn joined(&self) -> bool;
}
pub struct ProducerSet(pub [Arc<dyn ProducerHandle>; 4]);
impl ProducerSet {
    pub(super) fn validate(&self) -> Result<(), String> {
        let actual = self.0.iter().map(|p| p.kind()).collect::<BTreeSet<_>>();
        let expected = [
            "status_observations",
            "delivery_consumption",
            "source_git_refresh",
            "watcher_publication",
        ]
        .into_iter()
        .collect::<BTreeSet<_>>();
        if actual != expected {
            return Err(format!(
                "four actual producer handles required, got {actual:?}"
            ));
        }
        Ok(())
    }
    pub(super) fn stop(&self) {
        for producer in &self.0 {
            producer.stop_offering();
        }
    }
    pub(super) fn joined(&self) -> bool {
        self.0.iter().all(|producer| producer.joined())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Provenance {
    pub source_revision: String,
    pub source_tree: String,
    pub binary_sha256: String,
    pub profile: String,
    pub nice: i64,
    pub cpu_affinity: Vec<usize>,
    pub fresh_quiet_release_sha256: String,
    pub fixture_counts: Value,
    pub fixture_seed: String,
    pub service_hook_contract_sha256: String,
}
/// Arc ownership retains temporary roots, WAL store, four RPC openings, eight
/// MCP identities, fake sessions and service hooks through all pending work.
pub trait TrialBackend: Send + Sync + 'static {
    /// Install once before dispatch. Only validated actual decoded replies
    /// notify; terminal transport errors remain execution/cleanup outcomes.
    fn observe_decoded_replies(
        &self,
        observer: Arc<dyn Fn(&str, Instant) + Send + Sync>,
    ) -> Result<(), String>;
    fn provenance(&self) -> Provenance;
    fn catalog(&self) -> ReadCatalog;
    fn recipe(&self, slot: &Slot) -> Result<Recipe, String>;
    fn producers(&self) -> ProducerSet;
    fn independent_probe(&self) -> Arc<dyn IndependentProbe>;
    fn register_caller_schedule(&self, start: Instant, slots: &[Slot]) -> Result<(), String>;
    fn caller_offered(&self, original_id: &str, at: Instant);
    /// Must observe actual stages at their real boundaries through Observation.
    /// Finite jobs run the real effect then pad only remaining target time.
    /// A successful result must include an actual effect/read/control proof.
    fn execute(
        self: Arc<Self>,
        slot: Slot,
        recipe: Recipe,
        observe: Observation,
    ) -> Work<'static, ExecutionProof>;
    fn execute_control(
        self: Arc<Self>,
        slot: Slot,
        recipe: Recipe,
        observe: Observation,
    ) -> Work<'static, ControlExecution> {
        Box::pin(async move {
            self.execute(slot, recipe, observe)
                .await
                .map(ControlExecution::Verified)
        })
    }
    /// Flush archived registry/frame/service rows for every root and child.
    /// Does not wait for/apply mutations and never drops unfinished obligations.
    fn archive_into(&self, observe: &Observation) -> Result<(), String>;
    /// Ask wire receivers/services to close only after all obligations drain.
    fn request_close(&self);
    fn closed(&self) -> bool;
    /// Separate fault-only final proof. Normal implementations cannot certify
    /// a refusal; all native/accepted/transport audits must have closed first.
    fn confirm_overload_refusal(&self, _slot: &Slot, _method: &str) -> Result<(), String> {
        Err("backend has no final overload refusal proof".into())
    }
}
pub trait FixtureFactory: Send + Sync {
    fn create(&self, name: TrialName) -> Work<'_, Arc<dyn TrialBackend>>;
}
