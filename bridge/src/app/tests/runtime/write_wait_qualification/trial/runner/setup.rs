//! Pre-register the immutable plan and start real producer handles.
use super::*;
struct SeededRows {
    rows: BTreeMap<String, Row>,
    recipes: BTreeMap<String, Recipe>,
}
impl<'a> RunState<'a> {
    pub(super) fn new(
        name: TrialName,
        backend: Arc<dyn TrialBackend>,
        sink: &'a dyn ArtifactSink,
    ) -> Result<Self, String> {
        let plan = make_plan(name, &backend.catalog())?;
        Self::with_plan(name, backend, sink, plan, Duration::from_secs(SECONDS))
    }
    pub(super) fn with_plan(
        name: TrialName,
        backend: Arc<dyn TrialBackend>,
        sink: &'a dyn ArtifactSink,
        plan: Vec<Slot>,
        offer_window: Duration,
    ) -> Result<Self, String> {
        let provenance = backend.provenance();
        validate_provenance(&provenance)?;
        let producers = backend.producers();
        producers.validate()?;
        let probe = backend.independent_probe();
        let SeededRows { rows, recipes } = seed_rows(&*backend, &plan)?;
        let start = Instant::now() + Duration::from_millis(100);
        let started_unix_us = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_micros()
            + 100_000;
        let (watch_tx, watch_rx) = mpsc::unbounded_channel();
        backend.register_caller_schedule(start, &plan)?;
        let observe = Observation {
            start,
            ledger: Arc::new(Mutex::new(Ledger {
                rows,
                violations: Vec::new(),
                latched: BTreeSet::new(),
                other_frames: Vec::new(),
            })),
            wake: watch_tx,
            next_frame_observer: Arc::new(AtomicU64::new(1)),
        };
        let (done_tx, done_rx) = mpsc::unbounded_channel();
        let decoded_events = done_tx.clone();
        let decoded_errors = observe.clone();
        backend.observe_decoded_replies(Arc::new(move |original_id, actual_at| {
            if decoded_events
                .send(ExecutionEvent::DecodedReply {
                    original_id: original_id.into(),
                    actual_at,
                })
                .is_err()
            {
                decoded_errors.fail(
                    Some(original_id.into()),
                    "decoded_event_channel",
                    "decoded reply outlived its owned coordinator receiver".into(),
                );
            }
        }))?;
        let watchdog = WatchThread::start(observe.clone(), Arc::clone(&probe));
        let startup_error = start_producers(&producers, start);
        if let Some(error) = &startup_error {
            observe.fail(None, "producer_startup", error.clone());
        }
        Ok(Self {
            name,
            backend,
            sink,
            provenance,
            plan,
            recipes,
            producers,
            probe,
            observe,
            watchdog: Some(watchdog),
            watch_rx,
            done_tx,
            done_rx,
            pending: std::array::from_fn(|_| VecDeque::new()),
            active: [0; 3 * CALLERS],
            sockets: [0; CALLERS],
            running: Running::new(),
            start,
            end: start + offer_window,
            offer_window,
            started_unix_us,
            next: 0,
            failed: startup_error.is_some(),
            infrastructure: startup_error,
            failures: Vec::new(),
            fault_overload: None,
            control: offer_window == Duration::from_secs(2),
            control_gaps: Vec::new(),
            at_600: None,
            drain_deadline: None,
            next_progress: start + Duration::from_secs(1),
        })
    }
}
fn validate_provenance(provenance: &Provenance) -> Result<(), String> {
    if provenance.nice != 10
        || provenance.source_tree.is_empty()
        || provenance.binary_sha256.is_empty()
        || provenance.fresh_quiet_release_sha256.is_empty()
    {
        return Err("NI10, frozen binary/tree and fresh root quiet release required".into());
    }
    Ok(())
}
fn seed_rows(backend: &dyn TrialBackend, plan: &[Slot]) -> Result<SeededRows, String> {
    let mut rows = BTreeMap::new();
    let mut recipes = BTreeMap::new();
    for slot in plan {
        let recipe = backend.recipe(slot)?;
        if !recipe_valid(&recipe, slot) {
            return Err(format!("invalid actual recipe for {}", slot.original_id));
        }
        recipes.insert(slot.original_id.clone(), recipe.clone());
        rows.insert(
            slot.original_id.clone(),
            Row {
                slot: slot.clone(),
                recipe,
                milestones_us: BTreeMap::new(),
                proof: None,
                error: None,
                frames: Vec::new(),
                services: Vec::new(),
                waits: None,
            },
        );
    }
    for (id, recipe) in &recipes {
        if recipe
            .predecessor
            .as_ref()
            .is_some_and(|before| before == id || !recipes.contains_key(before))
        {
            return Err(format!("invalid predecessor {id}"));
        }
    }
    Ok(SeededRows { rows, recipes })
}
fn recipe_valid(recipe: &Recipe, slot: &Slot) -> bool {
    !recipe.method.is_empty()
        && !recipe.scenario.is_empty()
        && !recipe.recipe_key.is_empty()
        && slot
            .method
            .as_ref()
            .is_none_or(|method| *method == recipe.method)
}
fn start_producers(producers: &ProducerSet, start: Instant) -> Option<String> {
    for producer in &producers.0 {
        if let Err(error) = producer.start_at(start) {
            producers.stop();
            return Some(format!("producer startup {}: {error}", producer.kind()));
        }
    }
    None
}
