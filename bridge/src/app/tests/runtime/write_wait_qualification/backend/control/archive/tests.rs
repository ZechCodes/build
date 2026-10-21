//! Actual native registry/cursor controls. Declared slots and the final export
//! metadata shell are unit inputs, not fixture, transport or cleanup receipts.
use super::*;
use crate::timing::write_wait::WriteId;

const METHOD: &str = "thread.post";

#[tokio::test]
async fn native_pump_preserves_later_row_after_first_row_stage_and_import_errors() {
    let registry = WriteWaitRegistry::new();
    let first = slot("pump-first-invalid");
    let later = slot("pump-later-valid");
    // Missing first observation is an actual Observation::stage rejection.
    // Its native method mismatch separately exercises completion import Err.
    let observe = Observation::for_native_pump_control(std::slice::from_ref(&later));
    let completed = completed_origins::CompletedOrigins::new([
        (first.original_id.clone(), METHOD.into()),
        (later.original_id.clone(), METHOD.into()),
    ])
    .unwrap();
    let mut pump = Pump::new(
        &registry,
        Arc::clone(&completed),
        BTreeSet::from([first.original_id.clone(), later.original_id.clone()]),
    );
    let mut independent_cursor = registry.completed_snapshots_cursor();
    let first_id = finish_native(
        &registry,
        &first.original_id,
        "mcp.control:post_thread_message",
    );
    let later_id = finish_native(&registry, &later.original_id, METHOD);
    let actual = registry
        .completed_snapshots_since(&mut independent_cursor, MAX_ARCHIVE_BATCH)
        .unwrap();
    assert_eq!(
        actual.len(),
        2,
        "both real rows precede the single Pump tick"
    );
    assert_eq!(actual[0].id, first_id);
    assert_eq!(actual[1].id, later_id);

    let error = pump.tick(&registry, &observe).unwrap_err();
    assert!(
        error.contains("unknown original pump-first-invalid"),
        "{error}"
    );
    assert!(error.contains("native_method_mismatch"), "{error}");
    let proof = completed
        .wait(&later.original_id, tokio::time::Instant::now())
        .await
        .expect("later native row in the already-advanced batch must still import");
    assert_eq!(proof.write_id(), later_id);
    assert_eq!(proof.method(), METHOD);
    assert_eq!(Some(proof.completed_at()), actual[1].completed_at);
    assert!(observe
        .stage(&later.original_id, Stage::ServerIntake, Instant::now())
        .unwrap_err()
        .contains("duplicate ServerIntake"));
    let audit = completed.audit().join("; ");
    assert!(audit.contains("native_method_mismatch"), "{audit}");
    assert!(!audit.contains("missing_native_completion"), "{audit}");
    assert!(
        pump.tick(&registry, &observe)
            .unwrap_err()
            .contains("native_method_mismatch"),
        "an empty later batch cannot clear the first failure"
    );
}

