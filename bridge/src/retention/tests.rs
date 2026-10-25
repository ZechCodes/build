use super::*;
use serde_json::json;

#[test]
fn config_defaults_to_age_window_with_fifty_message_floor_and_validates_explicit_values() {
    let defaults = RetentionConfig::from_config(&json!({})).unwrap();
    assert!(defaults.enabled);
    assert_eq!(defaults.min_messages, 50);
    assert_eq!(defaults.max_messages, 100);
    assert_eq!(defaults.window_days, 7);
    let enabled = RetentionConfig::from_config(&json!({"retention":{"enabled":true}})).unwrap();
    assert!(enabled.enabled);
    for invalid in [
        json!(null),
        json!(false),
        json!({"enabled":"yes"}),
        json!({"max_messages":0}),
        json!({"min_messages":0}),
        json!({"min_messages":101}),
        json!({"max_messages":49}),
        json!({"max_messages":-1}),
        json!({"window_days":0}),
        json!({"window_days":1.5}),
        json!({"window_days":u64::MAX}),
        json!({"enable":true}),
    ] {
        assert!(
            RetentionConfig::from_config(&json!({"retention":invalid})).is_err(),
            "{invalid}"
        );
    }
}
