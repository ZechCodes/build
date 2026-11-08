//! Dependency markers are work even after the query projection becomes ready.
use super::*;
use crate::tracker::{TaskEventKind, TimelineEntry};

fn blocked_pair(app: &mut AppState, project: &str, title: &str) -> (Value, Value) {
    let prerequisite = filed(app, project, &format!("{title} prerequisite"));
    let dependent = call(app, "tasks.create", json!({"project_id":project,"title":title,"waiting_on":[prerequisite["id"]]}))["task"].clone();
    for _ in 0..20 {
        if !app.pump_task_deliveries_for_test().unwrap() { break; }
    }
    (prerequisite, dependent)
}

fn finish_prerequisite(app: &mut AppState, prerequisite: &Value) {
    call(app, "tasks.update", json!({"task_id":prerequisite["id"],"status":"done"}));
}

fn unblocked_count(store: &crate::store::Store, task: &Value) -> usize {
    store.load_tracker_timeline(task["id"].as_str().unwrap()).unwrap().iter()
        .filter(|entry| matches!(entry, TimelineEntry::Event(event) if event.kind == TaskEventKind::Unblocked)).count()
}

fn marker_rows(store: &crate::store::Store, source: &Value) -> Vec<String> {
    let conn = store.writer_for_test();
    let mut query = conn.prepare("SELECT json_object('id',id,'source',source_task_id,'after',after_number,'self_done',self_done,'fanout',fanout,'causes',cause_event_ids,'at',at,'resolution_changed',resolution_changed) FROM tracker_dependency_reconcile WHERE source_task_id=?1 ORDER BY id").unwrap();
    query.query_map([source["id"].as_str().unwrap()], |row| row.get(0)).unwrap().collect::<Result<Vec<_>,_>>().unwrap()
}

#[test]
fn archive_refused_with_pending_dependency_reconciliation() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, project) = tracked(home.path());
    let (prerequisite, dependent) = blocked_pair(&mut app, &project, "Ready dependent");
    finish_prerequisite(&mut app, &prerequisite);
    let store = app.store.as_ref().unwrap();
    assert!(!store.waiting_on_summary(&app.project(&project).unwrap().project_key, dependent["id"].as_str().unwrap()).unwrap().blocked);
    assert!(!marker_rows(store, &prerequisite).is_empty());
    assert_eq!(unblocked_count(store, &dependent), 0);
    let refused = app.handle(req("project.archive", json!({"project_id":project})));
    assert_eq!(refused["error_code"], "conflict", "{refused}");
    assert!(refused["error"].as_str().unwrap().contains("pending dependency reconciliation"), "{refused}");
}

#[test]
fn archived_dependency_markers_wait_without_starving_and_restore_exactly_once() {
    let home = tempfile::tempdir().unwrap();
    let (_repo, mut app, archived) = tracked(home.path());
    let active = crate::app::projects::test_support::register_sibling(&mut app, &archived);
    let (source, retained) = blocked_pair(&mut app, &archived, "Retained dependent");
    let (active_source, live) = blocked_pair(&mut app, &active, "Active dependent");
    finish_prerequisite(&mut app, &source);
    let store = app.store.clone().unwrap();
    // More than a worker page of old markers precedes active work.
    for _ in 0..100 {
        store.writer_for_test().execute("INSERT INTO tracker_dependency_reconcile(source_task_id,at,cause_event_ids,resolution_changed,self_done,fanout,after_number) SELECT source_task_id,at,cause_event_ids,resolution_changed,self_done,fanout,after_number FROM tracker_dependency_reconcile WHERE source_task_id=?1 ORDER BY id LIMIT 1", [source["id"].as_str().unwrap()]).unwrap();
    }
    finish_prerequisite(&mut app, &active_source);
    // Simulate admission winning just before a marker becomes visible.
    app.projects.at_mut(0).archived_at = Some(1);
    let markers = marker_rows(&store, &source);
    assert!(markers.len() > 100);
    let before = serde_json::to_value(store.load_tracker_task(retained["id"].as_str().unwrap()).unwrap()).unwrap();
    let history = serde_json::to_value(store.load_tracker_timeline(retained["id"].as_str().unwrap()).unwrap()).unwrap();
    let state = app.shared();
    for _ in 0..3 { AppState::pump_waiting_on_changes_for_test(&state).unwrap(); }
    assert_eq!(serde_json::to_value(store.load_tracker_task(retained["id"].as_str().unwrap()).unwrap()).unwrap(), before);
    assert_eq!(serde_json::to_value(store.load_tracker_timeline(retained["id"].as_str().unwrap()).unwrap()).unwrap(), history);
    assert_eq!(marker_rows(&store, &source), markers);
    assert_eq!(unblocked_count(&store, &live), 1);
    call(&mut state.lock().unwrap(), "project.restore", json!({"project_id":archived}));
    for _ in 0..120 {
        if !AppState::pump_waiting_on_changes_for_test(&state).unwrap() { break; }
    }
    assert!(marker_rows(&store, &source).is_empty());
    assert_eq!(unblocked_count(&store, &retained), 1);
    assert!(!AppState::pump_waiting_on_changes_for_test(&state).unwrap());
    assert_eq!(unblocked_count(&store, &retained), 1);
}
