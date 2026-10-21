//! Off-source split of the frozen coordinator; no execution evidence.
use super::*;

#[cfg(test)]
#[path = "observation/pump_test_support.rs"]
mod pump_test_support;

pub(super) struct Ledger {
    pub(super) rows: BTreeMap<String, Row>,
    pub(super) violations: Vec<Violation>,
    pub(super) latched: BTreeSet<(Option<String>, String)>,
    pub(super) other_frames: Vec<FrameTrace>,
}
#[derive(Clone)]
pub struct Observation {
    pub(super) start: Instant,
    pub(super) ledger: Arc<Mutex<Ledger>>,
    pub(super) wake: mpsc::UnboundedSender<Violation>,
    pub(super) next_frame_observer: Arc<AtomicU64>,
}
impl Observation {
    /// Final off-lock archive clone; no inferred or synthetic lifecycle fields.
    pub fn archived_row(&self, original: &str) -> Result<Row, String> {
        self.ledger
            .lock()
            .map_err(|_| "observation archive poisoned")?
            .rows
            .get(original)
            .cloned()
            .ok_or_else(|| format!("missing final observation for {original}"))
    }

    pub fn at_us(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.start)
            .as_micros()
            .min(u64::MAX as u128) as u64
    }
    pub fn original_due(&self, slot: &Slot) -> Instant {
        self.start + Duration::from_micros(slot.due_us)
    }
    pub fn now_us(&self) -> u64 {
        self.at_us(Instant::now())
    }
    pub fn stage(&self, id: &str, stage: Stage, actual: Instant) -> Result<(), String> {
        let at = self.at_us(actual);
        let mut ledger = self.ledger.lock().unwrap();
        let row = ledger
            .rows
            .get_mut(id)
            .ok_or_else(|| format!("unknown original {id}"))?;
        if row.milestones_us.contains_key(&stage) {
            return Err(format!("duplicate {stage:?} for {id}"));
        }
        if at < row.slot.due_us {
            return Err(format!("{stage:?} precedes intended time for {id}"));
        }
        row.milestones_us.insert(stage, at);
        Ok(())
    }
    pub fn frame(&self, mut trace: FrameTrace) -> Result<u64, String> {
        trace.observer_id = self.next_frame_observer.fetch_add(1, Ordering::Relaxed);
        let id = trace.observer_id;
        let mut ledger = self.ledger.lock().unwrap();
        let known = trace
            .original_ids
            .iter()
            .filter(|id| ledger.rows.contains_key(*id))
            .cloned()
            .collect::<BTreeSet<_>>();
        for original in &known {
            ledger
                .rows
                .get_mut(original)
                .unwrap()
                .frames
                .push(trace.clone());
        }
        if known.is_empty() || known.len() != trace.original_ids.len() {
            ledger.other_frames.push(trace);
        }
        Ok(id)
    }
    pub fn service(&self, trace: ServiceTrace) -> Result<(), String> {
        let mut ledger = self.ledger.lock().unwrap();
        let row = ledger
            .rows
            .get_mut(&trace.original_id)
            .ok_or("service has unknown original")?;
        if row
            .services
            .iter()
            .any(|old| old.registry_interval_id == trace.registry_interval_id)
        {
            return Err("duplicate finite registry interval id".into());
        }
        row.services.push(trace);
        Ok(())
    }
    pub fn union(&self, id: &str, trace: UnionTrace) -> Result<(), String> {
        let mut ledger = self.ledger.lock().unwrap();
        let row = ledger
            .rows
            .get_mut(id)
            .ok_or("union has unknown original")?;
        if row.waits.is_some() {
            return Err("duplicate final union archive".into());
        }
        row.waits = Some(trace);
        Ok(())
    }
    pub(super) fn violation(&self, v: Violation) {
        let mut ledger = self.ledger.lock().unwrap();
        if ledger
            .latched
            .insert((v.original_id.clone(), v.kind.clone()))
        {
            ledger.violations.push(v.clone());
            let _ = self.wake.send(v);
        }
    }
    pub(super) fn fail(&self, id: Option<String>, kind: &str, detail: String) {
        self.violation(Violation {
            original_id: id,
            kind: kind.into(),
            observed_at_us: self.now_us(),
            measured_us: None,
            limit_us: None,
            detail,
        });
    }
    pub(super) fn completed(&self, id: &str, proof: Result<ExecutionProof, String>) {
        let now = self.now_us();
        let mut ledger = self.ledger.lock().unwrap();
        let row = ledger
            .rows
            .get_mut(id)
            .expect("completion has immutable original");
        let due = row.slot.due_us;
        let traffic = row.slot.traffic;
        let method = row.recipe.method.clone();
        let error = match proof {
            Ok(proof) => {
                let valid = validate_proof(row, &proof);
                row.proof = Some(proof);
                valid.err()
            }
            Err(error) => Some(error),
        };
        row.error = error.clone();
        row.milestones_us.insert(Stage::Completed, now);
        let client_end = row.milestones_us.get(&Stage::ParsedFinalReply).copied();
        drop(ledger);
        if let Some(error) = error {
            self.fail(Some(id.into()), "execution_or_proof", error);
        }
        // All active clients retain the5s whole-caller ceiling. A single slow
        // ready read is reported, but only its method/scenario p99 is gated at
        //100ms in the long loaded workload. Isolation max gates live elsewhere.
        let limit = WRITE_BOUND_US;
        // Whole-caller endpoint is measured independently from server waits.
        // Read p99 also gets a sent->parsed histogram; neither hides late offers.
        if traffic != Traffic::Exempt
            && client_end.is_some_and(|client_end| client_end.saturating_sub(due) > limit)
        {
            self.violation(Violation {
                original_id: Some(id.into()),
                kind: "intended_to_decoded_final".into(),
                observed_at_us: now,
                measured_us: client_end.map(|client_end| client_end - due),
                limit_us: Some(limit),
                detail: method,
            });
        }
    }
}
