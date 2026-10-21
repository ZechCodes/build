//! Native read cases already backed by saved data. Missing families remain
//! visible to the complete-registry control until their fixtures are added.
use super::SeededReadInputs;
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(in crate::app::tests::runtime) struct RpcCase {
    pub method: &'static str,
    pub params: Value,
}

pub(in crate::app::tests::runtime) fn cases(i: &SeededReadInputs) -> Vec<RpcCase> {
    let make = |method, params| RpcCase { method, params };
    vec![
        make("tasks.get", json!({"task_id":i.core.task})),
        make(
            "tasks.waiting_on.blocks",
            json!({"task_id":i.core.task,"limit":20}),
        ),
        make("tasks.review.get", json!({"task_id":i.core.task})),
        make(
            "tasks.review.diff",
            json!({"task_id":i.core.task,"snapshot_id":i.core.snapshot,
            "directory_id":i.core.directory,"mode":"blob","path":"README.md",
            "range":{"offset":0,"bytes":4096}}),
        ),
        make(
            "tasks.attachment",
            json!({"task_id":i.core.task,"path":i.core.task_attachment,"offset":0,"length":4096}),
        ),
        make(
            "thread.attachment",
            json!({"entity_id":i.core.owner,"agent_id":i.core.agent,
            "path":i.core.thread_attachment,"offset":0,"length":4096}),
        ),
        make("task.doc", json!({"task_id":i.ordinary_plan})),
        make(
            "task.stage_doc",
            json!({"task_id":i.staged_plan,"stage_id":i.stage}),
        ),
        make("capture.get", json!({"capture_id":i.capture})),
    ]
}

pub(super) fn registered_methods() -> BTreeSet<&'static str> {
    use crate::api::effects::{effect, Effect};
    crate::api::v1::methods()
        .iter()
        .map(|(method, _)| *method)
        .chain(crate::api::LEGACY_METHODS.iter().copied())
        .chain(crate::api::QA_METHODS.iter().copied())
        .filter(|method| {
            matches!(
                effect(method),
                Some(
                    Effect::SqlRead
                        | Effect::SnapshotRead
                        | Effect::ExternalRead
                        | Effect::LegacyRead
                )
            )
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app::tests::runtime) enum ReadOutcome {
    AdmissionReceipt,
    Pending,
    Unavailable,
    Refused,
    Ready,
    LegacyFull,
}

/// Keep incomplete and legacy replies visible without crediting them as
/// ready values. Native case-specific oracles still validate every complete
/// response before the caller records its outcome.
pub(super) fn classify(method: &str, reply: &Value) -> ReadOutcome {
    if reply.get("accepted").is_some() {
        return ReadOutcome::AdmissionReceipt;
    }
    if reply["ok"] != true {
        return ReadOutcome::Refused;
    }
    let value = &reply["result"];
    if value.is_null() || value["pending"] == true {
        return ReadOutcome::Pending;
    }
    if value["status"] == "unavailable" {
        return ReadOutcome::Unavailable;
    }
    if matches!(
        method,
        "task.doc" | "task.stage_doc" | "archived.list" | "task.get" | "term.list"
    ) {
        return ReadOutcome::LegacyFull;
    }
    ReadOutcome::Ready
}

pub(in crate::app::tests::runtime) fn assert_rpc_reply(
    case: &RpcCase,
    reply: &Value,
    inputs: &SeededReadInputs,
) -> ReadOutcome {
    let outcome = classify(case.method, reply);
    if !matches!(outcome, ReadOutcome::Ready | ReadOutcome::LegacyFull) {
        return outcome;
    }
    let value = &reply["result"];
    assert!(!value.is_null(), "{} returned a placeholder", case.method);
    assert_ne!(value["pending"], true, "{} returned pending", case.method);
    match case.method {
        "tasks.get" => {
            assert_eq!(value["task"]["id"], inputs.core.task);
            let comment = value["timeline"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == inputs.core.comment)
                .expect("stored comment");
            assert_eq!(comment["body"], super::seed::COMMENT);
            assert!(comment["attachments"]
                .as_array()
                .unwrap()
                .iter()
                .any(|file| file["path"] == inputs.core.task_attachment));
        }
        "tasks.waiting_on.blocks" => assert!(value["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["task_id"] == inputs.core.dependent)),
        "tasks.review.get" => assert!(value["review"]["snapshots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == inputs.core.snapshot)),
        "tasks.review.diff" => assert_bytes(value, &inputs.core.blob_bytes),
        "tasks.attachment" | "thread.attachment" => {
            assert_bytes(value, &inputs.core.attachment_bytes)
        }
        "task.doc" => assert_eq!(value["contents"], "# Ordinary qualification document\n"),
        "task.stage_doc" => assert_eq!(value["contents"], "# Staged qualification document\n"),
        "capture.get" => {
            assert_eq!(value["id"], inputs.capture);
            assert_eq!(value["text"], "Qualification immutable router read witness");
        }
        method => panic!("{method} has no saved-value oracle"),
    }
    outcome
}

fn assert_bytes(value: &Value, expected: &[u8]) {
    assert_eq!(
        crate::encoding::b64decode(value["content_b64"].as_str().unwrap()).unwrap(),
        expected
    );
}

#[test]
fn read_classifier_keeps_placeholders_and_legacy_out_of_ready_counts() {
    let cases = [
        (
            "tasks.get",
            json!({"accepted":true,"id":"receipt"}),
            ReadOutcome::AdmissionReceipt,
        ),
        (
            "tasks.get",
            json!({"ok":false,"error_code":"not_found"}),
            ReadOutcome::Refused,
        ),
        (
            "tasks.get",
            json!({"ok":true,"result":{"task":{"id":"saved"}}}),
            ReadOutcome::Ready,
        ),
        (
            "workspace.get",
            json!({"ok":true,"result":null}),
            ReadOutcome::Pending,
        ),
        (
            "workspace.get",
            json!({"ok":true,"result":{"pending":true}}),
            ReadOutcome::Pending,
        ),
        (
            "task.stage_diff",
            json!({"ok":true,"result":{"status":"unavailable","reason":"stage_not_complete"}}),
            ReadOutcome::Unavailable,
        ),
        (
            "task.doc",
            json!({"ok":true,"result":{"contents":"full legacy doc"}}),
            ReadOutcome::LegacyFull,
        ),
    ];
    let actual = cases
        .iter()
        .map(|(method, reply, _)| classify(method, reply))
        .collect::<Vec<_>>();
    let expected = cases
        .iter()
        .map(|(_, _, expected)| *expected)
        .collect::<Vec<_>>();
    assert_eq!(
        actual, expected,
        "only actual ready values enter ready completion cohorts"
    );
}
