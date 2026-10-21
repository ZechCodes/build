//! Declared input rows for native Pump controls, never execution evidence.
use super::*;

impl Observation {
    pub(crate) fn native_control_rows(&self) -> Vec<Row> {
        self.ledger.lock().unwrap().rows.values().cloned().collect()
    }
    pub(crate) fn for_native_pump_control(plan: &[Slot]) -> Self {
        Self::for_native_control_at(plan, Instant::now())
    }
    pub(crate) fn for_native_control_at(plan: &[Slot], start: Instant) -> Self {
        let (wake, _receiver) = mpsc::unbounded_channel();
        let rows = plan
            .iter()
            .map(|slot| {
                let row = Row {
                    slot: slot.clone(),
                    recipe: Recipe {
                        method: slot.method.clone().expect("declared native control method"),
                        scenario: "native-registry-pump-control-only".into(),
                        recipe_key: slot.original_id.clone(),
                        predecessor: None,
                        needs_admission: false,
                        needs_durable_ack: false,
                        needs_registry_root: true,
                        expected_read_cohort: None,
                        durable_operation_id: None,
                    },
                    milestones_us: BTreeMap::new(),
                    proof: None,
                    error: None,
                    frames: Vec::new(),
                    services: Vec::new(),
                    waits: None,
                };
                (slot.original_id.clone(), row)
            })
            .collect();
        Self {
            start,
            ledger: Arc::new(Mutex::new(Ledger {
                rows,
                violations: Vec::new(),
                latched: BTreeSet::new(),
                other_frames: Vec::new(),
            })),
            wake,
            next_frame_observer: Arc::new(AtomicU64::new(1)),
        }
    }
}
