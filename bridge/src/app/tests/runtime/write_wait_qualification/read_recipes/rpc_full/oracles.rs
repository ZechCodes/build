//! Saved-value predicates. Inputs below are collected from actual
//! native Store records/files, not from the RPC response being verified.
use super::*;

pub(in crate::app::tests::runtime::write_wait_qualification) struct Witnesses {
    pub inputs: Inputs,
    pub all_projects: Vec<String>,
    pub task_title: String,
    pub comment: String,
    pub comment_body: String,
    pub operation: Value,
    pub thread_id: String,
    pub thread_generation: u64,
    pub message_body: String,
    pub activity_summary: String,
    pub revision_contents: String,
    pub capture_text: String,
    pub ordinary_doc: String,
    pub staged_doc: String,
    pub stage_start: String,
    pub stage_end: String,
    pub commit_subject: String,
    pub committed_file: String,
    pub diff_needle: String,
    pub branch_ref: String,
    pub upstream_ref: String,
    pub file_bytes: Vec<u8>,
    pub review_bytes: Vec<u8>,
    pub attachment_bytes: Vec<u8>,
    pub shell_id: String,
    pub model_id: String,
    pub installed_harness: String,
    pub missing_harness: String,
    pub default_harness: Value,
    pub expected_columns: Value,
    pub archived_state: String,
}

fn array<'a>(value: &'a Value, path: &str) -> &'a Vec<Value> {
    value
        .pointer(path)
        .unwrap_or_else(|| panic!("missing {path}: {value}"))
        .as_array()
        .unwrap_or_else(|| panic!("not array {path}: {value}"))
}
fn row<'a>(value: &'a Value, path: &str, key: &str, expected: &str) -> &'a Value {
    array(value, path)
        .iter()
        .find(|v| v[key] == expected)
        .unwrap_or_else(|| panic!("missing {path} {key}={expected}: {value}"))
}
fn text<'a>(value: &'a Value, path: &str) -> &'a str {
    value
        .pointer(path)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("missing text {path}: {value}"))
}
fn nonempty(value: &Value, path: &str) {
    assert!(!text(value, path).is_empty());
}
fn bytes(value: &Value, expected: &[u8]) {
    assert_eq!(
        crate::encoding::b64decode(text(value, "/content_b64")).unwrap(),
        expected
    );
}
fn file(value: &Value, path: &str) -> bool {
    array(value, "/files")
        .iter()
        .any(|entry| entry["path"] == path)
}
fn item_body(value: &Value, path: &str, body: &str) -> bool {
    array(value, path)
        .iter()
        .any(|item| item["type"] == "message" && item["data"]["body"] == body)
}

/// Every outcome is retained in its own class. Only Ready/LegacyFull reach
/// semantic checks. Caller must count >=1000 Ready per required ready scenario;
/// a fast Pending/Refused must not satisfy that count or be silently retried.
pub(in crate::app::tests::runtime::write_wait_qualification) fn assert_reply(
    case: &Case,
    reply: &Value,
    e: &Witnesses,
) -> Outcome {
    assert!(
        !case.oracle.is_empty(),
        "native case must name its saved witness"
    );
    let kind = classify(case, reply);
    if !matches!(kind, Outcome::Ready | Outcome::LegacyFull) {
        return kind;
    }
    let value = &reply["result"];
    if crate::api::v1::methods()
        .iter()
        .any(|(method, _)| *method == case.method)
    {
        crate::api::v1::check_read_result(case.method, value.clone())
            .unwrap_or_else(|error| panic!("{} typed result: {}", case.method, error.message()));
    } else {
        assert!(matches!(
            case.method,
            "stream.events" | "stream.state" | "term.list"
        ));
    }
    type Validator = fn(&str, &Value, &Witnesses) -> bool;
    let validators: [Validator; 8] = [
        sql::check,
        snapshots::check,
        services::check,
        artifacts::check,
        git::check,
        files::check,
        reviews::check,
        legacy::check,
    ];
    assert!(
        validators.iter().any(|check| check(case.method, value, e)),
        "{} has no native ready-value oracle",
        case.method
    );
    kind
}

mod artifacts;
mod files;
mod git;
mod legacy;
mod reviews;
mod services;
mod snapshots;
mod sql;
