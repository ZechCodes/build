use super::*;
use serde_json::json;

#[test]
fn config_is_opt_in_and_validates_explicit_values() {
    let defaults = RetentionConfig::from_config(&json!({})).unwrap();
    assert!(!defaults.enabled);
    assert_eq!(defaults.max_messages, 100);
    assert_eq!(defaults.max_age_days, 7);
    let enabled = RetentionConfig::from_config(&json!({"retention":{"enabled":true}})).unwrap();
    assert!(enabled.enabled);
    for invalid in [
        json!(null),
        json!(false),
        json!({"enabled":"yes"}),
        json!({"max_messages":0}),
        json!({"max_messages":-1}),
        json!({"max_age_days":0}),
        json!({"max_age_days":1.5}),
        json!({"max_age_days":u64::MAX}),
        json!({"enable":true}),
    ] {
        assert!(
            RetentionConfig::from_config(&json!({"retention":invalid})).is_err(),
            "{invalid}"
        );
    }
}
