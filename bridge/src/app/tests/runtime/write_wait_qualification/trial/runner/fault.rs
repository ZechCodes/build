//! Proposed child of trial/runner.rs. Shares its private RunState engine.
//! Register ONLY after reviewed seam integration; not an existing tracked API.
use super::*;
pub async fn run_fault_plan(
    name: TrialName,
    backend: Arc<dyn TrialBackend>,
    sink: &dyn ArtifactSink,
    plan: Vec<Slot>,
    window: Duration,
) -> Result<PipelineControlRun, String> {
    if window != Duration::from_secs(1)
        || plan.len() != 400
        || plan
            .iter()
            .any(|slot| slot.traffic != Traffic::Write || slot.due_us >= 1_000_000)
    {
        return Err("only explicit400writes/one-second fault plan supported".into());
    }
    let mut expected = make_semantic_slots(name)
        .into_iter()
        .take(400)
        .collect::<Vec<_>>();
    for slot in &mut expected {
        slot.due_us /= 10;
    }
    expected.sort_by_key(|slot| (slot.due_us, slot.traffic, slot.sequence));
    if serde_json::to_value(&plan).map_err(|error| error.to_string())?
        != serde_json::to_value(&expected).map_err(|error| error.to_string())?
    {
        return Err(
            "overload plan changed immutable first400 slots beyond approved due compression".into(),
        );
    }
    let mut run = RunState::with_plan(name, backend, sink, plan, window)?;
    run.control = true;
    run.fault_overload = Some(FaultOverloadState::default());
    run.drive().await;
    run.finish_control()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FaultOverloadState {
    pub actual_refusals: BTreeMap<String, ControlRefusal>,
    pub verified_refusals: BTreeSet<String>,
    // Offered and unsent. No cancellation, synthetic final or success recorded.
    pub censored_dependents: BTreeMap<String, String>,
}
impl RunState<'_> {
    pub(super) fn record_fault_refusal(&mut self, refusal: ControlRefusal) {
        let Some(state) = self.fault_overload.as_mut() else {
            self.observe.completed(
                &refusal.original_id,
                Err("fault refusal reached non-overload runner".into()),
            );
            return;
        };
        if !self
            .recipes
            .get(&refusal.original_id)
            .is_some_and(|recipe| recipe.method == refusal.method)
        {
            self.observe.completed(
                &refusal.original_id,
                Err("actual refusal identity/method mismatch".into()),
            );
            return;
        }
        {
            let mut ledger = self.observe.ledger.lock().unwrap();
            let row = ledger
                .rows
                .get_mut(&refusal.original_id)
                .expect("immutable overload original");
            // Explicit terminal refusal candidate, neither proof gap nor
            // successful effect. Final native validation must discharge it.
            row.milestones_us
                .insert(Stage::Completed, self.observe.now_us());
        }
        if refusal.sent_to_decoded_final_ns > 100_000_000
            || refusal.intended_to_decoded_final_ns > 100_000_000
        {
            self.observe.fail(
                Some(refusal.original_id.clone()),
                "actual_busy_over100ms",
                "actual sent-to-decoded duration exceeded100ms".into(),
            );
        }
        if state
            .actual_refusals
            .insert(refusal.original_id.clone(), refusal)
            .is_some()
        {
            self.observe.fail(
                None,
                "duplicate_refusal",
                "duplicate original refusal candidate".into(),
            );
        }
    }
    pub(super) fn censor_fault_dependents(&mut self) {
        let Some(state) = self.fault_overload.as_mut() else {
            return;
        };
        loop {
            let mut changed = false;
            for queue in &mut self.pending {
                changed |= censor_queue(queue, &self.recipes, state);
            }
            if !changed {
                break;
            }
        }
    }
}
fn refusal_cause(state: &FaultOverloadState, before: &str) -> Option<String> {
    if state.actual_refusals.contains_key(before) {
        Some(before.to_owned())
    } else {
        state.censored_dependents.get(before).cloned()
    }
}

fn censor_queue(
    queue: &mut VecDeque<Slot>,
    recipes: &BTreeMap<String, Recipe>,
    state: &mut FaultOverloadState,
) -> bool {
    let mut changed = false;
    let mut kept = VecDeque::new();
    while let Some(slot) = queue.pop_front() {
        let cause = recipes[&slot.original_id]
            .predecessor
            .as_ref()
            .and_then(|before| refusal_cause(state, before));
        if let Some(cause) = cause {
            state.censored_dependents.insert(slot.original_id, cause);
            changed = true;
        } else {
            kept.push_back(slot);
        }
    }
    *queue = kept;
    changed
}

impl RunState<'_> {
    pub(super) fn accepted_fault_rows(&mut self, rows: &[Row]) -> Vec<Row> {
        let Some(state) = &mut self.fault_overload else {
            return rows.to_vec();
        };
        let mut accepted = Vec::new();
        for row in rows {
            if let Some(candidate) = state.actual_refusals.get(&row.slot.original_id) {
                let result = crate::app::tests::runtime::write_wait_qualification::backend::faults::validate_candidate(row, candidate)
                    .and_then(|()| self.backend.confirm_overload_refusal(&row.slot, &row.recipe.method));
                match result {
                    Ok(()) => {
                        state.verified_refusals.insert(row.slot.original_id.clone());
                    }
                    Err(error) => {
                        self.observe.fail(
                            Some(row.slot.original_id.clone()),
                            "invalid_frontdoor_refusal",
                            error,
                        );
                        accepted.push(row.clone());
                    }
                }
            } else {
                accepted.push(row.clone());
            }
        }
        accepted
    }
}
