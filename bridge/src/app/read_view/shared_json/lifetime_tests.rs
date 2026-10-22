use super::{DigestPatch, SharedJson};
use serde_json::json;
use std::sync::Arc;

#[test]
fn patch_memo_retains_source_identity_until_its_scope_ends() {
    let value = Arc::new(json!({
        "thread_id":"selected", "thread_generation_revision":7, "last_sequence":4,
    }));
    let identity = Arc::downgrade(&value);
    let source = SharedJson::from_shared(value);
    let mut patch = DigestPatch::new("selected", 7, 9);
    let updated = patch.apply(&source).unwrap();
    drop(source);
    let retained = identity
        .upgrade()
        .expect("a memo key must retain its source allocation");
    let alias = SharedJson::from_shared(retained);
    assert!(patch.apply(&alias).unwrap().ptr_eq(&updated));
    drop(alias);
    drop(patch);
    assert!(identity.upgrade().is_none());
    assert_eq!(updated.materialize()["last_sequence"], 9);
}