#[tokio::test]
async fn native_pump_final_audit_rejects_late_duplicate_and_persists_both_actual_roots() {
    let registry = WriteWaitRegistry::new();
    let planned = slot("pump-late-duplicate");
    let observe = Observation::for_native_pump_control(std::slice::from_ref(&planned));
    let completed =
        completed_origins::CompletedOrigins::new([(planned.original_id.clone(), METHOD.into())])
            .unwrap();
    let mut pump = Pump::new(
        &registry,
        Arc::clone(&completed),
        BTreeSet::from([planned.original_id.clone()]),
    );
    let first_id = finish_native(&registry, &planned.original_id, METHOD);
    pump.tick(&registry, &observe).unwrap();
    let consumed = completed
        .wait(&planned.original_id, tokio::time::Instant::now())
        .await
        .unwrap();
    assert_eq!(consumed.write_id(), first_id);
    assert!(completed.audit().is_empty());

    // A distinct native root arrives only after the first proof was consumed.
    // No direct import: the real Pump.finish catch-up must find it.
    let late_id = finish_native(&registry, &planned.original_id, METHOD);
    assert_ne!(late_id, first_id);
    let root = tempfile::tempdir().unwrap();
    let sink = JsonArtifacts::new(root.path().join("artifacts")).unwrap();
    let mut prepared = final_export_metadata(vec![planned.clone()], root.path());
    let error = pump
        .finish(&mut prepared, &registry, &observe, &sink)
        .unwrap_err();
    assert!(error.contains("duplicate_native_completion"), "{error}");
    assert!(completed
        .audit()
        .join("; ")
        .contains("duplicate_native_completion"));
    assert!(completed
        .wait(&planned.original_id, tokio::time::Instant::now())
        .await
        .unwrap_err()
        .contains("duplicate_native_completion"));
    assert_eq!(
        consumed.write_id(),
        first_id,
        "a copied proof requires the final audit"
    );
    let raw: serde_json::Value = serde_json::from_reader(
        std::fs::File::open(
            root.path()
                .join("artifacts/small_trial_1/native-registry-final.json"),
        )
        .expect("failed final audit must still persist the native raw archive"),
    )
    .unwrap();
    assert_eq!(raw["qualification"], false);
    let rows = raw["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let actual_ids = rows
        .iter()
        .map(|row| row["write_id"].as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual_ids,
        BTreeSet::from([format!("{first_id:?}"), format!("{late_id:?}")])
    );
    for row in rows {
        assert_eq!(row["original_id"], planned.original_id);
        assert_eq!(row["method"], METHOD);
        assert_eq!(row["cohort"], "SemanticWrite");
        assert_eq!(row["root_finished"], true);
        assert!(row["completed_relative_us"].is_i64());
        assert_eq!(row["cancelled"], false);
        assert_eq!(row["abandoned"], false);
        assert!(row["outstanding"].as_array().unwrap().is_empty());
        assert!(row["active"].as_array().unwrap().is_empty());
    }
    assert!(
        prepared.native_archive.is_none(),
        "the actual sink persisted successfully"
    );
}

fn finish_native(registry: &Arc<WriteWaitRegistry>, original: &str, method: &str) -> WriteId {
    let root = registry.begin(original, method, registry.now());
    let id = root.id();
    root.admitted();
    root.acknowledged();
    root.complete();
    id
}

fn slot(original: &str) -> Slot {
    Slot {
        original_id: original.into(),
        sequence: 0,
        traffic: Traffic::Write,
        route: Route::SpaRpc,
        caller: 0,
        owner: 0,
        due_us: 0,
        class: None,
        finite_target_us: None,
        finite_ceiling_us: None,
        method: Some(METHOD.into()),
    }
}

fn final_export_metadata(plan: Vec<Slot>, root: &std::path::Path) -> PreparedBackend {
    // Only final-export metadata and real empty service registrations are used.
    // No app, transport, producer, watchdog, fixture or native join is claimed.
    PreparedBackend {
        name: TrialName {
            envelope: Envelope::Small,
            number: 1,
        },
        mode: PipelineMode::TwoSecondControl,
        provenance: Provenance {
            source_revision: String::new(),
            source_tree: String::new(),
            binary_sha256: String::new(),
            profile: "native-pump-unit-control-only".into(),
            nice: 0,
            cpu_affinity: Vec::new(),
            fresh_quiet_release_sha256: String::new(),
            fixture_counts: serde_json::Value::Null,
            fixture_seed: String::new(),
            service_hook_contract_sha256: String::new(),
        },
        plan,
        full_plan: Vec::new(),
        transport_archive: None,
        native_archive: None,
        optional_notices: None,
        mcp_shutdown: None,
        catalog: ReadCatalog {
            rpc: Vec::new(),
            mcp: Vec::new(),
            router_only_mcp: BTreeSet::new(),
        },
        common: BTreeMap::new(),
        saved_evidence: None,
        targets: None,
        reads: None,
        rpc_cases: Vec::new(),
        bounded_reads: None,
        bounded_rpc_cases: Vec::new(),
        mcp_inputs: Vec::new(),
        fixture: None,
        terminal: None,
        services: Some(FixtureServices::install(
            vec![root.to_path_buf()],
            Vec::new(),
            None,
        )),
        frames: super::super::super::archive::FrameArchive::new(),
        producers: None,
        watch: None,
        lifecycle: None,
        mcp: None,
        probe: None,
    }
}
