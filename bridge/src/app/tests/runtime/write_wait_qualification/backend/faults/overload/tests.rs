//! Parser-boundary corruption controls; these values are not native evidence.
use super::*;
#[test]
fn refusal_missing_or_malformed_frame_evidence_is_not_zero_frames() {
    let original = "parser-negative/original";
    for rows in [
        vec![],
        vec![json!({"slot":{"original_id":original}})],
        vec![
            json!({"slot":{"original_id":original},"recipe":{"method":"tasks.update"},"frames":null}),
        ],
        vec![
            json!({"slot":{"original_id":original},"recipe":{"method":"tasks.update"},"frames":0}),
        ],
    ] {
        assert!(frame_count(&rows, original, "tasks.update").is_err());
    }
}
#[test]
fn refusal_duplicate_final_client_rows_are_rejected() {
    let row = json!({"slot":{"original_id":"parser-negative/duplicate"},"recipe":{"method":"tasks.update"},"frames":[]});
    assert!(frame_count(
        &[row.clone(), row],
        "parser-negative/duplicate",
        "tasks.update"
    )
    .is_err());
}
#[test]
fn refusal_explicit_empty_frame_archive_is_distinct_from_missing() {
    let row = json!({"slot":{"original_id":"parser-negative/empty"},"recipe":{"method":"tasks.update"},"frames":[]});
    assert_eq!(
        frame_count(&[row], "parser-negative/empty", "tasks.update").unwrap(),
        0
    );
}
#[test]
fn refusal_executed_frame_archive_is_never_hidden() {
    let row = json!({"slot":{"original_id":"parser-negative/executed"},"recipe":{"method":"tasks.update"},"frames":[{"observer_id":1}]});
    assert_eq!(
        frame_count(&[row], "parser-negative/executed", "tasks.update").unwrap(),
        1
    );
}

#[test]
fn refusal_final_client_missing_or_wrong_method_is_rejected() {
    let original = "parser-negative/method";
    for recipe in [json!(null), json!({}), json!({"method":"different.method"})] {
        let row = json!({"slot":{"original_id":original},"recipe":recipe,"frames":[]});
        assert!(frame_count(&[row], original, "tasks.update").is_err());
    }
}
