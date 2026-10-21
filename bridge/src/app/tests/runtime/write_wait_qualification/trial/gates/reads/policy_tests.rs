//! Pure policy unit rows: injected durations and completion metadata. These are
//! NOT native replies, runtime measurements or qualification evidence. Native
//! typed recipes + encrypted saved-value coverage remain separate controls.
use super::*;

#[test]
fn legacy_mcp_list_tasks_keeps_100ms_p99_without_ready_credit() {
    let rows = policy_rows("list_tasks", Route::AgentMcp, 100_001);
    assert_eq!(rows.len(), 1_000);
    assert!(
        gather_ready_cases(&rows).is_empty(),
        "unpaged ListTasks must never earn bounded Ready credit"
    );
    let methods = method_metrics(&rows);
    assert_eq!(methods.len(), 1);
    assert_eq!(methods[0].read_cohort.as_deref(), Some("legacy_unbounded"));
    assert_eq!(
        methods[0].counts.completed, 1_000,
        "legacy samples stay in completed read denominator"
    );
    assert_eq!(methods[0].counts.verified_success, 1_000);
    assert_eq!(methods[0].metrics["due_to_parsed_reply"].samples, 1_000);
    assert_eq!(
        methods[0].metrics["due_to_parsed_reply"].p99_us,
        Some(100_001)
    );
    let totals = counts(rows.iter(), None);
    assert_eq!(totals[&Traffic::Read].completed, 1_000);
    // Legacy RPC docs remain explicitly legacy, never relabelled bounded just
    // to satisfy a ready quota. Their native case-specific oracle is elsewhere.
    let docs = policy_rows("task.doc", Route::SpaRpc, 100_001);
    assert!(gather_ready_cases(&docs).is_empty());
    assert_eq!(
        method_metrics(&docs)[0].read_cohort.as_deref(),
        Some("legacy_unbounded")
    );
    let observe = policy_observer();
    validate_loaded_ready_reads(&rows, &observe);
    let violations = observe.ledger.lock().unwrap().violations.clone();
    assert!(violations.iter().any(|v|
        v.kind.ends_with("read_p99/list_tasks/legacy-unpaged")
        && v.measured_us==Some(100_001) && v.limit_us==Some(100_000)),
        "all1000 legacy MCP samples must fail their unchanged100ms p99 gate without Ready credit: {violations:?}");
}

fn policy_observer() -> Observation {
    let (wake, _receive) = mpsc::unbounded_channel();
    Observation {
        start: Instant::now(),
        ledger: Arc::new(Mutex::new(Ledger {
            rows: BTreeMap::new(),
            violations: Vec::new(),
            latched: BTreeSet::new(),
            other_frames: Vec::new(),
        })),
        wake,
        next_frame_observer: Arc::new(AtomicU64::new(1)),
    }
}

fn policy_rows(method: &str, route: Route, elapsed: u64) -> Vec<Row> {
    (0..1_000)
        .map(|index| {
            let original_id = format!("policy-unit/{method}/{index}");
            let due = index as u64 * 600_000;
            let proof = policy_proof(&original_id, method);
            Row {
                slot: Slot {
                    original_id,
                    sequence: index,
                    traffic: Traffic::Read,
                    route,
                    caller: 4,
                    owner: 0,
                    due_us: due,
                    class: None,
                    finite_target_us: None,
                    finite_ceiling_us: None,
                    method: Some(method.into()),
                },
                recipe: Recipe {
                    method: method.into(),
                    scenario: "legacy-unpaged".into(),
                    recipe_key: "pure-policy-unit".into(),
                    predecessor: None,
                    needs_admission: false,
                    needs_durable_ack: false,
                    needs_registry_root: false,
                    expected_read_cohort: Some(ReadCohort::LegacyUnbounded),
                    durable_operation_id: None,
                },
                milestones_us: BTreeMap::from([
                    (Stage::Offered, due),
                    (Stage::WireSent, due),
                    (Stage::ParsedFinalReply, due + elapsed),
                    (Stage::Completed, due + elapsed),
                ]),
                proof: Some(proof),
                error: None,
                frames: Vec::new(),
                services: Vec::new(),
                waits: None,
            }
        })
        .collect()
}
fn policy_proof(original_id: &str, method: &str) -> ExecutionProof {
    ExecutionProof {
        original_id: original_id.into(),
        method: method.into(),
        durable_operation_id: None,
        response_evidence: ResponseEvidence {
            decoded_json_reserialized_bytes: 1,
            decoded_json_reserialized_sha256: "0".repeat(64),
            oracle_witness: OracleWitness::State {
                oracle: "policy-unit-only-not-native-coverage".into(),
                assertions: vec![OracleAssertion {
                    field: "cohort".into(),
                    actual: WitnessScalar::Text("legacy_unbounded".into()),
                    expected: WitnessScalar::Text("legacy_unbounded".into()),
                }],
            },
            representative_artifact: None,
        },
        effect: None,
        read_cohort: Some(ReadCohort::LegacyUnbounded),
        registry_root: None,
        mandatory_children_complete: true,
    }
}
