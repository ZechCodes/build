//! Shared producer-domain fixtures; Node independently checks supported instants.
#[path = "../src/store/task_summaries/query_timestamps.rs"]
mod query_timestamps;

#[test]
fn shared_timestamp_domain_fixture_matches_rust_parser() {
    use query_timestamps::{parse, QueryTimestamp};
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../fixtures/api/task-query-timestamps.json"
    ))
    .unwrap();
    assert_eq!(fixture["domain"], "ecmascript-iso-v1");
    for case in fixture["cases"].as_array().unwrap() {
        let expected = match case["outcome"].as_str().unwrap() {
            "known" => QueryTimestamp::Known(case["millis"].as_i64().unwrap()),
            "unknown" => QueryTimestamp::Unknown,
            "unsupported" => QueryTimestamp::Unsupported,
            other => panic!("unknown fixture classification: {other}"),
        };
        assert_eq!(parse(case["input"].as_str()), expected, "{case}");
    }
}
