//! Generated transport placement must cover the real shared typed wire registry.
use build_bridge::api::{v1, LEGACY_METHODS, QA_METHODS};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}
fn json(path: impl AsRef<Path>) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn every_registered_method_is_explicitly_mapped_or_deliberately_excluded() {
    let manifest = json(root().join("fixtures/api/http/resource-manifest.json"));
    let methods: BTreeSet<_> = v1::methods()
        .iter()
        .map(|(name, _)| *name)
        .chain(LEGACY_METHODS.iter().copied())
        .chain(QA_METHODS.iter().copied())
        .collect();
    let exported: BTreeSet<_> = manifest["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|op| op["method"].as_str().unwrap())
        .collect();
    let excluded: BTreeSet<_> = manifest["exclusions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|op| op["method"].as_str().unwrap())
        .collect();
    assert!(exported.is_disjoint(&excluded));
    assert_eq!(
        exported.union(&excluded).copied().collect::<BTreeSet<_>>(),
        methods
    );
    for name in [
        "agent.start",
        "agent.interrupt",
        "session.hello",
        "term.list",
        "ping",
    ] {
        assert!(exported.contains(name), "nonterminal consumer {name}");
    }
    for name in ["agent.attach", "term.input", "rtc.offer", "stream.start"] {
        assert!(excluded.contains(name));
    }
}

#[test]
fn generated_examples_are_accepted_by_the_existing_typed_registry() {
    let manifest = json(root().join("fixtures/api/http/resource-manifest.json"));
    for (method, handler) in v1::methods() {
        let op = manifest["operations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|op| op["method"] == *method)
            .unwrap();
        assert!(op["contract"]["source"]
            .as_str()
            .unwrap()
            .starts_with("bridge/src/api/v1/"));
        let fixture = json(root().join(format!("fixtures/api/v1/{method}.json")));
        handler
            .parse_params(&fixture["params"])
            .unwrap_or_else(|e| panic!("{method}: {e}"));
        assert_eq!(
            handler.round_trip_result(&fixture["result"]).unwrap(),
            fixture["result"],
            "{method}"
        );
        let mut invalid = fixture["params"].clone();
        invalid["unrecognized_http_param"] = serde_json::json!(true);
        assert!(
            handler.parse_params(&invalid).is_err(),
            "{method} silently accepted unknown field"
        );
    }
}
