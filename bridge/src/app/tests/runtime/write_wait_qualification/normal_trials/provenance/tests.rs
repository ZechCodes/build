use super::*;

fn manifest() -> Value {
    let expected = json!([
        {"label":"small","projects":4,"tracker_tasks":100,"registry_workspaces":10,
        "active_workspaces":8,"canonical_histories":16,"thread_items":1000},
        {"label":"large","projects":4,"tracker_tasks":10000,"registry_workspaces":200,
        "active_workspaces":8,"canonical_histories":206,"thread_items":100000}
    ]);
    json!({
        "schema":"build.qualification.inputs.v1","run_id":"six-originals-control",
        "artifact_root":"/tmp/qualification-control-originals",
        "expected_envelopes":expected,
        "binary_receipt":{"path":"/tmp/binary.json","sha256":"c".repeat(64)},
        "compiler_receipt":{"path":"/tmp/compiler.json","sha256":"d".repeat(64)},
        "provenance":{
            "source_revision":"a".repeat(40),"source_tree":"b".repeat(40),
            "binary_sha256":"c".repeat(64),"profile":"release-opt3-debug-assertions-off",
            "nice":10,"cpu_affinity":[1,2],"fresh_quiet_release_sha256":"e".repeat(64),
            "fixture_counts":{"qualification":true,"expected_envelopes":expected},
            "fixture_seed":"pinned-control-seed","service_hook_contract_sha256":"f".repeat(64)
        }
    })
}

fn input() -> RunInputs {
    serde_json::from_value(manifest()).unwrap()
}

fn quiet(input: &RunInputs) -> Value {
    json!({
        "schema":"build.qualification.quiet-release.v1","run_id":input.run_id,
        "purpose":"six-original-qualification",
        "exact_selector":"app::tests::runtime::write_wait_qualification::normal_trials::qualify_small_and_large_three_original_ten_minute_trials",
        "source_revision":input.provenance.source_revision,"source_tree":input.provenance.source_tree,
        "binary_sha256":input.provenance.binary_sha256,"artifact_root":input.artifact_root,
        "whole_host_quiet_authorized":true,"no_gates_or_builds_during_suite":true,
        "coordinator_authorization":"explicit-test-only-receipt",
        "issued_at_unix_us":100,"start_before_unix_us":200
    })
}

#[test]
fn manifest_requires_exact_fixture_counts_release_profile_and_known_fields() {
    validate_fields(&input(), false).unwrap();
    assert!(validate_fields(&input(), true).is_err());
    for (pointer, changed) in [
        ("/schema", json!("build.qualification.fault-inputs.v1")),
        ("/run_id", json!("")),
        ("/artifact_root", json!("relative")),
        ("/provenance/source_revision", json!("not-a-sha")),
        ("/provenance/profile", json!("debug")),
        ("/provenance/fixture_seed", json!("")),
        ("/provenance/fixture_counts/qualification", json!(false)),
    ] {
        let mut changed_manifest = manifest();
        *changed_manifest.pointer_mut(pointer).unwrap() = changed;
        let changed: RunInputs = serde_json::from_value(changed_manifest).unwrap();
        assert!(validate_fields(&changed, false).is_err(), "{pointer}");
    }
    for envelope in 0..2 {
        for field in [
            "projects",
            "tracker_tasks",
            "registry_workspaces",
            "active_workspaces",
            "canonical_histories",
            "thread_items",
        ] {
            let mut changed_manifest = manifest();
            changed_manifest["expected_envelopes"][envelope][field] = json!(0);
            changed_manifest["provenance"]["fixture_counts"]["expected_envelopes"] =
                changed_manifest["expected_envelopes"].clone();
            let changed: RunInputs = serde_json::from_value(changed_manifest).unwrap();
            assert!(
                validate_fields(&changed, false).is_err(),
                "{envelope}/{field}"
            );
        }
    }
    let mut unknown = manifest();
    unknown["allow_replacement"] = json!(true);
    assert!(serde_json::from_value::<RunInputs>(unknown).is_err());
}

#[test]
fn actual_process_binding_rejects_binary_priority_and_affinity_claims() {
    let input = input();
    let p = &input.provenance;
    validate_runtime(p, &p.binary_sha256, 10, &[1, 2]).unwrap();
    assert!(validate_runtime(p, &"0".repeat(64), 10, &[1, 2]).is_err());
    assert!(validate_runtime(p, &p.binary_sha256, 0, &[1, 2]).is_err());
    assert!(validate_runtime(p, &p.binary_sha256, 10, &[1]).is_err());
    let mut wrong_claim = p.clone();
    wrong_claim.nice = 0;
    assert!(validate_runtime(&wrong_claim, &p.binary_sha256, 10, &[1, 2]).is_err());
}

#[test]
fn quiet_release_rejects_identity_authorization_and_clock_mismatches() {
    let input = input();
    let valid = quiet(&input);
    validate_quiet(&input, &valid, 100).unwrap();
    validate_quiet(&input, &valid, 200).unwrap();
    assert!(validate_quiet(&input, &valid, 99).is_err());
    assert!(validate_quiet(&input, &valid, 201).is_err());
    for (field, changed) in [
        ("schema", json!("build.qualification.fault-quiet.v1")),
        ("run_id", json!("another-run")),
        ("source_revision", json!("0".repeat(40))),
        ("source_tree", json!("0".repeat(40))),
        ("binary_sha256", json!("0".repeat(64))),
        ("artifact_root", json!("/tmp/other-originals")),
        ("whole_host_quiet_authorized", json!(false)),
        ("no_gates_or_builds_during_suite", json!(false)),
        ("coordinator_authorization", json!("")),
        ("issued_at_unix_us", Value::Null),
        ("start_before_unix_us", Value::Null),
        ("start_before_unix_us", json!(50)),
    ] {
        let mut changed_quiet = valid.clone();
        changed_quiet[field] = changed;
        assert!(
            validate_quiet(&input, &changed_quiet, 150).is_err(),
            "{field}"
        );
    }
}

#[test]
fn quiet_release_cannot_authorize_another_purpose_or_selector() {
    let input = input();
    let valid = quiet(&input);
    validate_quiet(&input, &valid, 150).unwrap();
    for (field, changed) in [
        ("purpose", json!("physical-fault-control")),
        ("exact_selector", json!("two_second_pipeline_control")),
        ("purpose", Value::Null),
        ("exact_selector", Value::Null),
    ] {
        let mut changed_quiet = valid.clone();
        changed_quiet[field] = changed;
        assert!(
            validate_quiet(&input, &changed_quiet, 150).is_err(),
            "{field}"
        );
    }
}
