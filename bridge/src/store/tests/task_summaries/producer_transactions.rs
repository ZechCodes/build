//! Supported Store writes maintain the projection in their canonical transaction.
use super::*;

#[test]
fn store_opens_without_installed_task_projection_triggers() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let conn = store.connection();
    let names = conn
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type='trigger' AND
             (name GLOB 'tracker_summary_*' OR name GLOB 'tracker_choices_*'
              OR sql GLOB '*tracker_task_summar*' OR sql GLOB '*tracker_task_filter_*')
             ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        names.is_empty(),
        "supported producers must call Rust maintenance, installed triggers: {names:?}"
    );
}
