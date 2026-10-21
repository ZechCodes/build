//! Shared wire fixtures for journaled writes, before any semantic method opts in.
use build_bridge::api::writes::{
    AckReply, StatusParams, StatusResult, SubscribeParams, SubscribeResult, WriteConfirmation,
    WriteMetadata, ACK_CONFIRM_METHODS,
};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

fn fixture(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/api/v1/writes")
        .join(format!("{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn roundtrip<T: DeserializeOwned + Serialize>(value: Value) {
    let parsed: T = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), value);
}

#[test]
fn write_envelopes_roundtrip_without_reinterpreting_transport_receipts() {
    roundtrip::<AckReply<Value>>(fixture("ack"));
    roundtrip::<WriteConfirmation<Value>>(fixture("confirmed-success"));
    roundtrip::<WriteConfirmation<Value>>(fixture("confirmed-failure"));
    assert!(
        serde_json::from_value::<AckReply<Value>>(json!({"id":"r17","accepted":true})).is_err()
    );
    assert!(ACK_CONFIRM_METHODS.is_empty());
}

#[test]
fn status_and_subscribe_preserve_saved_acceptance_and_atomic_cursor() {
    roundtrip::<StatusParams>(fixture("status")["params"].clone());
    roundtrip::<StatusResult<Value>>(fixture("status")["result"].clone());
    roundtrip::<SubscribeParams>(fixture("subscribe")["params"].clone());
    roundtrip::<SubscribeResult<Value>>(fixture("subscribe")["result"].clone());
}

#[test]
fn metadata_distinguishes_inferred_dependencies_from_explicit_empty_set() {
    let id = fixture("request")["write"]["operation_id"].clone();
    let inferred: WriteMetadata = serde_json::from_value(json!({"operation_id":id})).unwrap();
    let explicit: WriteMetadata =
        serde_json::from_value(json!({"operation_id":id,"after":[]})).unwrap();
    assert_eq!(inferred.after, None);
    assert_eq!(explicit.after, Some(vec![]));
    assert!(serde_json::from_value::<WriteMetadata>(
        json!({"operation_id":id,"mode":"ack_confirm"})
    )
    .is_err());
    assert!(serde_json::from_value::<WriteMetadata>(json!({"operation_id":"not-a-uuid"})).is_err());
    assert!(serde_json::from_value::<WriteMetadata>(
        json!({"operation_id":id,"after":vec![id.clone();9]})
    )
    .is_err());
}

#[test]
fn status_and_subscription_sets_are_bounded_and_cursors_are_decimal() {
    let id = fixture("request")["write"]["operation_id"].clone();
    assert!(serde_json::from_value::<StatusParams>(json!({"operation_ids":[]})).is_err());
    assert!(
        serde_json::from_value::<StatusParams>(json!({"operation_ids":vec![id.clone();129]}))
            .is_err()
    );
    assert!(
        serde_json::from_value::<SubscribeParams>(json!({"operation_ids":vec![id;129]})).is_err()
    );
    let mut result = fixture("subscribe")["result"].clone();
    result["cursor"] = json!("42junk");
    assert!(serde_json::from_value::<SubscribeResult<Value>>(result).is_err());
}

#[test]
fn successful_confirmation_uses_the_complete_tasks_comment_result() {
    let confirmation = fixture("confirmed-success");
    let method_fixture: Value =
        serde_json::from_str(include_str!("../../fixtures/api/v1/tasks.comment.json")).unwrap();
    assert_eq!(confirmation["result"], method_fixture["result"]);
    let (_, handler) = build_bridge::api::v1::methods()
        .iter()
        .find(|(method, _)| *method == "tasks.comment")
        .unwrap();
    assert_eq!(
        handler.round_trip_result(&confirmation["result"]).unwrap(),
        confirmation["result"]
    );
}

#[test]
fn status_fixtures_cover_all_recovery_states_and_a_two_page_cursor() {
    let nested = fixture("status");
    let served: Value =
        serde_json::from_str(include_str!("../../fixtures/api/v1/writes.status.json")).unwrap();
    for contract in [&nested, &served] {
        roundtrip::<StatusParams>(contract["params"].clone());
        roundtrip::<StatusResult>(contract["result"].clone());
        let states: std::collections::BTreeSet<_> = contract["result"]["operations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|operation| operation["state"].as_str().unwrap())
            .collect();
        assert_eq!(
            states,
            [
                "accepted",
                "executing",
                "succeeded",
                "failed",
                "not_found",
                "result_expired"
            ]
            .into_iter()
            .collect(),
            "status fixtures cover every recovery state"
        );
        let pages = contract["examples"]
            .as_array()
            .expect("two pagination examples");
        assert_eq!(pages.len(), 2);
        for page in pages {
            roundtrip::<StatusParams>(page["params"].clone());
            roundtrip::<StatusResult>(page["result"].clone());
            assert_eq!(
                page["params"]["operation_ids"], contract["params"]["operation_ids"],
                "pagination keeps the complete ordered operation set"
            );
        }
        assert!(pages[0]["result"]["next_cursor"].is_string());
        assert_eq!(
            pages[1]["params"]["cursor"],
            pages[0]["result"]["next_cursor"]
        );
        assert!(pages[1]["result"].get("next_cursor").is_none());
        let combined: Vec<_> = pages
            .iter()
            .flat_map(|page| {
                page["result"]["operations"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .cloned()
            })
            .collect();
        assert_eq!(
            combined,
            *contract["result"]["operations"].as_array().unwrap(),
            "the continuation covers the ordered operation set without gaps"
        );
    }
    assert_eq!(nested["params"], served["params"]);
    assert_eq!(nested["result"], served["result"]);
    assert_eq!(nested["examples"], served["examples"]);
}
