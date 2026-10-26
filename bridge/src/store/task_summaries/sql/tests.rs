use super::*;

#[test]
fn installed_derivations_are_shared_instead_of_repeated_per_source_trigger() {
    let installed = format!("{TABLES}{}", triggers());
    assert_eq!(installed.matches("LAG(ts) OVER").count(), 1);
    assert_eq!(
        installed
            .matches("json_group_object(key,json(value))")
            .count(),
        1
    );
    assert!(installed.contains("CREATE TABLE IF NOT EXISTS tracker_task_summary_context"));
    let trigger_sql = triggers();
    for trigger in trigger_sql.split("CREATE TRIGGER").skip(1) {
        let name = trigger.split_whitespace().nth(3).unwrap();
        if name.starts_with("tracker_summary_apply_") {
            continue;
        }
        assert!(!trigger.contains("LAG(ts) OVER"));
        assert!(!trigger.contains("json_group_object(key,json(value))"));
    }
}

#[test]
fn append_program_compiles_its_keyed_derivation_once() {
    assert_eq!(
        activity::triggers()
            .matches("FROM tracker_summary_append")
            .count(),
        1
    );
}
