//! Actual312-request mixed control. Proof gaps are typed RED after traffic;
//! normal qualification is unavailable through this concrete backend.
use super::super::rpc::ReplyProof;
use super::super::trial_complete::*;
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use tokio::sync::RwLock;
#[path = "control/close.rs"]
mod close;
#[path = "control/effects.rs"]
mod effects;
pub(super) use effects::prove_native_read;
#[path = "control/overload.rs"]
mod overload_policy;
#[path = "control/archive.rs"]
mod receipt_archive;

pub(in super::super) struct ControlBackend {
    resources: Arc<RwLock<Option<PreparedBackend>>>,
    provenance: Provenance,
    catalog: ReadCatalog,
    slots: BTreeMap<String, Slot>,
    recipes: BTreeMap<String, Recipe>,
    producers: Arc<ConcreteProducers>,
    probe: Arc<probe::Probe>,
    watch: ClientWatch,
    registry: Arc<crate::timing::write_wait::WriteWaitRegistry>,
    completed: Arc<completed_origins::CompletedOrigins>,
    saved_evidence: Arc<saved_evidence::SavedEvidence>,
    frames: Arc<archive::FrameArchive>,
    sink: Arc<dyn ArtifactSink>,
    archive: Arc<Mutex<receipt_archive::Pump>>,
    observation: Arc<Mutex<Option<Observation>>>,
    start: OnceLock<Instant>,
    overload: Option<Arc<overload_refusals::RefusalAudit>>,
    close_started: AtomicBool,
    cleanup_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    closed: Arc<AtomicBool>,
    close_error: Arc<Mutex<Vec<String>>>,
}
impl ControlBackend {
    pub fn new(
        mut prepared: PreparedBackend,
        sink: Arc<dyn ArtifactSink>,
    ) -> Result<Self, PreparationFailure> {
        if prepared.mode != PipelineMode::TwoSecondControl {
            return Err(PreparationFailure {
                reason: "control backend cannot accept normal qualification fixture".into(),
                retained: prepared,
            });
        }
        let completed = match completed_origins::CompletedOrigins::new(
            prepared
                .plan
                .iter()
                .filter(|slot| slot.traffic == Traffic::Write)
                .map(|slot| (slot.original_id.clone(), recipe_for(&prepared, slot).method)),
        ) {
            Ok(completed) => completed,
            Err(reason) => {
                return Err(PreparationFailure {
                    reason,
                    retained: prepared,
                })
            }
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let watch = prepared
                .watch
                .as_ref()
                .expect("actual preregistered watch")
                .clone();
            prepared
                .fixture
                .as_mut()
                .expect("owned fixture")
                .rpc
                .attach_client_watch(watch);
        }));
        if result.is_err() {
            return Err(PreparationFailure {
                reason: "actual clean-bootstrap attachment failed before control traffic".into(),
                retained: prepared,
            });
        }
        Ok(Self::from_attached(prepared, sink, completed))
    }
    fn from_attached(
        prepared: PreparedBackend,
        sink: Arc<dyn ArtifactSink>,
        completed: Arc<completed_origins::CompletedOrigins>,
    ) -> Self {
        let registry = Arc::clone(&prepared.fixture.as_ref().unwrap().registry);
        let mut provenance = prepared.provenance.clone();
        provenance.fixture_counts = serde_json::json!({
            "supplied":provenance.fixture_counts,
            "observed_control_setup":prepared.fixture.as_ref().unwrap().pipeline_control_setup,
        });
        let recipes = prepared
            .plan
            .iter()
            .map(|slot| (slot.original_id.clone(), recipe_for(&prepared, slot)))
            .collect();
        Self {
            saved_evidence: Arc::clone(
                prepared
                    .saved_evidence
                    .as_ref()
                    .expect("actual saved evidence setup"),
            ),
            provenance,
            catalog: prepared.catalog.clone(),
            slots: prepared
                .plan
                .iter()
                .map(|slot| (slot.original_id.clone(), slot.clone()))
                .collect(),
            recipes,
            producers: Arc::clone(prepared.producers.as_ref().unwrap()),
            probe: Arc::clone(prepared.probe.as_ref().unwrap()),
            watch: prepared.watch.as_ref().unwrap().clone(),
            frames: Arc::clone(&prepared.frames),
            archive: Arc::new(Mutex::new(receipt_archive::Pump::new(
                &registry,
                Arc::clone(&completed),
                prepared
                    .plan
                    .iter()
                    .filter(|slot| slot.traffic == Traffic::Write)
                    .map(|slot| slot.original_id.clone())
                    .collect(),
            ))),
            registry,
            completed,
            resources: Arc::new(RwLock::new(Some(prepared))),
            sink,
            observation: Arc::new(Mutex::new(None)),
            start: OnceLock::new(),
            overload: None,
            close_started: AtomicBool::new(false),
            cleanup_task: Mutex::new(None),
            closed: Arc::new(AtomicBool::new(false)),
            close_error: Arc::new(Mutex::new(Vec::new())),
        }
    }
    async fn execute_actual(
        &self,
        slot: Slot,
        recipe: Recipe,
        observe: Observation,
    ) -> Result<ControlExecution, String> {
        let resources = self.resources.read().await;
        let prepared = resources
            .as_ref()
            .ok_or("control resources already closed")?;
        match slot.traffic {
            Traffic::Write => self.write_actual(prepared, &slot, &recipe, &observe).await,
            Traffic::Read => self.read_actual(prepared, &slot, &recipe, &observe).await,
            Traffic::Exempt => self.exempt_actual(prepared, &slot, &recipe, &observe).await,
        }
    }
}
impl TrialBackend for ControlBackend {
    fn observe_decoded_replies(
        &self,
        observer: Arc<dyn Fn(&str, Instant) + Send + Sync>,
    ) -> Result<(), String> {
        self.watch.observe_decoded_replies(observer)
    }
    fn provenance(&self) -> Provenance {
        self.provenance.clone()
    }
    fn catalog(&self) -> ReadCatalog {
        self.catalog.clone()
    }
    fn recipe(&self, slot: &Slot) -> Result<Recipe, String> {
        let planned = self
            .slots
            .get(&slot.original_id)
            .ok_or("original outside312 immutable control")?;
        if serde_json::to_value(planned).map_err(|e| e.to_string())?
            != serde_json::to_value(slot).map_err(|e| e.to_string())?
        {
            return Err("control immutable slot changed".into());
        }
        self.recipes
            .get(&slot.original_id)
            .cloned()
            .ok_or("control recipe missing".into())
    }
    fn producers(&self) -> ProducerSet {
        self.producers.trial_handles()
    }
    fn independent_probe(&self) -> Arc<dyn IndependentProbe> {
        Arc::clone(&self.probe) as Arc<dyn IndependentProbe>
    }
    fn register_caller_schedule(&self, start: Instant, slots: &[Slot]) -> Result<(), String> {
        // Every full slot was checked by recipe() before T0 selection, and the
        // entire relative watch schedule was acknowledged by watch() at setup.
        if slots.len() != self.slots.len() {
            return Err("armed control plan count changed".into());
        }
        self.start
            .set(start)
            .map_err(|_| "control start already published")?;
        self.probe
            .start
            .set(start)
            .map_err(|_| "probe start already published")?;
        self.watch.start_at(start);
        Ok(())
    }
    fn caller_offered(&self, id: &str, _: Instant) {
        self.watch.offer(id);
    }
    fn execute(
        self: Arc<Self>,
        _: Slot,
        _: Recipe,
        _: Observation,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ExecutionProof, String>> + Send + 'static>,
    > {
        Box::pin(async { Err("312 control backend cannot execute normal qualification".into()) })
    }
    fn execute_control(
        self: Arc<Self>,
        slot: Slot,
        recipe: Recipe,
        observe: Observation,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ControlExecution, String>> + Send + 'static>,
    > {
        Box::pin(async move { self.execute_actual(slot, recipe, observe).await })
    }
    fn archive_into(&self, observe: &Observation) -> Result<(), String> {
        *self.observation.lock().unwrap() = Some(observe.clone());
        self.frames.drain_into(observe)?;
        self.archive.lock().unwrap().tick(&self.registry, observe)?;
        let errors = self.close_error.lock().unwrap();
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        Ok(())
    }
    fn confirm_overload_refusal(&self, slot: &Slot, method: &str) -> Result<(), String> {
        if !self.closed() || !self.close_error.lock().unwrap().is_empty() {
            return Err(
                "overload final native/accepted/transport audit is not cleanly closed".into(),
            );
        }
        self.overload
            .as_ref()
            .ok_or("backend has no final overload refusal proof")?
            .confirm(slot, method)
    }
    fn request_close(&self) {
        self.start_close();
    }
    fn closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
            && self
                .cleanup_task
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(tokio::task::JoinHandle::is_finished)
    }
}
fn recipe_for(prepared: &PreparedBackend, slot: &Slot) -> Recipe {
    if let Some(request) = prepared.common.get(&slot.original_id) {
        let method = if slot.route == Route::AgentMcp {
            format!("mcp.control:{}", request.method())
        } else {
            request.method().into()
        };
        return Recipe {
            method,
            scenario: "actual-common-write".into(),
            recipe_key: slot.original_id.clone(),
            predecessor: request.predecessor.clone(),
            needs_admission: false,
            needs_durable_ack: request.durable_operation_id.is_some(),
            needs_registry_root: true,
            expected_read_cohort: None,
            durable_operation_id: request.durable_operation_id.clone(),
        };
    }
    let method = slot
        .method
        .clone()
        .expect("actual declared read/control method");
    // The actual native core oracle's classify() explicitly marks these two
    // legacy forms; successful responses are checked against its real outcome.
    let cohort = (slot.traffic == Traffic::Read).then(|| {
        if matches!(method.as_str(), "task.doc" | "task.stage_doc") {
            ReadCohort::LegacyUnbounded
        } else {
            ReadCohort::ReadyBounded
        }
    });
    Recipe {
        method,
        scenario: "native-core8-control".into(),
        recipe_key: slot.original_id.clone(),
        predecessor: None,
        needs_admission: false,
        needs_durable_ack: false,
        needs_registry_root: false,
        expected_read_cohort: cohort,
        durable_operation_id: None,
    }
}

pub(super) fn expected_read_cohort(
    prepared: &PreparedBackend,
    slot: &Slot,
) -> Result<Option<ReadCohort>, String> {
    use super::super::read_recipes::{mcp_cases, mcp_read_method, mcp_recipe_form, FullReadForm};
    if slot.traffic != Traffic::Read {
        return Ok(None);
    }
    let method = slot.method.as_deref().ok_or("native read method missing")?;
    let form = match slot.route {
        Route::SpaRpc if prepared.mode == PipelineMode::TwoSecondControl => {
            let case = prepared
                .bounded_rpc_cases
                .iter()
                .find(|case| case.method == method)
                .ok_or("bounded native read recipe missing")?;
            match case.method {
                "task.doc" | "task.stage_doc" => FullReadForm::LegacyFull,
                "tasks.get"
                | "tasks.waiting_on.blocks"
                | "tasks.review.get"
                | "tasks.review.diff"
                | "tasks.attachment"
                | "thread.attachment" => FullReadForm::Ready,
                _ => return Err("bounded native read form not explicitly classified".into()),
            }
        }
        Route::SpaRpc => {
            prepared
                .rpc_cases
                .iter()
                .find(|case| case.method == method)
                .ok_or("full native RPC recipe missing")?
                .form
        }
        Route::AgentMcp => {
            let (inputs, _) = prepared
                .mcp_inputs
                .get(slot.owner)
                .ok_or("native MCP owner missing")?;
            let (_, action) = mcp_cases(inputs)
                .into_iter()
                .find(|(_, action)| mcp_read_method(action) == method)
                .ok_or("native MCP action missing")?;
            mcp_recipe_form(&action).ok_or("native MCP action has no reviewed read form")?
        }
    };
    Ok(Some(match form {
        FullReadForm::Ready => ReadCohort::ReadyBounded,
        FullReadForm::LegacyFull => ReadCohort::LegacyUnbounded,
    }))
}
