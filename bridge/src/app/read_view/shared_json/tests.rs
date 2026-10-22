use super::*;
use serde_json::json;

fn digest(thread: &str, generation: u64, sequence: u64) -> SharedJson {
    SharedJson::value(json!({
        "thread_id": thread,
        "thread_generation_revision": generation,
        "last_sequence": sequence,
    }))
}

#[test]
fn composition_retains_leaf_allocations_and_exact_empty_shapes() {
    let leaf = SharedJson::value(json!({"body": "retained body ".repeat(128)}));
    let tree = SharedJson::object([
        ("rows".into(), SharedJson::array([leaf.clone()])),
        ("empty".into(), SharedJson::array([])),
        ("object".into(), SharedJson::object([])),
    ]);
    assert!(tree.field("rows").unwrap().elements().unwrap()[0].ptr_eq(&leaf));
    assert_eq!(
        tree.materialize(),
        json!({"rows": [leaf.materialize()], "empty": [], "object": {}})
    );
    // Cross-representation equality must stay semantic without materializing.
    assert_eq!(tree, SharedJson::value(tree.materialize()));
}

#[test]
fn sparse_receipt_preserves_unrelated_leaves_and_old_root() {
    let matching = digest("selected", 7, 4);
    let unrelated = SharedJson::value(json!({"large": "unrelated ".repeat(512)}));
    let tree = SharedJson::array([matching.clone(), unrelated.clone()]);
    let before = tree.materialize();
    let mut patch = DigestPatch::new("selected", 7, 9);
    let updated = patch.apply(&tree).unwrap();
    assert!(updated.elements().unwrap()[1].ptr_eq(&unrelated));
    assert_eq!(tree.materialize(), before);
    assert_eq!(updated.materialize()[0]["last_sequence"], 9);
    assert_eq!(patch.copied_leaves(), 1);
    assert!(patch.apply(&updated).is_none());
}

#[test]
fn receipt_aliases_share_one_patched_leaf() {
    let row = digest("selected", 7, 4);
    let tree = SharedJson::array([row.clone(), row.clone()]);
    let mut patch = DigestPatch::new("selected", 7, 9);
    let updated = patch.apply(&tree).unwrap();
    let rows = updated.elements().unwrap();
    assert!(rows[0].ptr_eq(&rows[1]));
    assert!(patch.apply(&row).unwrap().ptr_eq(&rows[0]));
    assert_eq!(patch.copied_leaves(), 1);
}

#[test]
fn composed_object_receipts_patch_parent_and_nested_digests() {
    let tree = SharedJson::object([
        ("thread_id".into(), SharedJson::value(json!("selected"))),
        (
            "thread_generation_revision".into(),
            SharedJson::value(json!(7)),
        ),
        ("last_sequence".into(), SharedJson::value(json!(4))),
        ("nested".into(), digest("selected", 7, 5)),
        ("other_generation".into(), digest("selected", 6, 1)),
    ]);
    let mut patch = DigestPatch::new("selected", 7, 9);
    let updated = patch.apply(&tree).unwrap().materialize();
    assert_eq!(updated["last_sequence"], 9);
    assert_eq!(updated["nested"]["last_sequence"], 9);
    assert_eq!(updated["other_generation"]["last_sequence"], 1);
}

#[test]
fn receipt_preserves_malformed_absent_and_already_advanced_values() {
    let values = json!([
        {"thread_id":"selected","thread_generation_revision":7,"last_sequence":9},
        {"thread_id":"selected","thread_generation_revision":7,"last_sequence":null},
        {"thread_id":"selected","thread_generation_revision":"7","last_sequence":1},
        {"thread_id":"selected","last_sequence":1},
        {"thread_id":"other","thread_generation_revision":7,"last_sequence":1},
        {"thread_id":"selected","thread_generation_revision":7,"last_sequence":-1},
    ]);
    let tree = SharedJson::value(values.clone());
    let mut patch = DigestPatch::new("selected", 7, 9);
    assert!(patch.apply(&tree).is_none());
    assert_eq!(tree.materialize(), values);
    assert_eq!(patch.copied_leaves(), 0);
}

#[test]
fn a_lagging_alias_is_patched_even_when_another_alias_is_ahead() {
    let tree = SharedJson::array([digest("selected", 7, 20), digest("selected", 7, 4)]);
    let mut patch = DigestPatch::new("selected", 7, 9);
    assert_eq!(
        patch.apply(&tree).unwrap().materialize(),
        json!([
            {"thread_id":"selected","thread_generation_revision":7,"last_sequence":20},
            {"thread_id":"selected","thread_generation_revision":7,"last_sequence":9},
        ])
    );
}
