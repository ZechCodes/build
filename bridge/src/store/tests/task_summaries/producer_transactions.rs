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

#[test]
fn store_task_metadata_history_and_read_markers_match_canonical_records() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = Task::drafted("/summary", "before", Actor::User, support::NOW);
    draft.watched = true;
    draft.labels = vec!["Before".into()];
    let mut task = store.create_tracker_task(draft, &[]).unwrap();
    let note = said(&task, "tc-00000000000000000000000081", support::NOW);
    store
        .save_tracker_task_activity(&task, std::slice::from_ref(&note), &[])
        .unwrap();
    assert_matches_history(&store, &task.id);
    assert_eq!(summary(&store, "/summary")["unread_count"], 1);

    task.title = "committed metadata".into();
    task.labels = vec!["After".into()];
    task.read_through = Some(note.id);
    store.save_tracker_task_activity(&task, &[], &[]).unwrap();
    assert_matches_history(&store, &task.id);
    let canonical = store.load_tracker_task(&task.id).unwrap().unwrap();
    let card = summary(&store, "/summary");
    assert_eq!(card["title"], canonical.title);
    assert_eq!(card["labels"], serde_json::json!(canonical.labels));
    assert_eq!(card["unread_count"], 0);
    let choices = store
        .list_task_filter_choices("/summary", "label", None, 10)
        .unwrap();
    assert_eq!(choices.len(), 1);
    assert_eq!(choices[0].key, "after");
}
