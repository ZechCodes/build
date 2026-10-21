//! Explicit policy literals are negative inputs, never execution receipts.
use super::*;
fn input(root: PathBuf) -> Inputs {
    Inputs {
        schema: "build.qualification.fault-inputs.v1".into(),
        run_id: "policy-negative-control".into(),
        purpose: "physical-pre-ack-app".into(),
        exact_selector: "exact-policy-control".into(),
        artifact_root: root,
        provenance: Provenance {
            source_revision: "a".repeat(40),
            source_tree: "b".repeat(40),
            binary_sha256: "c".repeat(64),
            profile: "release-opt3-debug-assertions-off".into(),
            nice: 10,
            cpu_affinity: vec![0],
            fresh_quiet_release_sha256: "d".repeat(64),
            fixture_counts: json!({"qualification":false}),
            fixture_seed: "explicit-policy-input".into(),
            service_hook_contract_sha256: "e".repeat(64),
        },
        binary_receipt: HashedInput {
            path: "/explicit/unread/binary.json".into(),
            sha256: "f".repeat(64),
        },
        compiler_receipt: HashedInput {
            path: "/explicit/unread/compiler.json".into(),
            sha256: "a".repeat(64),
        },
    }
}
fn receipt(input: &Inputs) -> Value {
    json!({"schema":"build.qualification.fault-quiet.v1","run_id":input.run_id,"purpose":input.purpose,
        "exact_selector":input.exact_selector,"artifact_root":input.artifact_root,
        "source_revision":input.provenance.source_revision,"source_tree":input.provenance.source_tree,"binary_sha256":input.provenance.binary_sha256,
        "whole_host_quiet_authorized":true,"no_gates_or_builds_during_control":true,"coordinator_authorization":"explicit-negative-policy-test-only",
        "issued_at_unix_us":90,"start_before_unix_us":110,"launcher":{"pid":123,"start_ticks":456}})
}
fn actual() -> process::Actual {
    process::Actual {
        lineage: vec![process::Identity {
            pid: 123,
            start_ticks: 456,
        }],
        nice: 10,
        affinity: vec![0],
    }
}
#[test]
fn fault_provenance_rejects_missing_or_different_purpose_quiet_receipt() {
    let input = input("/explicit/unread/new-root".into());
    assert!(quiet_binding(&input, &Value::Null, &actual(), 100).is_err());
    let mut wrong = receipt(&input);
    wrong["purpose"] = json!("six-original-normal-trials");
    assert!(quiet_binding(&input, &wrong, &actual(), 100).is_err());
}
#[test]
fn fault_provenance_rejects_reused_pid_with_wrong_start_ticks() {
    let input = input("/explicit/unread/new-root".into());
    let mut wrong = receipt(&input);
    wrong["launcher"]["start_ticks"] = json!(455);
    assert!(quiet_binding(&input, &wrong, &actual(), 100).is_err());
}
#[test]
fn fault_provenance_rejects_expired_future_or_wrong_binary_authorization() {
    let input = input("/explicit/unread/new-root".into());
    let valid = receipt(&input);
    assert!(quiet_binding(&input, &valid, &actual(), 89).is_err());
    assert!(quiet_binding(&input, &valid, &actual(), 111).is_err());
    let mut wrong = valid;
    wrong["binary_sha256"] = json!("other-binary");
    assert!(quiet_binding(&input, &wrong, &actual(), 100).is_err());
}
#[test]
fn fault_provenance_rejects_replacement_root_and_normal_manifest() {
    let root = tempfile::tempdir().unwrap();
    let mut input = input(root.path().to_owned());
    assert!(fields(&input, "physical-pre-ack-app", "exact-policy-control").is_err());
    input.artifact_root = root.path().join("unused");
    input.schema = "build.qualification.inputs.v1".into();
    assert!(fields(&input, "physical-pre-ack-app", "exact-policy-control").is_err());
}
