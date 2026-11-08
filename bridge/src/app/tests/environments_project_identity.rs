use super::*;
use crate::app::projects::test_support::register_sibling;

#[test]
fn sibling_scratch_dirs_differ_and_migrated_keeps_legacy_dir() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let migrated = app.default_project().unwrap();
    let sibling = register_sibling(&mut app, &migrated);
    let legacy = crate::app::projects::scratch_dir(&app.state_root, &repo);
    let first = app.project_ensure_conversation(&json!({"project_id": migrated})).unwrap();
    let second = app.project_ensure_conversation(&json!({"project_id": sibling})).unwrap();
    let first_id = first["entity_id"].as_str().unwrap();
    let second_id = second["entity_id"].as_str().unwrap();
    assert_ne!(first_id, second_id, "siblings must own separate conversations");
    assert_eq!(app.runs[first_id].worktree.path, legacy);
    assert_ne!(app.runs[second_id].worktree.path, legacy);
    assert!(app.is_project_conversation_owner(first_id));
    assert!(app.is_project_conversation_owner(second_id));
    assert_eq!(app.project_conversation_run(&migrated).as_deref(), Some(first_id));
    assert_eq!(app.project_conversation_run(&sibling).as_deref(), Some(second_id));
    let saved = app.store.as_ref().unwrap().load_all_runs().unwrap();
    let saved_sibling = saved.iter().find(|record| record.id == second_id).unwrap();
    assert_eq!(saved_sibling.project_path, app.project(&sibling).unwrap().project_key);
}

#[test]
fn parked_compaction_project_resolves_retained_key() {
    let (dir, repo) = init_repo();
    let mut app = qa_state(&repo, dir.path());
    let migrated = app.default_project().unwrap();
    let sibling = register_sibling(&mut app, &migrated);
    let key = app.project(&sibling).unwrap().project_key.clone();
    app.projects.park(json!({"id":migrated,"path":repo,"project_key":repo}));
    app.projects.park(json!({"id":sibling,"path":repo,"project_key":key}));
    app.projects.retain_entity_path("parked-owner".into(), key);
    assert_eq!(app.compaction_project_of("parked-owner"), Some(sibling));
}
