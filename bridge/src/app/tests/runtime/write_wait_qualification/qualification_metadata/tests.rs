use super::*;

mod build;
mod inputs;
mod source;

fn receipts() -> (Provenance, Value, Value) {
    let binary: Value = serde_json::from_str(include_str!("tests/fixtures/binary.json")).unwrap();
    let compiler: Value =
        serde_json::from_str(include_str!("tests/fixtures/release-profile-proof.json")).unwrap();
    let provenance = Provenance {
        source_revision: binary["source"]["head"].as_str().unwrap().into(),
        source_tree: binary["source"]["tree"].as_str().unwrap().into(),
        binary_sha256: binary["sha256"].as_str().unwrap().into(),
        profile: "release-opt3-debug-assertions-off".into(),
        nice: 10,
        cpu_affinity: vec![0, 1],
        fresh_quiet_release_sha256: "a".repeat(64),
        fixture_counts: json!({}),
        fixture_seed: "metadata-validation-only".into(),
        service_hook_contract_sha256: "b".repeat(64),
    };
    (provenance, binary, compiler)
}
